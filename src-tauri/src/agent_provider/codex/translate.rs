//! Pure translation layer from Codex protocol messages to the canonical
//! [`ProviderRuntimeEvent`] enum.
//!
//! Keeping this logic in one place (and free of I/O) means the whole
//! protocol-to-event mapping is unit-testable without spawning a
//! subprocess. Every function here is a pure transform — callers are
//! responsible for broadcasting the returned events.
//!
//! # Mapping policy
//!
//! * Streaming text (`agentMessage`, `commandExecution.outputDelta`,
//!   `fileChange.outputDelta`, `plan.delta`) → [`ContentDelta::Text`].
//! * Reasoning streams (`reasoning.textDelta` / `reasoning.summaryTextDelta`)
//!   → [`ContentDelta::Thinking`].
//! * MCP tool progress (`mcpToolCall.progress`) → [`ContentDelta::ToolInput`]
//!   so the UI's tool-call card updates with status text.
//! * `item/started` for `commandExecution` / `fileChange` / `mcpToolCall` /
//!   `dynamicToolCall` → [`CompletedItem::ToolUse`] (the tool has been
//!   called; result lands later via `item/completed`).
//! * `item/completed` for the same set → [`CompletedItem::ToolResult`].
//! * `item/completed` for `agentMessage` / `reasoning` / `plan` → emit the
//!   final-form [`CompletedItem`] variant so the UI replaces the streaming
//!   placeholder.
//! * Anything else surfaces as [`ProviderRuntimeEvent::RuntimeWarning`]
//!   so adapter drift is observable instead of silent.

use std::collections::{HashMap, HashSet};

use crate::agent_provider::{
    CompletedItem, ContentDelta, ProviderRuntimeEvent, ProviderSessionId, RequestId,
    SessionStatus, SubagentSnapshot, SubagentStatus, ThreadId, TurnId, TurnStatus,
};

use super::protocol::{
    CollabAgentToolCallItem, DeltaParams, ErrorParams, FileChangePatchUpdatedParams, ItemEnvelope,
    McpToolCallProgressParams, NotificationMessage, ReasoningSummaryPartAddedParams,
    ReasoningSummaryTextDeltaParams, ReasoningTextDeltaParams, ServerRequestMessage,
    SubAgentActivityItem, TerminalInteractionParams, ThreadStartedParams, TurnCompletedParams,
    TurnStartedParams,
};

/// Per-session subagent demux state.
///
/// Codex spawns each sub-agent as its own `threadId` (multi-agent is
/// stable and enabled by default). Notifications for a child thread arrive
/// interleaved with the parent's on the same stream, discriminated only by
/// their wire `threadId`. This tracker records which thread ids belong to
/// sub-agents (rooted at this session's parent thread) so
/// [`translate_notification_with`] can:
///
/// * route child-thread `item/*` / delta events into the subagent's
///   sub-transcript (tagging the canonical events with
///   `subagent_id = child threadId`), and
/// * translate child thread/turn lifecycle into
///   [`SubagentUpdated`](ProviderRuntimeEvent::SubagentUpdated) status
///   changes rather than mutating the *parent* turn/session state.
///
/// A subagent is learned from either a `collabAgentToolCall`'s
/// `receiverThreadIds` or a child `thread/started` whose `parentThreadId`
/// is already known (the parent thread or another sub-agent).
#[derive(Debug, Clone, Default)]
pub struct CodexSubagentDemux {
    /// This session's own (parent) Codex thread id. Notifications bearing
    /// this thread id drive the parent transcript / session state.
    parent_thread_id: String,
    /// Every thread id known to be a sub-agent of this session.
    subagents: HashSet<String>,
    /// child thread id → the thread that spawned it (parent thread or a
    /// higher sub-agent). Retained for diagnostics / future nested UI.
    #[allow(dead_code)]
    child_to_parent: HashMap<String, String>,
}

impl CodexSubagentDemux {
    /// Create a demux rooted at the session's parent Codex thread id.
    pub fn new(parent_thread_id: impl Into<String>) -> Self {
        Self {
            parent_thread_id: parent_thread_id.into(),
            subagents: HashSet::new(),
            child_to_parent: HashMap::new(),
        }
    }

    /// Register `child` as a sub-agent spawned by `parent`.
    fn register(&mut self, child: &str, parent: &str) {
        if child.is_empty() {
            return;
        }
        self.subagents.insert(child.to_string());
        self.child_to_parent
            .insert(child.to_string(), parent.to_string());
    }

    /// Whether `thread_id` is a known sub-agent of this session.
    fn is_subagent(&self, thread_id: &str) -> bool {
        self.subagents.contains(thread_id)
    }
}

/// Translate a single notification into zero or more canonical events.
///
/// Stateless convenience wrapper used by unit tests and any caller that
/// does not need cross-message subagent demux. Internally spins up a
/// throwaway [`CodexSubagentDemux`] so a lone `collabAgentToolCall` still
/// maps to [`SubagentUpdated`](ProviderRuntimeEvent::SubagentUpdated), but
/// child-thread transcript routing (which needs the child registered by a
/// prior message) requires [`translate_notification_with`].
///
/// `thread_id` is the runtime-owned thread identifier (not Codex's own
/// `threadId` — those match semantically but are stored separately to keep
/// the canonical event stream self-contained).
pub fn translate_notification(
    thread_id: &ThreadId,
    msg: NotificationMessage,
) -> Vec<ProviderRuntimeEvent> {
    let mut demux = CodexSubagentDemux::default();
    translate_notification_with(&mut demux, thread_id, msg)
}

/// Stateful translation that demuxes sub-agent threads via `demux`.
///
/// See [`CodexSubagentDemux`]. The session owns one demux for its whole
/// lifetime and passes it by `&mut` so child registrations persist across
/// messages.
pub fn translate_notification_with(
    demux: &mut CodexSubagentDemux,
    thread_id: &ThreadId,
    msg: NotificationMessage,
) -> Vec<ProviderRuntimeEvent> {
    // 1. Sub-agent orchestration items (collabAgentToolCall /
    //    subAgentActivity) map to SubagentUpdated regardless of which
    //    thread carried them, and their raw tool rendering is suppressed.
    if let Some(item_type) = special_subagent_item_type(&msg) {
        let env = match &msg {
            NotificationMessage::ItemStarted(env) | NotificationMessage::ItemCompleted(env) => env,
            _ => unreachable!("special_subagent_item_type only matches item envelopes"),
        };
        return match item_type {
            "collabAgentToolCall" => handle_collab_agent_tool_call(demux, thread_id, &env.item),
            "subAgentActivity" => handle_sub_agent_activity(demux, thread_id, &env.item),
            _ => vec![],
        };
    }

    // 2. Thread / turn lifecycle: when it belongs to a known sub-agent,
    //    drive SubagentUpdated status instead of the parent state.
    match &msg {
        NotificationMessage::ThreadStarted(p) => {
            return handle_thread_started(demux, thread_id, p);
        }
        NotificationMessage::TurnStarted(p) if demux.is_subagent(&p.thread_id) => {
            return vec![subagent_status_event(
                thread_id,
                &p.thread_id,
                SubagentStatus::Running,
            )];
        }
        NotificationMessage::TurnCompleted(p) if demux.is_subagent(&p.thread_id) => {
            return handle_subagent_turn_completed(thread_id, p);
        }
        NotificationMessage::Error(e) => {
            if let Some(child) = e.thread_id.as_deref() {
                if demux.is_subagent(child) {
                    return handle_subagent_error(thread_id, child, e);
                }
            }
        }
        _ => {}
    }

    // 3. Generic child-thread transcript routing: translate through the
    //    parent path, then tag the canonical events with the child's
    //    subagent_id so the drill-in view reuses every existing renderer.
    if let Some(wire_tid) = generic_wire_thread_id(&msg) {
        if demux.is_subagent(wire_tid) {
            let child = wire_tid.to_string();
            let mut events = translate_parent(thread_id, msg);
            tag_subagent_id(&mut events, &child);
            return events;
        }
    }

    // 4. Parent thread — unchanged legacy behaviour.
    translate_parent(thread_id, msg)
}

/// Legacy parent-thread translation (pre-subagent behaviour). Every arm
/// here assumes the notification belongs to this session's own thread.
fn translate_parent(
    thread_id: &ThreadId,
    msg: NotificationMessage,
) -> Vec<ProviderRuntimeEvent> {
    match msg {
        NotificationMessage::ThreadStarted(ThreadStartedParams {
            thread_id: codex_tid,
            ..
        }) => {
            vec![ProviderRuntimeEvent::SessionConfigured {
                thread_id: thread_id.clone(),
                provider_session_id: ProviderSessionId(codex_tid),
            }]
        }
        NotificationMessage::TurnStarted(TurnStartedParams {
            turn_id,
            thread_id: _codex_tid,
        }) => {
            let turn_id = TurnId(turn_id);
            vec![ProviderRuntimeEvent::SessionStateChanged {
                thread_id: thread_id.clone(),
                status: SessionStatus::Running {
                    active_turn: turn_id,
                },
            }]
        }
        NotificationMessage::TurnCompleted(params) => translate_turn_completed(thread_id, params),
        NotificationMessage::Error(ErrorParams {
            message,
            will_retry,
            thread_id: _,
        }) => {
            if will_retry {
                vec![ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread_id.clone()),
                    message: format!("codex retrying after error: {message}"),
                    original_payload: None,
                }]
            } else {
                vec![ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: thread_id.clone(),
                    status: SessionStatus::Error { message },
                }]
            }
        }
        NotificationMessage::AgentMessageDelta(p) => text_delta(thread_id, p, ContentStream::Text),
        NotificationMessage::CommandExecutionOutputDelta(p) => {
            text_delta(thread_id, p, ContentStream::CommandOutput)
        }
        NotificationMessage::FileChangeOutputDelta(p) => {
            text_delta(thread_id, p, ContentStream::FileChange)
        }
        NotificationMessage::PlanDelta(p) => text_delta(thread_id, p, ContentStream::Plan),
        NotificationMessage::ReasoningTextDelta(p) => translate_reasoning_text(thread_id, p),
        NotificationMessage::ReasoningSummaryTextDelta(p) => {
            translate_reasoning_summary_text(thread_id, p)
        }
        NotificationMessage::ReasoningSummaryPartAdded(p) => {
            // Pure structural marker — no text. Surface as a warning so
            // a future UI consumer that wants to render summary segments
            // can opt in by parsing the warning payload, but today we
            // don't need a per-segment event in the runtime stream.
            translate_reasoning_summary_marker(thread_id, p)
        }
        NotificationMessage::TerminalInteraction(p) => {
            // Stdin echo for an interactive command. Surface as a tool
            // input delta on the owning command-execution item so the UI
            // can show the user-typed text inline.
            translate_terminal_interaction(thread_id, p)
        }
        NotificationMessage::FileChangePatchUpdated(p) => {
            translate_patch_updated(thread_id, p)
        }
        NotificationMessage::McpToolCallProgress(p) => translate_mcp_progress(thread_id, p),
        NotificationMessage::ItemStarted(env) => translate_item_started(thread_id, env),
        NotificationMessage::ItemCompleted(env) => translate_item_completed(thread_id, env),
        NotificationMessage::Unknown { method, params } => {
            vec![ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(thread_id.clone()),
                message: format!("unknown codex notification: {method}"),
                original_payload: Some(params),
            }]
        }
    }
}

/// Translate a `turn/completed` notification. May emit both a
/// [`ProviderRuntimeEvent::TurnCompleted`] AND a
/// [`ProviderRuntimeEvent::SessionStateChanged`] depending on outcome.
pub fn translate_turn_completed(
    thread_id: &ThreadId,
    params: TurnCompletedParams,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = TurnId(params.turn_id);
    let (turn_status, next_session_status) = match params.status.as_str() {
        // "completed" is the v2 nested vocabulary; "succeeded"/"success"
        // the legacy flat vocabulary. Both are a clean success.
        "succeeded" | "success" | "completed" => {
            (TurnStatus::Success, Some(SessionStatus::Ready))
        }
        "failed" | "error" => {
            let msg = params.error.clone().unwrap_or_else(|| "turn failed".into());
            (
                TurnStatus::Error {
                    subtype: params.status.clone(),
                    message: msg.clone(),
                },
                Some(SessionStatus::Error { message: msg }),
            )
        }
        other => (
            TurnStatus::Error {
                subtype: other.to_string(),
                message: params
                    .error
                    .clone()
                    .unwrap_or_else(|| format!("turn ended with status {other}")),
            },
            Some(SessionStatus::Ready),
        ),
    };

    let mut events = vec![ProviderRuntimeEvent::TurnCompleted {
        thread_id: thread_id.clone(),
        turn_id,
        status: turn_status,
        usage: None,
    }];
    if let Some(status) = next_session_status {
        events.push(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: thread_id.clone(),
            status,
        });
    }
    events
}

// ---------------------------------------------------------------------------
// Subagent demux helpers
// ---------------------------------------------------------------------------

/// Build a minimal [`SubagentSnapshot`] with only the id + status set (and
/// `provider_ref` mirrored from the id, since Codex's demux key *is* the
/// child threadId). Callers overwrite additional fields as they learn
/// them; every unset field stays `None` so the frontend merge never
/// clobbers a previously-known value.
fn subagent_snapshot(subagent_id: &str, status: SubagentStatus) -> SubagentSnapshot {
    SubagentSnapshot {
        subagent_id: subagent_id.to_string(),
        parent_item_id: None,
        name: None,
        agent_type: None,
        model: None,
        status,
        activity: None,
        result_text: None,
        tool_use_count: None,
        total_tokens: None,
        duration_ms: None,
        provider_ref: Some(subagent_id.to_string()),
    }
}

/// Wrap a minimal snapshot in a [`SubagentUpdated`](ProviderRuntimeEvent::SubagentUpdated)
/// event scoped under the parent runtime `thread_id`.
fn subagent_status_event(
    thread_id: &ThreadId,
    subagent_id: &str,
    status: SubagentStatus,
) -> ProviderRuntimeEvent {
    ProviderRuntimeEvent::SubagentUpdated {
        thread_id: thread_id.clone(),
        subagent: subagent_snapshot(subagent_id, status),
    }
}

/// If `msg` is an `item/started` / `item/completed` carrying a
/// sub-agent orchestration item, return its `type` tag
/// (`"collabAgentToolCall"` or `"subAgentActivity"`).
fn special_subagent_item_type(msg: &NotificationMessage) -> Option<&'static str> {
    let env = match msg {
        NotificationMessage::ItemStarted(env) | NotificationMessage::ItemCompleted(env) => env,
        _ => return None,
    };
    match env.item.get("type").and_then(|v| v.as_str()) {
        Some("collabAgentToolCall") => Some("collabAgentToolCall"),
        Some("subAgentActivity") => Some("subAgentActivity"),
        _ => None,
    }
}

/// Map a Codex `CollabAgentStatus` to the canonical [`SubagentStatus`].
fn map_collab_agent_status(status: &str) -> SubagentStatus {
    match status {
        "pendingInit" => SubagentStatus::Pending,
        "running" => SubagentStatus::Running,
        "interrupted" => SubagentStatus::Stopped,
        "completed" => SubagentStatus::Completed,
        "errored" => SubagentStatus::Failed,
        "shutdown" => SubagentStatus::Stopped,
        "notFound" => SubagentStatus::Failed,
        _ => SubagentStatus::Pending,
    }
}

/// Fallback status for a receiver thread the `agentsStates` map doesn't
/// mention — derive it from the tool + collab-call status instead.
fn fallback_collab_status(tool: &str, call_status: &str) -> SubagentStatus {
    match (tool, call_status) {
        (_, "failed") => SubagentStatus::Failed,
        ("closeAgent", _) => SubagentStatus::Stopped,
        ("spawnAgent", "inProgress") => SubagentStatus::Pending,
        ("spawnAgent", "completed") => SubagentStatus::Running,
        _ => SubagentStatus::Running,
    }
}

/// Translate a `collabAgentToolCall` item into per-child
/// [`SubagentUpdated`](ProviderRuntimeEvent::SubagentUpdated) events and
/// register the receiver threads as sub-agents. Raw tool rendering is
/// suppressed (no `ToolUse` / `ToolResult`).
fn handle_collab_agent_tool_call(
    demux: &mut CodexSubagentDemux,
    thread_id: &ThreadId,
    item: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    let call: CollabAgentToolCallItem = match serde_json::from_value(item.clone()) {
        Ok(c) => c,
        // Suppressed regardless: a malformed collab item must not render
        // as a generic tool card.
        Err(_) => return vec![],
    };

    let parent = call
        .sender_thread_id
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| demux.parent_thread_id.clone());

    // Every receiver and every agent mentioned in `agentsStates` is a
    // sub-agent of `parent`. Use a sorted set so emission order is stable.
    let mut affected: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for rid in &call.receiver_thread_ids {
        demux.register(rid, &parent);
        affected.insert(rid.clone());
    }
    for id in call.agents_states.keys() {
        demux.register(id, &parent);
        affected.insert(id.clone());
    }

    affected
        .into_iter()
        .map(|tid| {
            let (status, activity) = match call.agents_states.get(&tid) {
                Some(state) => (map_collab_agent_status(&state.status), state.message.clone()),
                None => (fallback_collab_status(&call.tool, &call.status), None),
            };
            let mut snap = subagent_snapshot(&tid, status);
            snap.parent_item_id = Some(call.id.clone());
            snap.model = call.model.clone();
            snap.activity = activity;
            ProviderRuntimeEvent::SubagentUpdated {
                thread_id: thread_id.clone(),
                subagent: snap,
            }
        })
        .collect()
}

/// Translate a `subAgentActivity` item into a cheap status tick and
/// register the sub-agent thread. Raw rendering is suppressed.
fn handle_sub_agent_activity(
    demux: &mut CodexSubagentDemux,
    thread_id: &ThreadId,
    item: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    let act: SubAgentActivityItem = match serde_json::from_value(item.clone()) {
        Ok(a) => a,
        Err(_) => return vec![],
    };
    let parent = demux.parent_thread_id.clone();
    demux.register(&act.agent_thread_id, &parent);
    let status = match act.kind.as_str() {
        "started" | "interacted" => SubagentStatus::Running,
        "interrupted" => SubagentStatus::Stopped,
        // Unknown kind: register but emit nothing (avoid a spurious tick).
        _ => return vec![],
    };
    vec![subagent_status_event(thread_id, &act.agent_thread_id, status)]
}

/// Handle a `thread/started` notification. A child thread (identified by
/// `parentThreadId`, or already known as a sub-agent) becomes a
/// [`SubagentUpdated`](ProviderRuntimeEvent::SubagentUpdated) carrying its
/// nickname / role identity; the session's own thread keeps its legacy
/// [`SessionConfigured`](ProviderRuntimeEvent::SessionConfigured) mapping.
fn handle_thread_started(
    demux: &mut CodexSubagentDemux,
    thread_id: &ThreadId,
    p: &ThreadStartedParams,
) -> Vec<ProviderRuntimeEvent> {
    let is_child = match &p.parent_thread_id {
        Some(parent) => parent == &demux.parent_thread_id || demux.is_subagent(parent),
        // No parentThreadId (legacy flat shape or the parent's own
        // started): only a child if a prior collab call already
        // registered this thread id.
        None => demux.is_subagent(&p.thread_id),
    };

    if is_child {
        let parent = p
            .parent_thread_id
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| demux.parent_thread_id.clone());
        demux.register(&p.thread_id, &parent);
        let mut snap = subagent_snapshot(&p.thread_id, SubagentStatus::Running);
        snap.name = p.agent_nickname.clone().or_else(|| p.agent_role.clone());
        snap.agent_type = p.agent_role.clone();
        return vec![ProviderRuntimeEvent::SubagentUpdated {
            thread_id: thread_id.clone(),
            subagent: snap,
        }];
    }

    // Parent thread — unchanged legacy behaviour.
    translate_parent(
        thread_id,
        NotificationMessage::ThreadStarted(p.clone()),
    )
}

/// Map a child thread's `turn/completed` to a sub-agent status change
/// (Completed / Failed / Stopped) with duration — never the parent turn
/// state.
fn handle_subagent_turn_completed(
    thread_id: &ThreadId,
    p: &TurnCompletedParams,
) -> Vec<ProviderRuntimeEvent> {
    let status = match p.status.as_str() {
        "succeeded" | "success" | "completed" => SubagentStatus::Completed,
        "failed" | "error" | "errored" => SubagentStatus::Failed,
        "interrupted" | "stopped" => SubagentStatus::Stopped,
        _ => SubagentStatus::Completed,
    };
    let mut snap = subagent_snapshot(&p.thread_id, status);
    snap.duration_ms = p.duration_ms;
    if status == SubagentStatus::Failed {
        snap.activity = p.error.clone();
    }
    vec![ProviderRuntimeEvent::SubagentUpdated {
        thread_id: thread_id.clone(),
        subagent: snap,
    }]
}

/// Map a child thread's `error` notification to a sub-agent status change
/// (retryable → warning, terminal → Failed) instead of failing the parent
/// session.
fn handle_subagent_error(
    thread_id: &ThreadId,
    child: &str,
    e: &ErrorParams,
) -> Vec<ProviderRuntimeEvent> {
    if e.will_retry {
        return vec![ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(thread_id.clone()),
            message: format!("codex subagent {child} retrying after error: {}", e.message),
            original_payload: None,
        }];
    }
    let mut snap = subagent_snapshot(child, SubagentStatus::Failed);
    snap.activity = Some(e.message.clone());
    vec![ProviderRuntimeEvent::SubagentUpdated {
        thread_id: thread_id.clone(),
        subagent: snap,
    }]
}

/// Extract the wire `threadId` of a delta / item notification (the ones
/// whose canonical translation should be routed into a child transcript).
/// Lifecycle notifications (`thread/*`, `turn/*`, `error`) are handled
/// separately and return `None` here.
fn generic_wire_thread_id(msg: &NotificationMessage) -> Option<&str> {
    use NotificationMessage::*;
    match msg {
        AgentMessageDelta(p)
        | CommandExecutionOutputDelta(p)
        | FileChangeOutputDelta(p)
        | PlanDelta(p) => Some(&p.thread_id),
        ReasoningTextDelta(p) => Some(&p.thread_id),
        ReasoningSummaryTextDelta(p) => Some(&p.thread_id),
        ReasoningSummaryPartAdded(p) => Some(&p.thread_id),
        TerminalInteraction(p) => Some(&p.thread_id),
        FileChangePatchUpdated(p) => Some(&p.thread_id),
        McpToolCallProgress(p) => Some(&p.thread_id),
        ItemStarted(env) | ItemCompleted(env) => Some(&env.thread_id),
        _ => None,
    }
}

/// Tag the child `subagent_id` onto every canonical event that carries the
/// field, so the drill-in view reuses the same renderers as the parent
/// transcript. Other event kinds pass through untouched.
fn tag_subagent_id(events: &mut [ProviderRuntimeEvent], child: &str) {
    for ev in events.iter_mut() {
        match ev {
            ProviderRuntimeEvent::ContentDelta { subagent_id, .. }
            | ProviderRuntimeEvent::ItemCompleted { subagent_id, .. }
            | ProviderRuntimeEvent::RequestOpened { subagent_id, .. } => {
                *subagent_id = Some(child.to_string());
            }
            _ => {}
        }
    }
}

/// Which canonical text stream a delta belongs to. The chat UI doesn't
/// distinguish today, but the kind is forwarded as the `tool_name` on
/// non-text streams so the UI can render command-output / file-change
/// chunks in dedicated cards in a follow-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentStream {
    Text,
    CommandOutput,
    FileChange,
    Plan,
}

fn text_delta(
    thread_id: &ThreadId,
    p: DeltaParams,
    kind: ContentStream,
) -> Vec<ProviderRuntimeEvent> {
    if p.delta.is_empty() {
        return vec![];
    }
    match kind {
        ContentStream::Text => vec![ProviderRuntimeEvent::ContentDelta {
            subagent_id: None,
            thread_id: thread_id.clone(),
            turn_id: TurnId(p.turn_id),
            delta: ContentDelta::Text { text: p.delta },
        }],
        // Command output and file change deltas are routed to the
        // tool-input stream keyed on the item id — the UI threads them
        // onto the matching tool-call card emitted by the prior
        // `item/started` event.
        ContentStream::CommandOutput => vec![ProviderRuntimeEvent::ContentDelta {
            subagent_id: None,
            thread_id: thread_id.clone(),
            turn_id: TurnId(p.turn_id),
            delta: ContentDelta::ToolInput {
                tool_name: format!("commandExecution::{}", p.item_id),
                partial_json: p.delta,
            },
        }],
        ContentStream::FileChange => vec![ProviderRuntimeEvent::ContentDelta {
            subagent_id: None,
            thread_id: thread_id.clone(),
            turn_id: TurnId(p.turn_id),
            delta: ContentDelta::ToolInput {
                tool_name: format!("fileChange::{}", p.item_id),
                partial_json: p.delta,
            },
        }],
        ContentStream::Plan => vec![ProviderRuntimeEvent::ContentDelta {
            subagent_id: None,
            thread_id: thread_id.clone(),
            turn_id: TurnId(p.turn_id),
            delta: ContentDelta::ToolInput {
                tool_name: format!("plan::{}", p.item_id),
                partial_json: p.delta,
            },
        }],
    }
}

fn translate_reasoning_text(
    thread_id: &ThreadId,
    p: ReasoningTextDeltaParams,
) -> Vec<ProviderRuntimeEvent> {
    if p.delta.is_empty() {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        subagent_id: None,
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::Thinking { text: p.delta },
    }]
}

fn translate_reasoning_summary_text(
    thread_id: &ThreadId,
    p: ReasoningSummaryTextDeltaParams,
) -> Vec<ProviderRuntimeEvent> {
    if p.delta.is_empty() {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        subagent_id: None,
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::Thinking { text: p.delta },
    }]
}

fn translate_reasoning_summary_marker(
    thread_id: &ThreadId,
    p: ReasoningSummaryPartAddedParams,
) -> Vec<ProviderRuntimeEvent> {
    // Surface the marker as a warning so it stays visible for diagnostics
    // without polluting the canonical content stream — the chat UI
    // re-renders the reasoning block as a single accordion regardless of
    // segment boundaries.
    vec![ProviderRuntimeEvent::RuntimeWarning {
        thread_id: Some(thread_id.clone()),
        message: format!(
            "codex reasoning summary part added (item {} index {})",
            p.item_id, p.summary_index
        ),
        original_payload: None,
    }]
}

fn translate_terminal_interaction(
    thread_id: &ThreadId,
    p: TerminalInteractionParams,
) -> Vec<ProviderRuntimeEvent> {
    if p.stdin.is_empty() {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        subagent_id: None,
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::ToolInput {
            tool_name: format!("commandExecution::{}", p.item_id),
            partial_json: p.stdin,
        },
    }]
}

fn translate_patch_updated(
    thread_id: &ThreadId,
    p: FileChangePatchUpdatedParams,
) -> Vec<ProviderRuntimeEvent> {
    // Forward the patch payload onto the file-change tool stream so the
    // UI can render an updated diff snapshot.
    vec![ProviderRuntimeEvent::ContentDelta {
        subagent_id: None,
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::ToolInput {
            tool_name: format!("fileChange::{}", p.item_id),
            partial_json: p.changes.to_string(),
        },
    }]
}

fn translate_mcp_progress(
    thread_id: &ThreadId,
    p: McpToolCallProgressParams,
) -> Vec<ProviderRuntimeEvent> {
    if p.message.is_empty() {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        subagent_id: None,
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::ToolInput {
            tool_name: format!("mcpToolCall::{}", p.item_id),
            partial_json: p.message,
        },
    }]
}

/// Translate `item/started`. Tool-flavoured items emit a `ToolUse` so the
/// UI can render the card immediately; text/reasoning/plan items emit
/// nothing (the corresponding delta stream carries the actual content).
fn translate_item_started(
    thread_id: &ThreadId,
    env: ItemEnvelope,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = TurnId(env.turn_id);
    let item_type = env
        .item
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_id = env
        .item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match item_type.as_str() {
        "commandExecution" => vec![ProviderRuntimeEvent::ItemCompleted {
            subagent_id: None,
            thread_id: thread_id.clone(),
            turn_id,
            item: CompletedItem::ToolUse {
                tool_name: "commandExecution".into(),
                input: env.item.clone(),
                tool_use_id: item_id,
            },
        }],
        "fileChange" => vec![ProviderRuntimeEvent::ItemCompleted {
            subagent_id: None,
            thread_id: thread_id.clone(),
            turn_id,
            item: CompletedItem::ToolUse {
                tool_name: "fileChange".into(),
                input: env.item.clone(),
                tool_use_id: item_id,
            },
        }],
        "mcpToolCall" => vec![ProviderRuntimeEvent::ItemCompleted {
            subagent_id: None,
            thread_id: thread_id.clone(),
            turn_id,
            item: CompletedItem::ToolUse {
                tool_name: "mcpToolCall".into(),
                input: env.item.clone(),
                tool_use_id: item_id,
            },
        }],
        "dynamicToolCall" => vec![ProviderRuntimeEvent::ItemCompleted {
            subagent_id: None,
            thread_id: thread_id.clone(),
            turn_id,
            item: CompletedItem::ToolUse {
                tool_name: "dynamicToolCall".into(),
                input: env.item.clone(),
                tool_use_id: item_id,
            },
        }],
        // Text / reasoning / plan items have no started-time payload —
        // their content streams in via the matching delta event. Skip
        // emitting a duplicate event so the UI doesn't show an empty
        // placeholder.
        "agentMessage" | "reasoning" | "plan" | "userMessage" | "hookPrompt" => vec![],
        // Other variants (webSearch, imageGeneration, etc.) — surface as
        // a warning so we know the SDK shipped something new.
        other => vec![ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(thread_id.clone()),
            message: format!("codex item/started type not specifically handled: {other}"),
            original_payload: Some(env.item),
        }],
    }
}

/// Translate `item/completed`. For text-bearing items emit the final
/// [`CompletedItem`]; for tool items emit a [`CompletedItem::ToolResult`]
/// carrying the final output payload.
fn translate_item_completed(
    thread_id: &ThreadId,
    env: ItemEnvelope,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = TurnId(env.turn_id);
    let item_type = env
        .item
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_id = env
        .item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match item_type.as_str() {
        "agentMessage" => {
            let text = env
                .item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::ItemCompleted {
                subagent_id: None,
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::AssistantText { text },
            }]
        }
        "reasoning" => {
            // Reasoning items carry both `content` and `summary` arrays
            // of strings. Concatenate into a single thinking block —
            // the chat UI renders this as the model's reasoning trace.
            let mut buf = String::new();
            if let Some(arr) = env.item.get("content").and_then(|v| v.as_array()) {
                for s in arr.iter().filter_map(|v| v.as_str()) {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(s);
                }
            }
            if let Some(arr) = env.item.get("summary").and_then(|v| v.as_array()) {
                for s in arr.iter().filter_map(|v| v.as_str()) {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(s);
                }
            }
            if buf.is_empty() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::ItemCompleted {
                subagent_id: None,
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::AssistantThinking { text: buf },
            }]
        }
        "plan" => {
            let text = env
                .item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::ItemCompleted {
                subagent_id: None,
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::AssistantText { text },
            }]
        }
        // Tool-style items emit a result with the final payload.
        "commandExecution" => {
            let aggregated = env
                .item
                .get("aggregatedOutput")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let exit_code = env.item.get("exitCode").and_then(|v| v.as_i64());
            let is_error = exit_code.map(|c| c != 0).unwrap_or(false);
            vec![ProviderRuntimeEvent::ItemCompleted {
                subagent_id: None,
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::ToolResult {
                    tool_use_id: item_id,
                    content: serde_json::Value::String(aggregated.to_string()),
                    is_error,
                },
            }]
        }
        "fileChange" | "mcpToolCall" | "dynamicToolCall" | "webSearch" => {
            vec![ProviderRuntimeEvent::ItemCompleted {
                subagent_id: None,
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::ToolResult {
                    tool_use_id: item_id,
                    content: env.item,
                    is_error: false,
                },
            }]
        }
        // Other variants — userMessage, hookPrompt, etc. — are not
        // currently rendered by the chat UI. Surface as a warning to
        // make future SDK additions visible.
        other => vec![ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(thread_id.clone()),
            message: format!("codex item/completed type not specifically handled: {other}"),
            original_payload: Some(env.item),
        }],
    }
}

/// Translate a server-initiated request into a
/// [`ProviderRuntimeEvent::RequestOpened`]. The runtime is expected to
/// track the JSON-RPC id separately so it can respond later.
pub fn translate_server_request(
    thread_id: &ThreadId,
    request_id: &RequestId,
    msg: &ServerRequestMessage,
) -> ProviderRuntimeEvent {
    let turn_id = msg
        .payload()
        .get("turnId")
        .and_then(|v| v.as_str())
        .map(|s| TurnId(s.to_string()))
        .unwrap_or_else(|| TurnId(String::new()));

    ProviderRuntimeEvent::RequestOpened {
        subagent_id: None,
        thread_id: thread_id.clone(),
        turn_id,
        request_id: request_id.clone(),
        request_kind: msg.request_kind().to_string(),
        payload: msg.payload().clone(),
        tool_use_id: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tid() -> ThreadId {
        ThreadId("thr-local".into())
    }

    fn delta_payload(method_field: &str, delta: &str) -> serde_json::Value {
        let _ = method_field; // kept for readability
        json!({
            "delta": delta,
            "itemId": "i-1",
            "threadId": "c1",
            "turnId": "t1",
        })
    }

    #[test]
    fn agent_message_delta_uses_real_sdk_field_name() {
        // Pin: SDK wire field is `delta`, NOT `textDelta`. Earlier
        // versions of this adapter used `textDelta` and silently
        // dropped every assistant text chunk.
        let msg = NotificationMessage::from_raw(
            "item/agentMessage/delta",
            delta_payload("agentMessage", "hello"),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::Text { text } => assert_eq!(text, "hello"),
                other => panic!("wrong delta: {other:?}"),
            },
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn reasoning_text_delta_becomes_thinking() {
        let msg = NotificationMessage::from_raw(
            "item/reasoning/textDelta",
            json!({
                "delta": "Let me think...",
                "contentIndex": 0,
                "itemId": "i-2",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::Thinking { text } => assert_eq!(text, "Let me think..."),
                other => panic!("wrong delta: {other:?}"),
            },
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn reasoning_summary_text_delta_becomes_thinking() {
        let msg = NotificationMessage::from_raw(
            "item/reasoning/summaryTextDelta",
            json!({
                "delta": "Summary chunk",
                "summaryIndex": 0,
                "itemId": "i-3",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::ContentDelta {
                delta: ContentDelta::Thinking { .. },
                ..
            }
        ));
    }

    #[test]
    fn reasoning_summary_part_added_emits_diagnostic_warning() {
        let msg = NotificationMessage::from_raw(
            "item/reasoning/summaryPartAdded",
            json!({
                "summaryIndex": 1,
                "itemId": "i-3",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn command_execution_output_delta_routed_to_tool_input_stream() {
        let msg = NotificationMessage::from_raw(
            "item/commandExecution/outputDelta",
            json!({
                "delta": "fn main() {",
                "itemId": "cmd-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput {
                    tool_name,
                    partial_json,
                } => {
                    assert!(tool_name.starts_with("commandExecution::"));
                    assert!(tool_name.ends_with("cmd-1"));
                    assert_eq!(partial_json, "fn main() {");
                }
                other => panic!("wrong delta: {other:?}"),
            },
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn file_change_output_delta_routed_to_tool_input_stream() {
        let msg = NotificationMessage::from_raw(
            "item/fileChange/outputDelta",
            json!({
                "delta": "+ added line",
                "itemId": "fc-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput { tool_name, .. } => {
                    assert!(tool_name.starts_with("fileChange::"));
                }
                _ => panic!("wrong delta"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn plan_delta_routed_to_tool_input_stream() {
        let msg = NotificationMessage::from_raw(
            "item/plan/delta",
            json!({
                "delta": "- step 1",
                "itemId": "plan-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput { tool_name, .. } => {
                    assert!(tool_name.starts_with("plan::"));
                }
                _ => panic!("wrong delta"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn mcp_tool_call_progress_routed_to_tool_input_stream() {
        let msg = NotificationMessage::from_raw(
            "item/mcpToolCall/progress",
            json!({
                "message": "Connecting...",
                "itemId": "mcp-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput {
                    tool_name,
                    partial_json,
                } => {
                    assert!(tool_name.starts_with("mcpToolCall::"));
                    assert_eq!(partial_json, "Connecting...");
                }
                _ => panic!("wrong delta"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn empty_delta_is_dropped() {
        let msg = NotificationMessage::from_raw(
            "item/agentMessage/delta",
            json!({
                "delta": "",
                "itemId": "i-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        assert!(translate_notification(&tid(), msg).is_empty());
    }

    #[test]
    fn item_started_command_execution_emits_tool_use() {
        let msg = NotificationMessage::from_raw(
            "item/started",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "commandExecution",
                    "id": "cmd-9",
                    "command": "ls -la",
                    "cwd": "/tmp"
                }
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::ToolUse {
                    tool_name,
                    tool_use_id,
                    ..
                } => {
                    assert_eq!(tool_name, "commandExecution");
                    assert_eq!(tool_use_id, "cmd-9");
                }
                _ => panic!("wrong item"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn item_started_text_item_drops() {
        let msg = NotificationMessage::from_raw(
            "item/started",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "agentMessage",
                    "id": "am-1",
                    "text": ""
                }
            }),
        );
        assert!(translate_notification(&tid(), msg).is_empty());
    }

    #[test]
    fn item_completed_agent_message_emits_assistant_text() {
        let msg = NotificationMessage::from_raw(
            "item/completed",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "agentMessage",
                    "id": "am-1",
                    "text": "Final answer."
                }
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::AssistantText { text } => assert_eq!(text, "Final answer."),
                _ => panic!("wrong item"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn item_completed_reasoning_concatenates_content_and_summary() {
        let msg = NotificationMessage::from_raw(
            "item/completed",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "reasoning",
                    "id": "r-1",
                    "content": ["Think step one.", "Think step two."],
                    "summary": ["Summary."]
                }
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::AssistantThinking { text } => {
                    assert!(text.contains("Think step one."));
                    assert!(text.contains("Think step two."));
                    assert!(text.contains("Summary."));
                }
                _ => panic!("wrong item"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn item_completed_command_execution_emits_tool_result_with_exit_code() {
        let msg = NotificationMessage::from_raw(
            "item/completed",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "commandExecution",
                    "id": "cmd-9",
                    "command": "false",
                    "cwd": "/tmp",
                    "aggregatedOutput": "",
                    "exitCode": 1,
                    "commandActions": []
                }
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => {
                    assert_eq!(tool_use_id, "cmd-9");
                    assert!(*is_error, "non-zero exit code → is_error=true");
                }
                _ => panic!("wrong item"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn turn_completed_success_emits_both_events() {
        let msg = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId":"c1","turnId":"t1","status":"succeeded"}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ProviderRuntimeEvent::TurnCompleted { .. }));
        assert!(matches!(
            events[1],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Ready,
                ..
            }
        ));
    }

    #[test]
    fn turn_completed_failed_emits_error_for_both_events() {
        let msg = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId":"c1","turnId":"t1","status":"failed","error":"bad thing"}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 2);
        if let ProviderRuntimeEvent::TurnCompleted { status, .. } = &events[0] {
            match status {
                TurnStatus::Error { subtype, message } => {
                    assert_eq!(subtype, "failed");
                    assert_eq!(message, "bad thing");
                }
                _ => panic!("expected TurnStatus::Error"),
            }
        } else {
            panic!("first should be TurnCompleted");
        }
    }

    #[test]
    fn unknown_notification_preserves_payload_as_warning() {
        let msg = NotificationMessage::from_raw("foo/bar/baz", json!({"interesting":"data"}));
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn server_request_command_approval_classified() {
        let msg = ServerRequestMessage::from_raw(
            "item/commandExecution/requestApproval",
            json!({"cmd":"rm -rf /", "turnId":"t1"}),
        );
        let rid = RequestId("r1".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        match ev {
            ProviderRuntimeEvent::RequestOpened {
                request_kind,
                request_id,
                turn_id,
                ..
            } => {
                assert_eq!(request_kind, "command");
                assert_eq!(request_id.0, "r1");
                assert_eq!(turn_id.0, "t1");
            }
            _ => panic!("expected RequestOpened"),
        }
    }

    #[test]
    fn server_request_permissions_approval_classified() {
        let msg = ServerRequestMessage::from_raw(
            "item/permissions/requestApproval",
            json!({"permissions":{}, "turnId":"t1"}),
        );
        let rid = RequestId("r2".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = ev {
            assert_eq!(request_kind, "permissions");
        } else {
            panic!("expected RequestOpened");
        }
    }

    #[test]
    fn server_request_tool_call_classified() {
        let msg = ServerRequestMessage::from_raw(
            "item/tool/call",
            json!({"name":"shell", "turnId":"t1"}),
        );
        let rid = RequestId("r3".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = ev {
            assert_eq!(request_kind, "tool-call");
        } else {
            panic!("expected RequestOpened");
        }
    }

    #[test]
    fn server_request_unknown_method_is_still_surfaced() {
        let msg = ServerRequestMessage::from_raw("item/foo/requestBizarro", json!({}));
        let rid = RequestId("r5".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = ev {
            assert_eq!(request_kind, "unknown");
        } else {
            panic!("expected RequestOpened");
        }
    }

    // ── Subagent demux: collabAgentToolCall ──

    fn collab_item(value: serde_json::Value) -> NotificationMessage {
        NotificationMessage::from_raw(
            "item/completed",
            json!({"threadId": "c-1", "turnId": "pt-1", "item": value}),
        )
    }

    #[test]
    fn collab_spawn_completed_emits_running_subagent_and_suppresses_tool() {
        let msg = collab_item(json!({
            "type": "collabAgentToolCall", "id": "call-1", "tool": "spawnAgent",
            "status": "completed", "senderThreadId": "c-1", "receiverThreadIds": ["c-child"],
            "model": "gpt-5.4", "prompt": "go",
            "agentsStates": {"c-child": {"status": "running", "message": "reading files"}}
        }));
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1, "one subagent, one event; tool suppressed");
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.subagent_id, "c-child");
                assert_eq!(subagent.status, SubagentStatus::Running);
                assert_eq!(subagent.model.as_deref(), Some("gpt-5.4"));
                assert_eq!(subagent.activity.as_deref(), Some("reading files"));
                assert_eq!(subagent.parent_item_id.as_deref(), Some("call-1"));
                assert_eq!(subagent.provider_ref.as_deref(), Some("c-child"));
            }
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
    }

    #[test]
    fn collab_spawn_in_progress_maps_agents_states_pending() {
        let msg = collab_item(json!({
            "type": "collabAgentToolCall", "id": "call-1", "tool": "spawnAgent",
            "status": "inProgress", "senderThreadId": "c-1", "receiverThreadIds": ["c-child"],
            "agentsStates": {"c-child": {"status": "pendingInit"}}
        }));
        let events = translate_notification(&tid(), msg);
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.status, SubagentStatus::Pending);
            }
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
    }

    #[test]
    fn collab_wait_completed_marks_subagent_completed() {
        let msg = collab_item(json!({
            "type": "collabAgentToolCall", "id": "call-2", "tool": "wait",
            "status": "completed", "senderThreadId": "c-1", "receiverThreadIds": ["c-child"],
            "agentsStates": {"c-child": {"status": "completed", "message": "done"}}
        }));
        let events = translate_notification(&tid(), msg);
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.status, SubagentStatus::Completed);
                assert_eq!(subagent.activity.as_deref(), Some("done"));
            }
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
    }

    #[test]
    fn collab_errored_agent_state_maps_failed() {
        let msg = collab_item(json!({
            "type": "collabAgentToolCall", "id": "call-3", "tool": "wait",
            "status": "completed", "senderThreadId": "c-1", "receiverThreadIds": ["c-child"],
            "agentsStates": {"c-child": {"status": "errored", "message": "blew up"}}
        }));
        let events = translate_notification(&tid(), msg);
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.status, SubagentStatus::Failed);
            }
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
    }

    // ── Subagent demux: subAgentActivity ──

    #[test]
    fn sub_agent_activity_started_emits_running_tick_and_suppresses() {
        let msg = NotificationMessage::from_raw(
            "item/completed",
            json!({"threadId": "c-1", "turnId": "pt-1", "item": {
                "type": "subAgentActivity", "id": "sa-1", "agentThreadId": "c-child",
                "agentPath": "root/child", "kind": "started"
            }}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.subagent_id, "c-child");
                assert_eq!(subagent.status, SubagentStatus::Running);
            }
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
    }

    #[test]
    fn sub_agent_activity_interrupted_maps_stopped() {
        let msg = NotificationMessage::from_raw(
            "item/started",
            json!({"threadId": "c-1", "turnId": "pt-1", "item": {
                "type": "subAgentActivity", "id": "sa-2", "agentThreadId": "c-child",
                "agentPath": "root/child", "kind": "interrupted"
            }}),
        );
        let events = translate_notification(&tid(), msg);
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.status, SubagentStatus::Stopped);
            }
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
    }

    // ── Subagent demux: child thread/started identity ──

    #[test]
    fn child_thread_started_emits_identity_not_session_configured() {
        let mut demux = CodexSubagentDemux::new("c-1");
        let msg = NotificationMessage::from_raw(
            "thread/started",
            json!({"thread": {"id": "c-child", "parentThreadId": "c-1",
                "agentNickname": "Explore", "agentRole": "explore"}}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.subagent_id, "c-child");
                assert_eq!(subagent.name.as_deref(), Some("Explore"));
                assert_eq!(subagent.agent_type.as_deref(), Some("explore"));
                assert_eq!(subagent.status, SubagentStatus::Running);
            }
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
    }

    #[test]
    fn parent_thread_started_still_maps_session_configured() {
        let mut demux = CodexSubagentDemux::new("c-1");
        // Flat legacy shape for the parent's own thread → SessionConfigured.
        let msg = NotificationMessage::from_raw("thread/started", json!({"threadId": "c-1"}));
        let events = translate_notification_with(&mut demux, &tid(), msg);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::SessionConfigured { .. }
        ));
    }

    // ── Subagent demux: child transcript routing + turn lifecycle ──

    #[test]
    fn child_transcript_item_is_tagged_with_subagent_id() {
        let mut demux = CodexSubagentDemux::new("c-1");
        // Register the child via a spawn first.
        let _ = translate_notification_with(
            &mut demux,
            &tid(),
            collab_item(json!({
                "type": "collabAgentToolCall", "id": "call-1", "tool": "spawnAgent",
                "status": "completed", "senderThreadId": "c-1", "receiverThreadIds": ["c-child"],
                "agentsStates": {"c-child": {"status": "running"}}
            })),
        );
        // Now a child-thread agentMessage should route into the drill-in.
        let child_msg = NotificationMessage::from_raw(
            "item/completed",
            json!({"threadId": "c-child", "turnId": "ct-1", "item": {
                "type": "agentMessage", "id": "cm-1", "text": "hi from child"}}),
        );
        let events = translate_notification_with(&mut demux, &tid(), child_msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted {
                item,
                subagent_id,
                ..
            } => {
                assert_eq!(subagent_id.as_deref(), Some("c-child"));
                assert!(matches!(item, CompletedItem::AssistantText { text } if text == "hi from child"));
            }
            other => panic!("expected ItemCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parent_transcript_item_stays_untagged() {
        let mut demux = CodexSubagentDemux::new("c-1");
        let msg = NotificationMessage::from_raw(
            "item/completed",
            json!({"threadId": "c-1", "turnId": "pt-1", "item": {
                "type": "agentMessage", "id": "am-1", "text": "parent text"}}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { subagent_id, .. } => {
                assert!(subagent_id.is_none());
            }
            other => panic!("expected ItemCompleted, got {other:?}"),
        }
    }

    #[test]
    fn child_turn_completed_updates_subagent_status_not_parent_turn() {
        let mut demux = CodexSubagentDemux::new("c-1");
        // Register child.
        let _ = translate_notification_with(
            &mut demux,
            &tid(),
            collab_item(json!({
                "type": "collabAgentToolCall", "id": "call-1", "tool": "spawnAgent",
                "status": "completed", "senderThreadId": "c-1", "receiverThreadIds": ["c-child"],
                "agentsStates": {"c-child": {"status": "running"}}
            })),
        );
        // v2 nested child turn/completed with duration.
        let msg = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId": "c-child", "turn": {"id": "ct-1", "status": "completed",
                "durationMs": 4321, "items": []}}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        assert_eq!(events.len(), 1, "child turn must NOT emit parent TurnCompleted/SessionStateChanged");
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.subagent_id, "c-child");
                assert_eq!(subagent.status, SubagentStatus::Completed);
                assert_eq!(subagent.duration_ms, Some(4321));
            }
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
        assert!(!events.iter().any(|e| matches!(
            e,
            ProviderRuntimeEvent::TurnCompleted { .. }
                | ProviderRuntimeEvent::SessionStateChanged { .. }
        )));
    }

    #[test]
    fn child_turn_started_updates_subagent_running_not_parent_session() {
        let mut demux = CodexSubagentDemux::new("c-1");
        let _ = translate_notification_with(
            &mut demux,
            &tid(),
            collab_item(json!({
                "type": "collabAgentToolCall", "id": "call-1", "tool": "spawnAgent",
                "status": "completed", "senderThreadId": "c-1", "receiverThreadIds": ["c-child"],
                "agentsStates": {"c-child": {"status": "running"}}
            })),
        );
        // v2 nested child turn/started.
        let msg = NotificationMessage::from_raw(
            "turn/started",
            json!({"threadId": "c-child", "turn": {"id": "ct-1", "status": "inProgress",
                "items": []}}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. }
                if subagent.status == SubagentStatus::Running
        ));
    }

    #[test]
    fn parent_turn_started_still_maps_session_running() {
        let mut demux = CodexSubagentDemux::new("c-1");
        let msg = NotificationMessage::from_raw(
            "turn/started",
            json!({"threadId": "c-1", "turnId": "pt-1"}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Running { .. },
                ..
            }
        ));
    }

    #[test]
    fn parent_turn_completed_v2_completed_status_is_success() {
        // The v2 nested vocabulary reports "completed"; the parent path
        // must treat it as a clean success (not an error subtype).
        let mut demux = CodexSubagentDemux::new("c-1");
        let msg = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId": "c-1", "turn": {"id": "pt-1", "status": "completed",
                "items": []}}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::TurnCompleted {
                status: TurnStatus::Success,
                ..
            }
        ));
        assert!(matches!(
            &events[1],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Ready,
                ..
            }
        ));
    }

    #[test]
    fn child_error_maps_subagent_failed_not_parent_error() {
        let mut demux = CodexSubagentDemux::new("c-1");
        let _ = translate_notification_with(
            &mut demux,
            &tid(),
            collab_item(json!({
                "type": "collabAgentToolCall", "id": "call-1", "tool": "spawnAgent",
                "status": "completed", "senderThreadId": "c-1", "receiverThreadIds": ["c-child"],
                "agentsStates": {"c-child": {"status": "running"}}
            })),
        );
        let msg = NotificationMessage::from_raw(
            "error",
            json!({"message": "child exploded", "willRetry": false, "threadId": "c-child"}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.status, SubagentStatus::Failed);
                assert_eq!(subagent.activity.as_deref(), Some("child exploded"));
            }
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
    }
}
