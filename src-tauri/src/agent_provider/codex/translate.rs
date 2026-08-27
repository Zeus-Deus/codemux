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
    pricing, CompletedItem, ContentDelta, ContextUsageTracker, CostSource, ProviderKind, ProviderRuntimeEvent,
    ProviderSessionId, RequestId, SessionStatus, SubagentSnapshot, SubagentStatus, TaskSnapshotItem,
    PlanUsageWindow, PlanWindowKind, TaskStatus, TasksSnapshot, ThreadId, TurnId, TurnStatus,
    UsageBaseline,
};

use super::protocol::{
    CollabAgentToolCallItem, DeltaParams, ErrorParams, FileChangePatchUpdatedParams, ItemEnvelope,
    McpToolCallProgressParams, NotificationMessage, ReasoningSummaryPartAddedParams,
    ReasoningSummaryTextDeltaParams, ReasoningTextDeltaParams, ServerRequestMessage,
    RateLimitSnapshot, RateLimitWindow, SubAgentActivityItem, TerminalInteractionParams,
    ThreadStartedParams, ThreadTokenUsageUpdatedParams, TokenUsageBreakdown, TurnCompletedParams,
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
    /// Context-window accounting for the parent thread. Per-session and
    /// mutated across messages, exactly like the sub-agent maps, so it
    /// rides the `&mut` the translator already receives.
    context: ContextUsageTracker,
    /// Codex wire thread id → the last cumulative usage split recorded
    /// for it, so the usage ledger can emit deltas.
    ///
    /// `thread/tokenUsage/updated` carries a *lifetime running total* per
    /// thread, re-sent on every update. Keyed by the Codex thread id (not
    /// the runtime one) because each subagent runs as its own Codex
    /// thread with its own independent total — sharing one counter across
    /// them would make every child's first report look like a huge
    /// parent-side jump.
    usage_totals: HashMap<String, UsageSplit>,
    /// Ledger totals already recorded for the ROOT thread, seeded from
    /// the database at session build and consumed on that thread's first
    /// usage report.
    ///
    /// Exists because Codex's `total` is a provider-maintained lifetime
    /// counter that survives `thread/resume`, while the delta bookkeeping
    /// above lives only in this (rebuilt) adapter instance. A session is
    /// rebuilt both across app restarts and mid-lifetime — the child-exit
    /// watchdog marks the session dead and the next send resumes it — so
    /// without a baseline the first report after any rebuild would look
    /// like the whole thread history arriving at once.
    ///
    /// Only the root thread gets one: subagent child threads are never
    /// resumed, so their counters genuinely start at zero and seeding
    /// against them would suppress real work.
    pending_baseline: Option<UsageSplit>,
    /// The model the session is currently configured with, pushed in by
    /// the session layer.
    ///
    /// Codex never states a model on the usage notification itself, and
    /// the translator has no route to session state, so the value has to
    /// arrive out-of-band. `None` until the session sets one; a usage row
    /// with no model still counts tokens, it just cannot be priced.
    active_model: Option<String>,
}

/// A non-overlapping token split, in the shape
/// [`UsageRecorded`](ProviderRuntimeEvent::UsageRecorded) expects.
///
/// Used both as the per-thread cumulative high-water mark and as the
/// delta computed against it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct UsageSplit {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    /// Informational SUBSET of `output` — never added into a total.
    /// Tracked here (rather than read straight off each report) because
    /// Codex reports it cumulatively like everything else, so it needs
    /// the same delta and resume-baseline treatment or a resumed thread
    /// would re-report its whole reasoning history.
    reasoning: u64,
}

impl UsageSplit {
    /// Normalize one Codex [`TokenUsageBreakdown`] into disjoint buckets.
    ///
    /// Codex reports *overlapping* figures: `cachedInputTokens` and
    /// `cacheWriteInputTokens` are both carved out of `inputTokens`, and
    /// `reasoningOutputTokens` out of `outputTokens` (protocol.rs names
    /// the first and last explicitly). The decode fixture pins the
    /// arithmetic for cache-write, which the doc leaves implicit:
    /// `inputTokens 23_863 + outputTokens 679 == totalTokens 24_542`
    /// while `cacheWriteInputTokens` is 2_715 — a sibling term would
    /// have to appear in that sum to balance it, so it must already be
    /// inside `inputTokens`.
    ///
    /// Subtracting both carve-outs therefore preserves Codex's own
    /// total: `input + output + cache_read + cache_write ==
    /// total_tokens`. Reasoning stays folded into `output` because that
    /// is how it is billed.
    fn from_breakdown(b: &TokenUsageBreakdown) -> Self {
        Self {
            input: b
                .input_tokens
                .saturating_sub(b.cached_input_tokens)
                .saturating_sub(b.cache_write_input_tokens),
            output: b.output_tokens,
            cache_read: b.cached_input_tokens,
            cache_write: b.cache_write_input_tokens,
            reasoning: b.reasoning_output_tokens,
        }
    }

    /// This split minus `previous`, per field.
    ///
    /// Saturating on purpose: a cumulative counter that comes back
    /// *lower* than the last one (a thread restart, or a partial
    /// re-send) must contribute zero rather than wrap into a colossal
    /// bogus charge.
    fn delta_since(&self, previous: &Self) -> Self {
        Self {
            input: self.input.saturating_sub(previous.input),
            output: self.output.saturating_sub(previous.output),
            cache_read: self.cache_read.saturating_sub(previous.cache_read),
            cache_write: self.cache_write.saturating_sub(previous.cache_write),
            reasoning: self.reasoning.saturating_sub(previous.reasoning),
        }
    }

    /// Resolve `self` (a seeded baseline) against the provider's first
    /// reported total, per field.
    ///
    /// * **baseline <= current** — the counter survived the resume, so the
    ///   baseline is the right starting point and `current - baseline` is
    ///   exactly the new work.
    /// * **baseline > current** — the provider reset its counter, so the
    ///   baseline is stale. Falling back to zero records this report in
    ///   full instead of letting a stale high-water mark swallow it.
    ///
    /// A plain `min` would *look* like a clamp but is a no-op here:
    /// [`delta_since`](Self::delta_since) already saturates, so clamping
    /// the baseline down to `current` yields a zero delta either way and
    /// the fresh usage would still be lost. The reset branch has to drop
    /// to zero to actually recover it.
    ///
    /// Resolving per field rather than per split keeps a partial
    /// mismatch (one counter carried over, another reset) graceful.
    fn baseline_against(&self, current: &Self) -> Self {
        let pick = |baseline: u64, current: u64| if baseline <= current { baseline } else { 0 };
        Self {
            input: pick(self.input, current.input),
            output: pick(self.output, current.output),
            cache_read: pick(self.cache_read, current.cache_read),
            cache_write: pick(self.cache_write, current.cache_write),
            reasoning: pick(self.reasoning, current.reasoning),
        }
    }

    fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0 && self.cache_write == 0
    }
}

impl CodexSubagentDemux {
    /// Create a demux rooted at the session's parent Codex thread id.
    pub fn new(parent_thread_id: impl Into<String>) -> Self {
        Self {
            parent_thread_id: parent_thread_id.into(),
            subagents: HashSet::new(),
            child_to_parent: HashMap::new(),
            context: ContextUsageTracker::default(),
            usage_totals: HashMap::new(),
            pending_baseline: None,
            active_model: None,
        }
    }

    /// Seed the ledger baseline for this session's root thread.
    ///
    /// Called once at session build with the totals already written to
    /// `agent_usage_ledger` for this thread. A zero baseline is stored as
    /// `None`: a fresh thread needs no clamping, and keeping the state
    /// would only obscure the resumed case.
    pub fn set_recorded_usage_baseline(&mut self, baseline: UsageBaseline) {
        self.pending_baseline = if baseline.is_zero() {
            None
        } else {
            Some(UsageSplit {
                input: baseline.input_tokens,
                output: baseline.output_tokens,
                cache_read: baseline.cache_read_tokens,
                cache_write: baseline.cache_write_tokens,
                reasoning: baseline.reasoning_tokens,
            })
        };
    }

    /// Tell the demux which model the session is running, so usage rows
    /// can be attributed and priced. Called by the session layer on
    /// start and on every mid-session model change.
    pub fn set_active_model(&mut self, model: Option<String>) {
        self.active_model = model;
    }

    /// Register `child` as a sub-agent spawned by `parent`.
    ///
    /// The session's own thread is never a sub-agent of itself. Codex
    /// reports orchestrator-directed traffic (a child messaging the
    /// parent) as a `subAgentActivity` whose `agentThreadId` is the
    /// PARENT; without this guard that one item demotes the whole
    /// session: every later parent item is routed as a child transcript
    /// and the parent's `turn/completed` becomes a `SubagentUpdated`
    /// instead of a `TurnCompleted`, so the run never settles.
    fn register(&mut self, child: &str, parent: &str) {
        if child.is_empty() || child == self.parent_thread_id {
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
        NotificationMessage::ThreadTokenUsageUpdated(p) => {
            return handle_thread_token_usage(demux, thread_id, p);
        }
        // Account-scoped plan quota. Not routed through the subagent
        // demux at all — it describes the login, not a thread.
        NotificationMessage::AccountRateLimitsUpdated(p) => {
            return plan_usage_from_rate_limits(thread_id, &p.rate_limits);
        }
        // The right-panel plan belongs to the orchestrator. A child may
        // maintain its own plan, but projecting it over the parent would
        // make the toggle jump between unrelated scopes.
        NotificationMessage::TurnPlanUpdated(p) if demux.is_subagent(&p.thread_id) => {
            return vec![];
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
        // Intercepted before this function by `handle_thread_token_usage`
        // (it needs the session's `&mut` tracker to clamp, dedupe, and
        // carry the lifetime total), so this arm exists only to keep the
        // match exhaustive.
        NotificationMessage::ThreadTokenUsageUpdated(_) => vec![],
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
        // Consumed by the account-scoped arm in the outer dispatch above;
        // this arm exists only to keep the match exhaustive.
        NotificationMessage::AccountRateLimitsUpdated(_) => vec![],
        NotificationMessage::TurnPlanUpdated(p) => {
            let tasks = p
                .plan
                .into_iter()
                .enumerate()
                .filter_map(|(index, step)| {
                    let title = step.step.trim().to_string();
                    if title.is_empty() {
                        return None;
                    }
                    let status = match step.status.as_str() {
                        "completed" => TaskStatus::Completed,
                        "inProgress" | "in_progress" => TaskStatus::InProgress,
                        _ => TaskStatus::Pending,
                    };
                    Some(TaskSnapshotItem {
                        task_id: format!("codex-{index}"),
                        title,
                        status,
                        detail: None,
                        blocked_by: Vec::new(),
                    })
                })
                .collect();
            vec![ProviderRuntimeEvent::TasksUpdated {
                thread_id: thread_id.clone(),
                tasks: TasksSnapshot {
                    explanation: p.explanation,
                    tasks,
                },
            }]
        }
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
        description: None,
        // Neither provider reports a watch-loop-vs-agent distinction yet;
        // `None` reads as ordinary agent work everywhere downstream.
        task_kind: None,
        model: None,
        status,
        activity: None,
        result_text: None,
        tool_use_count: None,
        total_tokens: None,
        duration_ms: None,
        provider_ref: Some(subagent_id.to_string()),
        workflow_id: None,
        phase: None,
        // Codex has no background-task concept on this path — every row
        // here is a real delegated agent.
        background_task: false,
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
    // sub-agent of `parent` — except the session's own thread, which a
    // `wait` / `sendInput` may list alongside real children and which
    // must never be snapshotted as a sub-agent of itself. Use a sorted set
    // so emission order is stable.
    let own = demux.parent_thread_id.clone();
    let mut affected: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for id in call
        .receiver_thread_ids
        .iter()
        .chain(call.agents_states.keys())
        .filter(|id| **id != own)
    {
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
            // An explicit child override wins. `model` is only ever set on
            // `spawnAgent`, so the inherit-from-parent fallback is scoped to
            // that tool: a later `wait`/`sendInput`/`closeAgent` on the same
            // child must leave the slot `None` (the merge keeps the last
            // known value) instead of stamping the parent's *current* model
            // over an explicitly-routed child.
            snap.model = match (call.model.clone(), call.tool.as_str()) {
                (Some(model), _) => Some(model),
                (None, "spawnAgent") => demux.active_model.clone(),
                (None, _) => None,
            };
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
    // Activity attributed to the orchestrator itself (a child interacting
    // with the parent) is not a sub-agent tick: there is no child to
    // update, and a snapshot keyed by the parent id would render the
    // session as its own sub-agent. `register` refuses it too; skipping
    // here also suppresses the spurious event.
    if act.agent_thread_id == parent {
        return vec![];
    }
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
    // The session's own thread is never a child, whatever `parentThreadId`
    // claims — see `register`.
    let is_own = p.thread_id == demux.parent_thread_id;
    let is_child = !is_own
        && match &p.parent_thread_id {
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
/// `thread/tokenUsage/updated` → [`ProviderRuntimeEvent::ContextUsageUpdated`].
///
/// The app-server does the accounting for us: `last` is the current
/// window occupancy, `total` is the provider-maintained lifetime sum
/// (preferred over a local accumulator — it is the source of truth),
/// and `modelContextWindow` is the only place Codex states a window
/// size.
///
/// Usage hygiene: a sub-agent runs as its own Codex thread against its
/// own context window and reports usage under its own `threadId`.
/// Those reports are dropped rather than folded into the parent's
/// meter, matching the rule the Claude adapter applies to subagent
/// messages.
fn handle_thread_token_usage(
    demux: &mut CodexSubagentDemux,
    thread_id: &ThreadId,
    params: &ThreadTokenUsageUpdatedParams,
) -> Vec<ProviderRuntimeEvent> {
    let is_subagent = demux.is_subagent(&params.thread_id);

    // Ledger first, and for *every* thread. The context-meter early
    // return below is a deliberate hygiene rule for the composer gauge;
    // accounting has the opposite requirement, so it runs before that
    // return rather than after it.
    let mut events = record_usage(demux, thread_id, params, is_subagent);

    if is_subagent {
        return events;
    }
    let usage = &params.token_usage;
    let (last, total, window) = (
        usage.last.tokens(),
        usage.total.tokens(),
        usage.model_context_window,
    );
    demux.context.observe_max_tokens(window);
    demux.context.observe_lifetime_total(total);
    events.extend(demux.context.events(
        thread_id,
        last,
        None,
        // Codex compacts automatically as the window fills, and
        // Codemux exposes no toggle for it.
        Some(true),
    ));
    events
}

/// Turn one cumulative `thread/tokenUsage/updated` report into at most
/// one [`UsageRecorded`](ProviderRuntimeEvent::UsageRecorded) delta.
///
/// Codex hands us a running lifetime total per thread and re-sends it on
/// every update, so the ledger row is `total - previously_recorded`. The
/// high-water map is keyed by the Codex thread id, which makes subagent
/// threads self-isolating: each keeps its own counter and its rows are
/// flagged rather than dropped.
/// Classify a Codex rate-limit window from its duration.
///
/// Codex states a duration in minutes rather than naming the window, so
/// the mapping is by proximity: ~300 minutes is the 5-hour window and
/// ~10080 the weekly one. Anything else (or a missing duration) becomes
/// [`PlanWindowKind::Other`] and keeps a generic label — an unfamiliar
/// plan should surface a bar the user can read, not vanish.
fn plan_window_kind(duration_mins: Option<f64>) -> (PlanWindowKind, Option<String>) {
    match duration_mins {
        // Generous bands: the exact figure has drifted between plans.
        Some(mins) if (240.0..=420.0).contains(&mins) => (PlanWindowKind::FiveHour, None),
        Some(mins) if (8_640.0..=11_520.0).contains(&mins) => (PlanWindowKind::SevenDay, None),
        Some(mins) => (
            PlanWindowKind::Other,
            Some(humanize_window_duration(mins)),
        ),
        None => (PlanWindowKind::Other, None),
    }
}

/// A short human name for an unrecognized window length.
fn humanize_window_duration(mins: f64) -> String {
    if mins >= 1_440.0 {
        let days = (mins / 1_440.0).round() as i64;
        format!("{days}d")
    } else if mins >= 60.0 {
        let hours = (mins / 60.0).round() as i64;
        format!("{hours}h")
    } else {
        format!("{}m", mins.round() as i64)
    }
}

/// Build a [`PlanUsageWindow`] from one Codex window, if present.
fn codex_window(window: Option<&RateLimitWindow>) -> Option<PlanUsageWindow> {
    let window = window?;
    let (kind, label) = plan_window_kind(window.window_duration_mins);
    Some(PlanUsageWindow {
        kind,
        // Already a percent here — no ×100, unlike Claude.
        used_pct: window.used_percent.clamp(0.0, 100.0),
        // Epoch seconds → ms.
        resets_at_ms: window.resets_at.map(|secs| (secs * 1000.0) as i64),
        label,
    })
}

/// Translate an `account/rateLimits/updated` push into a plan snapshot.
///
/// Emitted with no `auth_mode`: the presence of rate limits does not by
/// itself prove a subscription on Codex the way it does on Claude, and
/// the session's `account/read` already supplies an authoritative
/// answer. Leaving it `None` lets the sink keep the better value.
pub(crate) fn plan_usage_from_rate_limits(
    thread_id: &ThreadId,
    snapshot: &RateLimitSnapshot,
) -> Vec<ProviderRuntimeEvent> {
    let windows: Vec<PlanUsageWindow> = [
        codex_window(snapshot.primary.as_ref()),
        codex_window(snapshot.secondary.as_ref()),
    ]
    .into_iter()
    .flatten()
    .collect();

    if windows.is_empty() {
        return Vec::new();
    }
    vec![ProviderRuntimeEvent::PlanUsageUpdated {
        thread_id: thread_id.clone(),
        provider: ProviderKind::Codex,
        windows,
        plan_label: snapshot.limit_name.clone(),
        auth_mode: None,
    }]
}

/// Whether `thread_id` is this session's root (parent) Codex thread.
///
/// A default-constructed demux (unit tests, the stateless
/// `translate_notification` wrapper) has an empty parent id and carries
/// no baseline, so this reports false and the seeding path is simply
/// never taken.
fn is_root_thread(demux: &CodexSubagentDemux, thread_id: &str) -> bool {
    !demux.parent_thread_id.is_empty() && demux.parent_thread_id == thread_id
}

fn record_usage(
    demux: &mut CodexSubagentDemux,
    thread_id: &ThreadId,
    params: &ThreadTokenUsageUpdatedParams,
    is_subagent: bool,
) -> Vec<ProviderRuntimeEvent> {
    let current = UsageSplit::from_breakdown(&params.token_usage.total);
    let previous = match demux.usage_totals.get(&params.thread_id).copied() {
        // Already tracking this thread in-process — ordinary delta.
        Some(previous) => previous,
        // First sighting. If a baseline was seeded for the ROOT thread,
        // this is a resumed session and `current` already includes work
        // sitting in the ledger, so start from what we recorded rather
        // than from zero.
        //
        // `baseline_against` resolves the branch we cannot observe from
        // here — whether Codex carried its counters across the resume or
        // reset them — per field. Worst case is a bounded single-report
        // error in the rare mismatch, instead of re-recording the whole
        // thread on every resume.
        None if is_root_thread(demux, &params.thread_id) => {
            match demux.pending_baseline.take() {
                Some(baseline) => baseline.baseline_against(&current),
                None => UsageSplit::default(),
            }
        }
        None => UsageSplit::default(),
    };
    let delta = current.delta_since(&previous);

    // Store even when the delta is empty: `current` may have moved
    // backwards, and keeping the *observed* value (rather than the old
    // high-water) means a later genuine increase is measured from where
    // the provider actually is.
    demux
        .usage_totals
        .insert(params.thread_id.clone(), current);

    if delta.is_empty() {
        return Vec::new();
    }

    let model = demux.active_model.clone();
    let cost_usd = pricing::cost_for(
        model.as_deref(),
        delta.input,
        delta.output,
        delta.cache_read,
        delta.cache_write,
    );
    vec![ProviderRuntimeEvent::UsageRecorded {
        thread_id: thread_id.clone(),
        provider: ProviderKind::Codex,
        model,
        subagent: is_subagent,
        input_tokens: delta.input,
        output_tokens: delta.output,
        cache_read_tokens: delta.cache_read,
        cache_write_tokens: delta.cache_write,
        reasoning_tokens: delta.reasoning,
        cost_usd,
        // Codex has no rate catalogue — every priced Codex row comes
        // from the static table.
        cost_source: cost_usd.map(|_| CostSource::Table),
    }]
}

fn generic_wire_thread_id(msg: &NotificationMessage) -> Option<&str> {
    use NotificationMessage::*;
    match msg {
        AgentMessageDelta(p)
        | CommandExecutionOutputDelta(p)
        | FileChangeOutputDelta(p)
        | PlanDelta(p) => Some(&p.thread_id),
        TurnPlanUpdated(p) => Some(&p.thread_id),
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
        let mut demux = CodexSubagentDemux::new("c-1");
        demux.set_active_model(Some("gpt-5.4".into()));
        let msg = collab_item(json!({
            "type": "collabAgentToolCall", "id": "call-1", "tool": "spawnAgent",
            "status": "inProgress", "senderThreadId": "c-1", "receiverThreadIds": ["c-child"],
            "agentsStates": {"c-child": {"status": "pendingInit"}}
        }));
        let events = translate_notification_with(&mut demux, &tid(), msg);
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.status, SubagentStatus::Pending);
                assert_eq!(subagent.model.as_deref(), Some("gpt-5.4"));
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
    fn collab_non_spawn_call_leaves_model_unset() {
        // `model` is only ever reported on `spawnAgent`. A later `wait` (or
        // `sendInput` / `closeAgent`) must not stamp the *parent's* current
        // model onto the child — the consumer merges last-non-null-wins, so
        // that would silently overwrite an explicitly-routed child model.
        let mut demux = CodexSubagentDemux::new("c-1");
        demux.set_active_model(Some("gpt-5.4".into()));
        for tool in ["wait", "sendInput", "closeAgent", "resumeAgent"] {
            let msg = collab_item(json!({
                "type": "collabAgentToolCall", "id": "call-2", "tool": tool,
                "status": "completed", "senderThreadId": "c-1",
                "receiverThreadIds": ["c-child"],
                "agentsStates": {"c-child": {"status": "running"}}
            }));
            let events = translate_notification_with(&mut demux, &tid(), msg);
            match &events[0] {
                ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                    assert_eq!(subagent.model, None, "{tool} must not restamp model");
                }
                other => panic!("expected SubagentUpdated, got {other:?}"),
            }
        }
    }

    #[test]
    fn collab_explicit_model_wins_over_active_session_model() {
        let mut demux = CodexSubagentDemux::new("c-1");
        demux.set_active_model(Some("gpt-5.4".into()));
        let msg = collab_item(json!({
            "type": "collabAgentToolCall", "id": "call-1", "tool": "spawnAgent",
            "status": "completed", "senderThreadId": "c-1",
            "receiverThreadIds": ["c-child"], "model": "gpt-5.4-codex-mini",
            "agentsStates": {"c-child": {"status": "running"}}
        }));
        let events = translate_notification_with(&mut demux, &tid(), msg);
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.model.as_deref(), Some("gpt-5.4-codex-mini"));
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

    // ── Subagent demux: the parent thread must never become a sub-agent ──
    //
    // Observed in the wild: when a child messages the orchestrator, Codex
    // emits a `subAgentActivity` whose `agentThreadId` is the PARENT
    // thread. Registering it demoted the whole session — every later
    // parent item was tagged as a child transcript, and the parent's
    // `turn/completed` became a `SubagentUpdated` instead of a
    // `TurnCompleted`, so the run never settled ("Run interrupted" +
    // Continue forever, sidebar pinned at Working).

    /// Replay the observed sequence: spawn a child, then Codex reports an
    /// `interacted` activity naming the parent thread itself.
    fn demux_after_parent_named_as_activity() -> CodexSubagentDemux {
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
        let activity = NotificationMessage::from_raw(
            "item/completed",
            json!({"threadId": "c-1", "turnId": "pt-1", "item": {
                "type": "subAgentActivity", "id": "sa-root", "agentThreadId": "c-1",
                "agentPath": "root", "kind": "interacted"
            }}),
        );
        let events = translate_notification_with(&mut demux, &tid(), activity);
        assert!(
            !events.iter().any(|e| matches!(
                e,
                ProviderRuntimeEvent::SubagentUpdated { subagent, .. } if subagent.subagent_id == "c-1"
            )),
            "an activity naming the parent must not emit a SubagentUpdated for the parent: {events:?}"
        );
        demux
    }

    #[test]
    fn sub_agent_activity_naming_parent_does_not_register_parent() {
        let demux = demux_after_parent_named_as_activity();
        assert!(!demux.is_subagent("c-1"));
        assert!(demux.is_subagent("c-child"), "real child registration must be unaffected");
    }

    #[test]
    fn parent_turn_completed_still_settles_after_parent_named_as_activity() {
        let mut demux = demux_after_parent_named_as_activity();
        let msg = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId": "c-1", "turn": {"id": "pt-1", "status": "completed",
                "durationMs": 433439, "items": []}}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        assert!(
            events.iter().any(|e| matches!(
                e,
                ProviderRuntimeEvent::TurnCompleted { turn_id, status: TurnStatus::Success, .. }
                    if turn_id.0 == "pt-1"
            )),
            "parent turn/completed must emit TurnCompleted, got {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                ProviderRuntimeEvent::SessionStateChanged { status: SessionStatus::Ready, .. }
            )),
            "parent turn/completed must return the session to Ready, got {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, ProviderRuntimeEvent::SubagentUpdated { .. })),
            "parent turn/completed must not be demoted to SubagentUpdated: {events:?}"
        );
    }

    #[test]
    fn parent_transcript_item_stays_untagged_after_parent_named_as_activity() {
        let mut demux = demux_after_parent_named_as_activity();
        let msg = NotificationMessage::from_raw(
            "item/completed",
            json!({"threadId": "c-1", "turnId": "pt-1", "item": {
                "type": "agentMessage", "id": "am-1", "text": "parent text"}}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { subagent_id, .. } => {
                assert!(subagent_id.is_none(), "parent item was routed as a child: {events:?}");
            }
            other => panic!("expected ItemCompleted, got {other:?}"),
        }
    }

    #[test]
    fn thread_started_for_parent_with_parent_thread_id_stays_session_configured() {
        // Third registration path: a `thread/started` for the session's own
        // thread that (mis)reports a parentThreadId must keep the legacy
        // SessionConfigured mapping, not become a sub-agent identity.
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
            "thread/started",
            json!({"thread": {"id": "c-1", "parentThreadId": "c-child", "agentNickname": "root"}}),
        );
        let events = translate_notification_with(&mut demux, &tid(), msg);
        assert!(!demux.is_subagent("c-1"));
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(
            matches!(&events[0], ProviderRuntimeEvent::SessionConfigured { .. }),
            "expected SessionConfigured, got {events:?}"
        );
    }

    #[test]
    fn collab_call_listing_parent_does_not_register_parent() {
        // Belt-and-braces for the other registration path: a `wait` /
        // `sendInput` whose receiver list or agentsStates mentions the
        // orchestrator itself.
        let mut demux = CodexSubagentDemux::new("c-1");
        let events = translate_notification_with(
            &mut demux,
            &tid(),
            collab_item(json!({
                "type": "collabAgentToolCall", "id": "call-2", "tool": "wait",
                "status": "completed", "senderThreadId": "c-1",
                "receiverThreadIds": ["c-child", "c-1"],
                "agentsStates": {"c-child": {"status": "completed"}, "c-1": {"status": "running"}}
            })),
        );
        assert!(!demux.is_subagent("c-1"));
        assert!(demux.is_subagent("c-child"));
        assert!(
            !events.iter().any(|e| matches!(
                e,
                ProviderRuntimeEvent::SubagentUpdated { subagent, .. } if subagent.subagent_id == "c-1"
            )),
            "no SubagentUpdated for the parent: {events:?}"
        );
        assert_eq!(events.len(), 1, "only the real child gets a snapshot: {events:?}");
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

    // ── Context-window usage (thread/tokenUsage/updated) ──

    fn usage_snapshots(events: &[ProviderRuntimeEvent]) -> Vec<crate::agent_provider::ContextUsageSnapshot> {
        events
            .iter()
            .filter_map(|e| match e {
                ProviderRuntimeEvent::ContextUsageUpdated { usage, .. } => Some(usage.clone()),
                _ => None,
            })
            .collect()
    }

    fn token_usage_msg(
        wire_thread_id: &str,
        last: u64,
        total: u64,
        window: Option<u64>,
    ) -> NotificationMessage {
        NotificationMessage::from_raw(
            "thread/tokenUsage/updated",
            json!({
                "threadId": wire_thread_id,
                "turnId": "turn-1",
                "tokenUsage": {
                    "last": {"totalTokens": last},
                    "total": {"totalTokens": total},
                    "modelContextWindow": window
                }
            }),
        )
    }

    #[test]
    fn token_usage_emits_a_thread_scoped_context_snapshot() {
        let mut demux = CodexSubagentDemux::new("c-1");
        let events = translate_notification_with(
            &mut demux,
            &tid(),
            token_usage_msg("c-1", 24_542, 180_000, Some(272_000)),
        );
        let snaps = usage_snapshots(&events);
        assert_eq!(snaps.len(), 1);
        let snap = &snaps[0];
        // `last` is the live window occupancy; `total` is the
        // app-server's own lifetime counter, adopted verbatim.
        assert_eq!(snap.used_tokens, 24_542);
        assert_eq!(snap.total_processed_tokens, Some(180_000));
        assert_eq!(snap.max_tokens, Some(272_000));
        assert_eq!(snap.compacts_automatically, Some(true));
        assert!(snap.last_used_tokens.is_none());
    }

    #[test]
    fn token_usage_clamps_used_to_the_reported_window() {
        let mut demux = CodexSubagentDemux::new("c-1");
        let events = translate_notification_with(
            &mut demux,
            &tid(),
            token_usage_msg("c-1", 300_000, 400_000, Some(272_000)),
        );
        let snap = usage_snapshots(&events).remove(0);
        assert_eq!(snap.used_tokens, 272_000, "never renders above 100%");
        assert_eq!(snap.total_processed_tokens, Some(400_000));
    }

    #[test]
    fn token_usage_window_is_sticky_across_reports() {
        let mut demux = CodexSubagentDemux::new("c-1");
        translate_notification_with(
            &mut demux,
            &tid(),
            token_usage_msg("c-1", 1_000, 1_000, Some(272_000)),
        );
        // A later report omits the window; the known one must persist.
        let events =
            translate_notification_with(&mut demux, &tid(), token_usage_msg("c-1", 2_000, 5_000, None));
        assert_eq!(usage_snapshots(&events).remove(0).max_tokens, Some(272_000));
    }

    #[test]
    fn token_usage_lifetime_total_never_regresses() {
        let mut demux = CodexSubagentDemux::new("c-1");
        translate_notification_with(
            &mut demux,
            &tid(),
            token_usage_msg("c-1", 1_000, 90_000, None),
        );
        let events = translate_notification_with(
            &mut demux,
            &tid(),
            token_usage_msg("c-1", 2_000, 10_000, None),
        );
        assert_eq!(
            usage_snapshots(&events).remove(0).total_processed_tokens,
            Some(90_000)
        );
    }

    #[test]
    fn repeated_identical_token_usage_is_suppressed() {
        let mut demux = CodexSubagentDemux::new("c-1");
        let first = translate_notification_with(
            &mut demux,
            &tid(),
            token_usage_msg("c-1", 500, 500, None),
        );
        assert_eq!(usage_snapshots(&first).len(), 1);
        let second = translate_notification_with(
            &mut demux,
            &tid(),
            token_usage_msg("c-1", 500, 500, None),
        );
        assert!(usage_snapshots(&second).is_empty());
    }

    #[test]
    fn zero_token_usage_emits_nothing() {
        let mut demux = CodexSubagentDemux::new("c-1");
        let events =
            translate_notification_with(&mut demux, &tid(), token_usage_msg("c-1", 0, 0, None));
        assert!(usage_snapshots(&events).is_empty());
    }

    #[test]
    fn subagent_token_usage_does_not_feed_the_parent_meter() {
        // Usage hygiene: a sub-agent runs as its own thread against its
        // own window, so its report must not move the parent's meter.
        let mut demux = CodexSubagentDemux::new("c-1");
        translate_notification_with(
            &mut demux,
            &tid(),
            NotificationMessage::from_raw(
                "thread/started",
                json!({"thread": {"id": "c-child", "parentThreadId": "c-1"}}),
            ),
        );
        let events = translate_notification_with(
            &mut demux,
            &tid(),
            token_usage_msg("c-child", 500_000, 900_000, Some(272_000)),
        );
        assert!(
            usage_snapshots(&events).is_empty(),
            "child-thread usage must be dropped"
        );
    }

    #[test]
    fn structured_plan_maps_to_tasks_snapshot() {
        let events = translate_notification(
            &tid(),
            NotificationMessage::from_raw(
                "turn/plan/updated",
                json!({
                    "threadId": "c-1",
                    "explanation": "Finish the feature",
                    "plan": [
                        {"step": "Research", "status": "completed"},
                        {"step": "Implement", "status": "inProgress"},
                        {"step": "Test", "status": "pending"}
                    ]
                }),
            ),
        );
        match &events[0] {
            ProviderRuntimeEvent::TasksUpdated { tasks, .. } => {
                assert_eq!(tasks.explanation.as_deref(), Some("Finish the feature"));
                assert_eq!(tasks.tasks[0].status, TaskStatus::Completed);
                assert_eq!(tasks.tasks[1].status, TaskStatus::InProgress);
                assert_eq!(tasks.tasks[2].status, TaskStatus::Pending);
            }
            other => panic!("expected TasksUpdated, got {other:?}"),
        }
    }
}

// ── usage ledger ──

#[cfg(test)]
mod usage_ledger_tests {
    use super::*;
    use serde_json::json;

    fn breakdown(
        total: u64,
        input: u64,
        cached: u64,
        cache_write: u64,
        output: u64,
        reasoning: u64,
    ) -> TokenUsageBreakdown {
        serde_json::from_value(json!({
            "totalTokens": total,
            "inputTokens": input,
            "cachedInputTokens": cached,
            "cacheWriteInputTokens": cache_write,
            "outputTokens": output,
            "reasoningOutputTokens": reasoning,
        }))
        .unwrap()
    }

    /// The exact numbers from the protocol decode fixture. They are the
    /// evidence that both cache figures are carved OUT of `inputTokens`:
    /// `23_863 + 679 == 24_542`, the app-server's own total.
    #[test]
    fn normalization_carves_both_cache_tiers_out_of_input() {
        let split = UsageSplit::from_breakdown(&breakdown(24_542, 23_863, 21_144, 2_715, 679, 512));
        assert_eq!(split.cache_read, 21_144);
        assert_eq!(split.cache_write, 2_715);
        assert_eq!(split.input, 23_863 - 21_144 - 2_715);
        // Reasoning stays folded into output — that is how it is billed.
        assert_eq!(split.output, 679);
        // The whole point: the disjoint buckets reproduce Codex's total.
        let sum = split.input + split.output + split.cache_read + split.cache_write;
        assert_eq!(sum, 24_542);
    }

    #[test]
    fn normalization_saturates_rather_than_wrapping() {
        // Nonsense payload where the carve-outs exceed the input figure.
        let split = UsageSplit::from_breakdown(&breakdown(0, 10, 50, 50, 5, 0));
        assert_eq!(split.input, 0);
        assert_eq!(split.output, 5);
    }

    fn usage_params(thread: &str, total: TokenUsageBreakdown) -> ThreadTokenUsageUpdatedParams {
        serde_json::from_value(json!({
            "threadId": thread,
            "turnId": "turn-1",
            "tokenUsage": {
                "last": {"totalTokens": total.total_tokens},
                "total": {
                    "totalTokens": total.total_tokens,
                    "inputTokens": total.input_tokens,
                    "cachedInputTokens": total.cached_input_tokens,
                    "cacheWriteInputTokens": total.cache_write_input_tokens,
                    "outputTokens": total.output_tokens,
                    "reasoningOutputTokens": total.reasoning_output_tokens,
                },
                "modelContextWindow": 200_000,
            }
        }))
        .unwrap()
    }

    fn reasoning_of(events: &[ProviderRuntimeEvent]) -> Vec<u64> {
        events
            .iter()
            .filter_map(|e| match e {
                ProviderRuntimeEvent::UsageRecorded {
                    reasoning_tokens, ..
                } => Some(*reasoning_tokens),
                _ => None,
            })
            .collect()
    }

    fn recorded(events: &[ProviderRuntimeEvent]) -> Vec<(u64, u64, u64, u64, bool, Option<String>)> {
        events
            .iter()
            .filter_map(|e| match e {
                ProviderRuntimeEvent::UsageRecorded {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    subagent,
                    model,
                    ..
                } => Some((
                    *input_tokens,
                    *output_tokens,
                    *cache_read_tokens,
                    *cache_write_tokens,
                    *subagent,
                    model.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Codex re-sends a cumulative lifetime total, so consecutive
    /// reports must yield the growth between them — never the total
    /// again.
    #[test]
    fn cumulative_totals_become_deltas() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.set_active_model(Some("gpt-5-codex".into()));
        let tid = ThreadId("local".into());

        let first = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_000, 800, 0, 0, 200, 0)),
        );
        assert_eq!(recorded(&first)[0].0, 800);
        assert_eq!(recorded(&first)[0].1, 200);

        let second = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_500, 1_100, 0, 0, 400, 0)),
        );
        let rows = recorded(&second);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 300, "input delta");
        assert_eq!(rows[0].1, 200, "output delta");
    }

    /// An unchanged total is not work — it must not create a row.
    #[test]
    fn repeated_identical_total_records_nothing() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        let tid = ThreadId("local".into());
        let params = usage_params("thr-parent", breakdown(1_000, 800, 0, 0, 200, 0));
        assert_eq!(recorded(&handle_thread_token_usage(&mut demux, &tid, &params)).len(), 1);
        assert_eq!(recorded(&handle_thread_token_usage(&mut demux, &tid, &params)).len(), 0);
    }

    /// A total that goes backwards contributes zero rather than
    /// wrapping into a colossal charge.
    #[test]
    fn regressing_total_records_nothing() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        let tid = ThreadId("local".into());
        handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_000, 800, 0, 0, 200, 0)),
        );
        let back = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(400, 300, 0, 0, 100, 0)),
        );
        assert!(recorded(&back).is_empty());
    }

    /// The context-meter guard drops subagent threads; the ledger must
    /// not. This is the whole reason the two paths are separate.
    #[test]
    fn subagent_threads_are_recorded_and_flagged_but_move_no_meter() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.register("thr-child", "thr-parent");
        let tid = ThreadId("local".into());

        let events = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-child", breakdown(500, 400, 0, 0, 100, 0)),
        );
        let rows = recorded(&events);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].4, "subagent flag must be set");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ProviderRuntimeEvent::ContextUsageUpdated { .. })),
            "subagent usage must not touch the composer meter"
        );
    }

    /// Parent and child threads keep independent counters, so a child's
    /// first (large) total is not read as a parent-side jump.
    #[test]
    fn parent_and_child_totals_do_not_interfere() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.register("thr-child", "thr-parent");
        let tid = ThreadId("local".into());

        handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(10_000, 9_000, 0, 0, 1_000, 0)),
        );
        let child = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-child", breakdown(100, 80, 0, 0, 20, 0)),
        );
        let rows = recorded(&child);
        assert_eq!(rows[0].0, 80, "child starts from its own zero");
    }

    // ── resume baseline (min-clamp) ──

    /// The bug this guards: Codex's `total` is a provider-maintained
    /// LIFETIME counter that survives `thread/resume`, but the demux is
    /// rebuilt with the session. Unseeded, the first post-resume report
    /// re-records the whole thread.
    #[test]
    fn resumed_thread_with_surviving_totals_records_only_new_work() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        // 900 in + 100 out already written to the ledger before the restart.
        demux.set_recorded_usage_baseline(UsageBaseline {
            input_tokens: 900,
            output_tokens: 100,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        });
        let tid = ThreadId("local".into());

        // Codex reports the surviving lifetime total plus one new turn.
        let events = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_250, 1_050, 0, 0, 200, 0)),
        );
        let r = recorded(&events);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, 150, "only input beyond the baseline");
        assert_eq!(r[0].1, 100, "only output beyond the baseline");
    }

    /// A baseline is consumed once. After it applies, the thread keeps
    /// producing ordinary in-process deltas.
    #[test]
    fn baseline_applies_once_then_normal_deltas_resume() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.set_recorded_usage_baseline(UsageBaseline {
            input_tokens: 900,
            output_tokens: 100,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        });
        let tid = ThreadId("local".into());
        handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_250, 1_050, 0, 0, 200, 0)),
        );
        let second = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_400, 1_150, 0, 0, 250, 0)),
        );
        let r = recorded(&second);
        assert_eq!(r[0].0, 100);
        assert_eq!(r[0].1, 50);
    }

    /// The resolver's reason for existing: if Codex RESET its counters
    /// instead of carrying them, a stale baseline would act as a
    /// high-water mark and swallow the fresh usage entirely. Note a bare
    /// `min` does NOT fix this — `delta_since` already saturates — which
    /// is why the reset branch drops the baseline to zero.
    #[test]
    fn reset_counters_below_the_baseline_still_record() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.set_recorded_usage_baseline(UsageBaseline {
            input_tokens: 900,
            output_tokens: 100,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        });
        let tid = ThreadId("local".into());
        // Fresh counters: far below the baseline.
        let events = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(60, 50, 0, 0, 10, 0)),
        );
        let r = recorded(&events);
        assert_eq!(r.len(), 1, "the work must not be swallowed");
        assert_eq!(r[0].0, 50, "min() clamped the baseline down to current");
        assert_eq!(r[0].1, 10);
    }

    /// One counter carrying over while another resets must be handled
    /// per field, not by classifying the whole split one way.
    #[test]
    fn partial_counter_reset_is_resolved_per_field() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.set_recorded_usage_baseline(UsageBaseline {
            input_tokens: 900,
            output_tokens: 100,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        });
        let tid = ThreadId("local".into());
        // input carried over (1_000 >= 900); output reset (40 < 100).
        let events = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_040, 1_000, 0, 0, 40, 0)),
        );
        let r = recorded(&events);
        assert_eq!(r[0].0, 100, "input measured from the surviving baseline");
        assert_eq!(r[0].1, 40, "output recorded in full after its reset");
    }

    /// Reasoning is cumulative like everything else, so it must be
    /// deltaed — and it must stay INSIDE output, never added on top.
    #[test]
    fn reasoning_is_deltaed_and_stays_inside_output() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        let tid = ThreadId("local".into());
        let first = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_000, 800, 0, 0, 200, 120)),
        );
        let r = reasoning_of(&first);
        assert_eq!(r, vec![120]);
        assert_eq!(recorded(&first)[0].1, 200, "output still the full 200");

        let second = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_400, 1_000, 0, 0, 400, 260)),
        );
        assert_eq!(reasoning_of(&second), vec![140], "delta, not the total");
    }

    /// The resume baseline needs a reasoning term for the same reason
    /// the other columns do: without it a resumed thread re-reports its
    /// whole reasoning history on the first post-resume notification.
    #[test]
    fn resume_baseline_covers_reasoning_too() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.set_recorded_usage_baseline(UsageBaseline {
            input_tokens: 800,
            output_tokens: 200,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 120,
        });
        let tid = ThreadId("local".into());
        let events = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_400, 1_000, 0, 0, 400, 260)),
        );
        assert_eq!(
            reasoning_of(&events),
            vec![140],
            "only the reasoning beyond the baseline"
        );
        assert_eq!(recorded(&events)[0].1, 200, "output delta unaffected");
    }

    /// An unseeded thread behaves exactly as before the baseline existed.
    #[test]
    fn unseeded_threads_are_unaffected() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        let tid = ThreadId("local".into());
        let events = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_000, 800, 0, 0, 200, 0)),
        );
        let r = recorded(&events);
        assert_eq!(r[0].0, 800, "full first total, as before");
        assert_eq!(r[0].1, 200);
    }

    /// A zero baseline is a no-op — a fresh thread needs no clamping.
    #[test]
    fn zero_baseline_is_not_stored() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.set_recorded_usage_baseline(UsageBaseline::default());
        assert!(demux.pending_baseline.is_none());
    }

    /// The baseline is for the ROOT thread only. Subagent child threads
    /// are never resumed, so seeding against them would suppress real
    /// work on the child's very first report.
    #[test]
    fn baseline_never_applies_to_subagent_threads() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.register("thr-child", "thr-parent");
        demux.set_recorded_usage_baseline(UsageBaseline {
            input_tokens: 10_000,
            output_tokens: 10_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        });
        let tid = ThreadId("local".into());
        let events = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-child", breakdown(500, 400, 0, 0, 100, 0)),
        );
        let r = recorded(&events);
        assert_eq!(r[0].0, 400, "child records in full");
        assert!(r[0].4, "and is still flagged as a subagent");
        assert!(
            demux.pending_baseline.is_some(),
            "the root's baseline must still be waiting for the root"
        );
    }

    #[test]
    fn model_comes_from_session_state_and_prices_the_row() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.set_active_model(Some("gpt-5-codex".into()));
        let tid = ThreadId("local".into());
        let events = handle_thread_token_usage(
            &mut demux,
            &tid,
            &usage_params("thr-parent", breakdown(1_000, 800, 0, 0, 200, 0)),
        );
        let cost = events.iter().find_map(|e| match e {
            ProviderRuntimeEvent::UsageRecorded { cost_usd, .. } => *cost_usd,
            _ => None,
        });
        assert_eq!(recorded(&events)[0].5.as_deref(), Some("gpt-5-codex"));
        assert!(cost.unwrap() > 0.0);
    }
}

// ── plan quota (account/rateLimits) ──

#[cfg(test)]
mod plan_quota_tests {
    use super::*;
    use serde_json::json;

    fn snapshot(value: serde_json::Value) -> RateLimitSnapshot {
        serde_json::from_value(value).unwrap()
    }

    /// Codex names windows only by duration, so the mapping is by band.
    #[test]
    fn window_kind_is_inferred_from_duration() {
        assert_eq!(plan_window_kind(Some(300.0)).0, PlanWindowKind::FiveHour);
        assert_eq!(plan_window_kind(Some(10_080.0)).0, PlanWindowKind::SevenDay);
        // Bands are generous — the exact figure has drifted between plans.
        assert_eq!(plan_window_kind(Some(240.0)).0, PlanWindowKind::FiveHour);
        assert_eq!(plan_window_kind(Some(10_000.0)).0, PlanWindowKind::SevenDay);
    }

    /// An unfamiliar window must still surface, with a readable name —
    /// not vanish.
    #[test]
    fn unknown_durations_degrade_to_other_with_a_label() {
        let (kind, label) = plan_window_kind(Some(1_440.0));
        assert_eq!(kind, PlanWindowKind::Other);
        assert_eq!(label.as_deref(), Some("1d"));
        assert_eq!(plan_window_kind(Some(90.0)).1.as_deref(), Some("2h"));
        assert_eq!(plan_window_kind(Some(30.0)).1.as_deref(), Some("30m"));
        // No duration at all is still `Other`, just unnamed.
        assert_eq!(plan_window_kind(None), (PlanWindowKind::Other, None));
    }

    /// `usedPercent` is ALREADY a percent here — the opposite of Claude's
    /// 0..1 `utilization`. No scaling, only clamping.
    #[test]
    fn used_percent_is_not_rescaled_but_is_clamped() {
        let events = plan_usage_from_rate_limits(
            &ThreadId("t".into()),
            &snapshot(json!({
                "primary": {"usedPercent": 41.0, "windowDurationMins": 300},
                "secondary": {"usedPercent": 143.0, "windowDurationMins": 10080},
            })),
        );
        match &events[0] {
            ProviderRuntimeEvent::PlanUsageUpdated { windows, provider, .. } => {
                assert_eq!(*provider, ProviderKind::Codex);
                assert_eq!(windows[0].used_pct, 41.0, "no x100");
                assert_eq!(windows[1].used_pct, 100.0, "clamped");
            }
            other => panic!("expected PlanUsageUpdated, got {other:?}"),
        }
    }

    /// `primary` is the 5h window, `secondary` the weekly one, and
    /// `resetsAt` is epoch seconds.
    #[test]
    fn primary_and_secondary_map_to_five_hour_and_weekly() {
        let events = plan_usage_from_rate_limits(
            &ThreadId("t".into()),
            &snapshot(json!({
                "limitName": "ChatGPT Pro",
                "primary": {
                    "usedPercent": 12.0,
                    "windowDurationMins": 300,
                    "resetsAt": 1_800_000_000_i64,
                },
                "secondary": {"usedPercent": 60.0, "windowDurationMins": 10080},
            })),
        );
        match &events[0] {
            ProviderRuntimeEvent::PlanUsageUpdated {
                windows,
                plan_label,
                auth_mode,
                ..
            } => {
                assert_eq!(windows[0].kind, PlanWindowKind::FiveHour);
                assert_eq!(windows[0].resets_at_ms, Some(1_800_000_000_000));
                assert_eq!(windows[1].kind, PlanWindowKind::SevenDay);
                assert_eq!(plan_label.as_deref(), Some("ChatGPT Pro"));
                // The push says nothing authoritative about auth mode —
                // `account/read` does — so it must not overwrite it.
                assert!(auth_mode.is_none());
            }
            other => panic!("expected PlanUsageUpdated, got {other:?}"),
        }
    }

    /// A metered API-key account reports no windows; emitting an event
    /// with an empty list would blank the store's merged state.
    #[test]
    fn no_windows_emits_nothing() {
        assert!(plan_usage_from_rate_limits(&ThreadId("t".into()), &snapshot(json!({}))).is_empty());
    }

    /// The notification decodes off the wire (camelCase) rather than
    /// falling through to `Unknown`, which is what used to happen.
    #[test]
    fn notification_decodes_instead_of_falling_through_to_unknown() {
        let msg = NotificationMessage::from_raw(
            "account/rateLimits/updated",
            json!({"rateLimits": {
                "primary": {"usedPercent": 5.0, "windowDurationMins": 300}
            }}),
        );
        match msg {
            NotificationMessage::AccountRateLimitsUpdated(p) => {
                assert_eq!(p.rate_limits.primary.unwrap().used_percent, 5.0);
            }
            other => panic!("expected AccountRateLimitsUpdated, got {other:?}"),
        }
    }

    /// End-to-end through the dispatcher, and NOT routed via the subagent
    /// demux — quota describes the login, not a thread.
    #[test]
    fn dispatch_emits_plan_usage_for_any_thread() {
        let mut demux = CodexSubagentDemux::new("thr-parent");
        demux.register("thr-child", "thr-parent");
        let events = translate_notification_with(
            &mut demux,
            &ThreadId("t".into()),
            NotificationMessage::from_raw(
                "account/rateLimits/updated",
                json!({"rateLimits": {
                    "primary": {"usedPercent": 7.0, "windowDurationMins": 300}
                }}),
            ),
        );
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::PlanUsageUpdated { .. }
        ));
    }
}
