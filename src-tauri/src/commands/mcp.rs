// Tauri commands for the cross-provider MCP server runtime.
//
// Stage 1 ships `list_mcp_servers` (config discovery). Stage 2 adds the
// runtime: spawn/stop/restart commands, the runtime-status snapshot the
// Settings UI hydrates from, and the disabled-set sync from the
// frontend zustand store. Stage 4 will wire the toggle UI.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Runtime, State};

use crate::mcp::{
    codemux_self::codemux_self_config,
    dedupe_servers,
    parser::{
        parse_claude_wrapped_config, parse_codex_config_file, parse_mcp_config_file,
        parse_opencode_config_file,
    },
    paths::{enumerate_mcp_paths, is_claude_wrapped_path},
    registry::{McpRegistry, McpServerRuntime},
    runtime::McpTool,
    source_rank, McpConfigSource, McpServerConfig,
};

/// Discover all configured MCP servers across every provider's known
/// config path, plus Codemux's own hardcoded entry, then dedupe identical
/// configs that show up in multiple sources. Stateless — no spawn, no
/// caching. Per-file errors are logged on stderr and the offending file
/// is dropped from the result rather than failing the whole list.
async fn discover_mcp_servers(
    project_root: Option<String>,
) -> Result<Vec<McpServerConfig>, String> {
    let project_path: Option<PathBuf> = project_root.map(PathBuf::from);
    let scan = enumerate_mcp_paths(project_path.as_deref());

    let mut servers: Vec<McpServerConfig> = vec![codemux_self_config()];
    let own = PathBuf::from(crate::mcp_server::codemux_mcp_command());

    for (path, source) in scan.paths {
        // `~/.claude.json` is a multi-section file; everything else is the
        // standard `mcpServers`-key shape.
        let parsed_result = if is_claude_wrapped_path(&path) {
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

        match parsed_result {
            Ok(parsed) => {
                for srv in parsed {
                    // The Codemux self-row already represents Codemux's
                    // built-in MCP. Provider CLIs may auto-write the same
                    // `codemux` entry into their own config, and a user may
                    // add `codemux mcp` under another name; suppress every
                    // discovered copy so users see the always-on row once.
                    if srv.is_codemux_entry(&own) {
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

#[tauri::command]
pub async fn list_mcp_servers(
    registry: State<'_, McpRegistry>,
    project_root: Option<String>,
) -> Result<Vec<McpServerConfig>, String> {
    let mut servers = discover_mcp_servers(project_root).await?;
    servers.extend(registry.plugin_configs().await);
    Ok(servers.into_iter().map(McpServerConfig::redacted).collect())
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
pub async fn set_mcp_disabled_ids<R: Runtime>(
    app: AppHandle<R>,
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
pub async fn prime_mcp_runtime<R: Runtime>(
    app: AppHandle<R>,
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
pub async fn start_mcp_server_cmd<R: Runtime>(
    app: AppHandle<R>,
    registry: State<'_, McpRegistry>,
    id: String,
    project_root: Option<String>,
) -> Result<McpServerRuntime, String> {
    let project_path: Option<PathBuf> = project_root.map(PathBuf::from);
    let cfg = registry
        .plugin_configs()
        .await
        .into_iter()
        .find(|cfg| cfg.id == id)
        .or_else(|| find_config(&id, project_path.as_deref()))
        .ok_or_else(|| format!("no config found for server id {id}"))?;
    Ok(registry.ensure_started(Some(&app), cfg).await)
}

/// Stop a server (graceful EOF then kill, 2s budget). Used by the
/// Settings toggle when the user disables a row.
#[tauri::command]
pub async fn stop_mcp_server_cmd<R: Runtime>(
    app: AppHandle<R>,
    registry: State<'_, McpRegistry>,
    id: String,
) -> Result<McpServerRuntime, String> {
    registry.stop_server(Some(&app), &id).await
}

/// Manual restart, exposed for the "Restart" affordance on errored rows.
#[tauri::command]
pub async fn restart_mcp_server_cmd<R: Runtime>(
    app: AppHandle<R>,
    registry: State<'_, McpRegistry>,
    id: String,
) -> Result<McpServerRuntime, String> {
    registry.restart_server(Some(&app), &id).await
}

/// List global running server tools for debugging via DevTools. Provider
/// sessions use their project-scoped registry view instead.
#[tauri::command]
pub async fn list_mcp_tools(registry: State<'_, McpRegistry>) -> Result<Vec<McpTool>, String> {
    Ok(registry.list_all_tools().await)
}

/// Tools registered by a single server. Drives the Settings
/// tool-list modal.
#[tauri::command]
pub async fn list_mcp_tools_for_server(
    registry: State<'_, McpRegistry>,
    id: String,
) -> Result<Vec<McpTool>, String> {
    Ok(registry.list_tools_for_server(&id).await)
}

/// Re-discover the config for a single server id, deduplicated and tagged
/// the same way chat priming discovers it. Used by the start command when
/// the registry doesn't already hold a handle.
fn find_config(id: &str, project_root: Option<&Path>) -> Option<McpServerConfig> {
    crate::mcp::registry::discover_configs(project_root)
        .into_iter()
        .find(|config| config.id == id)
}

/// Approve a server defined by the project's own files after the user
/// reviewed it in Settings, then start it. `fingerprint` comes from the
/// row's `needsApproval` status; the approval is stored locally only.
#[tauri::command]
pub async fn approve_mcp_server<R: Runtime>(
    app: AppHandle<R>,
    registry: State<'_, McpRegistry>,
    id: String,
    fingerprint: String,
) -> Result<McpServerRuntime, String> {
    registry.approve_project_server(&app, &id, &fingerprint).await
}

/// `codemux mcp-approve` on the host, the only way to approve on a headless
/// `codemux serve`. Without `fingerprint` or `revoke`: the servers
/// `checkout`'s own files define, those that wait for approval with their
/// full definition and those already approved. With `fingerprint`: approve
/// the waiting server whose definition it names and start it. With
/// `revoke`: forget that approval and stop the servers running it.
pub(crate) async fn project_approval<R: Runtime>(
    app: &AppHandle<R>,
    checkout: &Path,
    fingerprint: Option<&str>,
    revoke: Option<&str>,
) -> Result<serde_json::Value, String> {
    use crate::mcp::project_trust;
    use tauri::Manager;
    let registry = app
        .try_state::<McpRegistry>()
        .ok_or("The MCP runtime is not running")?;
    if let Some(revoke) = revoke {
        let revoked = registry.revoke_project_approval(app, revoke).await?;
        return Ok(serde_json::json!({ "revoked": revoked }));
    }
    let (approved, pending): (Vec<_>, Vec<_>) = crate::mcp::registry::discover_configs(Some(checkout))
        .into_iter()
        .filter_map(|config| Some((project_trust::fingerprint(&config)?, config)))
        .partition(|(fingerprint, _)| project_trust::is_approved(Some(app), fingerprint));
    let Some(wanted) = fingerprint else {
        let rows = |rows: Vec<(String, McpServerConfig)>| -> Vec<serde_json::Value> {
            rows.into_iter()
                .map(|(fingerprint, config)| {
                    serde_json::json!({
                        "name": project_trust::escape(&config.name),
                        "fingerprint": fingerprint,
                        "summary": project_trust::summary(&config),
                    })
                })
                .collect()
        };
        return Ok(serde_json::json!({ "pending": rows(pending), "approved": rows(approved) }));
    };
    let (_, config) = pending
        .into_iter()
        .find(|(fingerprint, _)| fingerprint == wanted)
        .ok_or("No server in this folder waits for that approval. It may have changed or already be approved; list them again.")?;
    let name = project_trust::escape(&config.name);
    let row = registry.approve_definition(app, config, wanted).await?;
    Ok(serde_json::json!({ "approved": name, "status": row.status }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn returns_at_least_codemux_self() {
        let result = discover_mcp_servers(None).await.unwrap();
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
        let result = discover_mcp_servers(Some(project_root)).await.unwrap();

        let demo = result.iter().find(|s| s.name == "demo");
        assert!(demo.is_some(), "expected demo server in {:?}", result);
        assert_eq!(
            demo.unwrap().primary_source(),
            McpConfigSource::ClaudeProject
        );
    }

    #[tokio::test]
    async fn malformed_file_does_not_break_list() {
        let project = TempDir::new().unwrap();
        fs::write(project.path().join(".mcp.json"), "not valid json").unwrap();

        let project_root = project.path().display().to_string();
        let result = discover_mcp_servers(Some(project_root)).await.unwrap();
        // Codemux self still present even though the project file failed.
        assert!(result
            .iter()
            .any(|s| s.primary_source() == McpConfigSource::Codemux));
    }

    #[tokio::test]
    async fn project_codemux_entry_is_filtered_out() {
        // Older Codemux builds wrote a `codemux` server entry into every
        // workspace's `.mcp.json`. The list-discovery path should drop it
        // so users don't see a second always-on row.
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

        let result = discover_mcp_servers(Some(project.path().display().to_string()))
            .await
            .unwrap();

        // The project-scope `codemux` entry must be suppressed: no
        // codemux row may carry a project-level source. (Scoped to
        // project sources rather than a global count because
        // `list_mcp_servers` also scans real user-level config paths —
        // on a developer machine where `codemux-remote serve` has
        // auto-registered itself into e.g. `~/.claude.json`, a
        // user-level codemux row legitimately coexists with the
        // hardcoded self row.)
        assert!(
            !result.iter().any(|s| {
                s.name == "codemux"
                    && s.sources.iter().any(|src| {
                        matches!(
                            src,
                            McpConfigSource::ClaudeProject | McpConfigSource::CodemuxProject
                        )
                    })
            }),
            "project-scope codemux entry leaked into {:?}",
            result
        );
        // The always-on hardcoded self row is present exactly once.
        assert_eq!(
            result
                .iter()
                .filter(|s| s.primary_source() == McpConfigSource::Codemux)
                .count(),
            1
        );
        // `demo` from the same file still shows up.
        assert!(result.iter().any(|s| s.name == "demo"));
    }

    /// What an entry runs decides, not what it is called: `codemux mcp`
    /// under another name is Codemux too, and is listed (and started) once,
    /// as the built-in row. Other `codemux` subcommands stay ordinary rows.
    #[tokio::test]
    async fn project_codemux_mcp_under_another_name_is_filtered_out() {
        let project = TempDir::new().unwrap();
        fs::write(
            project.path().join(".mcp.json"),
            r#"{
              "mcpServers": {
                "cmux":   { "command": "/usr/bin/codemux", "args": ["--client", "claude-code", "mcp"] },
                "notify": { "command": "codemux", "args": ["notify", "mcp"] },
                "demo":   { "command": "echo" }
              }
            }"#,
        )
        .unwrap();

        let result = discover_mcp_servers(Some(project.path().display().to_string()))
            .await
            .unwrap();

        assert!(!result.iter().any(|s| s.name == "cmux"), "codemux mcp leaked in as its own row: {result:?}");
        assert!(result.iter().any(|s| s.name == "notify"));
        assert!(result.iter().any(|s| s.name == "demo"));
    }

    /// `codemux mcp-approve`: lists what waits with its full definition,
    /// approves only a fingerprint it listed, lists it as approved, and
    /// `--revoke` stops it and makes it wait again.
    #[tokio::test]
    async fn host_approval_lists_approves_and_revokes_the_exact_definition() {
        use crate::mcp::runtime::McpServerStatus;
        use tauri::Manager;
        let (project, scratch) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let pid_file = scratch.path().join("pid");
        let fixture = format!("{}/tests/fixtures/plugin_mcp.py", env!("CARGO_MANIFEST_DIR"));
        fs::write(
            project.path().join(".mcp.json"),
            serde_json::json!({ "mcpServers": { "lint": {
                "command": "python3",
                "args": [fixture, "--pid-file", pid_file.display().to_string()],
                "env": { "NODE_OPTIONS": "--require ./x.js" },
            }}})
            .to_string(),
        )
        .unwrap();
        let app = tauri::test::mock_app();
        app.manage(crate::database::DatabaseStore::new_in_memory());
        app.manage(McpRegistry::new());

        let listed = project_approval(app.handle(), project.path(), None, None).await.unwrap();
        let pending = listed["pending"].as_array().unwrap();
        assert_eq!(pending.len(), 1, "{listed}");
        assert_eq!(listed["approved"], serde_json::json!([]));
        assert_eq!(pending[0]["name"], "lint");
        let summary = pending[0]["summary"].as_str().unwrap();
        let env = if cfg!(windows) { r#"NODE_OPTIONS="--require ./x.js""# } else { "NODE_OPTIONS='--require ./x.js'" };
        assert!(summary.contains(&format!("env {env}")), "{summary}");
        assert!(project_approval(app.handle(), project.path(), Some("stale"), None).await.is_err());
        assert!(!pid_file.exists());

        let fingerprint = pending[0]["fingerprint"].as_str().unwrap().to_string();
        let approved = project_approval(app.handle(), project.path(), Some(&fingerprint), None)
            .await
            .unwrap();
        assert_eq!(approved["approved"], "lint");
        assert_eq!(approved["status"]["kind"], "running", "{approved}");
        assert!(pid_file.exists(), "the approved server did not start");
        let listed = project_approval(app.handle(), project.path(), None, None).await.unwrap();
        assert_eq!(listed["pending"], serde_json::json!([]));
        assert_eq!(listed["approved"][0]["fingerprint"], fingerprint.as_str());

        let revoked = project_approval(app.handle(), project.path(), None, Some(&fingerprint))
            .await
            .unwrap();
        assert_eq!(revoked["revoked"], true);
        let registry = app.state::<McpRegistry>();
        let rows = registry.list_runtime().await;
        assert!(
            matches!(&rows[0].status, McpServerStatus::NeedsApproval { fingerprint: shown, .. } if *shown == fingerprint),
            "{rows:?}"
        );
        let listed = project_approval(app.handle(), project.path(), None, None).await.unwrap();
        assert_eq!(listed["pending"][0]["fingerprint"], fingerprint.as_str());
        registry.shutdown_all().await;
    }
}
