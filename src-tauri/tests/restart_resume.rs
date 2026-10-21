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
use codemux_lib::agent_provider::types::MessageDelivery;
use codemux_lib::commands::agent_chat::{
    agent_chat_interrupt_turn, send_turn_with_origin, TurnOrigin, forward_event, AgentChatChannelRegistry,
    GrokUsageLedgerBridge, ProviderRegistry, RunActivityTracker, SendTurnCommandInput,
    SubagentTracker,
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
        ProviderKind::OpenCode => registry.set_opencode(provider.clone() as _).await,
        _ => registry.set_codex(provider.clone() as _).await,
    }
    app.manage(registry);
    let dir = tempfile::tempdir().unwrap();
    let state: State<'_, AppStateStore> = app.state();
    let workspace = state.create_workspace_at_path(dir.path().into()).0;
    Harness {
        app,
        kind: match kind {
            ProviderKind::Claude => "claude",
            ProviderKind::OpenCode => "opencode",
            _ => "codex",
        },
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

    fn completed(&self, thread: &str, turn: &str, status: TurnStatus) {
        forward_event(
            &self.handle(),
            ProviderRuntimeEvent::TurnCompleted {
                thread_id: ThreadId(thread.into()),
                turn_id: TurnId(turn.into()),
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
                provenance: Default::default(),
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
    // The provider reopened the saved conversation, and only that one.
    let starts = h.provider.start_inputs();
    assert_eq!(
        starts[0].resume_cursor,
        Some(serde_json::json!({"resume": "sdk-rr-quit", "requireOriginal": true}))
    );

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
    h.completed("rr-settled", "turn-1", TurnStatus::Success);
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-settled"), None);
    assert!(h.sent("rr-settled").is_empty());
}

/// A turn's end can arrive after the next turn started: Claude clears its
/// turn before it publishes the end, so a send can start the next one in
/// between. The late end leaves the newer turn resumable.
#[tokio::test]
async fn a_late_end_of_an_older_turn_keeps_the_newer_turn_resumable() {
    let h = harness().await;
    h.chat("rr-late");
    h.running("rr-late", "turn-old");
    h.running("rr-late", "turn-new");
    h.completed("rr-late", "turn-old", TurnStatus::Success);
    let row = h.db().get_restart_turn("rr-late").map(|row| row.turn_id);
    assert_eq!(row.as_deref(), Some("turn-new"));
    on_exit(&h.handle());
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-late"), Some(ResumeDecision::Resume));
    // Its own end settles it.
    h.running("rr-late", "turn-next");
    h.completed("rr-late", "turn-next", TurnStatus::Success);
    assert!(h.db().get_restart_turn("rr-late").is_none());
}

/// Providers that can't name a turn end it with an empty id: that end
/// still settles whatever turn is recorded.
#[tokio::test]
async fn an_end_without_a_turn_id_still_settles() {
    let h = harness().await;
    h.chat("rr-anonymous");
    h.running("rr-anonymous", "turn-1");
    h.completed("rr-anonymous", "", TurnStatus::Success);
    assert!(h.db().get_restart_turn("rr-anonymous").is_none());
}

#[tokio::test]
async fn a_chat_the_user_stopped_is_not_resumed() {
    let h = harness().await;
    h.chat("rr-stopped");
    h.running("rr-stopped", "turn-1");
    agent_chat_interrupt_turn(
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
    h.completed("rr-drain", "turn-resume", TurnStatus::Success);
    let sent = wait_for_sends(&h, "rr-drain", 4).await;
    assert_eq!(sent.len(), 4, "{sent:?}");
    assert!(sent[0].starts_with(RESTART_RESUME_NOTE_PREFIX));
    assert_eq!(&sent[1..], ["first saved", "second saved", "third saved"]);
    // Each row goes once its send returns, just after the provider saw it.
    for _ in 0..200 {
        if h.db().list_queued_turns("rr-drain").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(h.db().list_queued_turns("rr-drain").is_empty());
}

/// A late end of the turn before the resumed one neither sends the saved
/// queue early nor disarms it: the resumed turn's own end does that.
#[tokio::test]
async fn a_late_end_of_an_older_turn_leaves_the_resumed_turn_its_drain() {
    let h = harness().await;
    h.chat("rr-late-drain");
    h.saved("rr-late-drain", "q-1", "saved");
    h.running("rr-late-drain", "turn-1");
    resume_after_restart_for_test(&h.handle(), "launch-2").await;
    h.running("rr-late-drain", "turn-resume");
    h.completed(
        "rr-late-drain",
        "turn-1",
        TurnStatus::Error { subtype: "interrupted".into(), message: "cut off".into() },
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.sent("rr-late-drain").len(), 1, "nothing drains while the resumed turn runs");
    assert!(h.db().get_restart_turn("rr-late-drain").is_some());
    h.completed("rr-late-drain", "turn-resume", TurnStatus::Success);
    let sent = wait_for_sends(&h, "rr-late-drain", 2).await;
    assert_eq!(&sent[1..], ["saved"]);
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
        "turn-resume",
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
    h.completed("rr-idle", "turn-1", TurnStatus::Success);
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
        Some(serde_json::json!({"resume": session, "requireOriginal": true}))
    );

    // A chat on another Claude account keeps its conversation in that
    // account's home, not the default one, which lacks it here.
    let account_home = tempfile::tempdir().unwrap();
    let account_project = account_home.path().join("projects").join("-workdir");
    std::fs::create_dir_all(&account_project).unwrap();
    let other = "00000000-0000-4000-8000-0000000acc07";
    std::fs::write(account_project.join(format!("{other}.jsonl")), b"{}").unwrap();
    h.db()
        .insert_provider_account(&codemux_lib::database::provider_accounts::ProviderAccountRow {
            id: "work".into(),
            provider: "claude".into(),
            label: "Work".into(),
            home: account_home.path().to_string_lossy().into_owned(),
            created_at_ms: 1,
        })
        .unwrap();
    h.chat("rr-claude-work");
    h.db().set_provider_account_binding("rr-claude-work", "claude", "work").unwrap();
    h.db().set_agent_chat_sdk_session_id("rr-claude-work", other).unwrap();
    h.running("rr-claude-work", "turn-1");
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-4").await;
    assert_eq!(decision(&decisions, "rr-claude-work"), Some(ResumeDecision::Resume));
}

fn typed(thread: &str, text: &str) -> SendTurnCommandInput {
    SendTurnCommandInput {
        input_source: None,
        delivery: MessageDelivery::Queue,
        thread_id: ThreadId(thread.into()),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_before_drain_submission_keeps_every_unsent_message_saved() {
    let h = harness().await;
    h.chat("rr-drain-stop");
    h.saved("rr-drain-stop", "q-1", "first saved");
    h.saved("rr-drain-stop", "q-2", "second saved");
    h.running("rr-drain-stop", "turn-1");
    resume_after_restart_for_test(&h.handle(), "launch-2").await;
    // Hold the first saved message before the mock authorizes submission.
    let (entered, release) = h.provider.hold_next_send();
    h.running("rr-drain-stop", "turn-resume");
    h.completed("rr-drain-stop", "turn-resume", TurnStatus::Success);
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    let stop = tokio::spawn(agent_chat_interrupt_turn(
        h.handle(),
        ProviderKind::Codex,
        ThreadId("rr-drain-stop".into()),
        None,
    ));
    // Wait for durable Stop fencing, rather than assuming the spawned Stop
    // won a scheduling race. The held send has written no provider bytes.
    tokio::time::timeout(Duration::from_secs(2), async {
        while h.db().delivery_generation("rr-drain-stop").unwrap().0 == 0 {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    release.notify_one();
    stop.await.unwrap().unwrap();
    let sent = h.sent("rr-drain-stop");
    assert!(sent[1..].is_empty(), "Stop before submission must hold the selected saved input: {sent:?}");
    let left: Vec<_> = h
        .db()
        .list_queued_turns("rr-drain-stop")
        .into_iter()
        .map(|row| row.queued_id)
        .collect();
    assert_eq!(left, ["q-1", "q-2"]);
    let held = h.db().message_delivery_for_message("rr-drain-stop", "nonce-q-1").unwrap().unwrap();
    assert_eq!(held.state, "held");
    assert_eq!(held.hold_reason.as_deref(), Some("stop"));
    assert!(!held.attempted);
    // Stop waits for the native send lock, but the cancelled drain can still
    // hold the saved-row claim while it retains the delivery for retry.
    // Wait for that claim, without hiding any other failure or resending an
    // accepted message.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let result = codemux_lib::commands::queued_turns::agent_chat_send_saved_queued_turn(
                h.handle(), ProviderKind::Codex, ThreadId("rr-drain-stop".into()), "q-1".into(), MessageDelivery::Queue,
            ).await;
            match result {
                Err(error) if error == "This message is being sent or changed. Try again in a moment." => {
                    tokio::task::yield_now().await;
                }
                result => break result,
            }
        }
    }).await.expect("the cancelled drain releases its saved-row claim").unwrap();
    assert_eq!(&h.sent("rr-drain-stop")[1..], ["first saved"]);
    assert_eq!(h.db().list_queued_turns("rr-drain-stop").iter().map(|row| row.queued_id.as_str()).collect::<Vec<_>>(), ["q-2"]);
}

#[tokio::test]
async fn the_drain_skips_a_saved_turn_that_was_already_sent() {
    let h = harness().await;
    h.chat("rr-drain-dup");
    h.saved("rr-drain-dup", "q-1", "already sent");
    h.saved("rr-drain-dup", "q-2", "not yet");
    // The first one reached the transcript before the crash; its row outlived it.
    h.db()
        .append_agent_chat_message(
            "rr-drain-dup",
            &serde_json::json!({
                "type": "user_message", "thread_id": "rr-drain-dup",
                "text": "already sent", "client_nonce": "nonce-q-1",
            })
            .to_string(),
        )
        .unwrap();
    h.running("rr-drain-dup", "turn-1");
    resume_after_restart_for_test(&h.handle(), "launch-2").await;
    h.running("rr-drain-dup", "turn-resume");
    h.completed("rr-drain-dup", "turn-resume", TurnStatus::Success);
    let sent = wait_for_sends(&h, "rr-drain-dup", 2).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(&h.sent("rr-drain-dup")[1..], ["not yet"], "{sent:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_codex_chat_interrupted_in_plan_resumes_in_plan() {
    let h = harness().await;
    h.chat("rr-plan");
    // Codex reports the mode each accepted turn runs in; the Plan pill that
    // chose it does not survive a restart.
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::TurnModeSelected {
            thread_id: ThreadId("rr-plan".into()),
            mode: "plan".into(),
        },
    );
    h.running("rr-plan", "turn-1");
    on_exit(&h.handle());

    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(decision(&decisions, "rr-plan"), Some(ResumeDecision::Resume));
    // The reopened session gets it, for servers that do not report it.
    let starts = h.provider.start_inputs();
    assert_eq!(starts[0].extra, serde_json::json!({ "turnMode": "plan" }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chat_the_user_takes_over_during_the_launch_pass_is_left_alone() {
    let h = harness().await;
    h.chat("rr-take-a");
    h.chat("rr-take-b");
    h.running("rr-take-a", "turn-a");
    std::thread::sleep(Duration::from_millis(5));
    h.running("rr-take-b", "turn-b");
    // Chat A's session takes a while to reopen; the user writes to B meanwhile.
    let (entered, release) = h.provider.hold_next_start();
    let pass = tokio::spawn({
        let handle = h.handle();
        async move { resume_after_restart_for_test(&handle, "launch-2").await }
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    send_turn_with_origin(h.handle(), ProviderKind::Codex, typed("rr-take-b", "do this instead"), TurnOrigin::User)
        .await
        .unwrap();
    // B's own turn finished before the pass reached it.
    h.provider.set_turn_active("rr-take-b", false);
    release.notify_one();
    let decisions = pass.await.unwrap();
    assert_eq!(decision(&decisions, "rr-take-a"), Some(ResumeDecision::Resume));
    assert_eq!(
        decision(&decisions, "rr-take-b"),
        Some(ResumeDecision::Skip(SkipReason::UserTookOver))
    );
    assert_eq!(h.sent("rr-take-b"), ["do this instead"]);
}

#[tokio::test]
async fn a_send_that_fails_after_running_leaves_nothing_to_resume() {
    let h = harness().await;
    h.chat("rr-ready");
    h.running("rr-ready", "turn-1");
    // The provider reported the turn, then the send failed: only `Ready` follows.
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ThreadId("rr-ready".into()),
            status: SessionStatus::Ready,
        },
    );
    for _ in 0..100 {
        if h.db().get_restart_turn("rr-ready").is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(h.db().get_restart_turn("rr-ready").is_none());

    // While a turn is live, `Ready` (a session finishing its setup) keeps it.
    h.chat("rr-ready-live");
    h.provider.set_turn_active("rr-ready-live", true);
    h.running("rr-ready-live", "turn-1");
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ThreadId("rr-ready-live".into()),
            status: SessionStatus::Ready,
        },
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.db().get_restart_turn("rr-ready-live").is_some());
}

#[tokio::test]
async fn a_conversation_that_cannot_be_reopened_is_not_resumed() {
    let h = harness().await;
    h.chat("rr-lost");
    h.running("rr-lost", "turn-1");
    h.provider.fail_next_start(codemux_lib::agent_provider::ProviderError::SessionNotFound {
        thread_id: ThreadId("rr-lost".into()),
    });
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(
        decision(&decisions, "rr-lost"),
        Some(ResumeDecision::Skip(SkipReason::ConversationLost))
    );
    // No fresh session was started in its place, and nothing was sent.
    assert_eq!(h.provider.start_inputs().len(), 1);
    assert!(h.sent("rr-lost").is_empty());
}

#[tokio::test]
async fn providers_that_would_start_a_fresh_conversation_stay_manual() {
    let h = harness_for(ProviderKind::OpenCode).await;
    h.chat("rr-opencode");
    h.running("rr-opencode", "turn-1");
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-2").await;
    assert_eq!(
        decision(&decisions, "rr-opencode"),
        Some(ResumeDecision::Skip(SkipReason::ManualOnly))
    );
    assert!(h.sent("rr-opencode").is_empty());
}

#[tokio::test]
async fn a_resume_that_fell_back_to_a_fresh_conversation_keeps_the_queue_manual() {
    let h = harness().await;
    h.chat("rr-fresh");
    h.saved("rr-fresh", "q-1", "saved");
    h.running("rr-fresh", "turn-1");
    resume_after_restart_for_test(&h.handle(), "launch-2").await;
    // The provider could not resume and rebuilt a fresh session.
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::ResumeCursorUpdated {
            thread_id: ThreadId("rr-fresh".into()),
            resume_cursor: serde_json::Value::Null,
        },
    );
    h.completed("rr-fresh", "turn-resume", TurnStatus::Success);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.sent("rr-fresh").len(), 1);
    assert_eq!(h.db().list_queued_turns("rr-fresh").len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_conversation_while_reopening_sends_nothing() {
    let h = harness().await;
    h.chat("rr-fresh-early");
    h.saved("rr-fresh-early", "q-1", "saved");
    h.running("rr-fresh-early", "turn-1");
    let (entered, release) = h.provider.hold_next_start();
    let pass = tokio::spawn({
        let handle = h.handle();
        async move { resume_after_restart_for_test(&handle, "launch-2").await }
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    // The provider gives up on the saved conversation before any turn starts.
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::ResumeCursorUpdated {
            thread_id: ThreadId("rr-fresh-early".into()),
            resume_cursor: serde_json::Value::Null,
        },
    );
    release.notify_one();
    let decisions = pass.await.unwrap();
    assert_eq!(
        decision(&decisions, "rr-fresh-early"),
        Some(ResumeDecision::Skip(SkipReason::ConversationLost))
    );
    assert!(h.sent("rr-fresh-early").is_empty());
    assert_eq!(h.db().list_queued_turns("rr-fresh-early").len(), 1);
}

fn subagent(thread: &str, status: codemux_lib::agent_provider::SubagentStatus) -> ProviderRuntimeEvent {
    child(thread, "sub-1", status)
}

fn child(thread: &str, id: &str, status: codemux_lib::agent_provider::SubagentStatus) -> ProviderRuntimeEvent {
    ProviderRuntimeEvent::SubagentUpdated {
        thread_id: ThreadId(thread.into()),
        subagent: codemux_lib::agent_provider::SubagentSnapshot {
            subagent_id: id.into(),
            status,
            ..Default::default()
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_that_yields_to_its_subagents_stays_resumable() {
    use codemux_lib::agent_provider::SubagentStatus;
    let h = harness().await;
    h.chat("rr-yield");
    h.saved("rr-yield", "q-1", "saved");
    h.running("rr-yield", "turn-1");
    resume_after_restart_for_test(&h.handle(), "launch-2").await;
    h.running("rr-yield", "turn-resume");
    h.provider.set_turn_active("rr-yield", true);
    forward_event(&h.handle(), subagent("rr-yield", SubagentStatus::Running));
    // The provider yields while its subagent works: the run goes on.
    h.provider.set_turn_active("rr-yield", false);
    h.completed("rr-yield", "turn-resume", TurnStatus::Success);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.db().get_restart_turn("rr-yield").is_some(), "a quit now must still resume it");
    assert_eq!(h.sent("rr-yield").len(), 1, "the saved queue waits for the real end");

    // The subagent finishes and the provider ends the turn for real.
    forward_event(&h.handle(), subagent("rr-yield", SubagentStatus::Completed));
    h.completed("rr-yield", "turn-resume", TurnStatus::Success);
    let sent = wait_for_sends(&h, "rr-yield", 2).await;
    assert_eq!(&sent[1..], ["saved"]);
    assert!(h.db().get_restart_turn("rr-yield").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_yielded_run_whose_subagents_finish_settles() {
    use codemux_lib::agent_provider::SubagentStatus;
    let h = harness().await;
    h.chat("rr-yield-end");
    h.running("rr-yield-end", "turn-1");
    h.provider.set_turn_active("rr-yield-end", true);
    forward_event(&h.handle(), subagent("rr-yield-end", SubagentStatus::Running));
    h.provider.set_turn_active("rr-yield-end", false);
    h.completed("rr-yield-end", "turn-1", TurnStatus::Success);
    // `Ready` while the child still works: the run is not over yet.
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ThreadId("rr-yield-end".into()),
            status: SessionStatus::Ready,
        },
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.db().get_restart_turn("rr-yield-end").is_some());
    // The child finishes and the parent never takes another turn.
    forward_event(&h.handle(), subagent("rr-yield-end", SubagentStatus::Completed));
    for _ in 0..100 {
        if h.db().get_restart_turn("rr-yield-end").is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(h.db().get_restart_turn("rr-yield-end").is_none(), "an idle chat must not resume");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resumed_turn_that_fails_while_its_subagents_run_keeps_the_queue_manual() {
    use codemux_lib::agent_provider::SubagentStatus;
    let h = harness().await;
    h.chat("rr-yield-fail");
    h.saved("rr-yield-fail", "q-1", "saved");
    h.running("rr-yield-fail", "turn-1");
    resume_after_restart_for_test(&h.handle(), "launch-2").await;
    h.running("rr-yield-fail", "turn-resume");
    h.provider.set_turn_active("rr-yield-fail", true);
    forward_event(&h.handle(), subagent("rr-yield-fail", SubagentStatus::Running));
    h.provider.set_turn_active("rr-yield-fail", false);
    h.completed(
        "rr-yield-fail",
        "turn-resume",
        TurnStatus::Error { subtype: "error".into(), message: "boom".into() },
    );
    // A later turn of the user's own finishes fine.
    h.running("rr-yield-fail", "turn-user");
    forward_event(&h.handle(), subagent("rr-yield-fail", SubagentStatus::Completed));
    h.completed("rr-yield-fail", "turn-user", TurnStatus::Success);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(h.sent("rr-yield-fail").len(), 1);
    assert_eq!(h.db().list_queued_turns("rr-yield-fail").len(), 1);
}

/// A late failed end of an older turn, while the resumed turn's subagents
/// hold the run, leaves the resumed turn its row and its drain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_failed_end_while_subagents_run_leaves_the_resumed_turn_its_drain() {
    use codemux_lib::agent_provider::SubagentStatus;
    let h = harness().await;
    h.chat("rr-late-yield");
    h.saved("rr-late-yield", "q-1", "saved");
    h.running("rr-late-yield", "turn-1");
    resume_after_restart_for_test(&h.handle(), "launch-2").await;
    h.running("rr-late-yield", "turn-resume");
    h.provider.set_turn_active("rr-late-yield", true);
    forward_event(&h.handle(), subagent("rr-late-yield", SubagentStatus::Running));
    h.provider.set_turn_active("rr-late-yield", false);
    h.completed(
        "rr-late-yield",
        "turn-1",
        TurnStatus::Error { subtype: "interrupted".into(), message: "cut off".into() },
    );
    let row = h.db().get_restart_turn("rr-late-yield").map(|row| row.turn_id);
    assert_eq!(row.as_deref(), Some("turn-resume"));
    forward_event(&h.handle(), subagent("rr-late-yield", SubagentStatus::Completed));
    h.completed("rr-late-yield", "turn-resume", TurnStatus::Success);
    let sent = wait_for_sends(&h, "rr-late-yield", 2).await;
    assert_eq!(&sent[1..], ["saved"]);
}

#[tokio::test]
async fn a_message_steered_into_a_resumed_turn_resets_the_cap() {
    let h = harness().await;
    h.chat("rr-steer-cap");
    h.running("rr-steer-cap", "turn-1");
    for launch in ["launch-2", "launch-3"] {
        resume_after_restart_for_test(&h.handle(), launch).await;
        h.running("rr-steer-cap", &format!("turn-{launch}"));
        h.provider.set_turn_active("rr-steer-cap", false);
    }
    assert_eq!(h.db().get_restart_turn("rr-steer-cap").unwrap().chain, 2);
    // The user writes into the running turn; no new turn starts.
    let mut steer = typed("rr-steer-cap", "also check the docs");
    steer.delivery = MessageDelivery::Steer;
    let _ = send_turn_with_origin(h.handle(), ProviderKind::Codex, steer, TurnOrigin::User).await;
    assert_eq!(h.db().get_restart_turn("rr-steer-cap").unwrap().chain, 0);
    let decisions = resume_after_restart_for_test(&h.handle(), "launch-4").await;
    assert_eq!(decision(&decisions, "rr-steer-cap"), Some(ResumeDecision::Resume));
}

/// Yield with one child (`sub-1`) or two (`sub-1`, `sub-2`) still running.
fn yield_to_children(h: &Harness, thread: &str, children: &[&str]) {
    use codemux_lib::agent_provider::SubagentStatus;
    h.running(thread, "turn-1");
    h.provider.set_turn_active(thread, true);
    for id in children {
        forward_event(&h.handle(), child(thread, id, SubagentStatus::Running));
    }
    h.provider.set_turn_active(thread, false);
    h.completed(thread, "turn-1", TurnStatus::Success);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_that_picks_itself_back_up_after_its_children_stays_resumable() {
    use codemux_lib::agent_provider::{CompletedItem, SubagentStatus};
    let h = harness().await;
    h.chat("rr-continue");
    yield_to_children(&h, "rr-continue", &["sub-1"]);
    forward_event(&h.handle(), subagent("rr-continue", SubagentStatus::Completed));
    // The provider continues the run without reporting a new turn.
    forward_event(
        &h.handle(),
        ProviderRuntimeEvent::ItemCompleted {
            thread_id: ThreadId("rr-continue".into()),
            turn_id: TurnId("turn-1".into()),
            item: CompletedItem::AssistantText { text: "Both checks passed; merging.".into() },
            subagent_id: None,
        },
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(h.db().get_restart_turn("rr-continue").is_some(), "the run is still live");
    // Its own turn end settles it.
    h.completed("rr-continue", "turn-1", TurnStatus::Success);
    assert!(h.db().get_restart_turn("rr-continue").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_last_child_to_finish_starts_the_settle_clock() {
    use codemux_lib::agent_provider::SubagentStatus;
    let h = harness().await;
    h.chat("rr-two-children");
    yield_to_children(&h, "rr-two-children", &["sub-1", "sub-2"]);
    forward_event(&h.handle(), child("rr-two-children", "sub-1", SubagentStatus::Completed));
    tokio::time::sleep(Duration::from_millis(150)).await;
    forward_event(&h.handle(), child("rr-two-children", "sub-2", SubagentStatus::Completed));
    // The first child's clock ran out at 200 ms; the second's has not.
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(h.db().get_restart_turn("rr-two-children").is_some());
    for _ in 0..100 {
        if h.db().get_restart_turn("rr-two-children").is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(h.db().get_restart_turn("rr-two-children").is_none());
}
