//! Integration tests for the agent-chat command surface.
//!
//! The `#[tauri::command]` wrapper functions themselves are tied to
//! the default Tauri runtime, which a test binary cannot drive. These
//! tests therefore exercise the same business logic as each command
//! via its public helpers — the feature-flag gate, the pane-tree
//! operations on `AppStateStore`, the [`ProviderRegistry`] routing
//! with a `MockAgentProvider`, and the event-bridge forwarder
//! reached through [`forward_event`].
//!
//! Unix-only: `tauri::test::mock_app()` (used by the event-bridge
//! test) pulls in Tauri runtime imports that need WebView2Loader.dll
//! at process startup on Windows — the GitHub Windows runner doesn't
//! ship that, so the test binary fails with STATUS_ENTRYPOINT_NOT_FOUND
//! before any test runs and skips the whole file's coverage. The
//! orchestration logic under test is platform-agnostic, so Linux
//! coverage is sufficient. Same gate pattern as `claude_adapter.rs`,
//! `codex_adapter.rs`, and `sidecar_sdk.rs` in this directory.

#![cfg(unix)]

#[path = "helpers/mock_agent_provider.rs"]
mod mock_agent_provider;

use std::sync::Arc;
use std::time::Duration;

use tauri::Listener;
use tokio::time::timeout;

use codemux_lib::agent_provider::{
    ApprovalDecision, CompletedItem, ProviderKind, ProviderRuntimeEvent, RequestId,
    SendTurnInput, StartSessionInput, ThreadId, TurnId,
};
use codemux_lib::commands::agent_chat::{
    checkpoint_ref_component, feature_flag_on, forward_event, perform_run_checkpoint,
    thread_id_for_event, AgentChatChannelRegistry, AgentChatEventPayload, ProviderRegistry,
    AGENT_CHAT_EVENT, FEATURE_DISABLED_ERROR,
};
use codemux_lib::database::DatabaseStore;
use codemux_lib::observability::{FeatureFlags, ObservabilityStore};
use codemux_lib::state::AppStateStore;
use tauri::{Manager, State};

use crate::mock_agent_provider::{MockAgentProvider, MockCall};

// ── Helpers ──

fn test_observability(enable_agent_chat: bool) -> ObservabilityStore {
    let store = ObservabilityStore::default();
    let mut flags = store.feature_flags();
    flags.enable_agent_chat = enable_agent_chat;
    store.set_feature_flags(FeatureFlags {
        enable_agent_chat,
        ..flags
    });
    store
}

fn start_input(thread_id: &str) -> StartSessionInput {
    StartSessionInput {
        thread_id: ThreadId(thread_id.into()),
        cwd: std::path::PathBuf::from("/tmp/codemux-test"),
        model: None,
        resume_cursor: None,
        permission_mode: None,
        effort: None,
        context_window: None,
        additional_directories: vec![],
        env: None,
        extra: serde_json::Value::Null,
    }
}

// ── Feature flag gate ──

#[test]
fn feature_flag_off_rejects_create_pane() {
    let observability = test_observability(false);
    let err = feature_flag_on(&observability).expect_err("flag off should reject");
    assert_eq!(err, FEATURE_DISABLED_ERROR);
}

#[test]
fn feature_flag_on_create_pane_succeeds() {
    let observability = test_observability(true);
    feature_flag_on(&observability).expect("flag on should accept");

    let store = AppStateStore::default();
    let snapshot = store.snapshot();
    let workspace_id = snapshot.active_workspace_id.0;

    let pane_id = store
        .create_agent_chat_pane(&workspace_id, None, None, None)
        .expect("create_agent_chat_pane should succeed when flag is on");

    let after = store.snapshot();
    let ws = after
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .expect("workspace present");
    let surface = &ws.surfaces[0];
    assert_eq!(surface.active_pane_id.0, pane_id.0);
}

// ── Pane lifecycle ──

#[test]
fn create_pane_in_nonexistent_workspace_errors() {
    let store = AppStateStore::default();
    let err = store
        .create_agent_chat_pane("ws-does-not-exist", None, None, None)
        .expect_err("unknown workspace should error");
    assert!(
        err.contains("ws-does-not-exist"),
        "error should mention the missing workspace id, got: {err}"
    );
}

#[test]
fn close_pane_is_idempotent() {
    // The Tauri command wrapper swallows close_pane's
    // unknown-pane error so double-close is a no-op. Verify the
    // swallow by invoking AppStateStore::close_pane twice on the
    // same id, mirroring the wrapper's `let _ = ...` pattern.
    let store = AppStateStore::default();
    let snapshot = store.snapshot();
    let workspace_id = snapshot.active_workspace_id.0;
    let pane_id = store
        .create_agent_chat_pane(&workspace_id, None, None, None)
        .expect("create pane");

    // First close succeeds.
    let first = store.close_pane(&pane_id.0);
    assert!(
        first.is_ok(),
        "first close should succeed, got: {first:?}"
    );

    // Second close fails — the wrapper swallows the error to stay
    // idempotent from the UI's perspective.
    let second = store.close_pane(&pane_id.0);
    assert!(
        second.is_err(),
        "second close on the same pane should error at the store level"
    );
    // Simulate the wrapper's `let _ = state.close_pane(...)`.
    let _ = second;
}

// ── Provider registry routing ──

#[tokio::test]
async fn start_session_routes_to_claude_provider() {
    let registry = ProviderRegistry::new();
    let claude_mock = Arc::new(MockAgentProvider::new(ProviderKind::Claude));
    let codex_mock = Arc::new(MockAgentProvider::new(ProviderKind::Codex));
    registry.set_claude(claude_mock.clone() as _).await;
    registry.set_codex(codex_mock.clone() as _).await;

    let provider = registry
        .get(ProviderKind::Claude)
        .await
        .expect("claude provider registered");

    let session = provider
        .start_session(start_input("thread-claude-1"))
        .await
        .expect("start_session ok");

    assert_eq!(session.thread_id.0, "thread-claude-1");
    assert_eq!(
        claude_mock.calls.snapshot(),
        vec![MockCall::StartSession(ThreadId("thread-claude-1".into()))]
    );
    assert!(
        codex_mock.calls.snapshot().is_empty(),
        "codex mock should not have been called"
    );
}

#[tokio::test]
async fn start_session_routes_to_codex_provider() {
    let registry = ProviderRegistry::new();
    let claude_mock = Arc::new(MockAgentProvider::new(ProviderKind::Claude));
    let codex_mock = Arc::new(MockAgentProvider::new(ProviderKind::Codex));
    registry.set_claude(claude_mock.clone() as _).await;
    registry.set_codex(codex_mock.clone() as _).await;

    let provider = registry
        .get(ProviderKind::Codex)
        .await
        .expect("codex provider registered");

    let session = provider
        .start_session(start_input("thread-codex-1"))
        .await
        .expect("start_session ok");

    assert_eq!(session.thread_id.0, "thread-codex-1");
    assert_eq!(
        codex_mock.calls.snapshot(),
        vec![MockCall::StartSession(ThreadId("thread-codex-1".into()))]
    );
    assert!(claude_mock.calls.snapshot().is_empty());
}

#[tokio::test]
async fn send_turn_forwards_to_selected_provider() {
    let registry = ProviderRegistry::new();
    let codex_mock = Arc::new(MockAgentProvider::new(ProviderKind::Codex));
    registry.set_codex(codex_mock.clone() as _).await;

    let provider = registry
        .get(ProviderKind::Codex)
        .await
        .expect("codex provider registered");

    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t".into()),
            text: "hello".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();

    assert_eq!(
        codex_mock.calls.snapshot(),
        vec![MockCall::SendTurn(ThreadId("t".into()), "hello".into())]
    );
}

#[tokio::test]
async fn stop_session_forwards_to_provider() {
    let registry = ProviderRegistry::new();
    let claude_mock = Arc::new(MockAgentProvider::new(ProviderKind::Claude));
    registry.set_claude(claude_mock.clone() as _).await;

    let provider = registry
        .get(ProviderKind::Claude)
        .await
        .expect("claude provider registered");
    provider.stop_session(ThreadId("t".into())).await.unwrap();

    assert_eq!(
        claude_mock.calls.snapshot(),
        vec![MockCall::StopSession(ThreadId("t".into()))]
    );
}

#[tokio::test]
async fn respond_to_request_forwards_to_provider() {
    let registry = ProviderRegistry::new();
    let claude_mock = Arc::new(MockAgentProvider::new(ProviderKind::Claude));
    registry.set_claude(claude_mock.clone() as _).await;

    let provider = registry
        .get(ProviderKind::Claude)
        .await
        .expect("claude provider registered");
    provider
        .respond_to_request(
            ThreadId("t".into()),
            RequestId("req-1".into()),
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        claude_mock.calls.snapshot(),
        vec![MockCall::RespondToRequest(
            ThreadId("t".into()),
            RequestId("req-1".into()),
        )]
    );
}

#[tokio::test]
async fn registry_returns_none_when_provider_missing() {
    let registry = ProviderRegistry::new();
    assert!(registry.get(ProviderKind::Claude).await.is_none());
    assert!(registry.get(ProviderKind::Codex).await.is_none());
}

// ── Run-start rollback checkpoints (issue #80) ──

#[test]
fn checkpoint_round_trips_through_session_row() {
    let db = DatabaseStore::new_in_memory();
    db.upsert_agent_chat_session("th-cp", "ws-1", Some("/tmp/repo"), "claude")
        .expect("upsert session");

    // No checkpoint recorded yet (setting off / snapshot pending).
    assert!(db.get_agent_chat_checkpoint("th-cp").is_none());

    db.set_agent_chat_checkpoint("th-cp", "abc123", "def456")
        .expect("set checkpoint");
    assert_eq!(
        db.get_agent_chat_checkpoint("th-cp"),
        Some(("abc123".to_string(), "def456".to_string()))
    );
    // The restore path resolves the repo via the session's cwd.
    assert_eq!(
        db.get_agent_chat_session_cwd("th-cp"),
        Some("/tmp/repo".to_string())
    );
    // Unknown threads stay None.
    assert!(db.get_agent_chat_checkpoint("th-other").is_none());
}

/// Full backend loop against a real repository: run-start checkpoint
/// records hashes on the session row; the recorded commit restores
/// the tree after an "agent run" mangles it.
#[tokio::test]
async fn run_checkpoint_records_and_restores_against_real_repo() {
    let app = tauri::test::mock_app();
    app.manage(DatabaseStore::new_in_memory());
    let handle = app.handle().clone();

    // Real repo with one committed file + one untracked WIP file.
    let dir = tempfile::TempDir::new().unwrap();
    let repo = dir.path();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "--initial-branch=main"]);
    git(&["config", "user.email", "t@e.com"]);
    git(&["config", "user.name", "T"]);
    std::fs::write(repo.join("code.txt"), "original\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "base"]);
    std::fs::write(repo.join("wip.txt"), "user wip\n").unwrap();

    let cwd = repo.to_string_lossy().to_string();
    let db: State<'_, DatabaseStore> = handle.state();
    db.upsert_agent_chat_session("th-real", "ws-1", Some(&cwd), "claude")
        .expect("upsert");

    // Run-start hook body (the spawn wrapper only adds the opt-in
    // gate + background spawn around exactly this call).
    perform_run_checkpoint(&handle, "th-real", &cwd);

    let (commit, head) = db
        .get_agent_chat_checkpoint("th-real")
        .expect("checkpoint recorded on the session row");
    assert!(!commit.is_empty() && !head.is_empty());

    // "Agent run": clobber + junk + delete.
    std::fs::write(repo.join("code.txt"), "agent broke this\n").unwrap();
    std::fs::write(repo.join("junk.txt"), "junk\n").unwrap();
    std::fs::remove_file(repo.join("wip.txt")).unwrap();

    // Restore using exactly what the restore command reads from the DB.
    codemux_lib::git::git_restore_workspace_checkpoint(
        repo,
        &commit,
        &format!(
            "refs/codemux/pre-restore/{}",
            checkpoint_ref_component("th-real")
        ),
    )
    .expect("restore");

    assert_eq!(
        std::fs::read_to_string(repo.join("code.txt")).unwrap(),
        "original\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("wip.txt")).unwrap(),
        "user wip\n"
    );
    assert!(!repo.join("junk.txt").exists());
}

#[test]
fn checkpoint_ref_component_sanitizes_thread_ids() {
    // Typical local thread id passes through untouched.
    assert_eq!(
        checkpoint_ref_component("chat-pane-1-1700000000"),
        "chat-pane-1-1700000000"
    );
    // Ref-hostile characters are flattened to dashes.
    assert_eq!(checkpoint_ref_component("a b/c~d:e?f"), "a-b-c-d-e-f");
    // Never produces an empty ref segment.
    assert_eq!(checkpoint_ref_component(""), "unknown");
}

// ── Event bridge ──

#[test]
fn thread_id_for_event_extracts_bound_threads() {
    let delta = ProviderRuntimeEvent::ContentDelta {
        thread_id: ThreadId("th-1".into()),
        turn_id: TurnId("tn-1".into()),
        delta: codemux_lib::agent_provider::ContentDelta::Text {
            text: "hi".into(),
        },
    };
    assert_eq!(
        thread_id_for_event(&delta),
        Some(ThreadId("th-1".into()))
    );

    let warn = ProviderRuntimeEvent::RuntimeWarning {
        thread_id: None,
        message: "x".into(),
        original_payload: None,
    };
    assert_eq!(thread_id_for_event(&warn), None);

    let warn_scoped = ProviderRuntimeEvent::RuntimeWarning {
        thread_id: Some(ThreadId("th-2".into())),
        message: "y".into(),
        original_payload: None,
    };
    assert_eq!(
        thread_id_for_event(&warn_scoped),
        Some(ThreadId("th-2".into()))
    );
}

/// Build a mock app managing everything `forward_event` touches:
/// DatabaseStore (transcript persistence) and the per-thread channel
/// registry (live routing). Without either, `app.state::<…>` panics
/// with "state() called before manage()".
fn channel_test_app() -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    app.manage(DatabaseStore::new_in_memory());
    app.manage(AgentChatChannelRegistry::new());
    app
}

/// Construct a test Channel whose deliveries land in the returned
/// shared Vec. `Channel::send` invokes the handler synchronously, so
/// tests can assert immediately after `forward_event` returns.
fn collecting_channel() -> (
    tauri::ipc::Channel<AgentChatEventPayload>,
    Arc<std::sync::Mutex<Vec<AgentChatEventPayload>>>,
) {
    let received: Arc<std::sync::Mutex<Vec<AgentChatEventPayload>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = received.clone();
    let channel = tauri::ipc::Channel::new(move |body: tauri::ipc::InvokeResponseBody| {
        let payload: AgentChatEventPayload = body
            .deserialize()
            .expect("channel payload should deserialize");
        sink.lock().unwrap().push(payload);
        Ok(())
    });
    (channel, received)
}

fn item_completed(thread: &str, text: &str) -> ProviderRuntimeEvent {
    ProviderRuntimeEvent::ItemCompleted {
        thread_id: ThreadId(thread.into()),
        turn_id: TurnId("turn-1".into()),
        item: CompletedItem::AssistantText { text: text.into() },
    }
}

#[tokio::test]
async fn event_bridge_routes_thread_events_to_attached_channel() {
    let app = channel_test_app();
    let handle = app.handle().clone();

    let (channel, received) = collecting_channel();
    let registry: State<'_, AgentChatChannelRegistry> = handle.state();
    registry.attach("thread-bridge", channel);

    forward_event(&handle, item_completed("thread-bridge", "hello"));

    let received = received.lock().unwrap();
    assert_eq!(received.len(), 1, "exactly one event should be received");
    assert_eq!(received[0].thread_id.0, "thread-bridge");
    match &received[0].event {
        ProviderRuntimeEvent::ItemCompleted { thread_id, .. } => {
            assert_eq!(thread_id.0, "thread-bridge");
        }
        other => panic!("unexpected event variant: {other:?}"),
    }
}

#[tokio::test]
async fn event_bridge_does_not_leak_across_threads() {
    // A pane attached to thread A must never see thread B's events —
    // the core no-cross-thread-leakage acceptance criterion of the
    // channel migration.
    let app = channel_test_app();
    let handle = app.handle().clone();

    let (channel_a, received_a) = collecting_channel();
    let (channel_b, received_b) = collecting_channel();
    let registry: State<'_, AgentChatChannelRegistry> = handle.state();
    registry.attach("thread-a", channel_a);
    registry.attach("thread-b", channel_b);

    forward_event(&handle, item_completed("thread-b", "for b only"));

    assert!(
        received_a.lock().unwrap().is_empty(),
        "thread-a channel must not receive thread-b events"
    );
    let received_b = received_b.lock().unwrap();
    assert_eq!(received_b.len(), 1);
    assert_eq!(received_b[0].thread_id.0, "thread-b");
}

#[tokio::test]
async fn event_bridge_stops_delivery_after_detach() {
    let app = channel_test_app();
    let handle = app.handle().clone();

    let (channel, received) = collecting_channel();
    let registry: State<'_, AgentChatChannelRegistry> = handle.state();
    let subscription_id = registry.attach("thread-detach", channel);
    assert_eq!(registry.attached_count("thread-detach"), 1);

    forward_event(&handle, item_completed("thread-detach", "first"));
    registry.detach("thread-detach", subscription_id);
    assert_eq!(registry.attached_count("thread-detach"), 0);
    forward_event(&handle, item_completed("thread-detach", "second"));

    let received = received.lock().unwrap();
    assert_eq!(
        received.len(),
        1,
        "only the pre-detach event should be delivered"
    );
}

#[tokio::test]
async fn event_bridge_preserves_delta_ordering() {
    // Channel sends are synchronous and per-channel ordered; a burst
    // of content deltas must arrive in emission order.
    let app = channel_test_app();
    let handle = app.handle().clone();

    let (channel, received) = collecting_channel();
    let registry: State<'_, AgentChatChannelRegistry> = handle.state();
    registry.attach("thread-order", channel);

    for i in 0..50 {
        forward_event(
            &handle,
            ProviderRuntimeEvent::ContentDelta {
                thread_id: ThreadId("thread-order".into()),
                turn_id: TurnId("turn-1".into()),
                delta: codemux_lib::agent_provider::ContentDelta::Text {
                    text: format!("tok-{i}"),
                },
            },
        );
    }

    let received = received.lock().unwrap();
    assert_eq!(received.len(), 50);
    for (i, payload) in received.iter().enumerate() {
        match &payload.event {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => {
                let codemux_lib::agent_provider::ContentDelta::Text { text } = delta else {
                    panic!("unexpected delta kind");
                };
                assert_eq!(text, &format!("tok-{i}"), "delta order must be preserved");
            }
            other => panic!("unexpected event variant: {other:?}"),
        }
    }
}

#[tokio::test]
async fn event_bridge_emits_threadless_warnings_on_legacy_bus() {
    // Global RuntimeWarnings have no owning thread/pane, so they keep
    // the legacy broadcast path with an empty ThreadId.
    let app = channel_test_app();
    let handle = app.handle().clone();

    let received: Arc<tokio::sync::Mutex<Vec<AgentChatEventPayload>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

    let received_clone = received.clone();
    let tx_clone = tx.clone();
    handle.listen(AGENT_CHAT_EVENT, move |event| {
        let payload: AgentChatEventPayload =
            serde_json::from_str(event.payload()).expect("valid payload JSON");
        let received = received_clone.clone();
        let tx = tx_clone.clone();
        tauri::async_runtime::spawn(async move {
            received.lock().await.push(payload);
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
        });
    });

    forward_event(
        &handle,
        ProviderRuntimeEvent::RuntimeWarning {
            thread_id: None,
            message: "global warning".into(),
            original_payload: None,
        },
    );

    timeout(Duration::from_secs(2), rx)
        .await
        .expect("listener should fire within 2s")
        .expect("one-shot sender should send before dropping");

    let received = received.lock().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].thread_id.0, "", "threadless payload uses empty id");
}
