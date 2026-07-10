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
    SessionStatus, ThreadId, TurnId,
};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

use super::protocol::{
    AccountReadResponse, ApprovalResponse, Capabilities, ClientInfo, InitializeParams,
    NotificationMessage, ServerRequestMessage, ThreadResumeParams, ThreadStartParams,
    ThreadStartResponse, TurnInputItem, TurnInterruptParams, TurnStartParams, TurnStartResponse,
    RECOVERABLE_THREAD_RESUME_ERROR_SNIPPETS,
};
use super::translate::{translate_notification_with, translate_server_request, CodexSubagentDemux};

/// Default per-request timeout, mirroring the upstream reference's
/// `sendRequest(..., 20_000)` default.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(20);

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
        resume_cursor: Option<Value>,
        caller_env: Option<HashMap<String, String>>,
        spawn: CodexSpawnConfig,
        event_tx: broadcast::Sender<ProviderRuntimeEvent>,
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
        // account/read is the canonical "are you logged in?" check. If
        // `requires_openai_auth: true` the user has no credentials —
        // surface as `NotAuthenticated` upfront instead of letting
        // `thread/start` fail later with a cryptic message. RPC failure
        // (binary error, not auth) is logged but tolerated; the session
        // proceeds and turns will fail with the underlying error.
        match child.request("account/read", json!({})).await {
            Ok(resp) => match serde_json::from_value::<AccountReadResponse>(resp) {
                Ok(info) if info.requires_openai_auth => {
                    let _ = child.shutdown().await;
                    return Err(ProviderError::NotAuthenticated {
                        provider: crate::agent_provider::ProviderKind::Codex,
                        hint: "Run `codex login` and try again.".into(),
                    });
                }
                Ok(_) => {}
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

        // --- thread/start or thread/resume ----------------------------------
        let codex_thread_id = match &resume_cursor {
            Some(cursor) => {
                let resume_id = cursor
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
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
                            service_tier: None,
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
                                start_fresh_thread(&child, cwd.clone(), model.clone(), permission_mode.clone()).await?
                            }
                            Err(e) => {
                                return Err(ProviderError::RpcError {
                                    message: format!("thread/resume failed: {e}"),
                                })
                            }
                        }
                    }
                    None => start_fresh_thread(&child, cwd.clone(), model.clone(), permission_mode.clone()).await?,
                }
            }
            None => start_fresh_thread(&child, cwd.clone(), model.clone(), permission_mode.clone()).await?,
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
                let text = input.text.clone();
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
        let turn_id = self
            .do_send(
                input.text,
                input.images,
                input.model_override,
                input.effort_override,
            )
            .await?;
        Ok(SendOutcome::Started(turn_id))
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
            let text = queued.input.text.clone();
            match self
                .do_send(
                    queued.input.text,
                    queued.input.images,
                    queued.input.model_override,
                    queued.input.effort_override,
                )
                .await
            {
                Ok(turn_id) => {
                    let _ = self.event_tx().send(ProviderRuntimeEvent::QueuedTurnDispatched {
                        thread_id: self.thread_id.clone(),
                        queued_id: queued.queued_id,
                        turn_id,
                        text,
                    });
                    return;
                }
                Err(err) => {
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
    /// [`ProviderRuntimeEvent::QueuedTurnCancelled`] when found; idempotent.
    pub async fn cancel_queued(&self, queued_id: &str) -> Result<(), ProviderError> {
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
        Ok(())
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
        let mut input_items: Vec<TurnInputItem> = Vec::with_capacity(images.len() + 1);
        for img in images {
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(&img.data);
            input_items.push(TurnInputItem::Image {
                url: format!("data:{};base64,{}", img.media_type, encoded),
            });
        }
        let trimmed_text = text.trim();
        if !trimmed_text.is_empty() || input_items.is_empty() {
            input_items.push(TurnInputItem::Text {
                text,
                text_elements: vec![],
            });
        }

        let params = TurnStartParams {
            thread_id: codex_thread_id,
            input: input_items,
            model: model_override.or(model_default),
            service_tier: None,
            effort: effort_override.or(effort_default),
            collaboration_mode: None,
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
                .ok_or_else(|| ProviderError::ValidationError {
                    message: format!("no pending approval for request {}", request_id.0),
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
) -> Result<String, ProviderError> {
    let (approval_policy, sandbox) =
        match codex_permission_mode_to_policy_pair(permission_mode.as_deref()) {
            Some((ap, sb)) => (Some(ap), Some(sb)),
            None => (None, None),
        };
    let params = ThreadStartParams {
        model,
        service_tier: None,
        cwd: Some(cwd),
        collaboration_mode: None,
        approval_policy,
        sandbox,
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
    fn recoverable_resume_error_matches_snippets() {
        assert!(is_recoverable_resume_error("rpc error: Thread not found (code 42)"));
        assert!(is_recoverable_resume_error("unknown thread"));
        assert!(is_recoverable_resume_error("NO SUCH THREAD"));
        assert!(!is_recoverable_resume_error("network is unreachable"));
    }

    #[test]
    fn minted_request_ids_are_unique() {
        let a = mint_request_id();
        let b = mint_request_id();
        assert_ne!(a.0, b.0);
    }
}
