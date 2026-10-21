//! Durable app-owned delegation. Memory tracks live provider activity;
//! SQLite owns task identity, immutable results and completion delivery.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::agent_provider::{
    CompletedItem, ProviderKind, ProviderRuntimeEvent, SessionStatus, StartSessionInput,
    SubagentSnapshot, SubagentStatus, SubagentTaskKind, ThreadId, TurnStatus,
};
use crate::database::delegation_tasks::{
    PendingRequest, PublishedResult, TaskRecord, TaskStatus, WorkState,
};
use crate::database::DatabaseStore;
use crate::mcp::registry::{HostCaller, HostTools};
use crate::mcp::runtime::{prefix_tool_name, McpTool};
use crate::state::AppStateStore;

use super::agent_chat::{
    fallback_permission_mode, forward_event,
    resolve_start_permission_mode, send_turn_with_context, thread_id_for_event, TurnOrigin,
};
use super::usage_resume::{self, now_ms, queued_turn};

/// Settings key for the off switch. Absent or anything but `"false"` means
/// enabled — the tool only acts when the user asks for another agent.
pub const DELEGATION_SETTING_KEY: &str = "agents.cross_provider_delegation";
/// Agent-facing header. Presentation uses structured notification metadata.
pub const DELEGATION_RESULTS_PREFIX: &str = "[Codemux: delegated task results]";
/// Marks a parent's card row as a delegated task (`delegate:<child thread>`).
pub const DELEGATED_ROW_PREFIX: &str = "delegate:";
const TOOL_NAME: &str = "delegate_task";

const MAX_RUNNING_PER_PARENT: usize = 3;
const MAX_PER_ROUND: u32 = 6;
const MIN_TASK_CHARS: usize = 20;
const MAX_TITLE_CHARS: usize = 60;
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// Lets a provider finish its own bookkeeping after `TurnCompleted` before
/// the idle check reads it.
const SETTLE_DELAY: Duration = Duration::from_secs(1);
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
const SWEEP_IDLE_PASSES: u8 = 2;
const CARD_RESULT_CHARS: usize = 4_000;
const REASON_CHARS: usize = 200;
/// Agents that can take a task, in the order the tool lists them. Hermes
/// needs a profile binding and has no tool path, so it is left out.
pub(super) const CANDIDATES: [ProviderKind; 5] = [
    ProviderKind::Claude,
    ProviderKind::Codex,
    ProviderKind::OpenCode,
    ProviderKind::Cursor,
    ProviderKind::Grok,
];

/// Agent-only instructions; display text contains the supplied brief.
const CHILD_FOOTER: &str = "\n\n---\nFrom Codemux: another coding agent handed you this task and will read your final message. Work independently. Ask a question if truly blocked; it reaches the coordinator. Otherwise choose a safe option and explain it. End with a short report: what you changed (files), how you checked it, and anything left undone.";
/// The footer for a started suggested task, which runs on its own branch.
pub(crate) const WORKTREE_CHILD_FOOTER: &str = "\n\n---\nFrom Codemux: the user started this task from another chat's suggestions; that chat will read your final message. You are in your own git worktree on a new branch made from that chat's latest commit (its uncommitted edits are not here). Work independently. Ask a question if truly blocked; it reaches the coordinator. Otherwise choose a safe option and explain it. When done, COMMIT all of your work on this branch (do not push, do not switch branches) so it can be merged back; leave nothing uncommitted. End with a short report: what you changed (files), the commit(s), how you checked it, and anything left undone.";

const TOOL_DESCRIPTION: &str = "Delegate a self-contained task to another installed coding agent in a background chat on this device. Each child gets its own workspace, nested under yours in the same folder. The child sees only the brief. Results arrive independently through durable automatic delivery. Use client_request_id for retries: the same key and unchanged task arguments return the same task and child; async/wait controls may change on a retry. mode defaults to async; wait waits at most 20 seconds without cancelling on timeout. Use task_status/task_cancel with task_id; child_thread_id is a different identity. Read a complete terminal result with task_status to acknowledge its automatic delivery. Chat links use codemux-chat://v1/<threadId>. For another independent review round, delegate again with a fresh key and complete brief. End your turn after async delegation; do not poll. If the brief asks you to report to a coordinator, use thread_send to that chat.";

mod durable;
#[cfg(test)]
mod cancellation_tests;
pub(crate) use durable::{
    local_task_call, preparation_failed, record_of, restoration_held, stop_confirmed,
};
#[tauri::command]
pub async fn delegation_local_context<R: Runtime>(
    app: AppHandle<R>,
    thread_id: String,
) -> Result<Value, String> {
    durable::local_context(&app, &thread_id)
}
use durable::*;

// ── State ────────────────────────────────────────────────────────────

/// How a thread was stopped, from the hook that saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopKind {
    /// Stop button (`agent_chat_interrupt_turn`).
    Stop,
    /// Pane, tab or workspace closed, or the pane rebound to a new thread.
    Close,
    /// Session restarted in place (`agent_chat_stop_session`).
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    User,
    AgentCancellation,
    TabClosed,
    /// A worktree task's workspace was closed or archived.
    WorkspaceClosed,
    Restarted,
    Parent,
    AppClosed,
}

impl StopReason {
    fn note(self) -> &'static str {
        match self {
            Self::User => "The user stopped this task. Partial changes may be in the working tree.",
            Self::AgentCancellation => "The parent agent cancelled this task. Partial changes may be in the working tree.",
            Self::TabClosed => {
                "The user closed its chat. Partial changes may be in the working tree."
            }
            Self::WorkspaceClosed => {
                "The user closed its chat or workspace. Its branch keeps whatever it committed."
            }
            Self::Restarted => {
                "Its chat was restarted or replaced before it finished; partial work is in its workspace."
            }
            Self::Parent => "Stopped together with this chat.",
            Self::AppClosed => "Codemux closed before this task finished.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Starting,
    /// The child accepted the delegated turn. Nothing finalizes a task
    /// before this, however long its start takes.
    Running,
    Completed(String),
    Failed(String),
    Stopped(StopReason),
}

impl Phase {
    fn is_terminal(&self) -> bool {
        !matches!(self, Self::Starting | Self::Running)
    }
}

/// Where a task's child chat runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Placement {
    /// `delegate_task`: its own background workspace nested under the
    /// parent's, in the same folder. It closes when the chat that
    /// delegated it closes.
    #[default]
    Shared,
    /// A suggested task the user started: its own workspace on its own git
    /// worktree branch.
    Worktree(WorktreePlacement),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreePlacement {
    pub branch: String,
    /// `<suggest_tasks tool call id>#<index>`: the card row this task
    /// fills, sent as the row's `parent_item_id`.
    pub suggestion: String,
}

impl Placement {
    fn is_shared(&self) -> bool {
        matches!(self, Self::Shared)
    }

    /// Where the user goes to watch the child, for activity lines.
    fn place(&self) -> &'static str {
        "its workspace"
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Task {
    task_id: String,
    tool_call_id: Option<String>,
    role: Option<String>,
    stopping: bool,
    stop_reason: Option<StopReason>,
    parent: String,
    provider: ProviderKind,
    title: String,
    /// The task text as the parent wrote it, without the child footer.
    brief: String,
    model: Option<String>,
    effort: Option<String>,
    round: u64,
    started_at_ms: i64,
    finished_at_ms: Option<i64>,
    phase: Phase,
    /// Nothing left to post: included in a results message, or dropped.
    last_text: Option<String>,
    /// Persisted row of `last_text`, for a `conversation_read` cursor.
    last_text_row: Option<i64>,
    /// Status of the child's latest finished turn; `None` while one runs.
    last_status: Option<TurnStatus>,
    open_requests: HashSet<String>,
    asking: bool,
    paused_until: Option<i64>,
    usage_reset_ms: Option<i64>,
    idle_passes: u8,
    /// Activity line last emitted, so cards go out on change only.
    activity: String,
    placement: Placement,
}

impl Task {
    pub(crate) fn new(
        parent: &str,
        provider: ProviderKind,
        title: &str,
        brief: &str,
        started_at_ms: i64,
    ) -> Self {
        Self {
            task_id: format!("local:{}", uuid::Uuid::new_v4()),
            tool_call_id: None,
            role: None,
            stopping: false,
            stop_reason: None,
            parent: parent.to_string(),
            provider,
            title: title.to_string(),
            brief: brief.to_string(),
            model: None,
            effort: None,
            round: 0,
            started_at_ms,
            finished_at_ms: None,
            phase: Phase::Starting,
            last_text: None,
            last_text_row: None,
            last_status: None,
            open_requests: HashSet::new(),
            asking: false,
            paused_until: None,
            usage_reset_ms: None,
            idle_passes: 0,
            activity: String::new(),
            placement: Placement::Shared,
        }
    }

    /// A started suggested task: no caps, its own workspace and branch.
    pub(crate) fn in_worktree(mut self, placement: WorktreePlacement) -> Self {
        self.placement = Placement::Worktree(placement);
        self
    }

    pub(crate) fn with_identity(mut self, task_id: String) -> Self {
        self.task_id = task_id;
        self
    }

    pub(crate) fn with_model(mut self, model: Option<String>, effort: Option<String>) -> Self {
        self.model = model;
        self.effort = effort;
        self
    }

    /// Recompute the activity line; true when it moved, so the card is stale.
    fn refresh_activity(&mut self) -> bool {
        let activity = activity_line(self);
        let changed = activity != self.activity;
        self.activity = activity;
        changed
    }

    /// Apply one of the child's events. Returns whether the child may just
    /// have gone idle (worth an idle check), and a failure that ends the task.
    fn observe(
        &mut self,
        event: &ProviderRuntimeEvent,
        persisted_id: Option<i64>,
        open_question: bool,
    ) -> (bool, Option<String>) {
        let mut settle = false;
        let mut failure = None;
        match event {
            ProviderRuntimeEvent::ItemCompleted {
                item: CompletedItem::AssistantText { text },
                subagent_id: None,
                ..
            } if !text.trim().is_empty() => {
                self.last_text = Some(text.clone());
                self.last_text_row = persisted_id;
            }
            ProviderRuntimeEvent::TurnCompleted { status, .. } => {
                self.last_status = Some(status.clone());
                // A finished turn waits on no approval or input request.
                // Codex never reports a request as resolved, so this is
                // where its requests close if the answer was not seen.
                if !matches!(status, TurnStatus::Success) {
                    self.open_requests.clear();
                }
                settle = true;
            }
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Running { .. },
                ..
            } => {
                self.last_status = None;
                self.paused_until = None;
            }
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Closed | SessionStatus::Error { .. },
                ..
            } => {
                // Stop, close and restarts end the task before their session
                // closes, so a session ending here is the child dying. A turn
                // it never finished, or a question it can no longer take an
                // answer to, is a failure, not a wait on the user.
                let waiting = self.awaits_user();
                self.open_requests.clear();
                if waiting || self.last_status.is_none() {
                    self.last_status = Some(TurnStatus::Error {
                        subtype: "session_ended".into(),
                        message: if waiting {
                            "Its session ended while it was waiting for your answer.".into()
                        } else {
                            "Its session ended before it finished.".into()
                        },
                    });
                }
                settle = true;
            }
            ProviderRuntimeEvent::RequestOpened { request_id, .. } => {
                self.open_requests.insert(request_id.0.clone());
            }
            ProviderRuntimeEvent::RequestResolved { request_id, .. }
            | ProviderRuntimeEvent::RequestResponseFailed { request_id, .. } => {
                self.open_requests.remove(&request_id.0);
                settle = self.last_status.is_some();
            }
            ProviderRuntimeEvent::QuestionsAsked { .. } => self.asking = true,
            ProviderRuntimeEvent::QuestionResolved { .. } => {
                self.asking = open_question;
                settle = self.last_status.is_some();
            }
            ProviderRuntimeEvent::UsageLimitReached {
                resets_at_ms,
                auto_resume_at_ms,
                ..
            } => {
                self.usage_reset_ms = *resets_at_ms;
                match auto_resume_at_ms {
                    Some(at) => self.paused_until = Some(*at),
                    None => failure = Some(one_line(&usage_limit_reason(*resets_at_ms))),
                }
            }
            ProviderRuntimeEvent::UsageResumeCancelled { .. } => {
                settle = self.paused_until.take().is_some();
            }
            _ => {}
        }
        // Only a dispatched task can finish.
        (settle && self.phase == Phase::Running, failure)
    }

    /// Whether the child is waiting on the user: an open question or
    /// approval, unless its turn already failed (a question left open by a
    /// failed or dead child will never be answered into this task).
    fn awaits_user(&self) -> bool {
        !self.turn_failed() && (self.asking || !self.open_requests.is_empty())
    }

    fn turn_failed(&self) -> bool {
        self.last_status
            .as_ref()
            .is_some_and(|status| !matches!(status, TurnStatus::Success))
    }

    /// How the task ends if its child is idle now (`finalize_outcome`).
    fn outcome(&self) -> Phase {
        finalize_outcome(
            self.last_status.as_ref(),
            self.last_text.as_deref(),
            self.usage_reset_ms,
        )
    }
}

#[derive(Debug, Default)]
struct ParentState {
    round: u64,
    started_this_round: u32,
    /// Every task this parent delegated in this run, oldest first, kept
    /// after the tasks are forgotten: a new tab goes after their tabs, so
    /// the tab bar reads in the order the tasks were started.
    children: Vec<String>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Keyed by child thread id.
    tasks: HashMap<String, Task>,
    wait_leases: HashMap<String, usize>,
    parents: HashMap<String, ParentState>,
}

/// Every delegated task of this process, behind one mutex. Managed in the
/// builder so hooks on hot paths can `try_state` it and return at once.
#[derive(Debug, Default)]
pub struct DelegationState {
    inner: Mutex<Inner>,
}

impl DelegationState {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn admit<T>(&self, control: Option<&crate::agent_control::ControlCaller>, work: impl FnOnce(&mut Inner) -> Result<T, String>) -> Result<T, String> {
        // Always take caller authority before live delegation state. Drop both
        // before recursive revocation or asynchronous provider work.
        let admission = || work(&mut self.lock());
        match control { Some(control) => control.admit(admission), None => admission() }
    }

    fn task_matches(&self, child: &str, test: impl FnOnce(&Task) -> bool) -> bool {
        self.lock().tasks.get(child).is_some_and(test)
    }

    pub(super) fn is_running_child(&self, thread_id: &str) -> bool {
        self.task_matches(thread_id, |task| !task.phase.is_terminal())
    }

    /// Reserved and not yet sent its task. A parent Stop or Close ends that
    /// (`on_thread_stopped`), after which nothing may launch the task.
    fn is_starting(&self, child: &str) -> bool {
        self.task_matches(child, |task| {
            task.phase == Phase::Starting && !task.stopping
        })
    }

    /// Apply `change` to `child`'s live task, then re-emit its card if the
    /// activity line moved. `None` when the child has no live task.
    fn update<R: Runtime, T>(
        &self,
        app: &AppHandle<R>,
        child: &str,
        change: impl FnOnce(&mut Task) -> T,
    ) -> Option<T> {
        let (out, changed) = {
            let mut inner = self.lock();
            let task = inner
                .tasks
                .get_mut(child)
                .filter(|task| !task.phase.is_terminal())?;
            let out = change(task);
            (out, task.refresh_activity())
        };
        persist_live(app, child);
        if changed {
            self.emit_card(app, child);
        }
        Some(out)
    }

    /// Emit the current card for `child` on its parent, through
    /// `forward_event` so it is persisted and hydrated like any subagent row.
    pub(super) fn emit_card<R: Runtime>(&self, app: &AppHandle<R>, child: &str) {
        let card = self
            .lock()
            .tasks
            .get(child)
            .map(|task| (task.parent.clone(), snapshot(child, task)));
        if let Some((parent, subagent)) = card {
            forward_event(
                app,
                ProviderRuntimeEvent::SubagentUpdated {
                    thread_id: ThreadId(parent),
                    subagent,
                },
            );
            let parent = self.lock().tasks.get(child).map(|t| t.parent.clone());
            if let Some(parent) = parent {
                let _ = app.emit("delegation-changed", json!({"parent_thread_id":parent}));
            }
        }
    }

    /// Move a task to a terminal phase (check-and-set: a terminal task stays
    /// as it is), persist its result, emit its card and cancel any usage
    /// resume armed on the child, so nothing restarts it later. Returns the
    /// parent when this call made the change.
    fn finish<R: Runtime>(&self, app: &AppHandle<R>, child: &str, phase: Phase) -> Option<String> {
        let (parent, worktree_outcome) = {
            let mut inner = self.lock();
            let task = inner
                .tasks
                .get_mut(child)
                .filter(|task| !task.phase.is_terminal())?;
            task.phase = phase;
            task.finished_at_ms = Some(now_ms());
            task.refresh_activity();
            let worktree_outcome = (!task.placement.is_shared()).then(|| outcome_id(&task.phase));
            (task.parent.clone(), worktree_outcome)
        };
        if let Err(error) = publish_finished(app, child) {
            log::error!("Task result publication retained for repair: {error}");
            return None;
        }
        self.emit_card(app, child);
        usage_resume::cancel_for_stopped_thread(app, child);
        if let Some(outcome) = worktree_outcome {
            super::follow_ups::on_task_finished(app, child, outcome);
        }
        Some(parent)
    }
}

/// The stored outcome of a finished task.
fn outcome_id(phase: &Phase) -> &'static str {
    match phase {
        Phase::Completed(_) => "completed",
        Phase::Failed(_) => "failed",
        Phase::Stopped(_) => "stopped",
        Phase::Starting | Phase::Running => "running",
    }
}

/// Whether a card row is a delegated task rather than a native subagent.
pub fn is_delegated_row(subagent_id: &str) -> bool {
    subagent_id.starts_with(DELEGATED_ROW_PREFIX)
}

/// Whether `thread_id` is a delegated child whose own "complete"/"failure"
/// pushes are noise: it is running, or its report is on the way to the
/// parent, which will notify in turn. A stopped task's tab is the user's
/// again, so it is never muted.
pub fn is_delegated_child<R: Runtime>(app: &AppHandle<R>, thread_id: &str) -> bool {
    if let Some(db) = app.try_state::<DatabaseStore>() {
        if let Ok(Some(task)) = db.delegation_task_for_child(thread_id) {
            return !task.status.terminal()
                || matches!(
                    task.completion_delivery.as_str(),
                    "pending" | "preparing" | "queued" | "submitting"
                );
        }
    }
    app.try_state::<DelegationState>().is_some_and(|state| {
        state.task_matches(thread_id, |task| !matches!(task.phase, Phase::Stopped(_)))
    })
}

// ── Pure rules ───────────────────────────────────────────────────────

pub(crate) fn provider_id(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "claude",
        ProviderKind::Codex => "codex",
        ProviderKind::Cursor => "cursor",
        ProviderKind::Grok => "grok",
        ProviderKind::Hermes => "hermes",
        ProviderKind::Acp => "acp",
        ProviderKind::OpenCode => "opencode",
    }
}

pub(crate) fn provider_label(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "Claude",
        ProviderKind::Codex => "Codex",
        ProviderKind::Cursor => "Cursor",
        ProviderKind::Grok => "Grok",
        ProviderKind::Hermes => "Hermes",
        ProviderKind::Acp => "Custom agent",
        ProviderKind::OpenCode => "OpenCode",
    }
}

pub(super) fn parse_provider(id: &str) -> Option<ProviderKind> {
    CANDIDATES
        .into_iter()
        .chain([ProviderKind::Hermes])
        .find(|kind| provider_id(*kind) == id.trim().to_ascii_lowercase())
}

/// Whether a parent may delegate: only Claude and Codex chats, and only in
/// Full access. `mode` is Claude's LIVE mode (the Plan/Ask pills switch it)
/// or the mode a Codex session started with.
pub(crate) fn parent_is_full(provider: ProviderKind, mode: Option<&str>) -> bool {
    matches!(provider, ProviderKind::Claude | ProviderKind::Codex)
        && resolve_start_permission_mode(provider, mode.map(str::to_string)).as_deref()
            == fallback_permission_mode(provider)
}

/// The picker's name for a permission mode, for the refusal sentence.
fn mode_label(mode: Option<&str>) -> String {
    match mode {
        Some("plan") => "Plan mode".into(),
        // Also where a Claude chat lands after a plan is accepted.
        Some("default") => "Supervised".into(),
        Some("acceptEdits") => "Auto-accept edits".into(),
        Some("read-only") => "Read only".into(),
        Some("workspace-write") => "Workspace write".into(),
        Some(other) => other.into(),
        None => "an unknown mode".into(),
    }
}

/// Agents to offer `caller` (with the setting on), or none when the tool
/// must not be listed. Codex decides once at session start, from the mode
/// it started in.
pub(crate) fn offered_agents(
    caller: &HostCaller,
    caller_is_child: bool,
    is_installed: impl Fn(ProviderKind) -> bool,
) -> Vec<ProviderKind> {
    let _ = caller_is_child;
    if !matches!(caller.provider, ProviderKind::Claude | ProviderKind::Codex) { return Vec::new(); }
    CANDIDATES
        .into_iter()
        .filter(|kind| is_installed(*kind))
        .collect()
}

/// The child's start input, from an explicit allowlist: nothing is copied
/// from the parent's session.
fn child_input(
    child_thread_id: &str,
    cwd: PathBuf,
    _provider: ProviderKind,
    model: Option<String>,
    effort: Option<String>,
    permission_mode: Option<String>,
) -> StartSessionInput {
    StartSessionInput {
        thread_id: ThreadId(child_thread_id.to_string()),
        cwd,
        model,
        resume_cursor: None,
        fresh_session: true,
        permission_mode,
        effort,
        context_window: None,
        fast_mode: false,
        service_tier: None,
        additional_directories: Vec::new(),
        env: None,
        workspace_id: None,
        extra: Value::Null,
        recorded_usage_baseline: None,
        // Filled by `agent_chat_start_session` from the workspace.
        remote: None,
    }
}

/// `text` cut to `max_chars` characters, ending in `marker` when it was cut.
pub(super) fn excerpt(text: &str, max_chars: usize, marker: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}{marker}", head.trim_end())
    } else {
        head
    }
}

/// One line, at most [`REASON_CHARS`] characters.
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    excerpt(&flat, REASON_CHARS, "…")
}

fn clock(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|at| at.format("%-I:%M %p").to_string())
        .unwrap_or_else(|| "later".to_string())
}

fn usage_limit_reason(resets_at_ms: Option<i64>) -> String {
    match resets_at_ms {
        Some(at) => format!("usage limit reached (resets {})", clock(at)),
        None => "usage limit reached".to_string(),
    }
}

/// How a child's finished run maps onto the task. `None` means Codemux
/// never saw the turn end (only the sweep calls it that way), which fails
/// closed: a report Codemux cannot vouch for is not a success.
fn finalize_outcome(
    status: Option<&TurnStatus>,
    last_text: Option<&str>,
    usage_reset_ms: Option<i64>,
) -> Phase {
    let text = last_text.map(str::trim).filter(|text| !text.is_empty());
    match status {
        Some(TurnStatus::Success) => match text {
            Some(text) => Phase::Completed(text.to_string()),
            None => Phase::Failed("It finished without a final message.".into()),
        },
        Some(TurnStatus::Error { subtype, .. })
            if subtype == crate::agent_provider::events::RATE_LIMIT_SUBTYPE =>
        {
            Phase::Failed(one_line(&usage_limit_reason(usage_reset_ms)))
        }
        Some(TurnStatus::Error { message, .. }) => Phase::Failed(one_line(message)),
        Some(TurnStatus::MaxTurns) => Phase::Failed("It hit its max-turns limit.".into()),
        Some(TurnStatus::MaxBudget) => Phase::Failed("It hit its budget limit.".into()),
        None => Phase::Failed(one_line(&match text {
            Some(text) => format!("It went idle without a final result; its last message: {text}"),
            None => "It went idle without a final result.".to_string(),
        })),
    }
}

fn activity_line(task: &Task) -> String {
    if task.stopping {
        return "Stopping; provider confirmation pending".into();
    }
    match &task.phase {
        Phase::Starting => "Starting…".into(),
        Phase::Running => match task.paused_until {
            Some(at) => format!(
                "Paused: {} usage limit · resumes {}",
                provider_label(task.provider),
                clock(at)
            ),
            None if task.awaits_user() => {
                format!("Waiting for your answer in {}", task.placement.place())
            }
            None => format!("Working in {}", task.placement.place()),
        },
        Phase::Completed(_) => "Finished".into(),
        Phase::Failed(reason) => reason.clone(),
        Phase::Stopped(reason) => reason.note().into(),
    }
}

/// The parent's card for one task. Always the full field set: the
/// frontend merges snapshots, and a partial one would read as unknown.
fn snapshot(child_thread_id: &str, task: &Task) -> SubagentSnapshot {
    let (status, result_text) = match &task.phase {
        Phase::Starting => (SubagentStatus::Pending, None),
        Phase::Running => (SubagentStatus::Running, None),
        Phase::Completed(report) => (
            SubagentStatus::Completed,
            Some(excerpt(
                report,
                CARD_RESULT_CHARS,
                &format!("\n\n… Full report in {}", task.placement.place()),
            )),
        ),
        Phase::Failed(reason) => (SubagentStatus::Failed, Some(reason.clone())),
        Phase::Stopped(reason) => (SubagentStatus::Stopped, Some(reason.note().to_string())),
    };
    SubagentSnapshot {
        task_id: Some(task.task_id.clone()),
        subagent_id: format!("{DELEGATED_ROW_PREFIX}{child_thread_id}"),
        parent_item_id: match &task.placement {
            Placement::Shared => task.tool_call_id.clone(),
            Placement::Worktree(worktree) => Some(worktree.suggestion.clone()),
        },
        name: Some(provider_label(task.provider).to_string()),
        agent_type: Some(provider_id(task.provider).to_string()),
        description: Some(task.title.clone()),
        task_kind: Some(SubagentTaskKind::Agent),
        model: task.model.clone(),
        effort: task.effort.clone(),
        status,
        activity: Some(activity_line(task)),
        result_text,
        duration_ms: task
            .finished_at_ms
            .map(|end| end.saturating_sub(task.started_at_ms).max(0) as u64),
        provider_ref: Some(child_thread_id.to_string()),
        ..SubagentSnapshot::default()
    }
}

fn delegate_tool(agents: &[ProviderKind]) -> McpTool {
    McpTool {
        name: TOOL_NAME.to_string(),
        prefixed_name: prefix_tool_name("codemux", TOOL_NAME),
        description: Some(TOOL_DESCRIPTION.to_string()),
        input_schema: json!({
            "type": "object",
            "properties": {
                "provider": {
                    "type": "string",
                    "enum": agents.iter().map(|kind| provider_id(*kind)).collect::<Vec<_>>(),
                    "description": "Agent that runs the task."
                },
                "title": {
                    "type": "string",
                    "description": "3-8 word label shown to the user (max 60 characters)."
                },
                "task": {
                    "type": "string",
                    "description": "Complete, self-contained instructions (at least 20 characters). The agent sees nothing else from this conversation."
                },
                "model": {
                    "type": "string",
                    "description": "Exact model id, only if the user named one. Omit for that agent's default."
                },
                "client_request_id": { "type": "string", "minLength": 1 },
                "mode": { "type": "string", "enum": ["async", "wait"], "default": "async" },
                "wait_ms": { "type": "integer", "minimum": 0, "maximum": 20000 },
                "role": { "type": "string" },
                "permission_mode": { "type": "string", "description":"Optional exact native mode, bounded by the calling chat. Omit to inherit safely." },
                "effort": {
                    "type": "string",
                    "description": "Effort/reasoning level, only if the user named one. Omit for that agent's default."
                }
            },
            "required": ["provider", "title", "task"]
        }),
        server_id: "codemux-host".to_string(),
    }
}

// ── The host tool ────────────────────────────────────────────────────

pub(crate) fn setting_on(db: &DatabaseStore) -> bool {
    db.get_setting(DELEGATION_SETTING_KEY).as_deref() != Some("false")
}

/// Serves `delegate_task` and `suggest_tasks` to Claude and Codex chats.
pub struct DelegationHost<R: Runtime> {
    app: AppHandle<R>,
    is_installed: fn(ProviderKind) -> bool,
}

impl<R: Runtime> DelegationHost<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self::with_install_check(app, |kind| {
            which::which(super::provider_updates::binary_name(kind)).is_ok()
        })
    }

    /// Like [`new`](Self::new), with the "is this agent's CLI on PATH"
    /// probe replaced — tests run on machines without the CLIs.
    pub fn with_install_check(app: AppHandle<R>, is_installed: fn(ProviderKind) -> bool) -> Self {
        Self { app, is_installed }
    }
}

impl<R: Runtime> DelegationHost<R> {
    fn delegate_tools(&self, caller: &HostCaller) -> Vec<McpTool> {
        let (Some(db), Some(state)) = (
            self.app.try_state::<DatabaseStore>(),
            self.app.try_state::<DelegationState>(),
        ) else {
            return Vec::new();
        };
        if !setting_on(&db) {
            return Vec::new();
        }
        let is_child = state.is_running_child(&caller.thread_id);
        let agents = offered_agents(caller, is_child, self.is_installed);
        if agents.is_empty() {
            Vec::new()
        } else {
            vec![
                delegate_tool(&agents),
                task_tool("task_status"),
                task_tool("task_cancel"),
            ]
        }
    }
}

/// Serves both host tools: `delegate_task` (this file) and `suggest_tasks`
/// (`follow_ups`). The registry installs a single host.
impl<R: Runtime> HostTools for DelegationHost<R> {
    fn tools_for(&self, caller: &HostCaller) -> Vec<McpTool> {
        let mut tools = self.delegate_tools(caller);
        tools.extend(super::follow_ups::tool_for(&self.app, caller));
        tools
    }

    fn call_async<'a>(
        &'a self,
        caller: &'a HostCaller,
        name: &'a str,
        args: &'a Value,
        call_id: Option<&'a str>,
        run_id: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Value>> + Send + 'a>> {
        Box::pin(async move {
            if name != prefix_tool_name("codemux", TOOL_NAME) {
                return self.call(caller, name, args);
            }
            let requested_deadline = now_ms() + args["wait_ms"].as_u64().unwrap_or(20_000).min(20_000) as i64;
            let validation = async {
                let input: DelegateArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
                let provider = parse_provider(&input.provider).ok_or("unsupported_provider")?;
                if let Some(mode) = input.permission_mode.as_deref() {
                    let caps = crate::agent_control::service::capabilities(&self.app, provider).await.map_err(|e| e.to_string())?;
                    crate::agent_control::service::validate_permission_mode(provider, Some(mode), &caps).map_err(|e| e.to_string())?;
                }
                Ok(())
            }.await;
            let result = validation.and_then(|()| delegate_with_call(&self.app, caller, args, self.is_installed, call_id, run_id, true));
            let result = match result {
                Ok(mut receipt) => {
                    if args["mode"] == "wait" {
                        let already_terminal = receipt["result"].is_object();
                        let id = receipt["task_id"].as_str().unwrap_or_default().to_string();
                        let _lease = WaitLease::acquire(self.app.clone(), id.clone());
                        let until = match self.app.state::<DatabaseStore>().arm_delegation_wait(&id, requested_deadline) {
                            Ok(until) => until,
                            Err(error) => return Some(mcp_result(Err(error))),
                        };
                        receipt["wait_until"] = json!(until);
                        let remaining = until.saturating_sub(now_ms()).max(0) as u64;
                        let deadline = tokio::time::Instant::now() + Duration::from_millis(remaining);
                        loop {
                            if let Some(control)=&caller.control {
                                if let Err(error)=crate::agent_control::service::authorize(&self.app,control){return Some(mcp_result(Err(error.to_string())));}
                            }
                            // Register before reading to avoid losing a completion.
                            let notified = task_changes().notified();
                            tokio::pin!(notified);
                            notified.as_mut().enable();
                            if let Ok(Some(task)) =
                                self.app.state::<DatabaseStore>().delegation_task(&id)
                            {
                                if task.wait_until.is_none() && !already_terminal {
                                    receipt["result"] = Value::Null;
                                    receipt["wait_released"] = json!(true);
                                    break;
                                }
                                if task.result.is_some() {
                                    let acknowledge=||self.app.state::<DatabaseStore>().acknowledge_delegation_result(&caller.thread_id,&id);
                                    let acknowledged=if let Some(control)=&caller.control {control.admit(acknowledge)}else{acknowledge()};
                                    match acknowledged {Ok(task)=>receipt=task_receipt(&task),Err(error)=>return Some(mcp_result(Err(error)))}
                                    break;
                                }
                            }
                            let revoked=async {if let Some(control)=&caller.control {control.revoked().await;}else{std::future::pending::<()>().await;}};
                            tokio::select! {
                                biased;
                                _=revoked=>return Some(mcp_result(Err("stale_caller: wait released; accepted task and result delivery remain available".into()))),
                                _=tokio::time::sleep_until(deadline)=>{receipt["wait_timed_out"]=json!(true);break;},
                                _=notified=>{},
                            }
                        }
                    }
                    Ok(receipt)
                }
                Err(error) => Err(error),
            };
            Some(mcp_result(result))
        })
    }

    fn call(&self, caller: &HostCaller, prefixed_name: &str, arguments: &Value) -> Option<Value> {
        if super::follow_ups::is_tool(prefixed_name) {
            return Some(super::follow_ups::call(&self.app, caller, arguments));
        }
        for name in ["task_status", "task_cancel"] {
            if prefixed_name == prefix_tool_name("codemux", name) {
                let result = arguments["task_id"]
                    .as_str()
                    .ok_or_else(|| "task_id required".to_string())
                    .and_then(|id| {
                        let work = || local_task_call(&self.app, &caller.thread_id, id, name == "task_cancel");
                        let admitted = if let Some(control) = &caller.control {
                            control.admit(work)
                        } else {
                            work()
                        }?;
                        let call = admitted.ok_or_else(|| "Use the remote task adapter for SSH task IDs".to_string())?;
                        call.finish(&self.app)
                    });
                if result
                    .as_ref()
                    .is_err_and(|e| e == "Use the remote task adapter for SSH task IDs")
                {
                    return None;
                }
                return Some(mcp_result(result));
            }
        }
        if prefixed_name != prefix_tool_name("codemux", TOOL_NAME) {
            return None;
        }
        let (text, is_error) = match delegate(&self.app, caller, arguments, self.is_installed) {
            Ok(started) => (started.to_string(), false),
            Err(refusal) => (refusal, true),
        };
        Some(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegateArgs {
    provider: String,
    #[serde(default)]
    title: String,
    task: String,
    model: Option<String>,
    effort: Option<String>,
    client_request_id: Option<String>,
    mode: Option<String>,
    wait_ms: Option<u64>,
    role: Option<String>,
    permission_mode: Option<String>,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The task a `delegate_task` call asks for; `Err` is the refusal.
fn requested_task(
    parent: &str,
    arguments: &Value,
    is_installed: fn(ProviderKind) -> bool,
) -> Result<Task, String> {
    let args: DelegateArgs = serde_json::from_value(arguments.clone())
        .map_err(|_| "Pass provider, title and task as strings.".to_string())?;
    let provider = parse_provider(&args.provider)
        .filter(|kind| CANDIDATES.contains(kind))
        .ok_or_else(|| {
            format!(
                "Unknown agent \"{}\"; use one of: {}.",
                args.provider.trim(),
                CANDIDATES.map(provider_id).join(", ")
            )
        })?;
    if !is_installed(provider) {
        return Err(format!(
            "{} is not installed on this machine.",
            provider_label(provider)
        ));
    }
    let brief = args.task.trim();
    if brief.chars().count() < MIN_TASK_CHARS {
        return Err("Write a complete, self-contained task (at least 20 characters).".into());
    }
    let flat_title = args.title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = match flat_title.as_str() {
        "" => brief.lines().next().unwrap_or_default(),
        title => title,
    };
    let title = excerpt(title.trim(), MAX_TITLE_CHARS, "");
    let mut task = Task::new(parent, provider, &title, brief, now_ms());
    task.model = non_empty(args.model);
    task.effort = non_empty(args.effort);
    task.role = non_empty(args.role);
    Ok(task)
}

/// Validate, record and launch one delegated task. Fast and synchronous:
/// memory, database rows and app state only — the provider start runs in
/// the spawned starter. `Err` is the one-sentence refusal the model sees.
fn delegate<R: Runtime>(
    app: &AppHandle<R>,
    caller: &HostCaller,
    arguments: &Value,
    is_installed: fn(ProviderKind) -> bool,
) -> Result<Value, String> {
    delegate_with_call(app, caller, arguments, is_installed, None, None, false)
}
fn delegate_with_call<R: Runtime>(
    app: &AppHandle<R>,
    caller: &HostCaller,
    arguments: &Value,
    is_installed: fn(ProviderKind) -> bool,
    tool_call_id: Option<&str>,
    run_id: Option<&str>,
    validated_mode: bool,
) -> Result<Value, String> {
    let unavailable = || "Delegation is unavailable in this Codemux build.".to_string();
    let db = app.try_state::<DatabaseStore>().ok_or_else(unavailable)?;
    let state = app.try_state::<DelegationState>().ok_or_else(unavailable)?;
    let app_state = app.try_state::<AppStateStore>().ok_or_else(unavailable)?;
    let parent = caller.thread_id.as_str();
    let admission_generation = caller.control.as_ref().and_then(|c| c.admission_generation())
        .map(Ok).unwrap_or_else(|| db.delivery_generation(parent).map(|g| g.0))?;
    let wait_until = (arguments["mode"] == "wait").then(|| now_ms() + arguments["wait_ms"].as_u64().unwrap_or(20_000).min(20_000) as i64);

    if !setting_on(&db) {
        return Err("Cross-provider delegation is off (Settings → Agents → Models & behavior).".into());
    }
    let ceiling = if let Some(control) = &caller.control {
        crate::agent_control::service::authorize(app, control).map_err(|e|e.to_string())?;
        control.ceiling().ok_or("unsupported_permission_ceiling")?
    } else {
        crate::agent_control::permissions::Ceiling::for_mode(caller.provider, caller.permission_mode.as_deref()).ok_or("unsupported_permission_ceiling")?
    };
    let args: DelegateArgs =
        serde_json::from_value(arguments.clone()).map_err(|e| e.to_string())?;
    if args
        .mode
        .as_deref()
        .is_some_and(|m| !matches!(m, "async" | "wait"))
        || args.wait_ms.is_some_and(|m| m > 20_000)
    {
        return Err("Use mode async|wait with wait_ms <= 20000".into());
    }
    if args
        .client_request_id
        .as_ref()
        .is_some_and(|k| k.trim().is_empty() || k.len() > 256)
    {
        return Err("client_request_id must contain 1-256 bytes".into());
    }
    let mut task = requested_task(parent, arguments, |_| true)?;
    task.tool_call_id = tool_call_id.map(str::to_string);
    let payload = json!({"provider":provider_id(task.provider),"title":task.title,"task":task.brief,"model":task.model,"effort":task.effort,"role":task.role,"mode":args.mode.as_deref().unwrap_or("async"),"wait_ms":args.wait_ms.unwrap_or(20000),"permission_mode":args.permission_mode});
    if let Some(key) = &args.client_request_id {
        if let Some(prior) = db.delegation_retry(parent, key, &payload)? {
            return Ok(task_receipt(&prior));
        }
    }
    let provider = task.provider;
    if caller.control.is_some() && !matches!(provider, ProviderKind::Claude | ProviderKind::Codex) {return Err("unsupported_provider: native child injection for this provider comes in a later layer".into());}
    if !is_installed(provider) {
        return Err(format!(
            "{} is not installed on this machine.",
            provider_label(provider)
        ));
    }

    // The child gets its own workspace, nested under the one the parent's
    // pane is in right now, in the parent's folder.
    let workspace = app_state
        .agent_chat_pane_id_for_thread(parent)
        .and_then(|pane| app_state.workspace_id_for_pane(&pane))
        .or_else(|| caller.workspace_id.clone())
        .and_then(|id| app_state.find_workspace(&id))
        .ok_or_else(|| "This chat has no open workspace.".to_string())?;
    let workspace_id = workspace.workspace_id.0.clone();
    let cwd = db
        .get_agent_chat_session(parent)
        .and_then(|record| record.cwd)
        .unwrap_or_else(|| workspace.cwd.clone());

    let child = uuid::Uuid::new_v4().to_string();
    // Named before admission so the durable record points at it.
    let child_workspace = AppStateStore::reserve_workspace_id();
    // Serialize capacity reservation and durable admission for concurrent calls.
    let (record, siblings) = state.admit(caller.control.as_ref(), |inner| {
        if let Some(key) = &args.client_request_id {
            if let Some(prior) = db.delegation_retry(parent, key, &payload)? {
                return Ok((prior, None));
            }
        }
        let mut record = record_of(&child, &task);
        record.workspace_id = child_workspace.0.clone();
        record.cwd = cwd.clone();
        record.parent_run_id = run_id.map(str::to_string).or_else(|| {
            app.try_state::<crate::agent_control::NativeControlState>()
                .and_then(|state| state.thread_runtime(parent))
                .and_then(|runtime| runtime.turn_id)
        });
        let root = db.orchestration_context(parent)?;
        record.budget_root_id = Some(root.root_id);
        if let Some(ancestor) = db.delegation_task_for_child(parent)? {
            record.lineage = ancestor.lineage.clone(); record.lineage.push(ancestor.task_id.clone());
            record.root_task_id = ancestor.root_task_id;
        }
        record.parent_title = db.get_agent_chat_title(parent);
        let mode = args.permission_mode.clone().map(Ok).unwrap_or_else(|| ceiling.inherited(provider).map_err(|e|e.to_string()))?;
        let target = crate::agent_control::permissions::Ceiling::for_mode(provider,Some(&mode)).ok_or("unsupported_permission_ceiling")?;
        if !ceiling.permits(target) { return Err("permission_ceiling: child mode exceeds this chat".into()); }
        if args.permission_mode.is_some() && !validated_mode {
            return Err("unsupported_permission_mode: explicit delegate modes require async catalog validation".into());
        }
        record.plan_mode = provider == ProviderKind::Codex && (mode == "plan" || (args.permission_mode.is_none() && ceiling.plan));
        record.permission_mode = Some(if provider == ProviderKind::Codex && mode == "plan" { "read-only".into() } else { mode });
        record.caller = caller.control.clone().map(|mut caller|{caller.admitted=Some(crate::agent_control::AdmittedAuthority::Task {task_id:record.task_id.clone()});caller});
        record.admission_generation = Some(admission_generation);
        record.wait_until = wait_until;
        let (record, new) = db.admit_delegation_task(&record, args.client_request_id.as_deref(), &payload)?;
        if !new {
            return Ok((record, None));
        }
        let parent_state = inner.parents.entry(parent.into()).or_default();
        parent_state.started_this_round += 1;
        let siblings = parent_state.children.clone();
        parent_state.children.push(child.clone());
        task.round = parent_state.round;
        task.refresh_activity();
        inner.tasks.insert(child.clone(), task.clone());
        Ok((record, Some(siblings)))
    })?;
    if siblings.is_none() { return Ok(task_receipt(&record)); }
    let tab_title = format!("{} · {}", provider_label(provider), task.title);
    let pane = app_state
        .create_chat_child_workspace(&child_workspace, &workspace_id, &cwd, &task.title)
        .and_then(|_| {
            app_state.create_background_chat_tab(
                &child_workspace.0,
                provider,
                Some(cwd.clone()),
                &child,
                parent,
                &[],
                &tab_title,
            )
        });
    let pane_id = match pane {
        Ok(pane) => pane.0,
        Err(error) => {
            if app_state.find_workspace(&child_workspace.0).is_some_and(|w| w.surfaces.is_empty()) {
                let _ = app_state.close_workspace(&child_workspace.0);
                crate::state::emit_app_state(app);
            }
            state.finish(
                app,
                &child,
                Phase::Failed(format!("Could not open task workspace: {error}")),
            );
            return Ok(task_receipt(
                &db.delegation_task(&record.task_id)?
                    .ok_or("Task disappeared")?,
            ));
        }
    };
    crate::state::emit_app_state(app);
    state.emit_card(app, &child);
    tauri::async_runtime::spawn(run_starter(app.clone(), child, pane_id, cwd.into()));
    Ok(task_receipt(&record))
}

pub(crate) fn resume_followup<R:Runtime>(app:&AppHandle<R>,delivery:&str){
    let (Some(db),Some(state))=(app.try_state::<DatabaseStore>(),app.try_state::<DelegationState>()) else{return;};
    let Ok(Some(record))=db.delegation_task(&format!("local:followup:{delivery}")) else{return;};
    let (Some(child),Some(provider))=(record.child_thread_id.clone(),parse_provider(&record.provider)) else{return;};
    let mut inner=state.lock();
    if inner.tasks.get(&child).is_some_and(|t|t.task_id==record.task_id){return;}
    let mut task=Task::new(&record.parent_thread_id,provider,&record.title,&record.brief,record.created_at).with_identity(record.task_id);
    task.placement=durable::placement_for_child(&db,&child);
    task.phase=Phase::Running;task.model=record.model;task.effort=record.effort;task.refresh_activity();inner.tasks.insert(child.clone(),task);drop(inner);
    state.emit_card(app,&child);
}

impl DelegationState {
    /// Track a started suggested task. The user started it, so none of the
    /// delegation caps apply, and it joins the parent's current round so its
    /// report shares the round's results message.
    pub(crate) fn reserve_worktree(&self, child: &str, mut task: Task) {
        let mut inner = self.lock();
        let parent = inner.parents.entry(task.parent.clone()).or_default();
        task.round = parent.round;
        task.refresh_activity();
        inner.tasks.insert(child.to_string(), task);
    }
}

/// Turn a command error (often a serialized provider error) into words.
pub(super) fn readable_error(raw: &str) -> String {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
        return raw.to_string();
    };
    let field = |key: &str| map.get(key).and_then(Value::as_str);
    let detail = field("hint")
        .or_else(|| field("message"))
        .or_else(|| field("source"));
    match (field("kind"), detail) {
        (Some("not_authenticated"), Some(detail)) => format!("not signed in: {detail}"),
        (Some("not_installed"), Some(detail)) => format!("not installed: {detail}"),
        (_, Some(detail)) => detail.to_string(),
        (Some(kind), None) => kind.replace('_', " "),
        (None, None) => raw.to_string(),
    }
}

/// Start the child's session and send it the task. Holds the child's
/// activity lock throughout, so a Stop that lands meanwhile interrupts the
/// child only after the task text is in.
pub(crate) async fn run_starter<R: Runtime>(
    app: AppHandle<R>,
    child: String,
    pane_id: String,
    cwd: PathBuf,
) {
    let lock = usage_resume::activity_lock(&child);
    let _guard = lock.lock_owned().await;
    // Managed: `delegate`, the only spawner, required it.
    let state = app.state::<DelegationState>();
    let Some(task) = state
        .lock()
        .tasks
        .get(&child)
        .filter(|task| task.phase == Phase::Starting && !task.stopping)
        .cloned()
    else {
        return;
    };
    if app
        .state::<DatabaseStore>()
        .delivery_generation(&child)
        .is_ok_and(|(_, stopped)| stopped)
    {
        return;
    }
    let provider = task.provider;
    let record = match app.state::<DatabaseStore>().delegation_task_for_child(&child) { Ok(Some(record)) => record, _ => return };
    let input = child_input(&child, cwd, provider, task.model, task.effort, record.permission_mode.clone());
    let control = record.caller.clone().map(|caller| crate::agent_control::NativeControlGuard { caller, workspace_id:record.workspace_id.clone(),thread_id:child.clone(),provider,admission:None }.retain_admission(&app));
    let start = super::agent_chat::agent_chat_start_session_guarded(app.clone(), pane_id, provider, input, None, control.clone());
    let failure = match tokio::time::timeout(START_TIMEOUT, start).await {
        Ok(Ok(_)) if !state.is_starting(&child) => return,
        Ok(Ok(_)) => {
            let footer = if task.placement.is_shared() {
                CHILD_FOOTER
            } else {
                WORKTREE_CHILD_FOOTER
            };
            let mut turn = queued_turn(
                ThreadId(child.clone()),
                format!(
                    "{}{}{footer}",
                    task.role
                        .as_ref()
                        .map(|r| format!("Role: {r}\n\n"))
                        .unwrap_or_default(),
                    task.brief
                ),
            );
            if provider == ProviderKind::Codex && record.plan_mode { turn.permission_mode_override = Some("plan".into()); }
            turn.display_text = Some(task.brief.clone());
            turn.client_nonce = Some(format!("delegated-task:{child}"));
            let mut context =
                crate::agent_control::delivery::SendContext::new(TurnOrigin::Delegation);
            context.control = control;
            context.provenance.created_by = crate::agent_provider::message::CreatedBy::Agent;
            context.provenance.creation_source =
                crate::agent_provider::message::CreationSource::Provider;
            context.provenance.sender_thread_id = Some(task.parent.clone());
            context.provenance.sender_title = app
                .state::<DatabaseStore>()
                .get_agent_chat_title(&task.parent);
            send_turn_with_context(app.clone(), provider, turn, context)
                .await
                .err()
                .map(|error| {
                    format!(
                        "Codemux could not send it the task: {}",
                        readable_error(&error)
                    )
                })
        }
        Ok(Err(error)) => Some(format!("It could not start: {}", readable_error(&error))),
        Err(_) => Some("It could not start: it did not start within 2 minutes".to_string()),
    };
    if let Some(reason) = failure {
        // The task fails and the child's tab shows why.
        let reason = one_line(&reason);
        let parent = state.finish(&app, &child, Phase::Failed(reason.clone()));
        forward_event(
            &app,
            ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ThreadId(child.clone()),
                status: SessionStatus::Error { message: reason },
            },
        );
        if let Some(parent) = parent {
            spawn_deliver(&app, &parent, Duration::ZERO);
        }
        return;
    }
    let turn_already_over = state.update(&app, &child, |task| {
        if task.phase != Phase::Starting {
            return false;
        }
        task.phase = Phase::Running;
        task.last_status.is_some()
    });
    if turn_already_over == Some(true) {
        spawn_settle(&app, &child);
    }
}

// ── Hooks ────────────────────────────────────────────────────────────

/// Called by `forward_event` after persistence for every event. Tracks a
/// child's final text, turn status, open requests and usage-limit pause,
/// re-emits its card when the activity line changes, and schedules the
/// idle check. A parent's `TurnCompleted` schedules a delivery attempt.
pub(crate) fn observe<R: Runtime>(
    app: &AppHandle<R>,
    event: &ProviderRuntimeEvent,
    persisted_id: Option<i64>,
) {
    // The token stream says nothing delegation cares about.
    if super::queued_turns::exiting() || matches!(event, ProviderRuntimeEvent::ContentDelta { .. })
    {
        return;
    }
    if let ProviderRuntimeEvent::ItemCompleted {
        thread_id,
        turn_id,
        item:
            CompletedItem::ToolResult {
                tool_use_id,
                content,
                ..
            },
        ..
    } = event
    {
        if let Some(task_id) = receipt_task_id(content) {
            if let Some(db) = app.try_state::<DatabaseStore>() {
                if let Ok(Some(record)) = db.delegation_task(&task_id) {
                    if record.parent_thread_id == thread_id.0 {
                        let _ = db.update_delegation_task(&task_id, |t| {
                            t.tool_call_id = Some(tool_use_id.clone());
                            t.parent_run_id = Some(turn_id.0.clone());
                        });
                        if let (Some(state), Some(child)) =
                            (app.try_state::<DelegationState>(), record.child_thread_id)
                        {
                            state.update(app, &child, |t| {
                                t.tool_call_id = Some(tool_use_id.clone())
                            });
                            state.emit_card(app, &child);
                        }
                    }
                }
            }
        }
    }
    let Some(state) = app.try_state::<DelegationState>() else {
        return;
    };
    let (thread, is_child, is_parent) = {
        let inner = state.lock();
        if inner.tasks.is_empty() {
            return;
        }
        let Some(ThreadId(thread)) = thread_id_for_event(event) else {
            return;
        };
        let is_child = inner
            .tasks
            .get(&thread)
            .is_some_and(|task| !task.phase.is_terminal());
        let is_parent = inner.tasks.values().any(|task| task.parent == thread);
        (thread, is_child, is_parent)
    };
    {
        if let Ok(Some(record)) = app
            .state::<DatabaseStore>()
            .delegation_task_for_child(&thread)
        {
            let _ = app
                .state::<DatabaseStore>()
                .update_delegation_task(&record.task_id, |task| {
                    if let ProviderRuntimeEvent::ItemCompleted { turn_id, .. }
                    | ProviderRuntimeEvent::TurnCompleted { turn_id, .. }
                    | ProviderRuntimeEvent::RequestOpened { turn_id, .. }
                    | ProviderRuntimeEvent::SessionStateChanged {
                        status: SessionStatus::Running { active_turn: turn_id },
                        ..
                    } = event
                    {
                        if task.original_child_run_id.is_none() {
                            task.original_child_run_id = Some(turn_id.0.clone());
                        }
                        if !task.child_run_ids.contains(&turn_id.0) {
                            task.child_run_ids.push(turn_id.0.clone());
                        }
                        if matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }) {
                            task.latest_terminal_run_id = Some(turn_id.0.clone());
                            task.has_pending_child_runs = false;
                        } else {
                            task.has_pending_child_runs = true;
                        }
                    }
                    if matches!(
                        event,
                        ProviderRuntimeEvent::SessionStateChanged {
                            status: SessionStatus::Running { .. },
                            ..
                        }
                    ) {
                        task.has_pending_child_runs = true;
                    }
                    match event {
                        ProviderRuntimeEvent::RequestOpened {
                            request_id,
                            request_kind,
                            ..
                        } => {
                            task.pending_request = Some(PendingRequest {
                                id: request_id.0.clone(),
                                kind: if request_kind.contains("input") {
                                    "user_input"
                                } else {
                                    "approval"
                                }
                                .into(),
                                created_at: now_ms(),
                            })
                        }
                        ProviderRuntimeEvent::QuestionsAsked { question, .. } => {
                            task.pending_request = Some(PendingRequest {
                                id: question.id.clone(),
                                kind: "user_input".into(),
                                created_at: now_ms(),
                            })
                        }
                        ProviderRuntimeEvent::RequestResolved { request_id, .. }
                        | ProviderRuntimeEvent::RequestResponseFailed { request_id, .. }
                            if task
                                .pending_request
                                .as_ref()
                                .is_some_and(|r| r.id == request_id.0) =>
                        {
                            task.pending_request = None
                        }
                        ProviderRuntimeEvent::QuestionResolved { .. } => {
                            task.pending_request = None
                        }
                        _ => {}
                    }
                });
        }
    }
    if is_parent && matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }) {
        if let Some(db) = app.try_state::<DatabaseStore>() {
            if let Ok(tasks) = db.delegation_tasks(Some(&thread)) {
                for task in tasks.into_iter().filter(|t| t.wait_until.is_some()) {
                    let _ = db.release_delegation_wait(&task.task_id);
                }
            }
        }
        task_changes().notify_waiters();
        spawn_deliver(app, &thread, SETTLE_DELAY);
    }
    if !is_child {
        return;
    }
    let open_question = matches!(event, ProviderRuntimeEvent::QuestionResolved { .. })
        && app
            .try_state::<DatabaseStore>()
            .is_some_and(|db| db.has_open_async_question(&thread));
    let Some((settle, failure)) = state.update(app, &thread, |task| {
        if task.stopping {
            return (false, None);
        }
        task.observe(event, persisted_id, open_question)
    }) else {
        return;
    };
    if let Some(reason) = failure {
        if let Some(parent) = state.finish(app, &thread, Phase::Failed(reason)) {
            spawn_deliver(app, &parent, Duration::ZERO);
        }
    } else if settle {
        spawn_settle(app, &thread);
    }
}

/// A user message on `thread_id` starts a new round for it: a fresh
/// 6-task budget and another chance for reports that failed to post.
pub(crate) fn on_user_turn<R: Runtime>(app: &AppHandle<R>, thread_id: &str) {
    let Some(state) = app.try_state::<DelegationState>() else {
        return;
    };
    let mut inner = state.lock();
    if let Some(db) = app.try_state::<DatabaseStore>() { let _ = db.orchestration_human_root(thread_id); }
    if let Some(parent) = inner.parents.get_mut(thread_id) {
        parent.round += 1;
        parent.started_this_round = 0;
    }
}

/// The user answered a request in a child's tab. Codex never reports that
/// as `RequestResolved`, so without this its card would keep asking for an
/// answer until the turn ends.
pub(crate) fn on_request_answered<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    request_id: &str,
) {
    if let Some(state) = app.try_state::<DelegationState>() {
        state.update(app, thread_id, |task| task.open_requests.remove(request_id));
    }
}

/// Persist recursive Stop disposition before waiting on providers.
/// Session restart and process exit preserve task ownership and delivery.
pub fn on_thread_stopped<R: Runtime>(app: &AppHandle<R>, thread_id: &str, kind: StopKind) {
    if kind == StopKind::Restart || super::queued_turns::exiting() {
        return;
    }
    if kind == StopKind::Close {
        on_thread_closed(app, thread_id);
        return;
    }
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    let tasks = match db.stop_delegation_tree(thread_id) {
        Ok(tasks) => tasks,
        Err(error) => {
            log::error!("Task Stop persistence failed: {error}");
            return;
        }
    };
    if let Ok(watch) = db.get_pr_watch(thread_id) {
        crate::pr_watch::emit_changed(app, thread_id, watch);
    }
    if let Some(state) = app.try_state::<DelegationState>() {
        state.update(app, thread_id, |task| {
            task.stopping = true;
            if task.stop_reason.is_none() {
                task.stop_reason = Some(StopReason::User);
            }
        });
    }
    for task in tasks {
        if task.backend == crate::database::delegation_tasks::TaskBackend::Ssh {
            if let (Ok(store), Some(id)) = (
                crate::delegation::app::journal(app),
                task.remote_task_id.as_deref(),
            ) {
                if store.cancel(id).is_ok() {
                    crate::delegation::app::start(app, id);
                }
            }
            continue;
        }
        if let Some(child) = &task.child_thread_id {
            if let Err(error) = crate::delegation::app::user_activity(app, child, true) {
                log::warn!("Could not revoke stopped descendant authority: {error}");
            }
            super::agent_chat::native_thread_gate(app, child).cancel();
        }
        if task.status.terminal() && !task.has_pending_child_runs {
            continue;
        }
        if let Some(state) = app.try_state::<DelegationState>() {
            if let Some(child) = &task.child_thread_id {
                state.update(app, child, |task| {
                    task.stopping = true;
                    task.stop_reason = Some(StopReason::Parent);
                });
            }
        }
        let Some(child) = task.child_thread_id.clone() else {
            continue;
        };
        let Some(provider) = current_child_provider(&db, &task) else {
            continue;
        };
        let owned = app.clone();
        tauri::async_runtime::spawn(async move {
            match super::agent_chat::interrupt_stopped_thread(owned.clone(), provider, ThreadId(child.clone()))
                .await
            {
                Ok(_) => {
                    if let Some(state) = owned.try_state::<DelegationState>() {
                        state.finish(&owned, &child, Phase::Stopped(StopReason::Parent));
                    }
                }
                Err(error) => {
                    let _ = owned
                        .state::<DatabaseStore>()
                        .update_delegation_task(&task.task_id, |t| {
                            t.progress = Some(format!("Stop unconfirmed: {error}"))
                        });
                }
            }
        });
    }
}

/// A chat left its pane: tab close, New Chat, or its workspace closed.
/// Returns the stops of the delegated chats that closed with it.
pub(crate) fn on_thread_closed<R: Runtime>(app: &AppHandle<R>, thread: &str) -> crate::checkout_claims::Stops {
    if super::queued_turns::exiting() {
        return Default::default();
    }
    close_tasks(app, thread)
}

/// Closing a parent retains independent worktree work. Only its direct
/// shared children follow the old tab lifecycle: they stop, and their own
/// workspaces close (closing theirs in turn). Ordinary Stop keeps them.
fn close_tasks<R: Runtime>(app: &AppHandle<R>, thread: &str) -> crate::checkout_claims::Stops {
    // SSH tasks follow the parent's existing New Chat / pane rebind lifecycle.
    if let Err(error) = crate::delegation::app::user_activity(app, thread, true) {
        log::warn!("Could not cancel closed chat's SSH tasks: {error}");
    }
    if let Some(state) = app.try_state::<DelegationState>() {
        stop_closed_tasks(app, &state, thread);
    }
    // From the durable records, so children from before a restart close too.
    let mut stops = crate::checkout_claims::Stops::default();
    if let (Some(app_state), Some(db)) = (app.try_state::<AppStateStore>(), app.try_state::<DatabaseStore>()) {
        let children = super::workspace::delegated_child_workspaces(&app_state.snapshot(), &db, &[thread.to_string()]);
        for workspace in children {
            stops.extend(super::workspace::close_delegated_child_workspace(app, &workspace));
        }
    }
    stops
}

fn stop_closed_tasks<R: Runtime>(app: &AppHandle<R>, state: &DelegationState, thread: &str) {
    let reason = if state.task_matches(thread, |task| !task.placement.is_shared()) {
        StopReason::WorkspaceClosed
    } else { StopReason::TabClosed };
    if let Some(parent) = state.finish(app, thread, Phase::Stopped(reason)) {
        spawn_deliver(app, &parent, Duration::ZERO);
    }
    let children: Vec<_> = state.lock().tasks.iter()
        .filter(|(_, task)| task.parent == thread && task.placement.is_shared())
        .map(|(child, task)| (child.clone(), task.clone())).collect();
    for (child, task) in children {
        crate::delegation::authority::NativeAuthority::user_activity_thread(&child, true);
        super::agent_chat::native_thread_gate(app, &child).cancel();
        if let Some(db) = app.try_state::<DatabaseStore>() {
            let _ = db.dispose_delegation_task_for_close(thread, &task.task_id);
        }
        if task.phase.is_terminal() { continue; }
        state.finish(app, &child, Phase::Stopped(StopReason::Parent));
        tauri::async_runtime::spawn(super::agent_chat::interrupt_stopped_thread(app.clone(), task.provider, ThreadId(child)));
    }
}

// ── Settling, delivery, sweep ────────────────────────────────────────

fn spawn_settle<R: Runtime>(app: &AppHandle<R>, child: &str) {
    let app = app.clone();
    let child = child.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SETTLE_DELAY).await;
        settle(&app, &child).await;
    });
}

fn spawn_deliver<R: Runtime>(app: &AppHandle<R>, parent: &str, delay: Duration) {
    let app = app.clone();
    let parent = parent.to_string();
    tauri::async_runtime::spawn(async move {
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        deliver(&app, &parent).await;
    });
}

fn resume_armed(db: &DatabaseStore, thread_id: &str) -> bool {
    db.get_agent_chat_usage_resume(thread_id)
        .is_some_and(|row| row.resume_at_ms.is_some())
}

/// Whether the child still has something going: a turn, its own subagents,
/// a question or approval waiting on the user, an armed usage resume, or a
/// send still on its way to the provider.
async fn child_busy<R: Runtime>(
    app: &AppHandle<R>,
    state: &DelegationState,
    provider: ProviderKind,
    child: &str,
) -> bool {
    // Senders hold the activity lock until the provider has the turn, and
    // "Resume now" disarms its resume before it dispatches.
    let dispatching = usage_resume::activity_lock(child).try_lock().is_err();
    // An armed usage resume still counts after a failed (rate-limited) turn;
    // an open question does not.
    let turn_failed = state.task_matches(child, Task::turn_failed);
    dispatching
        || app.try_state::<DatabaseStore>().is_some_and(|db| db.delegation_tasks(Some(child)).is_ok_and(|tasks|tasks.iter().any(|task|!task.status.terminal() || matches!(task.completion_delivery.as_str(),"pending"|"preparing"|"queued"|"submitting"))))
        || state.task_matches(child, |t| t.stopping)
        || state.task_matches(child, Task::awaits_user)
        || app.try_state::<DatabaseStore>().is_some_and(|db| {
            resume_armed(&db, child) || (!turn_failed && db.has_open_async_question(child))
        })
        || usage_resume::thread_busy(app, provider, &ThreadId(child.to_string())).await
}

/// Finalize a dispatched child that is idle after a finished turn.
pub async fn settle<R: Runtime>(app: &AppHandle<R>, child: &str) {
    let Some(state) = app.try_state::<DelegationState>() else {
        return;
    };
    let provider = state
        .lock()
        .tasks
        .get(child)
        .filter(|task| task.phase == Phase::Running && task.last_status.is_some())
        .map(|task| task.provider);
    let Some(provider) = provider else {
        return;
    };
    if child_busy(app, &state, provider, child).await {
        if let Some(db) = app.try_state::<DatabaseStore>() {
            if let Ok(Some(record)) = db.delegation_task_for_child(child) {
                let waiting = app
                    .try_state::<super::agent_chat::SubagentTracker>()
                    .is_some_and(|tracker| tracker.delegated_work_holding_turn(child));
                if waiting {
                    let _ = db.update_delegation_task(&record.task_id, |t| {
                        t.work_state = WorkState::WaitingForChildren
                    });
                }
            }
        }
        return;
    }
    let outcome = state
        .lock()
        .tasks
        .get(child)
        .filter(|task| task.last_status.is_some())
        .map(Task::outcome);
    if let Some(parent) = outcome.and_then(|phase| state.finish(app, child, phase)) {
        deliver(app, &parent).await;
    }
}

/// Delivery is owned by the committed task-result outbox, never by a round.
pub async fn deliver<R: Runtime>(app: &AppHandle<R>, parent: &str) {
    let Some(state) = app.try_state::<DelegationState>() else {
        return;
    };
    let children: Vec<String> = state
        .lock()
        .tasks
        .iter()
        .filter(|(_, t)| t.parent == parent && t.phase.is_terminal())
        .map(|(child, _)| child.clone())
        .collect();
    for child in children {
        if let Err(error) = publish_finished(app, &child) {
            log::error!("Task publication repair: {error}");
        }
    }
    crate::agent_control::delivery::notify();
    if let Some(db) = app.try_state::<DatabaseStore>() {
        if let Ok(rows) = db.pending_message_deliveries() {
            for row in rows.into_iter().filter(|row| row.thread_id == parent) {
                crate::agent_control::delivery::dispatch(app, row).await;
            }
        }
    }
}

/// Bounded repair for lost provider terminal events and uncommitted result
/// publication. Missing panes never imply cancellation.
pub async fn sweep_pass<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<DelegationState>() else {
        return;
    };
    let running: Vec<(String, ProviderKind)> = state
        .lock()
        .tasks
        .iter()
        .filter(|(_, task)| task.phase == Phase::Running)
        .map(|(child, task)| (child.clone(), task.provider))
        .collect();
    for (child, provider) in running {
        let idle = !child_busy(app, &state, provider, &child).await;
        let outcome = {
            let mut inner = state.lock();
            let Some(task) = inner.tasks.get_mut(&child) else {
                continue;
            };
            task.idle_passes = if idle {
                task.idle_passes.saturating_add(1)
            } else {
                0
            };
            (task.idle_passes >= SWEEP_IDLE_PASSES).then(|| task.outcome())
        };
        if let Some(phase) = outcome {
            state.finish(app, &child, phase);
        }
    }

    let parents: BTreeSet<String> = state
        .lock()
        .tasks
        .values()
        .map(|task| task.parent.clone())
        .collect();
    for parent in &parents {
        deliver(app, parent).await;
    }
}

/// Start the single sweep loop. Called once from app setup.
pub fn spawn_sweep<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick completes at once; there is nothing to sweep yet.
        interval.tick().await;
        loop {
            interval.tick().await;
            sweep_pass(&app).await;
        }
    });
}

/// Reconcile durable tasks before the provider/usage-resume workers start.
/// Retained terminal results keep their original outbox ownership.
pub fn drain_journal<R: Runtime>(app: &AppHandle<R>) {
    if let Err(error) = recover_tasks(app) {
        log::error!("Task recovery failed: {error}");
    }
    super::follow_ups::reconcile_on_launch(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(provider: ProviderKind, mode: Option<&str>) -> HostCaller {
        HostCaller {
            control: None,
            thread_id: "parent".into(),
            provider,
            workspace_id: Some("ws".into()),
            permission_mode: mode.map(str::to_string),
        }
    }

    fn task(round: u64, phase: Phase) -> Task {
        let mut task = Task::new(
            "parent",
            ProviderKind::Codex,
            "Add slugify helper",
            "Add slugify(text) to src/lib/strings.ts with table tests.",
            1_000,
        );
        task.round = round;
        task.finished_at_ms = phase.is_terminal().then_some(373_000);
        task.phase = phase;
        task
    }

    #[test]
    fn review600_concurrent_admission_and_cancel_share_authority_first_lock_order() {
        use std::sync::{Arc, mpsc};
        use std::time::{Duration, Instant};
        let thread = "review600-lock-order";
        let authority = crate::delegation::authority::NativeAuthority::new(thread);
        let generation = authority.begin(0);
        assert!(authority.bind(generation, "turn"));
        let identity = authority.capture().identity().unwrap();
        let control = crate::agent_control::ControlCaller {
            identity: crate::agent_control::CallerPrincipal::Thread {
                thread_id: thread.into(), provider: ProviderKind::Claude, provider_instance_id: "claude".into(), session_id: "session".into(),
                authority: identity.clone(), ceiling: crate::agent_control::permissions::Ceiling::for_mode(ProviderKind::Claude, Some("default")).unwrap(),
            }, ..crate::agent_control::ControlCaller::trusted()
        };
        let state = Arc::new(DelegationState::default());
        let (started_tx, started_rx) = mpsc::channel();
        let mut worker = None;
        let available = identity.admit(thread, || {
            let worker_state = state.clone();
            worker = Some(std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                worker_state.admit(Some(&control), |inner| {
                    inner.parents.entry(thread.into()).or_default().started_this_round += 1;
                    Ok(())
                })
            }));
            started_rx.recv().unwrap();
            // Cancellation owns authority and must be able to take live state.
            // Probe instead of blocking so an inverted order fails without hanging CI.
            let until = Instant::now() + Duration::from_millis(150);
            while Instant::now() < until {
                if state.inner.try_lock().is_err() { return Ok(false); }
                std::thread::yield_now();
            }
            state.lock().parents.entry(thread.into()).or_default().children.clear();
            Ok(true)
        }).unwrap();
        worker.unwrap().join().unwrap().unwrap();
        assert!(available, "admission held delegation state while waiting for cancellation's authority");
        assert_eq!(state.lock().parents[thread].started_this_round, 1);
    }

    #[test]
    fn parent_must_be_in_full_access_by_its_live_mode() {
        assert!(parent_is_full(
            ProviderKind::Claude,
            Some("bypassPermissions")
        ));
        assert!(
            parent_is_full(ProviderKind::Claude, None),
            "NULL is the provider default"
        );
        assert!(parent_is_full(
            ProviderKind::Claude,
            Some("danger-full-access")
        ));
        assert!(parent_is_full(
            ProviderKind::Codex,
            Some("danger-full-access")
        ));
        assert!(parent_is_full(
            ProviderKind::Codex,
            Some("bypassPermissions")
        ));
        // The Plan/Ask pills switch Claude's live mode to plan.
        assert!(!parent_is_full(ProviderKind::Claude, Some("plan")));
        // Accepting a plan leaves Claude in default (Supervised).
        assert!(!parent_is_full(ProviderKind::Claude, Some("default")));
        assert!(!parent_is_full(ProviderKind::Claude, Some("acceptEdits")));
        assert!(!parent_is_full(
            ProviderKind::Codex,
            Some("workspace-write")
        ));
        assert!(!parent_is_full(ProviderKind::Codex, Some("plan")));
        // Only Claude and Codex orchestrate.
        assert!(!parent_is_full(ProviderKind::Cursor, Some("agent")));
        assert!(!parent_is_full(ProviderKind::OpenCode, None));
        assert_eq!(mode_label(Some("plan")), "Plan mode");
        assert_eq!(mode_label(Some("default")), "Supervised");
    }

    #[test]
    fn tool_is_listed_only_for_eligible_callers() {
        let installed = |kind| {
            [
                ProviderKind::Codex,
                ProviderKind::Claude,
                ProviderKind::Hermes,
            ]
            .contains(&kind)
        };
        let claude = caller(ProviderKind::Claude, Some("plan"));
        // Claude lists by setting alone; its live mode is checked per call.
        assert_eq!(
            offered_agents(&claude, false, installed),
            vec![ProviderKind::Claude, ProviderKind::Codex],
            "own provider allowed, Hermes never, candidate order kept"
        );
        assert_eq!(offered_agents(&claude, true, installed).len(),2,"app-owned children may delegate within the durable root budget");
        assert!(
            offered_agents(&claude, false, |_| false).is_empty(),
            "nothing installed"
        );
        // Codex decides at session start, from the mode it started in.
        let codex_full = caller(ProviderKind::Codex, Some("danger-full-access"));
        assert_eq!(offered_agents(&codex_full, false, installed).len(), 2);
        let codex_ask = caller(ProviderKind::Codex, Some("workspace-write"));
        assert_eq!(offered_agents(&codex_ask, false, installed).len(),2);
        let cursor = caller(ProviderKind::Cursor, Some("agent"));
        assert!(offered_agents(&cursor, false, installed).is_empty());
    }

    #[test]
    fn child_input_is_an_allowlist_in_the_childs_full_mode() {
        let input = child_input(
            "child",
            PathBuf::from("/repo"),
            ProviderKind::Codex,
            Some("gpt-6.1-codex".into()),
            Some("high".into()),
            Some("danger-full-access".into()),
        );
        assert_eq!(input.thread_id.0, "child");
        assert_eq!(input.cwd, PathBuf::from("/repo"));
        assert_eq!(input.model.as_deref(), Some("gpt-6.1-codex"));
        assert_eq!(input.effort.as_deref(), Some("high"));
        assert_eq!(input.permission_mode.as_deref(), Some("danger-full-access"));
        assert!(input.fresh_session);
        assert!(input.resume_cursor.is_none());
        assert!(input.context_window.is_none());
        assert!(!input.fast_mode);
        assert!(input.additional_directories.is_empty());
        assert!(input.env.is_none());
        assert!(input.workspace_id.is_none());
        assert!(input.extra.is_null());
        assert!(input.recorded_usage_baseline.is_none());
        for (provider, mode) in [
            (ProviderKind::Claude, Some("bypassPermissions")),
            (ProviderKind::Cursor, Some("agent")),
            (ProviderKind::Grok, Some("agent")),
            (ProviderKind::OpenCode, None),
        ] {
            let input = child_input("c", PathBuf::from("/r"), provider, None, None, mode.map(str::to_string));
            assert_eq!(input.permission_mode.as_deref(), mode, "{provider:?}");
        }
    }

    #[test]
    fn finalize_fails_closed_on_anything_but_a_clean_finish() {
        assert_eq!(
            finalize_outcome(
                Some(&TurnStatus::Success),
                Some("  Done: added slugify.  "),
                None
            ),
            Phase::Completed("Done: added slugify.".into())
        );
        assert!(matches!(
            finalize_outcome(Some(&TurnStatus::Success), Some("   "), None),
            Phase::Failed(_)
        ));
        let error = TurnStatus::Error {
            subtype: "api_error".into(),
            message: "Invalid API key\n· Please run /login".into(),
        };
        assert_eq!(
            finalize_outcome(Some(&error), Some("partial"), None),
            Phase::Failed("Invalid API key · Please run /login".into())
        );
        let limit = TurnStatus::Error {
            subtype: crate::agent_provider::events::RATE_LIMIT_SUBTYPE.into(),
            message: "rate limited".into(),
        };
        assert!(matches!(
            finalize_outcome(Some(&limit), None, None),
            Phase::Failed(reason) if reason == "usage limit reached"
        ));
        assert!(matches!(
            finalize_outcome(Some(&TurnStatus::MaxTurns), Some("x"), None),
            Phase::Failed(_)
        ));
        assert!(matches!(
            finalize_outcome(Some(&TurnStatus::MaxBudget), Some("x"), None),
            Phase::Failed(_)
        ));
        // No TurnCompleted seen: failed, quoting the last captured text.
        match finalize_outcome(None, Some("Halfway through the parser"), None) {
            Phase::Failed(reason) => assert!(reason.contains("Halfway through the parser")),
            other => panic!("expected a failure, got {other:?}"),
        }
        let long = "x".repeat(500);
        match finalize_outcome(None, Some(&long), None) {
            Phase::Failed(reason) => assert!(reason.chars().count() <= REASON_CHARS + 1),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn card_snapshot_always_carries_the_full_field_set() {
        let mut starting = task(0, Phase::Starting);
        starting.model = Some("gpt-6.1-codex".into());
        starting.effort = Some("high".into());
        let snap = snapshot("child-1", &starting);
        assert_eq!(snap.subagent_id, "delegate:child-1");
        assert!(is_delegated_row(&snap.subagent_id));
        assert_eq!(snap.status, SubagentStatus::Pending);
        assert_eq!(snap.name.as_deref(), Some("Codex"));
        assert_eq!(snap.agent_type.as_deref(), Some("codex"));
        assert_eq!(snap.description.as_deref(), Some("Add slugify helper"));
        assert_eq!(snap.model.as_deref(), Some("gpt-6.1-codex"));
        assert_eq!(snap.effort.as_deref(), Some("high"));
        assert_eq!(snap.provider_ref.as_deref(), Some("child-1"));
        assert_eq!(snap.task_kind, Some(SubagentTaskKind::Agent));
        assert!(snap.parent_item_id.is_none());
        assert_eq!(snap.activity.as_deref(), Some("Starting…"));
        assert!(snap.duration_ms.is_none());

        let mut waiting = task(0, Phase::Running);
        waiting.open_requests.insert("req".into());
        assert_eq!(
            snapshot("c", &waiting).activity.as_deref(),
            Some("Waiting for your answer in its workspace")
        );
        waiting.open_requests.clear();
        assert_eq!(
            snapshot("c", &waiting).activity.as_deref(),
            Some("Working in its workspace")
        );
        waiting.paused_until = Some(1_800_000_000_000);
        assert!(snapshot("c", &waiting)
            .activity
            .is_some_and(|line| line.starts_with("Paused: Codex usage limit · resumes ")));

        let done = snapshot(
            "c",
            &task(0, Phase::Completed("y".repeat(CARD_RESULT_CHARS + 10))),
        );
        assert_eq!(done.status, SubagentStatus::Completed);
        assert_eq!(done.duration_ms, Some(372_000));
        assert!(done
            .result_text
            .is_some_and(|text| text.ends_with("… Full report in its workspace")));
        let stopped = snapshot("c", &task(0, Phase::Stopped(StopReason::AppClosed)));
        assert_eq!(stopped.status, SubagentStatus::Stopped);
        assert_eq!(
            stopped.result_text.as_deref(),
            Some("Codemux closed before this task finished.")
        );
    }

    #[test]
    fn readable_error_unwraps_serialized_provider_errors() {
        assert_eq!(
            readable_error(
                r#"{"kind":"not_authenticated","provider":"codex","hint":"Run `codex login` and try again."}"#
            ),
            "not signed in: Run `codex login` and try again."
        );
        assert_eq!(readable_error("plain words"), "plain words");
    }
    #[test]
    fn orchestration_message_child_footer_surfaces_truly_blocked_questions() {
        for footer in [CHILD_FOOTER,WORKTREE_CHILD_FOOTER] {
            assert!(footer.contains("Work independently"));
            assert!(footer.contains("Ask a question if truly blocked; it reaches the coordinator"));
            assert!(!footer.contains("do not ask questions"));
        }
    }

}
