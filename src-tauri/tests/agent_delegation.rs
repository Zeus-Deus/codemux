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
    AgentChatChannelRegistry,
    GrokUsageLedgerBridge, ProviderRegistry, RunActivityTracker, SendTurnCommandInput,
    SubagentTracker, TurnOrigin,
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
const CHILD_FOOTER: &str = "\n\n---\nFrom Codemux: another coding agent handed you this task and will read your final message. Work on your own: do not ask questions or wait for input; if something is unclear, pick the safest reasonable option and say so. End with a short report: what you changed (files), how you checked it, and anything left undone.";
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
        additional_directories: vec![],
        recorded_usage_baseline: None,
        env: None,
        workspace_id: None,
        extra: Value::Null,
    }
}

fn queued(thread_id: &str, text: &str) -> SendTurnCommandInput {
    SendTurnCommandInput {
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
    async fn delegating_opens_a_background_tab_beside_the_parent_and_sends_the_exact_task() {
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
            })
        );
        assert!(started["note"]
            .as_str()
            .unwrap()
            .starts_with("Codex is working in a background tab."));

        // Its own tab, right after the parent's, titled; focus stays put.
        let after = state.find_workspace(&h.workspace).unwrap();
        assert_eq!(state.snapshot().active_workspace_id, active_workspace);
        assert_eq!(after.active_tab_id, before.active_tab_id);
        assert_eq!(after.active_surface_id, before.active_surface_id);
        assert_eq!(after.tabs.len(), before.tabs.len() + 1);
        let child_tab = tab_index(&state, &after, &child);
        assert_eq!(child_tab, tab_index(&state, &after, &h.parent) + 1);
        assert_eq!(after.tabs[child_tab].title, "Codex · Add slugify helper");
        let child_pane = state.agent_chat_pane_id_for_thread(&child).unwrap();
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
        assert_eq!(input.workspace_id.as_deref(), Some(h.workspace.as_str()));

        // An ordinary chat row: its title, full mode and the exact task text.
        let db = h.db();
        let record = db.get_agent_chat_session(&child).unwrap();
        assert_eq!(record.provider, "codex");
        assert_eq!(record.workspace_id, h.workspace);
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
        assert_eq!(user_rows[0]["text"], expected_text);

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
        assert_eq!(running.activity.as_deref(), Some("Working in its tab"));
        assert!(running.result_text.is_none());
        assert!(running.duration_ms.is_none());

        // Tracked as a delegated child (its own finish pushes are muted),
        // journaled for a relaunch, and nothing posted to the parent yet.
        assert!(is_delegated_child(&h.handle, &child));
        assert!(!is_delegated_child(&h.handle, &h.parent));
        assert!(h.wakes().is_empty());
        let journal = db.drain_delegated_open_tasks().unwrap();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].child_thread_id, child);
        assert_eq!(journal[0].parent_thread_id, h.parent);
        assert_eq!(journal[0].provider, "codex");
        assert_eq!(journal[0].title, "Add slugify helper");
    }

    #[tokio::test]
    async fn a_finished_task_wakes_the_idle_parent_exactly_once() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        let report =
            "Added slugify and 9 table tests.\nChecked: npm run test -- src/lib/strings.test.ts.";

        // No direct settle here: the child's turn end is the trigger.
        h.child_reports(&child, report);
        wait_for("the results message", || h.wakes().len() == 1).await;

        let wake = &h.wakes()[0];
        assert!(wake.starts_with(&format!("{DELEGATION_RESULTS_PREFIX}\n")));
        assert!(wake.contains(&format!(
            "<task provider=\"codex\" model=\"default\" status=\"completed\" title=\"Add slugify helper\" duration=\""
        )));
        assert!(wake.contains(&format!("thread=\"{child}\">")));
        assert!(!wake.contains("effort="), "no effort was asked for");
        assert!(wake.contains(&format!("<brief>{TASK}</brief>")));
        assert!(wake.contains(&format!("<report>\n{report}\n</report>")));
        assert!(!wake.contains("A task failed"));
        assert!(wake.ends_with("No delegated tasks are still running."));

        // Posted as the parent's next user message, exactly as the model reads it.
        wait_for("the results message in the transcript", || {
            !h.parent_user_messages().is_empty()
        })
        .await;
        assert_eq!(h.parent_user_messages(), vec![wake.clone()]);

        let card = h.card(&child);
        assert_eq!(card.status, SubagentStatus::Completed);
        assert_eq!(card.activity.as_deref(), Some("Finished"));
        assert_eq!(card.result_text.as_deref(), Some(report));
        assert!(card.duration_ms.is_some());

        // Every later trigger finds nothing left to post.
        h.settle(&child).await;
        h.deliver().await;
        sweep_pass(&h.handle).await;
        h.turn_ends(&h.parent, TurnStatus::Success);
        h.deliver().await;
        assert_eq!(h.wakes().len(), 1);
        assert_eq!(h.parent_user_messages().len(), 1);
        assert!(h.db().drain_delegated_open_tasks().unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_results_post_while_the_parent_is_busy_paused_or_without_a_session() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;

        // The parent is mid-turn when the child finishes.
        h.claude.set_turn_active(&h.parent, true);
        h.child_reports(&child, "Done: slugify added.");
        h.settle(&child).await;
        assert_eq!(h.card(&child).status, SubagentStatus::Completed);
        assert!(h.wakes().is_empty(), "never into an active turn");
        h.claude.set_turn_active(&h.parent, false);

        // The parent itself waits on a usage-limit resume.
        let db = h.db();
        db.upsert_agent_chat_usage_resume(&h.parent, "claude", Some(now_ms() + 3_600_000))
            .unwrap();
        h.deliver().await;
        sweep_pass(&h.handle).await;
        assert!(h.wakes().is_empty(), "not while a usage resume is armed");
        db.delete_agent_chat_usage_resume(&h.parent).unwrap();

        // The parent has no live session.
        h.claude
            .stop_session(ThreadId(h.parent.clone()))
            .await
            .unwrap();
        h.deliver().await;
        sweep_pass(&h.handle).await;
        assert!(h.wakes().is_empty(), "not without a live session");
        h.claude
            .start_session(start_input(&h.parent, &h.cwd))
            .await
            .unwrap();

        // The parent's own turn end is a delivery trigger.
        h.turn_ends(&h.parent, TurnStatus::Success);
        wait_for("the report after the parent's turn ends", || {
            h.wakes().len() == 1
        })
        .await;
        assert!(h.wakes()[0].contains("status=\"completed\""));
    }

    #[tokio::test]
    async fn a_round_of_two_posts_one_message() {
        let h = Harness::new().await;
        let codex = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        let claude = h
            .delegate_and_dispatch(json!({
                "provider": "claude",
                "title": "Review the parser",
                "task": "Review src/lib/parser.ts for edge cases and report findings with file:line references.",
                "model": "claude-opus-4-7",
                "effort": "max",
            }))
            .await;

        // Their tabs follow the parent's in the order they were started.
        let state = h.app_state();
        let workspace = state.find_workspace(&h.workspace).unwrap();
        let parent_tab = tab_index(&state, &workspace, &h.parent);
        assert_eq!(tab_index(&state, &workspace, &codex), parent_tab + 1);
        assert_eq!(tab_index(&state, &workspace, &claude), parent_tab + 2);

        // Same-provider children are fine; Claude's full mode is its own.
        let claude_start = h
            .claude
            .start_inputs()
            .into_iter()
            .find(|input| input.thread_id.0 == claude)
            .expect("the Claude child started");
        assert_eq!(
            claude_start.permission_mode.as_deref(),
            Some("bypassPermissions")
        );
        assert_eq!(claude_start.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(claude_start.effort.as_deref(), Some("max"));

        h.child_reports(&codex, "Codex report: slugify added.");
        h.settle(&codex).await;
        assert_eq!(h.card(&codex).status, SubagentStatus::Completed);
        assert!(h.wakes().is_empty(), "a finished task waits for its round");

        h.child_reports(&claude, "Claude report: two edge cases found.");
        h.settle(&claude).await;
        let wakes = h.wait_for_wakes(1).await;
        let wake = &wakes[0];
        assert_eq!(wake.matches("<task ").count(), 2);
        assert!(wake.contains("Codex report: slugify added."));
        assert!(wake.contains(
            "<task provider=\"claude\" model=\"claude-opus-4-7\" effort=\"max\" status=\"completed\" title=\"Review the parser\""
        ));
        assert!(wake.contains("Claude report: two edge cases found."));
        assert!(wake.ends_with("No delegated tasks are still running."));
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
        assert!(wake.contains("status=\"failed\" title=\"Add slugify helper\""));
        assert!(
            wake.contains("It could not start: not signed in: Run `codex login` and try again.")
        );
        assert!(wake.contains("A task failed: do not retry it on your own"));
        assert!(wake.ends_with(
            "Still running: \"Review the parser\" (claude). Its report will arrive the same way."
        ));
        assert_eq!(wake.matches("<task ").count(), 1);

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
        assert!(wakes[1].contains("status=\"failed\" title=\"Review the parser\""));
        assert!(wakes[1].contains("<report>\nInvalid API key · Please run /login\n</report>"));
        assert!(!wakes[1].contains("Add slugify helper"), "posted once only");
        assert!(wakes[1].ends_with("No delegated tasks are still running."));
    }

    // ── Stop, close and restart ──

    #[tokio::test]
    async fn stopping_the_parent_stops_its_tasks_and_posts_nothing() {
        let h = Harness::new().await;
        let codex = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        let claude = h
            .delegate_and_dispatch(args("claude", "Review the parser"))
            .await;
        h.child_reports(&codex, "Codex report.");
        h.settle(&codex).await;
        assert_eq!(h.card(&codex).status, SubagentStatus::Completed);
        // A usage resume armed on the running child must not outlive the Stop.
        h.db()
            .upsert_agent_chat_usage_resume(&claude, "claude", Some(now_ms() + 3_600_000))
            .unwrap();

        agent_chat_interrupt_turn(
            h.handle.clone(),
            ProviderKind::Claude,
            ThreadId(h.parent.clone()),
            None,
        )
        .await
        .unwrap();

        let card = h.card(&claude);
        assert_eq!(card.status, SubagentStatus::Stopped);
        assert_eq!(card.result_text.as_deref(), Some(STOPPED_WITH_PARENT));
        assert!(card.duration_ms.is_some());
        assert_eq!(h.card(&codex).status, SubagentStatus::Completed);
        assert!(h.interrupted(ProviderKind::Claude, &h.parent));
        wait_for("the running child to be interrupted", || {
            h.interrupted(ProviderKind::Claude, &claude)
        })
        .await;
        assert!(h.db().get_agent_chat_usage_resume(&claude).is_none());
        assert!(
            !h.interrupted(ProviderKind::Codex, &codex),
            "a finished child is left alone"
        );

        // The finished report is dropped along with the rest: Stop is quiet.
        h.deliver().await;
        sweep_pass(&h.handle).await;
        h.turn_ends(&h.parent, TurnStatus::Success);
        h.deliver().await;
        assert!(h.wakes().is_empty());
    }

    #[tokio::test]
    async fn stopping_one_task_rides_along_without_a_message_of_its_own() {
        let h = Harness::new().await;
        let codex = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        let claude = h
            .delegate_and_dispatch(args("claude", "Review the parser"))
            .await;

        // The strip's Stop on one row.
        agent_chat_interrupt_turn(
            h.handle.clone(),
            ProviderKind::Codex,
            ThreadId(codex.clone()),
            None,
        )
        .await
        .unwrap();
        let card = h.card(&codex);
        assert_eq!(card.status, SubagentStatus::Stopped);
        assert_eq!(card.result_text.as_deref(), Some(STOPPED_BY_USER));
        assert!(h.interrupted(ProviderKind::Codex, &codex));
        assert!(
            !is_delegated_child(&h.handle, &codex),
            "its tab is the user's again, so its own pushes are not muted"
        );
        assert!(is_delegated_child(&h.handle, &claude));
        h.deliver().await;
        assert!(h.wakes().is_empty(), "a stopped task posts nothing alone");
        assert_eq!(h.card(&claude).status, SubagentStatus::Running);

        h.child_reports(&claude, "Claude report.");
        h.settle(&claude).await;
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains("status=\"completed\" title=\"Review the parser\""));
        assert!(wakes[0].contains("status=\"stopped\" title=\"Add slugify helper\""));
        assert!(wakes[0].contains(&format!("{STOPPED_BY_USER}\n</task>")));
    }

    #[tokio::test]
    async fn restarting_a_child_in_its_own_tab_stops_its_task_quietly() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;

        agent_chat_stop_session(
            h.handle.clone(),
            ProviderKind::Codex,
            ThreadId(child.clone()),
        )
        .await
        .unwrap();

        let card = h.card(&child);
        assert_eq!(card.status, SubagentStatus::Stopped);
        assert_eq!(
            card.result_text.as_deref(),
            Some("Its chat was restarted or replaced in its tab before it finished; partial work is in the tab.")
        );
        assert!(h
            .codex
            .calls
            .snapshot()
            .contains(&MockCall::StopSession(ThreadId(child.clone()))));

        // A round of only stopped tasks posts nothing, and a late turn end
        // from the replaced session changes nothing.
        h.deliver().await;
        assert!(!is_delegated_child(&h.handle, &child));
        h.child_reports(&child, "late report");
        h.settle(&child).await;
        sweep_pass(&h.handle).await;
        assert_eq!(h.card(&child).status, SubagentStatus::Stopped);
        assert!(h.wakes().is_empty());
    }

    #[tokio::test]
    async fn closing_a_childs_tab_stops_its_task_quietly() {
        let h = Harness::new().await;
        let codex = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        let claude = h
            .delegate_and_dispatch(args("claude", "Review the parser"))
            .await;

        // What closing the child's pane, tab or workspace runs.
        shutdown_agent_chat_threads(&h.handle, vec![(ProviderKind::Codex, codex.clone())]);

        let card = h.card(&codex);
        assert_eq!(card.status, SubagentStatus::Stopped);
        assert_eq!(
            card.result_text.as_deref(),
            Some("The user closed its tab. Partial changes may be in the working tree.")
        );
        wait_for("the closed child's session to stop", || {
            h.codex
                .calls
                .snapshot()
                .contains(&MockCall::StopSession(ThreadId(codex.clone())))
        })
        .await;
        h.deliver().await;
        assert!(h.wakes().is_empty());
        assert_eq!(h.card(&claude).status, SubagentStatus::Running);

        h.child_reports(&claude, "Claude report.");
        h.settle(&claude).await;
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains("status=\"stopped\" title=\"Add slugify helper\""));
        assert!(wakes[0].contains("status=\"completed\" title=\"Review the parser\""));
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

        let card = h.card(&child);
        assert_eq!(card.status, SubagentStatus::Stopped);
        assert_eq!(card.result_text.as_deref(), Some(STOPPED_WITH_PARENT));
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
        wait_for("the card to read Stopped at once", || {
            h.card(&child).status == SubagentStatus::Stopped
        })
        .await;
        release.notify_one();
        stop.await.unwrap().unwrap();

        assert!(h.interrupted(ProviderKind::Codex, &child));
        assert!(h.sent(ProviderKind::Codex, &child).is_empty());
        assert_eq!(h.card(&child).result_text.as_deref(), Some(STOPPED_BY_USER));
        h.deliver().await;
        assert!(h.wakes().is_empty());
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
            Some("Waiting for your answer in its tab")
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
        assert!(h.codex.calls.snapshot().contains(&MockCall::RespondToRequest(
            ThreadId(child.clone()),
            RequestId("req-1".into())
        )));
        assert_eq!(
            h.card(&child).activity.as_deref(),
            Some("Working in its tab")
        );

        // A request whose answer Codemux never saw closes with the turn.
        request("req-2");
        h.child_reports(&child, "Added slugify after the approval.");
        h.settle(&child).await;
        assert_eq!(h.card(&child).status, SubagentStatus::Completed);
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains("<report>\nAdded slugify after the approval.\n</report>"));
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
            Some("Waiting for your answer in its tab")
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
        assert!(wakes[0].contains("status=\"failed\" title=\"Add slugify helper\""));

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
            Some("Its session ended before it finished.")
        );
        let wakes = h.wait_for_wakes(2).await;
        assert!(wakes[1].contains("status=\"failed\" title=\"Review the parser\""));
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
            Some("Waiting for your answer in its tab")
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
            Some("Working in its tab")
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
            Some("Waiting for your answer in its tab")
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
            Some("Working in its tab")
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
        assert!(wakes[0].contains("status=\"failed\" title=\"Add slugify helper\""));
        assert!(wakes[0].contains("its last message: Halfway through the parser."));
    }

    #[tokio::test]
    async fn the_sweep_closes_tasks_of_a_parent_that_lost_its_pane() {
        let h = Harness::new().await;
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        h.codex.set_turn_active(&child, true);

        // The pane went away without the close hook (a missed path).
        h.app_state().set_agent_chat_thread_id(&h.parent_pane, None);
        sweep_pass(&h.handle).await;
        assert_eq!(h.card(&child).status, SubagentStatus::Running);
        sweep_pass(&h.handle).await;

        let card = h.card(&child);
        assert_eq!(card.status, SubagentStatus::Stopped);
        assert_eq!(card.result_text.as_deref(), Some(STOPPED_WITH_PARENT));
        wait_for("the child to be interrupted", || {
            h.interrupted(ProviderKind::Codex, &child)
        })
        .await;
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
            Some("Working in its tab")
        );
        h.child_reports(&child, "Finished after the reset.");
        h.settle(&child).await;

        assert_eq!(h.card(&child).status, SubagentStatus::Completed);
        let wakes = h.wait_for_wakes(1).await;
        assert!(wakes[0].contains("<report>\nFinished after the reset.\n</report>"));
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
        assert!(wakes[0].contains("<report>\nFinished after resuming.\n</report>"));
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
        assert!(h.wakes()[0].contains("status=\"failed\""));
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
        assert!(db.drain_delegated_open_tasks().unwrap().is_empty());

        // Nothing starts and nothing is posted.
        sweep_pass(&h.handle).await;
        h.deliver().await;
        assert!(h.codex.calls.snapshot().is_empty());
        assert_eq!(
            h.claude.calls.snapshot(),
            vec![MockCall::StartSession(ThreadId(h.parent.clone()))]
        );
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

        // The Plan/Ask pills switch Claude's live mode to plan.
        assert_eq!(
            h.call(Some("plan"), args("codex", "Add slugify helper"))
                .unwrap_err(),
            "Delegation needs this chat in Full access; it is in Plan mode. Switch to Full access to delegate."
        );
        // Accepting a plan leaves Claude in its default (Supervised) mode.
        assert_eq!(
            h.call(Some("default"), args("codex", "Add slugify helper"))
                .unwrap_err(),
            "Delegation needs this chat in Full access; it is in Supervised. Switch to Full access to delegate."
        );
        // A Codex chat started outside Full access.
        let codex_parent = HostCaller {
            provider: ProviderKind::Codex,
            permission_mode: Some("workspace-write".into()),
            ..h.caller(None)
        };
        assert_eq!(
            h.call_as(&codex_parent, args("claude", "Review the parser"))
                .unwrap_err(),
            "Delegation needs this chat in Full access; it is in Workspace write. Switch to Full access to delegate."
        );
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
            thread_id: unique("no-pane"),
            workspace_id: None,
            ..h.caller(FULL)
        };
        assert_eq!(
            h.call_as(&homeless, args("codex", "Add slugify helper"))
                .unwrap_err(),
            "This chat has no open workspace."
        );

        // None of them created a tab, a card, a session or a journal row.
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
        h.db().set_setting(DELEGATION_SETTING_KEY, "false").unwrap();
        assert!(h.host.tools_for(&h.caller(FULL)).is_empty());
        assert_eq!(
            h.call(FULL, args("codex", "Add slugify helper"))
                .unwrap_err(),
            "Cross-provider delegation is off (Settings → Agent)."
        );
    }

    #[tokio::test]
    async fn the_tool_is_listed_for_full_access_parents_only() {
        let h = Harness::with_install_check(|kind| kind != ProviderKind::Cursor).await;
        let listed = |caller: &HostCaller| {
            h.host
                .tools_for(caller)
                .into_iter()
                .map(|tool| {
                    assert_eq!(tool.prefixed_name, DELEGATE_TOOL);
                    tool.input_schema["properties"]["provider"]["enum"].clone()
                })
                .collect::<Vec<_>>()
        };
        // Claude lists by the setting alone; its live mode is checked per call.
        assert_eq!(
            listed(&h.caller(Some("plan"))),
            vec![json!(["claude", "codex", "opencode", "grok"])]
        );
        // Codex decides once, from the mode its session started in.
        let codex = |mode: &str| HostCaller {
            provider: ProviderKind::Codex,
            permission_mode: Some(mode.into()),
            ..h.caller(None)
        };
        assert_eq!(listed(&codex("danger-full-access")).len(), 1);
        assert!(listed(&codex("workspace-write")).is_empty());
        // Other providers never orchestrate.
        let cursor = HostCaller {
            provider: ProviderKind::Cursor,
            ..h.caller(None)
        };
        assert!(listed(&cursor).is_empty());

        // A running child neither sees the tool nor may call it.
        let child = h
            .delegate_and_dispatch(args("codex", "Add slugify helper"))
            .await;
        let child_caller = HostCaller {
            thread_id: child.clone(),
            provider: ProviderKind::Codex,
            workspace_id: Some(h.workspace.clone()),
            permission_mode: Some("danger-full-access".into()),
        };
        assert!(listed(&child_caller).is_empty());
        assert_eq!(
            h.call_as(&child_caller, args("claude", "Review the parser"))
                .unwrap_err(),
            "A delegated task cannot delegate further."
        );
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
        assert_eq!(
            h.call(FULL, args("codex", "Task 3")).unwrap_err(),
            "This chat already has 3 delegated tasks running. Wait for their reports."
        );

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
        assert_eq!(
            h.call(FULL, args("codex", "Task 6")).unwrap_err(),
            "Delegation limit for this request reached (6 tasks). Ask the user before delegating more."
        );

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
