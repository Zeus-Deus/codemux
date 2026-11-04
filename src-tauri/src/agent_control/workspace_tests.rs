//! Tests for the workspace and worktree tools, `thread_launch` into a new
//! worktree, and outside answers to approvals and questions, through the
//! registry as every outside door calls it.
//!
//! A test that creates or closes a workspace or a worktree runs in an
//! isolated child process (`test_isolation::isolated`): worktrees, the layout
//! file and terminals then live in a private root, never in the developer's
//! own Codemux state.
use super::tests::{connector_caller, connector_off, fixture, mock_agent_provider, native_caller_fixture, settle_operation};
use super::{execute, registry, ClientName, ControlAccess, ControlCaller, ControlError, ErrorCode, NativeControlState};
use crate::agent_provider::{
    CompletedItem, ProviderKind, ProviderRuntimeEvent as Event, QuestionResolution, RequestId, SessionStatus, ThreadId,
    TurnId, TurnStatus, UserQuestion, UserQuestionSet,
};
use crate::commands::agent_chat::forward_event;
use crate::database::DatabaseStore;
use crate::state::{AppStateStore, WorkspaceSnapshot};
use crate::test_isolation::isolated;
use mock_agent_provider::{MockAgentProvider, MockCall};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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

/// Holds the next receipted call's work until a permit is added.
fn hold_next_work(app: &App) -> Arc<tokio::sync::Semaphore> {
    let barrier = Arc::new(tokio::sync::Semaphore::new(0));
    *app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap() = Some(barrier.clone());
    barrier
}

/// Returns once a call took the barrier from `hold_next_work`: it was admitted
/// and its work is waiting for the permit.
async fn work_is_held(app: &App) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the call reaches its work");
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
    let barrier = hold_next_work(&app);
    let held_args = json!({"path": other.path(), "client_request_id": "create-2"});
    let held = {
        let (handle, caller, args) = (app.handle().clone(), owner.clone(), held_args.clone());
        tokio::spawn(async move { execute(&handle, &caller, "workspace_create", args).await })
    };
    work_is_held(&app).await;
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
    let root_delete = json!({"workspace_id": workspace, "delete_worktree": true, "client_request_id": "root-delete"});
    let error = refused(call(&app, &full, "workspace_close", root_delete.clone()).await, ErrorCode::WorkspaceCloseRefused);
    assert!(error.message.contains("cannot be deleted"), "{error}");
    assert!(app.state::<AppStateStore>().find_workspace(&workspace).is_some(), "a refused close closes nothing");
    // The refusal came after admission, so its key holds a failed receipt: a
    // retry returns that receipt as data, as every keyed tool's retry does.
    let retry = call(&app, &full, "workspace_close", root_delete).await.expect("a failed receipt is data");
    assert_eq!(retry["state"], "failed", "{retry}");
    assert_eq!(retry["error"]["code"], "workspace_close_refused", "{retry}");
    assert!(app.state::<AppStateStore>().find_workspace(&workspace).is_some(), "the retry closes nothing");
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

/// A call whose caller goes away (an HTTP client disconnecting, a proxy
/// timing out) only stops waiting. The checkout is still made, claimed and
/// recorded: no orphan checkout, no receipt left running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_create_finishes_when_the_caller_goes_away() {
    if isolated(&format!("{MODULE}worktree_create_finishes_when_the_caller_goes_away")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let owner = named(ControlAccess::FullAccess);
    let args = json!({"repo_path": repo, "branch": "dropped", "layout": "empty", "client_request_id": "wt-dropped"});
    let barrier = hold_next_work(&app);
    let held = {
        let (handle, caller, args) = (app.handle().clone(), owner.clone(), args.clone());
        tokio::spawn(async move { execute(&handle, &caller, "worktree_create", args).await })
    };
    work_is_held(&app).await;
    let running = call(&app, &owner, "worktree_create", args.clone()).await.unwrap();
    assert_eq!(running["state"], "running", "{running}");
    let reserved = running["workspace_id"].as_str().unwrap().to_string();

    held.abort();
    assert!(held.await.unwrap_err().is_cancelled(), "the caller went away while its work was held");
    barrier.add_permits(1);
    let settled = settle_once(&app, &owner, &running).await;
    assert_eq!((settled["state"].clone(), settled["workspace_id"].clone()), (json!("succeeded"), json!(reserved)), "{settled}");
    let workspace = app.state::<AppStateStore>().find_workspace(&reserved).expect("the workspace exists under the reserved id");
    let path = crate::git::conventional_worktree_path(&repo, "dropped").unwrap();
    assert_eq!(workspace.worktree_path.as_deref(), Some(path.display().to_string().as_str()));
    let retry = call(&app, &owner, "worktree_create", args).await.unwrap();
    assert_eq!((retry["workspace_id"].clone(), retry["replayed"].clone()), (json!(reserved), json!(true)), "{retry}");
}

#[tokio::test]
async fn connector_worktree_create_refuses_presets() {
    let (app, _provider, root, _workspace) = fixture().await;
    // An invalid branch stops every call that passes the preset rule before
    // anything is created.
    let preset = json!({"repo_path": root.path(), "branch": "-not-a-branch", "agent_preset_id": "claude"});
    let prompted = json!({"repo_path": root.path(), "branch": "-not-a-branch", "agent_preset_id": "claude", "initial_prompt": "Fix the parser"});
    for args in [&preset, &prompted] {
        let error = refused(call(&app, &connector(ControlAccess::FullAccess), "worktree_create", args.clone()).await, ErrorCode::AccessDenied);
        assert!(error.message.contains("Only the Codemux owner"), "{error}");
        refused(call(&app, &named(ControlAccess::FullAccess), "worktree_create", args.clone()).await, ErrorCode::InvalidArguments);
        refused(call(&app, &ControlCaller::trusted(), "worktree_create", args.clone()).await, ErrorCode::InvalidArguments);
    }
    // Without a preset nothing launches (a prompt alone is dropped), so the
    // connector client is past that rule.
    for args in [json!({"repo_path": root.path(), "branch": "-not-a-branch"}), json!({"repo_path": root.path(), "branch": "-not-a-branch", "initial_prompt": "Fix the parser"})] {
        refused(call(&app, &connector(ControlAccess::FullAccess), "worktree_create", args).await, ErrorCode::InvalidArguments);
    }
    let description = registry::tools_for(&connector(ControlAccess::FullAccess)).into_iter().find(|tool| tool.name == "worktree_create").unwrap().description;
    assert!(description.contains("Presets and prompts are available only on this computer"), "{description}");
}

// ── thread_launch into a new worktree ─────────────────────────────────────

fn launch_args(workspace: &str, key: &str, worktree: Value) -> Value {
    json!({
        "workspace_id": workspace, "client_request_id": key, "provider": "claude", "permission_mode": "default",
        "title": "Fix the parser", "message": "Synthetic fixture task", "worktree": worktree,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_launch_worktree_is_one_idempotent_background_call() {
    if isolated(&format!("{MODULE}thread_launch_worktree_is_one_idempotent_background_call")) {
        return;
    }
    let (app, provider, _root, workspace, repo) = git_fixture().await;
    let main = git(&repo, &["rev-parse", "HEAD"]);
    // The root checkout sits on another branch with its own commit, so a
    // worktree forked from HEAD instead of the default branch would show.
    git(&repo, &["checkout", "-b", "side"]);
    git(&repo, &["commit", "--allow-empty", "-m", "side only"]);
    let side = git(&repo, &["rev-parse", "HEAD"]);
    let state = app.state::<AppStateStore>();
    state.set_workspace_project_identity(&workspace, Some("project-uid".into()), None);
    let focused = active(&app);
    let owner = ControlCaller::trusted();
    let args = launch_args(&workspace, "wt-launch", json!({"branch": "feat-x"}));

    let launched = call(&app, &owner, "thread_launch", args.clone()).await.unwrap();
    let reserved = launched["workspace_id"].as_str().unwrap().to_string();
    assert_ne!(reserved, workspace, "the receipt names the new workspace at once");
    assert!(launched["thread_id"].is_string());
    let settled = settle_operation(&app, &launched).await;
    assert_eq!(settled["state"], "succeeded", "{settled}");
    let path = crate::git::conventional_worktree_path(&repo, "feat-x").unwrap();
    let created = state.find_workspace(&reserved).expect("created under the reserved id");
    assert_eq!(created.worktree_path.as_deref(), Some(path.display().to_string().as_str()));
    assert_eq!(created.project_uid.as_deref(), Some("project-uid"), "nests under the source workspace's project");
    assert_eq!(created.title, "Fix the parser");
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), main, "forked from the default branch");
    assert_eq!((git(&repo, &["rev-parse", "HEAD"]), git(&repo, &["branch", "--show-current"])), (side, "side".into()), "the root is untouched");
    assert_eq!(active(&app), focused);
    let result = &settled["result"];
    assert_eq!(result["workspace_id"], reserved);
    assert_eq!(result["worktree"], json!({"branch": "feat-x", "base": "main", "base_commit": main, "path": path.display().to_string()}));
    let thread = settled["thread_id"].as_str().unwrap();
    let start = provider.start_inputs().into_iter().find(|input| input.thread_id.0 == thread).unwrap();
    assert_eq!(start.cwd, path, "the thread runs in the new checkout");

    let retry = call(&app, &owner, "thread_launch", args.clone()).await.unwrap();
    assert_eq!(retry["operation_id"], launched["operation_id"]);
    let starts = provider.calls.snapshot().iter().filter(|call| matches!(call, MockCall::StartSession(_))).count();
    assert_eq!(starts, 1);
    let mut changed = args;
    changed["worktree"]["base"] = json!("side");
    refused(call(&app, &owner, "thread_launch", changed).await, ErrorCode::RequestKeyConflict);

    // Without a branch, the title names one that is free.
    let named_branch = settle_operation(&app, &call(&app, &owner, "thread_launch", launch_args(&workspace, "wt-titled", json!({}))).await.unwrap()).await;
    assert_eq!(named_branch["result"]["worktree"]["branch"], "fix-the-parser", "{named_branch}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_launch_worktree_refusals_are_synchronous() {
    if isolated(&format!("{MODULE}thread_launch_worktree_refusals_are_synchronous")) {
        return;
    }
    let (app, _provider, _root, workspace, repo) = git_fixture().await;
    git(&repo, &["branch", "taken"]);
    let owner = ControlCaller::trusted();
    refused(call(&app, &owner, "thread_launch", launch_args(&workspace, "taken", json!({"branch": "taken"}))).await, ErrorCode::WorktreeConflict);
    no_receipt(&app, &owner, "taken", "thread_launch");
    let supervised = named(ControlAccess::Supervised);
    refused(call(&app, &supervised, "thread_launch", launch_args(&workspace, "supervised", json!({"branch": "fresh"}))).await, ErrorCode::AccessDenied);
    no_receipt(&app, &supervised, "supervised", "thread_launch");
    refused(call(&app, &owner, "thread_launch", launch_args(&workspace, "bogus", json!({"bogus": 1}))).await, ErrorCode::InvalidArguments);
    refused(call(&app, &owner, "thread_launch", launch_args(&workspace, "dash", json!({"branch": "-x"}))).await, ErrorCode::InvalidArguments);
    refused(call(&app, &owner, "thread_launch", launch_args(&workspace, "no-base", json!({"base": "missing-base", "start_from_origin": false}))).await, ErrorCode::WorktreeBaseUnavailable);
    no_receipt(&app, &owner, "no-base", "thread_launch");
    let elsewhere = tempfile::tempdir().unwrap();
    let not_git = app.state::<AppStateStore>().create_empty_workspace_at_path(elsewhere.path().to_path_buf()).0;
    refused(call(&app, &owner, "thread_launch", launch_args(&not_git, "not-git", json!({}))).await, ErrorCode::NotAGitRepository);
    assert!(crate::git::conventional_worktree_path(&repo, "fresh").is_none_or(|path| !path.exists()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_launch_worktree_required_origin_failure_fails_receipt_without_checkout() {
    if isolated(&format!("{MODULE}thread_launch_worktree_required_origin_failure_fails_receipt_without_checkout")) {
        return;
    }
    let (app, provider, _root, workspace, repo) = git_fixture().await;
    git(&repo, &["remote", "add", "origin", repo.join("gone.git").to_str().unwrap()]);
    let owner = ControlCaller::trusted();
    let launched = call(&app, &owner, "thread_launch", launch_args(&workspace, "origin-gone", json!({"branch": "needs-origin", "start_from_origin": true}))).await.unwrap();
    let settled = settle_operation(&app, &launched).await;
    assert_eq!(settled["state"], "failed", "{settled}");
    assert_eq!(settled["error"]["code"], "worktree_base_unavailable", "{settled}");
    let path = crate::git::conventional_worktree_path(&repo, "needs-origin").unwrap();
    assert!(!path.exists(), "no checkout without a fresh base");
    assert!(app.state::<AppStateStore>().find_workspace(launched["workspace_id"].as_str().unwrap()).is_none());
    assert!(provider.start_inputs().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_launch_worktree_keeps_checkout_when_start_fails() {
    if isolated(&format!("{MODULE}thread_launch_worktree_keeps_checkout_when_start_fails")) {
        return;
    }
    let (app, provider, _root, workspace, repo) = git_fixture().await;
    provider.fail_next_start(crate::agent_provider::ProviderError::ValidationError { message: "synthetic start failure".into() });
    let owner = ControlCaller::trusted();
    let launched = call(&app, &owner, "thread_launch", launch_args(&workspace, "start-fails", json!({"branch": "kept"}))).await.unwrap();
    let settled = settle_operation(&app, &launched).await;
    assert_eq!(settled["state"], "failed", "{settled}");
    let reserved = launched["workspace_id"].as_str().unwrap();
    let message = settled["error"]["message"].as_str().unwrap();
    assert!(message.contains(reserved) && message.contains("kept for inspection"), "{message}");
    assert!(app.state::<AppStateStore>().find_workspace(reserved).is_some(), "setup scripts may have written to it");
    assert!(crate::git::conventional_worktree_path(&repo, "kept").unwrap().exists());
}

// ── authority withdrawn while queued on the lifecycle lock ───────────────
//
// A checkout removal holds the lifecycle lock exclusively for as long as its
// delete takes, and an open holds it shared. A lifecycle call queued behind
// either must ask again for its caller's authority once it gets the lock:
// the owner may have revoked the grant meanwhile.

/// Runs a call in a task of its own, as a door does.
fn spawn_call(app: &App, caller: &ControlCaller, tool: &'static str, args: &Value) -> tokio::task::JoinHandle<Result<Value, ControlError>> {
    let (handle, caller, args) = (app.handle().clone(), caller.clone(), args.clone());
    tokio::spawn(async move { execute(&handle, &caller, tool, args).await })
}

/// The receipt a keyed call was admitted under, once it is.
async fn admitted(app: &App, caller: &ControlCaller, tool: &str, args: &Value) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(receipt) = super::service::recorded(app.handle(), caller, tool, args).unwrap() {
                break receipt;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the call is admitted")
}

/// Nothing a lifecycle call does before the lock takes this long. Whatever it
/// reached, it must not go on once the grant is gone.
async fn reach_the_lock() {
    tokio::time::sleep(Duration::from_millis(500)).await;
}

/// A receipt once it settled, read directly: a revoked caller can no longer
/// ask for it.
async fn settled(app: &App, caller: &ControlCaller, operation_id: &Value) -> crate::database::agent_control::OperationReceipt {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let found = receipt(app, caller, operation_id);
            if found.state != "accepted" && found.state != "running" {
                break found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the receipt settles")
}

#[track_caller]
fn failed_as_revoked(receipt: &crate::database::agent_control::OperationReceipt) {
    assert_eq!(receipt.state, "failed", "{receipt:?}");
    assert_eq!(receipt.error.as_ref().map(|error| error["code"].clone()), Some(json!("grant_revoked")), "{receipt:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_create_revoked_while_queued_on_the_lifecycle_lock_opens_nothing() {
    if isolated(&format!("{MODULE}workspace_create_revoked_while_queued_on_the_lifecycle_lock_opens_nothing")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let caller = connector_caller(&app, "queued-open-client", "queued-open-grant", ControlAccess::FullAccess).await;
    std::fs::create_dir(repo.join("sub")).unwrap();
    let args = json!({"path": repo.join("sub"), "client_request_id": "queued-open"});
    let before = count(&app);

    let removal = crate::worktree_cleanup::removal_guard().await;
    let queued = spawn_call(&app, &caller, "workspace_create", &args);
    let running = admitted(&app, &caller, "workspace_create", &args).await;
    let reserved = running["workspace_id"].as_str().unwrap().to_string();
    reach_the_lock().await;
    assert_eq!(count(&app), before, "nothing opens while a removal holds the lock");
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "queued-open-client".into()).unwrap();
    drop(removal);

    refused(queued.await.unwrap(), ErrorCode::GrantRevoked);
    failed_as_revoked(&settled(&app, &caller, &running["operation_id"]).await);
    assert!(app.state::<AppStateStore>().find_workspace(&reserved).is_none());
    assert_eq!(count(&app), before, "no workspace, so no terminal and no setup script");
    connector_off(&app);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_create_revoked_while_queued_on_the_lifecycle_lock_checks_out_nothing() {
    if isolated(&format!("{MODULE}worktree_create_revoked_while_queued_on_the_lifecycle_lock_checks_out_nothing")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let caller = connector_caller(&app, "queued-wt-client", "queued-wt-grant", ControlAccess::FullAccess).await;
    let args = json!({"repo_path": repo, "branch": "queued-branch", "layout": "empty", "client_request_id": "queued-wt"});
    let path = crate::git::conventional_worktree_path(&repo.canonicalize().unwrap(), "queued-branch").unwrap();
    let before = count(&app);

    let removal = crate::worktree_cleanup::removal_guard().await;
    let queued = spawn_call(&app, &caller, "worktree_create", &args);
    let running = admitted(&app, &caller, "worktree_create", &args).await;
    let reserved = running["workspace_id"].as_str().unwrap().to_string();
    reach_the_lock().await;
    assert!(!path.exists(), "no checkout while a removal holds the lock");
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "queued-wt-client".into()).unwrap();
    drop(removal);

    refused(queued.await.unwrap(), ErrorCode::GrantRevoked);
    failed_as_revoked(&settled(&app, &caller, &running["operation_id"]).await);
    assert!(!path.exists(), "no checkout");
    assert_eq!(git(&repo, &["branch", "--list", "queued-branch"]), "", "no branch");
    assert!(app.state::<AppStateStore>().find_workspace(&reserved).is_none());
    assert_eq!(count(&app), before);
    connector_off(&app);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_launch_worktree_revoked_while_queued_on_the_lifecycle_lock_starts_nothing() {
    if isolated(&format!("{MODULE}thread_launch_worktree_revoked_while_queued_on_the_lifecycle_lock_starts_nothing")) {
        return;
    }
    let (app, provider, _root, workspace, repo) = git_fixture().await;
    let caller = connector_caller(&app, "queued-launch-client", "queued-launch-grant", ControlAccess::FullAccess).await;
    let path = crate::git::conventional_worktree_path(&repo, "queued-launch").unwrap();
    let before = count(&app);

    let removal = crate::worktree_cleanup::removal_guard().await;
    let launched = call(&app, &caller, "thread_launch", launch_args(&workspace, "queued-launch", json!({"branch": "queued-launch"}))).await.unwrap();
    let reserved = launched["workspace_id"].as_str().unwrap().to_string();
    reach_the_lock().await;
    assert!(!path.exists(), "no checkout while a removal holds the lock");
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "queued-launch-client".into()).unwrap();
    drop(removal);

    failed_as_revoked(&settled(&app, &caller, &launched["operation_id"]).await);
    assert!(!path.exists(), "no checkout, so no setup script");
    assert_eq!(git(&repo, &["branch", "--list", "queued-launch"]), "", "no branch");
    assert!(app.state::<AppStateStore>().find_workspace(&reserved).is_none());
    assert_eq!(count(&app), before);
    assert!(provider.start_inputs().is_empty(), "no thread started");
    connector_off(&app);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_close_revoked_while_queued_on_the_lifecycle_lock_deletes_nothing() {
    if isolated(&format!("{MODULE}workspace_close_revoked_while_queued_on_the_lifecycle_lock_deletes_nothing")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let parent = tempfile::tempdir().unwrap();
    let checkout = parent.path().join("queued-close");
    git(&repo, &["worktree", "add", "-b", "queued-close", checkout.to_str().unwrap()]);
    let state = app.state::<AppStateStore>();
    let closing = state.create_empty_workspace_at_path(checkout.clone()).0;
    state.set_workspace_worktree(&closing, checkout.display().to_string(), "queued-close".into());
    let caller = connector_caller(&app, "queued-close-client", "queued-close-grant", ControlAccess::FullAccess).await;
    let args = json!({"workspace_id": closing, "delete_worktree": true, "delete_branch": true, "client_request_id": "queued-close"});

    // An open holds the lock shared, so the removal waits for it.
    let opening = crate::worktree_cleanup::opening_guard().await;
    let queued = spawn_call(&app, &caller, "workspace_close", &args);
    let running = admitted(&app, &caller, "workspace_close", &args).await;
    // The close itself does not wait for the lock: the workspace goes first.
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.find_workspace(&closing).is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the workspace closes before the delete");
    reach_the_lock().await;
    assert!(checkout.exists(), "nothing is deleted while an open holds the lock");
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "queued-close-client".into()).unwrap();
    drop(opening);

    refused(queued.await.unwrap(), ErrorCode::GrantRevoked);
    failed_as_revoked(&settled(&app, &caller, &running["operation_id"]).await);
    assert!(checkout.join(".git").exists(), "the checkout's files stay");
    git(&repo, &["rev-parse", "--verify", "refs/heads/queued-close"]);
    connector_off(&app);
}

/// The close's dirty-checkout pre-flight reads git. A requester whose
/// authority was withdrawn meanwhile does not get to stop the workspace's
/// Hermes chats either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_close_withdrawn_during_its_preflight_stops_no_hermes_chat() {
    if isolated(&format!("{MODULE}workspace_close_withdrawn_during_its_preflight_stops_no_hermes_chat")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let hermes = Arc::new(MockAgentProvider::new(ProviderKind::Hermes));
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_hermes(hermes.clone()).await;
    let parent = tempfile::tempdir().unwrap();
    let checkout = parent.path().join("preflight-close");
    git(&repo, &["worktree", "add", "-b", "preflight-close", checkout.to_str().unwrap()]);
    let state = app.state::<AppStateStore>();
    let closing = state.create_empty_workspace_at_path(checkout.clone()).0;
    state.set_workspace_worktree(&closing, checkout.display().to_string(), "preflight-close".into());
    let profile = crate::agent_provider::hermes::profile::Profile {
        schema_version: 1,
        host: "local".into(),
        installation: parent.path().join("hermes"),
        root: parent.path().join("hermes-home"),
        id: "default".into(),
        home: parent.path().join("hermes-home"),
        identity: "fixture".into(),
    };
    app.state::<DatabaseStore>()
        .save_hermes_binding(&crate::agent_provider::hermes::binding::Binding {
            schema_version: 1, profile, thread_id: "preflight-hermes".into(), workspace_id: Some(closing.clone()),
            cwd: checkout.clone(), acp_session_id: None, current_native_id: None, root_native_id: None,
            model_override: None, resolved_model: None, permission_mode: None, compatibility: String::new(),
            cleanup_pending: false,
        })
        .unwrap();

    let withdrawn = crate::workspace_service::Checkpoint::new(|| Err("grant_revoked: withdrawn".into()));
    let error = crate::workspace_service::WorkspaceService::new(app.handle())
        .checked(withdrawn)
        .close(crate::workspace_service::CloseWorkspace {
            workspace_id: closing.clone(),
            remove_worktree: true,
            delete_branch: true,
            force: false,
        })
        .await
        .unwrap_err();

    assert_eq!(error, "grant_revoked: withdrawn");
    let stops: Vec<_> = hermes.calls.snapshot().into_iter().filter(|call| matches!(call, MockCall::StopSession(_))).collect();
    assert!(stops.is_empty(), "{stops:?}");
    assert!(state.find_workspace(&closing).is_some(), "the workspace stays as it was");
    assert!(checkout.join(".git").exists());
}

// ── outside answers: approvals and questions ──────────────────────────────

async fn launched(app: &App, key: &str, workspace: &str, mode: &str) -> String {
    let receipt = call(app, &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id": workspace, "client_request_id": key, "provider": "claude", "permission_mode": mode,
    })).await.unwrap();
    let launched = settle_operation(app, &receipt).await;
    assert_eq!(launched["state"], "succeeded", "{launched}");
    launched["thread_id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connector_respond_cannot_allow_always() {
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "respond-launch", &workspace, "default").await;
    app.state::<NativeControlState>().observe_event(&Event::RequestOpened {
        thread_id: ThreadId(thread.clone()), turn_id: TurnId("approval-turn".into()),
        request_id: RequestId("approval-request".into()), request_kind: "tool".into(),
        payload: json!({}), tool_use_id: None, subagent_id: None,
    });
    let status = call(&app, &ControlCaller::trusted(), "thread_status", json!({"workspace_id": workspace, "thread_id": thread})).await.unwrap();
    let request = status["pending_approvals"][0]["request_id"].clone();
    let respond = |key: &str, decision: &str| json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": key, "request_id": request, "decision": decision});
    let full = connector(ControlAccess::FullAccess);
    let error = refused(call(&app, &full, "thread_respond", respond("always", "allow_always")).await, ErrorCode::PermissionDenied);
    assert!(error.message.contains("allow_always"), "{error}");
    no_receipt(&app, &full, "always", "thread_respond");
    assert!(!provider.calls.snapshot().iter().any(|call| matches!(call, MockCall::RespondToRequest(_, _))));
    // A local client at full access still may; it is on this computer.
    let local = call(&app, &named(ControlAccess::FullAccess), "thread_respond", respond("local-always", "allow_always")).await.unwrap();
    assert_eq!(local["state"], "accepted", "{local}");
    let _ = settle_once(&app, &named(ControlAccess::FullAccess), &local).await;
    // The connector client can still allow once.
    app.state::<NativeControlState>().observe_event(&Event::RequestOpened {
        thread_id: ThreadId(thread.clone()), turn_id: TurnId("approval-turn".into()),
        request_id: RequestId("second-request".into()), request_kind: "tool".into(),
        payload: json!({}), tool_use_id: None, subagent_id: None,
    });
    let status = call(&app, &ControlCaller::trusted(), "thread_status", json!({"workspace_id": workspace, "thread_id": thread})).await.unwrap();
    let second = status["pending_approvals"].as_array().unwrap().iter().find(|a| a["request_id"] != request).unwrap()["request_id"].clone();
    let once = call(&app, &full, "thread_respond", json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": "once", "request_id": second, "decision": "allow_once"})).await.unwrap();
    assert_eq!(settle_once(&app, &full, &once).await["state"], "succeeded");
}

/// `settle_operation` reads receipts as the owner; these belong to other principals.
async fn settle_once(app: &App, caller: &ControlCaller, receipt: &Value) -> Value {
    let id = receipt["operation_id"].clone();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let status = call(app, caller, "operation_status", json!({"operation_id": id})).await.unwrap();
            if status["state"] != "accepted" && status["state"] != "running" {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

/// Run `turn` on `thread`, reply, ask one question, then end the turn: the
/// shape of a worker stopping to ask.
fn ask(app: &App, thread: &str, turn: &str, question: &str) {
    let id = |text: &str| (ThreadId(thread.into()), TurnId(text.into()));
    forward_event(app.handle(), Event::SessionStateChanged { thread_id: id(turn).0, status: SessionStatus::Running { active_turn: id(turn).1 } });
    forward_event(app.handle(), Event::ItemCompleted {
        thread_id: id(turn).0, turn_id: id(turn).1, item: CompletedItem::AssistantText { text: "I need one decision before I continue.".into() }, subagent_id: None,
    });
    forward_event(app.handle(), Event::QuestionsAsked {
        thread_id: id(turn).0,
        question: UserQuestionSet {
            id: question.into(), target: "provider-route".into(), source_item_id: "item-q".into(), source_turn_id: turn.into(),
            text: "Pick the storage".into(),
            questions: vec![UserQuestion { title: "Which database?".into(), options: vec!["SQLite".into(), "PostgreSQL".into()] }],
            subagent_id: None,
        },
    });
    forward_event(app.handle(), Event::TurnCompleted { thread_id: id(turn).0, turn_id: id(turn).1, status: TurnStatus::Success, usage: None });
}

/// Run `turn` on `thread` to its end with one reply: the run an answer
/// continues.
fn reply(app: &App, thread: &str, turn: &str, text: &str) {
    let (thread_id, turn_id) = (ThreadId(thread.into()), TurnId(turn.into()));
    forward_event(app.handle(), Event::SessionStateChanged { thread_id: thread_id.clone(), status: SessionStatus::Running { active_turn: turn_id.clone() } });
    forward_event(app.handle(), Event::ItemCompleted {
        thread_id: thread_id.clone(), turn_id: turn_id.clone(), item: CompletedItem::AssistantText { text: text.into() }, subagent_id: None,
    });
    forward_event(app.handle(), Event::TurnCompleted { thread_id, turn_id, status: TurnStatus::Success, usage: None });
}

fn answer(workspace: &str, thread: &str, key: &str, question: &str) -> Value {
    json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": key, "question_id": question, "answers": ["SQLite"]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outside_answer_resolves_a_question_and_the_wait_returns_done() {
    let (app, _, _root, workspace) = fixture().await;
    let mut asking = MockAgentProvider::new(ProviderKind::Claude);
    asking.async_questions = true;
    let provider = Arc::new(asking);
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_claude(provider.clone()).await;
    let thread = launched(&app, "answer-launch", &workspace, "default").await;
    ask(&app, &thread, "t1", "storage");
    let bot = named(ControlAccess::FullAccess);
    let target = json!({"workspace_id": workspace, "thread_id": thread});

    let mut wait = target.clone();
    wait["timeout_ms"] = json!(1000);
    let asked = call(&app, &bot, "thread_wait", wait).await.unwrap();
    assert_eq!(asked["state"], "needs_input", "{asked}");
    assert_eq!(asked["pending"]["questions"][0]["question_id"], "storage");
    let waiter = {
        let (handle, caller) = (app.handle().clone(), bot.clone());
        let mut args = target.clone();
        args["since"] = asked["wait_cursor"].clone();
        args["timeout_ms"] = json!(5000);
        tokio::spawn(async move { execute(&handle, &caller, "thread_wait", args).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    let answered = call(&app, &bot, "thread_answer", answer(&workspace, &thread, "answer-1", "storage")).await.unwrap();
    assert_eq!(answered["state"], "succeeded", "{answered}");
    assert_eq!(answered["resolution"]["status"], "answered");
    assert_eq!(answered["resolution"]["answers"], json!(["SQLite"]));
    assert_eq!(answered["turn_id"], "native-turn");
    let woken = waiter.await.unwrap().unwrap();
    assert_eq!(woken["pending"]["questions"], json!([]), "the answer wakes the waiter: {woken}");

    // The run that asked is over; the answer's result comes from the run
    // thread_answer names. Until that run ends, waiting on it is not done.
    let mut follow = target.clone();
    follow["turn_id"] = answered["turn_id"].clone();
    follow["timeout_ms"] = json!(200);
    let continuing = call(&app, &bot, "thread_wait", follow.clone()).await.unwrap();
    assert_eq!((continuing["state"].clone(), continuing["timed_out"].clone()), (json!("running"), json!(true)), "{continuing}");
    reply(&app, &thread, "native-turn", "SQLite it is: the schema is in place.");
    follow["timeout_ms"] = json!(5000);
    let done = call(&app, &bot, "thread_wait", follow).await.unwrap();
    assert_eq!((done["state"].clone(), done["turn_id"].clone()), (json!("done"), json!("native-turn")), "{done}");
    assert_eq!(done["final_text"], "SQLite it is: the schema is in place.");

    // One submission per key: a retry replays, a new key finds it closed.
    let retry = call(&app, &bot, "thread_answer", answer(&workspace, &thread, "answer-1", "storage")).await.unwrap();
    assert_eq!((retry["operation_id"].clone(), retry["replayed"].clone()), (answered["operation_id"].clone(), json!(true)));
    refused(call(&app, &bot, "thread_answer", answer(&workspace, &thread, "answer-2", "storage")).await, ErrorCode::QuestionClosed);
    let submissions: Vec<String> = provider.calls.snapshot().into_iter().filter_map(|call| match call {
        MockCall::AnswerQuestion(_, submission) => Some(submission),
        _ => None,
    }).collect();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(answered["resolution"]["submission_id"], submissions[0]);
    assert!(uuid::Uuid::parse_str(&submissions[0]).is_ok());

    refused(call(&app, &bot, "thread_answer", answer(&workspace, &thread, "answer-3", "no-such-question")).await, ErrorCode::QuestionNotFound);
    let mut wrong = answer(&workspace, &thread, "answer-4", "storage");
    wrong["answers"] = json!(["SQLite", "Two answers for one prompt"]);
    ask(&app, &thread, "t2", "second");
    wrong["question_id"] = json!("second");
    refused(call(&app, &bot, "thread_answer", wrong).await, ErrorCode::InvalidArguments);
    refused(call(&app, &named(ControlAccess::ReadOnly), "thread_answer", answer(&workspace, &thread, "answer-5", "second")).await, ErrorCode::AccessDenied);
    no_receipt(&app, &bot, "answer-4", "thread_answer");
}

/// An answer being delivered, or one the provider never confirmed, keeps the
/// question in pending.questions. Another answer is refused for that reason,
/// not as "already answered", so a client does not loop between the two.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsettled_answer_keeps_the_question_open_and_refuses_another() {
    let (app, _, _root, workspace) = fixture().await;
    let mut asking = MockAgentProvider::new(ProviderKind::Claude);
    asking.async_questions = true;
    let provider = Arc::new(asking);
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_claude(provider.clone()).await;
    let thread = launched(&app, "unsettled-launch", &workspace, "default").await;
    ask(&app, &thread, "t1", "storage");
    let bot = named(ControlAccess::FullAccess);
    let db = app.state::<DatabaseStore>();
    let answers = vec!["SQLite".to_string()];
    for (key, next) in [
        ("while-submitting", QuestionResolution::Submitting { submission_id: "card".into(), answers: answers.clone() }),
        ("while-unknown", QuestionResolution::Unknown { submission_id: "card".into(), answers: answers.clone(), message: "No reply".into() }),
    ] {
        let (_, current) = db.async_question(&thread, "storage").unwrap();
        assert!(db.change_async_question(&thread, "storage", &current, &next).unwrap().is_some());
        let status = call(&app, &bot, "thread_status", json!({"workspace_id": workspace, "thread_id": thread})).await.unwrap();
        assert_eq!(status["pending"]["questions"][0]["question_id"], "storage", "{status}");
        let error = refused(call(&app, &bot, "thread_answer", answer(&workspace, &thread, key, "storage")).await, ErrorCode::QuestionAnswerFailed);
        assert!(error.message.contains("being delivered or awaits confirmation"), "{error}");
        no_receipt(&app, &bot, key, "thread_answer");
    }
    assert!(!provider.calls.snapshot().iter().any(|call| matches!(call, MockCall::AnswerQuestion(..))), "nothing reached the worker");
}

/// Records what each answer that reached the worker asked of the thread's
/// mode. Like Codex, it authorizes each answer at its dispatch boundary;
/// `hold_next_answer` keeps one waiting just before that, as the outbound
/// lock or `thread/read` can.
struct ModeRecorder {
    inner: Arc<MockAgentProvider>,
    overrides: Mutex<Vec<Option<String>>>,
    held: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

impl ModeRecorder {
    fn new(inner: Arc<MockAgentProvider>) -> Self {
        Self { inner, overrides: Mutex::new(Vec::new()), held: Mutex::new(None) }
    }

    /// The first notify fires once the next answer waits; the second lets it go on.
    fn hold_next_answer(&self) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        let (waiting, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        *self.held.lock().unwrap() = Some((waiting.clone(), release.clone()));
        (waiting, release)
    }
}

#[async_trait::async_trait]
impl crate::agent_provider::AgentProvider for ModeRecorder {
    fn kind(&self) -> ProviderKind { self.inner.kind() }
    fn capabilities(&self) -> crate::agent_provider::ProviderCapabilities { self.inner.capabilities() }
    fn supports_async_questions(&self) -> bool { true }
    async fn answer_question(&self, input: crate::agent_provider::AnswerQuestionInput) -> Result<crate::agent_provider::QuestionDelivery, crate::agent_provider::QuestionDeliveryError> {
        let held = self.held.lock().unwrap().take();
        if let Some((waiting, release)) = held {
            waiting.notify_one();
            release.notified().await;
        }
        if let Some(checkpoint) = &input.checkpoint {
            checkpoint.authorize_dispatch().map_err(|error| crate::agent_provider::QuestionDeliveryError::Rejected(error.to_string()))?;
        }
        self.overrides.lock().unwrap().push(input.permission_mode_override.clone());
        self.inner.answer_question(input).await
    }
    async fn start_session(&self, input: crate::agent_provider::StartSessionInput) -> Result<crate::agent_provider::ProviderSession, crate::agent_provider::ProviderError> { self.inner.start_session(input).await }
    async fn send_turn(&self, input: crate::agent_provider::SendTurnInput) -> Result<crate::agent_provider::TurnStartResult, crate::agent_provider::ProviderError> { self.inner.send_turn(input).await }
    async fn interrupt_turn(&self, t: ThreadId, id: Option<TurnId>) -> Result<(), crate::agent_provider::ProviderError> { self.inner.interrupt_turn(t, id).await }
    async fn respond_to_request(&self, t: ThreadId, r: RequestId, d: crate::agent_provider::ApprovalDecision) -> Result<(), crate::agent_provider::ProviderError> { self.inner.respond_to_request(t, r, d).await }
    async fn set_model(&self, t: ThreadId, m: String) -> Result<(), crate::agent_provider::ProviderError> { self.inner.set_model(t, m).await }
    async fn set_permission_mode(&self, t: ThreadId, m: String) -> Result<(), crate::agent_provider::ProviderError> { self.inner.set_permission_mode(t, m).await }
    async fn stop_session(&self, t: ThreadId) -> Result<(), crate::agent_provider::ProviderError> { self.inner.stop_session(t).await }
    async fn list_sessions(&self) -> Result<Vec<crate::agent_provider::ProviderSession>, crate::agent_provider::ProviderError> { self.inner.list_sessions().await }
    async fn has_session(&self, t: &ThreadId) -> bool { self.inner.has_session(t).await }
    async fn turn_active(&self, t: &ThreadId) -> bool { self.inner.turn_active(t).await }
    fn event_stream(&self) -> crate::agent_provider::ProviderEventStream { self.inner.event_stream() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervised_answer_cannot_change_permission_mode() {
    let (app, _, _root, workspace) = fixture().await;
    let recorder = Arc::new(ModeRecorder::new(Arc::new(MockAgentProvider::new(ProviderKind::Claude))));
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_claude(recorder.clone()).await;
    let planning = launched(&app, "plan-launch", &workspace, "plan").await;
    ask(&app, &planning, "t1", "approach");
    let bot = named(ControlAccess::Supervised);

    // The tool has no way to ask for a mode change.
    let mut leave_plan = answer(&workspace, &planning, "leave-plan", "approach");
    leave_plan["permission_mode_override"] = json!("acceptEdits");
    let error = refused(call(&app, &bot, "thread_answer", leave_plan).await, ErrorCode::InvalidArguments);
    assert!(error.message.contains("permission_mode_override"), "{error}");
    assert!(recorder.overrides.lock().unwrap().is_empty(), "refused before reaching the worker");

    let answered = call(&app, &bot, "thread_answer", answer(&workspace, &planning, "plan-answer", "approach")).await.unwrap();
    assert_eq!(answered["resolution"]["status"], "answered", "{answered}");
    assert_eq!(*recorder.overrides.lock().unwrap(), [None], "the answer carries no mode change");
    let record = app.state::<DatabaseStore>().get_agent_chat_session(&planning).unwrap();
    assert_eq!(record.permission_mode.as_deref(), Some("plan"), "the thread is still planning");

    // A supervised client cannot drive a full-access worker by answering it either.
    let unbounded = launched(&app, "full-launch", &workspace, "bypassPermissions").await;
    ask(&app, &unbounded, "t1", "scope");
    refused(call(&app, &bot, "thread_answer", answer(&workspace, &unbounded, "full-answer", "scope")).await, ErrorCode::PermissionCeiling);
    assert_eq!(recorder.overrides.lock().unwrap().len(), 1);
    // Chats do not answer for the person.
    let (app, _, _root, workspace, thread, _authority, actor) = native_caller_fixture("bypassPermissions").await;
    let chat = super::native::qualify(app.handle(), &actor).await.unwrap();
    refused(super::service::thread_answer(app.handle(), &chat, answer(&workspace, &thread, "chat-answer", "q")).await, ErrorCode::PermissionDenied);
}

fn answer_in_background(app: &App, caller: &ControlCaller, args: Value) -> tokio::task::JoinHandle<Result<Value, ControlError>> {
    let (handle, caller) = (app.handle().clone(), caller.clone());
    tokio::spawn(async move { execute(&handle, &caller, "thread_answer", args).await })
}

async fn reaches(gate: &tokio::sync::Notify, what: &str) {
    tokio::time::timeout(Duration::from_secs(10), gate.notified()).await.unwrap_or_else(|_| panic!("the answer never reached {what}"));
}

/// An outside answer keeps its caller's authority until it reaches the
/// worker. Revoking the connector grant while the answer waits in delivery
/// stops it there, and the person can still answer the question.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answer_waiting_in_delivery_stops_when_the_grant_is_revoked() {
    let (app, _, _root, workspace) = fixture().await;
    let recorder = Arc::new(ModeRecorder::new(Arc::new(MockAgentProvider::new(ProviderKind::Claude))));
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_claude(recorder.clone()).await;
    let thread = launched(&app, "revoked-launch", &workspace, "default").await;
    ask(&app, &thread, "t1", "storage");
    let bot = super::tests::connector_caller(&app, "answer-client", "answer-grant", ControlAccess::FullAccess).await;

    let (waiting, release) = recorder.hold_next_answer();
    let answering = answer_in_background(&app, &bot, answer(&workspace, &thread, "revoked-answer", "storage"));
    reaches(&waiting, "delivery").await;
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "answer-client".into()).unwrap();
    release.notify_one();

    let error = refused(answering.await.unwrap(), ErrorCode::QuestionAnswerFailed);
    assert!(error.message.contains("grant_revoked"), "{error}");
    assert!(recorder.overrides.lock().unwrap().is_empty(), "the answer never reached the worker");
    let (_, resolution) = app.state::<DatabaseStore>().async_question(&thread, "storage").unwrap();
    assert!(matches!(resolution, QuestionResolution::Failed { .. }), "the person can still answer it: {resolution:?}");
    super::tests::connector_off(&app);
}

/// A supervised answer stops in delivery once its worker would run beyond
/// the supervised ceiling: the person switched the thread to full access,
/// or the live worker's mode no longer matches the saved one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervised_answer_waiting_in_delivery_stops_when_the_worker_mode_widens() {
    let (app, _, _root, workspace) = fixture().await;
    let recorder = Arc::new(ModeRecorder::new(Arc::new(MockAgentProvider::new(ProviderKind::Claude))));
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_claude(recorder.clone()).await;
    let bot = named(ControlAccess::Supervised);
    for (case, saved, code) in [("switched", true, "permission_ceiling"), ("live-only", false, "permission_state_unconfirmed")] {
        let thread = launched(&app, &format!("{case}-launch"), &workspace, "default").await;
        ask(&app, &thread, "t1", "scope");
        let (waiting, release) = recorder.hold_next_answer();
        let answering = answer_in_background(&app, &bot, answer(&workspace, &thread, case, "scope"));
        reaches(&waiting, "delivery").await;
        if saved {
            let full = crate::database::AgentChatSessionConfig { permission_mode: Some(Some("bypassPermissions".into())), ..Default::default() };
            app.state::<DatabaseStore>().update_agent_chat_session_config(&thread, &full).unwrap();
        }
        app.state::<NativeControlState>().record_worker_mode(&thread, ProviderKind::Claude, Some("bypassPermissions".into()));
        release.notify_one();

        let error = refused(answering.await.unwrap(), ErrorCode::QuestionAnswerFailed);
        assert!(error.message.contains(code), "{case}: {error}");
    }
    assert!(recorder.overrides.lock().unwrap().is_empty(), "no answer reached the worker");
}

/// An answer to a thread whose worker exited resumes the worker first.
/// Revoking the grant while the worker starts stops the answer before it is
/// handed to the worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answer_waiting_on_worker_startup_stops_when_the_grant_is_revoked() {
    use crate::agent_provider::AgentProvider;
    let (app, _, _root, workspace) = fixture().await;
    let mut asking = MockAgentProvider::new(ProviderKind::Codex);
    asking.async_questions = true;
    let codex = Arc::new(asking);
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_codex(codex.clone()).await;
    app.state::<NativeControlState>().fixture_capabilities.lock().unwrap()
        .insert(ProviderKind::Codex, crate::agent_provider::codex::capabilities::build_capabilities(vec![]));
    let launch = call(&app, &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id": workspace, "client_request_id": "startup-launch", "provider": "codex", "permission_mode": "workspace-write",
    })).await.unwrap();
    let launched = settle_operation(&app, &launch).await;
    assert_eq!(launched["state"], "succeeded", "{launched}");
    let thread = launched["thread_id"].as_str().unwrap().to_string();
    ask(&app, &thread, "t1", "storage");
    // The worker exited; its conversation can still be resumed.
    codex.stop_session(ThreadId(thread.clone())).await.unwrap();
    app.state::<DatabaseStore>().set_agent_chat_sdk_session_id(&thread, "codex-conversation").unwrap();
    let bot = super::tests::connector_caller(&app, "startup-client", "startup-grant", ControlAccess::FullAccess).await;

    let (starting, release) = codex.hold_next_start();
    let answering = answer_in_background(&app, &bot, answer(&workspace, &thread, "startup-answer", "storage"));
    reaches(&starting, "worker startup").await;
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "startup-client".into()).unwrap();
    release.notify_one();

    let error = refused(answering.await.unwrap(), ErrorCode::QuestionAnswerFailed);
    assert!(error.message.contains("grant_revoked"), "{error}");
    assert!(!codex.calls.snapshot().iter().any(|call| matches!(call, MockCall::AnswerQuestion(..))), "the answer never reached the worker");
    super::tests::connector_off(&app);
}
