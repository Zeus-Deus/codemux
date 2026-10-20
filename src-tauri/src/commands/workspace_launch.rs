//! Independent top-level workspaces launched by an identified in-app chat.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

use crate::agent_control::{service, ControlAccess, ControlCaller};
use crate::agent_provider::ProviderKind;
use crate::mcp::registry::{HostCaller, HostTools};
use crate::mcp::runtime::{prefix_tool_name, McpTool};
use crate::state::{AppStateStore, NotificationLevel, WorkspaceSnapshot};

use super::delegation::{offered_agents, parent_is_full, provider_id, DelegationState};

const TOOL_NAME: &str = "workspace_launch";
const MAX_IN_FLIGHT: usize = 6;
const TOOL_DESCRIPTION: &str = "Start a NEW top-level workspace with its own git worktree, branch and agent chat. Use ONLY when the user asks for separate/new workspaces, threads, or parallel independent work they will follow in the sidebar. Its results do NOT come back to this chat; use delegate_task when you need results back. Call workspace_launch once per workspace; several calls in one turn are fine. The caller stays in this chat.

The new agent sees ONLY `message`. Write a self-contained brief: goal, relevant files, constraints, how to verify, and whether to open a PR. Omit provider to use your own provider. Pass model and effort only when the user asked for them; otherwise omit for that provider's defaults. The new chat runs with full access, like this chat.

It returns at once while the workspace starts. Give the user a one-line status naming the workspaces and END YOUR TURN. They appear in the sidebar shortly; do not poll or wait. Startup failures appear in Codemux notifications.";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchArgs {
    title: String,
    message: String,
    branch: Option<String>,
    base: Option<String>,
    provider: Option<ProviderKind>,
    model: Option<String>,
    effort: Option<String>,
}

impl LaunchArgs {
    fn parse(arguments: &Value) -> Result<Self, String> {
        let input: Self = serde_json::from_value(arguments.clone()).map_err(|error| {
            format!("Pass title, message and optional launch settings: {error}")
        })?;
        service::identifier(&input.title, "title", 100).map_err(|error| error.message)?;
        if input.message.trim().is_empty() || input.message.len() > 65536 {
            return Err("message must contain text and fit in 65536 bytes".into());
        }
        for (name, value) in [
            ("branch", &input.branch),
            ("base", &input.base),
            ("model", &input.model),
            ("effort", &input.effort),
        ] {
            if let Some(value) = value {
                service::identifier(value, name, 256).map_err(|error| error.message)?;
                if matches!(name, "branch" | "base") && value.starts_with('-') {
                    return Err(format!("{name} must not start with '-'."));
                }
            }
        }
        Ok(input)
    }
}

fn eligible(caller: &HostCaller, is_child: bool, workspace: &WorkspaceSnapshot) -> bool {
    parent_is_full(caller.provider, caller.permission_mode.as_deref())
        && !is_child
        && workspace.host_id.is_none()
        && workspace.remote_cwd.is_none()
        && !workspace.is_local_import_snapshot_only()
        && workspace.is_git
}

fn caller_workspace<R: Runtime>(
    app: &AppHandle<R>,
    caller: &HostCaller,
) -> Result<(WorkspaceSnapshot, PathBuf), String> {
    let state = app.state::<AppStateStore>();
    let workspace = state
        .agent_chat_pane_id_for_thread(&caller.thread_id)
        .and_then(|pane| state.workspace_id_for_pane(&pane))
        .or_else(|| caller.workspace_id.clone())
        .and_then(|id| state.find_workspace(&id))
        .ok_or_else(|| "This chat has no open workspace.".to_string())?;
    let is_child = app
        .state::<DelegationState>()
        .is_running_child(&caller.thread_id);
    if !eligible(caller, is_child, &workspace) {
        return Err("Workspace launch needs a main Claude or Codex chat in Full access, in a local git workspace (not a delegated task or imported snapshot).".into());
    }
    let root = workspace.project_root.as_deref().unwrap_or(&workspace.cwd);
    let repo = crate::config::workspace_config::find_git_root(Path::new(root))
        .ok_or_else(|| "This workspace is not inside a git repository.".to_string())?;
    Ok((workspace, repo))
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = std::process::Command::new("git");
    command
        .current_dir(repo)
        .args(args)
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    crate::execution::sanitize_gui_env_std(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("Could not run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn resolve_base(
    workspace: &WorkspaceSnapshot,
    repo: &Path,
    base: Option<&str>,
) -> Result<String, String> {
    // Resolve before the async hand-off, from the caller's checkout rather
    // than the parent repository's possibly different checked-out branch.
    let cwd = if base.is_some() {
        repo
    } else {
        Path::new(&workspace.cwd)
    };
    git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{}^{{commit}}", base.unwrap_or("HEAD")),
        ],
    )
    .map_err(|error| format!("Cannot resolve base {}: {error}", base.unwrap_or("HEAD")))
}

#[derive(Default)]
struct LaunchState {
    pending: Mutex<HashMap<(PathBuf, String), String>>,
}

struct Reservation {
    state: Arc<LaunchState>,
    key: (PathBuf, String),
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.state
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

impl LaunchState {
    fn reserve(
        self: &Arc<Self>,
        parent: &str,
        repo: &Path,
        input: &LaunchArgs,
        existing: impl FnOnce() -> Result<HashSet<String>, String>,
    ) -> Result<Reservation, String> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending
            .values()
            .filter(|thread| thread.as_str() == parent)
            .count()
            >= MAX_IN_FLIGHT
        {
            return Err("This chat already has 6 workspace launches in flight. Wait for them to start before launching more.".into());
        }
        // Read refs while holding the reservation lock: a completed launch
        // must remain visible either here or in the pending set.
        let mut existing = existing()?;
        existing.extend(
            pending
                .keys()
                .filter(|(root, _)| root == repo)
                .map(|(_, branch)| branch.clone()),
        );
        let branch = match &input.branch {
            Some(branch) if existing.contains(branch) => {
                return Err(format!(
                    "Branch '{branch}' already exists or is starting. Choose a new branch."
                ))
            }
            Some(branch) => branch.clone(),
            None => {
                let title = input
                    .title
                    .to_ascii_lowercase()
                    .replace(|c: char| !c.is_ascii_alphanumeric(), "-");
                let slug = crate::branch_name::sanitize_branch_name(&title);
                crate::branch_name::deconflict_against(
                    if slug.is_empty() { "workspace" } else { &slug },
                    &existing,
                )
            }
        };
        let key = (repo.to_path_buf(), branch);
        pending.insert(key.clone(), parent.into());
        Ok(Reservation {
            state: self.clone(),
            key,
        })
    }
}

pub struct WorkspaceLaunchHost<R: Runtime> {
    app: AppHandle<R>,
    state: Arc<LaunchState>,
    is_installed: fn(ProviderKind) -> bool,
}

impl<R: Runtime> WorkspaceLaunchHost<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self {
            app,
            state: Arc::default(),
            is_installed: |kind| which::which(super::provider_updates::binary_name(kind)).is_ok(),
        }
    }

    fn launch(&self, caller: &HostCaller, arguments: &Value) -> Result<Value, String> {
        let input = LaunchArgs::parse(arguments)?;
        let (workspace, repo) = caller_workspace(&self.app, caller)?;
        let provider = input.provider.unwrap_or(caller.provider);
        if !offered_agents(caller, false, self.is_installed).contains(&provider) {
            return Err("Choose an installed provider offered by workspace_launch.".into());
        }
        if let Some(branch) = &input.branch {
            if git(&repo, &["check-ref-format", "--branch", branch])? != *branch {
                return Err("Pass a literal new branch name, not a checkout shorthand.".into());
            }
        }
        let base = resolve_base(&workspace, &repo, input.base.as_deref())?;
        let reservation = self.state.reserve(&caller.thread_id, &repo, &input, || {
            let mut existing: HashSet<_> = crate::git::git_list_branches(&repo, false)?
                .into_iter()
                .collect();
            existing.extend(crate::git::git_list_branches(&repo, true)?);
            Ok(existing)
        })?;
        let branch = reservation.key.1.clone();
        // Even an orphaned checkout must not be reused for independent work.
        if crate::git::conventional_worktree_path(&repo, &branch).is_some_and(|path| path.exists())
        {
            return Err(format!(
                "A worktree path already exists for '{branch}'. Choose a new branch."
            ));
        }
        let result = json!({
            "status": "starting", "title": input.title, "branch": branch,
            "base": input.base.as_deref().unwrap_or(&base), "provider": provider,
            "note": "The workspace appears in the sidebar shortly. Give the user a one-line status and end your turn.",
        });
        let app = self.app.clone();
        let caller = caller.clone();
        tauri::async_runtime::spawn(async move {
            let outcome = start(&app, &caller, input, provider, repo, branch, base).await;
            if let Err(error) = outcome {
                eprintln!("[codemux::workspace_launch] {error}");
                let state = app.state::<AppStateStore>();
                let pane = state.agent_chat_pane_id_for_thread(&caller.thread_id);
                if state
                    .add_notification(None, pane, error.clone(), NotificationLevel::Attention)
                    .is_err()
                {
                    // The caller's pane may have closed while startup ran.
                    let _ = state.add_notification(None, None, error, NotificationLevel::Attention);
                }
                crate::state::emit_app_state(&app);
            }
            drop(reservation);
        });
        Ok(result)
    }
}

async fn start<R: Runtime>(
    app: &AppHandle<R>,
    caller: &HostCaller,
    input: LaunchArgs,
    provider: ProviderKind,
    repo: PathBuf,
    branch: String,
    base: String,
) -> Result<(), String> {
    let title = input.title.clone();
    let outcome: Result<(), String> = async {
        let control = ControlCaller {
            created_by: crate::agent_provider::message::CreatedBy::Agent,
            principal: format!("thread:{}", caller.thread_id),
            access: ControlAccess::FullAccess,
            grant_id: None,
        };
        let prepared = service::PreparedWorkspaceChat::prepare(
            app,
            &control,
            provider,
            input.title.clone(),
            input.message,
            input.model,
            input.effort,
        )
        .await
        .map_err(|error| error.to_string())?;
        // Provider preparation may yield while another launch creates the checkout.
        if crate::git::conventional_worktree_path(&repo, &branch).is_some_and(|path| path.exists())
        {
            return Err(format!(
                "A worktree path already exists for '{branch}'. Choose a new branch."
            ));
        }
        let created = super::workspace::create_worktree_workspace_impl_with_selection(
            app.clone(),
            &app.state::<AppStateStore>(),
            &app.state::<crate::database::DatabaseStore>(),
            &app.state::<crate::terminal::PtyState>(),
            &app.state::<crate::presets::PresetStoreState>(),
            repo.to_string_lossy().into_owned(),
            branch,
            true,
            Some(base),
            "empty".into(),
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .await?;
        if created.adopted {
            return Err("The branch already has a live workspace; no prompt was sent.".into());
        }
        let state = app.state::<AppStateStore>();
        state.rename_workspace(&created.workspace_id, input.title);
        crate::state::emit_app_state(app);
        // Keep a failed launch's checkout for inspection: setup scripts may
        // already have written files, so automatic deletion is not safe.
        prepared
            .materialize(app, &control, created.workspace_id.clone())
            .await
            .map_err(|error| format!("{error}. The created workspace was kept for inspection."))?;
        Ok(())
    }
    .await;
    outcome.map_err(|error| format!("Could not start workspace '{title}': {error}"))
}

impl<R: Runtime> HostTools for WorkspaceLaunchHost<R> {
    fn tools_for(&self, caller: &HostCaller) -> Vec<McpTool> {
        if caller_workspace(&self.app, caller).is_err() {
            return vec![];
        }
        let agents = offered_agents(caller, false, self.is_installed);
        if agents.is_empty() {
            return vec![];
        }
        vec![McpTool {
            name: TOOL_NAME.into(),
            prefixed_name: prefix_tool_name("codemux", TOOL_NAME),
            server_id: "codemux-host".into(),
            description: Some(TOOL_DESCRIPTION.into()),
            input_schema: json!({
                "type": "object", "additionalProperties": false,
                "properties": {
                    "title": {"type":"string", "description":"Short workspace and chat title (max 100 bytes)."},
                    "message": {"type":"string", "description":"Self-contained first prompt (max 65536 bytes)."},
                    "branch": {"type":"string", "description":"New branch name. Omit for a unique kebab-case name from title."},
                    "base": {"type":"string", "description":"Base ref, e.g. main. Omit for this workspace's current HEAD."},
                    "provider": {"type":"string", "enum":agents.iter().map(|kind| provider_id(*kind)).collect::<Vec<_>>(), "description":"Installed agent. Omit to use this chat's provider."},
                    "model": {"type":"string", "description":"Exact advertised model id, only when the user requested it."},
                    "effort": {"type":"string", "description":"Advertised effort for the selected model, only when the user requested it."}
                },
                "required": ["title", "message"]
            }),
        }]
    }

    fn call(&self, caller: &HostCaller, name: &str, arguments: &Value) -> Option<Value> {
        if name != prefix_tool_name("codemux", TOOL_NAME) {
            return None;
        }
        let (text, is_error) = match self.launch(caller, arguments) {
            Ok(result) => (result.to_string(), false),
            Err(error) => (error, true),
        };
        Some(json!({"content":[{"type":"text", "text":text}], "isError":is_error}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> LaunchArgs {
        LaunchArgs::parse(&json!({"title":"Fix sidebar tests", "message":"Fix the flaky sidebar test and run the affected test file."})).unwrap()
    }

    #[test]
    fn workspace_launch_validates_arguments() {
        assert!(LaunchArgs::parse(
            &json!({"title":"Fix tests", "message":"Work", "provider":"claude"})
        )
        .is_ok());
        for bad in [
            json!({"title":"", "message":"Work"}),
            json!({"title":" Trailing ", "message":"Work"}),
            json!({"title":"Two\nlines", "message":"Work"}),
            json!({"title":"x".repeat(101), "message":"Work"}),
            json!({"title":"Fix", "message":"  "}),
            json!({"title":"Fix", "message":"x".repeat(65537)}),
            json!({"title":"Fix", "message":"Work", "base":"--all"}),
            json!({"title":"Fix", "message":"Work", "workspace_id":"spoof"}),
            json!({"title":"Fix", "message":"Work", "provider":"unknown"}),
        ] {
            assert!(LaunchArgs::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn workspace_launch_gates_live_callers_and_workspace_types() {
        let state = AppStateStore::default();
        let root = tempfile::tempdir().unwrap();
        let id = state.create_empty_workspace_at_path(root.path().into());
        let mut workspace = state.find_workspace(&id.0).unwrap();
        workspace.is_git = true;
        let mut caller = HostCaller {
            thread_id: "parent".into(),
            provider: ProviderKind::Claude,
            workspace_id: Some(id.0),
            permission_mode: Some("bypassPermissions".into()),
        };
        assert!(eligible(&caller, false, &workspace));
        assert!(!eligible(&caller, true, &workspace));
        for mode in ["plan", "default", "acceptEdits"] {
            caller.permission_mode = Some(mode.into());
            assert!(!eligible(&caller, false, &workspace));
        }
        caller.provider = ProviderKind::Codex;
        caller.permission_mode = Some("workspace-write".into());
        assert!(!eligible(&caller, false, &workspace));
        caller.permission_mode = Some("danger-full-access".into());
        assert!(eligible(&caller, false, &workspace));
        workspace.host_id = Some(1);
        assert!(!eligible(&caller, false, &workspace));
        workspace.host_id = None;
        workspace.remote_cwd = Some("/remote".into());
        assert!(!eligible(&caller, false, &workspace));
        workspace.remote_cwd = None;
        workspace.imported_snapshot_only = Some(true);
        assert!(!eligible(&caller, false, &workspace));
        workspace.imported_snapshot_only = Some(false);
        caller.provider = ProviderKind::Cursor;
        assert!(!eligible(&caller, false, &workspace));
        caller.provider = ProviderKind::Codex;
        workspace.is_git = false;
        assert!(!eligible(&caller, false, &workspace));
    }

    #[test]
    fn workspace_launch_cap_counts_starting_calls_and_releases_slots() {
        let state = Arc::new(LaunchState::default());
        let repo = Path::new("/repo");
        let mut slots = vec![];
        for i in 0..6 {
            let slot = state
                .reserve("parent", repo, &args(), || {
                    Ok(HashSet::from(["fix-sidebar-tests".into()]))
                })
                .unwrap();
            assert_eq!(slot.key.1, format!("fix-sidebar-tests-{}", i + 2));
            slots.push(slot);
        }
        assert!(state
            .reserve("parent", repo, &args(), || Ok(HashSet::new()))
            .err()
            .unwrap()
            .contains("6"));
        assert!(state
            .reserve("other", repo, &args(), || Ok(HashSet::new()))
            .is_ok());
        slots.pop();
        assert!(state
            .reserve("parent", repo, &args(), || Ok(HashSet::new()))
            .is_ok());
        let mut explicit = args();
        explicit.branch = Some("existing".into());
        assert!(state
            .reserve("other", repo, &explicit, || Ok(HashSet::from([
                "existing".into()
            ])))
            .is_err());
    }

    #[test]
    fn workspace_launch_uses_callers_head_and_parent_repo_with_setting_off() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-b", "main"]).unwrap();
        git(
            repo.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "base",
            ],
        )
        .unwrap();
        let parent_head = git(repo.path(), &["rev-parse", "HEAD"]).unwrap();
        let checkout_parent = tempfile::tempdir().unwrap();
        let checkout = checkout_parent.path().join("checkout");
        git(
            repo.path(),
            &[
                "worktree",
                "add",
                "-b",
                "caller",
                checkout.to_str().unwrap(),
            ],
        )
        .unwrap();
        git(
            &checkout,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "caller change",
            ],
        )
        .unwrap();
        let caller_head = git(&checkout, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(parent_head, caller_head);
        let app = tauri::test::mock_app();
        let state = AppStateStore::default();
        let id = state.create_empty_workspace_at_path(checkout);
        state.set_workspace_project_root(&id.0, repo.path().to_string_lossy().into_owned());
        app.manage(state);
        app.manage(DelegationState::default());
        let db = crate::database::DatabaseStore::new_in_memory();
        db.set_setting(super::super::delegation::DELEGATION_SETTING_KEY, "false")
            .unwrap();
        app.manage(db);
        let caller = HostCaller {
            thread_id: "parent".into(),
            provider: ProviderKind::Claude,
            workspace_id: Some(id.0),
            permission_mode: Some("bypassPermissions".into()),
        };
        let host = WorkspaceLaunchHost {
            app: app.handle().clone(),
            state: Arc::default(),
            is_installed: |_| true,
        };
        assert_eq!(host.tools_for(&caller).len(), 1);
        let (workspace, resolved_repo) = caller_workspace(app.handle(), &caller).unwrap();
        assert_eq!(resolved_repo, repo.path());
        assert_eq!(
            resolve_base(&workspace, &resolved_repo, None).unwrap(),
            caller_head
        );
        assert_eq!(
            resolve_base(&workspace, &resolved_repo, Some("main")).unwrap(),
            parent_head
        );
        assert!(resolve_base(&workspace, &resolved_repo, Some("missing-ref")).is_err());
        let invalid = args();
        assert!(host
            .launch(
                &caller,
                &json!({"title":invalid.title,"message":invalid.message,"provider":"hermes"})
            )
            .is_err());
        let mut supervised = caller.clone();
        supervised.permission_mode = Some("default".into());
        assert!(host.tools_for(&supervised).is_empty());
        assert!(host
            .call(
                &supervised,
                &prefix_tool_name("codemux", TOOL_NAME),
                &json!({"title":"Fix tests","message":"Work"})
            )
            .unwrap()["isError"]
            .as_bool()
            .unwrap());
    }

    #[tokio::test]
    async fn workspace_launch_async_failure_notifies_and_releases_its_slot() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-b", "main"]).unwrap();
        git(
            repo.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "base",
            ],
        )
        .unwrap();
        let app = tauri::test::mock_app();
        let state = AppStateStore::default();
        let id = state.create_empty_workspace_at_path(repo.path().into());
        app.manage(state);
        app.manage(DelegationState::default());
        app.manage(super::super::agent_chat::ProviderRegistry::new());
        let observability = crate::observability::ObservabilityStore::default();
        let mut flags = observability.feature_flags();
        flags.enable_agent_chat = true;
        observability.set_feature_flags(flags);
        app.manage(observability);
        let host = WorkspaceLaunchHost {
            app: app.handle().clone(),
            state: Arc::default(),
            is_installed: |_| true,
        };
        let caller = HostCaller {
            thread_id: "parent".into(),
            provider: ProviderKind::Claude,
            workspace_id: Some(id.0.clone()),
            permission_mode: Some("bypassPermissions".into()),
        };
        let workspace_count = app.state::<AppStateStore>().snapshot().workspaces.len();
        let receipt = host
            .launch(
                &caller,
                &json!({"title":"Fix sidebar tests", "message":"Self-contained brief"}),
            )
            .unwrap();
        assert_eq!(receipt["status"], "starting");
        assert_eq!(receipt["provider"], "claude");
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let finished = host.state.pending.lock().unwrap().is_empty();
                if finished
                    && !app
                        .state::<AppStateStore>()
                        .snapshot()
                        .notifications
                        .is_empty()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let snapshot = app.state::<AppStateStore>().snapshot();
        assert_eq!(
            snapshot.workspaces.len(),
            workspace_count,
            "provider validation precedes checkout creation"
        );
        assert_eq!(snapshot.active_workspace_id.0, id.0);
        assert!(snapshot.notifications[0]
            .message
            .contains("Could not start workspace 'Fix sidebar tests'"));
        assert!(snapshot.notifications[0]
            .message
            .contains("provider_not_configured"));
    }
}
