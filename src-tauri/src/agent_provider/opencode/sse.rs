//! Server-Sent Events listener for the OpenCode `/event` stream.
//!
//! OpenCode emits a single global SSE channel — every session's events
//! interleave on the same connection. The listener filters by the
//! caller-supplied session id, hands matching events to
//! [`super::translate::opencode_event_to_runtime`], and publishes the
//! resulting [`ProviderRuntimeEvent`]s on a `broadcast::Sender` shared
//! with the rest of the provider.
//!
//! # Format reminder
//!
//! The SSE wire format is `data: <bytes>\n\n` per record (optional
//! `event: <name>` and `id: <id>` lines are accepted but ignored —
//! OpenCode does not use them). Multiple `data:` lines in a single
//! record are concatenated with `\n` per the WHATWG spec.
//!
//! # Reconnect strategy
//!
//! Connection-level errors trigger a bounded retry loop with linear
//! backoff (1 s, 2 s, 4 s, capped at 8 s) up to
//! [`MAX_RECONNECT_ATTEMPTS`] attempts. Past that the listener emits a
//! final `RuntimeWarning` and exits — the provider can be restarted via
//! [`super::OpenCodeServerManager::stop`] + a fresh `start_session`.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use crate::agent_provider::events::ProviderRuntimeEvent;

use super::protocol::OpenCodeEvent;
use super::translate::{opencode_event_to_runtime, EventContext};

/// Cap on consecutive reconnect attempts. Beyond this the listener
/// gives up — a stuck/missing OpenCode server is a real failure
/// that must surface to the user.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Snapshot of the routing state for a single live session. Cheap to
/// clone (Arc-wrapped strings) so the listener can hand a clone to
/// [`opencode_event_to_runtime`] for each event without locking.
///
/// The session id is the OpenCode-minted id (returned from
/// `POST /session`); `EventContext` carries the Codemux-side
/// thread/turn ids the user-facing event stream is keyed on.
#[derive(Debug, Clone)]
pub struct SsePeer {
    pub session_id: String,
    pub event_ctx: Arc<Mutex<EventContext>>,
}

/// Spawn the SSE listener task. Returns the join handle so the
/// caller can abort it (typically on session shutdown). The task ends
/// on its own when the broadcast channel has no remaining receivers
/// or the reconnect budget is exhausted.
pub fn spawn_sse_listener(
    base_url: String,
    server_password: String,
    peer: SsePeer,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        run_sse_listener(base_url, server_password, peer, event_tx).await;
    })
}

async fn run_sse_listener(
    base_url: String,
    server_password: String,
    peer: SsePeer,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
) {
    let url = format!("{}/event", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        // No request timeout — SSE is long-polled by design and
        // OpenCode's idle keep-alive may go minutes between bytes.
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            let _ = event_tx.send(warn(
                &peer,
                format!("opencode_sse_client_build_failed: {err}"),
            ));
            return;
        }
    };

    let mut attempt: u32 = 0;
    loop {
        let session_filter = peer.session_id.clone();
        let connect = client
            .get(&url)
            .basic_auth("opencode", Some(&server_password))
            .header("accept", "text/event-stream")
            .send()
            .await;

        let response = match connect {
            Ok(r) if r.status().is_success() => {
                attempt = 0;
                r
            }
            Ok(r) => {
                let _ = event_tx.send(warn(
                    &peer,
                    format!("opencode_sse_http_status_{}", r.status().as_u16()),
                ));
                if !backoff_or_quit(&mut attempt, &peer, &event_tx).await {
                    return;
                }
                continue;
            }
            Err(err) => {
                let _ = event_tx.send(warn(
                    &peer,
                    format!("opencode_sse_connect_error: {err}"),
                ));
                if !backoff_or_quit(&mut attempt, &peer, &event_tx).await {
                    return;
                }
                continue;
            }
        };

        consume_stream(response, &session_filter, &peer, &event_tx).await;

        // The server closed the stream (or it ended cleanly). Try to
        // reconnect — the OpenCode server may still be live and a new
        // turn may have been queued.
        if !backoff_or_quit(&mut attempt, &peer, &event_tx).await {
            return;
        }
    }
}

async fn consume_stream(
    response: reqwest::Response,
    session_filter: &str,
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::<u8>::new();

    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(b) => b,
            Err(err) => {
                let _ = event_tx.send(warn(
                    peer,
                    format!("opencode_sse_chunk_error: {err}"),
                ));
                return;
            }
        };
        buffer.extend_from_slice(&bytes);
        // Walk the buffer for complete events (record terminator is a
        // blank line — `\n\n`). Keep any trailing partial record in
        // the buffer for the next chunk.
        while let Some(end) = find_record_end(&buffer) {
            let record_bytes = buffer.drain(..end).collect::<Vec<u8>>();
            let record = String::from_utf8_lossy(&record_bytes).into_owned();
            handle_record(&record, session_filter, peer, event_tx).await;
        }
    }
    // Stream closed: drain any final record without a trailing blank
    // line (some upstream tooling forgets that suffix).
    if !buffer.is_empty() {
        let record = String::from_utf8_lossy(&buffer).into_owned();
        handle_record(&record, session_filter, peer, event_tx).await;
        buffer.clear();
    }
}

/// Find the byte index AFTER the first `\n\n` (or `\r\n\r\n`) record
/// terminator. Returns `None` if the buffer does not yet contain a
/// complete record.
fn find_record_end(buffer: &[u8]) -> Option<usize> {
    // Look for both `\n\n` and `\r\n\r\n`. SSE spec accepts both.
    for (i, window) in buffer.windows(4).enumerate() {
        if window == b"\r\n\r\n" {
            return Some(i + 4);
        }
    }
    for (i, window) in buffer.windows(2).enumerate() {
        if window == b"\n\n" {
            return Some(i + 2);
        }
    }
    None
}

/// Parse a single SSE record (one or more lines, terminated by a
/// blank line) into its `data:` payload. Lines beginning with `:`
/// (comments / heartbeats) and unknown field lines are ignored.
pub fn parse_sse_record(record: &str) -> Option<String> {
    let mut data_lines: Vec<&str> = Vec::new();
    for line in record.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            // Per spec the value is everything after the colon, with a
            // single optional space stripped.
            let value = rest.strip_prefix(' ').unwrap_or(rest);
            data_lines.push(value);
        }
        // Other field lines (`event:`, `id:`, `retry:`) are accepted
        // and ignored — OpenCode does not currently use them.
    }
    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

async fn handle_record(
    record: &str,
    session_filter: &str,
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) {
    let Some(data) = parse_sse_record(record) else {
        return;
    };
    let event = match serde_json::from_str::<OpenCodeEvent>(&data) {
        Ok(e) => e,
        Err(err) => {
            let _ = event_tx.send(warn(
                peer,
                format!("opencode_sse_decode_error: {err}"),
            ));
            return;
        }
    };

    if !event_matches_session(&event, session_filter) {
        return;
    }

    let ctx = peer.event_ctx.lock().await.clone();
    for runtime in opencode_event_to_runtime(event, &ctx) {
        let _ = event_tx.send(runtime);
    }
}

/// Filter — only events whose `sessionID` (when present) matches the
/// active session reach the translator. Events without a session id
/// (e.g. `vcs.branch.updated`) fall through to the translator's
/// catch-all.
fn event_matches_session(event: &OpenCodeEvent, session_id: &str) -> bool {
    use super::protocol::KnownEvent as K;
    let known = match event {
        OpenCodeEvent::Known(k) => k,
        OpenCodeEvent::Other(_) => return false,
    };
    match known {
        K::SessionCreated(env) | K::SessionUpdated(env) | K::SessionDeleted(env) => {
            env.info.id == session_id
        }
        K::SessionIdle(p) => p.session_id == session_id,
        K::SessionStatus(p) => p.session_id == session_id,
        K::SessionError(p) => p.session_id.as_deref().is_some_and(|s| s == session_id)
            || p.session_id.is_none(),
        K::MessageUpdated(env) => env.info.session_id == session_id,
        K::MessageRemoved(p) => p.session_id == session_id,
        K::MessagePartUpdated(p) => match &p.part {
            super::protocol::PartPayload::Text(t) => t.session_id == session_id,
            super::protocol::PartPayload::Reasoning(r) => r.session_id == session_id,
            super::protocol::PartPayload::Tool(t) => t.session_id == session_id,
            super::protocol::PartPayload::Other => false,
        },
        K::MessagePartDelta(p) => p.session_id == session_id,
        K::MessagePartRemoved(p) => p.session_id == session_id,
        K::PermissionAsked(p) => p.session_id == session_id,
        K::PermissionReplied(p) => p.session_id == session_id,
    }
}

async fn backoff_or_quit(
    attempt: &mut u32,
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) -> bool {
    *attempt += 1;
    if *attempt > MAX_RECONNECT_ATTEMPTS {
        let _ = event_tx.send(warn(
            peer,
            format!("opencode_sse_giving_up_after_{MAX_RECONNECT_ATTEMPTS}_attempts"),
        ));
        return false;
    }
    let backoff_secs = std::cmp::min(8u64, 1u64 << (*attempt - 1));
    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
    true
}

fn warn(peer: &SsePeer, message: String) -> ProviderRuntimeEvent {
    // We don't peek the lock here because we're already inside an
    // async path that may have lost it; pass `None` for the thread id
    // and rely on the message body for correlation.
    let _ = peer;
    ProviderRuntimeEvent::RuntimeWarning {
        thread_id: None,
        message,
        original_payload: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::protocol::KnownEvent;
    use crate::agent_provider::types::{ProviderSessionId, ThreadId, TurnId};

    fn peer(session_id: &str) -> SsePeer {
        SsePeer {
            session_id: session_id.to_string(),
            event_ctx: Arc::new(Mutex::new(EventContext {
                thread_id: ThreadId("t1".into()),
                turn_id: TurnId("turn1".into()),
                provider_session_id: ProviderSessionId(session_id.to_string()),
            })),
        }
    }

    #[test]
    fn parse_sse_record_extracts_single_data_line() {
        let record = "data: {\"hello\":\"world\"}\n\n";
        assert_eq!(
            parse_sse_record(record),
            Some("{\"hello\":\"world\"}".to_string())
        );
    }

    #[test]
    fn parse_sse_record_concatenates_multiple_data_lines() {
        let record = "data: line1\ndata: line2\n\n";
        assert_eq!(parse_sse_record(record), Some("line1\nline2".to_string()));
    }

    #[test]
    fn parse_sse_record_ignores_event_and_id_lines() {
        let record = "event: ping\nid: 1\ndata: {\"a\":1}\n\n";
        assert_eq!(parse_sse_record(record), Some("{\"a\":1}".to_string()));
    }

    #[test]
    fn parse_sse_record_ignores_comments() {
        let record = ": heartbeat\ndata: {\"a\":1}\n\n";
        assert_eq!(parse_sse_record(record), Some("{\"a\":1}".to_string()));
    }

    #[test]
    fn parse_sse_record_returns_none_when_no_data() {
        assert_eq!(parse_sse_record(": only-comment\n\n"), None);
    }

    #[test]
    fn parse_sse_record_strips_leading_space_per_spec() {
        let record = "data:nospace\n\n";
        assert_eq!(parse_sse_record(record), Some("nospace".to_string()));
        let record = "data: with-space\n\n";
        assert_eq!(parse_sse_record(record), Some("with-space".to_string()));
    }

    #[test]
    fn find_record_end_returns_none_for_incomplete_buffer() {
        assert_eq!(find_record_end(b"data: foo\n"), None);
    }

    #[test]
    fn find_record_end_locates_lf_lf_terminator() {
        let buf = b"data: foo\n\nrest";
        let end = find_record_end(buf).unwrap();
        assert_eq!(&buf[..end], b"data: foo\n\n");
    }

    #[test]
    fn find_record_end_locates_crlf_terminator() {
        let buf = b"data: foo\r\n\r\nrest";
        let end = find_record_end(buf).unwrap();
        assert_eq!(&buf[..end], b"data: foo\r\n\r\n");
    }

    #[test]
    fn event_matches_session_filters_by_session_id() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartDelta(super::super::protocol::MessagePartDelta {
            session_id: "s1".into(),
            message_id: "m1".into(),
            part_id: "p1".into(),
            field: "text".into(),
            delta: "x".into(),
        }));
        assert!(event_matches_session(&event, "s1"));
        assert!(!event_matches_session(&event, "s2"));
    }

    #[tokio::test]
    async fn handle_record_publishes_translated_event_for_matching_session() {
        let (tx, mut rx) = broadcast::channel(8);
        let peer = peer("s1");
        let record = "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"s1\",\"messageID\":\"m1\",\"partID\":\"p1\",\"field\":\"text\",\"delta\":\"hi\"}}\n\n";
        handle_record(record, "s1", &peer, &tx).await;
        let event = rx.try_recv().expect("event published");
        match event {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => {
                use crate::agent_provider::events::ContentDelta;
                match delta {
                    ContentDelta::Text { text } => assert_eq!(text, "hi"),
                    other => panic!("wrong delta: {other:?}"),
                }
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_record_filters_other_session() {
        let (tx, mut rx) = broadcast::channel(8);
        let peer = peer("s1");
        let record = "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"OTHER\",\"messageID\":\"m1\",\"partID\":\"p1\",\"field\":\"text\",\"delta\":\"hi\"}}\n\n";
        handle_record(record, "s1", &peer, &tx).await;
        // Filtered: nothing is published.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn handle_record_emits_warning_on_decode_error() {
        let (tx, mut rx) = broadcast::channel(8);
        let peer = peer("s1");
        let record = "data: { not json }\n\n";
        handle_record(record, "s1", &peer, &tx).await;
        let event = rx.try_recv().expect("warning published");
        match event {
            ProviderRuntimeEvent::RuntimeWarning { message, .. } => {
                assert!(message.starts_with("opencode_sse_decode_error"));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }
}
