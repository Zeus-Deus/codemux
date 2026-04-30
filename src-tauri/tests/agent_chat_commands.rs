//! Integration tests for the agent-chat command surface.
//!
//! The `#[tauri::command]` wrapper functions themselves are tied to
//! the default Tauri runtime, which a test binary cannot drive. These
//! tests therefore exercise the same business logic as each command
//! via its public helpers — the feature-flag gate, the pane-tree
//! operations on `AppStateStore`, the [`ProviderRegistry`] routing
//! with a `MockAgentProvider`, and the event-bridge forwarder
//! reached through [`forward_event`].

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
    feature_flag_on, forward_event, thread_id_for_event, AgentChatEventPayload,
    ProviderRegistry, AGENT_CHAT_EVENT, FEATURE_DISABLED_ERROR,
};
use codemux_lib::database::DatabaseStore;
use codemux_lib::observability::{FeatureFlags, ObservabilityStore};
use codemux_lib::state::AppStateStore;
use tauri::Manager;

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
        .create_agent_chat_pane(&workspace_id, None, None)
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
        .create_agent_chat_pane("ws-does-not-exist", None, None)
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
        .create_agent_chat_pane(&workspace_id, None, None)
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

#[tokio::test]
async fn event_bridge_forwards_runtime_events_to_tauri() {
    // Build a MockRuntime app, subscribe to the agent_chat_event
    // channel, and verify that forwarding a runtime event through
    // forward_event produces a matching AgentChatEventPayload.
    //
    // forward_event persists ItemCompleted via DatabaseStore::append_*,
    // so the mock app must manage a DatabaseStore or `app.state::<…>`
    // panics with "state() called before manage()". An in-memory store
    // is sufficient — we're only asserting on the emitted event, not
    // on what gets written to disk.
    let app = tauri::test::mock_app();
    app.manage(DatabaseStore::new_in_memory());
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

    let event = ProviderRuntimeEvent::ItemCompleted {
        thread_id: ThreadId("thread-bridge".into()),
        turn_id: TurnId("turn-1".into()),
        item: CompletedItem::AssistantText {
            text: "hello".into(),
        },
    };
    forward_event(&handle, event);

    timeout(Duration::from_secs(2), rx)
        .await
        .expect("listener should fire within 2s")
        .expect("one-shot sender should send before dropping");

    let received = received.lock().await;
    assert_eq!(received.len(), 1, "exactly one event should be received");
    assert_eq!(received[0].thread_id.0, "thread-bridge");
    match &received[0].event {
        ProviderRuntimeEvent::ItemCompleted { thread_id, .. } => {
            assert_eq!(thread_id.0, "thread-bridge");
        }
        other => panic!("unexpected event variant: {other:?}"),
    }
}
