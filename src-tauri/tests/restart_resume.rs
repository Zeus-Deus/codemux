//! Integration tests for resuming chats the app stopped mid-turn.
//!
//! Each test plays one app run through `forward_event` (turn starts,
//! approvals, settles), optionally a normal quit, then the next launch's
//! pass with a `MockAgentProvider`, and checks what was sent.
//!
//! Unix-only for the same reason as `agent_chat_commands.rs`:
//! `tauri::test::mock_app()` needs WebView2Loader.dll on Windows.

#![cfg(unix)]

#[path = "helpers/mock_agent_provider.rs"]
mod mock_agent_provider;

use std::sync::Arc;
use std::time::Duration;

use tauri::{Manager, State};

use codemux_lib::agent_provider::{
    ProviderKind, ProviderRuntimeEvent, RequestId, SessionStatus, ThreadId, TurnId, TurnStatus,
};
use codemux_lib::commands::agent_chat::{
    forward_event, AgentChatChannelRegistry, GrokUsageLedgerBridge, ProviderRegistry,
    RunActivityTracker, SubagentTracker,
};
use codemux_lib::commands::restart_resume::{
    on_exit, resume_after_restart_for_test, ResumeDecision, SkipReason,
    RESTART_RESUME_NOTE_PREFIX, RESTART_RESUME_SETTING_KEY,
};
use codemux_lib::database::queued_turns::QueuedTurnRecord;
use codemux_lib::database::DatabaseStore;
use codemux_lib::observability::{FeatureFlags, ObservabilityStore};
use codemux_lib::state::AppStateStore;

use crate::mock_agent_provider::{MockAgentProvider, MockCall};

struct Harness {
    app: tauri::App<tauri::test::MockRuntime>,
    kind: &'static str,
    provider: Arc<MockAgentProvider>,
    workspace: String,
    dir: tempfile::TempDir,
}

async fn harness() -> Harness {
    harness_for(ProviderKind::Codex).await
}

async fn harness_for(kind: ProviderKind) -> Harness {
    let app = tauri::test::mock_app();
    app.manage(DatabaseStore::new_in_memory());
    app.manage(AgentChatChannelRegistry::default());
    app.manage(GrokUsageLedgerBridge::default());
    app.manage(SubagentTracker::default());
    app.manage(RunActivityTracker::default());
    app.manage(AppStateStore::default());
    let observability = ObservabilityStore::default();
    observability.set_feature_flags(FeatureFlags {
        enable_agent_chat: true,
        ..observability.feature_flags()
    });
    app.manage(observability);
    let registry = ProviderRegistry::new();
    let provider = Arc::new(MockAgentProvider::new(kind));
    match kind {
        ProviderKind::Claude => registry.set_claude(provider.clone() as _).await,
        _ => registry.set_codex(provider.clone() as _).await,
    }
    app.manage(registry);
    let dir = tempfile::tempdir().unwrap();
    let state: State<'_, AppStateStore> = app.state();
    let workspace = state.create_workspace_at_path(dir.path().into()).0;
    Harness {
        app,
        kind: if kind == ProviderKind::Claude { "claude" } else { "codex" },
        provider,
        workspace,
        dir,
    }
}

impl Harness {
    fn handle(&self) -> tauri::AppHandle<tauri::test::MockRuntime> {
        self.app.handle().clone()
    }

    fn db(&self) -> State<'_, DatabaseStore> {
        self.app.state()
    }

    /// A chat in the harness workspace with a saved conversation to reopen.
    fn chat(&self, thread: &str) {
        let state: State<'_, AppStateStore> = self.app.state();
        let pane = state
            .create_agent_chat_pane(&self.workspace, None, None, None, None)
            .unwrap();
        state.set_agent_chat_thread_id(&pane.0, Some(thread.into()));
        let db = self.db();
        db.upsert_agent_chat_session(thread, &self.workspace, self.dir.path().to_str(), self.kind)
            .unwrap();
        db.set_agent_chat_sdk_session_id(thread, &format!("sdk-{thread}"))
            .unwrap();
    }

    fn running(&self, thread: &str, turn: &str) {
        forward_event(
            &self.handle(),
            ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ThreadId(thread.into()),
                status: SessionStatus::Running {
                    active_turn: TurnId(turn.into()),
                },
            },
        );
    }

    fn completed(&self, thread: &str, status: TurnStatus) {
        forward_event(
            &self.handle(),
            ProviderRuntimeEvent::TurnCompleted {
                thread_id: ThreadId(thread.into()),
                turn_id: TurnId("t".into()),
                status,
                usage: None,
            },
        );
    }

    fn sent(&self, thread: &str) -> Vec<String> {
        self.provider
            .calls
            .snapshot()
            .into_iter()
            .filter_map(|call| match call {
                MockCall::SendTurn(t, text) if t.0 == thread => Some(text),
                _ => None,
            })
            .collect()
    }

    fn saved(&self, thread: &str, id: &str, text: &str) {
        self.db()
            .insert_queued_turn(&QueuedTurnRecord {
                thread_id: thread.into(),
                queued_id: id.into(),
                position: 0,
                text: text.into(),
                prompt: None,
                skill_ids: Vec::new(),
                skill_text: None,
                include_plugins: true,
                images: Vec::new(),
                client_nonce: Some(format!("nonce-{id}")),
                model_override: None,
                effort_override: None,
                permission_mode_override: None,
                epoch: "an-earlier-run".into(),
            })
            .unwrap();
    }
}

fn decision(decisions: &[(String, ResumeDecision)], thread: &str) -> Option<ResumeDecision> {
    decisions
        .iter()
        .find(|(t, _)| t == thread)
        .map(|(_, decision)| *decision)
}

async fn wait_for_sends(h: &Harness, thread: &str, count: usize) -> Vec<String> {
    for _ in 0..200 {
        let sent = h.sent(thread);
        if sent.len() >= count {
            return sent;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.sent(thread)
}

#[tokio::test]
async fn a_chat_running_at_a_normal_quit_resumes_once_at_the_next_launch() {
    let h = harness().await;
    h.chat("rr-quit");
    h.running("rr-quit", "turn-1");
    on_exit(&h.handle());
    assert!(h.db().get_restart_turn("rr-quit").unwrap().quit_at_ms.is_some());

    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-quit"), Some(ResumeDecision::Resume));
    let sent = h.sent("rr-quit");
    assert_eq!(sent.len(), 1);
    assert!(sent[0].starts_with(RESTART_RESUME_NOTE_PREFIX));
    assert!(sent[0].contains("Codemux quit"));
    // The provider reopened the saved conversation.
    let starts = h.provider.start_inputs();
    assert_eq!(starts[0].resume_cursor, Some(serde_json::json!({"resume": "sdk-rr-quit"})));

    // The same launch never resumes it twice.
    let again = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert!(again.is_empty());
    assert_eq!(h.sent("rr-quit").len(), 1);
}

#[tokio::test]
async fn a_chat_running_when_the_app_crashed_resumes_at_the_next_launch() {
    let h = harness().await;
    h.chat("rr-crash");
    h.running("rr-crash", "turn-1");
    // No quit: the row is still unstamped at the next launch.
    assert_eq!(h.db().get_restart_turn("rr-crash").unwrap().quit_at_ms, None);

    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-crash"), Some(ResumeDecision::Resume));
    let sent = h.sent("rr-crash");
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("stopped unexpectedly"));
}

#[tokio::test]
async fn a_settled_turn_leaves_nothing_to_resume() {
    let h = harness().await;
    h.chat("rr-settled");
    h.running("rr-settled", "turn-1");
    h.completed("rr-settled", TurnStatus::Success);
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-settled"), None);
    assert!(h.sent("rr-settled").is_empty());
}

#[tokio::test]
async fn a_chat_the_user_stopped_is_not_resumed() {
    let h = harness().await;
    h.chat("rr-stopped");
    h.running("rr-stopped", "turn-1");
    codemux_lib::commands::agent_chat::agent_chat_interrupt_turn(
        h.handle(),
        ProviderKind::Codex,
        ThreadId("rr-stopped".into()),
        None,
    )
    .await
    .unwrap();
    assert!(h.db().get_restart_turn("rr-stopped").is_none());
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-stopped"), None);
    assert!(h.sent("rr-stopped").is_empty());
}

#[tokio::test]
async fn a_chat_waiting_on_an_approval_is_resurfaced_instead() {
    let h = harness().await;
    h.chat("rr-approval");
    h.running("rr-approval", "turn-1");
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::RequestOpened {
            thread_id: ThreadId("rr-approval".into()),
            turn_id: TurnId("turn-1".into()),
            request_id: RequestId("req-1".into()),
            request_kind: "command".into(),
            payload: serde_json::json!({}),
            tool_use_id: None,
            subagent_id: None,
        },
    );
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-approval"), Some(ResumeDecision::Resurface));
    assert!(h.sent("rr-approval").is_empty());
    let state: State<'_, AppStateStore> = h.app.state();
    let pane = state.agent_chat_pane_id_for_thread("rr-approval").unwrap();
    assert_eq!(
        state.snapshot().pane_statuses.get(&pane).cloned(),
        Some(codemux_lib::state::PaneStatus::Permission)
    );
}

#[tokio::test]
async fn an_answered_approval_no_longer_blocks_the_resume() {
    let h = harness().await;
    h.chat("rr-answered");
    h.running("rr-answered", "turn-1");
    let thread_id = ThreadId("rr-answered".into());
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::RequestOpened {
            thread_id: thread_id.clone(),
            turn_id: TurnId("turn-1".into()),
            request_id: RequestId("req-1".into()),
            request_kind: "command".into(),
            payload: serde_json::json!({}),
            tool_use_id: None,
            subagent_id: None,
        },
    );
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::RequestResolved {
            thread_id,
            request_id: RequestId("req-1".into()),
            decision: codemux_lib::agent_provider::ApprovalDecision::AllowForSession,
        },
    );
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-answered"), Some(ResumeDecision::Resume));
}

#[tokio::test]
async fn a_chat_in_a_closed_workspace_is_not_resumed() {
    let h = harness().await;
    h.chat("rr-archived");
    h.db()
        .upsert_agent_chat_session("rr-archived", "ws-archived", h.dir.path().to_str(), h.kind)
        .unwrap();
    h.running("rr-archived", "turn-1");
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(
        decision(&decisions, "rr-archived"),
        Some(ResumeDecision::Skip(SkipReason::WorkspaceClosed))
    );
    assert!(h.sent("rr-archived").is_empty());
}

#[tokio::test]
async fn a_chat_whose_worktree_is_gone_is_not_resumed() {
    let h = harness().await;
    h.chat("rr-gone");
    let gone = h.dir.path().join("removed-worktree");
    h.db()
        .upsert_agent_chat_session("rr-gone", &h.workspace, gone.to_str(), h.kind)
        .unwrap();
    h.running("rr-gone", "turn-1");
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(
        decision(&decisions, "rr-gone"),
        Some(ResumeDecision::Skip(SkipReason::WorktreeMissing))
    );
    assert!(h.sent("rr-gone").is_empty());
}

#[tokio::test]
async fn turning_the_setting_off_keeps_the_manual_continue() {
    let h = harness().await;
    h.db().set_setting(RESTART_RESUME_SETTING_KEY, "false").unwrap();
    h.chat("rr-off");
    h.running("rr-off", "turn-1");
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(
        decision(&decisions, "rr-off"),
        Some(ResumeDecision::Skip(SkipReason::Disabled))
    );
    assert!(h.sent("rr-off").is_empty());
    // Consumed: turning it back on later does not revive it.
    assert!(h.db().get_restart_turn("rr-off").is_none());
}

#[tokio::test]
async fn resumes_in_a_row_are_capped_until_the_user_writes() {
    let h = harness().await;
    h.chat("rr-loop");
    h.running("rr-loop", "turn-1");
    // Each launch resumes the turn, which the app then dies in again.
    for (launch, expected) in [
        ("launch-2", ResumeDecision::Resume),
        ("launch-3", ResumeDecision::Resume),
        ("launch-4", ResumeDecision::Skip(SkipReason::CapReached)),
    ] {
        let decisions = resume_after_restart_for_test(&h.handle(), launch).await;
        assert_eq!(decision(&decisions, "rr-loop"), Some(expected), "{launch}");
        h.running("rr-loop", &format!("turn-{launch}"));
        // The provider's live session went with the "crash".
        h.provider.set_turn_active("rr-loop", false);
    }
    assert_eq!(h.sent("rr-loop").len(), 2);
}

#[tokio::test]
async fn the_saved_queue_drains_in_order_after_the_resumed_turn() {
    let h = harness().await;
    h.chat("rr-drain");
    h.saved("rr-drain", "q-1", "first saved");
    h.saved("rr-drain", "q-2", "second saved");
    h.saved("rr-drain", "q-3", "third saved");
    h.running("rr-drain", "turn-1");
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-drain"), Some(ResumeDecision::Resume));
    // Nothing drains while the resumed turn runs.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.sent("rr-drain").len(), 1);

    h.running("rr-drain", "turn-resume");
    h.completed("rr-drain", TurnStatus::Success);
    let sent = wait_for_sends(&h, "rr-drain", 4).await;
    assert_eq!(sent.len(), 4, "{sent:?}");
    assert!(sent[0].starts_with(RESTART_RESUME_NOTE_PREFIX));
    assert_eq!(&sent[1..], ["first saved", "second saved", "third saved"]);
    assert!(h.db().list_queued_turns("rr-drain").is_empty());
}

#[tokio::test]
async fn a_failed_resumed_turn_keeps_the_saved_queue_manual() {
    let h = harness().await;
    h.chat("rr-failed");
    h.saved("rr-failed", "q-1", "saved");
    h.running("rr-failed", "turn-1");
    resume_after_restart_for_test(&h.handle(), "launch-2").await;
    h.completed(
        "rr-failed",
        TurnStatus::Error {
            subtype: "interrupted".into(),
            message: "turn interrupted".into(),
        },
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.sent("rr-failed").len(), 1);
    assert_eq!(h.db().list_queued_turns("rr-failed").len(), 1);
}

#[tokio::test]
async fn an_idle_chat_keeps_its_saved_queue_manual() {
    let h = harness().await;
    h.chat("rr-idle");
    h.saved("rr-idle", "q-1", "saved");
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-idle"), None);
    // A later turn of its own finishing does not send the saved row.
    h.running("rr-idle", "turn-1");
    h.completed("rr-idle", TurnStatus::Success);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.sent("rr-idle").is_empty());
    assert_eq!(h.db().list_queued_turns("rr-idle").len(), 1);
}

/// The only test here that reads the Claude config dir, so it alone sets it.
#[tokio::test]
async fn a_claude_chat_resumes_only_while_its_conversation_file_exists() {
    let h = harness_for(ProviderKind::Claude).await;
    let config = tempfile::tempdir().unwrap();
    let project = config.path().join("projects").join("-workdir");
    std::fs::create_dir_all(&project).unwrap();
    std::env::set_var("CLAUDE_CONFIG_DIR", config.path());
    let session = "00000000-0000-4000-8000-0000000c1a0d";

    h.chat("rr-claude");
    h.db().set_agent_chat_sdk_session_id("rr-claude", session).unwrap();
    h.running("rr-claude", "turn-1");
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(
        decision(&decisions, "rr-claude"),
        Some(ResumeDecision::Skip(SkipReason::ManualOnly)),
        "a fresh conversation would not know what it was doing"
    );
    assert!(h.sent("rr-claude").is_empty());

    std::fs::write(project.join(format!("{session}.jsonl")), b"{}").unwrap();
    h.running("rr-claude", "turn-2");
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-3").await;
    assert_eq!(decision(&decisions, "rr-claude"), Some(ResumeDecision::Resume));
    assert_eq!(
        h.provider.start_inputs()[0].resume_cursor,
        Some(serde_json::json!({"resume": session}))
    );
}
