//! Integration tests for cross-provider delegation (`commands::delegation`).
//!
//! A Claude parent chat calls the `delegate_task` host tool through the same
//! [`HostTools`] entry its adapter uses; children run on mock Codex and
//! Claude providers, and their provider events are fed through
//! `forward_event` exactly as the event bridge would. The 1 s settle delay
//! and the 15 s sweep are never waited on: tests call `settle`, `deliver`
//! and `sweep_pass` directly. Work the backend spawns itself (the starter, a
//! failure's immediate delivery, the parent Stop cascade) runs on Tauri's
//! async runtime and is awaited by polling.
//!
//! Unix-only for the same reason as `agent_chat_commands.rs`:
//! `tauri::test::mock_app()` needs WebView2Loader.dll on Windows.

#![cfg(unix)]

#[path = "helpers/mock_agent_provider.rs"]
mod mock_agent_provider;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::{AppHandle, Manager, State};

use codemux_lib::agent_provider::types::MessageDelivery;
use codemux_lib::agent_provider::{
    AgentProvider, ApprovalDecision, CompletedItem, ProviderError, ProviderKind,
    ProviderRuntimeEvent, RequestId, SessionStatus, StartSessionInput, SubagentSnapshot,
    SubagentStatus, SubagentTaskKind, ThreadId, TurnId, TurnStatus, UserQuestion, UserQuestionSet,
};
use codemux_lib::commands::agent_chat::{
    agent_chat_interrupt_turn, agent_chat_respond_to_request, agent_chat_start_session,
    agent_chat_stop_session, forward_event, send_turn_with_origin, shutdown_agent_chat_threads,
    AgentChatChannelRegistry, GrokUsageLedgerBridge, ProviderRegistry, RunActivityTracker,
    SendTurnCommandInput, SubagentTracker, TurnOrigin,
};
use codemux_lib::commands::async_questions::{agent_chat_answer_question, QuestionAction};
use codemux_lib::commands::delegation::{
    deliver, drain_journal, is_delegated_child, settle, sweep_pass, DelegationHost,
    DelegationState, DELEGATION_RESULTS_PREFIX, DELEGATION_SETTING_KEY,
};
use codemux_lib::commands::usage_resume::agent_chat_resume_after_usage_limit;
use codemux_lib::database::delegated_tasks::DelegatedOpenTask;
use codemux_lib::database::{AgentChatSessionConfig, DatabaseStore};
use codemux_lib::mcp::registry::{HostCaller, HostTools};
use codemux_lib::observability::{FeatureFlags, ObservabilityStore};
use codemux_lib::presets::LaunchMode;
use codemux_lib::state::{AppStateStore, WorkspaceSnapshot};

use crate::mock_agent_provider::{MockAgentProvider, MockCall};

/// The tool's registry name; Codex's model-visible spelling maps back to it.
const DELEGATE_TOOL: &str = "mcp__codemux__delegate_task";
const FULL: Option<&str> = Some("bypassPermissions");
const TASK: &str = "Add slugify(text) to src/lib/strings.ts: lowercase, fold accents, collapse non-alphanumerics into one hyphen. Add table tests in src/lib/strings.test.ts.";
/// Appended to the task as the child's first message, verbatim.
const CHILD_FOOTER: &str = "\n\n---\nFrom Codemux: another coding agent handed you this task and will read your final message. Work independently. Ask a question if truly blocked; it reaches the coordinator. Otherwise choose a safe option and explain it. End with a short report: what you changed (files), how you checked it, and anything left undone.";
const STOPPED_WITH_PARENT: &str = "Stopped together with this chat.";
const STOPPED_BY_USER: &str =
    "The user stopped this task. Partial changes may be in the working tree.";

// ── Harness ──

/// One app with a Full-access Claude parent chat in its own tab (a second,
/// later tab holds focus), its session live on the mock, and the delegation
/// setting on.
struct Harness {
    _app: tauri::App<MockRuntime>,
    handle: AppHandle<MockRuntime>,
    claude: Arc<MockAgentProvider>,
    codex: Arc<MockAgentProvider>,
    host: DelegationHost<MockRuntime>,
    workspace: String,
    cwd: String,
    parent: String,
    parent_pane: String,
    _dir: tempfile::TempDir,
}

fn observability() -> ObservabilityStore {
    let store = ObservabilityStore::default();
    let flags = store.feature_flags();
    store.set_feature_flags(FeatureFlags {
        enable_agent_chat: true,
        ..flags
    });
    store
}

fn start_input(thread_id: &str, cwd: &str) -> StartSessionInput {
    StartSessionInput {
        thread_id: ThreadId(thread_id.into()),
        cwd: cwd.into(),
        model: None,
        resume_cursor: None,
        fresh_session: false,
        permission_mode: None,
        effort: None,
        context_window: None,
        fast_mode: false,
        service_tier: None,
        additional_directories: vec![],
        recorded_usage_baseline: None,
        env: None,
        workspace_id: None,
        extra: Value::Null,
        remote: None,
    }
}

fn queued(thread_id: &str, text: &str) -> SendTurnCommandInput {
    SendTurnCommandInput {
        input_source: None,
        delivery: MessageDelivery::Queue,
        thread_id: ThreadId(thread_id.into()),
        text: text.into(),
        display_text: None,
        skill_ids: Vec::new(),
        skill_text: None,
        include_plugins: true,
        images: Vec::new(),
        model_override: None,
        effort_override: None,
        permission_mode_override: None,
        client_nonce: None,
    }
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn args(provider: &str, title: &str) -> Value {
    json!({ "provider": provider, "title": title, "task": TASK })
}

/// Poll until `done` holds; the backend's own spawned work runs on another
/// runtime, so this only ever waits for that, never for a timer.
async fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Index of the tab whose surface holds `thread`'s chat pane.
fn tab_index(state: &AppStateStore, workspace: &WorkspaceSnapshot, thread: &str) -> usize {
    let pane = state
        .agent_chat_pane_id_for_thread(thread)
        .expect("thread should have a pane");
    let surface = workspace
        .surfaces
        .iter()
        .find(|surface| surface.active_pane_id.0 == pane)
        .map(|surface| surface.surface_id.clone())
        .expect("pane should have a surface");
    workspace
        .tabs
        .iter()
        .position(|tab| tab.surface_id.as_ref() == Some(&surface))
        .expect("surface should have a tab")
}

impl Harness {
    async fn new() -> Self {
        Self::with_install_check(|_| true).await
    }

    async fn with_install_check(is_installed: fn(ProviderKind) -> bool) -> Self {
        // Every test here emits app state, which persists the layout: keep
        // that in Cargo's scratch directory, never the user's config.
        codemux_lib::state::redirect_layout_persistence(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("agent-delegation-layout-{}.json", std::process::id())),
        );
        let app = tauri::test::mock_app();
        // The delegation setting is left unset: the feature is on by default.
        let db = DatabaseStore::new_in_memory();
        app.manage(db);
        app.manage(AgentChatChannelRegistry::default());
        app.manage(GrokUsageLedgerBridge::default());
        app.manage(SubagentTracker::default());
        app.manage(RunActivityTracker::default());
        app.manage(AppStateStore::default());
        app.manage(observability());
        app.manage(DelegationState::default());
        app.manage(codemux_lib::agent_control::NativeControlState::default());
        let claude = Arc::new(MockAgentProvider::new(ProviderKind::Claude));
        let mut codex = MockAgentProvider::new(ProviderKind::Codex);
        codex.async_questions = true;
        let codex = Arc::new(codex);
        let registry = ProviderRegistry::new();
        registry.set_claude(claude.clone() as _).await;
        registry.set_codex(codex.clone() as _).await;
        app.manage(registry);
        let handle = app.handle().clone();

        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let parent = unique("parent");
        let (workspace, parent_pane) = {
            let state: State<'_, AppStateStore> = handle.state();
            let workspace = state.create_workspace_at_path(dir.path().to_path_buf()).0;
            let parent_pane = state
                .create_agent_chat_pane(
                    &workspace,
                    Some(ProviderKind::Claude),
                    Some(cwd.clone()),
                    Some(LaunchMode::NewTab),
                    Some(parent.clone()),
                )
                .unwrap()
                .0;
            // A later tab holds focus, so "beside the parent" is not "last".
            state
                .create_agent_chat_pane(
                    &workspace,
                    Some(ProviderKind::Claude),
                    None,
                    Some(LaunchMode::NewTab),
                    None,
                )
                .unwrap();
            (workspace, parent_pane)
        };
        {
            let db: State<'_, DatabaseStore> = handle.state();
            db.upsert_agent_chat_session(&parent, &workspace, Some(&cwd), "claude")
                .unwrap();
        }
        claude
            .start_session(start_input(&parent, &cwd))
            .await
            .unwrap();
        let host = DelegationHost::with_install_check(handle.clone(), is_installed);
        Self {
            _app: app,
            handle,
            claude,
            codex,
            host,
            workspace,
            cwd,
            parent,
            parent_pane,
            _dir: dir,
        }
    }

    fn db(&self) -> State<'_, DatabaseStore> {
        self.handle.state()
    }

    fn app_state(&self) -> State<'_, AppStateStore> {
        self.handle.state()
    }

    fn mock(&self, provider: ProviderKind) -> &MockAgentProvider {
        match provider {
            ProviderKind::Claude => &self.claude,
            ProviderKind::Codex => &self.codex,
            other => panic!("no mock for {other:?}"),
        }
    }

    fn caller(&self, mode: Option<&str>) -> HostCaller {
        HostCaller {
            control: None,
            thread_id: self.parent.clone(),
            provider: ProviderKind::Claude,
            workspace_id: Some(self.workspace.clone()),
            permission_mode: mode.map(str::to_string),
        }
    }

    /// Call the tool as `caller`: the started JSON, or the refusal text.
    fn call_as(&self, caller: &HostCaller, arguments: Value) -> Result<Value, String> {
        let result = self
            .host
            .call(caller, DELEGATE_TOOL, &arguments)
            .expect("delegate_task is a host tool");
        let text = result["content"][0]["text"]
            .as_str()
            .expect("one text block")
            .to_string();
        if result["isError"] == json!(true) {
            Err(text)
        } else {
            Ok(serde_json::from_str(&text).expect("the started result is JSON"))
        }
    }

    fn call(&self, mode: Option<&str>, arguments: Value) -> Result<Value, String> {
        self.call_as(&self.caller(mode), arguments)
    }

    /// Delegate from the Full-access parent; returns the child thread.
    fn delegate(&self, arguments: Value) -> String {
        let started = self.call(FULL, arguments).expect("delegation accepted");
        started["started"]["thread"]
            .as_str()
            .expect("the child thread id")
            .to_string()
    }

    /// Delegate and wait until the child has the task (card Running).
    async fn delegate_and_dispatch(&self, arguments: Value) -> String {
        let child = self.delegate(arguments);
        wait_for("the task to reach its child", || {
            self.cards(&child)
                .last()
                .is_some_and(|card| card.status == SubagentStatus::Running)
        })
        .await;
        child
    }

    /// Every delegated card the parent persisted for `child`, in order.
    fn cards(&self, child: &str) -> Vec<SubagentSnapshot> {
        let id = format!("delegate:{child}");
        self.db()
            .list_agent_chat_messages(&self.parent)
            .iter()
            .filter_map(|row| serde_json::from_str::<ProviderRuntimeEvent>(row).ok())
            .filter_map(|event| match event {
                ProviderRuntimeEvent::SubagentUpdated { subagent, .. }
                    if subagent.subagent_id == id =>
                {
                    Some(subagent)
                }
                _ => None,
            })
            .collect()
    }

    fn card(&self, child: &str) -> SubagentSnapshot {
        self.cards(child).pop().expect("the parent has a card")
    }

    /// Texts `provider`'s mock was asked to send on `thread`.
    fn sent(&self, provider: ProviderKind, thread: &str) -> Vec<String> {
        self.mock(provider)
            .calls
            .snapshot()
            .into_iter()
            .filter_map(|call| match call {
                MockCall::SendTurn(ThreadId(t), text) if t == thread => Some(text),
                _ => None,
            })
            .collect()
    }

    /// Results messages posted into the parent.
    fn wakes(&self) -> Vec<String> {
        self.sent(ProviderKind::Claude, &self.parent)
            .into_iter()
            .filter(|text| text.starts_with(DELEGATION_RESULTS_PREFIX))
            .collect()
    }

    fn interrupted(&self, provider: ProviderKind, thread: &str) -> bool {
        self.mock(provider)
            .calls
            .snapshot()
            .contains(&MockCall::InterruptTurn(ThreadId(thread.into()), None))
    }

    fn child_says(&self, child: &str, text: &str) {
        forward_event(
            &self.handle,
            ProviderRuntimeEvent::ItemCompleted {
                thread_id: ThreadId(child.into()),
                turn_id: TurnId("child-turn".into()),
                item: CompletedItem::AssistantText { text: text.into() },
                subagent_id: None,
            },
        );
    }

    fn turn_ends(&self, thread: &str, status: TurnStatus) {
        forward_event(
            &self.handle,
            ProviderRuntimeEvent::TurnCompleted {
                thread_id: ThreadId(thread.into()),
                turn_id: TurnId("child-turn".into()),
                status,
                usage: None,
            },
        );
    }

    /// The child's final message and a clean end of its turn.
    fn child_reports(&self, child: &str, report: &str) {
        self.child_says(child, report);
        self.turn_ends(child, TurnStatus::Success);
    }

    async fn settle(&self, child: &str) {
        settle(&self.handle, child).await;
    }

    async fn deliver(&self) {
        deliver(&self.handle, &self.parent).await;
    }

    /// Wait until the parent has received exactly `n` results messages,
    /// retrying delivery as the sweep would: a report can come due while an
    /// earlier delivery (or a settle the backend spawned) still holds the
    /// parent's activity lock.
    async fn wait_for_wakes(&self, n: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let wakes = self.wakes();
            if wakes.len() >= n {
                assert_eq!(wakes.len(), n, "one message per report: {wakes:#?}");
                return wakes;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {n} results messages, got {}",
                wakes.len()
            );
            self.deliver().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The parent's persisted user messages, as the transcript shows them.
    fn parent_user_messages(&self) -> Vec<String> {
        self.db()
            .list_agent_chat_messages(&self.parent)
            .iter()
            .map(|row| serde_json::from_str::<Value>(row).unwrap())
            .filter(|row| row["type"] == "user_message")
            .filter_map(|row| row["text"].as_str().map(str::to_string))
            .collect()
    }
}

/// Every scenario lives under `delegation::`, so the usual
/// `cargo test delegat` filter runs all of them.
mod delegation {
    use super::*;

    // ── Delegating ──

    #[tokio::test]
    async fn delegating_opens_a_background_child_workspace_and_sends_the_exact_task() {
        let h = Harness::new().await;
        let state = h.app_state();
        let before = state.find_workspace(&h.workspace).unwrap();
        let active_workspace = state.snapshot().active_workspace_id;

        let started = h
            .call(
                FULL,
                json!({
                    "provider": "codex",
                    "title": "Add slugify helper",
                    "task": TASK,
                    "model": "gpt-6.1-codex",
                    "effort": "high",
                }),
            )
            .unwrap();
        let child = started["started"]["thread"].as_str().unwrap().to_string();
        assert_eq!(
            started["started"],
            json!({
                "title": "Add slugify helper",
                "provider": "codex",
                "model": "gpt-6.1-codex",
                "effort": "high",
                "thread": child,
                "task_id": started["task_id"],
                "child_thread_id": child,
            })
        );
        assert!(started["note"].as_str().unwrap().contains("durable"));

        // Its own workspace nested under the parent's, in the same folder,
        // titled; the parent's workspace and focus stay put.
        let after = state.find_workspace(&h.workspace).unwrap();
        assert_eq!(state.snapshot().active_workspace_id, active_workspace);
        assert_eq!(after.active_tab_id, before.active_tab_id);
        assert_eq!(after.active_surface_id, before.active_surface_id);
        assert_eq!(after.tabs.len(), before.tabs.len());
        let child_pane = state.agent_chat_pane_id_for_thread(&child).unwrap();
        let child_workspace = state.workspace_id_for_pane(&child_pane).unwrap();
        let ws = state.find_workspace(&child_workspace).unwrap();
        assert_ne!(child_workspace, h.workspace);
        assert_eq!(ws.parent_workspace_id.as_deref(), Some(h.workspace.as_str()));
        assert_eq!(ws.title, "Add slugify helper");
        assert_eq!(ws.cwd, h.cwd);
        assert_eq!(ws.worktree_path, None);
        assert_eq!(ws.project_root, after.project_root);
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.tabs[tab_index(&state, &ws, &child)].title, "Codex · Add slugify helper");
        assert_eq!(
            state.agent_chat_pane_thread(&child_pane),
            Some((ProviderKind::Codex, child.clone()))
        );

        // The child gets exactly the task plus the footer, in Codex's full
        // mode, with the requested model and effort, in the parent's folder.
        // (The Running card goes out only once that send has fully returned.)
        wait_for("the Running card", || {
            h.card(&child).status == SubagentStatus::Running
        })
        .await;
        let expected_text = format!("{TASK}{CHILD_FOOTER}");
        assert_eq!(
            h.sent(ProviderKind::Codex, &child),
            vec![expected_text.clone()]
        );
        let inputs = h.codex.start_inputs();
        assert_eq!(inputs.len(), 1);
        let input = &inputs[0];
        assert_eq!(input.thread_id.0, child);
        assert_eq!(input.permission_mode.as_deref(), Some("danger-full-access"));
        assert_eq!(input.model.as_deref(), Some("gpt-6.1-codex"));
        assert_eq!(input.effort.as_deref(), Some("high"));
        assert!(input.fresh_session);
        assert!(input.resume_cursor.is_none());
        assert_eq!(input.cwd.to_string_lossy(), h.cwd);
        assert_eq!(input.workspace_id.as_deref(), Some(child_workspace.as_str()));

        // An ordinary chat row: its title, full mode and the exact task text.
        let db = h.db();
        let record = db.get_agent_chat_session(&child).unwrap();
        assert_eq!(record.provider, "codex");
        assert_eq!(record.workspace_id, child_workspace);
        assert_eq!(record.cwd.as_deref(), Some(h.cwd.as_str()));
        assert_eq!(record.title.as_deref(), Some("Add slugify helper"));
        assert_eq!(
            record.permission_mode.as_deref(),
            Some("danger-full-access")
        );
        assert_eq!(record.model.as_deref(), Some("gpt-6.1-codex"));
        assert_eq!(record.effort.as_deref(), Some("high"));
        let user_rows: Vec<Value> = db
            .list_agent_chat_messages(&child)
            .iter()
            .map(|row| serde_json::from_str::<Value>(row).unwrap())
            .filter(|row| row["type"] == "user_message")
            .collect();
        assert_eq!(user_rows.len(), 1);
        assert_eq!(user_rows[0]["text"], TASK);
        assert_eq!(user_rows[0]["agent_text"], expected_text);

        // The parent's card: Starting, then Working, each with every field.
        let cards = h.cards(&child);
        assert_eq!(cards[0].status, SubagentStatus::Pending);
        assert_eq!(cards[0].activity.as_deref(), Some("Starting…"));
        for card in &cards {
            assert_eq!(card.name.as_deref(), Some("Codex"));
            assert_eq!(card.agent_type.as_deref(), Some("codex"));
            assert_eq!(card.description.as_deref(), Some("Add slugify helper"));
            assert_eq!(card.model.as_deref(), Some("gpt-6.1-codex"));
            assert_eq!(card.effort.as_deref(), Some("high"));
            assert_eq!(card.provider_ref.as_deref(), Some(child.as_str()));
            assert_eq!(card.task_kind, Some(SubagentTaskKind::Agent));
            assert!(card.parent_item_id.is_none());
        }
        let running = cards.last().unwrap();
        assert_eq!(running.activity.as_deref(), Some("Working in its workspace"));
        assert!(running.result_text.is_none());
        assert!(running.duration_ms.is_none());

        // Tracked as a delegated child (its own finish pushes are muted),
        // journaled for a relaunch, and nothing posted to the parent yet.
        assert!(is_delegated_child(&h.handle, &child));
        assert!(!is_delegated_child(&h.handle, &h.parent));
        assert!(h.wakes().is_empty());
        let durable = db.delegation_task_for_child(&child).unwrap().unwrap();
        assert_eq!(durable.child_thread_id.as_deref(), Some(child.as_str()));
        assert_ne!(durable.task_id, child);
        assert_eq!(durable.parent_thread_id, h.parent);
        assert_eq!(durable.provider, "codex");
        assert_eq!(durable.title, "Add slugify helper");
    }

    #[tokio::test]
    async fn a_finished_task_wakes_the_idle_parent_exactly_once() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        h.child_reports(&child, "Added slugify and checked 9 tests.");
        h.settle(&child).await;
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].starts_with(DELEGATION_RESULTS_PREFIX));
        assert!(wakes[0].contains("Added slugify and checked 9 tests."));
        let task = h.db().delegation_task_for_child(&child).unwrap().unwrap();
        assert!(wakes[0].contains(&task.task_id));
        assert_eq!(
            task.result.unwrap().text,
            "Added slugify and checked 9 tests."
        );
        assert_eq!(
            h.parent_user_messages(),
            vec!["Add slugify helper\nAdded slugify and checked 9 tests.".to_string()]
        );
        let row: Value = h.db().list_agent_chat_messages(&h.parent).into_iter()
            .filter_map(|row| serde_json::from_str::<Value>(&row).ok())
            .find(|row| row["type"] == "user_message").unwrap();
        assert_eq!(row["agent_text"], wakes[0]);
        for _ in 0..3 {
            h.deliver().await;
            h.settle(&child).await;
            sweep_pass(&h.handle).await;
        }
        assert_eq!(h.wakes().len(), 1);
        assert_eq!(h.parent_user_messages().len(), 1);
        assert_eq!(h.card(&child).status, SubagentStatus::Completed);
        assert!(h.db().delegation_task_for_child(&child).unwrap().is_some());
    }

    #[tokio::test]
    async fn auto_delivery_never_interrupts_a_busy_parent() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        h.claude.set_turn_active(&h.parent, true);
        h.child_reports(&child, "Done: slugify added.");
        h.settle(&child).await;
        let task = h.db().delegation_task_for_child(&child).unwrap().unwrap();
        assert!(task.result.is_some());
        assert!(task.delivery_id.is_some());
        assert!(!h.interrupted(ProviderKind::Claude, &h.parent));
        assert_eq!(h.wakes().len(), 1);
        h.claude.set_turn_active(&h.parent, false);
        h.deliver().await;
        assert_eq!(h.wakes().len(), 1);
    }

    #[tokio::test]
    async fn completed_siblings_publish_independently() {
        let h = Harness::new().await;
        let a = h.delegate_and_dispatch(args("codex", "First review")).await;
        let b = h
            .delegate_and_dispatch(args("claude", "Second review"))
            .await;
        h.child_reports(&a, "First independent result");
        h.settle(&a).await;
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains("First independent result"));
        assert_eq!(h.card(&b).status, SubagentStatus::Running);
        h.child_reports(&b, "Second independent result");
        h.settle(&b).await;
        let wakes = h.wait_for_wakes(2).await;
        assert!(wakes[1].contains("Second independent result"));
        assert!(!wakes[1].contains("First independent result"));
    }

    #[tokio::test]
    async fn a_failure_posts_at_once_while_the_rest_of_the_round_runs() {
        let h = Harness::new().await;
        // Running first, so the failure below always has a sibling to name.
        let claude = h
            .delegate_and_dispatch(args("claude", "Review the parser"))
            .await;
        h.codex.fail_next_start(ProviderError::NotAuthenticated {
            provider: ProviderKind::Codex,
            hint: "Run `codex login` and try again.".into(),
        });
        let codex = h.delegate(args("codex", "Add slugify helper"));

        wait_for("the failure to post at once", || h.wakes().len() == 1).await;
        let wake = &h.wakes()[0];
        assert!(wake.contains("Status: Failed"));
        assert!(
            wake.contains("It could not start: not signed in: Run `codex login` and try again.")
        );
        assert_eq!(h.codex.start_inputs().len(), 1);
        assert!(!wake.contains("<task "));

        let card = h.card(&codex);
        assert_eq!(card.status, SubagentStatus::Failed);
        assert_eq!(
            card.result_text.as_deref(),
            Some("It could not start: not signed in: Run `codex login` and try again.")
        );
        // The child's tab keeps the reason too.
        let errors: Vec<String> = h
            .db()
            .list_agent_chat_messages(&codex)
            .iter()
            .filter_map(|row| serde_json::from_str::<ProviderRuntimeEvent>(row).ok())
            .filter_map(|event| match event {
                ProviderRuntimeEvent::SessionStateChanged {
                    status: SessionStatus::Error { message },
                    ..
                } => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(
            errors,
            vec!["It could not start: not signed in: Run `codex login` and try again.".to_string()]
        );

        // A Claude child whose run ends in an API error (signed out, bad key)
        // fails as well, and the rest of the round posts with it.
        h.child_says(&claude, "Invalid API key · Please run /login");
        h.turn_ends(
            &claude,
            TurnStatus::Error {
                subtype: "api_error".into(),
                message: "Invalid API key · Please run /login".into(),
            },
        );
        h.settle(&claude).await;
        let wakes = h.wait_for_wakes(2).await;
        assert!(wakes[1].contains("Status: Failed"));
        assert!(wakes[1].contains("Invalid API key · Please run /login"));
        assert!(!wakes[1].contains("Add slugify helper"), "posted once only");
    }

    // ── Stop, close and restart ──

    #[tokio::test]
    async fn parent_stop_keeps_published_results_and_disposes_future_wakes() {
        let h = Harness::new().await;
        let a = h.delegate_and_dispatch(args("codex", "First review")).await;
        let b = h
            .delegate_and_dispatch(args("claude", "Second review"))
            .await;
        h.child_reports(&a, "Published before Stop");
        h.settle(&a).await;
        h.wait_for_wakes(1).await;
        agent_chat_interrupt_turn(
            h.handle.clone(),
            ProviderKind::Claude,
            ThreadId(h.parent.clone()),
            None,
        )
        .await
        .unwrap();
        wait_for("child stop acknowledgement", || {
            h.card(&b).status == SubagentStatus::Stopped
        })
        .await;
        for child in [&a, &b] {
            assert_eq!(
                h.db()
                    .delegation_task_for_child(child)
                    .unwrap()
                    .unwrap()
                    .completion_delivery,
                "disposed_by_stop"
            );
        }
        assert_eq!(
            h.db()
                .delegation_task_for_child(&a)
                .unwrap()
                .unwrap()
                .result
                .unwrap()
                .text,
            "Published before Stop"
        );
        h.deliver().await;
        sweep_pass(&h.handle).await;
        assert_eq!(h.wakes().len(), 1);
        assert!(h.interrupted(ProviderKind::Claude, &b));
    }

    #[tokio::test]
    async fn stopping_one_child_publishes_its_own_cancelled_result() {
        let h = Harness::new().await;
        let a = h.delegate_and_dispatch(args("codex", "First review")).await;
        let b = h
            .delegate_and_dispatch(args("claude", "Second review"))
            .await;
        agent_chat_interrupt_turn(
            h.handle.clone(),
            ProviderKind::Codex,
            ThreadId(a.clone()),
            None,
        )
        .await
        .unwrap();
        h.deliver().await;
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains(STOPPED_BY_USER));
        assert_eq!(h.card(&b).status, SubagentStatus::Running);
        h.child_reports(&b, "Second result");
        h.settle(&b).await;
        let wakes = h.wait_for_wakes(2).await;
        assert!(wakes[1].contains("Second result"));
    }

    #[tokio::test]
    async fn provider_restart_preserves_task_ownership_without_stop_disposition() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Review parser"))
            .await;
        agent_chat_stop_session(
            h.handle.clone(),
            ProviderKind::Codex,
            ThreadId(child.clone()),
        )
        .await
        .unwrap();
        let task = h.db().delegation_task_for_child(&child).unwrap().unwrap();
        assert_eq!(task.cancellation_generation, 0);
        assert_ne!(task.completion_delivery, "disposed_by_stop");
        assert_eq!(h.db().delivery_generation(&child).unwrap(), (0, false));
        assert!(h.wakes().is_empty());
        assert!(task.result.is_none());
    }

    #[tokio::test]
    async fn closing_a_child_reports_tab_close_wording_to_parent() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Review parser"))
            .await;
        shutdown_agent_chat_threads(&h.handle, vec![(ProviderKind::Codex, child.clone())]);
        wait_for("child close confirmation", || {
            h.card(&child).status == SubagentStatus::Stopped
        })
        .await;
        wait_for("closed session acknowledgement", || {
            h.interrupted(ProviderKind::Codex, &child) || h.codex.calls.snapshot().contains(&MockCall::StopSession(ThreadId(child.clone())))
        }).await;
        h.deliver().await;
        let record = h.db().delegation_task_for_child(&child).unwrap().unwrap();
        assert_eq!(record.result.as_ref().unwrap().text, "The user closed its chat. Partial changes may be in the working tree.");
        assert_ne!(record.completion_delivery, "disposed_by_stop");
        assert!(h
            .db()
            .delegation_task_for_child(&child)
            .unwrap()
            .unwrap()
            .result
            .is_some());
    }

    #[tokio::test]
    async fn new_chat_in_the_parent_tab_stops_its_tasks_without_a_message() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;

        // New Chat stops the old session in place first; a parent restart
        // alone leaves its tasks running.
        agent_chat_stop_session(
            h.handle.clone(),
            ProviderKind::Claude,
            ThreadId(h.parent.clone()),
        )
        .await
        .unwrap();
        assert_eq!(h.card(&child).status, SubagentStatus::Running);

        // Then it starts a new thread on the same pane, rebinding it.
        let fresh = unique("fresh");
        agent_chat_start_session(
            h.handle.clone(),
            h.parent_pane.clone(),
            ProviderKind::Claude,
            start_input(&fresh, &h.cwd),
            Some(h.parent.clone()),
        )
        .await
        .unwrap();
        assert_eq!(
            h.app_state().agent_chat_pane_thread(&h.parent_pane),
            Some((ProviderKind::Claude, fresh.clone()))
        );

        wait_for("rebound child stop confirmation", || {
            h.card(&child).status == SubagentStatus::Stopped
        })
        .await;
        let card = h.card(&child);
        assert_eq!(card.status, SubagentStatus::Stopped);
        assert_eq!(card.result_text.as_deref(), Some(STOPPED_WITH_PARENT));
        // Its own workspace closed with it.
        let child_workspace = h.db().delegation_task_for_child(&child).unwrap().unwrap().workspace_id;
        assert_ne!(child_workspace, h.workspace);
        assert!(h.app_state().find_workspace(&child_workspace).is_none());
        assert!(h.app_state().find_workspace(&h.workspace).is_some());
        wait_for("the orphaned child to be interrupted", || {
            h.interrupted(ProviderKind::Codex, &child)
        })
        .await;

        h.deliver().await;
        deliver(&h.handle, &fresh).await;
        sweep_pass(&h.handle).await;
        assert!(h.wakes().is_empty());
        assert!(h
            .sent(ProviderKind::Claude, &fresh)
            .iter()
            .all(|text| !text.starts_with(DELEGATION_RESULTS_PREFIX)));
    }

    #[tokio::test]
    async fn a_stop_during_a_slow_start_never_sends_the_task() {
        let h = Harness::new().await;
        let (entered, release) = h.codex.hold_next_start();
        let child = h.delegate(args("codex", "Add slugify helper"));
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .expect("the child's start is in flight");

        // The row Stop lands while the starter still holds the child's lock.
        let stop = tokio::spawn(agent_chat_interrupt_turn(
            h.handle.clone(),
            ProviderKind::Codex,
            ThreadId(child.clone()),
            None,
        ));
        wait_for("durable stopping state before acknowledgement", || {
            h.card(&child).activity.as_deref() == Some("Stopping; provider confirmation pending")
        })
        .await;
        release.notify_one();
        stop.await.unwrap().unwrap();

        assert!(h.interrupted(ProviderKind::Codex, &child));
        assert!(h.sent(ProviderKind::Codex, &child).is_empty());
        assert_eq!(h.card(&child).result_text.as_deref(), Some(STOPPED_BY_USER));
        h.deliver().await;
        assert_eq!(h.wait_for_wakes(1).await.len(), 1);
    }

    // ── Backstops ──

    #[tokio::test]
    async fn a_slow_start_is_never_swept() {
        let h = Harness::new().await;
        let (entered, release) = h.codex.hold_next_start();
        let child = h.delegate(args("codex", "Add slugify helper"));
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .expect("the child's start is in flight");

        // No session and no turn: idle by every signal, but not yet dispatched.
        for _ in 0..3 {
            sweep_pass(&h.handle).await;
        }
        h.settle(&child).await;
        let card = h.card(&child);
        assert_eq!(card.status, SubagentStatus::Pending);
        assert_eq!(card.activity.as_deref(), Some("Starting…"));
        assert!(h.wakes().is_empty());

        release.notify_one();
        wait_for("the task to reach its child", || {
            h.card(&child).status == SubagentStatus::Running
        })
        .await;
        assert_eq!(
            h.sent(ProviderKind::Codex, &child),
            vec![format!("{TASK}{CHILD_FOOTER}")]
        );
    }

    #[tokio::test]
    async fn a_codex_child_that_asked_for_approval_still_finishes_and_reports() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        let request = |id: &str| {
            forward_event(
                &h.handle,
                ProviderRuntimeEvent::RequestOpened {
                    thread_id: ThreadId(child.clone()),
                    turn_id: TurnId("child-turn".into()),
                    request_id: RequestId(id.into()),
                    request_kind: "tool".into(),
                    payload: json!({}),
                    tool_use_id: None,
                    subagent_id: None,
                },
            )
        };

        // The user answers in its tab. Codex never reports the request as
        // resolved, so the answer itself moves the card on.
        request("req-1");
        assert_eq!(
            h.card(&child).activity.as_deref(),
            Some("Waiting for your answer in its workspace")
        );
        agent_chat_respond_to_request(
            h.handle.clone(),
            ProviderKind::Codex,
            ThreadId(child.clone()),
            RequestId("req-1".into()),
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .unwrap();
        assert!(h
            .codex
            .calls
            .snapshot()
            .contains(&MockCall::RespondToRequest(
                ThreadId(child.clone()),
                RequestId("req-1".into())
            )));
        assert_eq!(
            h.card(&child).activity.as_deref(),
            Some("Working in its workspace")
        );

        // Only an explicit answer resolves an approval.
        request("req-2");
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::RequestResolved {
                thread_id: ThreadId(child.clone()),
                request_id: RequestId("req-2".into()),
                decision: ApprovalDecision::Cancel,
            },
        );
        h.child_reports(&child, "Added slugify after the approval.");
        h.settle(&child).await;
        assert_eq!(h.card(&child).status, SubagentStatus::Completed);
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains("Added slugify after the approval."));
    }

    #[tokio::test]
    async fn a_child_that_fails_or_dies_with_a_question_open_still_reports() {
        let h = Harness::new().await;
        let ask = |child: &str| {
            forward_event(
                &h.handle,
                ProviderRuntimeEvent::QuestionsAsked {
                    thread_id: ThreadId(child.to_string()),
                    question: UserQuestionSet {
                        id: format!("q-{child}"),
                        target: "native".into(),
                        source_item_id: "q".into(),
                        source_turn_id: "child-turn".into(),
                        text: String::new(),
                        questions: vec![UserQuestion {
                            title: "Which storage?".into(),
                            options: vec![],
                        }],
                        subagent_id: None,
                    },
                },
            );
        };

        // The turn fails while the child's question is still open: the
        // question can no longer be answered into this task, so it fails and
        // reports instead of waiting forever.
        let failed = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        ask(&failed);
        assert_eq!(
            h.card(&failed).activity.as_deref(),
            Some("Waiting for your answer in its workspace")
        );
        h.turn_ends(
            &failed,
            TurnStatus::Error {
                subtype: "error_during_execution".into(),
                message: "Codex crashed mid-task".into(),
            },
        );
        h.settle(&failed).await;
        assert_eq!(h.card(&failed).status, SubagentStatus::Failed);
        assert_eq!(
            h.card(&failed).result_text.as_deref(),
            Some("Codex crashed mid-task")
        );
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains("Status: Failed"));

        // The session dies with a question open and no turn end at all.
        let dead = h
            .delegate_and_dispatch(args("codex", "Review the parser"))
            .await;
        ask(&dead);
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ThreadId(dead.clone()),
                status: SessionStatus::Error {
                    message: "app-server exited".into(),
                },
            },
        );
        h.settle(&dead).await;
        assert_eq!(h.card(&dead).status, SubagentStatus::Failed);
        assert_eq!(
            h.card(&dead).result_text.as_deref(),
            Some("Its session ended while it was waiting for your answer.")
        );
        let wakes = h.wait_for_wakes(2).await;
        assert!(wakes[1].contains("Status: Failed"));

        // A successful turn that left its question open is a real wait, until
        // the session dies: then nobody can answer into this task any more.
        let waiting = h
            .delegate_and_dispatch(args("codex", "Pick a storage engine"))
            .await;
        ask(&waiting);
        h.child_reports(&waiting, "Which storage should I use?");
        h.settle(&waiting).await;
        assert_eq!(h.card(&waiting).status, SubagentStatus::Running);
        assert_eq!(
            h.card(&waiting).activity.as_deref(),
            Some("Waiting for your answer in its workspace")
        );
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ThreadId(waiting.clone()),
                status: SessionStatus::Error {
                    message: "app-server exited".into(),
                },
            },
        );
        h.settle(&waiting).await;
        assert_eq!(h.card(&waiting).status, SubagentStatus::Failed);
        assert_eq!(
            h.card(&waiting).result_text.as_deref(),
            Some("Its session ended while it was waiting for your answer.")
        );
        let wakes = h.wait_for_wakes(3).await;
        assert!(wakes[2].contains("Status: Failed"));

        // A session that dies mid-turn with nothing asked fails plainly.
        let crashed = h
            .delegate_and_dispatch(args("codex", "Bump the lockfile"))
            .await;
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ThreadId(crashed.clone()),
                status: SessionStatus::Error {
                    message: "app-server exited".into(),
                },
            },
        );
        h.settle(&crashed).await;
        assert_eq!(
            h.card(&crashed).result_text.as_deref(),
            Some("Its session ended before it finished.")
        );
    }

    #[tokio::test]
    async fn the_sweep_needs_two_idle_passes_and_waits_for_open_questions() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        // Its turn end is lost (broadcast lag); only this text arrived.
        h.child_says(&child, "Halfway through the parser.");

        // An approval prompt in the child's tab.
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::RequestOpened {
                thread_id: ThreadId(child.clone()),
                turn_id: TurnId("child-turn".into()),
                request_id: RequestId("req-1".into()),
                request_kind: "tool".into(),
                payload: json!({}),
                tool_use_id: None,
                subagent_id: None,
            },
        );
        assert_eq!(
            h.card(&child).activity.as_deref(),
            Some("Waiting for your answer in its workspace")
        );
        for _ in 0..3 {
            sweep_pass(&h.handle).await;
        }
        assert_eq!(h.card(&child).status, SubagentStatus::Running);
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::RequestResolved {
                thread_id: ThreadId(child.clone()),
                request_id: RequestId("req-1".into()),
                decision: ApprovalDecision::Allow {
                    updated_input: None,
                    updated_permissions: None,
                },
            },
        );
        assert_eq!(
            h.card(&child).activity.as_deref(),
            Some("Working in its workspace")
        );

        // A native async question, answered (here: dismissed) in its tab.
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::QuestionsAsked {
                thread_id: ThreadId(child.clone()),
                question: UserQuestionSet {
                    id: "q".into(),
                    target: "native".into(),
                    source_item_id: "q".into(),
                    source_turn_id: "child-turn".into(),
                    text: String::new(),
                    questions: vec![UserQuestion {
                        title: "Which storage?".into(),
                        options: vec![],
                    }],
                    subagent_id: None,
                },
            },
        );
        assert_eq!(
            h.card(&child).activity.as_deref(),
            Some("Waiting for your answer in its workspace")
        );
        for _ in 0..3 {
            sweep_pass(&h.handle).await;
        }
        assert_eq!(h.card(&child).status, SubagentStatus::Running);
        agent_chat_answer_question(
            h.handle.clone(),
            ThreadId(child.clone()),
            "q".into(),
            QuestionAction::Dismiss,
        )
        .await
        .unwrap();
        assert_eq!(
            h.card(&child).activity.as_deref(),
            Some("Working in its workspace")
        );

        // One idle pass is not enough; the second finalizes, failing closed.
        sweep_pass(&h.handle).await;
        assert_eq!(h.card(&child).status, SubagentStatus::Running);
        assert!(h.wakes().is_empty());
        sweep_pass(&h.handle).await;
        let card = h.card(&child);
        assert_eq!(card.status, SubagentStatus::Failed);
        assert_eq!(
            card.result_text.as_deref(),
            Some("It went idle without a final result; its last message: Halfway through the parser.")
        );
        let wakes = h.wakes();
        assert_eq!(wakes.len(), 1, "the sweep delivers too");
        assert!(wakes[0].contains("Status: Failed"));
        assert!(wakes[0].contains("its last message: Halfway through the parser."));
    }

    #[tokio::test]
    async fn missing_parent_pane_holds_result_without_implicit_stop() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Review parser"))
            .await;
        h.codex.set_turn_active(&child, true);
        h.app_state().set_agent_chat_thread_id(&h.parent_pane, None);
        sweep_pass(&h.handle).await;
        sweep_pass(&h.handle).await;
        assert_eq!(h.card(&child).status, SubagentStatus::Running);
        assert!(!h.interrupted(ProviderKind::Codex, &child));
        h.codex.set_turn_active(&child, false);
        h.child_reports(&child, "Retained result for an unbound parent");
        h.settle(&child).await;
        let task = h.db().delegation_task_for_child(&child).unwrap().unwrap();
        assert_eq!(
            task.result.unwrap().text,
            "Retained result for an unbound parent"
        );
        assert_eq!(task.completion_delivery, "held");
        assert!(h.wakes().is_empty());
    }

    // ── Usage limits ──

    #[tokio::test]
    async fn a_usage_limited_child_pauses_then_reports_and_leaves_no_resume() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        let reset = now_ms() + 3_600_000;
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::UsageLimitReached {
                thread_id: ThreadId(child.clone()),
                provider: ProviderKind::Codex,
                resets_at_ms: Some(reset),
                auto_resume_at_ms: None,
                auto_resume_blocked_reason: None,
                window: None,
            },
        );
        h.turn_ends(
            &child,
            TurnStatus::Error {
                subtype: "rate_limit".into(),
                message: "usage limit".into(),
            },
        );
        let db = h.db();
        let armed = db
            .get_agent_chat_usage_resume(&child)
            .expect("auto-resume is on by default");
        assert!(armed.resume_at_ms.is_some());

        // Paused, not finished: neither settling nor the sweep ends it.
        h.settle(&child).await;
        sweep_pass(&h.handle).await;
        sweep_pass(&h.handle).await;
        let card = h.card(&child);
        assert_eq!(card.status, SubagentStatus::Running);
        assert!(card
            .activity
            .as_deref()
            .is_some_and(|line| line.starts_with("Paused: Codex usage limit · resumes ")));
        assert!(h.wakes().is_empty());

        // The scheduler fires the resume, and the child finishes this time.
        assert!(db.mark_agent_chat_usage_resume_fired(&armed).unwrap());
        send_turn_with_origin(
            h.handle.clone(),
            ProviderKind::Codex,
            queued(
                &child,
                "[Resumed automatically after a provider usage limit reset.]",
            ),
            TurnOrigin::UsageResume,
        )
        .await
        .unwrap();
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ThreadId(child.clone()),
                status: SessionStatus::Running {
                    active_turn: TurnId("resumed-turn".into()),
                },
            },
        );
        assert_eq!(
            h.card(&child).activity.as_deref(),
            Some("Working in its workspace")
        );
        h.child_reports(&child, "Finished after the reset.");
        h.settle(&child).await;

        assert_eq!(h.card(&child).status, SubagentStatus::Completed);
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains("Finished after the reset."));
        assert!(
            db.get_agent_chat_usage_resume(&child).is_none(),
            "nothing left to restart the child later"
        );
    }

    #[tokio::test]
    async fn resume_now_on_a_paused_child_keeps_the_task_running_while_it_dispatches() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::UsageLimitReached {
                thread_id: ThreadId(child.clone()),
                provider: ProviderKind::Codex,
                resets_at_ms: Some(now_ms() + 3_600_000),
                auto_resume_at_ms: None,
                auto_resume_blocked_reason: None,
                window: None,
            },
        );
        h.turn_ends(
            &child,
            TurnStatus::Error {
                subtype: "rate_limit".into(),
                message: "usage limit".into(),
            },
        );
        assert!(h.db().get_agent_chat_usage_resume(&child).is_some());

        // "Resume now" drops the resume and announces it before its send
        // reaches the provider; the settle that announcement schedules must
        // not read the paused turn as the child's last word.
        let (entered, release) = h.codex.hold_next_send();
        let resume = tokio::spawn(agent_chat_resume_after_usage_limit(
            h.handle.clone(),
            ProviderKind::Codex,
            ThreadId(child.clone()),
        ));
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("the resume to reach the provider");
        assert!(h.db().get_agent_chat_usage_resume(&child).is_none());
        h.settle(&child).await;
        sweep_pass(&h.handle).await;
        sweep_pass(&h.handle).await;
        assert_eq!(h.card(&child).status, SubagentStatus::Running);
        assert!(h.wakes().is_empty());

        // The provider has the turn, so it reads as running from here.
        h.codex.set_turn_active(&child, true);
        release.notify_one();
        resume.await.unwrap().unwrap();
        h.child_reports(&child, "Finished after resuming.");
        h.codex.set_turn_active(&child, false);
        h.settle(&child).await;

        assert_eq!(h.card(&child).status, SubagentStatus::Completed);
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains("Finished after resuming."));
    }

    #[tokio::test]
    async fn a_usage_limit_with_no_resume_armed_fails_the_task_at_once() {
        let h = Harness::new().await;
        h.db()
            .set_setting("agents.auto_resume_usage_limit", "false")
            .unwrap();
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::UsageLimitReached {
                thread_id: ThreadId(child.clone()),
                provider: ProviderKind::Codex,
                resets_at_ms: Some(now_ms() + 3_600_000),
                auto_resume_at_ms: None,
                auto_resume_blocked_reason: None,
                window: None,
            },
        );

        let card = h.card(&child);
        assert_eq!(card.status, SubagentStatus::Failed);
        assert!(card
            .result_text
            .as_deref()
            .is_some_and(|reason| reason.starts_with("usage limit reached (resets ")));
        wait_for("the failure to post at once", || h.wakes().len() == 1).await;
        assert!(h.wakes()[0].contains("Status: Failed"));
        assert!(h.db().get_agent_chat_usage_resume(&child).is_none());
    }

    // ── Relaunch ──

    #[tokio::test]
    async fn a_relaunch_marks_open_tasks_stopped_and_starts_nothing() {
        let h = Harness::new().await;
        let child = unique("child-left-open");
        let created_at_ms = now_ms() - 60_000;
        let db = h.db();
        // What the last run left: the child's chat row and its journal entry.
        db.upsert_agent_chat_session(&child, &h.workspace, Some(&h.cwd), "codex")
            .unwrap();
        db.update_agent_chat_session_config(
            &child,
            &AgentChatSessionConfig {
                model: Some(Some("gpt-6.1-codex".into())),
                effort: Some(Some("high".into())),
                ..AgentChatSessionConfig::default()
            },
        )
        .unwrap();
        db.insert_delegated_open_task(&DelegatedOpenTask {
            child_thread_id: child.clone(),
            parent_thread_id: h.parent.clone(),
            provider: "codex".into(),
            title: "Add slugify helper".into(),
            created_at_ms,
        })
        .unwrap();
        // A resume that came due while Codemux was closed.
        db.upsert_agent_chat_usage_resume(&child, "codex", Some(now_ms() - 1_000))
            .unwrap();

        drain_journal(&h.handle);

        let cards = h.cards(&child);
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        assert_eq!(card.status, SubagentStatus::Stopped);
        assert_eq!(
            card.result_text.as_deref(),
            Some("Codemux closed before this task finished.")
        );
        assert_eq!(card.name.as_deref(), Some("Codex"));
        assert_eq!(card.agent_type.as_deref(), Some("codex"));
        assert_eq!(card.description.as_deref(), Some("Add slugify helper"));
        assert_eq!(card.provider_ref.as_deref(), Some(child.as_str()));
        assert_eq!(card.model.as_deref(), Some("gpt-6.1-codex"));
        assert_eq!(card.effort.as_deref(), Some("high"));
        assert!(
            card.duration_ms.is_none(),
            "how long it ran is unknown; the time Codemux was closed is not it"
        );
        assert!(db.get_agent_chat_usage_resume(&child).is_none());
        assert_eq!(db.list_delegated_open_tasks().unwrap().len(), 1);

        // Nothing starts and nothing is posted.
        sweep_pass(&h.handle).await;
        h.deliver().await;
        assert!(h.codex.calls.snapshot().is_empty());
        assert_eq!(h.wakes().len(), 1);
        assert!(h.wakes()[0].contains("Codemux closed before this task finished."));
    }

    // ── The parent's own status ──

    /// Threads the stall watchdog would flag, however long they were silent.
    fn stall_candidates(h: &Harness) -> Vec<String> {
        let activity: State<'_, RunActivityTracker> = h.handle.state();
        activity
            .stalled(
                SystemTime::now() + Duration::from_secs(86_400),
                Duration::ZERO,
            )
            .into_iter()
            .map(|(thread, _)| thread)
            .collect()
    }

    #[tokio::test]
    async fn delegated_cards_never_touch_the_parents_status_or_stall_clock() {
        let h = Harness::new().await;
        // The parent's workspace is in the background, where a finished turn
        // would show a Review dot.
        let state = h.app_state();
        let elsewhere = tempfile::tempdir().unwrap();
        let other = state.create_workspace_at_path(elsewhere.path().to_path_buf());
        state.activate_workspace(&other.0);
        let status = || state.snapshot().pane_statuses.get(&h.parent_pane).cloned();
        let tracker: State<'_, SubagentTracker> = h.handle.state();

        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        assert_eq!(
            status(),
            None,
            "a running task does not mark the parent Working"
        );
        assert!(!stall_candidates(&h).contains(&h.parent));
        assert!(!tracker.delegated_work_holding_turn(&h.parent));

        h.claude.set_turn_active(&h.parent, true);
        h.child_reports(&child, "Done.");
        h.settle(&child).await;
        assert_eq!(h.card(&child).status, SubagentStatus::Completed);
        assert_eq!(
            status(),
            None,
            "a finished task does not mark the parent Review"
        );
        assert!(!stall_candidates(&h).contains(&h.parent));
        assert!(!tracker.delegated_work_holding_turn(&h.parent));

        // The same snapshot from a native subagent does count, so the checks
        // above would see a missing guard.
        forward_event(
            &h.handle,
            ProviderRuntimeEvent::SubagentUpdated {
                thread_id: ThreadId(h.parent.clone()),
                subagent: SubagentSnapshot {
                    subagent_id: "native-explore".into(),
                    status: SubagentStatus::Running,
                    ..SubagentSnapshot::default()
                },
            },
        );
        assert!(stall_candidates(&h).contains(&h.parent));
    }

    // ── Gating and refusals ──

    #[tokio::test]
    async fn refusals_name_the_reason_and_create_nothing() {
        let h = Harness::with_install_check(|kind| kind != ProviderKind::Cursor).await;
        let tabs_before = h
            .app_state()
            .find_workspace(&h.workspace)
            .unwrap()
            .tabs
            .len();
        let workspaces_before = h.app_state().snapshot().workspaces.len();

        // Explicit escalation is rejected even though tools are visible in Plan.
        let mut escalation=args("codex", "Add slugify helper");escalation["permission_mode"]=json!("danger-full-access");
        assert!(h.call(Some("plan"),escalation).unwrap_err().contains("permission_ceiling"));
        assert_eq!(
            h.call(FULL, args("cursor", "Add slugify helper"))
                .unwrap_err(),
            "Cursor is not installed on this machine."
        );
        assert!(h
            .call(FULL, args("hermes", "Add slugify helper"))
            .unwrap_err()
            .starts_with("Unknown agent \"hermes\""));
        assert_eq!(
            h.call(
                FULL,
                json!({ "provider": "codex", "title": "Fix", "task": "TODO: fix it" })
            )
            .unwrap_err(),
            "Write a complete, self-contained task (at least 20 characters)."
        );
        let homeless = HostCaller {
            control: None,
            thread_id: unique("no-pane"),
            workspace_id: None,
            ..h.caller(FULL)
        };
        assert_eq!(
            h.call_as(&homeless, args("codex", "Add slugify helper"))
                .unwrap_err(),
            "This chat has no open workspace."
        );

        // None of them created a workspace, a tab, a card, a session or a
        // journal row.
        assert_eq!(h.app_state().snapshot().workspaces.len(), workspaces_before);
        assert_eq!(
            h.app_state()
                .find_workspace(&h.workspace)
                .unwrap()
                .tabs
                .len(),
            tabs_before
        );
        assert!(h
            .db()
            .list_agent_chat_messages(&h.parent)
            .iter()
            .all(|row| !row.contains("delegate:")));
        assert!(h.codex.calls.snapshot().is_empty());
        assert!(h.db().drain_delegated_open_tasks().unwrap().is_empty());

        // With the setting off, the tool is neither listed nor served.
        // (`suggest_tasks` shares this host but starts nothing, so it is not
        // gated on the delegation setting.)
        h.db().set_setting(DELEGATION_SETTING_KEY, "false").unwrap();
        assert!(h
            .host
            .tools_for(&h.caller(FULL))
            .iter()
            .all(|tool| tool.prefixed_name != DELEGATE_TOOL));
        assert_eq!(
            h.call(FULL, args("codex", "Add slugify helper"))
                .unwrap_err(),
            "Cross-provider delegation is off (Settings → Agents → Models & behavior)."
        );
    }

    #[tokio::test]
    async fn the_tool_is_listed_in_all_modes_and_for_app_owned_children() {
        let h = Harness::with_install_check(|kind| kind != ProviderKind::Cursor).await;
        let listed = |caller: &HostCaller| {
            h.host
                .tools_for(caller)
                .into_iter()
                // The host also serves `suggest_tasks`; this test is about
                // the delegate tool alone.
                .filter(|tool| tool.prefixed_name == DELEGATE_TOOL)
                .map(|tool| tool.input_schema["properties"]["provider"]["enum"].clone())
                .collect::<Vec<_>>()
        };
        // Claude lists by the setting alone; its live mode is checked per call.
        assert_eq!(
            listed(&h.caller(Some("plan"))),
            vec![json!(["claude", "codex", "opencode", "grok"])]
        );
        // Codex decides once, from the mode its session started in.
        let codex = |mode: &str| HostCaller {
            control: None,
            provider: ProviderKind::Codex,
            permission_mode: Some(mode.into()),
            ..h.caller(None)
        };
        assert_eq!(listed(&codex("danger-full-access")).len(), 1);
        assert_eq!(listed(&codex("workspace-write")).len(),1);
        // Other providers never orchestrate.
        let cursor = HostCaller {
            control: None,
            provider: ProviderKind::Cursor,
            ..h.caller(None)
        };
        assert!(listed(&cursor).is_empty());

        // A running child neither sees the tool nor may call it.
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        let child_caller = HostCaller {
            control: None,
            thread_id: child.clone(),
            provider: ProviderKind::Codex,
            workspace_id: Some(h.workspace.clone()),
            permission_mode: Some("danger-full-access".into()),
        };
        assert_eq!(listed(&child_caller).len(),1);
        let nested=h.call_as(&child_caller,args("claude","Review the parser")).unwrap();
        assert!(nested["task_id"].is_string());
    }

    #[tokio::test]
    async fn caps_bound_running_tasks_and_each_request() {
        let h = Harness::new().await;
        let stop = |child: String| {
            agent_chat_interrupt_turn(h.handle.clone(), ProviderKind::Codex, ThreadId(child), None)
        };

        // At most 3 running per chat.
        let mut first = Vec::new();
        for n in 0..3 {
            first.push(
                h.delegate_and_dispatch(args("codex", &format!("Task {n}")))
                    .await,
            );
        }
        assert!(h.call(FULL,args("codex","Task 3")).unwrap_err().starts_with("resource_limit:"));

        // At most 6 per user message, however many have stopped meanwhile.
        for child in first {
            stop(child).await.unwrap();
        }
        let mut second = Vec::new();
        for n in 3..6 {
            second.push(
                h.delegate_and_dispatch(args("codex", &format!("Task {n}")))
                    .await,
            );
        }
        for child in second {
            stop(child).await.unwrap();
        }
        assert!(h.call(FULL,args("codex","Task 6")).unwrap_err().starts_with("resource_limit:"));

        // The user's next message starts a new round with a fresh budget;
        // a results message does not.
        send_turn_with_origin(
            h.handle.clone(),
            ProviderKind::Claude,
            queued(&h.parent, "Now try the docs as well."),
            TurnOrigin::Delegation,
        )
        .await
        .unwrap();
        assert!(h.call(FULL, args("codex", "Task 6")).is_err());
        send_turn_with_origin(
            h.handle.clone(),
            ProviderKind::Claude,
            queued(&h.parent, "Now try the docs as well."),
            TurnOrigin::User,
        )
        .await
        .unwrap();
        h.delegate_and_dispatch(args("codex", "Task 6")).await;
    }
}

async fn delegation_delivery_ready(h: &Harness, child: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while h
            .cards(child)
            .last()
            .is_none_or(|card| card.status != SubagentStatus::Running)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn delegation_delivery_ambiguous_provider_ack_barrier_holds_without_resubmit() {
    let h = Harness::new().await;
    let child = h.delegate(args("codex", "Review parser"));
    delegation_delivery_ready(&h, &child).await;
    let (entered, release) = h.claude.hold_next_ack();
    h.claude
        .fail_next_send_after_accept(ProviderError::ValidationError {
            message: "ACK lost".into(),
        });
    h.child_reports(&child, "Final stable report");
    let owned = h.handle.clone();
    let child_copy = child.clone();
    let completion = tokio::spawn(async move {
        settle(&owned, &child_copy).await;
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "result not dispatched: {:?}",
                h.db().delegation_tasks(Some(&h.parent))
            )
        });
    assert_eq!(h.sent(ProviderKind::Claude, &h.parent).len(), 1, "provider accepted before ACK barrier");
    let task = h.db().delegation_task_for_child(&child).unwrap().unwrap();
    assert_eq!(task.result.as_ref().unwrap().text, "Final stable report");
    assert!(task.delivery_id.is_some());
    release.notify_one();
    completion.await.unwrap();
    let id = task.delivery_id.unwrap();
    assert_eq!(
        h.db().message_delivery(&id).unwrap().unwrap().state,
        "held_unknown"
    );
    for _ in 0..3 {
        h.deliver().await;
    }
    assert_eq!(h.sent(ProviderKind::Claude, &h.parent).len(), 1);
    assert_eq!(
        h.db()
            .delegation_task(&task.task_id)
            .unwrap()
            .unwrap()
            .result
            .unwrap()
            .text,
        "Final stable report"
    );
}

#[tokio::test]
async fn delegation_delivery_local_retry_returns_same_task_child_and_launch() {
    let h = Harness::new().await;
    let input = json!({"provider":"codex","title":"Review parser","task":TASK,"client_request_id":"review-1"});
    let first = h.call(FULL, input.clone()).unwrap();
    let retry = h.call(FULL, input.clone()).unwrap();
    assert_eq!(first["task_id"], retry["task_id"]);
    assert_eq!(first["child_thread_id"], retry["child_thread_id"]);
    assert_ne!(first["task_id"], first["child_thread_id"]);
    let child = first["child_thread_id"].as_str().unwrap();
    delegation_delivery_ready(&h, child).await;
    assert_eq!(h.codex.start_inputs().len(), 1);
    assert_eq!(h.sent(ProviderKind::Codex, child).len(), 1);
    let mut changed = input;
    changed["task"] = json!("A materially different complete task brief");
    assert!(h.call(FULL, changed).unwrap_err().contains("conflict"));
}

#[tokio::test]
async fn delegation_delivery_blocked_question_is_persisted_while_finished_sibling_reaches_parent() {
    let h = Harness::new().await;
    let blocked = h.delegate(args("codex", "Needs clarification"));
    delegation_delivery_ready(&h, &blocked).await;
    let finished = h.delegate(args("codex", "Independent review"));
    delegation_delivery_ready(&h, &finished).await;
    forward_event(
        &h.handle,
        ProviderRuntimeEvent::RequestOpened {
            thread_id: ThreadId(blocked.clone()),
            turn_id: TurnId("question-run".into()),
            request_id: RequestId("question-1".into()),
            request_kind: "user-input".into(),
            payload: json!({"question":"Which parser?"}),
            tool_use_id: None,
            subagent_id: None,
        },
    );
    h.child_reports(&finished, "Independent final result");
    h.settle(&finished).await;
    assert_eq!(
        h.db()
            .delegation_task_for_child(&blocked)
            .unwrap()
            .unwrap()
            .pending_request
            .unwrap()
            .id,
        "question-1"
    );
    assert!(h
        .sent(ProviderKind::Claude, &h.parent)
        .iter()
        .any(|text| text.contains("Independent final result")));
    assert!(h
        .db()
        .delegation_task_for_child(&blocked)
        .unwrap()
        .unwrap()
        .result
        .is_none());
}

#[tokio::test]
async fn delegation_delivery_bounded_wait_timeout_does_not_cancel_and_status_requires_owner() {
    let h = Harness::new().await;
    let caller = h.caller(FULL);
    let args = json!({"provider":"codex","title":"Review parser","task":TASK,"mode":"wait","wait_ms":0,"client_request_id":"wait-1"});
    let reply = h
        .host
        .call_async(
            &caller,
            DELEGATE_TOOL,
            &args,
            Some("call-1"),
            Some("parent-run"),
        )
        .await
        .unwrap();
    let receipt: Value =
        serde_json::from_str(reply["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(receipt["wait_timed_out"], true);
    let child = receipt["child_thread_id"].as_str().unwrap();
    let id = receipt["task_id"].as_str().unwrap();
    delegation_delivery_ready(&h, child).await;
    assert!(!h
        .db()
        .delegation_task(id)
        .unwrap()
        .unwrap()
        .status
        .terminal());
    assert!(h
        .db()
        .acknowledge_delegation_result("not-parent", id)
        .is_err());
    h.child_reports(child, "Final after timeout");
    h.settle(child).await;
    assert_eq!(
        h.db()
            .acknowledge_delegation_result(&h.parent, id)
            .unwrap()
            .result
            .unwrap()
            .text,
        "Final after timeout"
    );
}

#[tokio::test]
async fn delegation_delivery_stop_during_provider_start_is_confirmed_without_child_send() {
    let h = Harness::new().await;
    let (entered, release) = h.codex.hold_next_start();
    let child = h.delegate(args("codex", "Review parser"));
    entered.notified().await;
    agent_chat_interrupt_turn(
        h.handle.clone(),
        ProviderKind::Claude,
        ThreadId(h.parent.clone()),
        None,
    )
    .await
    .unwrap();
    let task = h.db().delegation_task_for_child(&child).unwrap().unwrap();
    assert_eq!(task.completion_delivery, "disposed_by_stop");
    assert!(
        task.result.is_none(),
        "an unconfirmed stop cannot fabricate a terminal result"
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if h.db()
                .delegation_task(&task.task_id)
                .unwrap()
                .unwrap()
                .result
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(h.sent(ProviderKind::Codex, &child).is_empty());
    assert!(h.sent(ProviderKind::Claude, &h.parent).is_empty());
    assert_eq!(
        h.db()
            .delegation_task(&task.task_id)
            .unwrap()
            .unwrap()
            .completion_delivery,
        "disposed_by_stop"
    );
}

#[tokio::test]
async fn delegation_delivery_wait_releases_on_parent_completion_or_disconnection_without_ack() {
    for parent_completes in [true, false] {
        let h = Harness::new().await;
        let caller = h.caller(FULL);
        let input = json!({"provider":"codex","title":"Review parser","task":TASK,"mode":"wait","wait_ms":20000,"client_request_id":"wait-release"});
        let mut waiting = h.host.call_async(
            &caller,
            DELEGATE_TOOL,
            &input,
            Some("call-wait"),
            Some("parent-run"),
        );
        assert!(futures_util::poll!(&mut waiting).is_pending());
        let task = h
            .db()
            .delegation_tasks(Some(&h.parent))
            .unwrap()
            .pop()
            .unwrap();
        let child = task.child_thread_id.as_ref().unwrap();
        delegation_delivery_ready(&h, child).await;
        if parent_completes {
            h.turn_ends(&h.parent, TurnStatus::Success);
            let response = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .unwrap()
                .unwrap();
            let receipt: Value =
                serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(receipt["wait_released"], true);
            assert!(receipt["result"].is_null());
        } else {
            drop(waiting);
        }
        assert!(h
            .db()
            .delegation_task(&task.task_id)
            .unwrap()
            .unwrap()
            .wait_until
            .is_none());
        h.child_reports(child, "Final after wait release");
        h.settle(child).await;
        assert_eq!(h.sent(ProviderKind::Claude, &h.parent).len(), 1);
        assert_eq!(
            h.db()
                .delegation_task(&task.task_id)
                .unwrap()
                .unwrap()
                .completion_delivery,
            "delivered"
        );
    }
}

#[tokio::test]
async fn delegation_delivery_legacy_terminal_import_reuses_receipt_or_holds_unknown() {
    for prior_receipt in [true, false] {
        let h = Harness::new().await;
        let child = unique("legacy-terminal");
        h.db()
            .upsert_agent_chat_session(&child, &h.workspace, Some(&h.cwd), "codex")
            .unwrap();
        h.db()
            .insert_delegated_open_task(&DelegatedOpenTask {
                child_thread_id: child.clone(),
                parent_thread_id: h.parent.clone(),
                provider: "codex".into(),
                title: "Legacy review".into(),
                created_at_ms: 1,
            })
            .unwrap();
        h.child_reports(&child, "Durable legacy report");
        if prior_receipt {
            let payload = json!({"origin":"delegation","provenance":{"notification":{"source":{"kind":"delegated_task","task_ids":[child]},"outcome":"completed"}}});
            h.db()
                .admit_message_delivery(
                    "legacy-receipt",
                    &h.parent,
                    "legacy-message",
                    &payload,
                    "prior-boot",
                    0,
                    None,
                    false,
                )
                .unwrap();
            h.db()
                .finish_message_delivery("legacy-receipt", "accepted", None, None)
                .unwrap();
        }
        drain_journal(&h.handle);
        let task = h.db().delegation_task_for_child(&child).unwrap().unwrap();
        assert_eq!(task.result.unwrap().text, "Durable legacy report");
        assert_eq!(
            task.completion_delivery,
            if prior_receipt {
                "delivered"
            } else {
                "held_unknown"
            }
        );
        assert_eq!(
            task.delivery_id.as_deref(),
            if prior_receipt {
                Some("legacy-receipt")
            } else {
                None
            }
        );
        h.deliver().await;
        assert!(
            h.sent(ProviderKind::Claude, &h.parent).is_empty(),
            "legacy import must not manufacture a second parent submission"
        );
        assert!(h.db().pending_message_deliveries().unwrap().is_empty());
    }
}

#[tokio::test]
async fn delegation_delivery_parent_stop_targets_later_run_current_provider() {
    let h = Harness::new().await;
    let child = h.delegate(args("codex", "Review parser"));
    delegation_delivery_ready(&h, &child).await;
    h.child_reports(&child, "Original immutable result");
    h.settle(&child).await;
    h.db()
        .upsert_agent_chat_session(&child, &h.workspace, Some(&h.cwd), "claude")
        .unwrap();
    forward_event(
        &h.handle,
        ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ThreadId(child.clone()),
            status: SessionStatus::Running {
                active_turn: TurnId("later-claude-run".into()),
            },
        },
    );
    agent_chat_interrupt_turn(
        h.handle.clone(),
        ProviderKind::Claude,
        ThreadId(h.parent.clone()),
        None,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !h.interrupted(ProviderKind::Claude, &child) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!h.interrupted(ProviderKind::Codex, &child));
    let task = h.db().delegation_task_for_child(&child).unwrap().unwrap();
    assert_eq!(
        task.provider, "codex",
        "original resolved selection is retained"
    );
    assert_eq!(task.result.unwrap().text, "Original immutable result");
    assert_eq!(task.completion_delivery, "disposed_by_stop");
}

#[tokio::test]
async fn delegation_delivery_wait_result_inside_window_has_no_automatic_delivery() {
    let h = Harness::new().await;
    let caller = h.caller(FULL);
    let input = json!({"provider":"codex","title":"Wait review","task":TASK,"mode":"wait","wait_ms":20000,"client_request_id":"wait-inside"});
    let mut waiting = h.host.call_async(&caller, DELEGATE_TOOL, &input, Some("wait-call"), None);
    assert!(futures_util::poll!(&mut waiting).is_pending());
    let task = h.db().delegation_tasks(Some(&h.parent)).unwrap().pop().unwrap();
    let child = task.child_thread_id.unwrap();
    delegation_delivery_ready(&h, &child).await;
    h.child_reports(&child, "Inside wait window");
    h.settle(&child).await;
    let result = tokio::time::timeout(Duration::from_secs(2), waiting).await.unwrap().unwrap();
    let receipt: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(receipt["result"]["text"], "Inside wait window");
    h.deliver().await;
    assert!(h.wakes().is_empty());
    assert_eq!(h.db().delegation_task(&task.task_id).unwrap().unwrap().completion_delivery, "acknowledged");
}

#[tokio::test]
async fn delegation_delivery_machine_turn_can_delegate_after_stop_without_human_release() {
    let h = Harness::new().await;
    agent_chat_interrupt_turn(h.handle.clone(), ProviderKind::Claude, ThreadId(h.parent.clone()), None).await.unwrap();
    let child = h.delegate_and_dispatch(args("codex", "New machine delegation")).await;
    h.child_reports(&child, "Fresh task result"); h.settle(&child).await;
    h.wait_for_wakes(1).await;
    assert_ne!(h.db().delegation_task_for_child(&child).unwrap().unwrap().completion_delivery, "disposed_by_stop");
}

#[tokio::test]
async fn delegation_delivery_agent_cancel_has_distinct_result_wording() {
    let h = Harness::new().await;
    let child = h.delegate_and_dispatch(args("codex", "Cancel review")).await;
    let task = h.db().delegation_task_for_child(&child).unwrap().unwrap();
    h.host.call(&h.caller(FULL), "mcp__codemux__task_cancel", &json!({"task_id":task.task_id})).unwrap();
    wait_for("agent cancellation result", || h.db().delegation_task(&task.task_id).unwrap().unwrap().result.is_some()).await;
    let text = h.db().delegation_task(&task.task_id).unwrap().unwrap().result.unwrap().text;
    assert!(text.contains("parent agent cancelled"), "{text}");
    assert!(!text.contains("user stopped"));
}

#[tokio::test]
async fn delegation_delivery_wait_uses_one_deadline_including_admission_work() {
    static INSTALL_ENTERED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    let h = Harness::with_install_check(|_| {
        INSTALL_ENTERED.store(now_ms(), std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(80));
        true
    }).await;
    let input = json!({"provider":"codex","title":"Bounded wait","task":TASK,"mode":"wait","wait_ms":20});
    let receipt = h.call(FULL, input).unwrap();
    let task = h.db().delegation_task(receipt["task_id"].as_str().unwrap()).unwrap().unwrap();
    assert!(task.wait_until.unwrap() <= INSTALL_ENTERED.load(std::sync::atomic::Ordering::SeqCst) + 20, "admission must not extend the wait budget");
    assert_eq!(receipt["wait_until"].as_i64(), task.wait_until);
}

#[tokio::test]
async fn delegation_delivery_round2_async_task_retry_can_wait_for_the_existing_result() {
    let h = Harness::new().await;
    let mut input = json!({"provider":"codex","title":"Async then wait","task":TASK,"client_request_id":"async-wait"});
    let first = h.call(FULL, input.clone()).unwrap();
    let child = first["child_thread_id"].as_str().unwrap().to_string();
    delegation_delivery_ready(&h, &child).await;
    let caller = h.caller(FULL);
    input["mode"] = json!("wait"); input["wait_ms"] = json!(2000);
    let mut waiting = h.host.call_async(&caller, DELEGATE_TOOL, &input, Some("retry"), None);
    assert!(futures_util::poll!(&mut waiting).is_pending(), "wait retry must wait rather than conflict/release immediately");
    assert_eq!(h.db().delegation_tasks(Some(&h.parent)).unwrap().len(), 1);
    h.child_reports(&child, "Existing task finished"); h.settle(&child).await;
    let result = tokio::time::timeout(Duration::from_secs(2), waiting).await.unwrap().unwrap();
    let receipt: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(receipt["task_id"], first["task_id"]);
    assert_eq!(receipt["result"]["text"], "Existing task finished");
    assert_ne!(receipt["wait_released"], true);
    assert!(h.wakes().is_empty());
}

#[tokio::test]
async fn delegation_delivery_round2_released_wait_retry_gets_a_bounded_window() {
    let h = Harness::new().await;
    let input = json!({"provider":"codex","title":"Wait again","task":TASK,"client_request_id":"wait-again","mode":"wait","wait_ms":40});
    let caller = h.caller(FULL);
    let first = h.host.call_async(&caller, DELEGATE_TOOL, &input, Some("first"), None).await.unwrap();
    let first: Value = serde_json::from_str(first["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(first["wait_timed_out"], true);
    let start = Instant::now();
    let retry = h.host.call_async(&caller, DELEGATE_TOOL, &input, Some("retry"), None).await.unwrap();
    let retry: Value = serde_json::from_str(retry["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(retry["task_id"], first["task_id"]);
    assert_eq!(retry["wait_timed_out"], true);
    assert_ne!(retry["wait_released"], true);
    assert!(start.elapsed() >= Duration::from_millis(25));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn delegation_delivery_round2_new_chat_and_pane_rebind_cancel_parent_ssh_tasks() {
    use codemux_lib::delegation::{journal::Journal, DelegateTaskInput, Grant, Parent, WakeState};
    for new_chat in [true, false] {
        let h = Harness::new().await;
        let dir = tempfile::tempdir().unwrap(); let journal = Arc::new(Journal::open(dir.path()).unwrap());
        let parent = Parent { thread_id:h.parent.clone(), workspace_id:h.workspace.clone(), provider:ProviderKind::Claude, label:"Parent".into(), permission_mode:"bypassPermissions".into(), event_id:None };
        let grant: Grant = serde_json::from_value(json!({"id":"grant","host_id":4,"host_name":"Fixture host","ssh_target":"fixture-host","workspace_path":"/fixture/checkout","workspace_name":"Checkout","provider":"codex","permission_mode":"danger-full-access","enabled":true,"created_at":"fixture"})).unwrap();
        journal.put_grant(&h.workspace, &grant).unwrap();
        let input = DelegateTaskInput { target_id:grant.id.clone(), prompt:"Existing SSH task".into(), title:None, model:None, effort:None, client_request_id:"ssh-close".into() };
        let task = journal.admit(&parent, &grant, "checkout", &input).unwrap();
        h.handle.manage(journal.clone());
        if new_chat {
            agent_chat_stop_session(h.handle.clone(), ProviderKind::Claude, ThreadId(h.parent.clone())).await.unwrap();
        } else {
            codemux_lib::commands::delegation::on_thread_stopped(&h.handle, &h.parent, codemux_lib::commands::delegation::StopKind::Close);
        }
        let stopped = journal.task(&task.id).unwrap();
        assert!(stopped.cancel_requested, "New Chat/rebind must cancel its SSH tasks ({new_chat})");
        assert_eq!(stopped.wake_state, WakeState::Suppressed);
        assert_eq!(h.db().delivery_generation(&h.parent).unwrap(), (0, false), "closing must preserve local lifecycle semantics");
    }
}
