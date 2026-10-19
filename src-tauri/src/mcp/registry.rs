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
pub use plugin::{PluginMcpActivity, PluginMcpServer, plugin_server_id, plugin_bridge_tools};

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::{broadcast, Mutex, OnceCell};

use super::codemux_self::codemux_self_config;
use super::parser::{
    parse_claude_wrapped_config, parse_codex_config_file, parse_mcp_config_file,
    parse_opencode_config_file,
};
use super::paths::{enumerate_mcp_paths, is_claude_wrapped_path};
use super::runtime::{
    start_mcp_server, McpServerHandle, McpServerStatus, McpTool,
};
use super::{McpConfigSource, McpServerConfig};

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
    /// Serializes concurrent [`prime_for_chat`](Self::prime_for_chat) runs so
    /// two primes racing on the same registry (the eager
    /// `agent_chat_prime_mcp` warm-up firing as a draft surface mounts, and
    /// the correctness-backstop prime inside `agent_chat_start_session`)
    /// cannot both slip past `ensure_started`'s fast-path check and
    /// double-spawn the same MCP child. `ensure_started` drops its lock
    /// between the "already running?" probe and inserting the `Starting`
    /// placeholder, so without this gate the two callers can each observe a
    /// server as absent and each launch it. Held across the whole prime loop:
    /// by the time the second prime acquires it every server the first prime
    /// touched is already `Starting`/`Running`, so the second prime is the
    /// cheap no-op the design wants.
    prime_lock: Arc<Mutex<()>>,
    /// Lazily-started authenticated loopback gateway shared by provider
    /// adapters that need a remote MCP URL (OpenCode and future providers).
    gateway: Arc<OnceCell<Arc<super::gateway::McpGatewayRuntime>>>,
    native_delegation: Arc<std::sync::Mutex<Option<NativeDelegationHandler>>>,
    /// Tools Codemux serves itself, in-process, to its own Claude and Codex
    /// chats (see [`HostTools`]). Set once at startup.
    host: Arc<std::sync::OnceLock<Arc<dyn HostTools>>>,
    plugin_epoch: Arc<std::sync::atomic::AtomicU64>,
    activity: Arc<std::sync::OnceLock<Arc<dyn Fn(PluginMcpActivity) + Send + Sync>>>,
}

/// The chat calling a host tool, as identified by the adapter session that
/// received the call. Nothing in it comes from the model's arguments, so a
/// tool can trust it for permission and ownership checks.
#[derive(Debug, Clone)]
pub struct HostCaller {
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
/// [`McpRegistry::dispatch_tool_call`]: those also back the OpenCode
/// gateway and `codemux mcp`, which cannot identify the calling chat. Only
/// adapters that know their session (Claude, Codex) list and call these.
pub trait HostTools: Send + Sync {
    /// Tools to offer `caller`, already prefixed like registry tools.
    fn tools_for(&self, caller: &HostCaller) -> Vec<McpTool>;
    /// Answer a call as an MCP `tools/call` result (`content` + `isError`),
    /// or `None` when `prefixed_name` is not a host tool. Codex answers its
    /// tool requests inline, so this must never wait on a provider.
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
        self.0.iter().flat_map(|host| host.tools_for(caller)).collect()
    }

    fn call(&self, caller: &HostCaller, name: &str, arguments: &serde_json::Value) -> Option<serde_json::Value> {
        self.0.iter().find_map(|host| host.call(caller, name, arguments))
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
    /// Keyed by `McpServerConfig.id`. Codemux's hardcoded row is also
    /// in here under `"codemux-self"` once spawned.
    handles: HashMap<String, McpServerHandle>,
    /// Mirror of the frontend zustand `disabledIds` — written by
    /// `set_disabled_ids` whenever the store changes. Disabled servers
    /// are skipped during `prime_for_chat` and stopped if currently
    /// running.
    disabled_ids: HashSet<String>,
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
                disabled_ids: HashSet::new(),
            })),
            status_tx,
            prime_lock: Arc::new(Mutex::new(())),
            gateway: Arc::new(OnceCell::new()),
            native_delegation: Arc::new(std::sync::Mutex::new(None)),
            host: Arc::new(std::sync::OnceLock::new()),
            plugin_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            activity: Arc::new(std::sync::OnceLock::new()),
        }
    }
}

pub(crate) type NativeDelegationHandler=Arc<dyn Fn(crate::delegation::app::NativeActor,String,serde_json::Value)->std::pin::Pin<Box<dyn std::future::Future<Output=Result<serde_json::Value,String>>+Send>>+Send+Sync>;
impl McpRegistry {
    pub(crate) fn set_native_delegation_handler(&self,handler:NativeDelegationHandler){*self.native_delegation.lock().unwrap()=Some(handler);}
    pub(crate) async fn dispatch_native_tool_call(
        &self,
        name: &str,
        arguments: serde_json::Value,
        actor: crate::delegation::app::NativeActor,
    ) -> Result<serde_json::Value, String> {
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
            let command = if raw == crate::mcp_server::REMOTE_DELEGATE_TOOL {
                "delegate_task"
            } else {
                raw.as_str()
            };
            let value = handler(actor, command.into(), arguments).await?;
            return Ok(serde_json::json!({"content":[{"type":"text","text":value.to_string()}]}));
        }
        self.dispatch_tool_call(name, arguments, actor.workspace_id.as_deref())
            .await
    }
    pub fn set_activity_emitter(&self, emit: Arc<dyn Fn(PluginMcpActivity) + Send + Sync>) { let _ = self.activity.set(emit); }

    pub fn new() -> Self {
        Self::default()
    }

    /// Return the process-wide authenticated MCP gateway, starting it on an
    /// ephemeral loopback port on first use.
    pub async fn gateway_connection(
        &self,
    ) -> Result<super::gateway::McpGatewayConnection, String> {
        let registry = self.clone();
        let runtime = self
            .gateway
            .get_or_try_init(|| async move {
                super::gateway::start_gateway(registry).await.map(Arc::new)
            })
            .await?;
        Ok(runtime.connection())
    }

    /// The URL's caller context is app-supplied configuration, never a tool
    /// argument. Token-bound caller identities can replace this seam.
    pub async fn gateway_connection_for_workspace(&self, workspace: Option<&str>) -> Result<super::gateway::McpGatewayConnection, String> {
        let mut connection = self.gateway_connection().await?;
        if let Some(id) = workspace {
            connection.url = format!("{}?workspaceId={}", connection.url, urlencoding::encode(id));
        }
        Ok(connection)
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
        {
            let mut inner = self.inner.lock().await;
            inner.disabled_ids = ids.iter().cloned().collect();
            // Codemux self can't be disabled.
            inner.disabled_ids.remove("codemux-self");
            for (id, handle) in inner.handles.iter() {
                if inner.disabled_ids.contains(id) && matches!(handle.status, McpServerStatus::Running { .. } | McpServerStatus::Starting) {
                    to_stop.push(id.clone());
                }
            }
        }

        for id in to_stop {
            let _ = self.stop_server(app, &id).await;
        }
        Ok(())
    }

    /// Whether a server id is currently in the disabled set.
    pub async fn is_disabled(&self, id: &str) -> bool {
        let inner = self.inner.lock().await;
        inner.disabled_ids.contains(id)
    }

    /// Start the given config if not already running. Idempotent: a
    /// second call against an already-running id is a no-op. Emits
    /// `mcp-status-changed` on every transition.
    pub async fn ensure_started<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        mut config: McpServerConfig,
    ) -> McpServerRuntime {
        let id = config.id.clone();

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

        // Reserve the generation under one lock; stop/unregister can revoke
        // it while the handshake is in flight without resurrecting a child.
        let generation = uuid::Uuid::new_v4().to_string();
        config.raw.as_object_mut().map(|raw| raw.insert("runtimeGeneration".into(), serde_json::json!(generation)));
        if !config.raw.is_object() { config.raw = serde_json::json!({"runtimeGeneration": generation}); }
        let starting_ms = now_ms();
        let mut start_cancelled;
        {
            let mut inner = self.inner.lock().await;
            if let Some(existing) = inner.handles.get(&id) {
                if matches!(existing.status, McpServerStatus::Running { .. } | McpServerStatus::Starting) { return McpServerRuntime::from_handle(existing); }
            }
            if config.sources.contains(&McpConfigSource::Plugin) && !inner.plugins.get(&id).is_some_and(|r| config.raw.get("pluginRegistrationToken").and_then(serde_json::Value::as_str) == Some(r.token.as_str())) {
                let h = McpServerHandle::discovered(config); return McpServerRuntime::from_handle(&h);
            }
            let mut placeholder = McpServerHandle::discovered(config.clone());
            placeholder.status = McpServerStatus::Starting;
            placeholder.started_at_ms = Some(starting_ms);
            let (tx, rx) = tokio::sync::watch::channel(false);
            start_cancelled = rx;
            inner.cancel_calls.insert(id.clone(), tx);
            inner.handles.insert(id.clone(), placeholder);
        }
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

        // Drop the lock for the actual spawn — the handshake can take
        // up to HANDSHAKE_TIMEOUT seconds and we don't want to block
        // every other command on it.
        let mut handle = tokio::select! {
            biased;
            _ = start_cancelled.changed() => return self.plugin_status(&id).await.unwrap_or(McpServerRuntime { id, name: config.name.clone(), status: McpServerStatus::Stopped, tools_count: 0, error_message: None, stderr_tail: None, started_at_ms: None }),
            handle = start_mcp_server(config.clone()) => handle,
        };
        let row;
        {
            let mut inner = self.inner.lock().await;
            let current = inner.handles.get(&id).is_some_and(|h| matches!(h.status, McpServerStatus::Starting) && h.config.raw.get("runtimeGeneration").and_then(serde_json::Value::as_str) == Some(&generation));
            if !current {
                drop(inner);
                if let Some(child) = handle.child.take() { let _ = child.shutdown().await; }
                return self.plugin_status(&id).await.unwrap_or_else(|| McpServerRuntime::from_handle(&handle));
            }
            if let Some(error) = plugin::tool_collision(&inner, &id, &handle.tools) { handle.tools.clear(); handle.status = McpServerStatus::Errored { message: error }; }
            row = McpServerRuntime::from_handle(&handle);
            inner.cancel_calls.insert(id.clone(), tokio::sync::watch::channel(false).0);
            inner.handles.insert(id.clone(), handle);
        }
        emit_status_with_bus(app, &self.status_tx, &row);
        self.watch_tool_changes(app.cloned(), &id).await;
        row
    }

    async fn watch_tool_changes<R: Runtime>(&self, app: Option<AppHandle<R>>, id: &str) {
        let (config, child, remote) = {
            let inner = self.inner.lock().await;
            let Some(h) = inner.handles.get(id).filter(|h| h.status.is_running()) else { return; };
            (h.config.clone(), h.child.clone(), h.remote.clone())
        };
        // Subscribe before the reconciliation fetch: notifications during
        // initialize/list, before registry installation, cannot leave stale tools.
        let mut notifications = if let Some(child) = child.as_ref() {
            child.notifications()
        } else if let Some(remote) = remote.as_ref() { remote.notifications() } else { return; };
        let registry = self.clone();
        let server_id = id.to_owned();
        let task = tokio::spawn(async move {
            let mut stream_task = remote.as_ref().map(|r| {
                let r = r.clone();
                tokio::spawn(async move { r.listen_notifications().await; }).abort_handle()
            });
            // An abort-on-drop guard also cancels HTTP GET on unregister/restart.
            struct StreamGuard(Option<tokio::task::AbortHandle>);
            impl Drop for StreamGuard { fn drop(&mut self) { if let Some(t) = self.0.take() { t.abort(); } } }
            let _stream_guard = StreamGuard(stream_task.take());
            loop {
                let result = if let Some(c) = child.as_ref() {
                    super::runtime::list_stdio_tools(c, &config).await.map_err(|e| e.to_string())
                } else if let Some(r) = remote.as_ref() {
                    super::runtime::list_http_tools(r, &config).await
                } else { return; };
                let row = {
                    let mut inner = registry.inner.lock().await;
                    let result = result.and_then(|tools| match plugin::tool_collision(&inner, &server_id, &tools) { Some(error) => Err(error), None => Ok(tools) });
                    let Some(h) = inner.handles.get_mut(&server_id) else { return; };
                    let same = match (&h.child, &child, &h.remote, &remote) {
                        (Some(a), Some(b), _, _) => Arc::ptr_eq(a, b),
                        (_, _, Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        _ => false,
                    };
                    if !same || !h.status.is_running() { return; }
                    match result {
                        Ok(tools) => { h.tools = tools; h.status = McpServerStatus::Running { tool_count: h.tools.len() }; h.stderr_tail = None; },
                        Err(error) => { h.tools.clear(); h.status = McpServerStatus::Errored { message: format!("tools/list refresh failed: {error}") }; },
                    }
                    McpServerRuntime::from_handle(h)
                };
                emit_status_with_bus(app.as_ref(), &registry.status_tx, &row);
                loop {
                    match notifications.recv().await {
                        Ok(n) if n.method == "notifications/tools/list_changed" => break,
                        Ok(_) => {},
                        Err(broadcast::error::RecvError::Lagged(_)) => break,
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        });
        let mut inner = self.inner.lock().await;
        if let Some(old) = inner.watchers.insert(id.into(), task.abort_handle()) { old.abort(); }
    }

    /// Stop a server (graceful EOF then kill, 2s budget) and mark it
    /// Stopped. Idempotent: stopping an already-stopped server is fine.
    pub async fn stop_server<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        id: &str,
    ) -> Result<McpServerRuntime, String> {
        let child = {
            let mut inner = self.inner.lock().await;
            if let Some(task) = inner.watchers.remove(id) { task.abort(); }
            inner.cancel_calls.remove(id);
            let handle = match inner.handles.get_mut(id) {
                Some(h) => h,
                None => return Err(format!("no such server: {id}")),
            };
            let child = handle.child.take();
            handle.remote.take();
            handle.status = McpServerStatus::Stopped;
            handle.tools.clear();
            handle.server_info = None;
            handle.started_at_ms = None;
            child
        };

        if let Some(child) = child {
            let _ = child.shutdown().await;
        }

        let row = {
            let inner = self.inner.lock().await;
            inner
                .handles
                .get(id)
                .map(McpServerRuntime::from_handle)
                .ok_or_else(|| format!("server vanished after stop: {id}"))?
        };
        emit_status_with_bus(app, &self.status_tx, &row);
        Ok(row)
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

    /// Spawn every enabled (non-disabled) MCP server discovered for the
    /// given project root, plus Codemux's own hardcoded entry. Errors
    /// per server are surfaced as `Errored` rows rather than failing
    /// the whole prime.
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
        for cfg in configs {
            if cfg.disabled {
                continue;
            }
            let _ = self.ensure_started(app, cfg).await;
        }
    }

    /// Snapshot of every server's runtime row. Used by the
    /// `get_mcp_runtime_status` Tauri command for the Settings UI.
    /// Includes `Discovered` rows for configs the registry has seen
    /// (via prior calls) but not yet spawned, and any error rows still
    /// retained from earlier attempts.
    pub async fn list_runtime(&self) -> Vec<McpServerRuntime> {
        let inner = self.inner.lock().await;
        let mut out: Vec<McpServerRuntime> =
            inner.handles.values().map(McpServerRuntime::from_handle).collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    /// Provider-facing tools across running servers, without a count cap.
    /// Trusted native-controller tools are never injected into a provider.
    pub async fn list_all_tools(&self) -> Vec<McpTool> {
        self.list_all_tools_excluding_sources(&[]).await
    }

    /// Running tools except servers already native to `source`. This lets a
    /// provider keep its own authenticated/native MCPs while Codemux injects
    /// only the tools it would otherwise be missing.
    pub async fn list_all_tools_excluding_source(
        &self,
        source: McpConfigSource,
    ) -> Vec<McpTool> {
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
        self.list_tools_for_workspace(sources, None).await
    }

    pub async fn list_tools_for_workspace(&self, sources: &[McpConfigSource], workspace_id: Option<&str>) -> Vec<McpTool> {
        let inner = self.inner.lock().await;
        let mut tools = Vec::new();
        for handle in inner.handles.values() {
            if inner.plugins.get(&handle.config.id).is_some_and(|r| !r.scope.visible(workspace_id)) { continue; }
            let provider_native = handle
                .config
                .sources
                .iter()
                .any(|source| sources.contains(source));
            if handle.status.is_running() && !provider_native {
                let is_self = handle.config.sources.contains(&McpConfigSource::Codemux);
                tools.extend(handle.tools.iter().filter(|tool|
                    !is_self || !crate::agent_control::is_native_tool(&tool.name)
                ).cloned());
            }
        }
        tools
    }

    #[cfg(test)]
    pub(crate) async fn insert_native_delegation_tools_for_test(&self) {
        let config = super::codemux_self::codemux_self_config();
        let mut handle = McpServerHandle::discovered(config);
        handle.tools = crate::mcp_server::remote_delegation_tools_for_test();
        handle.status = McpServerStatus::Running { tool_count: handle.tools.len() };
        self.inner.lock().await.handles.insert(handle.config.id.clone(), handle);
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
    /// fallback behavior.
    pub async fn dispatch_tool_call(
        &self,
        prefixed_name: &str,
        arguments: serde_json::Value,
        workspace_id: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        self.dispatch_tool_call_inner(prefixed_name, arguments, workspace_id, None).await
    }

    async fn dispatch_tool_call_inner(&self, prefixed_name: &str, arguments: serde_json::Value, workspace_id: Option<&str>, owned_server: Option<&str>) -> Result<serde_json::Value, String> {
        let (raw_name, is_codemux_self, child, remote, mut cancelled, mut activity) = {
            let inner = self.inner.lock().await;
            let mut found = None;
            for handle in inner.handles.values() {
                if owned_server.is_some_and(|id| id != handle.config.id) { continue; }
                if owned_server.is_none() && inner.plugins.get(&handle.config.id).is_some_and(|r| !r.scope.visible(workspace_id)) { continue; }
                for tool in &handle.tools {
                    if tool.prefixed_name == prefixed_name {
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
                        let cancelled = inner.cancel_calls.get(&handle.config.id).map(|tx| tx.subscribe());
                        let activity = inner.plugins.get(&handle.config.id).and_then(|r| self.activity.get().map(|emit| {
                            let event = PluginMcpActivity { plugin_id: r.plugin_id.clone(), server_id: handle.config.id.clone(), phase: "start".into(), call_id: uuid::Uuid::new_v4().to_string(), tool_name: tool.name.clone(), prefixed_name: tool.prefixed_name.clone(), workspace_id: workspace_id.map(str::to_owned), is_error: None, duration_ms: None };
                            emit(event.clone());
                            plugin::ActivityGuard { event, started: std::time::Instant::now(), emit: emit.clone() }
                        }));
                        found = Some((tool.name.clone(), is_self, child, remote, cancelled, activity));
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
        if is_codemux_self && crate::agent_control::is_native_tool(&raw_name) {
            return Err("provider_control_denied: native controller tools are not available through provider MCP relays".into());
        }
        let params = tool_call_params(&raw_name, arguments, is_codemux_self, workspace_id);
        let call = async {
            if let Some(child) = child {
                child.request("tools/call", params).await.map_err(|e| format!("tools/call failed: {e}"))
            } else if let Some(remote) = remote {
                tokio::time::timeout(super::runtime::DEFAULT_REQUEST_TIMEOUT, remote.request("tools/call", params)).await.map_err(|_| "tools/call timed out".to_string())?
            } else { Err("MCP transport disappeared before tools/call".into()) }
        };
        let cancellation = async { match cancelled.as_mut() { Some(rx) => { let _ = rx.changed().await; }, None => std::future::pending::<()>().await } };
        let result = tokio::select! { biased; _ = cancellation => Err("MCP server stopped during tools/call".into()), result = call => result };
        if let Some(guard) = activity.as_mut() { guard.event.is_error = Some(result.as_ref().map(|r| r.get("isError").and_then(serde_json::Value::as_bool).unwrap_or(false)).unwrap_or(true)); }
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
            for (_, task) in inner.watchers.drain() { task.abort(); }
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

/// Walk the configured MCP files for a project root and produce a flat
/// `Vec<McpServerConfig>` suitable for spawning. Identical definitions from
/// multiple providers are deduplicated before launch so Codemux owns one
/// child/connection and one stable tool namespace for all adapters.
fn discover_configs(project_root: Option<&std::path::Path>) -> Vec<McpServerConfig> {
    let mut out: Vec<McpServerConfig> = vec![codemux_self_config()];

    let project_path: Option<PathBuf> = project_root.map(|p| p.to_path_buf());
    let scan = enumerate_mcp_paths(project_path.as_deref());

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
                for srv in parsed {
                    // Same filter as the list command: drop provider-written
                    // `codemux` entries so we don't spawn the built-in twice.
                    if srv.name == "codemux" {
                        continue;
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
            .match_body(Matcher::PartialJson(serde_json::json!({ "method": "initialize" })))
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
            .match_body(Matcher::PartialJson(serde_json::json!({ "method": "tools/list" })))
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
            (prefixed_name == "mcp__codemux__echo")
                .then(|| serde_json::json!({ "content": [{"type": "text", "text": caller.thread_id}] }))
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_and_remote_delegation_keep_distinct_real_prefixes_when_local_is_disabled() {
        use crate::agent_provider::ProviderKind;
        use crate::commands::delegation::{DelegationHost, DelegationState, DELEGATION_SETTING_KEY};
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
            thread_id: "catalog-routing-parent".into(),
            provider: ProviderKind::Codex,
            workspace_id: Some("workspace".into()),
            permission_mode: Some("danger-full-access".into()),
        };
        let owner = crate::delegation::authority::NativeAuthority::new(&caller.thread_id);
        let generation = owner.begin();
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
        reg.set_native_delegation_handler(Arc::new(move |actor, command, arguments| {
            let expected = expected.clone();
            let count = count.clone();
            Box::pin(async move {
                let _admission = actor.permit.admit()?;
                assert_eq!(
                    command, "delegate_task",
                    "MCP rename must preserve the coordinator command"
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
            let names: HashSet<_> = local
                .iter()
                .chain(&remote)
                .map(|tool| tool.prefixed_name.as_str())
                .collect();
            assert_eq!(
                names.len(),
                local.len() + remote.len(),
                "combined native catalog must have unique names"
            );
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
            thread_id: "thread-1".into(),
            provider: crate::agent_provider::ProviderKind::Claude,
            workspace_id: None,
            permission_mode: None,
        };
        assert!(reg.host_tools(&caller).is_empty(), "nothing before install");
        assert!(reg.host_call(&caller, "mcp__codemux__echo", &serde_json::json!({})).is_none());

        // Installing through a clone reaches every other clone.
        reg.clone_handle().set_host_tools(Arc::new(EchoHost));
        let tools = reg.host_tools(&caller);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].description.as_deref(), Some("thread-1"));
        assert!(reg.host_call(&caller, "mcp__codemux__echo", &serde_json::json!({})).is_some());
        assert!(reg.host_call(&caller, "mcp__codemux__other", &serde_json::json!({})).is_none());

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
        let call = server.mock("POST", "/mcp")
            .match_body(Matcher::PartialJson(serde_json::json!({"method":"tools/call","params":{"name":"thread_launch"}})))
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"jsonrpc":"2.0","id":3,"result":{"content":[]}}).to_string())
            .expect(0).create_async().await;
        let result = registry.dispatch_tool_call("mcp__codemux__thread_launch",
            serde_json::json!({"workspace_id":"worker-ws","provider":"codex","permission_mode":"danger-full-access","client_request_id":"nested"}),
            Some("worker-ws")).await;
        assert!(result.is_err(), "provider relay must not confer trusted native-controller authority: {result:?}");
        assert!(result.unwrap_err().starts_with("provider_control_denied:"));
        call.assert_async().await;
        assert!(!registry.list_all_tools().await.iter().any(|t| t.name == "thread_launch"),
            "provider snapshot must not advertise an unusable trusted control route");
        assert!(!registry.list_all_tools_excluding_source(McpConfigSource::CodexUser).await.iter().any(|t| t.name == "thread_launch"));
        // Controller discovery itself and Settings retain the native catalog.
        assert!(crate::agent_control::native_tools().iter().any(|t| t.name == "thread_launch"));
        assert!(registry.list_tools_for_server("codemux-remote").await.iter().any(|t| t.name == "thread_launch"));
    }

    #[tokio::test]
    async fn ensure_started_marks_errored_on_bad_command() {
        let reg = McpRegistry::new();
        let cfg = make_config("bad", "bad", "/no/such/binary/at/all");
        let row = reg.ensure_started(None::<&tauri::AppHandle<tauri::Wry>>, cfg).await;
        assert!(matches!(row.status, McpServerStatus::Errored { .. }));
        assert!(row.error_message.is_some());
    }

    #[tokio::test]
    async fn disabled_ids_short_circuit_ensure_started() {
        let reg = McpRegistry::new();
        reg.set_disabled_ids(None::<&tauri::AppHandle<tauri::Wry>>, vec!["disabled-srv".into()])
            .await
            .unwrap();
        let cfg = make_config("disabled-srv", "disabled", "/no/such/binary");
        let row = reg.ensure_started(None::<&tauri::AppHandle<tauri::Wry>>, cfg).await;
        assert!(matches!(row.status, McpServerStatus::Stopped));
        // No handle should have been inserted because we short-circuited.
        let listed = reg.list_runtime().await;
        assert!(listed.iter().all(|r| r.id != "disabled-srv"));
    }

    #[tokio::test]
    async fn codemux_self_cannot_be_disabled() {
        let reg = McpRegistry::new();
        reg.set_disabled_ids(None::<&tauri::AppHandle<tauri::Wry>>, vec!["codemux-self".into()])
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
            h.status = McpServerStatus::Errored { message: "boom".into() };
            inner.handles.insert("x".into(), h);
        }
        let row = reg.stop_server(None::<&tauri::AppHandle<tauri::Wry>>, "x").await.unwrap();
        assert!(matches!(row.status, McpServerStatus::Stopped));
    }

    #[tokio::test]
    async fn stop_server_unknown_id_errors() {
        let reg = McpRegistry::new();
        let err = reg.stop_server(None::<&tauri::AppHandle<tauri::Wry>>, "nope").await.unwrap_err();
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
        let (_server, reg, call) =
            super::test_support::codemux_remote_server("ws-42").await;
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
            h.status = McpServerStatus::Errored { message: "x".into() };
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
        };
        let row = reg
            .ensure_started(None::<&tauri::AppHandle<tauri::Wry>>, config)
            .await;
        assert!(matches!(row.status, McpServerStatus::Running { tool_count: 1 }));
        let result = reg
            .dispatch_tool_call(
                "mcp__remote__echo",
                serde_json::json!({"text": "hi"}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "hi");
        initialize.assert_async().await;
        initialized.assert_async().await;
        list.assert_async().await;
        call.assert_async().await;
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
        assert!(matches!(row.status, McpServerStatus::Running { .. }), "{row:?}");
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
        assert!(result.get("content").and_then(serde_json::Value::as_array).is_some());
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
        reg.set_disabled_ids(None::<&tauri::AppHandle<tauri::Wry>>, vec!["x".into()]).await.unwrap();
        let rows = reg.list_runtime().await;
        let row = rows.iter().find(|r| r.id == "x").unwrap();
        assert!(matches!(row.status, McpServerStatus::Stopped));
    }
}
