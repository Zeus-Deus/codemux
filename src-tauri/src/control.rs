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
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    Some(runtime_dir.join("codemux.sock"))
}

pub fn spawn_control_server(app: AppHandle) {
    let Some(socket_path) = control_socket_path() else {
        crate::diagnostics::stderr_line(
            "[codemux::control] XDG_RUNTIME_DIR unavailable, skipping control server",
        );
        return;
    };

    // If a control socket already exists and responds, assume another Codemux
    // instance is running and do NOT steal the socket or start a second server.
    if socket_path.exists() {
        if let Ok(stream) = std::os::unix::net::UnixStream::connect(&socket_path) {
            drop(stream);
            crate::diagnostics::stderr_line(&format!(
                "[codemux::control] Existing control socket at {:?} is alive; skipping new control server",
                socket_path
            ));
            return;
        } else {
            crate::diagnostics::stderr_line(&format!(
                "[codemux::control] Existing control socket at {:?} appears stale; replacing it",
                socket_path
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

    let _ = fs::remove_file(&socket_path);

    tauri::async_runtime::spawn(async move {
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                crate::diagnostics::stderr_line(&format!(
                    "[codemux::control] Failed to bind control socket: {error}"
                ));
                return;
            }
        };

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
            let id = state.create_workspace();
            crate::state::emit_app_state(app);
            Ok(serde_json::json!({ "workspace_id": id.0 }))
        }
        "split_pane" => {
            let state: State<'_, AppStateStore> = app.state();
            let pane_id = request.params.get("pane_id").and_then(Value::as_str).unwrap_or_default();
            let direction = request.params.get("direction").and_then(Value::as_str).unwrap_or("horizontal");
            let direction = match direction {
                "vertical" => crate::state::SplitDirection::Vertical,
                _ => crate::state::SplitDirection::Horizontal,
            };
            state
                .split_pane(pane_id, direction)
                .map(|session_id| {
                    crate::state::emit_app_state(app);
                    serde_json::json!({ "session_id": session_id.0 })
                })
        }
        "create_browser_pane" => {
            let state: State<'_, AppStateStore> = app.state();
            let pane_id = request.params.get("pane_id").and_then(Value::as_str).unwrap_or_default();
            state
                .create_browser_pane(pane_id)
                .map(|browser_id| {
                    crate::state::emit_app_state(app);
                    serde_json::json!({ "browser_id": browser_id.0 })
                })
        }
        "open_url" => {
            let state: State<'_, AppStateStore> = app.state();
            let browser_id = request.params.get("browser_id").and_then(Value::as_str).unwrap_or_default();
            let url = request.params.get("url").and_then(Value::as_str).unwrap_or_default();
            state
                .update_browser_url(browser_id, url.to_string())
                .map(|_| {
                    crate::state::emit_app_state(app);
                    serde_json::json!({ "browser_id": browser_id, "url": url })
                })
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
            let agent_browser: State<'_, crate::agent_browser::AgentBrowserManager> = app.state();
            let browser_id = request.params.get("browser_id").and_then(Value::as_str).unwrap_or("default").to_string();
            
            let action_kind = request.params.get("action")
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str)
                .unwrap_or("open_url")
                .to_string();
            
            let params = request.params.get("action").cloned().unwrap_or(Value::Null);
            
            agent_browser.run_command(&browser_id, &action_kind, params)
                .await
                .and_then(|result| serde_json::to_value(result).map_err(|error| error.to_string()))
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
                .map(str::to_string);
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
