// Tauri commands for the cross-provider MCP server runtime.
//
// Stage 1 ships `list_mcp_servers` (config discovery). Stage 2 adds the
// runtime: spawn/stop/restart commands, the runtime-status snapshot the
// Settings UI hydrates from, and the disabled-set sync from the
// frontend zustand store. Stage 4 will wire the toggle UI.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};

use crate::mcp::{
    codemux_self::codemux_self_config,
    dedupe_servers,
    parser::{parse_claude_wrapped_config, parse_mcp_config_file},
    paths::{enumerate_mcp_paths, is_claude_wrapped_path},
    registry::{McpRegistry, McpServerRuntime},
    runtime::{CappedTools, McpTool},
    source_rank, McpConfigSource, McpServerConfig,
};

/// Discover all configured MCP servers across every provider's known
/// config path, plus Codemux's own hardcoded entry, then dedupe identical
/// configs that show up in multiple sources. Stateless — no spawn, no
/// caching. Per-file errors are logged on stderr and the offending file
/// is dropped from the result rather than failing the whole list.
#[tauri::command]
pub async fn list_mcp_servers(
    project_root: Option<String>,
) -> Result<Vec<McpServerConfig>, String> {
    let project_path: Option<PathBuf> = project_root.map(PathBuf::from);
    let scan = enumerate_mcp_paths(project_path.as_deref());

    let mut servers: Vec<McpServerConfig> = vec![codemux_self_config()];

    for (path, source) in scan.paths {
        // `~/.claude.json` is a multi-section file; everything else is the
        // standard `mcpServers`-key shape.
        let parsed_result = if is_claude_wrapped_path(&path) {
            parse_claude_wrapped_config(&path, project_path.as_deref())
        } else {
            parse_mcp_config_file(&path, source)
        };

        match parsed_result {
            Ok(parsed) => {
                for srv in parsed {
                    // The Codemux self-row already represents Codemux's
                    // built-in MCP. When Codemux's auto-write puts a
                    // `codemux` entry into a project's `.mcp.json` we
                    // suppress it here — otherwise users see the
                    // always-on row twice.
                    if srv.name == "codemux"
                        && matches!(
                            source,
                            McpConfigSource::ClaudeProject
                                | McpConfigSource::CodemuxProject
                        )
                    {
                        continue;
                    }
                    servers.push(srv);
                }
            }
            Err(e) => {
                eprintln!("[codemux::mcp] {}: {}", path.display(), e);
            }
        }
    }

    // Sort by canonical source rank then name BEFORE dedupe so identical
    // configs from multiple files merge with the canonical (lowest-rank)
    // source as the primary.
    servers.sort_by(|a, b| {
        source_rank(a.primary_source())
            .cmp(&source_rank(b.primary_source()))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(dedupe_servers(servers))
}

// ── Stage 2 runtime commands ────────────────────────────────────────────

/// Snapshot of every server's current runtime state. Frontend hydrates
/// the Settings panel from this on mount and then listens to the
/// `mcp-status-changed` event for incremental updates.
#[tauri::command]
pub async fn get_mcp_runtime_status(
    registry: State<'_, McpRegistry>,
) -> Result<Vec<McpServerRuntime>, String> {
    Ok(registry.list_runtime().await)
}

/// Mirror the frontend zustand `disabledIds` into the registry. Called
/// on App mount and on every toggle. Disabled servers currently running
/// get stopped here.
#[tauri::command]
pub async fn set_mcp_disabled_ids(
    app: AppHandle,
    registry: State<'_, McpRegistry>,
    ids: Vec<String>,
) -> Result<(), String> {
    registry.set_disabled_ids(Some(&app), ids).await
}

/// Spawn every enabled server discovered for the active project (plus
/// Codemux's hardcoded entry). Idempotent: already-running servers are
/// untouched. Triggered by:
///   1. Settings → MCP Servers mount (so users can inspect status
///      without first opening a chat).
///   2. The first agent-chat session (lazy spawn — see
///      `agent_chat_start_session`).
#[tauri::command]
pub async fn prime_mcp_runtime(
    app: AppHandle,
    registry: State<'_, McpRegistry>,
    project_root: Option<String>,
) -> Result<Vec<McpServerRuntime>, String> {
    let project_path: Option<PathBuf> = project_root.map(PathBuf::from);
    registry
        .prime_for_chat(Some(&app), project_path.as_deref())
        .await;
    Ok(registry.list_runtime().await)
}

/// Start a single server by id. Used by the Settings toggle when the
/// user re-enables a previously-disabled row.
#[tauri::command]
pub async fn start_mcp_server_cmd(
    app: AppHandle,
    registry: State<'_, McpRegistry>,
    id: String,
    project_root: Option<String>,
) -> Result<McpServerRuntime, String> {
    let project_path: Option<PathBuf> = project_root.map(PathBuf::from);
    let cfg = find_config(&id, project_path.as_deref())
        .ok_or_else(|| format!("no config found for server id {id}"))?;
    Ok(registry.ensure_started(Some(&app), cfg).await)
}

/// Stop a server (graceful EOF then kill, 2s budget). Used by the
/// Settings toggle when the user disables a row.
#[tauri::command]
pub async fn stop_mcp_server_cmd(
    app: AppHandle,
    registry: State<'_, McpRegistry>,
    id: String,
) -> Result<McpServerRuntime, String> {
    registry.stop_server(Some(&app), &id).await
}

/// Manual restart, exposed for the "Restart" affordance on errored rows.
#[tauri::command]
pub async fn restart_mcp_server_cmd(
    app: AppHandle,
    registry: State<'_, McpRegistry>,
    id: String,
) -> Result<McpServerRuntime, String> {
    registry.restart_server(Some(&app), &id).await
}

/// Aggregate every running server's tools, with the 50-tool cap
/// applied. Stage 3 will use this to register tools with the Claude
/// SDK; Stage 2 exposes it for debugging via DevTools.
#[tauri::command]
pub async fn list_mcp_tools(
    registry: State<'_, McpRegistry>,
) -> Result<Vec<McpTool>, String> {
    Ok(registry.list_all_tools().await)
}

/// Same as `list_mcp_tools` but returns the full [`CappedTools`]
/// envelope so the Settings UI can render a "N tools dropped to fit
/// cap" banner when the cap engaged.
#[tauri::command]
pub async fn list_mcp_tools_with_cap_info(
    registry: State<'_, McpRegistry>,
) -> Result<CappedTools, String> {
    Ok(registry.list_all_tools_with_cap_info().await)
}

/// Tools registered by a single server, uncapped. Used by the
/// Settings tool-list modal so the user sees the full surface even
/// when some tools were dropped from the agent's view to fit the cap.
#[tauri::command]
pub async fn list_mcp_tools_for_server(
    registry: State<'_, McpRegistry>,
    id: String,
) -> Result<Vec<McpTool>, String> {
    Ok(registry.list_tools_for_server(&id).await)
}

/// Re-discover the config for a single server id by walking the same
/// paths `list_mcp_servers` walks. Returns the first match. Used by
/// the start command when the registry doesn't already hold a handle.
fn find_config(id: &str, project_root: Option<&Path>) -> Option<McpServerConfig> {
    if id == "codemux-self" {
        return Some(codemux_self_config());
    }

    let project_path: Option<PathBuf> = project_root.map(|p| p.to_path_buf());
    let scan = enumerate_mcp_paths(project_path.as_deref());

    for (path, source) in scan.paths {
        let parsed = if is_claude_wrapped_path(&path) {
            parse_claude_wrapped_config(&path, project_path.as_deref())
        } else {
            parse_mcp_config_file(&path, source)
        };
        if let Ok(parsed) = parsed {
            for srv in parsed {
                if srv.id == id {
                    return Some(srv);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn returns_at_least_codemux_self() {
        let result = list_mcp_servers(None).await.unwrap();
        assert!(result
            .iter()
            .any(|s| s.primary_source() == McpConfigSource::Codemux));
        assert_eq!(result[0].primary_source(), McpConfigSource::Codemux);
    }

    #[tokio::test]
    async fn project_servers_picked_up_from_root_mcp_json() {
        let project = TempDir::new().unwrap();
        // Project-scope file is at `<project>/.mcp.json`, NOT
        // `<project>/.claude/.mcp.json`.
        fs::write(
            project.path().join(".mcp.json"),
            r#"{ "mcpServers": { "demo": { "command": "echo" } } }"#,
        )
        .unwrap();

        let project_root = project.path().display().to_string();
        let result = list_mcp_servers(Some(project_root)).await.unwrap();

        let demo = result.iter().find(|s| s.name == "demo");
        assert!(demo.is_some(), "expected demo server in {:?}", result);
        assert_eq!(demo.unwrap().primary_source(), McpConfigSource::ClaudeProject);
    }

    #[tokio::test]
    async fn malformed_file_does_not_break_list() {
        let project = TempDir::new().unwrap();
        fs::write(project.path().join(".mcp.json"), "not valid json").unwrap();

        let project_root = project.path().display().to_string();
        let result = list_mcp_servers(Some(project_root)).await.unwrap();
        // Codemux self still present even though the project file failed.
        assert!(result
            .iter()
            .any(|s| s.primary_source() == McpConfigSource::Codemux));
    }

    #[tokio::test]
    async fn project_codemux_entry_is_filtered_out() {
        // Codemux's `upsert_mcp_config` writes a `codemux` server entry
        // into every workspace's `.mcp.json`. The list-discovery path
        // should drop it so users don't see a second always-on row.
        let project = TempDir::new().unwrap();
        fs::write(
            project.path().join(".mcp.json"),
            r#"{
              "mcpServers": {
                "codemux": { "command": "/usr/bin/codemux", "args": ["mcp"] },
                "demo":    { "command": "echo" }
              }
            }"#,
        )
        .unwrap();

        let result =
            list_mcp_servers(Some(project.path().display().to_string()))
                .await
                .unwrap();

        // Exactly one `codemux` row — the always-on hardcoded one.
        let codemux_rows: Vec<_> =
            result.iter().filter(|s| s.name == "codemux").collect();
        assert_eq!(codemux_rows.len(), 1);
        assert_eq!(codemux_rows[0].primary_source(), McpConfigSource::Codemux);
        // `demo` from the same file still shows up.
        assert!(result.iter().any(|s| s.name == "demo"));
    }
}
