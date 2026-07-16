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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::{broadcast, Mutex};

use super::codemux_self::codemux_self_config;
use super::parser::{parse_claude_wrapped_config, parse_mcp_config_file};
use super::paths::{enumerate_mcp_paths, is_claude_wrapped_path};
use super::runtime::{
    aggregate_tools, apply_tool_cap, start_mcp_server, CappedTools, McpServerHandle,
    McpServerStatus, McpTool,
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
                disabled_ids: HashSet::new(),
            })),
            status_tx,
            prime_lock: Arc::new(Mutex::new(())),
        }
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mirror the frontend's disabled-set into the registry. The next
    /// `prime_for_chat` call respects it; servers currently running but
    /// just-disabled get stopped.
    pub async fn set_disabled_ids(
        &self,
        app: Option<&AppHandle>,
        ids: Vec<String>,
    ) -> Result<(), String> {
        let mut to_stop: Vec<String> = Vec::new();
        {
            let mut inner = self.inner.lock().await;
            inner.disabled_ids = ids.iter().cloned().collect();
            // Codemux self can't be disabled.
            inner.disabled_ids.remove("codemux-self");
            for (id, handle) in inner.handles.iter() {
                if inner.disabled_ids.contains(id) && handle.status.is_running() {
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
    pub async fn ensure_started(
        &self,
        app: Option<&AppHandle>,
        config: McpServerConfig,
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

        // Fast path: already running or starting.
        {
            let inner = self.inner.lock().await;
            if let Some(existing) = inner.handles.get(&id) {
                if matches!(
                    existing.status,
                    McpServerStatus::Running { .. } | McpServerStatus::Starting
                ) {
                    return McpServerRuntime::from_handle(existing);
                }
            }
        }

        // Mark Starting so the UI sees a spinner during the actual
        // spawn+handshake (which can take seconds for npx-launched
        // servers). `started_at_ms` is set to NOW so the frontend can
        // tell when handshake exceeds the slow-start threshold and
        // surface a "taking longer than usual" hint.
        let starting_ms = now_ms();
        {
            let mut inner = self.inner.lock().await;
            let placeholder = McpServerHandle {
                config: config.clone(),
                status: McpServerStatus::Starting,
                tools: Vec::new(),
                server_info: None,
                started_at_ms: Some(starting_ms),
                child: None,
                stderr_tail: None,
            };
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
        let handle = start_mcp_server(config).await;
        let row = McpServerRuntime::from_handle(&handle);

        {
            let mut inner = self.inner.lock().await;
            inner.handles.insert(id, handle);
        }
        emit_status_with_bus(app, &self.status_tx, &row);
        row
    }

    /// Stop a server (graceful EOF then kill, 2s budget) and mark it
    /// Stopped. Idempotent: stopping an already-stopped server is fine.
    pub async fn stop_server(
        &self,
        app: Option<&AppHandle>,
        id: &str,
    ) -> Result<McpServerRuntime, String> {
        let child = {
            let mut inner = self.inner.lock().await;
            let handle = match inner.handles.get_mut(id) {
                Some(h) => h,
                None => return Err(format!("no such server: {id}")),
            };
            let child = handle.child.take();
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
    pub async fn restart_server(
        &self,
        app: Option<&AppHandle>,
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
    pub async fn prime_for_chat(
        &self,
        app: Option<&AppHandle>,
        project_root: Option<&std::path::Path>,
    ) {
        // Serialize concurrent primes so a warm-up prime and the
        // start-session backstop prime can't double-spawn the same child
        // (see `prime_lock`). The second waiter finds every server already
        // `Starting`/`Running` and no-ops through `ensure_started`.
        let _prime_guard = self.prime_lock.lock().await;
        let configs = discover_configs(project_root);
        for cfg in configs {
            // Skip non-stdio for Stage 2 (HTTP support deferred).
            if !matches!(cfg.transport, super::McpTransport::Stdio) {
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

    /// All currently-running tools across all servers, with the 50-tool
    /// cap applied. Codemux's tools always survive the cap. Returns
    /// the trimmed `Vec<McpTool>` for the common path; use
    /// [`Self::list_all_tools_with_cap_info`] when the caller (e.g.
    /// the Settings UI) needs to know whether the cap was triggered.
    pub async fn list_all_tools(&self) -> Vec<McpTool> {
        self.list_all_tools_with_cap_info().await.tools
    }

    /// Like [`Self::list_all_tools`] but returns the full
    /// [`CappedTools`] envelope so the Settings UI can render a
    /// "23 tools dropped to fit cap" banner.
    pub async fn list_all_tools_with_cap_info(&self) -> CappedTools {
        let inner = self.inner.lock().await;
        apply_tool_cap(aggregate_tools(&inner.handles))
    }

    /// Every tool registered by `server_id`, regardless of whether
    /// the cap kicked in. Used by the Settings tool-list modal so the
    /// user sees a server's full surface even when some tools were
    /// dropped from the agent's view to fit `TOOL_CAP`.
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
    pub async fn dispatch_tool_call(
        &self,
        prefixed_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let (raw_name, child) = {
            let inner = self.inner.lock().await;
            let mut found: Option<(
                String,
                std::sync::Arc<crate::json_rpc_child::JsonRpcChild>,
            )> = None;
            for handle in inner.handles.values() {
                for tool in &handle.tools {
                    if tool.prefixed_name == prefixed_name {
                        if !handle.status.is_running() {
                            return Err(format!(
                                "MCP server '{}' is not running",
                                handle.config.name
                            ));
                        }
                        let child = match &handle.child {
                            Some(c) => c.clone(),
                            None => {
                                return Err(format!(
                                    "MCP server '{}' has no child process",
                                    handle.config.name
                                ));
                            }
                        };
                        found = Some((tool.name.clone(), child));
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

        child
            .request(
                "tools/call",
                serde_json::json!({
                    "name": raw_name,
                    "arguments": arguments,
                }),
            )
            .await
            .map_err(|e| format!("tools/call failed: {e}"))
    }

    /// Tear every spawned child down. Called from the
    /// `tauri::RunEvent::Exit` handler. Best-effort: errors are
    /// swallowed because the process is exiting anyway.
    pub async fn shutdown_all(&self) {
        let children: Vec<Arc<crate::json_rpc_child::JsonRpcChild>> = {
            let mut inner = self.inner.lock().await;
            let mut out = Vec::new();
            for handle in inner.handles.values_mut() {
                if let Some(child) = handle.child.take() {
                    out.push(child);
                }
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

/// Emit a status-changed event so the frontend updates without
/// polling AND fan out to in-process subscribers (the Claude adapter
/// listens here for dynamic-refresh pushes). `app` is optional so
/// registry tests don't need a real Tauri app; `status_tx` is taken
/// off the registry instance so the broadcast survives in tests too.
fn emit_status_with_bus(
    app: Option<&AppHandle>,
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
/// `Vec<McpServerConfig>` suitable for spawning. Mirrors the logic in
/// `commands/mcp::list_mcp_servers` but without dedupe — the runtime
/// already tracks one handle per `id`, and dedupe at the spawn layer
/// would obscure which file an error originated from.
fn discover_configs(project_root: Option<&std::path::Path>) -> Vec<McpServerConfig> {
    let mut out: Vec<McpServerConfig> = vec![codemux_self_config()];

    let project_path: Option<PathBuf> = project_root.map(|p| p.to_path_buf());
    let scan = enumerate_mcp_paths(project_path.as_deref());

    for (path, source) in scan.paths {
        let parsed = if is_claude_wrapped_path(&path) {
            parse_claude_wrapped_config(&path, project_path.as_deref())
        } else {
            parse_mcp_config_file(&path, source)
        };
        match parsed {
            Ok(parsed) => {
                for srv in parsed {
                    // Same filter as the list command: drop the
                    // auto-written project `.mcp.json` codemux entry so
                    // we don't try to spawn a second copy of it.
                    if srv.name == "codemux"
                        && matches!(
                            source,
                            McpConfigSource::ClaudeProject
                                | McpConfigSource::CodemuxProject
                        )
                    {
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{McpConfigSource, McpServerConfig, McpTransport};
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

    #[tokio::test]
    async fn ensure_started_marks_errored_on_bad_command() {
        let reg = McpRegistry::new();
        let cfg = make_config("bad", "bad", "/no/such/binary/at/all");
        let row = reg.ensure_started(None, cfg).await;
        assert!(matches!(row.status, McpServerStatus::Errored { .. }));
        assert!(row.error_message.is_some());
    }

    #[tokio::test]
    async fn disabled_ids_short_circuit_ensure_started() {
        let reg = McpRegistry::new();
        reg.set_disabled_ids(None, vec!["disabled-srv".into()])
            .await
            .unwrap();
        let cfg = make_config("disabled-srv", "disabled", "/no/such/binary");
        let row = reg.ensure_started(None, cfg).await;
        assert!(matches!(row.status, McpServerStatus::Stopped));
        // No handle should have been inserted because we short-circuited.
        let listed = reg.list_runtime().await;
        assert!(listed.iter().all(|r| r.id != "disabled-srv"));
    }

    #[tokio::test]
    async fn codemux_self_cannot_be_disabled() {
        let reg = McpRegistry::new();
        reg.set_disabled_ids(None, vec!["codemux-self".into()])
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
        let row = reg.stop_server(None, "x").await.unwrap();
        assert!(matches!(row.status, McpServerStatus::Stopped));
    }

    #[tokio::test]
    async fn stop_server_unknown_id_errors() {
        let reg = McpRegistry::new();
        let err = reg.stop_server(None, "nope").await.unwrap_err();
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
    async fn dispatch_tool_call_unknown_tool_errors() {
        let reg = McpRegistry::new();
        let err = reg
            .dispatch_tool_call("mcp__nope__x", serde_json::json!({}))
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
            .dispatch_tool_call("mcp__github__create_issue", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.contains("not running"));
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
        reg.set_disabled_ids(None, vec!["x".into()]).await.unwrap();
        let rows = reg.list_runtime().await;
        let row = rows.iter().find(|r| r.id == "x").unwrap();
        assert!(matches!(row.status, McpServerStatus::Stopped));
    }
}
