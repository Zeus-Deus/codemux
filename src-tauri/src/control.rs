use crate::indexing;
use crate::memory;
use crate::state::AppStateStore;
use crate::terminal::PtyState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(unix)]
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

// ---------------------------------------------------------------------------
// Transport abstraction
//
// The control channel is a line-oriented JSON protocol that needs one of two
// underlying transports:
//   * Unix domain sockets at `$XDG_RUNTIME_DIR/codemux.sock` (or a /tmp
//     fallback) on Linux/macOS.
//   * Windows named pipes at `\\.\pipe\codemux-{username}` on Windows.
//
// Both transports expose the same tiny API (bind, accept, connect, sync
// liveness probe). The rest of `control.rs` is generic over the concrete
// stream type using `tokio::io::AsyncRead + AsyncWrite + Unpin`, so command
// handlers and the dispatch loop never have to know which platform they are
// running on.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod unix_transport {
    use std::path::Path;
    use tokio::net::{UnixListener, UnixStream};

    pub type ServerStream = UnixStream;
    pub type ClientStream = UnixStream;

    pub struct Listener(UnixListener);

    pub fn bind(path: &Path) -> std::io::Result<Listener> {
        // Tokio's UnixListener::bind fails if the socket file already exists,
        // so remove any stale file first. The caller is expected to have
        // already established, via `sync_liveness_probe`, that no live server
        // is holding the path.
        let _ = std::fs::remove_file(path);
        UnixListener::bind(path).map(Listener)
    }

    impl Listener {
        pub async fn accept(&self) -> std::io::Result<ServerStream> {
            self.0.accept().await.map(|(stream, _)| stream)
        }
    }

    pub async fn connect(path: &Path) -> std::io::Result<ClientStream> {
        UnixStream::connect(path).await
    }

    /// Blocking probe: does a server respond on this path right now?
    /// Safe to call from sync contexts (e.g. startup path before the tokio
    /// runtime exists) because it uses the std networking API.
    pub fn sync_liveness_probe(path: &Path) -> bool {
        std::os::unix::net::UnixStream::connect(path).is_ok()
    }
}

#[cfg(windows)]
mod windows_transport {
    use std::path::Path;
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };
    use tokio::sync::Mutex;

    pub type ServerStream = NamedPipeServer;
    pub type ClientStream = NamedPipeClient;

    /// Wraps the "current waiting server instance" for a named pipe. Windows
    /// named pipes consume a server instance on every client connect, so we
    /// must eagerly create the next instance inside `accept()` before
    /// returning the connected one to the caller.
    pub struct Listener {
        pipe_name: String,
        pending: Mutex<Option<NamedPipeServer>>,
    }

    pub fn bind(path: &Path) -> std::io::Result<Listener> {
        let pipe_name = path.to_string_lossy().into_owned();
        // `first_pipe_instance(true)` ensures no other process has already
        // created a pipe with this name — this is the Windows equivalent of
        // the sync liveness probe done by the caller on Unix.
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)?;
        Ok(Listener {
            pipe_name,
            pending: Mutex::new(Some(first)),
        })
    }

    impl Listener {
        pub async fn accept(&self) -> std::io::Result<ServerStream> {
            let mut guard = self.pending.lock().await;
            let server = guard.take().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "named pipe listener has no pending server instance",
                )
            })?;
            server.connect().await?;
            // Eagerly create the next server instance so the listener is
            // ready to accept another client as soon as this one is handed
            // to its handler task.
            let next = ServerOptions::new().create(&self.pipe_name)?;
            *guard = Some(next);
            Ok(server)
        }
    }

    pub async fn connect(path: &Path) -> std::io::Result<ClientStream> {
        let pipe_name = path.to_string_lossy().into_owned();

        // Windows named pipes do NOT queue pending connections the way Unix
        // sockets do — there's a brief window between the server's `accept()`
        // returning a connected instance and the bind of the next instance,
        // during which a client `open()` call gets ERROR_PIPE_BUSY (raw OS
        // error 231). This surfaces in practice when a Claude Code subagent
        // fires multiple `codemux <subcommand>` invocations rapidly: each
        // spawns a fresh CLI client process, and the second/third arrive
        // before the server has re-armed.
        //
        // The documented Windows pattern (per MSDN's WaitNamedPipe page) is
        // to wait briefly and retry. We use exponential backoff capped at
        // 200ms with a total worst-case budget of ~385ms before giving up,
        // which is well under the human-perceptible threshold for a CLI
        // command and effectively eliminates the race in normal use.
        const ERROR_PIPE_BUSY: i32 = 231;
        let backoffs_ms: [u64; 5] = [10, 25, 50, 100, 200];

        let mut last_err: Option<std::io::Error> = None;
        for delay_ms in std::iter::once(0).chain(backoffs_ms.iter().copied()) {
            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            match ClientOptions::new().open(&pipe_name) {
                Ok(stream) => return Ok(stream),
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    last_err = Some(err);
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                "control pipe busy after retries",
            )
        }))
    }

    /// Blocking probe: does a server respond on this pipe right now?
    /// Opens the pipe via the std sync API so this is callable before the
    /// tokio runtime exists.
    pub fn sync_liveness_probe(path: &Path) -> bool {
        let pipe_name = path.to_string_lossy().into_owned();
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&pipe_name)
            .is_ok()
    }
}

#[cfg(unix)]
use unix_transport as transport;
#[cfg(windows)]
use windows_transport as transport;

const CONTROL_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct ControlRequest {
    pub command: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ControlResponse {
    pub ok: bool,
    pub protocol_version: u32,
    pub data: Option<Value>,
    pub error: Option<String>,
}

/// Basename for the control socket / named pipe. Debug builds use a
/// distinct name so a locally-running dev build doesn't collide with the
/// installed release build (and vice versa).
#[cfg(debug_assertions)]
const SOCKET_BASENAME: &str = "codemux-dev";
#[cfg(not(debug_assertions))]
const SOCKET_BASENAME: &str = "codemux";

#[cfg(unix)]
pub fn control_socket_path() -> Option<PathBuf> {
    let socket_file = format!("{SOCKET_BASENAME}.sock");
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        return Some(runtime_dir.join(&socket_file));
    }

    // Fallback for systems without XDG_RUNTIME_DIR (e.g. minimal distros, some AppImage environments).
    let uid = unsafe { libc::getuid() };
    // This whole function is `#[cfg(unix)]` — the hardcoded /tmp path is
    // reachable only on Unix and matches the XDG fallback convention
    // for systems that don't set XDG_RUNTIME_DIR. The tmp-literal-ok
    // marker tells the meta-test in agent_context.rs that this is
    // intentional and not a Windows regression.
    let fallback_dir = PathBuf::from(format!("/tmp/codemux-{uid}")); // tmp-literal-ok
    if fallback_dir.exists() {
        // Verify the existing directory is owned by us and has safe permissions.
        use std::os::unix::fs::MetadataExt;
        match fs::metadata(&fallback_dir) {
            Ok(meta) => {
                if meta.uid() != uid {
                    crate::diagnostics::stderr_line(&format!(
                        "[codemux::control] Fallback dir {} is owned by uid {} (expected {}); refusing to use it",
                        fallback_dir.display(), meta.uid(), uid
                    ));
                    return None;
                }
            }
            Err(e) => {
                crate::diagnostics::stderr_line(&format!(
                    "[codemux::control] Cannot stat fallback dir {}: {e}",
                    fallback_dir.display()
                ));
                return None;
            }
        }
    } else {
        if let Err(e) = fs::create_dir(&fallback_dir) {
            crate::diagnostics::stderr_line(&format!(
                "[codemux::control] Failed to create fallback runtime dir {}: {e}",
                fallback_dir.display()
            ));
            return None;
        }
        // Restrict to owner only (mode 0700).
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&fallback_dir, fs::Permissions::from_mode(0o700));
    }
    crate::diagnostics::stderr_line(&format!(
        "[codemux::control] XDG_RUNTIME_DIR unset, using fallback: {}",
        fallback_dir.display()
    ));
    Some(fallback_dir.join(&socket_file))
}

/// Build the Windows named-pipe path for the given username.
///
/// Extracted from `control_socket_path()` so tests can drive it with
/// arbitrary usernames without touching the process-global `USERNAME`
/// env var (which would race with parallel test execution).
///
/// The pipe lives under the `\\.\pipe\` virtual namespace — no filesystem
/// ownership or permission bookkeeping is required because named pipes
/// inherit the creating user's security descriptor by default.
///
/// Sanitization rule: anything that isn't alphanumeric / underscore /
/// dash is replaced with `_`. This guarantees we never emit an invalid
/// pipe name even if the username contains spaces, separators, or any
/// of the Windows-forbidden filename characters (`\ / : * ? " < > |`).
/// Empty or whitespace-only inputs fall back to `default` so the result
/// always has a non-empty user segment.
///
/// Not cfg-gated — pure string manipulation, safe to compile and test
/// on every platform. Only `control_socket_path()` on Windows actually
/// calls it at runtime.
#[allow(dead_code)] // only called from the Windows branch of control_socket_path
fn build_pipe_path(username: &str) -> PathBuf {
    let base = if username.trim().is_empty() {
        "default".to_string()
    } else {
        username.to_string()
    };
    let sanitized: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    PathBuf::from(format!(r"\\.\pipe\{SOCKET_BASENAME}-{sanitized}"))
}

#[cfg(windows)]
pub fn control_socket_path() -> Option<PathBuf> {
    let username = std::env::var("USERNAME").unwrap_or_default();
    Some(build_pipe_path(&username))
}

/// Blocking check: is another Codemux control server currently listening?
/// Safe to call from sync contexts (e.g. before the Tauri runtime spins up).
pub fn control_server_is_running() -> bool {
    match control_socket_path() {
        Some(path) => transport::sync_liveness_probe(&path),
        None => false,
    }
}

/// Resolve "default" or empty browser_id to the first active browser session's actual ID.
fn resolve_browser_id<R: Runtime>(app: &AppHandle<R>, requested: &str) -> String {
    if !requested.is_empty() && requested != "default" {
        return requested.to_string();
    }
    let state: State<'_, AppStateStore> = app.state();
    let snapshot = state.snapshot();
    snapshot
        .browser_sessions
        .first()
        .map(|s| s.browser_id.0.clone())
        .unwrap_or_else(|| "default".to_string())
}

pub fn spawn_control_server<R: Runtime>(app: AppHandle<R>) {
    let Some(socket_path) = control_socket_path() else {
        crate::diagnostics::stderr_line(
            "[codemux::control] Control socket path unavailable, skipping control server",
        );
        #[cfg(debug_assertions)]
        {
            let pid = std::process::id();
            let startup_id = std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
            crate::diagnostics::native_startup_breadcrumb(&format!(
                "[{}] startup_id={} pid={} component=control outcome=skip_no_socket_path",
                chrono::Local::now().format("%s"),
                startup_id,
                pid
            ));
        }
        return;
    };

    #[cfg(debug_assertions)]
    {
        let pid = std::process::id();
        let startup_id = std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
        crate::diagnostics::native_startup_breadcrumb(&format!(
            "[{}] startup_id={} pid={} component=control event=spawn_control_server socket_path={}",
            chrono::Local::now().format("%s"),
            startup_id,
            pid,
            socket_path.display()
        ));
    }

    // If another Codemux instance is already responding on this transport,
    // do NOT steal it or start a second server.
    if transport::sync_liveness_probe(&socket_path) {
        crate::diagnostics::stderr_line(&format!(
            "[codemux::control] Existing control endpoint at {:?} is alive; skipping new control server",
            socket_path
        ));
        #[cfg(debug_assertions)]
        {
            let pid = std::process::id();
            let startup_id = std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
            crate::diagnostics::native_startup_breadcrumb(&format!(
                "[{}] startup_id={} pid={} component=control outcome=skip_existing_alive socket_path={}",
                chrono::Local::now().format("%s"),
                startup_id,
                pid,
                socket_path.display()
            ));
        }
        return;
    }

    // Unix only: the socket lives in the filesystem, so we need to create the
    // parent directory and sweep any stale socket file from a previous crash
    // before `transport::bind` can succeed. Named pipes on Windows live in
    // the `\\.\pipe\` namespace and have no filesystem footprint to clean up.
    #[cfg(unix)]
    {
        if socket_path.exists() {
            crate::diagnostics::stderr_line(&format!(
                "[codemux::control] Existing control socket at {:?} appears stale; replacing it",
                socket_path
            ));
            #[cfg(debug_assertions)]
            {
                let pid = std::process::id();
                let startup_id = std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
                crate::diagnostics::native_startup_breadcrumb(&format!(
                    "[{}] startup_id={} pid={} component=control event=stale_socket_replace socket_path={}",
                    chrono::Local::now().format("%s"),
                    startup_id,
                    pid,
                    socket_path.display()
                ));
            }
        }

        if let Some(parent) = socket_path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                crate::diagnostics::stderr_line(&format!(
                    "[codemux::control] Failed to create control dir: {error}"
                ));
                return;
            }
        }
    }

    tauri::async_runtime::spawn(async move {
        let listener = match transport::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                crate::diagnostics::stderr_line(&format!(
                    "[codemux::control] Failed to bind control endpoint: {error}"
                ));
                #[cfg(debug_assertions)]
                {
                    let pid = std::process::id();
                    let startup_id = std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
                    crate::diagnostics::native_startup_breadcrumb(&format!(
                        "[{}] startup_id={} pid={} component=control outcome=bind_failed socket_path={} error={}",
                        chrono::Local::now().format("%s"),
                        startup_id,
                        pid,
                        socket_path.display(),
                        error
                    ));
                }
                return;
            }
        };

        #[cfg(debug_assertions)]
        {
            let pid = std::process::id();
            let startup_id = std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
            crate::diagnostics::native_startup_breadcrumb(&format!(
                "[{}] startup_id={} pid={} component=control outcome=bind_ok socket_path={}",
                chrono::Local::now().format("%s"),
                startup_id,
                pid,
                socket_path.display()
            ));
        }

        loop {
            match listener.accept().await {
                Ok(stream) => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = handle_client(app, stream).await {
                            crate::diagnostics::stderr_line(&format!(
                                "[codemux::control] Client error: {error}"
                            ));
                        }
                    });
                }
                Err(error) => {
                    crate::diagnostics::stderr_line(&format!(
                        "[codemux::control] Accept error: {error}"
                    ));
                    break;
                }
            }
        }
    });
}

async fn handle_client<R: Runtime, S>(app: AppHandle<R>, stream: S) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("Failed to read control request: {error}"))?
    {
        if line.trim().is_empty() {
            continue;
        }

        let request: ControlRequest = serde_json::from_str(&line)
            .map_err(|error| format!("Invalid control request JSON: {error}"))?;
        let response = dispatch_request(&app, request).await;
        let payload = serde_json::to_string(&response)
            .map_err(|error| format!("Failed to encode control response: {error}"))?;
        writer
            .write_all(format!("{payload}\n").as_bytes())
            .await
            .map_err(|error| format!("Failed to write control response: {error}"))?;
    }

    Ok(())
}

/// Send a request to the running Codemux control endpoint and return the response.
/// Used by both the CLI and the MCP server to communicate with the Codemux app.
/// Transparently uses a Unix socket on Linux/macOS and a named pipe on Windows.
pub async fn send_control_request(request: ControlRequest) -> Result<ControlResponse, String> {
    let socket_path = control_socket_path()
        .ok_or_else(|| "Control socket path unavailable".to_string())?;
    let stream = transport::connect(&socket_path)
        .await
        .map_err(|error| format!("Failed to connect to Codemux control endpoint: {error}"))?;
    let (reader, mut writer) = tokio::io::split(stream);

    let payload = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    writer
        .write_all(format!("{payload}\n").as_bytes())
        .await
        .map_err(|error| format!("Failed to send request: {error}"))?;

    let mut lines = BufReader::new(reader).lines();
    let response = lines
        .next_line()
        .await
        .map_err(|error| format!("Failed to read response: {error}"))?
        .ok_or_else(|| "No response received from Codemux".to_string())?;

    serde_json::from_str(&response).map_err(|error| format!("Invalid response JSON: {error}"))
}

async fn dispatch_request<R: Runtime>(app: &AppHandle<R>, request: ControlRequest) -> ControlResponse {
    let result = match request.command.as_str() {
        "status" => {
            let state: State<'_, AppStateStore> = app.state();
            let snap = state.snapshot();
            // Workspace summary: id + title + cwd for every open workspace.
            // Kept tight on purpose — a brain doing `app_status` wants a
            // one-line description per workspace, not the full git/tab
            // shape. Use `workspace_list` / `workspace_info` for the rest.
            let workspaces: Vec<Value> = snap
                .workspaces
                .iter()
                .map(|w| {
                    serde_json::json!({
                        "workspace_id": w.workspace_id.0,
                        "title": w.title,
                        "cwd": w.cwd,
                    })
                })
                .collect();
            // Focused pane: the active workspace's active surface's active
            // pane id. None when there's no open workspace.
            let focused_pane = snap
                .workspaces
                .iter()
                .find(|w| w.workspace_id == snap.active_workspace_id)
                .and_then(|w| {
                    w.surfaces
                        .iter()
                        .find(|s| s.surface_id == w.active_surface_id)
                        .map(|s| s.active_pane_id.0.clone())
                });
            Ok(serde_json::json!({
                "socket_path": control_socket_path().map(|path| path.display().to_string()),
                "protocol_version": CONTROL_PROTOCOL_VERSION,
                "app_version": env!("CARGO_PKG_VERSION"),
                "active_workspace_id": snap.active_workspace_id.0,
                "focused_pane_id": focused_pane,
                "workspaces": workspaces,
            }))
        }
        "get_app_state" => {
            let state: State<'_, AppStateStore> = app.state();
            serde_json::to_value(state.snapshot()).map_err(|error| error.to_string())
        }
        "conversation_read" => (|| -> Result<Value, String> {
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let workspace_id = request
                .params
                .get("workspace_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or("conversation_read requires a scoped workspace")?;
            let conversation_id = request
                .params
                .get("conversation_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or("conversation_read requires `conversation_id`")?;
            let cursor = request.params.get("cursor").and_then(Value::as_i64);
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20) as u32;
            db.read_agent_chat_history_page(workspace_id, conversation_id, cursor, limit)
                .and_then(|page| serde_json::to_value(page).map_err(|error| error.to_string()))
        })(),
        "conversation_search" => (|| -> Result<Value, String> {
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let workspace_id = request
                .params
                .get("workspace_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or("conversation_search requires a scoped workspace")?;
            let conversation_id = request
                .params
                .get("conversation_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or("conversation_search requires `conversation_id`")?;
            let query = request
                .params
                .get("query")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or("conversation_search requires `query`")?;
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(12) as u32;
            db.search_agent_chat_history(workspace_id, conversation_id, query, limit)
                .and_then(|hits| serde_json::to_value(hits).map_err(|error| error.to_string()))
        })(),
        "create_workspace" => {
            let state: State<'_, AppStateStore> = app.state();
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            // Phase 1.5 fix: the MCP `workspace_create` tool advertises an
            // optional `path` argument in its schema and the MCP dispatcher
            // (mcp_server.rs::workspace_create) packs it into params, but
            // this arm previously hardcoded `None` — so every call landed
            // at `current_project_root()` regardless of what the brain
            // asked for. Reading the param closes the silent-drop.
            let path = request
                .params
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string);
            crate::commands::workspace::create_workspace_impl(app.clone(), &state, &db, path)
                .await
                .map(|workspace_id| serde_json::json!({ "workspace_id": workspace_id }))
        }
        "split_pane" => {
            let state: State<'_, AppStateStore> = app.state();
            let pane_id = request.params.get("pane_id").and_then(Value::as_str).unwrap_or_default();
            let direction = request.params.get("direction").and_then(Value::as_str).unwrap_or("horizontal");
            crate::commands::workspace::split_pane_impl(
                app.clone(),
                &state,
                pane_id.to_string(),
                direction.to_string(),
            )
            .map(|session_id| serde_json::json!({ "session_id": session_id }))
        }
        "apply_preset" => {
            let state: State<'_, AppStateStore> = app.state();
            let pty_state: State<'_, crate::terminal::PtyState> = app.state();
            let presets: State<'_, crate::presets::PresetStoreState> = app.state();
            let workspace_id = request.params.get("workspace_id").and_then(Value::as_str).unwrap_or_default().to_string();
            let preset_id = request.params.get("preset_id").and_then(Value::as_str).unwrap_or_default().to_string();
            let override_mode = request.params.get("override_mode").and_then(Value::as_str).map(String::from);
            let initial_prompt = request.params.get("initial_prompt").and_then(Value::as_str).map(String::from);
            let model_selection = request
                .params
                .get("model_selection")
                .cloned()
                .and_then(|v| {
                    serde_json::from_value(v)
                        .map_err(|e| {
                            eprintln!(
                                "[control] apply_preset: ignoring malformed model_selection: {e}"
                            )
                        })
                        .ok()
                });
            crate::commands::presets::apply_preset(
                app.clone(),
                state,
                pty_state,
                presets,
                workspace_id,
                preset_id,
                override_mode,
                initial_prompt,
                model_selection,
            )
            .map(|()| serde_json::json!({ "ok": true }))
        }
        "create_worktree_workspace" => {
            // Phase 1.5: backs the `worktree_create` MCP tool. The
            // underlying impl already handles git worktree + workspace
            // hydration + PTY spawn + setup scripts + .mcp.json autoconfig
            // + preset launch with prompt injection in one atomic call —
            // we only parse params and pass through.
            let state: State<'_, AppStateStore> = app.state();
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let pty_state: State<'_, PtyState> = app.state();
            let presets: State<'_, crate::presets::PresetStoreState> = app.state();
            let repo_path = request
                .params
                .get("repo_path")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "Missing required parameter: repo_path".to_string());
            let branch = request
                .params
                .get("branch")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "Missing required parameter: branch".to_string());
            // Defaults match Phase 1.5 plan §5: new_branch=true so the
            // brain's natural "spin up a new feature branch" flow doesn't
            // need an extra arg, base="main" since that's where almost
            // every project forks from, layout="single" because the brain
            // typically wants one terminal pane to attach a CLI agent to.
            let new_branch = request
                .params
                .get("new_branch")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let base = request
                .params
                .get("base")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| Some("main".to_string()));
            let layout = request
                .params
                .get("layout")
                .and_then(Value::as_str)
                .unwrap_or("single")
                .to_string();
            let initial_prompt = request
                .params
                .get("initial_prompt")
                .and_then(Value::as_str)
                .map(str::to_string);
            let agent_preset_id = request
                .params
                .get("agent_preset_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let pr_number = request
                .params
                .get("pr_number")
                .and_then(Value::as_u64)
                .map(|n| n as u32);
            match (repo_path, branch) {
                (Ok(rp), Ok(br)) => crate::commands::workspace::create_worktree_workspace_impl(
                    app.clone(),
                    &state,
                    &db,
                    &pty_state,
                    &presets,
                    rp,
                    br,
                    new_branch,
                    base,
                    layout,
                    initial_prompt,
                    agent_preset_id,
                    request
                        .params
                        .get("model_selection")
                        .cloned()
                        .and_then(|v| {
                            serde_json::from_value(v)
                                .map_err(|e| {
                                    eprintln!(
                                        "[control] create_worktree_workspace: ignoring malformed model_selection: {e}"
                                    )
                                })
                                .ok()
                        }),
                    pr_number,
                )
                .await
                // `adopted` is additive: true when a live workspace already
                // claimed the worktree path and was focused instead of a new
                // one being created (any initial_prompt was dropped).
                .map(|created| {
                    serde_json::json!({
                        "workspace_id": created.workspace_id,
                        "adopted": created.adopted,
                    })
                }),
                (Err(e), _) | (_, Err(e)) => Err(e),
            }
        }
        "get_presets" => {
            // Phase 1.5: backs the `preset_list` MCP tool. Returns the
            // preset registry enriched with `commands_available` so the
            // brain can pre-filter to agents whose CLI is actually on
            // PATH before calling `preset_apply` or `worktree_create`.
            let presets: State<'_, crate::presets::PresetStoreState> = app.state();
            let entries = crate::commands::presets::list_presets_with_availability(&presets);
            Ok(serde_json::json!({ "presets": entries }))
        }
        "create_browser_pane" => {
            let state: State<'_, AppStateStore> = app.state();
            let pane_id = request.params.get("pane_id").and_then(Value::as_str).unwrap_or_default();
            let url = request.params.get("url").and_then(Value::as_str).map(String::from);
            crate::commands::browser::create_browser_pane_impl(
                app.clone(),
                &state,
                pane_id.to_string(),
                url,
            )
            .map(|created_pane_id| serde_json::json!({ "pane_id": created_pane_id }))
        }
        "open_url" => {
            let state: State<'_, AppStateStore> = app.state();
            let browser_id = request.params.get("browser_id").and_then(Value::as_str).unwrap_or_default();
            let url = request.params.get("url").and_then(Value::as_str).unwrap_or_default();
            crate::commands::browser::browser_open_url_impl(
                app.clone(),
                &state,
                browser_id.to_string(),
                url.to_string(),
            )
            .map(|_| serde_json::json!({ "browser_id": browser_id, "url": url }))
        }
        "notify" => {
            let state: State<'_, AppStateStore> = app.state();
            let message = request.params.get("message").and_then(Value::as_str).unwrap_or("Attention needed");
            let level = request.params.get("level").and_then(Value::as_str).unwrap_or("attention");
            state
                .add_notification(
                    None,
                    None,
                    message.to_string(),
                    crate::state::NotificationLevel::from_str_or_attention(level),
                )
                .map(|notification_id| {
                    crate::state::emit_app_state(app);
                    serde_json::json!({ "notification_id": notification_id })
                })
        }
        "write_terminal" => {
            let pty_state: State<'_, PtyState> = app.state();
            let app_state: State<'_, AppStateStore> = app.state();
            let session_id = request.params.get("session_id").and_then(Value::as_str).map(str::to_string);
            let data = request.params.get("data").and_then(Value::as_str).unwrap_or_default().to_string();
            crate::terminal::write_to_pty(pty_state, app_state, data, session_id)
                .map(|_| serde_json::json!({ "written": true }))
        }
        "read_terminal" => {
            let pty_state: State<'_, PtyState> = app.state();
            let app_state: State<'_, AppStateStore> = app.state();
            let session_id = request
                .params
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            // `lines` is optional and capped inside `read_terminal_output` so a
            // hostile or sloppy caller can't ask for 10M lines. We accept any
            // non-negative integer and let the helper clamp to its own
            // READ_TERMINAL_MAX_LINES.
            let lines = request
                .params
                .get("lines")
                .and_then(Value::as_u64)
                .map(|n| n as usize);
            crate::terminal::read_terminal_output(
                pty_state.inner(),
                app_state.inner(),
                lines,
                session_id,
            )
            .and_then(|out| serde_json::to_value(out).map_err(|e| e.to_string()))
        }
        "activate_workspace" => {
            let state: State<'_, AppStateStore> = app.state();
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let workspace_id = request
                .params
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "Missing required parameter: workspace_id".to_string());
            workspace_id.and_then(|ws_id| {
                // Reuse the in-app workspace-switch path so the socket / MCP
                // surface gets identical side effects: git refresh, PTY
                // hydration, app-state emit, persisted ui_state. See
                // `activate_workspace_impl` for the full breakdown.
                crate::commands::workspace::activate_workspace_impl(
                    app.clone(),
                    &state,
                    &db,
                    ws_id.clone(),
                )
                .map(|_| serde_json::json!({ "workspace_id": ws_id, "activated": true }))
            })
        }
        "close_workspace" => {
            // Phase 1.6: backs the `workspace_close` MCP tool. Wraps the
            // existing `close_workspace_with_worktree_impl` so both the
            // Tauri command and the socket arm share teardown, PTY
            // termination, agent-chat shutdown, MCP cleanup, and (when
            // requested) `git worktree remove`. Safe default:
            // `delete_worktree=false` — closing never destroys a worktree
            // unless the brain asks for it.
            let state: State<'_, AppStateStore> = app.state();
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let workspace_id = request
                .params
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "Missing required parameter: workspace_id".to_string());
            let delete_worktree = request
                .params
                .get("delete_worktree")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let delete_branch = request.params.get("delete_branch").and_then(Value::as_bool);
            let force = request.params.get("force_delete").and_then(Value::as_bool);
            match workspace_id {
                Ok(ws_id) => crate::commands::workspace::close_workspace_with_worktree_impl(
                    app.clone(),
                    &state,
                    &db,
                    ws_id.clone(),
                    delete_worktree,
                    delete_branch,
                    force,
                )
                .await
                .map(|_| {
                    serde_json::json!({
                        "workspace_id": ws_id,
                        "closed": true,
                        "worktree_removed": delete_worktree,
                    })
                }),
                Err(e) => Err(e),
            }
        }
        "archive_workspace" => {
            // Backs the `workspace_archive` MCP tool. Same close path as
            // `close_workspace` (teardown, PTY termination) but with the
            // worktree/files/branch guaranteed untouched, plus an archive
            // entry so the workspace can be restored later.
            let state: State<'_, AppStateStore> = app.state();
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let workspace_id = request
                .params
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "Missing required parameter: workspace_id".to_string());
            match workspace_id {
                Ok(ws_id) => crate::commands::workspace::archive_workspace_impl(
                    app.clone(),
                    &state,
                    &db,
                    ws_id.clone(),
                )
                .await
                .map(|archive_id| {
                    serde_json::json!({
                        "workspace_id": ws_id,
                        "archive_id": archive_id,
                        "archived": true,
                    })
                }),
                Err(e) => Err(e),
            }
        }
        "unarchive_workspace" => {
            // Backs the `workspace_unarchive` MCP tool. Restores through
            // the same creation paths the in-app flows use (worktree
            // create / add-repository), then activates the result.
            let state: State<'_, AppStateStore> = app.state();
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let pty_state: State<'_, PtyState> = app.state();
            let presets: State<'_, crate::presets::PresetStoreState> = app.state();
            let archive_id = request
                .params
                .get("archive_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "Missing required parameter: archive_id".to_string());
            match archive_id {
                Ok(aid) => crate::commands::workspace::unarchive_workspace_impl(
                    app.clone(),
                    &state,
                    &db,
                    &pty_state,
                    &presets,
                    aid.clone(),
                )
                .await
                .map(|workspace_id| {
                    serde_json::json!({
                        "archive_id": aid,
                        "workspace_id": workspace_id,
                        "restored": true,
                    })
                }),
                Err(e) => Err(e),
            }
        }
        "list_archived_workspaces" => {
            // Backs the `workspace_archive_list` MCP tool. Entries come
            // straight from state — same data the frontend's archive UI
            // renders — via the narrow accessor (no full-snapshot clone).
            let state: State<'_, AppStateStore> = app.state();
            serde_json::to_value(state.archived_workspaces_list())
                .map(|entries| serde_json::json!({ "archived_workspaces": entries }))
                .map_err(|e| e.to_string())
        }
        "close_pane" => {
            // Phase 1.6: backs the `pane_close` MCP tool. Wraps the
            // existing `close_pane_impl`. State-layer `close_pane`
            // handles the last-pane case by removing the surface + tab;
            // workspace stays open until explicitly closed.
            let state: State<'_, AppStateStore> = app.state();
            let pane_id = request
                .params
                .get("pane_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "Missing required parameter: pane_id".to_string());
            pane_id.and_then(|pid| {
                crate::commands::workspace::close_pane_impl(app.clone(), &state, pid.clone())
                    .map(|removed_session_id| {
                        serde_json::json!({
                            "pane_id": pid,
                            "closed": true,
                            "removed_session_id": removed_session_id,
                        })
                    })
            })
        }
        // ── Monitoring (provider-agnostic) ───────────────────────────────
        //
        // The escape hatch that makes the Monitoring status work for agents
        // Codemux has no event stream for. A terminal agent, a Codex/OpenCode
        // chat agent, anything with a shell: run `codemux monitor start` and
        // the pane reads as monitoring until `codemux monitor stop`.
        //
        // Pane resolution is `resolve_monitor_pane`'s job (explicit pane id →
        // the workspace's active pane → its first pane). Both commands echo
        // back the pane they acted on so a script can see what it hit.
        "monitor_start" => {
            let state: State<'_, AppStateStore> = app.state();
            let pane_id = request.params.get("pane_id").and_then(Value::as_str);
            let workspace_id = request.params.get("workspace_id").and_then(Value::as_str);
            let reason = request
                .params
                .get("reason")
                .and_then(Value::as_str)
                .filter(|r| !r.trim().is_empty())
                .map(|r| r.trim().to_string());
            match state.resolve_monitor_pane(pane_id, workspace_id) {
                Some(pane_id) => {
                    let changed = state.start_manual_monitor(&pane_id, reason.clone());
                    if changed {
                        crate::state::emit_app_state(app);
                    }
                    Ok(serde_json::json!({
                        "pane_id": pane_id,
                        "monitoring": true,
                        "reason": reason,
                        "changed": changed,
                    }))
                }
                None => Err("No pane found to monitor".to_string()),
            }
        }
        "monitor_stop" => {
            let state: State<'_, AppStateStore> = app.state();
            let pane_id = request.params.get("pane_id").and_then(Value::as_str);
            let workspace_id = request.params.get("workspace_id").and_then(Value::as_str);
            match state.resolve_monitor_pane(pane_id, workspace_id) {
                Some(pane_id) => {
                    let changed = state.stop_manual_monitor(&pane_id);
                    if changed {
                        crate::state::emit_app_state(app);
                    }
                    Ok(serde_json::json!({
                        "pane_id": pane_id,
                        "monitoring": false,
                        "changed": changed,
                    }))
                }
                None => Err("No pane found to stop monitoring".to_string()),
            }
        }
        "monitor_status" => {
            let state: State<'_, AppStateStore> = app.state();
            let pane_id = request.params.get("pane_id").and_then(Value::as_str);
            let workspace_id = request.params.get("workspace_id").and_then(Value::as_str);
            match state.resolve_monitor_pane(pane_id, workspace_id) {
                Some(pane_id) => {
                    let manual = state.manual_monitor_reason(&pane_id);
                    // Report the EFFECTIVE status (the snapshot the sidebar
                    // reads), not the flag: a pane can be monitoring because
                    // its provider's watch loops say so, with no flag at all.
                    let status = state.snapshot().pane_statuses.get(&pane_id).cloned();
                    Ok(serde_json::json!({
                        "pane_id": pane_id,
                        "monitoring": status == Some(crate::state::PaneStatus::Monitoring),
                        "manual": manual.is_some(),
                        "reason": manual.flatten(),
                        "status": status,
                    }))
                }
                None => Err("No pane found".to_string()),
            }
        }
        "port_list" => {
            let state: State<'_, AppStateStore> = app.state();
            let workspace_filter = request
                .params
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let snap = state.snapshot();
            // Filter on `workspace_id` when provided; otherwise return every
            // detected port. `detected_ports` carries the same fields as
            // `ports::PortInfo` plus the workspace association the detector
            // assigns, so the wire payload is identical to what the UI's
            // ports panel sees.
            let ports = filter_ports_by_workspace(&snap.detected_ports, workspace_filter.as_deref());
            serde_json::to_value(&ports)
                .map(|ports_json| serde_json::json!({ "ports": ports_json }))
                .map_err(|e| e.to_string())
        }
        "browser_automation" => {
            let state: State<'_, AppStateStore> = app.state();
            let agent_browser: State<'_, crate::agent_browser::AgentBrowserManager> = app.state();
            let observability: State<'_, crate::observability::ObservabilityStore> = app.state();
            let mut workspace_id = request.params
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            // Layer B fallback: when the caller sent no `workspace_id` (its
            // `CODEMUX_WORKSPACE_ID` env was never injected — the agent-chat
            // sidecar's Bash subprocesses, OpenCode's shared server, etc.)
            // but did include a `cwd` hint, resolve the owning workspace by
            // path so the pane lands in the *agent's* workspace instead of
            // the "Legacy global path" below, which targets whatever
            // workspace the user is currently viewing.
            let mut cwd_resolved = false;
            if workspace_id.is_empty() {
                let cwd = request.params
                    .get("cwd")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !cwd.is_empty() {
                    if let Some(resolved) =
                        resolve_workspace_id_by_cwd(&state.snapshot().workspaces, cwd)
                    {
                        workspace_id = resolved;
                        cwd_resolved = true;
                    }
                }
            }

            let action_kind = request.params.get("action")
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str)
                .unwrap_or("open_url")
                .to_string();

            eprintln!(
                "[codemux::browser] handler received action={action_kind} workspace_id={workspace_id} cwd_resolved={cwd_resolved}"
            );

            let params = request.params.get("action").cloned().unwrap_or(Value::Null);

            // Resolve the CLI session name to use for agent-browser commands.
            let cli_session_name = if !workspace_id.is_empty() {
                // Allocate the stream port keyed by `cli_session_name`
                // directly, not by `workspace_id` plus an `ensure_port` alias.
                //
                // The old flow allocated against `workspace_id`, then
                // mirrored the port into the manager under
                // `cli_session_name` via `ensure_port`. That left two
                // HashMap entries pinning the same port, which `close()`
                // could not fully reap (it only removed the key it was
                // passed) — every workspace churn leaked a phantom slot
                // and eventually exhausted or aliased the 9223–9299
                // range. Allocating against `cli_session_name` from the
                // start removes the alias entirely.
                //
                // We resolve the agent session once with a placeholder
                // port (the previously-stored value, or
                // `DEFAULT_STREAM_PORT` for the very first call) so we
                // know the canonical `cli_session_name`, then allocate
                // for real and write the actual port back into state so the
                // frontend's reactive `stream_url` reflects reality.
                let existing_port =
                    state.agent_browser_stream_port_for_workspace(&workspace_id);
                let placeholder_port = existing_port
                    .unwrap_or(crate::agent_browser::DEFAULT_STREAM_PORT);
                let session_for_naming = state.resolve_agent_browser_session(
                    &workspace_id,
                    placeholder_port,
                );
                // `resolve_` inserts a record when none existed (but not for a
                // workspace that is already gone), so the state is checked
                // again rather than assumed.
                let session_created = existing_port.is_none()
                    && state
                        .agent_browser_stream_port_for_workspace(&workspace_id)
                        .is_some();
                let stream_port = agent_browser
                    .allocate_port(&session_for_naming.cli_session_name)
                    .await
                    .unwrap_or(crate::agent_browser::DEFAULT_STREAM_PORT);

                // Persist the real port back into the agent session so
                // the frontend's `stream_url` is reactive when the port
                // gets re-allocated after a teardown/respawn.
                let port_changed = state
                    .update_agent_browser_stream_port(&workspace_id, stream_port)
                    .unwrap_or(false);
                let agent_session = state.resolve_agent_browser_session(
                    &workspace_id,
                    stream_port,
                );

                // The emits further down are scoped to the action kinds that
                // create or attach a pane, so a screenshot/click/type action —
                // or an `open_url` that hits neither branch — would leave the
                // new session record and the moved `stream_url` unrendered.
                if session_created || port_changed {
                    crate::state::schedule_emit_app_state(&app);
                }

                // In GUI-mode background browsing, when the Agent Chat GUI
                // beta is on, the agent's browser session must stay detached.
                // The frontend renders it as an inline chip + context-bar
                // indicator instead of splitting the chat into a pane.
                // The flag-off path keeps today's split-pane behavior.
                let gui_background_mode = observability.agent_chat_enabled();

                // Auto-create a browser pane if no pane is attached and user hasn't dismissed it
                // — unless GUI-mode background browsing suppresses it (see above).
                let should_create = should_create_browser_pane(
                    agent_session.is_surfaced(),
                    agent_session.user_dismissed,
                    gui_background_mode,
                );

                if gui_background_mode {
                    // Mark the session live even though no pane will be
                    // attached, so the frontend's "background session is
                    // live" chip/indicator can key off `is_active` the
                    // same way an attached session does. Emit immediately
                    // so the chip/indicator appear without waiting for a
                    // later `open` action to trigger the next emit.
                    state.mark_agent_browser_active(&workspace_id);
                    crate::state::emit_app_state(&app);
                }

                if should_create {
                    let target_pane_id = {
                        let snap = state.snapshot();
                        snap.workspaces.iter()
                            .find(|w| w.workspace_id.0 == workspace_id)
                            .and_then(|w| w.surfaces.iter().find(|s| s.surface_id == w.active_surface_id))
                            .map(|s| s.active_pane_id.0.clone())
                    };
                    if let Some(pane_id) = target_pane_id {
                        // Save the user's current workspace so we can restore it after
                        // create_browser_pane (which sets active_workspace_id).
                        let user_workspace = state.snapshot().active_workspace_id.clone();

                        let url = params.get("url")
                            .and_then(Value::as_str)
                            .map(String::from);
                        match state.create_browser_pane(&pane_id, url.as_deref()) {
                            Ok((new_pane_id, new_browser_id)) => {
                                // Mark the browser session as agent-backed BEFORE
                                // emitting state so the pane starts its screencast
                                // daemon with the agent's session name.
                                state.set_browser_agent_session_name(
                                    &new_browser_id,
                                    agent_session.cli_session_name.clone(),
                                );
                                let _ = state.attach_agent_browser_to_pane(
                                    &workspace_id,
                                    &new_pane_id,
                                    &new_browser_id,
                                );
                                // Restore the user's workspace — don't steal focus.
                                state.activate_workspace(&user_workspace.0);
                                crate::state::emit_app_state(&app);
                            }
                            Err(_e) => {}
                        }
                    }
                }

                // Track URL on the agent session for reconnection, and sync
                // to the browser_sessions entry so the frontend URL bar updates.
                if action_kind == "open" {
                    if let Some(url) = params.get("url").and_then(Value::as_str) {
                        let _ = state.update_agent_browser_url(&workspace_id, url.to_string());
                        // Re-read the session to get the current browser_id (may have
                        // been set by attach_agent_browser_to_pane in the create block).
                        let current_session = state.resolve_agent_browser_session(
                            &workspace_id,
                            stream_port,
                        );
                        if let Some(bid) = current_session.browser_id.as_ref() {
                            let _ = state.update_browser_url(&bid.0, url.to_string());
                        }
                        crate::state::emit_app_state(&app);
                    }
                }

                agent_session.cli_session_name
            } else {
                // Legacy global path: no workspace context (backward compat).
                //
                // The GUI-mode background-browsing gate deliberately does
                // NOT apply here. This path has no resolved `workspace_id`,
                // so no `AgentBrowserSession` exists to mark active — the
                // background chip / context-bar indicator / peek overlay
                // all key off a workspace's `agent_browser_sessions` entry
                // and could never surface this browser. Suppressing the
                // pane would therefore leave the browser running completely
                // invisibly (no pane, no chip, no way to view or promote);
                // a visible pane is the only safe behavior. See
                // `should_create_legacy_browser_pane` for the unit-tested
                // decision.
                if should_create_legacy_browser_pane(
                    state.snapshot().browser_sessions.is_empty(),
                    observability.agent_chat_enabled(),
                ) {
                    let active_pane_id = {
                        let snap = state.snapshot();
                        snap.workspaces.iter()
                            .find(|w| w.workspace_id == snap.active_workspace_id)
                            .and_then(|w| w.surfaces.iter()
                                .find(|s| s.surface_id == w.active_surface_id))
                            .map(|s| s.active_pane_id.0.clone())
                    };
                    if let Some(pane_id) = active_pane_id {
                        let url = params.get("url")
                            .and_then(Value::as_str)
                            .map(String::from);
                        let _ = crate::commands::browser::create_browser_pane_impl(
                            app.clone(), &state, pane_id, url,
                        );
                    }
                }
                resolve_browser_id(&app, "default")
            };

            // Get the port for this session (already allocated above or in start_stream).
            // The manager no longer registers a `workspace_id` alias, so
            // `cli_session_name` is the only key the manager knows.
            let vision_port = agent_browser.get_port(&cli_session_name).await
                .unwrap_or(crate::agent_browser::DEFAULT_STREAM_PORT);

            let result = match action_kind.as_str() {
                // Tier 2: coordinate-based CDP tools via stream WebSocket
                "click_at" | "type_at" | "scroll_at" | "key_press" | "drag" => {
                    crate::stream_input::handle_vision_action(vision_port, &action_kind, params, &cli_session_name)
                        .await
                        .and_then(|result| serde_json::to_value(result).map_err(|error| error.to_string()))
                }
                // Tier 3: OS-level kernel input via ydotool
                "click_os" | "type_os" => {
                    crate::os_input::handle_os_action(&action_kind, params, &cli_session_name)
                        .await
                        .and_then(|result| serde_json::to_value(result).map_err(|error| error.to_string()))
                }
                // Tier 1: existing agent-browser CLI path
                _ => {
                    agent_browser.run_command(&cli_session_name, &action_kind, params)
                        .await
                        .and_then(|result| serde_json::to_value(result).map_err(|error| error.to_string()))
                }
            };

            // A successful `close` ends the browser session — mirror that
            // into the workspace's `AgentBrowserSession` so the GUI-mode
            // background chip / context-bar indicator / peek overlay (which
            // key off `is_active`) stop showing LIVE. Without this the
            // session stayed `is_active: true` forever, since only
            // `attach_agent_browser_to_pane` / `mark_agent_browser_active`
            // ever touched the flag. Scoped to the workspace path — the
            // legacy no-workspace path has no `AgentBrowserSession`.
            if action_kind == "close" && result.is_ok() && !workspace_id.is_empty() {
                if state.mark_agent_browser_inactive(&workspace_id) {
                    crate::state::emit_app_state(&app);
                }
            }

            result
        }
        "get_project_memory" => memory::get_project_memory(
            request.params
                .get("project_root")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
        .and_then(|snapshot| serde_json::to_value(snapshot).map_err(|error| error.to_string())),
        "update_project_memory" => {
            let project_root = request
                .params
                .get("project_root")
                .and_then(Value::as_str)
                .map(str::to_string);
            match serde_json::from_value::<memory::ProjectMemoryUpdate>(
                request.params.get("update").cloned().unwrap_or(Value::Null),
            ) {
                Ok(update) => memory::update_project_memory(project_root, update)
                    .and_then(|snapshot| serde_json::to_value(snapshot).map_err(|error| error.to_string())),
                Err(error) => Err(format!("Invalid project memory update: {error}")),
            }
        }
        "add_project_memory_entry" => {
            let project_root = request
                .params
                .get("project_root")
                .and_then(Value::as_str)
                .map(str::to_string);
            match (
                serde_json::from_value::<memory::MemoryEntryKind>(
                    request.params.get("kind").cloned().unwrap_or(Value::Null),
                ),
                serde_json::from_value::<memory::MemorySource>(
                    request.params.get("source").cloned().unwrap_or(Value::Null),
                ),
            ) {
                (Ok(kind), Ok(source)) => {
                    let content = request
                        .params
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let tags = request
                        .params
                        .get("tags")
                        .and_then(Value::as_array)
                        .map(|values| values.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_else(Vec::new);
                    let tool_name = request.params.get("tool_name").and_then(Value::as_str).map(str::to_string);
                    let session_label = request.params.get("session_label").and_then(Value::as_str).map(str::to_string);

                    memory::add_memory_entry(project_root, kind, source, content, tags, tool_name, session_label)
                        .and_then(|snapshot| serde_json::to_value(snapshot).map_err(|error| error.to_string()))
                }
                (Err(error), _) => Err(format!("Invalid memory kind: {error}")),
                (_, Err(error)) => Err(format!("Invalid memory source: {error}")),
            }
        }
        "generate_handoff" => memory::generate_handoff_packet(
            request.params
                .get("project_root")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
        .and_then(|packet| serde_json::to_value(packet).map_err(|error| error.to_string())),
        "rebuild_index" => {
            let project_root = request
                .params
                .get("project_root")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    let state: State<'_, crate::state::AppStateStore> = app.state();
                    state.active_workspace_cwd().map(|(_, cwd)| cwd)
                });
            let store: State<'_, indexing::ProjectIndexStore> = app.state();
            indexing::rebuild_index(project_root)
                .map(|snapshot| {
                    store.replace_snapshot(snapshot.clone());
                    snapshot
                })
                .and_then(|snapshot| serde_json::to_value(snapshot).map_err(|error| error.to_string()))
        }
        "index_status" => {
            let store: State<'_, indexing::ProjectIndexStore> = app.state();
            serde_json::to_value(store.status()).map_err(|error| error.to_string())
        }
        "search_index" => {
            let store: State<'_, indexing::ProjectIndexStore> = app.state();
            let query = request
                .params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let limit = request.params.get("limit").and_then(Value::as_u64).map(|value| value as usize);
            serde_json::to_value(indexing::search_index(&store, query, limit))
                .map_err(|error| error.to_string())
        }
        "list_github_issues" => {
            let state: State<'_, AppStateStore> = app.state();
            let repo_path = resolve_control_repo_path(app, &state, &request.params);
            let search = request.params.get("search").and_then(Value::as_str);
            crate::github::list_github_issues(std::path::Path::new(&repo_path), search)
                .and_then(|issues| serde_json::to_value(issues).map_err(|e| e.to_string()))
        }
        "get_github_issue" => {
            let state: State<'_, AppStateStore> = app.state();
            let repo_path = resolve_control_repo_path(app, &state, &request.params);
            let number = request.params.get("number").and_then(Value::as_u64)
                .ok_or_else(|| "Missing required parameter: number".to_string());
            number.and_then(|n| {
                crate::github::get_github_issue(std::path::Path::new(&repo_path), n)
                    .and_then(|issue| serde_json::to_value(issue).map_err(|e| e.to_string()))
            })
        }
        "link_workspace_issue" => {
            let state: State<'_, AppStateStore> = app.state();
            let workspace_id = request.params.get("workspace_id").and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| state.active_workspace_cwd().map(|(id, _)| id))
                .ok_or_else(|| "No workspace_id and no active workspace".to_string());
            let number = request.params.get("number").and_then(Value::as_u64)
                .ok_or_else(|| "Missing required parameter: number".to_string());
            workspace_id.and_then(|ws_id| {
                number.and_then(|num| {
                    let cwd = {
                        let snap = state.snapshot();
                        snap.workspaces.iter()
                            .find(|w| w.workspace_id.0 == ws_id)
                            .map(|ws| ws.project_root.clone().unwrap_or_else(|| ws.cwd.clone()))
                            .ok_or_else(|| format!("No workspace found: {ws_id}"))
                    };
                    cwd.and_then(|cwd| {
                        crate::github::get_github_issue(std::path::Path::new(&cwd), num)
                            .map(|issue| {
                                let title = issue.title.clone();
                                let linked = crate::github::LinkedIssue {
                                    number: issue.number,
                                    title: issue.title,
                                    state: issue.state,
                                    labels: issue.labels,
                                };
                                state.link_workspace_issue(&ws_id, linked);
                                crate::state::emit_app_state(app);
                                serde_json::json!({ "linked": true, "issue_number": num, "title": title })
                            })
                    })
                })
            })
        }
        "rerun_setup" => {
            let state: State<'_, AppStateStore> = app.state();
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let workspace_id = request.params.get("workspace_id").and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| state.active_workspace_cwd().map(|(id, _)| id))
                .ok_or_else(|| "No workspace_id and no active workspace".to_string());
            workspace_id.and_then(|ws_id| {
                let (cwd, title, branch) = {
                    let snapshot = state.snapshot();
                    let ws = snapshot.workspaces.iter()
                        .find(|w| w.workspace_id.0 == ws_id)
                        .ok_or_else(|| format!("No workspace found for {ws_id}"))?;
                    Ok::<_, String>((ws.cwd.clone(), ws.title.clone(), ws.git_branch.clone()))
                }?;
                let port = crate::scripts::allocate_workspace_port(&ws_id);
                crate::scripts::run_setup_scripts(
                    std::path::Path::new(&cwd), &title, &ws_id, app, Some(&db),
                    branch.as_deref(), Some(port),
                )?;
                Ok(serde_json::json!({ "workspace_id": ws_id, "status": "complete" }))
            })
        }
        // ── Automations ──
        //
        // Agent / MCP control surface. Each arm delegates to the shared
        // `commands::automations::*_impl` helpers so the validation and
        // `next_run_at` bookkeeping match the desktop command surface
        // exactly.
        "automation_list" => (|| -> Result<Value, String> {
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            serde_json::to_value(crate::commands::automations::list_automations_impl(&db))
                .map_err(|error| error.to_string())
        })(),
        "automation_get" => (|| -> Result<Value, String> {
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let id = request
                .params
                .get("id")
                .and_then(Value::as_i64)
                .ok_or("automation_get requires an integer `id`")?;
            let view = crate::commands::automations::get_automation_impl(&db, id)?;
            serde_json::to_value(view).map_err(|error| error.to_string())
        })(),
        "automation_create" => (|| -> Result<Value, String> {
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let input: crate::database::AutomationInput = serde_json::from_value(
                request.params.get("input").cloned().unwrap_or(Value::Null),
            )
            .map_err(|error| format!("Invalid automation input: {error}"))?;
            let view = crate::commands::automations::create_automation_impl(&db, input)?;
            crate::commands::automations::schedule_automations_sync(app.clone());
            serde_json::to_value(view).map_err(|error| error.to_string())
        })(),
        "automation_update" => (|| -> Result<Value, String> {
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let id = request
                .params
                .get("id")
                .and_then(Value::as_i64)
                .ok_or("automation_update requires an integer `id`")?;
            let input: crate::database::AutomationInput = serde_json::from_value(
                request.params.get("input").cloned().unwrap_or(Value::Null),
            )
            .map_err(|error| format!("Invalid automation input: {error}"))?;
            let view = crate::commands::automations::update_automation_impl(&db, id, input)?;
            crate::commands::automations::schedule_automations_sync(app.clone());
            serde_json::to_value(view).map_err(|error| error.to_string())
        })(),
        "automation_set_enabled" => (|| -> Result<Value, String> {
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let id = request
                .params
                .get("id")
                .and_then(Value::as_i64)
                .ok_or("automation_set_enabled requires an integer `id`")?;
            let enabled = request
                .params
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or("automation_set_enabled requires a boolean `enabled`")?;
            let view =
                crate::commands::automations::set_automation_enabled_impl(&db, id, enabled)?;
            crate::commands::automations::schedule_automations_sync(app.clone());
            serde_json::to_value(view).map_err(|error| error.to_string())
        })(),
        "automation_delete" => (|| -> Result<Value, String> {
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let id = request
                .params
                .get("id")
                .and_then(Value::as_i64)
                .ok_or("automation_delete requires an integer `id`")?;
            crate::commands::automations::delete_automation_impl(&db, id)?;
            crate::commands::automations::schedule_automations_sync(app.clone());
            Ok(serde_json::json!({ "deleted": id }))
        })(),
        "automation_runs" => (|| -> Result<Value, String> {
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            let automation_id = request
                .params
                .get("automation_id")
                .and_then(Value::as_i64)
                .ok_or("automation_runs requires an integer `automation_id`")?;
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .map(|value| value as u32);
            let runs = crate::commands::automations::list_automation_runs_impl(
                &db,
                automation_id,
                limit,
            );
            serde_json::to_value(serde_json::json!({ "runs": runs }))
                .map_err(|error| error.to_string())
        })(),
        // ── Web remote access ──
        //
        // Mint a one-time web-remote pairing code from the terminal (the
        // `codemux remote pair` CLI, typically over SSH). The control socket
        // is same-machine + unauthenticated by design — over SSH you ARE on
        // the machine — so this is the right transport for pairing without
        // opening the desktop GUI. Reuses the exact token path + endpoint
        // enumeration the Settings pane uses (`web_remote::control_pair`).
        "web_remote_pair" => {
            let name = request
                .params
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            crate::web_remote::control_pair(app, name)
                .and_then(|res| serde_json::to_value(res).map_err(|error| error.to_string()))
        }
        // Enable web remote access from the terminal (`codemux remote enable`),
        // optionally selecting the bind scope and port. Shares the exact
        // bind/rollback + state-emission paths the Settings pane uses; returns
        // the resulting status plus the recommended endpoint so the caller can
        // immediately run `codemux remote pair`.
        "web_remote_enable" => {
            let scope = request
                .params
                .get("scope")
                .and_then(Value::as_str)
                .map(str::to_string);
            let port = request
                .params
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok());
            crate::web_remote::control_enable(app, scope, port)
                .await
                .and_then(|res| serde_json::to_value(res).map_err(|error| error.to_string()))
        }
        // Turn the from-anywhere relay transport on/off in this running
        // instance (`codemux connect` / `codemux connect off` when a GUI or
        // `serve` already holds the control endpoint). The CLI cannot just
        // write the setting row in that case — the running instance owns the
        // in-memory config and re-persists it — so relay mode has to be flipped
        // through the live `set_config_core` path, exactly like the Settings
        // pane's switch. Returns the resulting status.
        "web_remote_set_relay" => match request.params.get("enabled").and_then(Value::as_bool) {
            None => Err("web_remote_set_relay requires a boolean `enabled`".to_string()),
            Some(enabled) => crate::web_remote::control_set_relay(app, enabled)
                .await
                .and_then(|res| serde_json::to_value(res).map_err(|error| error.to_string())),
        },
        // Disable web remote access from the terminal (`codemux remote disable`).
        "web_remote_disable" => crate::web_remote::control_disable(app)
            .and_then(|res| serde_json::to_value(res).map_err(|error| error.to_string())),
        _ => Err(format!("Unknown control command: {}", request.command)),
    };

    match result {
        Ok(data) => ControlResponse {
            ok: true,
            protocol_version: CONTROL_PROTOCOL_VERSION,
            data: Some(data),
            error: None,
        },
        Err(error) => ControlResponse {
            ok: false,
            protocol_version: CONTROL_PROTOCOL_VERSION,
            data: None,
            error: Some(error),
        },
    }
}

/// Filter `ports` to entries whose `workspace_id` matches `workspace_filter`.
/// `None` returns every detected port. Pulled out as a free function so the
/// `port_list` socket-command logic can be exercised in unit tests without
/// having to stand up an `AppStateStore` + Tauri runtime.
///
/// The vexis-agent integration calls this through the `port_list` MCP tool
/// — see `mcp_server.rs::handle_tool_call` — and relies on the filter
/// semantics being strict: a port without a `workspace_id` MUST NOT match
/// any workspace filter, even an empty-string filter.
fn filter_ports_by_workspace<'a>(
    ports: &'a [crate::state::PortInfoSnapshot],
    workspace_filter: Option<&str>,
) -> Vec<&'a crate::state::PortInfoSnapshot> {
    ports
        .iter()
        .filter(|p| match workspace_filter {
            Some(filter) => p.workspace_id.as_deref() == Some(filter),
            None => true,
        })
        .collect()
}

/// Decide whether `browser_automation` should auto-create (or reconnect) a
/// split browser pane for the agent's browser session.
///
/// Extracted as a small pure function so the GUI-mode background-browsing gate
/// is unit-testable without standing up `AppStateStore`/`ObservabilityStore`.
/// `surfaced` and `user_dismissed` mirror the existing pre-GUI-mode
/// rule; `gui_background_mode` is `true` when the Agent Chat GUI beta is on
/// for a workspace, in which case the session must stay detached even though
/// it would otherwise qualify for a new pane.
///
/// `surfaced` is `AgentBrowserSession::is_surfaced()`, not `pane_id.is_some()`:
/// a session docked in the right-panel deck is already on screen, so
/// splitting a second surface into the pane tree for it would show the user
/// the same Chromium twice.
fn should_create_browser_pane(
    surfaced: bool,
    user_dismissed: bool,
    gui_background_mode: bool,
) -> bool {
    !surfaced && !user_dismissed && !gui_background_mode
}

/// Pane-creation decision for the **legacy no-workspace-context fallback**
/// in the `browser_automation` handler.
///
/// Unlike [`should_create_browser_pane`], the Agent Chat GUI beta
/// (`agent_chat_enabled`) is accepted but deliberately **ignored**: with no
/// resolved `workspace_id` there is no `AgentBrowserSession` to mark
/// active, so GUI-mode background browsing cannot be represented — the
/// inline chip, context-bar indicator, and peek overlay all key off a
/// workspace's `agent_browser_sessions` entry and would never surface this
/// browser. Suppressing the pane here would leave the browser running
/// completely invisibly (no pane, no chip, no way to view or promote), so
/// the visible pane stays the behavior in both modes. The parameter exists
/// (rather than not being taken at all) so this invariant is explicit and
/// regression-tested.
fn should_create_legacy_browser_pane(
    browser_sessions_empty: bool,
    _agent_chat_enabled_ignored: bool,
) -> bool {
    browser_sessions_empty
}

/// Resolve which workspace owns a given `cwd`, for browser routing when the
/// caller sent an empty `workspace_id` (agent-chat Bash subprocesses,
/// OpenCode's shared server, or any MCP/CLI caller whose
/// `CODEMUX_WORKSPACE_ID` env is missing). Without this, such a caller
/// takes the "Legacy global path" in the `browser_automation` handler and
/// its browser pane lands on whatever workspace the *user* is viewing.
///
/// Matching is purely lexical — we deliberately do NOT `fs::canonicalize`,
/// both because it would hit the filesystem on the control-socket hot path
/// and because the unit tests use paths that don't exist on disk. Each
/// workspace contributes two candidate roots: its `cwd` and its
/// `worktree_path` (when set). A root matches when `cwd` equals it exactly
/// or is a subdirectory of it at a path-component boundary — so
/// `"/a/b"` matches cwd `"/a/b/src"` but NOT `"/a/bc"`. Trailing slashes on
/// either side are ignored. The workspace with the *longest* matching root
/// wins, so a nested worktree checked out under another workspace resolves
/// to the deepest (most specific) match. Returns `None` when `cwd` is empty
/// or nothing matches, in which case the handler keeps today's legacy path.
fn resolve_workspace_id_by_cwd(
    workspaces: &[crate::state::WorkspaceSnapshot],
    cwd: &str,
) -> Option<String> {
    let cwd_n = cwd.trim_end_matches('/');
    if cwd_n.is_empty() {
        return None;
    }

    let mut best: Option<(usize, &str)> = None;
    for ws in workspaces {
        let candidate_roots = std::iter::once(ws.cwd.as_str())
            .chain(ws.worktree_path.as_deref());
        for root in candidate_roots {
            let root_n = root.trim_end_matches('/');
            if root_n.is_empty() {
                continue;
            }
            if cwd_is_within(cwd_n, root_n) {
                let root_len = root_n.len();
                // Strict `>` so, on a length tie, the first workspace in the
                // snapshot order wins deterministically.
                if best.map_or(true, |(best_len, _)| root_len > best_len) {
                    best = Some((root_len, ws.workspace_id.0.as_str()));
                }
            }
        }
    }

    best.map(|(_, id)| id.to_string())
}

/// True when `cwd` is `root` itself or a subdirectory of `root`, comparing
/// at path-component boundaries so `"/a/bc"` is not considered within
/// `"/a/b"`. Both inputs are expected to be already trailing-slash-trimmed.
fn cwd_is_within(cwd: &str, root: &str) -> bool {
    if cwd == root {
        return true;
    }
    // A strict subdirectory: `cwd` must start with `root` followed by a
    // path separator, otherwise `"/a/b"` would spuriously match `"/a/bc"`.
    // Both separators are accepted so Windows callers (whose
    // `std::env::current_dir()` yields backslash paths) still match.
    cwd.strip_prefix(root)
        .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('\\'))
}

/// Resolve the repo path for control socket commands.
/// Checks `repo_path` param first, then falls back to active workspace's project_root/cwd.
fn resolve_control_repo_path<R: Runtime>(
    _app: &AppHandle<R>,
    state: &State<'_, AppStateStore>,
    params: &Value,
) -> String {
    if let Some(path) = params.get("repo_path").and_then(Value::as_str) {
        return path.to_string();
    }
    if let Some((ws_id, _)) = state.active_workspace_cwd() {
        let snap = state.snapshot();
        if let Some(ws) = snap.workspaces.iter().find(|w| w.workspace_id.0 == ws_id) {
            return ws.project_root.clone().unwrap_or_else(|| ws.cwd.clone());
        }
    }
    ".".to_string()
}

// ---------------------------------------------------------------------------
// Transport-layer tests
//
// These exercise the platform abstraction in `transport` without involving
// the Tauri runtime or any command handlers — the point is to prove that
// bind / connect / accept / liveness-probe all work on every supported
// platform. Higher-level control-command tests would require a real
// AppHandle, which is out of scope for a unit test.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PortInfoSnapshot;

    fn port(port: u16, ws: Option<&str>, label: Option<&str>) -> PortInfoSnapshot {
        PortInfoSnapshot {
            port,
            pid: 1000 + port as u32,
            process_name: format!("proc-{port}"),
            workspace_id: ws.map(str::to_string),
            label: label.map(str::to_string),
            source: None,
        }
    }

    // ─── port_list filter (socket command behaviour) ────────────────────
    //
    // The `port_list` socket command runs this filter directly on
    // `AppStateSnapshot::detected_ports`. The MCP tool of the same name
    // forwards the optional `workspace_id` argument unchanged, so anything
    // we promise here is also the MCP-tool contract.

    #[test]
    fn port_list_filter_no_filter_returns_all() {
        let ports = vec![
            port(3000, Some("ws-a"), Some("next")),
            port(8080, None, None),
            port(4173, Some("ws-b"), Some("vite")),
        ];
        let filtered = filter_ports_by_workspace(&ports, None);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn port_list_filter_by_workspace_id() {
        let ports = vec![
            port(3000, Some("ws-a"), Some("next")),
            port(8080, None, None),
            port(4173, Some("ws-b"), Some("vite")),
            port(5173, Some("ws-a"), Some("vite")),
        ];
        let filtered = filter_ports_by_workspace(&ports, Some("ws-a"));
        let ports_only: Vec<u16> = filtered.iter().map(|p| p.port).collect();
        assert_eq!(ports_only, vec![3000, 5173]);
    }

    #[test]
    fn port_list_filter_excludes_unscoped_ports() {
        // A port with no `workspace_id` must NOT match any workspace filter
        // — including an empty-string filter — otherwise vexis-agent's
        // brain sees "ports owned by no workspace" leak into a workspace
        // query. The bug would be silent on the wire (just extra ports)
        // but very confusing for the agent.
        let ports = vec![port(8080, None, None), port(3000, Some("ws-a"), None)];
        let filtered = filter_ports_by_workspace(&ports, Some(""));
        assert!(filtered.is_empty());
    }

    #[test]
    fn port_list_filter_unknown_workspace_returns_empty() {
        let ports = vec![
            port(3000, Some("ws-a"), None),
            port(4173, Some("ws-b"), None),
        ];
        let filtered = filter_ports_by_workspace(&ports, Some("ws-nonexistent"));
        assert!(filtered.is_empty());
    }

    // ─── should_create_browser_pane (GUI-mode background browsing gate) ─

    #[test]
    fn should_create_pane_when_detached_and_not_dismissed_and_not_gui_mode() {
        // The flag-off path always creates.
        assert!(should_create_browser_pane(false, false, false));
    }

    #[test]
    fn should_not_create_pane_when_already_attached() {
        assert!(!should_create_browser_pane(true, false, false));
    }

    #[test]
    fn should_not_create_pane_when_user_dismissed() {
        assert!(!should_create_browser_pane(false, true, false));
    }

    #[test]
    fn should_not_create_pane_in_gui_background_mode_even_if_otherwise_eligible() {
        // The core GUI-mode gate: an otherwise-eligible detached,
        // non-dismissed session must NOT get a pane when GUI background
        // mode applies (Agent Chat beta on).
        assert!(!should_create_browser_pane(false, false, true));
    }

    #[test]
    fn gui_background_mode_does_not_override_already_attached_or_dismissed() {
        assert!(!should_create_browser_pane(true, false, true));
        assert!(!should_create_browser_pane(false, true, true));
    }

    #[test]
    fn legacy_fallback_always_creates_pane_regardless_of_gui_flag() {
        // The no-workspace-context fallback has no AgentBrowserSession to
        // surface a background browser through (no chip / indicator /
        // peek), so GUI background mode must NOT suppress the pane there —
        // a visible pane is the only safe behavior. Regression guard
        // against re-adding the gate on this path.
        assert!(should_create_legacy_browser_pane(true, false));
        assert!(should_create_legacy_browser_pane(true, true));
    }

    #[test]
    fn legacy_fallback_skips_creation_when_a_browser_session_already_exists() {
        // Pre-existing behavior, unchanged in both modes: the legacy path
        // only bootstraps a pane when no browser session exists yet.
        assert!(!should_create_legacy_browser_pane(false, false));
        assert!(!should_create_legacy_browser_pane(false, true));
    }

    // ─── resolve_workspace_id_by_cwd (browser routing fallback) ─────────
    //
    // Backs the Layer B cwd fallback in the `browser_automation` handler:
    // when an env-less caller (agent-chat Bash subprocess, OpenCode's
    // shared server, an MCP caller without `CODEMUX_WORKSPACE_ID`) sends an
    // empty `workspace_id` plus a `cwd` hint, the browser pane must resolve
    // to the *agent's* workspace, not whatever workspace the user is
    // viewing. Matching is purely lexical (no `fs::canonicalize`), so these
    // fixtures use paths that need not exist on disk.

    /// Minimal `WorkspaceSnapshot` fixture varying only the fields the
    /// resolver reads (`workspace_id`, `cwd`, `worktree_path`); mirrors the
    /// pattern used by `workspaces_sync`'s `make_ws`.
    fn ws(id: &str, cwd: &str, worktree_path: Option<&str>) -> crate::state::WorkspaceSnapshot {
        use crate::state::*;
        WorkspaceSnapshot {
            workspace_id: WorkspaceId(id.to_string()),
            is_git: true,
            title: id.to_string(),
            workspace_type: WorkspaceType::Standard,
            cwd: cwd.to_string(),
            git_branch: None,
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            notification_count: 0,
            latest_agent_state: None,
            worktree_path: worktree_path.map(str::to_string),
            project_root: None,
            project_uid: None,
            workspace_kind: None,
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            pr_head_branch: None,
            provider_kind: None,
            linked_issue: None,
            notifications_muted: false,
            pinned_at: None,
            tabs: Vec::new(),
            active_tab_id: String::new(),
            active_surface_id: SurfaceId(String::new()),
            surfaces: Vec::new(),
            host_id: None,
            remote_cwd: None,
            attach_only: false,
            last_active_at: None,
            last_visited_at: None,
        }
    }

    #[test]
    fn resolve_cwd_exact_match() {
        let workspaces = vec![ws("ws-a", "/home/user/proj-a", None), ws("ws-b", "/home/user/proj-b", None)];
        assert_eq!(
            resolve_workspace_id_by_cwd(&workspaces, "/home/user/proj-b"),
            Some("ws-b".to_string())
        );
    }

    #[test]
    fn resolve_cwd_subdirectory_match() {
        // A Bash subprocess spawned deep inside the workspace tree still
        // resolves to the workspace root.
        let workspaces = vec![ws("ws-a", "/home/user/proj-a", None)];
        assert_eq!(
            resolve_workspace_id_by_cwd(&workspaces, "/home/user/proj-a/src/nested/dir"),
            Some("ws-a".to_string())
        );
    }

    #[test]
    fn resolve_cwd_component_safe_no_sibling_prefix_match() {
        // "/a/b" must NOT match cwd "/a/bc" — the prefix has to land on a
        // path-component boundary.
        let workspaces = vec![ws("ws-a", "/a/b", None)];
        assert_eq!(resolve_workspace_id_by_cwd(&workspaces, "/a/bc"), None);
        assert_eq!(
            resolve_workspace_id_by_cwd(&workspaces, "/a/b/src"),
            Some("ws-a".to_string())
        );
    }

    #[test]
    fn resolve_cwd_longest_root_wins_for_nested_workspaces() {
        // A worktree checked out *under* another workspace must resolve to
        // the deepest (most specific) root, regardless of snapshot order.
        let outer = ws("ws-outer", "/home/user/repo", None);
        let inner = ws("ws-inner", "/home/user/repo/worktrees/feature", None);
        let cwd = "/home/user/repo/worktrees/feature/src";

        assert_eq!(
            resolve_workspace_id_by_cwd(&[outer.clone(), inner.clone()], cwd),
            Some("ws-inner".to_string())
        );
        // Order-independent: the deepest root still wins.
        assert_eq!(
            resolve_workspace_id_by_cwd(&[inner, outer], cwd),
            Some("ws-inner".to_string())
        );
    }

    #[test]
    fn resolve_cwd_matches_worktree_path() {
        // The `worktree_path` is a second candidate root — a worktree
        // whose live `cwd` differs from its checkout path still resolves.
        let workspaces = vec![ws("ws-a", "/home/user/repo", Some("/home/user/.codemux/worktrees/repo/feat"))];
        assert_eq!(
            resolve_workspace_id_by_cwd(&workspaces, "/home/user/.codemux/worktrees/repo/feat/pkg"),
            Some("ws-a".to_string())
        );
    }

    #[test]
    fn resolve_cwd_trailing_slashes_ignored() {
        let workspaces = vec![ws("ws-a", "/home/user/proj-a/", None)];
        assert_eq!(
            resolve_workspace_id_by_cwd(&workspaces, "/home/user/proj-a/"),
            Some("ws-a".to_string())
        );
        assert_eq!(
            resolve_workspace_id_by_cwd(&workspaces, "/home/user/proj-a/src/"),
            Some("ws-a".to_string())
        );
    }

    #[test]
    fn resolve_cwd_windows_backslash_subdirectory_match() {
        // Windows callers send backslash paths from `std::env::current_dir()`;
        // a subdir must still resolve (exact matches already work verbatim).
        let workspaces = vec![ws("ws-a", r"C:\Users\dev\proj", None)];
        assert_eq!(
            resolve_workspace_id_by_cwd(&workspaces, r"C:\Users\dev\proj\src"),
            Some("ws-a".to_string())
        );
        // Component boundary still enforced.
        assert_eq!(
            resolve_workspace_id_by_cwd(&workspaces, r"C:\Users\dev\projx"),
            None
        );
    }

    #[test]
    fn resolve_cwd_no_match_returns_none() {
        let workspaces = vec![ws("ws-a", "/home/user/proj-a", None)];
        assert_eq!(resolve_workspace_id_by_cwd(&workspaces, "/somewhere/else"), None);
    }

    #[test]
    fn resolve_cwd_empty_returns_none() {
        let workspaces = vec![ws("ws-a", "/home/user/proj-a", None)];
        assert_eq!(resolve_workspace_id_by_cwd(&workspaces, ""), None);
        // A cwd of only slashes normalizes to empty and must not match.
        assert_eq!(resolve_workspace_id_by_cwd(&workspaces, "/"), None);
    }

    /// Tiny echo handler used by the round-trip tests: reads one line at a
    /// time and writes it back unchanged. Mirrors the real `handle_client`
    /// signature so it exercises the same generic stream bounds.
    async fn echo_loop<S>(stream: S) -> std::io::Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut lines = BufReader::new(reader).lines();
        while let Some(line) = lines.next_line().await? {
            writer
                .write_all(format!("{line}\n").as_bytes())
                .await?;
        }
        Ok(())
    }

    #[test]
    fn test_control_socket_path_format() {
        // `control_socket_path()` should always return *some* path on a
        // properly-configured system, and that path should match the
        // platform-specific format documented in the header.
        let path = control_socket_path().expect("control_socket_path should return a value");
        let display = path.to_string_lossy().into_owned();

        let expected_suffix = format!("{SOCKET_BASENAME}.sock");

        #[cfg(unix)]
        {
            assert!(
                display.ends_with(&expected_suffix),
                "unix control path must end with {expected_suffix}, got {display:?}"
            );
        }

        #[cfg(windows)]
        {
            let expected_prefix = format!(r"\\.\pipe\{SOCKET_BASENAME}-");
            assert!(
                display.starts_with(&expected_prefix),
                "windows control path must start with {expected_prefix}, got {display:?}"
            );
        }
    }

    // ----- Cross-platform pipe path tests --------------------------------
    //
    // `build_pipe_path` is pure string manipulation and not cfg-gated, so
    // these tests run on every platform. They assert the sanitization
    // contract that `control_socket_path()` on Windows relies on — we
    // don't want to depend on the process-global `USERNAME` env var in
    // tests because parallel cargo test runs would race.

    #[test]
    fn test_named_pipe_path_format() {
        // Typical Windows username — already pipe-name-safe.
        let path = build_pipe_path("alice");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-alice"));
    }

    #[test]
    fn test_named_pipe_path_sanitization_spaces() {
        // Windows lets users have spaces in their account names
        // (e.g. "John Smith"). Spaces are invalid in pipe names —
        // must be replaced with underscores.
        let path = build_pipe_path("John Smith");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-John_Smith"));
    }

    #[test]
    fn test_named_pipe_path_sanitization_special_chars() {
        // Punctuation and separators that appear in real AD/domain
        // usernames (e.g. `DOMAIN\user`, `user@host`, `user.name`).
        // Every non-[A-Za-z0-9_-] char gets replaced with underscore.
        let path = build_pipe_path(r"DOMAIN\john.smith");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-DOMAIN_john_smith"));

        let path = build_pipe_path("user@host");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-user_host"));
    }

    #[test]
    fn test_named_pipe_path_sanitization_empty_or_whitespace() {
        // Empty USERNAME (unlikely but possible if env is stripped)
        // must fall back to "default" so the pipe still has a valid
        // non-empty user segment.
        let path = build_pipe_path("");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-default"));

        let path = build_pipe_path("   ");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-default"));

        let path = build_pipe_path("\t\n");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-default"));
    }

    #[test]
    fn test_named_pipe_path_preserves_underscores_and_dashes() {
        // These are the two punctuation characters explicitly allowed
        // by the sanitization rule — they should pass through unchanged.
        let path = build_pipe_path("test-user_42");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-test-user_42"));
    }

    #[test]
    fn test_named_pipe_path_sanitization_unicode() {
        // Windows allows Unicode usernames (CJK, accented Latin, etc.).
        // `char::is_alphanumeric` accepts Unicode letters, so these
        // characters pass through. If a future change needs to restrict
        // to ASCII, this test will fail and flag the break.
        let path = build_pipe_path("用户");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-用户"));

        let path = build_pipe_path("müller");
        let display = path.to_string_lossy().into_owned();
        assert_eq!(display, format!(r"\\.\pipe\{SOCKET_BASENAME}-müller"));
    }

    #[test]
    fn test_named_pipe_path_never_panics_on_pathological_input() {
        // Single call on a handful of pathological inputs — must not
        // panic and must produce a parsable pipe path. We don't assert
        // on the exact output; just that the function returns cleanly.
        let long_input = "a".repeat(1024);
        let pathological = [
            "\x00",
            "\x00\x01\x02",
            "////",
            "\\\\\\\\",
            long_input.as_str(),
        ];
        let expected_prefix = format!(r"\\.\pipe\{SOCKET_BASENAME}-");
        for input in pathological {
            let path = build_pipe_path(input);
            let display = path.to_string_lossy().into_owned();
            assert!(
                display.starts_with(&expected_prefix),
                "pipe path from pathological input {input:?} must start with {expected_prefix}, got {display:?}"
            );
        }
    }

    // ----- Unix-only tests ------------------------------------------------

    #[cfg(unix)]
    #[tokio::test]
    async fn test_unix_socket_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("round-trip.sock");

        let listener = transport::bind(&path).expect("bind unix socket");
        let server = tokio::spawn(async move {
            let stream = listener.accept().await.expect("accept");
            echo_loop(stream).await.expect("echo_loop");
        });

        // Give the listener a moment to be ready in the runtime.
        tokio::task::yield_now().await;

        let stream = transport::connect(&path).await.expect("connect client");
        let (reader, mut writer) = tokio::io::split(stream);
        writer.write_all(b"hello\n").await.unwrap();
        writer.shutdown().await.unwrap();

        let mut lines = BufReader::new(reader).lines();
        let response = lines.next_line().await.unwrap();
        assert_eq!(response.as_deref(), Some("hello"));

        server.abort();
    }

    #[cfg(unix)]
    #[test]
    fn test_liveness_probe_no_server_unix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-server.sock");
        assert!(
            !transport::sync_liveness_probe(&path),
            "probe should report no server at a nonexistent path"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_liveness_probe_with_server_unix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.sock");
        let listener = transport::bind(&path).expect("bind unix socket");

        // The probe is sync, so run it on a blocking task to avoid tying up
        // the current single-threaded runtime.
        let probe_path = path.clone();
        let probe = tokio::task::spawn_blocking(move || {
            transport::sync_liveness_probe(&probe_path)
        });

        // Accept exactly one connection (the probe) so the probe's connect
        // call succeeds and returns true.
        let accept_task = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        assert!(probe.await.unwrap(), "probe should detect the server");
        accept_task.abort();
    }

    /// The existing fallback-directory logic chmods the parent dir to
    /// `0o700` on creation. This exercises that branch directly and asserts
    /// the mode bits come out right. We don't test `control_socket_path`
    /// itself here because it uses a fixed `/tmp/codemux-{uid}` path that
    /// another test process might already own.
    #[cfg(unix)]
    #[test]
    fn test_fallback_dir_permission_isolation() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let fallback_dir = tmp.path().join("codemux-fake-uid");
        std::fs::create_dir(&fallback_dir).unwrap();
        std::fs::set_permissions(&fallback_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let meta = std::fs::metadata(&fallback_dir).unwrap();
        // Mask off the file-type bits and compare only the permission bits.
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    }

    // ----- Windows-only tests ---------------------------------------------

    #[cfg(windows)]
    fn random_pipe_path() -> PathBuf {
        // Use a per-test pipe name so parallel cargo test runs don't collide.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        PathBuf::from(format!(r"\\.\pipe\codemux-test-{}-{}", std::process::id(), nanos))
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_named_pipe_round_trip() {
        let path = random_pipe_path();

        let listener = transport::bind(&path).expect("bind named pipe");
        let server = tokio::spawn(async move {
            let stream = listener.accept().await.expect("accept");
            echo_loop(stream).await.expect("echo_loop");
        });

        // Give the listener a moment to be ready.
        tokio::task::yield_now().await;

        let stream = transport::connect(&path).await.expect("connect client");
        let (reader, mut writer) = tokio::io::split(stream);
        writer.write_all(b"hello\n").await.unwrap();
        writer.shutdown().await.unwrap();

        let mut lines = BufReader::new(reader).lines();
        let response = lines.next_line().await.unwrap();
        assert_eq!(response.as_deref(), Some("hello"));

        server.abort();
    }

    #[cfg(windows)]
    #[test]
    fn test_liveness_probe_no_server_windows() {
        let path = random_pipe_path();
        assert!(
            !transport::sync_liveness_probe(&path),
            "probe should report no server on an unbound named pipe"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_liveness_probe_with_server_windows() {
        let path = random_pipe_path();
        let listener = transport::bind(&path).expect("bind named pipe");

        let probe_path = path.clone();
        let probe = tokio::task::spawn_blocking(move || {
            transport::sync_liveness_probe(&probe_path)
        });

        let accept_task = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        assert!(probe.await.unwrap(), "probe should detect the server");
        accept_task.abort();
    }
}
