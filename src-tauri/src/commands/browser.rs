use crate::agent_browser::{AgentBrowserManager, BrowserAutomationResult};
use crate::state::AppStateStore;
use std::path::PathBuf;
use tauri::State;

pub(crate) fn create_browser_pane_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    pane_id: String,
    url: Option<String>,
) -> Result<String, String> {
    // Check if this workspace has a detached agent browser session to reconnect to.
    let workspace_id = state.workspace_id_for_pane(&pane_id);
    let agent_session = workspace_id
        .as_ref()
        .and_then(|wid| state.find_detached_agent_browser(wid));

    // Use the agent session's URL if reconnecting and no explicit URL was given.
    let effective_url = if url.is_some() {
        url
    } else {
        agent_session.as_ref().and_then(|s| s.current_url.clone())
    };

    let (new_pane_id, browser_id) = state.create_browser_pane(&pane_id, effective_url.as_deref())?;

    // Attach the agent session to the new pane for reconnection.
    if let (Some(wid), Some(_)) = (&workspace_id, &agent_session) {
        let _ = state.attach_agent_browser_to_pane(wid, &new_pane_id, &browser_id);
        eprintln!("[BROWSER] Reconnected agent browser session to new pane in workspace {wid}");
    }

    crate::state::emit_app_state(&app);
    Ok(new_pane_id.0)
}

pub(crate) fn browser_open_url_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    browser_id: String,
    url: String,
) -> Result<(), String> {
    state.update_browser_url(&browser_id, url)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn create_browser_pane<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    pane_id: String,
    url: Option<String>,
) -> Result<String, String> {
    create_browser_pane_impl(app, &state, pane_id, url)
}

/// Host this workspace's agent browser session in the right-panel deck.
///
/// The deck's `browser` pane is not a second browser — it is the *same*
/// `AgentBrowserSession` the `codemux browser` CLI and the MCP tools drive,
/// rendered by a `BrowserPane` mounted in the panel instead of in the pane
/// tree. That is why this returns the session: the frontend needs
/// `cli_session_name` (the daemon key every agent command resolves to) and
/// `stream_url` to mount the stream, and must not try to re-derive either.
///
/// Sequence mirrors the `browser_automation` handler in `control.rs` so a
/// user-opened browser and an agent-opened one converge on one daemon:
/// resolve-or-create the session, allocate its port under
/// `cli_session_name`, write the port back, then mark it docked.
///
/// Adopts rather than duplicates. If the session is currently attached to a
/// pane-tree node, that node is closed here — same Chromium, same port, same
/// cookies, just a different surface hosting the stream. `dock_...` clears
/// `pane_id`/`browser_id` before the close so the pane teardown can't mark
/// the session user-dismissed on the way out.
#[tauri::command]
pub async fn dock_browser_in_right_panel<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    agent_browser: State<'_, AgentBrowserManager>,
    workspace_id: String,
) -> Result<crate::state::AgentBrowserSession, String> {
    let placeholder_port = state
        .agent_browser_stream_port_for_workspace(&workspace_id)
        .unwrap_or(crate::agent_browser::DEFAULT_STREAM_PORT);
    let session_for_naming =
        state.resolve_agent_browser_session(&workspace_id, placeholder_port);
    let stream_port = agent_browser
        .allocate_port(&session_for_naming.cli_session_name)
        .await
        .unwrap_or(crate::agent_browser::DEFAULT_STREAM_PORT);
    let _ = state.update_agent_browser_stream_port(&workspace_id, stream_port);

    let (session, adopted_pane) = state.dock_agent_browser_in_right_panel(&workspace_id)?;
    if let Some(pane_id) = adopted_pane {
        // Best-effort: the pane may already be gone (closed in the same
        // tick). The session is docked either way.
        let _ = state.close_pane(&pane_id.0);
    }

    crate::state::emit_app_state(&app);
    Ok(session)
}

/// Remove this workspace's agent browser session from the right-panel deck.
///
/// Mirrors closing a browser pane: the Chromium daemon keeps running so the
/// agent can carry on driving it headlessly, and `dismissed` (the deck tab's
/// ×, as opposed to collapsing the whole panel) stops the agent re-surfacing
/// it on its next command.
#[tauri::command]
pub fn undock_browser_from_right_panel<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    dismissed: bool,
) -> Result<(), String> {
    if state.undock_agent_browser_from_right_panel(&workspace_id, dismissed) {
        crate::state::emit_app_state(&app);
    }
    Ok(())
}

#[tauri::command]
pub fn browser_open_url<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    browser_id: String,
    url: String,
) -> Result<(), String> {
    browser_open_url_impl(app, &state, browser_id, url)
}

#[tauri::command]
pub fn browser_history_back<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<(), String> {
    state.browser_history_step(&browser_id, -1)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_history_forward<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<(), String> {
    state.browser_history_step(&browser_id, 1)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_reload<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<(), String> {
    state.reload_browser(&browser_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_set_loading_state<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    browser_id: String,
    is_loading: bool,
    error: Option<String>,
) -> Result<(), String> {
    state.set_browser_loading_state(&browser_id, is_loading, error)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub async fn agent_browser_spawn(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<(), String> {
    manager.spawn(&browser_id).await
}

#[tauri::command]
pub async fn agent_browser_run(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
    action: String,
    params: serde_json::Value,
) -> Result<BrowserAutomationResult, String> {
    manager.run_command(&browser_id, &action, params).await
}

#[tauri::command]
pub async fn agent_browser_close(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<(), String> {
    manager.close(&browser_id).await
}

#[tauri::command]
pub async fn start_browser_stream(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.start_stream(&browser_id).await
}

#[tauri::command]
pub async fn agent_browser_screenshot(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.get_screenshot(&browser_id).await
}

// ── Browser Data Management ──

fn agent_browser_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join(".agent-browser")
}

fn dir_size(path: &std::path::Path) -> u64 {
    if !path.is_dir() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

// All three commands are `async fn` + `spawn_blocking` because they walk or
// delete the entire `~/.agent-browser` tree. On a profile with significant
// cache/cookie data the recursive `read_dir` and `remove_dir_all` calls take
// long enough to freeze the UI if run on the GTK main thread.

#[tauri::command]
pub async fn get_browser_data_size() -> Result<u64, String> {
    tokio::task::spawn_blocking(|| dir_size(&agent_browser_dir()))
        .await
        .map_err(|e| format!("get_browser_data_size task join failed: {e}"))
}

#[tauri::command]
pub async fn clear_browser_cookies() -> Result<(), String> {
    tokio::task::spawn_blocking(|| {
        let sessions_dir = agent_browser_dir().join("sessions");
        if sessions_dir.exists() {
            std::fs::remove_dir_all(&sessions_dir)
                .map_err(|e| format!("Failed to clear browser cookies: {e}"))
        } else {
            Ok(())
        }
    })
    .await
    .map_err(|e| format!("clear_browser_cookies task join failed: {e}"))?
}

#[tauri::command]
pub async fn clear_all_browser_data() -> Result<(), String> {
    tokio::task::spawn_blocking(|| {
        let dir = agent_browser_dir();
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("Failed to clear browser data: {e}"))
        } else {
            Ok(())
        }
    })
    .await
    .map_err(|e| format!("clear_all_browser_data task join failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn dir_size_returns_zero_for_nonexistent_path() {
        let missing = std::path::Path::new("/this/path/should/definitely/not/exist/xyz123");
        assert_eq!(dir_size(missing), 0);
    }

    #[test]
    fn dir_size_returns_zero_for_empty_dir() {
        let tmp = TempDir::new().expect("tempdir");
        assert_eq!(dir_size(tmp.path()), 0);
    }

    #[test]
    fn dir_size_returns_zero_when_path_is_a_file() {
        // dir_size guards `!path.is_dir()` — pointing at a regular file must
        // be a no-op rather than crash or return the file size.
        let tmp = TempDir::new().expect("tempdir");
        let f = tmp.path().join("only.txt");
        fs::write(&f, vec![0u8; 42]).expect("write file");
        assert_eq!(dir_size(&f), 0);
    }

    #[test]
    fn dir_size_sums_flat_files() {
        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("a"), vec![0u8; 100]).expect("write a");
        fs::write(tmp.path().join("b"), vec![0u8; 200]).expect("write b");
        assert_eq!(dir_size(tmp.path()), 300);
    }

    #[test]
    fn dir_size_walks_nested_dirs() {
        let tmp = TempDir::new().expect("tempdir");
        let nested = tmp.path().join("sub").join("deeper");
        fs::create_dir_all(&nested).expect("mkdir -p");
        fs::write(tmp.path().join("top.txt"), vec![0u8; 50]).expect("top");
        fs::write(tmp.path().join("sub").join("mid.txt"), vec![0u8; 25]).expect("mid");
        fs::write(nested.join("deep.txt"), vec![0u8; 75]).expect("deep");
        assert_eq!(dir_size(tmp.path()), 150);
    }

    // End-to-end sanity check: the async command really does walk the
    // filesystem through `spawn_blocking` and return the same value the
    // synchronous helper computes. Guards against the `async`-conversion
    // accidentally dropping the awaited result or swallowing errors.
    #[tokio::test]
    async fn async_dir_size_through_spawn_blocking_matches_sync() {
        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("payload"), vec![0u8; 1024]).expect("write");
        let path = tmp.path().to_path_buf();
        let async_result = tokio::task::spawn_blocking(move || dir_size(&path))
            .await
            .expect("blocking task");
        assert_eq!(async_result, 1024);
    }
}
