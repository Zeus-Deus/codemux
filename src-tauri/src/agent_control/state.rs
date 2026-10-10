use crate::agent_provider::{ProviderRuntimeEvent, SessionStatus, TurnStartResult, TurnStatus};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
};

/// Keep the Arc throughout admission/dispatch: pruning must never split an
/// active or waiting native lifecycle transaction into two independent gates.
#[derive(Default)]
pub(crate) struct ThreadGate {
    generation: AtomicU64,
    cancelled: tokio::sync::Notify,
    pub(crate) dispatch: Arc<tokio::sync::Mutex<()>>,
}
impl ThreadGate {
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    pub(crate) fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancelled.notify_waiters();
    }
    pub(crate) async fn cancelled_since(&self, generation: u64) {
        loop {
            let notified = self.cancelled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.generation() != generation { return; }
            notified.await;
        }
    }
    pub(crate) fn verify(&self, generation: u64) -> Result<(), String> {
        if self.generation() == generation {
            Ok(())
        } else {
            Err("cancelled: native operation stopped before dispatch".into())
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct PendingApproval {
    pub request_id: String,
    #[serde(skip)]
    pub(crate) outside_handle: String,
    #[serde(skip)]
    pub(crate) permission_options: Vec<(String, String)>,
    pub turn_id: String,
    pub request_kind: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct ThreadRuntimeView {
    pub phase: String,
    pub turn_id: Option<String>,
    pub last_turn: Option<Value>,
    pub pending_approvals: Vec<PendingApproval>,
    pub queued_ids: Vec<String>,
}
impl Default for ThreadRuntimeView {
    fn default() -> Self {
        Self {
            phase: "unknown".into(),
            turn_id: None,
            last_turn: None,
            pending_approvals: vec![],
            queued_ids: vec![],
        }
    }
}
const MAX_RUNTIME_THREADS: usize = 1024;
const RECENT_IDS: usize = 256;
struct RuntimeEntry {
    view: ThreadRuntimeView,
    seen_turns: VecDeque<String>,
    completed_turns: VecDeque<String>,
    process_dead: bool,
    retired_queues: VecDeque<String>,
}
impl Default for RuntimeEntry {
    fn default() -> Self {
        Self {
            view: Default::default(),
            seen_turns: Default::default(),
            completed_turns: Default::default(),
            process_dead: false,
            retired_queues: Default::default(),
        }
    }
}
fn remember(ids: &mut VecDeque<String>, id: &str) {
    if id.is_empty() || ids.iter().any(|seen| seen == id) {
        return;
    }
    if ids.len() == RECENT_IDS {
        ids.pop_front();
    }
    ids.push_back(id.into());
}
impl RuntimeEntry {
    fn inactive(&self) -> bool {
        self.view.pending_approvals.is_empty()
            && self.view.queued_ids.is_empty()
            && matches!(
                self.view.phase.as_str(),
                "completed" | "interrupted" | "error" | "closed"
            )
    }
    fn clear_callbacks(&mut self) {
        self.view.pending_approvals.clear();
        for id in self.view.queued_ids.drain(..) {
            remember(&mut self.retired_queues, &id);
        }
    }
}
// None means the live worker's permission state is uncertain. A DB-only
// future mode or a failed live apply must not masquerade as current authority.
#[derive(Clone)]
struct WorkerMode {
    provider: crate::agent_provider::ProviderKind,
    mode: Option<String>,
    confirmed: bool,
}

/// Per-app process identity, lifecycle owners and sanitized observations.
/// No persisted request is promoted into a current-process callback.
pub struct NativeControlState {
    #[cfg(test)]
    pub(crate) fixture_operation_barrier: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    pub epoch: String,
    gates: Mutex<HashMap<String, Weak<ThreadGate>>>,
    runtime: Mutex<HashMap<String, RuntimeEntry>>,
    worker_modes: Mutex<HashMap<String, WorkerMode>>,
    #[cfg(test)]
    pub(crate) fixture_capabilities: Mutex<
        HashMap<
            crate::agent_provider::ProviderKind,
            crate::agent_provider::ProviderChatCapabilities,
        >,
    >,
}
impl Default for NativeControlState {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            fixture_operation_barrier: Default::default(),
            epoch: uuid::Uuid::new_v4().to_string(),
            gates: Default::default(),
            runtime: Default::default(),
            worker_modes: Default::default(),
            #[cfg(test)]
            fixture_capabilities: Default::default(),
        }
    }
}
impl NativeControlState {
    pub(crate) fn thread_gate(&self, thread: &str) -> Arc<ThreadGate> {
        let mut gates = self.gates.lock().unwrap();
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(thread).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(ThreadGate::default());
        gates.insert(thread.into(), Arc::downgrade(&gate));
        gate
    }
    pub(crate) fn record_worker_mode(
        &self,
        thread: &str,
        provider: crate::agent_provider::ProviderKind,
        mode: Option<String>,
    ) {
        self.worker_modes.lock().unwrap().insert(
            thread.into(),
            WorkerMode {
                provider,
                mode,
                confirmed: true,
            },
        );
    }
    pub(crate) fn mark_worker_mode_uncertain(&self, thread: &str) {
        if let Some(mode) = self.worker_modes.lock().unwrap().get_mut(thread) {
            mode.confirmed = false;
        }
    }
    pub(crate) fn clear_worker(&self, thread: &str) {
        self.worker_modes.lock().unwrap().remove(thread);
    }
    pub(crate) fn verify_worker_mode(
        &self,
        thread: &str,
        provider: crate::agent_provider::ProviderKind,
        mode: Option<&str>,
    ) -> Result<(), String> {
        if self
            .worker_modes
            .lock()
            .unwrap()
            .get(thread)
            .is_some_and(|live| {
                !live.confirmed || live.provider != provider || live.mode.as_deref() != mode
            })
        {
            Err("permission_state_unconfirmed: live worker permissions differ from persisted configuration; apply the mode in CodeMux or restart the worker".into())
        } else {
            Ok(())
        }
    }
    fn entry<'a>(
        runtime: &'a mut HashMap<String, RuntimeEntry>,
        thread: &str,
    ) -> Option<&'a mut RuntimeEntry> {
        if !runtime.contains_key(thread) && runtime.len() >= MAX_RUNTIME_THREADS {
            let remove = runtime
                .iter()
                .find(|(_, entry)| entry.inactive())
                .map(|(id, _)| id.clone());
            if let Some(id) = remove {
                runtime.remove(&id);
            } else {
                return None;
            }
        }
        Some(runtime.entry(thread.into()).or_default())
    }
    pub fn observe_event(&self, event: &ProviderRuntimeEvent) {
        let thread = match event {
            ProviderRuntimeEvent::SessionStateChanged { thread_id, .. }
            | ProviderRuntimeEvent::TurnCompleted { thread_id, .. }
            | ProviderRuntimeEvent::RequestOpened { thread_id, .. }
            | ProviderRuntimeEvent::RequestResolved { thread_id, .. }
            | ProviderRuntimeEvent::RequestResponseFailed { thread_id, .. }
            | ProviderRuntimeEvent::TurnQueued { thread_id, .. }
            | ProviderRuntimeEvent::QueuedTurnDispatched { thread_id, .. }
            | ProviderRuntimeEvent::QueuedTurnCancelled { thread_id, .. } => &thread_id.0,
            _ => return,
        };
        if thread.is_empty() {
            return;
        }
        let mut runtime = self.runtime.lock().unwrap();
        let Some(entry) = Self::entry(&mut runtime, thread) else {
            return;
        };
        // Starting explicitly opens a new process epoch. Requests and queued
        // work delivered after a closed/error boundary are dead callbacks,
        // not evidence that the old process came back to life.
        if entry.process_dead
            && matches!(
                event,
                ProviderRuntimeEvent::RequestOpened { .. }
                    | ProviderRuntimeEvent::TurnQueued { .. }
                    | ProviderRuntimeEvent::QueuedTurnDispatched { .. }
            )
        {
            return;
        }
        match event {
            ProviderRuntimeEvent::SessionStateChanged { status, .. } => match status {
                SessionStatus::Starting => {
                    entry.process_dead = false;
                    entry.clear_callbacks();
                    entry.view.turn_id = None;
                    entry.view.phase = "starting".into();
                }
                SessionStatus::Ready => {
                    entry.process_dead = false;
                    if !entry.view.pending_approvals.is_empty() {
                        entry.view.phase = "waiting_approval".into();
                    } else if !matches!(
                        entry.view.phase.as_str(),
                        "completed" | "interrupted" | "error"
                    ) {
                        entry.view.phase = "ready".into();
                        entry.view.turn_id = None;
                    }
                }
                SessionStatus::Running { active_turn } => {
                    // A late running notice must not reopen a completed turn.
                    if entry
                        .view
                        .last_turn
                        .as_ref()
                        .is_none_or(|last| last["turn_id"] != active_turn.0)
                    {
                        entry.process_dead = false;
                        remember(&mut entry.seen_turns, &active_turn.0);
                        entry.view.turn_id = Some(active_turn.0.clone());
                        entry.view.phase = if entry.view.pending_approvals.is_empty() {
                            "running"
                        } else {
                            "waiting_approval"
                        }
                        .into();
                    }
                }
                SessionStatus::WaitingApproval { .. } => {
                    if !entry.process_dead {
                        entry.view.phase = "waiting_approval".into();
                    }
                }
                SessionStatus::Error { .. } => {
                    entry.process_dead = true;
                    entry.clear_callbacks();
                    entry.view.phase = "error".into();
                }
                SessionStatus::Closed => {
                    entry.process_dead = true;
                    entry.clear_callbacks();
                    entry.view.phase = "closed".into();
                }
            },
            ProviderRuntimeEvent::RequestOpened {
                turn_id,
                request_id,
                request_kind,
                payload,
                ..
            } => {
                if entry.completed_turns.contains(&turn_id.0) {
                    return;
                }
                remember(&mut entry.seen_turns, &turn_id.0);
                if !entry
                    .view
                    .pending_approvals
                    .iter()
                    .any(|p| p.request_id == request_id.0)
                {
                    entry.view.pending_approvals.push(PendingApproval {
                        request_id: request_id.0.clone(),
                        outside_handle: uuid::Uuid::new_v4().to_string(),
                        permission_options: payload.get("options").and_then(Value::as_array)
                            .into_iter().flatten().take(64).filter_map(|option| {
                                let kind = option.get("kind")?.as_str()?;
                                let id = option.get("optionId")?.as_str()?;
                                if !matches!(kind, "allow_once" | "allow_always" | "reject_once" | "reject_always")
                                    || id.is_empty() || id.len() > 256 { return None; }
                                Some((kind.to_string(), id.to_string()))
                            }).collect(),
                        turn_id: turn_id.0.clone(),
                        request_kind: request_kind.clone(),
                    });
                }
                entry.view.turn_id = Some(turn_id.0.clone());
                entry.view.phase = "waiting_approval".into();
            }
            ProviderRuntimeEvent::RequestResolved { request_id, .. }
            | ProviderRuntimeEvent::RequestResponseFailed { request_id, .. } => {
                entry
                    .view
                    .pending_approvals
                    .retain(|p| p.request_id != request_id.0);
                if entry.view.pending_approvals.is_empty() && entry.view.phase == "waiting_approval"
                {
                    entry.view.phase = if entry.view.turn_id.is_some() {
                        "running"
                    } else {
                        "ready"
                    }
                    .into();
                }
            }
            ProviderRuntimeEvent::TurnCompleted {
                turn_id, status, ..
            } => {
                if entry.completed_turns.contains(&turn_id.0) {
                    return;
                }
                remember(&mut entry.completed_turns, &turn_id.0);
                remember(&mut entry.seen_turns, &turn_id.0);
                let (phase, kind) = match status {
                    TurnStatus::Success => ("completed", "success"),
                    TurnStatus::Error { subtype, .. } if subtype == "interrupted" => {
                        ("interrupted", "interrupted")
                    }
                    TurnStatus::Error { .. } => ("error", "error"),
                    TurnStatus::MaxTurns => ("error", "max_turns"),
                    TurnStatus::MaxBudget => ("error", "max_budget"),
                };
                entry.view.last_turn = Some(
                    json!({"type":"turn_completed","turn_id":turn_id.0,"status":{"kind":kind}}),
                );
                entry
                    .view
                    .pending_approvals
                    .retain(|p| p.turn_id != turn_id.0);
                if entry.view.phase != "closed"
                    && entry
                        .view
                        .turn_id
                        .as_ref()
                        .is_none_or(|active| active == &turn_id.0)
                {
                    entry.view.turn_id = Some(turn_id.0.clone());
                    entry.view.phase = phase.into();
                }
            }
            ProviderRuntimeEvent::TurnQueued { queued_id, .. } => {
                if !entry.retired_queues.contains(queued_id)
                    && !entry.view.queued_ids.contains(queued_id)
                {
                    entry.view.queued_ids.push(queued_id.clone());
                }
            }
            ProviderRuntimeEvent::QueuedTurnDispatched {
                queued_id,
                turn_id,
                steered,
                ..
            } => {
                entry.view.queued_ids.retain(|id| id != queued_id);
                remember(&mut entry.retired_queues, queued_id);
                remember(&mut entry.seen_turns, &turn_id.0);
                if !steered
                    && entry
                        .view
                        .last_turn
                        .as_ref()
                        .is_none_or(|last| last["turn_id"] != turn_id.0)
                {
                    entry.view.turn_id = Some(turn_id.0.clone());
                    entry.view.phase = if entry.view.pending_approvals.is_empty() {
                        "running"
                    } else {
                        "waiting_approval"
                    }
                    .into();
                }
            }
            ProviderRuntimeEvent::QueuedTurnCancelled { queued_id, .. } => {
                entry.view.queued_ids.retain(|id| id != queued_id);
                remember(&mut entry.retired_queues, queued_id);
            }
            _ => {}
        }
    }
    pub fn thread_runtime(&self, thread: &str) -> Option<ThreadRuntimeView> {
        self.runtime
            .lock()
            .unwrap()
            .get(thread)
            .map(|entry| entry.view.clone())
    }
    pub fn request_is_current(&self, thread: &str, request: &str) -> bool {
        self.runtime
            .lock()
            .unwrap()
            .get(thread)
            .is_some_and(|entry| {
                entry
                    .view
                    .pending_approvals
                    .iter()
                    .any(|p| p.request_id == request)
            })
    }
    pub(crate) fn exact_acp_decision(&self, thread: &str, request: &str, decision: &crate::agent_provider::ApprovalDecision) -> Result<crate::agent_provider::ApprovalDecision, String> {
        use crate::agent_provider::ApprovalDecision;
        let runtime = self.runtime.lock().unwrap();
        let pending = runtime.get(thread).and_then(|entry| entry.view.pending_approvals.iter().find(|p| p.request_id == request))
            .ok_or_else(|| "stale_provider_callback: no current request".to_string())?;
        if pending.request_kind != "tool_approval" {
            return Err("unsupported_decision: exact permission decisions cannot answer this request kind".into());
        }
        let kinds: &[&str] = match decision {
            ApprovalDecision::Allow { .. } => &["allow_once"],
            ApprovalDecision::AllowForSession => &["allow_always"],
            ApprovalDecision::Deny { .. } => &["reject_once", "reject_always"],
            ApprovalDecision::Cancel => return Ok(ApprovalDecision::Cancel),
            ApprovalDecision::ProviderOption { option_id } if pending.permission_options.iter().any(|(_, id)| id == option_id) => return Ok(decision.clone()),
            _ => return Err("unsupported_decision: option was not advertised".into()),
        };
        let option_id = kinds.iter().find_map(|kind| pending.permission_options.iter().find(|(k, _)| k == kind).map(|(_, id)| id.clone()))
            .ok_or_else(|| "unsupported_decision: exact requested scope was not advertised".to_string())?;
        Ok(ApprovalDecision::ProviderOption { option_id })
    }
    pub(crate) fn provider_request_for_handle(&self, thread: &str, handle: &str) -> Option<String> {
        self.runtime.lock().unwrap().get(thread)?
            .view.pending_approvals.iter().find(|p| p.outside_handle == handle)
            .map(|p| p.request_id.clone())
    }
    /// Provider events and the RPC return race. Events own settled/queue state;
    /// an acknowledgement supplies missing state only, never resurrects it.
    pub fn record_accepted_turn(&self, thread: &str, result: &TurnStartResult) {
        let mut runtime = self.runtime.lock().unwrap();
        let Some(entry) = Self::entry(&mut runtime, thread) else {
            return;
        };
        if entry.process_dead {
            return;
        }
        if let Some(id) = &result.queued_id {
            if !entry.retired_queues.contains(id) && !entry.view.queued_ids.contains(id) {
                entry.view.queued_ids.push(id.clone());
            }
        } else if !result.steered
            && !result.turn_id.0.is_empty()
            && !entry.seen_turns.contains(&result.turn_id.0)
        {
            remember(&mut entry.seen_turns, &result.turn_id.0);
            entry.view.turn_id = Some(result.turn_id.0.clone());
            entry.view.phase = "running".into();
        }
    }
}
