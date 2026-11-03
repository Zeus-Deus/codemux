//! The tools `codemux-remote serve` runs for agents on this host.
//!
//! They manage this host's folders, git worktrees and shells: the
//! workspaces they register show up under Devices in the Codemux app.
//! They do not run Codemux chats and do not control the Codemux app.
//!
//! Each tool is a thin function taking the daemon's shared state, an
//! [`Identity`] and its arguments, and returning a `serde_json::Value`
//! payload. Handlers don't branch on `Identity` in v1; see `identity.rs`
//! for why the argument exists anyway.
//!
//! Arguments follow the desktop's names where both have the concept
//! (`workspace_id`), and unknown arguments are refused, so a call written
//! for the desktop's tools fails loudly here instead of half-working. The
//! terminal tools keep their own names (`terminal_id`, `max_bytes`) and
//! every tool keeps its output shape: vexis-agent's remote dialect
//! depends on both.
//!
//! A new tool goes in [`catalog`] and in [`dispatch`], both in this file.
//! `codemux-remote mcp` lists [`catalog`] directly and the daemon's HTTP
//! routes serve it. Tests live in this file's `mod tests` and in
//! `remote/server.rs` and `remote/mcp.rs`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::identity::Identity;
use super::pty::PtyManager;
use super::workspace::{Workspace, WorkspaceStore};

/// Longest one tool call to the daemon may take. `worktree_create` is the
/// slow one: it fetches the base (itself capped at 10 s), checks out the
/// branch and copies the include files before it answers.
pub const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Static metadata for every tool the daemon exposes. `codemux-remote mcp`
/// builds its `tools/list` reply from this list, and the daemon's
/// `/tools/list` route serves it as is.
#[derive(Debug, Serialize, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema as a `serde_json::Value` so we can hand it to
    /// the MCP client as-is.
    pub input_schema: Value,
}

/// The argument that names a workspace. Models only see `workspace_id`;
/// `id` stays accepted for older callers.
fn workspace_id_property() -> Value {
    json!({ "type": "string", "description": "The workspace's id, from workspace_list or workspace_create." })
}

pub fn catalog() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "workspace_create",
            description: "Registers a folder in this host's codemux-remote registry (shown under Devices in the Codemux app) and returns the new workspace. It doesn't create files: make the folder first (for example `git clone` or `git init` in a terminal), then pass its `path`. To branch off a repository that is already on this host, use worktree_create instead.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Human-readable label. Defaults to basename of path." },
                    "path": { "type": "string", "description": "Absolute path to the working directory." },
                    "branch": { "type": "string", "description": "Git branch (optional)." },
                    "project_root": { "type": "string", "description": "Originating repo root. Optional: pass it only for a worktree (point at the parent repo). For a normal project checkout, leave it unset — the daemon derives it from `path`'s git root so the workspace always carries a project identity." }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "worktree_create",
            description: "Creates a git worktree on this host and registers it in this host's codemux-remote registry (shown under Devices in the Codemux app), in one call. Runs `git worktree add` under ~/.codemux/worktrees/<repo>/<branch>, fetching `base` from origin first so new branches start at the remote tip. Also provisions the worktree like the desktop: gitignored include files (.env & co) are copied from the parent repo before this returns, and the project's `.codemux/config.json` setup commands run in the background with CODEMUX_ROOT_PATH/CODEMUX_WORKSPACE_PATH/CODEMUX_BRANCH/CODEMUX_PORT set (see the `setup` field of the response). Asking again for a branch that already has a worktree returns it with `created: false`. Use this (NOT workspace_create) to fork a branch off an existing git repo on this host. For a brand-new project, first `git init` a folder (e.g. via terminal_spawn/terminal_write), then call this against it.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": { "type": "string", "description": "Absolute path to the existing git repository root to fork from." },
                    "branch": { "type": "string", "description": "Branch name for the worktree (kebab-case recommended)." },
                    "new_branch": { "type": "boolean", "description": "Create a new branch (true, default) or attach an existing local branch (false)." },
                    "base": { "type": "string", "description": "Base ref for a new branch (e.g. \"main\"). Defaults to the repo's current HEAD." },
                    "name": { "type": "string", "description": "Human-readable workspace label. Defaults to the worktree dir basename." }
                },
                "required": ["repo_path", "branch"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "workspace_list",
            description: "Lists the workspaces in this host's codemux-remote registry, newest first.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        },
        ToolSpec {
            name: "workspace_info",
            description: "Returns one workspace from this host's codemux-remote registry. `id` is accepted too.",
            input_schema: json!({
                "type": "object",
                "properties": { "workspace_id": workspace_id_property() },
                "required": ["workspace_id"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "workspace_update",
            description: "Updates a workspace's name, recorded branch or notes in this host's codemux-remote registry. Only the fields you pass change, and nothing on disk does. `id` is accepted too.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": workspace_id_property(),
                    "name": { "type": "string" },
                    "branch": { "type": "string" },
                    "notes": { "type": "string" }
                },
                "required": ["workspace_id"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "workspace_close",
            description: "Removes a workspace from this host's codemux-remote registry. It never deletes files: there is no delete_worktree, delete_branch or force_delete here. Remove a worktree with git, or close it from the Codemux app. `id` is accepted too.",
            input_schema: json!({
                "type": "object",
                "properties": { "workspace_id": workspace_id_property() },
                "required": ["workspace_id"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "terminal_spawn",
            description: "Starts a shell on this host in `cwd`. Returns the terminal id that terminal_write, terminal_read and terminal_close take.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "cwd": { "type": "string", "description": "Working directory for the shell." },
                    "command": { "type": "string", "description": "Override $SHELL (optional)." }
                },
                "required": ["cwd"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "terminal_write",
            description: "Writes to the stdin of a shell on this host. Include `\\n` for newline; the daemon does not append one.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "terminal_id": { "type": "string" },
                    "data": { "type": "string", "description": "UTF-8 bytes to send." }
                },
                "required": ["terminal_id", "data"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "terminal_read",
            description: "Reads the buffered output of a shell on this host. Returns up to 1 MiB. Use max_bytes to cap to a tail.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "terminal_id": { "type": "string" },
                    "max_bytes": { "type": "integer", "minimum": 1 }
                },
                "required": ["terminal_id"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "terminal_list",
            description: "Lists the shells codemux-remote is running on this host.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        },
        ToolSpec {
            name: "terminal_close",
            description: "Ends a shell on this host (SIGHUP to the shell) and drops its terminal.",
            input_schema: json!({
                "type": "object",
                "properties": { "terminal_id": { "type": "string" } },
                "required": ["terminal_id"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "app_status",
            description: "Status of this host's codemux-remote daemon: version, host id, start time, workspace count and terminal count. It says nothing about the Codemux app.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        },
    ]
}

#[derive(Debug, Serialize)]
pub struct ToolError {
    pub kind: &'static str,
    pub message: String,
}

impl ToolError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self { kind: "invalid_input", message: msg.into() }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self { kind: "not_found", message: msg.into() }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self { kind: "internal", message: msg.into() }
    }
}

pub type ToolResult = Result<Value, ToolError>;

/// Dispatch a single tool call against the shared state. Called by
/// both the HTTP server (each POST /tools/call) and the integration
/// tests (against the same state, bypassing the network).
pub fn dispatch(
    name: &str,
    params: &Value,
    _identity: &Identity, // v1: tools don't branch on it; see identity.rs
    workspaces: &WorkspaceStore,
    ptys: &PtyManager,
    started_at: &str,
) -> ToolResult {
    match name {
        "workspace_create" => workspace_create(params, workspaces),
        "worktree_create" => worktree_create(params, workspaces),
        "workspace_list" => workspace_list(workspaces),
        "workspace_info" => workspace_info(params, workspaces),
        "workspace_update" => workspace_update(params, workspaces),
        "workspace_close" => workspace_close(params, workspaces),
        "terminal_spawn" => terminal_spawn(params, ptys),
        "terminal_write" => terminal_write(params, ptys),
        "terminal_read" => terminal_read(params, ptys),
        "terminal_list" => terminal_list(ptys),
        "terminal_close" => terminal_close(params, ptys),
        "app_status" => app_status(workspaces, ptys, started_at),
        other => Err(ToolError::not_found(format!("unknown tool: {other}"))),
    }
}

// Every input refuses unknown fields: the schemas say
// `additionalProperties: false`, and a desktop-style call such as
// `workspace_close{delete_worktree: true}` must fail instead of quietly
// doing less than asked.

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateInput {
    name: Option<String>,
    path: String,
    branch: Option<String>,
    project_root: Option<String>,
}

fn workspace_create(params: &Value, store: &WorkspaceStore) -> ToolResult {
    let input: CreateInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    let ws = store
        .create(input.name, input.path, input.branch, input.project_root)
        .map_err(workspace_err)?;
    Ok(json!({ "workspace": ws }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorktreeCreateInput {
    repo_path: String,
    branch: String,
    #[serde(default)]
    new_branch: Option<bool>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

fn worktree_create(params: &Value, store: &WorkspaceStore) -> ToolResult {
    worktree_create_with(params, store, &SetupRunner::Thread)
}

/// Where a new worktree's setup commands run.
pub enum SetupRunner {
    /// A background thread. Right for the long-lived `serve` daemon.
    Thread,
    /// A detached `<exe> worktree setup` process logging to `log_dir`.
    /// Right for a one-shot CLI call, whose threads die when it exits.
    Detached { exe: PathBuf, log_dir: PathBuf },
}

/// The `worktree_create` tool with a choice of setup runner, so the
/// `codemux-remote worktree create` CLI can run it in-process when no
/// daemon is up.
pub fn worktree_create_with(params: &Value, store: &WorkspaceStore, runner: &SetupRunner) -> ToolResult {
    let input: WorktreeCreateInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;

    // The daemon process owns HOME; resolve the worktree root against it
    // so a new worktree lands at the same `~/.codemux/worktrees/...`
    // location the desktop uses.
    let home = dirs::home_dir()
        .ok_or_else(|| ToolError::internal("could not resolve home directory"))?;

    let created = super::git::create_worktree(
        &home,
        std::path::Path::new(&input.repo_path),
        &input.branch,
        input.new_branch.unwrap_or(true),
        input.base.as_deref(),
    )
    .map_err(ToolError::invalid)?;
    let path = created.worktree_path.to_string_lossy().to_string();
    let repo_root = created.repo_root.to_string_lossy().to_string();

    // Asking again for the same branch reuses the worktree, so reuse its
    // registry row too instead of listing it twice. A leftover row at the
    // same path for another branch (branch names can sanitise to the same
    // folder) is corrected rather than duplicated.
    let ws = match store.find_by_path(&path).map_err(workspace_err)? {
        Some(ws) if ws.branch.as_deref() == Some(created.branch.as_str()) => ws,
        Some(ws) => store
            .update(&ws.id, None, Some(created.branch.clone()), None)
            .map_err(workspace_err)?,
        // `project_root` is the parent repo (so the daemon stamps the
        // shared project_uid + `worktree` kind), exactly as the desktop
        // worktree-create path does.
        None => store
            .create(input.name, path, Some(created.branch.clone()), Some(repo_root.clone()))
            .map_err(workspace_err)?,
    };

    // Desktop parity (issue #78): provision the new worktree the same
    // way the desktop does after `git worktree add`. Provisioning never
    // fails the tool — a workspace whose setup script broke is still a
    // registered, usable workspace (matching the desktop, where setup
    // failures only surface as a notification). A reused worktree was
    // provisioned when it was made.
    let setup = if created.created {
        provision_worktree_workspace(&ws, runner)
    } else {
        Value::Null
    };

    Ok(json!({
        "workspace": ws,
        "setup": setup,
        "created": created.created,
        "repo_root": repo_root,
    }))
}

/// Headless equivalent of the desktop's `spawn_setup_scripts` pipeline:
/// copy gitignored include files (`.env` & co) from the parent repo,
/// then run the project's setup commands with the standard `CODEMUX_*`
/// env and the deterministic per-workspace port.
///
/// - The includes copy is fast and runs inline, so the files are in
///   place the moment `worktree_create` returns.
/// - Setup commands can take minutes (`npm install`), so they run in the
///   background (see [`SetupRunner`]) — same fire-and-forget shape as
///   the desktop — and the tool response only reports what was scheduled.
///
/// Differences from the desktop, by design:
/// - Config comes from `.codemux/config.json` (workspace dir → repo
///   root) only. The Settings-UI fallback lives in the desktop's
///   SQLite database, which does not exist on a headless host.
/// - Progress goes to stderr (the daemon's journal, or a detached run's
///   log file) instead of Tauri events, because there is no frontend to
///   notify.
fn provision_worktree_workspace(ws: &Workspace, runner: &SetupRunner) -> Value {
    let workspace_path = std::path::PathBuf::from(&ws.path);
    let root_path = crate::scripts::resolve_root_path(&workspace_path);
    let config = crate::config::workspace_config::read_workspace_config(&workspace_path);

    // Step 1: worktree includes (file → setting is desktop-only → defaults).
    let setting_patterns = config
        .as_ref()
        .map(|c| c.worktree_includes.clone())
        .unwrap_or_default();
    let includes_copied = match crate::scripts::process_worktree_includes(
        &root_path,
        &workspace_path,
        &setting_patterns,
    ) {
        Ok(result) => result.copied,
        Err(e) => {
            eprintln!(
                "[codemux-remote] worktree includes failed for workspace {}: {e}",
                ws.id
            );
            Vec::new()
        }
    };

    // Step 2: setup commands, in the background. The port is derived
    // from the workspace id exactly like the desktop, so a project's
    // setup script sees a stable CODEMUX_PORT for this workspace.
    let port = crate::scripts::allocate_workspace_port(&ws.id);
    let setup_commands = config.as_ref().map(|c| c.setup.len()).unwrap_or(0);
    let mut setup_running = false;
    let mut log = None;

    if let Some(config) = config.filter(|c| !c.setup.is_empty()) {
        match runner {
            SetupRunner::Thread => {
                let ws_id = ws.id.clone();
                let ws_name = ws.name.clone();
                let branch = ws.branch.clone();
                std::thread::spawn(move || {
                    let outcome = crate::scripts::run_setup_commands(
                        &workspace_path,
                        &ws_name,
                        &ws_id,
                        &config,
                        &root_path,
                        branch.as_deref(),
                        Some(port),
                        &mut |event| log_setup_event(&ws_id, event),
                    );
                    if let Err(e) = outcome {
                        eprintln!("[codemux-remote] setup failed for workspace {ws_id}: {e}");
                    }
                });
                setup_running = true;
            }
            SetupRunner::Detached { exe, log_dir } => match spawn_detached_setup(exe, log_dir, ws) {
                Ok(path) => {
                    setup_running = true;
                    log = Some(path.to_string_lossy().to_string());
                }
                Err(e) => eprintln!(
                    "[codemux-remote] couldn't start setup for workspace {}: {e}",
                    ws.id
                ),
            },
        }
    }

    json!({
        "port": port,
        "includes_copied": includes_copied,
        "setup_commands": setup_commands,
        "setup_running": setup_running,
        "log": log,
    })
}

fn log_setup_event(ws_id: &str, event: crate::scripts::SetupEvent<'_>) {
    match event {
        crate::scripts::SetupEvent::Progress {
            command,
            index,
            total,
        } => eprintln!(
            "[codemux-remote] setup {}/{total} for workspace {ws_id}: {command}",
            index + 1
        ),
        crate::scripts::SetupEvent::Failed {
            command, exit_code, ..
        } => eprintln!(
            "[codemux-remote] setup command `{command}` failed (exit {exit_code:?}) for workspace {ws_id}"
        ),
        crate::scripts::SetupEvent::Complete => {
            eprintln!("[codemux-remote] setup complete for workspace {ws_id}")
        }
    }
}

/// Start `<exe> worktree setup` for `ws` in its own session, with output
/// to a log file. `setsid` keeps the end of the SSH session that ran the
/// CLI from taking setup down with it, and not holding the CLI's
/// stdout/stderr matters too: ssh waits for every holder of those pipes
/// to close before it returns.
fn spawn_detached_setup(exe: &Path, log_dir: &Path, ws: &Workspace) -> Result<PathBuf, String> {
    use std::os::unix::process::CommandExt;

    std::fs::create_dir_all(log_dir).map_err(|e| format!("create {}: {e}", log_dir.display()))?;
    let log_path = log_dir.join(format!("setup-{}.log", ws.id));
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("open {}: {e}", log_path.display()))?;
    let log_err = log.try_clone().map_err(|e| format!("open {}: {e}", log_path.display()))?;

    let mut cmd = std::process::Command::new(exe);
    // `--flag=value` so values that start with `-` stay values.
    cmd.args(["worktree", "setup"])
        .arg(format!("--path={}", ws.path))
        .arg(format!("--workspace-id={}", ws.id))
        .arg(format!("--name={}", ws.name));
    if let Some(branch) = &ws.branch {
        cmd.arg(format!("--branch={branch}"));
    }
    cmd.stdin(std::process::Stdio::null()).stdout(log).stderr(log_err);
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| format!("spawn {}: {e}", exe.display()))?;
    Ok(log_path)
}

/// Body of the detached `codemux-remote worktree setup` process: run the
/// worktree's setup commands to completion, logging progress to stderr.
pub fn run_worktree_setup(
    workspace_path: &Path,
    name: &str,
    workspace_id: &str,
    branch: Option<&str>,
) -> Result<(), String> {
    let Some(config) = crate::config::workspace_config::read_workspace_config(workspace_path)
        .filter(|c| !c.setup.is_empty())
    else {
        return Ok(());
    };
    let root_path = crate::scripts::resolve_root_path(workspace_path);
    crate::scripts::run_setup_commands(
        workspace_path,
        name,
        workspace_id,
        &config,
        &root_path,
        branch,
        Some(crate::scripts::allocate_workspace_port(workspace_id)),
        &mut |event| log_setup_event(workspace_id, event),
    )
}

fn workspace_list(store: &WorkspaceStore) -> ToolResult {
    let list = store.list().map_err(workspace_err)?;
    Ok(json!({ "workspaces": list }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceIdInput {
    /// `id` was this tool's first name for it; vexis-agent's remote
    /// dialect still sends `{"id": …}`. Sending both is a duplicate field.
    #[serde(alias = "id")]
    workspace_id: String,
}

fn workspace_info(params: &Value, store: &WorkspaceStore) -> ToolResult {
    let input: WorkspaceIdInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    let ws = store.get(&input.workspace_id).map_err(workspace_err)?;
    Ok(json!({ "workspace": ws }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateInput {
    /// Also accepted as `id`, like [`WorkspaceIdInput`].
    #[serde(alias = "id")]
    workspace_id: String,
    name: Option<String>,
    branch: Option<String>,
    notes: Option<String>,
}

fn workspace_update(params: &Value, store: &WorkspaceStore) -> ToolResult {
    let input: UpdateInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    let ws = store
        .update(&input.workspace_id, input.name, input.branch, input.notes)
        .map_err(workspace_err)?;
    Ok(json!({ "workspace": ws }))
}

fn workspace_close(params: &Value, store: &WorkspaceStore) -> ToolResult {
    let input: WorkspaceIdInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    // Registry-only: `WorkspaceStore::close` deletes the SQLite row and
    // touches nothing on disk, so the protected-root file-deletion
    // guard the desktop close path enforces
    // (`commands::workspace::refuse_worktree_removal`) has no
    // equivalent here — there are no files this tool could delete. If a
    // future revision adds worktree removal, it must adopt that guard.
    store.close(&input.workspace_id).map_err(workspace_err)?;
    Ok(json!({ "closed": input.workspace_id }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnInput {
    cwd: String,
    command: Option<String>,
}

fn terminal_spawn(params: &Value, ptys: &PtyManager) -> ToolResult {
    let input: SpawnInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    let info = ptys
        .spawn(std::path::PathBuf::from(input.cwd), input.command)
        .map_err(pty_err)?;
    Ok(json!({ "terminal": info }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    terminal_id: String,
    data: String,
}

fn terminal_write(params: &Value, ptys: &PtyManager) -> ToolResult {
    let input: WriteInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    ptys.write(&input.terminal_id, input.data.as_bytes())
        .map_err(pty_err)?;
    Ok(json!({ "written": input.data.len() }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    terminal_id: String,
    max_bytes: Option<usize>,
}

fn terminal_read(params: &Value, ptys: &PtyManager) -> ToolResult {
    let input: ReadInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    let bytes = ptys
        .read(&input.terminal_id, input.max_bytes)
        .map_err(pty_err)?;
    // Lossy UTF-8: PTY output is overwhelmingly text + ANSI escapes.
    // Anyone needing the raw bytes can base64 them at a higher
    // protocol revision.
    Ok(json!({
        "data": String::from_utf8_lossy(&bytes).into_owned(),
        "byte_count": bytes.len(),
    }))
}

fn terminal_list(ptys: &PtyManager) -> ToolResult {
    Ok(json!({ "terminals": ptys.list() }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalIdInput {
    terminal_id: String,
}

fn terminal_close(params: &Value, ptys: &PtyManager) -> ToolResult {
    let input: TerminalIdInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    ptys.close(&input.terminal_id).map_err(pty_err)?;
    Ok(json!({ "closed": input.terminal_id }))
}

fn app_status(
    workspaces: &WorkspaceStore,
    ptys: &PtyManager,
    started_at: &str,
) -> ToolResult {
    let workspace_count = workspaces.list().map_err(workspace_err)?.len();
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "host_id": workspaces.host_id(),
        "started_at": started_at,
        "workspace_count": workspace_count,
        "terminal_count": ptys.list().len(),
        "mode": "headless",
    }))
}

fn workspace_err(e: super::workspace::WorkspaceError) -> ToolError {
    use super::workspace::WorkspaceError::*;
    match e {
        NotFound(s) => ToolError::not_found(s),
        Invalid(s) => ToolError::invalid(s),
        Db(s) | Io(s) => ToolError::internal(s),
    }
}

fn pty_err(e: super::pty::PtyError) -> ToolError {
    use super::pty::PtyError::*;
    match e {
        NotFound(s) => ToolError::not_found(s),
        Io(s) => ToolError::internal(s),
    }
}

#[allow(dead_code)] // Workspace type re-exported only so external callers can name it
pub fn _workspace_typename() -> &'static str {
    std::any::type_name::<Workspace>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::process::Command;
    use tempfile::TempDir;

    fn open_store(dir: &TempDir) -> WorkspaceStore {
        WorkspaceStore::open(
            &dir.path().join("codemux.db"),
            "test-host".into(),
            dir.path().join("workspaces"),
        )
        .unwrap()
    }

    fn init_repo(dir: &std::path::Path) {
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(dir)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "git {args:?}"
            );
        };
        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "t@e.com"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);
    }

    #[test]
    #[serial]
    fn worktree_create_dispatch_registers_worktree_workspace() {
        // The handler resolves HOME via dirs::home_dir(); point it at a
        // temp dir for the duration of this (serialized) test so the
        // worktree lands somewhere disposable.
        let home = TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());

        let repo = TempDir::new().unwrap();
        init_repo(repo.path());
        let store_dir = TempDir::new().unwrap();
        let store = open_store(&store_dir);

        let params = json!({
            "repo_path": repo.path().to_string_lossy(),
            "branch": "feature/login",
            "base": "main"
        });
        let out = worktree_create(&params, &store).expect("worktree_create");
        let ws = &out["workspace"];

        // Registered as a worktree of the repo, with a populated checkout
        // under ~/.codemux/worktrees/<repo>/<branch>.
        assert_eq!(ws["kind"], "worktree");
        // The git branch name is preserved verbatim; only the on-disk
        // path segment is sanitized (`feature/login` → `feature-login`).
        assert_eq!(ws["branch"], "feature/login");
        let path = ws["path"].as_str().unwrap();
        assert!(path.contains("/.codemux/worktrees/"), "path: {path}");
        assert!(path.ends_with("/feature-login"), "sanitized path segment: {path}");
        assert!(std::path::Path::new(path).join("f.txt").exists(), "checkout populated");
        assert_eq!(
            ws["project_root"].as_str().unwrap(),
            repo.path().to_string_lossy()
        );
        // main + worktree of the same local repo share a project_uid.
        assert!(ws["project_uid"].is_string());

        // restore HOME
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// Commit any pending changes in `dir` (used to land `.codemux/`
    /// config + `.gitignore` after `init_repo`).
    fn commit_all(dir: &std::path::Path, message: &str) {
        for args in [&["add", "."][..], &["commit", "-m", message][..]] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(dir)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "git {args:?}"
            );
        }
    }

    /// Desktop-parity provisioning (issue #78): the daemon's
    /// worktree_create must copy gitignored include files into the new
    /// worktree and run the project's setup script with the standard
    /// CODEMUX_* env + deterministic port.
    #[test]
    #[serial]
    fn worktree_create_runs_setup_scripts_with_env_and_includes() {
        let home = TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());

        let repo = TempDir::new().unwrap();
        init_repo(repo.path());
        // Gitignored .env in the parent repo — the includes step must
        // copy it into the worktree (defaults cover `.env`).
        std::fs::write(repo.path().join(".gitignore"), ".env\nsetup-ran.txt\n").unwrap();
        std::fs::write(repo.path().join(".env"), "SECRET=from-root").unwrap();
        // Committed setup config: the script records its env into a
        // marker file inside the worktree.
        std::fs::create_dir_all(repo.path().join(".codemux")).unwrap();
        std::fs::write(
            repo.path().join(".codemux/config.json"),
            r#"{"setup": ["printf '%s|%s|%s|%s|%s' \"$CODEMUX_BRANCH\" \"$CODEMUX_PORT\" \"$CODEMUX_ROOT_PATH\" \"$CODEMUX_WORKSPACE_PATH\" \"$CODEMUX_WORKSPACE_ID\" > setup-ran.txt"]}"#,
        )
        .unwrap();
        commit_all(repo.path(), "add provisioning config");

        let store_dir = TempDir::new().unwrap();
        let store = open_store(&store_dir);
        let params = json!({
            "repo_path": repo.path().to_string_lossy(),
            "branch": "feature/provision",
            "base": "main"
        });
        let out = worktree_create(&params, &store).expect("worktree_create");
        let ws = &out["workspace"];
        let setup = &out["setup"];
        let wt = std::path::PathBuf::from(ws["path"].as_str().unwrap());

        // The tool response reports the provisioning summary.
        assert_eq!(setup["setup_commands"], 1);
        assert_eq!(setup["setup_running"], true);
        let port = setup["port"].as_u64().expect("port") as u16;
        assert!((3100..6500).contains(&port), "port {port} out of range");
        assert!(
            setup["includes_copied"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == ".env"),
            "includes_copied: {}",
            setup["includes_copied"]
        );

        // Includes are copied synchronously: present as soon as the
        // tool returns.
        assert_eq!(
            std::fs::read_to_string(wt.join(".env")).unwrap(),
            "SECRET=from-root"
        );

        // Setup commands run in the background; poll for the marker.
        let marker = wt.join("setup-ran.txt");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "setup script never ran (no {})",
                marker.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let recorded = std::fs::read_to_string(&marker).unwrap();
        let parts: Vec<&str> = recorded.split('|').collect();
        assert_eq!(parts.len(), 5, "marker: {recorded}");
        assert_eq!(parts[0], "feature/provision", "CODEMUX_BRANCH");
        assert_eq!(parts[1], port.to_string(), "CODEMUX_PORT");
        assert_eq!(
            std::fs::canonicalize(parts[2]).unwrap(),
            std::fs::canonicalize(repo.path()).unwrap(),
            "CODEMUX_ROOT_PATH should be the parent repo root"
        );
        assert_eq!(
            std::fs::canonicalize(parts[3]).unwrap(),
            std::fs::canonicalize(&wt).unwrap(),
            "CODEMUX_WORKSPACE_PATH should be the worktree"
        );
        assert_eq!(parts[4], ws["id"].as_str().unwrap(), "CODEMUX_WORKSPACE_ID");

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// No `.codemux/config.json` → no setup commands, but the tool
    /// still succeeds and reports an allocated port (graceful path).
    #[test]
    #[serial]
    fn worktree_create_without_setup_config_is_graceful() {
        let home = TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());

        let repo = TempDir::new().unwrap();
        init_repo(repo.path());
        let store_dir = TempDir::new().unwrap();
        let store = open_store(&store_dir);
        let params = json!({
            "repo_path": repo.path().to_string_lossy(),
            "branch": "no-config",
            "base": "main"
        });
        let out = worktree_create(&params, &store).expect("worktree_create");
        assert_eq!(out["setup"]["setup_commands"], 0);
        assert_eq!(out["setup"]["setup_running"], false);
        assert!(out["setup"]["port"].as_u64().is_some());

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// A failing setup script must not fail worktree_create: the
    /// workspace is still created and registered (matching the desktop,
    /// where setup failures only surface as a notification).
    #[test]
    #[serial]
    fn worktree_create_with_failing_setup_still_succeeds() {
        let home = TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());

        let repo = TempDir::new().unwrap();
        init_repo(repo.path());
        std::fs::create_dir_all(repo.path().join(".codemux")).unwrap();
        std::fs::write(
            repo.path().join(".codemux/config.json"),
            r#"{"setup": ["exit 7"]}"#,
        )
        .unwrap();
        commit_all(repo.path(), "add failing setup");

        let store_dir = TempDir::new().unwrap();
        let store = open_store(&store_dir);
        let params = json!({
            "repo_path": repo.path().to_string_lossy(),
            "branch": "failing-setup",
            "base": "main"
        });
        let out = worktree_create(&params, &store)
            .expect("tool must not fail when the setup script fails");
        assert_eq!(out["setup"]["setup_commands"], 1);
        assert!(out["workspace"]["id"].is_string());

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// Asking again for the same branch returns the same worktree and
    /// registry row, flagged `created: false`, without re-provisioning.
    #[test]
    #[serial]
    fn worktree_create_again_reuses_the_worktree_and_its_row() {
        let home = TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());

        let repo = TempDir::new().unwrap();
        init_repo(repo.path());
        let store_dir = TempDir::new().unwrap();
        let store = open_store(&store_dir);
        let params = json!({
            "repo_path": repo.path().to_string_lossy(),
            "branch": "again",
            "base": "main"
        });
        let first = worktree_create(&params, &store).expect("first");
        assert_eq!(first["created"], true);
        assert_eq!(first["repo_root"], repo.path().to_string_lossy().as_ref());

        let second = worktree_create(&params, &store).expect("second");
        assert_eq!(second["created"], false);
        assert_eq!(second["workspace"]["id"], first["workspace"]["id"]);
        assert!(second["setup"].is_null(), "a reused worktree isn't provisioned again");
        assert_eq!(store.list().unwrap().len(), 1, "no duplicate row");

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// The CLI's runner: setup goes to a separate `<exe> worktree setup`
    /// process in its own session, logging to a file, so it survives the
    /// one-shot CLI (and its SSH session) exiting.
    #[test]
    #[serial]
    fn detached_runner_starts_setup_in_its_own_session() {
        let home = TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());

        let repo = TempDir::new().unwrap();
        init_repo(repo.path());
        std::fs::create_dir_all(repo.path().join(".codemux")).unwrap();
        std::fs::write(repo.path().join(".codemux/config.json"), r#"{"setup": ["true"]}"#).unwrap();
        commit_all(repo.path(), "add setup");

        // Stand-in for codemux-remote: records its args and whether it
        // leads its own session (sid == pid after setsid).
        let bin = TempDir::new().unwrap();
        let marker = bin.path().join("ran.txt");
        let exe = bin.path().join("fake-codemux-remote");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\nsid=$(ps -o sid= -p $$ | tr -d ' ')\necho \"$sid $$ $*\" > {}\necho logged\n",
                marker.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        let store_dir = TempDir::new().unwrap();
        let store = open_store(&store_dir);
        let log_dir = store_dir.path().join("logs");
        let runner = SetupRunner::Detached { exe, log_dir: log_dir.clone() };
        let params = json!({
            "repo_path": repo.path().to_string_lossy(),
            "branch": "detached",
            "base": "main"
        });
        let out = worktree_create_with(&params, &store, &runner).expect("worktree_create");
        assert_eq!(out["setup"]["setup_running"], true);
        let log = std::path::PathBuf::from(out["setup"]["log"].as_str().expect("log path"));
        assert!(log.starts_with(&log_dir));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !marker.exists() || std::fs::read_to_string(&log).unwrap_or_default().is_empty() {
            assert!(std::time::Instant::now() < deadline, "detached setup never ran");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let recorded = std::fs::read_to_string(&marker).unwrap();
        let mut parts = recorded.split_whitespace();
        let (sid, pid) = (parts.next().unwrap(), parts.next().unwrap());
        assert_eq!(sid, pid, "setup runs as its own session leader: {recorded}");
        let ws = &out["workspace"];
        assert!(recorded.contains("worktree setup"), "{recorded}");
        assert!(recorded.contains(&format!("--workspace-id={}", ws["id"].as_str().unwrap())));
        assert!(recorded.contains("--branch=detached"));
        assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "logged");

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// The detached process body runs the setup commands to completion
    /// with the same env the daemon's thread uses.
    #[test]
    fn run_worktree_setup_runs_commands_synchronously() {
        let repo = TempDir::new().unwrap();
        init_repo(repo.path());
        std::fs::create_dir_all(repo.path().join(".codemux")).unwrap();
        std::fs::write(
            repo.path().join(".codemux/config.json"),
            r#"{"setup": ["printf '%s|%s' \"$CODEMUX_BRANCH\" \"$CODEMUX_WORKSPACE_ID\" > setup-ran.txt"]}"#,
        )
        .unwrap();

        run_worktree_setup(repo.path(), "ws", "ws-123", Some("feat")).expect("setup");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("setup-ran.txt")).unwrap(),
            "feat|ws-123"
        );

        std::fs::write(repo.path().join(".codemux/config.json"), r#"{"setup": ["exit 3"]}"#).unwrap();
        assert!(run_worktree_setup(repo.path(), "ws", "ws-123", None).is_err());
    }

    #[test]
    #[serial]
    fn worktree_create_dispatch_rejects_non_repo() {
        let home = TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());

        let not_repo = TempDir::new().unwrap();
        let store_dir = TempDir::new().unwrap();
        let store = open_store(&store_dir);
        let params = json!({ "repo_path": not_repo.path().to_string_lossy(), "branch": "x" });
        let err = worktree_create(&params, &store).unwrap_err();
        assert_eq!(err.kind, "invalid_input");
        assert!(err.message.contains("not a git repository"));

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// Runs `body` with `HOME` pointed at a fresh temp dir, so anything a
    /// tool writes under `~/.codemux` lands somewhere disposable. Callers
    /// are `#[serial]`: the environment is process-wide.
    fn with_temp_home(body: impl FnOnce(&std::path::Path)) {
        let home = TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(home.path())));
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }

    fn call(name: &str, params: Value, store: &WorkspaceStore) -> ToolResult {
        dispatch(
            name,
            &params,
            &Identity::local(),
            store,
            &PtyManager::new(),
            "2026-01-01T00:00:00Z",
        )
    }

    fn registered(store: &WorkspaceStore) -> String {
        let out = call(
            "workspace_create",
            json!({ "path": "/tmp/remote-tools-ws", "name": "ws" }),
            store,
        )
        .expect("workspace_create");
        out["workspace"]["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn workspace_tools_accept_workspace_id_and_the_older_id() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        let id = registered(&store);

        for args in [json!({ "workspace_id": id }), json!({ "id": id })] {
            let out = call("workspace_info", args.clone(), &store)
                .unwrap_or_else(|e| panic!("{args}: {e:?}"));
            assert_eq!(out["workspace"]["id"], json!(id), "{args}");
        }
        let updated = call(
            "workspace_update",
            json!({ "id": id, "notes": "n" }),
            &store,
        )
        .expect("update by id");
        assert_eq!(updated["workspace"]["notes"], json!("n"));
        let updated = call(
            "workspace_update",
            json!({ "workspace_id": id, "name": "renamed" }),
            &store,
        )
        .expect("update by workspace_id");
        assert_eq!(updated["workspace"]["name"], json!("renamed"));

        for tool in ["workspace_info", "workspace_update", "workspace_close"] {
            let both = call(tool, json!({ "workspace_id": id, "id": id }), &store).unwrap_err();
            assert_eq!(both.kind, "invalid_input", "{tool} with both keys");
            assert!(
                both.message.contains("duplicate field `workspace_id`"),
                "{tool}: {}",
                both.message
            );
            let neither = call(tool, json!({}), &store).unwrap_err();
            assert_eq!(neither.kind, "invalid_input", "{tool} with neither key");
            assert!(
                neither.message.contains("missing field `workspace_id`"),
                "{tool}: {}",
                neither.message
            );
        }
        assert_eq!(
            store.list().unwrap().len(),
            1,
            "no failed call closed the row"
        );

        let closed = call("workspace_close", json!({ "id": id }), &store).expect("close by id");
        assert_eq!(closed, json!({ "closed": id }));
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn workspace_close_refuses_desktop_delete_flags() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        let id = registered(&store);

        for flag in ["delete_worktree", "delete_branch", "force_delete"] {
            let err = call(
                "workspace_close",
                json!({ "workspace_id": id, flag: true }),
                &store,
            )
            .unwrap_err();
            assert_eq!(err.kind, "invalid_input", "{flag}");
            assert!(
                err.message.contains(&format!("unknown field `{flag}`")),
                "{flag}: {}",
                err.message
            );
        }
        assert_eq!(store.get(&id).unwrap().id, id, "the row still exists");
    }

    /// Every tool that takes arguments refuses one it doesn't know, before it
    /// does any work. The desktop's argument names are the likely mistakes.
    #[test]
    #[serial]
    fn tool_inputs_reject_unknown_fields() {
        with_temp_home(|home| {
            let repo = TempDir::new().unwrap();
            init_repo(repo.path());
            let dir = TempDir::new().unwrap();
            let store = open_store(&dir);
            let repo_path = repo.path().to_string_lossy().to_string();

            let err = call(
                "worktree_create",
                json!({ "repo_path": repo_path, "branch": "b", "layout": "single" }),
                &store,
            )
            .unwrap_err();
            assert_eq!(err.kind, "invalid_input");
            assert!(
                err.message.contains("unknown field `layout`"),
                "{}",
                err.message
            );
            assert!(
                store.list().unwrap().is_empty(),
                "no workspace was registered"
            );
            assert!(!home.join(".codemux").exists(), "no worktree was created");
            let worktrees = Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(["worktree", "list"])
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&worktrees.stdout).lines().count(),
                1,
                "only the main checkout"
            );

            let id = registered(&store);
            let cases = [
                (
                    "workspace_create",
                    json!({ "path": "/tmp/x", "select": true }),
                    "select",
                ),
                (
                    "worktree_create",
                    json!({ "repo_path": repo_path, "branch": "b", "initial_prompt": "go" }),
                    "initial_prompt",
                ),
                (
                    "workspace_info",
                    json!({ "workspace_id": id, "detail": "full" }),
                    "detail",
                ),
                (
                    "workspace_update",
                    json!({ "workspace_id": id, "title": "t" }),
                    "title",
                ),
                (
                    "workspace_close",
                    json!({ "workspace_id": id, "remove_worktree": true }),
                    "remove_worktree",
                ),
                (
                    "terminal_spawn",
                    json!({ "cwd": "/tmp", "workspace_id": id }),
                    "workspace_id",
                ),
                (
                    "terminal_write",
                    json!({ "terminal_id": "t", "data": "x", "session_id": "s" }),
                    "session_id",
                ),
                (
                    "terminal_read",
                    json!({ "terminal_id": "t", "lines": 5 }),
                    "lines",
                ),
                (
                    "terminal_close",
                    json!({ "terminal_id": "t", "force": true }),
                    "force",
                ),
            ];
            for (tool, args, field) in cases {
                let err = call(tool, args, &store).unwrap_err();
                assert_eq!(err.kind, "invalid_input", "{tool}");
                assert!(
                    err.message.contains(&format!("unknown field `{field}`")),
                    "{tool}: {}",
                    err.message
                );
            }
            assert_eq!(
                store.list().unwrap().len(),
                1,
                "only the workspace registered above"
            );
        });
    }

    #[test]
    fn catalog_names_workspaces_by_workspace_id() {
        let catalog = catalog();
        assert_eq!(catalog.len(), 12);
        for tool in ["workspace_info", "workspace_update", "workspace_close"] {
            let spec = catalog.iter().find(|spec| spec.name == tool).unwrap();
            assert_eq!(
                spec.input_schema["required"],
                json!(["workspace_id"]),
                "{tool}"
            );
            assert_eq!(
                spec.input_schema["additionalProperties"],
                json!(false),
                "{tool}"
            );
            // One name for models; `id` is only mentioned, never advertised.
            assert!(
                spec.input_schema["properties"].get("id").is_none(),
                "{tool}"
            );
            assert!(spec.description.contains("`id` is accepted too"), "{tool}");
        }
        for spec in &catalog {
            assert_eq!(
                spec.input_schema["additionalProperties"],
                json!(false),
                "{}",
                spec.name
            );
            assert!(
                spec.description.contains("this host"),
                "{} says what it acts on",
                spec.name
            );
        }
    }

    #[test]
    fn app_status_keeps_the_fields_vexis_detects() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir);
        let out = call("app_status", json!({}), &store).expect("app_status");
        assert_eq!(out["host_id"], json!("test-host"));
        assert_eq!(out["mode"], json!("headless"));
        // The desktop dialect's discriminators must never appear here.
        assert!(out.get("active_workspace_id").is_none());
        assert!(out.get("focused_pane_id").is_none());
    }

    /// The arguments the desktop's `codemux-remote worktree create` builds
    /// carry only keys the tool knows.
    #[test]
    #[serial]
    fn desktop_worktree_create_arguments_still_work() {
        with_temp_home(|_| {
            let repo = TempDir::new().unwrap();
            init_repo(repo.path());
            let dir = TempDir::new().unwrap();
            let store = open_store(&dir);
            for base in [None, Some("main")] {
                let branch = format!("desktop-{}", base.unwrap_or("default"));
                let args =
                    crate::remote::checkout::worktree_create_args(repo.path(), &branch, base)
                        .unwrap();
                let out = call("worktree_create", args.clone(), &store)
                    .unwrap_or_else(|e| panic!("{args}: {e:?}"));
                assert_eq!(out["created"], json!(true), "{args}");
                assert_eq!(out["workspace"]["branch"], json!(branch));
            }
        });
    }
}
