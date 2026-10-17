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
use tokio::sync::{broadcast, Mutex, RwLock};
#[path="managed_runtime.rs"]
mod managed_runtime;
pub(crate) use managed_runtime::read_model_catalog;
use tokio::task::JoinHandle;

use crate::agent_provider::{
    ApprovalDecision, PlanAuthMode, ProviderError, ProviderRuntimeEvent, ProviderSessionId,
    RequestId, SendOutcome, SendTurnInput, SessionStatus, ThreadId, TurnId, UsageBaseline,
};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};
use crate::mcp::registry::McpRegistry;

use super::protocol::{
    AccountReadResponse, ApprovalResponse, Capabilities, ClientInfo, CollaborationMode,
    CollaborationModeSettings, DynamicToolCallParams, DynamicToolSpec,
    GetAccountRateLimitsResponse, InitializeParams, NotificationMessage, ServerRequestMessage,
    ThreadResumeParams, ThreadRollbackParams, ThreadStartParams, ThreadStartResponse,
    TurnInputItem, TurnInterruptParams, TurnStartParams, TurnStartResponse,
    RECOVERABLE_THREAD_RESUME_ERROR_SNIPPETS,
};
use super::translate::{translate_notification_with, translate_server_request, CodexSubagentDemux};

fn resolve_catalog_service_tier(
    models: &[super::protocol::ModelEntry],
    model: Option<&str>,
    tier: &str,
) -> String {
    let entry = model
        .and_then(|id| {
            models
                .iter()
                .find(|entry| entry.id == id || entry.model == id)
        })
        .or_else(|| {
            if model.is_none() {
                models.iter().find(|entry| entry.is_default)
            } else {
                None
            }
        });
    let Some(entry) = entry else {
        return "default".into();
    };
    let tiers = super::capabilities::service_tiers_for_model(entry);
    if tiers.iter().any(|option| option.value == tier) {
        return tier.into();
    }
    if tier == "fast" && tiers.iter().any(|option| option.value == "priority") {
        return "priority".into();
    }
    "default".into()
}

/// Default per-request timeout, mirroring the upstream reference's
/// `sendRequest(..., 20_000)` default.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(20);

const CODEX_DEFAULT_MODE_INSTRUCTIONS: &str = r#"<collaboration_mode># Collaboration Mode: Default

You are now in Default mode. Any previous instructions for other modes (e.g. Plan mode) are no longer active.

Your active mode changes only when new developer instructions with a different `<collaboration_mode>...</collaboration_mode>` change it; user requests or tool descriptions do not change mode by themselves. Known mode names are Default and Plan.

## request_user_input availability

Use the `request_user_input` tool only when it is listed in the available tools for this turn.

When `request_user_input_async` is available, use it for clarification questions and continue useful work that does not depend on the answer. Incorporate answers as they arrive. Do not proceed with work that requires an unanswered question or approval. If that tool is unavailable, use the question behavior below.

In Default mode, strongly prefer making reasonable assumptions and executing the user's request rather than stopping to ask questions. If you absolutely must ask a question because the answer cannot be discovered from local context and a reasonable assumption would be risky, ask the user directly with a concise plain-text question. Never write a multiple choice question as a textual assistant message.
</collaboration_mode>"#;

fn runtime_collaboration_mode(
    model: Option<&str>,
    effort: Option<&str>,
    guidance: Option<&str>,
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
    let guidance = guidance
        .map(|text| format!("{text}\n\n"))
        .unwrap_or_default();
    let developer_instructions = format!(
        "{CODEX_DEFAULT_MODE_INSTRUCTIONS}\n\n{guidance}<runtime_info>In case you're asked: you are running in CodeMux through the Codex harness, as {model}{effort_text}. No need to mention this otherwise.</runtime_info>"
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
#[derive(Clone)]
pub struct CodexSpawnConfig {
    /// Path to the `codex` binary.
    pub codex_binary: PathBuf,
    pub chatgpt_owner: Option<Arc<super::chatgpt::Owner>>,
    /// Value to set for `CODEX_HOME`, if any.
    pub codex_home: Option<PathBuf>,
    /// Client identification block to hand Codex at `initialize` time.
    pub client_info: ClientInfo,
    /// Shared MCP registry used to publish and execute dynamic tools.
    pub mcp_registry: Option<McpRegistry>,
    /// Device to run `codex app-server` on over SSH; `None` runs it here.
    pub remote: Option<crate::agent_provider::types::RemoteSpawnTarget>,
}

/// A Codex server request awaiting the user's answer.
pub(crate) struct PendingApproval {
    /// Raw JSON-RPC id to reply to.
    rpc_id: Value,
    /// Codex thread and turn that raised it. Codex abandons a thread's
    /// requests when its turn ends, so they settle with that turn.
    codex_thread_id: Option<String>,
    turn_id: Option<String>,
}

/// Runtime-mutable state inside a [`CodexSession`].
pub(crate) struct CodexSessionState {
    /// Codex's own thread identifier (different from the runtime's
    /// [`ThreadId`], though semantically matched).
    pub codex_thread_id: String,
    /// The turn currently in flight, if any. `Option::None` means
    /// the session is idle and ready for a new turn.
    pub active_turn: Option<TurnId>,
    // Notifications can finish a turn before its start response is consumed.
    pending_turn_start: Option<PendingTurnStart>,
    /// Map from synthesised [`RequestId`] to the server request awaiting
    /// the user's answer.
    pub pending_approvals: HashMap<RequestId, PendingApproval>,
    /// Recently ended `(codex thread, turn)` pairs. Requests and
    /// notifications arrive on separate tasks, so a request can be
    /// recorded after its turn already settled; this catches it.
    settled_turns: VecDeque<(String, String)>,
    /// Lifecycle status.
    pub status: SessionStatus,
    /// Current model override (session-wide default).
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    /// Per-session reasoning-effort default. Applied to every
    /// `turn/start` whose caller does not provide an `effort_override`
    /// — mirrors the reference's `collaborationMode.settings.reasoning_effort`
    /// session default.
    pub default_effort: Option<String>,
    /// Service-speed tier applied to subsequent `turn/start` requests.
    /// Codex accepts this per turn, so changing Fast mode does not require
    /// rebuilding or resuming the provider thread.
    pub service_tier: String,
    pub tier_models: Vec<super::protocol::ModelEntry>,
    /// Follow-up turns queued while a turn was in flight, in FIFO
    /// dispatch order. Drained one-at-a-time as the session returns to
    /// idle (see [`CodexSession::drain_queue`]).
    pub queued_turns: VecDeque<QueuedTurn>,
    /// Holds the queue after a user Stop until an explicit send.
    pub(crate) queue_hold: crate::agent_provider::queue_hold::QueueHold,
}

#[derive(Default)]
struct PendingTurnStart {
    completed_turns: VecDeque<TurnId>,
    failed: bool,
}

/// Steering that started a new turn is a message sent by hand, which releases
/// a queue held by Stop. Guidance inside a turn, or a failure, restores the
/// hold taken before delivery (a Stop during delivery holds it regardless).
async fn restore_hold_unless_started(
    state: &Mutex<CodexSessionState>,
    was_held: bool,
    result: &Result<crate::agent_provider::TurnStartResult, ProviderError>,
) {
    if was_held {
        let started = matches!(result, Ok(started) if !started.steered);
        state.lock().await.queue_hold.settle_take(was_held, started);
    }
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
    child: RwLock<Arc<JsonRpcChild>>,
    chatgpt_runtime: Option<managed_runtime::ManagedSession>,
    runtime_lease: Mutex<Option<Arc<std::fs::File>>>,
    // Retain lifetime usage and subagent registrations across child renewal.
    notification_demux: Mutex<CodexSubagentDemux>,
    /// Mutable session state.
    pub state: Mutex<CodexSessionState>,
    outbound: Mutex<()>,
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
    /// Owning workspace of the chat pane this session runs in, from
    /// `StartSessionInput.workspace_id`. Attached per-call to dynamic
    /// tool dispatches so workspace-scoped built-in tools bind to THIS
    /// session's workspace — the registry's shared MCP child cannot
    /// learn the caller from its env.
    workspace_id: Option<String>,
    native_authority: Arc<crate::delegation::authority::NativeAuthority>,
    managed: Option<Arc<crate::agent_provider::managed::ManagedSession>>,
    /// This session as a host-tool caller. Its permission mode is the one
    /// the session started with: Codex cannot change mode mid-session, and
    /// the host tools it lists are fixed at `thread/start`.
    host_caller: crate::mcp::registry::HostCaller,
    /// Codemux instructions for the tools listed at `thread/start`, added to
    /// every turn's developer instructions.
    guidance: Option<String>,
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
        service_tier: String,
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
        let caller_env = caller_env.unwrap_or_default();
        let mut env = caller_env.clone();
        let mut spawn = spawn;
        // A device owns its own account; local credentials never enter SSH.
        if spawn.remote.is_some() { spawn.chatgpt_owner = None; }
        if let (Some(home), None) = (spawn.codex_home.as_ref(), spawn.remote.as_ref()) {
            env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
        }
        let managed = crate::agent_provider::managed::lookup_session(&thread_id);
        if managed.is_some() && !cfg!(target_os = "linux") {
            return Err(ProviderError::ValidationError {
                message: "managed-start-rejected: managed Codex requires Linux process containment".into(),
            });
        }
        if managed.is_some() && resume_cursor.is_some() {
            return Err(ProviderError::ValidationError {
                message: "managed-start-rejected: managed attempts cannot resume an unscoped native session".into(),
            });
        }
        // Managed threads select no native environment, including writer
        // attempts. File access goes through the host's scoped virtual tools.
        // The read-only policy is an independent defense for native startup.
        let permission_mode = if managed.is_some() {
            Some("managed-read-only".to_owned())
        } else {
            permission_mode
        };
        let runtime_lease = spawn.chatgpt_owner.as_ref()
            .map(|owner| owner.acquire_runtime().map(Arc::new)).transpose()
            .map_err(|message| ProviderError::ValidationError { message })?;
        let grant = match spawn.chatgpt_owner.as_ref() {
            Some(owner) => owner.usable().await.map_err(|hint| ProviderError::NotAuthenticated {
                provider: crate::agent_provider::ProviderKind::Codex, hint,
            })?,
            None => None,
        };
        let child = if let Some(context) = managed.as_ref() {
            spawn_managed_codex(&spawn, &env, &cwd, context, grant.as_ref(), runtime_lease.clone()).await?
        } else {
            managed_runtime::spawn_child(&spawn, &cwd, caller_env.clone(), grant.as_ref(), runtime_lease.clone()).await?
        };
        let mut startup_guard = child.managed_startup_guard();
        // Keep the outer lease alive through exact-child teardown on failed init.
        let initialized = async {
        let mut model = model;
        // Pull the single incoming-request receiver before any background
        // tasks start; otherwise the adapter could race the watchdog.
        let incoming_rx = child
            .incoming_requests()
            .ok_or_else(|| ProviderError::ProcessError {
                message: "JsonRpcChild incoming-request channel already claimed".into(),
                source: None,
            })?;

        // --- initialize handshake -------------------------------------------
        if managed.is_none() {
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
                .map_err(|e| {
                    // `initialize` is the first thing a device-side child
                    // answers, so an install or connection problem there
                    // surfaces here.
                    #[cfg(unix)]
                    if let Some(error) = spawn.remote.as_ref().and_then(|remote| {
                        crate::ssh::sidecar::device_start_error(
                            &e,
                            crate::agent_provider::ProviderKind::Codex,
                            remote,
                            format!(
                                "Install the Codex CLI on {} and run `codex login` there",
                                remote.host_name
                            ),
                        )
                    }) {
                        return error;
                    }
                    ProviderError::RpcError {
                        message: format!("initialize failed: {e}"),
                    }
                })?;
            child
                .notify("initialized", json!({}))
                .await
                .map_err(|e| ProviderError::RpcError {
                    message: format!("initialized notification failed: {e}"),
                })?;
        }

        let host_caller = crate::mcp::registry::HostCaller {
            thread_id: thread_id.0.clone(),
            provider: crate::agent_provider::ProviderKind::Codex,
            workspace_id: workspace_id.clone(),
            permission_mode: permission_mode.clone(),
        };
        // A session on a device publishes none of this computer's tools and
        // keeps no registry to run the device's tool calls through.
        let mcp_registry = crate::agent_provider::local_tool_registry(
            spawn.mcp_registry.as_ref(),
            spawn.remote.as_ref(),
        );
        // Capture the live registry tool surface once for thread/start.
        // Codex persists dynamic tool definitions in the rollout, while the
        // request handler below always dispatches through the live registry.
        let dynamic_tools = if let Some(context) = managed.as_ref() {
            Some(
                context
                    .handler
                    .tools()
                    .into_iter()
                    .map(|tool| DynamicToolSpec::Function {
                        name: tool.name,
                        description: tool.description,
                        input_schema: tool.input_schema,
                        defer_loading: None,
                    })
                    .collect(),
            )
        } else {
            // Host tools are decided here too, from the mode the session starts
            // in, so a Codex chat only ever lists what it could actually use.
            dynamic_tool_specs(
                mcp_registry.as_ref(),
                spawn.codex_home.is_some(),
                &host_caller,
            )
            .await
        };

        let tier_models = if grant.is_some() {
            let entries = read_model_catalog(&child).await
                .map_err(|message| ProviderError::RpcError { message })?;
            model = Some(managed_runtime::selected_model(&entries, model.as_deref())?);
            entries
        } else {
            match child.request("model/list", json!({})).await {
            Ok(value) => match serde_json::from_value::<super::protocol::ModelListResponse>(value) {
                Ok(response) => response.data,
                Err(error) => {
                    let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                        thread_id: Some(thread_id.clone()),
                        message: format!("model/list decode failed: {error}"),
                        original_payload: None,
                    });
                    vec![]
                }
            },
            Err(e) => {
                let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread_id.clone()),
                    message: format!("model/list probe failed: {e}"),
                    original_payload: None,
                });
                vec![]
            }
        }
        };
        let managed_models = if grant.is_some() {
            tier_models.iter().filter(|entry| !entry.hidden).map(|entry| entry.id.clone()).collect()
        } else { Vec::new() };
        let resolved_tier =
            resolve_catalog_service_tier(&tier_models, model.as_deref(), &service_tier);
        if resolved_tier == "default" && service_tier != "default" {
            let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(thread_id.clone()),
                message: "service-tier-fallback: Saved service tier could not be verified for this model/account; using Standard.".into(),
                original_payload: None,
            });
        }
        let service_tier = resolved_tier;
        if grant.is_none() {
        // account/read is the canonical "can this provider run?" check.
        // `requires_openai_auth` describes the active provider and stays true
        // for a logged-in ChatGPT/API-key account, so only gate when the
        // account is also absent. RPC failure (binary error, not auth) is
        // logged but tolerated; the session proceeds and turns will fail
        // with the underlying error.
        match child.request("account/read", json!({})).await {
            Ok(resp) => match serde_json::from_value::<AccountReadResponse>(resp) {
                Ok(info) if info.needs_login() => {
                    if managed.is_some() {
                        child.shutdown_managed().await.map_err(|error| {
                            ProviderError::ProcessError {
                                message: "managed Codex authentication rejection stop unconfirmed"
                                    .into(),
                                source: Some(error.to_string()),
                            }
                        })?;
                        return Err(ProviderError::ValidationError{message:"managed-start-rejected: Codex requires authentication; run codex login before launching a workflow".into()});
                    }
                    let _ = child.shutdown().await;
                    return Err(ProviderError::NotAuthenticated {
                        provider: crate::agent_provider::ProviderKind::Codex,
                        hint: match spawn.remote.as_ref() {
                            Some(remote) => {
                                format!("Run `codex login` on {} and try again.", remote.host_name)
                            }
                            None => "Run `codex login` and try again.".into(),
                        },
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

        }else{
            let _=event_tx.send(ProviderRuntimeEvent::PlanUsageUpdated{thread_id:thread_id.clone(),provider:crate::agent_provider::ProviderKind::Codex,windows:Vec::new(),plan_label:Some("Using ChatGPT plan".into()),auth_mode:Some(PlanAuthMode::Subscription)});
        }
        // --- thread/start or thread/resume ----------------------------------
        let require_original = resume_cursor
            .as_ref()
            .and_then(|v| v.get("requireOriginal"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let thread = match &resume_cursor {
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
                            service_tier: Some(
                                (service_tier != "default").then(|| service_tier.clone()),
                            ),
                            cwd: Some(cwd.clone()),
                            collaboration_mode: None,
                            approval_policy,
                            sandbox,
                            experimental_raw_events: false,
                        };
                        let params_value = serde_json::to_value(&params).unwrap();
                        match child.request("thread/resume", params_value).await {
                            Ok(resp) => match serde_json::from_value::<ThreadStartResponse>(resp) {
                                Ok(r) => r,
                                Err(e) => {
                                    return Err(ProviderError::RpcError {
                                        message: format!("malformed thread/resume response: {e}"),
                                    })
                                }
                            },
                            Err(err)
                                if !require_original
                                    && is_recoverable_resume_error(&err.to_string()) =>
                            {
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
                                start_fresh_thread(
                                    &child,
                                    cwd.clone(),
                                    model.clone(),
                                    permission_mode.clone(),
                                    service_tier.clone(),
                                    dynamic_tools.clone(),
                                )
                                .await?
                            }
                            Err(e) => {
                                return Err(ProviderError::RpcError {
                                    message: format!("thread/resume failed: {e}"),
                                })
                            }
                        }
                    }
                    None => {
                        start_fresh_thread(
                            &child,
                            cwd.clone(),
                            model.clone(),
                            permission_mode.clone(),
                            service_tier.clone(),
                            dynamic_tools.clone(),
                        )
                        .await?
                    }
                }
            }
            None => {
                start_fresh_thread(
                    &child,
                    cwd.clone(),
                    model.clone(),
                    permission_mode.clone(),
                    service_tier.clone(),
                    dynamic_tools.clone(),
                )
                .await?
            }
        };
        let codex_thread_id = thread.thread_id().to_string();
        // A chat that names no model runs on Codex's configured default. Keep
        // the model Codex reports, since each turn's runtime identity and tool
        // guidance ride on collaboration-mode settings, which require one.
        let model = model.or_else(|| thread.model().map(str::to_owned));

        // --- assemble session handle ----------------------------------------
        let mut demux=CodexSubagentDemux::new(codex_thread_id.clone());demux.set_recorded_usage_baseline(recorded_usage_baseline.unwrap_or_default());
        let (shutdown_tx, _shutdown_rx) = broadcast::channel(4);
        if let Some(guard) = startup_guard.as_mut() { guard.set_task_shutdown(shutdown_tx.clone()); }
        let state = Mutex::new(CodexSessionState {
            codex_thread_id: codex_thread_id.clone(),
            active_turn: None,
            pending_turn_start: None,
            pending_approvals: HashMap::new(),
            settled_turns: VecDeque::new(),
            status: SessionStatus::Ready,
            model,
            default_effort: effort,
            permission_mode: permission_mode.clone(),
            service_tier,
            tier_models,
            queued_turns: VecDeque::new(),
            queue_hold: Default::default(),
        });
        let session = Arc::new(Self {
            thread_id: thread_id.clone(),
            provider_session_id: ProviderSessionId(codex_thread_id.clone()),
            cwd,
            child: RwLock::new(Arc::clone(&child)),
            chatgpt_runtime: grant.as_ref().map(|g|managed_runtime::ManagedSession{owner:spawn.chatgpt_owner.as_ref().unwrap().clone(),spawn:spawn.clone(),env:env.iter().filter(|(k,_)|matches!(k.as_str(),"CODEMUX_WORKSPACE_ID"|"CODEMUX_PANE_ID"|"CODEMUX_HOOK_PORT"|"CODEMUX_CONTROL_SOCKET"|"CODEX_HOME")).map(|(k,v)|(k.clone(),v.clone())).collect(),revision:Mutex::new(g.revision.clone()),permission:permission_mode.clone(),models:Mutex::new(managed_models)}),
            runtime_lease: Mutex::new(runtime_lease.clone()),
            notification_demux: Mutex::new(demux),
            state,
            outbound: Mutex::new(()),
            event_tx: event_tx.clone(),
            shutdown_tx: shutdown_tx.clone(),
            tasks: Mutex::new(Vec::new()),
            dead: Arc::new(AtomicBool::new(false)),
            workspace_id,
            native_authority: crate::delegation::authority::NativeAuthority::new(&thread_id.0),
            managed: managed.clone(),
            host_caller,
            guidance: dynamic_tools.as_deref().and_then(developer_guidance),
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
            mcp_registry,
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

        if let Some(guard) = startup_guard.as_mut() { guard.disarm(); }
        Ok(session)
        }.await;
        if initialized.is_err(){if managed.is_some(){let _=child.shutdown_managed().await;}else if runtime_lease.is_some(){let _=child.shutdown_owned().await;}else{let _=child.shutdown().await;}}
        initialized
    }
    pub(crate) async fn child(&self)->Arc<JsonRpcChild>{self.child.read().await.clone()}

    pub(crate) async fn managed_startup_guard(&self) -> Option<crate::json_rpc_child::ManagedStartupGuard> {
        let mut guard = self.child().await.managed_startup_guard()?;
        guard.set_task_shutdown(self.shutdown_tx.clone());
        Some(guard)
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
        let outbound = self.outbound.lock().await;
        self.ensure_open().await?;
        let was_held = {
            let mut state = self.state.lock().await;
            if state.active_turn.is_some() {
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
                if state.queue_hold.goes_first() {
                    state.queued_turns.push_front(queued);
                    state.queue_hold.release_with(&queued_id);
                } else {
                    state.queued_turns.push_back(queued);
                }
                drop(state);
                let _ = self.event_tx().send(ProviderRuntimeEvent::TurnQueued {
                    thread_id: self.thread_id.clone(),
                    queued_id: queued_id.clone(),
                    client_nonce,
                    text,
                });
                return Ok(SendOutcome::Queued(queued_id));
            }
            // A message sent by hand goes first; the held queue follows it.
            state.queue_hold.take()
        };
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
                input.dispatch_guard,
                checkpoint.clone(),
            )
            .await;
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
        if was_held {
            self.state.lock().await.queue_hold.settle_take(was_held, outcome.is_ok());
        }
        drop(outbound);
        if was_held && outcome.is_ok() {
            // The turn may have settled before its start response while
            // notification drains were locked out; resume the queue now.
            self.drain_queue().await;
        }
        outcome
    }

    /// Serialize guidance with sends and queue drains. The provider owns the
    /// safe delivery boundary; no interrupt RPC is involved.
    pub async fn steer_turn(
        self: &Arc<Self>,
        input: SendTurnInput,
    ) -> Result<crate::agent_provider::TurnStartResult, ProviderError> {
        let result = {
            let _outbound = self.outbound.lock().await;
            let was_held = self.state.lock().await.queue_hold.take();
            let result = self.deliver_steer(input).await;
            restore_hold_unless_started(&self.state, was_held, &result).await;
            result
        };
        self.drain_queue().await;
        result
    }

    async fn deliver_steer(
        self: &Arc<Self>,
        input: SendTurnInput,
    ) -> Result<crate::agent_provider::TurnStartResult, ProviderError> {
        self.ensure_open().await?;
        let root = self.state.lock().await.codex_thread_id.clone();
        for attempt in 0..2 {
            // Native state, rather than a potentially delayed notification,
            // determines whether this is guidance or a new follow-up.
            let thread = self
                .read_question_thread(&root)
                .await
                .map_err(|message| ProviderError::RpcError { message })?;
            let active = thread
                .get("turns")
                .and_then(Value::as_array)
                .and_then(|turns| turns.iter().rev().find(|t| t["status"] == "inProgress"))
                .and_then(|turn| turn["id"].as_str());
            if let Some(active) = active {
                if super::slash_commands::request(&input.text, &root)
                    .map_err(|message| ProviderError::ValidationError { message })?
                    .is_some()
                {
                    return Err(ProviderError::ValidationError { message: "Native commands cannot steer a running turn. Choose Queue or Interrupt and send.".into() });
                }
                let params = super::protocol::TurnSteerParams {
                    thread_id: root.clone(),
                    expected_turn_id: active.to_owned(),
                    client_user_message_id: input
                        .client_nonce
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    input: turn_input_items(
                        input.text.clone(),
                        input.images.clone(),
                        input.skill_invocations.clone(),
                    ),
                };
                if let Some(checkpoint) = &input.turn_checkpoint { checkpoint.authorize_dispatch()?; }
                match self.child().await
                    .request("turn/steer", serde_json::to_value(params).unwrap())
                    .await
                {
                    Ok(value) => {
                        let turn_id = value.get("turnId").and_then(Value::as_str).ok_or_else(|| ProviderError::RpcError { message: "Codex accepted guidance without a turn ID; delivery is uncertain.".into() })?;
                        return Ok(crate::agent_provider::TurnStartResult {
                            turn_id: TurnId(turn_id.into()),
                            queued_id: None,
                            steered: true,
                        });
                    }
                    Err(crate::json_rpc_child::RpcChildError::RpcError(error))
                        if attempt == 0 && steer_precondition_rejected(&error) =>
                    {
                        continue
                    }
                    Err(error) => {
                        return Err(ProviderError::RpcError {
                            message: format!("Guidance could not be confirmed: {error}"),
                        })
                    }
                }
            }
            if !self.state.lock().await.pending_approvals.is_empty() {
                return Err(ProviderError::ValidationError {
                    message: "Resolve the pending approval before sending a follow-up.".into(),
                });
            }
            let native = super::slash_commands::request(&input.text, &root)
                .map_err(|message| ProviderError::ValidationError { message })?
                .is_some();
            let checkpoint = input.turn_checkpoint.as_ref();
            if let Some(checkpoint) = checkpoint {
                checkpoint.prepare().await;
            }
            let sent = if native {
                self.do_send(
                    input.text.clone(),
                    input.images.clone(),
                    input.skill_invocations.clone(),
                    input.model_override.clone(),
                    input.effort_override.clone(),
                    input.dispatch_guard.clone(),
                    input.turn_checkpoint.clone(),
                )
                .await
            } else {
                self.do_send_with_id(
                    input.text.clone(),
                    input.images.clone(),
                    input.skill_invocations.clone(),
                    input.model_override.clone(),
                    input.effort_override.clone(),
                    input.client_nonce.clone(),
                    input.dispatch_guard.clone(),
                    input.turn_checkpoint.clone(),
                )
                .await
                .map_err(plain_send_error)
            };
            return match sent {
                Ok(turn_id) => {
                    if let Some(checkpoint) = checkpoint {
                        checkpoint.commit().await;
                    }
                    Ok(crate::agent_provider::TurnStartResult {
                        turn_id,
                        queued_id: None,
                        steered: false,
                    })
                }
                Err(error) => {
                    if let Some(checkpoint) = checkpoint {
                        checkpoint.abort().await;
                    }
                    Err(error)
                }
            };
        }
        Err(ProviderError::ValidationError {
            message: "The active turn changed; your guidance was not sent. Try again.".into(),
        })
    }

    pub async fn steer_queued(self: &Arc<Self>, queued_id: &str) -> Result<(), ProviderError> {
        let outbound = self.outbound.lock().await;
        let result = async {
            let input = self
                .state
                .lock()
                .await
                .queued_turns
                .iter()
                .find(|q| q.queued_id == queued_id)
                .map(|q| q.input.clone());
            let Some(input) = input else { return Ok(()) };
            let text = input
                .display_text
                .clone()
                .unwrap_or_else(|| input.text.clone());
            let was_held = self.state.lock().await.queue_hold.take();
            let result = self.deliver_steer(input).await;
            restore_hold_unless_started(&self.state, was_held, &result).await;
            let result = result?;
            self.state
                .lock()
                .await
                .queued_turns
                .retain(|q| q.queued_id != queued_id);
            let _ = self
                .event_tx()
                .send(ProviderRuntimeEvent::QueuedTurnDispatched {
                    steered: result.steered,
                    thread_id: self.thread_id.clone(),
                    queued_id: queued_id.into(),
                    turn_id: result.turn_id,
                    text,
                });
            Ok(())
        }
        .await;
        drop(outbound);
        self.drain_queue().await;
        result
    }

    /// Pop the next queued turn (if idle) and dispatch it. No-op when the
    /// queue is empty or a turn is active. Emits
    /// [`ProviderRuntimeEvent::QueuedTurnDispatched`] on success; a failed
    /// dispatch cancels the item (never wedges the queue) and the next is
    /// attempted.
    pub async fn drain_queue(self: &Arc<Self>) {
        let Ok(_outbound) = self.outbound.try_lock() else {
            return;
        };
        loop {
            let next = {
                let mut state = self.state.lock().await;
                let idle = state.active_turn.is_none()
                    && matches!(state.status, SessionStatus::Ready)
                    && state.pending_approvals.is_empty();
                if !idle {
                    return;
                }
                let front = state.queued_turns.front().map(|q| q.queued_id.clone());
                if !state.queue_hold.admits(front.as_deref()) {
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
                    queued.input.dispatch_guard,
                    checkpoint.clone(),
                )
                .await
            {
                Ok(turn_id) => {
                    if let Some(checkpoint) = checkpoint.as_ref() {
                        checkpoint.commit().await;
                    }
                    self.state.lock().await.queue_hold.settle(&queued.queued_id, true);
                    let _ = self
                        .event_tx()
                        .send(ProviderRuntimeEvent::QueuedTurnDispatched {
                            steered: false,
                            thread_id: self.thread_id.clone(),
                            queued_id: queued.queued_id,
                            turn_id,
                            text,
                        });
                    // A terminal notification may have arrived before the RPC
                    // response. Recheck idle so the following item is not stranded.
                    continue;
                }
                Err(err) => {
                    if let Some(checkpoint) = checkpoint.as_ref() {
                        checkpoint.abort().await;
                    }
                    self.state.lock().await.queue_hold.settle(&queued.queued_id, false);
                    let _ = self.event_tx().send(ProviderRuntimeEvent::RuntimeWarning {
                        thread_id: Some(self.thread_id.clone()),
                        message: format!(
                            "queued turn {} failed to dispatch: {err}",
                            queued.queued_id
                        ),
                        original_payload: None,
                    });
                    let _ = self
                        .event_tx()
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
    /// [`ProviderRuntimeEvent::QueuedTurnCancelled`] when found. Returns
    /// whether the queued item was actually removed.
    pub async fn cancel_queued(self: &Arc<Self>, queued_id: &str) -> Result<bool, ProviderError> {
        let outbound = self.outbound.lock().await;
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
                .event_tx()
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread_id.clone(),
                    queued_id: queued_id.to_string(),
                });
        }
        drop(outbound);
        self.drain_queue().await;
        Ok(removed)
    }

    /// **Interrupt and send:** promote a queued follow-up to the front of
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
        let outbound = self.outbound.lock().await;
        let (has_active_turn, original_index, previous_release) = {
            let mut state = self.state.lock().await;
            match promote_queued_to_front(&mut state.queued_turns, queued_id) {
                Some(idx) => {
                    let previous = state.queue_hold.release_with(queued_id);
                    (state.active_turn.is_some(), idx, previous)
                }
                // Unknown / already-dispatched / cancelled — no-op.
                None => return Ok(()),
            }
        };
        let result = if has_active_turn {
            match self.interrupt_turn(None).await {
                Ok(()) | Err(ProviderError::ValidationError { .. }) => Ok(()),
                Err(err) => {
                    let mut state = self.state.lock().await;
                    restore_queued_position(&mut state.queued_turns, queued_id, original_index);
                    state.queue_hold.revert_release(queued_id, previous_release);
                    Err(err)
                }
            }
        } else {
            Ok(())
        };
        drop(outbound);
        self.drain_queue().await;
        result
    }

    /// Keep queued turns queued after a user Stop; see
    /// [`CodexSessionState::queue_hold`].
    pub async fn hold_queue(&self) {
        self.state.lock().await.queue_hold.hold();
    }

    /// Cancel every queued turn, emitting a cancellation for each. Used
    /// when the session closes or errors.
    async fn cancel_all_queued(&self) {
        let drained: Vec<String> = {
            let mut state = self.state.lock().await;
            state.queued_turns.drain(..).map(|q| q.queued_id).collect()
        };
        for queued_id in drained {
            let _ = self
                .event_tx()
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
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
        self: &Arc<Self>,
        text: String,
        images: Vec<crate::agent_provider::ImageInput>,
        skill_invocations: Vec<crate::skills::ResolvedSkillInvocation>,
        model_override: Option<String>,
        effort_override: Option<String>,
        dispatch_guard: Option<Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>>,
        checkpoint: Option<Arc<dyn crate::agent_provider::types::TurnDispatchCheckpoint>>,
    ) -> Result<TurnId, ProviderError> {
        self.ensure_managed_child().await?;
        let thread_id = self.state.lock().await.codex_thread_id.clone();
        let native = super::slash_commands::request(&text, &thread_id)
            .map_err(|message| ProviderError::ValidationError { message })?;
        if let Some((method, params)) = native {
            if dispatch_guard.is_some() {
                return Err(ProviderError::ValidationError {
                    message: "Guarded result must use the native turn/start path".into(),
                });
            }
            if !images.is_empty() || !skill_invocations.is_empty() {
                return Err(ProviderError::ValidationError {
                    message: "Codex commands cannot include images or skills. Send them in a separate message.".into(),
                });
            }
            // Reserve the queue before the RPC. Notifications may arrive before
            // its response, and own the final lifecycle/real turn identifier.
            let pending = TurnId(format!("native-{}", uuid::Uuid::new_v4()));
            self.state.lock().await.active_turn = Some(pending.clone());
            if let Some(checkpoint) = &checkpoint {
                if let Err(error) = checkpoint.authorize_dispatch() {
                    self.state.lock().await.active_turn = None;
                    return Err(error);
                }
            }
            let response = match self.child().await.request(method, params).await {
                Ok(response) => response,
                Err(error) => {
                    let mut state = self.state.lock().await;
                    if state.active_turn.as_ref() == Some(&pending) {
                        state.active_turn = None;
                    }
                    return Err(ProviderError::RpcError {
                        message: format!("{method} failed: {error}"),
                    });
                }
            };
            // Compaction returns {} and reports its turn exclusively through
            // notifications. Review returns a regular turn envelope.
            if method == "review/start" {
                if let Some(id) = response
                    .pointer("/turn/id")
                    .and_then(serde_json::Value::as_str)
                {
                    let mut state = self.state.lock().await;
                    if state.active_turn.as_ref() == Some(&pending) {
                        state.active_turn = Some(TurnId(id.into()));
                    }
                    return Ok(TurnId(id.into()));
                }
                let mut state = self.state.lock().await;
                if state.active_turn.as_ref() == Some(&pending) {
                    state.active_turn = None;
                }
                return Err(ProviderError::RpcError {
                    message: "Codex returned a malformed review/start response.".into(),
                });
            }
            return Ok(self
                .state
                .lock()
                .await
                .active_turn
                .clone()
                .unwrap_or(pending));
        }
        self.do_send_with_id(
            text,
            images,
            skill_invocations,
            model_override,
            effort_override,
            None,
            dispatch_guard,
            checkpoint,
        )
        .await
        .map_err(plain_send_error)
    }

    async fn do_send_with_id(
        self: &Arc<Self>,
        text: String,
        images: Vec<crate::agent_provider::ImageInput>,
        skill_invocations: Vec<crate::skills::ResolvedSkillInvocation>,
        model_override: Option<String>,
        effort_override: Option<String>,
        client_id: Option<String>,
        dispatch_guard: Option<Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>>,
        checkpoint: Option<Arc<dyn crate::agent_provider::types::TurnDispatchCheckpoint>>,
    ) -> Result<TurnId, TurnStartError> {
        self.ensure_managed_child().await.map_err(TurnStartError::Admission)?;
        let (codex_thread_id, model_default, effort_default, service_tier) = {
            let state = self.state.lock().await;
            (
                state.codex_thread_id.clone(),
                state.model.clone(),
                state.default_effort.clone(),
                resolve_catalog_service_tier(
                    &state.tier_models,
                    model_override.as_deref().or(state.model.as_deref()),
                    &state.service_tier,
                ),
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
        let input_items = turn_input_items(text, images, skill_invocations);

        let model = model_override.or(model_default);
        let effort = effort_override.or(effort_default);
        let collaboration_mode =
            runtime_collaboration_mode(model.as_deref(), effort.as_deref(), self.guidance.as_deref());
        let params = TurnStartParams {
            thread_id: codex_thread_id,
            input: input_items,
            model,
            // `serviceTierForTurn` scopes the choice to this turn only, so the
            // thread keeps whatever tier the user configured. Send an explicit
            // `default` when Fast is off: omission would inherit the thread's
            // tier and silently keep a previous Fast turn's speed.
            service_tier_for_turn: Some(service_tier),
            effort,
            collaboration_mode,
        };
        let mut params_value = serde_json::to_value(&params).unwrap();
        if self.managed.is_some() {
            restrict_managed_codex_environment(&mut params_value);
        }
        if let Some(id) = client_id {
            params_value["clientUserMessageId"] = json!(id);
        }
        if let Some(checkpoint) = &checkpoint {
            checkpoint.authorize_dispatch().map_err(|error| TurnStartError::Rpc(
                crate::json_rpc_child::RpcChildError::RpcError(crate::json_rpc_child::RpcError {code:-32001,message:error.to_string(),data:None})
            ))?;
        }
        let generation = {
            let mut state = self.state.lock().await;
            let generation = self.native_authority.begin();
            state.pending_turn_start = Some(PendingTurnStart::default());
            generation
        };
        let child = self.child().await;
        let response = match dispatch_guard.as_deref() {
            Some(guard) => child.request_guarded("turn/start", params_value, guard).await,
            None => child.request("turn/start", params_value).await,
        };
        let parsed = response
            .map_err(TurnStartError::Rpc)
            .and_then(|value| serde_json::from_value::<TurnStartResponse>(value)
                .map_err(TurnStartError::Malformed))
            .and_then(|response| {
                if let Some(guard) = dispatch_guard.as_ref() {
                    guard.accepted().map_err(|error| TurnStartError::Rpc(
                        crate::json_rpc_child::RpcChildError::ProtocolError(error)
                    ))?;
                }
                Ok(response)
            });
        let mut state = self.state.lock().await;
        let pending = state.pending_turn_start.take().unwrap_or_default();
        let parsed = parsed.map_err(|error| {
            self.native_authority.finish_pending(generation);
            error
        })?;
        let turn_id = TurnId(parsed.turn.id);
        if pending.completed_turns.contains(&turn_id) || pending.failed {
            self.native_authority.finish_pending(generation);
        }
        if self.native_authority.bind(generation, &turn_id.0) {
            state.active_turn = Some(turn_id.clone());
            state.status = SessionStatus::Running {
                active_turn: turn_id.clone(),
            };
        }
        Ok(turn_id)
    }

    // Called with outbound held, so explicit shutdown cannot race admission.
    async fn ensure_open(&self) -> Result<(), ProviderError> {
        if matches!(self.state.lock().await.status, SessionStatus::Closed) {
            return Err(ProviderError::ValidationError {
                message: "This Codex session has been stopped. Start a new session to continue.".into(),
            });
        }
        Ok(())
    }

    /// Whether the child-exit watchdog has declared this session's
    /// `codex app-server` dead (unintentional exit). A dead session is
    /// treated as absent by the provider so the next send auto-resumes.
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }

    /// A managed replacement temporarily marks its retired child dead while
    /// holding outbound. Provider eviction must wait for that owner to settle
    /// and recheck death, rather than removing an in-progress renewal.
    pub(super) async fn lifecycle_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.outbound.lock().await
    }

    pub async fn answer_question(
        self: &Arc<Self>,
        input: crate::agent_provider::AnswerQuestionInput,
    ) -> Result<crate::agent_provider::QuestionDelivery, crate::agent_provider::QuestionDeliveryError>
    {
        let result = {
            let _outbound = self.outbound.lock().await;
            self.deliver_question_answer(input).await
        };
        self.drain_queue().await;
        result
    }

    async fn deliver_question_answer(
        self: &Arc<Self>,
        input: crate::agent_provider::AnswerQuestionInput,
    ) -> Result<crate::agent_provider::QuestionDelivery, crate::agent_provider::QuestionDeliveryError>
    {
        use crate::agent_provider::{QuestionDelivery as Delivery, QuestionDeliveryError as Error};
        self.ensure_open().await.map_err(|error| Error::Rejected(error.to_string()))?;
        let text = input
            .question
            .answer_text(&input.answers)
            .map_err(Error::Rejected)?;
        let root = self.state.lock().await.codex_thread_id.clone();
        // Resuming a missing conversation can create a new native thread in the
        // legacy send path. Never deliver an old answer to that replacement.
        if input.question.subagent_id.is_none() && input.question.target != root {
            return Err(Error::Rejected(
                "The original conversation could not be resumed. Your answer has been kept.".into(),
            ));
        }
        for attempt in 0..2 {
            // Read native state once per submission, also supporting questions
            // from a child without substituting the parent's active turn ID.
            let thread = self
                .read_question_thread(&input.question.target)
                .await
                .map_err(Error::Rejected)?;
            if let Some(delivery) = find_answer_in_history(&thread, &input.submission_id) {
                return Ok(delivery);
            }
            let active = thread
                .get("turns")
                .and_then(Value::as_array)
                .and_then(|turns| turns.iter().rev().find(|t| t["status"] == "inProgress"))
                .and_then(|t| t["id"].as_str())
                .map(str::to_owned);
            if let Some(turn_id) = active {
                let params = super::protocol::TurnSteerParams {
                    thread_id: input.question.target.clone(),
                    expected_turn_id: turn_id.clone(),
                    client_user_message_id: input.submission_id.clone(),
                    input: vec![TurnInputItem::Text {
                        text: text.clone(),
                        text_elements: vec![],
                    }],
                };
                match self.child().await
                    .request("turn/steer", serde_json::to_value(params).unwrap())
                    .await
                {
                    Ok(value) => {
                        let accepted =
                            value.get("turnId").and_then(Value::as_str).ok_or_else(|| {
                                Error::Unknown(
                                    "Codex accepted the answer but returned no turn ID.".into(),
                                )
                            })?;
                        return Ok(Delivery::Inflight {
                            turn_id: TurnId(accepted.into()),
                        });
                    }
                    Err(crate::json_rpc_child::RpcChildError::RpcError(error))
                        if attempt == 0 && steer_precondition_rejected(&error) =>
                    {
                        continue
                    }
                    Err(error) => return Err(question_rpc_error(error)),
                }
            } else {
                if input.question.target != root {
                    return Err(Error::Rejected("This subagent is no longer running. Continue in the main conversation to discuss its question.".into()));
                }
                if !self.state.lock().await.pending_approvals.is_empty() {
                    return Err(Error::Rejected(
                        "Resolve the pending approval before starting a follow-up.".into(),
                    ));
                }
                if let Some(checkpoint) = &input.checkpoint {
                    checkpoint.prepare().await;
                }
                let result = self
                    .do_send_with_id(
                        text.clone(),
                        vec![],
                        vec![],
                        None,
                        None,
                        Some(input.submission_id.clone()),
                        None,
                        input.checkpoint.clone(),
                    )
                    .await;
                match result {
                    Ok(turn_id) => {
                        if let Some(checkpoint) = &input.checkpoint {
                            checkpoint.commit().await;
                        }
                        return Ok(Delivery::NewTurn { turn_id });
                    }
                    Err(error) => {
                        if let Some(checkpoint) = &input.checkpoint {
                            checkpoint.abort().await;
                        }
                        return Err(question_turn_start_error(error));
                    }
                }
            }
        }
        Err(Error::Rejected(
            "The active turn changed. Try submitting your answer again.".into(),
        ))
    }

    async fn read_question_thread(&self, target: &str) -> Result<Value, String> {
        let value = self.child().await
            .request(
                "thread/read",
                json!({"threadId": target, "includeTurns": true}),
            )
            .await
            .map_err(|e| e.to_string())?;
        let thread = value
            .get("thread")
            .ok_or("Codex returned no conversation.")?;
        if thread["id"].as_str() != Some(target) || !thread["turns"].is_array() {
            return Err("Codex did not return the requested conversation history.".into());
        }
        Ok(thread.clone())
    }

    pub async fn find_question_answer(
        &self,
        target: &str,
        submission_id: &str,
    ) -> Result<Option<crate::agent_provider::QuestionDelivery>, String> {
        let thread = self.read_question_thread(target).await?;
        Ok(find_answer_in_history(&thread, submission_id))
    }

    /// Interrupt the currently active turn. If `turn_id` is provided,
    /// only interrupt if it matches the live turn.
    pub async fn interrupt_turn(&self, turn_id: Option<TurnId>) -> Result<(), ProviderError> {
        // Blanket Stop revokes authority synchronously, even while state is busy.
        if turn_id.is_none() {
            self.native_authority.invalidate();
        }
        let (codex_thread_id, active_turn) = {
            let state = self.state.lock().await;
            let active_turn = state.active_turn.as_ref().ok_or_else(|| {
                ProviderError::ValidationError {
                    message: "no active turn to interrupt".into(),
                }
            })?;
            if let Some(requested) = turn_id.as_ref() {
                if requested != active_turn {
                    return Err(ProviderError::ValidationError {
                        message: format!(
                            "turn id mismatch: requested {}, active {}",
                            requested.0, active_turn.0
                        ),
                    });
                }
                // Selection and revocation share the lifecycle lock: a stale
                // selection must never revoke a subsequently active turn.
                self.native_authority.invalidate();
            }
            (state.codex_thread_id.clone(), active_turn.clone())
        };
        let params = TurnInterruptParams {
            thread_id: codex_thread_id.clone(),
            turn_id: active_turn.0.clone(),
        };
        let params_value = serde_json::to_value(&params).unwrap();
        self.child().await
            .request("turn/interrupt", params_value)
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("turn/interrupt failed: {e}"),
            })?;
        // Codex answers `turn/interrupt` once the turn is aborted, so its
        // approvals are already dead; settle them without waiting for the
        // completion notification.
        self.settle_turn_approvals(&codex_thread_id, &active_turn.0).await;
        Ok(())
    }

    /// Settle the approvals a finished Codex turn left unanswered.
    ///
    /// Codex aborts a thread's server requests when its turn completes or
    /// is interrupted, but never tells the client. Dropping them here
    /// unblocks follow-up sends and queue draining; the `Cancel`
    /// resolution closes the card and is persisted for hydration. Requests
    /// that carry no turn id are treated as belonging to whichever turn of
    /// their thread ends.
    pub(crate) async fn settle_turn_approvals(&self, codex_thread_id: &str, turn_id: &str) {
        let settled: Vec<RequestId> = {
            let mut state = self.state.lock().await;
            if state.settled_turns.len() == 64 {
                state.settled_turns.pop_front();
            }
            state
                .settled_turns
                .push_back((codex_thread_id.to_owned(), turn_id.to_owned()));
            let parent = state.codex_thread_id == codex_thread_id;
            let ids: Vec<RequestId> = state
                .pending_approvals
                .iter()
                .filter(|(_, pending)| {
                    pending
                        .codex_thread_id
                        .as_deref()
                        .map_or(parent, |thread| thread == codex_thread_id)
                        && pending.turn_id.as_deref().is_none_or(|turn| turn == turn_id)
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in &ids {
                state.pending_approvals.remove(id);
            }
            ids
        };
        for request_id in settled {
            let _ = self.event_tx.send(ProviderRuntimeEvent::RequestResolved {
                thread_id: self.thread_id.clone(),
                request_id,
                decision: ApprovalDecision::Cancel,
            });
        }
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
        self.child().await
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
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        let pending = {
            let mut state = self.state.lock().await;
            state.pending_approvals.remove(&request_id).ok_or_else(|| {
                ProviderError::RequestNotPending {
                    request_id: request_id.clone(),
                }
            })?
        };
        let payload = serde_json::to_value(ApprovalResponse::from(decision.clone())).unwrap();
        let child = self.child().await;
        // Hold the notification demux until the resolution is published, so
        // a turn completion Codex emits in reaction to this reply is always
        // broadcast after it (late resolutions re-arm mid-turn tracking).
        let _demux = self.notification_demux.lock().await;
        child
            .respond(pending.rpc_id, Ok(payload))
            .await
            .map_err(|e| match e {
                // A dead child cannot receive this answer: report the
                // request as orphaned instead of a retryable failure.
                crate::json_rpc_child::RpcChildError::ChildExited { .. }
                | crate::json_rpc_child::RpcChildError::AlreadyShutdown => {
                    ProviderError::SessionClosed {
                        thread_id: self.thread_id.clone(),
                    }
                }
                e => ProviderError::RpcError {
                    message: format!("respond failed: {e}"),
                },
            })?;
        // Codex sends no confirmation of its own; without this the card
        // stays "responding" and hydration later reports it expired.
        let _ = self.event_tx.send(ProviderRuntimeEvent::RequestResolved {
            thread_id: self.thread_id.clone(),
            request_id,
            decision,
        });
        Ok(())
    }

    /// Update the session-wide default model.
    pub async fn set_model(&self, model: String) {
        let mut state = self.state.lock().await;
        state.model = Some(model);
    }

    /// Update the service tier used by subsequent turns without replacing
    /// the Codex app-server or its loaded thread.
    pub async fn set_service_tier(&self, tier: String) -> Result<(), ProviderError> {
        // Refresh when choosing a premium tier: entitlement can change while
        // the conversation stays alive. Standard always clears a stale choice.
        let models = if tier != "default" {
            let value = self.child().await
                .request("model/list", json!({}))
                .await
                .map_err(|error| ProviderError::RpcError {
                    message: error.to_string(),
                })?;
            Some(
                serde_json::from_value::<super::protocol::ModelListResponse>(value)
                    .map_err(|error| ProviderError::RpcError {
                        message: error.to_string(),
                    })?
                    .data,
            )
        } else {
            None
        };
        let mut state = self.state.lock().await;
        if let Some(models) = models {
            state.tier_models = models;
        }
        let resolved =
            resolve_catalog_service_tier(&state.tier_models, state.model.as_deref(), &tier);
        if resolved == "default" && tier != "default" {
            return Err(ProviderError::ValidationError {
                message: "service tier is unavailable for this Codex model or account".into(),
            });
        }
        state.service_tier = resolved;
        Ok(())
    }

    /// Gracefully shut the session down: close the JSON-RPC child
    /// (EOF-then-kill), stop background tasks, and flip state to Closed.
    ///
    /// Idempotent: the underlying
    /// [`JsonRpcChild::shutdown`](crate::json_rpc_child::JsonRpcChild::shutdown)
    /// short-circuits on repeat calls, and the tasks list is drained, so
    /// later invocations are cheap no-ops.
    pub async fn shutdown(&self) -> Result<(), ProviderError> {
        self.native_authority.revoke();
        let _outbound = self.outbound.lock().await;
        // Cancel any queued follow-ups so the UI clears its greyed items.
        self.cancel_all_queued().await;

        // Signal background tasks first so they stop pumping events
        // mid-teardown.
        let _ = self.shutdown_tx.send(());

        // Close the JSON-RPC child cleanly (EOF → 2s grace → kill).
        let child = self.child().await;
        let stopped = if self.runtime_lease.lock().await.is_some() {
            child.shutdown_owned().await
        } else {
            child.shutdown().await
        };
        if stopped.is_err() && self.chatgpt_runtime.is_some() {
            let _ = self.event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(self.thread_id.clone()),
                message: "Codex is still stopping. Wait before changing the ChatGPT connection.".into(),
                original_payload: None,
            });
        }

        // Abort any tasks that haven't exited on their own.
        let tasks: Vec<_> = {
            let mut guard = self.tasks.lock().await;
            std::mem::take(&mut *guard)
        };
        for t in tasks {
            t.abort();
            let _ = t.await;
        }
        self.runtime_lease.lock().await.take();
        // Flip internal status so concurrent callers see Closed without
        // waiting for the child to actually exit.
        {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Closed;
            state.active_turn = None;
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
        self.child().await
            .shutdown_managed()
            .await
            .map_err(|error| ProviderError::ProcessError {
                message: "managed Codex stop is unconfirmed".into(),
                source: Some(error.to_string()),
            })?;
        if let Some(context) = self.managed.as_ref() {
            context
                .handler
                .quiesce()
                .await
                .map_err(|error| ProviderError::ProcessError {
                    message: "managed Codex host-tool stop unconfirmed".into(),
                    source: Some(error),
                })?;
        }
        self.shutdown().await

    }
}

const MANAGED_DISABLED_FEATURES: &[&str] = &[
    "multi_agent",
    "multi_agent_v2",
    "shell_tool",
    "unified_exec",
    "hooks",
    "plugins",
    "plugin_hooks",
    "apps",
    "code_mode",
    "code_mode_host",
    "code_mode_only",
    "code_mode_prewarm",
    "browser_use",
    "browser_use_external",
    "browser_use_full_cdp_access",
    "computer_use",
    "in_app_browser",
    "in_app_chat",
    "in_app_dictation",
    "in_app_local_automation",
    "in_app_updates",
    "remote_plugin",
    "skill_mcp_dependency_install",
    "skill_search",
    "tool_suggest",
    "image_generation",
    "view_image",
    "daemon_auto_start",
    "deferred_executor",
    "shell_snapshot",
    "apply_patch_freeform",
];

fn managed_codex_args(disabled_servers: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    for feature in MANAGED_DISABLED_FEATURES {
        args.extend(["-c".into(), format!("features.{feature}=false")]);
    }
    args.extend([
        "-c".into(),
        "agents.enabled=false".into(),
        "-c".into(),
        "web_search=\"disabled\"".into(),
    ]);
    for name in disabled_servers {
        // Quoted TOML keys preserve punctuation; these are argv entries,
        // never shell interpolation or mutations of the user's config.
        args.extend([
            "-c".into(),
            format!(
                "mcp_servers.{}.enabled=false",
                serde_json::to_string(name).unwrap_or_default()
            ),
        ]);
    }
    args.push("app-server".into());
    args
}

fn disabled(value: Option<&Value>) -> bool {
    value.is_some_and(|v| v == &json!(false) || v.get("enabled") == Some(&json!(false)))
}

fn enabled_native_servers(config: &Value) -> Result<Vec<String>, String> {
    let Some(servers) = config.get("mcp_servers") else {
        return Ok(vec![]);
    };
    if servers.is_null() {
        return Ok(vec![]);
    }
    let servers = servers
        .as_object()
        .ok_or("invalid native MCP configuration")?;
    Ok(servers
        .iter()
        .filter(|(_, server)| !disabled(server.get("enabled")))
        .map(|(name, _)| name.clone())
        .collect())
}

fn validate_managed_codex_config(config: &Value, feature_report: &Value) -> Result<(), String> {
    let features = config
        .get("features")
        .and_then(Value::as_object)
        .ok_or("Codex did not report effective feature controls")?;
    for key in MANAGED_DISABLED_FEATURES {
        if !disabled(features.get(*key)) {
            return Err(format!("managed Codex requires features.{key}=false"));
        }
    }
    let reported = feature_report
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Codex feature capabilities are unavailable")?;
    // Require the actual implementation to recognize the essential controls;
    // a permissive config parser accepting unknown keys proves nothing.
    for key in [
        "shell_tool",
        "multi_agent",
        "multi_agent_v2",
        "hooks",
        "plugins",
        "apps",
        "code_mode_host",
    ] {
        let feature = reported
            .iter()
            .find(|feature| feature.get("name").and_then(Value::as_str) == Some(key))
            .ok_or_else(|| format!("Codex lacks verified {key} control"))?;
        if feature.get("enabled") != Some(&json!(false)) {
            return Err(format!("Codex still enables {key}"));
        }
    }
    // 0.160 keeps this execution backend on despite config false. Tool
    // authority is gated separately by shell_tool and the acknowledged empty
    // environment selection; require the backend to be recognized, not off.
    let unified_exec = reported
        .iter()
        .find(|feature| feature.get("name").and_then(Value::as_str) == Some("unified_exec"));
    if unified_exec
        .and_then(|feature| feature.get("enabled"))
        .and_then(Value::as_bool)
        .is_none()
    {
        return Err("Codex lacks a recognized unified_exec backend".into());
    }
    if config.get("web_search").and_then(Value::as_str) != Some("disabled") {
        return Err("Codex web search was not disabled".into());
    }
    if !enabled_native_servers(config)?.is_empty() {
        return Err("native MCPs are still enabled in managed Codex".into());
    }
    Ok(())
}

async fn spawn_managed_codex(
    spawn: &CodexSpawnConfig,
    env: &HashMap<String, String>,
    cwd: &std::path::Path,
    context: &crate::agent_provider::managed::ManagedSession,
    grant: Option<&super::chatgpt::ManagedGrant>,
    lease: Option<Arc<std::fs::File>>,
) -> Result<Arc<JsonRpcChild>, ProviderError> {
    let mut disabled_servers = Vec::new();
    for pass in 0..2 {
        let mut args = managed_codex_args(&disabled_servers);
        let child_env = if let Some(grant) = grant {
            let (plan_args, mut plan_env) = grant.launch(env.clone());
            // Keep the workflow's configured home, and compose its feature
            // restrictions with the explicit bearer-bound model provider.
            if let Some(home) = env.get("CODEX_HOME") {
                plan_env.insert("CODEX_HOME".into(), home.clone());
            }
            args.pop(); // the plan launch supplies the app-server subcommand
            args.extend(plan_args);
            plan_env
        } else { env.clone() };
        let config = SpawnConfig {
            program: spawn.codex_binary.clone(), args, env: child_env,
            cwd: Some(cwd.into()), default_timeout: DEFAULT_RPC_TIMEOUT,
        };
        let spawned = match lease.as_ref() {
            Some(lease) => JsonRpcChild::spawn_managed_owned(config, grant.is_some(), lease.clone()).await,
            None => JsonRpcChild::spawn_managed(config).await,
        };
        let child = Arc::new(
            spawned
            .map_err(|error| match error {
                crate::json_rpc_child::RpcChildError::SpawnFailed(io) => ProviderError::ValidationError {
                    message: format!("managed-start-rejected: setup_required: failed to spawn Codex workflow app-server: {io}"),
                },
                other => ProviderError::ProcessError {
                    message: "managed Codex launch failed".into(),
                    source: Some(other.to_string()),
                },
            })?,
        );
        let mut startup_guard = child.managed_startup_guard();
        let recorded = child
            .managed_evidence()
            .map_err(|e| e.to_string())
            .and_then(|evidence| context.handler.record_runtime(evidence));
        if let Err(error) = recorded {
            child
                .shutdown_managed()
                .await
                .map_err(|stop| ProviderError::ProcessError {
                    message: "managed Codex startup stop unconfirmed".into(),
                    source: Some(stop.to_string()),
                })?;
            return Err(ProviderError::ValidationError {
                message: format!("managed-start-rejected: {error}"),
            });
        }
        let checked = async {
            child
                .request(
                    "initialize",
                    serde_json::to_value(InitializeParams {
                        client_info: spawn.client_info.clone(),
                        capabilities: Capabilities {
                            experimental_api: true,
                        },
                    })
                    .map_err(|e| e.to_string())?,
                )
                .await
                .map_err(|e| e.to_string())?;
            child
                .notify("initialized", json!({}))
                .await
                .map_err(|e| e.to_string())?;
            let response = child
                .request("config/read", json!({"includeLayers":false,"cwd":cwd}))
                .await
                .map_err(|e| e.to_string())?;
            let config = response
                .get("config")
                .ok_or("Codex did not report effective configuration")?;
            let servers = enabled_native_servers(config)?;
            if !servers.is_empty() && pass == 0 {
                return Ok(Some(servers));
            }
            let feature_report = child
                .request("experimentalFeature/list", json!({"limit":1000}))
                .await
                .map_err(|e| e.to_string())?;
            validate_managed_codex_config(config, &feature_report)?;
            Ok::<_, String>(None)
        }
        .await;
        match checked {
            Ok(None) => {
                if let Some(guard) = startup_guard.as_mut() { guard.disarm(); }
                return Ok(child);
            },
            Ok(Some(servers)) => {
                child
                    .shutdown_managed()
                    .await
                    .map_err(|error| ProviderError::ProcessError {
                        message: "managed Codex configuration probe stop unconfirmed".into(),
                        source: Some(error.to_string()),
                    })?;
                disabled_servers = servers;
            }
            Err(error) => {
                child.shutdown_managed().await.map_err(|stop_error| {
                    ProviderError::ProcessError {
                        message: "managed Codex rejected configuration but stop is unconfirmed"
                            .into(),
                        source: Some(stop_error.to_string()),
                    }
                })?;
                return Err(ProviderError::ValidationError {
                    message: format!("managed-start-rejected: {error}"),
                });
            }
        }
    }
    Err(ProviderError::ValidationError {
        message: "managed-start-rejected: native MCP configuration did not stabilize".into(),
    })
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
pub(crate) fn codex_permission_mode_to_policy_pair(mode: Option<&str>) -> Option<(String, String)> {
    match mode? {
        "managed-read-only" => Some(("never".into(), "read-only".into())),
        "read-only" => Some(("untrusted".into(), "read-only".into())),
        "workspace-write" => Some(("on-request".into(), "workspace-write".into())),
        "danger-full-access" => Some(("never".into(), "danger-full-access".into())),
        _ => None,
    }
}

fn restrict_managed_codex_environment(params: &mut Value) {
    // Exact experimental thread/start and turn/start API in Codex 0.160.
    // Omission selects the default environment; [] removes environment-native
    // shell, apply_patch, and view_image tools while retaining dynamic tools.
    // See rust-v0.160.0/core/src/tools/spec_plan.rs:1083,1269,1283,1425.
    params["environments"] = json!([]);
    params["approvalPolicy"] = json!("never");
}

fn validate_managed_codex_thread(response: &Value) -> Result<(), String> {
    if !response
        .get("thread")
        .and_then(|thread| thread.get("environments"))
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        return Err("Codex did not acknowledge an empty native environment selection".into());
    }
    if response.get("approvalPolicy").and_then(Value::as_str) != Some("never") {
        return Err("Codex did not acknowledge approvals disabled".into());
    }
    if response
        .get("sandbox")
        .and_then(|sandbox| sandbox.get("type"))
        .and_then(Value::as_str)
        != Some("readOnly")
    {
        return Err("Codex did not acknowledge the read-only native startup policy".into());
    }
    Ok(())
}

/// Issue a fresh `thread/start` and return the Codex-assigned thread id.
async fn start_fresh_thread(
    child: &JsonRpcChild,
    cwd: PathBuf,
    model: Option<String>,
    permission_mode: Option<String>,
    service_tier: String,
    dynamic_tools: Option<Vec<DynamicToolSpec>>,
) -> Result<ThreadStartResponse, ProviderError> {
    let managed = permission_mode.as_deref() == Some("managed-read-only");
    let (approval_policy, sandbox) =
        match codex_permission_mode_to_policy_pair(permission_mode.as_deref()) {
            Some((ap, sb)) => (Some(ap), Some(sb)),
            None => (None, None),
        };
    let params = ThreadStartParams {
        model,
        // `Some(None)` deliberately serializes as JSON null: it clears a
        // user-level fast default so the composer's Standard choice is honest.
        service_tier: Some((service_tier != "default").then(|| service_tier.clone())),
        cwd: Some(cwd),
        collaboration_mode: None,
        approval_policy,
        sandbox,
        dynamic_tools,
        experimental_raw_events: false,
    };
    let mut params_value = serde_json::to_value(&params).unwrap();
    if managed {
        restrict_managed_codex_environment(&mut params_value);
    }
    let started = async {
        let resp = child
            .request("thread/start", params_value)
            .await
            .map_err(|e| format!("thread/start failed: {e}"))?;
        if managed {
            validate_managed_codex_thread(&resp)?;
        }
        serde_json::from_value::<ThreadStartResponse>(resp)
            .map_err(|e| format!("malformed thread/start response: {e}"))
    }
    .await;
    match started {
        Ok(thread) => Ok(thread),
        Err(message) if managed => {
            // No turn has been started. Refuse older/unknown APIs instead of
            // falling back to native file access, and prove process shutdown.
            child
                .shutdown_managed()
                .await
                .map_err(|error| ProviderError::ProcessError {
                    message: "managed Codex thread rejection stop unconfirmed".into(),
                    source: Some(error.to_string()),
                })?;
            Err(ProviderError::ValidationError {
                message: format!("managed-start-rejected: {message}"),
            })
        }
        Err(message) => Err(ProviderError::RpcError { message }),
    }
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
pub(crate) fn codex_plan_label(plan_type: Option<&str>) -> Option<String> {
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
    let mut notifications = child.notifications();
    tokio::spawn(async move {
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
                            if let NotificationMessage::TurnCompleted(p) = &msg {
                                session.settle_turn_approvals(&p.thread_id, &p.turn_id).await;
                            }
                            let mut demux=session.notification_demux.lock().await;
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
                            drop(demux);
                            for ev in events {
                                if session.chatgpt_runtime.is_some() && matches!(ev,ProviderRuntimeEvent::PlanUsageUpdated{..}){continue}
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
                                _ => {let session=session.clone();tokio::spawn(async move{session.drain_queue().await;});},
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            if session.managed.is_some() {
                                let _=event_tx.send(ProviderRuntimeEvent::RuntimeWarning{
                                    thread_id:Some(session.thread_id.clone()),
                                    message:format!("Managed event stream lagged: native Codex notification channel dropped {count}"),original_payload:None,
                                });
                            }
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
                        if let Some(context) = session.managed.clone() {
                            // Managed callbacks never touch the broad MCP
                            // registry, and cannot block the request reader.
                            let child = Arc::clone(&child);
                            tokio::spawn(async move {
                                let result = match serde_json::from_value::<DynamicToolCallParams>(params) {
                                    Ok(call) => match context.call(&call.tool, call.arguments).await {
                                        Ok(value) => json!({"success":true,"contentItems":[{"type":"inputText","text":value.to_string()}]}),
                                        Err(error) => json!({"success":false,"contentItems":[{"type":"inputText","text":error}]}),
                                    },
                                    Err(error) => json!({"success":false,"contentItems":[{"type":"inputText","text":error.to_string()}]}),
                                };
                                let _ = child.respond(req.id, Ok(result)).await;
                            });
                            continue;
                        }
                        let actor = {
                            let state = session.state.lock().await;
                            let turn = params.get("turnId").and_then(Value::as_str);
                            let owned = params.get("threadId").and_then(Value::as_str) == Some(state.codex_thread_id.as_str())
                                && turn.is_some_and(|turn| state.active_turn.as_ref().is_some_and(|active| active.0 == turn)
                                    || state.pending_turn_start.is_some());
                            owned.then(|| crate::delegation::app::NativeActor {
                                thread_id: session.thread_id.0.clone(), provider: crate::agent_provider::ProviderKind::Codex,
                                workspace_id: session.workspace_id.clone(), session_id: session.provider_session_id.0.clone(),
                                permission_mode: state.permission_mode.clone(),
                                permit: session.native_authority.capture_for_turn(turn.unwrap()),
                            })
                        };
                        if let Some(actor) = &actor {
                            let _ = actor.permit.wait_ready().await;
                        }
                        let result = handle_dynamic_tool_call_native(
                            mcp_registry.as_ref(),
                            session.workspace_id.as_deref(),
                            Some((&session.host_caller, session.provider_session_id.0.as_str())),
                            params, actor,
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
                    let event = translate_server_request(&session.thread_id, &request_id, &msg);
                    let turn_ended = {
                        let payload = msg.payload();
                        let field = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_owned);
                        let pending = PendingApproval {
                            rpc_id: req.id,
                            codex_thread_id: field("threadId"),
                            turn_id: field("turnId"),
                        };
                        let mut state = session.state.lock().await;
                        let thread = pending.codex_thread_id.as_ref().unwrap_or(&state.codex_thread_id);
                        let turn_ended = pending.turn_id.as_ref().is_some_and(|turn| {
                            state.settled_turns.iter().any(|(t, u)| t == thread && u == turn)
                        });
                        // The crash watchdog marks the session dead before it
                        // drains this map under the same lock, so a request
                        // admitted after that drain is expired here instead.
                        let dead = session.dead.load(Ordering::Relaxed);
                        if !turn_ended && !dead {
                            state.pending_approvals.insert(request_id.clone(), pending);
                        }
                        // Publish while still holding the lock: a settlement
                        // on another worker must not broadcast this
                        // request's resolution ahead of its opening.
                        let _ = event_tx.send(event);
                        if dead {
                            let _ = event_tx.send(crate::agent_provider::expired_request_event(
                                session.thread_id.clone(),
                                request_id.clone(),
                            ));
                        }
                        turn_ended && !dead
                    };
                    if turn_ended {
                        let _ = event_tx.send(ProviderRuntimeEvent::RequestResolved {
                            thread_id: session.thread_id.clone(),
                            request_id,
                            decision: ApprovalDecision::Cancel,
                        });
                    }
                }
            }
        }
    })
}

/// The registry's tools, plus the host tools Codemux serves to
/// `host_caller`, as Codex dynamic tools, or `None` without a registry
/// (including every session on a device). With Codex's own config
/// (`codex_home_managed == false`) Codex already loads the user's Codex MCP
/// servers itself, so those are left out to avoid duplicates.
async fn dynamic_tool_specs(
    registry: Option<&McpRegistry>,
    codex_home_managed: bool,
    host_caller: &crate::mcp::registry::HostCaller,
) -> Option<Vec<DynamicToolSpec>> {
    let registry = registry?;
    let mut tools = if codex_home_managed {
        registry.list_all_tools().await
    } else {
        registry
            .list_all_tools_excluding_source(crate::mcp::McpConfigSource::CodexUser)
            .await
    };
    tools.extend(registry.host_tools(host_caller));
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

/// Codemux's instructions for the dynamic tools a thread starts with, so the
/// agent knows when to reach for them (show a page inline rather than only
/// write it to a file; suggest follow-up tasks rather than list them).
fn developer_guidance(tools: &[DynamicToolSpec]) -> Option<String> {
    let names: Vec<String> = tools
        .iter()
        .map(|DynamicToolSpec::Function { name, .. }| registry_tool_name(name))
        .collect();
    crate::mcp_server::tool_guidance(names.iter().map(String::as_str))
}

/// Execute a Codex dynamic-tool request through the process-wide registry and
/// translate the MCP content envelope into Codex's input-content shape. With
/// no registry (a session on a device, which must not reach this computer's
/// tools whatever name it sends) the call fails without dispatching.
#[cfg(test)]
async fn handle_dynamic_tool_call(
    registry: Option<&McpRegistry>,
    workspace_id: Option<&str>,
    // The root chat's caller and its Codex thread id.
    host: Option<(&crate::mcp::registry::HostCaller, &str)>,
    raw: Value,
) -> Value {
    handle_dynamic_tool_call_native(registry, workspace_id, host, raw, None).await
}

async fn handle_dynamic_tool_call_native(
    registry: Option<&McpRegistry>,
    workspace_id: Option<&str>,
    host: Option<(&crate::mcp::registry::HostCaller, &str)>,
    raw: Value,
    actor: Option<crate::delegation::app::NativeActor>,
) -> Value {
    let parsed = serde_json::from_value::<DynamicToolCallParams>(raw);
    let dispatched = match (registry, parsed) {
        (Some(registry), Ok(call)) => {
            let name = registry_tool_name(&call.tool);
            // Host tools answer synchronously (no provider waits), which
            // matters here: this request loop handles approvals too. They act
            // as the root chat, so a call from another thread (a native
            // subagent that inherited the dynamic tools) is refused rather
            // than handed the root's identity and permission mode.
            let host = host.and_then(|(caller, root_thread)| {
                if call.thread_id == root_thread {
                    registry.host_call(caller, &name, &call.arguments)
                } else {
                    registry
                        .host_tools(caller)
                        .iter()
                        .any(|tool| tool.prefixed_name == name)
                        .then(|| {
                            json!({
                                "content": [{ "type": "text", "text": "This tool is only available in the main conversation, not from a subagent." }],
                                "isError": true,
                            })
                        })
                }
            });
            match host {
                Some(result) => Ok(result),
                None => match actor {
                    Some(actor) => registry.dispatch_native_tool_call(&name, call.arguments, actor).await,
                    None => registry.dispatch_tool_call(&name, call.arguments, workspace_id).await,
                }
            }
        }
        (None, _) => Err("Codemux's tools aren't available to this thread".to_string()),
        (_, Err(error)) => Err(format!("invalid dynamic tool call: {error}")),
    };
    let (success, result) = match dispatched {
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
                            let (active_turn, orphaned) = {
                                let mut state = session.state.lock().await;
                                let active_turn = state.active_turn.take();
                                state.status = SessionStatus::Error {
                                    message: msg.clone(),
                                };
                                // A sub-agent can still be waiting on an
                                // approval after the parent turn ended, so
                                // expire every callback, not just the turn's.
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
                if session.native_authority.observe_start(&p.turn_id)
                    || state
                        .active_turn
                        .as_ref()
                        .is_some_and(|turn| turn.0.starts_with("native-"))
                {
                    state.active_turn = Some(TurnId(p.turn_id.clone()));
                }
            }
        }
        NotificationMessage::TurnCompleted(p) => {
            let mut state = session.state.lock().await;
            // Sub-agent turn completions are handled as SubagentUpdated in
            // the translator; leave the parent session state untouched.
            if p.thread_id != state.codex_thread_id {
                return;
            }
            if state.active_turn.as_ref().is_some_and(|t| t.0 != p.turn_id) {
                return;
            }
            if let Some(pending) = state.pending_turn_start.as_mut() {
                if pending.completed_turns.len() == 64 {
                    pending.completed_turns.pop_front();
                }
                pending.completed_turns.push_back(TurnId(p.turn_id.clone()));
            }
            if state
                .active_turn
                .as_ref()
                .map(|t| t.0 == p.turn_id)
                .unwrap_or(false)
            {
                session.native_authority.finish(&p.turn_id);
                state.active_turn = None;
            }
            // Status update follows from translate_turn_completed()
            // which emits a SessionStateChanged event; mirror it here so
            // adapter-internal reads stay consistent.
            state.status = match p.status.as_str() {
                "failed" | "error" => SessionStatus::Error {
                    message: p.error.clone().unwrap_or_else(|| "turn failed".into()),
                },
                _ => SessionStatus::Ready,
            };
        }
        NotificationMessage::Error(e) if !e.will_retry => {
            let mut state = session.state.lock().await;
            // Scope a terminal error to the parent session only when it is
            // unscoped or explicitly targets the parent thread; a
            // sub-agent's error must not fail the whole session.
            if e.thread_id
                .as_deref()
                .map_or(true, |t| t == state.codex_thread_id)
            {
                if let Some(pending) = state.pending_turn_start.as_mut() {
                    pending.failed = true;
                }
                state.status = SessionStatus::Error {
                    message: e.message.clone(),
                };
            }
        }
        _ => {}
    }
}

#[cfg(all(test,unix))]
#[path="managed_tests.rs"]
pub(crate) mod managed_tests;

#[cfg(all(test, unix))]
#[path = "approval_tests.rs"]
mod approval_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_codex_configuration_requires_verified_isolation() {
        let features: serde_json::Map<String, Value> = MANAGED_DISABLED_FEATURES
            .iter()
            .map(|key| (key.to_string(), json!(false)))
            .collect();
        let mut report = json!({"data":MANAGED_DISABLED_FEATURES.iter().map(|key|json!({"name":key,"enabled":false})).collect::<Vec<_>>()});
        let mut config = json!({"features":features,"web_search":"disabled","mcp_servers":{"native":{"enabled":false}}});
        assert!(validate_managed_codex_config(&config, &report).is_ok());
        config["features"]["multi_agent_v2"] = json!({"enabled":false});
        assert!(validate_managed_codex_config(&config, &report).is_ok());
        config["features"]["shell_tool"] = json!(true);
        assert!(validate_managed_codex_config(&config, &report).is_err());
        config["features"]["shell_tool"] = json!(false);
        config["mcp_servers"]["native"]["enabled"] = json!(true);
        assert!(validate_managed_codex_config(&config, &report).is_err());
        config["mcp_servers"]["native"]["enabled"] = json!(false);
        assert!(validate_managed_codex_config(&config, &json!({"data":[]})).is_err());
        let backend = report["data"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|feature| feature["name"] == "unified_exec")
            .unwrap();
        backend["enabled"] = json!(true);
        assert!(validate_managed_codex_config(&config, &report).is_ok());
        let shell = report["data"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|feature| feature["name"] == "shell_tool")
            .unwrap();
        shell["enabled"] = json!(true);
        assert!(validate_managed_codex_config(&config, &report).is_err());
        let args = managed_codex_args(&["name.with.dot\"quoted".into()]);
        assert!(args.contains(&"mcp_servers.\"name.with.dot\\\"quoted\".enabled=false".to_string()));
        assert_eq!(args.last().unwrap(), "app-server");
    }

    #[test]
    fn managed_codex_no_environment_request_preserves_captured_tools() {
        let tool = json!({"name":"workflow_submit_result","description":"Result","inputSchema":{"type":"object"}});
        let mut thread = json!({"dynamicTools":[tool.clone()],"sandbox":"read-only"});
        restrict_managed_codex_environment(&mut thread);
        assert_eq!(thread["environments"], json!([]));
        assert_eq!(thread["approvalPolicy"], "never");
        assert_eq!(thread["sandbox"], "read-only");
        assert_eq!(thread["dynamicTools"], json!([tool]));
        let mut turn = json!({"threadId":"owned","input":[{"type":"text","text":"work"}]});
        restrict_managed_codex_environment(&mut turn);
        assert_eq!(turn["environments"], json!([]));
        assert_eq!(turn["approvalPolicy"], "never");
        assert_eq!(turn["input"][0]["text"], "work");
    }

    #[test]
    fn managed_codex_no_environment_ack_fails_closed() {
        let response = json!({"thread":{"environments":[]},"approvalPolicy":"never","sandbox":{"type":"readOnly"}});
        assert!(validate_managed_codex_thread(&response).is_ok());
        for environments in [Value::Null, json!([{"id":"native"}]), json!({})] {
            let mut rejected = response.clone();
            rejected["thread"]["environments"] = environments;
            assert!(validate_managed_codex_thread(&rejected).is_err());
        }
        let mut missing = response.clone();
        missing["thread"]
            .as_object_mut()
            .unwrap()
            .remove("environments");
        assert!(validate_managed_codex_thread(&missing).is_err());
        for key in ["approvalPolicy", "sandbox"] {
            let mut missing = response.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(validate_managed_codex_thread(&missing).is_err());
        }
        let mut approvals = response.clone();
        approvals["approvalPolicy"] = json!("on-request");
        assert!(validate_managed_codex_thread(&approvals).is_err());
        let mut writable = response;
        writable["sandbox"]["type"] = json!("workspaceWrite");
        assert!(validate_managed_codex_thread(&writable).is_err());
    }

    #[test]
    fn runtime_identity_uses_the_resolved_turn_settings() {
        let mode = runtime_collaboration_mode(Some("gpt-5.6-sol"), Some("high"), None).unwrap();
        assert_eq!(mode.mode, "default");
        assert_eq!(mode.settings.model, "gpt-5.6-sol");
        assert_eq!(mode.settings.reasoning_effort.as_deref(), Some("high"));
        assert!(mode.settings.developer_instructions.ends_with(
            "<runtime_info>In case you're asked: you are running in CodeMux through the Codex harness, as gpt-5.6-sol with high reasoning effort. No need to mention this otherwise.</runtime_info>"
        ));

        assert!(runtime_collaboration_mode(None, Some("high"), None).is_none());
    }

    #[test]
    fn developer_instructions_guide_visuals_only_when_html_render_is_listed() {
        let tool = |registry_name: &str| DynamicToolSpec::Function {
            name: codex_dynamic_tool_name(registry_name),
            description: String::new(),
            input_schema: json!({}),
            defer_loading: None,
        };
        let guidance = developer_guidance(&[
            tool("mcp__codemux__browser_click"),
            tool("mcp__codemux__html_render"),
        ]);
        assert_eq!(guidance.as_deref(), Some(crate::mcp_server::HTML_RENDER_GUIDANCE));
        assert!(developer_guidance(&[]).is_none());
        assert!(developer_guidance(&[tool("mcp__other__html_render")]).is_none());

        let mode =
            runtime_collaboration_mode(Some("gpt-5.6-sol"), None, guidance.as_deref()).unwrap();
        let instructions = mode.settings.developer_instructions;
        assert!(instructions.contains(crate::mcp_server::HTML_RENDER_GUIDANCE));
        assert!(instructions.ends_with("</runtime_info>"), "runtime identity stays last");
    }

    #[test]
    fn developer_instructions_guide_suggestions_only_when_suggest_tasks_is_listed() {
        let tool = |registry_name: &str| DynamicToolSpec::Function {
            name: codex_dynamic_tool_name(registry_name),
            description: String::new(),
            input_schema: json!({}),
            defer_loading: None,
        };
        let suggest = crate::commands::follow_ups::SUGGEST_TASKS_GUIDANCE;
        assert_eq!(
            developer_guidance(&[tool("mcp__codemux__suggest_tasks")]).as_deref(),
            Some(suggest)
        );
        let both = developer_guidance(&[
            tool("mcp__codemux__html_render"),
            tool("mcp__codemux__suggest_tasks"),
        ])
        .unwrap();
        assert!(both.starts_with(crate::mcp_server::HTML_RENDER_GUIDANCE));
        assert!(both.ends_with(suggest));
        assert!(developer_guidance(&[tool("mcp__other__suggest_tasks")]).is_none());
        let mode = runtime_collaboration_mode(Some("gpt-5.6-sol"), None, Some(&both)).unwrap();
        assert!(mode.settings.developer_instructions.contains(suggest));
    }

    #[test]
    fn codex_plan_labels_are_humanized_and_unknowns_pass_through() {
        assert_eq!(
            codex_plan_label(Some("pro")).as_deref(),
            Some("ChatGPT Pro")
        );
        assert_eq!(
            codex_plan_label(Some("plus")).as_deref(),
            Some("ChatGPT Plus")
        );
        assert_eq!(
            codex_plan_label(Some("Team")).as_deref(),
            Some("ChatGPT Team")
        );
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
        assert!(is_recoverable_resume_error(
            "rpc error -32600: no rollout found for thread id 019db5ad"
        ));
        assert!(is_recoverable_resume_error(
            "rpc error: Thread not found (code 42)"
        ));
        assert!(is_recoverable_resume_error("missing thread"));
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
    pub(super) fn queued(id: &str) -> QueuedTurn {
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
                dispatch_guard: None,
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
            None,
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

    struct HandOffHost;

    impl crate::mcp::registry::HostTools for HandOffHost {
        fn tools_for(&self, _caller: &crate::mcp::registry::HostCaller) -> Vec<crate::mcp::runtime::McpTool> {
            vec![crate::mcp::runtime::McpTool {
                name: "delegate_task".into(),
                prefixed_name: "mcp__codemux__delegate_task".into(),
                description: None,
                input_schema: json!({}),
                server_id: "codemux-host".into(),
            }]
        }

        fn call(
            &self,
            caller: &crate::mcp::registry::HostCaller,
            prefixed_name: &str,
            _arguments: &Value,
        ) -> Option<Value> {
            (prefixed_name == "mcp__codemux__delegate_task").then(|| {
                json!({ "content": [{ "type": "text", "text": format!("as {}", caller.thread_id) }] })
            })
        }
    }

    #[tokio::test]
    async fn host_tools_answer_only_the_root_thread() {
        let registry = McpRegistry::new();
        registry.set_host_tools(Arc::new(HandOffHost));
        let caller = crate::mcp::registry::HostCaller {
            thread_id: "codemux-chat".into(),
            provider: crate::agent_provider::ProviderKind::Codex,
            workspace_id: None,
            permission_mode: Some("danger-full-access".into()),
        };
        let request = |thread: &str| {
            json!({
                "threadId": thread,
                "turnId": "turn",
                "callId": "call",
                "namespace": null,
                "tool": "codemux_mcp__codemux__delegate_task",
                "arguments": {}
            })
        };

        let root = handle_dynamic_tool_call(
            Some(&registry),
            None,
            Some((&caller, "codex-root")),
            request("codex-root"),
        )
        .await;
        assert_eq!(root["success"], true, "{root}");
        assert_eq!(root["contentItems"][0]["text"], "as codemux-chat");

        // A native subagent that inherited the dynamic tools has its own
        // thread id and must not act as the root chat.
        let native_child = handle_dynamic_tool_call(
            Some(&registry),
            None,
            Some((&caller, "codex-root")),
            request("codex-native-child"),
        )
        .await;
        assert_eq!(native_child["success"], false, "{native_child}");
        assert!(native_child["contentItems"][0]["text"]
            .as_str()
            .unwrap()
            .contains("only available in the main conversation"));
    }

    #[tokio::test]
    async fn dynamic_tool_call_without_registry_returns_failed_response() {
        let response = handle_dynamic_tool_call(
            None,
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
            .contains("aren't available to this thread"));
    }

    #[tokio::test]
    async fn device_session_gets_and_runs_none_of_this_computers_tools() {
        let (_server, registry, call) =
            crate::mcp::registry::test_support::codemux_remote_server("ws-42").await;
        let caller = crate::mcp::registry::HostCaller {
            thread_id: "codemux-chat".into(),
            provider: crate::agent_provider::ProviderKind::Codex,
            workspace_id: Some("ws-42".into()),
            permission_mode: None,
        };
        let local = crate::agent_provider::local_tool_registry(Some(&registry), None);
        let published = dynamic_tool_specs(local.as_ref(), false, &caller).await.unwrap();
        assert_eq!(published.len(), 1);

        let device = crate::agent_provider::types::RemoteSpawnTarget {
            host_id: 3,
            ssh_target: "deus@ai-node".into(),
            host_name: "ai-node".into(),
        };
        let on_device = crate::agent_provider::local_tool_registry(Some(&registry), Some(&device));
        assert!(dynamic_tool_specs(on_device.as_ref(), true, &caller).await.is_none());

        // A call the device sends anyway, by a name it guessed, fails and
        // never reaches the registry.
        let response = handle_dynamic_tool_call(
            on_device.as_ref(),
            Some("ws-42"),
            None,
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
        assert_eq!(response["success"], false);
        assert!(!call.matched_async().await);
    }

    fn rpc_failure(code: i64) -> TurnStartError {
        TurnStartError::Rpc(crate::json_rpc_child::RpcChildError::RpcError(
            crate::json_rpc_child::RpcError {
                code,
                message: "thread not found".into(),
                data: None,
            },
        ))
    }

    #[test]
    fn plain_send_failure_keeps_turn_start_wording() {
        // An ordinary user send must not borrow the answer-delivery
        // wording; the toast keeps naming the RPC that failed.
        for code in [-32000, -32603, -32600] {
            let ProviderError::RpcError { message } = plain_send_error(rpc_failure(code)) else {
                panic!("expected an rpc error");
            };
            assert!(message.starts_with("turn/start failed: "), "{message}");
            assert!(!message.contains("Answer delivery"), "{message}");
        }
        let malformed = serde_json::from_value::<TurnStartResponse>(json!({})).unwrap_err();
        let ProviderError::RpcError { message } =
            plain_send_error(TurnStartError::Malformed(malformed))
        else {
            panic!("expected an rpc error");
        };
        assert!(
            message.starts_with("malformed turn/start response: "),
            "{message}"
        );
    }

    #[test]
    fn question_turn_start_failure_distinguishes_unknown_from_rejected() {
        use crate::agent_provider::QuestionDeliveryError as Error;
        assert!(matches!(
            question_turn_start_error(rpc_failure(-32000)),
            Error::Unknown(m) if m.starts_with("Answer delivery could not be confirmed: ")
        ));
        assert!(matches!(
            question_turn_start_error(rpc_failure(-32600)),
            Error::Rejected(_)
        ));
        let malformed = serde_json::from_value::<TurnStartResponse>(json!({})).unwrap_err();
        assert!(matches!(
            question_turn_start_error(TurnStartError::Malformed(malformed)),
            Error::Unknown(m) if m.starts_with("Malformed turn/start response: ")
        ));
    }
}

/// Why `turn/start` produced no turn id. Kept raw so each caller words
/// the failure for its own surface: a plain user send reports a generic
/// RPC failure, while answer delivery must tell a confirmed rejection
/// apart from one whose outcome is unknown.
#[derive(Debug)]
enum TurnStartError {
    Admission(ProviderError),
    Rpc(crate::json_rpc_child::RpcChildError),
    Malformed(serde_json::Error),
}

/// Wording for an ordinary user send that failed at `turn/start`.
fn plain_send_error(error: TurnStartError) -> ProviderError {
    ProviderError::RpcError {
        message: match error {
            TurnStartError::Admission(e) => e.to_string(),
            TurnStartError::Rpc(e) => format!("turn/start failed: {e}"),
            TurnStartError::Malformed(e) => format!("malformed turn/start response: {e}"),
        },
    }
}

/// Wording for a question answer that failed at `turn/start`.
fn question_turn_start_error(
    error: TurnStartError,
) -> crate::agent_provider::QuestionDeliveryError {
    match error {
        TurnStartError::Admission(e) => crate::agent_provider::QuestionDeliveryError::Rejected(e.to_string()),
        TurnStartError::Rpc(e) => question_rpc_error(e),
        TurnStartError::Malformed(e) => crate::agent_provider::QuestionDeliveryError::Unknown(
            format!("Malformed turn/start response: {e}"),
        ),
    }
}

fn question_rpc_error(
    error: crate::json_rpc_child::RpcChildError,
) -> crate::agent_provider::QuestionDeliveryError {
    use crate::agent_provider::QuestionDeliveryError as Error;
    match error {
        // The transport also uses -32000 when its child exits with an
        // outstanding request. That is not proof the provider rejected it.
        crate::json_rpc_child::RpcChildError::RpcError(error)
            if error.code == -32000 || error.code == -32603 =>
        {
            Error::Unknown(format!("Answer delivery could not be confirmed: {error}"))
        }
        crate::json_rpc_child::RpcChildError::RpcError(error) => Error::Rejected(error.to_string()),
        other => Error::Unknown(format!("Answer delivery could not be confirmed: {other}")),
    }
}

fn steer_precondition_rejected(error: &crate::json_rpc_child::RpcError) -> bool {
    // Only explicit server rejections are eligible for retry; never timeouts.
    let message = error.message.to_ascii_lowercase();
    matches!(error.code, -32600 | -32000)
        && (message.contains("no active turn")
            || message.contains("expected turn")
            || message.contains("turn id mismatch"))
}

fn find_answer_in_history(
    thread: &Value,
    submission_id: &str,
) -> Option<crate::agent_provider::QuestionDelivery> {
    use crate::agent_provider::QuestionDelivery;
    for turn in thread.get("turns")?.as_array()? {
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };
        for (index, item) in items
            .iter()
            .filter(|item| item["type"] == "userMessage")
            .enumerate()
        {
            if item["type"] == "userMessage" && item["clientId"].as_str() == Some(submission_id) {
                let turn_id = TurnId(turn.get("id")?.as_str()?.to_string());
                return Some(if index == 0 {
                    QuestionDelivery::NewTurn { turn_id }
                } else {
                    QuestionDelivery::Inflight { turn_id }
                });
            }
        }
    }
    None
}

fn turn_input_items(
    text: String,
    images: Vec<crate::agent_provider::ImageInput>,
    skill_invocations: Vec<crate::skills::ResolvedSkillInvocation>,
) -> Vec<TurnInputItem> {
    use base64::Engine;
    let mut input_items: Vec<TurnInputItem> =
        Vec::with_capacity(images.len() + skill_invocations.len() + 1);
    for img in images {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&img.data);
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

    input_items
}
