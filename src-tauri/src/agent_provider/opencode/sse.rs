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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use crate::agent_provider::events::ProviderRuntimeEvent;

use super::protocol::{KnownEvent, OpenCodeEvent, PartPayload, SessionInfo};
use super::translate::{opencode_event_to_runtime, EventContext};

/// Cap on consecutive reconnect attempts. Beyond this the listener
/// gives up — a stuck/missing OpenCode server is a real failure
/// that must surface to the user.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Mutable subagent-routing state for one session's SSE stream.
///
/// OpenCode multiplexes **every** session onto one global `/event`
/// channel, so the listener has to decide, per event, whether it
/// belongs to the session Codemux owns (the root/parent), one of its
/// subagents' child sessions, or an unrelated session that must be
/// ignored. That decision is data-driven:
///
/// * `watched` starts as `{ root }` and grows every time a
///   `session.created` arrives whose `parentID` is already watched
///   (parent ∪ descendants, so nested subagents are picked up too).
/// * `subagent_id_for` maps a watched session id to the demux key the
///   frontend routes on: `None` for the root, `Some(child_id)` for a
///   subagent.
/// * `permission_sessions` remembers which session raised each
///   permission request, so a reply can target the **child** session
///   (`POST /session/{childID}/permissions/{permID}`) rather than the
///   root — otherwise a subagent's approval never resolves and the turn
///   stalls.
///
/// Shared (`Arc<Mutex<…>>`) between the SSE listener and the
/// [`super::session::OpenCodeSession`] so the approval reply path can
/// read `permission_sessions`.
#[derive(Debug, Default)]
pub struct SseRouter {
    root_session_id: String,
    watched: HashSet<String>,
    permission_sessions: HashMap<String, String>,
}

impl SseRouter {
    /// Seed the router with the root session (the one Codemux created);
    /// it is watched from the start.
    pub fn new(root_session_id: impl Into<String>) -> Self {
        let root = root_session_id.into();
        let mut watched = HashSet::new();
        watched.insert(root.clone());
        Self {
            root_session_id: root,
            watched,
            permission_sessions: HashMap::new(),
        }
    }

    /// Grow the watched set when a newly-created session's parent is
    /// already watched. Returns `true` when a new session was added.
    pub fn observe_session_created(&mut self, info: &SessionInfo) -> bool {
        match &info.parent_id {
            Some(parent) if self.watched.contains(parent) => self.watched.insert(info.id.clone()),
            _ => false,
        }
    }

    /// Whether events for `session_id` should reach the translator.
    pub fn is_watched(&self, session_id: &str) -> bool {
        self.watched.contains(session_id)
    }

    /// Demux key for an event on `session_id`: `None` for the root,
    /// `Some(session_id)` for a subagent's child session.
    pub fn subagent_id_for(&self, session_id: &str) -> Option<String> {
        if session_id == self.root_session_id {
            None
        } else {
            Some(session_id.to_string())
        }
    }

    /// Remember which session a permission request belongs to.
    pub fn record_permission(&mut self, permission_id: String, session_id: String) {
        self.permission_sessions.insert(permission_id, session_id);
    }

    /// The session a permission reply must target, if known.
    pub fn session_for_permission(&self, permission_id: &str) -> Option<String> {
        self.permission_sessions.get(permission_id).cloned()
    }
}

/// Snapshot of the routing state for a single live session. Cheap to
/// clone (Arc-wrapped) so the listener can hand a clone to
/// [`opencode_event_to_runtime`] for each event without locking.
///
/// The session id is the OpenCode-minted root id (returned from
/// `POST /session`); `event_ctx` carries the Codemux-side thread/turn
/// ids the user-facing event stream is keyed on; `router` carries the
/// mutable watched-session + permission-routing state.
#[derive(Debug, Clone)]
pub struct SsePeer {
    pub session_id: String,
    pub event_ctx: Arc<Mutex<EventContext>>,
    pub router: Arc<Mutex<SseRouter>>,
    /// Shared with [`super::session::OpenCodeSession`]: set true when this
    /// listener exhausts its reconnect budget so the provider treats the
    /// session as dead and the next send auto-resumes.
    pub dead: Arc<std::sync::atomic::AtomicBool>,
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

        consume_stream(response, &peer, &event_tx).await;

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
            handle_record(&record, peer, event_tx).await;
        }
    }
    // Stream closed: drain any final record without a trailing blank
    // line (some upstream tooling forgets that suffix).
    if !buffer.is_empty() {
        let record = String::from_utf8_lossy(&buffer).into_owned();
        handle_record(&record, peer, event_tx).await;
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

    // Resolve routing against the watched-session set, growing it for
    // newly-spawned subagents, before deciding whether the event is
    // ours and how to tag it.
    let Some(subagent_id) = resolve_routing(&event, &peer.router).await else {
        return;
    };

    let ctx = peer.event_ctx.lock().await.clone();
    let mut turn_settled = false;
    for runtime in opencode_event_to_runtime(event, &ctx, subagent_id.as_deref()) {
        // A parent-scoped turn completion or terminal session state means
        // the in-flight turn is over — disarm the give-up path so a later
        // server death while idle does NOT synthesize a spurious
        // `child_exited` turn. Subagents never produce these, so any such
        // event here is the root session's.
        if matches!(
            runtime,
            ProviderRuntimeEvent::TurnCompleted { .. }
                | ProviderRuntimeEvent::SessionStateChanged {
                    status: crate::agent_provider::SessionStatus::Ready
                        | crate::agent_provider::SessionStatus::Closed
                        | crate::agent_provider::SessionStatus::Error { .. },
                    ..
                }
        ) {
            turn_settled = true;
        }
        let _ = event_tx.send(runtime);
    }
    if turn_settled {
        peer.event_ctx.lock().await.turn_active = false;
    }
}

/// Decide whether an event belongs to a watched session and, if so,
/// return its subagent tag (`None` for the root, `Some(child)` for a
/// subagent). Returns `None` (the outer `Option`) to signal "drop this
/// event". Side effects: grows the watched set on `session.created` and
/// records permission→session routing.
async fn resolve_routing(
    event: &OpenCodeEvent,
    router: &Arc<Mutex<SseRouter>>,
) -> Option<Option<String>> {
    let mut router = router.lock().await;

    // Grow the watched set first so a child's own `session.created`
    // (which carries the parent id) is enough to start tracking it.
    if let OpenCodeEvent::Known(KnownEvent::SessionCreated(env)) = event {
        router.observe_session_created(&env.info);
    }

    // Unknown events are dropped at the SSE layer (as before) — the
    // translator's `Other` warning arm is reserved for direct callers.
    let known = match event {
        OpenCodeEvent::Known(k) => k,
        OpenCodeEvent::Other(_) => return None,
    };

    // Record which session a permission belongs to so the reply can
    // target the child session id later.
    if let KnownEvent::PermissionAsked(p) = known {
        router.record_permission(p.id.clone(), p.session_id.clone());
    }

    match event_session_id(known) {
        Some(session_id) => {
            if !router.is_watched(session_id) {
                return None;
            }
            Some(router.subagent_id_for(session_id))
        }
        // A known, session-less event (e.g. `session.error` with no
        // sessionID) is treated as root-scoped.
        None => Some(None),
    }
}

/// The session id an event belongs to, or `None` for a known event
/// carrying no session id.
fn event_session_id(known: &KnownEvent) -> Option<&str> {
    use KnownEvent as K;
    match known {
        K::SessionCreated(env) | K::SessionUpdated(env) | K::SessionDeleted(env) => {
            Some(env.info.id.as_str())
        }
        K::SessionIdle(p) => Some(p.session_id.as_str()),
        K::SessionStatus(p) => Some(p.session_id.as_str()),
        K::SessionError(p) => p.session_id.as_deref(),
        K::MessageUpdated(env) => Some(env.info.session_id.as_str()),
        K::MessageRemoved(p) => Some(p.session_id.as_str()),
        K::MessagePartUpdated(p) => match &p.part {
            PartPayload::Text(t) => Some(t.session_id.as_str()),
            PartPayload::Reasoning(r) => Some(r.session_id.as_str()),
            PartPayload::Tool(t) => Some(t.session_id.as_str()),
            PartPayload::Agent(a) => Some(a.session_id.as_str()),
            PartPayload::Subtask(s) => Some(s.session_id.as_str()),
            PartPayload::Other => None,
        },
        K::MessagePartDelta(p) => Some(p.session_id.as_str()),
        K::MessagePartRemoved(p) => Some(p.session_id.as_str()),
        K::PermissionAsked(p) => Some(p.session_id.as_str()),
        K::PermissionReplied(p) => Some(p.session_id.as_str()),
        K::TodoUpdated(p) => Some(p.session_id.as_str()),
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
        emit_give_up_terminal(peer, event_tx).await;
        return false;
    }
    let backoff_secs = std::cmp::min(8u64, 1u64 << (*attempt - 1));
    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
    true
}

/// Settle a session whose server became unreachable (reconnect budget
/// exhausted). Unlike a plain `RuntimeWarning`, this drives the session
/// through the standard terminal pipeline so the sidebar clears, the
/// background browser releases, and — if a turn was in flight — the
/// frontend surfaces an interrupted turn with a Continue affordance.
///
/// Marks the session dead so the provider's `has_session` reports it
/// absent and the next send rebuilds a fresh session via
/// `ensure_live_session` instead of POSTing to a corpse.
async fn emit_give_up_terminal(
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) {
    use std::sync::atomic::Ordering;
    peer.dead.store(true, Ordering::Relaxed);
    let (thread_id, active_turn) = {
        let ctx = peer.event_ctx.lock().await;
        let active_turn = if ctx.turn_active {
            Some(ctx.turn_id.clone())
        } else {
            None
        };
        (ctx.thread_id.clone(), active_turn)
    };
    for event in crate::agent_provider::child_exit_events(
        thread_id,
        active_turn,
        "opencode server unreachable".to_string(),
    ) {
        let _ = event_tx.send(event);
    }
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
                turn_active: false,
            })),
            router: Arc::new(Mutex::new(SseRouter::new(session_id))),
            dead: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    #[tokio::test]
    async fn give_up_with_active_turn_settles_then_errors_and_marks_dead() {
        use std::sync::atomic::Ordering;
        let p = peer("sess-1");
        p.event_ctx.lock().await.turn_active = true;
        let (tx, mut rx) = broadcast::channel(8);
        emit_give_up_terminal(&p, &tx).await;
        // Dead flag set so the provider auto-resumes on the next send.
        assert!(p.dead.load(Ordering::Relaxed));
        // TurnCompleted(child_exited) first…
        match rx.try_recv().unwrap() {
            ProviderRuntimeEvent::TurnCompleted { status, .. } => match status {
                crate::agent_provider::TurnStatus::Error { subtype, .. } => {
                    assert_eq!(subtype, "child_exited");
                }
                other => panic!("expected Error status, got {other:?}"),
            },
            other => panic!("expected TurnCompleted first, got {other:?}"),
        }
        // …then the terminal Error.
        assert!(matches!(
            rx.try_recv().unwrap(),
            ProviderRuntimeEvent::SessionStateChanged {
                status: crate::agent_provider::SessionStatus::Error { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn give_up_without_active_turn_only_errors() {
        // Server died while idle — no dangling turn to settle, so no
        // spurious `child_exited` TurnCompleted (which would falsely mark
        // the frontend thread interrupted).
        let p = peer("sess-2");
        let (tx, mut rx) = broadcast::channel(8);
        emit_give_up_terminal(&p, &tx).await;
        assert!(matches!(
            rx.try_recv().unwrap(),
            ProviderRuntimeEvent::SessionStateChanged {
                status: crate::agent_provider::SessionStatus::Error { .. },
                ..
            }
        ));
        assert!(rx.try_recv().is_err(), "no second event expected");
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

    fn delta_event(session_id: &str) -> OpenCodeEvent {
        OpenCodeEvent::Known(KnownEvent::MessagePartDelta(super::super::protocol::MessagePartDelta {
            session_id: session_id.into(),
            message_id: "m1".into(),
            part_id: "p1".into(),
            field: "text".into(),
            delta: "x".into(),
        }))
    }

    fn child_created_event(child: &str, parent: &str) -> OpenCodeEvent {
        serde_json::from_value(serde_json::json!({
            "type": "session.created",
            "properties": { "info": { "id": child, "parentID": parent, "title": "child" } }
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn resolve_routing_passes_root_untagged_drops_unrelated() {
        let router = Arc::new(Mutex::new(SseRouter::new("s1")));
        // Root event → watched, untagged.
        assert_eq!(
            resolve_routing(&delta_event("s1"), &router).await,
            Some(None)
        );
        // Unrelated session → dropped.
        assert_eq!(resolve_routing(&delta_event("OTHER"), &router).await, None);
    }

    #[tokio::test]
    async fn resolve_routing_grows_watched_set_on_child_created_then_tags_child() {
        let router = Arc::new(Mutex::new(SseRouter::new("s1")));
        // Before the child is announced its events are dropped.
        assert_eq!(resolve_routing(&delta_event("child"), &router).await, None);
        // session.created with a watched parent adds the child; the
        // creation event itself is a child-session event so it is
        // already tagged with the child id.
        assert_eq!(
            resolve_routing(&child_created_event("child", "s1"), &router).await,
            Some(Some("child".to_string()))
        );
        // Now child events pass, tagged with the child session id.
        assert_eq!(
            resolve_routing(&delta_event("child"), &router).await,
            Some(Some("child".to_string()))
        );
    }

    #[tokio::test]
    async fn resolve_routing_ignores_child_whose_parent_is_unwatched() {
        let router = Arc::new(Mutex::new(SseRouter::new("s1")));
        // A session.created whose parent we don't track must NOT be added.
        resolve_routing(&child_created_event("stranger", "someone_else"), &router).await;
        assert_eq!(resolve_routing(&delta_event("stranger"), &router).await, None);
    }

    #[tokio::test]
    async fn resolve_routing_tracks_grandchildren_via_transitive_parents() {
        let router = Arc::new(Mutex::new(SseRouter::new("s1")));
        resolve_routing(&child_created_event("child", "s1"), &router).await;
        // Grandchild's parent is `child`, which is now watched.
        resolve_routing(&child_created_event("grand", "child"), &router).await;
        assert_eq!(
            resolve_routing(&delta_event("grand"), &router).await,
            Some(Some("grand".to_string()))
        );
    }

    #[tokio::test]
    async fn resolve_routing_records_child_permission_session() {
        let router = Arc::new(Mutex::new(SseRouter::new("s1")));
        resolve_routing(&child_created_event("child", "s1"), &router).await;
        let perm = serde_json::from_value::<OpenCodeEvent>(serde_json::json!({
            "type": "permission.asked",
            "properties": {
                "id": "per_1", "sessionID": "child", "permission": "external_directory",
                "patterns": ["/*"], "metadata": {}, "tool": { "messageID": "m", "callID": "c" }
            }
        }))
        .unwrap();
        assert_eq!(
            resolve_routing(&perm, &router).await,
            Some(Some("child".to_string()))
        );
        // The reply path can now recover the child session id.
        assert_eq!(
            router.lock().await.session_for_permission("per_1").as_deref(),
            Some("child")
        );
    }

    #[tokio::test]
    async fn handle_record_publishes_translated_event_for_matching_session() {
        let (tx, mut rx) = broadcast::channel(8);
        let peer = peer("s1");
        let record = "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"s1\",\"messageID\":\"m1\",\"partID\":\"p1\",\"field\":\"text\",\"delta\":\"hi\"}}\n\n";
        handle_record(record, &peer, &tx).await;
        let event = rx.try_recv().expect("event published");
        match event {
            ProviderRuntimeEvent::ContentDelta { delta, subagent_id, .. } => {
                use crate::agent_provider::events::ContentDelta;
                assert!(subagent_id.is_none(), "root delta carries no subagent id");
                match delta {
                    ContentDelta::Text { text } => assert_eq!(text, "hi"),
                    other => panic!("wrong delta: {other:?}"),
                }
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_record_routes_child_delta_with_subagent_id() {
        let (tx, mut rx) = broadcast::channel(8);
        let peer = peer("s1");
        // Announce the child so it becomes watched.
        let created = "data: {\"type\":\"session.created\",\"properties\":{\"info\":{\"id\":\"child\",\"parentID\":\"s1\",\"title\":\"c\"}}}\n\n";
        handle_record(created, &peer, &tx).await;
        let record = "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"child\",\"messageID\":\"m1\",\"partID\":\"p1\",\"field\":\"text\",\"delta\":\"hi\"}}\n\n";
        handle_record(record, &peer, &tx).await;
        let event = rx.try_recv().expect("child event published");
        match event {
            ProviderRuntimeEvent::ContentDelta { subagent_id, .. } => {
                assert_eq!(subagent_id.as_deref(), Some("child"));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_record_filters_other_session() {
        let (tx, mut rx) = broadcast::channel(8);
        let peer = peer("s1");
        let record = "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"OTHER\",\"messageID\":\"m1\",\"partID\":\"p1\",\"field\":\"text\",\"delta\":\"hi\"}}\n\n";
        handle_record(record, &peer, &tx).await;
        // Filtered: nothing is published.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn handle_record_emits_warning_on_decode_error() {
        let (tx, mut rx) = broadcast::channel(8);
        let peer = peer("s1");
        let record = "data: { not json }\n\n";
        handle_record(record, &peer, &tx).await;
        let event = rx.try_recv().expect("warning published");
        match event {
            ProviderRuntimeEvent::RuntimeWarning { message, .. } => {
                assert!(message.starts_with("opencode_sse_decode_error"));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }
}
