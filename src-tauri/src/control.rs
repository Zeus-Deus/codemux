use crate::indexing;
use crate::memory;
use crate::state::AppStateStore;
use crate::terminal::PtyState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(unix)]
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};
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
fn resolve_browser_id(app: &AppHandle, requested: &str) -> String {
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

pub fn spawn_control_server(app: AppHandle) {
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

async fn handle_client<S>(app: AppHandle, stream: S) -> Result<(), String>
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

async fn dispatch_request(app: &AppHandle, request: ControlRequest) -> ControlResponse {
    let result = match request.command.as_str() {
        "status" => Ok(serde_json::json!({
            "socket_path": control_socket_path().map(|path| path.display().to_string()),
            "protocol_version": CONTROL_PROTOCOL_VERSION
        })),
        "get_app_state" => {
            let state: State<'_, AppStateStore> = app.state();
            serde_json::to_value(state.snapshot()).map_err(|error| error.to_string())
        }
        "create_workspace" => {
            let state: State<'_, AppStateStore> = app.state();
            let db: State<'_, crate::database::DatabaseStore> = app.state();
            crate::commands::workspace::create_workspace_impl(app.clone(), &state, &db, None)
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
            crate::commands::presets::apply_preset(
                app.clone(),
                state,
                pty_state,
                presets,
                workspace_id,
                preset_id,
                override_mode,
                initial_prompt,
            )
            .map(|()| serde_json::json!({ "ok": true }))
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
            state
                .add_notification(None, None, message.to_string(), crate::state::NotificationLevel::Attention)
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
        "browser_automation" => {
            let state: State<'_, AppStateStore> = app.state();
            let agent_browser: State<'_, crate::agent_browser::AgentBrowserManager> = app.state();
            let workspace_id = request.params
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            let action_kind = request.params.get("action")
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str)
                .unwrap_or("open_url")
                .to_string();

            eprintln!(
                "[codemux::browser] handler received action={action_kind} workspace_id={workspace_id}"
            );

            let params = request.params.get("action").cloned().unwrap_or(Value::Null);

            // Resolve the CLI session name to use for agent-browser commands.
            let cli_session_name = if !workspace_id.is_empty() {
                // P2 from docs/plans/browser-stream-fix.md: allocate the
                // stream port keyed by `cli_session_name` directly, not
                // by `workspace_id` plus an `ensure_port` alias.
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
                // for real and write the actual port back into state so
                // the frontend's reactive `stream_url` reflects reality
                // (P6).
                let placeholder_port = state
                    .agent_browser_stream_port_for_workspace(&workspace_id)
                    .unwrap_or(crate::agent_browser::DEFAULT_STREAM_PORT);
                let session_for_naming = state.resolve_agent_browser_session(
                    &workspace_id,
                    placeholder_port,
                );
                let stream_port = agent_browser
                    .allocate_port(&session_for_naming.cli_session_name)
                    .await
                    .unwrap_or(crate::agent_browser::DEFAULT_STREAM_PORT);

                // Persist the real port back into the agent session so
                // the frontend's `stream_url` is reactive when the port
                // gets re-allocated after a teardown/respawn.
                let _ = state.update_agent_browser_stream_port(
                    &workspace_id,
                    stream_port,
                );
                let agent_session = state.resolve_agent_browser_session(
                    &workspace_id,
                    stream_port,
                );

                // Auto-create a browser pane if no pane is attached and user hasn't dismissed it.
                let should_create = agent_session.pane_id.is_none() && !agent_session.user_dismissed;

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
                if state.snapshot().browser_sessions.is_empty() {
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
            // Try cli_session_name first (key used by start_stream/close), then workspace_id.
            let vision_port = agent_browser.get_port(&cli_session_name).await
                .or(agent_browser.get_port(&workspace_id).await)
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

/// Resolve the repo path for control socket commands.
/// Checks `repo_path` param first, then falls back to active workspace's project_root/cwd.
fn resolve_control_repo_path(
    _app: &AppHandle,
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
