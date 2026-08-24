//! Per-thread Codex session state.
//!
//! One [`CodexSession`] corresponds to a single live `codex app-server`
//! subprocess serving a single runtime thread. The adapter spawns them
//! on-demand, drives the init handshake, and wires up background tasks
//! that translate the child's notifications into the canonical event
//! stream.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use crate::agent_provider::{
    ProviderError, ProviderRuntimeEvent, ProviderSessionId, RequestId, SendOutcome, SendTurnInput,
    PlanAuthMode, SessionStatus, ThreadId, TurnId, UsageBaseline,
};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};
use crate::mcp::registry::McpRegistry;

use super::protocol::{
    AccountReadResponse, ApprovalResponse, Capabilities, ClientInfo, CollaborationMode,
    CollaborationModeSettings, DynamicToolCallParams, DynamicToolSpec,
    GetAccountRateLimitsResponse, InitializeParams,
    NotificationMessage, ServerRequestMessage, ThreadResumeParams, ThreadRollbackParams,
    ThreadStartParams, ThreadStartResponse, TurnInputItem, TurnInterruptParams, TurnStartParams,
    TurnStartResponse, RECOVERABLE_THREAD_RESUME_ERROR_SNIPPETS,
};
use super::translate::{translate_notification_with, translate_server_request, CodexSubagentDemux};

/// Default per-request timeout, mirroring the upstream reference's
/// `sendRequest(..., 20_000)` default.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(20);

const CODEX_DEFAULT_MODE_INSTRUCTIONS: &str = r#"<collaboration_mode># Collaboration Mode: Default

You are now in Default mode. Any previous instructions for other modes (e.g. Plan mode) are no longer active.

Your active mode changes only when new developer instructions with a different `<collaboration_mode>...</collaboration_mode>` change it; user requests or tool descriptions do not change mode by themselves. Known mode names are Default and Plan.

## request_user_input availability

Use the `request_user_input` tool only when it is listed in the available tools for this turn.

In Default mode, strongly prefer making reasonable assumptions and executing the user's request rather than stopping to ask questions. If you absolutely must ask a question because the answer cannot be discovered from local context and a reasonable assumption would be risky, ask the user directly with a concise plain-text question. Never write a multiple choice question as a textual assistant message.
</collaboration_mode>"#;

fn runtime_collaboration_mode(
    model: Option<&str>,
    effort: Option<&str>,
) -> Option<CollaborationMode> {
    let model = model?.split_whitespace().collect::<Vec<_>>().join(" ");
    if model.is_empty() {
        return None;
    }
    let effort = effort
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| !value.is_empty());
    let effort_text = effort
        .as_deref()
        .map(|value| format!(" with {value} reasoning effort"))
        .unwrap_or_default();
    let developer_instructions = format!(
        "{CODEX_DEFAULT_MODE_INSTRUCTIONS}\n\n<runtime_info>In case you're asked: you are running in CodeMux through the Codex harness, as {model}{effort_text}. No need to mention this otherwise.</runtime_info>"
    );

    Some(CollaborationMode {
        mode: "default".into(),
        settings: CollaborationModeSettings {
            model,
            reasoning_effort: effort,
            developer_instructions,
        },
    })
}

/// Monotonic request-id counter used to synthesise
/// [`RequestId`] handles for approval requests that the Codex server
/// emits without a stable identifier of their own.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Mint a new [`RequestId`] unique within this process.
fn mint_request_id() -> RequestId {
    RequestId(format!(
        "codex-req-{}",
        NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Configuration for spawning the `codex app-server` subprocess.
#[derive(Debug, Clone)]
pub struct CodexSpawnConfig {
    /// Path to the `codex` binary.
    pub codex_binary: PathBuf,
    /// Value to set for `CODEX_HOME`, if any.
    pub codex_home: Option<PathBuf>,
    /// Client identification block to hand Codex at `initialize` time.
    pub client_info: ClientInfo,
    /// Shared MCP registry used to publish and execute dynamic tools.
    pub mcp_registry: Option<McpRegistry>,
}

/// Runtime-mutable state inside a [`CodexSession`].
pub(crate) struct CodexSessionState {
    /// Codex's own thread identifier (different from the runtime's
    /// [`ThreadId`], though semantically matched).
    pub codex_thread_id: String,
    /// The turn currently in flight, if any. `Option::None` means
    /// the session is idle and ready for a new turn.
    pub active_turn: Option<TurnId>,
    /// Map from synthesised [`RequestId`] to the raw JSON-RPC id we need
    /// to reply to.
    pub pending_approvals: HashMap<RequestId, Value>,
    /// Lifecycle status.
    pub status: SessionStatus,
    /// Current model override (session-wide default).
    pub model: Option<String>,
    /// Per-session reasoning-effort default. Applied to every
    /// `turn/start` whose caller does not provide an `effort_override`
    /// — mirrors the reference's `collaborationMode.settings.reasoning_effort`
    /// session default.
    pub default_effort: Option<String>,
    /// Follow-up turns queued while a turn was in flight, in FIFO
    /// dispatch order. Drained one-at-a-time as the session returns to
    /// idle (see [`CodexSession::drain_queue`]).
    pub queued_turns: VecDeque<QueuedTurn>,
}

/// A user turn parked in the follow-up queue behind the active turn.
#[derive(Debug, Clone)]
pub(crate) struct QueuedTurn {
    /// Backend-generated id, stable for the lifetime of the queued item.
    pub queued_id: String,
    /// The original send input, replayed verbatim when the turn
    /// dispatches.
    pub input: SendTurnInput,
}

/// Mint a queue id unique within this process.
fn mint_queued_id() -> String {
    format!(
        "codex-queued-{}",
        NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
    )
}

/// Move the queued turn with `queued_id` to the front of `queue` so it
/// is the next item [`CodexSession::drain_queue`] dispatches. Returns
/// `Some(original_index)` when the item was found (and is now at the
/// front), `None` when no such id is queued. A no-op when it is already
/// at the front. The returned index lets a caller undo the reorder via
/// [`restore_queued_position`] when a follow-up step (e.g.
/// `interrupt_turn`) fails.
fn promote_queued_to_front(queue: &mut VecDeque<QueuedTurn>, queued_id: &str) -> Option<usize> {
    match queue.iter().position(|q| q.queued_id == queued_id) {
        Some(0) => Some(0),
        Some(pos) => {
            if let Some(item) = queue.remove(pos) {
                queue.push_front(item);
            }
            Some(pos)
        }
        None => None,
    }
}

/// Undo a [`promote_queued_to_front`] by moving `queued_id` back to
/// `original_index`. Finds the item's CURRENT position first — it may
/// have been dispatched or cancelled in the meantime, in which case this
/// is a no-op. Otherwise it is removed and re-inserted at
/// `min(original_index, queue.len())` so the queue order is restored.
fn restore_queued_position(
    queue: &mut VecDeque<QueuedTurn>,
    queued_id: &str,
    original_index: usize,
) {
    let Some(pos) = queue.iter().position(|q| q.queued_id == queued_id) else {
        return;
    };
    if let Some(item) = queue.remove(pos) {
        let insert_at = original_index.min(queue.len());
        queue.insert(insert_at, item);
    }
}

/// Handle for a live Codex session.
pub(crate) struct CodexSession {
    /// Runtime-owned thread identifier.
    pub thread_id: ThreadId,
    /// Stable provider session identifier exposed to the orchestrator.
    pub provider_session_id: ProviderSessionId,
    /// Working directory the thread was started with. Retained for
    /// diagnostic / future use (e.g. rebinding the session to a new path).
    #[allow(dead_code)]
    pub cwd: PathBuf,
    /// Wrapped JSON-RPC child. Shared via `Arc` with background tasks.
    pub child: Arc<JsonRpcChild>,
    /// Mutable session state.
    pub state: Mutex<CodexSessionState>,
    /// Broadcast sender used to emit runtime events from methods that
    /// mutate the follow-up queue (enqueue / dispatch / cancel). The
    /// background tasks receive their own clone via spawn args.
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    /// Shutdown signal for the background notification / request tasks.
    shutdown_tx: broadcast::Sender<()>,
    /// Background task JoinHandles retained so `Drop` can abort them.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Set by the child-exit watchdog when `codex app-server` dies
    /// *unintentionally* mid-session. [`is_dead`](CodexSession::is_dead)
    /// reports it so the provider's `has_session` treats the corpse as absent
    /// and the next send rebuilds a fresh session (with the resume cursor) via
    /// `ensure_live_session` instead of routing to a dead child.
    ///
    /// Deliberately NOT keyed off `SessionStatus::Error`: Codex also sets
    /// `Error` on a *turn* failure (a live child that just refused a prompt)
    /// and on a non-retryable server `error` notification — neither means the
    /// child is gone. Only the watchdog (a confirmed dead child) sets this, so
    /// a recoverable error stays recoverable in place without a rebuild.
    dead: Arc<AtomicBool>,
    /// Ledger totals already recorded for this thread at build time.
    /// Handed to the notification task's demux so a resumed thread does
    /// not re-record its history. Zero for a fresh thread.
    recorded_usage_baseline: UsageBaseline,
    /// Owning workspace of the chat pane this session runs in, from
    /// `StartSessionInput.workspace_id`. Attached per-call to dynamic
    /// tool dispatches so workspace-scoped built-in tools bind to THIS
    /// session's workspace — the registry's shared MCP child cannot
    /// learn the caller from its env.
    workspace_id: Option<String>,
}

impl CodexSession {
    /// Spawn a `codex app-server` child, run the init handshake, and
    /// attach background tasks that forward notifications and
    /// server-initiated requests to `event_tx`.
    ///
    /// If `resume_cursor` is `Some`, tries `thread/resume` first and
    /// falls back to `thread/start` on a recoverable "unknown thread"
    /// error. The fallback also emits a [`ProviderRuntimeEvent::RuntimeWarning`].
    pub async fn spawn_and_initialize(
        thread_id: ThreadId,
        cwd: PathBuf,
        model: Option<String>,
        permission_mode: Option<String>,
        effort: Option<String>,
        fast_mode: bool,
        resume_cursor: Option<Value>,
        caller_env: Option<HashMap<String, String>>,
        // Owning workspace of the calling chat pane; stored on the
        // session and tagged onto MCP dispatches (see the field doc).
        workspace_id: Option<String>,
        spawn: CodexSpawnConfig,
        event_tx: broadcast::Sender<ProviderRuntimeEvent>,
        // Usage already in the ledger for this thread. Seeded into the
        // demux so a resumed thread's lifetime counter is not
        // re-recorded from zero — see `CodexSubagentDemux`.
        recorded_usage_baseline: Option<UsageBaseline>,
    ) -> Result<Arc<Self>, ProviderError> {
        // Start from the caller-supplied workspace env (CODEMUX_WORKSPACE_ID,
        // CODEMUX_PANE_ID, BROWSER, …) so the agent's Bash subprocesses route
        // `codemux browser open` at their OWN workspace. Provider-critical
        // vars are overlaid AFTER so they win on conflict — CODEX_HOME must
        // point at Codemux's managed config dir regardless of what the
        // workspace env carries.
        let mut env = caller_env.unwrap_or_default();
        if let Some(home) = spawn.codex_home.as_ref() {
            env.insert("CODEX_HOME".to_string(), home.to_string_lossy().to_string());
        }

        let child = JsonRpcChild::spawn(SpawnConfig {
            program: spawn.codex_binary.clone(),
            args: vec!["app-server".into()],
            env,
            cwd: Some(cwd.clone()),
            default_timeout: DEFAULT_RPC_TIMEOUT,
        })
        .await
        .map_err(|e| ProviderError::ProcessError {
            message: "failed to spawn `codex app-server`".into(),
            source: Some(e.to_string()),
        })?;
        let child = Arc::new(child);

        // Pull the single incoming-request receiver before any background
        // tasks start; otherwise the adapter could race the watchdog.
        let incoming_rx = child
            .incoming_requests()
            .ok_or_else(|| ProviderError::ProcessError {
                message: "JsonRpcChild incoming-request channel already claimed".into(),
                source: None,
            })?;

        // --- initialize handshake -------------------------------------------
        let init_params = serde_json::to_value(InitializeParams {
            client_info: spawn.client_info.clone(),
            capabilities: Capabilities {
                experimental_api: true,
            },
        })
        .map_err(|e| ProviderError::ProcessError {
            message: "serialize initialize params".into(),
            source: Some(e.to_string()),
        })?;
        child
            .request("initialize", init_params)
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("initialize failed: {e}"),
            })?;
        child
            .notify("initialized", json!({}))
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("initialized notification failed: {e}"),
            })?;

        // Capture the live registry tool surface once for thread/start.
        // Codex persists dynamic tool definitions in the rollout, while the
        // request handler below always dispatches through the live registry.
        let dynamic_tools = match spawn.mcp_registry.as_ref() {
            Some(registry) => {
                let tools = if spawn.codex_home.is_none() {
                    registry
                        .list_all_tools_excluding_source(
                            crate::mcp::McpConfigSource::CodexUser,
                        )
                        .await
                } else {
                    registry.list_all_tools().await
                };
                Some(
                    tools
                        .into_iter()
                        .map(|tool| DynamicToolSpec::Function {
                            name: codex_dynamic_tool_name(&tool.prefixed_name),
                            description: tool.description.unwrap_or_default(),
                            input_schema: tool.input_schema,
                            defer_loading: None,
                        })
                        .collect(),
                )
            }
            None => None,
        };

        // Best-effort probes. Failures are non-fatal — we log via
        // RuntimeWarning and continue.
        match child.request("model/list", json!({})).await {
            Ok(_) => {}
            Err(e) => {
                let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread_id.clone()),
                    message: format!("model/list probe failed: {e}"),
                    original_payload: None,
                });
            }
        }
        // account/read is the canonical "can this provider run?" check.
        // `requires_openai_auth` describes the active provider and stays true
        // for a logged-in ChatGPT/API-key account, so only gate when the
        // account is also absent. RPC failure (binary error, not auth) is
        // logged but tolerated; the session proceeds and turns will fail
        // with the underlying error.
        match child.request("account/read", json!({})).await {
            Ok(resp) => match serde_json::from_value::<AccountReadResponse>(resp) {
                Ok(info) if info.needs_login() => {
                    let _ = child.shutdown().await;
                    return Err(ProviderError::NotAuthenticated {
                        provider: crate::agent_provider::ProviderKind::Codex,
                        hint: "Run `codex login` and try again.".into(),
                    });
                }
                Ok(info) => {
                    // The account block was already being fetched and then
                    // thrown away apart from the login gate. It is the
                    // authoritative answer to "is this a subscription or an
                    // API key", which the Usage dashboard needs to decide
                    // whether Codex tokens are a bill or plan-covered value.
                    if let Some(account) = info.account.as_ref() {
                        let auth_mode = match account.account_type.as_deref() {
                            Some("apiKey") => Some(PlanAuthMode::ApiKey),
                            Some("chatgpt") | Some("chatgptDeviceCode") => {
                                Some(PlanAuthMode::Subscription)
                            }
                            _ => None,
                        };
                        let plan_label = codex_plan_label(account.plan_type.as_deref());
                        if auth_mode.is_some() || plan_label.is_some() {
                            // No windows yet — the push (or the read below)
                            // supplies those. The sink merges the two
                            // halves rather than letting either clobber.
                            let _ = event_tx.send(ProviderRuntimeEvent::PlanUsageUpdated {
                                thread_id: thread_id.clone(),
                                provider: crate::agent_provider::ProviderKind::Codex,
                                windows: Vec::new(),
                                plan_label,
                                auth_mode,
                            });
                        }
                    }
                }
                Err(e) => {
                    let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                        thread_id: Some(thread_id.clone()),
                        message: format!("account/read decode failed: {e}"),
                        original_payload: None,
                    });
                }
            },
            Err(e) => {
                let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread_id.clone()),
                    message: format!("account/read probe failed: {e}"),
                    original_payload: None,
                });
            }
        }

        // Pull the plan quota once at init so the Usage dashboard's meters
        // populate immediately instead of waiting for the first
        // `account/rateLimits/updated` push (which may not arrive until the
        // user actually spends something). Entirely best-effort: older
        // app-servers do not implement the method, and a metered API-key
        // account has no windows to report — both are log-and-continue, and
        // an absent quota simply renders no meters.
        match child.request("account/rateLimits/read", json!({})).await {
            Ok(resp) => match serde_json::from_value::<GetAccountRateLimitsResponse>(resp) {
                Ok(limits) => {
                    for event in super::translate::plan_usage_from_rate_limits(
                        &thread_id,
                        &limits.rate_limits,
                    ) {
                        let _ = event_tx.send(event);
                    }
                }
                Err(e) => {
                    eprintln!("[codemux::codex] account/rateLimits/read decode failed: {e}");
                }
            },
            Err(e) => {
                // Expected on builds predating the method — not surfaced to
                // the user, since nothing is broken.
                eprintln!("[codemux::codex] account/rateLimits/read unavailable: {e}");
            }
        }

        // --- thread/start or thread/resume ----------------------------------
        let codex_thread_id = match &resume_cursor {
            Some(cursor) => {
                let resume_id = codex_thread_id_from_resume_cursor(cursor);
                match resume_id {
                    Some(rid) => {
                        let (approval_policy, sandbox) = match codex_permission_mode_to_policy_pair(
                            permission_mode.as_deref(),
                        ) {
                            Some((ap, sb)) => (Some(ap), Some(sb)),
                            None => (None, None),
                        };
                        let params = ThreadResumeParams {
                            thread_id: rid.clone(),
                            model: model.clone(),
                            service_tier: Some(fast_mode.then(|| "fast".to_string())),
                            cwd: Some(cwd.clone()),
                            collaboration_mode: None,
                            approval_policy,
                            sandbox,
                            experimental_raw_events: false,
                        };
                        let params_value = serde_json::to_value(&params).unwrap();
                        match child.request("thread/resume", params_value).await {
                            Ok(resp) => match serde_json::from_value::<ThreadStartResponse>(resp) {
                                Ok(r) => r.thread_id().to_string(),
                                Err(e) => {
                                    return Err(ProviderError::RpcError {
                                        message: format!("malformed thread/resume response: {e}"),
                                    })
                                }
                            },
                            Err(err) if is_recoverable_resume_error(&err.to_string()) => {
                                // Fall back to a fresh thread/start and
                                // emit a warning so the UI can show the
                                // transition.
                                let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                                    thread_id: Some(thread_id.clone()),
                                    message: format!(
                                        "codex thread/resume failed, falling back to thread/start: {err}"
                                    ),
                                    original_payload: None,
                                });
                                start_fresh_thread(&child, cwd.clone(), model.clone(), permission_mode.clone(), fast_mode, dynamic_tools.clone()).await?
                            }
                            Err(e) => {
                                return Err(ProviderError::RpcError {
                                    message: format!("thread/resume failed: {e}"),
                                })
                            }
                        }
                    }
                    None => start_fresh_thread(&child, cwd.clone(), model.clone(), permission_mode.clone(), fast_mode, dynamic_tools.clone()).await?,
                }
            }
            None => start_fresh_thread(&child, cwd.clone(), model.clone(), permission_mode.clone(), fast_mode, dynamic_tools.clone()).await?,
        };

        // --- assemble session handle ----------------------------------------
        let (shutdown_tx, _shutdown_rx) = broadcast::channel(4);
        let state = Mutex::new(CodexSessionState {
            codex_thread_id: codex_thread_id.clone(),
            active_turn: None,
            pending_approvals: HashMap::new(),
            status: SessionStatus::Ready,
            model,
            default_effort: effort,
            queued_turns: VecDeque::new(),
        });
        let session = Arc::new(Self {
            thread_id: thread_id.clone(),
            provider_session_id: ProviderSessionId(codex_thread_id.clone()),
            cwd,
            child: Arc::clone(&child),
            state,
            event_tx: event_tx.clone(),
            shutdown_tx: shutdown_tx.clone(),
            tasks: Mutex::new(Vec::new()),
            dead: Arc::new(AtomicBool::new(false)),
            recorded_usage_baseline: recorded_usage_baseline.unwrap_or_default(),
            workspace_id,
        });

        // Emit SessionConfigured up front so subscribers see the thread
        // binding even if Codex never sends a `thread/started` notification
        // (some versions skip it).
        let _ = event_tx.send(ProviderRuntimeEvent::SessionConfigured {
            thread_id: thread_id.clone(),
            provider_session_id: session.provider_session_id.clone(),
        });
        let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: thread_id.clone(),
            status: SessionStatus::Ready,
        });

        // --- background tasks -----------------------------------------------
        let notifications_task = spawn_notifications_task(
            Arc::clone(&session),
            Arc::clone(&child),
            event_tx.clone(),
            shutdown_tx.subscribe(),
        );
        let requests_task = spawn_incoming_requests_task(
            Arc::clone(&session),
            Arc::clone(&child),
            spawn.mcp_registry.clone(),
            incoming_rx,
            event_tx.clone(),
            shutdown_tx.subscribe(),
        );
        let watchdog_task = spawn_child_exit_watchdog(
            Arc::clone(&session),
            Arc::clone(&child),
            event_tx.clone(),
            shutdown_tx.subscribe(),
        );

        {
            let mut guard = session.tasks.lock().await;
            guard.push(notifications_task);
            guard.push(requests_task);
            guard.push(watchdog_task);
        }

        Ok(session)
    }

    /// Clone of the canonical event broadcaster, for methods that emit
    /// follow-up-queue events off the background-task path.
    fn event_tx(&self) -> broadcast::Sender<ProviderRuntimeEvent> {
        self.event_tx.clone()
    }

    /// Send a user turn, or **queue** it behind the active turn.
    ///
    /// Idle → the turn starts immediately ([`SendOutcome::Started`]).
    /// Busy → the input is pushed onto the FIFO follow-up queue, a
    /// [`ProviderRuntimeEvent::TurnQueued`] is emitted, and
    /// [`SendOutcome::Queued`] is returned; the turn dispatches later via
    /// [`drain_queue`](Self::drain_queue). The busy check re-runs under
    /// the lock so there is no TOCTOU with a just-finished turn.
    pub async fn enqueue_or_send(
        self: &Arc<Self>,
        input: SendTurnInput,
    ) -> Result<SendOutcome, ProviderError> {
        {
            let mut state = self.state.lock().await;
            if state.active_turn.is_some() {
                let queued_id = mint_queued_id();
                let text = input
                    .display_text
                    .clone()
                    .unwrap_or_else(|| input.text.clone());
                let client_nonce = input.client_nonce.clone();
                state.queued_turns.push_back(QueuedTurn {
                    queued_id: queued_id.clone(),
                    input,
                });
                drop(state);
                let _ = self.event_tx().send(ProviderRuntimeEvent::TurnQueued {
                    thread_id: self.thread_id.clone(),
                    queued_id: queued_id.clone(),
                    client_nonce,
                    text,
                });
                return Ok(SendOutcome::Queued(queued_id));
            }
        }
        let checkpoint = input.turn_checkpoint.clone();
        if let Some(checkpoint) = checkpoint.as_ref() {
            checkpoint.prepare().await;
        }
        let sent = self
            .do_send(
                input.text,
                input.images,
                input.skill_invocations,
                input.model_override,
                input.effort_override,
            )
            .await;
        match sent {
            Ok(turn_id) => {
                if let Some(checkpoint) = checkpoint.as_ref() {
                    checkpoint.commit().await;
                }
                Ok(SendOutcome::Started(turn_id))
            }
            Err(error) => {
                if let Some(checkpoint) = checkpoint.as_ref() {
                    checkpoint.abort().await;
                }
                Err(error)
            }
        }
    }

    /// Pop the next queued turn (if idle) and dispatch it. No-op when the
    /// queue is empty or a turn is active. Emits
    /// [`ProviderRuntimeEvent::QueuedTurnDispatched`] on success; a failed
    /// dispatch cancels the item (never wedges the queue) and the next is
    /// attempted.
    pub async fn drain_queue(self: &Arc<Self>) {
        loop {
            let next = {
                let mut state = self.state.lock().await;
                let idle = state.active_turn.is_none()
                    && matches!(state.status, SessionStatus::Ready)
                    && state.pending_approvals.is_empty();
                if !idle {
                    return;
                }
                state.queued_turns.pop_front()
            };
            let Some(queued) = next else { return };
            let text = queued
                .input
                .display_text
                .clone()
                .unwrap_or_else(|| queued.input.text.clone());
            let checkpoint = queued.input.turn_checkpoint.clone();
            if let Some(checkpoint) = checkpoint.as_ref() {
                checkpoint.prepare().await;
            }
            match self
                .do_send(
                    queued.input.text,
                    queued.input.images,
                    queued.input.skill_invocations,
                    queued.input.model_override,
                    queued.input.effort_override,
                )
                .await
            {
                Ok(turn_id) => {
                    if let Some(checkpoint) = checkpoint.as_ref() {
                        checkpoint.commit().await;
                    }
                    let _ = self.event_tx().send(ProviderRuntimeEvent::QueuedTurnDispatched {
                        thread_id: self.thread_id.clone(),
                        queued_id: queued.queued_id,
                        turn_id,
                        text,
                    });
                    return;
                }
                Err(err) => {
                    if let Some(checkpoint) = checkpoint.as_ref() {
                        checkpoint.abort().await;
                    }
                    let _ = self.event_tx().send(ProviderRuntimeEvent::RuntimeWarning {
                        thread_id: Some(self.thread_id.clone()),
                        message: format!(
                            "queued turn {} failed to dispatch: {err}",
                            queued.queued_id
                        ),
                        original_payload: None,
                    });
                    let _ = self.event_tx().send(ProviderRuntimeEvent::QueuedTurnCancelled {
                        thread_id: self.thread_id.clone(),
                        queued_id: queued.queued_id,
                    });
                    continue;
                }
            }
        }
    }

    /// Cancel a single queued turn by id (user pressed X). Emits
    /// [`ProviderRuntimeEvent::QueuedTurnCancelled`] when found. Returns
    /// whether the queued item was actually removed.
    pub async fn cancel_queued(&self, queued_id: &str) -> Result<bool, ProviderError> {
        let removed = {
            let mut state = self.state.lock().await;
            if let Some(pos) = state
                .queued_turns
                .iter()
                .position(|q| q.queued_id == queued_id)
            {
                state.queued_turns.remove(pos);
                true
            } else {
                false
            }
        };
        if removed {
            let _ = self.event_tx().send(ProviderRuntimeEvent::QueuedTurnCancelled {
                thread_id: self.thread_id.clone(),
                queued_id: queued_id.to_string(),
            });
        }
        Ok(removed)
    }

    /// **Send now (steer):** promote a queued follow-up to the front of
    /// the queue and dispatch it immediately, interrupting the active
    /// turn if one is running. Soft stop — the Codex thread, transcript,
    /// and on-disk work are preserved; nothing is discarded.
    ///
    /// Idempotent: an unknown / already-dispatched id is a silent no-op.
    ///
    /// When a turn is active we call
    /// [`interrupt_turn`](Self::interrupt_turn). Unlike Claude's
    /// `interrupt`, it does NOT drain inline — the abort lands as a
    /// `turn/completed` (or aborted) notification that returns the session
    /// to `Ready`, and the event loop drains there, dispatching the item
    /// we just promoted to the front. If `interrupt_turn` returns the
    /// "no active turn" `ValidationError` because the turn finished during
    /// the click→call race, we fall back to draining directly. When the
    /// session is already idle we drain directly.
    ///
    /// **Pending approvals:** like a manual interrupt, an outstanding
    /// approval keeps `drain_queue` from dispatching until it resolves, so
    /// the promoted message stays queued until then.
    pub async fn send_queued_now(self: &Arc<Self>, queued_id: &str) -> Result<(), ProviderError> {
        let (has_active_turn, original_index) = {
            let mut state = self.state.lock().await;
            match promote_queued_to_front(&mut state.queued_turns, queued_id) {
                Some(idx) => (state.active_turn.is_some(), idx),
                // Unknown / already-dispatched / cancelled — no-op.
                None => return Ok(()),
            }
        };
        if has_active_turn {
            // Soft-stop the running turn. The event-loop drain (on the
            // resulting turn/completed → Ready) dispatches the promoted
            // item — `interrupt_turn` does not drain inline.
            match self.interrupt_turn(None).await {
                Ok(()) => Ok(()),
                // Race: the turn finished between our busy check and the
                // interrupt RPC. The session is idle, so drain directly.
                Err(ProviderError::ValidationError { .. }) => {
                    self.drain_queue().await;
                    Ok(())
                }
                Err(err) => {
                    // A real interrupt failure: undo the reorder so a
                    // failed steer doesn't leave the queue silently
                    // rearranged.
                    {
                        let mut state = self.state.lock().await;
                        restore_queued_position(
                            &mut state.queued_turns,
                            queued_id,
                            original_index,
                        );
                    }
                    Err(err)
                }
            }
        } else {
            // Already idle — drain the promoted item out.
            self.drain_queue().await;
            Ok(())
        }
    }

    /// Cancel every queued turn, emitting a cancellation for each. Used
    /// when the session closes or errors.
    async fn cancel_all_queued(&self) {
        let drained: Vec<String> = {
            let mut state = self.state.lock().await;
            state.queued_turns.drain(..).map(|q| q.queued_id).collect()
        };
        for queued_id in drained {
            let _ = self.event_tx().send(ProviderRuntimeEvent::QueuedTurnCancelled {
                thread_id: self.thread_id.clone(),
                queued_id,
            });
        }
    }

    /// Dispatch a user turn on the `codex app-server`. Returns the
    /// Codex-assigned turn id. Codex applies `effort` per-turn, so the
    /// override is threaded straight into the RPC. Does NOT perform the
    /// busy check (callers gate that); assumes the session is idle.
    async fn do_send(
        &self,
        text: String,
        images: Vec<crate::agent_provider::ImageInput>,
        skill_invocations: Vec<crate::skills::ResolvedSkillInvocation>,
        model_override: Option<String>,
        effort_override: Option<String>,
    ) -> Result<TurnId, ProviderError> {
        let (codex_thread_id, model_default, effort_default) = {
            let state = self.state.lock().await;
            (
                state.codex_thread_id.clone(),
                state.model.clone(),
                state.default_effort.clone(),
            )
        };

        // Codex's `turn/start` accepts a heterogeneous input array.
        // The Responses-style format places images BEFORE text (per
        // OpenAI's "describe these images" pattern) so the model has
        // visual context anchored to the prompt that follows.
        // Bytes are encoded as `data:` URIs inline — Codex's
        // app-server doesn't expose a separate file-upload API, so
        // base64-in-URL is the canonical local-bytes path.
        //
        // Image-only turn handling: when `text` is empty AND there's at
        // least one image, skip the trailing empty text item entirely.
        // The pre-Stage-9 adapter pushed an empty `TurnInputItem::Text`
        // alongside images, which Codex rejects with a 400 on the
        // image-only path.
        use base64::Engine;
        let mut input_items: Vec<TurnInputItem> =
            Vec::with_capacity(images.len() + skill_invocations.len() + 1);
        for img in images {
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(&img.data);
            input_items.push(TurnInputItem::Image {
                url: format!("data:{};base64,{}", img.media_type, encoded),
            });
        }
        for skill in skill_invocations {
            if matches!(
                skill.invocation,
                crate::skills::SkillInvocationKind::CodexSkillItem
            ) {
                if let Some(path) = skill.path {
                    input_items.push(TurnInputItem::Skill {
                        name: skill.name,
                        path,
                    });
                }
            }
        }
        let trimmed_text = text.trim();
        if !trimmed_text.is_empty() || input_items.is_empty() {
            input_items.push(TurnInputItem::Text {
                text,
                text_elements: vec![],
            });
        }

        let model = model_override.or(model_default);
        let effort = effort_override.or(effort_default);
        let collaboration_mode = runtime_collaboration_mode(model.as_deref(), effort.as_deref());
        let params = TurnStartParams {
            thread_id: codex_thread_id,
            input: input_items,
            model,
            service_tier: None,
            effort,
            collaboration_mode,
        };
        let params_value = serde_json::to_value(&params).unwrap();
        let resp = self
            .child
            .request("turn/start", params_value)
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("turn/start failed: {e}"),
            })?;
        let parsed: TurnStartResponse =
            serde_json::from_value(resp).map_err(|e| ProviderError::RpcError {
                message: format!("malformed turn/start response: {e}"),
            })?;
        let turn_id = TurnId(parsed.turn.id);
        {
            let mut state = self.state.lock().await;
            state.active_turn = Some(turn_id.clone());
            state.status = SessionStatus::Running {
                active_turn: turn_id.clone(),
            };
        }
        Ok(turn_id)
    }

    /// Whether the child-exit watchdog has declared this session's
    /// `codex app-server` dead (unintentional exit). A dead session is
    /// treated as absent by the provider so the next send auto-resumes.
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }

    /// Interrupt the currently active turn. If `turn_id` is provided,
    /// only interrupt if it matches the live turn.
    pub async fn interrupt_turn(&self, turn_id: Option<TurnId>) -> Result<(), ProviderError> {
        let (codex_thread_id, active_turn) = {
            let state = self.state.lock().await;
            (state.codex_thread_id.clone(), state.active_turn.clone())
        };
        let active_turn = match active_turn {
            Some(t) => t,
            None => {
                return Err(ProviderError::ValidationError {
                    message: "no active turn to interrupt".into(),
                });
            }
        };
        if let Some(requested) = turn_id.as_ref() {
            if requested != &active_turn {
                return Err(ProviderError::ValidationError {
                    message: format!(
                        "turn id mismatch: requested {}, active {}",
                        requested.0, active_turn.0
                    ),
                });
            }
        }
        let params = TurnInterruptParams {
            thread_id: codex_thread_id,
            turn_id: active_turn.0.clone(),
        };
        let params_value = serde_json::to_value(&params).unwrap();
        self.child
            .request("turn/interrupt", params_value)
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("turn/interrupt failed: {e}"),
            })?;
        Ok(())
    }

    /// Remove completed tail turns from Codex's own durable thread.
    pub async fn rollback_conversation(&self, num_turns: u32) -> Result<(), ProviderError> {
        if num_turns == 0 {
            return Err(ProviderError::ValidationError {
                message: "rollback requires at least one turn".into(),
            });
        }
        let codex_thread_id = {
            let state = self.state.lock().await;
            if state.active_turn.is_some()
                || !state.pending_approvals.is_empty()
                || !state.queued_turns.is_empty()
            {
                return Err(ProviderError::ValidationError {
                    message: "cannot roll back while a turn, approval, or queued prompt is active"
                        .into(),
                });
            }
            state.codex_thread_id.clone()
        };
        let params = serde_json::to_value(ThreadRollbackParams {
            thread_id: codex_thread_id,
            num_turns,
        })
        .map_err(|error| ProviderError::ProcessError {
            message: format!("failed to serialize thread/rollback: {error}"),
            source: None,
        })?;
        self.child
            .request("thread/rollback", params)
            .await
            .map_err(|error| ProviderError::RpcError {
                message: format!("thread/rollback failed: {error}"),
            })?;
        Ok(())
    }

    /// Resolve a pending approval request with the caller's decision.
    pub async fn respond_to_request(
        &self,
        request_id: RequestId,
        response: ApprovalResponse,
    ) -> Result<(), ProviderError> {
        let jsonrpc_id = {
            let mut state = self.state.lock().await;
            state
                .pending_approvals
                .remove(&request_id)
                .ok_or_else(|| ProviderError::RequestNotPending {
                    request_id: request_id.clone(),
                })?
        };
        let payload = serde_json::to_value(&response).unwrap();
        self.child
            .respond(jsonrpc_id, Ok(payload))
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("respond failed: {e}"),
            })?;
        Ok(())
    }

    /// Update the session-wide default model.
    pub async fn set_model(&self, model: String) {
        let mut state = self.state.lock().await;
        state.model = Some(model);
    }

    /// Gracefully shut the session down: close the JSON-RPC child
    /// (EOF-then-kill), stop background tasks, and flip state to Closed.
    ///
    /// Idempotent: the underlying
    /// [`JsonRpcChild::shutdown`](crate::json_rpc_child::JsonRpcChild::shutdown)
    /// short-circuits on repeat calls, and the tasks list is drained, so
    /// later invocations are cheap no-ops.
    pub async fn shutdown(&self) {
        // Cancel any queued follow-ups so the UI clears its greyed items.
        self.cancel_all_queued().await;

        // Signal background tasks first so they stop pumping events
        // mid-teardown.
        let _ = self.shutdown_tx.send(());

        // Close the JSON-RPC child cleanly (EOF → 2s grace → kill).
        let _ = self.child.shutdown().await;

        // Abort any tasks that haven't exited on their own.
        let tasks: Vec<_> = {
            let mut guard = self.tasks.lock().await;
            std::mem::take(&mut *guard)
        };
        for t in tasks {
            t.abort();
        }
        // Flip internal status so concurrent callers see Closed without
        // waiting for the child to actually exit.
        {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Closed;
            state.active_turn = None;
        }
    }
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        // Best-effort task shutdown — async shutdown happens via explicit
        // `shutdown()`. If we got dropped without one, at least signal
        // background tasks so they stop pumping events into a dead
        // broadcaster.
        let _ = self.shutdown_tx.send(());
    }
}

/// Translate Codemux's logical Codex permission-mode string into the
/// `(approval_policy, sandbox)` pair the Codex RPC expects on
/// `thread/start` / `thread/resume`. Mirrors a reference
/// multi-provider client's table
/// (`apps/server/src/provider/Layers/CodexSessionRuntime.ts:237-258`).
///
/// Returns `None` when no mode is set — callers skip the RPC fields
/// entirely rather than sending empty strings.
pub(crate) fn codex_permission_mode_to_policy_pair(
    mode: Option<&str>,
) -> Option<(String, String)> {
    match mode? {
        "read-only" => Some(("untrusted".into(), "read-only".into())),
        "workspace-write" => Some(("on-request".into(), "workspace-write".into())),
        "danger-full-access" => Some(("never".into(), "danger-full-access".into())),
        _ => None,
    }
}

/// Issue a fresh `thread/start` and return the Codex-assigned thread id.
async fn start_fresh_thread(
    child: &JsonRpcChild,
    cwd: PathBuf,
    model: Option<String>,
    permission_mode: Option<String>,
    fast_mode: bool,
    dynamic_tools: Option<Vec<DynamicToolSpec>>,
) -> Result<String, ProviderError> {
    let (approval_policy, sandbox) =
        match codex_permission_mode_to_policy_pair(permission_mode.as_deref()) {
            Some((ap, sb)) => (Some(ap), Some(sb)),
            None => (None, None),
        };
    let params = ThreadStartParams {
        model,
        // `Some(None)` deliberately serializes as JSON null: it clears a
        // user-level fast default so the composer's Standard choice is honest.
        service_tier: Some(fast_mode.then(|| "fast".to_string())),
        cwd: Some(cwd),
        collaboration_mode: None,
        approval_policy,
        sandbox,
        dynamic_tools,
        experimental_raw_events: false,
    };
    let params_value = serde_json::to_value(&params).unwrap();
    let resp = child
        .request("thread/start", params_value)
        .await
        .map_err(|e| ProviderError::RpcError {
            message: format!("thread/start failed: {e}"),
        })?;
    let parsed: ThreadStartResponse =
        serde_json::from_value(resp).map_err(|e| ProviderError::RpcError {
            message: format!("malformed thread/start response: {e}"),
        })?;
    Ok(parsed.thread_id().to_string())
}

/// Case-insensitive match against the recoverable-resume-error snippet list.
fn is_recoverable_resume_error(err_msg: &str) -> bool {
    let lower = err_msg.to_lowercase();
    RECOVERABLE_THREAD_RESUME_ERROR_SNIPPETS
        .iter()
        .any(|s| lower.contains(&s.to_lowercase()))
}

/// Extract the Codex thread id from either the provider-native cursor returned
/// by this adapter or CodeMux's provider-neutral persisted resume shape.
/// `ThreadResumeParams::thread_id` still serializes to the official `threadId`
/// field at the app-server RPC boundary.
/// Short display label for a Codex `planType`.
///
/// The wire values are lowercase tags (`"pro"`, `"plus"`); the dashboard
/// wants something a human recognizes. Unknown tags are title-cased and
/// passed through rather than dropped, so a new plan still shows a name.
fn codex_plan_label(plan_type: Option<&str>) -> Option<String> {
    let plan = plan_type?.trim();
    if plan.is_empty() {
        return None;
    }
    Some(match plan.to_ascii_lowercase().as_str() {
        "pro" => "ChatGPT Pro".to_string(),
        "plus" => "ChatGPT Plus".to_string(),
        "team" => "ChatGPT Team".to_string(),
        "enterprise" => "ChatGPT Enterprise".to_string(),
        "edu" => "ChatGPT Edu".to_string(),
        "free" => "ChatGPT Free".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => format!("ChatGPT {}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => return None,
            }
        }
    })
}

fn codex_thread_id_from_resume_cursor(cursor: &Value) -> Option<String> {
    cursor
        .get("threadId")
        .or_else(|| cursor.get("resume"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Background task: consume notifications from the child, translate
/// each into canonical events, and broadcast.
fn spawn_notifications_task(
    session: Arc<CodexSession>,
    child: Arc<JsonRpcChild>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut notifications = child.notifications();
        // One demux per session, rooted at the parent Codex thread id, so
        // child-thread (sub-agent) registrations persist across messages.
        let parent_codex_thread_id = {
            let state = session.state.lock().await;
            state.codex_thread_id.clone()
        };
        let mut demux = CodexSubagentDemux::new(parent_codex_thread_id);
        // Codex's `total` is a lifetime counter that survives
        // `thread/resume`, but this demux is rebuilt with the session.
        // Seed what the ledger already holds so the first report after a
        // rebuild records only genuinely new work.
        demux.set_recorded_usage_baseline(session.recorded_usage_baseline);
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                maybe_note = notifications.recv() => {
                    match maybe_note {
                        Ok(note) => {
                            let msg = NotificationMessage::from_raw(&note.method, note.params);
                            // Update local state for a few special
                            // notifications before broadcasting.
                            update_state_from_notification(&session, &msg).await;
                            // Codex never states a model on the usage
                            // notification, so the ledger has to learn it
                            // from session state. Refreshed only on the
                            // usage notification itself — reading it on
                            // every notification would take a mutex on the
                            // per-token `item/updated` stream for nothing.
                            if matches!(msg, NotificationMessage::ThreadTokenUsageUpdated(_)) {
                                let model = session.state.lock().await.model.clone();
                                demux.set_active_model(model);
                            }
                            let events =
                                translate_notification_with(&mut demux, &session.thread_id, msg);
                            for ev in events {
                                if event_tx.send(ev).is_err() {
                                    // No subscribers — silently drop.
                                }
                            }
                            // Follow-up queue: a TurnCompleted notification
                            // returns the session to idle (Ready), an
                            // Error/child-exit tears it down. Drain the next
                            // queued turn when idle; cancel the whole queue
                            // when the session is gone. Runs after
                            // `update_state_from_notification` released the
                            // lock so `do_send` can re-acquire it.
                            let status = {
                                let state = session.state.lock().await;
                                state.status.clone()
                            };
                            match status {
                                SessionStatus::Closed | SessionStatus::Error { .. } => {
                                    session.cancel_all_queued().await;
                                }
                                _ => session.drain_queue().await,
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Lag is logged but ignored — the upstream
                            // buffer is large enough in practice.
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    })
}

/// Background task: consume server-initiated requests, record the
/// JSON-RPC id for later resolution, and emit a `RequestOpened` event.
fn spawn_incoming_requests_task(
    session: Arc<CodexSession>,
    child: Arc<JsonRpcChild>,
    mcp_registry: Option<McpRegistry>,
    mut incoming: tokio::sync::mpsc::Receiver<crate::json_rpc_child::IncomingRequest>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                maybe_req = incoming.recv() => {
                    let Some(req) = maybe_req else { break; };
                    let msg = ServerRequestMessage::from_raw(&req.method, req.params.clone());
                    if let ServerRequestMessage::ToolCall(params) = msg {
                        let result = handle_dynamic_tool_call(
                            mcp_registry.as_ref(),
                            session.workspace_id.as_deref(),
                            params,
                        )
                        .await;
                        if let Err(error) = child.respond(req.id, Ok(result)).await {
                            let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                                thread_id: Some(session.thread_id.clone()),
                                message: format!("failed to answer Codex dynamic tool call: {error}"),
                                original_payload: None,
                            });
                        }
                        continue;
                    }
                    let request_id = mint_request_id();
                    {
                        let mut state = session.state.lock().await;
                        state.pending_approvals.insert(request_id.clone(), req.id);
                    }
                    let event = translate_server_request(&session.thread_id, &request_id, &msg);
                    let _ = event_tx.send(event);
                }
            }
        }
    })
}

/// Execute a Codex dynamic-tool request through the process-wide registry and
/// translate the MCP content envelope into Codex's input-content shape.
async fn handle_dynamic_tool_call(
    registry: Option<&McpRegistry>,
    workspace_id: Option<&str>,
    raw: Value,
) -> Value {
    let parsed = serde_json::from_value::<DynamicToolCallParams>(raw);
    let (success, result) = match (registry, parsed) {
        (Some(registry), Ok(call)) => match registry
            .dispatch_tool_call(&registry_tool_name(&call.tool), call.arguments, workspace_id)
            .await
        {
            Ok(result) => {
                let success = !result
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                (success, result)
            }
            Err(message) => (
                false,
                json!({ "content": [{ "type": "text", "text": message }] }),
            ),
        },
        (None, _) => (
            false,
            json!({ "content": [{ "type": "text", "text": "Codemux MCP registry is unavailable" }] }),
        ),
        (_, Err(error)) => (
            false,
            json!({ "content": [{ "type": "text", "text": format!("invalid dynamic tool call: {error}") }] }),
        ),
    };

    let content_items = result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(mcp_content_to_codex).collect::<Vec<_>>())
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| vec![json!({ "type": "inputText", "text": result.to_string() })]);
    json!({ "contentItems": content_items, "success": success })
}

/// Codex reserves the `mcp__` prefix for MCP servers configured directly in
/// Codex. Shared tools arrive through the dynamic-tool API instead, so expose
/// a distinct, reversible namespace while keeping the registry's canonical
/// dispatch key provider-neutral.
fn codex_dynamic_tool_name(registry_name: &str) -> String {
    match registry_name.strip_prefix("mcp__") {
        Some(name) => format!("codemux_mcp__{name}"),
        None => format!("codemux_mcp__{registry_name}"),
    }
}

fn registry_tool_name(codex_name: &str) -> String {
    match codex_name.strip_prefix("codemux_mcp__") {
        Some(name) => format!("mcp__{name}"),
        None => codex_name.to_owned(),
    }
}

fn mcp_content_to_codex(item: &Value) -> Value {
    match item.get("type").and_then(Value::as_str) {
        Some("text") => json!({
            "type": "inputText",
            "text": item.get("text").and_then(Value::as_str).unwrap_or_default()
        }),
        Some("image") => binary_mcp_content(item, "inputImage", "imageUrl"),
        Some("audio") => binary_mcp_content(item, "inputAudio", "audioUrl"),
        _ => json!({ "type": "inputText", "text": item.to_string() }),
    }
}

fn binary_mcp_content(item: &Value, kind: &str, url_field: &str) -> Value {
    let data = item.get("data").and_then(Value::as_str).unwrap_or_default();
    let mime = item
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    let url = format!("data:{mime};base64,{data}");
    match url_field {
        "imageUrl" => json!({ "type": kind, "imageUrl": url }),
        _ => json!({ "type": kind, "audioUrl": url }),
    }
}

/// Background task: watch for child process exit. If the child dies
/// unexpectedly, flip the session status to `Error` and broadcast the
/// transition.
fn spawn_child_exit_watchdog(
    session: Arc<CodexSession>,
    child: Arc<JsonRpcChild>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if !child.is_alive() {
                        let status = {
                            let state = session.state.lock().await;
                            state.status.clone()
                        };
                        // Only flip to Error if the session wasn't
                        // already asked to close.
                        if !matches!(status, SessionStatus::Closed) {
                            // Mark the session dead so the provider's
                            // `has_session` reports it absent and the next send
                            // rebuilds via `ensure_live_session` with the resume
                            // cursor rather than routing to the dead child.
                            session.dead.store(true, Ordering::Relaxed);
                            let msg = "codex app-server exited unexpectedly".to_string();
                            // Recover the in-flight turn id before clearing it so
                            // the watchdog can settle it with a synthetic terminal
                            // `TurnCompleted` ahead of the Error state.
                            let active_turn = {
                                let mut state = session.state.lock().await;
                                let active_turn = state.active_turn.take();
                                state.status = SessionStatus::Error {
                                    message: msg.clone(),
                                };
                                active_turn
                            };
                            // TurnCompleted(child_exited) first, then the Error
                            // state — see `child_exit_events` for the ordering
                            // contract.
                            for event in crate::agent_provider::child_exit_events(
                                session.thread_id.clone(),
                                active_turn,
                                msg,
                            ) {
                                let _ = event_tx.send(event);
                            }
                            session.cancel_all_queued().await;
                        }
                        break;
                    }
                }
            }
        }
    })
}

/// Mutate session state in response to specific notifications before
/// they are translated/broadcast. Keeps the canonical state machine
/// in sync with Codex's view.
async fn update_state_from_notification(session: &CodexSession, msg: &NotificationMessage) {
    match msg {
        NotificationMessage::TurnStarted(p) => {
            let mut state = session.state.lock().await;
            // Only the parent thread drives the session's active turn;
            // a sub-agent's turn must never touch parent turn state.
            if p.thread_id == state.codex_thread_id {
                state.active_turn = Some(TurnId(p.turn_id.clone()));
            }
        }
        NotificationMessage::TurnCompleted(p) => {
            let mut state = session.state.lock().await;
            // Sub-agent turn completions are handled as SubagentUpdated in
            // the translator; leave the parent session state untouched.
            if p.thread_id != state.codex_thread_id {
                return;
            }
            if state
                .active_turn
                .as_ref()
                .map(|t| t.0 == p.turn_id)
                .unwrap_or(false)
            {
                state.active_turn = None;
            }
            // Status update follows from translate_turn_completed()
            // which emits a SessionStateChanged event; mirror it here so
            // adapter-internal reads stay consistent.
            state.status = match p.status.as_str() {
                "failed" | "error" => SessionStatus::Error {
                    message: p
                        .error
                        .clone()
                        .unwrap_or_else(|| "turn failed".into()),
                },
                _ => SessionStatus::Ready,
            };
        }
        NotificationMessage::Error(e) if !e.will_retry => {
            let mut state = session.state.lock().await;
            // Scope a terminal error to the parent session only when it is
            // unscoped or explicitly targets the parent thread; a
            // sub-agent's error must not fail the whole session.
            if e
                .thread_id
                .as_deref()
                .map_or(true, |t| t == state.codex_thread_id)
            {
                state.status = SessionStatus::Error {
                    message: e.message.clone(),
                };
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_identity_uses_the_resolved_turn_settings() {
        let mode = runtime_collaboration_mode(Some("gpt-5.6-sol"), Some("high")).unwrap();
        assert_eq!(mode.mode, "default");
        assert_eq!(mode.settings.model, "gpt-5.6-sol");
        assert_eq!(mode.settings.reasoning_effort.as_deref(), Some("high"));
        assert!(mode.settings.developer_instructions.ends_with(
            "<runtime_info>In case you're asked: you are running in CodeMux through the Codex harness, as gpt-5.6-sol with high reasoning effort. No need to mention this otherwise.</runtime_info>"
        ));

        assert!(runtime_collaboration_mode(None, Some("high")).is_none());
    }

    #[test]
    fn codex_plan_labels_are_humanized_and_unknowns_pass_through() {
        assert_eq!(codex_plan_label(Some("pro")).as_deref(), Some("ChatGPT Pro"));
        assert_eq!(codex_plan_label(Some("plus")).as_deref(), Some("ChatGPT Plus"));
        assert_eq!(codex_plan_label(Some("Team")).as_deref(), Some("ChatGPT Team"));
        // A plan tag we have never seen still gets a readable name
        // rather than being dropped.
        assert_eq!(
            codex_plan_label(Some("ultra")).as_deref(),
            Some("ChatGPT Ultra")
        );
        assert!(codex_plan_label(None).is_none());
        assert!(codex_plan_label(Some("  ")).is_none());
    }

    #[test]
    fn recoverable_resume_error_matches_snippets() {
        assert!(is_recoverable_resume_error("rpc error: Thread not found (code 42)"));
        assert!(is_recoverable_resume_error("unknown thread"));
        assert!(is_recoverable_resume_error("NO SUCH THREAD"));
        assert!(!is_recoverable_resume_error("network is unreachable"));
    }

    #[test]
    fn resume_cursor_accepts_native_and_persisted_shapes() {
        assert_eq!(
            codex_thread_id_from_resume_cursor(&json!({"threadId": "native-thread"})),
            Some("native-thread".into())
        );
        assert_eq!(
            codex_thread_id_from_resume_cursor(&json!({"resume": "persisted-thread"})),
            Some("persisted-thread".into())
        );
    }

    #[test]
    fn resume_cursor_prefers_provider_native_thread_id() {
        let both = json!({"threadId": "native", "resume": "persisted"});
        assert_eq!(
            codex_thread_id_from_resume_cursor(&both),
            Some("native".into())
        );
    }

    #[test]
    fn minted_request_ids_are_unique() {
        let a = mint_request_id();
        let b = mint_request_id();
        assert_ne!(a.0, b.0);
    }

    /// Build a bare `QueuedTurn` with a given id for reorder tests. Only
    /// `queued_id` is inspected by `promote_queued_to_front`.
    fn queued(id: &str) -> QueuedTurn {
        QueuedTurn {
            queued_id: id.to_string(),
            input: SendTurnInput {
                thread_id: ThreadId("t".into()),
                text: String::new(),
                display_text: None,
                images: vec![],
                skill_invocations: vec![],
                model_override: None,
                effort_override: None,
                permission_mode_override: None,
                client_nonce: None,
                turn_checkpoint: None,
            },
        }
    }

    #[test]
    fn promote_queued_to_front_unknown_id_is_noop() {
        let mut q: VecDeque<QueuedTurn> = [queued("a"), queued("b")].into();
        assert_eq!(promote_queued_to_front(&mut q, "missing"), None);
        let ids: Vec<_> = q.iter().map(|t| t.queued_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn promote_queued_to_front_moves_mid_queue_item() {
        let mut q: VecDeque<QueuedTurn> = [queued("a"), queued("b"), queued("c")].into();
        // Returns the item's original index so the reorder can be undone.
        assert_eq!(promote_queued_to_front(&mut q, "c"), Some(2));
        let ids: Vec<_> = q.iter().map(|t| t.queued_id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }

    #[test]
    fn promote_queued_to_front_head_item_stays_put() {
        let mut q: VecDeque<QueuedTurn> = [queued("a"), queued("b")].into();
        assert_eq!(promote_queued_to_front(&mut q, "a"), Some(0));
        let ids: Vec<_> = q.iter().map(|t| t.queued_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn restore_queued_position_undoes_a_promotion() {
        let mut q: VecDeque<QueuedTurn> = [queued("a"), queued("b"), queued("c")].into();
        let idx = promote_queued_to_front(&mut q, "c").unwrap();
        assert_eq!(idx, 2);
        restore_queued_position(&mut q, "c", idx);
        let ids: Vec<_> = q.iter().map(|t| t.queued_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn restore_queued_position_already_dispatched_is_noop() {
        // The item was promoted then removed (dispatched/cancelled)
        // before restore ran — restoring is a harmless no-op.
        let mut q: VecDeque<QueuedTurn> = [queued("a"), queued("b")].into();
        restore_queued_position(&mut q, "gone", 1);
        let ids: Vec<_> = q.iter().map(|t| t.queued_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn restore_queued_position_clamps_out_of_range_index() {
        // A stale original index past the current end clamps to the tail
        // rather than panicking.
        let mut q: VecDeque<QueuedTurn> = [queued("c"), queued("a")].into();
        restore_queued_position(&mut q, "c", 9);
        let ids: Vec<_> = q.iter().map(|t| t.queued_id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
    }

    #[test]
    fn mcp_binary_content_becomes_codex_data_urls() {
        assert_eq!(
            mcp_content_to_codex(&json!({
                "type": "image",
                "mimeType": "image/png",
                "data": "YWJj"
            })),
            json!({"type": "inputImage", "imageUrl": "data:image/png;base64,YWJj"})
        );
    }

    #[test]
    fn codex_dynamic_tool_alias_is_reversible_and_not_reserved() {
        let registry_name = "mcp__omarchy-kb__search_documentation";
        let codex_name = codex_dynamic_tool_name(registry_name);
        assert_eq!(codex_name, "codemux_mcp__omarchy-kb__search_documentation");
        assert!(!codex_name.starts_with("mcp__"));
        assert_eq!(registry_tool_name(&codex_name), registry_name);
    }

    #[tokio::test]
    async fn dynamic_tool_call_tags_builtin_dispatch_with_session_workspace() {
        // The session's workspace id must reach the built-in server as
        // `_meta[WORKSPACE_META_KEY]` on the outbound `tools/call`; the mock
        // only answers a request that carries it.
        let (_server, registry, call) =
            crate::mcp::registry::test_support::codemux_remote_server("ws-42").await;
        let response = handle_dynamic_tool_call(
            Some(&registry),
            Some("ws-42"),
            json!({
                "threadId": "thread",
                "turnId": "turn",
                "callId": "call",
                "namespace": null,
                "tool": "codemux_mcp__codemux-remote__echo",
                "arguments": {"text": "hi"}
            }),
        )
        .await;
        assert_eq!(response["success"], true, "{response}");
        assert_eq!(response["contentItems"][0]["text"], "routed");
        call.assert_async().await;
    }

    #[tokio::test]
    async fn dynamic_tool_call_without_registry_returns_failed_response() {
        let response = handle_dynamic_tool_call(
            None,
            None,
            json!({
                "threadId": "thread",
                "turnId": "turn",
                "callId": "call",
                "namespace": null,
                "tool": "mcp__demo__search",
                "arguments": {}
            }),
        )
        .await;
        assert_eq!(response["success"], false);
        assert!(response["contentItems"][0]["text"]
            .as_str()
            .unwrap()
            .contains("registry is unavailable"));
    }
}
