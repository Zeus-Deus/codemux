//! One Cursor ACP subprocess and session per Codemux thread.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent_provider::{
    pricing, ApprovalDecision, CompletedItem, ContentDelta, ContextUsageSnapshot, CostSource,
    ProviderError, ProviderKind, ProviderRuntimeEvent, ProviderSessionId, RequestId, SendOutcome,
    SendTurnInput, SessionStatus, TaskSnapshotItem, TaskStatus, TasksSnapshot, ThreadId, TurnId,
    TurnStatus,
};
use crate::json_rpc_child::{
    IncomingRequest, JsonRpcChild, Notification, RpcChildError, RpcError, SpawnConfig,
};

use super::capabilities::{initialize_params, looks_unauthenticated};
use super::protocol::{
    config_id_for, config_options, option_by_id, resolve_boolean_value, resolve_effort_value,
    resolve_select_value, session_id, set_config_params, ConfigKind,
};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const PROMPT_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const DISPATCHING_TURN_ID: &str = "cursor-dispatching-turn";

#[derive(Debug, Clone)]
pub struct CursorSpawnConfig {
    pub binary: PathBuf,
}

#[derive(Debug, Clone)]
struct QueuedTurn {
    id: String,
    input: SendTurnInput,
}

#[derive(Debug, Clone)]
struct CursorQuestion {
    id: String,
    prompt: String,
    options: Vec<CursorQuestionOption>,
}

#[derive(Debug, Clone)]
struct CursorQuestionOption {
    id: String,
    label: String,
}

#[derive(Debug, Clone)]
enum PendingRequest {
    Permission {
        rpc_id: Value,
        options: Vec<Value>,
    },
    UserInput {
        rpc_id: Value,
        questions: Vec<CursorQuestion>,
    },
    Plan {
        rpc_id: Value,
    },
}

/// Running token totals observed from Cursor's `usage_update`, so the
/// ledger can be fed deltas from a cumulative report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CursorUsageTotals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

pub(crate) struct CursorSessionState {
    pub status: SessionStatus,
    pub active_turn: Option<TurnId>,
    model: Option<String>,
    permission_mode: String,
    config_options: Vec<Value>,
    pending: HashMap<RequestId, PendingRequest>,
    queued: VecDeque<QueuedTurn>,
    assistant_text: String,
    thinking_text: String,
    /// `true` between the moment `session/prompt` is written and the
    /// moment its response lands. Interrupts outside that window cannot
    /// be delivered as `session/cancel` — the child has no turn to
    /// cancel yet — so they are recorded in `pending_interrupt` instead.
    prompt_in_flight: bool,
    /// A Stop the user pressed while the turn was still being dispatched
    /// (permission/model/effort config round-trips, queue promotion).
    /// Consumed by the prompt worker, which then never sends the prompt.
    pending_interrupt: bool,
    /// High-water mark of the cumulative usage Cursor last reported.
    usage_totals: CursorUsageTotals,
}

/// A token a message pump answers once it has drained everything the
/// child wrote ahead of it. See
/// [`CursorSession::await_child_messages_drained`].
type NotificationBarrier = oneshot::Sender<()>;

pub(crate) struct CursorSession {
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
    pub state: Mutex<CursorSessionState>,
    child: Arc<JsonRpcChild>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Ordering barriers into the two tasks that consume child messages.
    /// See [`CursorSession::await_child_messages_drained`].
    notification_barrier_tx: mpsc::UnboundedSender<NotificationBarrier>,
    request_barrier_tx: mpsc::UnboundedSender<NotificationBarrier>,
    stopped: AtomicBool,
}

impl CursorSession {
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_and_initialize(
        thread_id: ThreadId,
        cwd: PathBuf,
        model: Option<String>,
        permission_mode: Option<String>,
        effort: Option<String>,
        context_window: Option<String>,
        fast_mode: bool,
        resume_cursor: Option<Value>,
        env: Option<HashMap<String, String>>,
        spawn: CursorSpawnConfig,
        event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    ) -> Result<Arc<Self>, ProviderError> {
        let child = JsonRpcChild::spawn(SpawnConfig {
            program: spawn.binary,
            args: vec!["acp".into()],
            env: env.unwrap_or_default(),
            cwd: Some(cwd.clone()),
            default_timeout: RPC_TIMEOUT,
        })
        .await
        .map_err(map_spawn_error)?;
        let child = Arc::new(child);
        let notification_rx = child.notifications();
        let request_rx = child
            .incoming_requests()
            .ok_or_else(|| ProviderError::RpcError {
                message: "Cursor ACP request receiver was already claimed".into(),
            })?;

        let setup = async {
            child
                .request("initialize", initialize_params("codemux"))
                .await
                .map_err(map_rpc_error)?;
            child
                .request("authenticate", json!({ "methodId": "cursor_login" }))
                .await
                .map_err(map_auth_error)?;

            let resume_id = resume_cursor.as_ref().and_then(resume_session_id);
            if let Some(session_id) = resume_id {
                match child
                    .request(
                        "session/load",
                        json!({ "sessionId": session_id, "cwd": cwd, "mcpServers": [] }),
                    )
                    .await
                {
                    // ACP's LoadSessionResponse deliberately omits sessionId:
                    // the client already supplied it in the request. Keep the
                    // requested id instead of treating a standards-compliant
                    // response as a failed restart.
                    Ok(response) => Ok(loaded_session_setup(response, session_id)),
                    Err(load_error) => {
                        let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                            thread_id: Some(thread_id.clone()),
                            message: format!(
                                "Cursor could not resume the prior ACP session; started a new session instead: {load_error}"
                            ),
                            original_payload: resume_cursor.clone(),
                        });
                        let response = child
                            .request(
                                "session/new",
                                json!({ "cwd": cwd, "mcpServers": [] }),
                            )
                            .await
                            .map_err(map_rpc_error)?;
                        new_session_setup(response)
                    }
                }
            } else {
                let response = child
                    .request("session/new", json!({ "cwd": cwd, "mcpServers": [] }))
                    .await
                    .map_err(map_rpc_error)?;
                new_session_setup(response)
            }
        }
        .await;

        let (setup, provider_session_id) = match setup {
            Ok(value) => value,
            Err(error) => {
                let _ = child.shutdown().await;
                return Err(error);
            }
        };
        let (notification_barrier_tx, notification_barrier_rx) = mpsc::unbounded_channel();
        let (request_barrier_tx, request_barrier_rx) = mpsc::unbounded_channel();
        let session = Arc::new(Self {
            thread_id: thread_id.clone(),
            provider_session_id: ProviderSessionId(provider_session_id),
            state: Mutex::new(CursorSessionState {
                status: SessionStatus::Ready,
                active_turn: None,
                model: None,
                permission_mode: permission_mode.clone().unwrap_or_else(|| "agent".into()),
                config_options: config_options(&setup),
                pending: HashMap::new(),
                queued: VecDeque::new(),
                assistant_text: String::new(),
                thinking_text: String::new(),
                prompt_in_flight: false,
                pending_interrupt: false,
                usage_totals: CursorUsageTotals::default(),
            }),
            child,
            event_tx,
            tasks: Mutex::new(Vec::new()),
            notification_barrier_tx,
            request_barrier_tx,
            stopped: AtomicBool::new(false),
        });
        session
            .start_background_tasks(
                notification_rx,
                request_rx,
                notification_barrier_rx,
                request_barrier_rx,
            )
            .await;

        let configuration_result = async {
            if let Some(model) = model {
                session.set_model(model).await?;
            }
            if let Some(context_window) = context_window {
                session
                    .set_semantic_config(ConfigKind::Context, &context_window, false)
                    .await?;
            }
            if let Some(effort) = effort {
                session.set_effort_config(&effort).await?;
            }
            // `false` is an explicit user choice, not "leave Cursor's default
            // alone". Some promoted models currently default to Fast, so only
            // sending `true` makes the UI's Standard state dishonest.
            session
                .set_boolean_config(ConfigKind::Fast, fast_mode)
                .await?;
            if let Some(mode) = permission_mode {
                session.set_permission_mode(mode).await?;
            }
            Ok::<(), ProviderError>(())
        }
        .await;
        if let Err(error) = configuration_result {
            session.shutdown().await;
            return Err(error);
        }

        let _ = session
            .event_tx
            .send(ProviderRuntimeEvent::SessionConfigured {
                thread_id: thread_id.clone(),
                provider_session_id: session.provider_session_id.clone(),
            });
        let _ = session
            .event_tx
            .send(ProviderRuntimeEvent::ResumeCursorUpdated {
                thread_id,
                resume_cursor: session.resume_cursor(),
            });
        Ok(session)
    }

    fn resume_cursor(&self) -> Value {
        json!({ "schemaVersion": 1, "sessionId": self.provider_session_id.0 })
    }

    pub fn is_dead(&self) -> bool {
        self.stopped.load(Ordering::SeqCst) || !self.child.is_alive()
    }

    async fn start_background_tasks(
        self: &Arc<Self>,
        mut notification_rx: broadcast::Receiver<Notification>,
        mut request_rx: tokio::sync::mpsc::Receiver<IncomingRequest>,
        mut notification_barrier_rx: mpsc::UnboundedReceiver<NotificationBarrier>,
        mut request_barrier_rx: mpsc::UnboundedReceiver<NotificationBarrier>,
    ) {
        let notifications = Arc::clone(self);
        let notification_task = tokio::spawn(async move {
            loop {
                // `biased` is the whole barrier contract: the message arm is
                // polled first, so a barrier is only ever answered once the
                // channel is empty. An ack therefore means "every message
                // the child wrote so far has been applied to session state".
                tokio::select! {
                    biased;
                    received = notification_rx.recv() => match received {
                        Ok(notification) => notifications.handle_notification(notification).await,
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            notifications.warn(
                                format!("Cursor ACP notification stream dropped {count} messages"),
                                None,
                            );
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    barrier = notification_barrier_rx.recv() => match barrier {
                        Some(ack) => {
                            let _ = ack.send(());
                        }
                        None => break,
                    },
                }
            }
        });
        let requests = Arc::clone(self);
        let request_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    received = request_rx.recv() => match received {
                        Some(request) => requests.handle_request(request).await,
                        None => break,
                    },
                    barrier = request_barrier_rx.recv() => match barrier {
                        Some(ack) => {
                            let _ = ack.send(());
                        }
                        None => break,
                    },
                }
            }
        });
        self.tasks
            .lock()
            .await
            .extend([notification_task, request_task]);
    }

    pub async fn enqueue_or_send(
        self: &Arc<Self>,
        input: SendTurnInput,
    ) -> Result<SendOutcome, ProviderError> {
        let mut state = self.state.lock().await;
        if self.is_dead() {
            return Err(ProviderError::SessionClosed {
                thread_id: self.thread_id.clone(),
            });
        }
        if state.active_turn.is_some() {
            let queued_id = format!("cursor-queued-{}", Uuid::new_v4());
            state.queued.push_back(QueuedTurn {
                id: queued_id.clone(),
                input: input.clone(),
            });
            drop(state);
            let _ = self.event_tx.send(ProviderRuntimeEvent::TurnQueued {
                thread_id: self.thread_id.clone(),
                queued_id: queued_id.clone(),
                client_nonce: input.client_nonce,
                text: input.display_text.unwrap_or(input.text),
            });
            return Ok(SendOutcome::Queued(queued_id));
        }
        // Claim the only ACP prompt slot before configuration/checkpoint
        // work begins. Two frontend sends can otherwise both observe an
        // idle session and start concurrent `session/prompt` requests.
        state.active_turn = Some(TurnId(DISPATCHING_TURN_ID.into()));
        drop(state);
        match self.dispatch_turn(input, None).await {
            Ok(turn_id) => Ok(SendOutcome::Started(turn_id)),
            Err(error) => {
                self.release_dispatch_reservation().await;
                Err(error)
            }
        }
    }

    async fn dispatch_turn(
        self: &Arc<Self>,
        input: SendTurnInput,
        queued_id: Option<String>,
    ) -> Result<TurnId, ProviderError> {
        let (turn_id, prompt) = self.prepare_turn(input, queued_id).await?;
        let session = Arc::clone(self);
        let worker_turn_id = turn_id.clone();
        let task = tokio::spawn(async move {
            session.run_prompt_worker(worker_turn_id, prompt).await;
        });
        {
            // Reap finished workers on the way in: a long session would
            // otherwise accumulate one dead JoinHandle per turn forever.
            let mut tasks = self.tasks.lock().await;
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
        }
        Ok(turn_id)
    }

    /// Block until both message pumps have applied everything the child
    /// wrote before now.
    ///
    /// `session/prompt`'s response is delivered through the transport's
    /// oneshot, while the `agent_message_chunk` / `agent_thought_chunk`
    /// notifications and `session/request_permission` requests that precede
    /// it on stdout are delivered through a broadcast and an mpsc channel
    /// this session drains in two other tasks. All three wake at once, so
    /// completing the turn on the response alone races the tail of the
    /// child's output — chunks get dropped (no active turn) or folded into
    /// the next turn's buffer, and a just-arrived permission request lands
    /// in `pending` after the turn already cancelled everything there.
    ///
    /// The transport's reader routes lines in order, so by the time the
    /// response resolves every earlier message is already queued. Both
    /// pumps answer a barrier only when their channel is empty, so an ack
    /// from each restores the ordering the transcript depends on.
    async fn await_child_messages_drained(&self) {
        let barriers = [&self.request_barrier_tx, &self.notification_barrier_tx]
            .into_iter()
            .filter_map(|barrier_tx| {
                let (ack_tx, ack_rx) = oneshot::channel();
                barrier_tx.send(ack_tx).ok().map(|()| ack_rx)
            })
            .collect::<Vec<_>>();
        for ack_rx in barriers {
            // Bounded so a wedged pump can never strand a turn in Running
            // forever; a dropped ack (task exited) resolves instantly.
            let _ = tokio::time::timeout(RPC_TIMEOUT, ack_rx).await;
        }
    }

    async fn prepare_turn(
        &self,
        input: SendTurnInput,
        queued_id: Option<String>,
    ) -> Result<(TurnId, Vec<Value>), ProviderError> {
        if let Some(checkpoint) = &input.turn_checkpoint {
            checkpoint.prepare().await;
        }
        let configured = async {
            if let Some(model) = input.model_override.clone() {
                self.set_model(model).await?;
            }
            if let Some(effort) = input.effort_override.as_deref() {
                self.set_effort_config(effort).await?;
            }
            if let Some(mode) = input.permission_mode_override.clone() {
                self.set_permission_mode(mode).await?;
            }
            Ok::<(), ProviderError>(())
        }
        .await;
        if let Err(error) = configured {
            if let Some(checkpoint) = &input.turn_checkpoint {
                checkpoint.abort().await;
            }
            return Err(error);
        }

        let turn_id = TurnId(format!("cursor-turn-{}", Uuid::new_v4()));
        {
            let mut state = self.state.lock().await;
            state.active_turn = Some(turn_id.clone());
            state.status = SessionStatus::Running {
                active_turn: turn_id.clone(),
            };
            state.assistant_text.clear();
            state.thinking_text.clear();
        }
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::Running {
                    active_turn: turn_id.clone(),
                },
            });
        if let Some(id) = queued_id {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnDispatched {
                    thread_id: self.thread_id.clone(),
                    queued_id: id,
                    turn_id: turn_id.clone(),
                    text: input
                        .display_text
                        .clone()
                        .unwrap_or_else(|| input.text.clone()),
                });
        }
        if let Some(checkpoint) = &input.turn_checkpoint {
            checkpoint.commit().await;
        }

        Ok((turn_id, build_prompt(&input)))
    }

    async fn run_prompt_worker(self: Arc<Self>, mut turn_id: TurnId, mut prompt: Vec<Value>) {
        loop {
            // Claim the in-flight window under the same lock `interrupt`
            // reads, so a Stop pressed anywhere between dispatch and this
            // point aborts the turn instead of being swallowed by a child
            // that has not been asked to do anything yet.
            let interrupted_before_send = {
                let mut state = self.state.lock().await;
                let interrupted = std::mem::take(&mut state.pending_interrupt);
                state.prompt_in_flight = !interrupted;
                interrupted
            };
            let response = if interrupted_before_send {
                Ok(json!({ "stopReason": "cancelled" }))
            } else {
                self.child
                    .request_with_timeout(
                        "session/prompt",
                        json!({
                            "sessionId": self.provider_session_id.0,
                            "prompt": prompt
                        }),
                        PROMPT_TIMEOUT,
                    )
                    .await
            };
            let next = self.finish_turn(turn_id, response).await;
            let Some(queued) = next else {
                return;
            };
            match self
                .prepare_turn(queued.input, Some(queued.id.clone()))
                .await
            {
                Ok((next_turn_id, next_prompt)) => {
                    turn_id = next_turn_id;
                    prompt = next_prompt;
                }
                Err(error) => {
                    let cancelled = {
                        let mut state = self.state.lock().await;
                        state.active_turn = None;
                        state.status = SessionStatus::Ready;
                        state
                            .queued
                            .drain(..)
                            .map(|queued| queued.id)
                            .collect::<Vec<_>>()
                    };
                    self.warn(
                        format!("Could not dispatch queued Cursor turn: {error}"),
                        None,
                    );
                    let _ = self
                        .event_tx
                        .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                            thread_id: self.thread_id.clone(),
                            queued_id: queued.id,
                        });
                    for queued_id in cancelled {
                        let _ = self
                            .event_tx
                            .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                                thread_id: self.thread_id.clone(),
                                queued_id,
                            });
                    }
                    return;
                }
            }
        }
    }

    async fn finish_turn(
        &self,
        turn_id: TurnId,
        response: Result<Value, RpcChildError>,
    ) -> Option<QueuedTurn> {
        // Ordering barrier — see `await_child_messages_drained`. Must run
        // before the buffers below are taken, and without holding the
        // state lock the notification task needs.
        self.await_child_messages_drained().await;
        let (assistant_text, thinking_text, cancelled_requests) = {
            let mut state = self.state.lock().await;
            state.prompt_in_flight = false;
            state.pending_interrupt = false;
            (
                std::mem::take(&mut state.assistant_text),
                std::mem::take(&mut state.thinking_text),
                // ACP requires every outstanding request to be answered.
                // Dropping them would leave the child blocked forever and
                // the UI's approval surface unresolvable (a later Allow
                // would fail with RequestNotPending).
                state.pending.drain().collect::<Vec<_>>(),
            )
        };
        for (request_id, pending) in cancelled_requests {
            let response = pending_response(&pending, &ApprovalDecision::Cancel);
            let _ = self
                .child
                .respond(pending_rpc_id(pending), Ok(response))
                .await;
            let _ = self.event_tx.send(ProviderRuntimeEvent::RequestResolved {
                thread_id: self.thread_id.clone(),
                request_id,
                decision: ApprovalDecision::Cancel,
            });
        }
        if !thinking_text.is_empty() {
            let _ = self.event_tx.send(ProviderRuntimeEvent::ItemCompleted {
                thread_id: self.thread_id.clone(),
                turn_id: turn_id.clone(),
                item: CompletedItem::AssistantThinking {
                    text: thinking_text,
                },
                subagent_id: None,
            });
        }
        if !assistant_text.is_empty() {
            let _ = self.event_tx.send(ProviderRuntimeEvent::ItemCompleted {
                thread_id: self.thread_id.clone(),
                turn_id: turn_id.clone(),
                item: CompletedItem::AssistantText {
                    text: assistant_text,
                },
                subagent_id: None,
            });
        }
        let status = match response {
            Ok(result) => turn_status_from_stop_reason(
                result.get("stopReason").and_then(Value::as_str),
            ),
            Err(error) => TurnStatus::Error {
                subtype: "cursor_acp".into(),
                message: error.to_string(),
            },
        };
        let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
            thread_id: self.thread_id.clone(),
            turn_id: turn_id.clone(),
            status,
            usage: None,
        });
        let next = {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Ready;
            let next = state.queued.pop_front();
            // Reserve the dispatch slot before releasing the lock. Without
            // this marker a concurrent send could observe an idle session
            // between popping the queued turn and preparing it, starting a
            // second prompt worker for the same ACP session.
            state.active_turn = next.as_ref().map(|_| TurnId(DISPATCHING_TURN_ID.into()));
            next
        };
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::Ready,
            });
        next
    }

    pub async fn interrupt(&self, turn_id: Option<TurnId>) -> Result<(), ProviderError> {
        {
            let mut state = self.state.lock().await;
            let active = state.active_turn.clone();
            if active.is_none() || turn_id.is_some_and(|requested| Some(requested) != active) {
                return Ok(());
            }
            if !state.prompt_in_flight {
                // The turn is claimed but `session/prompt` has not been
                // written yet — it is still going through the dispatch
                // config round-trips, or the worker task has not been
                // scheduled. A `session/cancel` now would hit a child with
                // nothing to cancel and Stop would be silently lost, so
                // record it and let the worker abort the turn instead.
                state.pending_interrupt = true;
                return Ok(());
            }
        }
        self.child
            .notify(
                "session/cancel",
                json!({ "sessionId": self.provider_session_id.0 }),
            )
            .await
            .map_err(map_rpc_error)
    }

    pub async fn cancel_queued(&self, queued_id: &str) -> bool {
        let removed = {
            let mut state = self.state.lock().await;
            state
                .queued
                .iter()
                .position(|queued| queued.id == queued_id)
                .and_then(|index| state.queued.remove(index))
        };
        if let Some(removed) = removed {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread_id.clone(),
                    queued_id: removed.id,
                });
            true
        } else {
            false
        }
    }

    pub async fn send_queued_now(self: &Arc<Self>, queued_id: &str) -> Result<(), ProviderError> {
        let (ready_to_dispatch, should_interrupt) = {
            let mut state = self.state.lock().await;
            let queued = state
                .queued
                .iter()
                .position(|queued| queued.id == queued_id)
                .and_then(|index| state.queued.remove(index));
            match queued {
                Some(queued) if state.active_turn.is_some() => {
                    state.queued.push_front(queued);
                    (None, true)
                }
                Some(queued) => {
                    state.active_turn = Some(TurnId(DISPATCHING_TURN_ID.into()));
                    (Some(queued), false)
                }
                None => (None, false),
            }
        };
        if let Some(queued) = ready_to_dispatch {
            let queued_id = queued.id.clone();
            if let Err(error) = self.dispatch_turn(queued.input, Some(queued.id)).await {
                self.release_dispatch_reservation().await;
                let _ = self
                    .event_tx
                    .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                        thread_id: self.thread_id.clone(),
                        queued_id,
                    });
                return Err(error);
            }
        } else if should_interrupt {
            // The promoted item is already first in line before cancellation,
            // so prompt completion cannot race past it.
            self.interrupt(None).await?;
        }
        Ok(())
    }

    async fn release_dispatch_reservation(&self) {
        let mut state = self.state.lock().await;
        if state.active_turn.as_ref().map(|turn| turn.0.as_str()) == Some(DISPATCHING_TURN_ID) {
            state.active_turn = None;
            state.status = SessionStatus::Ready;
            state.pending_interrupt = false;
        }
    }

    pub async fn respond_to_request(
        &self,
        request_id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        let pending = self
            .state
            .lock()
            .await
            .pending
            .remove(&request_id)
            .ok_or_else(|| ProviderError::RequestNotPending {
                request_id: request_id.clone(),
            })?;
        let response = pending_response(&pending, &decision);
        self.child
            .respond(pending_rpc_id(pending), Ok(response))
            .await
            .map_err(map_rpc_error)?;
        let _ = self.event_tx.send(ProviderRuntimeEvent::RequestResolved {
            thread_id: self.thread_id.clone(),
            request_id,
            decision,
        });
        let status = {
            let mut state = self.state.lock().await;
            // Answering one of several parallel requests must not flip the
            // pane out of WaitingApproval: the others are still blocking
            // the child. Only an empty pending map means the turn resumed.
            let keep_current = matches!(
                &state.status,
                SessionStatus::WaitingApproval { request_id } if state.pending.contains_key(request_id)
            );
            let status = if keep_current {
                state.status.clone()
            } else if let Some(request_id) = state.pending.keys().next().cloned() {
                SessionStatus::WaitingApproval { request_id }
            } else {
                state.active_turn.clone().map_or(SessionStatus::Ready, |active_turn| {
                    SessionStatus::Running { active_turn }
                })
            };
            state.status = status.clone();
            status
        };
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status,
            });
        Ok(())
    }

    pub async fn set_model(&self, model: String) -> Result<(), ProviderError> {
        let model = base_model_id(&model);
        // The frontend uses `default` only while live capability discovery
        // has not produced a concrete row. Leaving ACP untouched is the
        // provider-native default and avoids inventing a Cursor model slug.
        if model == "default" {
            self.state.lock().await.model = None;
            return Ok(());
        }
        self.set_semantic_config(ConfigKind::Model, model, true)
            .await?;
        self.state.lock().await.model = Some(model.to_string());
        Ok(())
    }

    pub async fn set_permission_mode(&self, mode: String) -> Result<(), ProviderError> {
        if !matches!(mode.as_str(), "agent" | "ask" | "plan") {
            return Err(ProviderError::ValidationError {
                message: format!("Cursor does not expose permission mode `{mode}`"),
            });
        }
        // `mode` is a stable ACP config option. Cursor's current values are
        // Agent/Ask; resolve through the advertised options when available.
        self.set_semantic_config(ConfigKind::Mode, &mode, true)
            .await?;
        self.state.lock().await.permission_mode = mode;
        Ok(())
    }

    async fn set_semantic_config(
        &self,
        kind: ConfigKind,
        requested: &str,
        allow_fallback_id: bool,
    ) -> Result<(), ProviderError> {
        let (id, value) = {
            let state = self.state.lock().await;
            let id = config_id_for(&state.config_options, kind).or_else(|| {
                allow_fallback_id.then(|| match kind {
                    ConfigKind::Model => "model".into(),
                    ConfigKind::Mode => "mode".into(),
                    _ => unreachable!(),
                })
            });
            let Some(id) = id else {
                return Ok(());
            };
            let value = option_by_id(&state.config_options, &id)
                .and_then(|option| match kind {
                    ConfigKind::Effort => resolve_effort_value(option, requested),
                    _ => resolve_select_value(option, requested),
                })
                .unwrap_or_else(|| requested.to_string());
            (id, Value::String(value))
        };
        self.set_config(&id, value).await
    }

    async fn set_effort_config(&self, requested: &str) -> Result<(), ProviderError> {
        let has_thinking_toggle = {
            let state = self.state.lock().await;
            config_id_for(&state.config_options, ConfigKind::Thinking).is_some()
        };
        if has_thinking_toggle {
            if requested.eq_ignore_ascii_case("none") {
                return self.set_boolean_config(ConfigKind::Thinking, false).await;
            }
            self.set_boolean_config(ConfigKind::Thinking, true).await?;
        }
        self.set_semantic_config(ConfigKind::Effort, requested, false)
            .await
    }

    async fn set_boolean_config(
        &self,
        kind: ConfigKind,
        requested: bool,
    ) -> Result<(), ProviderError> {
        let resolved = {
            let state = self.state.lock().await;
            let Some(id) = config_id_for(&state.config_options, kind) else {
                return Ok(());
            };
            let value = option_by_id(&state.config_options, &id)
                .and_then(|option| resolve_boolean_value(option, requested))
                .unwrap_or(Value::Bool(requested));
            (id, value)
        };
        self.set_config(&resolved.0, resolved.1).await
    }

    async fn set_config(&self, id: &str, value: Value) -> Result<(), ProviderError> {
        let response = self
            .child
            .request(
                "session/set_config_option",
                set_config_params(&self.provider_session_id.0, id, value),
            )
            .await
            .map_err(map_rpc_error)?;
        let updated = config_options(&response);
        if !updated.is_empty() {
            self.state.lock().await.config_options = updated;
        }
        Ok(())
    }

    async fn handle_request(&self, request: IncomingRequest) {
        match request.method.as_str() {
            "session/request_permission" => self.handle_permission_request(request).await,
            "cursor/create_plan" => self.handle_plan_request(request).await,
            "cursor/ask_question" => {
                self.handle_question_request(request).await;
            }
            _ => {
                self.warn(
                    format!("Unsupported Cursor ACP request `{}`", request.method),
                    Some(request.params),
                );
                let _ = self
                    .child
                    .respond(
                        request.id,
                        Err(RpcError {
                            code: -32601,
                            message: "Method not supported by Codemux".into(),
                            data: None,
                        }),
                    )
                    .await;
            }
        }
    }

    async fn handle_permission_request(&self, request: IncomingRequest) {
        let options = request
            .params
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let full_access = self.state.lock().await.permission_mode == "agent";
        if full_access {
            let outcome = select_permission_option(&options, &["allow_always", "allow_once"])
                .map(|option_id| json!({ "outcome": "selected", "optionId": option_id }))
                .unwrap_or_else(|| json!({ "outcome": "cancelled" }));
            let _ = self
                .child
                .respond(request.id, Ok(json!({ "outcome": outcome })))
                .await;
            return;
        }

        let request_id = RequestId(format!("cursor-request-{}", Uuid::new_v4()));
        let turn_id = self
            .state
            .lock()
            .await
            .active_turn
            .clone()
            .unwrap_or_else(|| TurnId("cursor-turn-unknown".into()));
        {
            let mut state = self.state.lock().await;
            state.pending.insert(
                request_id.clone(),
                PendingRequest::Permission {
                    rpc_id: request.id,
                    options,
                },
            );
            state.status = SessionStatus::WaitingApproval {
                request_id: request_id.clone(),
            };
        }
        let tool = request
            .params
            .get("toolCall")
            .cloned()
            .unwrap_or(Value::Null);
        let tool_use_id = tool
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let _ = self.event_tx.send(ProviderRuntimeEvent::RequestOpened {
            thread_id: self.thread_id.clone(),
            turn_id,
            request_id: request_id.clone(),
            request_kind: "tool_approval".into(),
            payload: request.params,
            tool_use_id,
            subagent_id: None,
        });
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::WaitingApproval { request_id },
            });
    }

    async fn handle_plan_request(&self, request: IncomingRequest) {
        if let Some(tasks) = tasks_from_value(&request.params) {
            let _ = self.event_tx.send(ProviderRuntimeEvent::TasksUpdated {
                thread_id: self.thread_id.clone(),
                tasks,
            });
        }
        let request_id = RequestId(format!("cursor-plan-{}", Uuid::new_v4()));
        let turn_id = self
            .state
            .lock()
            .await
            .active_turn
            .clone()
            .unwrap_or_else(|| TurnId("cursor-turn-unknown".into()));
        {
            let mut state = self.state.lock().await;
            state.pending.insert(
                request_id.clone(),
                PendingRequest::Plan { rpc_id: request.id },
            );
            state.status = SessionStatus::WaitingApproval {
                request_id: request_id.clone(),
            };
        }
        let tool_use_id = request
            .params
            .get("toolCallId")
            .or_else(|| request.params.pointer("/toolCall/toolCallId"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let _ = self.event_tx.send(ProviderRuntimeEvent::RequestOpened {
            thread_id: self.thread_id.clone(),
            turn_id,
            request_id: request_id.clone(),
            request_kind: "plan".into(),
            payload: request.params,
            tool_use_id,
            subagent_id: None,
        });
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::WaitingApproval { request_id },
            });
    }

    async fn handle_question_request(&self, request: IncomingRequest) {
        let (questions, payload) = cursor_questions(&request.params);
        if questions.is_empty() {
            self.warn(
                "Cursor sent an empty or invalid structured question; it was skipped.".into(),
                Some(request.params),
            );
            let _ = self
                .child
                .respond(
                    request.id,
                    Ok(json!({
                        "outcome": {
                            "outcome": "skipped",
                            "reason": "No valid questions"
                        }
                    })),
                )
                .await;
            return;
        }

        let request_id = RequestId(format!("cursor-question-{}", Uuid::new_v4()));
        let turn_id = self
            .state
            .lock()
            .await
            .active_turn
            .clone()
            .unwrap_or_else(|| TurnId("cursor-turn-unknown".into()));
        {
            let mut state = self.state.lock().await;
            state.pending.insert(
                request_id.clone(),
                PendingRequest::UserInput {
                    rpc_id: request.id,
                    questions,
                },
            );
            state.status = SessionStatus::WaitingApproval {
                request_id: request_id.clone(),
            };
        }
        let tool_use_id = request
            .params
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let _ = self.event_tx.send(ProviderRuntimeEvent::RequestOpened {
            thread_id: self.thread_id.clone(),
            turn_id,
            request_id: request_id.clone(),
            request_kind: "user-input".into(),
            payload,
            tool_use_id,
            subagent_id: None,
        });
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::WaitingApproval { request_id },
            });
    }

    async fn handle_notification(&self, notification: Notification) {
        match notification.method.as_str() {
            "session/update" => self.handle_session_update(notification.params).await,
            "cursor/update_todos" => {
                if let Some(tasks) = tasks_from_value(&notification.params) {
                    let _ = self.event_tx.send(ProviderRuntimeEvent::TasksUpdated {
                        thread_id: self.thread_id.clone(),
                        tasks,
                    });
                }
            }
            // Cursor currently uses these for richer UI. They do not affect
            // transcript correctness; keep them observable until Codemux has
            // dedicated surfaces.
            "cursor/task" | "cursor/generate_image" => self.warn(
                format!(
                    "Cursor ACP extension notification `{}` is not rendered yet",
                    notification.method
                ),
                Some(notification.params),
            ),
            _ => self.warn(
                format!("Unknown Cursor ACP notification `{}`", notification.method),
                Some(notification.params),
            ),
        }
    }

    async fn handle_session_update(&self, params: Value) {
        let update = params.get("update").unwrap_or(&params);
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(turn_id) = self.state.lock().await.active_turn.clone() else {
            // Session-load replay is already persisted in Codemux. Avoid
            // duplicating it, but don't call a valid replay frame unknown.
            return;
        };
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => {
                if let Some(text) = update.pointer("/content/text").and_then(Value::as_str) {
                    let delta = if kind == "agent_thought_chunk" {
                        self.state.lock().await.thinking_text.push_str(text);
                        ContentDelta::Thinking { text: text.into() }
                    } else {
                        self.state.lock().await.assistant_text.push_str(text);
                        ContentDelta::Text { text: text.into() }
                    };
                    let _ = self.event_tx.send(ProviderRuntimeEvent::ContentDelta {
                        thread_id: self.thread_id.clone(),
                        turn_id,
                        delta,
                        subagent_id: None,
                    });
                }
            }
            "tool_call" => {
                let id = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("cursor-tool");
                let name = update
                    .get("kind")
                    .or_else(|| update.get("title"))
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let input = update
                    .get("rawInput")
                    .cloned()
                    .unwrap_or_else(|| update.clone());
                let _ = self.event_tx.send(ProviderRuntimeEvent::ItemCompleted {
                    thread_id: self.thread_id.clone(),
                    turn_id,
                    item: CompletedItem::ToolUse {
                        tool_name: name.into(),
                        input,
                        tool_use_id: id.into(),
                    },
                    subagent_id: None,
                });
            }
            "tool_call_update" => {
                let terminal = update
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| matches!(status, "completed" | "failed"));
                if terminal {
                    let id = update
                        .get("toolCallId")
                        .and_then(Value::as_str)
                        .unwrap_or("cursor-tool");
                    let is_error = update.get("status").and_then(Value::as_str) == Some("failed");
                    let content = update
                        .get("rawOutput")
                        .or_else(|| update.get("content"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let _ = self.event_tx.send(ProviderRuntimeEvent::ItemCompleted {
                        thread_id: self.thread_id.clone(),
                        turn_id,
                        item: CompletedItem::ToolResult {
                            tool_use_id: id.into(),
                            content,
                            is_error,
                        },
                        subagent_id: None,
                    });
                }
            }
            "plan" => {
                if let Some(tasks) = tasks_from_value(update) {
                    let _ = self.event_tx.send(ProviderRuntimeEvent::TasksUpdated {
                        thread_id: self.thread_id.clone(),
                        tasks,
                    });
                }
            }
            "usage_update" => self.handle_usage_update(update).await,
            "config_option_update" | "current_mode_update" | "available_commands_update" => {}
            _ => self.warn(
                format!("Unknown Cursor ACP session update `{kind}`"),
                Some(update.clone()),
            ),
        }
    }

    /// Feed Cursor's `usage_update` into the shared usage surfaces.
    ///
    /// The extension's payload shape is not documented, so every field is
    /// resolved through a tolerant alias list and a frame that carries no
    /// recognizable token counts is skipped rather than guessed at. Like
    /// Codex, the report is treated as a running total for the session, so
    /// the ledger receives the delta since the last observation.
    async fn handle_usage_update(&self, update: &Value) {
        let Some(totals) = usage_totals_from_value(update) else {
            return;
        };
        let (delta, model) = {
            let mut state = self.state.lock().await;
            let previous = state.usage_totals;
            state.usage_totals = totals;
            (totals.delta_since(&previous), state.model.clone())
        };
        if !delta.is_empty() {
            let cost_usd = pricing::cost_for(
                model.as_deref(),
                delta.input,
                delta.output,
                delta.cache_read,
                delta.cache_write,
            );
            let _ = self.event_tx.send(ProviderRuntimeEvent::UsageRecorded {
                thread_id: self.thread_id.clone(),
                provider: ProviderKind::Cursor,
                model,
                subagent: false,
                input_tokens: delta.input,
                output_tokens: delta.output,
                cache_read_tokens: delta.cache_read,
                cache_write_tokens: delta.cache_write,
                // Cursor reports no reasoning split.
                reasoning_tokens: 0,
                cost_usd,
                // No rate catalogue — a priced Cursor row is always the
                // static table's estimate.
                cost_source: cost_usd.map(|_| CostSource::Table),
            });
        }
        let max_tokens = context_window_from_value(update);
        let used_tokens = match max_tokens {
            Some(max) => totals.context_tokens().min(max),
            None => totals.context_tokens(),
        };
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::ContextUsageUpdated {
                thread_id: self.thread_id.clone(),
                usage: ContextUsageSnapshot {
                    used_tokens,
                    total_processed_tokens: None,
                    max_tokens,
                    last_used_tokens: None,
                    // Cursor manages its own context window and exposes no
                    // Codemux-side toggle for it.
                    compacts_automatically: Some(true),
                },
            });
    }

    fn warn(&self, message: String, original_payload: Option<Value>) {
        let _ = self.event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(self.thread_id.clone()),
            message,
            original_payload,
        });
    }

    pub async fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let queued = {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Closed;
            state
                .queued
                .drain(..)
                .map(|queued| queued.id)
                .collect::<Vec<_>>()
        };
        for queued_id in queued {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread_id.clone(),
                    queued_id,
                });
        }
        let _ = self
            .child
            .notify(
                "session/cancel",
                json!({ "sessionId": self.provider_session_id.0 }),
            )
            .await;
        let _ = self.child.shutdown().await;
        for task in self.tasks.lock().await.drain(..) {
            task.abort();
        }
    }
}

impl CursorUsageTotals {
    /// Tokens occupying the live window: fresh input plus everything the
    /// provider read back from cache, plus what it produced.
    fn context_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    /// Field-wise increase since `previous`. A total that moved backwards
    /// (session reset) contributes nothing rather than a negative row.
    fn delta_since(&self, previous: &Self) -> Self {
        Self {
            input: self.input.saturating_sub(previous.input),
            output: self.output.saturating_sub(previous.output),
            cache_read: self.cache_read.saturating_sub(previous.cache_read),
            cache_write: self.cache_write.saturating_sub(previous.cache_write),
        }
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// First `u64` found under any of `keys`, at the top level or nested one
/// level under a `usage`-ish container.
fn usage_number(value: &Value, keys: &[&str]) -> Option<u64> {
    let containers = [
        Some(value),
        value.get("usage"),
        value.get("tokenUsage"),
        value.get("token_usage"),
    ];
    containers.into_iter().flatten().find_map(|container| {
        keys.iter()
            .find_map(|key| container.get(*key).and_then(Value::as_u64))
    })
}

/// Parse a `usage_update` payload into cumulative token totals, or `None`
/// when the frame carries nothing recognizable. Deliberately tolerant:
/// the extension is undocumented and a shape change must degrade to "no
/// usage reported", never to invented numbers.
fn usage_totals_from_value(value: &Value) -> Option<CursorUsageTotals> {
    let input = usage_number(value, &["inputTokens", "input_tokens", "promptTokens"]);
    let output = usage_number(
        value,
        &["outputTokens", "output_tokens", "completionTokens"],
    );
    let cache_read = usage_number(
        value,
        &[
            "cacheReadTokens",
            "cache_read_tokens",
            "cachedInputTokens",
            "cached_input_tokens",
        ],
    );
    let cache_write = usage_number(
        value,
        &[
            "cacheWriteTokens",
            "cache_write_tokens",
            "cacheCreationInputTokens",
        ],
    );
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
        return None;
    }
    Some(CursorUsageTotals {
        input: input.unwrap_or(0),
        output: output.unwrap_or(0),
        cache_read: cache_read.unwrap_or(0),
        cache_write: cache_write.unwrap_or(0),
    })
}

/// The model's context-window size, when the frame states one. Never
/// guessed — the meter renders a bare token count without a denominator.
fn context_window_from_value(value: &Value) -> Option<u64> {
    usage_number(
        value,
        &[
            "contextWindow",
            "context_window",
            "maxTokens",
            "max_tokens",
            "modelContextWindow",
        ],
    )
    .filter(|window| *window > 0)
}

/// Map an ACP `stopReason` onto the closest shared [`TurnStatus`].
fn turn_status_from_stop_reason(stop_reason: Option<&str>) -> TurnStatus {
    match stop_reason {
        Some("cancelled") => TurnStatus::Error {
            subtype: "interrupted".into(),
            message: "Cursor turn was interrupted".into(),
        },
        // Both caps are "the runtime stopped this turn short", which is
        // exactly what MaxTurns renders.
        Some("max_tokens" | "max_turn_requests") => TurnStatus::MaxTurns,
        Some("refusal") => TurnStatus::Error {
            subtype: "refusal".into(),
            message: "Cursor declined to continue this turn".into(),
        },
        _ => TurnStatus::Success,
    }
}

/// The JSON-RPC response body for `pending` under `decision`.
fn pending_response(pending: &PendingRequest, decision: &ApprovalDecision) -> Value {
    match pending {
        PendingRequest::Permission { options, .. } => {
            json!({ "outcome": permission_outcome(options, decision) })
        }
        PendingRequest::UserInput { questions, .. } => {
            cursor_question_response(questions, decision)
        }
        PendingRequest::Plan { .. } => cursor_plan_response(decision),
    }
}

fn pending_rpc_id(pending: PendingRequest) -> Value {
    match pending {
        PendingRequest::Permission { rpc_id, .. }
        | PendingRequest::UserInput { rpc_id, .. }
        | PendingRequest::Plan { rpc_id } => rpc_id,
    }
}

fn build_prompt(input: &SendTurnInput) -> Vec<Value> {
    let mut prompt = vec![json!({ "type": "text", "text": input.text })];
    prompt.extend(input.images.iter().map(|image| {
        json!({
            "type": "image",
            "data": base64::engine::general_purpose::STANDARD.encode(&image.data),
            "mimeType": image.media_type
        })
    }));
    prompt
}

fn resume_session_id(value: &Value) -> Option<String> {
    value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .or_else(|| value.get("resume"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn new_session_setup(response: Value) -> Result<(Value, String), ProviderError> {
    let provider_session_id = session_id(&response).ok_or_else(|| ProviderError::RpcError {
        message: "Cursor ACP session/new returned no sessionId".into(),
    })?;
    Ok((response, provider_session_id))
}

fn loaded_session_setup(response: Value, requested_session_id: String) -> (Value, String) {
    (response, requested_session_id)
}

fn base_model_id(model: &str) -> &str {
    model.split_once('[').map_or(model, |(base, _)| base).trim()
}

fn cursor_questions(params: &Value) -> (Vec<CursorQuestion>, Value) {
    let header = params
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Question");
    let Some(raw_questions) = params.get("questions").and_then(Value::as_array) else {
        return (vec![], json!({ "questions": [] }));
    };

    let mut questions = Vec::new();
    let mut ui_questions = Vec::new();
    for raw in raw_questions {
        let Some(id) = raw
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(prompt) = raw
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let options = raw
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                let id = option
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())?;
                let label = option
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|label| !label.is_empty())?;
                Some(CursorQuestionOption {
                    id: id.to_string(),
                    label: label.to_string(),
                })
            })
            .collect::<Vec<_>>();
        questions.push(CursorQuestion {
            id: id.to_string(),
            prompt: prompt.to_string(),
            options: options.clone(),
        });
        ui_questions.push(json!({
            "header": header,
            "question": prompt,
            "multiSelect": raw.get("allowMultiple").and_then(Value::as_bool).unwrap_or(false),
            // Cursor's extension is deliberately multiple-choice only. The
            // shared Claude questionnaire otherwise adds a free-text row that
            // cannot be represented by CursorAskQuestionResponse.
            "allowOther": false,
            "options": options.into_iter().map(|option| json!({
                "label": option.label,
                "description": ""
            })).collect::<Vec<_>>(),
        }));
    }
    (questions, json!({ "questions": ui_questions }))
}

fn cursor_question_response(questions: &[CursorQuestion], decision: &ApprovalDecision) -> Value {
    match decision {
        ApprovalDecision::Allow { .. } => json!({
            "outcome": {
                "outcome": "answered",
                "answers": cursor_question_answers(questions, decision)
            }
        }),
        ApprovalDecision::Deny { message } => json!({
            "outcome": {
                "outcome": "skipped",
                "reason": message
            }
        }),
        ApprovalDecision::AllowForSession | ApprovalDecision::Cancel => {
            json!({ "outcome": { "outcome": "cancelled" } })
        }
    }
}

fn cursor_question_answers(
    questions: &[CursorQuestion],
    decision: &ApprovalDecision,
) -> Vec<Value> {
    let ApprovalDecision::Allow {
        updated_input: Some(input),
        ..
    } = decision
    else {
        return vec![];
    };
    let Some(answers) = input.get("answers").and_then(Value::as_object) else {
        return vec![];
    };
    questions
        .iter()
        .filter_map(|question| {
            let answer = answers
                .get(&question.id)
                .or_else(|| answers.get(&question.prompt))
                .and_then(Value::as_str)?;
            let exact = question.options.iter().find(|option| {
                option.id.eq_ignore_ascii_case(answer) || option.label.eq_ignore_ascii_case(answer)
            });
            let selected_option_ids = if let Some(option) = exact {
                vec![option.id.clone()]
            } else {
                let requested = answer
                    .split(", ")
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>();
                question
                    .options
                    .iter()
                    .filter(|option| {
                        requested.iter().any(|requested| {
                            option.id.eq_ignore_ascii_case(requested)
                                || option.label.eq_ignore_ascii_case(requested)
                        })
                    })
                    .map(|option| option.id.clone())
                    .collect::<Vec<_>>()
            };
            Some(json!({
                "questionId": question.id,
                "selectedOptionIds": selected_option_ids
            }))
        })
        .collect()
}

fn cursor_plan_response(decision: &ApprovalDecision) -> Value {
    match decision {
        ApprovalDecision::Allow { .. } | ApprovalDecision::AllowForSession => {
            json!({ "outcome": { "outcome": "accepted" } })
        }
        ApprovalDecision::Deny { message } => json!({
            "outcome": {
                "outcome": "rejected",
                "reason": message
            }
        }),
        ApprovalDecision::Cancel => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

fn permission_outcome(options: &[Value], decision: &ApprovalDecision) -> Value {
    let kinds: &[&str] = match decision {
        ApprovalDecision::AllowForSession => &["allow_always", "allow_once"],
        ApprovalDecision::Allow { .. } => &["allow_once", "allow_always"],
        ApprovalDecision::Deny { .. } => &["reject_once", "reject_always"],
        ApprovalDecision::Cancel => return json!({ "outcome": "cancelled" }),
    };
    select_permission_option(options, kinds)
        .map(|option_id| json!({ "outcome": "selected", "optionId": option_id }))
        .unwrap_or_else(|| json!({ "outcome": "cancelled" }))
}

fn select_permission_option(options: &[Value], kinds: &[&str]) -> Option<String> {
    kinds.iter().find_map(|kind| {
        options.iter().find_map(|option| {
            (option.get("kind").and_then(Value::as_str) == Some(*kind))
                .then(|| {
                    option
                        .get("optionId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .flatten()
        })
    })
}

fn tasks_from_value(value: &Value) -> Option<TasksSnapshot> {
    let todos = value
        .get("todos")
        .or_else(|| value.get("entries"))?
        .as_array()?;
    let tasks = todos
        .iter()
        .enumerate()
        .filter_map(|(index, todo)| {
            let title = todo
                .get("content")
                .or_else(|| todo.get("title"))
                .and_then(Value::as_str)?
                .trim();
            if title.is_empty() {
                return None;
            }
            let status = match todo.get("status").and_then(Value::as_str) {
                Some("completed") => TaskStatus::Completed,
                Some("in_progress" | "inProgress") => TaskStatus::InProgress,
                _ => TaskStatus::Pending,
            };
            Some(TaskSnapshotItem {
                task_id: todo
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("cursor-task-{index}")),
                title: title.into(),
                status,
                detail: None,
                blocked_by: vec![],
            })
        })
        .collect::<Vec<_>>();
    Some(TasksSnapshot {
        explanation: value
            .get("overview")
            .and_then(Value::as_str)
            .map(str::to_string),
        tasks,
    })
}

fn map_spawn_error(error: RpcChildError) -> ProviderError {
    match error {
        RpcChildError::SpawnFailed(source) if source.kind() == std::io::ErrorKind::NotFound => {
            ProviderError::NotInstalled {
                provider: crate::agent_provider::ProviderKind::Cursor,
                hint: "Install Cursor Agent and ensure `cursor-agent` is on PATH.".into(),
            }
        }
        other => ProviderError::ProcessError {
            message: "Could not start Cursor ACP".into(),
            source: Some(other.to_string()),
        },
    }
}

fn map_auth_error(error: RpcChildError) -> ProviderError {
    let message = error.to_string();
    if looks_unauthenticated(&message) {
        ProviderError::NotAuthenticated {
            provider: crate::agent_provider::ProviderKind::Cursor,
            hint: "Run `cursor-agent login` and try again.".into(),
        }
    } else {
        ProviderError::RpcError { message }
    }
}

fn map_rpc_error(error: RpcChildError) -> ProviderError {
    match error {
        RpcChildError::Timeout { method, elapsed } => ProviderError::Timeout {
            operation: method,
            elapsed_ms: elapsed.as_millis() as u64,
        },
        RpcChildError::SpawnFailed(_)
        | RpcChildError::ChildExited { .. }
        | RpcChildError::IoError(_)
        | RpcChildError::AlreadyShutdown => ProviderError::ProcessError {
            message: "Cursor ACP process failed".into(),
            source: Some(error.to_string()),
        },
        other => ProviderError::RpcError {
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_questions_are_shaped_for_the_shared_questionnaire() {
        let (questions, payload) = cursor_questions(&json!({
            "title": "Choose",
            "questions": [{
                "id": "language",
                "prompt": "Which language?",
                "allowMultiple": false,
                "options": [{"id": "rust", "label": "Rust"}]
            }]
        }));
        assert_eq!(questions[0].id, "language");
        assert_eq!(payload["questions"][0]["header"], "Choose");
        assert_eq!(payload["questions"][0]["question"], "Which language?");
        assert_eq!(payload["questions"][0]["options"][0]["label"], "Rust");
        assert_eq!(payload["questions"][0]["allowOther"], false);
        assert_eq!(questions[0].options[0].id, "rust");
    }

    #[test]
    fn questionnaire_answers_use_cursor_option_ids() {
        let questions = vec![CursorQuestion {
            id: "language".into(),
            prompt: "Which language?".into(),
            options: vec![
                CursorQuestionOption {
                    id: "rust".into(),
                    label: "Rust".into(),
                },
                CursorQuestionOption {
                    id: "typescript".into(),
                    label: "TypeScript".into(),
                },
            ],
        }];
        let decision = ApprovalDecision::Allow {
            updated_input: Some(json!({
                "answers": {"Which language?": "Rust"}
            })),
            updated_permissions: None,
        };
        assert_eq!(
            cursor_question_response(&questions, &decision),
            json!({
                "outcome": {
                    "outcome": "answered",
                    "answers": [{
                        "questionId": "language",
                        "selectedOptionIds": ["rust"]
                    }]
                }
            })
        );
    }

    #[test]
    fn questionnaire_preserves_single_option_labels_containing_commas() {
        let questions = vec![CursorQuestion {
            id: "stack".into(),
            prompt: "Which stack?".into(),
            options: vec![CursorQuestionOption {
                id: "ts-react".into(),
                label: "TypeScript, React".into(),
            }],
        }];
        let decision = ApprovalDecision::Allow {
            updated_input: Some(json!({
                "answers": {"Which stack?": "TypeScript, React"}
            })),
            updated_permissions: None,
        };
        assert_eq!(
            cursor_question_answers(&questions, &decision),
            vec![json!({
                "questionId": "stack",
                "selectedOptionIds": ["ts-react"]
            })]
        );
    }

    #[test]
    fn load_response_reuses_the_requested_session_id() {
        let response = json!({
            "configOptions": [{"id": "model", "currentValue": "grok-4.6"}]
        });
        let (setup, id) = loaded_session_setup(response.clone(), "cursor-session-1".into());
        assert_eq!(setup, response);
        assert_eq!(id, "cursor-session-1");
    }

    #[test]
    fn stop_reasons_map_to_the_closest_shared_turn_status() {
        assert!(matches!(
            turn_status_from_stop_reason(Some("end_turn")),
            TurnStatus::Success
        ));
        assert!(matches!(
            turn_status_from_stop_reason(Some("max_tokens")),
            TurnStatus::MaxTurns
        ));
        // Neither of these is a successful turn, and reporting them as one
        // told the transcript the model had finished its work.
        assert!(matches!(
            turn_status_from_stop_reason(Some("max_turn_requests")),
            TurnStatus::MaxTurns
        ));
        assert!(matches!(
            turn_status_from_stop_reason(Some("refusal")),
            TurnStatus::Error { ref subtype, .. } if subtype == "refusal"
        ));
        assert!(matches!(
            turn_status_from_stop_reason(Some("cancelled")),
            TurnStatus::Error { ref subtype, .. } if subtype == "interrupted"
        ));
    }

    #[test]
    fn cancelling_a_pending_request_answers_it_with_the_acp_cancelled_outcome() {
        // Every pending kind must produce a well-formed answer, because a
        // turn that ends with one outstanding has to unblock the child.
        assert_eq!(
            pending_response(
                &PendingRequest::Permission {
                    rpc_id: json!(7),
                    options: vec![json!({"kind": "allow_once", "optionId": "ok"})],
                },
                &ApprovalDecision::Cancel,
            ),
            json!({"outcome": {"outcome": "cancelled"}})
        );
        assert_eq!(
            pending_response(
                &PendingRequest::UserInput {
                    rpc_id: json!(8),
                    questions: vec![],
                },
                &ApprovalDecision::Cancel,
            ),
            json!({"outcome": {"outcome": "cancelled"}})
        );
        assert_eq!(
            pending_response(&PendingRequest::Plan { rpc_id: json!(9) }, &ApprovalDecision::Cancel),
            json!({"outcome": {"outcome": "cancelled"}})
        );
        assert_eq!(
            pending_rpc_id(PendingRequest::Plan { rpc_id: json!(9) }),
            json!(9)
        );
    }

    #[test]
    fn usage_updates_parse_defensively() {
        // Flat camelCase, the shape Cursor's ACP frames use today.
        assert_eq!(
            usage_totals_from_value(&json!({
                "sessionUpdate": "usage_update",
                "inputTokens": 120,
                "outputTokens": 30,
                "cachedInputTokens": 400
            })),
            Some(CursorUsageTotals {
                input: 120,
                output: 30,
                cache_read: 400,
                cache_write: 0,
            })
        );
        // Nested under a `usage` container.
        assert_eq!(
            usage_totals_from_value(&json!({
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })),
            Some(CursorUsageTotals {
                input: 5,
                output: 6,
                cache_read: 0,
                cache_write: 0,
            })
        );
        // Unrecognized shape → skipped, never guessed.
        assert_eq!(
            usage_totals_from_value(&json!({"sessionUpdate": "usage_update", "cost": 0.2})),
            None
        );
        assert_eq!(
            context_window_from_value(&json!({"contextWindow": 200_000})),
            Some(200_000)
        );
        assert_eq!(context_window_from_value(&json!({"contextWindow": 0})), None);
    }

    #[test]
    fn usage_deltas_never_go_negative_across_a_reset() {
        let first = CursorUsageTotals {
            input: 100,
            output: 50,
            cache_read: 0,
            cache_write: 0,
        };
        let second = CursorUsageTotals {
            input: 140,
            output: 60,
            cache_read: 0,
            cache_write: 0,
        };
        assert_eq!(
            second.delta_since(&first),
            CursorUsageTotals {
                input: 40,
                output: 10,
                cache_read: 0,
                cache_write: 0,
            }
        );
        assert!(first.delta_since(&second).is_empty());
        assert_eq!(second.context_tokens(), 200);
    }

    #[test]
    fn plan_decisions_match_the_documented_cursor_extension_schema() {
        assert_eq!(
            cursor_plan_response(&ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            }),
            json!({"outcome": {"outcome": "accepted"}})
        );
        assert_eq!(
            cursor_plan_response(&ApprovalDecision::Deny {
                message: "Revise it".into(),
            }),
            json!({"outcome": {"outcome": "rejected", "reason": "Revise it"}})
        );
    }
}
