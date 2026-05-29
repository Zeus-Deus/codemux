//! Headless MCP tool implementations.
//!
//! Each tool is a thin function taking the dispatcher's shared
//! state, an [`Identity`], and tool-specific params; returning a
//! `serde_json::Value` payload. Handlers don't branch on `Identity`
//! in v1 — see `identity.rs` for why the argument exists anyway.
//!
//! The tool surface is deliberately narrower than the desktop's
//! `mcp_server.rs` (which advertises 50+ tools). On a headless
//! host there are no panes, no browser, no system tray. The set
//! below covers the headline use case: an agent on the remote can
//! create workspaces, list them, write to and read from shells,
//! and inspect state.
//!
//! New tools should be added here and registered in [`Catalog`] and
//! the dispatch table in `server.rs`. Tests in `tests/remote_e2e.rs`
//! exercise the dispatcher end-to-end so a forgotten registration
//! is caught at CI time.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::identity::Identity;
use super::pty::PtyManager;
use super::workspace::{Workspace, WorkspaceStore};

/// Static metadata for every tool the daemon exposes. The MCP
/// `tools/list` JSON-RPC response is built directly from this slice.
#[derive(Debug, Serialize, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema as a `serde_json::Value` so we can hand it to
    /// the MCP client as-is.
    pub input_schema: Value,
}

pub fn catalog() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "workspace_create",
            description: "Create a new workspace on this host. Records it in the daemon's registry. Returns the new workspace's id and metadata. This does not materialise files on disk — create the project folder first (e.g. `git clone`/`git init` via the terminal tools), then register it here by passing its `path`.",
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
            description: "Create a git worktree + register a Codemux workspace in one call — the headless equivalent of the desktop's worktree_create. Runs `git worktree add` under ~/.codemux/worktrees/<repo>/<branch> and records the resulting workspace. Use this (NOT workspace_create) to fork a branch off an existing git repo on this host. For a brand-new project, first `git init` a folder (e.g. via terminal_spawn/terminal_write), then call this against it.",
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
            description: "List every workspace registered with this daemon, newest first.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        },
        ToolSpec {
            name: "workspace_info",
            description: "Get full metadata for a single workspace by id.",
            input_schema: json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "workspace_update",
            description: "Update mutable fields on a workspace (name, branch, notes). Other fields stay as-is.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "branch": { "type": "string" },
                    "notes": { "type": "string" }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "workspace_close",
            description: "Remove a workspace from the daemon's registry. Does not delete the worktree files on disk — that's the caller's job.",
            input_schema: json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "terminal_spawn",
            description: "Spawn a new shell PTY in a given working directory. Returns the terminal id used by terminal_write/terminal_read.",
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
            description: "Write bytes to a terminal's stdin. Include `\\n` for newline; the daemon does not append one.",
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
            description: "Read accumulated output from a terminal's PTY buffer. Returns up to 1 MiB. Use max_bytes to cap to a tail.",
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
            description: "List every PTY the daemon currently owns.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        },
        ToolSpec {
            name: "terminal_close",
            description: "Kill the terminal (SIGHUP to the shell, drop PTY).",
            input_schema: json!({
                "type": "object",
                "properties": { "terminal_id": { "type": "string" } },
                "required": ["terminal_id"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "app_status",
            description: "Daemon status: version, host id, uptime, workspace count, terminal count.",
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

#[derive(Debug, Deserialize)]
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

    // Register the worktree as a workspace. `project_root` is the parent
    // repo (so the daemon stamps the shared project_uid + `worktree`
    // kind), exactly as the desktop worktree-create path does.
    let ws = store
        .create(
            input.name,
            created.worktree_path.to_string_lossy().to_string(),
            Some(created.branch),
            Some(created.repo_root.to_string_lossy().to_string()),
        )
        .map_err(workspace_err)?;
    Ok(json!({ "workspace": ws }))
}

fn workspace_list(store: &WorkspaceStore) -> ToolResult {
    let list = store.list().map_err(workspace_err)?;
    Ok(json!({ "workspaces": list }))
}

#[derive(Debug, Deserialize)]
struct IdInput {
    id: String,
}

fn workspace_info(params: &Value, store: &WorkspaceStore) -> ToolResult {
    let input: IdInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    let ws = store.get(&input.id).map_err(workspace_err)?;
    Ok(json!({ "workspace": ws }))
}

#[derive(Debug, Deserialize)]
struct UpdateInput {
    id: String,
    name: Option<String>,
    branch: Option<String>,
    notes: Option<String>,
}

fn workspace_update(params: &Value, store: &WorkspaceStore) -> ToolResult {
    let input: UpdateInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    let ws = store
        .update(&input.id, input.name, input.branch, input.notes)
        .map_err(workspace_err)?;
    Ok(json!({ "workspace": ws }))
}

fn workspace_close(params: &Value, store: &WorkspaceStore) -> ToolResult {
    let input: IdInput =
        serde_json::from_value(params.clone()).map_err(|e| ToolError::invalid(e.to_string()))?;
    store.close(&input.id).map_err(workspace_err)?;
    Ok(json!({ "closed": input.id }))
}

#[derive(Debug, Deserialize)]
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
}
