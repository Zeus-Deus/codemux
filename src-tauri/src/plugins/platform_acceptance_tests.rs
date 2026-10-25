//! Native launch acceptance: real core receipts, temporary Git checkouts, fake agents.
//! Register as `plugins::platform_acceptance_tests`; every test isolates process-wide paths.

use super::{
    platform::PlatformService,
    platform_authority::PlatformCaller,
    platform_manifest::PlatformManifest,
    platform_store::{InstallationRecord, PlatformGrants},
    platform_workflow,
};
use crate::{
    agent_control::{CallerPrincipal, ControlAccess, ControlCaller, NativeControlState},
    agent_provider::{ProviderKind, StartSessionInput},
    database::{AgentChatSessionConfig, DatabaseStore},
    delegation::authority::NativeAuthority,
    presets::PresetStoreState,
    state::AppStateStore,
    terminal::PtyState,
};
use serde_json::{json, Value};
use std::{path::Path, process::Command, sync::Arc, time::Duration};
use tauri::{Listener, Manager};
use tokio::sync::{mpsc, Notify};

#[allow(dead_code)]
#[path = "../../tests/helpers/mock_agent_provider.rs"]
mod mock_agent_provider;
use mock_agent_provider::{MockAgentProvider, MockCall};

const INSTALLATION: &str = "native-acceptance";
const SOURCE: &str = "acceptance-source";
const MODEL: &str = "fixture-model";

/// Worktree placement and layout emits use test-only path overrides, including on
/// Windows where Known Folders ignore HOME. Set paths only on a new process.
fn isolated(name: &str) -> bool {
    if let Some(root) = std::env::var_os("CODEMUX_PLATFORM_ACCEPTANCE_ROOT") {
        assert_eq!(
            Path::new(&std::env::var_os("CODEMUX_TEST_WORKTREE_HOME").unwrap()),
            Path::new(&root)
        );
        assert!(
            Path::new(&std::env::var_os("CODEMUX_TEST_LAYOUT_PATH").unwrap()).starts_with(&root)
        );
        return false;
    }
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profile");
    std::fs::create_dir(&profile).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .current_dir(root.path())
        .args([
            "--exact",
            &format!("plugins::platform_acceptance_tests::{name}"),
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        // Keep HOME different so Unix runs exercise the explicit override too.
        .env("HOME", &profile)
        .env("USERPROFILE", &profile)
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("TMPDIR", root.path())
        .env("TEMP", root.path())
        .env("TMP", root.path())
        .env("CODEMUX_PLATFORM_ACCEPTANCE_ROOT", root.path())
        .env("CODEMUX_TEST_WORKTREE_HOME", root.path())
        .env("CODEMUX_TEST_LAYOUT_PATH", root.path().join("layout.json"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.path().join("gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Native acceptance")
        .env("GIT_AUTHOR_EMAIL", "acceptance@example.invalid")
        .env("GIT_COMMITTER_NAME", "Native acceptance")
        .env("GIT_COMMITTER_EMAIL", "acceptance@example.invalid");
    if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
        command.env("LD_LIBRARY_PATH", path);
    }
    #[cfg(windows)]
    for key in ["SystemRoot", "WINDIR", "ComSpec", "PATHEXT"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "isolated {name}: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn manifest() -> PlatformManifest {
    PlatformManifest::parse(
        br#"{"id":"test.native","name":"Native acceptance","version":"1.0.0","apiVersion":1,"entry":"plugin.mjs","capabilities":["workflow.query","threads.read","threads.create","worktrees.create"]}"#,
    )
    .unwrap()
}

fn caller(record: &InstallationRecord) -> PlatformCaller {
    PlatformCaller::new(
        INSTALLATION.into(),
        record.generation.clone(),
        ControlCaller::trusted(),
        None,
        None,
    )
    .with_operation_id("reviewed-launch".into())
}

struct Fixture {
    app: tauri::App<tauri::test::MockRuntime>,
    root: tempfile::TempDir,
    provider: Arc<MockAgentProvider>,
    platform: PlatformService,
    record: InstallationRecord,
    workspace: String,
    project: String,
    setup_events: mpsc::UnboundedReceiver<String>,
}

impl Fixture {
    async fn new() -> Self {
        let provider = Arc::new(MockAgentProvider::new(ProviderKind::Claude));
        let (app, root, workspace, _) =
            crate::agent_control::guard_tests::fixture(provider.clone(), SOURCE).await;
        // The shared fixture adds its workspace to AppStateStore's bootstrap
        // terminal workspace. Keep this fixture's open-project set exact.
        let state = app.state::<AppStateStore>();
        for bootstrap in state.snapshot().workspaces {
            if bootstrap.workspace_id.0 != workspace {
                state.close_workspace(&bootstrap.workspace_id.0).unwrap();
            }
        }
        assert_eq!(state.snapshot().workspaces.len(), 1);
        drop(state);
        git(root.path(), &["init", "-b", "main"]);
        std::fs::write(root.path().join("source.txt"), "reviewed base\n").unwrap();
        git(root.path(), &["add", "source.txt"]);
        git(root.path(), &["commit", "-m", "reviewed base"]);
        app.state::<AppStateStore>()
            .set_workspace_project_root(&workspace, root.path().display().to_string());
        let project = platform_workflow::project_id(
            &app.state::<AppStateStore>()
                .find_workspace(&workspace)
                .unwrap(),
        );
        let mut caps = crate::agent_provider::claude::capabilities::claude_fallback_capabilities();
        caps.models = vec![serde_json::from_value(json!({
            "id":MODEL,"label":"Fixture model","effort_levels":["high"],
            "supports_fast_mode":true,
            "context_window_options":[{"value":"fixture-window","label":"Fixture window"}],
            "service_tiers":[{"value":"fast","label":"Fast","description":"Fixture tier"}]
        }))
        .unwrap()];
        app.state::<NativeControlState>()
            .fixture_capabilities
            .lock()
            .unwrap()
            .insert(ProviderKind::Claude, caps);
        app.manage(PtyState::default());
        // Seed SQLite before loading so a Windows Known Folder cannot trigger
        // migration (and deletion) of the user's legacy preset file.
        crate::presets::save_presets(
            &app.state::<DatabaseStore>(),
            &crate::presets::PresetStore {
                schema_version: 2,
                presets: Vec::new(),
                default_preset_id: None,
                bar_visible: true,
            },
        )
        .unwrap();
        app.manage(PresetStoreState::new(&app.state::<DatabaseStore>()));
        let platform = PlatformService::open(root.path().join("platform"));
        platform
            .register(INSTALLATION, "initial", manifest(), "package-one")
            .unwrap();
        let record = platform
            .consent(
                INSTALLATION,
                "package-one",
                PlatformGrants {
                    capabilities: manifest().requested_capabilities(),
                    project_ids: vec![project.clone()],
                },
            )
            .unwrap();
        let (setup_tx, setup_events) = mpsc::unbounded_channel();
        app.listen("worktree-includes-applied", move |event| {
            if let Ok(value) = serde_json::from_str::<Value>(event.payload()) {
                if let Some(id) = value["workspace_id"].as_str() {
                    let _ = setup_tx.send(id.to_string());
                }
            }
        });
        Self {
            app,
            root,
            provider,
            platform,
            record,
            workspace,
            project,
            setup_events,
        }
    }

    fn request(&self, key: &str) -> Value {
        json!({
            "projectId":self.project,"idempotencyKey":key,
            "title":"Reviewed native task","text":"First line\nSecond line",
            "workspaceStrategy":{"type":"root"},"nativePermissionMode":"acceptEdits",
            "modelSelection":{"instanceId":"claude","model":MODEL,"options":[
                {"id":"effort","value":"high"},
                {"id":"context_window","value":"fixture-window"},
                {"id":"fast_mode","value":true},
                {"id":"service_tier","value":"fast"}
            ]}
        })
    }

    async fn create(&self, caller: &PlatformCaller, request: Value) -> Result<Value, String> {
        self.platform
            .dispatch(self.app.handle(), caller, "threads.create", request)
            .await
    }

    async fn milestone(&self, receipt: &Value, notification: &Notify) {
        tokio::select! {
            reached = tokio::time::timeout(Duration::from_secs(15), notification.notified()) => {
                reached.expect("fake provider did not reach its launch milestone");
            }
            terminal = self.settled(receipt) => {
                panic!("core launch ended before the held fake-provider milestone: {terminal}");
            }
        }
    }

    /// Provider ACK gates prove dispatch happened. Yield only to let that same
    /// core task persist its terminal receipt; no elapsed-time correctness assertion.
    async fn settled(&self, receipt: &Value) -> Value {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let status = crate::agent_control::execute(
                    self.app.handle(),
                    &ControlCaller::trusted(),
                    "operation_status",
                    json!({"operation_id":receipt["operation_id"]}),
                )
                .await
                .unwrap();
                if !matches!(status["state"].as_str(), Some("accepted" | "running")) {
                    return status;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("core launch did not persist a terminal receipt")
    }

    async fn setup_finished(&mut self, workspace: &str) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(id) = self.setup_events.recv().await {
                if id == workspace {
                    return;
                }
            }
            panic!("worktree setup event channel closed");
        })
        .await
        .expect("temporary worktree setup did not finish");
    }

    fn policy_caller(&self, mode: &str) -> (PlatformCaller, Arc<NativeAuthority>) {
        self.app
            .state::<DatabaseStore>()
            .update_agent_chat_session_config(
                SOURCE,
                &AgentChatSessionConfig {
                    permission_mode: Some(Some(mode.into())),
                    ..Default::default()
                },
            )
            .unwrap();
        let authority = NativeAuthority::with_policy(SOURCE, ProviderKind::Claude, Some(mode));
        let generation = authority.begin(0);
        assert!(authority.bind(generation, "fixture-turn"));
        let mut captured = caller(&self.record);
        captured.control.identity = CallerPrincipal::Thread {
            thread_id: SOURCE.into(),
            provider_instance_id: "claude".into(),
            provider: ProviderKind::Claude,
            session_id: "fixture-session".into(),
            authority: authority.capture().identity().unwrap(),
            ceiling: crate::agent_control::permissions::Ceiling::for_mode(
                ProviderKind::Claude,
                Some(mode),
            )
            .unwrap(),
        };
        captured.control.principal = format!("native-thread:{SOURCE}");
        (captured, authority)
    }

    fn worktree_path(&self, branch: &str) -> std::path::PathBuf {
        let path = crate::git::conventional_worktree_path(self.root.path(), branch).unwrap();
        let root =
            std::path::PathBuf::from(std::env::var_os("CODEMUX_PLATFORM_ACCEPTANCE_ROOT").unwrap());
        assert!(path.starts_with(root.join(".codemux").join("worktrees")));
        path
    }

    fn starts(&self) -> Vec<StartSessionInput> {
        self.provider.start_inputs()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn platform_native_create_dispatches_exact_selection_and_replays_after_generation_restart() {
    if isolated(
        "platform_native_create_dispatches_exact_selection_and_replays_after_generation_restart",
    ) {
        return;
    }
    let mut f = Fixture::new().await;
    let original = caller(&f.record);
    let request = f.request("root-effect");
    let (starting, release_start) = f.provider.hold_next_start();
    let (accepted, release_ack) = f.provider.hold_next_ack();
    let first = f.create(&original, request.clone()).await.unwrap();
    f.milestone(&first, &starting).await;
    assert_eq!(first["tool"], "thread_launch");
    assert_eq!(first["state"], "accepted");
    assert_eq!(first["workspace_id"], f.workspace);
    assert_eq!(first["operationId"], first["operation_id"]);
    let core = f
        .app
        .state::<DatabaseStore>()
        .control_operation(
            "local",
            first["operation_id"].as_str().unwrap(),
            &f.app.state::<NativeControlState>().epoch,
        )
        .unwrap()
        .unwrap();
    assert_eq!(core.state, "running");
    assert_eq!(core.thread_id.as_deref(), first["threadId"].as_str());

    let updated = f
        .platform
        .register(INSTALLATION, "updated", manifest(), "package-two")
        .unwrap();
    f.record = f
        .platform
        .consent(INSTALLATION, "package-two", updated.grants)
        .unwrap();
    f.platform = PlatformService::open(f.root.path().join("platform"));
    // A stale caller arriving first after restart cannot poison the current
    // generation's watch registry or force a second native admission.
    assert!(f
        .create(&original, request.clone())
        .await
        .unwrap_err()
        .contains("stale_generation"));
    let current = caller(&f.record);
    let replay = f.create(&current, request.clone()).await.unwrap();
    assert_eq!(replay["operation_id"], first["operation_id"]);
    assert_eq!(replay["threadId"], first["threadId"]);
    let mut changed = request;
    changed["modelSelection"]["options"][0]["value"] = json!("changed");
    assert!(f
        .create(&current, changed)
        .await
        .unwrap_err()
        .contains("idempotency_conflict"));
    release_start.notify_one();
    f.milestone(&first, &accepted).await;
    let inputs = f.starts();
    assert_eq!(inputs.len(), 1, "replay must not start a second provider");
    let start = &inputs[0];
    assert_eq!(start.thread_id.0, first["thread_id"]);
    assert_eq!(start.cwd, f.root.path());
    assert_eq!(start.workspace_id.as_deref(), Some(f.workspace.as_str()));
    assert_eq!(start.model.as_deref(), Some(MODEL));
    assert_eq!(start.permission_mode.as_deref(), Some("acceptEdits"));
    assert_eq!(start.effort.as_deref(), Some("high"));
    assert_eq!(start.context_window.as_deref(), Some("fixture-window"));
    assert_eq!(start.service_tier.as_deref(), Some("fast"));
    assert!(start.fast_mode);
    assert_eq!(
        start.extra["orchestration_selection"]["instance_id"],
        "claude"
    );
    assert_eq!(
        f.provider
            .calls
            .snapshot()
            .into_iter()
            .filter_map(|call| match call {
                MockCall::SendTurn(_, text) => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec!["[Plugin: Native acceptance]\nFirst line\nSecond line"]
    );
    release_ack.notify_one();
    let terminal = f.settled(&first).await;
    assert_eq!(terminal["state"], "succeeded", "{terminal}");
    let later = current.with_operation_id("later-status-click".into());
    let status = f
        .platform
        .dispatch(
            f.app.handle(),
            &later,
            "workflow.query",
            json!({"query":"operation","operationId":first["operation_id"]}),
        )
        .await
        .unwrap();
    assert_eq!(status["operationId"], first["operation_id"]);
    assert_eq!(status["state"], "succeeded");
    let saved = f
        .app
        .state::<DatabaseStore>()
        .get_agent_chat_session(first["thread_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(saved.model.as_deref(), Some(MODEL));
    assert_eq!(saved.permission_mode.as_deref(), Some("acceptEdits"));
    assert_eq!(saved.workspace_id, f.workspace);
    assert_eq!(
        f.platform.store.read(INSTALLATION).unwrap().receipts.len(),
        1
    );
    assert_eq!(
        f.app.state::<AppStateStore>().snapshot().workspaces.len(),
        1
    );
    assert_eq!(f.starts().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn platform_native_worktree_uses_local_ref_or_explicitly_fetches_local_origin() {
    if isolated("platform_native_worktree_uses_local_ref_or_explicitly_fetches_local_origin") {
        return;
    }
    let mut f = Fixture::new().await;
    let reviewed = git(f.root.path(), &["rev-parse", "HEAD"]);
    git(f.root.path(), &["branch", "review-base"]);
    std::fs::write(f.root.path().join("source.txt"), "unrelated main tip\n").unwrap();
    git(f.root.path(), &["commit", "-am", "main advanced"]);
    let main_tip = git(f.root.path(), &["rev-parse", "HEAD"]);
    let origin = f.root.path().join("origin.git");
    git(f.root.path(), &["init", "--bare", origin.to_str().unwrap()]);
    git(
        f.root.path(),
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let recomputed = crate::project_identity::project_uid_for(
        crate::project_identity::git_canonical_remote(f.root.path()).as_deref(),
        f.root.path().to_str().unwrap(),
    );
    assert_ne!(
        recomputed, f.project,
        "adding the local origin must exercise the open project's established identity"
    );
    git(f.root.path(), &["push", "origin", "review-base"]);
    let seed = f.root.path().join("origin-writer");
    git(
        f.root.path(),
        &[
            "clone",
            "--branch",
            "review-base",
            origin.to_str().unwrap(),
            seed.to_str().unwrap(),
        ],
    );
    std::fs::write(seed.join("source.txt"), "fresh origin base\n").unwrap();
    git(&seed, &["commit", "-am", "origin advanced"]);
    git(&seed, &["push", "origin", "review-base"]);
    let origin_tip = git(&seed, &["rev-parse", "HEAD"]);
    assert_ne!(origin_tip, reviewed);
    let active_before = f
        .app
        .state::<AppStateStore>()
        .snapshot()
        .active_workspace_id;
    let actor = caller(&f.record);
    let mut created = Vec::new();
    for (branch, from_origin, expected, contents) in [
        ("plugin-local", false, &reviewed, "reviewed base\n"),
        ("plugin-origin", true, &origin_tip, "fresh origin base\n"),
    ] {
        let mut request = f.request(branch);
        request["workspaceStrategy"] = json!({
            "type":"worktree","branch":branch,"baseRef":"review-base",
            "startFromOrigin":from_origin
        });
        let (accepted, release_ack) = f.provider.hold_next_ack();
        let first = f.create(&actor, request.clone()).await.unwrap();
        f.milestone(&first, &accepted).await;
        let starts = f.starts();
        let start = starts.last().unwrap();
        let path = f.worktree_path(branch);
        assert_eq!(start.cwd, path);
        assert_eq!(git(&path, &["rev-parse", "HEAD"]), *expected);
        assert_eq!(
            std::fs::read_to_string(path.join("source.txt")).unwrap(),
            contents
        );
        assert_ne!(first["workspace_id"], f.workspace);
        assert_eq!(
            start.workspace_id.as_deref(),
            first["workspace_id"].as_str()
        );
        assert_eq!(start.model.as_deref(), Some(MODEL));
        assert_eq!(start.permission_mode.as_deref(), Some("acceptEdits"));
        let workspace = first["workspace_id"].as_str().unwrap().to_string();
        let ws = f
            .app
            .state::<AppStateStore>()
            .find_workspace(&workspace)
            .unwrap();
        assert_eq!(ws.worktree_path.as_deref(), path.to_str());
        assert_eq!(ws.project_root.as_deref(), f.root.path().to_str());
        assert_eq!(platform_workflow::project_id(&ws), f.project);
        let replay = f.create(&actor, request.clone()).await.unwrap();
        assert_eq!(replay["operation_id"], first["operation_id"]);
        assert_eq!(f.starts().len(), starts.len());
        let mut changed = request;
        changed["workspaceStrategy"]["baseRef"] = json!("main");
        assert!(f
            .create(&actor, changed)
            .await
            .unwrap_err()
            .contains("idempotency_conflict"));
        release_ack.notify_one();
        let terminal = f.settled(&first).await;
        assert_eq!(terminal["state"], "succeeded", "{terminal}");
        f.setup_finished(&workspace).await;
        assert_eq!(git(f.root.path(), &["rev-parse", "HEAD"]), main_tip);
        assert_eq!(
            f.app
                .state::<AppStateStore>()
                .snapshot()
                .active_workspace_id,
            active_before,
            "background worktree launch must preserve focus"
        );
        if !from_origin {
            assert_eq!(
                git(
                    f.root.path(),
                    &["rev-parse", "refs/remotes/origin/review-base"]
                ),
                reviewed,
                "a local base must not fetch or silently prefer origin"
            );
        }
        created.push(first);
    }
    assert_eq!(
        git(
            f.root.path(),
            &["rev-parse", "refs/remotes/origin/review-base"]
        ),
        origin_tip
    );
    assert_ne!(created[0]["thread_id"], created[1]["thread_id"]);
    assert_eq!(f.starts().len(), 2);
    assert_eq!(
        f.platform.store.read(INSTALLATION).unwrap().receipts.len(),
        2
    );

    let mut grants = f.record.grants.clone();
    grants
        .capabilities
        .retain(|name| name != "worktrees.create");
    f.record = f
        .platform
        .consent(INSTALLATION, "package-one", grants)
        .unwrap();
    for query in [
        json!({"query":"effect","idempotencyKey":"plugin-local"}),
        json!({"query":"operation","operationId":created[0]["operation_id"]}),
    ] {
        let error = f
            .platform
            .dispatch(f.app.handle(), &caller(&f.record), "workflow.query", query)
            .await
            .unwrap_err();
        assert!(error.contains("permission_denied"), "{error}");
    }
    assert!(f.worktree_path("plugin-local").exists());
    assert!(f.worktree_path("plugin-origin").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn platform_native_root_from_only_open_worktree_targets_project_folder() {
    if isolated("platform_native_root_from_only_open_worktree_targets_project_folder") {
        return;
    }
    let f = Fixture::new().await;
    let linked = f.root.path().join("only-open-worktree");
    git(
        f.root.path(),
        &[
            "worktree",
            "add",
            "-b",
            "coordinator",
            linked.to_str().unwrap(),
        ],
    );
    let state = f.app.state::<AppStateStore>();
    let linked_workspace = state.create_empty_workspace_at_path(linked.clone()).0;
    state.set_workspace_project_root(&linked_workspace, f.root.path().display().to_string());
    state.set_workspace_worktree(
        &linked_workspace,
        linked.display().to_string(),
        "coordinator".into(),
    );
    assert!(state.activate_workspace(&linked_workspace));
    state.close_workspace(&f.workspace).unwrap();
    assert_eq!(state.snapshot().workspaces.len(), 1);
    let before = serde_json::to_value(state.find_workspace(&linked_workspace).unwrap()).unwrap();
    let (accepted, release_ack) = f.provider.hold_next_ack();
    let first = f
        .create(&caller(&f.record), f.request("project-root"))
        .await
        .unwrap();
    f.milestone(&first, &accepted).await;
    release_ack.notify_one();
    let terminal = f.settled(&first).await;
    assert_eq!(terminal["state"], "succeeded", "{terminal}");
    let starts = f.starts();
    assert_eq!(starts.len(), 1);
    assert_eq!(
        starts[0].cwd,
        f.root.path(),
        "explicit root must not silently launch in the only open worktree"
    );
    let destination = state
        .find_workspace(first["workspace_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(destination.cwd, f.root.path().display().to_string());
    assert!(destination.worktree_path.is_none());
    assert_eq!(state.snapshot().active_workspace_id.0, linked_workspace);
    assert_eq!(
        serde_json::to_value(state.find_workspace(&linked_workspace).unwrap()).unwrap(),
        before,
        "root dispatch must not add a chat to the linked checkout"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn platform_native_launch_authority_is_checked_before_checkout_or_provider_start() {
    if isolated("platform_native_launch_authority_is_checked_before_checkout_or_provider_start") {
        return;
    }
    for case in [
        "capability",
        "project-grant",
        "actor-scope",
        "read-only",
        "supervised",
        "plan",
        "narrowed-caller",
        "stale-generation",
        "unsupported-model",
        "unsupported-option",
    ] {
        let mut f = Fixture::new().await;
        let mut actor = caller(&f.record);
        let mut request = f.request(case);
        request["workspaceStrategy"] = json!({
            "type":"worktree","branch":case,"baseRef":"main","startFromOrigin":false
        });
        let mut authority = None;
        let expected = match case {
            "capability" => {
                let mut grants = f.record.grants.clone();
                grants
                    .capabilities
                    .retain(|name| name != "worktrees.create");
                f.record = f
                    .platform
                    .consent(INSTALLATION, "package-one", grants)
                    .unwrap();
                actor = caller(&f.record);
                "permission_denied"
            }
            "project-grant" => {
                let mut grants = f.record.grants.clone();
                grants.project_ids.clear();
                f.record = f
                    .platform
                    .consent(INSTALLATION, "package-one", grants)
                    .unwrap();
                actor = caller(&f.record);
                "permission_denied"
            }
            "actor-scope" => {
                actor.project_ids = Some(vec!["unrelated-project".into()]);
                "permission_denied"
            }
            "read-only" => {
                actor.control.access = ControlAccess::ReadOnly;
                "permission_denied"
            }
            "supervised" => {
                actor.control.access = ControlAccess::Supervised;
                request["nativePermissionMode"] = json!("bypassPermissions");
                "permission_ceiling"
            }
            "plan" => {
                let (captured, owner) = f.policy_caller("plan");
                actor = captured;
                authority = Some(owner);
                request["nativePermissionMode"] = json!("plan");
                "permission_denied"
            }
            "narrowed-caller" => {
                let (captured, owner) = f.policy_caller("bypassPermissions");
                actor = captured;
                owner.set_policy(ProviderKind::Claude, Some("plan"));
                authority = Some(owner);
                "permission_ceiling"
            }
            "stale-generation" => {
                f.platform.set_enabled(INSTALLATION, false).unwrap();
                f.record = f.platform.set_enabled(INSTALLATION, true).unwrap();
                "stale_generation"
            }
            "unsupported-model" => {
                request["modelSelection"]["model"] = json!("unadvertised-model");
                "unsupported_model"
            }
            "unsupported-option" => {
                request["modelSelection"]["options"][0]["value"] = json!("unadvertised-effort");
                "unsupported_model_option"
            }
            _ => unreachable!(),
        };
        let before = git(f.root.path(), &["worktree", "list", "--porcelain"]);
        let workspace_count = f.app.state::<AppStateStore>().snapshot().workspaces.len();
        let error = f.create(&actor, request).await.unwrap_err();
        assert!(error.contains(expected), "{case}: {error}");
        assert!(
            f.starts().is_empty(),
            "{case}: refused launch started an agent"
        );
        assert!(
            !f.worktree_path(case).exists(),
            "{case}: checkout was created"
        );
        assert_eq!(
            git(f.root.path(), &["worktree", "list", "--porcelain"]),
            before
        );
        assert!(git(
            f.root.path(),
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/codemux/plugin-worktrees/"
            ]
        )
        .is_empty());
        assert_eq!(
            f.app.state::<AppStateStore>().snapshot().workspaces.len(),
            workspace_count
        );
        assert_eq!(
            f.app
                .state::<DatabaseStore>()
                .control_thread_ids(&f.workspace, None, 100)
                .unwrap(),
            vec![SOURCE.to_string()]
        );
        drop(authority);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn platform_native_partial_worktree_preparation_retains_unknown_receipt_and_frozen_ref() {
    if isolated(
        "platform_native_partial_worktree_preparation_retains_unknown_receipt_and_frozen_ref",
    ) {
        return;
    }
    let mut f = Fixture::new().await;
    let commit = git(f.root.path(), &["rev-parse", "HEAD"]);
    // Git can freeze the reviewed base, but cannot create this branch. This
    // models a partial preparation without a racing test thread or real agent.
    let lock = f.root.path().join(".git/refs/heads/plugin-partial.lock");
    std::fs::write(&lock, "test-owned branch lock").unwrap();
    let mut request = f.request("partial-preparation");
    request["workspaceStrategy"] = json!({
        "type":"worktree","branch":"plugin-partial","baseRef":"main","startFromOrigin":false
    });
    let original = caller(&f.record);
    let error = f.create(&original, request.clone()).await.unwrap_err();
    assert!(error.contains("lock"), "{error}");
    let retained = git(
        f.root.path(),
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/codemux/plugin-worktrees/",
        ],
    );
    assert_eq!(retained.lines().count(), 1);
    assert!(retained.ends_with(&commit));
    let record = f.platform.store.read(INSTALLATION).unwrap();
    assert_eq!(record.receipts.len(), 1);
    let receipt = record.receipts.values().next().unwrap().clone();
    assert!(receipt.core_args.is_none());
    assert!(receipt.result.is_none());
    assert!(receipt.error.as_deref().unwrap().contains("lock"));
    assert!(f
        .app
        .state::<DatabaseStore>()
        .control_operation_by_key(
            "local",
            &receipt.command_key,
            "thread_launch",
            "no-core-command-was-prepared",
            &f.app.state::<NativeControlState>().epoch,
        )
        .unwrap()
        .is_none());
    assert!(f.starts().is_empty());
    std::fs::remove_file(lock).unwrap();
    let upgraded = f
        .platform
        .register(INSTALLATION, "next", manifest(), "package-two")
        .unwrap();
    f.record = f
        .platform
        .consent(INSTALLATION, "package-two", upgraded.grants)
        .unwrap();
    f.platform = PlatformService::open(f.root.path().join("platform"));
    let current = caller(&f.record);
    assert!(f
        .create(&original, request.clone())
        .await
        .unwrap_err()
        .contains("stale_generation"));
    for method in ["threads.create", "effects.read"] {
        let input = if method == "threads.create" {
            request.clone()
        } else {
            json!({"idempotencyKey":"partial-preparation"})
        };
        let error = f
            .platform
            .dispatch(f.app.handle(), &current, method, input)
            .await
            .unwrap_err();
        assert!(error.contains("outcome_unknown"), "{method}: {error}");
    }
    let mut changed = request;
    changed["workspaceStrategy"]["branch"] = json!("would-duplicate");
    assert!(f
        .create(&current, changed)
        .await
        .unwrap_err()
        .contains("idempotency_conflict"));
    assert_eq!(
        git(
            f.root.path(),
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/codemux/plugin-worktrees/"
            ]
        ),
        retained
    );
    assert!(f.starts().is_empty());
    assert_eq!(
        f.app.state::<AppStateStore>().snapshot().workspaces.len(),
        1
    );
    assert!(!f.worktree_path("plugin-partial").exists());
    assert!(!f.worktree_path("would-duplicate").exists());
    let retained_receipt = f.platform.store.read(INSTALLATION).unwrap();
    assert_eq!(retained_receipt.receipts.len(), 1);
    assert_eq!(
        retained_receipt
            .receipts
            .values()
            .next()
            .unwrap()
            .command_key,
        receipt.command_key
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn platform_native_failed_origin_fetch_never_falls_back_to_stale_or_local_base() {
    if isolated("platform_native_failed_origin_fetch_never_falls_back_to_stale_or_local_base") {
        return;
    }
    let f = Fixture::new().await;
    let commit = git(f.root.path(), &["rev-parse", "HEAD"]);
    let origin = f.root.path().join("empty-origin.git");
    git(f.root.path(), &["init", "--bare", origin.to_str().unwrap()]);
    git(
        f.root.path(),
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(
        f.root.path(),
        &["update-ref", "refs/remotes/origin/main", &commit],
    );
    let mut request = f.request("must-fetch");
    request["workspaceStrategy"] = json!({
        "type":"worktree","branch":"no-stale-fallback","baseRef":"origin/main","startFromOrigin":true
    });
    let actor = caller(&f.record);
    let error = f.create(&actor, request.clone()).await.unwrap_err();
    assert!(error.contains("could not be fetched"), "{error}");
    assert_eq!(
        git(f.root.path(), &["rev-parse", "refs/remotes/origin/main"]),
        commit
    );
    assert_eq!(git(f.root.path(), &["rev-parse", "main"]), commit);
    assert!(!f.worktree_path("no-stale-fallback").exists());
    assert!(f.starts().is_empty());
    assert_eq!(
        f.app.state::<AppStateStore>().snapshot().workspaces.len(),
        1
    );
    let replay = f.create(&actor, request).await.unwrap_err();
    assert!(replay.contains("outcome_unknown"), "{replay}");
    assert_eq!(
        f.platform.store.read(INSTALLATION).unwrap().receipts.len(),
        1
    );
}
