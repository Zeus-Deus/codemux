use serde_json::Value;
// MCP runtime registry.
//
// Holds the live `McpServerHandle`s plus the disabled-id set the
// frontend syncs from its zustand store. All mutations emit
// `mcp-status-changed` so the Settings UI doesn't have to poll.
//
// Locked decisions from the Step 9 research:
//
// * **Lazy spawn.** Children are only started by an explicit
//   `ensure_started` / `prime_for_chat` call — never at app boot.
//   Stage 2 wires `prime_for_chat` into `agent_chat_start_session` and
//   into the Settings panel's mount effect (the latter so users can
//   inspect status without first opening a chat).
// * **App-shutdown kill.** `shutdown_all` is called from the Tauri
//   `RunEvent::Exit` handler in `lib.rs`. Each child gets a graceful
//   2-second budget via `JsonRpcChild::shutdown` before SIGKILL.
// * **No auto-restart.** A crashed server stays in `Errored`; the user
//   restarts manually via `restart_mcp_server`.
// * **Codemux exception.** The hardcoded `Codemux` source row is
//   always-on; toggling it is suppressed at the command layer.

mod plugin;
pub use plugin::{
    plugin_bridge_tools, plugin_catalog_key, plugin_catalog_notice, plugin_server_id,
    strip_plugin_catalog_notice, PluginMcpActivity, PluginMcpServer,
};

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::{broadcast, Mutex, OnceCell};

use super::codemux_self::codemux_self_config;
use super::http_client::HttpRequestAdmission;
use super::parser::{
    parse_claude_wrapped_config, parse_codex_config_file, parse_mcp_config_file,
    parse_opencode_config_file,
};
use super::paths::{enumerate_mcp_paths_with_home, is_claude_wrapped_path};
use super::runtime::{McpServerHandle, McpServerStatus, McpTool};
use super::{McpConfigSource, McpProjectScope, McpServerConfig};

// Pause an approved first start immediately before its reservation in the
// revocation race test. Scoped to that test's task and absent in production.
#[cfg(test)]
tokio::task_local! {
    static APPROVAL_CHECKED_GATE: (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);
    static BEFORE_PROJECT_ADMISSION_GATE: (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);
}

/// Tauri event the frontend listens to for live status updates.
pub const MCP_STATUS_CHANGED_EVENT: &str = "mcp-status-changed";

/// One row's runtime state, cheap to clone for events / list queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRuntime {
    pub id: String,
    pub name: String,
    pub status: McpServerStatus,
    pub tools_count: usize,
    pub error_message: Option<String>,
    pub stderr_tail: Option<String>,
    pub started_at_ms: Option<i64>,
}

impl McpServerRuntime {
    fn from_handle(h: &McpServerHandle) -> Self {
        let error_message = match &h.status {
            McpServerStatus::Errored { message } => Some(message.clone()),
            _ => None,
        };
        Self {
            id: h.config.id.clone(),
            name: h.config.name.clone(),
            tools_count: h.tools.len(),
            status: h.status.clone(),
            error_message,
            stderr_tail: h.stderr_tail.clone(),
            started_at_ms: h.started_at_ms,
        }
    }
}

/// Process-wide registry of MCP children. Stored as Tauri managed state
/// (`tauri::Manager::manage`) so commands can reach it via
/// `State<'_, McpRegistry>`. `Clone` is cheap (Arc bump) — used by the
/// Tauri Exit handler to take an owned handle into the shutdown future.
///
/// The `status_tx` broadcaster fans every status transition out to
/// in-process subscribers. The Claude adapter uses this to push
/// `update-mcp-tools` to live SDK sessions when an MCP server starts
/// or stops mid-chat (Stage 4 dynamic refresh). The Tauri event
/// emission (`mcp-status-changed`) is independent — frontend
/// subscribers go through that channel.
#[derive(Clone)]
pub struct McpRegistry {
    inner: Arc<Mutex<RegistryInner>>,
    status_tx: broadcast::Sender<McpServerRuntime>,
    /// Coalesces concurrent config discovery for chat priming. Plugin
    /// registration and shutdown use independent per-server locks.
    prime_lock: Arc<Mutex<()>>,
    /// Serializes project approval checks and start reservations with revocation
    /// across server ids (including the same definition in another worktree).
    project_approval_lock: Arc<Mutex<()>>,
    lifecycle: Arc<std::sync::Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>>,
    /// Lazily-started authenticated loopback gateway shared by provider
    /// adapters that need a remote MCP URL (OpenCode and future providers).
    gateway: Arc<OnceCell<Arc<super::gateway::McpGatewayRuntime>>>,
    native_delegation: Arc<std::sync::Mutex<Option<NativeDelegationHandler>>>,
    /// Tools Codemux serves itself, in-process, to its own Claude and Codex
    /// chats and authenticated gateway sessions (see [`HostTools`]). Set once at startup.
    host: Arc<std::sync::OnceLock<Arc<dyn HostTools>>>,
    plugin_epoch: Arc<std::sync::atomic::AtomicU64>,
    activity: Arc<std::sync::OnceLock<Arc<dyn Fn(PluginMcpActivity) + Send + Sync>>>,
}

/// The chat calling a host tool, as identified by the adapter session that
/// received the call. Nothing in it comes from the model's arguments, so a
/// tool can trust it for permission and ownership checks.
#[derive(Debug, Clone)]
pub struct HostCaller {
    pub control: Option<crate::agent_control::ControlCaller>,
    pub thread_id: String,
    pub provider: crate::agent_provider::ProviderKind,
    /// Owning workspace of the session's chat pane, when it has one.
    pub workspace_id: Option<String>,
    /// Claude: the session's live mode. Codex: the mode its session started
    /// with, since Codex cannot change mode mid-session.
    pub permission_mode: Option<String>,
}

/// Tools Codemux answers itself instead of forwarding to an MCP child.
///
/// Deliberately invisible to [`McpRegistry::list_all_tools`] and
/// [`McpRegistry::dispatch_tool_call`]: unbound MCP clients cannot identify
/// the calling chat. Session adapters and authenticated per-chat gateway
/// credentials list and call these through the native route.
pub trait HostTools: Send + Sync {
    /// Tools to offer `caller`, already prefixed like registry tools.
    fn tools_for(&self, caller: &HostCaller) -> Vec<McpTool>;
    fn call_async<'a>(&'a self, caller: &'a HostCaller, name: &'a str, args: &'a Value, call_id: Option<&'a str>, run_id: Option<&'a str>) -> std::pin::Pin<Box<dyn std::future::Future<Output=Option<Value>> + Send + 'a>> {
        let _=(call_id,run_id);
        Box::pin(async move {self.call(caller,name,args)})
    }
    /// Answer a call as an MCP `tools/call` result (`content` + `isError`),
    /// or `None` when `prefixed_name` is not a host tool. This synchronous
    /// fallback must never wait on a provider; native adapters use `call_async`.
    fn call(
        &self,
        caller: &HostCaller,
        prefixed_name: &str,
        arguments: &serde_json::Value,
    ) -> Option<serde_json::Value>;
}

/// One registry slot serves each independent in-process host tool owner.
pub struct CompositeHostTools(pub Vec<Arc<dyn HostTools>>);

impl HostTools for CompositeHostTools {
    fn tools_for(&self, caller: &HostCaller) -> Vec<McpTool> {
        self.0
            .iter()
            .flat_map(|host| host.tools_for(caller))
            .collect()
    }

    fn call_async<'a>(&'a self,caller:&'a HostCaller,name:&'a str,args:&'a Value,call_id:Option<&'a str>,run_id:Option<&'a str>)->std::pin::Pin<Box<dyn std::future::Future<Output=Option<Value>> + Send + 'a>> {
        Box::pin(async move {for host in &self.0 {if let Some(result)=host.call_async(caller,name,args,call_id,run_id).await {return Some(result);}} None})
    }
    fn call(
        &self,
        caller: &HostCaller,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        self.0
            .iter()
            .find_map(|host| host.call(caller, name, arguments))
    }
}

impl std::fmt::Debug for McpRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The inner Mutex is async; logging the handle count would
        // require taking the lock and risking a deadlock from a Debug
        // impl. Print the type only; this is plenty for the
        // Tauri-state log lines that trigger the Debug requirement.
        f.debug_struct("McpRegistry").finish_non_exhaustive()
    }
}

impl McpRegistry {
    /// Cheap clone of the registry handle. Exit-handler shorthand.
    pub fn clone_handle(&self) -> Self {
        self.clone()
    }

    /// Subscribe to the in-process status broadcaster. Each transition
    /// (Discovered → Starting → Running / Errored / Stopped) yields
    /// one `McpServerRuntime` row. The Claude adapter uses this to
    /// push fresh tool snapshots to live SDK sessions for dynamic
    /// refresh.
    pub fn subscribe_status(&self) -> broadcast::Receiver<McpServerRuntime> {
        self.status_tx.subscribe()
    }
}

struct RegistryInner {
    plugins: HashMap<String, plugin::PluginRegistration>,
    watchers: HashMap<String, tokio::task::AbortHandle>,
    cancel_calls: HashMap<String, tokio::sync::watch::Sender<bool>>,
    calls_in_flight: HashMap<String, Arc<plugin::CallTracker>>,
    /// Keyed by `McpServerConfig.id`. Codemux's hardcoded row is also
    /// in here under `"codemux-self"` once spawned.
    handles: HashMap<String, McpServerHandle>,
    /// Mirror of the frontend zustand `disabledIds` — written by
    /// `set_disabled_ids` whenever the store changes. Disabled servers
    /// are skipped during `prime_for_chat` and stopped if currently
    /// running.
    disabled_ids: HashSet<String>,
    /// Native config sources discovered in each checkout. A merged global
    /// handle may also appear in a checkout's Claude-local or project file;
    /// that native source must hide the handle only in that checkout.
    native_scoped_sources: HashMap<PathBuf, HashSet<(String, McpConfigSource)>>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        // Capacity 256: status events are bursty (one per server during
        // a prime cycle). Subscribers (`ClaudeSession::on_mcp_status`)
        // re-collect tools on each event so dropping a Lagged frame is
        // a recoverable no-op.
        let (status_tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                handles: HashMap::new(),
                plugins: HashMap::new(),
                watchers: HashMap::new(),
                cancel_calls: HashMap::new(),
                calls_in_flight: HashMap::new(),
                disabled_ids: HashSet::new(),
                native_scoped_sources: HashMap::new(),
            })),
            status_tx,
            prime_lock: Arc::new(Mutex::new(())),
            project_approval_lock: Arc::new(Mutex::new(())),
            lifecycle: Arc::new(std::sync::Mutex::new(HashMap::new())),
            gateway: Arc::new(OnceCell::new()),
            native_delegation: Arc::new(std::sync::Mutex::new(None)),
            host: Arc::new(std::sync::OnceLock::new()),
            plugin_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            activity: Arc::new(std::sync::OnceLock::new()),
        }
    }
}

/// The legacy self tools a chat below full access may still call: they read,
/// or (remote delegation) are granted per host.
pub(crate) const RELAY_READ_TOOLS: [&str; 26] = [
    "browser_snapshot", "browser_accessibility_snapshot", "browser_screenshot", "browser_console_logs", "browser_get_styles", "browser_viewport_presets", "browser_wait",
    "app_status", "pane_list", "port_list",
    "conversation_read", "conversation_search", "workspace_list", "workspace_info", "workspace_archive_list",
    "automation_list", "automation_get", "automation_runs", "git_status", "git_diff", "terminal_read",
    "issue_list", "issue_get", "preset_list", "delegation_targets", "delegate_remote_task",
];

/// Full-access native callers retain the base self-server catalog.
/// Provider adapters apply the same predicate to discovery and dispatch.
pub(crate) fn native_relay_allowed(raw:&str, full_access:bool)->bool {
    full_access || RELAY_READ_TOOLS.contains(&raw)
}
pub(crate) fn native_tool_visible(tool:&McpTool, caller:&HostCaller)->bool {
    if matches!(
        tool.name.as_str(),
        "delegation_targets" | "delegate_remote_task"
    ) && !matches!(
        caller.provider,
        crate::agent_provider::ProviderKind::Claude | crate::agent_provider::ProviderKind::Codex
    ) {
        return false;
    }
    let full = crate::agent_control::permissions::Ceiling::for_mode(caller.provider, caller.permission_mode.as_deref()).is_some_and(|c| c.full_access());
    !tool.prefixed_name.starts_with("mcp__codemux__") || native_relay_allowed(&tool.name, full)
}

pub(crate) type NativeDelegationHandler = Arc<
    dyn Fn(
            crate::delegation::app::NativeActor,
            String,
            serde_json::Value,
            Option<String>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
        > + Send
        + Sync,
>;
impl McpRegistry {
    pub(crate) fn set_native_delegation_handler(&self, handler: NativeDelegationHandler) {
        *self.native_delegation.lock().unwrap() = Some(handler);
    }
    #[cfg(test)]
    pub(crate) async fn dispatch_native_tool_call(
        &self,
        name: &str,
        arguments: serde_json::Value,
        actor: crate::delegation::app::NativeActor,
    ) -> Result<serde_json::Value, String> {
        self.dispatch_native_tool_call_with_id(name, arguments, actor, None).await
    }
    pub(crate) async fn dispatch_native_tool_call_scoped(
        &self,
        name: &str,
        arguments: serde_json::Value,
        actor: crate::delegation::app::NativeActor,
        project: Option<&McpProjectScope>,
    ) -> Result<serde_json::Value, String> {
        self.dispatch_native_tool_call_with_id_scoped(name, arguments, actor, None, project).await
    }
    pub(crate) async fn dispatch_native_tool_call_with_id(
        &self, name: &str, arguments: serde_json::Value,
        actor: crate::delegation::app::NativeActor, call_id: Option<String>,
    ) -> Result<serde_json::Value, String> {
        self.dispatch_native_tool_call_with_id_scoped(name, arguments, actor, call_id, None).await
    }
    pub(crate) async fn dispatch_native_tool_call_with_id_scoped(
        &self, name: &str, arguments: serde_json::Value,
        actor: crate::delegation::app::NativeActor, call_id: Option<String>,
        project: Option<&McpProjectScope>,
    ) -> Result<serde_json::Value, String> {
        if let Some(raw) = crate::agent_control::native::tool_name(name) {
            let handler = self.native_delegation.lock().unwrap().clone()
                .ok_or("Native orchestration service is unavailable")?;
            let value = handler(actor, raw.into(), arguments, call_id).await?;
            return Ok(if value.get("content").is_some() { value } else {serde_json::json!({"content":[{"type":"text","text":value.to_string()}]})});
        }
        let raw = {
            let inner = self.inner.lock().await;
            inner
                .handles
                .values()
                .filter(|h| {
                    h.config.id == "codemux-self"
                        && h.config.sources.contains(&super::McpConfigSource::Codemux)
                        && h.status.is_running()
                })
                .flat_map(|h| h.tools.iter())
                .find(|t| t.prefixed_name == name)
                .map(|t| t.name.clone())
        };
        if let Some(raw) = raw.filter(|r| {
            matches!(
                r.as_str(),
                "delegation_targets"
                    | crate::mcp_server::REMOTE_DELEGATE_TOOL
                    | "task_status"
                    | "task_cancel"
            )
        }) {
            let handler = self
                .native_delegation
                .lock()
                .unwrap()
                .clone()
                .ok_or("Native delegation coordinator is unavailable")?;
            // Keep the coordinator/Tauri command stable while giving remote MCP
            // launch a distinct identity from the local delegate_task host tool.
            let command = raw.as_str();
            let value = handler(actor, command.into(), arguments, call_id).await?;
            return Ok(serde_json::json!({"content":[{"type":"text","text":value.to_string()}]}));
        }
        if let Some(raw) = name.strip_prefix("mcp__codemux__") {
            let handler = self.native_delegation.lock().unwrap().clone().ok_or("Native caller policy is unavailable")?;
            return handler(actor, raw.into(), arguments, call_id).await;
        }
        self.dispatch_tool_call_scoped(name, arguments, actor.workspace_id.as_deref(), project).await
    }
    pub fn set_activity_emitter(&self, emit: Arc<dyn Fn(PluginMcpActivity) + Send + Sync>) {
        let _ = self.activity.set(emit);
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Return the process-wide authenticated MCP gateway, starting it on an
    /// ephemeral loopback port on first use.
    pub async fn gateway_connection(
        &self,
    ) -> Result<super::gateway::McpGatewayConnection, String> {
        Ok(self.gateway().await?.connection())
    }

    /// The gateway connection for one chat (`key` is its thread id): a token
    /// of its own that tells the gateway the chat's workspace and which
    /// sources its agent already loads.
    pub async fn gateway_chat_connection(
        &self,
        key: &str,
        caller: super::gateway::GatewayCaller,
    ) -> Result<super::gateway::McpGatewayConnection, String> {
        Ok(self.gateway().await?.chat_connection(key, caller))
    }

    /// The gateway if it is already running; never starts it.
    pub fn started_gateway(&self) -> Option<Arc<super::gateway::McpGatewayRuntime>> {
        self.gateway.get().cloned()
    }

    /// The process-wide gateway, started on first use. For callers that must
    /// issue a chat connection synchronously, inside their own critical
    /// section.
    pub async fn gateway_runtime(&self) -> Result<Arc<super::gateway::McpGatewayRuntime>, String> {
        self.gateway().await.cloned()
    }

    async fn gateway(&self) -> Result<&Arc<super::gateway::McpGatewayRuntime>, String> {
        let registry = self.clone();
        self.gateway
            .get_or_try_init(|| async move {
                super::gateway::start_gateway(registry).await.map(Arc::new)
            })
            .await
    }

    /// Install the process-wide host tools. Only the first call wins; every
    /// clone of this registry (one per provider adapter) sees it.
    pub fn set_host_tools(&self, tools: Arc<dyn HostTools>) {
        let _ = self.host.set(tools);
    }

    /// Host tools to offer `caller`. Empty until [`set_host_tools`] runs.
    ///
    /// [`set_host_tools`]: Self::set_host_tools
    pub fn host_tools(&self, caller: &HostCaller) -> Vec<McpTool> {
        self.host
            .get()
            .map(|host| host.tools_for(caller))
            .unwrap_or_default()
    }

    fn collision_host_tools(&self) -> Vec<McpTool> {
        self.host_tools(&HostCaller {
            control: None,
            thread_id: String::new(),
            provider: crate::agent_provider::ProviderKind::Claude,
            workspace_id: None,
            permission_mode: Some("bypassPermissions".into()),
        })
    }

    /// Answer `prefixed_name` if it is a host tool; `None` means "not mine,
    /// dispatch it to the MCP children as usual".
    pub fn host_call(
        &self,
        caller: &HostCaller,
        prefixed_name: &str,
        arguments: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        self.host.get()?.call(caller, prefixed_name, arguments)
    }

    pub async fn host_call_async(&self,caller:&HostCaller,name:&str,args:&Value,call_id:Option<&str>,run_id:Option<&str>)->Option<Value> {
        self.host.get()?.call_async(caller,name,args,call_id,run_id).await
    }

    pub(crate) async fn host_call_native(
        &self, caller: &HostCaller, name: &str, args: &Value,
        call_id: Option<&str>, run_id: Option<&str>,
        actor: Option<&crate::delegation::app::NativeActor>,
    ) -> Option<Value> {
        // Native orchestration must acquire a qualified thread principal before
        // reaching a host tool. Admission rechecks the turn fence after any
        // asynchronous capability lookup, in the same critical section as Stop.
        if let Some(actor) = actor.filter(|_| crate::agent_control::native::tool_name(name).is_some()) {
            let result = match actor.permit.identity() {
                Ok(_) => self.dispatch_native_tool_call_with_id(name, args.clone(), actor.clone(), call_id.map(str::to_string)).await,
                Err(error) => Err(error),
            };
            return Some(match result {
                Ok(result) => result,
                Err(error) => serde_json::json!({"content":[{"type":"text","text":error}],"isError":true}),
            });
        }
        if name == "mcp__codemux__delegate_task" {
            return Some(serde_json::json!({"content":[{"type":"text","text":"Native caller runtime/turn authority was revoked"}],"isError":true}));
        }
        // Adapters that do not name the turn (Claude) still have the actor's.
        let actor_turn = actor
            .filter(|_| run_id.is_none())
            .and_then(|actor| actor.permit.identity().ok())
            .and_then(|identity| identity.run_id);
        let run_id = run_id.or(actor_turn.as_deref());
        self.host.get()?.call_async(caller, name, args, call_id, run_id).await
    }

    /// Tell live sessions to re-collect their tool lists because the host
    /// tool surface changed (e.g. a setting that gates it flipped). Rides the
    /// in-process status bus only — the Settings UI has no row for it.
    pub fn notify_host_tools_changed(&self) {
        let row = McpServerRuntime {
            id: "codemux-host".to_string(),
            name: "codemux".to_string(),
            status: McpServerStatus::Running { tool_count: 0 },
            tools_count: 0,
            error_message: None,
            stderr_tail: None,
            started_at_ms: None,
        };
        let _ = self.status_tx.send(row);
    }

    /// Mirror the frontend's disabled-set into the registry. The next
    /// `prime_for_chat` call respects it; servers currently running but
    /// just-disabled get stopped.
    pub async fn set_disabled_ids<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        ids: Vec<String>,
    ) -> Result<(), String> {
        let mut to_stop: Vec<String> = Vec::new();
        let to_start;
        {
            let mut inner = self.inner.lock().await;
            let next: HashSet<String> = ids.iter().cloned().collect();
            to_start = inner
                .plugins
                .keys()
                .filter(|id| inner.disabled_ids.contains(*id) && !next.contains(*id))
                .filter_map(|id| inner.handles.get(id).map(|handle| handle.config.clone()))
                .collect::<Vec<_>>();
            inner.disabled_ids = next;
            // Codemux self can't be disabled.
            inner.disabled_ids.remove("codemux-self");
            for (id, handle) in inner.handles.iter() {
                if inner.disabled_ids.contains(id)
                    && matches!(
                        handle.status,
                        McpServerStatus::Running { .. } | McpServerStatus::Starting
                    )
                {
                    to_stop.push(id.clone());
                }
            }
        }

        for id in to_stop {
            let _ = self.stop_server(app, &id).await;
        }
        for config in to_start {
            self.ensure_started(app, config).await;
        }
        Ok(())
    }

    pub async fn disabled_plugin_ids(&self) -> Vec<String> {
        self.inner
            .lock()
            .await
            .disabled_ids
            .iter()
            .filter(|id| id.starts_with("plugin:"))
            .cloned()
            .collect()
    }

    /// Whether a server id is currently in the disabled set.
    pub async fn is_disabled(&self, id: &str) -> bool {
        let inner = self.inner.lock().await;
        inner.disabled_ids.contains(id)
    }

    fn server_lock(&self, id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.lifecycle.lock().unwrap();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(id).and_then(std::sync::Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(id.into(), Arc::downgrade(&lock));
        lock
    }

    /// Start the given config if not already running. Idempotent: a
    /// second call against an already-running id is a no-op. Emits
    /// `mcp-status-changed` on every transition.
    pub async fn ensure_started<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        config: McpServerConfig,
    ) -> McpServerRuntime {
        let (config, token) = if config.sources.contains(&McpConfigSource::Plugin) {
            let inner = self.inner.lock().await;
            match (inner.plugins.get(&config.id), inner.handles.get(&config.id)) {
                (Some(owner), Some(handle)) => (handle.config.clone(), Some(owner.token.clone())),
                _ => return McpServerRuntime::from_handle(&McpServerHandle::discovered(config)),
            }
        } else {
            (config, None)
        };
        self.ensure_started_owned(app, config, token.as_deref())
            .await
    }

    async fn ensure_started_owned<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        mut config: McpServerConfig,
        token: Option<&str>,
    ) -> McpServerRuntime {
        let id = config.id.clone();
        let server_lock = self.server_lock(&id);
        let lifecycle = server_lock.lock().await;

        // Codemux self is always-on regardless of disabled set; everyone
        // else respects the user's toggle.
        if id != "codemux-self" {
            let inner = self.inner.lock().await;
            if inner.disabled_ids.contains(&id) {
                let row = McpServerRuntime {
                    id: id.clone(),
                    name: config.name.clone(),
                    status: McpServerStatus::Stopped,
                    tools_count: 0,
                    error_message: None,
                    stderr_tail: None,
                    started_at_ms: None,
                };
                drop(inner);
                emit_status_with_bus(app, &self.status_tx, &row);
                return row;
            }
        }

        // A server from the project's own files waits for the user's
        // approval of this exact definition. Every start path (chat prime,
        // Settings, start by id, restart) arrives here.
        let fingerprint = super::project_trust::fingerprint(&config);
        let approval_guard = if fingerprint.is_some() {
            Some(self.project_approval_lock.lock().await)
        } else {
            None
        };
        if let Some(ref fingerprint) = fingerprint {
            if !super::project_trust::is_approved(app, fingerprint) {
                let mut inner = self.inner.lock().await;
                if let Some(existing) = inner.handles.get(&id).filter(|h| {
                    matches!(h.status, McpServerStatus::Running { .. } | McpServerStatus::Starting)
                }) {
                    return McpServerRuntime::from_handle(existing);
                }
                let summary = super::project_trust::summary(&config);
                let mut handle = McpServerHandle::discovered(config);
                handle.status = McpServerStatus::NeedsApproval { fingerprint: fingerprint.clone(), summary };
                let row = McpServerRuntime::from_handle(&handle);
                inner.handles.insert(id, handle);
                drop(inner);
                emit_status_with_bus(app, &self.status_tx, &row);
                return row;
            }
            #[cfg(test)]
            if let Ok((entered, release)) = APPROVAL_CHECKED_GATE.try_with(Clone::clone) {
                entered.notify_one();
                release.notified().await;
            }
        }

        // Reserve the generation under one lock; stop/unregister can revoke
        // it while the handshake is in flight without resurrecting a child.
        let generation = uuid::Uuid::new_v4().to_string();
        config
            .raw
            .as_object_mut()
            .map(|raw| raw.insert("runtimeGeneration".into(), serde_json::json!(generation)));
        if !config.raw.is_object() {
            config.raw = serde_json::json!({"runtimeGeneration": generation});
        }
        let starting_ms = now_ms();
        let start_cancelled;
        let _start_guard;
        {
            let mut inner = self.inner.lock().await;
            if let Some(existing) = inner.handles.get(&id) {
                if matches!(
                    existing.status,
                    McpServerStatus::Running { .. } | McpServerStatus::Starting
                ) {
                    return McpServerRuntime::from_handle(existing);
                }
            }
            if config.sources.contains(&McpConfigSource::Plugin)
                && !inner
                    .plugins
                    .get(&id)
                    .is_some_and(|r| token == Some(r.token.as_str()))
            {
                let h = McpServerHandle::discovered(config);
                return McpServerRuntime::from_handle(&h);
            }
            let mut placeholder = McpServerHandle::discovered(config.clone());
            placeholder.status = McpServerStatus::Starting;
            placeholder.started_at_ms = Some(starting_ms);
            let (tx, rx) = tokio::sync::watch::channel(false);
            start_cancelled = rx;
            inner.cancel_calls.insert(id.clone(), tx);
            let tracker = Arc::new(plugin::CallTracker::default());
            _start_guard = plugin::CallGuard::new(tracker.clone(), None);
            inner.calls_in_flight.insert(id.clone(), tracker);
            inner.handles.insert(id.clone(), placeholder);
        }
        drop(approval_guard);
        emit_status_with_bus(
            app,
            &self.status_tx,
            &McpServerRuntime {
                id: id.clone(),
                name: config.name.clone(),
                status: McpServerStatus::Starting,
                tools_count: 0,
                error_message: None,
                stderr_tail: None,
                started_at_ms: Some(starting_ms),
            },
        );

        drop(lifecycle);
        // A revocation or stop can complete after the reservation but before
        // process creation. Recheck under the approval and registry locks,
        // then hold them only through the actual spawn. The handshake can
        // take much longer and must not block revocation. We cannot reacquire
        // the server lock here: stop waits for the startup call guard to drop.
        #[cfg(test)]
        if fingerprint.is_some() {
            if let Ok((entered, release)) = BEFORE_PROJECT_ADMISSION_GATE.try_with(Clone::clone) {
                entered.notify_one();
                release.notified().await;
            }
        }
        let spawn_approval_guard = if fingerprint.is_some() {
            Some(self.project_approval_lock.lock().await)
        } else {
            None
        };
        let mut spawn_inner_guard = self.inner.lock().await;
        let current = spawn_inner_guard.handles.get(&id).is_some_and(|handle| {
            matches!(handle.status, McpServerStatus::Starting)
                && handle.config.raw.get("runtimeGeneration")
                    .and_then(serde_json::Value::as_str) == Some(&generation)
        });
        let still_approved = fingerprint.as_ref().is_none_or(|fingerprint| {
            super::project_trust::is_approved(app, fingerprint)
        });
        let cancelled_before_spawn = *start_cancelled.borrow() || start_cancelled.has_changed().is_err();
        if !current || cancelled_before_spawn || !still_approved {
            if current && !still_approved {
                let mut denied = McpServerHandle::discovered(config.clone());
                denied.status = McpServerStatus::NeedsApproval {
                    fingerprint: fingerprint.clone().expect("project approval fingerprint"),
                    summary: super::project_trust::summary(&config),
                };
                let row = McpServerRuntime::from_handle(&denied);
                spawn_inner_guard.cancel_calls.remove(&id);
                spawn_inner_guard.handles.insert(id, denied);
                drop(spawn_inner_guard);
                drop(spawn_approval_guard);
                emit_status_with_bus(app, &self.status_tx, &row);
                return row;
            }
            drop(spawn_inner_guard);
            drop(spawn_approval_guard);
            return self.plugin_status(&id).await.unwrap_or_else(|| {
                McpServerRuntime::from_handle(&McpServerHandle::discovered(config))
            });
        }
        let http_admission = fingerprint.as_ref().map(|_| {
            HttpRequestAdmission::new(self.project_approval_lock.clone(), start_cancelled.clone())
        });
        let mut handle = super::runtime::start_mcp_server_cancellable(
            config.clone(),
            Some(start_cancelled),
            (spawn_inner_guard, spawn_approval_guard),
            http_admission,
        ).await;
        let row;
        let host_tools = self.collision_host_tools();
        {
            let mut inner = self.inner.lock().await;
            let current = inner.handles.get(&id).is_some_and(|h| {
                matches!(h.status, McpServerStatus::Starting)
                    && h.config
                        .raw
                        .get("runtimeGeneration")
                        .and_then(serde_json::Value::as_str)
                        == Some(&generation)
            });
            if !current {
                drop(inner);
                if let Some(child) = handle.child.take() {
                    let _ = child.shutdown().await;
                }
                return self
                    .plugin_status(&id)
                    .await
                    .unwrap_or_else(|| McpServerRuntime::from_handle(&handle));
            }
            if let Some(error) = plugin::tool_collision(&inner, &id, &handle.tools, &host_tools) {
                handle.tools.clear();
                handle.status = McpServerStatus::Errored { message: error };
            }
            row = McpServerRuntime::from_handle(&handle);
            inner.handles.insert(id.clone(), handle);
        }
        emit_status_with_bus(app, &self.status_tx, &row);
        self.watch_tool_changes(app.cloned(), &id).await;
        row
    }

    async fn watch_tool_changes<R: Runtime>(&self, app: Option<AppHandle<R>>, id: &str) {
        let (config, child, remote) = {
            let inner = self.inner.lock().await;
            let Some(h) = inner.handles.get(id).filter(|h| h.status.is_running()) else {
                return;
            };
            (h.config.clone(), h.child.clone(), h.remote.clone())
        };
        // Subscribe before the reconciliation fetch: notifications during
        // initialize/list, before registry installation, cannot leave stale tools.
        let mut notifications = if let Some(child) = child.as_ref() {
            child.notifications()
        } else if let Some(remote) = remote.as_ref() {
            remote.notifications()
        } else {
            return;
        };
        let registry = self.clone();
        let server_id = id.to_owned();
        let task = tokio::spawn(async move {
            let mut stream_task = remote.as_ref().map(|r| {
                let r = r.clone();
                tokio::spawn(async move {
                    r.listen_notifications().await;
                })
                .abort_handle()
            });
            // An abort-on-drop guard also cancels HTTP GET on unregister/restart.
            struct StreamGuard(Option<tokio::task::AbortHandle>);
            impl Drop for StreamGuard {
                fn drop(&mut self) {
                    if let Some(t) = self.0.take() {
                        t.abort();
                    }
                }
            }
            let _stream_guard = StreamGuard(stream_task.take());
            let mut reconcile_now = child.is_some();
            loop {
                let mut stream_error = None;
                let mut new_catalog_change = true;
                // Stdio needs an immediate race-closing fetch. HTTP reconciles
                // only after a real GET stream opens or a session is renewed.
                if !reconcile_now {
                    loop {
                        match notifications.recv().await {
                            Ok(n) if n.method == "notifications/tools/list_changed" => break,
                            Ok(n)
                                if remote.is_some()
                                    && n.method == super::http_client::NOTIFICATION_STREAM_OPENED =>
                            {
                                new_catalog_change = false;
                                break;
                            }
                            Ok(n)
                                if remote.is_some()
                                    && n.method == super::http_client::NOTIFICATION_STREAM_ERROR =>
                            {
                                stream_error = Some(
                                    n.params["message"]
                                        .as_str()
                                        .unwrap_or("MCP notifications stopped; restart this server")
                                        .to_owned(),
                                );
                                break;
                            }
                            Ok(_) => {}
                            Err(broadcast::error::RecvError::Lagged(_)) => break,
                            Err(broadcast::error::RecvError::Closed) => return,
                        }
                    }
                }
                reconcile_now = false;
                let terminal = stream_error.is_some();
                let result = if let Some(error) = stream_error {
                    Err(error)
                } else if let Some(c) = child.as_ref() {
                    super::runtime::list_stdio_tools(c, &config)
                        .await
                        .map_err(|e| e.to_string())
                } else if let Some(r) = remote.as_ref() {
                    let mut refresh = r.begin_catalog_refresh(new_catalog_change);
                    let result = super::runtime::list_http_tools(r, &config).await;
                    if result.is_ok() {
                        refresh.complete();
                    }
                    result
                } else {
                    return;
                };
                let host_tools = registry.collision_host_tools();
                let row = {
                    let mut inner = registry.inner.lock().await;
                    let result = result.and_then(|tools| {
                        match plugin::tool_collision(&inner, &server_id, &tools, &host_tools) {
                            Some(error) => Err(error),
                            None => Ok(tools),
                        }
                    });
                    let Some(h) = inner.handles.get_mut(&server_id) else {
                        return;
                    };
                    let same = match (&h.child, &child, &h.remote, &remote) {
                        (Some(a), Some(b), _, _) => Arc::ptr_eq(a, b),
                        (_, _, Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        _ => false,
                    };
                    if !same
                        || matches!(
                            h.status,
                            McpServerStatus::Stopping
                                | McpServerStatus::Stopped
                                | McpServerStatus::Starting
                                | McpServerStatus::Discovered
                        )
                    {
                        return;
                    }
                    match result {
                        Ok(tools) => {
                            h.tools = tools;
                            h.status = McpServerStatus::Running {
                                tool_count: h.tools.len(),
                            };
                            h.stderr_tail = None;
                        }
                        Err(error) => {
                            h.tools.clear();
                            h.status = McpServerStatus::Errored {
                                message: if terminal {
                                    error
                                } else {
                                    format!("tools/list refresh failed: {error}")
                                },
                            };
                        }
                    }
                    McpServerRuntime::from_handle(h)
                };
                emit_status_with_bus(app.as_ref(), &registry.status_tx, &row);
                if terminal {
                    return;
                }
            }
        });
        let mut inner = self.inner.lock().await;
        if let Some(old) = inner.watchers.insert(id.into(), task.abort_handle()) {
            old.abort();
        }
    }

    /// Stop a server (graceful EOF then kill, 2s budget) and mark it
    /// Stopped. Idempotent: stopping an already-stopped server is fine.
    pub async fn stop_server<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        id: &str,
    ) -> Result<McpServerRuntime, String> {
        let server_lock = self.server_lock(id);
        let _lifecycle = server_lock.lock().await;
        self.stop_server_locked(app, id).await
    }

    async fn stop_server_locked<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        id: &str,
    ) -> Result<McpServerRuntime, String> {
        let (child, tracker, stopping) = {
            let mut inner = self.inner.lock().await;
            if let Some(task) = inner.watchers.remove(id) {
                task.abort();
            }
            inner.cancel_calls.remove(id);
            let tracker = inner.calls_in_flight.remove(id);
            let handle = inner
                .handles
                .get_mut(id)
                .ok_or_else(|| format!("no such server: {id}"))?;
            let child = handle.child.take();
            handle.remote.take();
            handle.status = McpServerStatus::Stopping;
            handle.tools.clear();
            (child, tracker, McpServerRuntime::from_handle(handle))
        };
        emit_status_with_bus(app, &self.status_tx, &stopping);
        let result = match child.as_ref() {
            Some(child) => child.shutdown().await.map_err(|e| e.to_string()),
            None => Ok(()),
        };
        if let Some(tracker) = tracker {
            tracker.wait_idle().await;
        }
        let row = {
            let mut inner = self.inner.lock().await;
            let handle = inner
                .handles
                .get_mut(id)
                .ok_or_else(|| format!("server vanished after stop: {id}"))?;
            match &result {
                Ok(()) => {
                    handle.status = McpServerStatus::Stopped;
                    handle.server_info = None;
                    handle.started_at_ms = None;
                }
                Err(message) => {
                    handle.status = McpServerStatus::Errored {
                        message: format!("Could not confirm MCP shutdown: {message}"),
                    };
                    handle.child = child;
                }
            }
            McpServerRuntime::from_handle(handle)
        };
        emit_status_with_bus(app, &self.status_tx, &row);
        result.map(|_| row)
    }

    /// Stop then re-start. Used by the manual "Restart" affordance for
    /// servers in `Errored` state.
    pub async fn restart_server<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        id: &str,
    ) -> Result<McpServerRuntime, String> {
        let config = {
            let inner = self.inner.lock().await;
            match inner.handles.get(id) {
                Some(h) => h.config.clone(),
                None => return Err(format!("no such server: {id}")),
            }
        };
        let _ = self.stop_server(app, id).await;
        Ok(self.ensure_started(app, config).await)
    }

    /// Approve a project-defined server the user reviewed in Settings, then
    /// start it. `fingerprint` is the one its `NeedsApproval` row showed, so
    /// a definition that changed after the user looked is not approved.
    pub async fn approve_project_server<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        id: &str,
        fingerprint: &str,
    ) -> Result<McpServerRuntime, String> {
        let config = {
            let inner = self.inner.lock().await;
            match inner.handles.get(id) {
                Some(handle)
                    if matches!(
                        &handle.status,
                        McpServerStatus::NeedsApproval { fingerprint: shown, .. } if shown == fingerprint
                    ) =>
                {
                    handle.config.clone()
                }
                _ => {
                    return Err(
                        "This server changed or no longer needs approval. Refresh and review it again."
                            .into(),
                    )
                }
            }
        };
        self.approve_definition(app, config, fingerprint).await
    }

    /// Approve `config`, a definition the user reviewed as `fingerprint`
    /// (in Settings or with `codemux mcp-approve` on the host), then start it.
    pub async fn approve_definition<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        config: McpServerConfig,
        fingerprint: &str,
    ) -> Result<McpServerRuntime, String> {
        if super::project_trust::fingerprint(&config).as_deref() != Some(fingerprint) {
            return Err("This server changed. Refresh and review it again.".into());
        }
        let _approval_guard = self.project_approval_lock.lock().await;
        super::project_trust::record_approval(app, fingerprint, &config)?;
        drop(_approval_guard);
        Ok(self.ensure_started(Some(app), config).await)
    }

    /// Forget the approval of `fingerprint` and stop every server running
    /// that definition; they wait for approval again. Returns whether an
    /// approval was recorded.
    pub async fn revoke_project_approval<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        fingerprint: &str,
    ) -> Result<bool, String> {
        let approval_guard = self.project_approval_lock.lock().await;
        // The transport takes this same lock before admitting each HTTP
        // request. A failed delete leaves both approval and starts intact.
        let revoked = super::project_trust::revoke_approval(app, fingerprint)?;
        let configs: Vec<McpServerConfig> = {
            let inner = self.inner.lock().await;
            let configs: Vec<_> = inner
                .handles
                .values()
                .filter(|handle| {
                    super::project_trust::fingerprint(&handle.config).as_deref() == Some(fingerprint)
                })
                .map(|handle| handle.config.clone())
                .collect();
            // Close every in-flight reservation before releasing approval
            // admission, including startup retries and notification requests.
            for config in &configs {
                if let Some(cancel) = inner.cancel_calls.get(&config.id) {
                    cancel.send_replace(true);
                }
            }
            configs
        };
        drop(approval_guard);
        for config in configs {
            let _ = self.stop_server(Some(app), &config.id).await;
            self.ensure_started(Some(app), config).await;
        }
        Ok(revoked)
    }

    /// Spawn every enabled (non-disabled) MCP server discovered for the
    /// given project root, plus Codemux's own hardcoded entry. Errors
    /// per server are surfaced as `Errored` rows rather than failing
    /// the whole prime. Servers from the project's own files that the
    /// user has not approved become `NeedsApproval` rows instead.
    ///
    /// Used by:
    ///   1. `agent_chat_start_session` — first chat session triggers spawn.
    ///   2. Settings → MCP Servers mount — so users can inspect status.
    pub async fn prime_for_chat<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        project_root: Option<&std::path::Path>,
    ) {
        // Serialize concurrent primes so a warm-up prime and the
        // start-session backstop prime can't double-spawn the same child
        // (see `prime_lock`). The second waiter finds every server already
        // `Starting`/`Running` and no-ops through `ensure_started`.
        let _prime_guard = self.prime_lock.lock().await;
        let configs = discover_configs(project_root);
        self.remember_native_scoped_sources(project_root, &configs).await;
        for cfg in configs {
            if cfg.disabled {
                continue;
            }
            let _ = self.ensure_started(app, cfg).await;
        }
    }

    async fn remember_native_scoped_sources(
        &self,
        project_root: Option<&std::path::Path>,
        configs: &[McpServerConfig],
    ) {
        if let Some(root) = project_root {
            let checkout = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            let native_sources = configs
                .iter()
                .flat_map(|config| {
                    config.sources.iter().copied().filter_map(|source| {
                        (super::is_project_source(source)
                            || source == McpConfigSource::ClaudeLocal)
                            .then(|| (config.id.clone(), source))
                    })
                })
                .collect();
            self.inner
                .lock()
                .await
                .native_scoped_sources
                .insert(checkout, native_sources);
        }
    }

    /// Snapshot of every server's runtime row. Used by the
    /// `get_mcp_runtime_status` Tauri command for the Settings UI.
    /// Includes `Discovered` rows for configs the registry has seen
    /// (via prior calls) but not yet spawned, and any error rows still
    /// retained from earlier attempts.
    pub async fn list_runtime(&self) -> Vec<McpServerRuntime> {
        let inner = self.inner.lock().await;
        let mut out: Vec<McpServerRuntime> = inner
            .handles
            .values()
            .map(McpServerRuntime::from_handle)
            .collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    /// Global running tools, without a count cap. Sessions use
    /// `list_tools_for_workspace` with their fixed project scope.
    pub async fn list_all_tools(&self) -> Vec<McpTool> {
        self.list_all_tools_excluding_sources(&[]).await
    }

    /// Running tools except servers already native to `source`. This lets a
    /// provider keep its own authenticated/native MCPs while Codemux injects
    /// only the tools it would otherwise be missing.
    pub async fn list_all_tools_excluding_source(&self, source: McpConfigSource) -> Vec<McpTool> {
        self.list_all_tools_excluding_sources(&[source]).await
    }

    /// Running tools except servers already native to any of `sources`.
    /// Providers with multiple configuration scopes (Claude user/local/
    /// project, for example) use this to avoid duplicate tool surfaces while
    /// preserving provider-owned authentication such as OAuth sessions.
    pub async fn list_all_tools_excluding_sources(
        &self,
        sources: &[McpConfigSource],
    ) -> Vec<McpTool> {
        self.list_tools_for_workspace(sources, None, None).await
    }

    pub async fn list_tools_for_workspace(
        &self,
        sources: &[McpConfigSource],
        workspace_id: Option<&str>,
        project: Option<&McpProjectScope>,
    ) -> Vec<McpTool> {
        let inner = self.inner.lock().await;
        let mut tools = Vec::new();
        for handle in inner.handles.values() {
            if !visible_in_project(&handle.config, project) {
                continue;
            }
            if inner
                .plugins
                .get(&handle.config.id)
                .is_some_and(|r| !r.scope.visible(workspace_id))
            {
                continue;
            }
            let native_here = project
                .and_then(|project| inner.native_scoped_sources.get(&project.checkout))
                .is_some_and(|native| {
                    sources.iter().any(|source| {
                        native.contains(&(handle.config.id.clone(), *source))
                    })
                });
            // The primary scoped handle is already restricted by
            // visible_in_project. A secondary scoped source on a global
            // handle is native only where that source was discovered.
            let provider_native = native_here || handle.config.sources.iter().any(|source| {
                sources.contains(source)
                    && ((!super::is_project_source(*source)
                        && *source != McpConfigSource::ClaudeLocal)
                        || handle.config.primary_source() == *source)
            });
            if handle.status.is_running() && !provider_native {
                let is_self = handle.config.sources.contains(&McpConfigSource::Codemux);
                tools.extend(
                    handle
                        .tools
                        .iter()
                        .filter(|tool| {
                            !is_self || !crate::agent_control::registry::relay_refuses(&tool.name)
                        })
                        .cloned(),
                );
            }
        }
        tools
    }

    #[cfg(test)]
    pub(crate) async fn insert_native_delegation_tools_for_test(&self) {
        let config = super::codemux_self::codemux_self_config();
        let mut handle = McpServerHandle::discovered(config);
        handle.tools = crate::mcp_server::remote_delegation_tools_for_test();
        handle.status = McpServerStatus::Running {
            tool_count: handle.tools.len(),
        };
        self.inner
            .lock()
            .await
            .handles
            .insert(handle.config.id.clone(), handle);
    }
    /// Stage a running server with a single tool, for tests in sibling
    /// modules (the gateway) that need a populated registry without
    /// spawning real MCP children.
    #[cfg(test)]
    pub(crate) async fn insert_running_server_for_test(
        &self,
        id: &str,
        sources: Vec<McpConfigSource>,
    ) {
        self.insert_running_server_for_test_at(id, sources, None).await;
    }

    #[cfg(test)]
    pub(crate) async fn insert_running_server_for_test_at(
        &self,
        id: &str,
        sources: Vec<McpConfigSource>,
        scope: Option<&McpProjectScope>,
    ) {
        let project_root = scope.map(|scope| {
            if sources.first() == Some(&McpConfigSource::ClaudeLocal) {
                scope.checkout.clone()
            } else {
                scope.approval_root.clone()
            }
        });
        let config = McpServerConfig {
            id: id.into(),
            name: id.into(),
            sources,
            command: "/bin/true".into(),
            args: Vec::new(),
            env: HashMap::new(),
            disabled: false,
            transport: super::McpTransport::Stdio,
            raw: serde_json::Value::Null,
            checkout_root: scope.map(|scope| scope.checkout.clone()),
            project_root,
        };
        let mut handle = McpServerHandle::discovered(config);
        handle.status = McpServerStatus::Running { tool_count: 1 };
        handle.tools.push(McpTool {
            name: "tool".into(),
            prefixed_name: format!("mcp__{id}__tool"),
            description: None,
            input_schema: serde_json::json!({}),
            server_id: id.into(),
        });
        self.inner.lock().await.handles.insert(id.into(), handle);
    }

    /// Every tool registered by `server_id`. Used by the Settings
    /// tool-list modal to show a server's full surface.
    pub async fn list_tools_for_server(&self, server_id: &str) -> Vec<McpTool> {
        let inner = self.inner.lock().await;
        inner
            .handles
            .get(server_id)
            .map(|h| h.tools.clone())
            .unwrap_or_default()
    }

    /// Look up the (server-id, raw-tool-name) pair from a prefixed
    /// name. Stage 3 uses this to dispatch `tools/call` to the right
    /// child. Returns `None` for unknown names.
    pub async fn resolve_tool(&self, prefixed_name: &str) -> Option<(String, String)> {
        let inner = self.inner.lock().await;
        for handle in inner.handles.values() {
            for tool in &handle.tools {
                if tool.prefixed_name == prefixed_name {
                    return Some((handle.config.id.clone(), tool.name.clone()));
                }
            }
        }
        None
    }

    /// Dispatch a tool call from the Claude SDK facade to the backing
    /// MCP child. Used by `spawn_incoming_requests_task` in the Claude
    /// adapter when the sidecar issues an `mcp-tool-call` RPC. Returns
    /// the JSON-RPC `result` object the child returned (an MCP
    /// `tools/call` response with `content: [...]` and optional
    /// `isError`). Errors come through as `Err(String)` so the caller
    /// can wrap them in a synthetic `isError: true` result for the SDK.
    ///
    /// `workspace_id` is the calling session's owning workspace. All
    /// provider sessions share ONE spawned `codemux mcp` child, so the
    /// child's env cannot identify the caller — the id travels per-call
    /// in the request's `_meta` instead (see [`tool_call_params`]) so
    /// workspace-scoped tools (browser routing, git cwd) bind to the
    /// session's own workspace rather than whichever one the app
    /// process happens to sit in. `None` keeps the child's env/cwd
    /// fallback behavior. This unscoped entry point cannot call a
    /// project-bound server.
    pub async fn dispatch_tool_call(
        &self,
        prefixed_name: &str,
        arguments: serde_json::Value,
        workspace_id: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        self.dispatch_tool_call_scoped(prefixed_name, arguments, workspace_id, None)
            .await
    }

    pub async fn dispatch_tool_call_scoped(
        &self,
        prefixed_name: &str,
        arguments: serde_json::Value,
        workspace_id: Option<&str>,
        project: Option<&McpProjectScope>,
    ) -> Result<serde_json::Value, String> {
        self.dispatch_tool_call_inner(prefixed_name, arguments, workspace_id, project, None).await
    }

    async fn dispatch_tool_call_inner(
        &self,
        prefixed_name: &str,
        arguments: serde_json::Value,
        workspace_id: Option<&str>,
        project: Option<&McpProjectScope>,
        owned_server: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let (raw_name, is_codemux_self, child, remote, mut cancelled, mut call_guard) = {
            let inner = self.inner.lock().await;
            let mut found = None;
            for handle in inner.handles.values() {
                if !visible_in_project(&handle.config, project) {
                    continue;
                }
                if owned_server.is_some_and(|id| id != handle.config.id) {
                    continue;
                }
                if owned_server.is_none()
                    && inner
                        .plugins
                        .get(&handle.config.id)
                        .is_some_and(|r| !r.scope.visible(workspace_id))
                {
                    continue;
                }
                for tool in &handle.tools {
                    if tool.prefixed_name == prefixed_name {
                        if inner
                            .cancel_calls
                            .get(&handle.config.id)
                            .is_some_and(|cancel| *cancel.borrow())
                        {
                            return Err("MCP server stopped during tools/call".into());
                        }
                        if !handle.status.is_running() {
                            return Err(format!(
                                "MCP server '{}' is not running",
                                handle.config.name
                            ));
                        }
                        let child = handle.child.clone();
                        let remote = handle.remote.clone();
                        if child.is_none() && remote.is_none() {
                            return Err(format!(
                                "MCP server '{}' has no live transport",
                                handle.config.name
                            ));
                        }
                        // Only the built-in server understands the
                        // `_meta` workspace tag; the Codemux source is
                        // its marker (see `codemux_self_config`).
                        let is_self = handle
                            .config
                            .sources
                            .contains(&super::McpConfigSource::Codemux);
                        let cancelled = inner
                            .cancel_calls
                            .get(&handle.config.id)
                            .map(|tx| tx.subscribe());
                        let activity = inner.plugins.get(&handle.config.id).and_then(|r| {
                            self.activity.get().map(|emit| {
                                let event = PluginMcpActivity {
                                    plugin_id: r.plugin_id.clone(),
                                    server_id: handle.config.id.clone(),
                                    phase: "start".into(),
                                    call_id: uuid::Uuid::new_v4().to_string(),
                                    tool_name: tool.name.clone(),
                                    prefixed_name: tool.prefixed_name.clone(),
                                    workspace_id: workspace_id.map(str::to_owned),
                                    is_error: None,
                                    duration_ms: None,
                                };
                                emit(event.clone());
                                plugin::ActivityGuard {
                                    event,
                                    started: std::time::Instant::now(),
                                    emit: emit.clone(),
                                }
                            })
                        });
                        let tracker = inner
                            .calls_in_flight
                            .get(&handle.config.id)
                            .cloned()
                            .unwrap_or_default();
                        let guard = plugin::CallGuard::new(tracker, activity);
                        found = Some((tool.name.clone(), is_self, child, remote, cancelled, guard));
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            match found {
                Some(p) => p,
                None => return Err(format!("unknown MCP tool: {prefixed_name}")),
            }
        };

        // This relay has provider identity only, never a local controller or
        // connector grant. In particular a persisted Codex dynamic tool can
        // outlive its catalog. Omission alone is not an authority boundary.
        if is_codemux_self && crate::agent_control::registry::relay_refuses(&raw_name) {
            return Err("provider_control_denied: native controller tools are not available through provider MCP relays".into());
        }
        let params = tool_call_params(&raw_name, arguments, is_codemux_self, workspace_id);
        let budget = super::runtime::tool_call_timeout(is_codemux_self, &raw_name);
        let call = super::runtime::relay_tool_call(child.as_deref(), remote.as_deref(), params, budget);
        let cancellation = async {
            match cancelled.as_mut() {
                Some(rx) => {
                    if *rx.borrow() {
                        return;
                    }
                    let _ = rx.changed().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        let result = tokio::select! { biased; _ = cancellation => Err("MCP server stopped during tools/call".into()), result = call => result };
        if let Some(guard) = call_guard.activity.as_mut() {
            guard.event.is_error = Some(
                result
                    .as_ref()
                    .map(|r| {
                        r.get("isError")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                    })
                    .unwrap_or(true),
            );
        }
        result
    }

    /// Tear every spawned child down. Called from the
    /// `tauri::RunEvent::Exit` handler. Best-effort: errors are
    /// swallowed because the process is exiting anyway.
    pub async fn shutdown_all(&self) {
        let children: Vec<Arc<crate::json_rpc_child::JsonRpcChild>> = {
            let mut inner = self.inner.lock().await;
            let mut out = Vec::new();
            inner.cancel_calls.clear();
            for (_, task) in inner.watchers.drain() {
                task.abort();
            }
            for handle in inner.handles.values_mut() {
                if let Some(child) = handle.child.take() {
                    out.push(child);
                }
                handle.remote.take();
                handle.status = McpServerStatus::Stopped;
                handle.tools.clear();
            }
            out
        };

        // Kick off shutdowns in parallel — each has its own 2-second
        // budget so the worst case is ~2s regardless of count.
        let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        for c in children {
            tasks.push(tokio::spawn(async move {
                let _ = c.shutdown().await;
            }));
        }
        for t in tasks {
            let _ = t.await;
        }
    }
}

/// Build the `tools/call` params for a registry dispatch. The caller's
/// workspace id rides in `_meta` (MCP's per-request metadata channel)
/// under a namespaced key, and ONLY for the built-in codemux server —
/// third-party servers must keep receiving the exact `{name, arguments}`
/// shape they got before, so a stray `_meta` cannot confuse a strict
/// implementation.
fn tool_call_params(
    raw_name: &str,
    arguments: serde_json::Value,
    is_codemux_self: bool,
    workspace_id: Option<&str>,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "name": raw_name,
        "arguments": arguments,
    });
    if is_codemux_self {
        if let Some(id) = workspace_id {
            params["_meta"] = serde_json::json!({ super::WORKSPACE_META_KEY: id });
        }
    }
    params
}

/// Emit a status-changed event so the frontend updates without
/// polling AND fan out to in-process subscribers (the Claude adapter
/// listens here for dynamic-refresh pushes). `app` is optional so
/// registry tests don't need a real Tauri app; `status_tx` is taken
/// off the registry instance so the broadcast survives in tests too.
fn emit_status_with_bus<R: Runtime>(
    app: Option<&AppHandle<R>>,
    bus: &broadcast::Sender<McpServerRuntime>,
    row: &McpServerRuntime,
) {
    if let Some(app) = app {
        let _ = app.emit(MCP_STATUS_CHANGED_EVENT, row);
    }
    let _ = bus.send(row.clone());
}

/// Wall-clock now in epoch-ms. Lifted out of `runtime.rs::now_ms`
/// because that function is module-private; we keep a local copy here
/// rather than widen the visibility for one call.
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A server bound to one checkout must have a matching caller scope. A
/// missing scope fails closed for project and Claude-local definitions.
fn visible_in_project(config: &McpServerConfig, project: Option<&McpProjectScope>) -> bool {
    let Some(project) = project else {
        return !super::is_project_source(config.primary_source())
            && config.primary_source() != McpConfigSource::ClaudeLocal;
    };
    let approval_root = match config.primary_source() {
        source if super::is_project_source(source) => Some(&project.approval_root),
        McpConfigSource::ClaudeLocal => Some(&project.checkout),
        _ => return true,
    };
    config.project_root.as_ref() == approval_root
        && config.checkout_root.as_ref() == Some(&project.checkout)
}

/// Walk the configured MCP files for a project root and produce a flat
/// `Vec<McpServerConfig>` suitable for spawning. Identical definitions from
/// multiple providers are deduplicated before launch so Codemux owns one
/// child/connection and one stable tool namespace for all adapters.
/// Project-sourced entries carry the project root their approval is keyed
/// by (see [`super::project_trust::approval_root`]).
pub(crate) fn discover_configs(project_root: Option<&std::path::Path>) -> Vec<McpServerConfig> {
    discover_configs_with_home(project_root, dirs::home_dir().as_deref())
}

fn discover_configs_with_home(
    project_root: Option<&std::path::Path>,
    home: Option<&std::path::Path>,
) -> Vec<McpServerConfig> {
    let mut out: Vec<McpServerConfig> = vec![codemux_self_config()];

    let project_path: Option<PathBuf> = project_root.map(|p| p.to_path_buf());
    // Resolved only when the project defines a server: it can ask Git.
    let approval_root = std::cell::OnceCell::new();
    let scan = enumerate_mcp_paths_with_home(project_path.as_deref(), home);
    let own = PathBuf::from(crate::mcp_server::codemux_mcp_command());

    for (path, source) in scan.paths {
        let parsed = if is_claude_wrapped_path(&path) {
            parse_claude_wrapped_config(&path, project_path.as_deref())
        } else if source == McpConfigSource::CodexUser {
            parse_codex_config_file(&path)
        } else if matches!(
            source,
            McpConfigSource::OpenCodeUser | McpConfigSource::OpenCodeProject
        ) {
            parse_opencode_config_file(&path, source)
        } else {
            parse_mcp_config_file(&path, source)
        };
        match parsed {
            Ok(parsed) => {
                for mut srv in parsed {
                    // Same filter as the list command: drop provider-written
                    // Codemux entries, whatever their name, so we don't spawn
                    // the built-in twice.
                    if srv.is_codemux_entry(&own) {
                        continue;
                    }
                    if super::is_project_source(source) {
                        srv.project_root = approval_root
                            .get_or_init(|| project_path.as_deref().map(super::project_trust::approval_root))
                            .clone();
                        srv.checkout_root = project_path.as_deref().map(|path| {
                            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
                        });
                    }
                    out.push(srv);
                }
            }
            Err(e) => {
                eprintln!("[codemux::mcp::registry] {}: {}", path.display(), e);
            }
        }
    }
    out.sort_by(|a, b| {
        super::source_rank(a.primary_source())
            .cmp(&super::source_rank(b.primary_source()))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    super::dedupe_servers(out)
}

/// Shared fixture for tests that need to observe the exact `tools/call`
/// the registry sends to the built-in server: a mock HTTP MCP server
/// registered under the `Codemux` source (the marker `dispatch_tool_call`
/// keys the `_meta` workspace tag on), so the adapters' dispatch paths can
/// assert on the outbound wire shape without spawning a real child.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::mcp::{McpConfigSource, McpServerConfig, McpTransport};
    use mockito::Matcher;

    pub(crate) async fn plugin_fixture(scope: &str) -> McpRegistry {
        let registry = McpRegistry::new();
        let server=serde_json::from_value(serde_json::json!({
            "id":"fixture","name":"Fixture","toolPrefix":"fixture","scope":serde_json::from_str::<serde_json::Value>(scope).unwrap_or(serde_json::json!(scope)),
            "toolDescriptions":{"new_tool":{"replace":"Use structured tools first.","append":"Always use background delivery."}},
            "transport":"stdio","command":"python3","args":[format!("{}/tests/fixtures/plugin_mcp.py",env!("CARGO_MANIFEST_DIR"))]
        })).unwrap();
        registry
            .register_plugin_server::<tauri::Wry>(None, "test-plugin", "token".into(), server)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while registry
                .list_tools_for_server("plugin:test-plugin:fixture")
                .await
                .len()
                != 4
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        registry
    }

    /// One mock `codemux-remote` server exposing a single `echo` tool. The
    /// returned `Mock` is the `tools/call` expectation: it only matches a
    /// request whose params carry `_meta[WORKSPACE_META_KEY] == workspace_id`,
    /// so `assert_async` proves the tag reached the wire.
    pub(crate) async fn codemux_remote_server(
        workspace_id: &str,
    ) -> (mockito::ServerGuard, McpRegistry, mockito::Mock) {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(
                serde_json::json!({ "method": "initialize" }),
            ))
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "serverInfo": {"name": "codemux", "version": "1"}
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;
        server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(
                serde_json::json!({ "method": "notifications/initialized" }),
            ))
            .with_status(202)
            .create_async()
            .await;
        server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(
                serde_json::json!({ "method": "tools/list" }),
            ))
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "result": {"tools": [{
                        "name": "echo",
                        "description": "Echo",
                        "inputSchema": {"type": "object"}
                    }]}
                })
                .to_string(),
            )
            .create_async()
            .await;
        let call = server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "method": "tools/call",
                "params": {
                    "name": "echo",
                    "_meta": { crate::mcp::WORKSPACE_META_KEY: workspace_id }
                }
            })))
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "result": {"content": [{"type": "text", "text": "routed"}]}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let registry = McpRegistry::new();
        let config = McpServerConfig {
            id: "codemux-remote".into(),
            name: "codemux-remote".into(),
            sources: vec![McpConfigSource::Codemux],
            command: format!("{}/mcp", server.url()),
            args: Vec::new(),
            env: HashMap::new(),
            disabled: false,
            transport: McpTransport::Http,
            raw: serde_json::json!({}),
            project_root: None,
            checkout_root: None,
        };
        let row = registry
            .ensure_started(None::<&tauri::AppHandle<tauri::Wry>>, config)
            .await;
        assert!(
            matches!(row.status, McpServerStatus::Running { tool_count: 1 }),
            "{row:?}"
        );
        (server, registry, call)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{McpConfigSource, McpServerConfig, McpTransport, WORKSPACE_META_KEY};
    use std::collections::HashMap;

    #[tokio::test]
    async fn discovery_excludes_a_global_copy_only_where_claude_local_is_native() {
        let home = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::fs::write(
            home.path().join(".cursor/mcp.json"),
            serde_json::json!({"mcpServers":{"shared":{"command":"fixture-server"}}}).to_string(),
        ).unwrap();
        let mut projects = serde_json::Map::new();
        projects.insert(
            first.path().display().to_string(),
            serde_json::json!({"mcpServers":{"shared":{"command":"fixture-server"}}}),
        );
        std::fs::write(home.path().join(".claude.json"), serde_json::json!({"projects":projects}).to_string()).unwrap();

        let first_configs = discover_configs_with_home(Some(first.path()), Some(home.path()));
        let second_configs = discover_configs_with_home(Some(second.path()), Some(home.path()));
        let global = first_configs.iter().find(|config| config.name == "shared").unwrap().clone();
        assert_eq!(global.primary_source(), McpConfigSource::CursorUser);
        assert!(global.sources.contains(&McpConfigSource::ClaudeLocal));
        assert_eq!(second_configs.iter().find(|config| config.name == "shared").unwrap().sources,
            vec![McpConfigSource::CursorUser]);

        let registry = McpRegistry::new();
        registry.remember_native_scoped_sources(Some(first.path()), &first_configs).await;
        registry.remember_native_scoped_sources(Some(second.path()), &second_configs).await;
        // The global handle may have started from the other checkout first,
        // before this checkout's Claude-local source was discovered.
        let running_global = second_configs.iter().find(|config| config.name == "shared").unwrap().clone();
        let mut handle = McpServerHandle::discovered(running_global);
        handle.status = McpServerStatus::Running { tool_count: 1 };
        handle.tools.push(McpTool {
            name: "tool".into(), prefixed_name: "mcp__shared__tool".into(),
            description: None, input_schema: serde_json::json!({}), server_id: global.id.clone(),
        });
        registry.inner.lock().await.handles.insert(global.id, handle);
        let native = &[McpConfigSource::ClaudeLocal, McpConfigSource::ClaudeUser, McpConfigSource::ClaudeProject];
        let first_scope = McpProjectScope::new(first.path());
        let second_scope = McpProjectScope::new(second.path());
        assert!(registry.list_tools_for_workspace(native, None, Some(&first_scope)).await.is_empty());
        assert_eq!(registry.list_tools_for_workspace(native, None, Some(&second_scope)).await.len(), 1);

        std::fs::write(home.path().join(".claude.json"), "{}").unwrap();
        let refreshed = discover_configs_with_home(Some(first.path()), Some(home.path()));
        registry.remember_native_scoped_sources(Some(first.path()), &refreshed).await;
        assert_eq!(registry.list_tools_for_workspace(native, None, Some(&first_scope)).await.len(), 1);
    }

    fn make_config(id: &str, name: &str, command: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            name: name.into(),
            sources: vec![McpConfigSource::CodemuxUser],
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            disabled: false,
            transport: McpTransport::Stdio,
            raw: serde_json::Value::Null,
            project_root: None,
            checkout_root: None,
        }
    }

    struct EchoHost;

    impl HostTools for EchoHost {
        fn tools_for(&self, caller: &HostCaller) -> Vec<McpTool> {
            vec![McpTool {
                name: "echo".into(),
                prefixed_name: "mcp__codemux__echo".into(),
                description: Some(caller.thread_id.clone()),
                input_schema: serde_json::json!({}),
                server_id: "codemux-host".into(),
            }]
        }

        fn call(
            &self,
            caller: &HostCaller,
            prefixed_name: &str,
            _arguments: &serde_json::Value,
        ) -> Option<serde_json::Value> {
            (prefixed_name == "mcp__codemux__echo").then(
                || serde_json::json!({ "content": [{"type": "text", "text": caller.thread_id}] }),
            )
        }
    }

    #[tokio::test]
    async fn native_callers_host_delegation_uses_native_admission_after_async_validation() {
        let reg=McpRegistry::new();
        reg.set_host_tools(Arc::new(EchoHost));
        let owner=crate::delegation::authority::NativeAuthority::new("native-host-admission");
        let generation=owner.begin(0);assert!(owner.bind(generation,"turn"));
        let caller=HostCaller{control:None,thread_id:"native-host-admission".into(),provider:crate::agent_provider::ProviderKind::Claude,workspace_id:None,permission_mode:Some("default".into())};
        let actor=crate::delegation::app::NativeActor{thread_id:caller.thread_id.clone(),provider:caller.provider,workspace_id:None,session_id:"session".into(),permission_mode:caller.permission_mode.clone(),permit:owner.capture_for_turn("turn")};
        let entered=Arc::new(tokio::sync::Notify::new());
        let release=Arc::new(tokio::sync::Semaphore::new(0));
        let admitted=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (start,gate,count)=(entered.clone(),release.clone(),admitted.clone());
        reg.set_native_delegation_handler(Arc::new(move|actor,name,_,call_id|{
            let(start,gate,count)=(start.clone(),gate.clone(),count.clone());
            Box::pin(async move{
                assert_eq!(name,"delegate_task");
                assert_eq!(call_id.as_deref(),Some("provider-tool-call"));
                let identity=actor.permit.identity()?;
                start.notify_one();gate.acquire().await.unwrap().forget();
                identity.admit(&actor.thread_id,||{count.fetch_add(1,std::sync::atomic::Ordering::SeqCst);Ok(serde_json::json!({"task_id":"admitted"}))})
            })
        }));
        let args=serde_json::json!({"permission_mode":"plan"});
        let call=reg.host_call_native(&caller,"mcp__codemux__delegate_task",&args,Some("provider-tool-call"),Some("turn"),Some(&actor));
        tokio::pin!(call);
        tokio::select!{result=&mut call=>panic!("admitted before validation: {result:?}"),_=entered.notified()=>{}}
        owner.invalidate();release.add_permits(1);
        let result=call.await.unwrap();
        assert_eq!(result["isError"],true,"{result}");
        assert!(result.to_string().contains("stale_caller"));
        assert_eq!(admitted.load(std::sync::atomic::Ordering::SeqCst),0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_and_remote_delegation_keep_distinct_real_prefixes_when_local_is_disabled() {
        use crate::agent_provider::ProviderKind;
        use crate::commands::delegation::{
            DelegationHost, DelegationState, DELEGATION_SETTING_KEY,
        };
        use tauri::Manager;
        let app = tauri::test::mock_app();
        app.manage(crate::database::DatabaseStore::new_in_memory());
        app.manage(DelegationState::default());
        let reg = McpRegistry::new();
        reg.set_host_tools(Arc::new(DelegationHost::with_install_check(
            app.handle().clone(),
            |_| true,
        )));
        reg.insert_native_delegation_tools_for_test().await;
        let caller = HostCaller {
            control: None,
            thread_id: "catalog-routing-parent".into(),
            provider: ProviderKind::Codex,
            workspace_id: Some("workspace".into()),
            permission_mode: Some("danger-full-access".into()),
        };
        let owner = crate::delegation::authority::NativeAuthority::new(&caller.thread_id);
        let generation = owner.begin(0);
        assert!(owner.bind(generation, "turn"));
        let actor = crate::delegation::app::NativeActor {
            thread_id: caller.thread_id.clone(),
            provider: caller.provider,
            workspace_id: caller.workspace_id.clone(),
            session_id: "session".into(),
            permission_mode: caller.permission_mode.clone(),
            permit: owner.capture_for_turn("turn"),
        };
        let input = serde_json::json!({ "target_id": "remote-checkout", "prompt": "Inspect the checkout", "client_request_id": "request" });
        let expected = input.clone();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        reg.set_native_delegation_handler(Arc::new(move |actor, command, arguments, _call_id| {
            let expected = expected.clone();
            let count = count.clone();
            Box::pin(async move {
                let _admission = actor.permit.admit()?;
                assert_eq!(
                    command, "delegate_remote_task",
                    "remote task selection stays distinct from local task admission"
                );
                assert_eq!(
                    arguments, expected,
                    "remote launch must bypass the local argument parser"
                );
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(serde_json::json!({ "id": "remote-receipt" }))
            })
        }));
        for enabled in [true, false] {
            app.state::<crate::database::DatabaseStore>()
                .set_setting(
                    DELEGATION_SETTING_KEY,
                    if enabled { "true" } else { "false" },
                )
                .unwrap();
            let local = reg.host_tools(&caller);
            // `suggest_tasks` is not behind the delegation setting.
            assert_eq!(
                local
                    .iter()
                    .filter(|tool| tool.name == "delegate_task")
                    .count(),
                usize::from(enabled)
            );
            assert!(local.iter().any(|tool| tool.name == "suggest_tasks"));
            let remote = reg
                .list_all_tools_excluding_source(McpConfigSource::CodexUser)
                .await;
            let local_names: HashSet<_> = local.iter().map(|tool| tool.prefixed_name.as_str()).collect();
            let remote_names: HashSet<_> = remote.iter().map(|tool| tool.prefixed_name.as_str()).collect();
            // Task control is canonical for local and SSH tasks; the native
            // adapters prefer these host schemas over the SSH-only versions.
            let shared: HashSet<_> = local_names.intersection(&remote_names).copied().collect();
            let expected_shared = if enabled {
                HashSet::from(["mcp__codemux__task_status", "mcp__codemux__task_cancel"])
            } else {
                HashSet::new()
            };
            assert_eq!(shared, expected_shared);
            for name in shared {
                let canonical = local.iter().find(|tool| tool.prefixed_name == name).unwrap();
                assert!(canonical.input_schema["properties"].get("task_id").is_some());
            }
            let launch = remote
                .iter()
                .find(|tool| tool.name == crate::mcp_server::REMOTE_DELEGATE_TOOL)
                .unwrap();
            assert_eq!(launch.prefixed_name, "mcp__codemux__delegate_remote_task");
            assert!(launch.input_schema["properties"].get("target_id").is_some());
            if enabled {
                assert_eq!(local[0].prefixed_name, "mcp__codemux__delegate_task");
                assert!(local[0].input_schema["properties"]
                    .get("provider")
                    .is_some());
            }
            assert!(
                reg.host_call(&caller, &launch.prefixed_name, &input)
                    .is_none(),
                "local host must never claim remote launch, even when disabled"
            );
            let result = reg
                .dispatch_native_tool_call(&launch.prefixed_name, input.clone(), actor.clone())
                .await
                .unwrap();
            assert!(result.to_string().contains("remote-receipt"));
            let local_result = reg
                .host_call(
                    &caller,
                    "mcp__codemux__delegate_task",
                    &serde_json::json!({}),
                )
                .unwrap();
            assert_eq!(
                local_result["isError"], true,
                "local calls must keep their own validation/refusal path"
            );
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn host_tools_are_served_per_caller_and_never_reach_shared_surfaces() {
        let reg = McpRegistry::new();
        let caller = HostCaller {
            control: None,
            thread_id: "thread-1".into(),
            provider: crate::agent_provider::ProviderKind::Claude,
            workspace_id: None,
            permission_mode: None,
        };
        assert!(reg.host_tools(&caller).is_empty(), "nothing before install");
        assert!(reg
            .host_call(&caller, "mcp__codemux__echo", &serde_json::json!({}))
            .is_none());

        // Installing through a clone reaches every other clone.
        reg.clone_handle().set_host_tools(Arc::new(EchoHost));
        let tools = reg.host_tools(&caller);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].description.as_deref(), Some("thread-1"));
        assert!(reg
            .host_call(&caller, "mcp__codemux__echo", &serde_json::json!({}))
            .is_some());
        assert!(reg
            .host_call(&caller, "mcp__codemux__other", &serde_json::json!({}))
            .is_none());

        // The gateway / `codemux mcp` surfaces stay unaware of host tools.
        assert!(reg.list_all_tools().await.is_empty());
        assert!(reg
            .dispatch_tool_call("mcp__codemux__echo", serde_json::json!({}), None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn review_s1_provider_relay_rejects_native_controller_tools_before_transport() {
        use mockito::Matcher;
        let (mut server, registry, _) = test_support::codemux_remote_server("worker-ws").await;
        {
            let mut inner = registry.inner.lock().await;
            let handle = inner.handles.get_mut("codemux-remote").unwrap();
            handle.tools[0].name = "thread_launch".into();
            handle.tools[0].prefixed_name = "mcp__codemux__thread_launch".into();
        }
        let call = server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(
                serde_json::json!({"method":"tools/call","params":{"name":"thread_launch"}}),
            ))
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({"jsonrpc":"2.0","id":3,"result":{"content":[]}}).to_string(),
            )
            .expect(0)
            .create_async()
            .await;
        let result = registry.dispatch_tool_call("mcp__codemux__thread_launch",
            serde_json::json!({"workspace_id":"worker-ws","provider":"codex","permission_mode":"danger-full-access","client_request_id":"nested"}),
            Some("worker-ws")).await;
        assert!(
            result.is_err(),
            "provider relay must not confer trusted native-controller authority: {result:?}"
        );
        assert!(result.unwrap_err().starts_with("provider_control_denied:"));
        call.assert_async().await;
        assert!(
            !registry
                .list_all_tools()
                .await
                .iter()
                .any(|t| t.name == "thread_launch"),
            "provider snapshot must not advertise an unusable trusted control route"
        );
        assert!(!registry
            .list_all_tools_excluding_source(McpConfigSource::CodexUser)
            .await
            .iter()
            .any(|t| t.name == "thread_launch"));
        // Controller discovery itself and Settings retain the native catalog.
        assert!(crate::agent_control::registry::lookup("thread_launch").is_some());
        assert!(registry
            .list_tools_for_server("codemux-remote")
            .await
            .iter()
            .any(|t| t.name == "thread_launch"));
    }

    #[tokio::test]
    async fn ensure_started_marks_errored_on_bad_command() {
        let reg = McpRegistry::new();
        let cfg = make_config("bad", "bad", "/no/such/binary/at/all");
        let row = reg
            .ensure_started(None::<&tauri::AppHandle<tauri::Wry>>, cfg)
            .await;
        assert!(matches!(row.status, McpServerStatus::Errored { .. }));
        assert!(row.error_message.is_some());
    }

    #[tokio::test]
    async fn disabled_ids_short_circuit_ensure_started() {
        let reg = McpRegistry::new();
        reg.set_disabled_ids(
            None::<&tauri::AppHandle<tauri::Wry>>,
            vec!["disabled-srv".into()],
        )
        .await
        .unwrap();
        let cfg = make_config("disabled-srv", "disabled", "/no/such/binary");
        let row = reg
            .ensure_started(None::<&tauri::AppHandle<tauri::Wry>>, cfg)
            .await;
        assert!(matches!(row.status, McpServerStatus::Stopped));
        // No handle should have been inserted because we short-circuited.
        let listed = reg.list_runtime().await;
        assert!(listed.iter().all(|r| r.id != "disabled-srv"));
    }

    #[tokio::test]
    async fn codemux_self_cannot_be_disabled() {
        let reg = McpRegistry::new();
        reg.set_disabled_ids(
            None::<&tauri::AppHandle<tauri::Wry>>,
            vec!["codemux-self".into()],
        )
        .await
        .unwrap();
        // The set should ignore the codemux-self id.
        assert!(!reg.is_disabled("codemux-self").await);
    }

    #[tokio::test]
    async fn stop_server_sets_status_stopped() {
        let reg = McpRegistry::new();
        // Inject an Errored handle directly so we don't have to pay the
        // full spawn cost in this unit test.
        {
            let mut inner = reg.inner.lock().await;
            let mut h = McpServerHandle::discovered(make_config("x", "x", "/bin/true"));
            h.status = McpServerStatus::Errored {
                message: "boom".into(),
            };
            inner.handles.insert("x".into(), h);
        }
        let row = reg
            .stop_server(None::<&tauri::AppHandle<tauri::Wry>>, "x")
            .await
            .unwrap();
        assert!(matches!(row.status, McpServerStatus::Stopped));
    }

    #[tokio::test]
    async fn stop_server_unknown_id_errors() {
        let reg = McpRegistry::new();
        let err = reg
            .stop_server(None::<&tauri::AppHandle<tauri::Wry>>, "nope")
            .await
            .unwrap_err();
        assert!(err.contains("no such server"));
    }

    #[tokio::test]
    async fn list_runtime_is_sorted_by_name() {
        let reg = McpRegistry::new();
        {
            let mut inner = reg.inner.lock().await;
            let mut h1 = McpServerHandle::discovered(make_config("z", "zeta", "/bin/true"));
            h1.status = McpServerStatus::Discovered;
            let mut h2 = McpServerHandle::discovered(make_config("a", "alpha", "/bin/true"));
            h2.status = McpServerStatus::Discovered;
            inner.handles.insert("z".into(), h1);
            inner.handles.insert("a".into(), h2);
        }
        let rows = reg.list_runtime().await;
        assert_eq!(rows[0].name, "alpha");
        assert_eq!(rows[1].name, "zeta");
    }

    #[tokio::test]
    async fn shutdown_all_resets_handles() {
        let reg = McpRegistry::new();
        {
            let mut inner = reg.inner.lock().await;
            let mut h = McpServerHandle::discovered(make_config("x", "x", "/bin/true"));
            h.status = McpServerStatus::Running { tool_count: 3 };
            // No real child — shutdown_all gracefully handles None.
            inner.handles.insert("x".into(), h);
        }
        reg.shutdown_all().await;
        let rows = reg.list_runtime().await;
        for r in rows {
            assert!(matches!(r.status, McpServerStatus::Stopped));
        }
    }

    #[tokio::test]
    async fn resolve_tool_returns_server_and_raw_name() {
        let reg = McpRegistry::new();
        {
            let mut inner = reg.inner.lock().await;
            let mut h = McpServerHandle::discovered(make_config("github", "github", "/bin/true"));
            h.status = McpServerStatus::Running { tool_count: 1 };
            h.tools.push(McpTool {
                name: "create_issue".into(),
                prefixed_name: "mcp__github__create_issue".into(),
                description: None,
                input_schema: serde_json::json!({}),
                server_id: "github".into(),
            });
            inner.handles.insert("github".into(), h);
        }
        let resolved = reg.resolve_tool("mcp__github__create_issue").await;
        assert_eq!(resolved, Some(("github".into(), "create_issue".into())));
        assert!(reg.resolve_tool("mcp__nope__x").await.is_none());
    }

    #[tokio::test]
    async fn provider_native_sources_can_be_excluded_from_injected_tools() {
        let reg = McpRegistry::new();
        {
            let mut inner = reg.inner.lock().await;
            for (id, source) in [
                ("codex-native", McpConfigSource::CodexUser),
                ("claude-only", McpConfigSource::ClaudeUser),
            ] {
                let mut config = make_config(id, id, "/bin/true");
                config.sources = vec![source];
                let mut handle = McpServerHandle::discovered(config);
                handle.status = McpServerStatus::Running { tool_count: 1 };
                handle.tools.push(McpTool {
                    name: "tool".into(),
                    prefixed_name: format!("mcp__{id}__tool"),
                    description: None,
                    input_schema: serde_json::json!({}),
                    server_id: id.into(),
                });
                inner.handles.insert(id.into(), handle);
            }
        }
        let tools = reg
            .list_all_tools_excluding_source(McpConfigSource::CodexUser)
            .await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].server_id, "claude-only");
        let tools = reg
            .list_all_tools_excluding_sources(&[
                McpConfigSource::CodexUser,
                McpConfigSource::ClaudeUser,
            ])
            .await;
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn project_tools_are_bound_to_the_callers_checkout() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_scope = McpProjectScope::new(first.path());
        let second_scope = McpProjectScope::new(second.path());
        let registry = McpRegistry::new();
        registry
            .insert_running_server_for_test("global", vec![McpConfigSource::CodemuxUser])
            .await;
        registry
            .insert_running_server_for_test_at(
                "first",
                vec![McpConfigSource::OpenCodeProject],
                Some(&first_scope),
            )
            .await;
        registry
            .insert_running_server_for_test_at(
                "second-local",
                vec![McpConfigSource::ClaudeLocal],
                Some(&second_scope),
            )
            .await;

        let names = |tools: Vec<McpTool>| {
            let mut names: Vec<String> = tools.into_iter().map(|tool| tool.prefixed_name).collect();
            names.sort();
            names
        };
        assert_eq!(
            names(registry.list_tools_for_workspace(&[], Some("same-workspace"), Some(&first_scope)).await),
            ["mcp__first__tool", "mcp__global__tool"],
        );
        assert_eq!(
            names(registry.list_tools_for_workspace(&[], Some("same-workspace"), Some(&second_scope)).await),
            ["mcp__global__tool", "mcp__second-local__tool"],
        );
        assert_eq!(
            names(registry.list_tools_for_workspace(&[], Some("same-workspace"), None).await),
            ["mcp__global__tool"],
        );
        assert!(registry.dispatch_tool_call_scoped("mcp__first__tool", serde_json::json!({}), None, Some(&first_scope)).await.unwrap_err().contains("no live transport"));
        assert!(registry.dispatch_tool_call_scoped("mcp__first__tool", serde_json::json!({}), None, Some(&second_scope)).await.unwrap_err().contains("unknown MCP tool"));
        assert!(registry.dispatch_tool_call("mcp__second-local__tool", serde_json::json!({}), None).await.unwrap_err().contains("unknown MCP tool"));
    }

    #[tokio::test]
    async fn worktrees_with_shared_approval_do_not_share_running_tools() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_scope = McpProjectScope::new(first.path());
        let second_scope = McpProjectScope {
            checkout: std::fs::canonicalize(second.path()).unwrap(),
            approval_root: first_scope.approval_root.clone(),
        };
        let registry = McpRegistry::new();
        for (id, scope) in [("first", &first_scope), ("second", &second_scope)] {
            registry
                .insert_running_server_for_test_at(
                    id,
                    vec![McpConfigSource::OpenCodeProject],
                    Some(scope),
                )
                .await;
        }
        let first_tools = registry.list_tools_for_workspace(&[], None, Some(&first_scope)).await;
        let second_tools = registry.list_tools_for_workspace(&[], None, Some(&second_scope)).await;
        assert_eq!(first_tools.len(), 1);
        assert_eq!(first_tools[0].server_id, "first");
        assert_eq!(second_tools.len(), 1);
        assert_eq!(second_tools[0].server_id, "second");
        assert!(registry
            .dispatch_tool_call_scoped("mcp__first__tool", serde_json::json!({}), None, Some(&second_scope))
            .await
            .unwrap_err()
            .contains("unknown MCP tool"));
    }

    #[tokio::test]
    async fn equal_tool_names_dispatch_to_the_callers_project() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_scope = McpProjectScope::new(first.path());
        let second_scope = McpProjectScope::new(second.path());
        let registry = McpRegistry::new();
        for (id, scope) in [("first", &first_scope), ("second", &second_scope)] {
            registry
                .insert_running_server_for_test_at(
                    id,
                    vec![McpConfigSource::OpenCodeProject],
                    Some(scope),
                )
                .await;
        }
        {
            let mut inner = registry.inner.lock().await;
            for id in ["first", "second"] {
                inner.handles.get_mut(id).unwrap().tools[0].prefixed_name = "mcp__shared__tool".into();
            }
        }
        for (scope, expected) in [(&first_scope, "first"), (&second_scope, "second")] {
            let error = registry
                .dispatch_tool_call_scoped("mcp__shared__tool", serde_json::json!({}), None, Some(scope))
                .await
                .unwrap_err();
            assert!(error.contains(&format!("MCP server '{expected}' has no live transport")), "{error}");
        }
    }

    /// Regression guard: a large built-in tool surface must never
    /// crowd out user MCP tools. An earlier hard cap silently dropped
    /// every user tool once Codemux's own tools outgrew the budget,
    /// which made shared servers unreachable from providers that rely
    /// on injection.
    #[tokio::test]
    async fn large_builtin_surface_never_drops_user_tools() {
        let reg = McpRegistry::new();
        {
            let mut inner = reg.inner.lock().await;
            let mut config = make_config("codemux-self", "codemux", "/bin/true");
            config.sources = vec![McpConfigSource::Codemux];
            let mut builtin = McpServerHandle::discovered(config);
            builtin.status = McpServerStatus::Running { tool_count: 100 };
            for i in 0..100 {
                builtin.tools.push(McpTool {
                    name: format!("t{i}"),
                    prefixed_name: format!("mcp__codemux__t{i}"),
                    description: None,
                    input_schema: serde_json::json!({}),
                    server_id: "codemux-self".into(),
                });
            }
            inner.handles.insert("codemux-self".into(), builtin);

            let mut config = make_config("user-server", "user-server", "/bin/true");
            config.sources = vec![McpConfigSource::ClaudeUser];
            let mut user = McpServerHandle::discovered(config);
            user.status = McpServerStatus::Running { tool_count: 4 };
            for i in 0..4 {
                user.tools.push(McpTool {
                    name: format!("u{i}"),
                    prefixed_name: format!("mcp__user-server__u{i}"),
                    description: None,
                    input_schema: serde_json::json!({}),
                    server_id: "user-server".into(),
                });
            }
            inner.handles.insert("user-server".into(), user);
        }

        // The Codex injection path: excludes Codex-native servers only.
        let tools = reg
            .list_all_tools_excluding_source(McpConfigSource::CodexUser)
            .await;
        assert_eq!(tools.len(), 104, "no tool may be dropped");
        let user_tools = tools
            .iter()
            .filter(|t| t.server_id == "user-server")
            .count();
        assert_eq!(user_tools, 4, "all user MCP tools must be registered");
    }

    #[test]
    fn tool_call_params_tags_codemux_self_with_workspace_meta() {
        let params = tool_call_params(
            "browser_navigate",
            serde_json::json!({"url": "http://localhost:1420"}),
            true,
            Some("ws-42"),
        );
        assert_eq!(params["name"], "browser_navigate");
        assert_eq!(params["arguments"]["url"], "http://localhost:1420");
        assert_eq!(params["_meta"][WORKSPACE_META_KEY], "ws-42");
    }

    #[test]
    fn tool_call_params_omits_meta_without_workspace() {
        let params = tool_call_params("browser_navigate", serde_json::json!({}), true, None);
        assert!(params.get("_meta").is_none());
    }

    #[test]
    fn tool_call_params_never_tags_third_party_servers() {
        // A third-party server must see the exact pre-existing shape even
        // when the caller supplied a workspace id.
        let params = tool_call_params("create_issue", serde_json::json!({}), false, Some("ws-42"));
        assert!(params.get("_meta").is_none());
        assert_eq!(
            params,
            serde_json::json!({ "name": "create_issue", "arguments": {} })
        );
    }

    #[tokio::test]
    async fn dispatch_tool_call_sends_workspace_meta_to_builtin_server() {
        // The wire check for the whole chain: a dispatch with a workspace id
        // against the built-in server must produce an outbound `tools/call`
        // whose params carry `_meta[WORKSPACE_META_KEY]` — the mock only
        // answers a request that has it.
        let (_server, reg, call) = super::test_support::codemux_remote_server("ws-42").await;
        let result = reg
            .dispatch_tool_call(
                "mcp__codemux-remote__echo",
                serde_json::json!({"text": "hi"}),
                Some("ws-42"),
            )
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "routed");
        call.assert_async().await;
    }

    #[tokio::test]
    async fn dispatch_tool_call_unknown_tool_errors() {
        let reg = McpRegistry::new();
        let err = reg
            .dispatch_tool_call("mcp__nope__x", serde_json::json!({}), None)
            .await
            .unwrap_err();
        assert!(err.contains("unknown MCP tool"));
    }

    #[tokio::test]
    async fn dispatch_tool_call_server_not_running_errors() {
        let reg = McpRegistry::new();
        {
            let mut inner = reg.inner.lock().await;
            let mut h = McpServerHandle::discovered(make_config("github", "github", "/bin/true"));
            // Errored — has tools metadata but no live child.
            h.status = McpServerStatus::Errored {
                message: "x".into(),
            };
            h.tools.push(McpTool {
                name: "create_issue".into(),
                prefixed_name: "mcp__github__create_issue".into(),
                description: None,
                input_schema: serde_json::json!({}),
                server_id: "github".into(),
            });
            inner.handles.insert("github".into(), h);
        }
        let err = reg
            .dispatch_tool_call("mcp__github__create_issue", serde_json::json!({}), None)
            .await
            .unwrap_err();
        assert!(err.contains("not running"));
    }

    #[tokio::test]
    async fn remote_http_server_lists_and_dispatches_tools() {
        use mockito::Matcher;

        let mut server = mockito::Server::new_async().await;
        let no_events = server
            .mock("GET", "/mcp")
            .with_status(405)
            .expect(1)
            .create_async()
            .await;
        let initialize = server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "method": "initialize"
            })))
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "serverInfo": {"name": "test", "version": "1"}
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let initialized = server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "method": "notifications/initialized"
            })))
            .with_status(202)
            .create_async()
            .await;
        let list = server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "method": "tools/list"
            })))
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "result": {"tools": [{
                        "name": "echo",
                        "description": "Echo",
                        "inputSchema": {"type": "object"}
                    }]}
                })
                .to_string(),
            )
            .create_async()
            .await;
        let call = server
            .mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "method": "tools/call",
                "params": {"name": "echo", "arguments": {"text": "hi"}}
            })))
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "result": {"content": [{"type": "text", "text": "hi"}]}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let reg = McpRegistry::new();
        let config = McpServerConfig {
            id: "remote".into(),
            name: "remote".into(),
            sources: vec![McpConfigSource::CodexUser],
            command: format!("{}/mcp", server.url()),
            args: Vec::new(),
            env: HashMap::new(),
            disabled: false,
            transport: McpTransport::Http,
            raw: serde_json::json!({}),
            project_root: None,
            checkout_root: None,
        };
        let row = reg
            .ensure_started(None::<&tauri::AppHandle<tauri::Wry>>, config)
            .await;
        assert!(matches!(
            row.status,
            McpServerStatus::Running { tool_count: 1 }
        ));
        let result = reg
            .dispatch_tool_call("mcp__remote__echo", serde_json::json!({"text": "hi"}), None)
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "hi");
        // Cover the old 100/200 ms retry windows as well as the initial list.
        tokio::time::sleep(std::time::Duration::from_millis(450)).await;
        reg.shutdown_all().await;
        initialize.assert_async().await;
        initialized.assert_async().await;
        list.assert_async().await;
        call.assert_async().await;
        no_events.assert_async().await;
    }

    /// Developer smoke test against the user's real cross-provider config.
    /// Kept ignored for CI because it requires the local Omarchy container.
    #[tokio::test]
    #[ignore]
    async fn live_omarchy_server_lists_and_executes_search() {
        let config = discover_configs(None)
            .into_iter()
            .find(|config| config.name == "omarchy-kb")
            .expect("omarchy-kb should be discovered from a provider config");
        let reg = McpRegistry::new();
        let row = reg
            .ensure_started(None::<&tauri::AppHandle<tauri::Wry>>, config)
            .await;
        assert!(
            matches!(row.status, McpServerStatus::Running { .. }),
            "{row:?}"
        );
        let tool = reg
            .list_all_tools()
            .await
            .into_iter()
            .find(|tool| tool.name == "search_documentation")
            .expect("search_documentation tool should be listed");
        let result = reg
            .dispatch_tool_call(
                &tool.prefixed_name,
                serde_json::json!({"query": "default browser"}),
                None,
            )
            .await
            .expect("real MCP tool call should succeed");
        assert!(result
            .get("content")
            .and_then(serde_json::Value::as_array)
            .is_some());
        reg.shutdown_all().await;
    }

    #[tokio::test]
    async fn set_disabled_stops_running_server() {
        let reg = McpRegistry::new();
        {
            let mut inner = reg.inner.lock().await;
            let mut h = McpServerHandle::discovered(make_config("x", "x", "/bin/true"));
            h.status = McpServerStatus::Running { tool_count: 0 };
            inner.handles.insert("x".into(), h);
        }
        reg.set_disabled_ids(None::<&tauri::AppHandle<tauri::Wry>>, vec!["x".into()])
            .await
            .unwrap();
        let rows = reg.list_runtime().await;
        let row = rows.iter().find(|r| r.id == "x").unwrap();
        assert!(matches!(row.status, McpServerStatus::Stopped));
    }

    /// A user's `codemux mcp` entry under another name (`codex mcp add cmux
    /// -- codemux mcp`) is never started for chats: its tools would reach
    /// every chat outside the permission ceiling Codemux's own row keeps.
    #[test]
    fn chats_never_start_a_codemux_mcp_entry_under_another_name() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers": {
                "cmux": {"command": "codemux", "args": ["mcp", "--client", "codex"]},
                "demo": {"command": "echo"}
            }}"#,
        )
        .unwrap();
        let configs = discover_configs(Some(project.path()));
        let names: Vec<&str> = configs.iter().map(|config| config.name.as_str()).collect();
        assert!(!names.contains(&"cmux"), "{names:?}");
        assert!(names.contains(&"demo"), "{names:?}");
        assert_eq!(configs.iter().filter(|config| config.sources.contains(&McpConfigSource::Codemux)).count(), 1, "{names:?}");
    }
}

#[cfg(test)]
mod native_visibility_tests {
    use super::*;
    #[test]
    fn native_listing_and_legacy_dispatch_use_the_same_policy() {
        for (provider, full_mode, low_mode) in [(crate::agent_provider::ProviderKind::Claude,"bypassPermissions","plan"),(crate::agent_provider::ProviderKind::Codex,"danger-full-access","read-only")] {
            let mut caller=HostCaller {control:None,thread_id:"caller".into(),provider,workspace_id:None,permission_mode:None};
            for (mode,full) in [(full_mode,true),(low_mode,false)] {
                caller.permission_mode=Some(mode.into());
                for name in ["automation_create","workspace_create","worktree_create","git_commit","git_stage","git_push","terminal_write","pane_close","preset_apply","issue_link_workspace","browser_navigate","html_render"] {
                    let tool=McpTool{name:name.into(),prefixed_name:format!("mcp__codemux__{name}"),description:None,input_schema:serde_json::json!({}),server_id:"codemux-self".into()};
                    assert_eq!(native_tool_visible(&tool,&caller),full,"{name} {mode}");
                    assert_eq!(native_relay_allowed(name,full),full);
                }
                for name in ["conversation_read","workspace_info","browser_snapshot","git_status","terminal_read"] {assert!(native_relay_allowed(name,full));}
                let plugin=McpTool{name:"write".into(),prefixed_name:"mcp__plugin__write".into(),description:None,input_schema:serde_json::json!({}),server_id:"plugin".into()};
                assert!(native_tool_visible(&plugin,&caller));
            }
        }
    }
}

#[cfg(test)]
mod project_trust_tests {
    use super::*;
    use tauri::Manager;

    fn app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        app.manage(crate::database::DatabaseStore::new_in_memory());
        app
    }

    fn write_project(root: &std::path::Path, servers: serde_json::Value) {
        std::fs::write(
            root.join(".mcp.json"),
            serde_json::json!({ "mcpServers": servers }).to_string(),
        )
        .unwrap();
    }

    #[cfg(unix)]
    fn touch_server(marker: &std::path::Path) -> serde_json::Value {
        serde_json::json!({
            "command": "sh",
            "args": ["-c", format!("touch '{}'", marker.display())],
            "disabled": false,
        })
    }

    fn discovered(root: &std::path::Path, name: &str) -> McpServerConfig {
        discover_configs(Some(root))
            .into_iter()
            .find(|config| config.name == name)
            .unwrap()
    }

    fn shown_fingerprint(row: &McpServerRuntime) -> String {
        match &row.status {
            McpServerStatus::NeedsApproval { fingerprint, .. } => fingerprint.clone(),
            other => panic!("expected needsApproval, got {other:?}"),
        }
    }

    async fn appears(path: &std::path::Path) -> bool {
        for _ in 0..40 {
            if path.exists() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    /// Negative checks: the gate decides before spawning, so a short wait
    /// is enough to see a command that should not have run.
    async fn stays_absent(path: &std::path::Path) -> bool {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        !path.exists()
    }

    /// A repository's `.mcp.json` starts nothing until the user approves that
    /// definition, and asks again once the definition changes.
    #[cfg(unix)] // The fixture is a POSIX `sh` command.
    #[tokio::test]
    async fn project_server_waits_for_approval_and_asks_again_after_a_change() {
        let (project, scratch) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (first, second) = (scratch.path().join("first"), scratch.path().join("second"));
        write_project(project.path(), serde_json::json!({ "lint": touch_server(&first) }));
        let app = app();
        let handle = app.handle();
        let reg = McpRegistry::new();

        let config = discovered(project.path(), "lint");
        assert_eq!(
            config.project_root.as_deref(),
            Some(std::fs::canonicalize(project.path()).unwrap().as_path())
        );
        let row = reg.ensure_started(Some(handle), config.clone()).await;
        let fingerprint = shown_fingerprint(&row);
        // Restart reuses the stored config and still spawns nothing.
        let restarted = reg.restart_server(Some(handle), &config.id).await.unwrap();
        assert_eq!(shown_fingerprint(&restarted), fingerprint);
        assert!(stays_absent(&first).await, "the project command ran before approval");

        assert!(reg
            .approve_project_server(handle, &config.id, "stale")
            .await
            .is_err());
        reg.approve_project_server(handle, &config.id, &fingerprint)
            .await
            .unwrap();
        assert!(appears(&first).await, "the approved command did not start");

        write_project(project.path(), serde_json::json!({ "lint": touch_server(&second) }));
        let changed = discovered(project.path(), "lint");
        assert_eq!(changed.id, config.id);
        let row = reg.ensure_started(Some(handle), changed).await;
        assert_ne!(shown_fingerprint(&row), fingerprint);
        assert!(stays_absent(&second).await, "a changed definition ran without approval");
        reg.shutdown_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn opencode_dot_directory_server_waits_for_approval() {
        let (project, scratch) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let marker = scratch.path().join("started");
        std::fs::create_dir(project.path().join(".opencode")).unwrap();
        std::fs::write(
            project.path().join(".opencode/opencode.json"),
            serde_json::json!({"mcp":{"lint":{"type":"local",
                "command":["sh","-c",format!("touch '{}'",marker.display())]}}}).to_string(),
        ).unwrap();
        let app = app();
        let reg = McpRegistry::new();
        let config = discovered(project.path(), "lint");
        assert_eq!(config.sources, vec![McpConfigSource::OpenCodeProject]);
        let row = reg.ensure_started(Some(app.handle()), config.clone()).await;
        let fingerprint = shown_fingerprint(&row);
        assert!(stays_absent(&marker).await);
        reg.approve_project_server(app.handle(), &config.id, &fingerprint)
            .await
            .unwrap();
        assert!(appears(&marker).await);
        reg.shutdown_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_project_stdio_generation_rejects_calls_before_shutdown() {
        let project = tempfile::tempdir().unwrap();
        let fixture = format!("{}/tests/fixtures/plugin_mcp.py", env!("CARGO_MANIFEST_DIR"));
        write_project(project.path(), serde_json::json!({"lint": {
            "command": "python3", "args": [fixture]
        }}));
        let app = app();
        let reg = McpRegistry::new();
        let config = discovered(project.path(), "lint");
        let row = reg.ensure_started(Some(app.handle()), config.clone()).await;
        let row = reg.approve_project_server(app.handle(), &config.id, &shown_fingerprint(&row))
            .await.unwrap();
        assert!(row.status.is_running());
        let tool = reg.list_tools_for_server(&config.id).await.into_iter()
            .find(|tool| tool.name == "echo").unwrap();

        // Revocation marks every matching generation before it stops them
        // one by one. This server is still running but must admit no call.
        {
            let inner = reg.inner.lock().await;
            inner.cancel_calls.get(&config.id).unwrap().send_replace(true);
        }
        let result = reg.dispatch_tool_call_scoped(
            &tool.prefixed_name,
            serde_json::json!({}),
            None,
            Some(&McpProjectScope::new(project.path())),
        ).await;
        assert!(result.unwrap_err().contains("stopped during tools/call"));
        reg.shutdown_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn revocation_waits_for_a_first_start_reservation() {
        let project = tempfile::tempdir().unwrap();
        let fixture = format!("{}/tests/fixtures/plugin_mcp.py", env!("CARGO_MANIFEST_DIR"));
        write_project(project.path(), serde_json::json!({"lint": {
            "command": "python3", "args": [fixture]
        }}));
        let app = app();
        let reg = McpRegistry::new();
        let config = discovered(project.path(), "lint");
        let fingerprint = super::super::project_trust::fingerprint(&config).unwrap();
        super::super::project_trust::record_approval(app.handle(), &fingerprint, &config).unwrap();

        let (entered, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        let start_reg = reg.clone();
        let start_app = app.handle().clone();
        let start_config = config.clone();
        let start_entered = entered.clone();
        let start_release = release.clone();
        let start = tokio::spawn(async move {
            APPROVAL_CHECKED_GATE.scope((start_entered, start_release), async move {
                start_reg.ensure_started(Some(&start_app), start_config).await
            }).await
        });
        // Wait at the exact gap that used to exist between the approval read
        // and insertion of the Starting handle.
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        let revoke_reg = reg.clone();
        let revoke_app = app.handle().clone();
        let revoke_fingerprint = fingerprint.clone();
        let mut revoke = tokio::spawn(async move {
            revoke_reg.revoke_project_approval(&revoke_app, &revoke_fingerprint).await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut revoke).await.is_err(),
            "revocation returned before the first start was reserved"
        );
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(10), start)
            .await.unwrap().unwrap();
        assert!(tokio::time::timeout(std::time::Duration::from_secs(10), revoke)
            .await.unwrap().unwrap().unwrap());
        let row = reg.list_runtime().await.into_iter()
            .find(|row| row.id == config.id).unwrap();
        shown_fingerprint(&row);
        reg.shutdown_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn revoking_after_reservation_but_before_spawn_runs_no_project_command() {
        let (project, scratch) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let marker = scratch.path().join("started");
        write_project(project.path(), serde_json::json!({"lint": touch_server(&marker)}));
        let app = app();
        let reg = McpRegistry::new();
        let config = discovered(project.path(), "lint");
        let fingerprint = super::super::project_trust::fingerprint(&config).unwrap();
        super::super::project_trust::record_approval(app.handle(), &fingerprint, &config).unwrap();

        let (entered, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        let start_reg = reg.clone();
        let start_app = app.handle().clone();
        let start_config = config.clone();
        let start_entered = entered.clone();
        let start_release = release.clone();
        let start = tokio::spawn(async move {
            BEFORE_PROJECT_ADMISSION_GATE.scope((start_entered, start_release), async move {
                start_reg.ensure_started(Some(&start_app), start_config).await
            }).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await.unwrap();

        let revoke_reg = reg.clone();
        let revoke_app = app.handle().clone();
        let revoke_fingerprint = fingerprint.clone();
        let revoke = tokio::spawn(async move {
            revoke_reg.revoke_project_approval(&revoke_app, &revoke_fingerprint).await
        });
        for _ in 0..40 {
            if !super::super::project_trust::is_approved(Some(app.handle()), &fingerprint) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(!super::super::project_trust::is_approved(Some(app.handle()), &fingerprint));
        assert!(!marker.exists(), "the command has not run at the pre-spawn gate");
        release.notify_one();

        tokio::time::timeout(std::time::Duration::from_secs(10), start)
            .await.unwrap().unwrap();
        assert!(tokio::time::timeout(std::time::Duration::from_secs(10), revoke)
            .await.unwrap().unwrap().unwrap());
        assert!(stays_absent(&marker).await, "revoked project command still spawned");
        let row = reg.list_runtime().await.into_iter()
            .find(|row| row.id == config.id).unwrap();
        shown_fingerprint(&row);
        reg.shutdown_all().await;
    }

    /// Approval is per project: the same file in another checkout asks again.
    #[cfg(unix)] // The fixture is a POSIX `sh` command.
    #[tokio::test]
    async fn project_approval_does_not_carry_to_another_project() {
        let (a, b, scratch) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let marker = scratch.path().join("ran");
        for root in [a.path(), b.path()] {
            write_project(root, serde_json::json!({ "lint": touch_server(&marker) }));
        }
        let app = app();
        let reg = McpRegistry::new();
        let in_a = discovered(a.path(), "lint");
        let row = reg.ensure_started(Some(app.handle()), in_a.clone()).await;
        reg.approve_project_server(app.handle(), &in_a.id, &shown_fingerprint(&row))
            .await
            .unwrap();
        let row = reg
            .ensure_started(Some(app.handle()), discovered(b.path(), "lint"))
            .await;
        shown_fingerprint(&row);
        reg.shutdown_all().await;
    }

    /// An HTTP entry is not contacted, so its `{env:}` headers are never
    /// expanded or sent, until it is approved.
    #[tokio::test]
    async fn project_http_server_receives_nothing_before_approval() {
        let mut server = mockito::Server::new_async().await;
        let untouched = server
            .mock("POST", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let project = tempfile::tempdir().unwrap();
        write_project(
            project.path(),
            serde_json::json!({ "remote": {
                "type": "http",
                "url": format!("{}/mcp", server.url()),
                "headers": { "X-Secret": "{env:HOME}" },
                "bearer_token_env_var": "HOME",
            }}),
        );
        let app = app();
        let reg = McpRegistry::new();
        let row = reg
            .ensure_started(Some(app.handle()), discovered(project.path(), "remote"))
            .await;
        match &row.status {
            McpServerStatus::NeedsApproval { summary, .. } => {
                assert!(summary.ends_with(" · sends HOME"), "{summary}");
            }
            other => panic!("expected needsApproval, got {other:?}"),
        }
        untouched.assert_async().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_approval_delete_keeps_reserved_start_retryable() {
        let (project, scratch) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let marker = scratch.path().join("started");
        write_project(project.path(), serde_json::json!({"lint": touch_server(&marker)}));
        let app = app();
        let reg = McpRegistry::new();
        let config = discovered(project.path(), "lint");
        let fingerprint = super::super::project_trust::fingerprint(&config).unwrap();
        super::super::project_trust::record_approval(app.handle(), &fingerprint, &config).unwrap();

        let (entered, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        let start_reg = reg.clone();
        let start_app = app.handle().clone();
        let start_config = config.clone();
        let start_entered = entered.clone();
        let start_release = release.clone();
        let start = tokio::spawn(async move {
            BEFORE_PROJECT_ADMISSION_GATE.scope((start_entered, start_release), async move {
                start_reg.ensure_started(Some(&start_app), start_config).await
            }).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await.unwrap();
        app.state::<crate::database::DatabaseStore>().test_query_only(true);
        assert!(reg.revoke_project_approval(app.handle(), &fingerprint).await.is_err());
        assert!(super::super::project_trust::is_approved(Some(app.handle()), &fingerprint));
        release.notify_one();
        let row = tokio::time::timeout(std::time::Duration::from_secs(10), start)
            .await.unwrap().unwrap();
        assert!(appears(&marker).await, "failed revocation cancelled approved startup");
        assert!(!matches!(row.status, McpServerStatus::Starting));
        app.state::<crate::database::DatabaseStore>().test_query_only(false);
        reg.shutdown_all().await;
    }

    #[tokio::test]
    async fn revoking_before_http_admission_sends_no_project_headers() {
        let mut server = mockito::Server::new_async().await;
        let untouched = server.mock("POST", mockito::Matcher::Any)
            .expect(0).create_async().await;
        let project = tempfile::tempdir().unwrap();
        write_project(project.path(), serde_json::json!({"remote": {
            "type": "http",
            "url": format!("{}/mcp", server.url()),
            "headers": {"X-Secret": "{env:HOME}"},
        }}));
        let app = app();
        let reg = McpRegistry::new();
        let config = discovered(project.path(), "remote");
        let fingerprint = super::super::project_trust::fingerprint(&config).unwrap();
        super::super::project_trust::record_approval(app.handle(), &fingerprint, &config).unwrap();

        let (entered, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        let start_reg = reg.clone();
        let start_app = app.handle().clone();
        let start_config = config.clone();
        let start_entered = entered.clone();
        let start_release = release.clone();
        let start = tokio::spawn(async move {
            BEFORE_PROJECT_ADMISSION_GATE.scope((start_entered, start_release), async move {
                start_reg.ensure_started(Some(&start_app), start_config).await
            }).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await.unwrap();
        let revoke_reg = reg.clone();
        let revoke_app = app.handle().clone();
        let revoke_fingerprint = fingerprint.clone();
        let revoke = tokio::spawn(async move {
            revoke_reg.revoke_project_approval(&revoke_app, &revoke_fingerprint).await
        });
        for _ in 0..40 {
            if !super::super::project_trust::is_approved(Some(app.handle()), &fingerprint) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(!super::super::project_trust::is_approved(Some(app.handle()), &fingerprint));
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(10), start)
            .await.unwrap().unwrap();
        assert!(tokio::time::timeout(std::time::Duration::from_secs(10), revoke)
            .await.unwrap().unwrap().unwrap());
        untouched.assert_async().await;
        let row = reg.list_runtime().await.into_iter()
            .find(|row| row.id == config.id).unwrap();
        shown_fingerprint(&row);
        reg.shutdown_all().await;
    }

    /// Servers from the user's own files start as before, with or without
    /// an app to read approvals from.
    #[cfg(unix)] // The fixture is a POSIX `sh` command.
    #[tokio::test]
    async fn project_trust_leaves_user_scope_servers_alone() {
        let scratch = tempfile::tempdir().unwrap();
        let marker = scratch.path().join("ran");
        let config = McpServerConfig {
            id: "user".into(),
            name: "user".into(),
            sources: vec![McpConfigSource::CodemuxUser, McpConfigSource::ClaudeProject],
            command: "sh".into(),
            args: vec!["-c".into(), format!("touch '{}'", marker.display())],
            env: HashMap::new(),
            disabled: false,
            transport: super::super::McpTransport::Stdio,
            raw: serde_json::json!({}),
            project_root: None,
            checkout_root: None,
        };
        let reg = McpRegistry::new();
        let row = reg
            .ensure_started(None::<&tauri::AppHandle<tauri::Wry>>, config)
            .await;
        assert!(!matches!(row.status, McpServerStatus::NeedsApproval { .. }), "{row:?}");
        assert!(appears(&marker).await);
        reg.shutdown_all().await;
    }

    #[cfg(unix)]
    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.test", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A worktree workspace primes its own folder, and Settings lists that
    /// same folder, so its row can be approved there. The approval belongs
    /// to the project, so the next worktree of it starts the same server,
    /// while a folder whose `.git` file merely points at the repository
    /// does not.
    #[cfg(unix)] // The fixture is a POSIX `sh` command.
    #[tokio::test]
    async fn worktrees_share_their_project_approvals() {
        let (repo, worktrees, scratch) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        git(repo.path(), &["init", "-q", "-b", "main"]);
        git(repo.path(), &["commit", "-q", "--allow-empty", "-m", "first"]);
        let (first, second) = (worktrees.path().join("first"), worktrees.path().join("second"));
        for (path, branch) in [(&first, "first"), (&second, "second")] {
            git(repo.path(), &["worktree", "add", "-q", "-b", branch, path.to_str().unwrap()]);
        }
        let fake = worktrees.path().join("fake");
        std::fs::create_dir(&fake).unwrap();
        std::fs::write(
            fake.join(".git"),
            format!("gitdir: {}/.git/worktrees/fake\n", repo.path().display()),
        )
        .unwrap();
        let marker = scratch.path().join("ran");
        for root in [&first, &second, &fake] {
            write_project(root, serde_json::json!({ "lint": touch_server(&marker) }));
        }
        let app = app();
        app.manage(McpRegistry::new());
        let handle = app.handle();
        let reg = app.state::<McpRegistry>();

        // Chat priming of the first worktree's folder.
        let in_first = discovered(&first, "lint");
        let canonical_repo = std::fs::canonicalize(repo.path()).unwrap();
        assert_eq!(in_first.project_root.as_deref(), Some(canonical_repo.as_path()));
        let row = reg.ensure_started(Some(handle), in_first.clone()).await;
        let fingerprint = shown_fingerprint(&row);
        // Settings' start toggle for the same folder still starts nothing.
        let row = crate::commands::start_mcp_server_cmd(
            handle.clone(),
            app.state(),
            in_first.id.clone(),
            Some(first.display().to_string()),
        )
        .await
        .unwrap();
        assert_eq!(shown_fingerprint(&row), fingerprint);
        assert!(stays_absent(&marker).await, "the project command ran before approval");

        crate::commands::approve_mcp_server(handle.clone(), app.state(), in_first.id.clone(), fingerprint)
            .await
            .unwrap();
        assert!(appears(&marker).await, "the approved command did not start");
        reg.shutdown_all().await;
        std::fs::remove_file(&marker).unwrap();

        let in_second = discovered(&second, "lint");
        assert_ne!(in_second.id, in_first.id);
        let row = reg.ensure_started(Some(handle), in_second).await;
        assert!(!matches!(row.status, McpServerStatus::NeedsApproval { .. }), "{row:?}");
        assert!(appears(&marker).await, "the second worktree asked again");
        reg.shutdown_all().await;
        std::fs::remove_file(&marker).unwrap();

        let in_fake = discovered(&fake, "lint");
        assert_eq!(
            in_fake.project_root.as_deref(),
            Some(std::fs::canonicalize(&fake).unwrap().as_path())
        );
        shown_fingerprint(&reg.ensure_started(Some(handle), in_fake).await);
        assert!(stays_absent(&marker).await, "an unregistered folder borrowed the approval");
    }

    /// An approved server keeps running when its file changes, and the
    /// changed definition neither replaces it nor starts beside it.
    #[tokio::test]
    async fn a_changed_definition_does_not_start_beside_the_approved_one() {
        let (project, scratch) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let fixture = format!("{}/tests/fixtures/plugin_mcp.py", env!("CARGO_MANIFEST_DIR"));
        let server = |pid_file: &std::path::Path| {
            serde_json::json!({
                "command": "python3",
                "args": [fixture, "--pid-file", pid_file.display().to_string()],
            })
        };
        let (old_pid, new_pid) = (scratch.path().join("old"), scratch.path().join("new"));
        write_project(project.path(), serde_json::json!({ "fixture": server(&old_pid) }));
        let app = app();
        let reg = McpRegistry::new();
        let config = discovered(project.path(), "fixture");
        let row = reg.ensure_started(Some(app.handle()), config.clone()).await;
        let row = reg
            .approve_project_server(app.handle(), &config.id, &shown_fingerprint(&row))
            .await
            .unwrap();
        assert!(matches!(row.status, McpServerStatus::Running { .. }), "{row:?}");

        write_project(project.path(), serde_json::json!({ "fixture": server(&new_pid) }));
        let row = reg
            .ensure_started(Some(app.handle()), discovered(project.path(), "fixture"))
            .await;
        assert!(matches!(row.status, McpServerStatus::Running { .. }), "{row:?}");
        assert!(old_pid.exists());
        assert!(stays_absent(&new_pid).await, "the changed definition started");
        reg.shutdown_all().await;
    }

    /// A repository copying a server the user turned off (in Settings, or
    /// with `"disabled": true` in their own file) does not turn it back on,
    /// even when the project file ranks ahead of the user's.
    #[tokio::test]
    async fn a_project_copy_does_not_restart_a_server_the_user_turned_off() {
        let scratch = tempfile::tempdir().unwrap();
        let pid_file = scratch.path().join("pid");
        let fixture = format!("{}/tests/fixtures/plugin_mcp.py", env!("CARGO_MANIFEST_DIR"));
        let entry = |id: &str, source: McpConfigSource, disabled: bool| McpServerConfig {
            id: id.into(),
            name: "docs".into(),
            sources: vec![source],
            command: "python3".into(),
            args: vec![fixture.clone(), "--pid-file".into(), pid_file.display().to_string()],
            env: HashMap::new(),
            disabled,
            transport: super::super::McpTransport::Stdio,
            raw: serde_json::json!({}),
            project_root: (source == McpConfigSource::CodemuxProject).then(|| scratch.path().into()),
            checkout_root: (source == McpConfigSource::CodemuxProject).then(|| scratch.path().into()),
        };
        let app = app();
        let reg = McpRegistry::new();

        let merged = super::super::dedupe_servers(vec![
            entry("repo", McpConfigSource::CodemuxProject, false),
            entry("user", McpConfigSource::ClaudeUser, true),
        ]);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].disabled, "the project copy re-enabled the user's server");

        let merged = super::super::dedupe_servers(vec![
            entry("repo", McpConfigSource::CodemuxProject, false),
            entry("user", McpConfigSource::ClaudeUser, false),
        ]);
        assert_eq!(merged[0].id, "user");
        reg.set_disabled_ids(Some(app.handle()), vec!["user".into()]).await.unwrap();
        let row = reg.ensure_started(Some(app.handle()), merged[0].clone()).await;
        assert!(matches!(row.status, McpServerStatus::Stopped), "{row:?}");
        assert!(stays_absent(&pid_file).await, "the user's off switch was bypassed");
        reg.shutdown_all().await;
    }
}
