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
use tokio::sync::{broadcast, watch, Mutex};
use tokio::task::JoinHandle;

use crate::agent_provider::events::ProviderRuntimeEvent;

use super::protocol::{KnownEvent, OpenCodeEvent, PartPayload, SessionInfo};
use super::translate::{opencode_event_to_runtime_with, EventContext, OpenCodeUsageState};

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
    interrupted_turn: Option<crate::agent_provider::TurnId>,
    legacy_message_turns: HashMap<String, crate::agent_provider::TurnId>,
    legacy_terminal: Option<(
        crate::agent_provider::TurnId,
        crate::agent_provider::TurnStatus,
    )>,
    forms: HashMap<String, serde_json::Value>,
    pub native: super::v2::State,
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
            interrupted_turn: None,
            legacy_message_turns: HashMap::new(),
            legacy_terminal: None,
            forms: HashMap::new(),
            native: super::v2::State::default(),
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

    pub fn record_form(&mut self, form: serde_json::Value) {
        if let (Some(id), Some(session)) = (form["id"].as_str(), form["sessionID"].as_str()) {
            self.permission_sessions.insert(id.into(), session.into());
            self.forms.insert(id.into(), form);
        }
    }
    pub fn form(&self, id: &str) -> Option<serde_json::Value> {
        self.forms.get(id).cloned()
    }
    pub fn forget_request(&mut self, id: &str) {
        self.permission_sessions.remove(id);
        self.forms.remove(id);
    }
    pub fn mark_interrupted(&mut self, turn: Option<crate::agent_provider::TurnId>) {
        self.interrupted_turn = turn;
    }
    fn take_interrupted(&mut self, turn: &crate::agent_provider::TurnId) -> bool {
        if self.interrupted_turn.as_ref() == Some(turn) {
            self.interrupted_turn = None;
            true
        } else {
            false
        }
    }
    pub fn watched_sessions(&self) -> Vec<String> {
        self.watched.iter().cloned().collect()
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
    pub api: super::api::OpenCodeApi,
    pub ready: watch::Sender<bool>,
    pub directory: Option<std::path::PathBuf>,
    pub session_id: String,
    pub event_ctx: Arc<Mutex<EventContext>>,
    pub router: Arc<Mutex<SseRouter>>,
    /// Shared with [`super::session::OpenCodeSession`]: set true when this
    /// listener exhausts its reconnect budget so the provider treats the
    /// session as dead and the next send auto-resumes.
    pub dead: Arc<std::sync::atomic::AtomicBool>,
    /// Per-session context-window accounting, shared with the session
    /// so a rebuilt listener keeps the thread's lifetime token total
    /// instead of restarting it from zero.
    pub usage: Arc<Mutex<OpenCodeUsageState>>,
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
    let mut url = format!(
        "{}{}",
        base_url.trim_end_matches('/'),
        peer.api.route("/event", "/api/event")
    );
    if let Some(directory) = peer.directory.as_ref() {
        url.push_str(&format!(
            "?{}={}",
            peer.api.route("directory", "location%5Bdirectory%5D"),
            urlencoding::encode(&directory.to_string_lossy())
        ));
    }
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
                if peer.api == super::api::OpenCodeApi::Legacy {
                    attempt = 0;
                }
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
                let _ = event_tx.send(warn(&peer, format!("opencode_sse_connect_error: {err}")));
                if !backoff_or_quit(&mut attempt, &peer, &event_tx).await {
                    return;
                }
                continue;
            }
        };

        if peer.api == super::api::OpenCodeApi::Legacy {
            peer.ready.send_replace(true);
        }
        consume_stream(
            response,
            &peer,
            &event_tx,
            &base_url,
            &server_password,
            &mut attempt,
        )
        .await;
        if peer.api == super::api::OpenCodeApi::V2 {
            peer.ready.send_replace(false);
        }

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
    base_url: &str,
    server_password: &str,
    attempt: &mut u32,
) {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::<u8>::new();

    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(b) => b,
            Err(err) => {
                let _ = event_tx.send(warn(peer, format!("opencode_sse_chunk_error: {err}")));
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
            if peer.api == super::api::OpenCodeApi::V2
                && parse_sse_record(&record)
                    .and_then(|s| serde_json::from_str::<super::v2::Event>(&s).ok())
                    .is_some_and(|e| e.kind == "server.connected")
            {
                if let Err(message) =
                    reconcile_native(base_url, server_password, peer, event_tx).await
                {
                    let _ = event_tx.send(warn(
                        peer,
                        format!("opencode_reconnect_snapshot_failed: {message}"),
                    ));
                    return;
                }
                // HTTP 200 alone is not a healthy native attachment. Only a
                // completed recovery/readiness handshake resets its budget.
                *attempt = 0;
            }
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
            let _ = event_tx.send(warn(peer, format!("opencode_sse_decode_error: {err}")));
            return;
        }
    };

    if let OpenCodeEvent::V2(event) = event {
        handle_native(event, peer, event_tx).await;
        return;
    }

    // Resolve routing against the watched-session set, growing it for
    // newly-spawned subagents, before deciding whether the event is
    // ours and how to tag it.
    let Some(subagent_id) = resolve_routing(&event, &peer.router).await else {
        return;
    };

    // v1 emits status(idle) followed by session.idle. Settle only on the
    // latter, or the duplicate can complete the next queued turn.
    if subagent_id.is_none()
        && matches!(&event, OpenCodeEvent::Known(KnownEvent::SessionStatus(status)) if matches!(status.status, super::protocol::SessionStatusValue::Idle))
    {
        return;
    }
    let ctx = peer.event_ctx.lock().await.clone();
    let mut usage = peer.usage.lock().await;
    let root_idle =
        subagent_id.is_none() && matches!(&event, OpenCodeEvent::Known(KnownEvent::SessionIdle(_)));
    if root_idle && !ctx.turn_active {
        return;
    }
    let stale_message = if subagent_id.is_none() {
        let message = match &event {
            OpenCodeEvent::Known(KnownEvent::MessageUpdated(message))
                if message.info.role == "assistant" =>
            {
                Some(&message.info.id)
            }
            OpenCodeEvent::Known(KnownEvent::MessagePartDelta(part)) => Some(&part.message_id),
            _ => None,
        };
        if let Some(message) = message {
            let mut router = peer.router.lock().await;
            if ctx.turn_active {
                router
                    .legacy_message_turns
                    .entry(message.clone())
                    .or_insert_with(|| ctx.turn_id.clone());
            }
            router
                .legacy_message_turns
                .get(message)
                .is_some_and(|turn| turn != &ctx.turn_id)
        } else {
            false
        }
    } else {
        false
    };
    let mut events =
        opencode_event_to_runtime_with(event, &ctx, subagent_id.as_deref(), &mut usage);
    if subagent_id.is_none() && !root_idle {
        // v1 sends session.error, then idle, then often repeats the same
        // error on message.updated. Idle is the execution boundary; retain
        // the failure until that boundary instead of emitting three terminals.
        for event in &events {
            if let ProviderRuntimeEvent::TurnCompleted { status, .. } = event {
                if ctx.turn_active && !stale_message {
                    peer.router.lock().await.legacy_terminal =
                        Some((ctx.turn_id.clone(), status.clone()));
                }
            }
        }
        events.retain(|e| !matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }));
    }
    if root_idle {
        let pending = peer.router.lock().await.legacy_terminal.take();
        if let Some((turn, status)) = pending {
            if turn == ctx.turn_id {
                for event in &mut events {
                    if let ProviderRuntimeEvent::TurnCompleted {
                        status: current, ..
                    } = event
                    {
                        *current = status.clone();
                    }
                }
            }
        }
        if peer.router.lock().await.take_interrupted(&ctx.turn_id) {
            for event in &mut events {
                if let ProviderRuntimeEvent::TurnCompleted { status, .. } = event {
                    *status = crate::agent_provider::TurnStatus::Error {
                        subtype: "interrupted".into(),
                        message: "OpenCode execution interrupted".into(),
                    };
                }
            }
        }
        events.push(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ctx.thread_id.clone(),
            status: crate::agent_provider::SessionStatus::Ready,
        });
    }
    drop(usage);
    let mut turn_settled = false;
    let mut turn_running = false;
    for runtime in &events {
        if matches!(
            runtime,
            ProviderRuntimeEvent::SessionStateChanged {
                status: crate::agent_provider::SessionStatus::Running { .. },
                ..
            }
        ) {
            turn_running = true;
        }
        // A parent-scoped turn completion or terminal session state means
        // the in-flight turn is over — disarm the give-up path so a later
        // server death while idle does NOT synthesize a spurious
        // `child_exited` turn. Subagents never produce these, so any such
        // event here is the root session's.
        if matches!(
            runtime,
            ProviderRuntimeEvent::SessionStateChanged {
                status: crate::agent_provider::SessionStatus::Ready
                    | crate::agent_provider::SessionStatus::Closed
                    | crate::agent_provider::SessionStatus::Error { .. },
                ..
            }
        ) {
            turn_settled = true;
        }
    }
    // Commit the state before the queue worker observes completion. Hold
    // the lock across the batch so Ready cannot trail a new Running event.
    let mut live = peer.event_ctx.lock().await;
    if live.turn_id == ctx.turn_id {
        if turn_settled {
            live.turn_active = false;
        } else if turn_running {
            live.turn_active = true;
        }
    }
    for runtime in events {
        let _ = event_tx.send(runtime);
    }
}

fn admit_native_root(ctx: &mut EventContext) {
    // A background execution restored while idle needs its own identity;
    // a user turn already admitted locally keeps the identity it was given.
    if !ctx.turn_active {
        ctx.turn_id = crate::agent_provider::TurnId(format!("turn_{}", uuid::Uuid::new_v4()));
        ctx.turn_active = true;
    }
}

pub(super) async fn handle_native(
    event: super::v2::Event,
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) {
    if event.kind == "server.connected" {
        peer.ready.send_replace(true);
        return;
    }
    let Some(session) = event.session_id().map(str::to_string) else {
        return;
    };
    // Root lifecycle reduction and turn admission share this lock, so a
    // queue drain cannot enter between an accepted start and its Running state.
    let mut live = peer.event_ctx.lock().await;
    let mut router = peer.router.lock().await;
    if event.kind == "session.created" {
        if let Some(parent) = event.data["parentID"].as_str() {
            if router.is_watched(parent) {
                router.watched.insert(session.clone());
            }
        }
    }
    if !router.is_watched(&session) {
        return;
    }
    let subagent = router.subagent_id_for(&session);
    if event.kind == "permission.asked" {
        if let Some(id) = event.data["id"].as_str() {
            if router.session_for_permission(id).is_some() {
                return;
            }
            router.record_permission(id.into(), session.clone());
        }
    }
    if event.kind == "form.created" {
        if let Some(id) = event.data["form"]["id"].as_str() {
            if router.session_for_permission(id).is_some() {
                return;
            }
        }
        router.record_form(event.data["form"].clone());
    }
    // Retain routing until a reply has actually succeeded, not merely clicked.
    let updates = match router.native.reduce(&event) {
        Ok(updates) => updates,
        Err(message) => {
            drop(router);
            let _ = event_tx.send(warn(peer, message));
            return;
        }
    };
    let parked_user = router.native.has_pending_user_admission(&session);
    drop(router);
    let mut usage = peer.usage.lock().await;
    let mut events = Vec::new();
    for update in updates {
        if subagent.is_none() {
            if let super::v2::Update::UserDelivered(turn) = &update {
                let turn = crate::agent_provider::TurnId(turn.clone());
                if live.turn_active && live.turn_id != turn {
                    // The native busy period delivered the queued user after
                    // background output. Handoff once without waking the queue.
                    events.push(ProviderRuntimeEvent::TurnCompleted {
                        thread_id: live.thread_id.clone(),
                        turn_id: live.turn_id.clone(),
                        status: crate::agent_provider::TurnStatus::Success,
                        usage: None,
                    });
                }
                live.turn_id = turn;
                live.turn_active = true;
                events.push(ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: live.thread_id.clone(),
                    status: crate::agent_provider::SessionStatus::Running {
                        active_turn: live.turn_id.clone(),
                    },
                });
                continue;
            }
            if parked_user && !live.turn_active {
                if matches!(&update, super::v2::Update::Started) {
                    // Started precedes inbox drain and cannot identify the
                    // parked user. Its delivery event owns that local turn.
                    continue;
                }
                if matches!(
                    &update,
                    super::v2::Update::Content { .. }
                        | super::v2::Update::ToolUse { .. }
                        | super::v2::Update::ToolResult { .. }
                        | super::v2::Update::Permission(_)
                        | super::v2::Update::Form(_)
                ) {
                    admit_native_root(&mut live);
                    events.push(ProviderRuntimeEvent::SessionStateChanged {
                        thread_id: live.thread_id.clone(),
                        status: crate::agent_provider::SessionStatus::Running {
                            active_turn: live.turn_id.clone(),
                        },
                    });
                }
            }
        }
        // A background child's callback can admit a new root execution after
        // the user turn settled. Only a live start accepted by the reducer
        // may reactivate it; recovery and replay filtering happen above.
        if subagent.is_none() && matches!(&update, super::v2::Update::Started) {
            admit_native_root(&mut live);
        }
        // Root content that precedes attachment is already in durable history.
        // Child callbacks and lifecycle remain live after the parent settles.
        if !live.turn_active
            && subagent.is_none()
            && matches!(
                &update,
                super::v2::Update::Content { .. }
                    | super::v2::Update::ToolUse { .. }
                    | super::v2::Update::ToolResult { .. }
            )
        {
            continue;
        }
        events.extend(super::translate::v2_update_to_runtime(
            update,
            &live,
            subagent.as_deref(),
            &mut usage,
        ));
    }
    if events
        .iter()
        .any(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &live.turn_id))
    {
        live.turn_active = false;
    }
    for event in events {
        let _ = event_tx.send(event);
    }
}

async fn native_get(
    client: &reqwest::Client,
    base: &str,
    password: &str,
    path: &str,
) -> Result<serde_json::Value, String> {
    client
        .get(format!("{}{path}", base.trim_end_matches('/')))
        .basic_auth("opencode", Some(password))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

/// The CLI does not retain log payloads by default, but always emits a synced
/// per-session sequence. Bracket projection reads with these exact watermarks.
async fn native_watermark(
    client: &reqwest::Client,
    base: &str,
    password: &str,
    session: &str,
    after: Option<u64>,
) -> Result<Option<u64>, String> {
    let mut url = format!(
        "{}/api/experimental/session/{}/log?follow=false",
        base.trim_end_matches('/'),
        urlencoding::encode(session)
    );
    if let Some(after) = after {
        url.push_str(&format!("&after={after}"));
    }
    let response = client
        .get(url)
        .basic_auth("opencode", Some(password))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        while let Some(end) = find_record_end(&buffer) {
            let record = buffer.drain(..end).collect::<Vec<_>>();
            let Some(data) = parse_sse_record(&String::from_utf8_lossy(&record)) else {
                continue;
            };
            let value: serde_json::Value =
                serde_json::from_str(&data).map_err(|e| e.to_string())?;
            if value["type"] != "log.synced" {
                continue;
            }
            if value["aggregateID"] != session {
                return Err("synced log marker has a different session".into());
            }
            // A newly created session has no durable execution records yet.
            return value
                .get("seq")
                .map(|seq| seq.as_u64().ok_or("invalid synced durable sequence".into()))
                .transpose();
        }
    }
    Err("native durable log was missing its synced marker".into())
}

/// The native feed is volatile. Subscribe first, then recover durable state
/// before releasing the readiness barrier; never recreate/delete on failure.
async fn reconcile_native(
    base: &str,
    password: &str,
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let active = native_get(&client, base, password, "/api/session/active").await?;
    if !active["data"].is_object() {
        return Err("missing active-session snapshot".into());
    }
    let mut parents = vec![peer.session_id.clone()];
    let mut visited = HashSet::new();
    while let Some(parent) = parents.pop() {
        if !visited.insert(parent.clone()) {
            continue;
        }
        let mut cursor: Option<String> = None;
        loop {
            let path = match &cursor {
                Some(cursor) => format!(
                    "/api/session?parentID={}&limit=200&cursor={}",
                    urlencoding::encode(&parent),
                    urlencoding::encode(cursor)
                ),
                None => format!(
                    "/api/session?parentID={}&limit=200&order=asc",
                    urlencoding::encode(&parent)
                ),
            };
            let page = native_get(&client, base, password, &path).await?;
            let children = page["data"]
                .as_array()
                .ok_or("missing child-session snapshot")?;
            for child in children {
                let id = child["id"].as_str().ok_or("missing child id")?;
                peer.router.lock().await.watched.insert(id.into());
                parents.push(id.into());
                if active["data"].get(id).is_some() {
                    peer.router.lock().await.native.mark_running(id);
                }
            }
            let next = page["cursor"]["next"].as_str().map(str::to_string);
            if next.is_none() {
                break;
            }
            if next == cursor {
                return Err("child cursor did not advance".into());
            }
            cursor = next;
        }
    }
    let sessions = peer.router.lock().await.watched_sessions();
    for session in sessions {
        let session_running = active["data"].get(&session).is_some();
        // A callback may have started and finished entirely while disconnected.
        // The volatile feed cannot recover it; idle roots need a checkpoint too.
        reconcile_native_history(
            &client,
            base,
            password,
            peer,
            event_tx,
            &session,
            session_running,
        )
        .await?;
    }
    let sessions = peer.router.lock().await.watched_sessions();
    for session in sessions {
        let encoded = urlencoding::encode(&session);
        // Pending asks can be raised by grandchildren and background children.
        for (suffix, kind, key) in [
            ("permission", "permission.asked", None),
            ("form", "form.created", Some("form")),
        ] {
            let page = native_get(
                &client,
                base,
                password,
                &format!("/api/session/{encoded}/{suffix}"),
            )
            .await?;
            for request in page["data"]
                .as_array()
                .ok_or("missing pending-request snapshot")?
            {
                let id = request["id"].as_str().ok_or("missing pending request id")?;
                let known = peer
                    .router
                    .lock()
                    .await
                    .session_for_permission(id)
                    .is_some();
                if !known {
                    let data = if let Some(key) = key {
                        serde_json::json!({key:request})
                    } else {
                        request.clone()
                    };
                    handle_native(
                        super::v2::Event {
                            durable: None,
                            id: format!("snapshot:request:{id}"),
                            kind: kind.into(),
                            data,
                        },
                        peer,
                        event_tx,
                    )
                    .await;
                }
            }
        }
    }
    Ok(())
}

/// Replay the same durable content boundary for root and descendant sessions.
async fn reconcile_native_history(
    client: &reqwest::Client,
    base: &str,
    password: &str,
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
    session: &str,
    _running: bool,
) -> Result<(), String> {
    // Completed roots may have several missed executions. Recover back to
    // their last observed Idle identity, across cursor pages. Other snapshots
    // retain the current execution's live-lifecycle recovery behavior.
    let encoded = urlencoding::encode(session);
    let observed_sequence = peer
        .router
        .lock()
        .await
        .native
        .observed_execution_sequence(session);
    let observed_idle = peer.router.lock().await.native.observed_idle_id(session);
    let mut snapshot = None;
    for attempt in 0..3 {
        let previous = peer.router.lock().await.native.recovered_sequence(session);
        let before = native_watermark(client, base, password, session, previous).await?;
        // Coordinator ownership is process-local and includes pre-start/settlement
        // gaps. Read it inside the projection window, never from the earlier scan.
        let active = native_get(client, base, password, "/api/session/active").await?;
        if !active["data"].is_object() {
            return Err("missing active-session snapshot".into());
        }
        let running = active["data"].get(session).is_some();
        let mut messages = Vec::new();
        let mut cursor: Option<String> = None;
        let mut boundaries = 0;
        let mut idle_message = None;
        let recover_completed_root = session == peer.session_id && observed_idle.is_some();
        let mut found_observed_idle = false;
        'history: loop {
            let path = match &cursor {
                Some(cursor) => format!(
                    "/api/session/{encoded}/message?limit=200&cursor={}",
                    urlencoding::encode(cursor)
                ),
                None => format!("/api/session/{encoded}/message?limit=200&order=desc"),
            };
            let page = native_get(client, base, password, &path).await?;
            for message in page["data"].as_array().ok_or("missing history snapshot")? {
                if message["type"] == "idle" {
                    boundaries += 1;
                    if boundaries == 1 {
                        idle_message = Some(message.clone());
                    }
                    if recover_completed_root {
                        if message["id"].as_str() == observed_idle.as_deref() {
                            found_observed_idle = true;
                            break 'history;
                        }
                    } else if boundaries > if running { 0 } else { 1 } {
                        break 'history;
                    }
                }
                messages.push(message.clone());
            }
            let next = page["cursor"]["next"].as_str().map(str::to_string);
            if next.is_none() {
                break;
            }
            if next == cursor {
                return Err("history cursor did not advance".into());
            }
            cursor = next;
        }
        let final_active = native_get(client, base, password, "/api/session/active").await?;
        if !final_active["data"].is_object() {
            return Err("missing active-session snapshot".into());
        }
        let final_running = final_active["data"].get(session).is_some();
        let after = native_watermark(client, base, password, session, before).await?;
        // Coordinator removal trails durable terminal publication, so equal
        // sequence checkpoints alone cannot prove its running flag stayed put.
        let stable = before == after && running == final_running;
        snapshot = Some((
            messages,
            idle_message,
            running,
            if stable { after } else { None },
            found_observed_idle,
        ));
        if stable || attempt == 2 {
            break;
        }
    }
    let (mut messages, idle_message, running, sequence, found_observed_idle) = snapshot.unwrap();
    if found_observed_idle && sequence.is_some() {
        let (reserved_turn, admitted_input) = {
            let ctx = peer.event_ctx.lock().await;
            let router = peer.router.lock().await;
            let pending = router.native.pending_user_admission(session);
            let turn = pending
                .as_ref()
                .map(|(_, turn)| crate::agent_provider::TurnId(turn.clone()))
                .or_else(|| ctx.turn_active.then(|| ctx.turn_id.clone()));
            let input = pending
                .map(|(input, _)| input)
                .or_else(|| router.native.user_admission_for(session, &ctx.turn_id.0));
            (turn, input)
        };
        let mut executions = Vec::new();
        let mut current = Vec::new();
        for message in messages.into_iter().rev() {
            if message["type"] == "idle" {
                executions.push((std::mem::take(&mut current), message));
            } else {
                current.push(message);
            }
        }
        let execution_count = executions.len();
        if !running
            && peer
                .router
                .lock()
                .await
                .native
                .clear_deferred_execution(session)
        {
            let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: peer.event_ctx.lock().await.thread_id.clone(),
                status: crate::agent_provider::SessionStatus::Ready,
            });
        }
        let mut reserved_completed = false;
        for (index, (execution, terminal)) in executions.into_iter().enumerate() {
            let mut ctx = peer.event_ctx.lock().await;
            let mut router = peer.router.lock().await;
            if !router
                .native
                .recover_terminal(session, terminal["id"].clone())
            {
                continue;
            }
            let user_index = admitted_input.as_ref().and_then(|input| {
                execution.iter().position(|message| {
                    message["type"] == "user" && message["id"].as_str() == Some(input)
                })
            });
            if let Some((user_index, turn)) = user_index.zip(reserved_turn.as_ref()) {
                drop(router);
                drop(ctx);
                replay_native_user_history(&execution, user_index, session, turn, peer, event_tx)
                    .await?;
                reserved_completed = true;
                replay_native_terminal(&terminal, session, peer, event_tx).await?;
                continue;
            }
            let preserve_latest = admitted_input.is_none()
                && (!running || current.is_empty())
                && index + 1 == execution_count;
            if let Some(turn) = reserved_turn.as_ref().filter(|_| preserve_latest) {
                ctx.turn_id = turn.clone();
                ctx.turn_active = true;
                reserved_completed = true;
            } else {
                ctx.turn_active = false;
                admit_native_root(&mut ctx);
            }
            router.native.mark_running(session);
            drop(router);
            let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ctx.thread_id.clone(),
                status: crate::agent_provider::SessionStatus::Running {
                    active_turn: ctx.turn_id.clone(),
                },
            });
            drop(ctx);
            replay_native_messages(&execution, session, peer, event_tx).await?;
            replay_native_terminal(&terminal, session, peer, event_tx).await?;
        }
        if running && current.is_empty() && (reserved_turn.is_none() || reserved_completed) {
            // This can be the recovered terminal's coordinator cleanup or a
            // newer execution before its first projected message. Keep claimed
            // terminals retired; only genuine new output/lifecycle admits a turn.
            peer.router
                .lock()
                .await
                .native
                .defer_execution(session, sequence.unwrap());
            return Ok(());
        }
        if running || (reserved_turn.is_some() && !reserved_completed) {
            let user_index = admitted_input.as_ref().and_then(|input| {
                current.iter().position(|message| {
                    message["type"] == "user" && message["id"].as_str() == Some(input)
                })
            });
            if let Some((user_index, turn)) =
                user_index.zip(reserved_turn.as_ref().filter(|_| !reserved_completed))
            {
                replay_native_user_history(&current, user_index, session, turn, peer, event_tx)
                    .await?;
            } else {
                let mut ctx = peer.event_ctx.lock().await;
                let mut router = peer.router.lock().await;
                if let Some(turn) = reserved_turn.filter(|_| !reserved_completed) {
                    let delivered = admitted_input.as_ref().is_some_and(|input| {
                        current.iter().any(|message| {
                            message["type"] == "user" && message["id"].as_str() == Some(input)
                        })
                    });
                    if delivered {
                        router.native.claim_pending_user_admission(
                            session,
                            admitted_input.as_deref().unwrap(),
                        );
                    }
                    if !router.native.has_pending_user_admission(session) {
                        if delivered && ctx.turn_active && ctx.turn_id != turn {
                            let _ = event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                                thread_id: ctx.thread_id.clone(),
                                turn_id: ctx.turn_id.clone(),
                                status: crate::agent_provider::TurnStatus::Success,
                                usage: None,
                            });
                        }
                        ctx.turn_id = turn;
                        ctx.turn_active = true;
                    }
                } else {
                    // Coordinator ownership can trail an already observed Idle.
                    let completed = idle_message
                        .as_ref()
                        .and_then(|m| m["id"].as_str())
                        .and_then(|id| router.native.completed_idle_sequence(session, id));
                    if current.is_empty()
                        && sequence
                            .zip(completed)
                            .is_some_and(|(seq, completed)| seq <= completed)
                    {
                        return Ok(());
                    }
                    admit_native_root(&mut ctx);
                }
                router.native.mark_running(session);
                drop(router);
                if ctx.turn_active {
                    let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
                        thread_id: ctx.thread_id.clone(),
                        status: crate::agent_provider::SessionStatus::Running {
                            active_turn: ctx.turn_id.clone(),
                        },
                    });
                }
                drop(ctx);
                replay_native_messages(&current, session, peer, event_tx).await?;
            }
        }
        if !running {
            // Retire buffered durable records only after every recovered answer
            // has its own execution/terminal identity. Active tails remain live.
            peer.router
                .lock()
                .await
                .native
                .recover_through(session, sequence.unwrap());
        }
        return Ok(());
    }
    // A moving checkpoint cannot settle executions. If the known boundary
    // disappeared, retain the conservative latest-execution snapshot too.
    let mut boundaries = 0;
    messages.retain(|message| {
        if message["type"] == "idle" {
            boundaries += 1;
        }
        boundaries <= if running { 0 } else { 1 }
    });
    let mut terminal_claimed = false;
    // Moving/running snapshots must not retire lifecycle records. Completed
    // item tombstones still deduplicate their replay; live terminals settle it.
    if !running && idle_message.is_some() {
        if let Some(sequence) = sequence {
            peer.router
                .lock()
                .await
                .native
                .recover_through(session, sequence);
        }
    }
    if running {
        if session == peer.session_id {
            let mut ctx = peer.event_ctx.lock().await;
            let mut router = peer.router.lock().await;
            let completed = idle_message
                .as_ref()
                .and_then(|message| message["id"].as_str())
                .and_then(|id| router.native.completed_idle_sequence(session, id));
            if !ctx.turn_active
                && messages.is_empty()
                && sequence
                    .zip(completed)
                    .is_some_and(|(current, completed)| current <= completed)
            {
                // Ownership can trail the completed Idle. With no newer
                // durable event/content, it cannot admit another root turn.
                return Ok(());
            }
            admit_native_root(&mut ctx);
            router.native.mark_running(session);
            drop(router);
            let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ctx.thread_id.clone(),
                status: crate::agent_provider::SessionStatus::Running {
                    active_turn: ctx.turn_id.clone(),
                },
            });
        } else {
            peer.router.lock().await.native.mark_running(session);
        }
    } else if session == peer.session_id {
        let mut ctx = peer.event_ctx.lock().await;
        if !ctx.turn_active {
            let Some(message) = idle_message.as_ref().filter(|_| sequence.is_some()) else {
                return Ok(());
            };
            if !message["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("msg_"))
            {
                return Err("missing native idle message identity".into());
            }
            let mut router = peer.router.lock().await;
            let advanced = sequence
                .zip(observed_sequence)
                .is_some_and(|(current, observed)| current > observed);
            let unseen = router
                .native
                .recover_terminal(session, message["id"].clone());
            if !advanced || !unseen {
                // Initial attachment records its baseline silently. An unchanged
                // Idle remains retired even if unrelated durable state advances.
                return Ok(());
            }
            admit_native_root(&mut ctx);
            router.native.mark_running(session);
            terminal_claimed = true;
            drop(router);
            let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ctx.thread_id.clone(),
                status: crate::agent_provider::SessionStatus::Running {
                    active_turn: ctx.turn_id.clone(),
                },
            });
        }
    }
    let messages: Vec<_> = messages.into_iter().rev().collect();
    replay_native_messages(&messages, session, peer, event_tx).await?;
    if !running && sequence.is_some() {
        // An admitted prompt can still be waiting to execute. Only a durable
        // idle marker proves completion; an empty/idle active snapshot cannot.
        let Some(message) = idle_message else {
            return Ok(());
        };
        let boundary = message["id"].clone();
        if !message["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("msg_"))
        {
            return Err("missing native idle message identity".into());
        }
        let mut router = peer.router.lock().await;
        if !terminal_claimed && !router.native.recover_terminal(session, boundary.clone()) {
            return Ok(());
        }
        router.native.mark_running(session);
        drop(router);
        replay_native_terminal(&message, session, peer, event_tx).await?;
    }
    Ok(())
}

/// A native busy period can drain background work and then a queued User
/// without another execution start. The exact User row is the ownership
/// boundary; assistant messages before it must not borrow the User's turn.
async fn replay_native_user_history(
    messages: &[serde_json::Value],
    user_index: usize,
    session: &str,
    user_turn: &crate::agent_provider::TurnId,
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) -> Result<(), String> {
    let prefix = &messages[..user_index];
    let user_already_active = {
        let ctx = peer.event_ctx.lock().await;
        ctx.turn_active && ctx.turn_id == *user_turn
    };
    // A live exact delivery already handed off this ordered prefix. Keeping
    // its remembered User mapping must not mint another background turn.
    if !user_already_active {
        let mut ctx = peer.event_ctx.lock().await;
        if ctx.turn_active || prefix.iter().any(|message| message["type"] == "assistant") {
            admit_native_root(&mut ctx);
            peer.router.lock().await.native.mark_running(session);
            let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ctx.thread_id.clone(),
                status: crate::agent_provider::SessionStatus::Running {
                    active_turn: ctx.turn_id.clone(),
                },
            });
            drop(ctx);
            replay_native_messages(prefix, session, peer, event_tx).await?;
            let mut ctx = peer.event_ctx.lock().await;
            ctx.turn_active = false;
            // The busy period continues with the User: this handoff has no
            // Ready state and cannot release another queued input.
            let _ = event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                status: crate::agent_provider::TurnStatus::Success,
                usage: None,
            });
        }
    }
    {
        let mut ctx = peer.event_ctx.lock().await;
        let mut router = peer.router.lock().await;
        let input = messages[user_index]["id"]
            .as_str()
            .ok_or("missing native User input identity")?;
        router.native.claim_pending_user_admission(session, input);
        router.native.mark_running(session);
        ctx.turn_id = user_turn.clone();
        ctx.turn_active = true;
        let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ctx.thread_id.clone(),
            status: crate::agent_provider::SessionStatus::Running {
                active_turn: ctx.turn_id.clone(),
            },
        });
    }
    replay_native_messages(&messages[user_index + 1..], session, peer, event_tx).await
}

async fn replay_native_terminal(
    message: &serde_json::Value,
    session: &str,
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) -> Result<(), String> {
    let boundary = message["id"]
        .as_str()
        .filter(|id| id.starts_with("msg_"))
        .ok_or("missing native idle message identity")?;
    let outcome = message["outcome"]
        .as_str()
        .ok_or("missing native idle outcome")?;
    handle_native(
        super::v2::Event {
            durable: None,
            id: format!("snapshot:terminal:{boundary}"),
            kind: format!("session.execution.{outcome}"),
            data: serde_json::json!({"sessionID":session,"error":message.get("error")}),
        },
        peer,
        event_tx,
    )
    .await;
    Ok(())
}

async fn replay_native_messages(
    messages: &[serde_json::Value],
    session: &str,
    peer: &SsePeer,
    event_tx: &broadcast::Sender<ProviderRuntimeEvent>,
) -> Result<(), String> {
    for message in messages.iter().filter(|m| m["type"] == "assistant") {
        let id = message["id"]
            .as_str()
            .ok_or("missing assistant message id")?;
        handle_native(super::v2::Event{durable:None,id:format!("snapshot:step:{id}"),kind:"session.step.started".into(),data:serde_json::json!({"sessionID":session,"assistantMessageID":id,"model":message["model"]})},peer,event_tx).await;
        let mut text_ordinal = 0_u64;
        let mut reasoning_ordinal = 0_u64;
        for part in message["content"]
            .as_array()
            .ok_or("missing assistant content")?
            .iter()
        {
            if part["type"] == "tool" {
                let tool_id = part["id"].as_str().ok_or("missing tool id")?;
                let base =
                    serde_json::json!({"sessionID":session,"assistantMessageID":id,"id":tool_id});
                let mut started = base.clone();
                started["name"] = part["name"].clone();
                handle_native(
                    super::v2::Event {
                        durable: None,
                        id: format!("snapshot:tool-start:{id}:{tool_id}"),
                        kind: "session.tool.input.started".into(),
                        data: started,
                    },
                    peer,
                    event_tx,
                )
                .await;
                if part["state"]["input"].is_object() {
                    let mut called = base.clone();
                    called["input"] = part["state"]["input"].clone();
                    handle_native(
                        super::v2::Event {
                            durable: None,
                            id: format!("snapshot:tool-call:{id}:{tool_id}"),
                            kind: "session.tool.called".into(),
                            data: called,
                        },
                        peer,
                        event_tx,
                    )
                    .await;
                }
                let status = part["state"]["status"].as_str().unwrap_or("streaming");
                if matches!(status, "completed" | "error") {
                    let mut result = base;
                    result["content"] = part["state"]["content"].clone();
                    result["error"] = part["state"]["error"].clone();
                    result["metadata"] = part["state"]["metadata"].clone();
                    handle_native(
                        super::v2::Event {
                            durable: None,
                            id: format!("snapshot:tool-result:{id}:{tool_id}"),
                            kind: if status == "completed" {
                                "session.tool.success"
                            } else {
                                "session.tool.failed"
                            }
                            .into(),
                            data: result,
                        },
                        peer,
                        event_tx,
                    )
                    .await;
                }
                continue;
            }
            let (prefix, ordinal) = match part["type"].as_str() {
                Some("text") => {
                    let ordinal = text_ordinal;
                    text_ordinal += 1;
                    ("session.text", ordinal)
                }
                Some("reasoning") => {
                    let ordinal = reasoning_ordinal;
                    reasoning_ordinal += 1;
                    ("session.reasoning", ordinal)
                }
                _ => continue,
            };
            // A still-streaming message can race buffered live deltas.
            // Do not append its partial snapshot or prematurely end it;
            // the authoritative ended event restores the full segment.
            if !message["time"]["completed"].is_number() {
                continue;
            }
            let suffix = "ended";
            handle_native(super::v2::Event{durable:None,id:format!("snapshot:content:{id}:{prefix}:{ordinal}"),kind:format!("{prefix}.{suffix}"),data:serde_json::json!({"sessionID":session,"assistantMessageID":id,"ordinal":ordinal,"text":part["text"]})},peer,event_tx).await;
        }
        if message.get("tokens").is_some() {
            handle_native(super::v2::Event{durable:None,id:format!("snapshot:usage:{id}"),kind:"session.step.ended".into(),data:serde_json::json!({"sessionID":session,"assistantMessageID":id,"tokens":message["tokens"],"cost":message["cost"]})},peer,event_tx).await;
        }
    }
    Ok(())
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
        OpenCodeEvent::V2(_) | OpenCodeEvent::Other(_) => return None,
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
async fn emit_give_up_terminal(peer: &SsePeer, event_tx: &broadcast::Sender<ProviderRuntimeEvent>) {
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
    use super::super::protocol::KnownEvent;
    use super::*;
    use crate::agent_provider::types::{ProviderSessionId, ThreadId, TurnId};

    fn peer(session_id: &str) -> SsePeer {
        SsePeer {
            api: super::super::api::OpenCodeApi::Legacy,
            ready: watch::channel(true).0,
            directory: None,
            session_id: session_id.to_string(),
            event_ctx: Arc::new(Mutex::new(EventContext {
                thread_id: ThreadId("t1".into()),
                turn_id: TurnId("turn1".into()),
                provider_session_id: ProviderSessionId(session_id.to_string()),
                turn_active: false,
                context_window_tokens: None,
                model_cost: None,
            })),
            router: Arc::new(Mutex::new(SseRouter::new(session_id))),
            dead: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            usage: Arc::new(Mutex::new(OpenCodeUsageState::default())),
        }
    }

    fn native_record(id: &str, kind: &str, data: serde_json::Value, seq: Option<u64>) -> String {
        let mut value = serde_json::json!({"id":id,"type":kind,"created":0,"data":data});
        if let Some(seq) = seq {
            value["durable"] =
                serde_json::json!({"aggregateID":value["data"]["sessionID"],"seq":seq,"version":1});
        }
        format!("data: {value}\n\n")
    }

    fn watermark_record(session: &str, seq: u64) -> String {
        format!(
            "data: {}\n\n",
            serde_json::json!({"type":"log.synced","aggregateID":session,"seq":seq})
        )
    }

    async fn mock_watermarks(
        server: &mut mockito::ServerGuard,
        session: &str,
        seq: u64,
    ) -> Vec<mockito::Mock> {
        let mut mocks = Vec::new();
        for suffix in [String::new(), format!("&after={seq}")] {
            mocks.push(
                server
                    .mock(
                        "GET",
                        format!("/api/experimental/session/{session}/log?follow=false{suffix}")
                            .as_str(),
                    )
                    .with_status(200)
                    .with_body(watermark_record(session, seq))
                    .expect_at_least(0)
                    .create_async()
                    .await,
            );
        }
        mocks
    }

    async fn mock_completed_root_callback(
        server: &mut mockito::ServerGuard,
        running: bool,
    ) -> Vec<mockito::Mock> {
        let mut mocks = Vec::new();
        for (path, body) in [
            (
                "/api/session/active",
                serde_json::json!({"data":if running { serde_json::json!({"root":{}}) } else { serde_json::json!({}) }}),
            ),
            (
                "/api/session?parentID=root&limit=200&order=asc",
                serde_json::json!({"data":[]}),
            ),
            (
                "/api/session/root/permission",
                serde_json::json!({"data":[]}),
            ),
            ("/api/session/root/form", serde_json::json!({"data":[]})),
            (
                "/api/session/root/message?limit=200&order=desc",
                serde_json::json!({"data":[
                    {"id":"msg_callback_done","type":"idle","outcome":"succeeded"},
                    {"id":"callback_message","type":"assistant","time":{"completed":3},
                        "content":[{"type":"text","text":"background follow-up"}]},
                    {"id":"msg_a_done","type":"idle","outcome":"succeeded"},
                    {"id":"old_message","type":"assistant","time":{"completed":2},
                        "content":[{"type":"text","text":"old response"}]}
                ]}),
            ),
        ] {
            mocks.push(
                server
                    .mock("GET", path)
                    .with_status(200)
                    .with_body(body.to_string())
                    .expect_at_least(1)
                    .create_async()
                    .await,
            );
        }
        mocks.extend(mock_watermarks(server, "root", 20).await);
        mocks
    }

    #[tokio::test]
    async fn native_recovery_recovers_root_callback_completed_wholly_during_outage_once() {
        let mut server = mockito::Server::new_async().await;
        let mocks = mock_completed_root_callback(&mut server, false).await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        let (tx, mut rx) = broadcast::channel(64);
        for (id, kind, seq) in [
            ("evt_a_start", "session.execution.started", 1),
            ("evt_a_done", "session.execution.succeeded", 10),
        ] {
            handle_record(
                &native_record(id, kind, serde_json::json!({"sessionID":"root"}), Some(seq)),
                &p,
                &tx,
            )
            .await;
        }
        let original_turn = p.event_ctx.lock().await.turn_id.clone();
        assert!(!p.event_ctx.lock().await.turn_active);
        while rx.try_recv().is_ok() {}
        // Both start and completion happened before resubscribing, so the
        // volatile feed contributes no callback event to this recovery.
        reconcile_native(&server.url(), "fixture", &p, &tx)
            .await
            .unwrap();
        let callback_turn = p.event_ctx.lock().await.turn_id.clone();
        assert_ne!(callback_turn, original_turn);
        assert!(!p.event_ctx.lock().await.turn_active);
        let observed: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(
            observed
                .iter()
                .filter(|event| matches!(event,
            ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &callback_turn))
                .count(),
            1
        );
        assert_eq!(observed.iter().filter(|event| matches!(event,
            ProviderRuntimeEvent::ItemCompleted { item: crate::agent_provider::CompletedItem::AssistantText { text }, .. }
                if text == "background follow-up")).count(), 1);
        assert!(!observed.iter().any(|event| matches!(event,
            ProviderRuntimeEvent::ItemCompleted { item: crate::agent_provider::CompletedItem::AssistantText { text }, .. }
                if text == "old response")));
        reconcile_native(&server.url(), "fixture", &p, &tx)
            .await
            .unwrap();
        assert_eq!(p.event_ctx.lock().await.turn_id, callback_turn);
        assert!(
            rx.try_recv().is_err(),
            "unchanged idle callback must not replay"
        );
        for mock in mocks {
            mock.assert_async().await;
        }
    }

    #[tokio::test]
    async fn native_recovery_recovers_two_completed_root_callbacks_across_pages_on_distinct_turns()
    {
        for (admitted_first, running_tail, absent_input) in [
            (false, false, false),
            (true, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            let mut server = mockito::Server::new_async().await;
            let mut mocks = mock_watermarks(&mut server, "root", 30).await;
            for (path, body) in [
                (
                    "/api/session/active",
                    serde_json::json!({"data":if running_tail { serde_json::json!({"root":{}}) } else { serde_json::json!({}) }}),
                ),
                (
                    "/api/session?parentID=root&limit=200&order=asc",
                    serde_json::json!({"data":[]}),
                ),
                (
                    "/api/session/root/permission",
                    serde_json::json!({"data":[]}),
                ),
                ("/api/session/root/form", serde_json::json!({"data":[]})),
            ] {
                mocks.push(
                    server
                        .mock("GET", path)
                        .with_status(200)
                        .with_body(body.to_string())
                        .expect_at_least(1)
                        .create_async()
                        .await,
                );
            }
            let mut newest = Vec::new();
            if running_tail {
                newest.extend([
                    serde_json::json!({"id":"m_d","type":"assistant","time":{"completed":7},"content":[{"type":"text","text":"D"}]}),
                    serde_json::json!({"id":"input_d","type":"user","text":"D"}),
                ]);
            }
            newest.extend([
                serde_json::json!({"id":"msg_c_done","type":"idle","outcome":"succeeded"}),
                serde_json::json!({"id":"m_c","type":"assistant","time":{"completed":5},"content":[{"type":"text","text":"C"}]}),
                serde_json::json!({"id":"msg_b_done","type":"idle","outcome":"succeeded"}),
            ]);
            let first_page = server
                .mock("GET", "/api/session/root/message?limit=200&order=desc")
                .with_status(200)
                .with_body(serde_json::json!({"data":newest,"cursor":{"next":"older"}}).to_string())
                .expect(2)
                .create_async()
                .await;
            let older_page = server.mock("GET", "/api/session/root/message?limit=200&cursor=older")
                .with_status(200).with_body(serde_json::json!({"data":[
                    {"id":"m_b","type":"assistant","time":{"completed":3},"content":[{"type":"text","text":"B"}]},
                    {"id":"input_b","type":"user","text":"B"},
                    {"id":"msg_a_done","type":"idle","outcome":"succeeded"},
                    {"id":"m_a","type":"assistant","time":{"completed":1},"content":[{"type":"text","text":"old A"}]}
                ]}).to_string()).expect(if running_tail { 2 } else { 1 }).create_async().await;
            let mut p = peer("root");
            p.api = super::super::api::OpenCodeApi::V2;
            let (tx, mut rx) = broadcast::channel(64);
            for (id, kind, seq) in [
                ("evt_a_start", "session.execution.started", 1),
                ("evt_a_done", "session.execution.succeeded", 10),
            ] {
                handle_record(
                    &native_record(id, kind, serde_json::json!({"sessionID":"root"}), Some(seq)),
                    &p,
                    &tx,
                )
                .await;
            }
            let original_turn = p.event_ctx.lock().await.turn_id.clone();
            let admitted_turn = TurnId("admitted_b".into());
            if admitted_first {
                let mut ctx = p.event_ctx.lock().await;
                ctx.turn_id = admitted_turn.clone();
                ctx.turn_active = true;
                p.router.lock().await.native.remember_user_admission(
                    "root",
                    if absent_input {
                        "input_pending"
                    } else if running_tail {
                        "input_d"
                    } else {
                        "input_b"
                    },
                    &admitted_turn.0,
                );
            }
            while rx.try_recv().is_ok() {}
            reconcile_native(&server.url(), "fixture", &p, &tx)
                .await
                .unwrap();
            let observed: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            let responses: Vec<_> = observed
                .iter()
                .filter_map(|event| match event {
                    ProviderRuntimeEvent::ItemCompleted {
                        turn_id,
                        item: crate::agent_provider::CompletedItem::AssistantText { text },
                        ..
                    } => Some((turn_id.clone(), text.as_str())),
                    _ => None,
                })
                .collect();
            assert_eq!(
                responses.iter().map(|(_, text)| *text).collect::<Vec<_>>(),
                if running_tail {
                    vec!["B", "C", "D"]
                } else {
                    vec!["B", "C"]
                }
            );
            assert_ne!(responses[0].0, responses[1].0);
            assert_ne!(responses[0].0, original_turn);
            assert_eq!(
                responses[0].0 == admitted_turn,
                admitted_first && !running_tail && !absent_input
            );
            assert_ne!(responses[1].0, original_turn);
            for (turn, _) in responses.iter().take(2) {
                assert_eq!(
                    observed
                        .iter()
                        .filter(|event| matches!(event,
                    ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == turn))
                        .count(),
                    1
                );
            }
            assert_eq!(
                p.router.lock().await.native.recovered_sequence("root"),
                if running_tail { None } else { Some(30) }
            );
            assert_eq!(p.event_ctx.lock().await.turn_active, running_tail);
            if running_tail {
                assert_eq!(responses[2].0, admitted_turn);
            }
            if absent_input {
                assert!(p
                    .router
                    .lock()
                    .await
                    .native
                    .has_pending_user_admission("root"));
            }
            reconcile_native(&server.url(), "fixture", &p, &tx)
                .await
                .unwrap();
            for (id, kind, seq) in [
                ("evt_b_start", "session.execution.started", 11),
                ("evt_b_done", "session.execution.succeeded", 20),
                ("evt_c_start", "session.execution.started", 21),
                ("evt_c_done", "session.execution.succeeded", 30),
            ] {
                handle_record(
                    &native_record(id, kind, serde_json::json!({"sessionID":"root"}), Some(seq)),
                    &p,
                    &tx,
                )
                .await;
            }
            let replayed: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert!(
                !replayed.iter().any(|event| matches!(
                    event,
                    ProviderRuntimeEvent::TurnCompleted { .. }
                        | ProviderRuntimeEvent::ItemCompleted { .. }
                )),
                "checkpoint and buffered lifecycle replay must not repeat B or C"
            );
            assert_eq!(p.event_ctx.lock().await.turn_active, running_tail);
            if running_tail || absent_input {
                if absent_input {
                    handle_record(
                        &native_record(
                            "evt_pending_delivered",
                            "session.inbox.delivered",
                            serde_json::json!({"sessionID":"root","inboxID":"input_pending"}),
                            Some(35),
                        ),
                        &p,
                        &tx,
                    )
                    .await;
                }
                assert_eq!(p.event_ctx.lock().await.turn_id, admitted_turn);
                handle_record(
                    &native_record(
                        "evt_d_done",
                        "session.execution.succeeded",
                        serde_json::json!({"sessionID":"root"}),
                        Some(40),
                    ),
                    &p,
                    &tx,
                )
                .await;
                assert_eq!(std::iter::from_fn(|| rx.try_recv().ok()).filter(|event| matches!(event,
                    ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &admitted_turn)).count(), 1);
                assert!(!p.event_ctx.lock().await.turn_active);
            }
            first_page.assert_async().await;
            older_page.assert_async().await;
            for mock in mocks {
                mock.assert_async().await;
            }
        }
    }

    #[tokio::test]
    async fn native_recovery_splits_background_and_exact_user_within_one_busy_period() {
        for running_tail in [false, true] {
            for live_prefix in ["unseen", "partial", "delivered"] {
                let mut server = mockito::Server::new_async().await;
                let mut mocks = mock_watermarks(&mut server, "root", 30).await;
                for (path, body) in [
                    (
                        "/api/session/active",
                        serde_json::json!({"data":if running_tail { serde_json::json!({"root":{}}) } else { serde_json::json!({}) }}),
                    ),
                    (
                        "/api/session?parentID=root&limit=200&order=asc",
                        serde_json::json!({"data":[]}),
                    ),
                    (
                        "/api/session/root/permission",
                        serde_json::json!({"data":[]}),
                    ),
                    ("/api/session/root/form", serde_json::json!({"data":[]})),
                ] {
                    mocks.push(
                        server
                            .mock("GET", path)
                            .with_status(200)
                            .with_body(body.to_string())
                            .expect_at_least(1)
                            .create_async()
                            .await,
                    );
                }
                let mut history = Vec::new();
                if !running_tail {
                    history.push(
                        serde_json::json!({"id":"msg_x_done","type":"idle","outcome":"succeeded"}),
                    );
                }
                history.extend([
                    serde_json::json!({"id":"msg_x_assistant","type":"assistant","time":{"completed":5},"content":[{"type":"text","text":"answer X"}]}),
                    serde_json::json!({"id":"msg_user_x","type":"user","text":"User X"}),
                    serde_json::json!({"id":"msg_b_assistant","type":"assistant","time":{"completed":3},"content":[{"type":"text","text":"background B"}]}),
                    serde_json::json!({"id":"msg_a_done","type":"idle","outcome":"succeeded"}),
                ]);
                mocks.push(
                    server
                        .mock("GET", "/api/session/root/message?limit=200&order=desc")
                        .with_status(200)
                        .with_body(serde_json::json!({"data":history}).to_string())
                        .expect(2)
                        .create_async()
                        .await,
                );
                let mut p = peer("root");
                p.api = super::super::api::OpenCodeApi::V2;
                let (tx, mut rx) = broadcast::channel(64);
                for (id, kind, seq) in [
                    ("evt_a_start", "session.execution.started", 1),
                    ("evt_a_done", "session.execution.succeeded", 10),
                ] {
                    handle_record(
                        &native_record(
                            id,
                            kind,
                            serde_json::json!({"sessionID":"root"}),
                            Some(seq),
                        ),
                        &p,
                        &tx,
                    )
                    .await;
                }
                while rx.try_recv().is_ok() {}
                let user_turn = TurnId("reserved_user_x".into());
                {
                    let mut ctx = p.event_ctx.lock().await;
                    ctx.turn_id = user_turn.clone();
                    ctx.turn_active = false;
                    p.router.lock().await.native.remember_user_admission(
                        "root",
                        "msg_user_x",
                        &user_turn.0,
                    );
                }
                let mut original_background = None;
                if live_prefix != "unseen" {
                    handle_record(
                        &native_record(
                            "evt_b_start",
                            "session.execution.started",
                            serde_json::json!({"sessionID":"root"}),
                            Some(11),
                        ),
                        &p,
                        &tx,
                    )
                    .await;
                    let (kind, data) = if live_prefix == "delivered" {
                        (
                            "session.text.ended",
                            serde_json::json!({"sessionID":"root","assistantMessageID":"msg_b_assistant","ordinal":0,"text":"background B"}),
                        )
                    } else {
                        (
                            "session.text.delta",
                            serde_json::json!({"sessionID":"root","assistantMessageID":"msg_b_assistant","ordinal":0,"delta":"background "}),
                        )
                    };
                    handle_record(&native_record("evt_b_text", kind, data, Some(12)), &p, &tx)
                        .await;
                    original_background = Some(p.event_ctx.lock().await.turn_id.clone());
                }
                if live_prefix == "delivered" {
                    handle_record(
                        &native_record(
                            "evt_x_delivered",
                            "session.inbox.delivered",
                            serde_json::json!({"sessionID":"root","inboxID":"msg_user_x"}),
                            Some(21),
                        ),
                        &p,
                        &tx,
                    )
                    .await;
                    assert!(!p
                        .router
                        .lock()
                        .await
                        .native
                        .has_pending_user_admission("root"));
                }
                reconcile_native(&server.url(), "fixture", &p, &tx)
                    .await
                    .unwrap();
                let observed: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
                let responses: Vec<_> = observed
                    .iter()
                    .filter_map(|event| match event {
                        ProviderRuntimeEvent::ItemCompleted {
                            turn_id,
                            item: crate::agent_provider::CompletedItem::AssistantText { text },
                            ..
                        } => Some((turn_id.clone(), text.as_str())),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    responses.iter().map(|(_, text)| *text).collect::<Vec<_>>(),
                    vec!["background B", "answer X"],
                    "{running_tail}/{live_prefix}"
                );
                let background_turn = &responses[0].0;
                assert_ne!(background_turn, &user_turn);
                assert_eq!(responses[1].0, user_turn);
                if let Some(original) = original_background {
                    assert_eq!(
                        *background_turn, original,
                        "a live background prefix keeps its turn"
                    );
                }
                let terminals = |turn: &TurnId| {
                    observed.iter().filter(|event|
                    matches!(event, ProviderRuntimeEvent::TurnCompleted {turn_id, ..} if turn_id == turn)).count()
                };
                assert_eq!(terminals(background_turn), 1);
                assert_eq!(terminals(&user_turn), if running_tail { 0 } else { 1 });
                let background_end = observed.iter().position(|event|
                    matches!(event, ProviderRuntimeEvent::TurnCompleted {turn_id, ..} if turn_id == background_turn)).unwrap();
                let user_output = observed.iter().position(|event|
                    matches!(event, ProviderRuntimeEvent::ItemCompleted {turn_id, ..} if turn_id == &user_turn)).unwrap();
                assert!(
                    !observed[background_end..=user_output]
                        .iter()
                        .any(|event| matches!(
                            event,
                            ProviderRuntimeEvent::SessionStateChanged {
                                status: crate::agent_provider::SessionStatus::Ready,
                                ..
                            }
                        )),
                    "handoff must not release the queue between background and User"
                );
                assert_eq!(p.event_ctx.lock().await.turn_active, running_tail);
                assert!(!p
                    .router
                    .lock()
                    .await
                    .native
                    .has_pending_user_admission("root"));
                reconcile_native(&server.url(), "fixture", &p, &tx)
                    .await
                    .unwrap();
                let duplicate: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
                assert!(
                    !duplicate.iter().any(|event| matches!(
                        event,
                        ProviderRuntimeEvent::ItemCompleted { .. }
                            | ProviderRuntimeEvent::TurnCompleted { .. }
                    )),
                    "a repeated checkpoint must not replay either ownership segment"
                );
                if running_tail {
                    handle_record(
                        &native_record(
                            "evt_x_done",
                            "session.execution.succeeded",
                            serde_json::json!({"sessionID":"root"}),
                            Some(40),
                        ),
                        &p,
                        &tx,
                    )
                    .await;
                    assert_eq!(std::iter::from_fn(|| rx.try_recv().ok()).filter(|event|
                        matches!(event, ProviderRuntimeEvent::TurnCompleted {turn_id, ..} if turn_id == &user_turn)).count(), 1);
                }
                for mock in mocks {
                    mock.assert_async().await;
                }
            }
        }
    }

    #[tokio::test]
    async fn native_recovery_initial_idle_attachment_records_baseline_without_replaying_history() {
        let mut server = mockito::Server::new_async().await;
        let mocks = mock_completed_root_callback(&mut server, false).await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        let (tx, mut rx) = broadcast::channel(64);
        let original_turn = p.event_ctx.lock().await.turn_id.clone();
        for _ in 0..2 {
            reconcile_native(&server.url(), "fixture", &p, &tx)
                .await
                .unwrap();
            assert_eq!(p.event_ctx.lock().await.turn_id, original_turn);
            assert!(!p.event_ctx.lock().await.turn_active);
            assert!(rx.try_recv().is_err());
        }
        assert_eq!(
            p.router
                .lock()
                .await
                .native
                .observed_execution_sequence("root"),
            Some(20)
        );
        for mock in mocks {
            mock.assert_async().await;
        }
    }

    #[tokio::test]
    async fn native_recovery_active_without_new_history_defers_identity_until_real_execution_or_idle(
    ) {
        for activity in ["idle", "start", "content", "permission", "form"] {
            let mut server = mockito::Server::new_async().await;
            let mocks = mock_completed_root_callback(&mut server, true).await;
            let mut p = peer("root");
            p.api = super::super::api::OpenCodeApi::V2;
            let (tx, mut rx) = broadcast::channel(64);
            handle_record(
                &native_record(
                    "evt_a_start",
                    "session.execution.started",
                    serde_json::json!({"sessionID":"root"}),
                    Some(1),
                ),
                &p,
                &tx,
            )
            .await;
            handle_record(
                &native_record(
                    "evt_a_done",
                    "session.execution.succeeded",
                    serde_json::json!({"sessionID":"root"}),
                    Some(10),
                ),
                &p,
                &tx,
            )
            .await;
            while rx.try_recv().is_ok() {}
            reconcile_native(&server.url(), "fixture", &p, &tx)
                .await
                .unwrap();
            let completed_turn = p.event_ctx.lock().await.turn_id.clone();
            assert!(!p.event_ctx.lock().await.turn_active);
            assert!(p.router.lock().await.native.awaiting_execution("root"));
            assert!(p
                .router
                .lock()
                .await
                .native
                .recovered_sequence("root")
                .is_none());
            assert_eq!(
                std::iter::from_fn(|| rx.try_recv().ok())
                    .filter(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }))
                    .count(),
                1
            );
            handle_record(
                &native_record(
                    "evt_buffered_callback_start",
                    "session.execution.started",
                    serde_json::json!({"sessionID":"root"}),
                    Some(11),
                ),
                &p,
                &tx,
            )
            .await;
            handle_record(
                &native_record(
                    "evt_callback_done",
                    "session.execution.succeeded",
                    serde_json::json!({"sessionID":"root"}),
                    Some(20),
                ),
                &p,
                &tx,
            )
            .await;
            assert!(
                rx.try_recv().is_err(),
                "the recovered callback terminal must not admit or settle a phantom turn"
            );
            if activity != "idle" {
                let (kind, data, seq) = match activity {
                    "start" => (
                        "session.execution.started",
                        serde_json::json!({"sessionID":"root"}),
                        Some(21),
                    ),
                    "content" => (
                        "session.text.ended",
                        serde_json::json!({"sessionID":"root","assistantMessageID":"m_d","ordinal":0,"text":"D"}),
                        Some(21),
                    ),
                    "permission" => (
                        "permission.asked",
                        serde_json::json!({"sessionID":"root","id":"per_d","action":"read","resources":["x"]}),
                        None,
                    ),
                    "form" => (
                        "form.created",
                        serde_json::json!({"form":{"sessionID":"root","id":"f_d","metadata":{"kind":"question"},"fields":[{"key":"q0","type":"string","description":"D?","custom":true,"options":[]}]}}),
                        None,
                    ),
                    _ => unreachable!(),
                };
                handle_record(&native_record("evt_d_activity", kind, data, seq), &p, &tx).await;
                let d_turn = p.event_ctx.lock().await.turn_id.clone();
                assert_ne!(d_turn, completed_turn);
                assert!(p.event_ctx.lock().await.turn_active);
                assert!(!p.router.lock().await.native.awaiting_execution("root"));
                handle_record(
                    &native_record(
                        "evt_d_done",
                        "session.execution.succeeded",
                        serde_json::json!({"sessionID":"root"}),
                        Some(25),
                    ),
                    &p,
                    &tx,
                )
                .await;
                assert_eq!(
                    std::iter::from_fn(|| rx.try_recv().ok())
                        .filter(|event| matches!(event,
                    ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &d_turn))
                        .count(),
                    1
                );
            } else {
                mocks[0].remove_async().await;
                server
                    .mock("GET", "/api/session/active")
                    .with_status(200)
                    .with_body("{\"data\":{}}")
                    .expect_at_least(1)
                    .create_async()
                    .await;
                reconcile_native(&server.url(), "fixture", &p, &tx)
                    .await
                    .unwrap();
                assert!(!p.router.lock().await.native.awaiting_execution("root"));
                assert_eq!(p.event_ctx.lock().await.turn_id, completed_turn);
                assert!(!std::iter::from_fn(|| rx.try_recv().ok())
                    .any(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { .. })));
            }
            assert!(!p.event_ctx.lock().await.turn_active);
        }
    }

    #[tokio::test]
    async fn native_recovery_pending_root_request_admits_current_execution_after_completed_callbacks(
    ) {
        for form in [false, true] {
            let mut server = mockito::Server::new_async().await;
            let mocks = mock_completed_root_callback(&mut server, true).await;
            let (index, suffix, request) = if form {
                (
                    3,
                    "form",
                    serde_json::json!({"sessionID":"root","id":"f_d","metadata":{"kind":"question"},
                    "fields":[{"key":"q0","type":"string","description":"D?","custom":true,"options":[]}]}),
                )
            } else {
                (
                    2,
                    "permission",
                    serde_json::json!({"sessionID":"root","id":"per_d","action":"read","resources":["x"]}),
                )
            };
            mocks[index].remove_async().await;
            server
                .mock("GET", format!("/api/session/root/{suffix}").as_str())
                .with_status(200)
                .with_body(serde_json::json!({"data":[request]}).to_string())
                .create_async()
                .await;
            let mut p = peer("root");
            p.api = super::super::api::OpenCodeApi::V2;
            let (tx, mut rx) = broadcast::channel(64);
            handle_record(
                &native_record(
                    "evt_a_start",
                    "session.execution.started",
                    serde_json::json!({"sessionID":"root"}),
                    Some(1),
                ),
                &p,
                &tx,
            )
            .await;
            handle_record(
                &native_record(
                    "evt_a_done",
                    "session.execution.succeeded",
                    serde_json::json!({"sessionID":"root"}),
                    Some(10),
                ),
                &p,
                &tx,
            )
            .await;
            while rx.try_recv().is_ok() {}
            reconcile_native(&server.url(), "fixture", &p, &tx)
                .await
                .unwrap();
            let observed: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            let completed_turn = observed
                .iter()
                .find_map(|event| match event {
                    ProviderRuntimeEvent::TurnCompleted { turn_id, .. } => Some(turn_id.clone()),
                    _ => None,
                })
                .expect("completed callback must be recovered");
            let request_turn = observed
                .iter()
                .find_map(|event| match event {
                    ProviderRuntimeEvent::RequestOpened { turn_id, .. } => Some(turn_id.clone()),
                    _ => None,
                })
                .expect("pending D request must be recovered");
            assert_ne!(request_turn, completed_turn);
            assert_eq!(p.event_ctx.lock().await.turn_id, request_turn);
            assert!(p.event_ctx.lock().await.turn_active);
            assert!(!p.router.lock().await.native.awaiting_execution("root"));
            handle_record(
                &native_record(
                    "evt_callback_done",
                    "session.execution.succeeded",
                    serde_json::json!({"sessionID":"root"}),
                    Some(20),
                ),
                &p,
                &tx,
            )
            .await;
            assert!(rx.try_recv().is_err());
            handle_record(
                &native_record(
                    "evt_d_done",
                    "session.execution.succeeded",
                    serde_json::json!({"sessionID":"root"}),
                    Some(25),
                ),
                &p,
                &tx,
            )
            .await;
            assert_eq!(
                std::iter::from_fn(|| rx.try_recv().ok())
                    .filter(|event| matches!(event,
                ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &request_turn))
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn native_reconnect_admits_running_background_root_with_fresh_turn_identity() {
        let mut server = mockito::Server::new_async().await;
        let _checkpoints = mock_watermarks(&mut server, "root", 10).await;
        let active = server
            .mock("GET", "/api/session/active")
            .expect_at_least(1)
            .with_status(200)
            .with_body(r#"{"data":{"root":{}}}"#)
            .create_async()
            .await;
        for path in [
            "/api/session?parentID=root&limit=200&order=asc",
            "/api/session/root/permission",
            "/api/session/root/form",
        ] {
            server
                .mock("GET", path)
                .with_status(200)
                .with_body(r#"{"data":[]}"#)
                .create_async()
                .await;
        }
        let history = server
            .mock("GET", "/api/session/root/message?limit=200&order=desc")
            .with_status(200)
            .with_body(
                serde_json::json!({"data":[{
                    "id":"msg_callback","type":"assistant","time":{"completed":3},
                    "content":[{"type":"text","text":"background result"}]
                }]})
                .to_string(),
            )
            .create_async()
            .await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        let previous_turn = p.event_ctx.lock().await.turn_id.clone();
        let (tx, mut rx) = broadcast::channel(32);
        reconcile_native(&server.url(), "fixture", &p, &tx)
            .await
            .unwrap();
        let resumed_turn = p.event_ctx.lock().await.turn_id.clone();
        assert_ne!(resumed_turn, previous_turn);
        assert!(p.event_ctx.lock().await.turn_active);
        let mut events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().any(|event| matches!(event,
            ProviderRuntimeEvent::SessionStateChanged { status: crate::agent_provider::SessionStatus::Running { active_turn }, .. }
                if active_turn == &resumed_turn
        )));
        assert!(events.iter().any(|event| matches!(event,
            ProviderRuntimeEvent::ItemCompleted { turn_id, item: crate::agent_provider::CompletedItem::AssistantText { text }, .. }
                if turn_id == &resumed_turn && text == "background result"
        )));
        handle_record(
            &native_record(
                "evt_callback_done",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
                Some(11),
            ),
            &p,
            &tx,
        )
        .await;
        events.extend(std::iter::from_fn(|| rx.try_recv().ok()));
        assert!(!p.event_ctx.lock().await.turn_active);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event,
                    ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &resumed_turn
                ))
                .count(),
            1
        );
        assert!(!events.iter().any(|event| matches!(event,
            ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &previous_turn
        )));
        active.assert_async().await;
        history.assert_async().await;
    }

    #[tokio::test]
    async fn native_reconnect_cached_idle_ownership_does_not_mint_a_phantom_root() {
        let mut server = mockito::Server::new_async().await;
        let _checkpoints = mock_watermarks(&mut server, "root", 10).await;
        let active = server
            .mock("GET", "/api/session/active")
            .with_status(200)
            .with_body(r#"{"data":{"root":{}}}"#)
            .expect(3)
            .create_async()
            .await;
        for path in [
            "/api/session?parentID=root&limit=200&order=asc",
            "/api/session/root/permission",
            "/api/session/root/form",
        ] {
            server
                .mock("GET", path)
                .with_status(200)
                .with_body(r#"{"data":[]}"#)
                .create_async()
                .await;
        }
        let history = server
            .mock("GET", "/api/session/root/message?limit=200&order=desc")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"msg_previous","type":"idle","outcome":"succeeded"}]}"#)
            .create_async()
            .await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.event_ctx.lock().await.turn_active = true;
        let previous_turn = p.event_ctx.lock().await.turn_id.clone();
        let (tx, mut rx) = broadcast::channel(32);
        handle_record(
            &native_record(
                "evt_previous_start",
                "session.execution.started",
                serde_json::json!({"sessionID":"root"}),
                Some(5),
            ),
            &p,
            &tx,
        )
        .await;
        handle_record(
            &native_record(
                "evt_previous",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
                Some(10),
            ),
            &p,
            &tx,
        )
        .await;
        while rx.try_recv().is_ok() {}
        reconcile_native(&server.url(), "fixture", &p, &tx)
            .await
            .unwrap();
        assert_eq!(p.event_ctx.lock().await.turn_id, previous_turn);
        assert!(!p.event_ctx.lock().await.turn_active);
        assert!(rx.try_recv().is_err());
        handle_record(
            &native_record(
                "evt_callback_start",
                "session.execution.started",
                serde_json::json!({"sessionID":"root"}),
                Some(11),
            ),
            &p,
            &tx,
        )
        .await;
        let resumed_turn = p.event_ctx.lock().await.turn_id.clone();
        assert_ne!(resumed_turn, previous_turn);
        assert!(p.event_ctx.lock().await.turn_active);
        handle_record(
            &native_record(
                "evt_callback_done",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
                Some(15),
            ),
            &p,
            &tx,
        )
        .await;
        assert!(!p.event_ctx.lock().await.turn_active);
        assert_eq!(
            std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|event| matches!(event,
                    ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &resumed_turn
                ))
                .count(),
            1
        );
        active.assert_async().await;
        history.assert_async().await;
    }

    #[tokio::test]
    async fn native_reconnect_settlement_during_checkpoint_does_not_arm_idle_root() {
        let mut server = mockito::Server::new_async().await;
        let _checkpoints = mock_watermarks(&mut server, "root", 10).await;
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let active = server
            .mock("GET", "/api/session/active")
            .with_status(200)
            .with_body_from_request(move |_| {
                if reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 2 {
                    r#"{"data":{"root":{}}}"#.into()
                } else {
                    r#"{"data":{}}"#.into()
                }
            })
            .expect(5)
            .create_async()
            .await;
        for path in [
            "/api/session?parentID=root&limit=200&order=asc",
            "/api/session/root/permission",
            "/api/session/root/form",
        ] {
            server
                .mock("GET", path)
                .with_status(200)
                .with_body(r#"{"data":[]}"#)
                .create_async()
                .await;
        }
        let history = server
            .mock("GET", "/api/session/root/message?limit=200&order=desc")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"msg_previous","type":"idle","outcome":"succeeded"}]}"#)
            .expect(2)
            .create_async()
            .await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        let previous_turn = p.event_ctx.lock().await.turn_id.clone();
        let (tx, mut rx) = broadcast::channel(16);
        reconcile_native(&server.url(), "fixture", &p, &tx)
            .await
            .unwrap();
        assert_eq!(p.event_ctx.lock().await.turn_id, previous_turn);
        assert!(!p.event_ctx.lock().await.turn_active);
        assert_eq!(
            p.router.lock().await.native.recovered_sequence("root"),
            Some(10)
        );
        assert!(rx.try_recv().is_err());
        active.assert_async().await;
        history.assert_async().await;
    }

    #[tokio::test]
    async fn native_reconnect_stale_active_scan_does_not_arm_an_idle_root() {
        let mut server = mockito::Server::new_async().await;
        let _checkpoints = mock_watermarks(&mut server, "root", 10).await;
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let active = server
            .mock("GET", "/api/session/active")
            .expect_at_least(1)
            .with_status(200)
            .with_body_from_request(move |_| {
                if reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                    r#"{"data":{"root":{}}}"#.into()
                } else {
                    r#"{"data":{}}"#.into()
                }
            })
            .expect(3)
            .create_async()
            .await;
        for path in [
            "/api/session?parentID=root&limit=200&order=asc",
            "/api/session/root/permission",
            "/api/session/root/form",
        ] {
            server
                .mock("GET", path)
                .with_status(200)
                .with_body(r#"{"data":[]}"#)
                .create_async()
                .await;
        }
        let history = server
            .mock("GET", "/api/session/root/message?limit=200&order=desc")
            .with_status(200)
            .with_body(r#"{"data":[{"id":"msg_previous","type":"idle","outcome":"succeeded"}]}"#)
            .create_async()
            .await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        let previous_turn = p.event_ctx.lock().await.turn_id.clone();
        let (tx, mut rx) = broadcast::channel(16);
        reconcile_native(&server.url(), "fixture", &p, &tx)
            .await
            .unwrap();
        assert_eq!(p.event_ctx.lock().await.turn_id, previous_turn);
        assert!(!p.event_ctx.lock().await.turn_active);
        assert_eq!(
            p.router.lock().await.native.recovered_sequence("root"),
            Some(10)
        );
        assert!(rx.try_recv().is_err());
        handle_record(
            &native_record(
                "evt_old_start",
                "session.execution.started",
                serde_json::json!({"sessionID":"root"}),
                Some(5),
            ),
            &p,
            &tx,
        )
        .await;
        handle_record(
            &native_record(
                "evt_previous",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
                Some(10),
            ),
            &p,
            &tx,
        )
        .await;
        assert!(rx.try_recv().is_err());
        active.assert_async().await;
        history.assert_async().await;
    }

    #[tokio::test]
    async fn native_recovery_uses_independent_content_ordinals_without_replaying_items() {
        let mut server = mockito::Server::new_async().await;
        let checkpoints = mock_watermarks(&mut server, "root", 10).await;
        let active = server
            .mock("GET", "/api/session/active")
            .expect_at_least(1)
            .with_status(200)
            .with_body("{\"data\":{\"root\":{}}}")
            .create_async()
            .await;
        let history = server.mock("GET", "/api/session/root/message?limit=200&order=desc")
            .with_status(200).with_body(serde_json::json!({"data":[{
                "id":"m","type":"assistant","time":{"completed":3},"content":[
                    {"type":"reasoning","text":"reason 0"},
                    {"type":"tool","id":"t","name":"read","state":{"status":"completed","input":{},"content":[]}},
                    {"type":"text","text":"text 0"},
                    {"type":"reasoning","text":"reason 1"},
                    {"type":"text","text":"text 1"}
                ]
            }]}).to_string()).create_async().await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.event_ctx.lock().await.turn_active = true;
        let (tx, mut rx) = broadcast::channel(32);
        for (kind, ordinal, text) in [
            ("reasoning", 0, "reason 0"),
            ("text", 0, "text 0"),
            ("reasoning", 1, "reason 1"),
            ("text", 1, "text 1"),
        ] {
            handle_record(&native_record(&format!("live-{kind}-{ordinal}"), &format!("session.{kind}.ended"),
                serde_json::json!({"sessionID":"root","assistantMessageID":"m","ordinal":ordinal,"text":text}), None), &p, &tx).await;
        }
        for (kind, data) in [
            (
                "session.tool.input.started",
                serde_json::json!({"name":"read"}),
            ),
            ("session.tool.called", serde_json::json!({"input":{}})),
            ("session.tool.success", serde_json::json!({"content":[]})),
        ] {
            let mut data = data;
            data["sessionID"] = serde_json::json!("root");
            data["assistantMessageID"] = serde_json::json!("m");
            data["id"] = serde_json::json!("t");
            handle_record(&native_record(kind, kind, data, None), &p, &tx).await;
        }
        let original: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(
            original
                .iter()
                .filter(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { .. }))
                .count(),
            6
        );
        reconcile_native_history(
            &reqwest::Client::new(),
            &server.url(),
            "fixture",
            &p,
            &tx,
            "root",
            true,
        )
        .await
        .unwrap();
        assert!(
            !std::iter::from_fn(|| rx.try_recv().ok())
                .any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { .. })),
            "mixed reasoning/text/tool history must retain the live fragment identities"
        );
        history.assert_async().await;
        active.assert_async().await;
        for checkpoint in checkpoints {
            checkpoint.assert_async().await;
        }
    }

    #[tokio::test]
    async fn native_recovery_deduplicates_buffered_child_execution_and_preserves_later_execution() {
        let mut server = mockito::Server::new_async().await;
        let checkpoints = mock_watermarks(&mut server, "child", 10).await;
        let active = server
            .mock("GET", "/api/session/active")
            .expect_at_least(1)
            .with_status(200)
            .with_body("{\"data\":{}}")
            .create_async()
            .await;
        let history = server.mock("GET", "/api/session/child/message?limit=200&order=desc")
            .with_status(200).with_body(serde_json::json!({"data":[
                {"id":"msg_child_done","type":"idle","outcome":"succeeded"},
                {"id":"m","type":"assistant","time":{"completed":3},"content":[
                    {"type":"text","text":"recovered child"},
                    {"type":"tool","id":"t","name":"read","state":{"status":"completed","input":{},"content":[]}}
                ]}
            ]}).to_string()).create_async().await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.router.lock().await.watched.insert("child".into());
        let (tx, mut rx) = broadcast::channel(64);
        reconcile_native_history(
            &reqwest::Client::new(),
            &server.url(),
            "fixture",
            &p,
            &tx,
            "child",
            false,
        )
        .await
        .unwrap();
        let mut observed: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        for (id, kind, data, seq) in [
            (
                "evt_old_start",
                "session.execution.started",
                serde_json::json!({"sessionID":"child"}),
                Some(5),
            ),
            (
                "evt_old_text",
                "session.text.ended",
                serde_json::json!({"sessionID":"child","assistantMessageID":"m","ordinal":0,"text":"recovered child"}),
                None,
            ),
            (
                "evt_old_tool",
                "session.tool.success",
                serde_json::json!({"sessionID":"child","assistantMessageID":"m","id":"t","content":[]}),
                None,
            ),
            (
                "evt_child_done",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"child"}),
                Some(10),
            ),
        ] {
            handle_record(&native_record(id, kind, data, seq), &p, &tx).await;
        }
        assert!(
            rx.try_recv().is_err(),
            "recovered child must not restart or repeat content/terminal during buffered replay"
        );
        for (id, kind, data, seq) in [
            (
                "evt_new_start",
                "session.execution.started",
                serde_json::json!({"sessionID":"child"}),
                Some(11),
            ),
            (
                "evt_late_old_text",
                "session.text.ended",
                serde_json::json!({"sessionID":"child","assistantMessageID":"m","ordinal":0,"text":"recovered child"}),
                None,
            ),
            (
                "evt_new_text",
                "session.text.ended",
                serde_json::json!({"sessionID":"child","assistantMessageID":"m2","ordinal":0,"text":"later child"}),
                None,
            ),
            (
                "evt_new_end",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"child"}),
                Some(15),
            ),
        ] {
            handle_record(&native_record(id, kind, data, seq), &p, &tx).await;
        }
        observed.extend(std::iter::from_fn(|| rx.try_recv().ok()));
        assert_eq!(
            observed
                .iter()
                .filter(
                    |e| matches!(e, ProviderRuntimeEvent::SubagentUpdated { subagent, .. }
            if subagent.status == crate::agent_provider::SubagentStatus::Running)
                )
                .count(),
            1
        );
        assert_eq!(
            observed
                .iter()
                .filter(
                    |e| matches!(e, ProviderRuntimeEvent::SubagentUpdated { subagent, .. }
            if subagent.status == crate::agent_provider::SubagentStatus::Completed)
                )
                .count(),
            2
        );
        for expected in ["recovered child", "later child"] {
            assert_eq!(observed.iter().filter(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted {
                item: crate::agent_provider::CompletedItem::AssistantText { text }, .. } if text == expected)).count(), 1);
        }
        history.assert_async().await;
        active.assert_async().await;
        for checkpoint in checkpoints {
            checkpoint.assert_async().await;
        }
    }

    #[tokio::test]
    async fn native_recovery_buffered_old_execution_does_not_complete_the_next_queued_turn() {
        let mut server = mockito::Server::new_async().await;
        let checkpoints = mock_watermarks(&mut server, "root", 10).await;
        let active = server
            .mock("GET", "/api/session/active")
            .expect_at_least(1)
            .with_status(200)
            .with_body("{\"data\":{}}")
            .expect(4)
            .create_async()
            .await;
        let prompts = server
            .mock("POST", "/api/session/root/prompt")
            .with_status(200)
            .expect(2)
            .create_async()
            .await;
        let interrupt = server
            .mock("POST", "/api/session/root/interrupt?resume=false")
            .with_status(200)
            .create_async()
            .await;
        let history = server.mock("GET", "/api/session/root/message?limit=200&order=desc")
            .with_status(200).with_body(serde_json::json!({"data":[
                {"id":"msg_a_done","type":"idle","outcome":"succeeded"},
                {"id":"m_a","type":"assistant","time":{"completed":3},"content":[{"type":"text","text":"A"}]}
            ]}).to_string()).expect(2).create_async().await;
        let (session, p, tx, mut rx) =
            super::super::session::tests::mock_native_peer(server.url()).await;
        let input = |text: &str| {
            serde_json::from_value(
                serde_json::json!({"thread_id":"t1","text":text,"model_override":null}),
            )
            .unwrap()
        };
        let a = session.enqueue_or_send(input("A")).await.unwrap();
        let b = session.enqueue_or_send(input("B")).await.unwrap();
        assert!(b.queued_id.is_some());
        reconcile_native_history(
            &reqwest::Client::new(),
            &server.url(),
            "fixture",
            &p,
            &tx,
            "root",
            false,
        )
        .await
        .unwrap();
        let mut observed = Vec::new();
        let b_turn = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = rx.recv().await.unwrap();
                let dispatched = match &event {
                    ProviderRuntimeEvent::QueuedTurnDispatched { turn_id, .. } => {
                        Some(turn_id.clone())
                    }
                    _ => None,
                };
                observed.push(event);
                if let Some(turn) = dispatched {
                    break turn;
                }
            }
        })
        .await
        .expect("the real queue worker must dispatch B after recovering A");
        handle_record(
            &native_record(
                "evt_b_start",
                "session.execution.started",
                serde_json::json!({"sessionID":"root"}),
                Some(11),
            ),
            &p,
            &tx,
        )
        .await;
        // A stale idle snapshot and buffered A events must both preserve B.
        reconcile_native_history(
            &reqwest::Client::new(),
            &server.url(),
            "fixture",
            &p,
            &tx,
            "root",
            false,
        )
        .await
        .unwrap();
        handle_record(
            &native_record(
                "evt_a_start",
                "session.execution.started",
                serde_json::json!({"sessionID":"root"}),
                Some(5),
            ),
            &p,
            &tx,
        )
        .await;
        handle_record(
            &native_record(
                "evt_a_done",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
                Some(10),
            ),
            &p,
            &tx,
        )
        .await;
        assert!(session.turn_active().await);
        assert_eq!(p.event_ctx.lock().await.turn_id, b_turn);
        handle_record(
            &native_record(
                "evt_b_done",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
                Some(15),
            ),
            &p,
            &tx,
        )
        .await;
        observed.extend(std::iter::from_fn(|| rx.try_recv().ok()));
        for turn in [&a.turn_id, &b_turn] {
            assert_eq!(observed.iter().filter(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == turn)).count(), 1);
        }
        prompts.assert_async().await;
        history.assert_async().await;
        active.assert_async().await;
        for checkpoint in checkpoints {
            checkpoint.assert_async().await;
        }
        session.shutdown().await;
        interrupt.assert_async().await;
    }

    #[tokio::test]
    async fn native_recovery_without_a_durable_idle_does_not_settle_pending_admission() {
        let mut server = mockito::Server::new_async().await;
        let checkpoint = server
            .mock("GET", "/api/experimental/session/root/log?follow=false")
            .with_status(200)
            .with_body(format!(
                "data: {}\n\n",
                serde_json::json!({
                    "type":"log.synced","aggregateID":"root"
                })
            ))
            .expect(2)
            .create_async()
            .await;
        let active = server
            .mock("GET", "/api/session/active")
            .expect_at_least(1)
            .with_status(200)
            .with_body("{\"data\":{}}")
            .create_async()
            .await;
        let history = server
            .mock("GET", "/api/session/root/message?limit=200&order=desc")
            .with_status(200)
            .with_body("{\"data\":[]}")
            .create_async()
            .await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.event_ctx.lock().await.turn_active = true;
        let (tx, mut rx) = broadcast::channel(16);
        reconcile_native_history(
            &reqwest::Client::new(),
            &server.url(),
            "fixture",
            &p,
            &tx,
            "root",
            false,
        )
        .await
        .unwrap();
        assert!(p.event_ctx.lock().await.turn_active);
        assert!(rx.try_recv().is_err());
        history.assert_async().await;
        active.assert_async().await;
        checkpoint.assert_async().await;
    }

    #[tokio::test]
    async fn native_recovery_advancing_windows_preserve_live_lifecycle_without_failing() {
        let mut server = mockito::Server::new_async().await;
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(1));
        let log = server
            .mock(
                "GET",
                mockito::Matcher::Regex(
                    "^/api/experimental/session/child/log\\?follow=false(?:&after=[0-9]+)?$".into(),
                ),
            )
            .with_status(200)
            .with_body_from_request(move |_| {
                watermark_record(
                    "child",
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                )
                .into_bytes()
            })
            .expect(6)
            .create_async()
            .await;
        let active = server
            .mock("GET", "/api/session/active")
            .expect_at_least(1)
            .with_status(200)
            .with_body("{\"data\":{}}")
            .expect(6)
            .create_async()
            .await;
        let history = server.mock("GET", "/api/session/child/message?limit=200&order=desc")
            .with_status(200).with_body(serde_json::json!({"data":[
                {"id":"msg_done","type":"idle","outcome":"succeeded"},
                {"id":"m","type":"assistant","time":{"completed":3},"content":[{"type":"text","text":"child"}]}
            ]}).to_string()).expect(3).create_async().await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.router.lock().await.watched.insert("child".into());
        let (tx, mut rx) = broadcast::channel(32);
        reconcile_native_history(
            &reqwest::Client::new(),
            &server.url(),
            "fixture",
            &p,
            &tx,
            "child",
            false,
        )
        .await
        .unwrap();
        assert!(p
            .router
            .lock()
            .await
            .native
            .recovered_sequence("child")
            .is_none());
        let mut observed: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(!observed.iter().any(
            |e| matches!(e, ProviderRuntimeEvent::SubagentUpdated { subagent, .. }
            if subagent.status == crate::agent_provider::SubagentStatus::Completed)
        ));
        handle_record(
            &native_record(
                "evt_start",
                "session.execution.started",
                serde_json::json!({"sessionID":"child"}),
                Some(5),
            ),
            &p,
            &tx,
        )
        .await;
        handle_record(
            &native_record(
                "evt_done",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"child"}),
                Some(10),
            ),
            &p,
            &tx,
        )
        .await;
        observed.extend(std::iter::from_fn(|| rx.try_recv().ok()));
        assert_eq!(
            observed
                .iter()
                .filter(
                    |e| matches!(e, ProviderRuntimeEvent::SubagentUpdated { subagent, .. }
            if subagent.status == crate::agent_provider::SubagentStatus::Completed)
                )
                .count(),
            1
        );
        log.assert_async().await;
        active.assert_async().await;
        history.assert_async().await;
    }

    #[tokio::test]
    async fn native_recovery_active_settlement_gap_preserves_the_live_terminal() {
        let mut server = mockito::Server::new_async().await;
        let checkpoints = mock_watermarks(&mut server, "root", 10).await;
        let active = server
            .mock("GET", "/api/session/active")
            .expect_at_least(1)
            .with_status(200)
            .with_body("{\"data\":{\"root\":{}}}")
            .create_async()
            .await;
        let history = server
            .mock("GET", "/api/session/root/message?limit=200&order=desc")
            .with_status(200)
            .with_body(
                serde_json::json!({"data":[
                    {"id":"msg_done","type":"idle","outcome":"succeeded"}
                ]})
                .to_string(),
            )
            .create_async()
            .await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.event_ctx.lock().await.turn_active = true;
        let (tx, mut rx) = broadcast::channel(16);
        handle_record(
            &native_record(
                "evt_start",
                "session.execution.started",
                serde_json::json!({"sessionID":"root"}),
                Some(5),
            ),
            &p,
            &tx,
        )
        .await;
        reconcile_native_history(
            &reqwest::Client::new(),
            &server.url(),
            "fixture",
            &p,
            &tx,
            "root",
            true,
        )
        .await
        .unwrap();
        assert!(p
            .router
            .lock()
            .await
            .native
            .recovered_sequence("root")
            .is_none());
        assert!(p.event_ctx.lock().await.turn_active);
        handle_record(
            &native_record(
                "evt_done",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
                Some(10),
            ),
            &p,
            &tx,
        )
        .await;
        assert!(!p.event_ctx.lock().await.turn_active);
        assert_eq!(
            std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
                .count(),
            1
        );
        active.assert_async().await;
        history.assert_async().await;
        for checkpoint in checkpoints {
            checkpoint.assert_async().await;
        }
    }

    #[tokio::test]
    async fn legacy_aborted_error_idle_and_late_assistant_error_settle_once() {
        let p = peer("root");
        p.event_ctx.lock().await.turn_active = true;
        let (tx, mut rx) = broadcast::channel(16);
        let error = r#"data: {"type":"session.error","properties":{"sessionID":"root","error":{"name":"MessageAbortedError","data":{"message":"Aborted"}}}}"#;
        let idle = r#"data: {"type":"session.idle","properties":{"sessionID":"root"}}"#;
        let assistant = r#"data: {"type":"message.updated","properties":{"info":{"id":"m","sessionID":"root","role":"assistant","error":{"name":"MessageAbortedError","data":{"message":"Aborted"}}}}}"#;
        handle_record(error, &p, &tx).await;
        handle_record(idle, &p, &tx).await;
        handle_record(assistant, &p, &tx).await;
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let terminals: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
            .collect();
        assert_eq!(
            terminals.len(),
            1,
            "one legacy terminal, not session.error + idle + assistant.error"
        );
        assert!(matches!(
            terminals[0],
            ProviderRuntimeEvent::TurnCompleted {
                status: crate::agent_provider::TurnStatus::Error { .. },
                ..
            }
        ));
        assert!(!p.event_ctx.lock().await.turn_active);
    }

    #[tokio::test]
    async fn legacy_repeated_idle_does_not_emit_a_second_turn_terminal() {
        let p = peer("root");
        p.event_ctx.lock().await.turn_active = true;
        let (tx, mut rx) = broadcast::channel(8);
        let record =
            "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"root\"}}\n\n";
        handle_record(record, &p, &tx).await;
        handle_record(record, &p, &tx).await;
        assert_eq!(
            std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn legacy_abort_idle_is_interrupted_not_success_for_the_selected_turn() {
        let p = peer("root");
        let mut ctx = p.event_ctx.lock().await;
        ctx.turn_active = true;
        let turn = ctx.turn_id.clone();
        drop(ctx);
        p.router.lock().await.mark_interrupted(Some(turn));
        let (tx, mut rx) = broadcast::channel(8);
        handle_record(
            "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"root\"}}\n\n",
            &p,
            &tx,
        )
        .await;
        assert!(
            matches!(rx.try_recv().unwrap(),ProviderRuntimeEvent::TurnCompleted{status:crate::agent_provider::TurnStatus::Error{subtype,..},..} if subtype=="interrupted")
        );
    }

    #[tokio::test]
    async fn native_reconnect_recovers_completed_output_terminal_and_child_question() {
        let mut server = mockito::Server::new_async().await;
        let specs = [
            ("/api/session/active", serde_json::json!({"data":{}})),
            (
                "/api/session?parentID=root&limit=200&order=asc",
                serde_json::json!({"data":[{"id":"child","parentID":"root"}]}),
            ),
            (
                "/api/session?parentID=child&limit=200&order=asc",
                serde_json::json!({"data":[]}),
            ),
            (
                "/api/session/root/permission",
                serde_json::json!({"data":[]}),
            ),
            ("/api/session/root/form", serde_json::json!({"data":[]})),
            (
                "/api/session/child/permission",
                serde_json::json!({"data":[]}),
            ),
            (
                "/api/session/child/form",
                serde_json::json!({"data":[{"id":"f","sessionID":"child","metadata":{"kind":"question"},"fields":[{"key":"q0","type":"string","description":"Pick","custom":true,"options":[]}]}]}),
            ),
            (
                "/api/session/child/message?limit=200&order=desc",
                serde_json::json!({"data":[]}),
            ),
            (
                "/api/session/root/message?limit=200&order=desc",
                serde_json::json!({"data":[{"id":"msg_root_done","type":"idle","outcome":"succeeded"},{"id":"m","type":"assistant","time":{"completed":3},"model":{"providerID":"p","id":"m"},"content":[{"type":"text","text":"recovered"},{"type":"tool","id":"t","name":"read","state":{"status":"completed","input":{"path":"x"},"content":[{"type":"text","text":"ok"}]}}]}]}),
            ),
        ];
        let mut mocks = Vec::new();
        for (path, body) in specs {
            mocks.push(
                server
                    .mock("GET", path)
                    .with_status(200)
                    .with_body(body.to_string())
                    .expect_at_least(1)
                    .create_async()
                    .await,
            );
        }
        mocks.extend(mock_watermarks(&mut server, "root", 10).await);
        mocks.extend(mock_watermarks(&mut server, "child", 0).await);
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.event_ctx.lock().await.turn_active = true;
        let (tx, mut rx) = broadcast::channel(32);
        reconcile_native(&server.url(), "fixture", &p, &tx)
            .await
            .unwrap();
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().any(|e|matches!(e,ProviderRuntimeEvent::RequestOpened{subagent_id:Some(s),request_kind,..} if s=="child" && request_kind=="user-input")));
        assert!(events.iter().any(|e|matches!(e,ProviderRuntimeEvent::ItemCompleted{item:crate::agent_provider::CompletedItem::AssistantText{text},..} if text=="recovered")));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
                .count(),
            1
        );
        assert!(!p.event_ctx.lock().await.turn_active);
        for mock in mocks {
            mock.assert_async().await;
        }
    }

    #[tokio::test]
    async fn native_reconnect_recovers_descendants_without_settling_root() {
        for (root_active, live_terminal_first) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let mut server = mockito::Server::new_async().await;
            let mut mocks = Vec::new();
            let mut specs = vec![
                (
                    "/api/session/active".to_string(),
                    serde_json::json!({"data":if root_active {serde_json::json!({"root":{}})} else {serde_json::json!({})}}),
                ),
                (
                    "/api/session?parentID=root&limit=200&order=asc".into(),
                    serde_json::json!({"data":[{"id":"child","parentID":"root"}]}),
                ),
                (
                    "/api/session?parentID=child&limit=200&order=asc".into(),
                    serde_json::json!({"data":[{"id":"grandchild","parentID":"child"}]}),
                ),
                (
                    "/api/session?parentID=grandchild&limit=200&order=asc".into(),
                    serde_json::json!({"data":[]}),
                ),
                (
                    "/api/session/root/message?limit=200&order=desc".into(),
                    serde_json::json!({"data":[]}),
                ),
            ];
            for session in ["root", "child", "grandchild"] {
                specs.push((
                    format!("/api/session/{session}/permission"),
                    serde_json::json!({"data":[]}),
                ));
                specs.push((format!("/api/session/{session}/form"), serde_json::json!({"data":if session=="grandchild" {serde_json::json!([{"id":"f","sessionID":session,"metadata":{"kind":"question"},"fields":[{"key":"q0","type":"string","description":"Pick","custom":true,"options":[]}]}])} else {serde_json::json!([])}})));
            }
            for (session, outcome) in [("child", "succeeded"), ("grandchild", "failed")] {
                specs.push((format!("/api/session/{session}/message?limit=200&order=desc"), serde_json::json!({"data":[{"id":format!("msg_{session}_done"),"type":"idle","outcome":outcome},{"id":format!("m-{session}"),"type":"assistant","time":{"completed":3},"content":[{"type":"text","text":session}]}]})));
            }
            for (path, body) in specs {
                mocks.push(
                    server
                        .mock("GET", path.as_str())
                        .with_status(200)
                        .with_body(body.to_string())
                        .expect_at_least(0)
                        .create_async()
                        .await,
                );
            }
            for session in ["root", "child", "grandchild"] {
                mocks.extend(mock_watermarks(&mut server, session, 10).await);
            }
            let mut p = peer("root");
            p.api = super::super::api::OpenCodeApi::V2;
            p.event_ctx.lock().await.turn_active = root_active;
            // The child started before the outage; the grandchild was created during it.
            p.router.lock().await.watched.insert("child".into());
            p.router.lock().await.native.mark_running("child");
            let (tx, mut rx) = broadcast::channel(64);
            if live_terminal_first {
                handle_record(
                    &native_record(
                        "evt_child_done",
                        "session.execution.succeeded",
                        serde_json::json!({"sessionID":"child"}),
                        Some(10),
                    ),
                    &p,
                    &tx,
                )
                .await;
            }
            reconcile_native(&server.url(), "fixture", &p, &tx)
                .await
                .unwrap();
            let mut events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(
                        e,
                        ProviderRuntimeEvent::ItemCompleted {
                            subagent_id: Some(_),
                            item: crate::agent_provider::CompletedItem::AssistantText { .. },
                            ..
                        }
                    ))
                    .count(),
                2,
                "snapshot must recover descendant output before live replay"
            );
            reconcile_native(&server.url(), "fixture", &p, &tx)
                .await
                .unwrap();
            for session in ["child", "grandchild"] {
                handle_record(&format!("data: {}\n\n", serde_json::json!({"id":format!("live-content-{session}"),"type":"session.text.ended","data":{"sessionID":session,"assistantMessageID":format!("m-{session}"),"ordinal":0,"text":session}})), &p, &tx).await;
                handle_record(&format!("data: {}\n\n", serde_json::json!({"id":format!("live-end-{session}"),"type":if session=="child" {"session.execution.succeeded"} else {"session.execution.failed"},"data":{"sessionID":session}})), &p, &tx).await;
            }
            handle_record("data: {\"id\":\"live-form\",\"type\":\"form.created\",\"data\":{\"form\":{\"id\":\"f\",\"sessionID\":\"grandchild\",\"metadata\":{\"kind\":\"question\"},\"fields\":[{\"key\":\"q0\",\"type\":\"string\",\"description\":\"Pick\",\"custom\":true,\"options\":[]}]}}}\n\n", &p, &tx).await;
            events.extend(std::iter::from_fn(|| rx.try_recv().ok()));
            for session in ["child", "grandchild"] {
                assert_eq!(events.iter().filter(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { subagent_id:Some(s), item:crate::agent_provider::CompletedItem::AssistantText { text }, .. } if s==session && text==session)).count(), 1, "missed descendant output recovered once");
                assert_eq!(events.iter().filter(|e| matches!(e, ProviderRuntimeEvent::SubagentUpdated { subagent, .. } if subagent.subagent_id==session && subagent.status == if session=="child" {crate::agent_provider::SubagentStatus::Completed} else {crate::agent_provider::SubagentStatus::Failed})).count(), 1, "descendant terminal recovered once");
            }
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, ProviderRuntimeEvent::RequestOpened { .. }))
                    .count(),
                1
            );
            assert!(!events
                .iter()
                .any(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. })));
            assert_eq!(p.event_ctx.lock().await.turn_active, root_active);
        }
    }

    #[tokio::test]
    async fn native_reconnect_failed_tools_keep_native_error_without_content() {
        for content in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!([{"type":"text","text":"explicit result"}])),
        ] {
            let mut server = mockito::Server::new_async().await;
            let checkpoints = mock_watermarks(&mut server, "root", 10).await;
            let mut state = serde_json::json!({"status":"error","input":{"path":"x"},"error":{"type":"ToolError","message":"native failure"}});
            if let Some(content) = content.clone() {
                state["content"] = content;
            }
            let specs = [
                (
                    "/api/session/active",
                    serde_json::json!({"data":{"root":{}}}),
                ),
                (
                    "/api/session?parentID=root&limit=200&order=asc",
                    serde_json::json!({"data":[]}),
                ),
                (
                    "/api/session/root/permission",
                    serde_json::json!({"data":[]}),
                ),
                ("/api/session/root/form", serde_json::json!({"data":[]})),
                (
                    "/api/session/root/message?limit=200&order=desc",
                    serde_json::json!({"data":[{"id":"m","type":"assistant","time":{"completed":3},"content":[{"type":"tool","id":"t","name":"read","state":state}]}]}),
                ),
            ];
            let mut mocks = Vec::new();
            for (path, body) in specs {
                mocks.push(
                    server
                        .mock("GET", path)
                        .with_status(200)
                        .with_body(body.to_string())
                        .expect_at_least(1)
                        .create_async()
                        .await,
                );
            }
            let mut p = peer("root");
            p.api = super::super::api::OpenCodeApi::V2;
            p.event_ctx.lock().await.turn_active = true;
            let (tx, mut rx) = broadcast::channel(32);
            reconcile_native(&server.url(), "fixture", &p, &tx)
                .await
                .unwrap();
            let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            let results: Vec<_> = events
                .iter()
                .filter_map(|e| match e {
                    ProviderRuntimeEvent::ItemCompleted {
                        item:
                            crate::agent_provider::CompletedItem::ToolResult {
                                content, is_error, ..
                            },
                        ..
                    } => Some((content, is_error)),
                    _ => None,
                })
                .collect();
            assert_eq!(results.len(), 1);
            assert!(*results[0].1);
            assert_eq!(
                *results[0].0,
                content.filter(|v| !v.is_null()).unwrap_or_else(
                    || serde_json::json!([{"type":"text","text":"native failure"}])
                )
            );
            assert!(p.event_ctx.lock().await.turn_active);
            for mock in mocks {
                mock.assert_async().await;
            }
            for checkpoint in checkpoints {
                checkpoint.assert_async().await;
            }
        }
        // Live native failures use the same fallback, not just the snapshot adapter.
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.event_ctx.lock().await.turn_active = true;
        let (tx, mut rx) = broadcast::channel(16);
        handle_record(&format!("data: {}\n\n", serde_json::json!({"id":"failed","type":"session.tool.failed","data":{"sessionID":"root","assistantMessageID":"m","id":"t","content":null,"error":{"message":"live failure"}}})), &p, &tx).await;
        assert!(std::iter::from_fn(||rx.try_recv().ok()).any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { item:crate::agent_provider::CompletedItem::ToolResult { content, is_error:true, .. }, .. } if content==serde_json::json!([{"type":"text","text":"live failure"}]))));
    }

    #[tokio::test]
    async fn native_connected_gates_readiness_and_child_requests_keep_their_owner() {
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        let (ready, rx_ready) = watch::channel(false);
        p.ready = ready;
        let (tx, mut rx) = broadcast::channel(32);
        p.event_ctx.lock().await.turn_active = true;
        let record = |id: &str, kind: &str, data: serde_json::Value| {
            format!(
                "data: {}\n\n",
                serde_json::json!({"id":id,"type":kind,"data":data})
            )
        };
        handle_record(
            &record("connected", "server.connected", serde_json::json!({})),
            &p,
            &tx,
        )
        .await;
        assert!(
            *rx_ready.borrow(),
            "observed server.connected must unblock dispatch"
        );
        handle_record(
            &record(
                "child",
                "session.created",
                serde_json::json!({"sessionID":"child","parentID":"root"}),
            ),
            &p,
            &tx,
        )
        .await;
        handle_record(&record("ask","permission.asked",serde_json::json!({"id":"req","sessionID":"child","action":"edit","resources":["x"]})),&p,&tx).await;
        assert_eq!(
            p.router
                .lock()
                .await
                .session_for_permission("req")
                .as_deref(),
            Some("child")
        );
        assert!(std::iter::from_fn(||rx.try_recv().ok()).any(|e|matches!(e,ProviderRuntimeEvent::RequestOpened{subagent_id:Some(id),..} if id=="child")));
        handle_record(
            &record(
                "start",
                "session.execution.started",
                serde_json::json!({"sessionID":"root"}),
            ),
            &p,
            &tx,
        )
        .await;
        handle_record(
            &record(
                "end",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
            ),
            &p,
            &tx,
        )
        .await;
        handle_record(
            &record(
                "idle",
                "session.idle",
                serde_json::json!({"sessionID":"root"}),
            ),
            &p,
            &tx,
        )
        .await;
        handle_record(
            &record(
                "end2",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
            ),
            &p,
            &tx,
        )
        .await;
        assert_eq!(
            std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
                .count(),
            1
        );
        assert!(!p.event_ctx.lock().await.turn_active);
    }

    #[tokio::test]
    async fn native_live_root_start_resumes_after_parent_completion() {
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.event_ctx.lock().await.turn_active = true;
        let initial = p.event_ctx.lock().await.turn_id.clone();
        let (tx, mut rx) = broadcast::channel(32);
        let record = |id: &str, kind: &str, data: serde_json::Value| {
            format!(
                "data: {}\n\n",
                serde_json::json!({"id":id,"type":kind,"data":data})
            )
        };
        handle_record(
            &record(
                "first-start",
                "session.execution.started",
                serde_json::json!({"sessionID":"root"}),
            ),
            &p,
            &tx,
        )
        .await;
        assert_eq!(
            p.event_ctx.lock().await.turn_id,
            initial,
            "an already-admitted user turn keeps its ID"
        );
        handle_record(
            &record(
                "first-end",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
            ),
            &p,
            &tx,
        )
        .await;
        assert!(!p.event_ctx.lock().await.turn_active);
        let first: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(first.iter().any(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &initial)));

        handle_record(&record("cold-text", "session.text.ended", serde_json::json!({"sessionID":"root","assistantMessageID":"cold","ordinal":0,"text":"old history"})), &p, &tx).await;
        assert!(
            rx.try_recv().is_err(),
            "content alone does not admit a live execution"
        );
        handle_record(
            &record(
                "callback-start",
                "session.execution.started",
                serde_json::json!({"sessionID":"root"}),
            ),
            &p,
            &tx,
        )
        .await;
        let resumed = p.event_ctx.lock().await.turn_id.clone();
        assert_ne!(resumed, initial);
        assert!(p.event_ctx.lock().await.turn_active);
        handle_record(&record("callback-text", "session.text.ended", serde_json::json!({"sessionID":"root","assistantMessageID":"follow-up","ordinal":0,"text":"child result received"})), &p, &tx).await;
        handle_record(
            &record(
                "callback-end",
                "session.execution.succeeded",
                serde_json::json!({"sessionID":"root"}),
            ),
            &p,
            &tx,
        )
        .await;
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().any(|event| matches!(event, ProviderRuntimeEvent::SessionStateChanged { status: crate::agent_provider::SessionStatus::Running { active_turn }, .. } if active_turn == &resumed)));
        assert!(events.iter().any(|event| matches!(event, ProviderRuntimeEvent::ItemCompleted { turn_id, item: crate::agent_provider::CompletedItem::AssistantText { text }, .. } if turn_id == &resumed && text == "child result received")));
        assert_eq!(events.iter().filter(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == &resumed)).count(), 1);
        assert!(!p.event_ctx.lock().await.turn_active);
    }

    #[tokio::test]
    async fn native_reconciliation_failures_exhaust_the_reconnect_budget() {
        let mut server = mockito::Server::new_async().await;
        let connections = server
            .mock("GET", "/api/event")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"id\":\"connected\",\"type\":\"server.connected\",\"data\":{}}\n\n")
            .expect((MAX_RECONNECT_ATTEMPTS + 1) as usize)
            .create_async()
            .await;
        let snapshots = server
            .mock("GET", "/api/session/active")
            .with_status(503)
            .expect((MAX_RECONNECT_ATTEMPTS + 1) as usize)
            .create_async()
            .await;
        let mut p = peer("root");
        p.api = super::super::api::OpenCodeApi::V2;
        p.ready = watch::channel(false).0;
        p.event_ctx.lock().await.turn_active = true;
        let (tx, mut rx) = broadcast::channel(64);
        tokio::time::timeout(
            Duration::from_secs(35),
            run_sse_listener(server.url(), "fixture".into(), p.clone(), tx),
        )
        .await
        .expect("failed reconciliation must have a finite retry budget");
        assert!(p.dead.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!*p.ready.borrow());
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(event, ProviderRuntimeEvent::RuntimeWarning { message, .. } if message.contains("giving_up"))));
        connections.assert_async().await;
        snapshots.assert_async().await;
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
        OpenCodeEvent::Known(KnownEvent::MessagePartDelta(
            super::super::protocol::MessagePartDelta {
                session_id: session_id.into(),
                message_id: "m1".into(),
                part_id: "p1".into(),
                field: "text".into(),
                delta: "x".into(),
            },
        ))
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
        assert_eq!(
            resolve_routing(&delta_event("stranger"), &router).await,
            None
        );
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
            router
                .lock()
                .await
                .session_for_permission("per_1")
                .as_deref(),
            Some("child")
        );
    }

    #[tokio::test]
    async fn queue_boundary_waits_for_legacy_idle_and_commits_state_before_ready() {
        let (tx, mut rx) = broadcast::channel(8);
        let peer = peer("s1");
        peer.event_ctx.lock().await.turn_active = true;
        handle_record("data: {\"type\":\"session.status\",\"properties\":{\"sessionID\":\"s1\",\"status\":{\"type\":\"idle\"}}}\n\n", &peer, &tx).await;
        assert!(rx.try_recv().is_err());
        assert!(peer.event_ctx.lock().await.turn_active);
        handle_record(
            "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"s1\"}}\n\n",
            &peer,
            &tx,
        )
        .await;
        assert!(!peer.event_ctx.lock().await.turn_active);
        assert!(matches!(
            rx.try_recv().unwrap(),
            ProviderRuntimeEvent::TurnCompleted { .. }
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            ProviderRuntimeEvent::SessionStateChanged {
                status: crate::agent_provider::SessionStatus::Ready,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn handle_record_publishes_translated_event_for_matching_session() {
        let (tx, mut rx) = broadcast::channel(8);
        let peer = peer("s1");
        let record = "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"s1\",\"messageID\":\"m1\",\"partID\":\"p1\",\"field\":\"text\",\"delta\":\"hi\"}}\n\n";
        handle_record(record, &peer, &tx).await;
        let event = rx.try_recv().expect("event published");
        match event {
            ProviderRuntimeEvent::ContentDelta {
                delta, subagent_id, ..
            } => {
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
