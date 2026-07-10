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

use tauri::ipc::Channel;
use tauri::{Listener, State};
use tokio::time::timeout;

use codemux_lib::agent_provider::{
    ApprovalDecision, CompletedItem, ProviderKind, ProviderRuntimeEvent, RequestId,
    SendTurnInput, StartSessionInput, ThreadId, TurnId,
};
use codemux_lib::commands::agent_chat::{
    feature_flag_on, forward_event, thread_id_for_event, AgentChatChannelRegistry,
    AgentChatEventPayload, ProviderRegistry, RunActivityTracker, SubagentTracker,
    AGENT_CHAT_EVENT, FEATURE_DISABLED_ERROR,
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
            client_nonce: None,
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
        subagent_id: None,
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

/// Build a MockRuntime app with the managed state `forward_event`
/// touches: an in-memory DatabaseStore (transcript persistence), the
/// per-thread channel registry (issue #75 routing), and the
/// AppStateStore (chat sessions publish their lifecycle into
/// `pane_statuses` so the sidebar shows working/needs-input/review).
fn mock_app_with_chat_state() -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    app.manage(DatabaseStore::new_in_memory());
    app.manage(AgentChatChannelRegistry::default());
    app.manage(SubagentTracker::default());
    app.manage(RunActivityTracker::default());
    app.manage(AppStateStore::default());
    app
}

/// Real `tauri::ipc::Channel` whose handler decodes and captures every
/// payload, mirroring what the frontend's `useAgentChatEvents` channel
/// receives.
fn capture_channel() -> (
    Channel<AgentChatEventPayload>,
    Arc<std::sync::Mutex<Vec<AgentChatEventPayload>>>,
) {
    let captured: Arc<std::sync::Mutex<Vec<AgentChatEventPayload>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_handler = captured.clone();
    let channel = Channel::new(move |body| {
        let payload = body
            .deserialize::<AgentChatEventPayload>()
            .expect("decode AgentChatEventPayload");
        captured_handler.lock().unwrap().push(payload);
        Ok(())
    });
    (channel, captured)
}

fn text_delta(thread: &str, text: &str) -> ProviderRuntimeEvent {
    ProviderRuntimeEvent::ContentDelta {
        subagent_id: None,
        thread_id: ThreadId(thread.into()),
        turn_id: TurnId("turn-1".into()),
        delta: codemux_lib::agent_provider::ContentDelta::Text { text: text.into() },
    }
}

#[tokio::test]
async fn event_bridge_routes_thread_events_to_attached_channel() {
    // Thread-scoped events must arrive over the per-thread Channel —
    // and must NOT be broadcast on the global agent_chat_event bus.
    let app = mock_app_with_chat_state();
    let handle = app.handle().clone();

    // Bus spy: any thread-scoped event landing here is a regression.
    let bus_hits = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let bus_hits_clone = bus_hits.clone();
    handle.listen(AGENT_CHAT_EVENT, move |event| {
        bus_hits_clone
            .lock()
            .unwrap()
            .push(event.payload().to_string());
    });

    let registry: State<'_, AgentChatChannelRegistry> = handle.state();
    let (channel, captured) = capture_channel();
    registry.attach("thread-bridge", channel);

    let event = ProviderRuntimeEvent::ItemCompleted {
        subagent_id: None,
        thread_id: ThreadId("thread-bridge".into()),
        turn_id: TurnId("turn-1".into()),
        item: CompletedItem::AssistantText {
            text: "hello".into(),
        },
    };
    forward_event(&handle, event);

    // Channel delivery is synchronous in the mock runtime (the closure
    // runs inline inside send), so the capture is observable now.
    let received = captured.lock().unwrap();
    assert_eq!(received.len(), 1, "exactly one event over the channel");
    assert_eq!(received[0].thread_id.0, "thread-bridge");
    match &received[0].event {
        ProviderRuntimeEvent::ItemCompleted { thread_id, .. } => {
            assert_eq!(thread_id.0, "thread-bridge");
        }
        other => panic!("unexpected event variant: {other:?}"),
    }
    drop(received);

    // Give the (async) event bus a moment, then assert silence.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        bus_hits.lock().unwrap().is_empty(),
        "thread-scoped events must not ride the global event bus"
    );
}

#[tokio::test]
async fn event_bridge_isolates_threads_and_preserves_order() {
    // Two panes attached to two threads: each receives only its own
    // thread's token stream, in emission order.
    let app = mock_app_with_chat_state();
    let handle = app.handle().clone();

    let registry: State<'_, AgentChatChannelRegistry> = handle.state();
    let (channel_a, captured_a) = capture_channel();
    let (channel_b, captured_b) = capture_channel();
    registry.attach("thread-a", channel_a);
    registry.attach("thread-b", channel_b);

    for i in 0..50 {
        forward_event(&handle, text_delta("thread-a", &format!("a{i}")));
        forward_event(&handle, text_delta("thread-b", &format!("b{i}")));
    }

    let a = captured_a.lock().unwrap();
    let b = captured_b.lock().unwrap();
    assert_eq!(a.len(), 50, "pane A sees exactly its own 50 deltas");
    assert_eq!(b.len(), 50, "pane B sees exactly its own 50 deltas");
    for (i, payload) in a.iter().enumerate() {
        assert_eq!(payload.thread_id.0, "thread-a", "no cross-thread leakage");
        match &payload.event {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                codemux_lib::agent_provider::ContentDelta::Text { text } => {
                    assert_eq!(text, &format!("a{i}"), "ordering preserved");
                }
                other => panic!("unexpected delta: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }
    for payload in b.iter() {
        assert_eq!(payload.thread_id.0, "thread-b", "no cross-thread leakage");
    }
}

#[tokio::test]
async fn event_bridge_persists_transcript_even_without_channel() {
    // The replay-semantics split: transcript events are persisted to
    // the DB regardless of whether a pane is attached, so a
    // late-attaching pane can hydrate the full transcript and the
    // channel only ever needs to carry live events.
    let app = mock_app_with_chat_state();
    let handle = app.handle().clone();

    // append_agent_chat_message silently drops rows whose parent
    // session is missing (FK), so seed the session row the way
    // agent_chat_start_session does.
    {
        let db: State<'_, DatabaseStore> = handle.state();
        db.upsert_agent_chat_session("thread-unattached", "ws-1", None, "claude")
            .expect("seed session row");
    }

    let event = ProviderRuntimeEvent::ItemCompleted {
        subagent_id: None,
        thread_id: ThreadId("thread-unattached".into()),
        turn_id: TurnId("turn-1".into()),
        item: CompletedItem::AssistantText {
            text: "persisted while nobody watched".into(),
        },
    };
    // No channel attached — must not panic, must still persist.
    forward_event(&handle, event);

    let db: State<'_, DatabaseStore> = handle.state();
    let messages = db.list_agent_chat_messages("thread-unattached");
    assert_eq!(messages.len(), 1, "transcript persisted without subscriber");
    assert!(messages[0].contains("persisted while nobody watched"));
}

#[tokio::test]
async fn event_bridge_emits_threadless_warnings_on_event_bus() {
    // Global RuntimeWarnings have no thread to route by; they keep the
    // low-frequency global event bus.
    let app = mock_app_with_chat_state();
    let handle = app.handle().clone();

    let (tx, rx) = tokio::sync::oneshot::channel::<AgentChatEventPayload>();
    let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
    handle.listen(AGENT_CHAT_EVENT, move |event| {
        let payload: AgentChatEventPayload =
            serde_json::from_str(event.payload()).expect("valid payload JSON");
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(payload);
        }
    });

    forward_event(
        &handle,
        ProviderRuntimeEvent::RuntimeWarning {
            thread_id: None,
            message: "global warning".into(),
            original_payload: None,
        },
    );

    let payload = timeout(Duration::from_secs(2), rx)
        .await
        .expect("bus listener should fire within 2s")
        .expect("one-shot sender should send before dropping");
    assert_eq!(payload.thread_id.0, "", "thread-less payload has empty id");
    match payload.event {
        ProviderRuntimeEvent::RuntimeWarning { message, .. } => {
            assert_eq!(message, "global warning");
        }
        other => panic!("unexpected event variant: {other:?}"),
    }
}

/// Bind a chat pane to `thread` inside `state` and return its pane id.
/// The pane must carry a thread id for the status walker to resolve it.
fn bind_chat_pane(state: &AppStateStore, workspace_id: &str, thread: &str) -> String {
    let pane = state
        .create_agent_chat_pane(workspace_id, None, None, None)
        .expect("create_agent_chat_pane");
    state.set_agent_chat_thread_id(&pane.0, Some(thread.into()));
    pane.0
}

#[tokio::test]
async fn forward_event_publishes_status_into_pane_statuses() {
    // A chat session's lifecycle must land in the shared pane_statuses
    // store so the sidebar renders working/needs-input/review for chat
    // workspaces, exactly like terminal agents.
    let app = mock_app_with_chat_state();
    let handle = app.handle().clone();
    let state: State<'_, AppStateStore> = handle.state();

    // Put the chat pane in a NON-active workspace so a completed turn
    // resolves to Review (the active-workspace path downgrades to Idle,
    // covered separately below).
    let bg_ws = state
        .create_workspace_with_layout(
            std::path::PathBuf::from("/tmp/codemux-chat-status-bg"),
            codemux_lib::state::WorkspacePresetLayout::Single,
        )
        .0;
    let pane = bind_chat_pane(&state, &bg_ws, "thread-status");
    // Foreground a different workspace so `bg_ws` is not active.
    let fg_ws = state
        .create_workspace_with_layout(
            std::path::PathBuf::from("/tmp/codemux-chat-status-fg"),
            codemux_lib::state::WorkspacePresetLayout::Single,
        )
        .0;
    state.activate_workspace(&fg_ws);

    let status_of = |pane: &str| state.snapshot().pane_statuses.get(pane).cloned();

    // Streaming deltas → Working.
    forward_event(&handle, text_delta("thread-status", "hello"));
    assert_eq!(status_of(&pane), Some(codemux_lib::state::PaneStatus::Working));

    // A pending approval → Permission.
    forward_event(
        &handle,
        ProviderRuntimeEvent::RequestOpened {
            thread_id: ThreadId("thread-status".into()),
            turn_id: TurnId("turn-1".into()),
            request_id: RequestId("req-1".into()),
            request_kind: "tool".into(),
            payload: serde_json::json!({}),
            tool_use_id: None,
            subagent_id: None,
        },
    );
    assert_eq!(
        status_of(&pane),
        Some(codemux_lib::state::PaneStatus::Permission)
    );

    // Turn finishes while the workspace is NOT active → Review.
    forward_event(
        &handle,
        ProviderRuntimeEvent::TurnCompleted {
            thread_id: ThreadId("thread-status".into()),
            turn_id: TurnId("turn-1".into()),
            status: codemux_lib::agent_provider::TurnStatus::Success,
            usage: None,
        },
    );
    assert_eq!(status_of(&pane), Some(codemux_lib::state::PaneStatus::Review));

    // Session closes → entry cleared.
    forward_event(
        &handle,
        ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ThreadId("thread-status".into()),
            status: codemux_lib::agent_provider::SessionStatus::Closed,
        },
    );
    assert_eq!(status_of(&pane), None, "closed session clears the indicator");
}

#[tokio::test]
async fn forward_event_downgrades_review_to_idle_in_active_workspace() {
    // Parity with the terminal path: a turn that finishes in the
    // workspace the user is already looking at clears to Idle instead of
    // nagging with a review dot.
    let app = mock_app_with_chat_state();
    let handle = app.handle().clone();
    let state: State<'_, AppStateStore> = handle.state();

    let active_ws = state.snapshot().active_workspace_id.0;
    let pane = bind_chat_pane(&state, &active_ws, "thread-focused");
    // create_agent_chat_pane keeps its workspace active; assert that.
    assert!(state.is_thread_pane_in_active_workspace("thread-focused"));

    forward_event(&handle, text_delta("thread-focused", "hi"));
    assert_eq!(
        state.snapshot().pane_statuses.get(&pane).cloned(),
        Some(codemux_lib::state::PaneStatus::Working)
    );

    forward_event(
        &handle,
        ProviderRuntimeEvent::TurnCompleted {
            thread_id: ThreadId("thread-focused".into()),
            turn_id: TurnId("turn-1".into()),
            status: codemux_lib::agent_provider::TurnStatus::Success,
            usage: None,
        },
    );
    assert_eq!(
        state.snapshot().pane_statuses.get(&pane).cloned(),
        None,
        "completed turn in the active workspace clears instead of Review"
    );
}

#[tokio::test]
async fn forward_event_ignores_status_for_unbound_threads() {
    // An event for a thread with no resolvable chat pane must not create
    // a spurious pane_statuses entry (and must not panic).
    let app = mock_app_with_chat_state();
    let handle = app.handle().clone();
    let state: State<'_, AppStateStore> = handle.state();

    forward_event(&handle, text_delta("no-such-thread", "hi"));
    assert!(
        state.snapshot().pane_statuses.is_empty(),
        "unbound thread must not seed a status entry"
    );
}

#[tokio::test]
async fn full_bridge_pipeline_streams_provider_events_per_thread() {
    // End-to-end through the real pipeline: provider broadcast →
    // spawn_event_bridge task → forward_event → per-thread Channel.
    // This is the same path a live Claude/Codex session takes, with
    // only the provider mocked.
    let app = mock_app_with_chat_state();
    app.manage(ProviderRegistry::new());
    let handle = app.handle().clone();

    let registry: State<'_, ProviderRegistry> = handle.state();
    let provider = Arc::new(MockAgentProvider::new(ProviderKind::Claude));
    registry.set_claude(provider.clone() as _).await;

    codemux_lib::commands::agent_chat::spawn_event_bridge(handle.clone()).await;

    let channels: State<'_, AgentChatChannelRegistry> = handle.state();
    let (channel_a, captured_a) = capture_channel();
    let (channel_b, captured_b) = capture_channel();
    channels.attach("pipe-a", channel_a);
    channels.attach("pipe-b", channel_b);

    // Interleave token streams for two threads plus a completion.
    for i in 0..20 {
        provider.emit(text_delta("pipe-a", &format!("tok-a{i}")));
        provider.emit(text_delta("pipe-b", &format!("tok-b{i}")));
    }
    provider.emit(ProviderRuntimeEvent::ItemCompleted {
        subagent_id: None,
        thread_id: ThreadId("pipe-a".into()),
        turn_id: TurnId("turn-1".into()),
        item: CompletedItem::AssistantText {
            text: "done".into(),
        },
    });

    // The bridge task consumes the broadcast asynchronously — poll
    // until everything has flowed through (bounded by a timeout).
    timeout(Duration::from_secs(5), async {
        loop {
            if captured_a.lock().unwrap().len() == 21
                && captured_b.lock().unwrap().len() == 20
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bridge should deliver all events within 5s");

    let a = captured_a.lock().unwrap();
    assert!(a.iter().all(|p| p.thread_id.0 == "pipe-a"));
    // Token ordering survives the async hop.
    for (i, payload) in a.iter().take(20).enumerate() {
        match &payload.event {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                codemux_lib::agent_provider::ContentDelta::Text { text } => {
                    assert_eq!(text, &format!("tok-a{i}"));
                }
                other => panic!("unexpected delta: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }
    match &a[20].event {
        ProviderRuntimeEvent::ItemCompleted { .. } => {}
        other => panic!("expected trailing ItemCompleted, got {other:?}"),
    }
    let b = captured_b.lock().unwrap();
    assert!(b.iter().all(|p| p.thread_id.0 == "pipe-b"));
}

// ── Backend auto-resume after a restart ──
//
// The bug: after the app is closed and reopened, the provider's live
// session map is empty (no startup rehydration) but the persisted
// `agent_chat_sessions` row survives. Every turn on the reopened pane
// used to hit `session_not_found`. `ensure_live_session` must transparently
// rebuild the session from that row — reusing the SAME thread_id, resuming
// from the persisted `sdk_session_id`, with the persisted model — so the
// turn goes through.
//
// Drives the REAL `ClaudeAgentProvider` against the `fake_claude_sidecar`
// (script-driven, no SDK / Anthropic account) so we can assert on exactly
// what `start-session` the auto-resume sent.

mod auto_resume {
    use std::path::PathBuf;
    use std::sync::Arc;

    use codemux_lib::agent_provider::claude::{ClaudeAgentProvider, ClaudeProviderConfig};
    use codemux_lib::agent_provider::{AgentProvider, ProviderKind, SendTurnInput, ThreadId};
    use codemux_lib::commands::agent_chat::{ensure_live_session, ProviderRegistry};
    use codemux_lib::database::{AgentChatSessionConfig, DatabaseStore};
    use codemux_lib::state::AppStateStore;
    use serde_json::Value;
    use tauri::Manager;

    fn fake_sidecar() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_fake_claude_sidecar"))
    }

    /// Minimal POSIX shell escaper, mirroring the local module in
    /// `claude_adapter.rs` (which is a test-local module, not a crate).
    mod shell_escape {
        pub fn escape<S: AsRef<str>>(s: S) -> String {
            let s = s.as_ref();
            if s.is_empty() {
                return "''".into();
            }
            if s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-/=".contains(c))
            {
                return s.to_string();
            }
            let mut out = String::from("'");
            for c in s.chars() {
                if c == '\'' {
                    out.push_str("'\\''");
                } else {
                    out.push(c);
                }
            }
            out.push('\'');
            out
        }
    }

    /// Bash wrapper that exports `FAKE_CLAUDE_SIDECAR_CAPTURE=<path>` then
    /// `exec`s the fake sidecar, so the sidecar records every received
    /// `start-session`'s params to `capture` for later assertion. Mirrors
    /// `claude_adapter.rs::wrapper_with_env`. Written into `dir` (a
    /// caller-owned tempdir that must outlive the spawned session).
    fn capture_wrapper(dir: &std::path::Path, capture: &std::path::Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("wrap.sh");
        let body = format!(
            "#!/usr/bin/env bash\nset -e\nexport FAKE_CLAUDE_SIDECAR_CAPTURE={}\nexec {} \"$@\"\n",
            shell_escape::escape(capture.to_string_lossy()),
            shell_escape::escape(fake_sidecar().to_string_lossy()),
        );
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    async fn claude_provider_with_capture(wrapper: PathBuf) -> Arc<ClaudeAgentProvider> {
        Arc::new(
            ClaudeAgentProvider::new(ClaudeProviderConfig {
                sidecar_binary: Some(wrapper),
                claude_binary: Some(PathBuf::from("/usr/bin/claude")),
                event_channel_capacity: 1024,
                mcp_registry: None,
            })
            .await
            .expect("provider"),
        )
    }

    fn read_capture(path: &std::path::Path) -> Vec<Value> {
        let raw = std::fs::read_to_string(path).unwrap_or_default();
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).expect("valid capture json"))
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_after_dead_session_auto_resumes_with_persisted_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let capture = tmp.path().join("start-session-capture.jsonl");
        let cwd = tmp.path().join("workdir");
        std::fs::create_dir_all(&cwd).unwrap();
        let wrapper = capture_wrapper(tmp.path(), &capture);

        // Managed state the command path touches. A fresh provider whose
        // session map is EMPTY models the post-restart process.
        let app = tauri::test::mock_app();
        app.manage(DatabaseStore::new_in_memory());
        app.manage(AppStateStore::default());
        app.manage(ProviderRegistry::new());
        let handle = app.handle().clone();

        let provider = claude_provider_with_capture(wrapper).await;
        {
            let registry: tauri::State<'_, ProviderRegistry> = handle.state();
            registry.set_claude(provider.clone() as Arc<dyn AgentProvider>).await;
        }

        // Seed the persisted row exactly as it would survive a restart:
        // a live sdk_session_id + a chosen model, but NO live session.
        let thread = ThreadId("chat-pane-448-1783369893274".into());
        {
            let db: tauri::State<'_, DatabaseStore> = handle.state();
            db.upsert_agent_chat_session(
                &thread.0,
                "ws-1",
                Some(&cwd.to_string_lossy()),
                "claude",
            )
            .unwrap();
            db.set_agent_chat_sdk_session_id(&thread.0, "sdk-uuid-123")
                .unwrap();
            db.update_agent_chat_session_config(
                &thread.0,
                &AgentChatSessionConfig {
                    model: AgentChatSessionConfig::set("claude-opus-4-8"),
                    ..AgentChatSessionConfig::default()
                },
            )
            .unwrap();
        }

        // Precondition: the map is empty (the restart bug's starting point).
        assert!(
            !provider.has_session(&thread).await,
            "no live session should exist before auto-resume"
        );

        // The choke point: rebuild the dead session from the DB row.
        ensure_live_session(&handle, ProviderKind::Claude, &thread)
            .await
            .expect("auto-resume should succeed");

        // The session is live again under the SAME thread_id.
        assert!(
            provider.has_session(&thread).await,
            "auto-resume must rebind a live session to the same thread_id"
        );

        // The fake sidecar received a start-session carrying the persisted
        // resume cursor and model.
        let captured = read_capture(&capture);
        assert_eq!(captured.len(), 1, "exactly one start-session was sent");
        let params = &captured[0]["params"];
        assert_eq!(
            params["threadId"].as_str(),
            Some(thread.0.as_str()),
            "resumed session reuses the same thread_id"
        );
        assert_eq!(
            params["resume"].as_str(),
            Some("sdk-uuid-123"),
            "resume cursor threaded through from the persisted sdk_session_id"
        );
        assert_eq!(
            params["model"].as_str(),
            Some("claude-opus-4-8"),
            "persisted model threaded through to the SDK start-session"
        );

        // And a turn now goes through instead of session_not_found.
        provider
            .send_turn(SendTurnInput {
                thread_id: thread.clone(),
                text: "still here after restart?".into(),
                images: vec![],
                model_override: None,
                effort_override: None,
                permission_mode_override: None,
                client_nonce: None,
            })
            .await
            .expect("send_turn succeeds on the resumed session");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ensure_live_session_is_a_noop_when_session_already_live() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let capture = tmp.path().join("capture.jsonl");
        let cwd = tmp.path().join("workdir");
        std::fs::create_dir_all(&cwd).unwrap();
        let wrapper = capture_wrapper(tmp.path(), &capture);

        let app = tauri::test::mock_app();
        app.manage(DatabaseStore::new_in_memory());
        app.manage(AppStateStore::default());
        app.manage(ProviderRegistry::new());
        let handle = app.handle().clone();

        let provider = claude_provider_with_capture(wrapper).await;
        {
            let registry: tauri::State<'_, ProviderRegistry> = handle.state();
            registry.set_claude(provider.clone() as Arc<dyn AgentProvider>).await;
        }

        let thread = ThreadId("thread-live".into());
        {
            let db: tauri::State<'_, DatabaseStore> = handle.state();
            db.upsert_agent_chat_session(&thread.0, "ws-1", Some(&cwd.to_string_lossy()), "claude")
                .unwrap();
        }

        // Start a real session so the map has an entry.
        provider
            .start_session(codemux_lib::agent_provider::StartSessionInput {
                thread_id: thread.clone(),
                cwd: cwd.clone(),
                model: None,
                resume_cursor: None,
                permission_mode: None,
                effort: None,
                context_window: None,
                additional_directories: vec![],
                env: None,
                extra: Value::Null,
            })
            .await
            .expect("start ok");
        let captures_after_start = read_capture(&capture).len();

        // ensure_live_session must NOT start a second session.
        ensure_live_session(&handle, ProviderKind::Claude, &thread)
            .await
            .expect("no-op ok");
        assert_eq!(
            read_capture(&capture).len(),
            captures_after_start,
            "a live session must not be restarted by ensure_live_session"
        );
    }
}

// ── Run checkpoints (issue #80) ──
//
// End-to-end backend round trip through the same blocking helpers the
// Tauri commands and the start-session background task call: a REAL
// temp git repo + an in-memory DatabaseStore. Create a checkpoint,
// simulate an agent trashing the workspace (edits, deletions, new
// files, a commit), restore, and verify the pre-run state is back.

mod run_checkpoints {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use codemux_lib::commands::agent_chat::{
        create_run_checkpoint_blocking, restore_run_checkpoint_blocking,
    };
    use codemux_lib::database::DatabaseStore;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    fn setup_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().to_path_buf();
        git(&path, &["init", "-b", "main"]);
        git(&path, &["config", "user.name", "Test"]);
        git(&path, &["config", "user.email", "t@t.t"]);
        std::fs::write(path.join("code.txt"), "original").unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "base"]);
        (dir, path)
    }

    fn session_db(thread_id: &str, repo: &Path) -> DatabaseStore {
        let db = DatabaseStore::new_in_memory();
        // The checkpoint row FKs onto the session row, mirroring the
        // production ordering (session persisted before the
        // checkpoint task runs).
        db.upsert_agent_chat_session(
            thread_id,
            "ws-1",
            Some(&repo.to_string_lossy()),
            "claude",
        )
        .expect("session row");
        db
    }

    #[test]
    fn checkpoint_round_trip_restores_pre_run_state() {
        let (_dir, repo) = setup_repo();
        let db = session_db("thread-cp", &repo);

        // Pre-run dirty state: unstaged edit + untracked file.
        std::fs::write(repo.join("code.txt"), "user-edit").unwrap();
        std::fs::write(repo.join("scratch.txt"), "user-notes").unwrap();

        let record = create_run_checkpoint_blocking(&db, &repo, "thread-cp", "ws-1")
            .expect("create ok")
            .expect("repo is snapshottable");
        assert_eq!(record.thread_id, "thread-cp");
        assert_eq!(record.workspace_id, "ws-1");
        assert!(!record.created_at.is_empty(), "created_at re-read from DB");
        assert_eq!(record.branch.as_deref(), Some("main"));
        // Recorded in the DB and anchored in the repo.
        let stored = db
            .get_agent_chat_checkpoint("thread-cp")
            .expect("row persisted");
        assert_eq!(stored.snapshot_commit, record.snapshot_commit);
        assert_eq!(
            git(&repo, &["rev-parse", &stored.ref_name]),
            stored.snapshot_commit
        );

        // The checkpoint did not disturb the user's state.
        let status = git(&repo, &["status", "--porcelain"]);
        assert!(status.contains(" M code.txt"), "got: {status}");
        assert!(status.contains("?? scratch.txt"), "got: {status}");

        // Simulated agent run.
        std::fs::write(repo.join("code.txt"), "agent-rewrite").unwrap();
        std::fs::remove_file(repo.join("scratch.txt")).unwrap();
        std::fs::write(repo.join("agent.txt"), "artifact").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-m", "agent went wild"]);

        restore_run_checkpoint_blocking(&db, "thread-cp").expect("restore ok");

        assert_eq!(
            std::fs::read_to_string(repo.join("code.txt")).unwrap(),
            "user-edit"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("scratch.txt")).unwrap(),
            "user-notes"
        );
        assert!(!repo.join("agent.txt").exists(), "agent artifact removed");
        assert_eq!(
            git(&repo, &["rev-parse", "HEAD"]),
            record.head_commit,
            "agent commit undone"
        );
    }

    #[test]
    fn checkpoint_skips_non_repo_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = DatabaseStore::new_in_memory();
        let result = create_run_checkpoint_blocking(&db, dir.path(), "t", "ws")
            .expect("non-repo must not error");
        assert!(result.is_none(), "non-repo is a silent skip");
        assert!(db.get_agent_chat_checkpoint("t").is_none());
    }

    #[test]
    fn restore_without_checkpoint_errors_cleanly() {
        let db = DatabaseStore::new_in_memory();
        let err = restore_run_checkpoint_blocking(&db, "unknown-thread")
            .expect_err("no checkpoint recorded");
        assert!(err.contains("No checkpoint"), "got: {err}");
    }

    #[test]
    fn second_run_checkpoint_replaces_the_first() {
        let (_dir, repo) = setup_repo();
        let db = session_db("thread-a", &repo);

        let first = create_run_checkpoint_blocking(&db, &repo, "thread-a", "ws-1")
            .unwrap()
            .unwrap();
        std::fs::write(repo.join("code.txt"), "later").unwrap();
        let second = create_run_checkpoint_blocking(&db, &repo, "thread-a", "ws-1")
            .unwrap()
            .unwrap();
        assert_ne!(first.snapshot_commit, second.snapshot_commit);
        let stored = db.get_agent_chat_checkpoint("thread-a").unwrap();
        assert_eq!(stored.snapshot_commit, second.snapshot_commit);
    }
}

/// Full production glue for the background run checkpoint (issue #80):
/// `spawn_run_checkpoint_with_gate` on a mock app — async spawn →
/// blocking pool → REAL git snapshot → DB row through managed state →
/// `agent_chat_checkpoint` event emission. Only the settings-cache
/// read is injected (gate fn), so the test never touches the user's
/// real settings cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_checkpoint_spawn_persists_and_emits_event() {
    use codemux_lib::commands::agent_chat::{
        spawn_run_checkpoint_with_gate, AgentChatCheckpointEventPayload,
        AGENT_CHAT_CHECKPOINT_EVENT,
    };

    // Real repo with a dirty worktree.
    let dir = tempfile::TempDir::new().expect("temp dir");
    let repo = dir.path().to_path_buf();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "t@t.t"]);
    std::fs::write(repo.join("f.txt"), "v1").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "base"]);
    std::fs::write(repo.join("f.txt"), "dirty").unwrap();

    let app = tauri::test::mock_app();
    let db = DatabaseStore::new_in_memory();
    db.upsert_agent_chat_session(
        "thread-bg",
        "ws-1",
        Some(&repo.to_string_lossy()),
        "claude",
    )
    .expect("session row");
    app.manage(db);
    let handle = app.handle().clone();

    let (tx, rx) = tokio::sync::oneshot::channel::<AgentChatCheckpointEventPayload>();
    let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
    handle.listen(AGENT_CHAT_CHECKPOINT_EVENT, move |event| {
        let payload: AgentChatCheckpointEventPayload =
            serde_json::from_str(event.payload()).expect("valid payload JSON");
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(payload);
        }
    });

    spawn_run_checkpoint_with_gate(
        &handle,
        "thread-bg".to_string(),
        "ws-1".to_string(),
        repo.to_string_lossy().to_string(),
        || true,
    );

    let payload = timeout(Duration::from_secs(10), rx)
        .await
        .expect("checkpoint event should fire within 10s")
        .expect("sender not dropped");
    assert_eq!(payload.thread_id.0, "thread-bg");
    assert_eq!(payload.checkpoint.workspace_id, "ws-1");
    assert!(!payload.checkpoint.snapshot_commit.is_empty());

    // The row is queryable through the same managed state the restore
    // command uses.
    let db = handle.state::<DatabaseStore>();
    let stored = db
        .get_agent_chat_checkpoint("thread-bg")
        .expect("row persisted by the background task");
    assert_eq!(stored.snapshot_commit, payload.checkpoint.snapshot_commit);
    // And the snapshot is a real commit anchored in the repo.
    let out = std::process::Command::new("git")
        .args(["cat-file", "-t", &stored.snapshot_commit])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "commit");
}

/// The gate is evaluated INSIDE the background task: when it reports
/// "feature off", nothing is written and no event fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_checkpoint_spawn_is_a_noop_when_gate_is_off() {
    use codemux_lib::commands::agent_chat::spawn_run_checkpoint_with_gate;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let app = tauri::test::mock_app();
    app.manage(DatabaseStore::new_in_memory());
    let handle = app.handle().clone();

    spawn_run_checkpoint_with_gate(
        &handle,
        "thread-off".to_string(),
        "ws-1".to_string(),
        dir.path().to_string_lossy().to_string(),
        || false,
    );

    // Give the spawned task time to run, then assert nothing landed.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let db = handle.state::<DatabaseStore>();
    assert!(db.get_agent_chat_checkpoint("thread-off").is_none());
}
