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
    drop(router);
    let mut usage = peer.usage.lock().await;
    let mut events = Vec::new();
    for update in updates {
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
        .any(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
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
    let root = &peer.session_id;
    let running = active["data"].get(root).is_some();
    if running {
        let mut ctx = peer.event_ctx.lock().await;
        ctx.turn_active = true;
        peer.router.lock().await.native.mark_running(root);
        let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ctx.thread_id.clone(),
            status: crate::agent_provider::SessionStatus::Running {
                active_turn: ctx.turn_id.clone(),
            },
        });
    }
    let sessions = peer.router.lock().await.watched_sessions();
    for session in sessions {
        let session_running = active["data"].get(&session).is_some();
        if session != peer.session_id || peer.event_ctx.lock().await.turn_active {
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
    running: bool,
) -> Result<(), String> {
    // Recover completed content from the current/latest execution, stopping
    // at its preceding durable idle boundary. Pagination follows cursor,
    // with no order parameter on cursor pages.
    let encoded = urlencoding::encode(session);
    let mut messages = Vec::new();
    let mut cursor: Option<String> = None;
    let mut boundaries = 0;
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
                if boundaries > if running { 0 } else { 1 } {
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
    for message in messages
        .into_iter()
        .rev()
        .filter(|m| m["type"] == "assistant")
    {
        let id = message["id"]
            .as_str()
            .ok_or("missing assistant message id")?;
        handle_native(super::v2::Event{id:format!("snapshot:step:{id}"),kind:"session.step.started".into(),data:serde_json::json!({"sessionID":session,"assistantMessageID":id,"model":message["model"]})},peer,event_tx).await;
        for (ordinal, part) in message["content"]
            .as_array()
            .ok_or("missing assistant content")?
            .iter()
            .enumerate()
        {
            if part["type"] == "tool" {
                let tool_id = part["id"].as_str().ok_or("missing tool id")?;
                let base =
                    serde_json::json!({"sessionID":session,"assistantMessageID":id,"id":tool_id});
                let mut started = base.clone();
                started["name"] = part["name"].clone();
                handle_native(
                    super::v2::Event {
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
            let prefix = match part["type"].as_str() {
                Some("text") => "session.text",
                Some("reasoning") => "session.reasoning",
                _ => continue,
            };
            // A still-streaming message can race buffered live deltas.
            // Do not append its partial snapshot or prematurely end it;
            // the authoritative ended event restores the full segment.
            if !message["time"]["completed"].is_number() {
                continue;
            }
            let suffix = "ended";
            handle_native(super::v2::Event{id:format!("snapshot:content:{id}:{ordinal}"),kind:format!("{prefix}.{suffix}"),data:serde_json::json!({"sessionID":session,"assistantMessageID":id,"ordinal":ordinal,"text":part["text"]})},peer,event_tx).await;
        }
        if message.get("tokens").is_some() {
            handle_native(super::v2::Event{id:format!("snapshot:usage:{id}"),kind:"session.step.ended".into(),data:serde_json::json!({"sessionID":session,"assistantMessageID":id,"tokens":message["tokens"],"cost":message["cost"]})},peer,event_tx).await;
        }
    }
    if !running {
        let info = native_get(client, base, password, &format!("/api/session/{encoded}")).await?;
        let boundary = info["data"]["time"]["idle"].clone();
        let mut router = peer.router.lock().await;
        if session != peer.session_id && !router.native.recover_terminal(session, boundary) {
            return Ok(());
        }
        router.native.mark_running(session);
        drop(router);
        let outcome = info["data"]["outcome"].as_str().unwrap_or("interrupted");
        handle_native(
            super::v2::Event {
                id: format!("snapshot:terminal:{}", info["data"]["time"]["idle"]),
                kind: format!("session.execution.{outcome}"),
                data: serde_json::json!({"sessionID":session}),
            },
            peer,
            event_tx,
        )
        .await;
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
                "/api/session/child",
                serde_json::json!({"data":{"id":"child","outcome":"succeeded","time":{"idle":3}}}),
            ),
            (
                "/api/session/root/message?limit=200&order=desc",
                serde_json::json!({"data":[{"id":"idle","type":"idle","outcome":"succeeded"},{"id":"m","type":"assistant","time":{"completed":3},"model":{"providerID":"p","id":"m"},"content":[{"type":"text","text":"recovered"},{"type":"tool","id":"t","name":"read","state":{"status":"completed","input":{"path":"x"},"content":[{"type":"text","text":"ok"}]}}]}]}),
            ),
            (
                "/api/session/root",
                serde_json::json!({"data":{"id":"root","outcome":"succeeded","time":{"idle":3}}}),
            ),
        ];
        let mut mocks = Vec::new();
        for (path, body) in specs {
            mocks.push(
                server
                    .mock("GET", path)
                    .with_status(200)
                    .with_body(body.to_string())
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
                specs.push((format!("/api/session/{session}/message?limit=200&order=desc"), serde_json::json!({"data":[{"type":"idle"},{"id":format!("m-{session}"),"type":"assistant","time":{"completed":3},"content":[{"type":"text","text":session}]}]})));
                specs.push((
                    format!("/api/session/{session}"),
                    serde_json::json!({"data":{"id":session,"outcome":outcome,"time":{"idle":3}}}),
                ));
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
            let mut p = peer("root");
            p.api = super::super::api::OpenCodeApi::V2;
            p.event_ctx.lock().await.turn_active = root_active;
            // The child started before the outage; the grandchild was created during it.
            p.router.lock().await.watched.insert("child".into());
            p.router.lock().await.native.mark_running("child");
            let (tx, mut rx) = broadcast::channel(64);
            if live_terminal_first {
                handle_record("data: {\"id\":\"early-terminal\",\"type\":\"session.execution.succeeded\",\"data\":{\"sessionID\":\"child\"}}\n\n", &p, &tx).await;
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
