//! Per-thread Claude session state.
//!
//! One [`ClaudeSession`] per runtime thread. Each owns its own
//! sidecar subprocess (via [`JsonRpcChild`]) — deliberately NOT
//! multiplexed, because:
//!
//! * Memory of each session is isolated.
//! * One session's sidecar crashing doesn't take down others.
//! * The state model stays simple (no cross-session tracking inside
//!   the sidecar).
//!
//! Background tasks consume the sidecar's notification broadcast and
//! incoming-request mpsc, translate via [`super::translate`], and
//! fan canonical events out on the adapter's broadcaster.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::agent_provider::{
    ProviderError, ProviderRuntimeEvent, ProviderSessionId, RequestId, SessionStatus, ThreadId,
    TurnId,
};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

use super::protocol::{
    InterruptParams, RespondToRequestParams, RespondToUserInputParams, SendTurnImage,
    SendTurnParams, SetModelParams, SetPermissionModeParams, SidecarDecision,
    SidecarNotification, StartSessionParams, StartSessionResponse, StopSessionParams,
    StopSessionResponse, METHOD_INTERRUPT, METHOD_RESPOND_TO_REQUEST,
    METHOD_RESPOND_TO_USER_INPUT, METHOD_SEND_TURN, METHOD_SET_MODEL,
    METHOD_SET_PERMISSION_MODE, METHOD_START_SESSION, METHOD_STOP_SESSION,
};
use crate::agent_provider::ImageInput;
use base64::Engine;
use super::translate::{translate_notification_with, SubagentDemux};

/// Default per-RPC timeout for the sidecar. Matches the value
/// production integrations use.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(20);

/// Tighter timeout for `stop-session` — we do not want a broken
/// sidecar to hold up shutdown.
const STOP_RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// Information needed to spawn and drive a Claude session.
#[derive(Debug, Clone)]
pub struct ClaudeSpawnConfig {
    /// Path to the bundled claude-agent sidecar binary.
    pub sidecar_binary: PathBuf,
    /// Path to the user's local `claude` CLI, forwarded to the SDK
    /// via `pathToClaudeCodeExecutable`.
    pub claude_binary: PathBuf,
    /// Optional MCP registry. When set, the sidecar can issue
    /// `mcp-tool-call` server-initiated requests and the session's
    /// requests task forwards them to the registry's
    /// `dispatch_tool_call`. Tests pass `None`.
    pub mcp_registry: Option<crate::mcp::registry::McpRegistry>,
}

/// Mutable per-session state.
pub(crate) struct ClaudeSessionState {
    /// Current lifecycle status.
    pub status: SessionStatus,
    /// Currently active turn, if any. Claude is one-turn-at-a-time
    /// per thread.
    pub active_turn: Option<TurnId>,
    /// Map from adapter-side request id to the sidecar's request id.
    /// They are the same string in current sidecar code, but
    /// storing the mapping explicitly future-proofs against drift.
    pub pending_approvals: HashMap<RequestId, Value>,
    /// Session-wide default model (last call to `set_model`).
    pub model: Option<String>,
    /// Last-applied permission mode.
    pub permission_mode: Option<String>,
    /// SDK-assigned session id, learnt from the sidecar's
    /// `sdk-session-id` notification. `None` until the first SDK
    /// message arrives. Passed back to the sidecar as `resume` on a
    /// mid-session restart.
    pub sdk_session_id: Option<String>,
}

/// A live Claude session handle.
pub(crate) struct ClaudeSession {
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
    #[allow(dead_code)]
    pub cwd: PathBuf,
    pub sidecar: Arc<JsonRpcChild>,
    pub state: Mutex<ClaudeSessionState>,
    /// Broadcast sender used to emit runtime events from methods that
    /// mutate session status (e.g. `send_turn`, `interrupt`). Background
    /// tasks receive their own clone via spawn args.
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    /// Signal used to tell background tasks to exit.
    shutdown_tx: broadcast::Sender<()>,
    /// Handles retained so `shutdown()` can abort them.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Flag telling the watchdog "closed on purpose, don't emit
    /// SessionStateChanged::Error on exit".
    intentionally_closed: Arc<RwLock<bool>>,
    /// Optional MCP runtime registry — populated when the provider was
    /// constructed with `mcp_registry: Some(...)`. Used by
    /// `handle_mcp_tool_call` to route `mcp-tool-call` requests from
    /// the sidecar to the right MCP child.
    mcp_registry: Option<crate::mcp::registry::McpRegistry>,
}

impl ClaudeSession {
    /// Spawn a sidecar subprocess, send `start-session`, hook up the
    /// notification and server-request streams, and return the
    /// session handle.
    pub async fn spawn_and_initialize(
        thread_id: ThreadId,
        input: crate::agent_provider::StartSessionInput,
        spawn: ClaudeSpawnConfig,
        event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    ) -> Result<Arc<Self>, ProviderError> {
        // Spawn the sidecar.
        let sidecar = JsonRpcChild::spawn(SpawnConfig {
            program: spawn.sidecar_binary,
            args: vec![],
            env: HashMap::new(),
            cwd: Some(input.cwd.clone()),
            default_timeout: DEFAULT_RPC_TIMEOUT,
        })
        .await
        .map_err(|e| ProviderError::ProcessError {
            message: "failed to spawn claude-agent sidecar".into(),
            source: Some(e.to_string()),
        })?;
        let sidecar = Arc::new(sidecar);

        // Claim the single incoming-request receiver before any
        // background task starts.
        let incoming_rx = sidecar.incoming_requests().ok_or_else(|| {
            ProviderError::ProcessError {
                message: "sidecar incoming-request channel already claimed".into(),
                source: None,
            }
        })?;

        // Build `start-session` params from the trait-level input.
        //
        // First-class fields on `StartSessionInput` (effort,
        // context_window) win over keys in the free-form `extra`
        // passthrough. Context window is encoded into the model id
        // itself via `resolve_claude_api_model_id` — Anthropic's API
        // expects the `[1m]` suffix as part of the model string.
        let resolved_model = input
            .model
            .as_deref()
            .map(|m| {
                crate::agent_provider::claude::capabilities::resolve_claude_api_model_id(
                    m,
                    input.context_window.as_deref(),
                )
            });
        let resolved_effort = input.effort.clone().or_else(|| {
            input
                .extra
                .get("effort")
                .and_then(|v| v.as_str())
                .map(String::from)
        });
        let params = StartSessionParams {
            thread_id: thread_id.0.clone(),
            cwd: input.cwd.clone(),
            model: resolved_model,
            effort: resolved_effort,
            permission_mode: input.permission_mode.clone(),
            allow_dangerously_skip_permissions: match input.permission_mode.as_deref() {
                Some("bypassPermissions") => Some(true),
                _ => None,
            },
            additional_directories: if input.additional_directories.is_empty() {
                None
            } else {
                Some(input.additional_directories.clone())
            },
            settings: input
                .extra
                .get("settings")
                .cloned(),
            resume: input
                .resume_cursor
                .as_ref()
                .and_then(|c| c.get("resume").or_else(|| c.get("sessionId")))
                .and_then(|v| v.as_str())
                .map(String::from),
            session_id: input
                .extra
                .get("sessionId")
                .and_then(|v| v.as_str())
                .map(String::from),
            path_to_claude_code_executable: spawn.claude_binary.clone(),
            extra_args: input
                .extra
                .get("extraArgs")
                .and_then(|v| v.as_object())
                .cloned(),
            // Stage 3 — snapshot every running MCP tool from the
            // registry and pass it through to the sidecar so the SDK
            // can register the in-process `codemux` virtual MCP
            // server. Tools added after session start aren't visible
            // until the chat is restarted (Stage 4 polish will add a
            // `setMcpServers` push path so dynamic refreshes work).
            mcp_tools: collect_mcp_tools(spawn.mcp_registry.as_ref()).await,
        };
        let params_value = serde_json::to_value(&params).map_err(|e| ProviderError::ProcessError {
            message: "failed to serialize start-session params".into(),
            source: Some(e.to_string()),
        })?;
        let resp = sidecar
            .request(METHOD_START_SESSION, params_value)
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("start-session RPC failed: {e}"),
            })?;
        let parsed: StartSessionResponse =
            serde_json::from_value(resp).map_err(|e| ProviderError::RpcError {
                message: format!("malformed start-session response: {e}"),
            })?;

        let provider_session_id = ProviderSessionId(parsed.thread_id.clone());
        let (shutdown_tx, _) = broadcast::channel(4);
        let state = Mutex::new(ClaudeSessionState {
            status: SessionStatus::Ready,
            active_turn: None,
            pending_approvals: HashMap::new(),
            model: input.model.clone(),
            permission_mode: input.permission_mode.clone(),
            sdk_session_id: None,
        });
        let session = Arc::new(Self {
            thread_id: thread_id.clone(),
            provider_session_id: provider_session_id.clone(),
            cwd: input.cwd,
            sidecar: Arc::clone(&sidecar),
            state,
            event_tx: event_tx.clone(),
            shutdown_tx: shutdown_tx.clone(),
            tasks: Mutex::new(Vec::new()),
            intentionally_closed: Arc::new(RwLock::new(false)),
            mcp_registry: spawn.mcp_registry.clone(),
        });

        // Announce the session up front.
        let _ = event_tx.send(ProviderRuntimeEvent::SessionConfigured {
            thread_id: thread_id.clone(),
            provider_session_id,
        });
        let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: thread_id.clone(),
            status: SessionStatus::Ready,
        });

        // Background tasks.
        let notifications_task = spawn_notifications_task(
            Arc::clone(&session),
            Arc::clone(&sidecar),
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
            Arc::clone(&sidecar),
            event_tx.clone(),
            shutdown_tx.subscribe(),
        );
        // Stage 4 — dynamic MCP tool refresh. Subscribe to the
        // registry's status broadcaster; whenever a server transitions
        // (Running ↔ Stopped / Errored), re-collect tools and push the
        // fresh snapshot to the sidecar via `update-mcp-tools`. The
        // task does nothing when no registry is configured.
        let mcp_refresh_task = spawn_mcp_refresh_task(
            Arc::clone(&session),
            shutdown_tx.subscribe(),
        );

        {
            let mut guard = session.tasks.lock().await;
            guard.push(notifications_task);
            guard.push(requests_task);
            guard.push(watchdog_task);
            guard.push(mcp_refresh_task);
        }

        Ok(session)
    }

    /// Queue a user turn. Sidecar synthesizes the SDK turn id —
    /// this method returns our adapter-local [`TurnId`].
    pub async fn send_turn(
        &self,
        text: String,
        images: Vec<ImageInput>,
        model_override: Option<String>,
    ) -> Result<TurnId, ProviderError> {
        {
            let state = self.state.lock().await;
            if let Some(active) = &state.active_turn {
                return Err(ProviderError::ValidationError {
                    message: format!(
                        "session has an active turn ({}); wait or interrupt",
                        active.0
                    ),
                });
            }
        }
        // Encode raw bytes as standard base64 here so the JSON-RPC
        // frame stays text-only and the sidecar pipes the data
        // straight into Anthropic's `image/base64` content block
        // shape without re-decoding.
        let encoded_images = images
            .into_iter()
            .map(|img| SendTurnImage {
                media_type: img.media_type,
                data_base64: base64::engine::general_purpose::STANDARD.encode(&img.data),
            })
            .collect();
        let params = SendTurnParams {
            thread_id: self.thread_id.0.clone(),
            text,
            model_override,
            images: encoded_images,
        };
        self.sidecar
            .request(
                METHOD_SEND_TURN,
                serde_json::to_value(&params).unwrap(),
            )
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("send-turn RPC failed: {e}"),
            })?;
        let turn_id = TurnId(format!("claude-turn-{}", uuid::Uuid::new_v4()));
        {
            let mut state = self.state.lock().await;
            state.active_turn = Some(turn_id.clone());
            state.status = SessionStatus::Running {
                active_turn: turn_id.clone(),
            };
        }
        // Announce the state flip so subscribers (composer, reducer)
        // learn that a turn is in flight. Claude's SDK never emits a
        // text delta for a pure-tool turn, so without this event the
        // frontend's streaming flag would stay false for the entire
        // turn and let the user queue a second one.
        let _ = self.event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: self.thread_id.clone(),
            status: SessionStatus::Running {
                active_turn: turn_id.clone(),
            },
        });
        Ok(turn_id)
    }

    pub async fn interrupt(&self, turn_id: Option<TurnId>) -> Result<(), ProviderError> {
        let active = {
            let state = self.state.lock().await;
            state.active_turn.clone()
        };
        if let (Some(requested), Some(active)) = (turn_id.as_ref(), active.as_ref()) {
            if requested != active {
                return Err(ProviderError::ValidationError {
                    message: format!(
                        "turn id mismatch: requested {}, active {}",
                        requested.0, active.0
                    ),
                });
            }
        }
        let params = InterruptParams {
            thread_id: self.thread_id.0.clone(),
        };
        self.sidecar
            .request(METHOD_INTERRUPT, serde_json::to_value(&params).unwrap())
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("interrupt RPC failed: {e}"),
            })?;
        // Announce Ready before mutating local state so subscribers
        // never observe `Ready` in the struct with no corresponding
        // event in flight. The SessionEnded notification that the
        // sidecar will emit next translates to `Closed`, so this is
        // the only place Ready is surfaced after an interrupt.
        let _ = self.event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: self.thread_id.clone(),
            status: SessionStatus::Ready,
        });
        let mut state = self.state.lock().await;
        state.active_turn = None;
        state.status = SessionStatus::Ready;
        Ok(())
    }

    pub async fn set_model(&self, model: Option<String>) -> Result<(), ProviderError> {
        let params = SetModelParams {
            thread_id: self.thread_id.0.clone(),
            model: model.clone(),
        };
        self.sidecar
            .request(METHOD_SET_MODEL, serde_json::to_value(&params).unwrap())
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("set-model RPC failed: {e}"),
            })?;
        let mut state = self.state.lock().await;
        state.model = model;
        Ok(())
    }

    pub async fn set_permission_mode(&self, mode: String) -> Result<(), ProviderError> {
        let params = SetPermissionModeParams {
            thread_id: self.thread_id.0.clone(),
            mode: mode.clone(),
        };
        self.sidecar
            .request(
                METHOD_SET_PERMISSION_MODE,
                serde_json::to_value(&params).unwrap(),
            )
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("set-permission-mode RPC failed: {e}"),
            })?;
        let mut state = self.state.lock().await;
        state.permission_mode = Some(mode);
        Ok(())
    }

    pub async fn respond_to_request(
        &self,
        request_id: RequestId,
        decision: crate::agent_provider::ApprovalDecision,
    ) -> Result<(), ProviderError> {
        // Resolve any local bookkeeping first so a later RPC error
        // doesn't leave the map in a bad state.
        {
            let mut state = self.state.lock().await;
            state.pending_approvals.remove(&request_id);
        }
        let wire: SidecarDecision = decision.into();
        let params = RespondToRequestParams {
            thread_id: self.thread_id.0.clone(),
            request_id: request_id.0.clone(),
            decision: wire,
        };
        let res = self
            .sidecar
            .request(
                METHOD_RESPOND_TO_REQUEST,
                serde_json::to_value(&params).unwrap(),
            )
            .await;
        match res {
            Ok(_) => Ok(()),
            Err(crate::json_rpc_child::RpcChildError::RpcError(err))
                if err.code == -32602 =>
            {
                Err(ProviderError::ValidationError {
                    message: format!(
                        "request {} not found or already resolved",
                        request_id.0
                    ),
                })
            }
            Err(e) => Err(ProviderError::RpcError {
                message: format!("respond-to-request RPC failed: {e}"),
            }),
        }
    }

    /// Reserved for the AskUserQuestion flow. Unused until the UI
    /// surfaces that path.
    #[allow(dead_code)]
    pub async fn respond_to_user_input(
        &self,
        request_id: RequestId,
        answers: Vec<Value>,
    ) -> Result<(), ProviderError> {
        let params = RespondToUserInputParams {
            thread_id: self.thread_id.0.clone(),
            request_id: request_id.0,
            answers,
        };
        self.sidecar
            .request(
                METHOD_RESPOND_TO_USER_INPUT,
                serde_json::to_value(&params).unwrap(),
            )
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("respond-to-user-input RPC failed: {e}"),
            })?;
        Ok(())
    }

    /// Exposed for the trait-level init-result call. Currently
    /// invoked only from ad-hoc paths; trait does not require it.
    #[allow(dead_code)]
    pub async fn initialization_result(&self) -> Result<Value, ProviderError> {
        self.sidecar
            .request(
                super::protocol::METHOD_INITIALIZATION_RESULT,
                json!({ "threadId": self.thread_id.0 }),
            )
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("initialization-result RPC failed: {e}"),
            })
    }

    /// Close the session: send `stop-session` (with tight timeout),
    /// signal background tasks to exit, shut the sidecar down,
    /// flip local state.
    pub async fn shutdown(&self) {
        {
            let mut flag = self.intentionally_closed.write().await;
            *flag = true;
        }

        // Best-effort stop-session RPC — ignore errors, we're
        // tearing down anyway.
        let params = StopSessionParams {
            thread_id: self.thread_id.0.clone(),
        };
        let _ = tokio::time::timeout(
            STOP_RPC_TIMEOUT,
            self.sidecar.request_with_timeout(
                METHOD_STOP_SESSION,
                serde_json::to_value(&params).unwrap_or(Value::Null),
                STOP_RPC_TIMEOUT,
            ),
        )
        .await;

        // Signal tasks to break out of their loops.
        let _ = self.shutdown_tx.send(());

        // Abort anything still running.
        let tasks: Vec<_> = {
            let mut guard = self.tasks.lock().await;
            std::mem::take(&mut *guard)
        };
        for t in tasks {
            t.abort();
        }

        // Close the sidecar's JSON-RPC child (EOF → 2s grace → kill).
        let _ = self.sidecar.shutdown().await;

        // Flip local status.
        {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Closed;
            state.active_turn = None;
            // Pending approvals never get a response — drop them so
            // a later `respond_to_request` sees an empty map.
            state.pending_approvals.clear();
        }
    }
}

/// Replace an empty [`TurnId`] on an event with the session's
/// currently-active turn id. No-op when the event already carries a
/// non-empty turn id or when the session has no active turn.
///
/// Claude's SDK does not expose per-turn identifiers — every message
/// carries `session_id` but no `turn_id`. The translator falls back to
/// an empty string, which breaks every frontend consumer that keys on
/// turn_id. We stamp the adapter's own turn id (minted in `send_turn`)
/// onto events as they leave the broadcaster.
fn stamp_turn_id(event: &mut ProviderRuntimeEvent, active: Option<&TurnId>) {
    let Some(active) = active else { return };
    if active.0.is_empty() {
        return;
    }
    match event {
        ProviderRuntimeEvent::ContentDelta { turn_id, .. }
        | ProviderRuntimeEvent::ItemCompleted { turn_id, .. }
        | ProviderRuntimeEvent::TurnCompleted { turn_id, .. }
        | ProviderRuntimeEvent::RequestOpened { turn_id, .. } => {
            if turn_id.0.is_empty() {
                *turn_id = active.clone();
            }
        }
        _ => {}
    }
}

/// Detect whether a `stop-session` response indicates the sidecar
/// had already forgotten the thread. Used as a light sanity check.
#[cfg_attr(not(test), allow(dead_code))]
fn stop_response_indicates_already_closed(value: &Value) -> bool {
    serde_json::from_value::<StopSessionResponse>(value.clone())
        .map(|r| r.already_closed)
        .unwrap_or(false)
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
    }
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

fn spawn_notifications_task(
    session: Arc<ClaudeSession>,
    sidecar: Arc<JsonRpcChild>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut notifications = sidecar.notifications();
        // Per-session subagent demux state. Owned by this single
        // notification task and passed by `&mut` into the translator so
        // subagent launch → inner routing → progress → completion spans
        // multiple messages. Task-local: no lock, no cross-task sharing.
        let mut demux = SubagentDemux::default();
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                maybe_note = notifications.recv() => {
                    match maybe_note {
                        Ok(note) => {
                            // Update local state for specific notifications
                            // before broadcasting.
                            let structured = SidecarNotification::from_method_params(
                                &note.method,
                                note.params.clone(),
                            );
                            mutate_state_from_notification(&session, &structured).await;
                            // Catch any panic in the pure translator so
                            // a bug can't kill this task.
                            let events = std::panic::catch_unwind(
                                std::panic::AssertUnwindSafe(|| {
                                    translate_notification_with(
                                        &session.thread_id,
                                        structured,
                                        &mut demux,
                                    )
                                }),
                            )
                            .unwrap_or_else(|_| {
                                vec![ProviderRuntimeEvent::RuntimeWarning {
                                    thread_id: Some(session.thread_id.clone()),
                                    message: "panic while translating sidecar notification".into(),
                                    original_payload: Some(note.params),
                                }]
                            });
                            // Claude SDK messages carry session_id
                            // but no per-turn id, so the translator
                            // emits events with `turn_id = ""`. Stamp
                            // the session's active turn before
                            // broadcasting so frontend consumers can
                            // key merges (delta coalescing) and
                            // lookups by turn_id. Without this, the
                            // frontend's find-trailing-assistant
                            // match-by-turn_id collapses across turns
                            // and the 2nd turn's deltas overwrite the
                            // 1st turn's assistant message.
                            let active_turn = {
                                let state = session.state.lock().await;
                                state.active_turn.clone()
                            };
                            for mut e in events {
                                stamp_turn_id(&mut e, active_turn.as_ref());
                                let _ = event_tx.send(e);
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                                thread_id: Some(session.thread_id.clone()),
                                message: format!("sidecar notification channel lagged, dropped {n}"),
                                original_payload: None,
                            });
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    })
}

fn spawn_incoming_requests_task(
    session: Arc<ClaudeSession>,
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

                    // Stage 3 — the sidecar issues `mcp-tool-call`
                    // server-initiated requests to dispatch agent tool
                    // calls back through the runtime registry. Each
                    // request handler runs in its own task so a slow
                    // tool doesn't block other incoming requests.
                    if req.method == METHOD_MCP_TOOL_CALL {
                        let session_for_task = Arc::clone(&session);
                        let event_tx_for_task = event_tx.clone();
                        tokio::spawn(async move {
                            handle_mcp_tool_call(session_for_task, event_tx_for_task, req).await;
                        });
                        continue;
                    }

                    // Anything else is unexpected.
                    let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                        thread_id: Some(session.thread_id.clone()),
                        message: format!(
                            "unexpected sidecar server-initiated request: {}",
                            req.method
                        ),
                        original_payload: Some(req.params),
                    });
                    let _ = session
                        .sidecar
                        .respond(
                            req.id,
                            Err(crate::json_rpc_child::RpcError {
                                code: -32601,
                                message: "adapter does not handle server-initiated requests".into(),
                                data: None,
                            }),
                        )
                        .await;
                }
            }
        }
    })
}

/// Method name the sidecar uses for tool dispatch. Centralised so the
/// session and the sidecar share a single source of truth (mirrored in
/// `sidecar/claude-agent/src/mcp-bridge.ts` as `METHOD_MCP_TOOL_CALL`).
pub const METHOD_MCP_TOOL_CALL: &str = "mcp-tool-call";

/// How long to wait between status events before issuing a refresh.
/// Bursts during `prime_for_chat` arrive within ~50ms; debouncing
/// prevents N rapid `setMcpServers` calls when the user toggles
/// servers in quick succession.
const MCP_REFRESH_DEBOUNCE: Duration = Duration::from_millis(250);

/// Listen on the registry's status broadcaster and push fresh tool
/// snapshots to the live SDK session whenever a server transitions.
/// Debounced — bursty status changes coalesce into one refresh.
fn spawn_mcp_refresh_task(
    session: Arc<ClaudeSession>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Some(registry) = session.mcp_registry.clone() else {
            // No registry — nothing to subscribe to. Task exits
            // cleanly so the spawn is harmless even in tests.
            return;
        };
        let mut rx = registry.subscribe_status();
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                event = rx.recv() => {
                    match event {
                        Ok(_row) => {
                            // Coalesce rapid bursts. After a status event
                            // arrives, wait MCP_REFRESH_DEBOUNCE for more
                            // events; if any arrive, restart the timer.
                            // Once silent for the debounce window, push
                            // one refresh.
                            loop {
                                match tokio::time::timeout(
                                    MCP_REFRESH_DEBOUNCE,
                                    rx.recv(),
                                )
                                .await
                                {
                                    Ok(Ok(_)) => continue,
                                    Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                                    Ok(Err(broadcast::error::RecvError::Closed)) => return,
                                    Err(_) => break, // debounce window elapsed
                                }
                            }
                            push_mcp_refresh(&session, &registry).await;
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    })
}

/// Snapshot the registry's tool list and ship it to the sidecar via
/// `update-mcp-tools`. Failures are logged via a runtime warning so a
/// transient sidecar hiccup doesn't kill the refresh task.
async fn push_mcp_refresh(
    session: &Arc<ClaudeSession>,
    registry: &crate::mcp::registry::McpRegistry,
) {
    let tools = registry.list_all_tools().await;
    let entries: Vec<super::protocol::McpToolEntry> = tools
        .into_iter()
        .map(|t| super::protocol::McpToolEntry {
            name: t.name,
            prefixed_name: t.prefixed_name,
            description: t.description,
            input_schema: t.input_schema,
            server_id: t.server_id,
        })
        .collect();

    let params = super::protocol::UpdateMcpToolsParams {
        thread_id: session.thread_id.0.clone(),
        mcp_tools: entries,
    };
    let params_value = match serde_json::to_value(&params) {
        Ok(v) => v,
        Err(_) => return,
    };
    if let Err(err) = session
        .sidecar
        .request(
            super::protocol::METHOD_UPDATE_MCP_TOOLS,
            params_value,
        )
        .await
    {
        let _ = session.event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(session.thread_id.clone()),
            message: format!("update-mcp-tools failed: {err}"),
            original_payload: None,
        });
    }
}

/// Snapshot every running MCP tool from the registry into the wire
/// shape the sidecar expects. Returns an empty `Vec` when no registry
/// is configured or no tools are running — both fine, the sidecar
/// just doesn't register the in-process MCP server.
async fn collect_mcp_tools(
    registry: Option<&crate::mcp::registry::McpRegistry>,
) -> Vec<super::protocol::McpToolEntry> {
    let Some(registry) = registry else {
        return Vec::new();
    };
    let tools = registry.list_all_tools().await;
    tools
        .into_iter()
        .map(|t| super::protocol::McpToolEntry {
            name: t.name,
            prefixed_name: t.prefixed_name,
            description: t.description,
            input_schema: t.input_schema,
            server_id: t.server_id,
        })
        .collect()
}

/// Format a tool result payload the way Anthropic's MCP SDK expects:
/// `{ content: [{ type: "text", text: ... }], isError?: bool }`. The
/// sidecar passes this verbatim to the SDK's tool callback.
fn tool_result_text(text: impl Into<String>, is_error: bool) -> Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": is_error,
    })
}

async fn handle_mcp_tool_call(
    session: Arc<ClaudeSession>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    req: crate::json_rpc_child::IncomingRequest,
) {
    // Validate params shape.
    let params = req.params.clone();
    let prefixed_name = match params.get("name").and_then(Value::as_str) {
        Some(n) => n.to_string(),
        None => {
            let _ = session
                .sidecar
                .respond(
                    req.id,
                    Err(crate::json_rpc_child::RpcError {
                        code: -32602,
                        message: "mcp-tool-call requires a string `name`".into(),
                        data: None,
                    }),
                )
                .await;
            return;
        }
    };
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    // Look up the registry. If the provider was constructed without
    // one (tests, headless contexts) surface a clean error to the
    // SDK rather than panicking.
    let Some(registry) = session.mcp_registry.clone() else {
        let _ = session
            .sidecar
            .respond(
                req.id,
                Ok(tool_result_text(
                    "MCP runtime not available in this Codemux build",
                    true,
                )),
            )
            .await;
        return;
    };

    match registry.dispatch_tool_call(&prefixed_name, arguments).await {
        Ok(result) => {
            // The MCP child returns the full `tools/call` result —
            // including `content` and optional `isError`. Forward it
            // verbatim. If the shape is wrong (no `content` key)
            // wrap it so the SDK still sees a valid shape.
            let payload = if result.get("content").is_some() {
                result
            } else {
                tool_result_text(result.to_string(), false)
            };
            let _ = session.sidecar.respond(req.id, Ok(payload)).await;
        }
        Err(message) => {
            let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(session.thread_id.clone()),
                message: format!("mcp-tool-call {prefixed_name} failed: {message}"),
                original_payload: None,
            });
            let _ = session
                .sidecar
                .respond(req.id, Ok(tool_result_text(message, true)))
                .await;
        }
    }
}

fn spawn_child_exit_watchdog(
    session: Arc<ClaudeSession>,
    sidecar: Arc<JsonRpcChild>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if !sidecar.is_alive() {
                        // If the caller asked for shutdown, the status
                        // transition to Closed is handled by
                        // ClaudeSession::shutdown.
                        let intentional = *session.intentionally_closed.read().await;
                        if !intentional {
                            let msg = "claude-agent sidecar exited unexpectedly".to_string();
                            {
                                let mut state = session.state.lock().await;
                                state.status = SessionStatus::Error {
                                    message: msg.clone(),
                                };
                                state.active_turn = None;
                            }
                            let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
                                thread_id: session.thread_id.clone(),
                                status: SessionStatus::Error { message: msg },
                            });
                        }
                        break;
                    }
                }
            }
        }
    })
}

/// Mutate session state based on specific notification variants
/// before the translator runs.
async fn mutate_state_from_notification(
    session: &ClaudeSession,
    notification: &SidecarNotification,
) {
    match notification {
        SidecarNotification::RequestOpened {
            request_id,
            tool_input,
            ..
        } => {
            let mut state = session.state.lock().await;
            state
                .pending_approvals
                .insert(RequestId(request_id.clone()), tool_input.clone());
        }
        SidecarNotification::SdkSessionId { session_id, .. } => {
            let mut state = session.state.lock().await;
            state.sdk_session_id = Some(session_id.clone());
        }
        SidecarNotification::SdkMessage { message, .. } => {
            // The SDK's `result` message marks the end of a single
            // turn. The session stays alive (the prompt queue is
            // persistent) so `session-ended` does NOT fire between
            // turns — that notification only appears on full
            // teardown. Without this hook, `state.active_turn` stays
            // populated forever and `send_turn` rejects every
            // subsequent turn with "session has an active turn".
            //
            // We deliberately do NOT emit `SessionStateChanged(Ready)`
            // from here — `translate_result` (called right after this
            // function returns) emits `TurnCompleted` which the
            // frontend reducer already treats as the
            // streaming=false signal. Emitting both events created
            // a Ready → TurnCompleted ordering inversion that briefly
            // re-armed the send button before the turn was visually
            // complete.
            if message.get("type").and_then(|v| v.as_str()) == Some("result") {
                let mut state = session.state.lock().await;
                state.active_turn = None;
                if matches!(state.status, SessionStatus::Running { .. }) {
                    state.status = SessionStatus::Ready;
                }
            }
        }
        SidecarNotification::SessionEnded { reason, .. } => {
            let mut state = session.state.lock().await;
            state.active_turn = None;
            state.status = if reason == "iteration-complete" {
                SessionStatus::Ready
            } else {
                SessionStatus::Closed
            };
        }
        SidecarNotification::SessionError { error, .. } => {
            let mut state = session.state.lock().await;
            state.status = SessionStatus::Error {
                message: error.message.clone(),
            };
            state.active_turn = None;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_response_indicates_already_closed_parses() {
        assert!(stop_response_indicates_already_closed(&json!({
            "alreadyClosed": true
        })));
        assert!(!stop_response_indicates_already_closed(&json!({})));
    }
}
