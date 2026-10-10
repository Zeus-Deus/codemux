use super::*;
use crate::agent_provider::*;
use crate::commands::agent_chat::{self, ProviderRegistry};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tauri::Manager;

#[allow(dead_code)]
#[path = "../../tests/helpers/mock_agent_provider.rs"]
mod mock_agent_provider;

struct HeldStartProvider {
    inner: mock_agent_provider::MockAgentProvider,
    starts: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    response_error: std::sync::Mutex<Option<ProviderError>>,
    interrupt_error: std::sync::Mutex<Option<ProviderError>>,
}
impl HeldStartProvider {
    fn new() -> Self {
        Self {
            inner: mock_agent_provider::MockAgentProvider::new(ProviderKind::Claude),
            starts: AtomicUsize::new(0),
            entered: Default::default(),
            release: tokio::sync::Semaphore::new(0),
            response_error: std::sync::Mutex::new(None),
            interrupt_error: std::sync::Mutex::new(None),
        }
    }
}
#[async_trait::async_trait]
impl AgentProvider for HeldStartProvider {
    fn kind(&self) -> ProviderKind {
        self.inner.kind()
    }
    fn capabilities(&self) -> ProviderCapabilities {
        self.inner.capabilities()
    }
    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        self.inner.start_session(input).await
    }
    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        self.inner.send_turn(input).await
    }
    async fn interrupt_turn(&self, t: ThreadId, turn: Option<TurnId>) -> Result<(), ProviderError> {
        if let Some(error) = self.interrupt_error.lock().unwrap().take() { return Err(error); }
        self.inner.interrupt_turn(t, turn).await
    }
    async fn respond_to_request(
        &self,
        t: ThreadId,
        r: RequestId,
        d: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        if let Some(error) = self.response_error.lock().unwrap().take() {
            return Err(error);
        }
        self.inner.respond_to_request(t, r, d).await
    }
    async fn set_model(&self, t: ThreadId, m: String) -> Result<(), ProviderError> {
        self.inner.set_model(t, m).await
    }
    async fn set_permission_mode(&self, t: ThreadId, m: String) -> Result<(), ProviderError> {
        self.inner.set_permission_mode(t, m).await
    }
    async fn stop_session(&self, t: ThreadId) -> Result<(), ProviderError> {
        self.inner.stop_session(t).await
    }
    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        self.inner.list_sessions().await
    }
    async fn has_session(&self, t: &ThreadId) -> bool {
        self.inner.has_session(t).await
    }
    fn event_stream(&self) -> ProviderEventStream {
        self.inner.event_stream()
    }
}

pub(crate) async fn fixture(
    provider: Arc<dyn AgentProvider>,
    thread: &str,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tempfile::TempDir,
    String,
    String,
) {
    fixture_with_root(provider, thread, tempfile::tempdir().unwrap()).await
}

async fn fixture_with_root(
    provider: Arc<dyn AgentProvider>,
    thread: &str,
    root: tempfile::TempDir,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tempfile::TempDir,
    String,
    String,
) {
    let app = tauri::test::mock_app();
    let state = crate::state::AppStateStore::default();
    let workspace = state
        .create_empty_workspace_at_path(root.path().to_path_buf())
        .0;
    let pane = state
        .create_agent_chat_pane(
            &workspace,
            Some(ProviderKind::Claude),
            Some(root.path().to_string_lossy().into_owned()),
            None,
            Some(thread.into()),
        )
        .unwrap()
        .0;
    app.manage(state);
    app.manage(crate::database::DatabaseStore::new_in_memory());
    let observability = crate::observability::ObservabilityStore::default();
    let mut flags = observability.feature_flags();
    flags.enable_agent_chat = true;
    observability.set_feature_flags(flags);
    app.manage(observability);
    app.manage(ProviderRegistry::new());
    app.state::<ProviderRegistry>().set_claude(provider).await;
    app.manage(agent_chat::SubagentTracker::default());
    app.manage(agent_chat::RunActivityTracker::default());
    app.manage(agent_chat::AgentChatChannelRegistry::default());
    app.manage(crate::mcp::registry::McpRegistry::default());
    app.state::<crate::mcp::registry::McpRegistry>()
        .insert_running_server_for_test("codemux-self", vec![crate::mcp::McpConfigSource::Codemux])
        .await;
    app.manage(NativeControlState::default());
    let db = app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session(
        thread,
        &workspace,
        Some(root.path().to_str().unwrap()),
        "claude",
    )
    .unwrap();
    db.update_agent_chat_session_config(
        thread,
        &crate::database::AgentChatSessionConfig {
            permission_mode: Some(Some("default".into())),
            ..Default::default()
        },
    )
    .unwrap();
    (app, root, workspace, pane)
}
fn start(thread: &str, cwd: &std::path::Path) -> StartSessionInput {
    StartSessionInput {
        thread_id: ThreadId(thread.into()),
        cwd: cwd.into(),
        model: None,
        resume_cursor: None,
        fresh_session: false,
        permission_mode: Some("default".into()),
        effort: None,
        context_window: None,
        fast_mode: false,
        additional_directories: vec![],
        env: None,
        workspace_id: None,
        extra: serde_json::Value::Null,
        recorded_usage_baseline: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_ui_start_and_auto_resume_launch_one_session() {
    let provider = Arc::new(HeldStartProvider::new());
    let (app, root, _, pane) = fixture(provider.clone(), "launch-race").await;
    let handle = app.handle().clone();
    let input = start("launch-race", root.path());
    let first = tokio::spawn(async move {
        agent_chat::agent_chat_start_session(handle, pane, ProviderKind::Claude, input, None).await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        provider.entered.notified(),
    )
    .await
    .unwrap();
    let handle = app.handle().clone();
    let second = tokio::spawn(async move {
        agent_chat::ensure_live_session(
            &handle,
            ProviderKind::Claude,
            &ThreadId("launch-race".into()),
        )
        .await
    });
    // If resume reaches start_session before the first publishes its session,
    // it lacks the UI start's lifecycle owner. A short timeout is a negative
    // concurrency assertion; both tasks are released before checking results.
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(80),
        provider.entered.notified(),
    )
    .await;
    provider.release.add_permits(2);
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(
        provider.starts.load(Ordering::SeqCst),
        1,
        "UI start and automatic resume must share a launch gate"
    );
}

fn guard(workspace: &str, thread: &str) -> NativeControlGuard {
    NativeControlGuard {
        caller: ControlCaller::trusted(),
        workspace_id: workspace.into(),
        thread_id: thread.into(),
        provider: ProviderKind::Claude,
        admission: None,
    }
}

#[tokio::test]
async fn guarded_stale_approval_is_an_error_without_restarting_callback_owner() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, _root, workspace, _pane) = fixture(provider.clone(), "stale-approval").await;
    let result = agent_chat::agent_chat_respond_to_request_guarded(
        app.handle().clone(),
        ProviderKind::Claude,
        ThreadId("stale-approval".into()),
        RequestId("expired".into()),
        ApprovalDecision::Cancel,
        Some(guard(&workspace, "stale-approval")),
    )
    .await;
    assert!(
        result.is_err(),
        "outside control must report an expired process callback as failed, not successful"
    );
    assert!(!provider.calls.snapshot().iter().any(|c| matches!(
        c,
        mock_agent_provider::MockCall::StartSession(_)
            | mock_agent_provider::MockCall::RespondToRequest(_, _)
    )));
}

#[tokio::test]
async fn guard_revalidates_stored_permission_and_exact_pane_binding() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, _root, workspace, pane) = fixture(provider, "guard-scope").await;
    let mut control = guard(&workspace, "guard-scope");
    control.caller.access = ControlAccess::Supervised;
    control.verify(app.handle()).unwrap();
    app.state::<crate::database::DatabaseStore>()
        .update_agent_chat_session_config(
            "guard-scope",
            &crate::database::AgentChatSessionConfig {
                permission_mode: Some(Some("bypassPermissions".into())),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(control.verify(app.handle()).is_err());
    control.caller.access = ControlAccess::FullAccess;
    app.state::<crate::state::AppStateStore>()
        .close_pane(&pane)
        .unwrap();
    assert!(control.verify(app.handle()).is_err());
}

#[test]
fn runtime_observations_are_sanitized_current_process_and_late_receipts_do_not_resurrect_work() {
    let state = NativeControlState::default();
    assert!(state.thread_runtime("runtime").is_none());
    state.observe_event(&ProviderRuntimeEvent::SessionStateChanged {
        thread_id: ThreadId("runtime".into()),
        status: SessionStatus::Running {
            active_turn: TurnId("turn-a".into()),
        },
    });
    state.observe_event(&ProviderRuntimeEvent::RequestOpened {
        thread_id: ThreadId("runtime".into()),
        turn_id: TurnId("turn-a".into()),
        request_id: RequestId("request-a".into()),
        request_kind: "tool_approval".into(),
        payload: serde_json::json!({"hidden":"synthetic secret tool input"}),
        tool_use_id: None,
        subagent_id: None,
    });
    let view = state
        .thread_runtime("runtime")
        .expect("current process events establish runtime state");
    assert_eq!(view.phase, "waiting_approval");
    assert_eq!(view.pending_approvals.len(), 1);
    assert!(!serde_json::to_string(&view)
        .unwrap()
        .contains("synthetic secret"));
    state.observe_event(&ProviderRuntimeEvent::QueuedTurnDispatched {
        thread_id: ThreadId("runtime".into()),
        queued_id: "queue-a".into(),
        turn_id: TurnId("turn-a".into()),
        text: "hidden queued prompt".into(),
        steered: false,
    });
    state.observe_event(&ProviderRuntimeEvent::TurnCompleted {
        thread_id: ThreadId("runtime".into()),
        turn_id: TurnId("turn-a".into()),
        status: TurnStatus::Success,
        usage: None,
    });
    state.record_accepted_turn(
        "runtime",
        &TurnStartResult {
            turn_id: TurnId("turn-a".into()),
            steered: false,
            queued_id: None,
        },
    );
    state.record_accepted_turn(
        "runtime",
        &TurnStartResult {
            turn_id: TurnId(String::new()),
            steered: false,
            queued_id: Some("queue-a".into()),
        },
    );
    let view = state.thread_runtime("runtime").unwrap();
    assert_eq!(view.phase, "completed");
    assert!(view.pending_approvals.is_empty());
    assert!(view.queued_ids.is_empty());
    assert_eq!(view.last_turn.as_ref().unwrap()["turn_id"], "turn-a");
    state.observe_event(&ProviderRuntimeEvent::SessionStateChanged {
        thread_id: ThreadId("runtime".into()),
        status: SessionStatus::Ready,
    });
    assert_eq!(
        state.thread_runtime("runtime").unwrap().phase,
        "completed",
        "ready after terminal completion is not a new run"
    );
    state.observe_event(&ProviderRuntimeEvent::SessionStateChanged {
        thread_id: ThreadId("runtime".into()),
        status: SessionStatus::Starting,
    });
    assert_eq!(state.thread_runtime("runtime").unwrap().phase, "starting");
    state.observe_event(&ProviderRuntimeEvent::SessionStateChanged {
        thread_id: ThreadId("runtime".into()),
        status: SessionStatus::Closed,
    });
    assert!(state
        .thread_runtime("runtime")
        .unwrap()
        .pending_approvals
        .is_empty());
    assert!(
        NativeControlState::default()
            .thread_runtime("runtime")
            .is_none(),
        "history is not current-process callback state"
    );
}

#[tokio::test]
async fn persisted_supervised_mode_does_not_disguise_a_live_full_access_worker() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, root, workspace, pane) = fixture(provider, "mode-divergence").await;
    let mut input = start("mode-divergence", root.path());
    input.permission_mode = Some("bypassPermissions".into());
    agent_chat::agent_chat_start_session(
        app.handle().clone(),
        pane,
        ProviderKind::Claude,
        input,
        None,
    )
    .await
    .unwrap();
    agent_chat::agent_chat_update_session_config(
        app.handle().clone(),
        app.state(),
        "mode-divergence".into(),
        crate::database::AgentChatSessionConfig {
            permission_mode: Some(Some("default".into())),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut control = guard(&workspace, "mode-divergence");
    control.caller.access = ControlAccess::Supervised;
    assert!(
        control.verify(app.handle()).is_err(),
        "DB-only future permission selection must not grant access to a full-access live worker"
    );
}

fn request(thread: &str, id: &str) -> ProviderRuntimeEvent {
    ProviderRuntimeEvent::RequestOpened {
        thread_id: ThreadId(thread.into()),
        turn_id: TurnId("approval-turn".into()),
        request_id: RequestId(id.into()),
        request_kind: "tool_approval".into(),
        payload: serde_json::json!({"hidden":"synthetic tool input"}),
        tool_use_id: None,
        subagent_id: None,
    }
}

#[tokio::test]
async fn guarded_current_callback_is_terminal_after_success_and_retry_is_strict() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, root, workspace, pane) = fixture(provider.clone(), "current-callback").await;
    agent_chat::agent_chat_start_session(
        app.handle().clone(),
        pane,
        ProviderKind::Claude,
        start("current-callback", root.path()),
        None,
    )
    .await
    .unwrap();
    agent_chat::forward_event(app.handle(), request("current-callback", "current-request"));
    agent_chat::agent_chat_respond_to_request_guarded(
        app.handle().clone(),
        ProviderKind::Claude,
        ThreadId("current-callback".into()),
        RequestId("current-request".into()),
        ApprovalDecision::Cancel,
        Some(guard(&workspace, "current-callback")),
    )
    .await
    .unwrap();
    assert!(
        !app.state::<NativeControlState>()
            .request_is_current("current-callback", "current-request"),
        "a successful provider callback must stop being actionable immediately"
    );
    let retry = agent_chat::agent_chat_respond_to_request_guarded(
        app.handle().clone(),
        ProviderKind::Claude,
        ThreadId("current-callback".into()),
        RequestId("current-request".into()),
        ApprovalDecision::Cancel,
        Some(guard(&workspace, "current-callback")),
    )
    .await;
    assert!(retry.is_err());
    assert_eq!(
        provider
            .calls
            .snapshot()
            .iter()
            .filter(|call| matches!(call, mock_agent_provider::MockCall::RespondToRequest(_, _)))
            .count(),
        1
    );
}

#[tokio::test]
async fn guarded_request_not_pending_is_strict_but_ui_stale_contract_remains_ok() {
    let provider = Arc::new(HeldStartProvider::new());
    provider.release.add_permits(1);
    let (app, root, workspace, pane) = fixture(provider.clone(), "lost-callback").await;
    agent_chat::agent_chat_start_session(
        app.handle().clone(),
        pane,
        ProviderKind::Claude,
        start("lost-callback", root.path()),
        None,
    )
    .await
    .unwrap();
    agent_chat::forward_event(app.handle(), request("lost-callback", "lost-request"));
    *provider.response_error.lock().unwrap() = Some(ProviderError::RequestNotPending {
        request_id: RequestId("lost-request".into()),
    });
    assert!(agent_chat::agent_chat_respond_to_request_guarded(
        app.handle().clone(),
        ProviderKind::Claude,
        ThreadId("lost-callback".into()),
        RequestId("lost-request".into()),
        ApprovalDecision::Cancel,
        Some(guard(&workspace, "lost-callback"))
    )
    .await
    .is_err());
    assert!(!app
        .state::<NativeControlState>()
        .request_is_current("lost-callback", "lost-request"));
    *provider.response_error.lock().unwrap() = Some(ProviderError::RequestNotPending {
        request_id: RequestId("lost-request".into()),
    });
    agent_chat::agent_chat_respond_to_request(
        app.handle().clone(),
        ProviderKind::Claude,
        ThreadId("lost-callback".into()),
        RequestId("lost-request".into()),
        ApprovalDecision::Cancel,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn guarded_supervised_client_cannot_approve_its_own_worker_request() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, root, workspace, pane) = fixture(provider.clone(), "self-approval").await;
    agent_chat::agent_chat_start_session(
        app.handle().clone(),
        pane,
        ProviderKind::Claude,
        start("self-approval", root.path()),
        None,
    )
    .await
    .unwrap();
    agent_chat::forward_event(app.handle(), request("self-approval", "permission"));
    let mut control = guard(&workspace, "self-approval");
    control.caller.access = ControlAccess::Supervised;
    for decision in [
        ApprovalDecision::Allow {
            updated_input: None,
            updated_permissions: None,
        },
        ApprovalDecision::AllowForSession,
        ApprovalDecision::ProviderOption {
            option_id: "opaque-option".into(),
        },
    ] {
        assert!(agent_chat::agent_chat_respond_to_request_guarded(
            app.handle().clone(),
            ProviderKind::Claude,
            ThreadId("self-approval".into()),
            RequestId("permission".into()),
            decision,
            Some(control.clone())
        )
        .await
        .is_err());
    }
    assert!(!provider
        .calls
        .snapshot()
        .iter()
        .any(|call| matches!(call, mock_agent_provider::MockCall::RespondToRequest(_, _))));
}

#[tokio::test]
async fn stop_cancels_a_guarded_start_before_the_lifecycle_mutex_opens() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, root, workspace, pane) = fixture(provider.clone(), "cancel-start").await;
    let gate = app
        .state::<NativeControlState>()
        .thread_gate("cancel-start");
    let held = gate.dispatch.clone().lock_owned().await;
    let launch = agent_chat::agent_chat_start_session_guarded(
        app.handle().clone(),
        pane,
        ProviderKind::Claude,
        start("cancel-start", root.path()),
        None,
        Some(guard(&workspace, "cancel-start")),
    );
    tokio::pin!(launch);
    assert!(futures_util::poll!(launch.as_mut()).is_pending());
    let stop = agent_chat::agent_chat_stop_session_guarded(
        app.handle().clone(),
        ProviderKind::Claude,
        ThreadId("cancel-start".into()),
        Some(guard(&workspace, "cancel-start")),
    );
    tokio::pin!(stop);
    assert!(futures_util::poll!(stop.as_mut()).is_pending());
    drop(held);
    let (launch, stop) = tokio::join!(launch, stop);
    assert!(launch.is_err());
    stop.unwrap();
    assert!(!provider
        .calls
        .snapshot()
        .iter()
        .any(|call| matches!(call, mock_agent_provider::MockCall::StartSession(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guarded_send_rechecks_permission_after_activity_wait() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, root, workspace, pane) = fixture(provider.clone(), "permission-wait").await;
    agent_chat::agent_chat_start_session(
        app.handle().clone(),
        pane,
        ProviderKind::Claude,
        start("permission-wait", root.path()),
        None,
    )
    .await
    .unwrap();
    let (entered, release) = provider.hold_next_send();
    let handle = app.handle().clone();
    let first = tokio::spawn(async move {
        agent_chat::agent_chat_send_turn(handle, ProviderKind::Claude, send("permission-wait"))
            .await
    });
    entered.notified().await;
    let mut control = guard(&workspace, "permission-wait");
    control.caller.access = ControlAccess::Supervised;
    let second = agent_chat::send_turn_with_control(
        app.handle().clone(),
        ProviderKind::Claude,
        send("permission-wait"),
        Some(control),
    );
    tokio::pin!(second);
    assert!(futures_util::poll!(second.as_mut()).is_pending());
    // Direct persisted mutation is a fixture for a post-admission change,
    // not a connector grant or an alternative native permission owner.
    app.state::<crate::database::DatabaseStore>()
        .update_agent_chat_session_config(
            "permission-wait",
            &crate::database::AgentChatSessionConfig {
                permission_mode: Some(Some("bypassPermissions".into())),
                ..Default::default()
            },
        )
        .unwrap();
    release.notify_one();
    first.await.unwrap().unwrap();
    assert!(second.await.is_err());
    assert_eq!(
        provider
            .calls
            .snapshot()
            .iter()
            .filter(|call| matches!(call, mock_agent_provider::MockCall::SendTurn(_, _)))
            .count(),
        1
    );
}

#[test]
fn closed_process_state_cannot_be_reopened_by_a_late_rpc_receipt() {
    let state = NativeControlState::default();
    state.observe_event(&ProviderRuntimeEvent::SessionStateChanged {
        thread_id: ThreadId("closed-receipt".into()),
        status: SessionStatus::Closed,
    });
    state.record_accepted_turn(
        "closed-receipt",
        &TurnStartResult {
            steered: false,
            turn_id: TurnId("late-turn".into()),
            queued_id: None,
        },
    );
    state.record_accepted_turn(
        "closed-receipt",
        &TurnStartResult {
            steered: false,
            turn_id: TurnId(String::new()),
            queued_id: Some("late-queue".into()),
        },
    );
    let view = state.thread_runtime("closed-receipt").unwrap();
    assert_eq!(view.phase, "closed");
    assert!(view.queued_ids.is_empty());
}

#[tokio::test]
async fn guarded_native_apis_start_send_interrupt_and_stop_reuse_one_provider_owner() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, root, workspace, pane) = fixture(provider.clone(), "guarded-apis").await;
    let control = guard(&workspace, "guarded-apis");
    let actual = agent_chat::agent_chat_start_session_guarded(
        app.handle().clone(),
        pane,
        ProviderKind::Claude,
        start("guarded-apis", root.path()),
        None,
        Some(control.clone()),
    )
    .await
    .unwrap();
    assert_eq!(actual.0, "guarded-apis");
    agent_chat::send_turn_with_control(
        app.handle().clone(),
        ProviderKind::Claude,
        send("guarded-apis"),
        Some(control.clone()),
    )
    .await
    .unwrap();
    assert!(agent_chat::agent_chat_interrupt_turn_guarded(
        app.handle().clone(),
        ProviderKind::Claude,
        actual.clone(),
        None,
        Some(control.clone())
    )
    .await
    .unwrap());
    agent_chat::agent_chat_stop_session_guarded(
        app.handle().clone(),
        ProviderKind::Claude,
        actual,
        Some(control),
    )
    .await
    .unwrap();
    let calls = provider.calls.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, mock_agent_provider::MockCall::StartSession(_)))
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, mock_agent_provider::MockCall::SendTurn(_, _)))
            .count(),
        1
    );
    let messages = app
        .state::<crate::database::DatabaseStore>()
        .read_agent_chat_history_page(&workspace, "guarded-apis", None, 20)
        .unwrap();
    assert_eq!(messages.messages.len(), 1);
    assert_eq!(
        app.state::<NativeControlState>()
            .thread_runtime("guarded-apis")
            .unwrap()
            .phase,
        "closed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_permission_changes_wait_for_the_dispatch_owner() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, root, workspace, pane) = fixture(provider.clone(), "mode-lock").await;
    agent_chat::agent_chat_start_session(
        app.handle().clone(),
        pane,
        ProviderKind::Claude,
        start("mode-lock", root.path()),
        None,
    )
    .await
    .unwrap();
    let (entered, release) = provider.hold_next_send();
    let handle = app.handle().clone();
    let first = tokio::spawn(async move {
        agent_chat::agent_chat_send_turn(handle, ProviderKind::Claude, send("mode-lock")).await
    });
    entered.notified().await;
    let permission = agent_chat::agent_chat_set_permission_mode(
        app.handle().clone(),
        ProviderKind::Claude,
        ThreadId("mode-lock".into()),
        "bypassPermissions".into(),
    );
    tokio::pin!(permission);
    assert!(futures_util::poll!(permission.as_mut()).is_pending());
    assert_eq!(
        app.state::<crate::database::DatabaseStore>()
            .get_agent_chat_session("mode-lock")
            .unwrap()
            .permission_mode
            .as_deref(),
        Some("default")
    );
    release.notify_one();
    first.await.unwrap().unwrap();
    permission.await.unwrap();
    let mut control = guard(&workspace, "mode-lock");
    control.caller.access = ControlAccess::Supervised;
    assert!(agent_chat::send_turn_with_control(
        app.handle().clone(),
        ProviderKind::Claude,
        send("mode-lock"),
        Some(control)
    )
    .await
    .is_err());
}

#[test]
fn terminal_runtime_events_cannot_replace_a_fresh_completion_or_reopen_dead_callbacks() {
    let state = NativeControlState::default();
    for turn in ["older-turn", "fresh-turn", "older-turn"] {
        state.observe_event(&ProviderRuntimeEvent::TurnCompleted {
            thread_id: ThreadId("event-order".into()),
            turn_id: TurnId(turn.into()),
            status: TurnStatus::Success,
            usage: None,
        });
    }
    assert_eq!(
        state
            .thread_runtime("event-order")
            .unwrap()
            .last_turn
            .unwrap()["turn_id"],
        "fresh-turn"
    );
    state.observe_event(&ProviderRuntimeEvent::SessionStateChanged {
        thread_id: ThreadId("event-order".into()),
        status: SessionStatus::Closed,
    });
    state.observe_event(&request("event-order", "late-callback"));
    state.observe_event(&ProviderRuntimeEvent::TurnQueued {
        thread_id: ThreadId("event-order".into()),
        queued_id: "dead-queue".into(),
        client_nonce: None,
        text: "not retained".into(),
    });
    let closed = state.thread_runtime("event-order").unwrap();
    assert_eq!(closed.phase, "closed");
    assert!(closed.pending_approvals.is_empty());
    assert!(closed.queued_ids.is_empty());
    state.observe_event(&ProviderRuntimeEvent::SessionStateChanged {
        thread_id: ThreadId("event-order".into()),
        status: SessionStatus::Starting,
    });
    state.observe_event(&request("event-order", "fresh-callback"));
    assert!(state.request_is_current("event-order", "fresh-callback"));
}

#[test]
fn lifecycle_gates_belong_to_the_app_and_pruning_keeps_live_owners() {
    let one = NativeControlState::default();
    let two = NativeControlState::default();
    let first = one.thread_gate("same-thread");
    let second = one.thread_gate("same-thread");
    assert!(Arc::ptr_eq(&first, &second));
    assert!(!Arc::ptr_eq(&first, &two.thread_gate("same-thread")));
    let generation = first.generation();
    first.cancel();
    assert!(second.verify(generation).is_err());
    one.thread_gate("another-thread");
    assert!(Arc::ptr_eq(&first, &one.thread_gate("same-thread")));
}

#[tokio::test]
async fn review_l1_ui_stop_then_provider_switch_accepts_real_missing_old_session() {
    let old = Arc::new(crate::agent_provider::claude::ClaudeAgentProvider::new(
        crate::agent_provider::claude::ClaudeProviderConfig {
            sidecar_binary: Some("/synthetic/never-launch".into()),
            claude_binary: Some("/synthetic/never-launch".into()),
            ..Default::default()
        },
    ).await.unwrap());
    let (app, root, _, pane) = fixture(old.clone(), "switch-after-stop").await;
    // Real adapter contract: the renderer already removed the old session.
    assert!(matches!(old.stop_session(ThreadId("switch-after-stop".into())).await,
        Err(ProviderError::SessionNotFound { .. })));
    agent_chat::agent_chat_stop_session(app.handle().clone(), ProviderKind::Claude,
        ThreadId("switch-after-stop".into())).await.unwrap();
    let next = Arc::new(mock_agent_provider::MockAgentProvider::new(ProviderKind::Codex));
    app.state::<ProviderRegistry>().set_codex(next.clone()).await;
    let mut input = start("switch-after-stop", root.path());
    input.permission_mode = Some("workspace-write".into());
    let result = agent_chat::agent_chat_start_session(app.handle().clone(), pane.clone(),
        ProviderKind::Codex, input, Some("switch-after-stop".into())).await;
    assert!(result.is_ok(), "UI stop-then-start handoff must accept missing old session: {result:?}");
    assert!(next.has_session(&ThreadId("switch-after-stop".into())).await);
    assert_eq!(app.state::<crate::state::AppStateStore>().agent_chat_pane_thread(&pane),
        Some((ProviderKind::Codex, "switch-after-stop".into())));
}

#[tokio::test]
async fn review_s3_guarded_acp_scope_is_exact_and_unsupported_callback_stays_pending() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(ProviderKind::Cursor));
    let (app, root, workspace, pane) = fixture(provider.clone(), "exact-acp").await;
    app.state::<ProviderRegistry>().set_cursor(provider.clone()).await;
    app.state::<crate::state::AppStateStore>().claim_agent_chat_pane_checked(&pane,
        ProviderKind::Cursor, "exact-acp", Some("exact-acp"), |_| Ok(())).unwrap();
    app.state::<crate::database::DatabaseStore>().upsert_agent_chat_session("exact-acp", &workspace,
        Some(root.path().to_str().unwrap()), "cursor").unwrap();
    app.state::<crate::database::DatabaseStore>().update_agent_chat_session_config("exact-acp",
        &crate::database::AgentChatSessionConfig { permission_mode:Some(Some("ask".into())),..Default::default() }).unwrap();
    let mut start = start("exact-acp", root.path()); start.permission_mode = Some("ask".into());
    provider.start_session(start).await.unwrap();
    let mut control = guard(&workspace, "exact-acp"); control.provider = ProviderKind::Cursor;
    for (id, available, decision) in [
        ("only-always", "allow_always", ApprovalDecision::Allow { updated_input:None,updated_permissions:None }),
        ("only-once", "allow_once", ApprovalDecision::AllowForSession),
    ] {
        let mut event = request("exact-acp", id);
        if let ProviderRuntimeEvent::RequestOpened { payload, .. } = &mut event {
            *payload = serde_json::json!({"options":[{"kind":available,"optionId":"permit"}]});
        }
        agent_chat::forward_event(app.handle(), event);
        let result = agent_chat::agent_chat_respond_to_request_guarded(app.handle().clone(), ProviderKind::Cursor,
            ThreadId("exact-acp".into()),RequestId(id.into()),decision,Some(control.clone())).await;
        assert!(result.is_err(), "outside exact scope must not substitute once/always: {result:?}");
        assert!(result.unwrap_err().contains("unsupported_decision"));
        assert!(app.state::<NativeControlState>().request_is_current("exact-acp", id));
    }
    assert!(!provider.calls.snapshot().iter().any(|c| matches!(c, mock_agent_provider::MockCall::RespondToRequest(_, _))));
}

#[tokio::test]
async fn review_s5_native_cursor_override_cannot_mutate_session_mode_behind_witness() {
    let binary = std::env::var_os("CODEMUX_NATIVE_CURSOR_FIXTURE")
        .map(std::path::PathBuf::from).unwrap_or_else(|| {
            std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join(format!("fake_cursor_acp{}", std::env::consts::EXE_SUFFIX))
        });
    assert!(binary.is_file(), "build the fake_cursor_acp test fixture first");
    let provider = Arc::new(crate::agent_provider::cursor::CursorAgentProvider::new(
        crate::agent_provider::cursor::CursorProviderConfig { binary,event_channel_capacity:128 },
    ));
    let (app, root, workspace, pane) = fixture(provider.clone(), "cursor-override").await;
    app.state::<ProviderRegistry>().set_cursor(provider.clone()).await;
    let mut input = start("cursor-override", root.path()); input.permission_mode = Some("ask".into());
    agent_chat::agent_chat_start_session(app.handle().clone(), pane, ProviderKind::Cursor, input, Some("cursor-override".into())).await.unwrap();
    let mut control = guard(&workspace, "cursor-override");
    control.provider = ProviderKind::Cursor; control.caller.access = ControlAccess::Supervised;
    control.verify(app.handle()).unwrap();
    let mut turn = send("cursor-override"); turn.permission_mode_override = Some("agent".into());
    let result = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, turn).await;
    provider.stop_session(ThreadId("cursor-override".into())).await.unwrap();
    assert!(result.is_err(), "ACP overrides are session-wide: require the native permission setter, not a hidden per-turn mutation: {result:?}");
    assert!(result.unwrap_err().starts_with("unsupported_permission_override:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_s2_actual_acp_queue_drain_revalidates_native_worker_ceiling() {
    use futures_util::StreamExt;
    let binary = std::env::var_os("CODEMUX_NATIVE_CURSOR_FIXTURE").map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join(format!("fake_cursor_acp{}", std::env::consts::EXE_SUFFIX)));
    assert!(binary.is_file(), "build fake_cursor_acp first");
    let provider = Arc::new(crate::agent_provider::cursor::CursorAgentProvider::new(
        crate::agent_provider::cursor::CursorProviderConfig { binary,event_channel_capacity:128 },
    ));
    let (app, root, workspace, pane) = fixture(provider.clone(), "guarded-queue").await;
    app.state::<ProviderRegistry>().set_cursor(provider.clone()).await;
    let mut input = start("guarded-queue", root.path()); input.permission_mode = Some("ask".into());
    agent_chat::agent_chat_start_session(app.handle().clone(), pane, ProviderKind::Cursor, input, Some("guarded-queue".into())).await.unwrap();
    let mut events = provider.event_stream();
    let mut first = send("guarded-queue"); first.text = "await-cancel".into();
    agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, first).await.unwrap();
    let mut control = guard(&workspace, "guarded-queue"); control.provider=ProviderKind::Cursor; control.caller.access=ControlAccess::Supervised;
    let queued = agent_chat::send_turn_with_control(app.handle().clone(), ProviderKind::Cursor, send("guarded-queue"), Some(control)).await.unwrap();
    let queued_id = queued.queued_id.unwrap();
    app.state::<crate::database::DatabaseStore>().update_agent_chat_session_config("guarded-queue",
        &crate::database::AgentChatSessionConfig {permission_mode:Some(Some("agent".into())),..Default::default()}).unwrap();
    provider.interrupt_turn(ThreadId("guarded-queue".into()), None).await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(event) = events.next().await {
            match &event {
                ProviderRuntimeEvent::QueuedTurnCancelled {queued_id:id,..} | ProviderRuntimeEvent::QueuedTurnDispatched {queued_id:id,..} if id == &queued_id => return event,
                _ => {}
            }
        }
        panic!("fixture stream closed before queue disposition")
    }).await.unwrap();
    provider.stop_session(ThreadId("guarded-queue".into())).await.unwrap();
    assert!(matches!(result, ProviderRuntimeEvent::QueuedTurnCancelled { .. }),
        "the actual provider drain must cancel undispatched authority that changed after enqueue: {result:?}");
}

fn review_acp_rig(root: &std::path::Path) -> std::path::PathBuf {
    // Optional retained evidence for external reducer replay; ordinary tests
    // own all peer files beneath their disposable TempDir.
    std::env::var_os("CODEMUX_FR1_PEER_DIR")
        .map(|base| std::path::PathBuf::from(base).join(root.file_name().unwrap()))
        .unwrap_or_else(|| root.join("acp-fixture"))
}

async fn review_acp_fixture(thread: &str) -> (
    tauri::App<tauri::test::MockRuntime>, tempfile::TempDir, String,
    Arc<crate::agent_provider::cursor::CursorAgentProvider>,
) {
    let root = tempfile::tempdir().unwrap();
    let rig = review_acp_rig(root.path());
    std::fs::create_dir_all(&rig).unwrap();
    let binary = root.path().join("agent_control_acp_peer.py");
    std::fs::write(&binary, include_str!("../../tests/helpers/agent_control_acp_peer.py")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let provider = Arc::new(crate::agent_provider::cursor::CursorAgentProvider::new(
        crate::agent_provider::cursor::CursorProviderConfig { binary, event_channel_capacity: 256 },
    ));
    let (app, root, workspace, pane) = fixture_with_root(provider.clone(), thread, root).await;
    app.state::<ProviderRegistry>().set_cursor(provider.clone()).await;
    let mut input = start(thread, root.path()); input.permission_mode = Some("ask".into());
    // StartSessionInput exposes the child overlay as `env` (not extra_env).
    // Never mutate the process environment: parallel peers get distinct rigs.
    input.env = Some(std::collections::HashMap::from([(
        "CODEMUX_AGENT_CONTROL_FIXTURE_ROOT".into(), rig.to_string_lossy().into_owned(),
    )]));
    agent_chat::agent_chat_start_session(app.handle().clone(), pane, ProviderKind::Cursor,
        input, Some(thread.into())).await.unwrap();
    (app, root, workspace, provider)
}

// Scheduling-only wrapper: the native owner still creates the real guard,
// and Cursor/ACP still own intake, the queue, prompt workers and transport.
#[derive(Debug)]
struct Fr1DispatchBarrier {
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    prepares: AtomicUsize,
    commits: AtomicUsize,
    aborts: AtomicUsize,
    rejection: std::sync::Mutex<Option<String>>,
}
impl Fr1DispatchBarrier {
    fn new() -> Self {
        Self {
            entered: Default::default(), release: tokio::sync::Semaphore::new(0),
            prepares: AtomicUsize::new(0), commits: AtomicUsize::new(0),
            aborts: AtomicUsize::new(0), rejection: Default::default(),
        }
    }
}
#[derive(Debug)]
struct Fr1HeldCheckpoint {
    inner: Arc<dyn crate::agent_provider::types::TurnDispatchCheckpoint>,
    barrier: Arc<Fr1DispatchBarrier>,
}
#[async_trait::async_trait]
impl crate::agent_provider::types::TurnDispatchCheckpoint for Fr1HeldCheckpoint {
    fn authorize_dispatch(&self) -> Result<(), ProviderError> {
        let result = self.inner.authorize_dispatch();
        if let Err(error) = &result {
            *self.barrier.rejection.lock().unwrap() = Some(error.to_string());
        }
        result
    }
    async fn prepare(&self) {
        self.inner.prepare().await;
        self.barrier.prepares.fetch_add(1, Ordering::SeqCst);
        self.barrier.entered.notify_one();
        self.barrier.release.acquire().await.unwrap().forget();
    }
    async fn commit(&self) {
        self.barrier.commits.fetch_add(1, Ordering::SeqCst);
        self.inner.commit().await;
    }
    async fn abort(&self) {
        self.barrier.aborts.fetch_add(1, Ordering::SeqCst);
        self.inner.abort().await;
    }
}
struct Fr1HeldCursor {
    inner: Arc<crate::agent_provider::cursor::CursorAgentProvider>,
    barrier: Arc<Fr1DispatchBarrier>,
}
#[async_trait::async_trait]
impl AgentProvider for Fr1HeldCursor {
    fn kind(&self) -> ProviderKind { self.inner.kind() }
    fn capabilities(&self) -> ProviderCapabilities { self.inner.capabilities() }
    async fn start_session(&self, input: StartSessionInput) -> Result<ProviderSession, ProviderError> { self.inner.start_session(input).await }
    async fn send_turn(&self, mut input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        if input.text == "fr1-never-dispatch-q1" {
            input.turn_checkpoint = Some(Arc::new(Fr1HeldCheckpoint {
                inner: input.turn_checkpoint.take().expect("real native dispatch guard"),
                barrier: self.barrier.clone(),
            }));
        }
        self.inner.send_turn(input).await
    }
    async fn interrupt_turn(&self, t: ThreadId, turn: Option<TurnId>) -> Result<(), ProviderError> { self.inner.interrupt_turn(t, turn).await }
    async fn respond_to_request(&self, t: ThreadId, r: RequestId, d: ApprovalDecision) -> Result<(), ProviderError> { self.inner.respond_to_request(t, r, d).await }
    async fn set_model(&self, t: ThreadId, m: String) -> Result<(), ProviderError> { self.inner.set_model(t, m).await }
    async fn set_permission_mode(&self, t: ThreadId, m: String) -> Result<(), ProviderError> { self.inner.set_permission_mode(t, m).await }
    async fn stop_session(&self, t: ThreadId) -> Result<(), ProviderError> { self.inner.stop_session(t).await }
    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> { self.inner.list_sessions().await }
    async fn has_session(&self, t: &ThreadId) -> bool { self.inner.has_session(t).await }
    fn event_stream(&self) -> ProviderEventStream { self.inner.event_stream() }
}

type C1Fanout = Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
fn c1_capture_fanout(app: &tauri::AppHandle<tauri::test::MockRuntime>, thread: &str) -> (C1Fanout,C1Fanout) {
    let make = || {
        let captured: C1Fanout = Default::default();
        let target = captured.clone();
        let channel = tauri::ipc::Channel::<agent_chat::AgentChatEventPayload>::new(move |body| {
            target.lock().unwrap().push(body.deserialize::<serde_json::Value>().unwrap());
            Ok(())
        });
        agent_chat::attach_agent_chat_output(app.state::<agent_chat::AgentChatChannelRegistry>(), thread.into(), channel).unwrap();
        captured
    };
    (make(),make())
}
fn c1_assert_fanout(a: &C1Fanout,b: &C1Fanout,ids: &[String]) {
    let a = a.lock().unwrap(); let b = b.lock().unwrap();
    assert_eq!(*a,*b,"real thread channels must receive identical serialized native fan-out");
    for id in ids {
        assert_eq!(a.iter().filter(|p| p["event"]["type"] == "turn_queued" && p["event"]["queued_id"] == *id).count(),1);
        assert_eq!(a.iter().filter(|p| matches!(p["event"]["type"].as_str(),Some("queued_turn_cancelled"|"queued_turn_dispatched")) && p["event"]["queued_id"] == *id).count(),1);
    }
}

#[derive(Debug)]
struct C1CheckpointSchedule {
    stage: &'static str,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    prepares: AtomicUsize,
    commits: AtomicUsize,
    commit_done: AtomicUsize,
    aborts: AtomicUsize,
    abort_done: AtomicUsize,
    prepared: AtomicUsize,
}
impl C1CheckpointSchedule {
    fn new(stage: &'static str) -> Self {
        Self { stage, entered: Default::default(), release: tokio::sync::Semaphore::new(0),
            prepares: AtomicUsize::new(0), commits: AtomicUsize::new(0), commit_done: AtomicUsize::new(0),
            aborts: AtomicUsize::new(0), abort_done: AtomicUsize::new(0), prepared: AtomicUsize::new(0) }
    }
    async fn pause(&self, stage: &str) {
        if self.stage == stage {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
    }
}
#[derive(Debug)]
struct C1ScheduledCheckpoint {
    inner: Arc<dyn crate::agent_provider::types::TurnDispatchCheckpoint>,
    schedule: Arc<C1CheckpointSchedule>,
}
#[async_trait::async_trait]
impl crate::agent_provider::types::TurnDispatchCheckpoint for C1ScheduledCheckpoint {
    fn authorize_dispatch(&self) -> Result<(), ProviderError> { self.inner.authorize_dispatch() }
    async fn prepare(&self) {
        self.inner.prepare().await;
        self.schedule.prepares.fetch_add(1, Ordering::SeqCst);
        self.schedule.prepared.store(1, Ordering::SeqCst);
        self.schedule.pause("prepare").await;
    }
    async fn commit(&self) {
        self.schedule.commits.fetch_add(1, Ordering::SeqCst);
        self.schedule.pause("commit").await;
        self.inner.commit().await;
        self.schedule.prepared.store(0, Ordering::SeqCst);
        self.schedule.commit_done.fetch_add(1, Ordering::SeqCst);
    }
    async fn abort(&self) {
        self.schedule.aborts.fetch_add(1, Ordering::SeqCst);
        self.schedule.pause("abort").await;
        self.inner.abort().await;
        self.schedule.prepared.store(0, Ordering::SeqCst);
        self.schedule.abort_done.fetch_add(1, Ordering::SeqCst);
    }
}
struct C1ScheduledCursor {
    inner: Arc<crate::agent_provider::cursor::CursorAgentProvider>,
    schedule: Arc<C1CheckpointSchedule>,
}
#[async_trait::async_trait]
impl AgentProvider for C1ScheduledCursor {
    fn kind(&self) -> ProviderKind { self.inner.kind() }
    fn capabilities(&self) -> ProviderCapabilities { self.inner.capabilities() }
    async fn start_session(&self, input: StartSessionInput) -> Result<ProviderSession, ProviderError> { self.inner.start_session(input).await }
    async fn send_turn(&self, mut input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        if input.text.starts_with("c1-selected") {
            input.turn_checkpoint = Some(Arc::new(C1ScheduledCheckpoint {
                inner: input.turn_checkpoint.take().expect("actual native owner dispatch checkpoint"),
                schedule: self.schedule.clone(),
            }));
        }
        self.inner.send_turn(input).await
    }
    async fn interrupt_turn(&self, t: ThreadId, turn: Option<TurnId>) -> Result<(), ProviderError> { self.inner.interrupt_turn(t, turn).await }
    async fn respond_to_request(&self, t: ThreadId, r: RequestId, d: ApprovalDecision) -> Result<(), ProviderError> { self.inner.respond_to_request(t, r, d).await }
    async fn set_model(&self, t: ThreadId, m: String) -> Result<(), ProviderError> { self.inner.set_model(t, m).await }
    async fn set_permission_mode(&self, t: ThreadId, m: String) -> Result<(), ProviderError> { self.inner.set_permission_mode(t, m).await }
    async fn stop_session(&self, t: ThreadId) -> Result<(), ProviderError> { self.inner.stop_session(t).await }
    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> { self.inner.list_sessions().await }
    async fn has_session(&self, t: &ThreadId) -> bool { self.inner.has_session(t).await }
    async fn turn_active(&self, t: &ThreadId) -> bool { self.inner.turn_active(t).await }
    async fn cancel_queued_turn(&self, t: ThreadId, id: String) -> Result<bool, ProviderError> { self.inner.cancel_queued_turn(t, id).await }
    async fn send_queued_turn_now(&self, t: ThreadId, id: String) -> Result<(), ProviderError> { self.inner.send_queued_turn_now(t, id).await }
    fn event_stream(&self) -> ProviderEventStream { self.inner.event_stream() }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_direct_first_dequeue_config_rpc() { c1_stop_control("config").await; }
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_before_selected_prepare_first_poll() { c1_stop_control("before-prepare").await; }
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_during_selected_checkpoint_prepare() { c1_stop_control("prepare").await; }
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_during_selected_checkpoint_abort() { c1_stop_control("abort").await; }
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_after_dispatch_while_commit_awaits() { c1_stop_control("commit").await; }
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_after_dispatch_actual_prompt() { c1_stop_control("prompt").await; }
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_promoted_active_queue_config_rpc() { c1_stop_control("promote").await; }
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_explicit_queue_cancel_then_stop_selected_config() { c1_stop_control("queued-cancel").await; }

#[cfg(unix)]
async fn c1_stop_control(mode: &'static str) {
    use futures_util::StreamExt;
    let thread = format!("review-fr1-c1-{mode}");
    let (app, root, workspace, provider) = review_acp_fixture(&thread).await;
    let (fanout_a, fanout_b) = c1_capture_fanout(app.handle(), &thread);
    let rig = review_acp_rig(root.path());
    let schedule = Arc::new(C1CheckpointSchedule::new(mode));
    app.state::<ProviderRegistry>().set_cursor(Arc::new(C1ScheduledCursor { inner: provider.clone(), schedule: schedule.clone() })).await;
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = observed.clone(); let bridge_app = app.handle().clone(); let mut bridge_stream = provider.event_stream();
    let bridge = tokio::spawn(async move {
        while let Some(event) = bridge_stream.next().await {
            agent_chat::forward_event(&bridge_app, event.clone()); captured.lock().unwrap().push(event);
        }
    });
    let mut stream = provider.event_stream();
    let mut first = send(&thread); first.text = "review-hold".into();
    agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, first).await.unwrap();
    let request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop { if let ProviderRuntimeEvent::RequestOpened {request_id, ..} = stream.next().await.unwrap() { break request_id; } }
    }).await.unwrap();
    let mut ids = vec![];
    if mode == "promote" {
        let mut ahead = send(&thread); ahead.text = "c1-never-dispatch-ahead".into();
        ids.push(agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, ahead).await.unwrap().queued_id.unwrap());
    }
    let mut q2 = send(&thread); q2.text = if mode == "prompt" { "c1-selected review-hold" } else { "c1-selected-controlled" }.into();
    q2.model_override = match mode { "abort" => Some("review-fatal".into()), "config"|"promote"|"queued-cancel" => Some("stop-race-held".into()), _ => None };
    let mut control = guard(&workspace, &thread); control.provider = ProviderKind::Cursor;
    let selected = agent_chat::send_turn_with_control(app.handle().clone(), ProviderKind::Cursor, q2, Some(control)).await.unwrap().queued_id.unwrap();
    ids.push(selected.clone());
    let mut trailing = send(&thread); trailing.text = "c1-never-dispatch-trailing".into();
    let q3 = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, trailing).await.unwrap().queued_id.unwrap(); ids.push(q3.clone());
    if mode == "queued-cancel" {
        assert!(provider.cancel_queued_turn(ThreadId(thread.clone()), q3).await.unwrap());
    }
    let before_prepare = (mode == "before-prepare").then(|| crate::agent_provider::acp::session::install_selected_prepare_test_barrier(ThreadId(thread.clone())));
    if mode == "promote" { provider.send_queued_turn_now(ThreadId(thread.clone()), selected.clone()).await.unwrap(); }
    else { provider.respond_to_request(ThreadId(thread.clone()), request.clone(), ApprovalDecision::Allow {updated_input:None,updated_permissions:None}).await.unwrap(); }
    let mut exit = None;
    match mode {
        "before-prepare" => { tokio::time::timeout(std::time::Duration::from_secs(3), before_prepare.as_ref().unwrap().entered.notified()).await.unwrap(); }
        "prepare"|"commit"|"abort" => { tokio::time::timeout(std::time::Duration::from_secs(3), schedule.entered.notified()).await.unwrap(); }
        "prompt" => {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    let event = stream.next().await.unwrap();
                    if let ProviderRuntimeEvent::RequestOpened {request_id, ..} = event {
                        if request_id != request { break; }
                    }
                }
            }).await.unwrap();
        }
        _ => {
            let config: serde_json::Value = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if let Ok(bytes) = std::fs::read(rig.join("config-held.json")) {
                        if let Ok(config) = serde_json::from_slice(&bytes) { break config; }
                    }
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
            assert_eq!(config["params"]["value"], "stop-race-held");
            exit = Some(crate::json_rpc_child::install_exit_drain_test_barrier(root.path()));
        }
    }
    let stop_app = app.handle().clone(); let stop_thread = thread.clone();
    let stop = tokio::spawn(async move { agent_chat::agent_chat_stop_session(stop_app, ProviderKind::Cursor, ThreadId(stop_thread)).await });
    if mode == "abort" {
        // Stop must preserve the independent in-flight abort cleanup, not
        // double-abort or drop its prepared checkpoint with the worker.
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            // Cursor's stop owner may retain its registry write guard while
            // awaiting shutdown. Use its real cancellation event, not a
            // has_session read that would itself wait behind that guard.
            while !observed.lock().unwrap().iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id,..} if queued_id == &selected)) { tokio::task::yield_now().await; }
        }).await.unwrap();
        assert_eq!(schedule.abort_done.load(Ordering::SeqCst), 0);
        assert!(!stop.is_finished(), "Stop returned while owned checkpoint cleanup was outstanding");
        schedule.release.add_permits(1);
    }
    tokio::time::timeout(std::time::Duration::from_secs(3), stop).await.unwrap().unwrap().unwrap();
    if let Some(exit) = exit { tokio::time::timeout(std::time::Duration::from_secs(3), exit.entered.notified()).await.unwrap(); exit.release.add_permits(1); }
    if let Some(before_prepare) = before_prepare { before_prepare.release.add_permits(1); }
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !observed.lock().unwrap().iter().any(|e| matches!(e, ProviderRuntimeEvent::SessionStateChanged {status:SessionStatus::Closed,..})) { tokio::task::yield_now().await; }
    }).await.unwrap();
    let status = super::service::thread_status(app.handle(), serde_json::json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    let events = observed.lock().unwrap().clone();
    let wire = std::fs::read_to_string(rig.join("peer-wire.jsonl")).unwrap();
    let counts = serde_json::json!({"prepares":schedule.prepares.load(Ordering::SeqCst),"commits":schedule.commits.load(Ordering::SeqCst),"commit_done":schedule.commit_done.load(Ordering::SeqCst),"aborts":schedule.aborts.load(Ordering::SeqCst),"abort_done":schedule.abort_done.load(Ordering::SeqCst),"prepared":schedule.prepared.load(Ordering::SeqCst)});
    std::fs::write(rig.join("stop-race-observed.json"), serde_json::to_vec_pretty(&serde_json::json!({"mode":mode,"events":events,"fanout":*fanout_a.lock().unwrap(),"status":status,"accepted_ids":ids,"selected_id":selected,"checkpoint":counts})).unwrap()).unwrap();
    bridge.abort(); let _ = bridge.await;
    c1_assert_fanout(&fanout_a, &fanout_b, &ids);
    for id in &ids {
        assert_eq!(events.iter().filter(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id,..}|ProviderRuntimeEvent::QueuedTurnDispatched {queued_id,..} if queued_id == id)).count(), 1, "exactly one disposition for {id}: {events:?}");
        let dispatched = id == &selected && matches!(mode, "commit"|"prompt");
        assert_eq!(events.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id,..} if queued_id == id)), dispatched);
    }
    assert!(!wire.contains("c1-never-dispatch"));
    assert_eq!(wire.contains("c1-selected"), mode == "prompt");
    assert_eq!(schedule.prepares.load(Ordering::SeqCst), usize::from(mode != "before-prepare"));
    assert_eq!(schedule.commits.load(Ordering::SeqCst), usize::from(matches!(mode,"commit"|"prompt")));
    assert_eq!(schedule.commit_done.load(Ordering::SeqCst), usize::from(mode == "prompt"));
    assert_eq!(schedule.aborts.load(Ordering::SeqCst), usize::from(mode != "prompt"));
    assert_eq!(schedule.abort_done.load(Ordering::SeqCst), usize::from(mode != "prompt"));
    assert_eq!(schedule.prepared.load(Ordering::SeqCst), 0, "Stop lost checkpoint preparation ownership");
    assert_eq!(status["runtime_live"], false); assert_eq!(status["phase"], "closed"); assert_eq!(status["queued_ids"], serde_json::json!([]));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_idle_queue_promotion_during_config_rpc() { c1_idle_promotion_control(false).await; }
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_idle_queue_promotion_before_prepare_poll() { c1_idle_promotion_control(true).await; }

#[cfg(unix)]
async fn c1_idle_promotion_control(before_prepare: bool) {
    use futures_util::StreamExt;
    let thread = format!("review-fr1-c1-idle-{before_prepare}");
    let (app, root, workspace, provider) = review_acp_fixture(&thread).await;
    let (fanout_a, fanout_b) = c1_capture_fanout(app.handle(), &thread);
    let rig = review_acp_rig(root.path());
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = observed.clone(); let bridge_app = app.handle().clone(); let mut stream = provider.event_stream();
    let bridge = tokio::spawn(async move {
        while let Some(event) = stream.next().await { agent_chat::forward_event(&bridge_app, event.clone()); captured.lock().unwrap().push(event); }
    });
    // Reach the adapter's existing idle-queue selection branch without
    // inventing queue state: a real initial configuration RPC fails after
    // the adapter has accepted a follow-up. Only this initial dispatch uses
    // the adapter API directly; follow-ups and Stop use the native owner.
    let initial: SendTurnInput = serde_json::from_value(serde_json::json!({"thread_id":thread,"text":"c1-initial-never-prompt","model_override":"stop-race-reject"})).unwrap();
    let initial_provider = provider.clone();
    let first = tokio::spawn(async move { initial_provider.send_turn(initial).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(bytes) = std::fs::read(rig.join("config-reject-held.json")) {
                if serde_json::from_slice::<serde_json::Value>(&bytes).is_ok() { break; }
            }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let mut input = send(&thread); input.text = "c1-idle-selected".into(); input.model_override = Some("stop-race-held".into());
    let selected = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, input).await.unwrap().queued_id.unwrap();
    let mut trailing = send(&thread); trailing.text = "c1-idle-trailing".into();
    let q3 = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, trailing).await.unwrap().queued_id.unwrap();
    std::fs::write(rig.join("release-config-reject"), b"release actual held error").unwrap();
    assert!(tokio::time::timeout(std::time::Duration::from_secs(3), first).await.unwrap().unwrap().is_err());
    assert!(!provider.turn_active(&ThreadId(thread.clone())).await, "real failed initial dispatch must release its reservation");
    let barrier = before_prepare.then(|| crate::agent_provider::acp::session::install_selected_prepare_test_barrier(ThreadId(thread.clone())));
    let promotion_provider = provider.clone(); let promotion_thread = ThreadId(thread.clone()); let promotion_id = selected.clone();
    let promotion = tokio::spawn(async move { promotion_provider.send_queued_turn_now(promotion_thread, promotion_id).await });
    let mut exit = None;
    if let Some(barrier) = &barrier {
        tokio::time::timeout(std::time::Duration::from_secs(3), barrier.entered.notified()).await.unwrap();
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Ok(bytes) = std::fs::read(rig.join("config-held.json")) {
                    if serde_json::from_slice::<serde_json::Value>(&bytes).is_ok() { break; }
                }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        exit = Some(crate::json_rpc_child::install_exit_drain_test_barrier(root.path()));
    }
    agent_chat::agent_chat_stop_session(app.handle().clone(), ProviderKind::Cursor, ThreadId(thread.clone())).await.unwrap();
    if let Some(exit) = exit { tokio::time::timeout(std::time::Duration::from_secs(3), exit.entered.notified()).await.unwrap(); exit.release.add_permits(1); }
    if let Some(barrier) = barrier { barrier.release.add_permits(1); }
    assert!(tokio::time::timeout(std::time::Duration::from_secs(3), promotion).await.unwrap().unwrap().is_err());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !observed.lock().unwrap().iter().any(|e| matches!(e, ProviderRuntimeEvent::SessionStateChanged {status:SessionStatus::Closed,..})) { tokio::task::yield_now().await; }
    }).await.unwrap();
    let status = super::service::thread_status(app.handle(), serde_json::json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    let events = observed.lock().unwrap().clone();
    let wire = std::fs::read_to_string(rig.join("peer-wire.jsonl")).unwrap();
    std::fs::write(rig.join("stop-race-observed.json"), serde_json::to_vec_pretty(&serde_json::json!({"mode":if before_prepare {"idle-before-prepare"}else{"idle-config"},"events":events,"fanout":*fanout_a.lock().unwrap(),"status":status,"accepted_ids":[selected,q3],"selected_id":selected,"initial_admission":"actual adapter API; followups and Stop native owner"})).unwrap()).unwrap();
    bridge.abort(); let _ = bridge.await;
    c1_assert_fanout(&fanout_a, &fanout_b, &[selected.clone(),q3.clone()]);
    for id in [&selected, &q3] {
        assert_eq!(events.iter().filter(|e| matches!(e,ProviderRuntimeEvent::QueuedTurnCancelled {queued_id,..}|ProviderRuntimeEvent::QueuedTurnDispatched {queued_id,..} if queued_id == id)).count(),1);
        assert!(!events.iter().any(|e| matches!(e,ProviderRuntimeEvent::QueuedTurnDispatched {queued_id,..} if queued_id == id)));
    }
    assert!(!wire.contains("c1-idle-selected") && !wire.contains("c1-idle-trailing") && !wire.contains("c1-initial-never-prompt"));
    assert_eq!(status["runtime_live"],false); assert_eq!(status["phase"],"closed"); assert_eq!(status["queued_ids"],serde_json::json!([]));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn review_fr1_c1_r1_failed_locked_drain_native_stop_preserves_trailing_ids() {
    use futures_util::{StreamExt, poll};
    let thread = "review-fr1-c1-r1-locked-failure-drain";
    let (app, root, workspace, provider) = review_acp_fixture(thread).await;
    let (fanout_a, fanout_b) = c1_capture_fanout(app.handle(), thread);
    let rig = review_acp_rig(root.path());
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = observed.clone();
    let mut bridge_stream = provider.event_stream();
    let bridge_app = app.handle().clone();
    let bridge = tokio::spawn(async move {
        while let Some(event) = bridge_stream.next().await {
            agent_chat::forward_event(&bridge_app, event.clone());
            captured.lock().unwrap().push(event);
        }
    });
    let mut stream = provider.event_stream();
    let mut first = send(thread); first.text = "review-hold".into();
    agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, first).await.unwrap();
    let request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop { if let ProviderRuntimeEvent::RequestOpened {request_id, ..} = stream.next().await.unwrap() { break request_id; } }
    }).await.unwrap();
    let mut selected = send(thread);
    selected.text = "r1-never-prompt-selected-q2".into();
    selected.model_override = Some("stop-race-held".into());
    let q2 = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, selected).await.unwrap().queued_id.unwrap();
    let mut ids = vec![q2.clone()];
    for text in ["r1-never-prompt-trailing-q3", "r1-never-prompt-trailing-q4"] {
        let mut trailing = send(thread); trailing.text = text.into();
        ids.push(agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, trailing).await.unwrap().queued_id.unwrap());
    }
    let admitted = app.state::<NativeControlState>().thread_runtime(thread).unwrap();
    assert!(ids.iter().all(|id| admitted.queued_ids.contains(id)));
    let drain = crate::agent_provider::acp::session::install_failed_drain_locked_test_barrier(ThreadId(thread.into()));
    provider.respond_to_request(ThreadId(thread.into()), request, ApprovalDecision::Allow {updated_input:None,updated_permissions:None}).await.unwrap();
    let config: serde_json::Value = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(bytes) = std::fs::read(rig.join("config-held.json")) {
                if let Ok(config) = serde_json::from_slice(&bytes) { break config; }
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("selected Q2 must issue its actual configuration RPC");
    assert_eq!(config["method"], "session/set_config_option");
    assert_eq!(config["params"]["value"], "stop-race-held");
    // Only the PID written by this owned synthetic ACP child is terminated.
    // Its outstanding configuration request fails through the real transport.
    let pid: u32 = std::fs::read_to_string(rig.join("peer-pid")).unwrap().parse().unwrap();
    assert!(pid > 1);
    assert!(std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status().unwrap().success());
    tokio::time::timeout(std::time::Duration::from_secs(3), drain.entered.notified()).await
        .expect("genuine config failure must reach Failed while holding the drain lock");
    assert_eq!(*drain.drained.lock().unwrap(), ids[1..]);
    assert!(!provider.has_session(&ThreadId(thread.into())).await);
    let stop = agent_chat::agent_chat_stop_session(app.handle().clone(), ProviderKind::Cursor, ThreadId(thread.into()));
    tokio::pin!(stop);
    // Poll the actual native Stop once while the worker owns the state lock.
    // FIFO mutex admission puts it ahead of selected cleanup. A single-thread
    // runtime then runs Stop to abort before repolling the suspended worker.
    assert!(poll!(stop.as_mut()).is_pending());
    drain.release.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(3), stop).await.unwrap().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !observed.lock().unwrap().iter().any(|event| matches!(event, ProviderRuntimeEvent::SessionStateChanged {status:SessionStatus::Closed,..})) {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let status = super::service::thread_status(app.handle(), serde_json::json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    let events = observed.lock().unwrap().clone();
    let wire = std::fs::read_to_string(rig.join("peer-wire.jsonl")).unwrap();
    // Preserve real Channel payloads before the outer TempDir is dropped,
    // including RED missing dispositions; never synthesize reducer events.
    std::fs::write(rig.join("correction-observed.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "mode":"locked-failure-drain", "events":events, "fanout":*fanout_a.lock().unwrap(),
        "fanout_b":*fanout_b.lock().unwrap(), "accepted_ids":ids, "selected_id":q2,
        "drained_ids":*drain.drained.lock().unwrap(), "config":config, "peer_pid":pid, "status":status,
    })).unwrap()).unwrap();
    bridge.abort(); let _ = bridge.await;
    assert!(events.iter().any(|e| matches!(e,ProviderRuntimeEvent::RuntimeWarning {message,..} if message.contains("Could not dispatch queued Cursor turn"))), "failure must be real ACP preparation error");
    assert!(!wire.contains("r1-never-prompt"));
    assert_eq!(status["runtime_live"], false);
    assert_eq!(status["queued_ids"], serde_json::json!([]));
    assert_eq!(status["phase"], "closed");
    assert_eq!(*fanout_a.lock().unwrap(), *fanout_b.lock().unwrap());
    for id in &ids {
        let dispositions = events.iter().filter(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id,..}|ProviderRuntimeEvent::QueuedTurnDispatched {queued_id,..} if queued_id == id)).count();
        assert_eq!(dispositions, 1, "FR1-C1-R1: lost accepted ID {id} after Failed drained trailing IDs and actual Stop aborted the worker");
        assert!(events.iter().any(|e| matches!(e,ProviderRuntimeEvent::QueuedTurnCancelled {queued_id,..} if queued_id == id)));
    }
    c1_assert_fanout(&fanout_a,&fanout_b,&ids);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_c1_stop_during_selected_config_rpc_terminalizes_every_id() {
    use futures_util::StreamExt;
    let thread = "review-fr1-c1-config-after-rejection";
    let (app, root, workspace, provider) = review_acp_fixture(thread).await;
    let (fanout_a, fanout_b) = c1_capture_fanout(app.handle(), thread);
    let rig = review_acp_rig(root.path());
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = observed.clone();
    let mut stream = provider.event_stream();
    let bridge_app = app.handle().clone();
    let bridge = tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            agent_chat::forward_event(&bridge_app, event.clone());
            captured.lock().unwrap().push(event);
        }
    });
    let mut events = provider.event_stream();
    let mut first = send(thread); first.text = "review-hold".into();
    agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, first).await.unwrap();
    let request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop { if let ProviderRuntimeEvent::RequestOpened {request_id, ..} = events.next().await.unwrap() { break request_id; } }
    }).await.unwrap();
    let mut control = guard(&workspace, thread); control.provider = ProviderKind::Cursor;
    control.caller.access = ControlAccess::Supervised;
    let mut outside = send(thread); outside.text = "c1-rejected-q1".into();
    let q1 = agent_chat::send_turn_with_control(app.handle().clone(), ProviderKind::Cursor, outside, Some(control)).await.unwrap().queued_id.unwrap();
    let mut native = send(thread); native.text = "c1-selected-q2".into(); native.model_override = Some("stop-race-held".into());
    let q2 = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, native).await.unwrap().queued_id.unwrap();
    let mut trailing = send(thread); trailing.text = "c1-trailing-q3".into();
    let q3 = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, trailing).await.unwrap().queued_id.unwrap();
    let admitted = app.state::<NativeControlState>().thread_runtime(thread).unwrap();
    assert!(admitted.queued_ids.contains(&q1) && admitted.queued_ids.contains(&q2) && admitted.queued_ids.contains(&q3));
    app.state::<crate::database::DatabaseStore>().update_agent_chat_session_config(thread,
        &crate::database::AgentChatSessionConfig {permission_mode: Some(Some("agent".into())), ..Default::default()}).unwrap();
    provider.respond_to_request(ThreadId(thread.into()), request, ApprovalDecision::Allow {updated_input: None, updated_permissions: None}).await.unwrap();
    let config: serde_json::Value = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(bytes) = std::fs::read(rig.join("config-held.json")) {
                if let Ok(config) = serde_json::from_slice(&bytes) { break config; }
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("Q2 must reach its actual session/set_config_option RPC after healthy Q1 rejection");
    assert_eq!(config["method"], "session/set_config_option");
    assert_eq!(config["params"]["value"], "stop-race-held");
    assert!(provider.has_session(&ThreadId(thread.into())).await);
    // Hold real exit-drain cleanup after alive=false. This preserves the
    // existing scheduling window in which shutdown aborts the worker with
    // its real configuration request still outstanding; no fake cancellation.
    let exit = crate::json_rpc_child::install_exit_drain_test_barrier(root.path());
    agent_chat::agent_chat_stop_session(app.handle().clone(), ProviderKind::Cursor, ThreadId(thread.into())).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), exit.entered.notified()).await.unwrap();
    exit.release.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !observed.lock().unwrap().iter().any(|event| matches!(event, ProviderRuntimeEvent::SessionStateChanged {status:SessionStatus::Closed, ..})) {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let status = super::service::thread_status(app.handle(), serde_json::json!({"workspace_id":workspace, "thread_id":thread})).await.unwrap();
    let captured = observed.lock().unwrap().clone();
    let wire = std::fs::read_to_string(rig.join("peer-wire.jsonl")).unwrap();
    std::fs::write(rig.join("stop-race-observed.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "events":captured, "fanout":*fanout_a.lock().unwrap(), "status":status, "accepted_ids":[q1,q2,q3], "selected_id":q2, "config":config,
    })).unwrap()).unwrap();
    bridge.abort(); let _ = bridge.await;
    assert!(!provider.has_session(&ThreadId(thread.into())).await);
    c1_assert_fanout(&fanout_a, &fanout_b, &[q1.clone(),q2.clone(),q3.clone()]);
    for id in [&q1, &q2, &q3] {
        let dispositions = captured.iter().filter(|event| matches!(event,
            ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} | ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == id)).count();
        assert_eq!(dispositions, 1, "FR1-C1: accepted queued ID {id} lost across actual config RPC + whole-session Stop; events={captured:?}");
        assert!(captured.iter().any(|event| matches!(event, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == id)));
    }
    assert!(!wire.contains("c1-rejected-q1") && !wire.contains("c1-selected-q2") && !wire.contains("c1-trailing-q3"));
    assert_eq!(status["runtime_live"], false);
    assert_eq!(status["queued_ids"], serde_json::json!([]));
    assert_eq!(status["phase"], "closed");
    // Closed intentionally is not a completed-turn wait result.
    assert_eq!(status["settled"], false);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_child_exit_during_guarded_rejection_terminalizes_native_queue() {
    review_fr1_rejection_case("after-terminal").await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_child_exit_before_parent_terminal_terminalizes_native_queue() {
    review_fr1_rejection_case("before-terminal").await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_fr1_selected_interrupt_during_rejection_preserves_native_survivor() {
    review_fr1_rejection_case("interrupt").await;
}

#[cfg(unix)]
async fn review_fr1_rejection_case(mode: &str) {
    use futures_util::StreamExt;
    let thread_name = format!("review-fr1-{mode}");
    let thread = thread_name.as_str();
    let (app, root, workspace, provider) = review_acp_fixture(thread).await;
    let rig = review_acp_rig(root.path());
    let barrier = Arc::new(Fr1DispatchBarrier::new());
    app.state::<ProviderRegistry>().set_cursor(Arc::new(Fr1HeldCursor {
        inner: provider.clone(), barrier: barrier.clone(),
    })).await;
    let mut bridge_stream = provider.event_stream();
    let bridge_app = app.handle().clone();
    let bridge = tokio::spawn(async move {
        while let Some(event) = bridge_stream.next().await {
            // The production bridge's actual persistence/fan-out/observer
            // body, independent of the assertion stream below.
            agent_chat::forward_event(&bridge_app, event);
        }
    });
    let mut stream = provider.event_stream();
    let mut first = send(thread); first.text = format!("fr1-hold {mode}");
    let parent = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, first).await.unwrap();
    let mut observed = vec![];
    let request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let event = stream.next().await.unwrap();
            observed.push(event.clone());
            if let ProviderRuntimeEvent::RequestOpened {request_id, ..} = event { break request_id; }
        }
    }).await.unwrap();
    let mut control = guard(&workspace, thread);
    control.provider = ProviderKind::Cursor; control.caller.access = ControlAccess::Supervised;
    let mut outside = send(thread); outside.text = "fr1-never-dispatch-q1".into();
    let q1 = agent_chat::send_turn_with_control(app.handle().clone(), ProviderKind::Cursor,
        outside, Some(control)).await.unwrap().queued_id.unwrap();
    let mut native = send(thread); native.text = "fr1-native-q2".into();
    let q2 = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor,
        native).await.unwrap().queued_id.unwrap();
    assert!(provider.has_session(&ThreadId(thread.into())).await);
    let admitted = app.state::<NativeControlState>().thread_runtime(thread).unwrap();
    assert!(admitted.queued_ids.contains(&q1) && admitted.queued_ids.contains(&q2));
    app.state::<crate::database::DatabaseStore>().update_agent_chat_session_config(thread,
        &crate::database::AgentChatSessionConfig {permission_mode: Some(Some("agent".into())), ..Default::default()}).unwrap();
    provider.respond_to_request(ThreadId(thread.into()), request, ApprovalDecision::Allow {
        updated_input: None, updated_permissions: None,
    }).await.unwrap();
    // A's actual terminal response (or watchdog error) selects Q1. Hold
    // its real checkpoint prepare before the real authority rejection.
    tokio::time::timeout(std::time::Duration::from_secs(3), barrier.entered.notified()).await.unwrap();
    let pid: u32 = std::fs::read_to_string(rig.join("fr1-peer-pid")).unwrap().parse().unwrap();
    assert!(pid > 1);
    if mode == "after-terminal" {
        // Only the PID recorded by our peer while handling this prompt.
        assert!(std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status().unwrap().success());
    }
    if mode == "interrupt" {
        // The current dispatch claim is Q1. Its interrupt must be retired
        // on rejection rather than inherited by independent native Q2.
        provider.interrupt_turn(ThreadId(thread.into()), None).await.unwrap();
        assert!(provider.has_session(&ThreadId(thread.into())).await);
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while provider.has_session(&ThreadId(thread.into())).await { tokio::task::yield_now().await; }
        }).await.unwrap();
    }
    // has_session=false is the production JsonRpcChild EOF/death predicate;
    // no Stop, resume, new send or manual queue cancellation has occurred.
    barrier.release.add_permits(1);
    let mut q2_turn = None;
    let terminalized = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let event = stream.next().await.unwrap();
            if let ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, turn_id, ..} = &event {
                if queued_id == &q2 { q2_turn = Some(turn_id.clone()); }
            }
            let done = matches!(&event, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == &q2)
                || matches!(&event, ProviderRuntimeEvent::TurnCompleted {turn_id, ..} if Some(turn_id) == q2_turn.as_ref());
            observed.push(event); if done { break; }
        }
    }).await.is_ok();
    let status = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let status = super::service::thread_status(app.handle(), serde_json::json!({
                "workspace_id":workspace, "thread_id":thread,
            })).await.unwrap();
            if status["settled"] == true || !terminalized { break status; }
            tokio::task::yield_now().await;
        }
    }).await.expect("native event bridge must settle status autonomously");
    let wire = std::fs::read_to_string(rig.join("peer-wire.jsonl")).unwrap();
    std::fs::write(rig.join("fr1-observed.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "events":observed, "status":status, "q1":q1, "q2":q2, "pid":pid,
        "prepares":barrier.prepares.load(Ordering::SeqCst),
        "commits":barrier.commits.load(Ordering::SeqCst),
        "aborts":barrier.aborts.load(Ordering::SeqCst),
        "rejection":*barrier.rejection.lock().unwrap(),
    })).unwrap()).unwrap();
    assert_eq!(barrier.prepares.load(Ordering::SeqCst), 1);
    assert_eq!(barrier.commits.load(Ordering::SeqCst), 0);
    assert_eq!(barrier.aborts.load(Ordering::SeqCst), 1);
    assert!(barrier.rejection.lock().unwrap().as_ref().unwrap().contains("permission"));
    assert!(!observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q1)), "invalid Q1 dispatched: {observed:?}");
    assert!(!wire.contains("fr1-never-dispatch-q1"), "invalid Q1 reached the peer: {wire}");
    assert!(terminalized, "FR1: real child death stranded independent native Q2; status={status}, events={observed:?}");
    assert_eq!(observed.iter().filter(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == &q1)).count(), 1);
    if mode == "interrupt" {
        assert_eq!(observed.iter().filter(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q2)).count(), 1);
        assert!(!observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == &q2)));
        assert!(observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted {turn_id, status:TurnStatus::Success, ..} if Some(turn_id) == q2_turn.as_ref())), "Q1 interrupt leaked into Q2: {observed:?}");
        assert!(wire.contains("fr1-native-q2"));
    } else {
        assert_eq!(observed.iter().filter(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == &q2)).count(), 1);
        assert!(!observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q2)));
        assert!(!wire.contains("fr1-native-q2"), "native survivor sent to dead peer: {wire}");
    }
    assert_eq!(observed.iter().filter(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted {turn_id, ..} if turn_id == &parent.turn_id)).count(), 1);
    if mode == "before-terminal" {
        assert!(observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted {turn_id, status:TurnStatus::Error {..}, ..} if turn_id == &parent.turn_id)), "the genuine child exit must fail the outstanding parent RPC: {observed:?}");
        assert_eq!(status["phase"], "error");
    }
    assert_eq!(status["runtime_live"], mode == "interrupt");
    assert_eq!(status["queued_ids"], serde_json::json!([]));
    assert_eq!(status["settled"], true, "native status must settle without later cleanup: {status}");
    if mode == "interrupt" {
        let mut fresh = send(thread); fresh.text = "fr1-fresh-native".into();
        let fresh = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, fresh).await.unwrap();
        assert!(fresh.queued_id.is_none());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let event = stream.next().await.unwrap();
                if let ProviderRuntimeEvent::TurnCompleted {turn_id, status, ..} = event {
                    if turn_id == fresh.turn_id { assert!(matches!(status, TurnStatus::Success)); break; }
                }
            }
        }).await.unwrap();
        assert!(std::fs::read_to_string(rig.join("peer-wire.jsonl")).unwrap().contains("fr1-fresh-native"));
        provider.stop_session(ThreadId(thread.into())).await.unwrap();
    }
    bridge.abort();
    let _ = bridge.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r1_denied_guarded_acp_followup_preserves_native_followup() {
    use futures_util::StreamExt;
    let thread = "review-r1";
    let (app, _root, workspace, provider) = review_acp_fixture(thread).await;
    let mut events = provider.event_stream();
    let mut first = send(thread); first.text = "review-hold".into();
    agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, first).await.unwrap();
    let request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let e = events.next().await.unwrap();
            if let ProviderRuntimeEvent::RequestOpened {request_id, ..} = e { break request_id; }
        }
    }).await.unwrap();
    let mut control = guard(&workspace, thread);
    control.provider = ProviderKind::Cursor; control.caller.access = ControlAccess::Supervised;
    let mut outside = send(thread); outside.text = "never-dispatch-q1".into();
    let q1 = agent_chat::send_turn_with_control(app.handle().clone(), ProviderKind::Cursor,
        outside, Some(control)).await.unwrap().queued_id.unwrap();
    let mut native = send(thread); native.text = "native-q2".into();
    let q2 = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor,
        native).await.unwrap().queued_id.unwrap();
    // Revoke only Q1's dispatch authority, without Stop or provider failure.
    app.state::<crate::database::DatabaseStore>().update_agent_chat_session_config(thread,
        &crate::database::AgentChatSessionConfig {permission_mode: Some(Some("agent".into())), ..Default::default()}).unwrap();
    provider.respond_to_request(ThreadId(thread.into()), request, ApprovalDecision::Allow {
        updated_input: None, updated_permissions: None,
    }).await.unwrap();
    let observed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut observed = vec![]; let mut q2_turn = None;
        loop {
            let e = events.next().await.unwrap();
            if let ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, turn_id, ..} = &e {
                if queued_id == &q2 { q2_turn = Some(turn_id.clone()); }
            }
            let done = matches!(&e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == &q2)
                || matches!(&e, ProviderRuntimeEvent::TurnCompleted {turn_id, ..} if Some(turn_id) == q2_turn.as_ref());
            observed.push(e); if done { break observed; }
        }
    }).await.unwrap();
    let healthy = provider.has_session(&ThreadId(thread.into())).await;
    provider.stop_session(ThreadId(thread.into())).await.unwrap();
    assert!(observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == &q1)), "{observed:?}");
    assert!(!observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q1)), "unauthorized Q1 dispatched: {observed:?}");
    assert!(observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q2)), "a per-input denial must not cancel native Q2: {observed:?}");
    assert!(observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted {item: CompletedItem::AssistantText {text}, ..} if text == "native-q2")), "native Q2 must reach the real transport: {observed:?}");
    assert!(healthy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r1_stop_and_real_config_failure_still_cancel_queued_work() {
    use futures_util::StreamExt;
    for stop in [true, false] {
        let thread = if stop { "review-r1-stop" } else { "review-r1-fatal" };
        let (app, _root, _workspace, provider) = review_acp_fixture(thread).await;
        let mut events = provider.event_stream();
        let mut first = send(thread); first.text="review-hold".into();
        agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, first).await.unwrap();
        let request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop { if let ProviderRuntimeEvent::RequestOpened {request_id, ..} = events.next().await.unwrap() { break request_id; } }
        }).await.unwrap();
        let mut q1_input = send(thread); q1_input.text="q1-control".into();
        if !stop { q1_input.model_override=Some("review-fatal".into()); }
        let q1 = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, q1_input).await.unwrap().queued_id.unwrap();
        let q2 = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, send(thread)).await.unwrap().queued_id.unwrap();
        if stop {
            agent_chat::agent_chat_stop_session(app.handle().clone(), ProviderKind::Cursor, ThreadId(thread.into())).await.unwrap();
        } else {
            provider.respond_to_request(ThreadId(thread.into()), request, ApprovalDecision::Allow {updated_input:None, updated_permissions:None}).await.unwrap();
        }
        let observed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let mut observed=vec![];
            loop {
                let event=events.next().await.unwrap();
                observed.push(event);
                // Configuration failure publishes trailing cancellation first;
                // completion requires both actual IDs, not their wire order.
                let done = [&q1, &q2].iter().all(|id| observed.iter().any(|e|
                    matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == *id)));
                if done { break observed; }
            }
        }).await.unwrap();
        for id in [&q1, &q2] {
            assert_eq!(observed.iter().filter(|e| matches!(e,
                ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == id)).count(), 1);
        }
        assert!(!observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q1 || queued_id == &q2)));
        if stop { assert!(!provider.has_session(&ThreadId(thread.into())).await); }
        else {
            assert!(observed.iter().any(|e| matches!(e, ProviderRuntimeEvent::RuntimeWarning {message, ..} if message.contains("synthetic config transport failure"))), "the real configuration RPC failure must remain fatal to this queue: {observed:?}");
            provider.stop_session(ThreadId(thread.into())).await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r3_strict_owner_acp_options_emit_logical_resolution_and_preserve_unsupported() {
    use futures_util::StreamExt;
    let mut captures = vec![];
    for (index, decision, kind, logical, unsupported_first) in [
        (0, ApprovalDecision::Allow {updated_input:None, updated_permissions:None}, "allow_once", "allow", false),
        (1, ApprovalDecision::AllowForSession, "allow_always", "allow_for_session", false),
        (2, ApprovalDecision::Deny {message:"Not approved".into()}, "reject_once", "deny", false),
        (3, ApprovalDecision::ProviderOption {option_id:"opaque-reject_always".into()}, "reject_always", "deny", false),
        (4, ApprovalDecision::AllowForSession, "allow_always", "allow_for_session", true),
    ] {
        let thread = format!("review-r3-{index}");
        let (app, _root, workspace, provider) = review_acp_fixture(&thread).await;
        let mut stream = provider.event_stream();
        let mut input = send(&thread); input.text = if unsupported_first { "review-approval only-always" } else { "review-approval" }.into();
        agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, input).await.unwrap();
        let mut events = vec![];
        let request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let event = stream.next().await.unwrap();
                agent_chat::forward_event(app.handle(), event.clone());
                events.push(event.clone());
                if let ProviderRuntimeEvent::RequestOpened {request_id, ..} = event { break request_id; }
            }
        }).await.unwrap();
        let mut control = guard(&workspace, &thread); control.provider=ProviderKind::Cursor;
        // Both owner-level unsupported scope and adapter-level unknown option
        // must leave the real callback available for the subsequent exact reply.
        if unsupported_first {
            let result = agent_chat::agent_chat_respond_to_request_guarded(app.handle().clone(), ProviderKind::Cursor,
                ThreadId(thread.clone()), request.clone(), ApprovalDecision::Allow {updated_input:None, updated_permissions:None}, Some(control.clone())).await;
            assert!(result.unwrap_err().contains("unsupported_decision"));
            assert!(app.state::<NativeControlState>().request_is_current(&thread, &request.0));
        }
        assert!(provider.respond_to_request(ThreadId(thread.clone()), request.clone(),
            ApprovalDecision::ProviderOption {option_id:"not-advertised".into()}).await.is_err());
        agent_chat::agent_chat_respond_to_request_guarded(app.handle().clone(), ProviderKind::Cursor,
            ThreadId(thread.clone()), request.clone(), decision, Some(control)).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let mut completed = false;
            let mut resolved = false;
            loop {
                let event = stream.next().await.unwrap();
                // The peer may complete its prompt before the response writer
                // resumes to emit RequestResolved. Observe both real events;
                // neither order changes the strict approval assertions below.
                completed |= matches!(&event, ProviderRuntimeEvent::TurnCompleted {..});
                resolved |= matches!(&event, ProviderRuntimeEvent::RequestResolved {request_id, ..} if request_id == &request);
                events.push(event);
                if completed && resolved { break; }
            }
        }).await.unwrap();
        provider.stop_session(ThreadId(thread)).await.unwrap();
        assert!(events.iter().any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted {item:CompletedItem::AssistantText {text}, ..} if text == &format!("wire-option:opaque-{kind}"))), "exact advertised wire option was not selected: {events:?}");
        let resolution = events.iter().find_map(|e| match e {
            ProviderRuntimeEvent::RequestResolved {request_id, decision, ..} if request_id == &request => Some(serde_json::to_value(decision).unwrap()), _ => None,
        }).expect("actual ACP adapter resolution event");
        captures.push(serde_json::json!({"kind":kind,"expected_logical":logical,"resolution":resolution,"events":events}));
    }
    if let Some(path) = std::env::var_os("CODEMUX_REVIEW_EVENTS") {
        std::fs::write(path, serde_json::to_vec_pretty(&captures).unwrap()).unwrap();
    }
    for capture in &captures {
        assert_eq!(capture["resolution"]["decision"], capture["expected_logical"],
            "strict wire ProviderOption must not leak into ACP native UI events: {capture}");
    }
}

#[tokio::test]
async fn review_l4_rejected_selected_interrupt_does_not_cancel_newer_native_admission() {
    let provider = Arc::new(HeldStartProvider::new()); provider.release.add_permits(1);
    let (app, root, workspace, pane) = fixture(provider.clone(), "selected-interrupt").await;
    agent_chat::agent_chat_start_session(app.handle().clone(),pane,ProviderKind::Claude,
        start("selected-interrupt",root.path()),None).await.unwrap();
    let admitted = guard(&workspace,"selected-interrupt").retain_admission(app.handle());
    *provider.interrupt_error.lock().unwrap() = Some(ProviderError::ValidationError {message:"stale_turn".into()});
    assert!(agent_chat::agent_chat_interrupt_turn(app.handle().clone(),ProviderKind::Claude,
        ThreadId("selected-interrupt".into()),Some(TurnId("old-a".into()))).await.is_err());
    assert!(admitted.verify(app.handle()).is_ok(), "a rejected turn-specific interrupt must not cancel an unrelated admitted send");
}

fn send(thread: &str) -> agent_chat::SendTurnCommandInput {
    serde_json::from_value(
        serde_json::json!({"thread_id":thread,"text":"Synthetic task","model_override":null}),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_cancels_native_send_already_waiting_for_activity() {
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(
        ProviderKind::Claude,
    ));
    let (app, root, _, pane) = fixture(provider.clone(), "cancel-waiter").await;
    agent_chat::agent_chat_start_session(
        app.handle().clone(),
        pane,
        ProviderKind::Claude,
        start("cancel-waiter", root.path()),
        None,
    )
    .await
    .unwrap();
    let (entered, release) = provider.hold_next_send();
    let handle = app.handle().clone();
    let first = tokio::spawn(async move {
        agent_chat::agent_chat_send_turn(handle, ProviderKind::Claude, send("cancel-waiter")).await
    });
    entered.notified().await;
    let second = agent_chat::agent_chat_send_turn(
        app.handle().clone(),
        ProviderKind::Claude,
        send("cancel-waiter"),
    );
    tokio::pin!(second);
    assert!(futures_util::poll!(second.as_mut()).is_pending());
    let stop = agent_chat::agent_chat_stop_session(
        app.handle().clone(),
        ProviderKind::Claude,
        ThreadId("cancel-waiter".into()),
    );
    tokio::pin!(stop);
    assert!(futures_util::poll!(stop.as_mut()).is_pending());
    release.notify_one();
    first.await.unwrap().unwrap();
    let (second, stop) = tokio::join!(second, stop);
    stop.unwrap();
    assert!(second.is_err(),"a stop must cancel an undispatched send, not dispatch it before the stop obtains the activity lock");
    assert_eq!(
        provider
            .calls
            .snapshot()
            .iter()
            .filter(|c| matches!(c, mock_agent_provider::MockCall::SendTurn(_, _)))
            .count(),
        1
    );
}
