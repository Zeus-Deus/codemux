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

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::agent_provider::{
    ProviderError, ProviderRuntimeEvent, ProviderSessionId, RequestId, SendOutcome, SendTurnInput,
    SessionStatus, ThreadId, TurnId,
};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

use super::protocol::{
    InterruptParams, RespondToRequestParams, RespondToUserInputParams, SendTurnImage,
    SendTurnParams, SetModelParams, SetPermissionModeParams, SidecarDecision, SidecarNotification,
    StartSessionParams, StartSessionResponse, StopSessionParams, StopSessionResponse,
    METHOD_INTERRUPT, METHOD_RESPOND_TO_REQUEST, METHOD_RESPOND_TO_USER_INPUT, METHOD_SEND_TURN,
    METHOD_SET_MODEL, METHOD_SET_PERMISSION_MODE, METHOD_START_SESSION, METHOD_STOP_SESSION,
};
use super::translate::{translate_notification_with, SubagentDemux};
use crate::agent_provider::ImageInput;

mod queue_edits;
mod steer;
use base64::Engine;

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
    /// Follow-up turns queued while a turn was in flight, in FIFO
    /// dispatch order. Drained one-at-a-time as the session returns to
    /// idle (see [`ClaudeSession::drain_queue`]).
    pub queued_turns: VecDeque<QueuedTurn>,
    /// Set while a `drain_queue` has popped an item and is mid-flight in
    /// `do_send`, before `active_turn` is populated. Guards against two
    /// concurrent drains (notifications task + inline interrupt fallback)
    /// each popping and dispatching an item in the same idle window.
    pub dispatching: bool,
    /// Set while a compaction requested by a plugin (see
    /// [`super::idle_compaction`]) runs on the live query. User turns queue
    /// behind it, exactly as they would behind a turn, and its `result` is
    /// not a user turn ending.
    pub idle_compaction: Option<IdleRun>,
    /// Holds the queue after a user Stop until an explicit send.
    pub(crate) queue_hold: crate::agent_provider::queue_hold::QueueHold,
    /// Guidance delivered into the running turn; see [`steer`].
    pub(crate) steer: steer::SteerState,
}

/// One idle compaction in flight. The claim is released only once the SDK
/// is known to be done with it, so its late output can never land on a
/// user turn dispatched after it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IdleRun {
    id: u64,
    /// The SDK reported the compaction boundary, so the history really
    /// was replaced by a summary.
    compacted: bool,
    /// The run reported its end (its `result`, or an interrupt).
    settled: bool,
    /// An interrupt confirmed the query stopped.
    stopped: bool,
    /// The run outlived its timeout and is being stopped; the claim stays
    /// until that is confirmed (see `abandon_idle_compaction`).
    abandoning: bool,
    /// The stop was never confirmed and the session is torn down, but its
    /// child is not reaped yet, so the run may still write native history.
    draining: bool,
}

/// What one sidecar notification means for an idle compaction that owned
/// the session when the notification arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdleSignal {
    /// A main-thread SDK message that neither compacts nor ends the run.
    Message,
    /// The SDK's `compact_boundary`: the history was replaced.
    Boundary,
    /// The main-thread `result`: the run ended.
    Result,
    /// `turn-interrupted`: the query stopped.
    Interrupted,
    /// The session ended or failed; the run is over with it.
    Gone,
    /// Anything else (approvals, subagent traffic, ...): left alone.
    Unrelated,
}

impl IdleSignal {
    fn of(notification: &SidecarNotification) -> Self {
        match notification {
            SidecarNotification::SdkMessage { message, .. } => {
                if super::translate::parent_tool_use_id(message).is_some() {
                    return IdleSignal::Message;
                }
                match message.get("type").and_then(Value::as_str) {
                    Some("result") => IdleSignal::Result,
                    Some("system")
                        if message.get("subtype").and_then(Value::as_str)
                            == Some("compact_boundary") =>
                    {
                        IdleSignal::Boundary
                    }
                    _ => IdleSignal::Message,
                }
            }
            SidecarNotification::TurnInterrupted { .. } => IdleSignal::Interrupted,
            SidecarNotification::SessionEnded { .. } | SidecarNotification::SessionError { .. } => {
                IdleSignal::Gone
            }
            _ => IdleSignal::Unrelated,
        }
    }
}

/// After the timeout's interrupt RPC returns, how long to wait for the
/// sidecar's `turn-interrupted` (written before the RPC response) to be
/// processed before deciding the query did not stop.
const IDLE_STOP_GRACE: Duration = Duration::from_secs(2);

impl ClaudeSessionState {
    /// Whether the SDK is mid-turn from Codemux's point of view.
    ///
    /// `active_turn` only covers turns Codemux dispatched. The SDK can also
    /// start a turn on its own after a `result` — e.g. when background
    /// subagents finish and their notifications wake the main agent — and
    /// that turn can park on a `canUseTool` callback (AskUserQuestion, a
    /// tool approval) with `active_turn` still `None`. A pending callback
    /// means the SDK is blocked on the user, so it counts as busy: a new
    /// message sent now would sit behind the unanswered callback forever,
    /// and hydrate must keep the request answerable instead of expiring it.
    pub(crate) fn turn_busy(&self) -> bool {
        self.active_turn.is_some() || !self.pending_approvals.is_empty()
    }

    /// Whether the SDK cannot take a new prompt right now. Unlike
    /// [`turn_busy`](Self::turn_busy) this includes an idle compaction,
    /// which is not a run the UI should show as live.
    pub(crate) fn sdk_busy(&self) -> bool {
        self.turn_busy() || self.idle_compaction.is_some()
    }
}

/// A user turn parked in the follow-up queue behind the active turn.
#[derive(Debug, Clone)]
pub(crate) struct QueuedTurn {
    /// Backend-generated id, stable for the lifetime of the queued item.
    /// The UI keys its greyed bubble and cancel button on this.
    pub queued_id: String,
    /// The original send input, replayed verbatim when the turn
    /// dispatches.
    pub input: SendTurnInput,
}

/// Mint a queue id unique within this process.
fn mint_queued_id() -> String {
    format!("claude-queued-{}", uuid::Uuid::new_v4())
}

/// Move the queued turn with `queued_id` to the front of `queue` so it
/// is the next item [`ClaudeSession::drain_queue`] dispatches. Returns
/// `Some(original_index)` when the item was found (and is now at the
/// front), `None` when no such id is queued (already dispatched /
/// cancelled / unknown). A no-op when the item is already at the front.
/// The returned index lets a caller undo the reorder via
/// [`restore_queued_position`] when a follow-up step (e.g. `interrupt`)
/// fails.
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

/// A live Claude session handle.
pub(crate) struct ClaudeSession {
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
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
    /// Set by the child-exit watchdog when the sidecar dies *unintentionally*
    /// mid-session. [`is_dead`](ClaudeSession::is_dead) reports it so the
    /// provider's `has_session` treats the corpse as absent and the next send
    /// rebuilds a fresh session (with the resume cursor) via
    /// `ensure_live_session` instead of routing to a dead child.
    ///
    /// Deliberately NOT keyed off `SessionStatus::Error`: only the watchdog
    /// (a confirmed dead child) sets this. A transient error status set by a
    /// `session-error` notification while the sidecar is still alive must stay
    /// recoverable in place without a teardown/rebuild.
    dead: Arc<AtomicBool>,
    /// Optional MCP runtime registry — populated when the provider was
    /// constructed with `mcp_registry: Some(...)`, and always `None` for a
    /// session on a device. Used by `handle_mcp_tool_call` to route
    /// `mcp-tool-call` requests from the sidecar to the right MCP child.
    mcp_registry: Option<crate::mcp::registry::McpRegistry>,
    /// Owning workspace of the chat pane this session runs in, from
    /// `StartSessionInput.workspace_id`. Attached per-call to MCP
    /// dispatches so workspace-scoped built-in tools bind to THIS
    /// session's workspace — the registry's shared MCP child cannot
    /// learn the caller from its env.
    workspace_id: Option<String>,
    native_authority: Arc<crate::delegation::authority::NativeAuthority>,
    managed: Option<Arc<crate::agent_provider::managed::ManagedSession>>,
    managed_diagnostics: bool,
    /// What a plugin-requested compaction needs to honour the CLI's own
    /// opt-outs.
    compaction: CompactionSupport,
}

struct CompactionSupport {
    /// Local, unmanaged chats only: a device's CLI reads that device's
    /// settings, which this computer cannot see, so its opt-outs couldn't
    /// be honoured; managed workflow attempts own their whole run.
    eligible: bool,
    /// `--settings` object the CLI was launched with.
    flag_settings: Option<Value>,
    /// Env overlay the sidecar (and so the CLI) was launched with.
    env: HashMap<String, String>,
    next_id: std::sync::atomic::AtomicU64,
    /// Give-up time for one compaction, in milliseconds
    /// ([`super::idle_compaction::IN_FLIGHT_TIMEOUT`]; tests shorten it).
    timeout_ms: std::sync::atomic::AtomicU64,
}

/// Spawn the bundled sidecar on this computer in `cwd`.
async fn spawn_local_sidecar(
    sidecar_binary: &std::path::Path,
    env: HashMap<String, String>,
    cwd: PathBuf,
    managed: bool,
) -> Result<JsonRpcChild, ProviderError> {
    let sidecar_config = SpawnConfig {
        program: sidecar_binary.to_path_buf(),
        args: vec![],
        env,
        cwd: Some(cwd),
        default_timeout: DEFAULT_RPC_TIMEOUT,
    };
    if managed {
        JsonRpcChild::spawn_managed(sidecar_config).await
    } else {
        JsonRpcChild::spawn(sidecar_config).await
    }
    .map_err(|e| match e {
        crate::json_rpc_child::RpcChildError::SpawnFailed(ref io) if managed => {
            ProviderError::ValidationError {
                message: format!("managed-start-rejected: setup_required: failed to spawn Claude workflow sidecar: {io}"),
            }
        }
        // A missing sidecar binary is an install problem, not a
        // generic process failure — classify it so the UI can render
        // the actionable "not installed" state (mirrors the probe
        // path in `claude/auth.rs`).
        crate::json_rpc_child::RpcChildError::SpawnFailed(ref io)
            if io.kind() == std::io::ErrorKind::NotFound =>
        {
            ProviderError::NotInstalled {
                provider: crate::agent_provider::ProviderKind::Claude,
                hint: format!(
                    "claude-agent sidecar not found at {}",
                    sidecar_binary.display()
                ),
            }
        }
        other => ProviderError::ProcessError {
            message: "failed to spawn claude-agent sidecar".into(),
            source: Some(other.to_string()),
        },
    })
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
        //
        // Overlay the caller-supplied workspace env (CODEMUX_WORKSPACE_ID,
        // CODEMUX_PANE_ID, BROWSER, …) so the agent's Bash subprocesses route
        // `codemux browser open` at their OWN workspace instead of whichever
        // one the user is viewing. `JsonRpcChild` applies these as overlays on
        // the inherited env, so an empty map (no workspace resolved) is a
        // no-op and the sidecar inherits Codemux's env unchanged.
        let managed = crate::agent_provider::managed::lookup_session(&thread_id);
        if managed.is_some() && !cfg!(target_os = "linux") {
            return Err(ProviderError::ValidationError {
                message: "managed-start-rejected: managed Claude requires Linux process containment".into(),
            });
        }
        let budget_error = |message: String| ProviderError::ValidationError {
            message: if managed.is_some() {
                format!("managed-start-rejected: {message}")
            } else {
                message
            },
        };
        let max_budget_usd = optional_positive_budget(&input.extra).map_err(budget_error)?;
        let max_turns = optional_positive_turns(&input.extra).map_err(budget_error)?;
        if managed.is_some() && input.resume_cursor.is_some() {
            return Err(ProviderError::ValidationError {
                message: "managed-start-rejected: managed attempts cannot resume an unscoped native session".into(),
            });
        }
        let sidecar_binary = spawn.sidecar_binary;
        let env = input.env.clone().unwrap_or_default();
        let compaction_env = env.clone();
        let sidecar = match input.remote.as_ref() {
            // On a device the runtime installed there runs over `ssh -T`;
            // `input.cwd` is already a path on that device.
            #[cfg(unix)]
            Some(remote) => {
                let claude =
                    crate::ssh::sidecar::device_program(remote, &spawn.claude_binary).await?;
                JsonRpcChild::spawn(crate::ssh::sidecar::claude_spawn_config(
                    remote,
                    &claude,
                    &env,
                    &input.cwd,
                    DEFAULT_RPC_TIMEOUT,
                )?)
                .await
                .map_err(crate::ssh::sidecar::ssh_spawn_error)?
            }
            #[cfg(not(unix))]
            Some(_) => {
                return Err(ProviderError::ValidationError {
                    message: "Threads on another device need macOS or Linux on this computer"
                        .into(),
                })
            }
            None => {
                spawn_local_sidecar(&sidecar_binary, env, input.cwd.clone(), managed.is_some())
                    .await?
            }
        };
        let sidecar = Arc::new(sidecar);
        let mut startup_guard = sidecar.managed_startup_guard();
        if let Some(context) = managed.as_ref() {
            let recorded = sidecar
                .managed_evidence()
                .map_err(|e| e.to_string())
                .and_then(|evidence| context.handler.record_runtime(evidence));
            if let Err(error) = recorded {
                sidecar
                    .shutdown_managed()
                    .await
                    .map_err(|stop| ProviderError::ProcessError {
                        message: "managed Claude startup stop unconfirmed".into(),
                        source: Some(stop.to_string()),
                    })?;
                return Err(ProviderError::ValidationError {
                    message: format!("managed-start-rejected: {error}"),
                });
            }
        }

        // Claim the single incoming-request receiver before any
        // background task starts.
        let incoming_rx =
            sidecar
                .incoming_requests()
                .ok_or_else(|| ProviderError::ProcessError {
                    message: "sidecar incoming-request channel already claimed".into(),
                    source: None,
                })?;

        // A session on a device publishes none of this computer's tools and
        // keeps no registry to run the device's tool calls through.
        let mcp_registry = crate::agent_provider::local_tool_registry(
            spawn.mcp_registry.as_ref(),
            input.remote.as_ref(),
        );

        // Build `start-session` params from the trait-level input.
        //
        // First-class fields on `StartSessionInput` (effort,
        // context_window) win over keys in the free-form `extra`
        // passthrough. Context window is encoded into the model id
        // itself via `resolve_claude_api_model_id` — Anthropic's API
        // expects the `[1m]` suffix as part of the model string.
        let resolved_model = input.model.as_deref().map(|m| {
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
        let mut params = StartSessionParams {
            thread_id: thread_id.0.clone(),
            cwd: input.cwd.clone(),
            model: resolved_model,
            effort: resolved_effort,
            // Claude Fast is disabled in our capabilities until entitlement
            // and served-speed checks exist. Do not revive an older saved
            // Fast choice as a hidden premium override on resume.
            fast_mode: Some(false),
            max_budget_usd,
            max_turns,
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
            settings: input.extra.get("settings").cloned(),
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
            // On a device the bare name resolves through the PATH the
            // remote launch sets up; a local path would mean nothing there.
            path_to_claude_code_executable: if input.remote.is_some() {
                PathBuf::from("claude")
            } else {
                spawn.claude_binary.clone()
            },
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
            mcp_tools: match managed.as_ref() {
                Some(context) => context
                    .handler
                    .tools()
                    .into_iter()
                    .map(|tool| super::protocol::McpToolEntry {
                        name: tool.name.clone(),
                        prefixed_name: tool.name,
                        description: Some(tool.description),
                        input_schema: tool.input_schema,
                        server_id: "workflow".into(),
                    })
                    .collect(),
                None => {
                    collect_mcp_tools(
                        mcp_registry.as_ref(),
                        &crate::mcp::registry::HostCaller {
                            control: None,
                            thread_id: thread_id.0.clone(),
                            provider: crate::agent_provider::ProviderKind::Claude,
                            workspace_id: input.workspace_id.clone(),
                            permission_mode: input.permission_mode.clone(),
                        },
                    )
                    .await
                }
            },
            append_system_prompt: None,
        };
        if managed.is_none() {
            params.append_system_prompt = system_prompt_append(&params.mcp_tools);
        }
        let mut params_value =
            serde_json::to_value(&params).map_err(|e| ProviderError::ProcessError {
                message: "failed to serialize start-session params".into(),
                source: Some(e.to_string()),
            })?;
        if managed.is_some() {
            params_value["managed"] = json!(true);
        }
        let resp = match sidecar.request(METHOD_START_SESSION, params_value).await {
            Ok(response) => response,
            Err(error) if managed.is_some() => {
                // No user prompt is sent before this managed readiness ACK.
                // Known-zero rejection still requires actual group shutdown.
                sidecar
                    .shutdown_managed()
                    .await
                    .map_err(|stop| ProviderError::ProcessError {
                        message: "managed Claude initialization stop unconfirmed".into(),
                        source: Some(stop.to_string()),
                    })?;
                return Err(ProviderError::ValidationError {
                    message: format!(
                        "managed-start-rejected: native tool initialization failed: {error}"
                    ),
                });
            }
            Err(error) => {
                // A runtime that dies before answering on a device is an
                // install or connection problem there; name the device.
                #[cfg(unix)]
                if let Some(error) = input
                    .remote
                    .as_ref()
                    .and_then(|remote| crate::ssh::sidecar::claude_start_error(&error, remote))
                {
                    return Err(error);
                }
                return Err(ProviderError::RpcError {
                    message: format!("start-session RPC failed: {error}"),
                });
            }
        };
        let parsed: StartSessionResponse =
            serde_json::from_value(resp).map_err(|e| ProviderError::RpcError {
                message: format!("malformed start-session response: {e}"),
            })?;

        let provider_session_id = ProviderSessionId(parsed.thread_id.clone());
        let (shutdown_tx, _) = broadcast::channel(4);
        if let Some(guard) = startup_guard.as_mut() { guard.set_task_shutdown(shutdown_tx.clone()); }
        let state = Mutex::new(ClaudeSessionState {
            status: SessionStatus::Ready,
            active_turn: None,
            pending_approvals: HashMap::new(),
            model: input.model.clone(),
            permission_mode: input.permission_mode.clone(),
            sdk_session_id: None,
            queued_turns: VecDeque::new(),
            dispatching: false,
            idle_compaction: None,
            queue_hold: Default::default(),
            steer: Default::default(),
        });
        let compaction = CompactionSupport {
            eligible: managed.is_none() && input.remote.is_none(),
            flag_settings: input.extra.get("settings").cloned(),
            env: compaction_env,
            next_id: Default::default(),
            timeout_ms: (super::idle_compaction::IN_FLIGHT_TIMEOUT.as_millis() as u64).into(),
        };
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
            dead: Arc::new(AtomicBool::new(false)),
            mcp_registry: if managed.is_some() {
                None
            } else {
                mcp_registry
            },
            workspace_id: input.workspace_id.clone(),
            native_authority: crate::delegation::authority::NativeAuthority::new(&thread_id.0),
            managed_diagnostics: input
                .extra
                .get("managedDiagnostics")
                .and_then(Value::as_bool)
                == Some(true),
            managed,
            compaction,
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
        // task does nothing when no registry is configured, which
        // includes every session on a device.
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

        if let Some(guard) = startup_guard.as_mut() { guard.disarm(); }
        Ok(session)
    }

    /// Run Claude Code's `/compact` on this idle session, on behalf of a
    /// plugin. Every gate is re-checked here: the CLI's own opt-outs, then,
    /// under the lifecycle lock, that nothing runs or waits. The claim made
    /// under that lock makes a user send arriving from then on queue behind
    /// the compaction instead of racing it.
    pub async fn request_compaction(
        self: &Arc<Self>,
    ) -> Result<(), super::idle_compaction::Refusal> {
        use super::idle_compaction::{self as idle, Refusal};
        use crate::json_rpc_child::RpcChildError;
        if !self.compaction.eligible {
            return Err(Refusal::Unsupported);
        }
        if self.is_dead() {
            return Err(Refusal::Closed);
        }
        let env = |key: &str| {
            self.compaction
                .env
                .get(key)
                .cloned()
                .or_else(|| idle::process_env(key))
        };
        let settings =
            idle::ClaudeSettings::load(&self.cwd, self.compaction.flag_settings.as_ref(), &env);
        if let Some(refusal) = settings.opt_out(&env) {
            return Err(refusal);
        }
        let id = self
            .compaction
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        {
            let mut state = self.state.lock().await;
            match state.status {
                SessionStatus::Closed => return Err(Refusal::Closed),
                SessionStatus::Starting | SessionStatus::Error { .. } => {
                    return Err(Refusal::NotReady)
                }
                SessionStatus::Ready
                | SessionStatus::Running { .. }
                | SessionStatus::WaitingApproval { .. } => {}
            }
            if state.sdk_busy()
                || state.dispatching
                || !matches!(state.status, SessionStatus::Ready)
            {
                return Err(Refusal::Busy);
            }
            if !state.queued_turns.is_empty() {
                return Err(Refusal::Queued);
            }
            state.idle_compaction = Some(IdleRun {
                id,
                compacted: false,
                settled: false,
                stopped: false,
                abandoning: false,
                draining: false,
            });
            // Announced with the claim, so no event of the run (which can
            // only start once the RPC below reaches the sidecar) can
            // overtake it. The SDK reports its own `compacting` too.
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::ContextCompactionChanged {
                    thread_id: self.thread_id.clone(),
                    active: true,
                });
        }
        let ack = self
            .sidecar
            .request(
                super::protocol::METHOD_COMPACT_IDLE,
                json!({ "threadId": self.thread_id.0 }),
            )
            .await;
        let refusal = match &ack {
            Ok(ack) if ack.get("started").and_then(Value::as_bool) == Some(true) => None,
            Ok(ack) => Some(match ack.get("reason").and_then(Value::as_str) {
                Some("closed") => Refusal::Closed,
                Some("query_dead" | "mcp_refresh") => Refusal::NotReady,
                _ => Refusal::Failed,
            }),
            // The sidecar never ran the method: nothing was queued.
            Err(
                RpcChildError::RpcError(_)
                | RpcChildError::AlreadyShutdown
                | RpcChildError::CancelledBeforeWrite
                | RpcChildError::SpawnFailed(_)
                | RpcChildError::ChildExited { .. },
            ) => Some(Refusal::Failed),
            // Unknown outcome: `/compact` may be running. Keep the claim
            // and stop it the same way a timed-out run is stopped.
            Err(error) => {
                eprintln!(
                    "[codemux::claude] compact-idle outcome unknown for {}: {error}",
                    self.thread_id.0
                );
                let session = Arc::clone(self);
                tokio::spawn(async move { session.abandon_idle_compaction(id).await });
                return Err(Refusal::Failed);
            }
        };
        if let Some(refusal) = refusal {
            if self.take_idle_claim(id).await.is_some() {
                self.emit_compaction_ended();
                self.drain_queue().await;
            }
            return Err(refusal);
        }
        eprintln!(
            "[codemux::claude] idle compaction started for {}",
            self.thread_id.0
        );
        // Never leave follow-ups stuck behind a compaction that never settles.
        let timeout = Duration::from_millis(self.compaction.timeout_ms.load(Ordering::Relaxed));
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            if let Some(session) = weak.upgrade() {
                session.abandon_idle_compaction(id).await;
            }
        });
        Ok(())
    }

    /// Release the claim of compaction `id`, if it still holds it.
    async fn take_idle_claim(&self, id: u64) -> Option<IdleRun> {
        let mut state = self.state.lock().await;
        if state.idle_compaction.is_some_and(|run| run.id == id) {
            state.idle_compaction.take()
        } else {
            None
        }
    }

    fn emit_compaction_ended(&self) {
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::ContextCompactionChanged {
                thread_id: self.thread_id.clone(),
                active: false,
            });
    }

    /// Stop compaction `id`, which outlived its timeout (or whose start
    /// RPC had an unknown outcome), before letting queued sends through.
    /// Only a stop the sidecar confirmed (`turn-interrupted`) releases the
    /// claim: the run's own `result` does not, since an interrupt already
    /// sent can still land afterwards. Without that confirmation the query
    /// can't be trusted with a user turn, so the session is torn down while
    /// it keeps the claim, and with it the output filter and the queue, for
    /// good; the next send rebuilds it from the resume cursor.
    pub(crate) async fn abandon_idle_compaction(self: &Arc<Self>, id: u64) {
        {
            let mut state = self.state.lock().await;
            match state.idle_compaction.as_mut() {
                Some(run) if run.id == id && !run.abandoning => run.abandoning = true,
                _ => return,
            }
        }
        eprintln!(
            "[codemux::claude] stopping an unsettled idle compaction for {}",
            self.thread_id.0
        );
        let params = InterruptParams {
            thread_id: self.thread_id.0.clone(),
        };
        let interrupted = self
            .sidecar
            .request(METHOD_INTERRUPT, serde_json::to_value(&params).unwrap())
            .await
            .is_ok();
        if interrupted {
            let deadline = tokio::time::Instant::now() + IDLE_STOP_GRACE;
            loop {
                let stopped = {
                    let state = self.state.lock().await;
                    match state.idle_compaction {
                        Some(run) if run.id == id => run.stopped,
                        _ => return,
                    }
                };
                if stopped {
                    let run = self.take_idle_claim(id).await;
                    if run.is_some_and(|run| !run.settled) {
                        self.emit_compaction_ended();
                    }
                    self.drain_queue().await;
                    return;
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        eprintln!(
            "[codemux::claude] idle compaction did not confirm its stop for {}; restarting its session",
            self.thread_id.0
        );
        let settled = {
            let mut state = self.state.lock().await;
            let Some(run) = state.idle_compaction.as_mut().filter(|run| run.id == id) else {
                return;
            };
            // Under the lifecycle lock: from here the provider treats the
            // session as gone, and the app's next send rebuilds it.
            self.dead.store(true, Ordering::Relaxed);
            run.draining = true;
            let settled = run.settled;
            run.settled = true;
            settled
        };
        if !settled {
            self.emit_compaction_ended();
        }
        // The claim is never released: sends still queue behind it and the
        // filter stays on while the child drains; the watchdog cancels the
        // queue when it exits, as for any dead sidecar.
        let _ = self.sidecar.shutdown().await;
        if let Some(run) = self.state.lock().await.idle_compaction.as_mut() {
            if run.id == id {
                run.draining = false;
            }
        }
        self.cancel_all_queued().await;
    }

    /// Whether a compaction torn down by
    /// [`abandon_idle_compaction`](Self::abandon_idle_compaction) may still
    /// be writing: the session is already dead, but its child is not reaped
    /// yet.
    pub(crate) async fn compaction_draining(&self) -> bool {
        self.state
            .lock()
            .await
            .idle_compaction
            .is_some_and(|run| run.draining)
    }

    /// Route the events of one notification that arrived while compaction
    /// `owner` held the session (`owner` is read before the notification
    /// touches session state, so a claim made after it arrived never owns
    /// it). The compaction is not a user turn: only its own facts (the
    /// compacting marker, context size, resume cursor, usage) pass through.
    /// Turn starts/ends and transcript items are dropped, so there is no
    /// "Working" state, no "finished" ping and no reply bubble, and a
    /// failed compaction stays silent as in Claude Code (the CLI's error
    /// text would otherwise render as an assistant reply). Its `result` (or
    /// a confirmed interrupt) ends the run and adds the notice when it
    /// compacted.
    async fn settle_idle_events(
        self: &Arc<Self>,
        owner: Option<u64>,
        signal: IdleSignal,
        mut events: Vec<ProviderRuntimeEvent>,
    ) -> Vec<ProviderRuntimeEvent> {
        let Some(owner) = owner else {
            return events;
        };
        if signal == IdleSignal::Unrelated {
            return events;
        }
        let mut state = self.state.lock().await;
        let Some(run) = state.idle_compaction.as_mut().filter(|run| run.id == owner) else {
            return events;
        };
        if signal == IdleSignal::Gone && !run.abandoning {
            // The session itself ended or failed; that is the user's to see.
            let settled = run.settled;
            state.idle_compaction = None;
            drop(state);
            if !settled {
                events.push(ProviderRuntimeEvent::ContextCompactionChanged {
                    thread_id: self.thread_id.clone(),
                    active: false,
                });
            }
            return events;
        }
        if signal == IdleSignal::Boundary {
            run.compacted = true;
        }
        if signal == IdleSignal::Interrupted {
            run.stopped = true;
        }
        events.retain(|event| {
            matches!(
                event,
                ProviderRuntimeEvent::ContextCompactionChanged { .. }
                    | ProviderRuntimeEvent::ContextUsageUpdated { .. }
                    | ProviderRuntimeEvent::ResumeCursorUpdated { .. }
                    | ProviderRuntimeEvent::UsageRecorded { .. }
                    | ProviderRuntimeEvent::PlanUsageUpdated { .. }
            )
        });
        let ends = matches!(signal, IdleSignal::Result | IdleSignal::Interrupted);
        if !ends || run.settled {
            return events;
        }
        run.settled = true;
        let compacted = run.compacted;
        // A run being stopped keeps its claim until the stop is confirmed.
        if !run.abandoning {
            state.idle_compaction = None;
        }
        drop(state);
        // The marker must not outlive a compaction that never reported its
        // own end (e.g. one that failed before compacting).
        events.push(ProviderRuntimeEvent::ContextCompactionChanged {
            thread_id: self.thread_id.clone(),
            active: false,
        });
        if compacted {
            eprintln!(
                "[codemux::claude] idle compaction finished for {}",
                self.thread_id.0
            );
            events.push(ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(self.thread_id.clone()),
                message: super::idle_compaction::notice_message(),
                original_payload: None,
            });
        } else {
            eprintln!(
                "[codemux::claude] idle compaction ended without compacting for {}",
                self.thread_id.0
            );
        }
        events
    }

    /// Tests only: shorten the compaction give-up time.
    pub(crate) fn set_compaction_timeout(&self, timeout: Duration) {
        self.compaction
            .timeout_ms
            .store(timeout.as_millis() as u64, Ordering::Relaxed);
    }

    /// The live state a plugin's `host.chat.getStatus` reads.
    pub async fn runtime_status(&self) -> crate::agent_provider::ThreadRuntimeStatus {
        let state = self.state.lock().await;
        crate::agent_provider::ThreadRuntimeStatus {
            busy: state.sdk_busy() || state.dispatching,
            queued: Some(state.queued_turns.len()),
            compacting: state.idle_compaction.is_some(),
            status: crate::agent_provider::ThreadRuntimeStatus::status_name(&state.status)
                .into(),
        }
    }

    pub(crate) fn managed_startup_guard(&self) -> Option<crate::json_rpc_child::ManagedStartupGuard> {
        let mut guard = self.sidecar.managed_startup_guard()?;
        guard.set_task_shutdown(self.shutdown_tx.clone());
        Some(guard)
    }

    /// Send a user turn, or **queue** it behind the active turn.
    ///
    /// When the session is idle the turn starts immediately and the
    /// method returns [`SendOutcome::Started`]. When a turn is already in
    /// flight the input is pushed onto the FIFO follow-up queue, a
    /// [`ProviderRuntimeEvent::TurnQueued`] is emitted, and the method
    /// returns [`SendOutcome::Queued`] — the turn dispatches later via
    /// [`drain_queue`](Self::drain_queue). The busy check re-runs under
    /// the state lock so there is no TOCTOU with a turn that finished
    /// between the frontend send and this call.
    pub async fn enqueue_or_send(
        self: &Arc<Self>,
        input: SendTurnInput,
    ) -> Result<SendOutcome, ProviderError> {
        // Busy check under the lock. `turn_busy` also covers a pending
        // approval/question raised by an SDK-initiated turn, where
        // `active_turn` is `None` but the SDK cannot read new input until
        // the callback is answered.
        let was_held = {
            let mut state = self.state.lock().await;
            // A reserved dispatch is about to start a turn; go behind it.
            // An idle compaction holds the SDK the same way a turn does.
            if state.sdk_busy() || state.dispatching {
                if input.dispatch_guard.is_some() {
                    return Err(ProviderError::ValidationError {
                        message: "Guarded result cannot queue behind a busy parent".into(),
                    });
                }
                let queued_id = mint_queued_id();
                let text = input
                    .display_text
                    .clone()
                    .unwrap_or_else(|| input.text.clone());
                let client_nonce = input.client_nonce.clone();
                let queued = QueuedTurn {
                    queued_id: queued_id.clone(),
                    input,
                };
                // Sent by hand while a Stop settles: it goes first and
                // releases the held queue behind it once it starts.
                let first = queued.input.releases_queue_hold() && state.queue_hold.goes_first();
                if first {
                    state.queued_turns.push_front(queued);
                    state.queue_hold.release_with(&queued_id);
                } else {
                    state.queued_turns.push_back(queued);
                }
                let _ = self.event_tx.send(ProviderRuntimeEvent::TurnQueued {
                    provenance: Default::default(),
                    thread_id: self.thread_id.clone(),
                    queued_id: queued_id.clone(),
                    client_nonce,
                    text,
                });
                if first {
                    let order = crate::agent_provider::queue_edit::reordered(&state.queued_turns, &self.thread_id);
                    let _ = self.event_tx.send(order);
                }
                drop(state);
                return Ok(SendOutcome::Queued(queued_id));
            }
            // A message sent by hand goes first; the held queue follows it.
            // Reserve dispatch so a drain cannot start a queued turn before
            // this one becomes active.
            let was_held = input.releases_queue_hold() && state.queue_hold.take();
            if was_held {
                state.dispatching = true;
            }
            was_held
        };
        let checkpoint = input.turn_checkpoint.clone();
        if let Some(checkpoint) = checkpoint.as_ref() {
            checkpoint.prepare().await;
        }
        let sent = self
            .do_send(
                input.text,
                input.images,
                input.model_override,
                input.dispatch_guard,
                checkpoint.clone(),
            )
            .await;
        if was_held {
            let mut state = self.state.lock().await;
            state.dispatching = false;
            state.queue_hold.settle_take(was_held, sent.is_ok());
        }
        let outcome = match sent {
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
        };
        if was_held && outcome.is_ok() {
            // A drain skipped while dispatch was reserved; the turn may
            // already have settled.
            self.drain_queue().await;
        }
        outcome
    }

    /// Pop the next queued turn (if the session is idle) and dispatch it.
    /// No-op when the queue is empty or a turn is still active. Emits
    /// [`ProviderRuntimeEvent::QueuedTurnDispatched`] on success; on a
    /// dispatch failure the item is cancelled (never wedges the queue)
    /// and the next item is attempted.
    pub async fn drain_queue(self: &Arc<Self>) {
        loop {
            // Pop the next item only if the session is truly idle: no
            // active turn, Ready status, nothing parked on approval, and
            // no concurrent drain already mid-dispatch. The `dispatching`
            // flag closes the gap between popping here and `do_send`
            // setting `active_turn` — without it a second drain sees the
            // session idle and pops the NEXT item, double-dispatching.
            let next = {
                let mut state = self.state.lock().await;
                let idle = state.active_turn.is_none()
                    && matches!(state.status, SessionStatus::Ready)
                    && state.pending_approvals.is_empty()
                    && state.idle_compaction.is_none()
                    && !state.dispatching;
                if !idle {
                    return;
                }
                let state = &mut *state;
                let next = state.queued_turns.iter().position(|q| state.queue_hold.can_select(&q.queued_id, q.input.queued_delivery_state()));
                match next.and_then(|i| state.queued_turns.remove(i)) {
                    Some(queued) => {
                        state.dispatching = true;
                        Some(queued)
                    }
                    None => None,
                }
            };
            let Some(queued) = next else { return };
            let retained_input = queued.input.clone();
            let text = queued
                .input
                .display_text
                .clone()
                .unwrap_or_else(|| queued.input.text.clone());
            let checkpoint = queued.input.turn_checkpoint.clone();
            if retained_input.queued_delivery_state() == crate::agent_provider::types::QueuedDeliveryState::Disposed {
                self.state.lock().await.dispatching = false;
                log::debug!("Disposed queued wake {}", queued.queued_id);
                let _ = self.event_tx.send(ProviderRuntimeEvent::QueuedTurnCancelled { thread_id: self.thread_id.clone(), queued_id: queued.queued_id });
                continue;
            }
            if let Some(checkpoint) = checkpoint.as_ref() {
                checkpoint.prepare().await;
            }
            match self
                .do_send(
                    queued.input.text,
                    queued.input.images,
                    queued.input.model_override,
                    queued.input.dispatch_guard,
                    checkpoint.clone(),
                )
                .await
            {
                Ok(turn_id) => {
                    if let Some(checkpoint) = checkpoint.as_ref() {
                        checkpoint.commit().await;
                    }
                    {
                        let mut state = self.state.lock().await;
                        state.dispatching = false;
                        state.queue_hold.settle(&queued.queued_id, true);
                    }
                    let _ = self
                        .event_tx
                        .send(ProviderRuntimeEvent::QueuedTurnDispatched {
                            steered: false,
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
                    {
                        let mut state = self.state.lock().await;
                        state.dispatching = false;
                        state.queue_hold.settle(&queued.queued_id, false);
                    }
                    match retained_input.queued_delivery_state() {
                        crate::agent_provider::types::QueuedDeliveryState::Held => {
                            let mut state = self.state.lock().await;
                            state.queued_turns.push_front(QueuedTurn { queued_id: queued.queued_id, input: retained_input });
                            state.queue_hold.hold();
                            return;
                        }
                        crate::agent_provider::types::QueuedDeliveryState::Blocked => {
                            self.state.lock().await.queued_turns.push_front(QueuedTurn { queued_id: queued.queued_id, input: retained_input });
                            continue;
                        }
                        crate::agent_provider::types::QueuedDeliveryState::Disposed => {
                            log::debug!("Disposed queued wake {}", queued.queued_id);
                            let _ = self.event_tx.send(ProviderRuntimeEvent::QueuedTurnCancelled { thread_id: self.thread_id.clone(), queued_id: queued.queued_id });
                            continue;
                        }
                        crate::agent_provider::types::QueuedDeliveryState::Ready => {}
                    }
                    // Don't wedge the queue: surface the failure, cancel
                    // this item, and try the next one.
                    let _ = self.event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                        thread_id: Some(self.thread_id.clone()),
                        message: format!(
                            "queued turn {} failed to dispatch: {err}",
                            queued.queued_id
                        ),
                        original_payload: None,
                    });
                    let _ = self
                        .event_tx
                        .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                            thread_id: self.thread_id.clone(),
                            queued_id: queued.queued_id,
                        });
                    continue;
                }
            }
        }
    }

    /// Cancel a single queued turn by id (user pressed X). Emits
    /// [`ProviderRuntimeEvent::QueuedTurnCancelled`] when the item was
    /// found. Returns whether the queued item was actually removed; an
    /// unknown / already-dispatched id returns `false`.
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
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread_id.clone(),
                    queued_id: queued_id.to_string(),
                });
        }
        Ok(removed)
    }

    /// **Send now (steer):** promote a queued follow-up to the front of
    /// the queue and dispatch it immediately, interrupting the active
    /// turn if one is running. This is a *soft* stop — the SDK session,
    /// the full transcript, and all on-disk work are preserved; nothing
    /// is discarded. The promoted message then dispatches as a normal
    /// follow-up turn, so the agent re-plans with the user's steer.
    ///
    /// Idempotent: an unknown / already-dispatched / already-cancelled id
    /// is a silent no-op `Ok(())` (the same philosophy as
    /// [`cancel_queued`](Self::cancel_queued) — the item may have just
    /// dispatched between the UI click and this call).
    ///
    /// When a turn is active we call [`interrupt`](Self::interrupt), which
    /// ends the SDK query's current turn. The sidecar emits
    /// `turn-interrupted` (the session stays alive and rebuilds a resumed
    /// query on the next send-turn; the queue is preserved) rather than
    /// tearing the session down. The promoted item then dispatches via the
    /// notifications-task drain that fires on `turn-interrupted`, or via
    /// `interrupt`'s own inline fallback drain — whichever runs first; the
    /// `dispatching` guard in [`drain_queue`](Self::drain_queue) makes that
    /// race safe. When the session is already idle (the turn finished
    /// during the click→call race) we just `drain_queue()` directly.
    ///
    /// If the promotion succeeds but the interrupt RPC then fails, we undo
    /// the reorder (via [`restore_queued_position`]) before propagating the
    /// error, so a failed steer does not silently leave the queue
    /// reordered.
    ///
    /// **Pending approvals:** an interrupt now aborts any outstanding
    /// approval sidecar-side and emits `request-resolved` for it, which
    /// clears the stale `pending_approvals` entry (see
    /// [`mutate_state_from_notification`]). So the promoted message no
    /// longer waits on a zombie approval — the following drain dispatches
    /// it as soon as the turn ends.
    pub async fn send_queued_now(self: &Arc<Self>, queued_id: &str) -> Result<(), ProviderError> {
        let (has_active_turn, original_index, previous_release) = {
            let mut state = self.state.lock().await;
            if let Some(q) = state.queued_turns.iter().find(|q|q.queued_id == queued_id) { q.input.authorize_queue_promotion()?; }
            match promote_queued_to_front(&mut state.queued_turns, queued_id) {
                // `turn_busy`, not `active_turn`: an SDK-initiated turn
                // parked on a question has no `active_turn`, and draining
                // would just leave the item stuck behind the callback.
                // Interrupting aborts the callback sidecar-side.
                Some(idx) => {
                    let previous = state.queue_hold.release_with(queued_id);
                    if idx > 0 {
                        let _ = self.event_tx.send(crate::agent_provider::queue_edit::reordered(&state.queued_turns, &self.thread_id));
                    }
                    (state.turn_busy(), idx, previous)
                }
                // Unknown / already-dispatched / cancelled — no-op.
                None => return Ok(()),
            }
        };
        if has_active_turn {
            // Soft-stop the running turn; the sidecar's `turn-interrupted`
            // notification (or `interrupt`'s inline fallback drain)
            // dispatches the item we just promoted to the front.
            match self.interrupt(None).await {
                Ok(()) => Ok(()),
                Err(err) => {
                    // Undo the reorder so a failed steer doesn't leave the
                    // queue silently rearranged.
                    {
                        let mut state = self.state.lock().await;
                        restore_queued_position(&mut state.queued_turns, queued_id, original_index);
                        state.queue_hold.revert_release(queued_id, previous_release);
                        if original_index > 0 {
                            let _ = self.event_tx.send(crate::agent_provider::queue_edit::reordered(&state.queued_turns, &self.thread_id));
                        }
                    }
                    Err(err)
                }
            }
        } else {
            // Race: the turn finished between the UI click and this call.
            // The session is idle, so just drain the promoted item out.
            self.drain_queue().await;
            Ok(())
        }
    }

    /// Keep queued turns queued after a user Stop; see
    /// [`ClaudeSessionState::queue_hold`].
    pub async fn hold_queue(&self) {
        self.state.lock().await.queue_hold.hold();
    }

    /// Cancel every queued turn, emitting a
    /// [`ProviderRuntimeEvent::QueuedTurnCancelled`] for each. Used when
    /// the session closes or errors so the UI cleans up its greyed items.
    async fn cancel_all_queued(&self) {
        let drained: Vec<String> = {
            let mut state = self.state.lock().await;
            state.queued_turns.drain(..).map(|q| q.queued_id).collect()
        };
        for queued_id in drained {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread_id.clone(),
                    queued_id,
                });
        }
    }

    /// Queue a user turn on the sidecar. Sidecar synthesizes the SDK
    /// turn id — this method returns our adapter-local [`TurnId`]. Does
    /// NOT perform the busy check (callers gate that); assumes the
    /// session is idle.
    async fn do_send(
        &self,
        text: String,
        images: Vec<ImageInput>,
        model_override: Option<String>,
        dispatch_guard: Option<Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>>,
        checkpoint: Option<Arc<dyn crate::agent_provider::types::TurnDispatchCheckpoint>>,
    ) -> Result<TurnId, ProviderError> {
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
        if let Some(checkpoint) = &checkpoint { checkpoint.authorize_dispatch()?; }
        let generation = self.native_authority.begin();
        let turn_id = TurnId(format!("claude-turn-{}", uuid::Uuid::new_v4()));
        {
            let mut state = self.state.lock().await;
            if !self.native_authority.bind(generation, &turn_id.0) {
                // Completion/Stop already settled this generation. The RPC
                // receipt is still successful, but cannot recreate busy state.
                return Ok(turn_id);
            }
            state.active_turn = Some(turn_id.clone());
            state.status = SessionStatus::Running {
                active_turn: turn_id.clone(),
            };
            // Announce under the lifecycle lock before a concurrent result can
            // settle this turn and publish completion.
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: self.thread_id.clone(),
                    status: SessionStatus::Running {
                        active_turn: turn_id.clone(),
                    },
                });
        }
        // Announce the state flip so subscribers (composer, reducer)
        // learn that a turn is in flight. Claude's SDK never emits a
        // text delta for a pure-tool turn, so without this event the
        // frontend's streaming flag would stay false for the entire
        // turn and let the user queue a second one.
        let params = serde_json::to_value(&params).unwrap();
        let ack = match dispatch_guard.as_deref() {
            Some(guard) => self.sidecar.request_guarded(METHOD_SEND_TURN, params, guard).await,
            None => self.sidecar.request(METHOD_SEND_TURN, params).await,
        };
        let outcome = ack.map_err(|error| ProviderError::RpcError {
            message: format!("send-turn RPC failed: {error}"),
        }).and_then(|ack| {
            if let Some(guard) = dispatch_guard.as_ref() {
                if ack.get("turnStarted").and_then(Value::as_bool) != Some(true) {
                    return Err(ProviderError::RpcError {
                        message: "Malformed guarded send-turn acknowledgement; native outcome unknown".into(),
                    });
                }
                guard.accepted().map_err(|message| ProviderError::RpcError { message })?;
            }
            Ok(())
        });
        if let Err(error) = outcome {
            self.native_authority.finish(&turn_id.0);
            let mut state = self.state.lock().await;
            if state.active_turn.as_ref() == Some(&turn_id) {
                state.active_turn = None;
                if matches!(state.status, SessionStatus::Running { .. }) {
                    state.status = SessionStatus::Ready;
                    let _ = self.event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
                        thread_id: self.thread_id.clone(), status: SessionStatus::Ready,
                    });
                }
            }
            return Err(error);
        }
        Ok(turn_id)
    }

    /// This session as a host-tool caller, with its current permission mode.
    async fn host_caller(&self) -> crate::mcp::registry::HostCaller {
        crate::mcp::registry::HostCaller {
            control: None,
            thread_id: self.thread_id.0.clone(),
            provider: crate::agent_provider::ProviderKind::Claude,
            workspace_id: self.workspace_id.clone(),
            permission_mode: self.state.lock().await.permission_mode.clone(),
        }
    }

    /// Whether the child-exit watchdog has declared this session's sidecar
    /// dead (unintentional exit). A dead session is treated as absent by the
    /// provider so the next send auto-resumes instead of routing to a corpse.
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }

    pub async fn interrupt(self: &Arc<Self>, turn_id: Option<TurnId>) -> Result<(), ProviderError> {
        // Blanket Stop revokes authority synchronously, including idle waits.
        if turn_id.is_none() {
            self.native_authority.invalidate();
        }
        let active = {
            let mut state = self.state.lock().await;
            if let Some(requested) = turn_id.as_ref() {
                let active = state.active_turn.as_ref().ok_or_else(|| {
                    ProviderError::ValidationError {
                        message: "no active turn to interrupt".into(),
                    }
                })?;
                if requested != active {
                    return Err(ProviderError::ValidationError {
                        message: format!(
                            "turn id mismatch: requested {}, active {}",
                            requested.0, active.0
                        ),
                    });
                }
                // Keep selection and revocation atomic with turn transitions.
                self.native_authority.invalidate();
            }
            // A newer Stop wins over guidance sent into this turn.
            let active = state.active_turn.clone();
            state.steer.stop(active.clone());
            active
        };
        let params = InterruptParams {
            thread_id: self.thread_id.0.clone(),
        };
        if let Err(e) = self
            .sidecar
            .request(METHOD_INTERRUPT, serde_json::to_value(&params).unwrap())
            .await
        {
            self.recover_failed_stop(&mut *self.state.lock().await, &active);
            self.announce_steer_continuation().await;
            return Err(ProviderError::RpcError {
                message: format!("interrupt RPC failed: {e}"),
            });
        }
        // By the time the RPC returns, the sidecar's `turn-interrupted`
        // notification may ALREADY have been processed by the
        // notifications task, which may already have flipped the session
        // to Ready and drained the queue — dispatching the next turn and
        // setting `active_turn` to a NEW turn. Only surface Ready + clear
        // state when the turn we interrupted is still the active one (the
        // notification hasn't landed yet); otherwise we would clobber the
        // freshly-dispatched turn and could double-dispatch.
        //
        // A Stop with NO active turn is the user cancelling a background
        // wait: the `result` already cleared `active_turn` without emitting
        // Ready (see the SdkMessage handler), and the frontend holds the
        // turn open until something settles it. Announce Ready then too —
        // but only while the session is still idle, so a turn dispatched
        // meanwhile is never clobbered.
        {
            let mut state = self.state.lock().await;
            // The stopped turn's undelivered guidance died with it, unless a
            // newer turn has already started and owns the pending set.
            if state.active_turn.is_none() || state.active_turn == active {
                state.steer.stopped();
            }
            let still_idle = active.is_none()
                && state.active_turn.is_none()
                && matches!(state.status, SessionStatus::Ready);
            if (state.active_turn == active && active.is_some()) || still_idle {
                state.active_turn = None;
                state.status = SessionStatus::Ready;
                // Announce Ready while holding the lock so subscribers
                // never observe `Ready` in the struct with no
                // corresponding event in flight.
                let _ = self
                    .event_tx
                    .send(ProviderRuntimeEvent::SessionStateChanged {
                        thread_id: self.thread_id.clone(),
                        status: SessionStatus::Ready,
                    });
            }
        }
        // Fallback drain: if the notification hasn't run yet, this
        // dispatches the next (queued) turn now that the session is idle —
        // the promoted item of a "send now"; a user Stop holds the queue.
        // The `dispatching` guard makes a concurrent drain (from the
        // notifications task) safe.
        self.drain_queue().await;
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
        if self.managed.is_some() {
            return Err(ProviderError::ValidationError {
                message: "managed workflow tool permissions cannot be relaxed".into(),
            });
        }
        if self.state.lock().await.permission_mode.as_deref() != Some(&mode) { self.native_authority.invalidate(); }
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
        self: &Arc<Self>,
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
            Ok(_) => {
                // The approval cleared, so the session may now be idle
                // with a queued follow-up waiting. The notification path
                // usually covers this, but draining here closes the
                // ordering gap between the local removal above and the
                // sidecar's `request-resolved` notification arriving.
                self.drain_queue().await;
                Ok(())
            }
            Err(crate::json_rpc_child::RpcChildError::RpcError(err))
                if is_request_not_pending_rpc(&err) =>
            {
                Err(ProviderError::RequestNotPending { request_id })
            }
            // A dead sidecar cannot receive this answer: report the request
            // as orphaned instead of a retryable failure. A reply still
            // outstanding when the child exits fails as a plain RPC error,
            // after the child is already marked not alive.
            Err(
                crate::json_rpc_child::RpcChildError::ChildExited { .. }
                | crate::json_rpc_child::RpcChildError::AlreadyShutdown,
            ) => Err(ProviderError::SessionClosed {
                thread_id: self.thread_id.clone(),
            }),
            Err(crate::json_rpc_child::RpcChildError::IoError(e))
                if e.kind() == std::io::ErrorKind::BrokenPipe =>
            {
                Err(ProviderError::SessionClosed {
                    thread_id: self.thread_id.clone(),
                })
            }
            Err(crate::json_rpc_child::RpcChildError::RpcError(_)) if !self.sidecar.is_alive() => {
                Err(ProviderError::SessionClosed {
                    thread_id: self.thread_id.clone(),
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
    pub async fn shutdown(&self) -> Result<(), ProviderError> {
        self.native_authority.revoke();
        {
            let mut flag = self.intentionally_closed.write().await;
            *flag = true;
        }

        // Cancel any queued follow-ups so the UI clears its greyed items
        // — they will never dispatch now.
        self.cancel_all_queued().await;

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
            let _ = t.await;
        }

        // Close the sidecar's JSON-RPC child (EOF → 2s grace → kill).
        let stopped = self.sidecar.shutdown().await;

        // Flip local status.
        {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Closed;
            state.active_turn = None;
            state.idle_compaction = None;
            // Pending approvals never get a response — drop them so
            // a later `respond_to_request` sees an empty map.
            state.pending_approvals.clear();
        }
        stopped.map_err(|e| ProviderError::ProcessError {
            message: "Native owned child teardown was not acknowledged".into(),
            source: Some(e.to_string()),
        })
    }

    pub async fn shutdown_managed(&self) -> Result<(), ProviderError> {
        if self.managed.is_none() {
            return Err(ProviderError::ValidationError {
                message: "session is not a managed attempt".into(),
            });
        }
        // Quiesce first; never drop a failed cleanup handle from the map.
        self.sidecar
            .shutdown_managed()
            .await
            .map_err(|error| ProviderError::ProcessError {
                message: "managed Claude stop is unconfirmed".into(),
                source: Some(error.to_string()),
            })?;
        if let Some(context) = self.managed.as_ref() {
            context
                .handler
                .quiesce()
                .await
                .map_err(|error| ProviderError::ProcessError {
                    message: "managed Claude host-tool stop unconfirmed".into(),
                    source: Some(error),
                })?;
        }
        self.shutdown().await

    }
}

/// The sidecar now reports a missing callback as InvalidParams (`-32602`),
/// but older bundled sidecars threw a plain `Error`, which the dispatcher
/// surfaced as InternalError (`-32603`). Match both only when the message is
/// the known missing-request diagnostic so unrelated invalid params/internal
/// failures retain their real classification.
fn is_request_not_pending_rpc(err: &crate::json_rpc_child::RpcError) -> bool {
    if err.code != -32602 && err.code != -32603 {
        return false;
    }
    let message = err.message.to_ascii_lowercase();
    message.contains("not found or already resolved")
        || message.contains("unknown pending")
        || message.contains("no pending approval")
}

fn optional_positive_budget(extra: &Value) -> Result<Option<f64>, String> {
    match extra.get("maxBudgetUsd").filter(|value| !value.is_null()) {
        None => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite() && *value > 0.0)
            .map(Some)
            .ok_or_else(|| "maxBudgetUsd must be a finite positive number".into()),
    }
}

fn optional_positive_turns(extra: &Value) -> Result<Option<u32>, String> {
    match extra.get("maxTurns").filter(|value| !value.is_null()) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| "maxTurns must be a positive 32-bit integer".into()),
    }
}

/// SDK messages carry session IDs rather than turn IDs; preserve the host's
/// active turn before result handling clears it.
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

fn managed_catalog_diagnostic(params: &Value) -> Option<Value> {
    let message = params.get("message")?;
    if message.get("type").and_then(Value::as_str) != Some("system")
        || message.get("subtype").and_then(Value::as_str) != Some("init")
    {
        return None;
    }
    let tools = message
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(Value::as_str)
                .take(256)
                .map(|name| name.chars().take(512).collect::<String>())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let servers = message.get("mcp_servers").and_then(Value::as_array).map(|servers| {
        servers.iter().take(32).map(|server|json!({
            "name":server.get("name").and_then(Value::as_str).map(|name|name.chars().take(512).collect::<String>()),
            "status":server.get("status").and_then(Value::as_str),
        })).collect::<Vec<_>>()
    }).unwrap_or_default();
    Some(json!({"tools":tools,"mcp_servers":servers}))
}

fn spawn_notifications_task(
    session: Arc<ClaudeSession>,
    sidecar: Arc<JsonRpcChild>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> JoinHandle<()> {
    // Subscribe synchronously before the caller can start a native turn.
    let mut notifications = sidecar.notifications();
    tokio::spawn(async move {
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
                            // The sidecar connection owns exactly this outer
                            // thread. SDK subagent payloads retain that outer id;
                            // a foreign envelope is not parent lifecycle evidence.
                            if note.params.get("threadId").and_then(Value::as_str).is_some_and(|t|t!=session.thread_id.0) {
                                let _=event_tx.send(ProviderRuntimeEvent::RuntimeWarning {thread_id:Some(session.thread_id.clone()),message:"Ignored foreign sidecar notification".into(),original_payload:None});
                                continue;
                            }
                            if session.managed.is_some() && session.managed_diagnostics {
                                if let Some(catalog) = managed_catalog_diagnostic(&note.params) {
                                    let _=event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                                        thread_id:Some(session.thread_id.clone()),
                                        message:"Managed native tool catalog".into(),
                                        original_payload:Some(catalog),
                                    });
                                }
                            }
                            // Update local state for specific notifications
                            // before broadcasting.
                            let structured = SidecarNotification::from_method_params(
                                &note.method,
                                note.params.clone(),
                            );
                            // Result/interruption handling clears the active
                            // turn. Preserve its identity before that mutation.
                            // An idle compaction owns only notifications that
                            // arrived while it held the session: read the
                            // owner before this one touches session state.
                            let (active_turn, idle_owner) = {
                                let state = session.state.lock().await;
                                (state.active_turn.clone(), state.idle_compaction.map(|run| run.id))
                            };
                            let idle_signal = IdleSignal::of(&structured);
                            mutate_state_from_notification(&session, &structured).await;
                            let opened_request = match &structured {
                                SidecarNotification::RequestOpened { request_id, .. } => {
                                    Some(RequestId(request_id.clone()))
                                }
                                _ => None,
                            };
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
                                    message: if session.managed.is_some() {"Managed event stream lagged: sidecar notification translation failed"} else {"panic while translating sidecar notification"}.into(),
                                    original_payload: Some(note.params),
                                }]
                            });
                            let events = session.settle_idle_events(idle_owner, idle_signal, events).await;
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
                            for mut e in events {
                                stamp_turn_id(&mut e, active_turn.as_ref());
                                let _ = event_tx.send(e);
                            }
                            // The crash watchdog may have drained (and
                            // expired) pending requests before this one was
                            // published. It marks the session dead before
                            // taking the state lock, so checking under that
                            // lock catches every request it missed.
                            if let Some(request_id) = opened_request {
                                let mut state = session.state.lock().await;
                                if session.dead.load(Ordering::Relaxed) {
                                    state.pending_approvals.remove(&request_id);
                                    let _ = event_tx.send(crate::agent_provider::expired_request_event(
                                        session.thread_id.clone(),
                                        request_id,
                                    ));
                                }
                            }
                            session.announce_steer_continuation().await;
                            // Follow-up queue: a notification may have
                            // just returned the session to idle (SDK
                            // `result` → Ready) or torn it down
                            // (session-ended / session-error). Drain the
                            // next queued turn when idle; cancel the whole
                            // queue when the session is gone. This runs
                            // after the state lock from
                            // `mutate_state_from_notification` is released,
                            // so `do_send` can re-acquire it without
                            // deadlocking.
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
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                                thread_id: Some(session.thread_id.clone()),
                                message: if session.managed.is_some() {format!("Managed event stream lagged: sidecar notification channel dropped {n}")} else {format!("sidecar notification channel lagged, dropped {n}")},
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
                        let actor = {
                            let state = session.state.lock().await;
                            crate::delegation::app::NativeActor {
                                thread_id: session.thread_id.0.clone(),
                                provider: crate::agent_provider::ProviderKind::Claude,
                                workspace_id: session.workspace_id.clone(),
                                session_id: session.provider_session_id.0.clone(),
                                permission_mode: state.permission_mode.clone(),
                                permit: session.native_authority.capture(),
                            }
                        };
                        let job = tokio::spawn(async move {
                            // The SDK can call tools before send-turn acknowledges.
                            // Wait only for this captured generation to bind.
                            if let Err(error) = actor.permit.wait_ready().await {
                                return Ok(tool_result_text(error, true));
                            }
                            handle_mcp_tool_call(session_for_task, event_tx_for_task, req.params, actor).await
                        });
                        session.native_authority.track(job.abort_handle());
                        // Stop cancels tool work, never the obligation to answer
                        // the sidecar's RPC, including immediate track rejection.
                        let sidecar = session.sidecar.clone();
                        tokio::spawn(async move {
                            let response = job.await.unwrap_or_else(|error| {
                                Ok(tool_result_text(format!("MCP tool call cancelled: {error}"), true))
                            });
                            let _ = sidecar.respond(req.id, response).await;
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
    let caller = session.host_caller().await;
    let tools = collect_shared_tools_for_claude(registry, &caller).await;
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
        .request(super::protocol::METHOD_UPDATE_MCP_TOOLS, params_value)
        .await
    {
        let _ = session.event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(session.thread_id.clone()),
            message: format!("update-mcp-tools failed: {err}"),
            original_payload: None,
        });
    }
}

/// Codemux's instructions for the tools this chat can call, appended to
/// Claude Code's system prompt. Claude lists MCP tools by name only until it
/// searches for one, so a tool's description alone does not tell the agent
/// when to reach for it.
fn system_prompt_append(tools: &[super::protocol::McpToolEntry]) -> Option<String> {
    crate::mcp_server::tool_guidance(tools.iter().map(|tool| tool.prefixed_name.as_str()))
}

/// Snapshot every running MCP tool from the registry into the wire
/// shape the sidecar expects. Returns an empty `Vec` when no registry
/// is configured or no tools are running — both fine, the sidecar
/// just doesn't register the in-process MCP server.
async fn collect_mcp_tools(
    registry: Option<&crate::mcp::registry::McpRegistry>,
    caller: &crate::mcp::registry::HostCaller,
) -> Vec<super::protocol::McpToolEntry> {
    let Some(registry) = registry else {
        return Vec::new();
    };
    let tools = collect_shared_tools_for_claude(registry, caller).await;
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

/// The single choke point for the tools a Claude session sees, at start and
/// on every live refresh: the shared registry surface plus the host tools
/// Codemux serves to this particular chat.
async fn collect_shared_tools_for_claude(
    registry: &crate::mcp::registry::McpRegistry,
    caller: &crate::mcp::registry::HostCaller,
) -> Vec<crate::mcp::runtime::McpTool> {
    let mut tools = registry
        .list_tools_for_workspace(
            &[
                crate::mcp::McpConfigSource::ClaudeUser,
                crate::mcp::McpConfigSource::ClaudeLocal,
                crate::mcp::McpConfigSource::ClaudeProject,
            ],
            caller.workspace_id.as_deref(),
        )
        .await;
    tools.retain(crate::mcp::registry::native_tool_visible);
    let host_tools=registry.host_tools(caller);
    // App-owned task tools also serve local IDs; replace the SSH-only schema
    // rather than publishing duplicate names to the native provider.
    tools.retain(|tool|!host_tools.iter().any(|host|host.prefixed_name==tool.prefixed_name));
    tools.extend(host_tools);
    // These stay callable while native catalog replacement waits for a safe
    // turn boundary (for example, an Editor connecting during this turn).
    if registry
        .has_visible_plugin_servers(caller.workspace_id.as_deref())
        .await
    {
        tools.extend(crate::mcp::registry::plugin_bridge_tools());
    }
    tools
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
    params: Value,
    actor: crate::delegation::app::NativeActor,
) -> Result<Value, crate::json_rpc_child::RpcError> {
    // Validate params shape.
    let prefixed_name = match params.get("name").and_then(Value::as_str) {
        Some(n) => n.to_string(),
        None => {
            return Err(crate::json_rpc_child::RpcError {
                code: -32602,
                message: "mcp-tool-call requires a string `name`".into(),
                data: None,
            })
        }
    };
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    if let Some(context) = session.managed.as_ref() {
        let payload = match context.call(&prefixed_name, arguments).await {
            Ok(value) => tool_result_text(value.to_string(), false),
            Err(error) => tool_result_text(error, true),
        };
        return Ok(payload);
    }

    let caller = session.host_caller().await;
    let (payload, failure) = run_mcp_tool_call(
        session.mcp_registry.as_ref(),
        &caller,
        session.workspace_id.as_deref(),
        &prefixed_name,
        arguments,
        Some(actor),
        &plugin_direct_tool_names(&params),
    )
    .await;
    if let Some(message) = failure {
        let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(session.thread_id.clone()),
            message: format!("mcp-tool-call {prefixed_name} failed: {message}"),
            original_payload: None,
        });
    }
    Ok(payload)
}

/// Sidecar-supplied snapshot metadata, never model arguments. It only labels
/// discovery results; visibility and dispatch still use the registry scope.
fn plugin_direct_tool_names(params: &Value) -> Vec<String> {
    params
        .get("directPluginTools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(512)
        .filter_map(Value::as_str)
        .filter(|name| name.len() <= 64)
        .map(str::to_owned)
        .collect()
}

/// Run one `mcp-tool-call` and build the tool result for the SDK, plus the
/// dispatch failure worth a runtime warning, if any.
async fn run_mcp_tool_call(
    registry: Option<&crate::mcp::registry::McpRegistry>,
    caller: &crate::mcp::registry::HostCaller,
    workspace_id: Option<&str>,
    prefixed_name: &str,
    arguments: Value,
    actor: Option<crate::delegation::app::NativeActor>,
    direct_tools: &[String],
) -> (Value, Option<String>) {
    // No registry: a session on a device (which must not reach this
    // computer's tools whatever name it sends), or a provider built without
    // one (tests, headless contexts). Answer with a tool error rather than
    // dispatching.
    let Some(registry) = registry else {
        return (
            tool_result_text("Codemux's tools aren't available to this thread", true),
            None,
        );
    };

    if let Some(result) = registry
        .call_plugin_bridge(prefixed_name, arguments.clone(), workspace_id, direct_tools)
        .await
    {
        return match result {
            Ok(payload) => (payload, None),
            Err(error) => (tool_result_text(error.clone(), true), Some(error)),
        };
    }

    // Host tools precede child dispatch. The caller carries the LIVE permission mode, so a
    // Plan/Ask pill flipped after the session started is honoured.
    if let Some(result) = registry.host_call_native(caller, prefixed_name, &arguments, None, None, actor.as_ref().map(|actor| &actor.permit)).await {
        return (result, None);
    }

    let dispatched = match actor {
        Some(actor) => registry.dispatch_native_tool_call(prefixed_name, arguments, actor).await,
        None => registry.dispatch_tool_call(prefixed_name, arguments, workspace_id).await,
    };
    match dispatched {
        // The MCP child returns the full `tools/call` result — including
        // `content` and optional `isError`. Forward it verbatim. If the
        // shape is wrong (no `content` key) wrap it so the SDK still sees a
        // valid shape.
        Ok(result) if result.get("content").is_some() => (result, None),
        Ok(result) => (tool_result_text(result.to_string(), false), None),
        Err(message) => (tool_result_text(message.clone(), true), Some(message)),
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
                            // Mark the session dead so the provider's
                            // `has_session` reports it absent and the next send
                            // rebuilds via `ensure_live_session` with the resume
                            // cursor rather than routing to the dead child.
                            session.dead.store(true, Ordering::Relaxed);
                            let msg = "claude-agent sidecar exited unexpectedly".to_string();
                            // Recover the in-flight turn id (populated during
                            // both Running and WaitingApproval) BEFORE clearing
                            // it, so the watchdog can settle it with a synthetic
                            // terminal `TurnCompleted`.
                            let (active_turn, orphaned) = {
                                let mut state = session.state.lock().await;
                                let active_turn = state.active_turn.take();
                                state.status = SessionStatus::Error {
                                    message: msg.clone(),
                                };
                                // Background subagents can hold approvals
                                // after the parent turn ended; expire them all.
                                let orphaned: Vec<RequestId> =
                                    state.pending_approvals.drain().map(|(id, _)| id).collect();
                                (active_turn, orphaned)
                            };
                            for request_id in orphaned {
                                let _ = event_tx.send(crate::agent_provider::expired_request_event(
                                    session.thread_id.clone(),
                                    request_id,
                                ));
                            }
                            // TurnCompleted(child_exited) first, then the Error
                            // state — see `child_exit_events` for why the order
                            // matters.
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
        SidecarNotification::ResumeFallback { .. } => {
            // The sidecar couldn't resume the persisted SDK session and
            // rebuilt a fresh query, transparently replaying the same
            // turn. Drop the stale cursor so we never try that dead id
            // again — the rebuilt query emits a new `sdk-session-id`
            // shortly, which repopulates this. Deliberately DON'T touch
            // `active_turn` / `status`: the replayed turn is still in
            // flight and stays `Running` until its real `result` lands.
            let mut state = session.state.lock().await;
            state.sdk_session_id = None;
            state.steer.drop_pending();
        }
        SidecarNotification::SdkMessage { message, .. } => {
            if message.get("user_message_uuids").is_some() {
                session.state.lock().await.steer.observe_echo();
            }
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
            if message.get("type").and_then(|v| v.as_str()) == Some("result")
                && super::translate::parent_tool_use_id(message).is_none()
            {
                let mut state = session.state.lock().await;
                // SDK completion can precede the send RPC acknowledgement.
                // Invalidate the generation even before active_turn is bound;
                // the delayed acknowledgement must not revive its authority.
                session.native_authority.finish_current();
                let ended = state.active_turn.take();
                if matches!(state.status, SessionStatus::Running { .. }) {
                    state.status = SessionStatus::Ready;
                }
                session.continue_unconsumed_steers(&mut state, message, ended);
            }
        }
        SidecarNotification::SessionEnded { reason, .. } => {
            let mut state = session.state.lock().await;
            if let Some(turn) = &state.active_turn {
                session.native_authority.finish(&turn.0);
            }
            if reason != "iteration-complete" {
                session.native_authority.revoke();
            }
            state.steer.drop_pending();
            state.active_turn = None;
            state.status = if reason == "iteration-complete" {
                SessionStatus::Ready
            } else {
                SessionStatus::Closed
            };
        }
        SidecarNotification::TurnInterrupted { .. } => {
            // An explicit interrupt RPC ended the turn but the session
            // survives. Clear the active turn and return to Ready — but
            // only from Running, so we never resurrect a session that has
            // since gone Closed / Error.
            let mut state = session.state.lock().await;
            state.steer.drop_pending();
            state.active_turn = None;
            if matches!(state.status, SessionStatus::Running { .. }) {
                state.status = SessionStatus::Ready;
            }
        }
        SidecarNotification::RequestResolved { request_id, .. } => {
            // The sidecar resolved a pending approval on its own — e.g.
            // an interrupt that aborted a turn with an outstanding tool
            // approval emits `request-resolved` with a deny decision.
            // Nothing else removes entries on the notification path (only
            // the local `respond_to_request` / `shutdown` do), so without
            // this the stale entry would block `drain_queue` forever.
            let mut state = session.state.lock().await;
            state
                .pending_approvals
                .remove(&RequestId(request_id.clone()));
        }
        SidecarNotification::SessionError { error, .. } => {
            session.native_authority.finish_current();
            let mut state = session.state.lock().await;
            state.steer.drop_pending();
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
    fn managed_claude_budget_and_turn_caps_are_optional_and_bounded() {
        assert_eq!(optional_positive_budget(&Value::Null).unwrap(), None);
        assert_eq!(optional_positive_turns(&json!({})).unwrap(), None);
        assert_eq!(
            optional_positive_budget(&json!({"maxBudgetUsd":0.2})).unwrap(),
            Some(0.2)
        );
        assert_eq!(
            optional_positive_turns(&json!({"maxTurns":6})).unwrap(),
            Some(6)
        );
        for value in [json!(0), json!(-1), json!("1")] {
            assert!(optional_positive_budget(&json!({"maxBudgetUsd":value})).is_err());
        }
        for value in [json!(0), json!(-1), json!(1.5), json!(u64::MAX)] {
            assert!(optional_positive_turns(&json!({"maxTurns":value})).is_err());
        }
    }

    #[test]
    fn managed_claude_catalog_diagnostic_excludes_authentication_and_prompts() {
        let catalog=managed_catalog_diagnostic(&json!({"message":{"type":"system","subtype":"init",
            "tools":["mcp__codemux__workflow_wait"],"mcp_servers":[{"name":"codemux","status":"connected","secret":"excluded"}],
            "apiKey":"excluded","prompt":"excluded"}})).unwrap();
        assert_eq!(
            catalog,
            json!({"tools":["mcp__codemux__workflow_wait"],"mcp_servers":[{"name":"codemux","status":"connected"}]})
        );
        assert!(managed_catalog_diagnostic(&json!({"message":{"type":"assistant"}})).is_none());
    }

    #[test]
    fn stop_response_indicates_already_closed_parses() {
        assert!(stop_response_indicates_already_closed(&json!({
            "alreadyClosed": true
        })));
        assert!(!stop_response_indicates_already_closed(&json!({})));
    }

    #[test]
    fn missing_request_rpc_classifier_accepts_old_and_new_sidecar_codes_only() {
        for code in [-32602, -32603] {
            assert!(is_request_not_pending_rpc(
                &crate::json_rpc_child::RpcError {
                    code,
                    message: "request req-1 not found or already resolved".into(),
                    data: None,
                }
            ));
        }
        assert!(!is_request_not_pending_rpc(
            &crate::json_rpc_child::RpcError {
                code: -32603,
                message: "database exploded".into(),
                data: None,
            }
        ));
        assert!(!is_request_not_pending_rpc(
            &crate::json_rpc_child::RpcError {
                code: -32000,
                message: "request req-1 not found or already resolved".into(),
                data: None,
            }
        ));
    }

    /// Build a bare `QueuedTurn` with a given id for reorder tests. The
    /// `input` payload is irrelevant to `promote_queued_to_front`, which
    /// only inspects `queued_id`.
    fn queued(id: &str) -> QueuedTurn {
        QueuedTurn {
            queued_id: id.to_string(),
            input: SendTurnInput {
                thread_id: ThreadId("t".into()),
                text: String::new(),
                images: vec![],
                model_override: None,
                effort_override: None,
                permission_mode_override: None,
                client_nonce: None,
                display_text: None,
                skill_invocations: vec![],
                turn_checkpoint: None,
                dispatch_guard: None,
            },
        }
    }

    #[test]
    fn promote_queued_to_front_unknown_id_is_noop() {
        let mut q: VecDeque<QueuedTurn> = [queued("a"), queued("b")].into();
        assert_eq!(promote_queued_to_front(&mut q, "missing"), None);
        // Order unchanged.
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

    fn test_caller() -> crate::mcp::registry::HostCaller {
        crate::mcp::registry::HostCaller {
            control: None,
            thread_id: "thread".into(),
            provider: crate::agent_provider::ProviderKind::Claude,
            workspace_id: Some("ws-42".into()),
            permission_mode: None,
        }
    }

    #[tokio::test]
    async fn delegation_delivery_round2_claude_late_host_delegate_cannot_cross_stop() {
        struct Host;
        impl crate::mcp::registry::HostTools for Host {
            fn tools_for(&self, _: &crate::mcp::registry::HostCaller) -> Vec<crate::mcp::runtime::McpTool> {
                vec![crate::mcp::runtime::McpTool { name:"delegate_task".into(), prefixed_name:"mcp__codemux__delegate_task".into(), description:None, input_schema:json!({}), server_id:"host".into() }]
            }
            fn call(&self, _: &crate::mcp::registry::HostCaller, _: &str, _: &Value) -> Option<Value> { Some(tool_result_text("must not be admitted", false)) }
        }
        for stop_before_capture in [false, true] {
            let registry = crate::mcp::registry::McpRegistry::new(); registry.set_host_tools(Arc::new(Host));
            let mut caller = test_caller(); caller.thread_id = format!("claude-late-host-{stop_before_capture}");
            let owner = crate::delegation::authority::NativeAuthority::new(&caller.thread_id);
            let generation = owner.begin(); assert!(owner.bind(generation, "turn"));
            let db = crate::database::DatabaseStore::new_in_memory();
            if stop_before_capture { owner.invalidate(); db.stop_message_deliveries(&caller.thread_id).unwrap(); }
            let actor = crate::delegation::app::NativeActor { thread_id:caller.thread_id.clone(), provider:caller.provider, workspace_id:caller.workspace_id.clone(), session_id:"session".into(), permission_mode:caller.permission_mode.clone(), permit:owner.capture() };
            if !stop_before_capture { owner.invalidate(); db.stop_message_deliveries(&caller.thread_id).unwrap(); }
            let (response, _) = run_mcp_tool_call(Some(&registry), &caller, caller.workspace_id.as_deref(), "mcp__codemux__delegate_task", json!({}), Some(actor), &[]).await;
            assert_eq!(response["isError"], true, "a late host call must not acquire the post-Stop generation: {response}");
            assert!(response["content"][0]["text"].as_str().unwrap().contains("revoked"));
        }
    }

    #[tokio::test]
    async fn plugin_mcp_claude_payload_has_guidance_and_hides_private_servers() {
        for scope in ["all", "plugin"] {
            let registry = crate::mcp::registry::test_support::plugin_fixture(scope).await;
            let tools = collect_mcp_tools(Some(&registry), &test_caller()).await;
            if scope == "plugin" {
                assert!(!tools
                    .iter()
                    .any(|tool| tool.server_id.starts_with("plugin:")));
            } else {
                let tool = tools.iter().find(|t| t.name == "new_tool").unwrap();
                assert_eq!(
                    tool.description.as_deref(),
                    Some("Use structured tools first.\n\nAlways use background delivery.")
                );
            }
            registry.shutdown_all().await;
        }
    }

    #[tokio::test]
    async fn plugin_mcp_claude_has_no_bridge_without_a_visible_plugin_server() {
        let registry = crate::mcp::registry::McpRegistry::new();
        let tools = collect_mcp_tools(Some(&registry), &test_caller()).await;
        assert!(!tools
            .iter()
            .any(|tool| tool.server_id == "codemux-plugin-bridge"));
    }

    #[tokio::test]
    async fn plugin_mcp_claude_preserves_structured_results_for_native_and_bridge() {
        let registry = crate::mcp::registry::test_support::plugin_fixture("all").await;
        let expected = registry
            .plugin_call("plugin:test-plugin:fixture", "image", json!({}))
            .await
            .unwrap();
        for (name, arguments) in [
            ("fixture__image", json!({})),
            (
                "codemux_call_plugin_tool",
                json!({"name":"fixture__image","arguments":{}}),
            ),
        ] {
            let (result, failure) = run_mcp_tool_call(
                Some(&registry),
                &test_caller(),
                Some("ws-42"),
                name,
                arguments,
                None,
                &[],
            )
            .await;
            assert!(failure.is_none());
            assert_eq!(result, expected);
            assert_eq!(result.to_string().matches("STRUCTURED-581").count(), 1);
        }
        registry.shutdown_all().await;
    }

    #[tokio::test]
    async fn plugin_mcp_claude_bridge_lists_and_calls_live_tools_with_scope() {
        for scope in ["all", "plugin"] {
            let registry = crate::mcp::registry::test_support::plugin_fixture(scope).await;
            let tools = collect_mcp_tools(Some(&registry), &test_caller()).await;
            assert_eq!(
                tools
                    .iter()
                    .any(|tool| tool.prefixed_name == "codemux_list_plugin_tools"),
                scope == "all"
            );
            let (list, failure) = run_mcp_tool_call(
                Some(&registry),
                &test_caller(),
                Some("ws-42"),
                "codemux_list_plugin_tools",
                json!({}),
                None,
                &["fixture__new_tool".into()],
            )
            .await;
            assert!(failure.is_none());
            if scope == "all" {
                assert!(list["structuredContent"]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["name"] == "fixture__new_tool"
                        && tool["directlyCallable"] == true));
            } else {
                assert_eq!(list["structuredContent"]["tools"], json!([]));
            }
            let (result, _) = run_mcp_tool_call(
                Some(&registry),
                &test_caller(),
                Some("ws-42"),
                "codemux_call_plugin_tool",
                json!({"name":"fixture__new_tool","arguments":{}}),
                None,
                &[],
            )
            .await;
            if scope == "all" {
                assert_eq!(result["content"][0]["text"], "ORCHID-742");
            } else {
                assert_eq!(result["isError"], true);
            }
            registry.shutdown_all().await;
        }
        let names = plugin_direct_tool_names(&json!({"directPluginTools":vec!["x"; 700]}));
        assert_eq!(names.len(), 512);
    }

    #[tokio::test]
    async fn local_session_publishes_and_runs_shared_tools() {
        let (mut server, registry, call) =
            crate::mcp::registry::test_support::codemux_remote_server("ws-42").await;
        let local = crate::agent_provider::local_tool_registry(Some(&registry), None);
        assert_eq!(
            collect_mcp_tools(local.as_ref(), &test_caller())
                .await
                .len(),
            1
        );
        // An empty visible registration must add both bridge tools before an
        // external app connects, without changing the existing native tool.
        server
            .mock("POST", "/empty")
            .match_body(mockito::Matcher::PartialJson(json!({"method":"initialize"})))
            .with_body(json!({
                "jsonrpc":"2.0", "id":1,
                "result":{"protocolVersion":"2025-06-18","capabilities":{}}
            }).to_string())
            .create_async()
            .await;
        server
            .mock("POST", "/empty")
            .match_body(mockito::Matcher::PartialJson(
                json!({"method":"notifications/initialized"}),
            ))
            .with_status(202)
            .create_async()
            .await;
        server
            .mock("POST", "/empty")
            .match_body(mockito::Matcher::PartialJson(
                json!({"method":"tools/list"}),
            ))
            .with_body(json!({"jsonrpc":"2.0","id":2,"result":{"tools":[]}}).to_string())
            .create_async()
            .await;
        server
            .mock("GET", "/empty")
            .with_status(405)
            .create_async()
            .await;
        registry
            .register_plugin_server::<tauri::Wry>(
                None, "empty", "token".into(),
                serde_json::from_value(json!({
                    "id":"empty", "name":"Empty", "toolPrefix":"empty",
                    "scope":"all", "transport":"http", "url":format!("{}/empty",server.url())
                })).unwrap(),
            )
            .await
            .unwrap();
        let tools = collect_mcp_tools(local.as_ref(), &test_caller()).await;
        assert_eq!(tools.len(), 3);
        assert!(tools
            .iter()
            .any(|tool| tool.prefixed_name == "codemux_list_plugin_tools"));
        assert!(tools
            .iter()
            .any(|tool| tool.prefixed_name == "codemux_call_plugin_tool"));
        let (payload, failure) = run_mcp_tool_call(
            local.as_ref(),
            &test_caller(),
            Some("ws-42"),
            "mcp__codemux-remote__echo",
            json!({"text": "hi"}),
            None,
            &[],
        )
        .await;
        assert_eq!(payload["content"][0]["text"], "routed");
        assert!(failure.is_none());
        call.assert_async().await;
        registry.shutdown_all().await;
    }

    #[tokio::test]
    async fn device_session_gets_and_runs_none_of_this_computers_tools() {
        let (_server, registry, call) =
            crate::mcp::registry::test_support::codemux_remote_server("ws-42").await;
        let device = crate::agent_provider::types::RemoteSpawnTarget {
            host_id: 3,
            ssh_target: "deus@ai-node".into(),
            host_name: "ai-node".into(),
        };
        let on_device = crate::agent_provider::local_tool_registry(Some(&registry), Some(&device));
        assert!(collect_mcp_tools(on_device.as_ref(), &test_caller())
            .await
            .is_empty());

        // A call the device sends anyway, by a name it guessed, is answered
        // with a tool error and never reaches the registry.
        let (payload, _) = run_mcp_tool_call(
            on_device.as_ref(),
            &test_caller(),
            Some("ws-42"),
            "mcp__codemux-remote__echo",
            json!({"text": "hi"}),
            None,
            &[],
        )
        .await;
        assert_eq!(payload["isError"], true);
        assert!(!call.matched_async().await);
    }

    #[test]
    fn system_prompt_guides_visuals_only_when_html_render_is_listed() {
        let tool = |prefixed_name: &str| super::super::protocol::McpToolEntry {
            name: "html_render".into(),
            prefixed_name: prefixed_name.into(),
            description: None,
            input_schema: json!({}),
            server_id: "codemux-self".into(),
        };
        assert_eq!(
            system_prompt_append(&[tool("mcp__codemux__browser_click"), tool("mcp__codemux__html_render")]).as_deref(),
            Some(crate::mcp_server::HTML_RENDER_GUIDANCE)
        );
        // No Codemux tools, or a same-named tool on another server.
        assert!(system_prompt_append(&[]).is_none());
        assert!(system_prompt_append(&[tool("mcp__other__html_render")]).is_none());
    }

    #[test]
    fn system_prompt_guides_suggestions_only_when_suggest_tasks_is_listed() {
        let tool = |prefixed_name: &str| super::super::protocol::McpToolEntry {
            name: "suggest_tasks".into(),
            prefixed_name: prefixed_name.into(),
            description: None,
            input_schema: json!({}),
            server_id: "codemux-host".into(),
        };
        let suggest = crate::commands::follow_ups::SUGGEST_TASKS_GUIDANCE;
        assert_eq!(
            system_prompt_append(&[tool("mcp__codemux__suggest_tasks")]).as_deref(),
            Some(suggest)
        );
        let both = system_prompt_append(&[
            tool("mcp__codemux__suggest_tasks"),
            tool("mcp__codemux__html_render"),
        ])
        .unwrap();
        assert_eq!(
            both,
            format!("{}\n\n{suggest}", crate::mcp_server::HTML_RENDER_GUIDANCE),
            "every listed tool's block, in a fixed order"
        );
        assert!(system_prompt_append(&[tool("mcp__other__suggest_tasks")]).is_none());
    }
}
