//! Tests for the workspace and worktree tools, `thread_launch` into a new
//! worktree, and outside answers to approvals and questions, through the
//! registry as every outside door calls it.
//!
//! A test that creates or closes a workspace or a worktree runs in an
//! isolated child process (`test_isolation::isolated`): worktrees, the layout
//! file and terminals then live in a private root, never in the developer's
//! own Codemux state.
use super::tests::{connector_caller, connector_off, fixture, mock_agent_provider, native_caller_fixture, settle_operation, web_remote_on};
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

/// A retry of a keyed create replays its receipt even once the folder it
/// opened is gone: the folder is checked only on a new call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_create_retry_replays_after_the_folder_is_gone() {
    if isolated(&format!("{MODULE}workspace_create_retry_replays_after_the_folder_is_gone")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let parent = tempfile::tempdir().unwrap();
    let cases = [
        (named(ControlAccess::FullAccess), parent.path().join("local"), "gone-local"),
        // A connector client's folder must be under a known project root.
        (connector(ControlAccess::FullAccess), repo.join("connector"), "gone-connector"),
    ];
    for (caller, folder, key) in cases {
        std::fs::create_dir(&folder).unwrap();
        let args = json!({"path": folder, "client_request_id": key});
        let created = call(&app, &caller, "workspace_create", args.clone()).await.unwrap();
        assert_eq!(created["created"], true, "{key}: {created}");
        // Close it first: on Windows a folder can't be moved while the
        // workspace's shell still runs in it, and the shell may take a
        // moment to exit after the close.
        let close = json!({"workspace_id": created["workspace_id"], "client_request_id": format!("{key}-close")});
        call(&app, &named(ControlAccess::FullAccess), "workspace_close", close).await.unwrap();
        let moved = folder.with_extension("moved");
        let mut attempts = 0;
        while let Err(error) = std::fs::rename(&folder, &moved) {
            attempts += 1;
            assert!(attempts < 50, "{key}: could not move the folder: {error}");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let after = count(&app);

        let retry = call(&app, &caller, "workspace_create", args).await.expect("a retry replays, not a path error");
        assert_eq!((retry["operation_id"].clone(), retry["workspace_id"].clone()), (created["operation_id"].clone(), created["workspace_id"].clone()), "{key}");
        assert_eq!(retry["replayed"], true, "{key}: {retry}");
        assert_eq!(count(&app), after, "{key}: a replay opens nothing");
        // The key still binds its arguments.
        let other = folder.with_extension("missing");
        refused(call(&app, &caller, "workspace_create", json!({"path": other, "client_request_id": key})).await, ErrorCode::RequestKeyConflict);
    }
}

/// Two opens of one unopened folder under different keys make one
/// workspace: the second gets the first's back, and only the first ran the
/// create's terminal and setup scripts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_creates_of_one_folder_open_one_workspace() {
    if isolated(&format!("{MODULE}concurrent_creates_of_one_folder_open_one_workspace")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, _root, _workspace) = fixture().await;
    app.manage(crate::terminal::PtyState::default());
    let owner = named(ControlAccess::FullAccess);
    let folder = tempfile::tempdir().unwrap();
    let before = count(&app);

    // Both opens have looked for a workspace at the folder and found none
    // before either goes on to make one.
    crate::commands::workspace::open_pause::line_up(folder.path(), 2);
    let first = spawn_call(&app, &owner, "workspace_create", &json!({"path": folder.path(), "client_request_id": "race-a"}));
    let second = spawn_call(&app, &owner, "workspace_create", &json!({"path": folder.path(), "client_request_id": "race-b"}));
    let (first, second) = (first.await.unwrap().unwrap(), second.await.unwrap().unwrap());

    let at_folder: Vec<String> = app.state::<AppStateStore>().snapshot().workspaces.iter()
        .filter(|w| Path::new(&w.cwd) == folder.path())
        .map(|w| w.workspace_id.0.clone())
        .collect();
    assert_eq!(at_folder.len(), 1, "one workspace per folder: {at_folder:?}");
    assert_eq!(count(&app), before + 1);
    assert_eq!(first["workspace_id"], second["workspace_id"], "{first} {second}");
    let mut created = [first["created"].as_bool().unwrap(), second["created"].as_bool().unwrap()];
    created.sort();
    assert_eq!(created, [false, true], "one created it, the other reused it");
    for result in [&first, &second] {
        assert_eq!(receipt(&app, &owner, &result["operation_id"]).workspace_id.as_deref(), Some(at_folder[0].as_str()), "each receipt names the one workspace");
    }
}

/// Every `.mcp.json` below `dir`, symlinks not followed.
#[cfg(unix)]
fn mcp_json_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                pending.push(entry.path());
            } else if entry.file_name() == ".mcp.json" {
                found.push(entry.path());
            }
        }
    }
    found
}

/// Opening, creating and closing workspaces and worktrees, their terminals,
/// and the startup cleanup write no MCP config where an agent would read it:
/// not in the folders, not in their parents up to HOME, not in git's
/// exclude file. Older builds wrote `.mcp.json` at each of these steps, and
/// Claude Code then gave Codemux's tools to every agent below those folders.
/// Unix only: Windows Known Folders ignore the fixture's HOME override.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_lifecycle_writes_no_mcp_config() {
    if isolated(&format!("{MODULE}workspace_lifecycle_writes_no_mcp_config")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, root, _workspace, repo) = git_fixture().await;
    let owner = named(ControlAccess::FullAccess);
    let home = dirs::home_dir().unwrap();
    assert!(home.starts_with(crate::test_isolation::isolated_root().unwrap()), "an isolated HOME");
    let project = home.join("Projects").join("app");
    std::fs::create_dir_all(&project).unwrap();
    git(&project, &["init", "-b", "main"]);
    git(&project, &["commit", "--allow-empty", "-m", "first"]);

    // A workspace in a project under HOME, with its terminal and setup run.
    let created = call(&app, &owner, "workspace_create", json!({"path": project})).await.unwrap();
    assert_eq!(created["created"], true, "{created}");
    // A workspace at HOME itself: the folder that leaked into every project.
    let at_home = call(&app, &owner, "workspace_create", json!({"path": home})).await.unwrap();
    // A worktree workspace.
    let worktree = call(&app, &owner, "worktree_create", json!({"repo_path": repo, "branch": "no-mcp"})).await.unwrap();
    let worktree_cwd = PathBuf::from(worktree["cwd"].as_str().unwrap());
    // Reopening an open folder.
    let reopened = call(&app, &owner, "workspace_create", json!({"path": project})).await.unwrap();
    assert_eq!(reopened["created"], false, "{reopened}");
    // Codemux's own terminal plugin lives in its data dir, never in a folder.
    let plugin = crate::claude_plugin::local_plugin_dir().expect("the terminal plugin is ready").to_path_buf();
    assert!(!plugin.starts_with(&home) && !plugin.starts_with(&repo), "{plugin:?}");

    for checked in [&home, &repo, &worktree_cwd, &root.path().to_path_buf()] {
        assert_eq!(mcp_json_files(checked), Vec::<PathBuf>::new(), "after create/open, below {checked:?}");
    }

    for opened in [&created, &at_home, &worktree] {
        call(&app, &owner, "workspace_close", json!({"workspace_id": opened["workspace_id"]})).await.unwrap();
    }
    // The startup cleanup finds nothing to do on a clean machine.
    let report = crate::mcp_cleanup::run(&home, dirs::config_dir().as_deref(), [project.clone(), worktree_cwd.clone(), repo.clone()]);
    assert_eq!(report, crate::mcp_cleanup::Report::default());

    for checked in [&home, &repo, &worktree_cwd] {
        assert_eq!(mcp_json_files(checked), Vec::<PathBuf>::new(), "after close and cleanup, below {checked:?}");
    }
    for checkout in [&project, &repo] {
        let exclude = std::fs::read_to_string(checkout.join(".git/info/exclude")).unwrap_or_default();
        assert!(!exclude.contains(".mcp.json"), "{checkout:?}: {exclude}");
    }
}

/// A folder the startup cleanup never saw: an older build wrote a `codemux`
/// entry into its parent. Opening it removes that entry before its terminal
/// starts, so a Claude typed there never loads it. The shell's own startup
/// file records what it found.
/// Unix only: the probe uses Bash startup files and a redirected HOME.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_a_folder_cleans_an_old_entry_above_it_before_its_terminal_starts() {
    if isolated(&format!("{MODULE}opening_a_folder_cleans_an_old_entry_above_it_before_its_terminal_starts")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, _root, _workspace, _repo) = git_fixture().await;
    let owner = named(ControlAccess::FullAccess);
    let home = dirs::home_dir().unwrap();
    assert!(home.starts_with(crate::test_isolation::isolated_root().unwrap()), "an isolated HOME");
    let old = home.join("Projects").join("old");
    let repo = old.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let entry = json!({"mcpServers": {"codemux": {"command": "/old/target/debug/codemux", "args": ["mcp"], "env": {"CODEMUX_MCP_PROVIDER_ORIGIN": "1"}}}});
    std::fs::write(old.join(".mcp.json"), serde_json::to_vec(&entry).unwrap()).unwrap();
    let seen = home.join("seen-at-shell-start");
    let probe = format!("[ -e '{}' ] && echo present > '{}' || echo absent > '{}'\n", old.join(".mcp.json").display(), seen.display(), seen.display());
    for rc in [".bashrc", ".bash_profile"] {
        std::fs::write(home.join(rc), &probe).unwrap();
    }

    let created = call(&app, &owner, "workspace_create", json!({"path": repo})).await.unwrap();
    assert_eq!(created["created"], true, "{created}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !seen.exists() {
        assert!(std::time::Instant::now() < deadline, "the workspace's shell never started");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(std::fs::read_to_string(&seen).unwrap().trim(), "absent", "cleaned before the shell started");
    assert!(!old.join(".mcp.json").exists());
    call(&app, &owner, "workspace_close", json!({"workspace_id": created["workspace_id"]})).await.unwrap();
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

fn branch_exists(repo: &Path, branch: &str) -> bool {
    !git(repo, &["branch", "--list", branch]).is_empty()
}

/// A new worktree on `branch` whose worker committed once and never pushed:
/// no remote, no upstream. Returns its workspace, checkout and branch tip.
async fn committed_worktree(app: &App, owner: &ControlCaller, repo: &Path, branch: &str) -> (String, PathBuf, String) {
    let created = call(app, owner, "worktree_create", json!({"repo_path": repo, "branch": branch, "layout": "empty"})).await.unwrap();
    let cwd = PathBuf::from(created["cwd"].as_str().unwrap());
    git(&cwd, &["commit", "--allow-empty", "-m", "only copy"]);
    let upstream = std::process::Command::new("git").current_dir(&cwd).args(["rev-parse", "--abbrev-ref", "@{upstream}"]).output().unwrap();
    assert!(!upstream.status.success(), "the premise: {branch} has no upstream");
    (created["workspace_id"].as_str().unwrap().to_string(), cwd, git(repo, &["rev-parse", branch]))
}

/// A worker's branch that holds commits nothing else keeps (never pushed,
/// no upstream, not merged) is the only reference to that work. A close
/// that is not forced refuses to delete it before any teardown, and a
/// connector client can never force. Once another ref keeps the commits,
/// the branch may go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_close_keeps_a_branch_holding_the_only_copy_of_its_commits() {
    if isolated(&format!("{MODULE}workspace_close_keeps_a_branch_holding_the_only_copy_of_its_commits")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let owner = named(ControlAccess::FullAccess);
    let close = |id: &str, key: &str| json!({"workspace_id": id, "delete_worktree": true, "delete_branch": true, "client_request_id": key});
    let full = connector(ControlAccess::FullAccess);
    let (id, cwd, tip) = committed_worktree(&app, &owner, &repo, "worker").await;

    for (caller, key) in [(&full, "connector-close"), (&owner, "local-close")] {
        let error = refused(call(&app, caller, "workspace_close", close(&id, key)).await, ErrorCode::WorkspaceCloseRefused);
        assert!(error.message.contains("'worker'") && error.message.contains("1 commit"), "{key}: {error}");
        assert!(app.state::<AppStateStore>().find_workspace(&id).is_some(), "{key}: refused before any teardown");
        assert!(cwd.join(".git").exists(), "{key}: the checkout stays");
        assert_eq!(git(&repo, &["rev-parse", "worker"]), tip, "{key}: the branch keeps the commit");
    }

    // The default branch now holds the commit, so deleting the branch loses
    // nothing.
    git(&repo, &["merge", "--ff-only", "worker"]);
    let closed = call(&app, &full, "workspace_close", close(&id, "connector-merged")).await.unwrap();
    assert_eq!((closed["closed"].clone(), closed["worktree_removed"].clone()), (json!(true), json!(true)), "{closed}");
    assert!(!cwd.exists());
    assert!(!branch_exists(&repo, "worker"), "a branch whose commits main holds is deleted");

    // Forcing stays the local escalation that discards the work.
    let (forced, forced_cwd, _) = committed_worktree(&app, &owner, &repo, "forced").await;
    let closed = call(&app, &owner, "workspace_close", json!({"workspace_id": forced, "delete_worktree": true, "delete_branch": true, "force_delete": true})).await.unwrap();
    assert_eq!(closed["worktree_removed"], true, "{closed}");
    assert!(!forced_cwd.exists());
    assert!(!branch_exists(&repo, "forced"), "force_delete deletes it as before");
}

/// A worker that committed on a detached HEAD (`git checkout --detach`, or a
/// rebase stopped at an edit) holds commits no branch has, so removing the
/// checkout would lose them. A close that is not forced refuses before any
/// teardown, with or without `delete_branch`. Once a branch holds them, the
/// checkout may go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_close_keeps_commits_only_a_detached_head_holds() {
    if isolated(&format!("{MODULE}workspace_close_keeps_commits_only_a_detached_head_holds")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let owner = named(ControlAccess::FullAccess);
    let full = connector(ControlAccess::FullAccess);
    let created = call(&app, &owner, "worktree_create", json!({"repo_path": repo, "branch": "detached", "layout": "empty"})).await.unwrap();
    let id = created["workspace_id"].as_str().unwrap().to_string();
    let cwd = PathBuf::from(created["cwd"].as_str().unwrap());
    git(&cwd, &["checkout", "--detach"]);
    git(&cwd, &["commit", "--allow-empty", "-m", "only on HEAD"]);
    let only = git(&cwd, &["rev-parse", "HEAD"]);
    assert_eq!(git(&repo, &["for-each-ref", "--contains", &only]), "", "the premise: no ref holds it");

    for (caller, key, delete_branch) in [(&full, "connector", false), (&full, "connector-branch", true), (&owner, "local", false)] {
        let close = json!({"workspace_id": id, "delete_worktree": true, "delete_branch": delete_branch, "client_request_id": key});
        let error = refused(call(&app, caller, "workspace_close", close).await, ErrorCode::WorkspaceCloseRefused);
        assert!(error.message.contains("detached HEAD") && error.message.contains("1 commit"), "{key}: {error}");
        assert!(app.state::<AppStateStore>().find_workspace(&id).is_some(), "{key}: refused before any teardown");
        assert_eq!(git(&cwd, &["rev-parse", "HEAD"]), only, "{key}: the checkout keeps the commit");
    }

    // A branch holds the commit now, so removing the checkout loses nothing.
    git(&repo, &["branch", "saved", &only]);
    let closed = call(&app, &full, "workspace_close", json!({"workspace_id": id, "delete_worktree": true, "client_request_id": "connector-saved"})).await.unwrap();
    assert_eq!(closed["worktree_removed"], true, "{closed}");
    assert!(!cwd.exists());
    assert_eq!(git(&repo, &["rev-parse", "saved"]), only);
}

/// A new worktree detached at the default branch's tip: its HEAD holds no
/// commit of its own, so a close's pre-flight passes. Returns its workspace
/// and checkout.
async fn detached_worktree(app: &App, owner: &ControlCaller, repo: &Path, branch: &str) -> (String, PathBuf) {
    let created = call(app, owner, "worktree_create", json!({"repo_path": repo, "branch": branch, "layout": "empty"})).await.unwrap();
    let cwd = PathBuf::from(created["cwd"].as_str().unwrap());
    git(&cwd, &["checkout", "--detach"]);
    (created["workspace_id"].as_str().unwrap().to_string(), cwd)
}

/// The close's teardown runs in the checkout after its pre-flight, and can
/// commit, as a checkpoint script does. A commit it made on a detached HEAD
/// keeps the checkout of a close that is not forced: the close stands and
/// says why the worktree stayed. A branch asked to go that gained a commit
/// only it holds stays the same way, and the close says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_close_keeps_commits_its_teardown_made() {
    if isolated(&format!("{MODULE}workspace_close_keeps_commits_its_teardown_made")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    // Every checkout of the repository commits on teardown.
    write_scripts(&repo, json!({"teardown": ["git -c commit.gpgsign=false commit --allow-empty -m checkpoint"]}));
    git(&repo, &["add", ".codemux"]);
    git(&repo, &["commit", "-m", "teardown checkpoint"]);
    let owner = named(ControlAccess::FullAccess);
    let full = connector(ControlAccess::FullAccess);

    let (id, cwd) = detached_worktree(&app, &owner, &repo, "detached-teardown").await;
    let close = json!({"workspace_id": id, "delete_worktree": true, "client_request_id": "detached-teardown"});
    let closed = call(&app, &full, "workspace_close", close).await.unwrap();
    assert_eq!((closed["closed"].clone(), closed["worktree_removed"].clone()), (json!(true), json!(false)), "{closed}");
    assert!(closed["kept_because"].as_str().is_some_and(|why| why.contains("detached HEAD has 1 commit")), "{closed}");
    assert!(app.state::<AppStateStore>().find_workspace(&id).is_none(), "the close stands");
    assert_eq!(git(&cwd, &["log", "-1", "--format=%s"]), "checkpoint", "the checkout keeps the commit");
    let only = git(&cwd, &["rev-parse", "HEAD"]);
    assert_eq!(git(&repo, &["for-each-ref", "--contains", &only]), "", "the premise: no ref holds it");

    // On its branch, asked to delete it: the checkout goes, the branch stays.
    let created = call(&app, &owner, "worktree_create", json!({"repo_path": repo, "branch": "branch-teardown", "layout": "empty"})).await.unwrap();
    let (id, cwd) = (created["workspace_id"].as_str().unwrap(), PathBuf::from(created["cwd"].as_str().unwrap()));
    let close = json!({"workspace_id": id, "delete_worktree": true, "delete_branch": true, "client_request_id": "branch-teardown"});
    let closed = call(&app, &full, "workspace_close", close).await.unwrap();
    assert_eq!((closed["closed"].clone(), closed["worktree_removed"].clone()), (json!(true), json!(true)), "{closed}");
    assert!(closed["kept_because"].as_str().is_some_and(|why| why.contains("'branch-teardown' has 1 commit")), "{closed}");
    assert!(!cwd.exists());
    assert_eq!(git(&repo, &["log", "-1", "--format=%s", "branch-teardown"]), "checkpoint", "the branch keeps the commit");
}

/// A worker still running in the checkout can commit after the close's
/// pre-flight, until the close stops it. A commit it made on a detached HEAD
/// keeps the checkout too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_close_keeps_a_detached_commit_made_while_it_closed() {
    if isolated(&format!("{MODULE}workspace_close_keeps_a_detached_commit_made_while_it_closed")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let owner = named(ControlAccess::FullAccess);
    let full = connector(ControlAccess::FullAccess);
    let (id, cwd) = detached_worktree(&app, &owner, &repo, "detached-worker").await;

    // Paused once the workspace left app state, before its agents stop.
    let (reached, release) = crate::commands::workspace::close_pause::pause(&id);
    let args = json!({"workspace_id": id, "delete_worktree": true, "client_request_id": "detached-worker"});
    let close = spawn_call(&app, &full, "workspace_close", &args);
    tokio::time::timeout(Duration::from_secs(10), reached.notified()).await.expect("the close passes its pre-flight");
    git(&cwd, &["commit", "--allow-empty", "-m", "late work"]);
    release.notify_one();

    let closed = close.await.unwrap().unwrap();
    assert_eq!((closed["closed"].clone(), closed["worktree_removed"].clone()), (json!(true), json!(false)), "{closed}");
    assert!(closed["kept_because"].as_str().is_some_and(|why| why.contains("detached HEAD has 1 commit")), "{closed}");
    assert!(app.state::<AppStateStore>().find_workspace(&id).is_none(), "the close stands");
    assert_eq!(git(&cwd, &["log", "-1", "--format=%s"]), "late work", "the checkout keeps the commit");
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
    assert!(description.contains("Presets are available only on this computer; without one, initial_prompt is ignored"), "{description}");
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
            commits: crate::workspace_service::CommitLoss::Allow,
        })
        .await
        .unwrap_err();

    assert_eq!(error, "grant_revoked: withdrawn");
    let stops: Vec<_> = hermes.calls.snapshot().into_iter().filter(|call| matches!(call, MockCall::StopSession(_))).collect();
    assert!(stops.is_empty(), "{stops:?}");
    assert!(state.find_workspace(&closing).is_some(), "the workspace stays as it was");
    assert!(checkout.join(".git").exists());
}

// ── setup and teardown commands after a revocation ────────────────────────
//
// Setup runs in the background once a create returned; teardown runs while a
// close waits. Each command is repository content and can take long, so none
// may start once the requester's grant is gone.

/// A command that marks `started` in its folder, then waits for `release`
/// there (at most 10 s).
fn held_command(started: &str) -> String {
    format!("touch {started}; i=0; while [ ! -e release ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done")
}

fn write_scripts(folder: &Path, scripts: Value) {
    std::fs::create_dir_all(folder.join(".codemux")).unwrap();
    std::fs::write(folder.join(".codemux/config.json"), scripts.to_string()).unwrap();
}

async fn appears(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} appears", path.display()));
}

/// How each setup ends, as the desktop is told: (event, payload).
fn setup_outcomes(app: &App) -> tokio::sync::mpsc::UnboundedReceiver<(&'static str, Value)> {
    use tauri::Listener;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    for name in ["workspace-setup-failed", "workspace-setup-complete"] {
        let sender = sender.clone();
        app.listen_any(name, move |event| {
            let _ = sender.send((name, serde_json::from_str(event.payload()).unwrap_or_default()));
        });
    }
    receiver
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_setup_revoked_during_a_command_starts_no_further_command() {
    if isolated(&format!("{MODULE}worktree_setup_revoked_during_a_command_starts_no_further_command")) {
        return;
    }
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    // The new checkout falls back to the repository's own scripts.
    write_scripts(&repo, json!({"setup": [held_command("setup-started"), "touch setup-second"]}));
    let mut outcomes = setup_outcomes(&app);
    let caller = connector_caller(&app, "setup-client", "setup-grant", ControlAccess::FullAccess).await;
    let args = json!({"repo_path": repo, "branch": "setup-revoked", "layout": "empty", "client_request_id": "setup-revoked"});

    // The create returns while its setup still runs.
    let created = call(&app, &caller, "worktree_create", args).await.unwrap();
    assert_eq!(created["state"], "succeeded", "{created}");
    let cwd = PathBuf::from(created["cwd"].as_str().unwrap());
    appears(&cwd.join("setup-started")).await;
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "setup-client".into()).unwrap();
    std::fs::write(cwd.join("release"), "").unwrap();

    let (event, payload) = tokio::time::timeout(Duration::from_secs(15), outcomes.recv()).await.expect("the setup ends").unwrap();
    assert_eq!(event, "workspace-setup-failed", "{payload}");
    assert_eq!(payload["workspace_id"], created["workspace_id"]);
    assert_eq!(payload["command"], "touch setup-second", "the first command it did not start");
    assert!(payload["stderr"].as_str().unwrap().starts_with("grant_revoked: "), "{payload}");
    assert!(!cwd.join("setup-second").exists(), "no command starts once the grant is gone");
    assert!(app.state::<AppStateStore>().find_workspace(created["workspace_id"].as_str().unwrap()).is_some());
    connector_off(&app);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_close_revoked_during_teardown_leaves_the_workspace_open() {
    if isolated(&format!("{MODULE}workspace_close_revoked_during_teardown_leaves_the_workspace_open")) {
        return;
    }
    let (app, provider, _root, _workspace, repo) = git_fixture().await;
    let parent = tempfile::tempdir().unwrap();
    let checkout = parent.path().join("teardown-close");
    git(&repo, &["worktree", "add", "-b", "teardown-close", checkout.to_str().unwrap()]);
    write_scripts(&checkout, json!({"teardown": [held_command("teardown-started"), "touch teardown-second"]}));
    let state = app.state::<AppStateStore>();
    let closing = state.create_empty_workspace_at_path(checkout.clone()).0;
    state.set_workspace_worktree(&closing, checkout.display().to_string(), "teardown-close".into());
    // A chat in the workspace, which closing it would stop.
    let chat = call(&app, &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id": closing, "client_request_id": "teardown-chat", "provider": "claude", "permission_mode": "default",
    })).await.unwrap();
    assert_eq!(settle_operation(&app, &chat).await["state"], "succeeded");
    let caller = connector_caller(&app, "teardown-client", "teardown-grant", ControlAccess::FullAccess).await;
    let args = json!({"workspace_id": closing, "client_request_id": "teardown-close"});

    let close = spawn_call(&app, &caller, "workspace_close", &args);
    let running = admitted(&app, &caller, "workspace_close", &args).await;
    appears(&checkout.join("teardown-started")).await;
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "teardown-client".into()).unwrap();
    std::fs::write(checkout.join("release"), "").unwrap();

    refused(close.await.unwrap(), ErrorCode::GrantRevoked);
    failed_as_revoked(&settled(&app, &caller, &running["operation_id"]).await);
    assert!(state.find_workspace(&closing).is_some(), "the workspace stays open");
    assert!(!checkout.join("teardown-second").exists(), "no teardown command starts once the grant is gone");
    let stops: Vec<_> = provider.calls.snapshot().into_iter().filter(|call| matches!(call, MockCall::StopSession(_))).collect();
    assert!(stops.is_empty(), "its chat keeps running: {stops:?}");
    connector_off(&app);
}

/// A new workspace's setup outlives the call: a paired device's connection
/// and a chat's turn end normally before it does, and its background check
/// lets it run on. Revoking the device, or the person stopping the chat,
/// stops it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_checkpoint_outlives_a_connection_and_a_turn_but_not_revocation_or_stop() {
    let (app, _provider, _root, _workspace, thread, authority, actor) = native_caller_fixture("default").await;
    app.manage(crate::web_remote::WebRemoteState::default());
    web_remote_on(&app);
    let shared = app.state::<crate::web_remote::WebRemoteState>().shared();
    let connections = &shared.connections;
    app.state::<DatabaseStore>().web_remote_insert_session("paired", None, None, "paired-hash", true).unwrap();
    let connection = connections.connect_for_test("paired");
    let device = super::workspace_tools::checkpoint(app.handle(), &ControlCaller::remote("paired".into()));
    device.check().unwrap();
    device.background().check().unwrap();
    connections.disconnect_for_test(connection);
    assert!(device.check().unwrap_err().starts_with("stale_caller: "));
    device.background().check().unwrap();
    crate::web_remote::web_remote_revoke_session(app.handle().clone(), "paired".into()).unwrap();
    let refusal = device.background().check().unwrap_err();
    assert!(refusal.starts_with("stale_caller: "), "{refusal}");

    let caller = super::native::qualify(app.handle(), &actor).await.unwrap();
    let chat = super::workspace_tools::checkpoint(app.handle(), &caller);
    chat.check().unwrap();
    chat.background().check().unwrap();
    authority.finish("native-run");
    assert!(chat.check().unwrap_err().starts_with("stale_caller: "));
    chat.background().check().unwrap();
    app.state::<DatabaseStore>().stop_message_deliveries(&thread).unwrap();
    let refusal = chat.background().check().unwrap_err();
    assert!(refusal.starts_with("stale_caller: "), "{refusal}");
    authority.revoke();
}

/// Turning Remote Access off withdraws every paired device at once, as
/// switching the network door off withdraws every grant: setup a device's
/// call left running stops, while the device itself stays paired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_checkpoint_stops_once_remote_access_is_turned_off() {
    let (app, _provider, _root, _workspace) = fixture().await;
    app.manage(crate::web_remote::WebRemoteState::default());
    web_remote_on(&app);
    let db = app.state::<DatabaseStore>();
    db.web_remote_insert_session("paired", None, None, "paired-hash", true).unwrap();
    let device = super::workspace_tools::checkpoint(app.handle(), &ControlCaller::remote("paired".into()));
    device.background().check().unwrap();

    crate::web_remote::web_remote_disable(app.handle().clone()).unwrap();
    let refusal = device.background().check().unwrap_err();
    assert!(refusal.starts_with("stale_caller: Remote Access was turned off"), "{refusal}");
    assert!(db.web_remote_get_session("paired").is_some_and(|session| session.approved && !session.revoked));
}

/// An in-app chat's `workspace_launch` returns before the new workspace's
/// setup runs. The end of the chat's turn leaves that setup running; the
/// person stopping the chat while the setup waits to start means no setup
/// command starts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_launch_setup_outlives_the_turn_but_not_a_stop() {
    if isolated(&format!("{MODULE}workspace_launch_setup_outlives_the_turn_but_not_a_stop")) {
        return;
    }
    without_terminal_daemon();
    for stopped in [true, false] {
        let (app, _provider, root, workspace, thread, authority, actor) = native_caller_fixture("default").await;
        app.manage(crate::terminal::PtyState::default());
        app.manage(crate::presets::PresetStoreState::new(&app.state::<DatabaseStore>()));
        let repo = root.path().to_path_buf();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["commit", "--allow-empty", "-m", "first"]);
        // The new checkout falls back to the repository's own scripts.
        write_scripts(&repo, json!({"setup": ["touch setup-first"]}));
        let mut outcomes = setup_outcomes(&app);
        let branch = if stopped { "launch-stopped" } else { "launch-turn-ended" };
        let path = crate::git::conventional_worktree_path(&repo, branch).unwrap();
        let caller = crate::mcp::registry::HostCaller {
            control: Some(super::native::qualify(app.handle(), &actor).await.unwrap()),
            thread_id: thread.clone(),
            provider: ProviderKind::Claude,
            workspace_id: Some(workspace),
            permission_mode: Some("default".into()),
        };
        let host = crate::commands::workspace_launch::WorkspaceLaunchHost::with_install_check(app.handle().clone(), |_| true);
        let args = json!({"title": "Setup after launch", "message": "Work on this self-contained fixture", "branch": branch});
        let result = crate::mcp::registry::HostTools::call(&host, &caller, "mcp__codemux__workspace_launch", &args).unwrap();
        assert_eq!(result["isError"], false, "{result}");

        // The workspace appears once the create made it, just before its
        // setup starts waiting.
        let created = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let found = app.state::<AppStateStore>().snapshot().workspaces.into_iter()
                    .find(|w| w.worktree_path.as_deref().map(Path::new) == Some(path.as_path()));
                if let Some(found) = found {
                    break found.workspace_id.0;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("the launch creates its workspace");
        if stopped {
            // Stop, as the person presses it in the chat.
            crate::commands::agent_chat::agent_chat_interrupt_turn(app.handle().clone(), ProviderKind::Claude, ThreadId(thread.clone()), None)
                .await
                .unwrap();
        } else {
            authority.finish("native-run");
        }

        let (event, payload) = tokio::time::timeout(Duration::from_secs(15), outcomes.recv()).await.expect("the setup ends").unwrap();
        assert_eq!(payload["workspace_id"], created, "{payload}");
        if stopped {
            assert_eq!(event, "workspace-setup-failed", "{payload}");
            assert_eq!(payload["command"], "touch setup-first", "{payload}");
            assert!(payload["stderr"].as_str().unwrap().starts_with("stale_caller: "), "{payload}");
            assert!(!path.join("setup-first").exists(), "no setup command starts once the chat is stopped");
        } else {
            assert_eq!(event, "workspace-setup-complete", "{payload}");
            assert!(path.join("setup-first").exists(), "the end of a turn leaves setup running");
        }
    }
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
/// worker. Revoking the connector grant, or pausing the network door, while
/// the answer waits in delivery stops it there with that refusal's own code,
/// and the person can still answer the question.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answer_waiting_in_delivery_stops_when_the_grant_is_revoked_or_paused() {
    let (app, _, _root, workspace) = fixture().await;
    let recorder = Arc::new(ModeRecorder::new(Arc::new(MockAgentProvider::new(ProviderKind::Claude))));
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_claude(recorder.clone()).await;
    // The connector never changes a live grant's client or access, so the
    // refusal for a grant that no longer matches its caller is checked on its own.
    let thread = launched(&app, "changed-launch", &workspace, "default").await;
    super::tests::connector_caller(&app, "changed-client", "changed-grant", ControlAccess::FullAccess).await;
    let caller = ControlCaller::outside("changed-client".into(), "changed-grant".into(), ControlAccess::Supervised);
    let standing = super::NativeControlGuard { caller, workspace_id: workspace.clone(), thread_id: thread, provider: ProviderKind::Claude, admission: None };
    let refusal = super::service::authority_refusal(app.handle(), &standing).map(|error| error.code);
    assert_eq!(refusal, Some(ErrorCode::GrantChanged));

    for (case, code) in [("revoked", ErrorCode::GrantRevoked), ("paused", ErrorCode::ConnectorDisabled)] {
        let thread = launched(&app, &format!("{case}-launch"), &workspace, "default").await;
        ask(&app, &thread, "t1", "storage");
        let client = format!("{case}-client");
        let bot = super::tests::connector_caller(&app, &client, &format!("{case}-grant"), ControlAccess::FullAccess).await;

        let (waiting, release) = recorder.hold_next_answer();
        let answering = answer_in_background(&app, &bot, answer(&workspace, &thread, &format!("{case}-answer"), "storage"));
        reaches(&waiting, "delivery").await;
        if code == ErrorCode::GrantRevoked {
            crate::mcp_connector::agent_connector_revoke(app.handle().clone(), client).unwrap();
        } else {
            super::tests::connector_off(&app);
        }
        release.notify_one();

        // Codemux refused it, not the worker: answering again would not help.
        let error = answering.await.unwrap().expect_err("the answer must be refused");
        assert_eq!(error.code, code, "{case}: {error}");
        assert!(recorder.overrides.lock().unwrap().is_empty(), "{case}: the answer never reached the worker");
        let (_, resolution) = app.state::<DatabaseStore>().async_question(&thread, "storage").unwrap();
        assert!(matches!(resolution, QuestionResolution::Failed { .. }), "{case}: the person can still answer it: {resolution:?}");
    }
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
    for (case, saved, code) in [("switched", true, ErrorCode::PermissionCeiling), ("live-only", false, ErrorCode::PermissionStateUnconfirmed)] {
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

        let error = answering.await.unwrap().expect_err("the call must be refused");
        assert_eq!(error.code, code, "{case}: {error}");
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

    refused(answering.await.unwrap(), ErrorCode::GrantRevoked);
    assert!(!codex.calls.snapshot().iter().any(|call| matches!(call, MockCall::AnswerQuestion(..))), "the answer never reached the worker");
    super::tests::connector_off(&app);
}

/// An answer's authority is checked before it resumes an exited worker, not
/// only once the worker is up: an answer held before the resume starts no
/// worker once the grant is revoked. A worker that fails to start refused
/// the answer itself, and the caller may answer again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answer_held_before_resume_starts_no_worker_once_the_grant_is_revoked() {
    use crate::agent_provider::AgentProvider;
    let (app, _, _root, workspace) = fixture().await;
    let mut asking = MockAgentProvider::new(ProviderKind::Codex);
    asking.async_questions = true;
    let codex = Arc::new(asking);
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_codex(codex.clone()).await;
    app.state::<NativeControlState>().fixture_capabilities.lock().unwrap()
        .insert(ProviderKind::Codex, crate::agent_provider::codex::capabilities::build_capabilities(vec![]));
    let launch = call(&app, &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id": workspace, "client_request_id": "held-resume-launch", "provider": "codex", "permission_mode": "workspace-write",
    })).await.unwrap();
    let launched = settle_operation(&app, &launch).await;
    assert_eq!(launched["state"], "succeeded", "{launched}");
    let thread = launched["thread_id"].as_str().unwrap().to_string();
    ask(&app, &thread, "t1", "storage");
    codex.stop_session(ThreadId(thread.clone())).await.unwrap();
    app.state::<DatabaseStore>().set_agent_chat_sdk_session_id(&thread, "codex-conversation").unwrap();
    let bot = super::tests::connector_caller(&app, "held-resume-client", "held-resume-grant", ControlAccess::FullAccess).await;
    let starts = || codex.calls.snapshot().iter().filter(|call| matches!(call, MockCall::StartSession(_))).count();
    let resolution = || app.state::<DatabaseStore>().async_question(&thread, "storage").unwrap().1;

    codex.fail_next_start(crate::agent_provider::ProviderError::ValidationError { message: "synthetic start failure".into() });
    let error = refused(call(&app, &bot, "thread_answer", answer(&workspace, &thread, "failed-start", "storage")).await, ErrorCode::QuestionAnswerFailed);
    assert!(error.message.starts_with("The worker did not take the answer"), "{error}");
    let started = starts();

    // Holding the thread's dispatch lock keeps the answer just before the
    // resume, once it has claimed the question. The gate lives only while
    // someone holds it, so keep it until the answer has its own.
    let gate = app.state::<NativeControlState>().thread_gate(&thread);
    let held = gate.dispatch.clone().lock_owned().await;
    let answering = answer_in_background(&app, &bot, answer(&workspace, &thread, "held-answer", "storage"));
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(resolution(), QuestionResolution::Submitting { .. }) && !answering.is_finished() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the answer never claimed the question");
    assert!(matches!(resolution(), QuestionResolution::Submitting { .. }), "the answer waits to resume: {:?}", resolution());
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "held-resume-client".into()).unwrap();
    drop((held, gate));

    refused(answering.await.unwrap(), ErrorCode::GrantRevoked);
    assert_eq!(starts(), started, "no worker started for the revoked caller's answer");
    assert!(!codex.calls.snapshot().iter().any(|call| matches!(call, MockCall::AnswerQuestion(..))), "the answer never reached the worker");
    assert!(matches!(resolution(), QuestionResolution::Failed { .. }), "the person can still answer it: {:?}", resolution());
    super::tests::connector_off(&app);
}

// ── thread lifecycle: open, rename, close, delete, search ────────────────

fn thread_args(workspace: &str, thread: &str, key: &str) -> Value {
    json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": key})
}

fn pane_of(app: &App, thread: &str) -> Option<String> {
    app.state::<AppStateStore>().agent_chat_pane_id_for_thread(thread)
}

fn active_tab(app: &App, workspace: &str) -> String {
    app.state::<AppStateStore>().find_workspace(workspace).unwrap().active_tab_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_open_rebinds_a_closed_thread_without_taking_focus() {
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "open-launch", &workspace, "default").await;
    let pane = pane_of(&app, &thread).unwrap();
    app.state::<AppStateStore>().close_pane(&pane).unwrap();
    // The person looks at another thread's tab meanwhile.
    let watched = launched(&app, "open-watched", &workspace, "default").await;
    let watched_tab = active_tab(&app, &workspace);
    assert!(!watched_tab.is_empty() && pane_of(&app, &watched).is_some());
    let bot = named(ControlAccess::Supervised);

    let send = json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": "open-send", "message": "Continue"});
    let error = refused(call(&app, &bot, "thread_send", send.clone()).await, ErrorCode::ThreadNotBound);
    assert!(error.message.contains("thread_open"), "the refusal names the way back: {error}");

    let (focused, tab) = (active(&app), active_tab(&app, &workspace));
    let starts = || provider.calls.snapshot().iter().filter(|call| matches!(call, MockCall::StartSession(_))).count();
    let started = starts();
    let args = thread_args(&workspace, &thread, "open-1");
    let opened = call(&app, &bot, "thread_open", args.clone()).await.unwrap();
    assert_eq!((opened["state"].clone(), opened["reopened"].clone()), (json!("succeeded"), json!(true)), "{opened}");
    let rebound = pane_of(&app, &thread).expect("the thread is bound again");
    assert_eq!(opened["pane_id"], rebound);
    assert_eq!(opened["workspace_id"], workspace);
    assert_eq!((active(&app), active_tab(&app, &workspace)), (focused, tab), "an open without select moves no focus");
    assert_eq!(starts(), started, "opening starts no provider");

    let retry = call(&app, &bot, "thread_open", args.clone()).await.unwrap();
    assert_eq!((retry["operation_id"].clone(), retry["replayed"].clone()), (opened["operation_id"].clone(), json!(true)));
    let mut changed = args;
    changed["select"] = json!(true);
    refused(call(&app, &bot, "thread_open", changed).await, ErrorCode::RequestKeyConflict);
    let again = call(&app, &bot, "thread_open", thread_args(&workspace, &thread, "open-2")).await.unwrap();
    assert_eq!((again["reopened"].clone(), again["pane_id"].clone()), (json!(false), json!(rebound)), "an open thread is returned as it is");

    let sent = call(&app, &bot, "thread_send", send).await.unwrap();
    assert_eq!(settle_once(&app, &bot, &sent).await["state"], "succeeded", "a reopened thread takes messages");

    // Read-only callers cannot, and a thread above the caller's ceiling stays closed.
    for reader in [named(ControlAccess::ReadOnly), connector(ControlAccess::ReadOnly)] {
        refused(call(&app, &reader, "thread_open", thread_args(&workspace, &thread, "open-3")).await, ErrorCode::AccessDenied);
    }
    let full = launched(&app, "open-full-launch", &workspace, "bypassPermissions").await;
    app.state::<AppStateStore>().close_pane(&pane_of(&app, &full).unwrap()).unwrap();
    for supervised in [bot.clone(), connector(ControlAccess::Supervised)] {
        refused(call(&app, &supervised, "thread_open", thread_args(&workspace, &full, "open-full")).await, ErrorCode::PermissionCeiling);
        no_receipt(&app, &supervised, "open-full", "thread_open");
    }
    assert!(pane_of(&app, &full).is_none());
    let network = call(&app, &connector(ControlAccess::FullAccess), "thread_open", thread_args(&workspace, &full, "open-full")).await.unwrap();
    assert_eq!(network["reopened"], true, "{network}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_rename_and_close_keep_history_and_replay() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "close-launch", &workspace, "default").await;
    let bot = connector(ControlAccess::Supervised);
    let db = app.state::<DatabaseStore>();

    let mut rename = thread_args(&workspace, &thread, "rename-1");
    rename["title"] = json!("Fix the login test");
    refused(call(&app, &named(ControlAccess::ReadOnly), "thread_rename", rename.clone()).await, ErrorCode::AccessDenied);
    let renamed = call(&app, &bot, "thread_rename", rename.clone()).await.unwrap();
    assert_eq!(renamed["title"], "Fix the login test", "{renamed}");
    assert_eq!(db.get_agent_chat_title(&thread).as_deref(), Some("Fix the login test"));
    assert_eq!(call(&app, &bot, "thread_rename", rename.clone()).await.unwrap()["replayed"], true);
    rename["title"] = json!("Another title");
    refused(call(&app, &bot, "thread_rename", rename).await, ErrorCode::RequestKeyConflict);
    let mut blank = thread_args(&workspace, &thread, "rename-2");
    blank["title"] = json!(" padded ");
    refused(call(&app, &bot, "thread_rename", blank).await, ErrorCode::InvalidArguments);

    refused(call(&app, &named(ControlAccess::ReadOnly), "thread_close", thread_args(&workspace, &thread, "close-0")).await, ErrorCode::AccessDenied);
    let args = thread_args(&workspace, &thread, "close-1");
    let closed = call(&app, &bot, "thread_close", args.clone()).await.unwrap();
    assert_eq!((closed["closed"].clone(), closed["state"].clone()), (json!(true), json!("succeeded")), "{closed}");
    assert!(pane_of(&app, &thread).is_none(), "the pane is gone");
    tokio::time::timeout(Duration::from_secs(10), async {
        while provider.has_session(&ThreadId(thread.clone())).await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("closing stops the session");
    assert!(db.get_agent_chat_session(&thread).is_some(), "the thread and its history stay");
    let retry = call(&app, &bot, "thread_close", args).await.unwrap();
    assert_eq!((retry["closed"].clone(), retry["replayed"].clone()), (json!(true), json!(true)));
    let again = call(&app, &bot, "thread_close", thread_args(&workspace, &thread, "close-2")).await.unwrap();
    assert_eq!(again["closed"], false, "a thread without a pane is already closed: {again}");

    let reopened = call(&app, &bot, "thread_open", thread_args(&workspace, &thread, "close-open")).await.unwrap();
    assert_eq!(reopened["reopened"], true, "a closed thread reopens: {reopened}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_delete_needs_full_access_and_stops_the_session_first() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "delete-launch", &workspace, "default").await;
    assert!(provider.has_session(&ThreadId(thread.clone())).await);
    let args = thread_args(&workspace, &thread, "delete-1");
    for caller in [named(ControlAccess::Supervised), connector(ControlAccess::Supervised), named(ControlAccess::ReadOnly)] {
        refused(call(&app, &caller, "thread_delete", args.clone()).await, ErrorCode::AccessDenied);
    }
    assert!(app.state::<DatabaseStore>().get_agent_chat_session(&thread).is_some());

    let owner = connector(ControlAccess::FullAccess);
    let deleted = call(&app, &owner, "thread_delete", args.clone()).await.unwrap();
    assert_eq!((deleted["deleted"].clone(), deleted["state"].clone()), (json!(true), json!("succeeded")), "{deleted}");
    assert!(app.state::<DatabaseStore>().get_agent_chat_session(&thread).is_none());
    assert!(pane_of(&app, &thread).is_none());
    assert!(!provider.has_session(&ThreadId(thread.clone())).await, "the session stopped before the delete");
    let retry = call(&app, &owner, "thread_delete", args).await.unwrap();
    assert_eq!((retry["deleted"].clone(), retry["replayed"].clone()), (json!(true), json!(true)), "a retry replays, not thread_not_found");
    refused(call(&app, &owner, "thread_delete", thread_args(&workspace, &thread, "delete-2")).await, ErrorCode::ThreadNotFound);
}

/// A close succeeds once the session stopped, not when the pane went: a
/// reopen and send right after it are never stopped by the close's own
/// cleanup. A stop the provider refused fails the close.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_close_settles_only_once_the_session_stopped() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "close-wait-launch", &workspace, "default").await;
    let bot = connector(ControlAccess::Supervised);
    let (entered, release) = provider.hold_next_stop();
    let closing = spawn_call(&app, &bot, "thread_close", &thread_args(&workspace, &thread, "close-wait"));
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await.expect("the close stops the session");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!closing.is_finished(), "the close waits for the stop");
    release.notify_one();
    let closed = closing.await.unwrap().unwrap();
    assert_eq!((closed["state"].clone(), closed["closed"].clone()), (json!("succeeded"), json!(true)), "{closed}");
    assert!(!provider.has_session(&ThreadId(thread.clone())).await, "the session stopped before the close succeeded");

    // Right away: reopen. No stop of the close is still on its way to the
    // reopened thread's next session.
    let stops = || provider.calls.snapshot().iter().filter(|call| matches!(call, MockCall::StopSession(_))).count();
    assert_eq!(stops(), 1);
    call(&app, &bot, "thread_open", thread_args(&workspace, &thread, "close-wait-open")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(stops(), 1, "nothing of the close stops the reopened thread");

    provider.fail_next_stop(crate::agent_provider::ProviderError::RpcError { message: "synthetic stop failure".into() });
    let error = refused(call(&app, &bot, "thread_close", thread_args(&workspace, &thread, "close-fails")).await, ErrorCode::NativeControlFailed);
    assert!(error.message.contains("did not stop"), "{error}");
}

/// A delete holds the thread from closing its pane through deleting it: an
/// open that arrives while its session stops binds no pane, and finds the
/// thread gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_delete_lets_no_open_in_while_the_session_stops() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "delete-hold-launch", &workspace, "default").await;
    let owner = connector(ControlAccess::FullAccess);
    let (entered, release) = provider.hold_next_stop();
    let deleting = spawn_call(&app, &owner, "thread_delete", &thread_args(&workspace, &thread, "delete-hold"));
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await.expect("the delete stops the session");
    assert!(pane_of(&app, &thread).is_none(), "the pane closed first");
    let open = thread_args(&workspace, &thread, "delete-hold-open");
    let opening = spawn_call(&app, &owner, "thread_open", &open);
    admitted(&app, &owner, "thread_open", &open).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(pane_of(&app, &thread).is_none(), "no pane binds while the delete runs");
    release.notify_one();

    let deleted = deleting.await.unwrap().unwrap();
    assert_eq!(deleted["deleted"], true, "{deleted}");
    refused(opening.await.unwrap(), ErrorCode::ThreadNotFound);
    assert!(pane_of(&app, &thread).is_none(), "no pane is left bound to a deleted thread");
    assert!(app.state::<DatabaseStore>().get_agent_chat_session(&thread).is_none());
    assert!(!provider.has_session(&ThreadId(thread.clone())).await);
}

/// A check for `open_thread_pane_checked` that, the first time it is asked,
/// reports that the open got there and waits for the test to let it go on.
/// It blocks only its own worker: `block_in_place` hands the rest of the
/// runtime's work to the other.
fn first_check_waits() -> (impl Fn() -> Result<(), String> + Send + 'static, std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (reached, arrived) = std::sync::mpsc::channel();
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let first = Mutex::new(Some((reached, wait)));
    let check = move || {
        if let Some((reached, wait)) = first.lock().unwrap().take() {
            reached.send(()).unwrap();
            tokio::task::block_in_place(|| wait.recv_timeout(Duration::from_secs(10))).expect("the test lets the open go on");
        }
        Ok(())
    };
    (check, arrived, go)
}

async fn arrives(arrived: std::sync::mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || arrived.recv_timeout(Duration::from_secs(10)))
        .await
        .unwrap()
        .expect("the open reaches its check");
}

/// An open that found the thread's pane before a delete closed it binds no
/// new pane while the delete still runs: it takes the lock the delete holds
/// and then finds the thread gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_open_that_saw_the_pane_before_a_delete_binds_none_after_it() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "delete-race-launch", &workspace, "default").await;
    assert!(pane_of(&app, &thread).is_some());
    let (check, arrived, go) = first_check_waits();
    let opening = {
        let (handle, thread) = (app.handle().clone(), thread.clone());
        tokio::spawn(async move { crate::commands::agent_chat::open_thread_pane_checked(&handle, &thread, false, check).await.map(|opened| opened.pane_id) })
    };
    arrives(arrived).await;

    let (entered, release) = provider.hold_next_stop();
    let deleting = spawn_call(&app, &connector(ControlAccess::FullAccess), "thread_delete", &thread_args(&workspace, &thread, "delete-race"));
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await.expect("the delete stops the session");
    assert!(pane_of(&app, &thread).is_none(), "the delete closed the pane");
    go.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(pane_of(&app, &thread).is_none(), "no pane binds while the delete runs");
    release.notify_one();

    assert_eq!(deleting.await.unwrap().unwrap()["deleted"], true);
    let error = opening.await.unwrap().expect_err("the thread is gone");
    assert!(error.starts_with("conversation_not_found"), "{error}");
    assert!(pane_of(&app, &thread).is_none(), "no pane is left bound to a deleted thread");
    assert!(!provider.has_session(&ThreadId(thread.clone())).await);
}

/// An open binding a new pane keeps the thread's gate, not only its lock: a
/// delete that arrives meanwhile finds that gate and waits, rather than a
/// fresh one whose lock is free.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_delete_waits_for_an_open_binding_a_pane() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "bind-race-launch", &workspace, "default").await;
    app.state::<AppStateStore>().close_pane(&pane_of(&app, &thread).unwrap()).unwrap();
    let (check, arrived, go) = first_check_waits();
    let opening = {
        let (handle, thread) = (app.handle().clone(), thread.clone());
        tokio::spawn(async move { crate::commands::agent_chat::open_thread_pane_checked(&handle, &thread, false, check).await.map(|opened| opened.pane_id) })
    };
    arrives(arrived).await;

    let deleting = spawn_call(&app, &connector(ControlAccess::FullAccess), "thread_delete", &thread_args(&workspace, &thread, "bind-race"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!deleting.is_finished(), "the delete waits for the open's bind");
    assert!(app.state::<DatabaseStore>().get_agent_chat_session(&thread).is_some());
    go.send(()).unwrap();

    opening.await.unwrap().expect("the open binds before the delete");
    assert_eq!(deleting.await.unwrap().unwrap()["deleted"], true);
    assert!(pane_of(&app, &thread).is_none(), "the delete closed the pane the open bound");
    assert!(app.state::<DatabaseStore>().get_agent_chat_session(&thread).is_none());
}

/// A close that finds the pane already gone still settles only once no
/// worker runs: it joins a stop still pending, and stops a worker an earlier
/// close failed to stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_close_without_a_pane_settles_once_the_session_stopped() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, root, workspace) = fixture().await;
    let thread = launched(&app, "close-race-launch", &workspace, "default").await;
    let bot = connector(ControlAccess::Supervised);

    // The first close's stop cannot start while the thread's activity lock is held.
    let activity = crate::commands::agent_chat::delegation_activity_lock(&thread).lock_owned().await;
    let first = spawn_call(&app, &bot, "thread_close", &thread_args(&workspace, &thread, "close-race-1"));
    tokio::time::timeout(Duration::from_secs(10), async {
        while pane_of(&app, &thread).is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the first close closes the pane");
    let second = spawn_call(&app, &bot, "thread_close", &thread_args(&workspace, &thread, "close-race-2"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!second.is_finished(), "a close without a pane waits for the pending stop");
    assert!(provider.has_session(&ThreadId(thread.clone())).await);
    drop(activity);
    assert_eq!(first.await.unwrap().unwrap()["closed"], true);
    assert_eq!(second.await.unwrap().unwrap()["closed"], false);
    assert!(!provider.has_session(&ThreadId(thread.clone())).await, "the session stopped before either close succeeded");

    // A close whose stop failed leaves the worker running and no pane; the
    // next close stops it.
    let thread = launched(&app, "close-fail-launch", &workspace, "default").await;
    assert!(provider.has_session(&ThreadId(thread.clone())).await);
    provider.fail_next_stop(crate::agent_provider::ProviderError::RpcError { message: "synthetic stop failure".into() });
    refused(call(&app, &bot, "thread_close", thread_args(&workspace, &thread, "close-race-fails")).await, ErrorCode::NativeControlFailed);
    assert!(pane_of(&app, &thread).is_none() && provider.has_session(&ThreadId(thread.clone())).await, "the worker survived its failed stop");
    let checkout = PathBuf::from(app.state::<DatabaseStore>().get_agent_chat_session(&thread).unwrap().cwd.unwrap());
    assert!(checkout.starts_with(root.path()), "{checkout:?}");
    assert!(crate::checkout_claims::refuse_failed_in(&checkout).is_err(), "its files stay while the worker may run");
    let retried = call(&app, &bot, "thread_close", thread_args(&workspace, &thread, "close-race-3")).await.unwrap();
    assert_eq!((retried["state"].clone(), retried["closed"].clone()), (json!("succeeded"), json!(false)), "{retried}");
    assert!(!provider.has_session(&ThreadId(thread.clone())).await, "the surviving worker stopped before the close succeeded");
    assert_eq!(crate::checkout_claims::refuse_in(&checkout, None), Ok(()), "once it stopped, its checkout may go");
}

/// A delete after a close whose stop failed stops the surviving worker and
/// frees its checkout, as a retried close does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_delete_after_a_failed_stop_frees_the_checkout() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, root, workspace) = fixture().await;
    let thread = launched(&app, "delete-fail-launch", &workspace, "default").await;
    provider.fail_next_stop(crate::agent_provider::ProviderError::RpcError { message: "synthetic stop failure".into() });
    refused(
        call(&app, &connector(ControlAccess::Supervised), "thread_close", thread_args(&workspace, &thread, "delete-fail-close")).await,
        ErrorCode::NativeControlFailed,
    );
    assert!(pane_of(&app, &thread).is_none() && provider.has_session(&ThreadId(thread.clone())).await, "the worker survived its failed stop");
    let checkout = PathBuf::from(app.state::<DatabaseStore>().get_agent_chat_session(&thread).unwrap().cwd.unwrap());
    assert!(checkout.starts_with(root.path()), "{checkout:?}");
    assert!(crate::checkout_claims::refuse_failed_in(&checkout).is_err(), "its files stay while the worker may run");
    let deleted = call(&app, &connector(ControlAccess::FullAccess), "thread_delete", thread_args(&workspace, &thread, "delete-fail-delete")).await.unwrap();
    assert_eq!(deleted["deleted"], true, "{deleted}");
    assert!(!provider.has_session(&ThreadId(thread.clone())).await, "the delete stopped the surviving worker");
    assert!(app.state::<DatabaseStore>().get_agent_chat_session(&thread).is_none());
    assert_eq!(crate::checkout_claims::refuse_in(&checkout, None), Ok(()), "once it stopped, its checkout may go");
}

/// A close that finds no pane joins an earlier close's stop that has not run
/// yet before it stops the session itself, so that stop cannot run after
/// this close settled and stop a session reopened since.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_close_without_a_pane_joins_an_earlier_stop_not_yet_run() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "join-stop-launch", &workspace, "default").await;
    app.state::<AppStateStore>().close_pane(&pane_of(&app, &thread).unwrap()).unwrap();
    // An earlier close's stop, claimed but not yet run.
    let (mut earlier, _) = crate::checkout_claims::begin_stops(vec![(thread.clone(), Some(workspace.clone()), None)]);
    let closing = spawn_call(&app, &connector(ControlAccess::Supervised), "thread_close", &thread_args(&workspace, &thread, "join-stop-close"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!closing.is_finished(), "the close waits for the earlier stop");
    assert!(provider.has_session(&ThreadId(thread.clone())).await, "and stops nothing before it");
    earlier.settle(&thread, Ok(()));
    let closed = closing.await.unwrap().unwrap();
    assert_eq!((closed["state"].clone(), closed["closed"].clone()), (json!("succeeded"), json!(false)), "{closed}");
    assert!(!provider.has_session(&ThreadId(thread.clone())).await);
}

/// A close admitted before the thread's mode rose above the caller's ceiling
/// checks again before it cancels the thread's work, so once refused it has
/// cancelled nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_close_refused_after_admission_cancels_nothing() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "cancel-check-launch", &workspace, "default").await;
    let bot = connector(ControlAccess::Supervised);
    let barrier = Arc::new(tokio::sync::Semaphore::new(0));
    *app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap() = Some(barrier.clone());
    let args = thread_args(&workspace, &thread, "cancel-check-close");
    let closing = spawn_call(&app, &bot, "thread_close", &args);
    admitted(&app, &bot, "thread_close", &args).await;
    app.state::<DatabaseStore>().update_agent_chat_session_config(&thread, &crate::database::AgentChatSessionConfig {
        permission_mode: Some(Some("bypassPermissions".into())),
        ..Default::default()
    }).unwrap();
    let gate = app.state::<NativeControlState>().thread_gate(&thread);
    let generation = gate.generation();
    barrier.add_permits(1);
    refused(closing.await.unwrap(), ErrorCode::PermissionCeiling);
    assert_eq!(gate.generation(), generation, "the refused close cancelled no work");
    assert!(pane_of(&app, &thread).is_some(), "the thread stays open");
}

/// An open that waited for the thread's dispatch lock is judged as things are
/// once it holds it: a thread whose mode rose above the caller's ceiling
/// meanwhile stays closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_open_checks_the_mode_again_after_waiting() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "open-wait-launch", &workspace, "default").await;
    app.state::<AppStateStore>().close_pane(&pane_of(&app, &thread).unwrap()).unwrap();
    let bot = connector(ControlAccess::Supervised);
    let gate = app.state::<NativeControlState>().thread_gate(&thread);
    let held = gate.dispatch.clone().lock_owned().await;
    let open = thread_args(&workspace, &thread, "open-wait");
    let opening = spawn_call(&app, &bot, "thread_open", &open);
    admitted(&app, &bot, "thread_open", &open).await;
    app.state::<DatabaseStore>().update_agent_chat_session_config(&thread, &crate::database::AgentChatSessionConfig {
        permission_mode: Some(Some("bypassPermissions".into())),
        ..Default::default()
    }).unwrap();
    drop((held, gate));
    refused(opening.await.unwrap(), ErrorCode::PermissionCeiling);
    assert!(pane_of(&app, &thread).is_none(), "the thread stays closed");
}

/// The lifecycle tools ask what the guarded thread tools ask: a supervised
/// caller cannot rename, close or reopen a live full-access worker whose
/// persisted mode only says supervised.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_tools_see_through_a_persisted_mode_to_a_live_full_access_worker() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "disguise-launch", &workspace, "bypassPermissions").await;
    crate::commands::agent_chat::agent_chat_update_session_config(
        app.handle().clone(), app.state(), thread.clone(),
        crate::database::AgentChatSessionConfig { permission_mode: Some(Some("default".into())), ..Default::default() },
    ).await.unwrap();
    let bot = connector(ControlAccess::Supervised);
    let mut rename = thread_args(&workspace, &thread, "disguise-rename");
    rename["title"] = json!("Renamed");
    refused(call(&app, &bot, "thread_rename", rename).await, ErrorCode::PermissionStateUnconfirmed);
    no_receipt(&app, &bot, "disguise-rename", "thread_rename");
    refused(call(&app, &bot, "thread_close", thread_args(&workspace, &thread, "disguise-close")).await, ErrorCode::PermissionStateUnconfirmed);
    no_receipt(&app, &bot, "disguise-close", "thread_close");
    assert!(pane_of(&app, &thread).is_some() && provider.has_session(&ThreadId(thread.clone())).await, "the worker keeps running");
    // Closed in the app without stopping its worker: still not reopened.
    app.state::<AppStateStore>().close_pane(&pane_of(&app, &thread).unwrap()).unwrap();
    refused(call(&app, &bot, "thread_open", thread_args(&workspace, &thread, "disguise-open")).await, ErrorCode::PermissionStateUnconfirmed);
    assert!(pane_of(&app, &thread).is_none());
    let full = call(&app, &connector(ControlAccess::FullAccess), "thread_open", thread_args(&workspace, &thread, "disguise-open")).await.unwrap();
    assert_eq!(full["reopened"], true, "a full-access caller may: {full}");
}

#[tokio::test]
async fn outside_thread_search_spans_open_workspaces() {
    let (app, provider, root, workspace) = fixture().await;
    let other_folder = root.path().join("other");
    std::fs::create_dir(&other_folder).unwrap();
    let other = app.state::<AppStateStore>().create_empty_workspace_at_path(other_folder).0;
    let db = app.state::<DatabaseStore>();
    for (thread, ws) in [("search-a", &workspace), ("search-b", &other)] {
        db.upsert_agent_chat_session(thread, ws, None, "claude").unwrap();
        db.append_agent_chat_message(thread, &json!({"type": "user_message", "thread_id": thread, "text": "Where is the flaky login test?"}).to_string()).unwrap();
    }
    let reader = named(ControlAccess::ReadOnly);
    let everywhere = call(&app, &reader, "thread_search", json!({"query": "flaky login"})).await.unwrap();
    let mut found: Vec<&str> = everywhere["matches"].as_array().unwrap().iter().map(|hit| hit["thread_id"].as_str().unwrap()).collect();
    found.sort();
    assert_eq!(found, ["search-a", "search-b"], "{everywhere}");
    let scoped = call(&app, &reader, "thread_search", json!({"query": "flaky login", "workspace_id": other, "limit": 5})).await.unwrap();
    assert_eq!(scoped["matches"].as_array().unwrap().len(), 1, "{scoped}");
    assert_eq!(scoped["matches"][0]["workspace_id"], other);
    refused(call(&app, &reader, "thread_search", json!({"query": "x", "workspace_id": "no-such-workspace"})).await, ErrorCode::WorkspaceNotFound);
    refused(call(&app, &reader, "thread_search", json!({"query": "   "})).await, ErrorCode::InvalidArguments);
    assert!(provider.calls.snapshot().is_empty(), "searching starts no provider");
}

// ── thread_respond answers to user-input requests ────────────────────────

fn user_input(app: &App, thread: &str, request: &str) {
    app.state::<NativeControlState>().observe_event(&Event::RequestOpened {
        thread_id: ThreadId(thread.into()), turn_id: TurnId("input-turn".into()),
        request_id: RequestId(request.into()), request_kind: "user-input".into(),
        payload: json!({"questions": [
            {"question": "Which database?", "header": "Database", "multiSelect": false, "allowOther": false,
             "options": [{"label": "staging", "description": "Shared"}, {"label": "prod"}]},
            {"question": "Which checks?", "header": "Checks", "multiSelect": true, "options": [{"label": "lint"}, {"label": "tests"}]},
        ]}),
        tool_use_id: None, subagent_id: None,
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_respond_answers_user_input_requests_for_supervised_callers() {
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, "input-launch", &workspace, "default").await;
    user_input(&app, &thread, "input-request");
    let target = json!({"workspace_id": workspace, "thread_id": thread});
    let status = call(&app, &ControlCaller::trusted(), "thread_status", target.clone()).await.unwrap();
    let approval = &status["pending_approvals"][0];
    assert_eq!(approval["request_kind"], "user-input");
    assert_eq!(approval["questions"][0], json!({"question": "Which database?", "header": "Database", "options": ["staging", "prod"], "multi_select": false, "allow_other": false}));
    assert_eq!(approval["questions"][1]["multi_select"], true);
    let mut wait = target.clone();
    wait["timeout_ms"] = json!(1);
    let waited = call(&app, &ControlCaller::trusted(), "thread_wait", wait).await.unwrap();
    assert_eq!(waited["pending"]["approvals"][0]["questions"], approval["questions"], "thread_wait shows what to answer: {waited}");

    let request = approval["request_id"].clone();
    let respond = |key: &str, answers: Value| json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": key, "request_id": request, "answers": answers});
    let bot = connector(ControlAccess::Supervised);
    for (key, answers) in [
        ("one-answer", json!(["staging"])),
        ("not-an-option", json!(["dev", "lint"])),
        ("two-for-one", json!([["staging", "prod"], "lint"])),
        ("empty-choice", json!(["staging", [" "]])),
    ] {
        refused(call(&app, &bot, "thread_respond", respond(key, answers)).await, ErrorCode::InvalidArguments);
        no_receipt(&app, &bot, key, "thread_respond");
    }
    let mut both = respond("both", json!(["staging", "lint"]));
    both["decision"] = json!("deny");
    refused(call(&app, &bot, "thread_respond", both).await, ErrorCode::InvalidArguments);
    let mut allow = respond("allow", json!(null));
    allow.as_object_mut().unwrap().remove("answers");
    allow["decision"] = json!("allow_once");
    refused(call(&app, &bot, "thread_respond", allow).await, ErrorCode::PermissionCeiling);
    refused(call(&app, &named(ControlAccess::ReadOnly), "thread_respond", respond("reader", json!(["staging", "lint"]))).await, ErrorCode::AccessDenied);

    let args = respond("answer-1", json!(["staging", ["lint", "tests"]]));
    let receipt = call(&app, &bot, "thread_respond", args.clone()).await.unwrap();
    assert_eq!(settle_once(&app, &bot, &receipt).await["state"], "succeeded");
    let decisions = provider.decisions();
    let Some(crate::agent_provider::ApprovalDecision::Allow { updated_input: Some(input), .. }) = decisions.last() else {
        panic!("the answer reaches the worker as an allow with input: {decisions:?}");
    };
    assert_eq!(input["answers"], json!({"Which database?": "staging", "Which checks?": "lint, tests"}), "keyed as the question card keys them");
    assert_eq!(input["questions"][0]["question"], "Which database?");
    assert_eq!(call(&app, &bot, "thread_respond", args).await.unwrap()["operation_id"], receipt["operation_id"]);
    let status = call(&app, &ControlCaller::trusted(), "thread_status", target).await.unwrap();
    assert_eq!(status["pending_approvals"], json!([]), "{status}");

    // Answers are for questions; a tool approval still takes a decision.
    app.state::<NativeControlState>().observe_event(&Event::RequestOpened {
        thread_id: ThreadId(thread.clone()), turn_id: TurnId("input-turn".into()),
        request_id: RequestId("tool-request".into()), request_kind: "tool".into(),
        payload: json!({}), tool_use_id: None, subagent_id: None,
    });
    let status = call(&app, &ControlCaller::trusted(), "thread_status", json!({"workspace_id": workspace, "thread_id": thread})).await.unwrap();
    assert!(status["pending_approvals"][0].get("questions").is_none(), "{status}");
    let tool = json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": "tool-answer",
        "request_id": status["pending_approvals"][0]["request_id"], "answers": ["yes"]});
    refused(call(&app, &named(ControlAccess::FullAccess), "thread_respond", tool).await, ErrorCode::InvalidArguments);
}

// ── workspace archive and projects ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_archive_and_unarchive_replay_and_need_full_access() {
    if isolated(&format!("{MODULE}workspace_archive_and_unarchive_replay_and_need_full_access")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, root, _workspace) = fixture().await;
    app.manage(crate::terminal::PtyState::default());
    let state = app.state::<AppStateStore>();
    let folder = root.path().join("archived");
    std::fs::create_dir(&folder).unwrap();
    let archived = state.create_empty_workspace_at_path(folder.clone()).0;

    for caller in [named(ControlAccess::Supervised), connector(ControlAccess::Supervised)] {
        refused(call(&app, &caller, "workspace_archive", json!({"workspace_id": archived})).await, ErrorCode::AccessDenied);
        refused(call(&app, &caller, "workspace_unarchive", json!({"archive_id": "any"})).await, ErrorCode::AccessDenied);
    }
    let owner = connector(ControlAccess::FullAccess);
    let args = json!({"workspace_id": archived, "client_request_id": "archive-1"});
    let done = call(&app, &owner, "workspace_archive", args.clone()).await.unwrap();
    assert_eq!((done["archived"].clone(), done["workspace_id"].clone()), (json!(true), json!(archived)), "{done}");
    assert!(state.find_workspace(&archived).is_none());
    assert!(folder.is_dir(), "archiving keeps the folder");
    let archive_id = done["archive_id"].clone();
    let retry = call(&app, &owner, "workspace_archive", args).await.unwrap();
    assert_eq!((retry["archive_id"].clone(), retry["replayed"].clone()), (archive_id.clone(), json!(true)));

    let listed = call(&app, &named(ControlAccess::ReadOnly), "workspace_archive_list", json!({})).await.unwrap();
    assert!(listed["archived_workspaces"].as_array().unwrap().iter().any(|entry| entry["archive_id"] == archive_id), "{listed}");

    let focused = active(&app);
    refused(call(&app, &owner, "workspace_unarchive", json!({"archive_id": "no-such-archive"})).await, ErrorCode::ArchiveNotFound);
    let restore = json!({"archive_id": archive_id, "client_request_id": "unarchive-1"});
    let restored = call(&app, &owner, "workspace_unarchive", restore.clone()).await.unwrap();
    assert_eq!(restored["restored"], true, "{restored}");
    let restored_id = restored["workspace_id"].as_str().unwrap().to_string();
    assert_eq!(state.find_workspace(&restored_id).unwrap().cwd, folder.display().to_string());
    assert!(state.find_archived_workspace(archive_id.as_str().unwrap()).is_none(), "the entry is retired");
    assert_eq!(active(&app), focused, "a restore without select takes no focus");
    let retry = call(&app, &owner, "workspace_unarchive", restore).await.unwrap();
    assert_eq!((retry["workspace_id"].clone(), retry["replayed"].clone()), (json!(restored_id), json!(true)), "a retry replays, not archive_not_found");
}

/// Archives a plain folder workspace; returns the folder and its archive id.
async fn archived_folder(app: &App, root: &Path, name: &str) -> (PathBuf, String) {
    let folder = root.join(name);
    std::fs::create_dir(&folder).unwrap();
    let workspace = app.state::<AppStateStore>().create_empty_workspace_at_path(folder.clone()).0;
    let done = call(app, &connector(ControlAccess::FullAccess), "workspace_archive", json!({"workspace_id": workspace})).await.unwrap();
    (folder, done["archive_id"].as_str().unwrap().to_string())
}

fn workspaces_at(app: &App, folder: &Path) -> usize {
    let cwd = folder.display().to_string();
    app.state::<AppStateStore>().snapshot().workspaces.iter().filter(|w| w.cwd == cwd).count()
}

/// Two restores of one entry under different keys, both waiting on the
/// lifecycle lock: one workspace, so one terminal and one setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_restores_of_one_archive_make_one_workspace() {
    if isolated(&format!("{MODULE}simultaneous_restores_of_one_archive_make_one_workspace")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, root, _workspace) = fixture().await;
    app.manage(crate::terminal::PtyState::default());
    let (folder, archive_id) = archived_folder(&app, root.path(), "restored-twice").await;
    let owner = connector(ControlAccess::FullAccess);

    let removal = crate::worktree_cleanup::removal_guard().await;
    let restores: Vec<_> = ["restore-a", "restore-b"].iter().map(|key| {
        let args = json!({"archive_id": archive_id, "client_request_id": key});
        (spawn_call(&app, &owner, "workspace_unarchive", &args), args)
    }).collect();
    for (_, args) in &restores {
        admitted(&app, &owner, "workspace_unarchive", args).await;
    }
    reach_the_lock().await;
    drop(removal);

    let mut restored = vec![];
    for (call, _) in restores {
        if let Ok(done) = call.await.unwrap() {
            restored.push(done["workspace_id"].clone());
        }
    }
    assert_eq!(restored.len(), 1, "one restore makes the workspace: {restored:?}");
    assert_eq!(workspaces_at(&app, &folder), 1, "no second workspace over the folder");
    assert!(app.state::<AppStateStore>().find_archived_workspace(&archive_id).is_none());
}

/// A restore waiting on the lifecycle lock asks the caller's grant again
/// once it holds it: revoked meanwhile, it creates nothing and keeps the
/// entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_unarchive_revoked_while_queued_on_the_lifecycle_lock_restores_nothing() {
    if isolated(&format!("{MODULE}workspace_unarchive_revoked_while_queued_on_the_lifecycle_lock_restores_nothing")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, root, _workspace) = fixture().await;
    app.manage(crate::terminal::PtyState::default());
    let (folder, archive_id) = archived_folder(&app, root.path(), "restored-revoked").await;
    let caller = connector_caller(&app, "queued-restore-client", "queued-restore-grant", ControlAccess::FullAccess).await;
    let args = json!({"archive_id": archive_id, "client_request_id": "queued-restore"});

    let removal = crate::worktree_cleanup::removal_guard().await;
    let queued = spawn_call(&app, &caller, "workspace_unarchive", &args);
    let running = admitted(&app, &caller, "workspace_unarchive", &args).await;
    reach_the_lock().await;
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "queued-restore-client".into()).unwrap();
    drop(removal);

    refused(queued.await.unwrap(), ErrorCode::GrantRevoked);
    failed_as_revoked(&settled(&app, &caller, &running["operation_id"]).await);
    assert_eq!(workspaces_at(&app, &folder), 0, "no workspace, so no terminal and no setup script");
    assert!(app.state::<AppStateStore>().find_archived_workspace(&archive_id).is_some(), "the entry stays restorable");
    connector_off(&app);
}

/// A restore that adopts a worktree in place asks the caller's grant again
/// after its git read: revoked during the read, it creates no workspace, so
/// no terminal or setup starts, and the entry stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_unarchive_revoked_during_the_worktree_git_read_adopts_nothing() {
    if isolated(&format!("{MODULE}workspace_unarchive_revoked_during_the_worktree_git_read_adopts_nothing")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let parent = tempfile::tempdir().unwrap();
    let checkout = parent.path().join("adopted");
    git(&repo, &["worktree", "add", "-b", "adopted", checkout.to_str().unwrap()]);
    let state = app.state::<AppStateStore>();
    let imported = state.create_empty_workspace_at_path(checkout.clone()).0;
    state.set_workspace_worktree(&imported, checkout.display().to_string(), "adopted".into());
    state.set_workspace_project_root(&imported, repo.display().to_string());
    // An archive records the branch the workspace shows.
    crate::commands::workspace::populate_git_info(&state, &imported, &checkout);
    let archived = call(&app, &connector(ControlAccess::FullAccess), "workspace_archive", json!({"workspace_id": imported})).await.unwrap();
    let archive_id = archived["archive_id"].as_str().unwrap().to_string();
    assert_eq!(workspaces_at(&app, &checkout), 0);
    let caller = connector_caller(&app, "adopt-restore-client", "adopt-restore-grant", ControlAccess::FullAccess).await;
    let args = json!({"archive_id": archive_id, "client_request_id": "adopt-restore"});

    let (entered, release) = crate::commands::workspace::open_pause::hold(&checkout);
    let restoring = spawn_call(&app, &caller, "workspace_unarchive", &args);
    let running = admitted(&app, &caller, "workspace_unarchive", &args).await;
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await.expect("the restore reads the worktree's git state");
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "adopt-restore-client".into()).unwrap();
    release.notify_one();

    refused(restoring.await.unwrap(), ErrorCode::GrantRevoked);
    failed_as_revoked(&settled(&app, &caller, &running["operation_id"]).await);
    assert_eq!(workspaces_at(&app, &checkout), 0, "no workspace, so no terminal and no setup script");
    assert!(state.find_archived_workspace(&archive_id).is_some(), "the entry stays restorable");
    connector_off(&app);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_list_and_clone_make_a_known_project() {
    if isolated(&format!("{MODULE}project_list_and_clone_make_a_known_project")) {
        return;
    }
    without_terminal_daemon();
    let (app, _provider, _root, _workspace, repo) = git_fixture().await;
    let reader = connector(ControlAccess::ReadOnly);
    let paths = |listed: &Value| listed["projects"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let listed = call(&app, &reader, "project_list", json!({})).await.unwrap();
    assert!(paths(&listed).contains(&repo.canonicalize().unwrap().display().to_string()), "{listed}");

    let sources = tempfile::tempdir().unwrap();
    let origin = sources.path().join("tool.git");
    git(sources.path(), &["clone", "--bare", &repo.display().to_string(), &origin.display().to_string()]);
    let url = format!("file://{}", origin.display());
    let parent = tempfile::tempdir().unwrap();
    let clone = json!({"url": url, "parent": parent.path(), "client_request_id": "clone-1"});

    refused(call(&app, &named(ControlAccess::Supervised), "project_clone", clone.clone()).await, ErrorCode::AccessDenied);
    let network = connector(ControlAccess::FullAccess);
    refused(call(&app, &network, "project_clone", clone.clone()).await, ErrorCode::InvalidArguments);
    let unknown = json!({"url": "https://example.invalid/team/tool.git", "parent": parent.path(), "client_request_id": "clone-net"});
    refused(call(&app, &network, "project_clone", unknown).await, ErrorCode::UnknownProjectRoot);
    no_receipt(&app, &network, "clone-net", "project_clone");
    let owner = named(ControlAccess::FullAccess);
    for bad in [
        json!({"url": "-uupload-pack=touch /tmp/x", "parent": parent.path()}),
        json!({"url": "ext::sh -c touch% /tmp/x", "parent": parent.path()}),
        json!({"url": url, "parent": parent.path(), "name": ".hidden"}),
        json!({"url": url, "parent": "relative/folder"}),
    ] {
        refused(call(&app, &owner, "project_clone", bad).await, ErrorCode::InvalidArguments);
    }

    let cloned = call(&app, &owner, "project_clone", clone.clone()).await.unwrap();
    assert_eq!((cloned["state"].clone(), cloned["created"].clone()), (json!("succeeded"), json!(true)), "{cloned}");
    let path = parent.path().join("tool");
    assert!(path.join(".git").exists(), "the repository was cloned into parent/<name>");
    assert_eq!(cloned["path"], path.display().to_string());
    let workspace = app.state::<AppStateStore>().find_workspace(cloned["workspace_id"].as_str().unwrap()).expect("it opened as a workspace");
    assert_eq!(workspace.cwd, path.display().to_string());
    assert!(app.state::<DatabaseStore>().get_recent_projects(10).iter().any(|p| p.path == path.display().to_string()), "it is a Recent project");
    let listed = call(&app, &reader, "project_list", json!({})).await.unwrap();
    assert!(paths(&listed).contains(&path.canonicalize().unwrap().display().to_string()), "{listed}");
    let retry = call(&app, &owner, "project_clone", clone).await.unwrap();
    assert_eq!((retry["workspace_id"].clone(), retry["replayed"].clone()), (cloned["workspace_id"].clone(), json!(true)));
    let taken = json!({"url": url, "parent": parent.path(), "client_request_id": "clone-2"});
    refused(call(&app, &owner, "project_clone", taken).await, ErrorCode::InvalidArguments);
}
