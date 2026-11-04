//! Event-driven waits on native threads: `thread_wait` and `thread_status`,
//! the entry points every route calls.
//!
//! One process-wide change signal is raised after state is persisted (turn
//! ends, approvals, questions, secret cards, delivery rows). Waiters subscribe
//! before they read, so a change between the read and the wait is never lost,
//! and a fallback tick bounds the cost of a transition nobody signalled.
use super::outcome::{run_state, RunFacts, RunState, TurnEnd};
use super::{service, ControlCaller, ControlError, ErrorCode, NativeControlState};
use crate::database::agent_control::OperationReceipt;
use crate::database::message_delivery::DeliveryRecord;
use crate::database::DatabaseStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::watch;
use tokio::time::Instant;

/// Waits re-check on this tick too, so a missed signal costs latency, not a hang.
pub(crate) const FALLBACK_TICK: Duration = Duration::from_secs(5);
/// Bursts (subagent updates, queue churn) re-evaluate a waiter at most this often.
const MIN_EVALUATION_GAP: Duration = Duration::from_millis(100);

/// Outside clients choose up to the maximum. The default stops short of 60 s,
/// the request timeout of clients built on the MCP TypeScript SDK and the tool
/// timeout of older Codex releases (current ones allow 300 s), so a wait that
/// names no timeout returns before its client gives up.
pub(crate) const DEFAULT_WAIT_MS: u64 = 50_000;
pub(crate) const MAX_OUTSIDE_WAIT_MS: u64 = 600_000;
/// Agents inside Codemux call tools under their provider's ~60 s tool-call
/// limit (see secret_requests::CALL_WAIT), so their waits stop short of it.
pub(crate) const MAX_AGENT_WAIT_MS: u64 = 50_000;

pub(crate) fn max_wait_ms(caller: &ControlCaller) -> u64 {
    if caller.thread_id().is_some() { MAX_AGENT_WAIT_MS } else { MAX_OUTSIDE_WAIT_MS }
}

pub(crate) fn default_wait_ms(caller: &ControlCaller) -> u64 {
    DEFAULT_WAIT_MS.min(max_wait_ms(caller))
}

/// Tools that park. Transports read `ToolSpec::may_wait`; this only lets
/// tests check that the two agree.
#[cfg(test)]
pub(crate) fn is_long_wait(tool: &str) -> bool {
    tool == "thread_wait"
}

// ── The change signal ─────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub(super) struct Change {
    generation: u64,
    /// The thread the change concerns; `None` concerns every thread.
    thread: Option<Arc<str>>,
}

fn sender() -> &'static watch::Sender<Change> {
    static SIGNAL: OnceLock<watch::Sender<Change>> = OnceLock::new();
    SIGNAL.get_or_init(|| watch::channel(Change::default()).0)
}

/// Raise after persisting anything that can change `thread`'s wait view.
pub(crate) fn signal(thread: Option<&str>) {
    signal_on(sender(), thread);
}

/// Authority changed for some caller (grant revoked, connector disabled):
/// every waiter re-authorizes.
pub(crate) fn signal_all() {
    signal(None);
}

pub(super) fn signal_on(sender: &watch::Sender<Change>, thread: Option<&str>) {
    sender.send_modify(|change| {
        change.generation = change.generation.wrapping_add(1);
        change.thread = thread.filter(|thread| !thread.is_empty()).map(Arc::from);
    });
}

pub(crate) struct Changes {
    rx: watch::Receiver<Change>,
    thread: String,
    seen: u64,
}

/// Subscribe BEFORE the first read of the state being waited on.
pub(crate) fn subscribe(thread: &str) -> Changes {
    subscribe_on(sender(), thread)
}

pub(super) fn subscribe_on(sender: &watch::Sender<Change>, thread: &str) -> Changes {
    let mut rx = sender.subscribe();
    let seen = rx.borrow_and_update().generation;
    Changes { rx, thread: thread.to_string(), seen }
}

impl Changes {
    /// Resolves once something that may concern this thread changed. A single
    /// change to another thread is skipped. Coalesced changes (a generation
    /// jump) resolve, because one of them may have been ours. Cancel-safe:
    /// nothing awaits between `changed()` and `borrow_and_update()`.
    pub(crate) async fn changed(&mut self) {
        loop {
            if self.rx.changed().await.is_err() {
                return std::future::pending().await;
            }
            let change = self.rx.borrow_and_update().clone();
            let coalesced = change.generation != self.seen.wrapping_add(1);
            self.seen = change.generation;
            if coalesced || change.thread.as_deref().is_none_or(|thread| thread == self.thread) {
                return;
            }
        }
    }
}

/// Provider events that can change a wait view. Token deltas and items do
/// not: a run's final text is persisted before its TurnCompleted.
pub(crate) fn wakes_waiters(event: &crate::agent_provider::ProviderRuntimeEvent) -> bool {
    use crate::agent_provider::ProviderRuntimeEvent as E;
    matches!(
        event,
        E::SessionStateChanged { .. }
            | E::TurnCompleted { .. }
            | E::RequestOpened { .. }
            | E::RequestResolved { .. }
            | E::RequestResponseFailed { .. }
            | E::TurnQueued { .. }
            | E::QueuedTurnDispatched { .. }
            | E::QueuedTurnCancelled { .. }
            | E::QueuedTurnDisposed { .. }
            | E::QuestionsAsked { .. }
            | E::QuestionResolved { .. }
            | E::SubagentUpdated { .. }
            | E::RunStalled { .. }
            | E::UsageLimitReached { .. }
            | E::UsageResumeCancelled { .. }
    )
}

// ── Observation ───────────────────────────────────────────────────────────

/// Matches native `thread_read`'s largest `max_chars`.
const MAX_FINAL_TEXT_CHARS: usize = 16_000;
/// Per kind. Question text, titles and options are bounded too.
const MAX_PENDING: usize = 20;
const MAX_QUESTION_TEXT_CHARS: usize = 2_000;
const MAX_QUESTION_TITLE_CHARS: usize = 500;
const MAX_QUESTION_OPTIONS: usize = 20;
const MAX_OPTION_CHARS: usize = 200;

fn bounded(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// What a person must answer before the run can go on. Approval handles are
/// the outside handles `thread_respond` takes; secret cards never carry values.
#[derive(Debug, Serialize, Default, Clone, PartialEq)]
pub(crate) struct Pending {
    approvals: Vec<PendingApproval>,
    questions: Vec<PendingQuestion>,
    secrets: Vec<PendingSecret>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
struct PendingApproval {
    request_id: String,
    turn_id: String,
    request_kind: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
struct PendingQuestion {
    question_id: String,
    turn_id: String,
    text: String,
    questions: Vec<QuestionPrompt>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
struct QuestionPrompt {
    title: String,
    options: Vec<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
struct PendingSecret {
    request_id: String,
    label: String,
    reason: String,
    expires_at_ms: i64,
}

impl Pending {
    fn read(db: &DatabaseStore, thread: &str, status: &Value) -> Result<Self, ControlError> {
        let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
        let approvals = status["pending_approvals"].as_array().into_iter().flatten().take(MAX_PENDING)
            .map(|approval| PendingApproval {
                request_id: text(&approval["request_id"]),
                turn_id: text(&approval["turn_id"]),
                request_kind: text(&approval["request_kind"]),
            })
            .collect();
        // Questions omit the provider route (`target`) and source item.
        let questions = db.open_async_questions(thread, MAX_PENDING)?.into_iter()
            .map(|set| PendingQuestion {
                question_id: set.id,
                turn_id: set.source_turn_id,
                text: bounded(&set.text, MAX_QUESTION_TEXT_CHARS),
                questions: set.questions.into_iter().take(MAX_PENDING).map(|question| QuestionPrompt {
                    title: bounded(&question.title, MAX_QUESTION_TITLE_CHARS),
                    options: question.options.iter().take(MAX_QUESTION_OPTIONS).map(|option| bounded(option, MAX_OPTION_CHARS)).collect(),
                }).collect(),
            })
            .collect();
        let secrets = crate::commands::secret_requests::open_for_thread(thread).into_iter().take(MAX_PENDING)
            .map(|card| PendingSecret { request_id: card.id, label: card.label, reason: card.reason, expires_at_ms: card.expires_at_ms })
            .collect();
        Ok(Self { approvals, questions, secrets })
    }

    fn is_empty(&self) -> bool {
        self.approvals.is_empty() && self.questions.is_empty() && self.secrets.is_empty()
    }

    fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.approvals.iter().map(|a| format!("a:{}", a.request_id))
            .chain(self.questions.iter().map(|q| format!("q:{}", q.question_id)))
            .chain(self.secrets.iter().map(|s| format!("s:{}", s.request_id)))
            .collect();
        ids.sort();
        ids
    }
}

/// The run's last top-level assistant message, at most `MAX_FINAL_TEXT_CHARS`.
#[derive(Debug, Clone, PartialEq)]
struct FinalMessage {
    id: i64,
    text: String,
    truncated: bool,
}

impl FinalMessage {
    fn bounded((id, text): (i64, String)) -> Self {
        let truncated = text.chars().nth(MAX_FINAL_TEXT_CHARS).is_some();
        let text = if truncated { bounded(&text, MAX_FINAL_TEXT_CHARS) } else { text };
        Self { id, text, truncated }
    }
}

/// The wait cursor: it changes only when the state, the thread's current run,
/// the legacy settle flag or the set of pending items changes, so a client
/// passing it back as `since` blocks until something new happens.
pub(super) fn cursor(state: RunState, current_turn: Option<&str>, settled: bool, pending: &[String]) -> String {
    let digest = Sha256::digest(json!([state, current_turn, settled, pending]).to_string());
    digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

/// One read of a thread as a waiter sees it.
pub(crate) struct WaitView {
    /// The legacy `thread_status` object, unchanged.
    status: Value,
    /// The legacy settle rule for the selected run.
    settled: bool,
    /// The selected run is over: it settled, or its session went idle
    /// without a recorded end (a stop mid-turn), so it will never settle.
    ended: bool,
    /// Codemux has seen the run end, live or persisted.
    end_known: bool,
    /// Legacy `{turn_id, outcome}` of the selected run, when it has ended.
    selected_turn: Option<Value>,
    /// The selected run, else the thread's current or latest.
    turn_id: Option<String>,
    state: RunState,
    failure: Option<&'static str>,
    pending: Pending,
    paused_until_ms: Option<i64>,
    final_message: Option<FinalMessage>,
    cursor: String,
    /// `{operation_id, state, error}` of the receipt a wait on an operation follows.
    operation: Option<Value>,
}

impl WaitView {
    fn into_result(self, timed_out: bool) -> Value {
        let final_message = self.final_message.as_ref();
        let mut result = json!({
            "timed_out": timed_out,
            "settled": self.settled,
            "status": self.status,
            "selected_turn": self.selected_turn,
            "state": self.state,
            "turn_id": self.turn_id,
            "failure": self.failure,
            "final_text": final_message.map(|message| message.text.as_str()),
            "final_text_truncated": final_message.is_some_and(|message| message.truncated),
            "final_message_id": final_message.map(|message| message.id),
            "pending": self.pending,
            "paused_until_ms": self.paused_until_ms,
            "wait_cursor": self.cursor,
        });
        if let Some(operation) = self.operation {
            result["operation"] = operation;
        }
        result
    }

    /// `thread_status` is the legacy object plus the current run's state.
    pub(super) fn into_status(self) -> Value {
        let mut status = self.status;
        status["state"] = json!(self.state);
        status["failure"] = json!(self.failure);
        status["pending"] = json!(self.pending);
        status["paused_until_ms"] = json!(self.paused_until_ms);
        status["wait_cursor"] = json!(self.cursor);
        status
    }
}

/// Read `target` as a waiter on `selected` (or on its current run) sees it.
pub(crate) async fn observe<R: Runtime>(app: &AppHandle<R>, target: &service::Target, selected: Option<&str>) -> Result<WaitView, ControlError> {
    let status = service::thread_status(app, json!({"workspace_id": target.workspace_id, "thread_id": target.thread_id}));
    observe_with(app, target, selected, status).await
}

/// [`observe`] over a status read it has yet to await, so a test can move a
/// launch on while that read is under way.
pub(super) async fn observe_with<R: Runtime>(
    app: &AppHandle<R>,
    target: &service::Target,
    selected: Option<&str>,
    status: impl Future<Output = Result<Value, ControlError>>,
) -> Result<WaitView, ControlError> {
    let thread = target.thread_id.as_str();
    // A launch saves its new thread and shows its pane, then starts the
    // provider session, which can take seconds while MCP servers start, and
    // admits its message only after that. Until its receipt settles, the
    // thread reads unknown or idle with no run, yet its first run is coming.
    // The receipt is read before the status: a launch that admits its message
    // and settles while the status is read leaves that read idle with no run.
    let launch_open = app.state::<DatabaseStore>().control_thread_launch_open(thread, &app.state::<NativeControlState>().epoch)?;
    let status = status.await?;
    let db = app.state::<DatabaseStore>();
    let current = status["turn_id"].as_str();
    let selected_event = match selected {
        Some(turn) => db.control_turn_outcome(thread, turn)?,
        None => None,
    };
    // The legacy settle rule: a selected run settles once its end is recorded
    // and the thread went idle or moved on to a newer run.
    let settled = match selected {
        None => status["settled"] == true,
        Some(turn) => selected_event.is_some() && (status["settled"] == true || current.is_some_and(|current| current != turn)),
    };
    let selected_turn = selected_event.as_ref().map(|event| json!({
        "turn_id": event["turn_id"],
        "outcome": super::state::turn_outcome(event["status"]["kind"].as_str(), event["status"]["subtype"].as_str()).1,
    }));
    let turn_id = selected.or(current);
    // With no run in view, one may still be starting: a launch was starting
    // the thread when this read began, or a run with an ID or a message in
    // delivery landed after the status was read (the status named no run,
    // and an idle one had nothing in delivery). A launch that settled during
    // the read still counts; the next read sees its run.
    let (on_record, delivering) = match turn_id {
        None => (db.control_last_thread_run_event(thread)?, db.has_inflight_message_deliveries(thread)?),
        Some(_) => (None, false),
    };
    let run_named = on_record.as_ref().is_some_and(|event| event["turn_id"].as_str().is_some_and(|turn| !turn.is_empty()));
    let starting = turn_id.is_none() && (launch_open || run_named || delivering);
    let settled = settled && !starting;
    // A newer run's approvals and questions do not make an older selected run wait on input.
    let is_current = selected.is_none_or(|turn| current == Some(turn));
    let pending = Pending::read(&db, thread, &status)?;
    let paused_until_ms = if is_current {
        db.get_agent_chat_usage_resume(thread).and_then(|resume| resume.resume_at_ms)
    } else {
        None
    };
    let end_event = match (selected, turn_id) {
        (Some(_), _) => selected_event,
        (None, Some(turn)) => db.control_turn_outcome(thread, turn)?,
        (None, None) => None,
    };
    // The live view knows a run ended before its row is persisted.
    let live_end = status["last_turn"].as_object()
        .filter(|last| turn_id.is_some_and(|turn| last.get("turn_id").and_then(Value::as_str) == Some(turn)))
        .and_then(|last| last.get("status"));
    let end = end_event.as_ref().map(|event| TurnEnd::of_json(&event["status"])).or(live_end.map(TurnEnd::of_json));
    // A selected run that is not current and has no end: it was cut off once
    // a newer run started after it, or the thread never ran it. The first
    // rows naming the two runs order them. The live view can name a new run
    // before its row lands, or still show an older one after a receipt named
    // the new one, so a run with no row yet proves nothing.
    let (superseded, unknown) = match selected {
        Some(turn) if !is_current && end.is_none() => {
            let first = db.control_turn_first_row(thread, turn)?;
            let newer = match (first, current) {
                (Some(first), Some(current)) => db.control_turn_first_row(thread, current)?.is_some_and(|row| row > first),
                _ => false,
            };
            (newer, first.is_none())
        }
        _ => (false, false),
    };
    // While work is under way, an unknown turn may be a run whose first row
    // has yet to land; a thread at rest will not run it.
    if unknown && at_rest(&db, thread, &status)? {
        return Err(ControlError::new(ErrorCode::InvalidTarget, "This thread has no run with this turn_id"));
    }
    // A stopped session records no end for the run it cut off. Once the thread
    // is idle on that run, or has moved on to a newer one, it is over, though
    // the legacy rule never settles it. The live end above covers a run whose
    // end row has not landed yet.
    let vanished = turn_id.is_some() && is_current && status["settled"] == true && end.is_none();
    // With no turn to follow and nothing starting the thread, there is
    // nothing to wait for once the thread is idle, or once it is at rest
    // with no run on record at all (a thread Codemux has not reopened since a
    // restart never settles). A run a restart cut off stays open on a thread
    // at rest: restart resume may continue the chat through the normal send
    // path, which queues and delivers nothing.
    let never_ran = turn_id.is_none()
        && !starting
        && (settled || (at_rest(&db, thread, &status)? && on_record.is_none()));
    let ended = settled || vanished || superseded || never_ran;
    let final_message = match turn_id {
        Some(turn) if ended => db.control_turn_final_text(thread, turn)?.map(FinalMessage::bounded),
        _ => None,
    };
    let (state, failure) = if is_current && status["phase"] == "needs_attention" && pending.is_empty() {
        // A message delivery needs the person: the run it asked for never started.
        (RunState::Failed, Some("delivery_needs_attention"))
    } else if never_ran {
        (RunState::Done, None)
    } else {
        let (state, failure) = run_state(&RunFacts {
            ended,
            end,
            last_text: final_message.as_ref().map(|message| message.text.as_str()),
            open_requests: is_current && !pending.is_empty(),
            resume_armed: paused_until_ms.is_some(),
        });
        (state, failure.map(|failure| failure.code()))
    };
    let cursor = cursor(state, current, settled, &pending.ids());
    let end_known = end.is_some();
    let turn_id = turn_id.map(str::to_owned);
    Ok(WaitView { status, settled, ended, end_known, selected_turn, turn_id, state, failure, pending, paused_until_ms, final_message, cursor, operation: None })
}

/// Whether nothing on the thread can start a run any more: it is idle, or it
/// is `unknown` (no live session and no run in view, as for a thread Codemux
/// has not reopened since a restart, which never settles) with no message
/// queued or being delivered. A launch still starting the thread is not
/// seen here: `observe` asks its receipt (`starting`).
fn at_rest(db: &DatabaseStore, thread: &str, status: &Value) -> Result<bool, ControlError> {
    if status["settled"] == true {
        return Ok(true);
    }
    let empty = |key: &str| status[key].as_array().is_none_or(Vec::is_empty);
    // Redundant today: with an empty queue, uncertain entries already make
    // the phase `needs_attention`. Kept so this rule does not lean on that.
    Ok(status["phase"] == "unknown" && empty("queued_ids") && empty("uncertain_queued_ids") && !db.has_inflight_message_deliveries(thread)?)
}

/// Where the message of a launch or send stands, by its receipt.
enum Placement {
    /// Codemux is still delivering it, or it waits in the thread's queue.
    NotStarted,
    /// It started this run, or was steered into it.
    Run(String),
    /// It will never run: its receipt failed, or needs the person.
    Ended(&'static str),
}

impl Placement {
    fn of(db: &DatabaseStore, principal: &str, receipt: &OperationReceipt) -> Result<Self, ControlError> {
        let result = receipt.result.as_ref().unwrap_or(&Value::Null);
        let text = |value: &Value| value.as_str().filter(|text| !text.is_empty()).map(str::to_owned);
        let queued = result["accepted_as"] == "queued" || text(&result["turn"]["queued_id"]).is_some();
        match receipt.state.as_str() {
            "failed" => Ok(Self::Ended("operation_failed")),
            "needs_attention" => Ok(Self::Ended("delivery_needs_attention")),
            "succeeded" => match text(&result["turn"]["turn_id"]) {
                Some(turn) => Ok(Self::Run(turn)),
                // A send's receipt follows its message's delivery. A launch's
                // settles once, while its message may still be queued, so
                // that message's own delivery says where it stands.
                None if queued && receipt.tool == "thread_launch" => Ok(db.control_launch_delivery(principal, &receipt.operation_id)?
                    .map_or(Self::NotStarted, |delivery| Self::of_delivery(&delivery))),
                None if queued => Ok(Self::NotStarted),
                None => Err(ControlError::new(ErrorCode::InvalidTarget, "This operation started no run (a launch without a message); wait on the thread instead")),
            },
            // Accepted or running: Codemux is still carrying it out.
            _ => Ok(Self::NotStarted),
        }
    }

    /// A message by its delivery, read as `finish_message_delivery` maps a
    /// delivery onto a send's receipt: cancelled or rejected fails, held
    /// needs the person, and a delivery that names its run started it.
    fn of_delivery(delivery: &DeliveryRecord) -> Self {
        match delivery.state.as_str() {
            "cancelled" | "rejected" => Self::Ended("operation_failed"),
            "held" | "held_unknown" => Self::Ended("delivery_needs_attention"),
            _ => match delivery.turn_id.as_deref().filter(|turn| !turn.is_empty()) {
                Some(turn) => Self::Run(turn.to_owned()),
                None => Self::NotStarted,
            },
        }
    }
}

/// Read the run `operation` started, as a waiter on it sees it. The operation
/// is the caller's own `thread_launch` or `thread_send` for `target`; its
/// receipt, or a launch's message delivery, names the run once the message
/// starts it.
async fn observe_operation<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, target: &service::Target, operation: &str) -> Result<WaitView, ControlError> {
    let epoch = app.state::<NativeControlState>().epoch.clone();
    let receipt = app.state::<DatabaseStore>().control_operation(&caller.principal, operation, &epoch)?
        .ok_or_else(|| ControlError::new(ErrorCode::OperationNotFound, "No operation belongs to this client and ID"))?;
    if !matches!(receipt.tool.as_str(), "thread_launch" | "thread_send") {
        return Err(ControlError::new(ErrorCode::InvalidTarget, "operation_id must name a thread_launch or thread_send"));
    }
    if receipt.workspace_id.as_deref() != Some(target.workspace_id.as_str()) || receipt.thread_id.as_deref() != Some(target.thread_id.as_str()) {
        return Err(ControlError::new(ErrorCode::InvalidTarget, "This operation is for another workspace or thread"));
    }
    let placement = Placement::of(&app.state::<DatabaseStore>(), &caller.principal, &receipt)?;
    let mut view = match placement {
        Placement::Run(turn) => observe(app, target, Some(&turn)).await?,
        Placement::NotStarted => before_run(app, target, None).await?,
        Placement::Ended(failure) => before_run(app, target, Some(failure)).await?,
    };
    view.operation = Some(json!({"operation_id": receipt.operation_id, "state": receipt.state, "error": receipt.error}));
    Ok(view)
}

/// A message whose run has not started: Codemux is still delivering it, it
/// waits in the queue, or (`failure`) it never will. Input the thread waits
/// on holds the queue, so it reads as this message's too.
async fn before_run<R: Runtime>(app: &AppHandle<R>, target: &service::Target, failure: Option<&'static str>) -> Result<WaitView, ControlError> {
    let thread = match observe(app, target, None).await {
        Ok(view) => Some(view),
        // A launch names its thread, and a worktree launch its workspace, before it creates them.
        Err(error) if matches!(error.code, ErrorCode::ThreadNotFound | ErrorCode::WorkspaceNotFound) => None,
        Err(error) => return Err(error),
    };
    let blocked = thread.as_ref().is_some_and(|view| view.state == RunState::NeedsInput);
    let state = match failure {
        Some(_) => RunState::Failed,
        None if blocked => RunState::NeedsInput,
        None => RunState::Running,
    };
    let (status, pending, paused_until_ms) = match thread {
        Some(view) => (view.status, view.pending, view.paused_until_ms),
        None => (Value::Null, Pending::default(), None),
    };
    // No run of its own yet, so the cursor moves when that run starts.
    let cursor = cursor(state, None, false, &pending.ids());
    Ok(WaitView {
        status, settled: false, ended: failure.is_some(), end_known: false, selected_turn: None, turn_id: None,
        state, failure, pending, paused_until_ms, final_message: None, cursor, operation: None,
    })
}

/// One read of what a wait follows: an operation's run, else `selected`, else
/// the thread's current run.
async fn read<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, target: &service::Target, operation: Option<&str>, selected: Option<&str>) -> Result<WaitView, ControlError> {
    match operation {
        Some(operation) => observe_operation(app, caller, target, operation).await,
        None => observe(app, target, selected).await,
    }
}

// ── Tools ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Args {
    workspace_id: String,
    thread_id: String,
    turn_id: Option<String>,
    /// A `thread_launch` or `thread_send` receipt of this caller: follow the
    /// run its message starts.
    operation_id: Option<String>,
    timeout_ms: Option<u64>,
    since: Option<String>,
}

/// Without `since`, returns when the selected run settles or is otherwise
/// over, when its view changes from the start of the call (an approval opens,
/// a question appears, the state flips) or at the deadline; something pending
/// at the start does not return early. A run that is over without a recorded
/// end (a stop cut it off, or a newer run replaced it) returns at once on
/// every call with `settled: false`, so loops stop on `state`, not on
/// `settled`. With `since` (a previous `wait_cursor`), returns once the view
/// differs from it. `operation_id` selects the run a launch's or send's
/// message starts, once it starts; until then the wait reads `running`.
pub(crate) async fn thread_wait<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args: Args = service::parse(args)?;
    let maximum = max_wait_ms(caller);
    let timeout = args.timeout_ms.unwrap_or_else(|| default_wait_ms(caller));
    if !(1..=maximum).contains(&timeout) {
        return Err(ControlError::new(ErrorCode::InvalidArguments, format!("timeout_ms must be between 1 and {maximum}")));
    }
    if let Some(turn) = &args.turn_id { service::identifier(turn, "turn_id", 256)?; }
    if let Some(operation) = &args.operation_id { service::identifier(operation, "operation_id", 256)?; }
    if args.turn_id.is_some() && args.operation_id.is_some() {
        return Err(ControlError::new(ErrorCode::InvalidArguments, "Pass turn_id or operation_id, not both"));
    }
    if let Some(since) = &args.since { service::identifier(since, "since", 64)?; }
    let deadline = Instant::now() + Duration::from_millis(timeout);
    let target = service::Target { workspace_id: args.workspace_id, thread_id: args.thread_id };
    let operation = args.operation_id.as_deref();
    let mut changes = subscribe(&target.thread_id);
    service::authorize(app, caller)?;
    let first = read(app, caller, &target, operation, args.turn_id.as_deref()).await?;
    // Legacy: a thread already settled at call start returns at once.
    if args.turn_id.is_none() && operation.is_none() && args.since.is_none() && first.settled {
        return Ok(first.into_result(false));
    }
    // A chat's own run cannot end while it waits on it, so this would only
    // hold the turn for the whole timeout.
    if caller.thread_id() == Some(target.thread_id.as_str()) {
        return Err(ControlError::new(ErrorCode::InvalidTarget, "A chat cannot wait for its own run"));
    }
    // Follow only the run in progress, never a newer one (legacy adoption).
    // A run whose end is already known is not in progress: right after
    // thread_send the thread is busy with a delivery while its current run is
    // still the finished one, and adopting that would return its old answer
    // as soon as the new run starts. Follow the thread instead; the new run
    // appearing changes the view, and the next wait adopts it.
    // An operation's receipt selects its run on every read.
    let adopt = operation.is_none() && !first.settled && !first.end_known;
    let selected = args.turn_id.clone().or_else(|| adopt.then(|| first.turn_id.clone()).flatten());
    let baseline = args.since.clone().unwrap_or_else(|| first.cursor.clone());
    let mut view = first;
    let mut evaluated = Instant::now();
    loop {
        let changed = view.cursor != baseline;
        let returns = if args.since.is_some() { changed } else { view.ended || changed };
        if returns { return Ok(view.into_result(false)); }
        if Instant::now() >= deadline { return Ok(view.into_result(true)); }
        tokio::select! {
            biased;
            _ = caller.revoked() => {
                return Err(ControlError::new(ErrorCode::StaleCaller, "Wait ended: the calling chat was stopped or narrowed"));
            }
            _ = changes.changed() => {}
            _ = tokio::time::sleep(FALLBACK_TICK) => {}
            _ = tokio::time::sleep_until(deadline) => {}
        }
        let next = evaluated + MIN_EVALUATION_GAP;
        if Instant::now() < next {
            tokio::time::sleep_until(next.min(deadline)).await;
        }
        // A revoked grant reads grant_revoked; a disabled connector, connector_disabled.
        service::authorize(app, caller)?;
        view = read(app, caller, &target, operation, selected.as_deref()).await?;
        evaluated = Instant::now();
    }
}

/// The legacy status object plus `state`, `failure`, `pending`,
/// `paused_until_ms` and `wait_cursor` for the thread's current run.
pub(crate) async fn thread_status<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    let target: service::Target = service::parse(args)?;
    Ok(observe(app, &target, None).await?.into_status())
}
