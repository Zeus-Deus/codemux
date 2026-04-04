use crate::indexing;
use crate::memory;
use crate::state::AppStateStore;
use crate::terminal::PtyState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

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

pub fn control_socket_path() -> Option<PathBuf> {
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        return Some(runtime_dir.join("codemux.sock"));
    }

    // Fallback for systems without XDG_RUNTIME_DIR (e.g. minimal distros, some AppImage environments).
    let uid = unsafe { libc::getuid() };
    let fallback_dir = PathBuf::from(format!("/tmp/codemux-{uid}"));
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
    Some(fallback_dir.join("codemux.sock"))
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
            "[codemux::control] XDG_RUNTIME_DIR unavailable, skipping control server",
        );
        #[cfg(debug_assertions)]
        {
            let pid = std::process::id();
            let startup_id = std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
            crate::diagnostics::native_startup_breadcrumb(&format!(
                "[{}] startup_id={} pid={} component=control outcome=skip_no_xdg_runtime_dir",
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

    // If a control socket already exists and responds, assume another Codemux
    // instance is running and do NOT steal the socket or start a second server.
    if socket_path.exists() {
        if let Ok(stream) = std::os::unix::net::UnixStream::connect(&socket_path) {
            drop(stream);
            crate::diagnostics::stderr_line(&format!(
                "[codemux::control] Existing control socket at {:?} is alive; skipping new control server",
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
        } else {
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
    }

    if let Some(parent) = socket_path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            crate::diagnostics::stderr_line(&format!(
                "[codemux::control] Failed to create control dir: {error}"
            ));
            return;
        }
    }

    let _ = fs::remove_file(&socket_path);

    tauri::async_runtime::spawn(async move {
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                crate::diagnostics::stderr_line(&format!(
                    "[codemux::control] Failed to bind control socket: {error}"
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
                Ok((stream, _)) => {
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

async fn handle_client(app: AppHandle, stream: UnixStream) -> Result<(), String> {
    let (reader, mut writer) = stream.into_split();
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

/// Send a request to the running Codemux control socket and return the response.
/// Used by both the CLI and the MCP server to communicate with the Codemux app.
pub async fn send_control_request(request: ControlRequest) -> Result<ControlResponse, String> {
    let socket_path = control_socket_path()
        .ok_or_else(|| "Control socket path unavailable".to_string())?;
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(|error| format!("Failed to connect to Codemux control socket: {error}"))?;
    let (reader, mut writer) = stream.into_split();

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

            let params = request.params.get("action").cloned().unwrap_or(Value::Null);

            // Resolve the CLI session name to use for agent-browser commands.
            let cli_session_name = if !workspace_id.is_empty() {
                // Allocate a unique stream port for this workspace.
                let stream_port = agent_browser.allocate_port(&workspace_id).await
                    .unwrap_or(crate::agent_browser::DEFAULT_STREAM_PORT);


                // Workspace-scoped path: find or create agent session for this workspace.
                let agent_session = state.resolve_agent_browser_session(
                    &workspace_id,
                    stream_port,
                );

                // Register the same port under cli_session_name so that
                // start_stream(cli_session_name) and close(cli_session_name) find it.

                agent_browser.ensure_port(&agent_session.cli_session_name, stream_port).await;

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
