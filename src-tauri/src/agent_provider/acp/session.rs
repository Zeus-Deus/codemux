//! One ACP subprocess and session per Codemux thread.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent_provider::{
    pricing, ApprovalDecision, CompletedItem, ContentDelta, ContextUsageTracker, CostSource,
    ProviderError, ProviderKind, ProviderRuntimeEvent, ProviderSessionId, RequestId, SendOutcome,
    SendTurnInput, SessionStatus, TaskSnapshotItem, TaskStatus, TasksSnapshot, ThreadId, TurnId,
    TurnStatus, TurnUsage,
};
use crate::json_rpc_child::{
    IncomingRequest, JsonRpcChild, Notification, RpcChildError, RpcError, SpawnConfig,
};

use super::protocol::{
    config_id_for, config_options, current_model_id, grok_auth_method, grok_model_effort_catalog,
    initialize_params, looks_unauthenticated, option_by_id, resolve_boolean_value,
    resolve_effort_value, resolve_select_value, session_id, set_config_params, set_model_params,
    ConfigKind, GrokModelEffortCatalog,
};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const PROMPT_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// After Grok's durable `turn_completed` lifecycle update, keep waiting for
/// the authoritative standard prompt response through a normal RPC watchdog
/// window. Grok can still be uploading traces/session state after the terminal
/// update, so a short grace could overlap finalization with the next prompt.
/// Older agents sometimes omit the response entirely, which is why the
/// terminal-update fallback still exists after this conservative delay.
const XAI_TERMINAL_RESPONSE_GRACE: Duration = RPC_TIMEOUT;
const DISPATCHING_TURN_SUFFIX: &str = "dispatching-turn";

/// Provider-specific ACP behavior layered over the shared session engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpDialect {
    Cursor,
    Grok,
}

impl AcpDialect {
    fn provider(self) -> ProviderKind {
        match self {
            Self::Cursor => ProviderKind::Cursor,
            Self::Grok => ProviderKind::Grok,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cursor => "Cursor",
            Self::Grok => "Grok",
        }
    }

    fn id_prefix(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Grok => "grok",
        }
    }

    fn spawn_args(self) -> Vec<String> {
        match self {
            Self::Cursor => vec!["acp".into()],
            // Disable the CLI's own background updater inside a managed GUI
            // process. The user's PATH-level package manager remains the
            // authority for upgrades and newly released models.
            Self::Grok => vec![
                "--no-auto-update".into(),
                "agent".into(),
                "--no-leader".into(),
                "stdio".into(),
            ],
        }
    }

    fn default_permission_mode(self) -> &'static str {
        match self {
            Self::Cursor | Self::Grok => "agent",
        }
    }

    fn is_grok(self) -> bool {
        self == Self::Grok
    }
}

#[derive(Debug, Clone)]
pub struct AcpSpawnConfig {
    pub binary: PathBuf,
    pub dialect: AcpDialect,
    /// Shared only by the Grok adapter. Cursor leaves this unset and retains
    /// its existing command behavior.
    pub grok_slash_command_cache:
        Option<Arc<crate::agent_provider::grok::slash_commands::GrokSlashCommandCache>>,
}

#[derive(Debug)]
struct XAiPromptCompletion {
    prompt_id: String,
    tx: oneshot::Sender<Value>,
    announced: Option<Value>,
}

#[derive(Debug, Clone)]
struct QueuedTurn {
    id: String,
    input: SendTurnInput,
}

#[derive(Debug, Clone)]
struct AcpQuestion {
    id: String,
    prompt: String,
    options: Vec<AcpQuestionOption>,
}

#[derive(Debug, Clone)]
struct AcpQuestionOption {
    id: String,
    label: String,
}

#[derive(Debug, Clone, Copy)]
enum UserInputDialect {
    Cursor,
    XAi,
}

#[derive(Debug, Clone, Copy)]
enum PlanRequestDialect {
    Cursor,
    XAi,
}

#[derive(Debug, Clone)]
enum PendingRequest {
    Permission {
        rpc_id: Value,
        options: Vec<Value>,
    },
    UserInput {
        rpc_id: Value,
        questions: Vec<AcpQuestion>,
        dialect: UserInputDialect,
    },
    Plan {
        rpc_id: Value,
        dialect: PlanRequestDialect,
    },
}

/// Running token totals observed from an ACP `usage_update`, so the
/// ledger can be fed deltas from a cumulative report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AcpUsageTotals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

/// Grok's per-prompt usage report. Unlike Cursor's cumulative session
/// counter, this is one completed user turn and is recorded exactly once.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct GrokPromptUsage {
    tokens: AcpUsageTotals,
    reasoning: u64,
    num_turns: u32,
    api_duration_ms: u64,
    /// Provider-measured spend. Grok omits this whenever its bill is partial
    /// or incomplete, so presence is the trust signal. We deliberately do not
    /// substitute Codemux's static price table: the provider's dynamic model
    /// catalogue can advance independently of a Codemux release.
    cost_usd: Option<f64>,
}

pub(crate) struct AcpSessionState {
    pub status: SessionStatus,
    pub active_turn: Option<TurnId>,
    model: Option<String>,
    permission_mode: String,
    config_options: Vec<Value>,
    pending: HashMap<RequestId, PendingRequest>,
    queued: VecDeque<QueuedTurn>,
    assistant_text: String,
    thinking_text: String,
    /// Monotonic id of the current claim on the session's single prompt
    /// slot, bumped every time the slot is claimed (see
    /// [`AcpSessionState::claim_turn`]).
    turn_generation: u64,
    /// The generation a Stop applies to, if any.
    ///
    /// Stop cannot always be forwarded the instant it arrives: while a
    /// turn is still being dispatched (config round-trips, queue
    /// promotion) the child has no prompt to cancel, so the request has
    /// to be parked. Parking it as a bare boolean leaked — a Stop that
    /// landed in a window where nothing consumed it survived into the
    /// NEXT turn and completed it as interrupted before its prompt was
    /// ever written. Tagging it with the generation it was aimed at makes
    /// staleness self-invalidating: the next claim bumps the counter and
    /// the orphaned request can never match again.
    interrupt_generation: Option<u64>,
    /// High-water mark of the cumulative usage the agent last reported.
    usage_totals: AcpUsageTotals,
    /// Context-window occupancy, clamped/deduplicated by the shared
    /// tracker every other adapter uses.
    context: ContextUsageTracker,
    /// Authoritative effort lists advertised by Grok's live model catalog.
    /// Unknown models/metadata remain permissive for forward compatibility.
    grok_model_efforts: GrokModelEffortCatalog,
    /// Effective effort acknowledged by Grok's `model_changed` lifecycle
    /// update. This can differ from the requested value after defaults or
    /// provider policy are applied.
    grok_effort: Option<String>,
}

impl AcpSessionState {
    /// Claim the prompt slot for a new turn and return its generation.
    ///
    /// Callers set `active_turn` themselves — some claim the
    /// `DISPATCHING` marker first and only mint the real turn id later —
    /// but every claim must come through here so interrupt targeting
    /// stays accurate.
    fn claim_turn(&mut self) -> u64 {
        self.turn_generation = self.turn_generation.wrapping_add(1);
        self.turn_generation
    }

    /// Aim a Stop at whatever is claimed right now. No-op when the
    /// session is idle: there is nothing to interrupt.
    fn request_interrupt(&mut self) {
        if self.active_turn.is_some() {
            self.interrupt_generation = Some(self.turn_generation);
        }
    }

    /// Consume a Stop aimed at `generation`. A request aimed at any other
    /// generation is left alone — it belongs to a turn that has already
    /// finished, and honoring it would cancel an unrelated turn.
    fn take_interrupt(&mut self, generation: u64) -> bool {
        if self.interrupt_generation == Some(generation) {
            self.interrupt_generation = None;
            return true;
        }
        false
    }

    /// Release the prompt slot. Drops any Stop still aimed at the
    /// generation being released so it cannot outlive its turn.
    fn release_turn(&mut self) {
        self.active_turn = None;
        self.interrupt_generation = None;
    }
}

/// A token a message pump answers once it has drained everything the
/// child wrote ahead of it. See
/// [`AcpSession::await_child_messages_drained`].
type NotificationBarrier = oneshot::Sender<()>;

pub(crate) struct AcpSession {
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
    pub state: Mutex<AcpSessionState>,
    cwd: PathBuf,
    child: Arc<JsonRpcChild>,
    dialect: AcpDialect,
    grok_slash_command_cache:
        Option<Arc<crate::agent_provider::grok::slash_commands::GrokSlashCommandCache>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Ordering barriers into the two tasks that consume child messages.
    /// See [`AcpSession::await_child_messages_drained`].
    notification_barrier_tx: mpsc::UnboundedSender<NotificationBarrier>,
    request_barrier_tx: mpsc::UnboundedSender<NotificationBarrier>,
    /// Woken by [`AcpSession::interrupt`] so the prompt worker can
    /// deliver `session/cancel` itself. See
    /// [`AcpSession::send_prompt`].
    interrupt_notify: Notify,
    xai_prompt_completion: Mutex<Option<XAiPromptCompletion>>,
    stopped: AtomicBool,
}

impl AcpSession {
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
        spawn: AcpSpawnConfig,
        event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    ) -> Result<Arc<Self>, ProviderError> {
        let dialect = spawn.dialect;
        let grok_slash_command_cache = spawn.grok_slash_command_cache.clone();
        let child_env = env.unwrap_or_default();
        let child = JsonRpcChild::spawn(SpawnConfig {
            program: spawn.binary,
            args: dialect.spawn_args(),
            env: child_env.clone(),
            cwd: Some(cwd.clone()),
            default_timeout: RPC_TIMEOUT,
        })
        .await
        .map_err(|error| map_spawn_error(error, dialect))?;
        let child = Arc::new(child);
        let notification_rx = child.notifications();
        let request_rx = child
            .incoming_requests()
            .ok_or_else(|| ProviderError::RpcError {
                message: format!(
                    "{} ACP request receiver was already claimed",
                    dialect.label()
                ),
            })?;

        let setup = async {
            let initialized = child
                .request("initialize", initialize_params("codemux"))
                .await
                .map_err(|error| map_rpc_error(error, dialect))?;
            if dialect.is_grok() {
                if let Some(cache) = grok_slash_command_cache.as_ref() {
                    cache.replace_from_value(&cwd, &initialized).await;
                }
            }
            let auth_method = match dialect {
                AcpDialect::Cursor => "cursor_login".to_string(),
                AcpDialect::Grok => grok_auth_method(&initialized, &child_env).ok_or_else(|| {
                    ProviderError::NotAuthenticated {
                        provider: ProviderKind::Grok,
                        hint: "Run `grok login --device-auth` (or set `XAI_API_KEY`) and try again."
                            .into(),
                    }
                })?,
            };
            let auth_params = if dialect.is_grok() {
                json!({ "methodId": auth_method, "_meta": { "headless": true } })
            } else {
                json!({ "methodId": auth_method })
            };
            child
                .request("authenticate", auth_params)
                .await
                .map_err(|error| map_auth_error(error, dialect))?;

            // `initialize` is Grok's first authoritative model catalogue.
            // Reconcile persisted startup preferences before they reach
            // `session/new`: a model (or effort for its replacement) may have
            // disappeared since Codemux last saved the composer selection.
            let initialized_grok_catalog = dialect
                .is_grok()
                .then(|| grok_model_effort_catalog(&initialized))
                .unwrap_or_default();
            let (startup_grok_model, startup_grok_effort) = if dialect.is_grok() {
                reconcile_grok_startup_config(
                    &initialized_grok_catalog,
                    current_model_id(&initialized).as_deref(),
                    model.as_deref(),
                    effort.as_deref(),
                )
            } else {
                (None, None)
            };
            let requested_grok_model = model
                .as_deref()
                .map(base_model_id)
                .filter(|value| !value.is_empty() && *value != "default");
            if dialect.is_grok()
                && !initialized_grok_catalog.is_empty()
                && requested_grok_model.is_some()
                && requested_grok_model != startup_grok_model.as_deref()
            {
                let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread_id.clone()),
                    message: format!(
                        "Grok no longer advertises startup model `{}`; using the live provider default",
                        requested_grok_model.unwrap_or_default()
                    ),
                    original_payload: None,
                });
            }
            if dialect.is_grok()
                && effort.as_deref().is_some_and(|value| !value.trim().is_empty())
                && startup_grok_model.is_some()
                && startup_grok_effort.is_none()
            {
                let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread_id.clone()),
                    message: format!(
                        "Grok no longer advertises reasoning effort `{}` for startup model `{}`; using the live model default",
                        effort.as_deref().unwrap_or_default(),
                        startup_grok_model.as_deref().unwrap_or_default()
                    ),
                    original_payload: None,
                });
            }

            let new_session_params = || {
                let mut params = json!({ "cwd": cwd.clone(), "mcpServers": [] });
                if dialect.is_grok() {
                    let mut meta = json!({
                        "yoloMode": permission_mode
                            .as_deref()
                            .unwrap_or_else(|| dialect.default_permission_mode())
                            == "agent"
                    });
                    if let Some(model) = startup_grok_model.as_deref() {
                        meta["modelId"] = Value::String(model.to_string());
                    }
                    params["_meta"] = meta;
                }
                params
            };

            let resume_id = resume_cursor.as_ref().and_then(resume_session_id);
            let (session_setup, provider_session_id) = if let Some(session_id) = resume_id {
                let mut load_params = new_session_params();
                load_params["sessionId"] = Value::String(session_id.clone());
                match child
                    .request("session/load", load_params)
                    .await
                {
                    // ACP's LoadSessionResponse deliberately omits sessionId:
                    // the client already supplied it in the request. Keep the
                    // requested id instead of treating a standards-compliant
                    // response as a failed restart.
                    Ok(response) => loaded_session_setup(response, session_id),
                    Err(load_error) => {
                        let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                            thread_id: Some(thread_id.clone()),
                            message: format!(
                                "{} could not resume the prior ACP session; started a new session instead: {load_error}",
                                dialect.label()
                            ),
                            original_payload: resume_cursor.clone(),
                        });
                        let response = child
                            .request("session/new", new_session_params())
                            .await
                            .map_err(|error| map_rpc_error(error, dialect))?;
                        new_session_setup(response, dialect)?
                    }
                }
            } else {
                let response = child
                    .request("session/new", new_session_params())
                    .await
                    .map_err(|error| map_rpc_error(error, dialect))?;
                new_session_setup(response, dialect)?
            };
            Ok::<_, ProviderError>((
                session_setup,
                provider_session_id,
                initialized,
                startup_grok_model,
            ))
        }
        .await;

        let (setup, provider_session_id, initialized, startup_grok_model) = match setup {
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
            state: Mutex::new(AcpSessionState {
                status: SessionStatus::Ready,
                active_turn: None,
                model: current_model_id(&setup).or_else(|| current_model_id(&initialized)),
                permission_mode: permission_mode
                    .clone()
                    .unwrap_or_else(|| dialect.default_permission_mode().into()),
                config_options: config_options(&setup),
                pending: HashMap::new(),
                queued: VecDeque::new(),
                assistant_text: String::new(),
                thinking_text: String::new(),
                turn_generation: 0,
                interrupt_generation: None,
                usage_totals: AcpUsageTotals::default(),
                context: ContextUsageTracker::default(),
                grok_model_efforts: {
                    let mut catalog = grok_model_effort_catalog(&initialized);
                    catalog.extend(grok_model_effort_catalog(&setup));
                    catalog
                },
                grok_effort: grok_reasoning_effort(&setup)
                    .or_else(|| grok_reasoning_effort(&initialized)),
            }),
            cwd,
            child,
            dialect,
            grok_slash_command_cache,
            event_tx,
            tasks: Mutex::new(Vec::new()),
            notification_barrier_tx,
            request_barrier_tx,
            interrupt_notify: Notify::new(),
            xai_prompt_completion: Mutex::new(None),
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
            if dialect.is_grok() {
                let selected_model = startup_grok_model.or_else(|| current_model_id(&setup));
                if let Some(selected_model) = selected_model {
                    let reconciled_effort = {
                        let state = session.state.lock().await;
                        reconcile_grok_effort(
                            &state.grok_model_efforts,
                            &selected_model,
                            effort.as_deref(),
                        )
                    };
                    session
                        .set_grok_model(selected_model, reconciled_effort.as_deref())
                        .await?;
                }
            } else {
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
                // `false` is an explicit user choice, not "leave Cursor's
                // default alone". Some promoted models default to Fast.
                session
                    .set_boolean_config(ConfigKind::Fast, fast_mode)
                    .await?;
                if let Some(mode) = permission_mode {
                    session.set_permission_mode(mode).await?;
                }
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

    fn opaque_id(&self, kind: &str) -> String {
        format!("{}-{kind}-{}", self.dialect.id_prefix(), Uuid::new_v4())
    }

    fn dispatching_turn_id(&self) -> TurnId {
        TurnId(format!(
            "{}-{DISPATCHING_TURN_SUFFIX}",
            self.dialect.id_prefix()
        ))
    }

    fn is_dispatching_turn(&self, turn: &TurnId) -> bool {
        turn.0 == format!("{}-{DISPATCHING_TURN_SUFFIX}", self.dialect.id_prefix())
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
                                format!("{} ACP notification stream dropped {count} messages", notifications.dialect.label()),
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
            let queued_id = self.opaque_id("queued");
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
        state.active_turn = Some(self.dispatching_turn_id());
        let generation = state.claim_turn();
        drop(state);
        match self.dispatch_turn(input, None, generation).await {
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
        generation: u64,
    ) -> Result<TurnId, ProviderError> {
        let (turn_id, prompt) = self.prepare_turn(input, queued_id).await?;
        let session = Arc::clone(self);
        let worker_turn_id = turn_id.clone();
        let task = tokio::spawn(async move {
            session
                .run_prompt_worker(worker_turn_id, prompt, generation)
                .await;
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
        mut input: SendTurnInput,
        queued_id: Option<String>,
    ) -> Result<(TurnId, Vec<Value>), ProviderError> {
        if let Some(checkpoint) = &input.turn_checkpoint {
            checkpoint.prepare().await;
        }
        let configured = async {
            if self.dialect.is_grok() {
                let requested_model = input
                    .model_override
                    .as_deref()
                    .map(base_model_id)
                    .filter(|model| !model.is_empty() && *model != "default")
                    .map(str::to_string);
                let selected_model = match requested_model {
                    Some(model) => Some(model),
                    None => self.state.lock().await.model.clone(),
                };
                if let Some(model) = selected_model {
                    if input.model_override.is_some() || input.effort_override.is_some() {
                        let reconciled_effort = {
                            let state = self.state.lock().await;
                            reconcile_grok_effort(
                                &state.grok_model_efforts,
                                &model,
                                input.effort_override.as_deref(),
                            )
                        };
                        if input.effort_override.is_some() && reconciled_effort.is_none() {
                            self.warn(
                                format!(
                                    "Grok no longer advertises reasoning effort `{}` for model `{model}`; using the live model default",
                                    input.effort_override.as_deref().unwrap_or_default()
                                ),
                                None,
                            );
                            input.effort_override = None;
                        }
                        self.set_grok_model(model, reconciled_effort.as_deref())
                            .await?;
                    }
                }
            } else {
                if let Some(model) = input.model_override.clone() {
                    self.set_model(model).await?;
                }
                if let Some(effort) = input.effort_override.as_deref() {
                    self.set_effort_config(effort).await?;
                }
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

        let turn_id = TurnId(self.opaque_id("turn"));
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

    async fn run_prompt_worker(
        self: Arc<Self>,
        mut turn_id: TurnId,
        mut prompt: Vec<Value>,
        mut generation: u64,
    ) {
        loop {
            // A Stop that landed while this turn was still being
            // dispatched has nothing to cancel on the child yet, so the
            // turn simply never starts. Checked under the same lock
            // `interrupt` writes, so the request cannot slip past.
            let interrupted_before_send = self.state.lock().await.take_interrupt(generation);
            let response = if interrupted_before_send {
                Ok(json!({ "stopReason": "cancelled" }))
            } else {
                self.send_prompt(&prompt, generation).await
            };
            let next = self.finish_turn(turn_id, response, generation).await;
            let Some((queued, next_generation)) = next else {
                return;
            };
            generation = next_generation;
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
                        // `release_turn` (not a bare `active_turn = None`)
                        // so a Stop aimed at this abandoned claim cannot
                        // outlive it.
                        state.release_turn();
                        state.status = SessionStatus::Ready;
                        state
                            .queued
                            .drain(..)
                            .map(|queued| queued.id)
                            .collect::<Vec<_>>()
                    };
                    self.warn(
                        format!(
                            "Could not dispatch queued {} turn: {error}",
                            self.dialect.label()
                        ),
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

    /// Run one `session/prompt`, forwarding any Stop that arrives while
    /// it is in flight.
    ///
    /// The cancel is issued from HERE rather than from
    /// [`AcpSession::interrupt`] to fix an ordering race: an interrupt
    /// firing microseconds after the worker claimed the in-flight window
    /// could win the stdin write race and reach the child before the
    /// prompt it was meant to cancel, leaving the child with nothing to
    /// cancel and the user's Stop silently dropped. `biased` polls the
    /// prompt future first, so the request has already claimed the
    /// transport's writer mutex before the cancel arm is ever reachable,
    /// and that mutex is FIFO so the cancel queues behind the prompt and
    /// still reaches the child second. `session/cancel` is a notification
    /// with no reply, so repeating it is harmless.
    async fn send_prompt(&self, prompt: &[Value], generation: u64) -> Result<Value, RpcChildError> {
        let prompt_id = self.dialect.is_grok().then(|| self.opaque_id("prompt"));
        let fallback_rx = if let Some(prompt_id) = prompt_id.as_ref() {
            let (tx, rx) = oneshot::channel();
            *self.xai_prompt_completion.lock().await = Some(XAiPromptCompletion {
                prompt_id: prompt_id.clone(),
                tx,
                announced: None,
            });
            Some(rx)
        } else {
            None
        };
        let mut params = json!({
            "sessionId": self.provider_session_id.0,
            "prompt": prompt
        });
        if let Some(prompt_id) = prompt_id.as_ref() {
            params["_meta"] = json!({
                "promptId": prompt_id,
                "requestId": prompt_id
            });
        }
        let request = self
            .child
            .request_with_timeout("session/prompt", params, PROMPT_TIMEOUT);
        let fallback = async move {
            match fallback_rx {
                Some(rx) => rx.await.ok(),
                None => std::future::pending::<Option<Value>>().await,
            }
        };
        let completion =
            prefer_standard_prompt_response(request, fallback, XAI_TERMINAL_RESPONSE_GRACE);
        tokio::pin!(completion);
        loop {
            tokio::select! {
                biased;
                response = &mut completion => {
                    self.clear_xai_prompt_completion(prompt_id.as_deref()).await;
                    return response;
                },
                () = self.interrupt_notify.notified() => {
                    if !self.state.lock().await.take_interrupt(generation) {
                        continue;
                    }
                    // Hand the cancel to a detached task instead of
                    // awaiting it here. A child's stdin is a blocking-pool
                    // wrapper on Windows, whose `flush` yields at least
                    // once, so the pinned prompt future can be parked
                    // *inside* `write_line` while still holding the
                    // transport's writer mutex. Awaiting the cancel in this
                    // branch body would then block on that same mutex —
                    // and `select!` does not poll the prompt arm that owns
                    // it while a branch body is awaiting, so the turn
                    // deadlocks and the Stop is swallowed. Spawning keeps
                    // the loop polling the prompt so its write can finish
                    // and release the lock; the cancel queues behind it on
                    // the FIFO mutex, preserving the order described above.
                    let child = Arc::clone(&self.child);
                    let session_id = self.provider_session_id.0.clone();
                    let event_tx = self.event_tx.clone();
                    let thread_id = self.thread_id.clone();
                    let provider_label = self.dialect.label();
                    tokio::spawn(async move {
                        if let Err(error) = child
                            .notify("session/cancel", json!({ "sessionId": session_id }))
                            .await
                        {
                            let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                                thread_id: Some(thread_id),
                                message: format!(
                                    "Could not cancel the {} turn: {error}",
                                    provider_label
                                ),
                                original_payload: None,
                            });
                        }
                    });
                }
            }
        }
    }

    async fn clear_xai_prompt_completion(&self, prompt_id: Option<&str>) {
        let mut pending = self.xai_prompt_completion.lock().await;
        if pending
            .as_ref()
            .is_some_and(|entry| Some(entry.prompt_id.as_str()) == prompt_id)
        {
            pending.take();
        }
    }

    /// Record xAI's early completion announcement without ending the turn.
    /// Grok can emit more chunks after this notification; only the durable
    /// terminal update below is allowed to release the fallback waiter.
    async fn record_xai_prompt_completion(&self, params: Value) {
        let Some(session_id) = xai_string_field(&params, "sessionId", "session_id") else {
            return;
        };
        let Some(prompt_id) = xai_string_field(&params, "promptId", "prompt_id") else {
            return;
        };
        if session_id != self.provider_session_id.0 {
            return;
        }
        let Some(completion) = xai_completion_value(&params, &session_id, &prompt_id) else {
            return;
        };
        let mut pending = self.xai_prompt_completion.lock().await;
        if let Some(entry) = pending
            .as_mut()
            .filter(|entry| entry.prompt_id == prompt_id)
        {
            entry.announced = Some(completion);
        }
    }

    /// Resolve an omitted standard prompt response only from Grok's durable
    /// lifecycle terminal, which is ordered after the final session update.
    async fn resolve_xai_terminal_completion(&self, params: Value) {
        let mut pending = self.xai_prompt_completion.lock().await;
        let Some(entry) = pending.as_ref() else {
            return;
        };
        let Some(completion) = xai_terminal_completion(
            &params,
            &self.provider_session_id.0,
            &entry.prompt_id,
            entry.announced.clone(),
        ) else {
            return;
        };
        let Some(entry) = pending.take() else {
            return;
        };
        let _ = entry.tx.send(completion);
    }

    /// xAI extension envelopes carry the live context occupancy separately
    /// from their per-prompt billing aggregate. Keep that level reading fresh
    /// for every extension update, then process the durable terminal marker.
    async fn handle_xai_session_update(&self, params: Value) {
        if crate::agent_provider::grok::slash_commands::is_available_commands_update(&params) {
            if let Some(cache) = self.grok_slash_command_cache.as_ref() {
                cache.replace_from_value(&self.cwd, &params).await;
            }
        }
        if let Some((model, effort)) = xai_model_changed(&params) {
            let mut state = self.state.lock().await;
            state.model = Some(model);
            state.grok_effort = effort;
        }
        if let Some(used_tokens) = xai_live_total_tokens(&params) {
            let events = self.state.lock().await.context.events(
                &self.thread_id,
                used_tokens,
                None,
                Some(true),
            );
            for event in events {
                let _ = self.event_tx.send(event);
            }
        }
        self.resolve_xai_terminal_completion(params).await;
    }

    async fn finish_turn(
        &self,
        turn_id: TurnId,
        response: Result<Value, RpcChildError>,
        generation: u64,
    ) -> Option<(QueuedTurn, u64)> {
        // Ordering barrier — see `await_child_messages_drained`. Must run
        // before the buffers below are taken, and without holding the
        // state lock the notification task needs.
        self.await_child_messages_drained().await;
        let (assistant_text, thinking_text, cancelled_requests) = {
            let mut state = self.state.lock().await;
            // This turn is over, so a Stop still aimed at it is moot.
            state.take_interrupt(generation);
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
        let (grok_usage, grok_live_tokens) = if self.dialect.is_grok() {
            match &response {
                Ok(result) => (
                    grok_prompt_usage_from_value(result),
                    xai_live_total_tokens(result),
                ),
                Err(RpcChildError::RpcError(error)) => {
                    let data = error.data.as_ref();
                    (
                        data.and_then(grok_prompt_usage_from_value),
                        data.and_then(xai_live_total_tokens),
                    )
                }
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };
        if self.dialect.is_grok() {
            self.record_grok_turn_usage(grok_usage, grok_live_tokens)
                .await;
        }
        let turn_usage = grok_usage.map(|usage| TurnUsage {
            total_cost_usd: usage.cost_usd,
            duration_ms: usage.api_duration_ms,
            num_turns: usage.num_turns,
        });
        let status = match response {
            Ok(result) => turn_status_from_result(&result, self.dialect),
            Err(error) => turn_status_from_rpc_error(&error, self.dialect),
        };
        let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
            thread_id: self.thread_id.clone(),
            turn_id: turn_id.clone(),
            status,
            usage: turn_usage,
        });
        let next = {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Ready;
            // A Stop can also land in the gap between the two locked
            // sections above, where `active_turn` still names the turn
            // that just finished. Dropping it here — and claiming a fresh
            // generation for any queued turn — is what stops it becoming
            // an instant cancellation of somebody else's turn.
            state.interrupt_generation = None;
            match state.queued.pop_front() {
                Some(queued) => {
                    // Reserve the dispatch slot before releasing the lock.
                    // Without this marker a concurrent send could observe
                    // an idle session between popping the queued turn and
                    // preparing it, starting a second prompt worker for
                    // the same ACP session.
                    state.active_turn = Some(self.dispatching_turn_id());
                    Some((queued, state.claim_turn()))
                }
                None => {
                    state.release_turn();
                    None
                }
            }
        };
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::Ready,
            });
        next
    }

    /// Record a Stop against the turn that is claimed right now.
    ///
    /// Nothing is written to the child from here. The prompt worker owns
    /// delivery — it either abandons a turn whose prompt has not gone out
    /// yet, or issues `session/cancel` from inside
    /// [`AcpSession::send_prompt`], where the cancel provably follows
    /// the prompt it cancels.
    pub async fn interrupt(&self, turn_id: Option<TurnId>) -> Result<(), ProviderError> {
        {
            let mut state = self.state.lock().await;
            let active = state.active_turn.clone();
            if active.is_none() || turn_id.is_some_and(|requested| Some(requested) != active) {
                return Ok(());
            }
            state.request_interrupt();
        }
        self.interrupt_notify.notify_one();
        Ok(())
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
                    state.active_turn = Some(self.dispatching_turn_id());
                    let generation = state.claim_turn();
                    (Some((queued, generation)), false)
                }
                None => (None, false),
            }
        };
        if let Some((queued, generation)) = ready_to_dispatch {
            let queued_id = queued.id.clone();
            if let Err(error) = self
                .dispatch_turn(queued.input, Some(queued.id), generation)
                .await
            {
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
        if state
            .active_turn
            .as_ref()
            .is_some_and(|turn| self.is_dispatching_turn(turn))
        {
            state.release_turn();
            state.status = SessionStatus::Ready;
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
            .map_err(|error| map_rpc_error(error, self.dialect))?;
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
                state
                    .active_turn
                    .clone()
                    .map_or(SessionStatus::Ready, |active_turn| SessionStatus::Running {
                        active_turn,
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
            if !self.dialect.is_grok() {
                self.state.lock().await.model = None;
            }
            return Ok(());
        }
        if self.dialect.is_grok() {
            return self.set_grok_model(model.to_string(), None).await;
        }
        self.set_semantic_config(ConfigKind::Model, model, true)
            .await?;
        self.state.lock().await.model = Some(model.to_string());
        Ok(())
    }

    /// Apply the Fast session option without replacing the ACP session.
    /// Dialects that never advertise a Fast config option resolve to a
    /// no-op inside [`Self::set_boolean_config`].
    pub async fn set_fast_mode(&self, fast_mode: bool) -> Result<(), ProviderError> {
        self.set_boolean_config(ConfigKind::Fast, fast_mode).await
    }

    pub async fn set_permission_mode(&self, mode: String) -> Result<(), ProviderError> {
        if !matches!(mode.as_str(), "agent" | "ask" | "plan") {
            return Err(ProviderError::ValidationError {
                message: format!(
                    "{} does not expose permission mode `{mode}`",
                    self.dialect.label()
                ),
            });
        }
        if self.dialect.is_grok() {
            let current = self.state.lock().await.permission_mode.clone();
            if current == mode {
                return Ok(());
            }
            return Err(ProviderError::ValidationError {
                message: "Grok permission mode is selected when the session starts; restart the session to change it."
                    .into(),
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
        if self.dialect.is_grok() {
            let model = self.state.lock().await.model.clone().ok_or_else(|| {
                ProviderError::ValidationError {
                    message:
                        "Grok did not advertise an active model for the reasoning-effort change."
                            .into(),
                }
            })?;
            return self.set_grok_model(model, Some(requested)).await;
        }
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
            .map_err(|error| map_rpc_error(error, self.dialect))?;
        let updated = config_options(&response);
        if !updated.is_empty() {
            self.state.lock().await.config_options = updated;
        }
        Ok(())
    }

    async fn set_grok_model(
        &self,
        model: String,
        effort: Option<&str>,
    ) -> Result<(), ProviderError> {
        let model = base_model_id(&model).to_string();
        if model.is_empty() || model == "default" {
            return Ok(());
        }
        let response = self
            .child
            .request(
                "session/set_model",
                set_model_params(&self.provider_session_id.0, &model, effort),
            )
            .await
            .map_err(|error| map_grok_model_error(error, self.dialect))?;
        self.state.lock().await.model = current_model_id(&response).or(Some(model));
        Ok(())
    }

    async fn handle_request(&self, request: IncomingRequest) {
        match request.method.as_str() {
            "session/request_permission" => self.handle_permission_request(request).await,
            "cursor/create_plan" if self.dialect == AcpDialect::Cursor => {
                self.handle_plan_request(request).await
            }
            "cursor/ask_question" if self.dialect == AcpDialect::Cursor => {
                self.handle_question_request(request).await;
            }
            "x.ai/ask_user_question" | "_x.ai/ask_user_question" if self.dialect.is_grok() => {
                self.handle_xai_question_request(request).await;
            }
            "x.ai/exit_plan_mode" | "_x.ai/exit_plan_mode" if self.dialect.is_grok() => {
                self.handle_xai_plan_request(request).await;
            }
            _ => {
                self.warn(
                    format!(
                        "Unsupported {} ACP request `{}`",
                        self.dialect.label(),
                        request.method
                    ),
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

        let request_id = RequestId(self.opaque_id("request"));
        let turn_id = self
            .state
            .lock()
            .await
            .active_turn
            .clone()
            .unwrap_or_else(|| TurnId(format!("{}-turn-unknown", self.dialect.id_prefix())));
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
                PendingRequest::Plan {
                    rpc_id: request.id,
                    dialect: PlanRequestDialect::Cursor,
                },
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
                    dialect: UserInputDialect::Cursor,
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

    async fn handle_xai_question_request(&self, request: IncomingRequest) {
        let (questions, payload) = xai_questions(&request.params);
        if questions.is_empty() {
            let _ = self
                .child
                .respond(request.id, Ok(json!({ "outcome": "cancelled" })))
                .await;
            return;
        }

        let request_id = RequestId(self.opaque_id("question"));
        let turn_id = self
            .state
            .lock()
            .await
            .active_turn
            .clone()
            .unwrap_or_else(|| TurnId(format!("{}-turn-unknown", self.dialect.id_prefix())));
        {
            let mut state = self.state.lock().await;
            state.pending.insert(
                request_id.clone(),
                PendingRequest::UserInput {
                    rpc_id: request.id,
                    questions,
                    dialect: UserInputDialect::XAi,
                },
            );
            state.status = SessionStatus::WaitingApproval {
                request_id: request_id.clone(),
            };
        }
        let unwrapped = request.params.get("params").unwrap_or(&request.params);
        let tool_use_id = unwrapped
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

    async fn handle_xai_plan_request(&self, request: IncomingRequest) {
        let unwrapped = request.params.get("params").unwrap_or(&request.params);
        let payload = xai_plan_payload(&request.params);

        let request_id = RequestId(self.opaque_id("plan"));
        let turn_id = self
            .state
            .lock()
            .await
            .active_turn
            .clone()
            .unwrap_or_else(|| TurnId(format!("{}-turn-unknown", self.dialect.id_prefix())));
        {
            let mut state = self.state.lock().await;
            state.pending.insert(
                request_id.clone(),
                PendingRequest::Plan {
                    rpc_id: request.id,
                    dialect: PlanRequestDialect::XAi,
                },
            );
            state.status = SessionStatus::WaitingApproval {
                request_id: request_id.clone(),
            };
        }
        let tool_use_id = unwrapped
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let _ = self.event_tx.send(ProviderRuntimeEvent::RequestOpened {
            thread_id: self.thread_id.clone(),
            turn_id,
            request_id: request_id.clone(),
            request_kind: "plan".into(),
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
            "x.ai/session/prompt_complete" | "_x.ai/session/prompt_complete"
                if self.dialect.is_grok() =>
            {
                self.record_xai_prompt_completion(notification.params).await;
            }
            "x.ai/session/update"
            | "_x.ai/session/update"
            | "x.ai/session_notification"
            | "_x.ai/session_notification"
                if self.dialect.is_grok() =>
            {
                self.handle_xai_session_update(notification.params).await;
            }
            "x.ai/models/update" | "_x.ai/models/update" if self.dialect.is_grok() => {
                let mut state = self.state.lock().await;
                if let Some(model) = current_model_id(&notification.params) {
                    state.model = Some(model);
                }
                replace_grok_model_catalog(&mut state.grok_model_efforts, &notification.params);
            }
            "cursor/update_todos" if self.dialect == AcpDialect::Cursor => {
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
            "cursor/task" | "cursor/generate_image" if self.dialect == AcpDialect::Cursor => self
                .warn(
                    format!(
                        "Cursor ACP extension notification `{}` is not rendered yet",
                        notification.method
                    ),
                    Some(notification.params),
                ),
            // xAI extensions are explicitly forward-compatible. New model
            // catalogue notifications must never break a running session.
            method
                if self.dialect.is_grok()
                    && (method.starts_with("x.ai/") || method.starts_with("_x.ai/")) => {}
            _ => self.warn(
                format!(
                    "Unknown {} ACP notification `{}`",
                    self.dialect.label(),
                    notification.method
                ),
                Some(notification.params),
            ),
        }
    }

    async fn handle_session_update(&self, params: Value) {
        let update = params.get("update").unwrap_or(&params);
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .or_else(|| {
                if self.dialect.is_grok() {
                    update
                        .get("session_update")
                        .or_else(|| update.get("type"))
                        .and_then(Value::as_str)
                } else {
                    None
                }
            })
            .unwrap_or_default();
        if self.dialect.is_grok()
            && crate::agent_provider::grok::slash_commands::is_available_commands_update(&params)
        {
            if let Some(cache) = self.grok_slash_command_cache.as_ref() {
                cache.replace_from_value(&self.cwd, &params).await;
            }
            return;
        }
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
                    .unwrap_or("acp-tool");
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
                        .unwrap_or("acp-tool");
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
            "current_model_update" => {
                if let Some(model) = current_model_id(update).or_else(|| {
                    update
                        .get("modelId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                }) {
                    self.state.lock().await.model = Some(model);
                }
            }
            _ => self.warn(
                format!(
                    "Unknown {} ACP session update `{kind}`",
                    self.dialect.label()
                ),
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
        let (delta, model, context_events) = {
            let mut state = self.state.lock().await;
            let previous = state.usage_totals;
            state.usage_totals = totals;
            let delta = totals.delta_since(&previous);
            state
                .context
                .observe_max_tokens(context_window_from_value(update));
            // Cursor's report is cumulative for the session, which is
            // exactly what the lifetime accumulator wants.
            state.context.observe_lifetime_total(totals.total_tokens());
            // Occupancy is the LATEST request, never the session total:
            // `cache_read` re-counts the whole context on every request,
            // so summing it across the session would make the meter climb
            // monotonically and peg at the window. The per-frame delta is
            // one request's token count — the same quantity Codex reads
            // off its `last` breakdown.
            let events = state.context.events(
                &self.thread_id,
                delta.total_tokens(),
                None,
                // Cursor manages its own context window and exposes no
                // Codemux-side toggle for it.
                Some(true),
            );
            (delta, state.model.clone(), events)
        };
        for event in context_events {
            let _ = self.event_tx.send(event);
        }
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
                provider: self.dialect.provider(),
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
    }

    /// Record Grok's authoritative per-prompt bill and refresh the context
    /// snapshot from the response's last-call occupancy. `PromptUsage`
    /// includes child-agent work; Grok does not expose a stable per-child
    /// split here, so the complete aggregate is kept on the parent turn.
    async fn record_grok_turn_usage(
        &self,
        usage: Option<GrokPromptUsage>,
        live_tokens: Option<u64>,
    ) {
        let (model, context_events) = {
            let mut state = self.state.lock().await;
            if let Some(usage) = usage {
                state.context.add_processed(usage.tokens.total_tokens());
            }
            let used_tokens = live_tokens.unwrap_or_else(|| state.context.last_live_used());
            let events = state
                .context
                .events(&self.thread_id, used_tokens, None, Some(true));
            (state.model.clone(), events)
        };
        for event in context_events {
            let _ = self.event_tx.send(event);
        }

        let Some(usage) = usage.filter(|usage| !usage.tokens.is_empty()) else {
            return;
        };
        // Grok's wire bill is model-aware and authoritative. In particular,
        // `costUsdTicks` is scrubbed for partial/incomplete usage; estimating
        // that under-counted bill with a static table would turn "unknown"
        // into a deceptively precise value.
        let cost_usd = usage.cost_usd;
        let cost_source = cost_usd.map(|_| CostSource::Provider);
        let _ = self.event_tx.send(ProviderRuntimeEvent::UsageRecorded {
            thread_id: self.thread_id.clone(),
            provider: ProviderKind::Grok,
            model,
            subagent: false,
            input_tokens: usage.tokens.input,
            output_tokens: usage.tokens.output,
            cache_read_tokens: usage.tokens.cache_read,
            cache_write_tokens: usage.tokens.cache_write,
            reasoning_tokens: usage.reasoning,
            cost_usd,
            cost_source,
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

impl AcpUsageTotals {
    /// Every token in the record, summed.
    ///
    /// On a cumulative report this is the session's lifetime total; on a
    /// per-frame delta it is one request's token count — which, because
    /// Cursor re-reads the whole context from cache each request, is what
    /// currently occupies the context window.
    fn total_tokens(&self) -> u64 {
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

fn grok_prompt_usage_container(value: &Value) -> Option<&Value> {
    value
        .pointer("/_meta/usage")
        .or_else(|| value.get("usage"))
        .or_else(|| value.get("promptUsage"))
        .or_else(|| value.get("prompt_usage"))
        .or_else(|| value.pointer("/data/promptUsage"))
        .or_else(|| value.pointer("/data/prompt_usage"))
        .or_else(|| {
            value
                .get("inputTokens")
                .or_else(|| value.get("input_tokens"))
                .map(|_| value)
        })
}

/// Parse Grok's official `PromptUsage` and normalize its overlapping input
/// figure into Codemux's four disjoint buckets. ACP `inputTokens` includes
/// cache reads and cache creations; both are subtracted from fresh input.
fn grok_prompt_usage_from_value(value: &Value) -> Option<GrokPromptUsage> {
    let usage = grok_prompt_usage_container(value)?;
    let full_input = usage_number(usage, &["inputTokens", "input_tokens"]);
    let output = usage_number(usage, &["outputTokens", "output_tokens"]);
    let cache_read = usage_number(
        usage,
        &[
            "cachedReadTokens",
            "cached_read_tokens",
            "cacheReadInputTokens",
            "cache_read_input_tokens",
        ],
    )
    .unwrap_or(0);
    let cache_write = usage_number(
        usage,
        &[
            "cacheCreationTokens",
            "cache_creation_tokens",
            "cacheCreationInputTokens",
            "cache_creation_input_tokens",
        ],
    )
    .unwrap_or(0);
    if full_input.is_none() && output.is_none() && cache_read == 0 && cache_write == 0 {
        return None;
    }

    let incomplete = usage
        .get("usageIsIncomplete")
        .or_else(|| usage.get("usage_is_incomplete"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let cost_partial = usage
        .get("costIsPartial")
        .or_else(|| usage.get("cost_is_partial"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let cost_usd = (!incomplete && !cost_partial)
        .then(|| {
            usage
                .get("costUsdTicks")
                .or_else(|| usage.get("cost_usd_ticks"))
                .and_then(|ticks| ticks.as_i64().or_else(|| ticks.as_u64()?.try_into().ok()))
                .filter(|ticks| *ticks >= 0)
                .map(|ticks| ticks as f64 / 10_000_000_000.0)
        })
        .flatten();

    let full_input = full_input.unwrap_or(0);
    Some(GrokPromptUsage {
        tokens: AcpUsageTotals {
            input: full_input
                .saturating_sub(cache_read)
                .saturating_sub(cache_write),
            output: output.unwrap_or(0),
            cache_read,
            cache_write,
        },
        reasoning: usage_number(usage, &["reasoningTokens", "reasoning_tokens"]).unwrap_or(0),
        num_turns: usage_number(usage, &["numTurns", "num_turns"])
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32,
        api_duration_ms: usage_number(usage, &["apiDurationMs", "api_duration_ms"]).unwrap_or(0),
        cost_usd,
    })
}

/// Grok's xAI session envelope and standard prompt response expose current
/// context occupancy at `_meta.totalTokens`. Keep it distinct from
/// `usage.totalTokens`, which is an aggregate bill across model calls.
fn xai_live_total_tokens(value: &Value) -> Option<u64> {
    value
        .pointer("/_meta/totalTokens")
        .or_else(|| value.pointer("/_meta/total_tokens"))
        .and_then(Value::as_u64)
        .filter(|tokens| *tokens > 0)
}

/// Parse a `usage_update` payload into cumulative token totals, or `None`
/// when the frame carries nothing recognizable. Deliberately tolerant:
/// the extension is undocumented and a shape change must degrade to "no
/// usage reported", never to invented numbers.
fn usage_totals_from_value(value: &Value) -> Option<AcpUsageTotals> {
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
    Some(AcpUsageTotals {
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
fn turn_status_from_result(result: &Value, dialect: AcpDialect) -> TurnStatus {
    match result.get("stopReason").and_then(Value::as_str) {
        Some("error") => TurnStatus::Error {
            subtype: format!("{}_acp", dialect.id_prefix()),
            message: agent_result_message(result)
                .unwrap_or_else(|| format!("{} turn failed", dialect.label())),
        },
        Some("rate_limit") => TurnStatus::Error {
            subtype: "rate_limit".into(),
            message: agent_result_message(result).unwrap_or_else(|| {
                format!("{} was rate limited; try again later", dialect.label())
            }),
        },
        stop_reason => turn_status_from_stop_reason(stop_reason, dialect),
    }
}

fn turn_status_from_rpc_error(error: &RpcChildError, dialect: AcpDialect) -> TurnStatus {
    let detail = match error {
        RpcChildError::RpcError(error) => rpc_error_detail(error),
        _ => None,
    };
    let message = detail.unwrap_or_else(|| error.to_string());
    let lower = message.to_ascii_lowercase();
    let rate_limited = lower.contains("rate limit")
        || matches!(error, RpcChildError::RpcError(error) if error.data.as_ref().is_some_and(|data| {
            data.get("http_status").and_then(Value::as_u64) == Some(429)
                || data.get("httpStatus").and_then(Value::as_u64) == Some(429)
        }));
    TurnStatus::Error {
        subtype: if rate_limited {
            "rate_limit".into()
        } else {
            format!("{}_acp", dialect.id_prefix())
        },
        message,
    }
}

fn rpc_error_detail(error: &RpcError) -> Option<String> {
    let data = error.data.as_ref()?;
    data.as_str()
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
        .or_else(|| {
            data.get("message")
                .or_else(|| data.get("detail"))
                .or_else(|| data.get("agentResult"))
                .or_else(|| data.get("agent_result"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|message| !message.is_empty())
                .map(str::to_string)
        })
}

fn agent_result_message(result: &Value) -> Option<String> {
    let value = result
        .pointer("/_meta/agentResult")
        .or_else(|| result.get("agentResult"))?;
    value
        .as_str()
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
        .or_else(|| {
            value
                .get("message")
                .or_else(|| value.get("detail"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|message| !message.is_empty())
                .map(str::to_string)
        })
}

fn turn_status_from_stop_reason(stop_reason: Option<&str>, dialect: AcpDialect) -> TurnStatus {
    match stop_reason {
        Some("cancelled") => TurnStatus::Error {
            subtype: "interrupted".into(),
            message: format!("{} turn was interrupted", dialect.label()),
        },
        // Both caps are "the runtime stopped this turn short", which is
        // exactly what MaxTurns renders.
        Some("max_tokens" | "max_turn_requests") => TurnStatus::MaxTurns,
        Some("refusal") => TurnStatus::Error {
            subtype: "refusal".into(),
            message: format!("{} declined to continue this turn", dialect.label()),
        },
        _ => TurnStatus::Success,
    }
}

fn normalize_xai_stop_reason(stop_reason: Option<&str>) -> String {
    stop_reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(|reason| reason.to_ascii_lowercase())
        .unwrap_or_else(|| "end_turn".into())
}

fn xai_string_field(value: &Value, camel: &str, snake: &str) -> Option<String> {
    value
        .get(camel)
        .or_else(|| value.get(snake))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn grok_reasoning_effort(value: &Value) -> Option<String> {
    value
        .pointer("/_meta/modelState/reasoningEffort")
        .or_else(|| value.pointer("/modelState/reasoningEffort"))
        .or_else(|| value.pointer("/_meta/reasoningEffort"))
        .or_else(|| value.get("reasoningEffort"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn xai_model_changed(value: &Value) -> Option<(String, Option<String>)> {
    let update = value.get("update").unwrap_or(value);
    if xai_string_field(update, "sessionUpdate", "type").as_deref() != Some("model_changed") {
        return None;
    }
    let model = xai_string_field(update, "modelId", "model_id")?;
    let effort = xai_string_field(update, "reasoningEffort", "reasoning_effort");
    Some((model, effort))
}

fn xai_completion_value(value: &Value, session_id: &str, prompt_id: &str) -> Option<Value> {
    let stop_reason = xai_string_field(value, "stopReason", "stop_reason")?;
    let mut meta = json!({
        "sessionId": session_id,
        "promptId": prompt_id,
        "requestId": prompt_id
    });
    if let Some(agent_result) = value
        .get("agentResult")
        .or_else(|| value.get("agent_result"))
    {
        meta["agentResult"] = agent_result.clone();
    }
    if let Some(usage) = grok_prompt_usage_container(value) {
        meta["usage"] = usage.clone();
    }
    if let Some(total_tokens) = xai_live_total_tokens(value) {
        meta["totalTokens"] = total_tokens.into();
    }
    Some(json!({
        "stopReason": normalize_xai_stop_reason(Some(&stop_reason)),
        "_meta": meta
    }))
}

/// Validate and translate Grok's durable live/replay turn terminal. Both
/// `_x.ai/session_notification` (live) and `_x.ai/session/update` (replay)
/// carry this same envelope. Foreign session/prompt IDs are ignored so a
/// delayed update can never finish a newer Codemux turn.
fn xai_terminal_completion(
    params: &Value,
    expected_session_id: &str,
    expected_prompt_id: &str,
    announced: Option<Value>,
) -> Option<Value> {
    let update = params.get("update").unwrap_or(params);
    if xai_string_field(update, "sessionUpdate", "type").as_deref() != Some("turn_completed") {
        return None;
    }
    let session_id = xai_string_field(params, "sessionId", "session_id")
        .or_else(|| xai_string_field(update, "sessionId", "session_id"))?;
    let prompt_id = xai_string_field(update, "promptId", "prompt_id")
        .or_else(|| xai_string_field(params, "promptId", "prompt_id"))?;
    if session_id != expected_session_id || prompt_id != expected_prompt_id {
        return None;
    }

    let mut completion = xai_completion_value(update, &session_id, &prompt_id)
        .or(announced)
        .unwrap_or_else(|| {
            json!({
                "stopReason": "end_turn",
                "_meta": {
                    "sessionId": session_id,
                    "promptId": prompt_id,
                    "requestId": prompt_id
                }
            })
        });
    if let Some(total_tokens) = xai_live_total_tokens(params) {
        completion["_meta"]["totalTokens"] = total_tokens.into();
    }
    Some(completion)
}

/// Prefer ACP's standard `session/prompt` response while retaining Grok's
/// durable `turn_completed` notification as a compatibility fallback.
///
/// The fallback starts a grace period rather than completing immediately:
/// current Grok builds can emit their durable lifecycle event while the
/// standard RPC response is still being finalized. Keeping this arbitration
/// independent from the subprocess makes the ordering contract directly
/// testable and leaves [`AcpSession::send_prompt`] responsible only for Stop
/// forwarding.
async fn prefer_standard_prompt_response<R, F>(
    request: R,
    fallback: F,
    fallback_grace: Duration,
) -> Result<Value, RpcChildError>
where
    R: std::future::Future<Output = Result<Value, RpcChildError>>,
    F: std::future::Future<Output = Option<Value>>,
{
    tokio::pin!(request);
    tokio::pin!(fallback);

    let completion = tokio::select! {
        biased;
        response = &mut request => return response,
        completion = &mut fallback => completion,
    };
    let Some(completion) = completion else {
        // Cursor and a closed Grok fallback channel both use the standard
        // response exclusively.
        return request.await;
    };

    tokio::select! {
        biased;
        response = &mut request => response,
        () = tokio::time::sleep(fallback_grace) => Ok(completion),
    }
}

/// The JSON-RPC response body for `pending` under `decision`.
fn pending_response(pending: &PendingRequest, decision: &ApprovalDecision) -> Value {
    match pending {
        PendingRequest::Permission { options, .. } => {
            json!({ "outcome": permission_outcome(options, decision) })
        }
        PendingRequest::UserInput {
            questions,
            dialect: UserInputDialect::Cursor,
            ..
        } => cursor_question_response(questions, decision),
        PendingRequest::UserInput {
            questions,
            dialect: UserInputDialect::XAi,
            ..
        } => xai_question_response(questions, decision),
        PendingRequest::Plan {
            dialect: PlanRequestDialect::Cursor,
            ..
        } => cursor_plan_response(decision),
        PendingRequest::Plan {
            dialect: PlanRequestDialect::XAi,
            ..
        } => xai_plan_response(decision),
    }
}

fn pending_rpc_id(pending: PendingRequest) -> Value {
    match pending {
        PendingRequest::Permission { rpc_id, .. }
        | PendingRequest::UserInput { rpc_id, .. }
        | PendingRequest::Plan { rpc_id, .. } => rpc_id,
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

fn new_session_setup(
    response: Value,
    dialect: AcpDialect,
) -> Result<(Value, String), ProviderError> {
    let provider_session_id = session_id(&response).ok_or_else(|| ProviderError::RpcError {
        message: format!("{} ACP session/new returned no sessionId", dialect.label()),
    })?;
    Ok((response, provider_session_id))
}

fn loaded_session_setup(response: Value, requested_session_id: String) -> (Value, String) {
    (response, requested_session_id)
}

fn base_model_id(model: &str) -> &str {
    model.split_once('[').map_or(model, |(base, _)| base).trim()
}

/// Reconcile preferences saved by an older Codemux session against Grok's
/// initialize-time catalogue. A non-empty catalogue is authoritative for
/// removals; an empty one remains permissive for older/future ACP builds that
/// do not expose the extension.
fn reconcile_grok_startup_config(
    catalog: &GrokModelEffortCatalog,
    provider_default: Option<&str>,
    requested_model: Option<&str>,
    requested_effort: Option<&str>,
) -> (Option<String>, Option<String>) {
    let requested_model = requested_model
        .map(base_model_id)
        .filter(|model| !model.is_empty() && *model != "default");
    let provider_default = provider_default
        .map(str::trim)
        .filter(|model| !model.is_empty());
    let model = match requested_model {
        Some(model) if catalog.is_empty() || catalog.contains_key(model) => Some(model.to_string()),
        Some(_) => provider_default
            .filter(|model| catalog.contains_key(*model))
            .map(str::to_string),
        // With no explicit preference, keep `session/new` model-free so a
        // resumed session retains its provider-owned model selection.
        None => None,
    };
    let effort = model
        .as_deref()
        .and_then(|model| reconcile_grok_effort(catalog, model, requested_effort));
    (model, effort)
}

fn reconcile_grok_effort(
    catalog: &GrokModelEffortCatalog,
    model: &str,
    requested: Option<&str>,
) -> Option<String> {
    let requested = requested.map(str::trim).filter(|value| !value.is_empty())?;
    match catalog.get(model) {
        Some(Some(supported)) if !supported.contains(requested) => None,
        // Missing metadata is deliberately permissive: model and effort ids
        // are provider-owned and future values must work without a release.
        _ => Some(requested.to_string()),
    }
}

/// `x.ai/models/update` is a full SessionModelState snapshot. Replace a
/// non-empty catalogue instead of merging it, otherwise retired models remain
/// selectable forever. An empty/unrecognized payload is ignored for forward
/// compatibility rather than erasing the last authoritative state.
fn replace_grok_model_catalog(catalog: &mut GrokModelEffortCatalog, update: &Value) -> bool {
    let replacement = grok_model_effort_catalog(update);
    if replacement.is_empty() {
        return false;
    }
    *catalog = replacement;
    true
}

fn cursor_questions(params: &Value) -> (Vec<AcpQuestion>, Value) {
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
                Some(AcpQuestionOption {
                    id: id.to_string(),
                    label: label.to_string(),
                })
            })
            .collect::<Vec<_>>();
        questions.push(AcpQuestion {
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

fn xai_questions(params: &Value) -> (Vec<AcpQuestion>, Value) {
    let params = params.get("params").unwrap_or(params);
    let Some(raw_questions) = params.get("questions").and_then(Value::as_array) else {
        return (vec![], json!({ "questions": [] }));
    };
    let mut questions = Vec::new();
    let mut ui_questions = Vec::new();
    for raw in raw_questions {
        let Some(prompt) = raw
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(prompt);
        let raw_options = raw
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut options = raw_options
            .iter()
            .filter_map(|option| {
                let label = option
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|label| !label.is_empty())?;
                let id = option
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .unwrap_or(label);
                Some(AcpQuestionOption {
                    id: id.to_string(),
                    label: label.to_string(),
                })
            })
            .collect::<Vec<_>>();
        if options.is_empty() {
            options.push(AcpQuestionOption {
                id: "OK".into(),
                label: "OK".into(),
            });
        }
        let ui_options = options
            .iter()
            .map(|option| {
                let description = raw_options
                    .iter()
                    .find(|raw| {
                        raw.get("label").and_then(Value::as_str) == Some(option.label.as_str())
                    })
                    .and_then(|raw| raw.get("description"))
                    .and_then(Value::as_str)
                    .unwrap_or(option.label.as_str());
                json!({
                    "label": option.label,
                    "description": description
                })
            })
            .collect::<Vec<_>>();
        questions.push(AcpQuestion {
            id: id.to_string(),
            prompt: prompt.to_string(),
            options,
        });
        ui_questions.push(json!({
            "id": id,
            "header": "Question",
            "question": prompt,
            "multiSelect": raw.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
            "allowOther": true,
            "options": ui_options
        }));
    }
    (questions, json!({ "questions": ui_questions }))
}

fn cursor_question_response(questions: &[AcpQuestion], decision: &ApprovalDecision) -> Value {
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

fn xai_question_response(questions: &[AcpQuestion], decision: &ApprovalDecision) -> Value {
    let ApprovalDecision::Allow {
        updated_input: Some(input),
        ..
    } = decision
    else {
        return json!({ "outcome": "cancelled" });
    };
    let Some(raw_answers) = input.get("answers").and_then(Value::as_object) else {
        return json!({ "outcome": "cancelled" });
    };
    let mut answers = serde_json::Map::new();
    let mut annotations = serde_json::Map::new();
    for question in questions {
        let Some(answer) = raw_answers
            .get(&question.id)
            .or_else(|| raw_answers.get(&question.prompt))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let mut labels = Vec::new();
        let mut notes = Vec::new();
        let exact = question.options.iter().find(|option| {
            option.id.eq_ignore_ascii_case(answer.trim())
                || option.label.eq_ignore_ascii_case(answer.trim())
        });
        let values = exact.map_or_else(
            || {
                answer
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
            },
            |_| vec![answer.trim()],
        );
        for value in values {
            if let Some(option) = question.options.iter().find(|option| {
                option.id.eq_ignore_ascii_case(value) || option.label.eq_ignore_ascii_case(value)
            }) {
                labels.push(Value::String(option.label.clone()));
            } else {
                notes.push(value.to_string());
            }
        }
        if labels.is_empty() && !notes.is_empty() {
            labels.push(Value::String("Other".into()));
        }
        if !labels.is_empty() {
            answers.insert(question.prompt.clone(), Value::Array(labels));
        }
        if !notes.is_empty() {
            annotations.insert(
                question.prompt.clone(),
                json!({ "notes": notes.join("\n") }),
            );
        }
    }
    let mut response = json!({
        "outcome": "accepted",
        "answers": Value::Object(answers)
    });
    if !annotations.is_empty() {
        response["annotations"] = Value::Object(annotations);
    }
    response
}

fn cursor_question_answers(questions: &[AcpQuestion], decision: &ApprovalDecision) -> Vec<Value> {
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

fn xai_plan_payload(params: &Value) -> Value {
    let mut payload = params.get("params").unwrap_or(params).clone();
    if let Some(object) = payload.as_object_mut() {
        let plan = object
            .get("planContent")
            .or_else(|| object.get("plan"))
            .or_else(|| object.get("input").and_then(|input| input.get("plan")))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|plan| !plan.is_empty())
            .map(str::to_string);
        if let Some(plan) = plan {
            object.insert("plan".into(), Value::String(plan));
        }
    }
    payload
}

fn xai_plan_response(decision: &ApprovalDecision) -> Value {
    match decision {
        ApprovalDecision::Allow { .. } | ApprovalDecision::AllowForSession => {
            json!({ "outcome": "approved" })
        }
        ApprovalDecision::Deny { message } => json!({
            "outcome": "cancelled",
            "feedback": message
        }),
        ApprovalDecision::Cancel => json!({ "outcome": "abandoned" }),
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

fn map_spawn_error(error: RpcChildError, dialect: AcpDialect) -> ProviderError {
    match error {
        RpcChildError::SpawnFailed(source) if source.kind() == std::io::ErrorKind::NotFound => {
            ProviderError::NotInstalled {
                provider: dialect.provider(),
                hint: match dialect {
                    AcpDialect::Cursor => {
                        "Install Cursor Agent and ensure `cursor-agent` is on PATH.".into()
                    }
                    AcpDialect::Grok => {
                        "Install the official Grok CLI and ensure `grok` is on PATH.".into()
                    }
                },
            }
        }
        other => ProviderError::ProcessError {
            message: format!("Could not start {} ACP", dialect.label()),
            source: Some(other.to_string()),
        },
    }
}

fn map_auth_error(error: RpcChildError, dialect: AcpDialect) -> ProviderError {
    let message = error.to_string();
    if looks_unauthenticated(&message) {
        ProviderError::NotAuthenticated {
            provider: dialect.provider(),
            hint: match dialect {
                AcpDialect::Cursor => "Run `cursor-agent login` and try again.".into(),
                AcpDialect::Grok => {
                    "Run `grok login --device-auth` (or set `XAI_API_KEY`) and try again.".into()
                }
            },
        }
    } else {
        ProviderError::RpcError { message }
    }
}

fn map_rpc_error(error: RpcChildError, dialect: AcpDialect) -> ProviderError {
    match error {
        RpcChildError::Timeout { method, elapsed } => ProviderError::Timeout {
            operation: method,
            elapsed_ms: elapsed.as_millis() as u64,
        },
        RpcChildError::SpawnFailed(_)
        | RpcChildError::ChildExited { .. }
        | RpcChildError::IoError(_)
        | RpcChildError::AlreadyShutdown => ProviderError::ProcessError {
            message: format!("{} ACP process failed", dialect.label()),
            source: Some(error.to_string()),
        },
        other => ProviderError::RpcError {
            message: other.to_string(),
        },
    }
}

fn map_grok_model_error(error: RpcChildError, dialect: AcpDialect) -> ProviderError {
    let incompatible = match &error {
        RpcChildError::RpcError(rpc) => {
            rpc.message.contains("MODEL_SWITCH_INCOMPATIBLE_AGENT")
                || rpc.data.as_ref().is_some_and(|data| {
                    value_contains_text(data, "MODEL_SWITCH_INCOMPATIBLE_AGENT")
                })
        }
        other => other
            .to_string()
            .contains("MODEL_SWITCH_INCOMPATIBLE_AGENT"),
    };
    if incompatible {
        ProviderError::ValidationError {
            message: "grok_model_restart_required: Grok cannot switch between those model families inside this ACP session. Start a new session with the selected model."
                .into(),
        }
    } else {
        map_rpc_error(error, dialect)
    }
}

fn value_contains_text(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_text(value, needle)),
        Value::Object(values) => values
            .iter()
            .any(|(key, value)| key.contains(needle) || value_contains_text(value, needle)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_incompatible_model_marker_is_read_from_structured_error_data() {
        let error = RpcChildError::RpcError(RpcError {
            code: -32603,
            message: "Internal error".into(),
            data: Some(json!({
                "cause": {
                    "code": "MODEL_SWITCH_INCOMPATIBLE_AGENT"
                }
            })),
        });
        let mapped = map_grok_model_error(error, AcpDialect::Grok);
        assert!(matches!(
            mapped,
            ProviderError::ValidationError { message }
                if message.starts_with("grok_model_restart_required:")
        ));
    }

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
        let questions = vec![AcpQuestion {
            id: "language".into(),
            prompt: "Which language?".into(),
            options: vec![
                AcpQuestionOption {
                    id: "rust".into(),
                    label: "Rust".into(),
                },
                AcpQuestionOption {
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
    fn xai_questions_and_answers_use_live_labels_without_hardcoded_choices() {
        let (questions, payload) = xai_questions(&json!({
            "method": "x.ai/ask_user_question",
            "params": {
                "questions": [{
                    "question": "Which scope should Grok use?",
                    "multiSelect": false,
                    "options": [{
                        "id": "workspace",
                        "label": "Workspace",
                        "description": "Use the current workspace"
                    }]
                }]
            }
        }));
        assert_eq!(payload["questions"][0]["allowOther"], true);
        assert_eq!(
            payload["questions"][0]["options"][0]["description"],
            "Use the current workspace"
        );
        let decision = ApprovalDecision::Allow {
            updated_input: Some(json!({
                "answers": {"Which scope should Grok use?": "Workspace"}
            })),
            updated_permissions: None,
        };
        assert_eq!(
            xai_question_response(&questions, &decision),
            json!({
                "outcome": "accepted",
                "answers": {"Which scope should Grok use?": ["Workspace"]}
            })
        );
    }

    #[test]
    fn xai_free_text_answers_are_returned_as_other_annotations() {
        let questions = vec![AcpQuestion {
            id: "scope".into(),
            prompt: "Which scope?".into(),
            options: vec![AcpQuestionOption {
                id: "workspace".into(),
                label: "Workspace".into(),
            }],
        }];
        let decision = ApprovalDecision::Allow {
            updated_input: Some(json!({ "answers": {"scope": "Only src/"} })),
            updated_permissions: None,
        };
        assert_eq!(
            xai_question_response(&questions, &decision),
            json!({
                "outcome": "accepted",
                "answers": {"Which scope?": ["Other"]},
                "annotations": {"Which scope?": {"notes": "Only src/"}}
            })
        );
    }

    #[test]
    fn questionnaire_preserves_single_option_labels_containing_commas() {
        let questions = vec![AcpQuestion {
            id: "stack".into(),
            prompt: "Which stack?".into(),
            options: vec![AcpQuestionOption {
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
            turn_status_from_stop_reason(Some("end_turn"), AcpDialect::Cursor),
            TurnStatus::Success
        ));
        assert!(matches!(
            turn_status_from_stop_reason(Some("max_tokens"), AcpDialect::Cursor),
            TurnStatus::MaxTurns
        ));
        // Neither of these is a successful turn, and reporting them as one
        // told the transcript the model had finished its work.
        assert!(matches!(
            turn_status_from_stop_reason(Some("max_turn_requests"), AcpDialect::Cursor),
            TurnStatus::MaxTurns
        ));
        assert!(matches!(
            turn_status_from_stop_reason(Some("refusal"), AcpDialect::Cursor),
            TurnStatus::Error { ref subtype, .. } if subtype == "refusal"
        ));
        assert!(matches!(
            turn_status_from_stop_reason(Some("cancelled"), AcpDialect::Cursor),
            TurnStatus::Error { ref subtype, .. } if subtype == "interrupted"
        ));
        assert!(matches!(
            turn_status_from_result(
                &json!({
                    "stopReason": "error",
                    "_meta": {"agentResult": "upstream unavailable"}
                }),
                AcpDialect::Grok
            ),
            TurnStatus::Error { ref subtype, ref message }
                if subtype == "grok_acp" && message == "upstream unavailable"
        ));
        assert!(matches!(
            turn_status_from_result(
                &json!({"stopReason": "rate_limit"}),
                AcpDialect::Grok
            ),
            TurnStatus::Error { ref subtype, .. } if subtype == "rate_limit"
        ));
        assert!(matches!(
            turn_status_from_rpc_error(
                &RpcChildError::RpcError(RpcError {
                    code: -32603,
                    message: "Internal error".into(),
                    data: Some(json!({"message": "Account credits exhausted"})),
                }),
                AcpDialect::Grok,
            ),
            TurnStatus::Error { ref subtype, ref message }
                if subtype == "grok_acp" && message == "Account credits exhausted"
        ));
        assert!(matches!(
            turn_status_from_rpc_error(
                &RpcChildError::RpcError(RpcError {
                    code: -32603,
                    message: "Internal error".into(),
                    data: Some(json!({"http_status": 429, "message": "Too many requests"})),
                }),
                AcpDialect::Grok,
            ),
            TurnStatus::Error { ref subtype, .. } if subtype == "rate_limit"
        ));
    }

    #[test]
    fn exact_xai_live_terminal_requires_matching_session_and_prompt_ids() {
        let terminal = json!({
            "sessionId": "session-1",
            "update": {
                "sessionUpdate": "turn_completed",
                "prompt_id": "prompt-1",
                "stop_reason": "error",
                "agent_result": "upstream unavailable",
                "usage": {
                    "inputTokens": 100,
                    "cachedReadTokens": 40,
                    "outputTokens": 10
                }
            },
            "_meta": {"totalTokens": 77}
        });
        let announced = json!({"stopReason": "end_turn"});
        let completion =
            xai_terminal_completion(&terminal, "session-1", "prompt-1", Some(announced))
                .expect("matching durable terminal");
        assert_eq!(completion["stopReason"], "error");
        assert_eq!(completion["_meta"]["agentResult"], "upstream unavailable");
        assert_eq!(completion["_meta"]["usage"]["inputTokens"], 100);
        assert_eq!(completion["_meta"]["totalTokens"], 77);

        assert!(xai_terminal_completion(&terminal, "other-session", "prompt-1", None).is_none());
        assert!(xai_terminal_completion(&terminal, "session-1", "other-prompt", None).is_none());
        assert!(xai_terminal_completion(
            &json!({
                "sessionId": "session-1",
                "promptId": "prompt-1",
                "stopReason": "end_turn"
            }),
            "session-1",
            "prompt-1",
            None,
        )
        .is_none());
    }

    #[test]
    fn xai_model_changed_captures_the_provider_effective_effort() {
        assert_eq!(
            xai_model_changed(&json!({
                "sessionId": "session-1",
                "update": {
                    "sessionUpdate": "model_changed",
                    "modelId": "grok-4.6",
                    "reasoningEffort": "high"
                }
            })),
            Some(("grok-4.6".into(), Some("high".into())))
        );
        assert_eq!(
            xai_model_changed(&json!({
                "update": {
                    "type": "model_changed",
                    "model_id": "grok-future",
                    "reasoning_effort": "adaptive"
                }
            })),
            Some(("grok-future".into(), Some("adaptive".into())))
        );
    }

    #[tokio::test]
    async fn standard_prompt_response_wins_inside_the_grok_terminal_grace() {
        let (request_tx, request_rx) = oneshot::channel();
        let (fallback_tx, fallback_rx) = oneshot::channel();
        let task = tokio::spawn(prefer_standard_prompt_response(
            async move { request_rx.await.expect("request sender") },
            async move { fallback_rx.await.ok() },
            Duration::from_millis(250),
        ));

        fallback_tx
            .send(json!({"stopReason": "error"}))
            .expect("fallback receiver");
        tokio::task::yield_now().await;
        request_tx
            .send(Ok(json!({"stopReason": "end_turn", "source": "standard"})))
            .expect("request receiver");

        let response = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("arbiter timeout")
            .expect("arbiter task")
            .expect("standard response");
        assert_eq!(response["source"], "standard");
        assert_eq!(response["stopReason"], "end_turn");
    }

    #[tokio::test]
    async fn durable_grok_terminal_completes_when_the_standard_response_is_omitted() {
        let (_request_tx, request_rx) = oneshot::channel();
        let (fallback_tx, fallback_rx) = oneshot::channel();
        let task = tokio::spawn(prefer_standard_prompt_response(
            async move { request_rx.await.expect("request sender remains live") },
            async move { fallback_rx.await.ok() },
            Duration::from_millis(10),
        ));
        fallback_tx
            .send(json!({
                "stopReason": "error",
                "_meta": {
                    "agentResult": "provider detail",
                    "usage": {"inputTokens": 9, "outputTokens": 1}
                }
            }))
            .expect("fallback receiver");

        let response = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("arbiter timeout")
            .expect("arbiter task")
            .expect("durable fallback");
        assert_eq!(response["stopReason"], "error");
        assert_eq!(response["_meta"]["agentResult"], "provider detail");
        assert_eq!(response["_meta"]["usage"]["inputTokens"], 9);
    }

    #[test]
    fn grok_prompt_usage_normalizes_cache_and_preserves_reasoning_and_cost() {
        let usage = grok_prompt_usage_from_value(&json!({
            "stopReason": "end_turn",
            "_meta": {
                "totalTokens": 42_000,
                "usage": {
                    "inputTokens": 10_000,
                    "outputTokens": 800,
                    "totalTokens": 10_800,
                    "cachedReadTokens": 4_000,
                    "cacheCreationTokens": 1_000,
                    "reasoningTokens": 250,
                    "numTurns": 3,
                    "apiDurationMs": 1_234,
                    "costUsdTicks": 2_500_000_000_i64
                }
            }
        }))
        .expect("official PromptUsage");
        assert_eq!(usage.tokens.input, 5_000);
        assert_eq!(usage.tokens.cache_read, 4_000);
        assert_eq!(usage.tokens.cache_write, 1_000);
        assert_eq!(usage.tokens.output, 800);
        assert_eq!(usage.reasoning, 250);
        assert_eq!(usage.num_turns, 3);
        assert_eq!(usage.api_duration_ms, 1_234);
        assert_eq!(usage.cost_usd, Some(0.25));
        assert_eq!(
            xai_live_total_tokens(&json!({
                "_meta": {
                    "totalTokens": 42_000,
                    "usage": {"totalTokens": 10_800}
                }
            })),
            Some(42_000)
        );
    }

    #[test]
    fn grok_prompt_usage_accepts_snake_case_and_fails_closed_on_partial_cost() {
        let usage = grok_prompt_usage_from_value(&json!({
            "promptUsage": {
                "input_tokens": 50,
                "output_tokens": 5,
                "cached_read_tokens": 20,
                "cache_creation_tokens": 10,
                "reasoning_tokens": 2,
                "cost_usd_ticks": 9_000_000_000_i64,
                "cost_is_partial": true
            }
        }))
        .expect("snake-case PromptUsage");
        assert_eq!(usage.tokens.input, 20);
        assert_eq!(usage.tokens.total_tokens(), 55);
        assert_eq!(usage.reasoning, 2);
        assert_eq!(usage.cost_usd, None);
    }

    #[test]
    fn durable_grok_terminal_normalizes_usage_but_keeps_envelope_context() {
        let terminal = json!({
            "sessionId": "session-1",
            "update": {
                "sessionUpdate": "turn_completed",
                "prompt_id": "prompt-1",
                "stop_reason": "end_turn",
                "usage": {
                    "inputTokens": 1_000,
                    "outputTokens": 100,
                    "totalTokens": 1_100,
                    "cachedReadTokens": 400,
                    "cacheCreationTokens": 100,
                    "reasoningTokens": 25,
                    "numTurns": 2,
                    "apiDurationMs": 750,
                    "costUsdTicks": 500_000_000_i64
                }
            },
            "_meta": {"totalTokens": 24_000}
        });

        let completion = xai_terminal_completion(&terminal, "session-1", "prompt-1", None)
            .expect("matching durable terminal");
        let usage = grok_prompt_usage_from_value(&completion).expect("terminal PromptUsage");
        assert_eq!(
            usage.tokens,
            AcpUsageTotals {
                input: 500,
                output: 100,
                cache_read: 400,
                cache_write: 100,
            }
        );
        assert_eq!(usage.reasoning, 25);
        assert_eq!(usage.num_turns, 2);
        assert_eq!(usage.api_duration_ms, 750);
        assert_eq!(usage.cost_usd, Some(0.05));
        assert_eq!(xai_live_total_tokens(&completion), Some(24_000));

        let mut context = ContextUsageTracker::default();
        context.add_processed(usage.tokens.total_tokens());
        let snapshot = context
            .snapshot(
                xai_live_total_tokens(&completion).expect("live occupancy"),
                None,
                Some(true),
            )
            .expect("context snapshot");
        assert_eq!(snapshot.used_tokens, 24_000);
        assert_eq!(snapshot.total_processed_tokens, None);
        assert_eq!(snapshot.compacts_automatically, Some(true));
    }

    #[test]
    fn queued_grok_effort_reconciles_against_the_latest_live_catalog() {
        let catalog = GrokModelEffortCatalog::from([
            (
                "grok-live".into(),
                Some(["high".to_string()].into_iter().collect()),
            ),
            ("grok-opaque".into(), None),
        ]);
        assert_eq!(
            reconcile_grok_effort(&catalog, "grok-live", Some("retired")),
            None
        );
        assert_eq!(
            reconcile_grok_effort(&catalog, "grok-live", Some("high")).as_deref(),
            Some("high")
        );
        assert_eq!(
            reconcile_grok_effort(&catalog, "grok-opaque", Some("future/deep")).as_deref(),
            Some("future/deep")
        );
        assert_eq!(
            reconcile_grok_effort(&catalog, "grok-new", Some("future/deep")).as_deref(),
            Some("future/deep")
        );
    }

    #[test]
    fn grok_models_update_replaces_catalog_and_removes_retired_models() {
        let mut catalog = GrokModelEffortCatalog::from([
            ("grok-retired".into(), None),
            (
                "grok-live".into(),
                Some(["old-effort".to_string()].into_iter().collect()),
            ),
        ]);
        assert!(replace_grok_model_catalog(
            &mut catalog,
            &json!({
                "_meta": {
                    "modelState": {
                        "currentModelId": "grok-live",
                        "availableModels": [{
                            "modelId": "grok-live",
                            "_meta": { "reasoningEfforts": ["high", "future/deep"] }
                        }]
                    }
                }
            }),
        ));

        assert!(!catalog.contains_key("grok-retired"));
        assert_eq!(
            catalog.get("grok-live"),
            Some(&Some(
                ["high".to_string(), "future/deep".to_string()]
                    .into_iter()
                    .collect()
            ))
        );
    }

    #[test]
    fn retired_grok_startup_config_falls_back_to_the_live_default() {
        let catalog = GrokModelEffortCatalog::from([(
            "grok-live".into(),
            Some(
                ["low".to_string(), "high".to_string()]
                    .into_iter()
                    .collect(),
            ),
        )]);
        assert_eq!(
            reconcile_grok_startup_config(
                &catalog,
                Some("grok-live"),
                Some("grok-retired[legacy]"),
                Some("retired-effort"),
            ),
            (Some("grok-live".into()), None),
        );
        assert_eq!(
            reconcile_grok_startup_config(
                &catalog,
                Some("grok-live"),
                Some("grok-live"),
                Some("high"),
            ),
            (Some("grok-live".into()), Some("high".into())),
        );

        // Missing model metadata is not evidence of removal. Preserve opaque
        // future model/effort ids for older or newer extension variants.
        assert_eq!(
            reconcile_grok_startup_config(
                &GrokModelEffortCatalog::new(),
                None,
                Some("grok-future"),
                Some("adaptive/deep"),
            ),
            (Some("grok-future".into()), Some("adaptive/deep".into())),
        );
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
                    dialect: UserInputDialect::Cursor,
                },
                &ApprovalDecision::Cancel,
            ),
            json!({"outcome": {"outcome": "cancelled"}})
        );
        assert_eq!(
            pending_response(
                &PendingRequest::Plan {
                    rpc_id: json!(9),
                    dialect: PlanRequestDialect::Cursor,
                },
                &ApprovalDecision::Cancel
            ),
            json!({"outcome": {"outcome": "cancelled"}})
        );
        assert_eq!(
            pending_rpc_id(PendingRequest::Plan {
                rpc_id: json!(9),
                dialect: PlanRequestDialect::XAi,
            }),
            json!(9)
        );
    }

    #[test]
    fn xai_plan_decisions_use_the_official_flat_outcomes() {
        assert_eq!(
            xai_plan_response(&ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            }),
            json!({"outcome": "approved"})
        );
        assert_eq!(
            xai_plan_response(&ApprovalDecision::Deny {
                message: "Please cover rollback.".into(),
            }),
            json!({
                "outcome": "cancelled",
                "feedback": "Please cover rollback."
            })
        );
        assert_eq!(
            xai_plan_response(&ApprovalDecision::Cancel),
            json!({"outcome": "abandoned"})
        );
        assert_eq!(
            cursor_plan_response(&ApprovalDecision::Cancel),
            json!({"outcome": {"outcome": "cancelled"}})
        );
    }

    #[test]
    fn xai_exit_plan_payload_exposes_plan_content_without_losing_wire_fields() {
        let payload = xai_plan_payload(&json!({
            "sessionId": "session-1",
            "toolCallId": "tool-1",
            "planContent": "  First do A, then B.  "
        }));
        assert_eq!(payload["sessionId"], "session-1");
        assert_eq!(payload["toolCallId"], "tool-1");
        assert_eq!(payload["planContent"], "  First do A, then B.  ");
        assert_eq!(payload["plan"], "First do A, then B.");

        let null_plan = xai_plan_payload(&json!({
            "params": {
                "sessionId": "session-1",
                "toolCallId": "tool-2",
                "planContent": null
            }
        }));
        assert!(null_plan.get("plan").is_none());
        assert_eq!(null_plan["planContent"], Value::Null);
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
            Some(AcpUsageTotals {
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
            Some(AcpUsageTotals {
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
        assert_eq!(
            context_window_from_value(&json!({"contextWindow": 0})),
            None
        );
    }

    #[test]
    fn usage_deltas_never_go_negative_across_a_reset() {
        let first = AcpUsageTotals {
            input: 100,
            output: 50,
            cache_read: 0,
            cache_write: 0,
        };
        let second = AcpUsageTotals {
            input: 140,
            output: 60,
            cache_read: 0,
            cache_write: 0,
        };
        assert_eq!(
            second.delta_since(&first),
            AcpUsageTotals {
                input: 40,
                output: 10,
                cache_read: 0,
                cache_write: 0,
            }
        );
        assert!(first.delta_since(&second).is_empty());
        assert_eq!(second.total_tokens(), 200);
    }

    /// Occupancy has to come from the latest request, not the session
    /// total. Cursor re-reads the whole context from cache on every
    /// request, so a cumulative `cache_read` would make the composer
    /// meter climb monotonically and peg at the window on any long
    /// session even while the real context stayed flat.
    #[test]
    fn context_occupancy_tracks_the_latest_request_not_the_session_total() {
        let mut tracker = ContextUsageTracker::default();
        tracker.observe_max_tokens(Some(200_000));
        let mut previous = AcpUsageTotals::default();
        let mut occupancies = Vec::new();
        // Three requests against a ~50k context: fresh input is small,
        // but each request reads the whole context back from cache.
        for request in 1..=3u64 {
            let cumulative = AcpUsageTotals {
                input: 100 * request,
                output: 200 * request,
                cache_read: 50_000 * request,
                cache_write: 0,
            };
            let delta = cumulative.delta_since(&previous);
            previous = cumulative;
            tracker.observe_lifetime_total(cumulative.total_tokens());
            occupancies.push(
                tracker
                    .snapshot(delta.total_tokens(), None, Some(true))
                    .expect("a changed reading emits")
                    .used_tokens,
            );
        }
        assert_eq!(occupancies, vec![50_300, 50_300, 50_300]);
        // The cumulative figure still drives the lifetime line.
        assert_eq!(tracker.lifetime_total(), 150_900);
    }

    fn interrupt_test_state() -> AcpSessionState {
        AcpSessionState {
            status: SessionStatus::Ready,
            active_turn: None,
            model: None,
            permission_mode: "agent".into(),
            config_options: vec![],
            pending: HashMap::new(),
            queued: VecDeque::new(),
            assistant_text: String::new(),
            thinking_text: String::new(),
            turn_generation: 0,
            interrupt_generation: None,
            usage_totals: AcpUsageTotals::default(),
            context: ContextUsageTracker::default(),
            grok_model_efforts: GrokModelEffortCatalog::default(),
            grok_effort: None,
        }
    }

    /// A Stop that nothing consumes must die with the turn it was aimed
    /// at. As a bare flag it did not: `finish_turn` cleared it in its
    /// first locked section, and an interrupt landing in the gap before
    /// the second one re-set it on a session that was already done with
    /// that turn. The flag then survived idle and instantly "interrupted"
    /// whatever ran next — the user's next turn hours later, or the
    /// queued turn that had just been promoted.
    #[test]
    fn a_stop_never_outlives_the_turn_it_was_aimed_at() {
        let mut state = interrupt_test_state();

        // Turn A runs and its prompt goes out.
        state.active_turn = Some(TurnId("turn-a".into()));
        let turn_a = state.claim_turn();
        assert!(!state.take_interrupt(turn_a));

        // Stop lands in `finish_turn`'s gap: turn A is over, but
        // `active_turn` still names it.
        state.request_interrupt();
        assert_eq!(state.interrupt_generation, Some(turn_a));

        // finish_turn promotes a queued turn B.
        let turn_b = state.claim_turn();
        assert!(
            !state.take_interrupt(turn_b),
            "a Stop aimed at turn A must not cancel queued turn B"
        );

        // And with an empty queue the request cannot survive idle into
        // whatever the user sends next.
        state.request_interrupt();
        state.release_turn();
        state.active_turn = Some(TurnId("turn-c".into()));
        let turn_c = state.claim_turn();
        assert!(
            !state.take_interrupt(turn_c),
            "a stale Stop must not complete the next turn before it starts"
        );
    }

    #[test]
    fn a_stop_aimed_at_the_running_turn_is_honored_exactly_once() {
        let mut state = interrupt_test_state();
        state.active_turn = Some(TurnId("turn-a".into()));
        let turn_a = state.claim_turn();
        state.request_interrupt();
        assert!(state.take_interrupt(turn_a));
        // Consumed — the in-flight cancel path must not fire again for
        // the same request.
        assert!(!state.take_interrupt(turn_a));
        // An idle session has nothing to interrupt.
        state.release_turn();
        state.request_interrupt();
        assert_eq!(state.interrupt_generation, None);
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
