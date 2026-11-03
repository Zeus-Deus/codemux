//! Tests for the workspace and worktree tools, through the registry as every
//! outside door calls them.
//!
//! A test that creates or closes a workspace or a worktree runs in an
//! isolated child process (`test_isolation::isolated`): worktrees, the layout
//! file and terminals then live in a private root, never in the developer's
//! own Codemux state.
use super::tests::{fixture, mock_agent_provider, native_caller_fixture};
use super::{execute, registry, ClientName, ControlAccess, ControlCaller, ControlError, ErrorCode, NativeControlState};
use crate::database::DatabaseStore;
use crate::state::{AppStateStore, WorkspaceSnapshot};
use crate::test_isolation::isolated;
use mock_agent_provider::MockAgentProvider;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

type App = tauri::App<tauri::test::MockRuntime>;

const MODULE: &str = "agent_control::workspace_tests::";

fn git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(repo)
        .args(["-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "-c", "user.name=Codemux test", "-c", "user.email=test@example.invalid"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A connector client: off this computer, governed by `access`. Without a
/// grant id it needs no live connector, which the network door tests cover.
fn connector(access: ControlAccess) -> ControlCaller {
    ControlCaller { grant_id: None, ..ControlCaller::outside("workspace-tests".into(), "unused-grant".into(), access) }
}

/// A named local client, as `codemux mcp --client "Hermes Agent"` is.
fn named(access: ControlAccess) -> ControlCaller {
    ControlCaller::local(Some(&ClientName::parse("Hermes Agent").unwrap()), access)
}

async fn call(app: &App, caller: &ControlCaller, tool: &str, args: Value) -> Result<Value, ControlError> {
    execute(app.handle(), caller, tool, args).await
}

#[track_caller]
fn refused(result: Result<Value, ControlError>, code: ErrorCode) -> ControlError {
    let error = result.expect_err("the call must be refused");
    assert_eq!(error.code, code, "{error}");
    error
}

/// Only in an isolated child, where this test is the only one running: a new
/// workspace's terminal starts in this process, never as a terminal daemon
/// launched from the test binary.
fn without_terminal_daemon() {
    assert!(crate::test_isolation::isolated_root().is_some(), "call this only in an isolated child");
    std::env::set_var("CODEMUX_DISABLE_PTY_DAEMON", "1");
}

/// The shared fixture whose workspace folder is a git repository on `main`.
async fn git_fixture() -> (App, Arc<MockAgentProvider>, tempfile::TempDir, String, PathBuf) {
    let (app, provider, root, workspace) = fixture().await;
    app.manage(crate::terminal::PtyState::default());
    let repo = root.path().to_path_buf();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["commit", "--allow-empty", "-m", "first"]);
    app.state::<AppStateStore>().set_workspace_project_root(&workspace, repo.display().to_string());
    (app, provider, root, workspace, repo)
}

fn count(app: &App) -> usize {
    app.state::<AppStateStore>().snapshot().workspaces.len()
}

fn active(app: &App) -> String {
    app.state::<AppStateStore>().snapshot().active_workspace_id.0
}

fn receipt(app: &App, caller: &ControlCaller, operation_id: &Value) -> crate::database::agent_control::OperationReceipt {
    let epoch = app.state::<NativeControlState>().epoch.clone();
    app.state::<DatabaseStore>().control_operation(&caller.principal, operation_id.as_str().unwrap(), &epoch).unwrap().unwrap()
}

/// No receipt was admitted under `key`: the refusal came before admission.
fn no_receipt(app: &App, caller: &ControlCaller, key: &str, tool: &str) {
    let epoch = app.state::<NativeControlState>().epoch.clone();
    let found = app.state::<DatabaseStore>().control_operation_by_key(&caller.principal, key, tool, "", &epoch).unwrap();
    assert!(found.is_none(), "{key} was admitted: {found:?}");
}

// ── workspace_create ──────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_create_reuses_open_folder_and_creates_new_in_background() {
    if isolated(&format!("{MODULE}workspace_create_reuses_open_folder_and_creates_new_in_background")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, root, workspace) = fixture().await;
    app.manage(crate::terminal::PtyState::default());
    let owner = ControlCaller::trusted();
    let focused = active(&app);
    let before = count(&app);

    let opened = call(&app, &owner, "workspace_create", json!({"path": root.path()})).await.unwrap();
    assert_eq!(opened, json!({"workspace_id": workspace, "cwd": root.path().display().to_string(), "created": false}));
    assert_eq!(count(&app), before, "an open folder is returned, not opened twice");

    let fresh = tempfile::tempdir().unwrap();
    let created = call(&app, &owner, "workspace_create", json!({"path": fresh.path()})).await.unwrap();
    assert_eq!(created["created"], true, "{created}");
    assert_eq!(created["cwd"], fresh.path().display().to_string());
    assert!(created.get("operation_id").is_none(), "an unkeyed call has no receipt");
    let id = created["workspace_id"].as_str().unwrap();
    assert_ne!(id, workspace);
    assert_eq!(count(&app), before + 1);
    assert_eq!(active(&app), focused, "created in the background");

    let selected_folder = tempfile::tempdir().unwrap();
    let selected = call(&app, &owner, "workspace_create", json!({"path": selected_folder.path(), "select": true})).await.unwrap();
    assert_eq!(active(&app), selected["workspace_id"].as_str().unwrap(), "select focuses it");
    // select only ever focuses: reopening the first folder leaves focus alone.
    call(&app, &owner, "workspace_create", json!({"path": root.path(), "select": null})).await.unwrap();
    assert_eq!(active(&app), selected["workspace_id"].as_str().unwrap());

    let missing = root.path().join("missing");
    refused(call(&app, &owner, "workspace_create", json!({"path": missing})).await, ErrorCode::InvalidArguments);
    refused(call(&app, &owner, "workspace_create", json!({"path": "relative/folder"})).await, ErrorCode::InvalidArguments);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_create_receipt_names_reserved_id_and_replays() {
    if isolated(&format!("{MODULE}workspace_create_receipt_names_reserved_id_and_replays")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, _root, _workspace) = fixture().await;
    app.manage(crate::terminal::PtyState::default());
    let owner = named(ControlAccess::FullAccess);
    let folder = tempfile::tempdir().unwrap();
    let args = json!({"path": folder.path(), "client_request_id": "create-1"});

    let created = call(&app, &owner, "workspace_create", args.clone()).await.unwrap();
    assert_eq!((created["created"].clone(), created["state"].clone()), (json!(true), json!("succeeded")), "{created}");
    let id = created["workspace_id"].clone();
    assert_eq!(receipt(&app, &owner, &created["operation_id"]).workspace_id.as_deref(), id.as_str());
    let after = count(&app);

    let retry = call(&app, &owner, "workspace_create", args.clone()).await.unwrap();
    assert_eq!((retry["operation_id"].clone(), retry["workspace_id"].clone()), (created["operation_id"].clone(), id.clone()));
    assert_eq!(retry["replayed"], true);
    assert_eq!(count(&app), after, "a replay opens nothing");
    let other = tempfile::tempdir().unwrap();
    refused(call(&app, &owner, "workspace_create", json!({"path": other.path(), "client_request_id": "create-1"})).await, ErrorCode::RequestKeyConflict);

    // A duplicate while the first call is still working sees its receipt,
    // which already names the workspace it will create.
    let barrier = Arc::new(tokio::sync::Semaphore::new(0));
    *app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap() = Some(barrier.clone());
    let held_args = json!({"path": other.path(), "client_request_id": "create-2"});
    let held = {
        let (handle, caller, args) = (app.handle().clone(), owner.clone(), held_args.clone());
        tokio::spawn(async move { execute(&handle, &caller, "workspace_create", args).await })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the first call reaches its work");
    let duplicate = call(&app, &owner, "workspace_create", held_args).await.unwrap();
    assert_eq!(duplicate["state"], "running", "{duplicate}");
    let reserved = duplicate["workspace_id"].as_str().unwrap().to_string();
    assert!(app.state::<AppStateStore>().find_workspace(&reserved).is_none(), "named before it exists");
    barrier.add_permits(1);
    let finished = held.await.unwrap().unwrap();
    assert_eq!(finished["workspace_id"], reserved);
    assert_eq!(finished["operation_id"], duplicate["operation_id"]);
    assert!(app.state::<AppStateStore>().find_workspace(&reserved).is_some());
}

#[tokio::test]
async fn workspace_create_reuse_overwrites_reserved_receipt_target() {
    let (app, _provider, root, workspace) = fixture().await;
    let owner = ControlCaller::trusted();
    let reused = call(&app, &owner, "workspace_create", json!({"path": root.path(), "client_request_id": "reuse-1"})).await.unwrap();
    assert_eq!((reused["workspace_id"].clone(), reused["created"].clone()), (json!(workspace), json!(false)));
    assert_eq!(receipt(&app, &owner, &reused["operation_id"]).workspace_id.as_deref(), Some(workspace.as_str()),
        "the receipt names the open workspace, not the id reserved at admission");
    let status = call(&app, &owner, "operation_status", json!({"operation_id": reused["operation_id"]})).await.unwrap();
    assert_eq!((status["state"].clone(), status["workspace_id"].clone()), (json!("succeeded"), json!(workspace)));
}

// ── access ────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_lifecycle_requires_full_access() {
    let (app, _provider, root, workspace) = fixture().await;
    let lifecycle = [
        ("workspace_create", json!({"path": root.path()})),
        ("worktree_create", json!({"repo_path": root.path(), "branch": "never-created"})),
        ("workspace_close", json!({"workspace_id": workspace})),
    ];
    for caller in [named(ControlAccess::Supervised), connector(ControlAccess::Supervised), ControlCaller::local(None, ControlAccess::ReadOnly)] {
        for (tool, args) in &lifecycle {
            refused(call(&app, &caller, tool, args.clone()).await, ErrorCode::AccessDenied);
        }
    }
    assert!(app.state::<AppStateStore>().find_workspace(&workspace).is_some());
    let reader = ControlCaller::local(None, ControlAccess::ReadOnly);
    assert!(call(&app, &reader, "workspace_list", json!({})).await.unwrap()["workspaces"].is_array());
    assert_eq!(call(&app, &reader, "workspace_info", json!({"workspace_id": workspace})).await.unwrap()["workspace_id"], workspace);

    // A chat without a full-access ceiling cannot change the machine either.
    let (app, _provider, root, workspace, _thread, _authority, actor) = native_caller_fixture("default").await;
    let chat = super::native::qualify(app.handle(), &actor).await.unwrap();
    for (tool, args) in [
        ("workspace_create", json!({"path": root.path()})),
        ("workspace_close", json!({"workspace_id": workspace})),
    ] {
        refused(call(&app, &chat, tool, args).await, ErrorCode::AccessDenied);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connector_workspace_create_is_limited_to_known_roots() {
    if isolated(&format!("{MODULE}connector_workspace_create_is_limited_to_known_roots")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let listed: Vec<&str> = registry::tools_for(&connector(ControlAccess::FullAccess)).iter().map(|tool| tool.name).collect();
    for name in ["workspace_list", "workspace_info", "workspace_create", "worktree_create", "workspace_close"] {
        assert!(listed.contains(&name), "{name}: {listed:?}");
    }
    refused(call(&app, &connector(ControlAccess::ReadOnly), "workspace_create", json!({"path": repo})).await, ErrorCode::AccessDenied);

    let full = connector(ControlAccess::FullAccess);
    let unknown = tempfile::tempdir().unwrap();
    refused(call(&app, &full, "workspace_create", json!({"path": unknown.path()})).await, ErrorCode::UnknownProjectRoot);
    refused(call(&app, &full, "workspace_create", json!({})).await, ErrorCode::InvalidArguments);
    refused(call(&app, &full, "workspace_create", json!({"path": "relative"})).await, ErrorCode::InvalidArguments);
    let before = count(&app);
    std::fs::create_dir(repo.join("sub")).unwrap();
    let created = call(&app, &full, "workspace_create", json!({"path": repo.join("sub")})).await.unwrap();
    assert_eq!(created["created"], true, "{created}");
    assert_eq!(created["cwd"], repo.join("sub").canonicalize().unwrap().display().to_string());
    assert_eq!(count(&app), before + 1);
}

// ── workspace_info and workspace_list ─────────────────────────────────────

#[tokio::test]
async fn workspace_info_defaults_to_focus_only_for_local_callers() {
    let (app, _provider, _root, workspace) = fixture().await;
    let owner = ControlCaller::trusted();
    let focused = call(&app, &owner, "workspace_info", json!({})).await.unwrap();
    assert_eq!(focused["workspace_id"], active(&app));
    assert!(focused["surfaces"].is_array(), "the full snapshot: {focused}");
    assert_eq!(focused["native_control_available"], true);
    let named_info = call(&app, &named(ControlAccess::ReadOnly), "workspace_info", json!({"workspace_id": null})).await.unwrap();
    assert_eq!(named_info["workspace_id"], active(&app), "named local clients are on this computer");
    refused(call(&app, &connector(ControlAccess::FullAccess), "workspace_info", json!({})).await, ErrorCode::InvalidArguments);
    assert_eq!(call(&app, &connector(ControlAccess::ReadOnly), "workspace_info", json!({"workspace_id": workspace})).await.unwrap()["workspace_id"], workspace);
    refused(call(&app, &owner, "workspace_info", json!({"workspace_id": "missing"})).await, ErrorCode::WorkspaceNotFound);

    let state = app.state::<AppStateStore>();
    for workspace in state.snapshot().workspaces {
        state.close_workspace(&workspace.workspace_id.0).unwrap();
    }
    refused(call(&app, &owner, "workspace_info", json!({})).await, ErrorCode::NoActiveWorkspace);
}

#[tokio::test]
async fn workspace_list_summary_and_full() {
    let (app, _provider, _root, _workspace) = fixture().await;
    let state = app.state::<AppStateStore>();
    // Workspaces thread tools cannot target, next to a local one.
    let mut snapshot = state.snapshot();
    let locals: Vec<String> = snapshot.workspaces.iter().map(|w| w.workspace_id.0.clone()).collect();
    let local = snapshot.workspaces[0].clone();
    let variants: [(&str, fn(&mut WorkspaceSnapshot)); 3] = [
        ("imported-copy", |w| w.imported_snapshot_only = Some(true)),
        ("on-a-host", |w| w.host_id = Some(7)),
        ("remote-folder", |w| w.remote_cwd = Some("/srv/remote".into())),
    ];
    for (id, edit) in variants {
        let mut workspace = local.clone();
        workspace.workspace_id = crate::state::WorkspaceId(id.into());
        edit(&mut workspace);
        snapshot.workspaces.push(workspace);
    }
    state.replace_snapshot(snapshot);
    let owner = ControlCaller::trusted();

    let summary = call(&app, &owner, "workspace_list", json!({})).await.unwrap();
    let rows = summary["workspaces"].as_array().unwrap();
    assert_eq!(rows.len(), locals.len() + 3);
    for row in rows {
        let mut keys: Vec<&str> = row.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["branch", "cwd", "native_control_available", "parent_workspace_id", "project_root", "title", "workspace_id", "worktree_path"]);
        // A row is controllable exactly when thread tools accept it.
        let id = row["workspace_id"].as_str().unwrap();
        let accepted = super::service::workspace(app.handle(), id, true).is_ok();
        assert_eq!(row["native_control_available"], accepted, "{id}");
        let info = call(&app, &owner, "workspace_info", json!({"workspace_id": id})).await.unwrap();
        assert_eq!(info["native_control_available"], accepted, "{id}");
    }
    let controllable: Vec<String> = rows.iter().filter(|row| row["native_control_available"] == true).filter_map(|row| row["workspace_id"].as_str().map(str::to_owned)).collect();
    assert_eq!(controllable, locals, "only the local workspaces");

    let full = call(&app, &owner, "workspace_list", json!({"detail": "full"})).await.unwrap();
    assert_eq!(full["workspaces"], serde_json::to_value(state.snapshot().workspaces).unwrap());
    assert_eq!(call(&app, &owner, "workspace_list", json!({"detail": null})).await.unwrap(), summary, "null reads as the default");
    refused(call(&app, &owner, "workspace_list", json!({"detail": "everything"})).await, ErrorCode::InvalidArguments);
}

// ── workspace_close ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_close_receipt_replays_after_close() {
    if isolated(&format!("{MODULE}workspace_close_receipt_replays_after_close")) {
        return;
    }
    let (app, _provider, root, _workspace) = fixture().await;
    app.manage(crate::terminal::PtyState::default());
    let state = app.state::<AppStateStore>();
    let (first, second) = (root.path().join("first"), root.path().join("second"));
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let first = state.create_empty_workspace_at_path(first).0;
    let second = state.create_empty_workspace_at_path(second).0;
    let owner = named(ControlAccess::FullAccess);
    let args = json!({"workspace_id": first, "client_request_id": "close-1"});

    let closed = call(&app, &owner, "workspace_close", args.clone()).await.unwrap();
    assert_eq!((closed["workspace_id"].clone(), closed["closed"].clone(), closed["worktree_removed"].clone()), (json!(first), json!(true), json!(false)));
    assert_eq!(closed["state"], "succeeded");
    assert!(state.find_workspace(&first).is_none());
    let retry = call(&app, &owner, "workspace_close", args).await.unwrap();
    assert_eq!(retry["operation_id"], closed["operation_id"]);
    assert_eq!((retry["closed"].clone(), retry["replayed"].clone()), (json!(true), json!(true)), "a retry replays, not workspace_not_found");
    refused(call(&app, &owner, "workspace_close", json!({"workspace_id": second, "client_request_id": "close-1"})).await, ErrorCode::RequestKeyConflict);
    assert!(state.find_workspace(&second).is_some());
    refused(call(&app, &owner, "workspace_close", json!({"workspace_id": first})).await, ErrorCode::WorkspaceNotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connector_workspace_close_refuses_force_delete() {
    if isolated(&format!("{MODULE}connector_workspace_close_refuses_force_delete")) {
        return;
    }
    let (app, _provider, _root, workspace) = fixture().await;
    app.manage(crate::terminal::PtyState::default());
    let full = connector(ControlAccess::FullAccess);
    let force = json!({"workspace_id": workspace, "delete_worktree": true, "force_delete": true, "client_request_id": "force"});
    let error = refused(call(&app, &full, "workspace_close", force).await, ErrorCode::PermissionDenied);
    assert!(error.message.contains("only available on this computer"), "{error}");
    no_receipt(&app, &full, "force", "workspace_close");
    assert!(app.state::<AppStateStore>().find_workspace(&workspace).is_some());
    // Removal itself is allowed, under the desktop's own guards.
    let error = refused(call(&app, &full, "workspace_close", json!({"workspace_id": workspace, "delete_worktree": true, "client_request_id": "root-delete"})).await, ErrorCode::WorkspaceCloseRefused);
    assert!(error.message.contains("cannot be deleted"), "{error}");
    assert!(app.state::<AppStateStore>().find_workspace(&workspace).is_some(), "a refused close closes nothing");
}

// ── worktree_create ───────────────────────────────────────────────────────

/// A bare origin seeded from `seed`, cloned into the fixture repository,
/// then advanced by one commit the clone has not fetched. Returns the local
/// and origin tips.
fn advance_origin(parent: &Path, repo: &Path) -> (String, String) {
    let origin = parent.join("origin.git");
    git(parent, &["clone", "--bare", repo.to_str().unwrap(), origin.to_str().unwrap()]);
    git(repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(repo, &["fetch", "origin"]);
    let local = git(repo, &["rev-parse", "HEAD"]);
    let seed = parent.join("seed");
    git(parent, &["clone", origin.to_str().unwrap(), seed.to_str().unwrap()]);
    git(&seed, &["commit", "--allow-empty", "-m", "origin only"]);
    git(&seed, &["push", "origin", "main"]);
    let tip = git(&seed, &["rev-parse", "HEAD"]);
    assert_ne!(local, tip);
    (local, tip)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_worktree_create_receipt_reserves_id_adopts_and_honours_origin_policy() {
    if isolated(&format!("{MODULE}registry_worktree_create_receipt_reserves_id_adopts_and_honours_origin_policy")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let parent = tempfile::tempdir().unwrap();
    let (local_tip, origin_tip) = advance_origin(parent.path(), &repo);
    let owner = named(ControlAccess::FullAccess);
    let focused = active(&app);
    let args = json!({"repo_path": repo, "branch": "feat-a", "layout": "empty", "client_request_id": "wt-a"});

    let created = call(&app, &owner, "worktree_create", args.clone()).await.unwrap();
    assert_eq!((created["adopted"].clone(), created["state"].clone()), (json!(false), json!("succeeded")), "{created}");
    let id = created["workspace_id"].as_str().unwrap().to_string();
    let path = crate::git::conventional_worktree_path(&repo, "feat-a").unwrap();
    assert_eq!(created["cwd"], path.display().to_string());
    assert_eq!(created["base_branch"], "main");
    assert_eq!(receipt(&app, &owner, &created["operation_id"]).workspace_id.as_deref(), Some(id.as_str()));
    let workspace = app.state::<AppStateStore>().find_workspace(&id).unwrap();
    assert_eq!(workspace.worktree_path.as_deref(), created["cwd"].as_str());
    assert_eq!(active(&app), focused, "created in the background");
    let after = count(&app);
    let replay = call(&app, &owner, "worktree_create", args).await.unwrap();
    assert_eq!((replay["operation_id"].clone(), replay["replayed"].clone()), (created["operation_id"].clone(), json!(true)));
    assert_eq!(count(&app), after);

    // The open checkout of an existing branch is adopted, not duplicated.
    let adopted = call(&app, &owner, "worktree_create", json!({"repo_path": repo, "branch": "feat-a", "new_branch": false, "layout": "empty"})).await.unwrap();
    assert_eq!((adopted["adopted"].clone(), adopted["workspace_id"].clone()), (json!(true), json!(id)));
    assert_eq!(count(&app), after);

    for (branch, from_origin, tip) in [("from-origin", true, &origin_tip), ("from-local", false, &local_tip)] {
        let created = call(&app, &owner, "worktree_create", json!({
            "repo_path": repo, "branch": branch, "start_from_origin": from_origin, "layout": "empty",
        })).await.unwrap();
        assert_eq!(git(Path::new(created["cwd"].as_str().unwrap()), &["rev-parse", "HEAD"]), *tip, "{branch}");
        assert_eq!(created["base_branch"], "main");
    }

    // A connector client never makes git switch the person's own checkout.
    let full = connector(ControlAccess::FullAccess);
    let error = refused(call(&app, &full, "worktree_create", json!({"repo_path": repo, "branch": "main", "new_branch": false, "layout": "empty"})).await, ErrorCode::WorktreeConflict);
    assert!(error.message.contains("checked out in the project root"), "{error}");
    assert_eq!(git(&repo, &["branch", "--show-current"]), "main");
    let outside = tempfile::tempdir().unwrap();
    git(outside.path(), &["init", "-b", "main"]);
    refused(call(&app, &full, "worktree_create", json!({"repo_path": outside.path(), "branch": "x", "layout": "empty"})).await, ErrorCode::UnknownProjectRoot);
    // Connector clients branch from the project root, wherever in it they point.
    std::fs::create_dir(repo.join("src")).unwrap();
    let from_sub = call(&app, &full, "worktree_create", json!({"repo_path": repo.join("src"), "branch": "connector-branch", "layout": "empty"})).await.unwrap();
    assert_eq!(from_sub["cwd"], crate::git::conventional_worktree_path(&repo.canonicalize().unwrap(), "connector-branch").unwrap().display().to_string(), "{from_sub}");
    let not_git = tempfile::tempdir().unwrap();
    refused(call(&app, &owner, "worktree_create", json!({"repo_path": not_git.path(), "branch": "x", "layout": "empty"})).await, ErrorCode::NotAGitRepository);
    refused(call(&app, &owner, "worktree_create", json!({"repo_path": repo, "branch": "-x"})).await, ErrorCode::InvalidArguments);
}

#[tokio::test]
async fn connector_worktree_create_refuses_presets_and_prompts() {
    let (app, _provider, root, _workspace) = fixture().await;
    // An invalid branch stops every call that passes the preset rule before
    // anything is created.
    let preset = json!({"repo_path": root.path(), "branch": "-not-a-branch", "agent_preset_id": "claude"});
    let prompt = json!({"repo_path": root.path(), "branch": "-not-a-branch", "initial_prompt": "Fix the parser"});
    for args in [&preset, &prompt] {
        let error = refused(call(&app, &connector(ControlAccess::FullAccess), "worktree_create", args.clone()).await, ErrorCode::AccessDenied);
        assert!(error.message.contains("Only the Codemux owner"), "{error}");
        refused(call(&app, &named(ControlAccess::FullAccess), "worktree_create", args.clone()).await, ErrorCode::InvalidArguments);
        refused(call(&app, &ControlCaller::trusted(), "worktree_create", args.clone()).await, ErrorCode::InvalidArguments);
    }
    // Without a preset or prompt the connector client is past that rule too.
    refused(call(&app, &connector(ControlAccess::FullAccess), "worktree_create", json!({"repo_path": root.path(), "branch": "-not-a-branch"})).await, ErrorCode::InvalidArguments);
    let description = registry::tools_for(&connector(ControlAccess::FullAccess)).into_iter().find(|tool| tool.name == "worktree_create").unwrap().description;
    assert!(description.contains("Presets and prompts are available only on this computer"), "{description}");
}
