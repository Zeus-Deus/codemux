use super::*;
use serde_json::json;
use std::sync::Arc;
use tauri::Manager;

#[allow(dead_code)]
#[path = "../../tests/helpers/mock_agent_provider.rs"]
mod mock_agent_provider;

async fn fixture() -> (tauri::App<tauri::test::MockRuntime>, Arc<mock_agent_provider::MockAgentProvider>, tempfile::TempDir, String) {
    let app = tauri::test::mock_app();
    let state = crate::state::AppStateStore::default();
    let root = tempfile::tempdir().unwrap();
    let workspace = state.create_empty_workspace_at_path(root.path().to_path_buf()).0;
    app.manage(state);
    app.manage(crate::database::DatabaseStore::new_in_memory());
    let observability = crate::observability::ObservabilityStore::default();
    let mut flags = observability.feature_flags();
    flags.enable_agent_chat = true;
    observability.set_feature_flags(flags);
    app.manage(observability);
    app.manage(crate::commands::agent_chat::ProviderRegistry::new());
    app.manage(crate::commands::agent_chat::AgentChatChannelRegistry::default());
    app.manage(crate::commands::agent_chat::SubagentTracker::default());
    app.manage(crate::commands::agent_chat::RunActivityTracker::default());
    app.manage(crate::mcp::registry::McpRegistry::default());
    app.state::<crate::mcp::registry::McpRegistry>().insert_running_server_for_test(
        "codemux-self", vec![crate::mcp::McpConfigSource::Codemux],
    ).await;
    app.manage(NativeControlState::default());
    let provider = Arc::new(mock_agent_provider::MockAgentProvider::new(crate::agent_provider::ProviderKind::Claude));
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_claude(provider.clone()).await;
    app.state::<NativeControlState>().fixture_capabilities.lock().unwrap().insert(
        crate::agent_provider::ProviderKind::Claude,
        crate::agent_provider::claude::capabilities::claude_fallback_capabilities(),
    );
    (app, provider, root, workspace)
}

// Capture the actual native Channel serializer, never a hand-built DTO.
fn mario_channel(app: &tauri::AppHandle<tauri::test::MockRuntime>, thread: &str) -> Arc<std::sync::Mutex<Vec<serde_json::Value>>> {
    let events: Arc<std::sync::Mutex<Vec<serde_json::Value>>> = Default::default();
    let output = events.clone();
    let channel = tauri::ipc::Channel::<crate::commands::agent_chat::AgentChatEventPayload>::new(move |body| {
        output.lock().unwrap().push(body.deserialize::<serde_json::Value>().unwrap());
        Ok(())
    });
    crate::commands::agent_chat::attach_agent_chat_output(app.state(), thread.into(), channel).unwrap();
    events
}

#[tokio::test]
async fn mario_r7_interrupted_native_event_is_consistent_live_restart_and_selected_wait() {
    use crate::agent_provider::{ProviderRuntimeEvent, ThreadId, TurnId, TurnStatus};
    let (app, provider, _root, workspace) = fixture().await;
    let thread = "mario-interrupted";
    app.state::<crate::database::DatabaseStore>().upsert_agent_chat_session(thread, &workspace, None, "claude").unwrap();
    let events = mario_channel(app.handle(), thread);
    let event = ProviderRuntimeEvent::TurnCompleted {
        thread_id: ThreadId(thread.into()), turn_id: TurnId("interrupted-parent".into()),
        status: TurnStatus::Error { subtype: "interrupted".into(), message: "synthetic private diagnostic".into() }, usage: None,
    };
    crate::commands::agent_chat::forward_event(app.handle(), event.clone());
    let raw = serde_json::to_value(&event).unwrap();
    assert_eq!(raw["status"]["kind"], "error");
    assert_eq!(raw["status"]["subtype"], "interrupted");
    assert_eq!(events.lock().unwrap()[0]["event"], raw);
    assert_eq!(app.state::<crate::database::DatabaseStore>().control_turn_outcome(thread, "interrupted-parent").unwrap().unwrap(), raw);
    let target = json!({"workspace_id": workspace, "thread_id":thread});
    let live = execute(app.handle(), &ControlCaller::trusted(), "thread_status", target.clone()).await.unwrap();
    assert_eq!(live["phase"], "interrupted");
    assert_eq!(live["last_turn"]["status"]["kind"], "interrupted");
    assert!(!live.to_string().contains("private diagnostic"));
    let live_wait = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", json!({
        "workspace_id":workspace,"thread_id":thread,"turn_id":"interrupted-parent","timeout_ms":1,
    })).await.unwrap();
    assert_eq!(live_wait["selected_turn"]["outcome"], "interrupted");
    app.unmanage::<NativeControlState>().unwrap();
    app.manage(NativeControlState::default());
    let restarted = execute(app.handle(), &ControlCaller::trusted(), "thread_status", target.clone()).await.unwrap();
    assert_eq!(restarted["phase"], "interrupted", "persisted native error subtype must not become a true error: {restarted}");
    assert_eq!(restarted["last_turn"], live["last_turn"]);
    let waited = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", json!({
        "workspace_id":workspace,"thread_id":thread,"turn_id":"interrupted-parent","timeout_ms":1,
    })).await.unwrap();
    assert_eq!(waited["selected_turn"]["outcome"], "interrupted");
    assert_eq!(waited["settled"], true);
    mario_save_receipt("r7-interrupted", &json!({"raw":raw,"fanout":*events.lock().unwrap(),"live":live,"live_wait":live_wait,"restarted":restarted,"waited":waited}));
    assert!(provider.calls.snapshot().is_empty());
}

#[tokio::test]
async fn mario_r7_true_errors_and_limits_remain_distinct_across_native_history() {
    use crate::agent_provider::{ProviderRuntimeEvent, ThreadId, TurnId, TurnStatus};
    for (native, outcome, phase) in [
        (TurnStatus::Success, "success", "completed"),
        (TurnStatus::Error {subtype:"provider_failure".into(),message:"synthetic private diagnostic".into()}, "error", "error"),
        (TurnStatus::MaxTurns, "max_turns", "error"),
        (TurnStatus::MaxBudget, "max_budget", "error"),
    ] {
        let (app, provider, root, workspace) = fixture().await;
        let thread = "mario-native-outcome";
        app.state::<crate::database::DatabaseStore>().upsert_agent_chat_session(thread,&workspace,None,"claude").unwrap();
        let channel = mario_channel(app.handle(),thread);
        crate::commands::agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::TurnCompleted {
            thread_id:ThreadId(thread.into()),turn_id:TurnId("parent".into()),status:native,usage:None,
        });
        let target = json!({"workspace_id":workspace,"thread_id":thread,"turn_id":"parent","timeout_ms":1});
        let live = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", target.clone()).await.unwrap();
        assert_eq!(live["selected_turn"]["outcome"], outcome);
        let db = app.state::<crate::database::DatabaseStore>().control_fixture_snapshot(&root.path().join("durable.sqlite"));
        app.unmanage::<crate::database::DatabaseStore>().unwrap();app.manage(db);
        app.unmanage::<NativeControlState>().unwrap();app.manage(NativeControlState::default());
        let recovered = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", target).await.unwrap();
        assert_eq!(recovered["selected_turn"]["outcome"], outcome);
        assert_eq!(recovered["status"]["phase"], phase);
        assert_eq!(recovered["status"]["last_turn"], live["status"]["last_turn"]);
        assert_eq!(recovered["settled"], true);
        assert!(!recovered.to_string().contains("private diagnostic"));
        mario_save_receipt(&format!("r7-{outcome}"), &json!({"fanout":*channel.lock().unwrap(),"live":live,"recovered":recovered}));
        assert!(provider.calls.snapshot().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mario_r4_child_unknown_and_empty_approvals_keep_selected_parent_wait_busy() {
    use crate::agent_provider::{ProviderRuntimeEvent, RequestId, ThreadId, TurnId, TurnStatus, SessionStatus, SubagentSnapshot, SubagentStatus};
    use crate::commands::agent_chat::{forward_event, SubagentTracker};
    for callback_turn in ["child-b", "", "unknown-turn"] {
        let (app, provider, _root, workspace) = fixture().await;
        let launch = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
            "workspace_id":workspace,"client_request_id":"mario-parent-launch","provider":"claude","permission_mode":"default",
        })).await.unwrap();
        let launched = settle_operation(&app, &launch).await;
        let thread = launched["thread_id"].as_str().unwrap();
        let channel = mario_channel(app.handle(), thread);
        forward_event(app.handle(), ProviderRuntimeEvent::SessionStateChanged {
            thread_id:ThreadId(thread.into()), status:SessionStatus::Running { active_turn:TurnId("parent-a".into()) },
        });
        forward_event(app.handle(), ProviderRuntimeEvent::SubagentUpdated {
            thread_id:ThreadId(thread.into()), subagent:SubagentSnapshot {
                subagent_id:"delegate-b".into(),status:SubagentStatus::Running,..Default::default()
            },
        });
        forward_event(app.handle(), ProviderRuntimeEvent::TurnCompleted {
            thread_id:ThreadId(thread.into()),turn_id:TurnId("parent-a".into()),status:TurnStatus::Success,usage:None,
        });
        let request = ProviderRuntimeEvent::RequestOpened {
            thread_id:ThreadId(thread.into()),turn_id:TurnId(callback_turn.into()),request_id:RequestId("child-request".into()),
            request_kind:"tool_approval".into(),payload:json!({"private_tool_args":"synthetic confidential"}),tool_use_id:None,subagent_id:None,
        };
        forward_event(app.handle(), request.clone());
        assert!(channel.lock().unwrap().iter().any(|p|p["event"]==serde_json::to_value(&request).unwrap()));
        assert!(app.state::<SubagentTracker>().delegated_work_holding_turn(thread));
        let target = json!({"workspace_id":workspace,"thread_id":thread,"turn_id":"parent-a","timeout_ms":1});
        let blocked = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", target.clone()).await.unwrap();
        mario_save_receipt(&format!("r4-{}",if callback_turn.is_empty(){"empty"}else{callback_turn}), &json!({"request":request,"fanout":*channel.lock().unwrap(),"blocked":blocked}));
        assert_eq!(blocked["settled"], false, "child/unknown callbacks cannot stand in for a newer parent: {blocked}");
        assert_eq!(blocked["timed_out"], true);
        assert_eq!(blocked["status"]["turn_id"], "parent-a");
        assert_eq!(blocked["status"]["pending_approvals"][0]["turn_id"], callback_turn);
        assert!(!blocked["status"].to_string().contains("private_tool_args"));
        let handle = blocked["status"]["pending_approvals"][0]["request_id"].clone();
        let replied = execute(app.handle(), &ControlCaller::trusted(), "thread_respond", json!({
            "workspace_id":workspace,"thread_id":thread,"client_request_id":"mario-child-reply","request_id":handle,"decision":"deny",
        })).await.unwrap();
        assert_eq!(settle_operation(&app, &replied).await["state"], "succeeded");
        assert!(provider.calls.snapshot().iter().any(|call|matches!(call,mock_agent_provider::MockCall::RespondToRequest(t,r) if t.0==thread && r.0=="child-request")));
        let resolved = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", target.clone()).await.unwrap();
        assert_eq!(resolved["status"]["turn_id"], "parent-a");
        assert_eq!(resolved["status"]["phase"], "completed", "resolving child must restore parent's outcome");
        assert_eq!(resolved["settled"], false, "delegate is still holding the native parent run");
        forward_event(app.handle(), ProviderRuntimeEvent::SubagentUpdated {
            thread_id:ThreadId(thread.into()),subagent:SubagentSnapshot {subagent_id:"delegate-b".into(),status:SubagentStatus::Completed,..Default::default()},
        });
        assert_eq!(execute(app.handle(), &ControlCaller::trusted(), "thread_wait", target.clone()).await.unwrap()["settled"], true);
        forward_event(app.handle(), ProviderRuntimeEvent::SessionStateChanged {
            thread_id:ThreadId(thread.into()),status:SessionStatus::Running {active_turn:TurnId("new-parent-c".into())},
        });
        forward_event(app.handle(), ProviderRuntimeEvent::RequestOpened {
            thread_id:ThreadId(thread.into()),turn_id:TurnId("new-parent-c".into()),request_id:RequestId("parent-c-request".into()),
            request_kind:"tool_approval".into(),payload:json!({}),tool_use_id:None,subagent_id:None,
        });
        assert_eq!(execute(app.handle(), &ControlCaller::trusted(), "thread_wait", target).await.unwrap()["settled"], true,
            "an explicitly newer parent C must not block the completed selected A");
    }
}

#[test]
fn mario_r4_early_single_owner_approval_correlates_without_child_identity_adoption() {
    use crate::agent_provider::{ProviderRuntimeEvent, RequestId, ThreadId, TurnId, TurnStartResult};
    let state = NativeControlState::default();
    state.observe_event(&ProviderRuntimeEvent::RequestOpened {
        thread_id:ThreadId("early".into()),turn_id:TurnId("early-parent".into()),request_id:RequestId("early-request".into()),
        request_kind:"tool_approval".into(),payload:json!({}),tool_use_id:None,subagent_id:None,
    });
    state.record_accepted_turn("early", &TurnStartResult {turn_id:TurnId("early-parent".into()),queued_id:None,steered:false});
    let view = state.thread_runtime("early").unwrap();
    assert_eq!(view.turn_id.as_deref(), Some("early-parent"));
    assert_eq!(view.phase, "waiting_approval");
    assert!(state.request_is_current("early", "early-request"));
    state.observe_event(&ProviderRuntimeEvent::RequestOpened {
        thread_id:ThreadId("orphan-child".into()),turn_id:TurnId("child".into()),request_id:RequestId("child-request".into()),
        request_kind:"tool_approval".into(),payload:json!({}),tool_use_id:None,subagent_id:Some("child".into()),
    });
    assert!(state.thread_runtime("orphan-child").unwrap().turn_id.is_none());
    // The fresh native send ACK, not that child callback, establishes the
    // parent while retaining the already-actionable callback and its phase.
    state.record_accepted_turn("orphan-child", &TurnStartResult {turn_id:TurnId("fresh-parent".into()),queued_id:None,steered:false});
    let view = state.thread_runtime("orphan-child").unwrap();
    assert_eq!(view.turn_id.as_deref(), Some("fresh-parent"));
    assert_eq!(view.phase, "waiting_approval");
    assert!(state.request_is_current("orphan-child", "child-request"));
}

fn mario_save_receipt(label: &str, value: &serde_json::Value) {
    if let Some(path) = std::env::var_os("CODEMUX_MARIO_RECEIPT") {
        let path = std::path::PathBuf::from(path);
        std::fs::write(path.with_file_name(format!("{label}.json")), serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
}

// Scheduling-only test adapter. Native Cursor/ACP still owns all queue,
// callback, guard, prompt and child lifecycle behavior.
#[cfg(unix)]
#[derive(Debug)]
struct MarioQueueGate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}
#[cfg(unix)]
#[derive(Debug)]
struct MarioQueueCheckpoint {
    inner: Arc<dyn crate::agent_provider::types::TurnDispatchCheckpoint>,
    gate: Arc<MarioQueueGate>,
}
#[cfg(unix)]
#[async_trait::async_trait]
impl crate::agent_provider::types::TurnDispatchCheckpoint for MarioQueueCheckpoint {
    fn authorize_dispatch(&self) -> Result<(), crate::agent_provider::ProviderError> { self.inner.authorize_dispatch() }
    async fn prepare(&self) {
        self.inner.prepare().await;
        self.gate.entered.notify_one();
        self.gate.release.acquire().await.unwrap().forget();
    }
    async fn commit(&self) { self.inner.commit().await; }
    async fn abort(&self) { self.inner.abort().await; }
}
#[cfg(unix)]
struct MarioQueuedCursor {
    inner: Arc<crate::agent_provider::cursor::CursorAgentProvider>,
    gate: Arc<MarioQueueGate>,
    starts: std::sync::atomic::AtomicUsize,
}
#[cfg(unix)]
#[async_trait::async_trait]
impl crate::agent_provider::AgentProvider for MarioQueuedCursor {
    fn kind(&self) -> crate::agent_provider::ProviderKind { self.inner.kind() }
    fn capabilities(&self) -> crate::agent_provider::ProviderCapabilities { self.inner.capabilities() }
    async fn start_session(&self, input: crate::agent_provider::StartSessionInput) -> Result<crate::agent_provider::ProviderSession, crate::agent_provider::ProviderError> {
        self.starts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.start_session(input).await
    }
    async fn send_turn(&self, mut input: crate::agent_provider::SendTurnInput) -> Result<crate::agent_provider::TurnStartResult, crate::agent_provider::ProviderError> {
        if input.text == "mario-held-b" {
            input.turn_checkpoint = Some(Arc::new(MarioQueueCheckpoint {
                inner: input.turn_checkpoint.take().expect("actual native dispatch guard"),gate:self.gate.clone(),
            }));
        }
        self.inner.send_turn(input).await
    }
    async fn interrupt_turn(&self, t: crate::agent_provider::ThreadId, id: Option<crate::agent_provider::TurnId>) -> Result<(), crate::agent_provider::ProviderError> { self.inner.interrupt_turn(t,id).await }
    async fn cancel_queued_turn(&self, t: crate::agent_provider::ThreadId, id: String) -> Result<bool, crate::agent_provider::ProviderError> { self.inner.cancel_queued_turn(t,id).await }
    async fn send_queued_turn_now(&self, t: crate::agent_provider::ThreadId, id: String) -> Result<(), crate::agent_provider::ProviderError> { self.inner.send_queued_turn_now(t,id).await }
    async fn respond_to_request(&self, t: crate::agent_provider::ThreadId, r: crate::agent_provider::RequestId, d: crate::agent_provider::ApprovalDecision) -> Result<(), crate::agent_provider::ProviderError> { self.inner.respond_to_request(t,r,d).await }
    async fn set_model(&self, t: crate::agent_provider::ThreadId, m: String) -> Result<(), crate::agent_provider::ProviderError> { self.inner.set_model(t,m).await }
    async fn set_permission_mode(&self, t: crate::agent_provider::ThreadId, m: String) -> Result<(), crate::agent_provider::ProviderError> { self.inner.set_permission_mode(t,m).await }
    async fn stop_session(&self, t: crate::agent_provider::ThreadId) -> Result<(), crate::agent_provider::ProviderError> { self.inner.stop_session(t).await }
    async fn list_sessions(&self) -> Result<Vec<crate::agent_provider::ProviderSession>, crate::agent_provider::ProviderError> { self.inner.list_sessions().await }
    async fn has_session(&self, t: &crate::agent_provider::ThreadId) -> bool { self.inner.has_session(t).await }
    async fn turn_active(&self, t: &crate::agent_provider::ThreadId) -> bool { self.inner.turn_active(t).await }
    fn event_stream(&self) -> crate::agent_provider::ProviderEventStream { self.inner.event_stream() }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mario_r3_real_acp_queue_crash_after_parent_completion_is_restart_uncertain() {
    mario_r3_queue_boundary("crash").await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mario_r3_actual_queue_cancellation_is_not_restart_uncertainty() {
    mario_r3_queue_boundary("cancelled").await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mario_r3_actual_queue_dispatch_and_completion_are_not_restart_uncertainty() {
    mario_r3_queue_boundary("dispatched").await;
}

#[cfg(unix)]
async fn mario_r3_queue_boundary(mode: &str) {
    use crate::agent_provider::{AgentProvider, ProviderKind, StartSessionInput, ThreadId};
    use crate::commands::agent_chat::{self, ProviderRegistry};
    use futures_util::StreamExt;
    use std::os::unix::fs::PermissionsExt;
    let peer_root = tempfile::tempdir().unwrap();
    let binary = peer_root.path().join("peer.py");
    std::fs::write(&binary, include_str!("../../tests/helpers/agent_control_acp_peer.py")).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let rig = peer_root.path().join("rig");std::fs::create_dir(&rig).unwrap();
    let inner = Arc::new(crate::agent_provider::cursor::CursorAgentProvider::new(crate::agent_provider::cursor::CursorProviderConfig {binary,event_channel_capacity:256}));
    let gate = Arc::new(MarioQueueGate {entered:Default::default(),release:tokio::sync::Semaphore::new(0)});
    let provider = Arc::new(MarioQueuedCursor {inner,gate:gate.clone(),starts:Default::default()});
    let thread_name = format!("mario-queue-{mode}");
    let thread = thread_name.as_str();
    let (app, root, workspace, pane) = super::guard_tests::fixture(provider.clone(), thread).await;
    app.state::<ProviderRegistry>().set_cursor(provider.clone()).await;
    let channel = mario_channel(app.handle(), thread);
    let mut stream = provider.event_stream();
    let bridge_app = app.handle().clone();
    let bridge = tokio::spawn(async move {
        while let Some(event) = stream.next().await { agent_chat::forward_event(&bridge_app,event); }
    });
    agent_chat::agent_chat_start_session(app.handle().clone(), pane, ProviderKind::Cursor, StartSessionInput {
        thread_id:ThreadId(thread.into()),cwd:root.path().canonicalize().unwrap(),model:None,resume_cursor:None,fresh_session:true,
        permission_mode:Some("ask".into()),effort:None,context_window:None,fast_mode:false,additional_directories:vec![],
        env:Some(std::collections::HashMap::from([("CODEMUX_AGENT_CONTROL_FIXTURE_ROOT".into(),rig.to_string_lossy().into_owned())])),
        workspace_id:None,extra:serde_json::Value::Null,recorded_usage_baseline:None,
    }, Some(thread.into())).await.unwrap();
    let first = agent_chat::agent_chat_send_turn(app.handle().clone(), ProviderKind::Cursor, serde_json::from_value(json!({
        "thread_id":thread,"text":"review-hold","delivery":"queue","client_nonce":"mario-parent-nonce",
    })).unwrap()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if app.state::<NativeControlState>().thread_runtime(thread).is_some_and(|v|!v.pending_approvals.is_empty()) {break;}
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let sent = execute(app.handle(), &ControlCaller::trusted(), "thread_send", json!({
        "workspace_id":workspace,"thread_id":thread,"client_request_id":"mario-queue-b","message":"mario-held-b","delivery":"queue",
    })).await.unwrap();
    let accepted = settle_operation(&app, &sent).await;
    assert_eq!(accepted["state"], "succeeded");
    let queued = accepted["result"]["turn"]["queued_id"].as_str().unwrap().to_string();
    let target = json!({"workspace_id":workspace,"thread_id":thread});
    let healthy = execute(app.handle(), &ControlCaller::trusted(), "thread_status", target.clone()).await.unwrap();
    assert!(healthy["queued_ids"].as_array().unwrap().iter().any(|id|id==&queued));
    assert_eq!(healthy["uncertain_queued_ids"], json!([]), "current native queues are not historical uncertainty");
    if mode == "cancelled" {
        assert!(agent_chat::agent_chat_cancel_queued_turn(app.handle().clone(), ProviderKind::Cursor, ThreadId(thread.into()), queued.clone()).await.unwrap());
    } else if mode == "dispatched" { gate.release.add_permits(1); }
    let reply = execute(app.handle(), &ControlCaller::trusted(), "thread_respond", json!({
        "workspace_id":workspace,"thread_id":thread,"client_request_id":"mario-parent-reply","request_id":healthy["pending_approvals"][0]["request_id"],"decision":"allow_once",
    })).await.unwrap();
    assert_eq!(settle_operation(&app, &reply).await["state"], "succeeded");
    if mode != "cancelled" {
        tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.notified()).await.unwrap();
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let db = app.state::<crate::database::DatabaseStore>();
            let parent_done = db.control_turn_outcome(thread,&first.turn_id.0).unwrap().is_some();
            let latest = db.control_last_thread_run_event(thread).unwrap();
            let b_done = latest.is_some_and(|e|e["type"]=="turn_completed" && e["turn_id"]!=first.turn_id.0);
            let disposed = channel.lock().unwrap().iter().any(|p| matches!(p["event"]["type"].as_str(),Some("queued_turn_dispatched"|"queued_turn_cancelled")) && p["event"]["queued_id"]==queued);
            if parent_done && (mode=="crash" || disposed && (mode=="cancelled" || b_done)) {break;}
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let dispositions = channel.lock().unwrap().iter().filter(|p| matches!(p["event"]["type"].as_str(),Some("queued_turn_dispatched"|"queued_turn_cancelled")) && p["event"]["queued_id"]==queued).count();
    assert_eq!(dispositions, usize::from(mode!="crash"));
    let wire = std::fs::read_to_string(rig.join("peer-wire.jsonl")).unwrap();
    assert_eq!(wire.contains("mario-held-b"), mode=="dispatched", "only actual dispatch reaches the child");
    let snapshot = std::env::var_os("CODEMUX_MARIO_RECEIPT").map(std::path::PathBuf::from)
        .map(|p|p.with_file_name(format!("r3-{mode}-boundary.sqlite"))).unwrap_or_else(||peer_root.path().join("snapshot.sqlite"));
    let recovered_db = app.state::<crate::database::DatabaseStore>().control_fixture_snapshot(&snapshot);
    let rows: Vec<serde_json::Value> = recovered_db.list_agent_chat_messages(thread).iter().map(|p|serde_json::from_str(p).unwrap()).collect();
    let fanout = channel.lock().unwrap().clone();
    // Cleanup cannot rewrite the frozen crash boundary. The actor remains
    // unchanged; its real Stop reaps the synthetic peer after snapshotting.
    bridge.abort();let _ = bridge.await;
    let peer_pid: u32 = std::fs::read_to_string(rig.join("peer-pid")).unwrap().parse().unwrap();
    provider.stop_session(ThreadId(thread.into())).await.unwrap();
    assert!(!provider.has_session(&ThreadId(thread.into())).await);
    #[cfg(target_os = "linux")]
    assert!(!std::path::Path::new(&format!("/proc/{peer_pid}")).exists(), "Stop must reap the exact synthetic peer PID");
    gate.release.add_permits(1);
    let old_epoch = app.state::<NativeControlState>().epoch.clone();
    app.unmanage::<NativeControlState>().unwrap();app.manage(NativeControlState::default());
    app.unmanage::<ProviderRegistry>().unwrap();app.manage(ProviderRegistry::new());
    let recovered_provider = Arc::new(mock_agent_provider::MockAgentProvider::new(ProviderKind::Cursor));
    app.state::<ProviderRegistry>().set_cursor(recovered_provider.clone()).await;
    app.unmanage::<crate::database::DatabaseStore>().unwrap();app.manage(recovered_db);
    let restarted = execute(app.handle(), &ControlCaller::trusted(), "thread_status", target).await.unwrap();
    let waited = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", json!({
        "workspace_id":workspace,"thread_id":thread,"timeout_ms":1,
    })).await.unwrap();
    mario_save_receipt(&format!("r3-{mode}"), &json!({"queued_id":queued,"parent_turn":first.turn_id,"accepted":accepted,"healthy":healthy,
        "fanout":fanout,"rows":rows,"wire":wire,"snapshot":snapshot,"peer_pid":peer_pid,"peer_stopped":true,"restarted":restarted,"waited":waited}));
    assert_ne!(app.state::<NativeControlState>().epoch, old_epoch);
    assert!(app.state::<NativeControlState>().thread_runtime(thread).is_none(), "history must not resurrect an actor queue or callbacks");
    assert!(recovered_provider.calls.snapshot().is_empty(), "status and wait must not create a worker or reserve a turn");
    assert_eq!(provider.starts.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(restarted["settled"], mode!="crash", "only undisposed accepted B is restart uncertainty: {restarted}");
    assert_eq!(restarted["phase"], if mode=="crash" {"needs_attention"} else {"completed"});
    assert_eq!(restarted["queued_ids"], json!([]));
    assert_eq!(restarted["pending_approvals"], json!([]));
    assert_eq!(restarted["uncertain_queued_ids"], if mode=="crash" {json!([queued])} else {json!([])});
    assert_eq!(waited["settled"], mode!="crash");assert_eq!(waited["timed_out"], mode=="crash");
    assert_eq!(rows.iter().filter(|r|r["type"]=="native_queue_control" && r["queued_id"]==queued && r["disposition"]=="accepted").count(),1,
        "native enqueue event and send acknowledgement deduplicate in the real journal");
    assert_eq!(rows.iter().filter(|r|r["type"]=="native_queue_control" && r["queued_id"]==queued && r["disposition"]!="accepted").count(),usize::from(mode!="crash"));
    assert!(!rows.iter().any(|r|r["type"]=="turn_queued"), "durable queue evidence must not rehydrate actionable queued bubbles");
    assert!(!rows.iter().any(|r|r["type"]=="native_queue_control" && r.to_string().contains("mario-held-b")));
    assert!(!fanout.iter().any(|r|r["event"]["type"]=="native_queue_control"), "control journal is not an actor event");
}

// The only substituted seam is provider discovery/session construction. All
// sends, FIFO drain, HTTP/SSE translation, native guards, bridge and DB are real.
struct RecoveryOpenCode {
    session: Arc<crate::agent_provider::opencode::OpenCodeSession>,
    tx: tokio::sync::broadcast::Sender<crate::agent_provider::ProviderRuntimeEvent>,
    metadata: mock_agent_provider::MockAgentProvider,
}
#[async_trait::async_trait]
impl crate::agent_provider::AgentProvider for RecoveryOpenCode {
    fn kind(&self) -> crate::agent_provider::ProviderKind { crate::agent_provider::ProviderKind::OpenCode }
    fn capabilities(&self) -> crate::agent_provider::ProviderCapabilities { self.metadata.capabilities() }
    async fn start_session(&self, input: crate::agent_provider::StartSessionInput) -> Result<crate::agent_provider::ProviderSession, crate::agent_provider::ProviderError> { self.metadata.start_session(input).await }
    async fn send_turn(&self, input: crate::agent_provider::SendTurnInput) -> Result<crate::agent_provider::TurnStartResult, crate::agent_provider::ProviderError> {
        self.session.enqueue_or_send(input).await
    }
    async fn steer_turn(&self, input: crate::agent_provider::SendTurnInput) -> Result<crate::agent_provider::TurnStartResult, crate::agent_provider::ProviderError> { self.session.steer_turn(input).await }
    async fn interrupt_turn(&self, _: crate::agent_provider::ThreadId, id: Option<crate::agent_provider::TurnId>) -> Result<(), crate::agent_provider::ProviderError> { self.session.interrupt_selected(id).await }
    async fn cancel_queued_turn(&self, _: crate::agent_provider::ThreadId, id: String) -> Result<bool, crate::agent_provider::ProviderError> { Ok(self.session.cancel_queued(&id).await) }
    async fn send_queued_turn_now(&self, _: crate::agent_provider::ThreadId, id: String) -> Result<(), crate::agent_provider::ProviderError> { self.session.send_queued_now(&id, false).await }
    async fn respond_to_request(&self, _: crate::agent_provider::ThreadId, r: crate::agent_provider::RequestId, d: crate::agent_provider::ApprovalDecision) -> Result<(), crate::agent_provider::ProviderError> { self.session.respond_to_request(r,d).await }
    async fn set_model(&self, _: crate::agent_provider::ThreadId, m: String) -> Result<(), crate::agent_provider::ProviderError> { self.session.set_model(m).await; Ok(()) }
    async fn set_permission_mode(&self, t: crate::agent_provider::ThreadId, m: String) -> Result<(), crate::agent_provider::ProviderError> { self.metadata.set_permission_mode(t,m).await }
    async fn stop_session(&self, _: crate::agent_provider::ThreadId) -> Result<(), crate::agent_provider::ProviderError> { self.session.shutdown().await; Ok(()) }
    async fn list_sessions(&self) -> Result<Vec<crate::agent_provider::ProviderSession>, crate::agent_provider::ProviderError> { Ok(vec![]) }
    async fn has_session(&self, _: &crate::agent_provider::ThreadId) -> bool { !self.session.is_dead() }
    async fn turn_active(&self, _: &crate::agent_provider::ThreadId) -> bool { self.session.turn_active().await }
    fn event_stream(&self) -> crate::agent_provider::ProviderEventStream {
        Box::pin(futures_util::stream::unfold(self.tx.subscribe(), |mut rx| async move { Some((rx.recv().await.unwrap(),rx)) }))
    }
}

struct RecoveryHttp {
    events: tokio::sync::broadcast::Sender<String>,
    connected: tokio::sync::Notify,
    b_received: tokio::sync::Notify,
    b_ack: tokio::sync::Semaphore,
    calls: std::sync::atomic::AtomicUsize,
    wire: std::sync::Mutex<Vec<serde_json::Value>>,
}
async fn recovery_sse(axum::extract::State(peer): axum::extract::State<Arc<RecoveryHttp>>) -> axum::response::Sse<impl futures_util::Stream<Item=Result<axum::response::sse::Event,std::convert::Infallible>>> {
    let rx = peer.events.subscribe();
    peer.connected.notify_one();
    axum::response::Sse::new(futures_util::stream::unfold(rx, |mut rx| async move {
        Some((Ok(axum::response::sse::Event::default().data(rx.recv().await.unwrap())),rx))
    }))
}
async fn recovery_prompt(axum::extract::State(peer): axum::extract::State<Arc<RecoveryHttp>>, axum::Json(body): axum::Json<serde_json::Value>) -> axum::http::StatusCode {
    peer.wire.lock().unwrap().push(body);
    if peer.calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst) == 1 {
        peer.b_received.notify_one();
        peer.b_ack.acquire().await.unwrap().forget();
    }
    axum::http::StatusCode::NO_CONTENT
}
async fn recovery_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !predicate() { tokio::task::yield_now().await; }
    }).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review5479417968_completion_before_queue_ack_recovers_matching_terminal() {
    recovery_opencode_boundary(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review5479417968_dispatched_incomplete_settles_only_selected_historical_a() {
    recovery_opencode_boundary(false).await;
}

async fn recovery_opencode_boundary(complete_b: bool) {
    use crate::agent_provider::{AgentProvider, ProviderKind, ThreadId};
    use crate::commands::agent_chat::{self, ProviderRegistry};
    use futures_util::StreamExt;
    let peer = Arc::new(RecoveryHttp {events:tokio::sync::broadcast::channel(16).0,connected:Default::default(),b_received:Default::default(),b_ack:tokio::sync::Semaphore::new(0),calls:Default::default(),wire:Default::default()});
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = axum::Router::new().route("/event",axum::routing::get(recovery_sse))
        .route("/session/recovery/prompt_async",axum::routing::post(recovery_prompt))
        .route("/session/recovery",axum::routing::delete(|| async {axum::http::StatusCode::NO_CONTENT}))
        .with_state(peer.clone());
    let server = tokio::spawn(async move {axum::serve(listener,router).await.unwrap();});
    let (session,tx) = crate::agent_provider::opencode::session::tests::recovery_session(format!("http://{address}")).await;
    tokio::time::timeout(std::time::Duration::from_secs(5),peer.connected.notified()).await.unwrap();
    let provider = Arc::new(RecoveryOpenCode {session,tx,metadata:mock_agent_provider::MockAgentProvider::new(ProviderKind::OpenCode)});
    let (app, _, root, workspace) = fixture().await;
    let thread = "t1";
    app.state::<crate::state::AppStateStore>().create_agent_chat_pane(&workspace,Some(ProviderKind::OpenCode),Some(root.path().to_string_lossy().into_owned()),None,Some(thread.into())).unwrap();
    app.state::<crate::database::DatabaseStore>().upsert_agent_chat_session(thread,&workspace,Some(root.path().to_str().unwrap()),"opencode").unwrap();
    app.state::<ProviderRegistry>().set_opencode(provider.clone()).await;
    let channel = mario_channel(app.handle(),thread);
    let mut stream = provider.event_stream();let bridge_app = app.handle().clone();
    let bridge = tokio::spawn(async move {while let Some(event) = stream.next().await {agent_chat::forward_event(&bridge_app,event);}});
    let a = agent_chat::agent_chat_send_turn(app.handle().clone(),ProviderKind::OpenCode,serde_json::from_value(json!({"thread_id":thread,"text":"recovery-a","client_nonce":"recovery-a-nonce"})).unwrap()).await.unwrap();
    assert!(provider.turn_active(&ThreadId(thread.into())).await);
    let receipt = execute(app.handle(),&ControlCaller::trusted(),"thread_send",json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"recovery-b-nonce","message":"recovery-b","delivery":"queue"})).await.unwrap();
    let accepted = settle_operation(&app,&receipt).await;
    assert_eq!(accepted["state"],"succeeded","{accepted}");
    let queued = accepted["result"]["turn"]["queued_id"].as_str().unwrap().to_string();
    let idle = json!({"type":"session.idle","properties":{"sessionID":"recovery"}}).to_string();
    peer.events.send(idle.clone()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5),peer.b_received.notified()).await.unwrap();
    recovery_until(||app.state::<crate::database::DatabaseStore>().control_turn_outcome(thread,&a.turn_id.0).unwrap().is_some()).await;
    recovery_until(||app.state::<NativeControlState>().thread_runtime(thread).is_some_and(|v|v.turn_id.as_deref().is_some_and(|id|id!=a.turn_id.0))).await;
    let b = app.state::<NativeControlState>().thread_runtime(thread).unwrap().turn_id.unwrap();
    assert_ne!(b,a.turn_id.0);
    if complete_b {
        peer.events.send(idle).unwrap();
        recovery_until(||app.state::<crate::database::DatabaseStore>().control_turn_outcome(thread,&b).unwrap().is_some()).await;
        assert!(!channel.lock().unwrap().iter().any(|p|p["event"]["type"]=="queued_turn_dispatched"),"B completed while its actual HTTP ACK is held");
    }
    peer.b_ack.add_permits(1);
    recovery_until(||app.state::<crate::database::DatabaseStore>().list_agent_chat_messages(thread).iter().any(|p| {
        let v:serde_json::Value=serde_json::from_str(p).unwrap();v["type"]=="user_message" && v["client_nonce"]=="recovery-b-nonce"
    })).await;
    recovery_until(||channel.lock().unwrap().iter().any(|p|p["event"]["type"]=="queued_turn_dispatched" && p["event"]["queued_id"]==queued)).await;
    let live = execute(app.handle(),&ControlCaller::trusted(),"thread_status",json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    let mode = if complete_b {"completed-before-ack"} else {"dispatched-incomplete"};
    let snapshot = std::env::var_os("CODEMUX_MARIO_RECEIPT").map(std::path::PathBuf::from).map(|p|p.with_file_name(format!("review-{mode}.sqlite"))).unwrap_or_else(||root.path().join("snapshot.sqlite"));
    let recovered_db = app.state::<crate::database::DatabaseStore>().control_fixture_snapshot(&snapshot);
    let rows:Vec<serde_json::Value> = recovered_db.list_agent_chat_messages(thread).iter().map(|p|serde_json::from_str(p).unwrap()).collect();
    let fanout = channel.lock().unwrap().clone();
    // Freeze before cleanup; shutdown cannot manufacture a terminal in it.
    bridge.abort();let _ = bridge.await;
    provider.stop_session(ThreadId(thread.into())).await.unwrap();
    server.abort();let _ = server.await;
    assert!(tokio::net::TcpListener::bind(address).await.is_ok(),"owned HTTP listener must be released");
    app.unmanage::<NativeControlState>().unwrap();app.manage(NativeControlState::default());
    app.unmanage::<ProviderRegistry>().unwrap();app.manage(ProviderRegistry::new());
    let cold_provider = Arc::new(mock_agent_provider::MockAgentProvider::new(ProviderKind::OpenCode));
    app.state::<ProviderRegistry>().set_opencode(cold_provider.clone()).await;
    app.unmanage::<crate::database::DatabaseStore>().unwrap();app.manage(recovered_db);
    let target = json!({"workspace_id":workspace,"thread_id":thread});
    let cold = execute(app.handle(),&ControlCaller::trusted(),"thread_status",target.clone()).await.unwrap();
    let mut wait_target = target;wait_target["timeout_ms"]=json!(1);
    let ordinary = execute(app.handle(),&ControlCaller::trusted(),"thread_wait",wait_target.clone()).await.unwrap();
    wait_target["turn_id"]=json!(a.turn_id.0);
    let selected_a = execute(app.handle(),&ControlCaller::trusted(),"thread_wait",wait_target.clone()).await.unwrap();
    wait_target["turn_id"]=json!(b);
    let selected_b = execute(app.handle(),&ControlCaller::trusted(),"thread_wait",wait_target).await.unwrap();
    mario_save_receipt(&format!("review-{mode}"),&json!({"fanout":fanout,"rows":rows,"wire":*peer.wire.lock().unwrap(),"snapshot":snapshot,"a":a.turn_id,"b":b,"queued_id":queued,"accepted":accepted,"live":live,"cold":cold,"ordinary":ordinary,"selected_a":selected_a,"selected_b":selected_b,"listener_released":true}));
    assert!(app.state::<NativeControlState>().thread_runtime(thread).is_none());
    assert!(cold_provider.calls.snapshot().is_empty(),"history reads must not dispatch, reserve or restore callbacks");
    assert_eq!(cold["queued_ids"],json!([]));assert_eq!(cold["uncertain_queued_ids"],json!([]));assert_eq!(cold["pending_approvals"],json!([]));
    assert_eq!(rows.iter().filter(|r|r["type"]=="user_message" && r["client_nonce"]=="recovery-b-nonce").count(),1);
    assert!(!rows.iter().any(|r|r["type"]=="native_queue_control" && r.to_string().contains("recovery-b")));
    assert_eq!(cold["phase"],if complete_b {"completed"} else {"unknown"},"late ACK/envelope cannot hide B's matching completion: {cold}");
    assert_eq!(cold["settled"],complete_b);
    assert_eq!(ordinary["settled"],complete_b);assert_eq!(ordinary["timed_out"],!complete_b);
    assert_eq!(selected_a["settled"],true,"completed historical A is independent of known newer B: {selected_a}");
    assert_eq!(selected_a["timed_out"],false);
    assert_eq!(selected_b["settled"],complete_b);assert_eq!(selected_b["timed_out"],!complete_b);
    assert_eq!(cold["turn_id"],b,"historical current identity is not runtime authority");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review5479417968_immediate_envelope_and_late_steering_remain_one_completed_run() {
    use crate::agent_provider::{ProviderKind, ProviderRuntimeEvent, ThreadId, TurnId, TurnStatus};
    use crate::commands::agent_chat::{self, ProviderRegistry};
    let (app, provider, root, workspace) = fixture().await;
    let launch = execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"recovery-immediate-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched = settle_operation(&app,&launch).await;
    let thread = launched["thread_id"].as_str().unwrap();
    let channel = mario_channel(app.handle(),thread);
    let (entered,release) = provider.hold_next_send();
    let send_app = app.handle().clone();let send_thread=thread.to_string();
    let send = tokio::spawn(async move {agent_chat::agent_chat_send_turn(send_app,ProviderKind::Claude,serde_json::from_value(json!({"thread_id":send_thread,"text":"immediate visible","client_nonce":"immediate-nonce"})).unwrap()).await.unwrap()});
    tokio::time::timeout(std::time::Duration::from_secs(5),entered.notified()).await.unwrap();
    // Scheduling-only synthetic completion before the mock send's ACK. The
    // real command must still correlate its late visible envelope by ID.
    agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::TurnCompleted {thread_id:ThreadId(thread.into()),turn_id:TurnId("mock-turn".into()),status:TurnStatus::Success,usage:None});
    release.notify_one();let result=send.await.unwrap();assert_eq!(result.turn_id.0,"mock-turn");
    agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::TurnQueued {thread_id:ThreadId(thread.into()),queued_id:"cancel-me".into(),text:"must not replay".into(),client_nonce:None});
    agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::QueuedTurnCancelled {thread_id:ThreadId(thread.into()),queued_id:"cancel-me".into()});
    agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::QueuedTurnDispatched {thread_id:ThreadId(thread.into()),queued_id:"late-steer".into(),turn_id:TurnId("mock-turn".into()),text:"late guidance".into(),steered:true});
    let snapshot=std::env::var_os("CODEMUX_MARIO_RECEIPT").map(std::path::PathBuf::from).map(|p|p.with_file_name("review-immediate-steered.sqlite")).unwrap_or_else(||root.path().join("snapshot.sqlite"));
    let db=app.state::<crate::database::DatabaseStore>().control_fixture_snapshot(&snapshot);
    let rows:Vec<serde_json::Value>=db.list_agent_chat_messages(thread).iter().map(|p|serde_json::from_str(p).unwrap()).collect();
    let user=rows.iter().find(|r|r["type"]=="user_message" && r["client_nonce"]=="immediate-nonce").unwrap();
    assert_eq!(user,&json!({"type":"user_message","thread_id":thread,"text":"immediate visible","client_nonce":"immediate-nonce"}),"ordinary user envelope fields are unchanged");
    let steer=rows.iter().find(|r|r["type"]=="user_message" && r["text"]=="late guidance").unwrap();
    assert_eq!(steer["steered_turn_id"],"mock-turn");
    for journal in rows.iter().filter(|r|r["type"]=="native_user_control") {
        let keys:std::collections::BTreeSet<_>=journal.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys,std::collections::BTreeSet::from(["type","thread_id","user_message_id","turn_id","steered"]));
    }
    app.unmanage::<crate::database::DatabaseStore>().unwrap();app.manage(db);
    app.unmanage::<NativeControlState>().unwrap();app.manage(NativeControlState::default());
    app.unmanage::<ProviderRegistry>().unwrap();app.manage(ProviderRegistry::new());
    let status=execute(app.handle(),&ControlCaller::trusted(),"thread_status",json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    let read=execute(app.handle(),&ControlCaller::trusted(),"thread_read",json!({"workspace_id":workspace,"thread_id":thread,"limit":100})).await.unwrap();
    assert_eq!(status["phase"],"completed");assert_eq!(status["settled"],true);assert_eq!(status["turn_id"],"mock-turn");
    assert_eq!(status["uncertain_queued_ids"],json!([]));assert_eq!(status["pending_approvals"],json!([]));assert_eq!(status["queued_ids"],json!([]));
    assert_eq!(read["total_visible_messages"],2);assert!(!read.to_string().contains("native_user_control"));
    assert!(app.state::<NativeControlState>().thread_runtime(thread).is_none());
    mario_save_receipt("review-immediate-steered",&json!({"fanout":*channel.lock().unwrap(),"rows":rows,"status":status,"read":read,"snapshot":snapshot}));
}

#[tokio::test]
async fn workspace_discovery_uses_app_owned_state() {
    let app = tauri::test::mock_app();
    app.manage(crate::state::AppStateStore::default());
    let expected = app.state::<crate::state::AppStateStore>().snapshot().workspaces.len();
    let result = execute(app.handle(), &ControlCaller::trusted(), "workspace_list", json!({})).await.unwrap();
    assert_eq!(result["workspaces"].as_array().unwrap().len(), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_binds_and_starts_one_normal_native_thread() {
    let (app, provider, _root, workspace) = fixture().await;
    let args = json!({"workspace_id": workspace, "client_request_id":"launch-a", "provider":"claude", "permission_mode":"default", "message":"Synthetic fixture task"});
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", args.clone()).await
        .expect("launch must be admitted through the native facade");
    let operation_id = receipt["operation_id"].as_str().unwrap();
    let settled = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let status = execute(app.handle(), &ControlCaller::trusted(), "operation_status", json!({"operation_id":operation_id})).await.unwrap();
            if status["state"] != "accepted" && status["state"] != "running" { break status; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(settled["state"], "succeeded", "{settled}");
    let thread = settled["thread_id"].as_str().unwrap();
    let state = app.state::<crate::state::AppStateStore>();
    let pane = state.agent_chat_pane_id_for_thread(thread).expect("must be a real pane binding");
    assert_eq!(state.workspace_id_for_pane(&pane).as_deref(), Some(workspace.as_str()));
    let record = app.state::<crate::database::DatabaseStore>().get_agent_chat_session(thread).unwrap();
    assert_eq!(record.permission_mode.as_deref(), Some("default"));
    assert!(provider.start_inputs().iter().all(|input| input.permission_mode.as_deref() == Some("default")));
    let retry = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", args).await.unwrap();
    assert_eq!(retry["operation_id"], receipt["operation_id"]);
    assert_eq!(provider.calls.snapshot().iter().filter(|call| matches!(call, mock_agent_provider::MockCall::StartSession(_))).count(), 1);
    assert_eq!(provider.calls.snapshot().iter().filter(|call| matches!(call, mock_agent_provider::MockCall::SendTurn(_, _))).count(), 1);
}

#[tokio::test]
async fn thread_discovery_includes_threads_without_provider_resume_ids_or_messages() {
    let (app, provider, _root, workspace) = fixture().await;
    app.state::<crate::database::DatabaseStore>().upsert_agent_chat_session("empty-native", &workspace, None, "claude").unwrap();
    let result = execute(app.handle(), &ControlCaller::trusted(), "thread_list", json!({"workspace_id":workspace})).await.unwrap();
    assert_eq!(result["threads"][0]["thread_id"], "empty-native");
    assert_eq!(result["threads"].as_array().unwrap().len(), 1);
    assert!(provider.calls.snapshot().is_empty(), "discovery must not start a provider");
}

#[tokio::test]
async fn thread_read_returns_only_safe_visible_prose_and_validates_workspace() {
    let (app, provider, _root, workspace) = fixture().await;
    let db = app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session("safe-thread", &workspace, None, "claude").unwrap();
    for event in [
        json!({"type":"user_message","thread_id":"safe-thread","text":"Visible task"}),
        json!({"type":"item_completed","thread_id":"safe-thread","turn_id":"t-a","item":{"kind":"assistant_text","text":"Visible answer"},"subagent_id":null}),
        json!({"type":"item_completed","thread_id":"safe-thread","turn_id":"t-a","item":{"kind":"reasoning","text":"hidden fixture reasoning"},"subagent_id":null}),
        json!({"type":"item_completed","thread_id":"safe-thread","turn_id":"t-a","item":{"kind":"assistant_text","text":"hidden fixture child"},"subagent_id":"child-a"}),
    ] { db.append_agent_chat_message("safe-thread", &event.to_string()).unwrap(); }
    let page = execute(app.handle(), &ControlCaller::trusted(), "thread_read", json!({"workspace_id":workspace,"thread_id":"safe-thread","limit":1})).await.unwrap();
    assert_eq!(page["messages"][0]["content"], "Visible task");
    assert_eq!(page["total_visible_messages"], 2);
    let tail = execute(app.handle(), &ControlCaller::trusted(), "thread_read", json!({"workspace_id":workspace,"thread_id":"safe-thread","cursor":page["next_cursor"]})).await.unwrap();
    assert_eq!(tail["messages"][0]["content"], "Visible answer");
    assert!(!tail.to_string().contains("hidden fixture"));
    let error = execute(app.handle(), &ControlCaller::trusted(), "thread_read", json!({"workspace_id":"missing-workspace","thread_id":"safe-thread"})).await.unwrap_err();
    assert_eq!(error.code, "workspace_not_found");
    assert!(provider.calls.snapshot().is_empty());
}

#[tokio::test]
async fn status_does_not_claim_an_unsettled_run_completed_after_restart() {
    let (app, provider, _root, workspace) = fixture().await;
    let db = app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session("unsettled", &workspace, None, "claude").unwrap();
    db.append_agent_chat_message("unsettled", &json!({"type":"user_message","thread_id":"unsettled","text":"Unsettled fixture task"}).to_string()).unwrap();
    let target = json!({"workspace_id":workspace,"thread_id":"unsettled"});
    let status = execute(app.handle(), &ControlCaller::trusted(), "thread_status", target.clone()).await.unwrap();
    assert_eq!(status["phase"], "unknown");
    assert_eq!(status["runtime_live"], false);
    assert_eq!(status["settled"], false);
    db.append_agent_chat_message("unsettled", &json!({"type":"turn_completed","thread_id":"unsettled","turn_id":"finished-turn","status":{"kind":"success"},"usage":null}).to_string()).unwrap();
    let status = execute(app.handle(), &ControlCaller::trusted(), "thread_status", target).await.unwrap();
    assert_eq!(status["phase"], "completed");
    assert_eq!(status["settled"], true);
    assert!(provider.calls.snapshot().is_empty(), "status must not resume a provider");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_l5_identical_retry_recovers_receipt_after_pane_close() {
    let (app, provider, _root, workspace) = fixture().await;
    let launch_args = json!({"workspace_id":workspace,"client_request_id":"review-l5-launch","provider":"claude","permission_mode":"default"});
    let launch = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", launch_args.clone()).await.unwrap();
    let launched = settle_operation(&app, &launch).await;
    let thread = launched["thread_id"].as_str().unwrap();
    let args = json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"review-l5-send","message":"One dispatch"});
    let sent = execute(app.handle(), &ControlCaller::trusted(), "thread_send", args.clone()).await.unwrap();
    assert_eq!(settle_operation(&app, &sent).await["state"], "succeeded");
    let pane = app.state::<crate::state::AppStateStore>().agent_chat_pane_id_for_thread(thread).unwrap();
    app.state::<crate::state::AppStateStore>().close_pane(&pane).unwrap();
    let retry = execute(app.handle(), &ControlCaller::trusted(), "thread_send", args.clone()).await;
    assert!(retry.is_ok(), "receipt recovery is not new dispatch and must not require a live pane: {retry:?}");
    assert_eq!(retry.unwrap()["operation_id"], sent["operation_id"]);
    let mut conflict = args;
    conflict["message"] = json!("Changed dispatch");
    assert_eq!(execute(app.handle(), &ControlCaller::trusted(), "thread_send", conflict).await.unwrap_err().code, "request_key_conflict");
    // Availability changes must not hide a launch receipt either.
    app.state::<NativeControlState>().fixture_capabilities.lock().unwrap().remove(&crate::agent_provider::ProviderKind::Claude);
    assert_eq!(execute(app.handle(), &ControlCaller::trusted(), "thread_launch", launch_args).await.unwrap()["operation_id"], launch["operation_id"]);
    assert_eq!(provider.calls.snapshot().iter().filter(|c| matches!(c, mock_agent_provider::MockCall::SendTurn(_, _))).count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_l6_selected_parent_wait_remains_busy_until_delegate_settles() {
    use crate::agent_provider::{ProviderRuntimeEvent, ThreadId, TurnId, TurnStatus, SubagentSnapshot, SubagentStatus};
    let (app, _, _root, workspace) = fixture().await;
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"review-l6-launch","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app, &receipt).await;
    let thread = launched["thread_id"].as_str().unwrap();
    crate::commands::agent_chat::forward_event(app.handle(), ProviderRuntimeEvent::SubagentUpdated {
        thread_id:ThreadId(thread.into()), subagent:SubagentSnapshot {
            subagent_id:"real-agent-child".into(), status:SubagentStatus::Running, ..Default::default()
        },
    });
    crate::commands::agent_chat::forward_event(app.handle(), ProviderRuntimeEvent::TurnCompleted {
        thread_id:ThreadId(thread.into()),turn_id:TurnId("parent-a".into()),status:TurnStatus::Success,usage:None,
    });
    assert!(app.state::<crate::commands::agent_chat::SubagentTracker>().delegated_work_holding_turn(thread));
    let target = json!({"workspace_id":workspace,"thread_id":thread,"turn_id":"parent-a","timeout_ms":1});
    let response = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", target.clone()).await.unwrap();
    assert_eq!(response["status"]["settled"], false);
    assert_eq!(response["settled"], false, "selected current parent must honor actual native delegate liveness: {response}");
    assert_eq!(response["timed_out"], true);
    crate::commands::agent_chat::forward_event(app.handle(), ProviderRuntimeEvent::SubagentUpdated {
        thread_id:ThreadId(thread.into()), subagent:SubagentSnapshot {
            subagent_id:"real-agent-child".into(), status:SubagentStatus::Completed, ..Default::default()
        },
    });
    assert_eq!(execute(app.handle(), &ControlCaller::trusted(), "thread_wait", target.clone()).await.unwrap()["settled"], true);
    crate::commands::agent_chat::forward_event(app.handle(), ProviderRuntimeEvent::SessionStateChanged {
        thread_id:ThreadId(thread.into()),status:crate::agent_provider::SessionStatus::Running { active_turn:TurnId("newer-b".into()) },
    });
    assert_eq!(execute(app.handle(), &ControlCaller::trusted(), "thread_wait", target).await.unwrap()["settled"], true,
        "historical settled selected turn must remain independent of newer work");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_l2_receipt_admitted_send_cannot_restart_after_completed_native_stop() {
    use crate::agent_provider::{ThreadId, ProviderKind, AgentProvider};
    let (app, provider, _root, workspace) = fixture().await;
    let launch = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"review-l2-launch","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app, &launch).await;
    let thread = launched["thread_id"].as_str().unwrap();
    let barrier = Arc::new(tokio::sync::Semaphore::new(0));
    *app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap() = Some(barrier.clone());
    let send = execute(app.handle(), &ControlCaller::trusted(), "thread_send", json!({
        "workspace_id":workspace,"thread_id":thread,"client_request_id":"review-l2-send","message":"Must not resurrect",
    })).await.unwrap();
    assert_eq!(send["state"], "accepted");
    crate::commands::agent_chat::agent_chat_stop_session(app.handle().clone(), ProviderKind::Claude, ThreadId(thread.into())).await.unwrap();
    assert!(!provider.has_session(&ThreadId(thread.into())).await);
    barrier.add_permits(1);
    let outcome = settle_operation(&app, &send).await;
    assert_eq!(outcome["state"], "failed", "accepted pre-Stop send must keep its admitted generation: {outcome}");
    assert!(!provider.has_session(&ThreadId(thread.into())).await);
    assert_eq!(provider.calls.snapshot().iter().filter(|c| matches!(c, mock_agent_provider::MockCall::StartSession(_))).count(), 1);
    assert_eq!(provider.calls.snapshot().iter().filter(|c| matches!(c, mock_agent_provider::MockCall::SendTurn(_, _))).count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_s4_outside_approval_handle_survives_no_provider_id_recycling() {
    use crate::agent_provider::{ProviderRuntimeEvent, ThreadId, TurnId, RequestId};
    let (app, provider, _root, workspace) = fixture().await;
    let launch = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"review-s4-launch","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app, &launch).await;
    let thread = launched["thread_id"].as_str().unwrap();
    let request = || ProviderRuntimeEvent::RequestOpened {
        thread_id:ThreadId(thread.into()),turn_id:TurnId("turn".into()),request_id:RequestId("codex-req-1".into()),
        request_kind:"tool_approval".into(),payload:json!({}),tool_use_id:None,subagent_id:None,
    };
    app.state::<NativeControlState>().observe_event(&request());
    let target = json!({"workspace_id":workspace,"thread_id":thread});
    let before = execute(app.handle(), &ControlCaller::trusted(), "thread_status", target.clone()).await.unwrap()["pending_approvals"][0]["request_id"].clone();
    // Process B starts with new state and the same first provider-local id.
    let _old_state = app.unmanage::<NativeControlState>().unwrap();
    app.manage(NativeControlState::default());
    app.state::<NativeControlState>().observe_event(&request());
    let after = execute(app.handle(), &ControlCaller::trusted(), "thread_status", target).await.unwrap()["pending_approvals"][0]["request_id"].clone();
    assert_ne!(before, after, "outside callback handles must not recycle when provider counters restart");
    let args = json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"old-unadmitted-reply","request_id":before,"decision":"deny"});
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_respond", args).await.unwrap();
    assert_eq!(settle_operation(&app, &receipt).await["state"], "failed");
    assert_eq!(provider.calls.snapshot().iter().filter(|c| matches!(c, mock_agent_provider::MockCall::RespondToRequest(_, _))).count(), 0);
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_respond", json!({
        "workspace_id":workspace,"thread_id":thread,"client_request_id":"current-reply","request_id":after,"decision":"deny",
    })).await.unwrap();
    assert_eq!(settle_operation(&app, &receipt).await["state"], "succeeded");
}

async fn settle_operation(app: &tauri::App<tauri::test::MockRuntime>, receipt: &serde_json::Value) -> serde_json::Value {
    let id = receipt["operation_id"].as_str().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let status = execute(app.handle(), &ControlCaller::trusted(), "operation_status", json!({"operation_id":id})).await.unwrap();
            if status["state"] != "accepted" && status["state"] != "running" { break status; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_up_send_uses_native_intake_once_and_persists_visible_user_message() {
    let (app, provider, _root, workspace) = fixture().await;
    let launched = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"launch-send","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app,&launched).await;
    assert_eq!(launched["state"], "succeeded");
    let thread = launched["thread_id"].as_str().unwrap();
    let args = json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"send-a","message":"Synthetic follow-up","delivery":"queue"});
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_send", args.clone()).await.unwrap();
    let delivered = settle_operation(&app,&receipt).await;
    assert_eq!(delivered["state"], "succeeded", "{delivered}");
    let retry = execute(app.handle(), &ControlCaller::trusted(), "thread_send", args).await.unwrap();
    assert_eq!(retry["operation_id"], receipt["operation_id"]);
    let transcript = execute(app.handle(), &ControlCaller::trusted(), "thread_read", json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    assert_eq!(transcript["messages"][0]["content"], "Synthetic follow-up");
    assert_eq!(provider.calls.snapshot().iter().filter(|call| matches!(call,mock_agent_provider::MockCall::SendTurn(_, _))).count(),1);
}

#[tokio::test]
async fn capabilities_discover_provider_owned_models_and_operations() {
    let (app, provider, _root, workspace) = fixture().await;
    let expected = app.state::<NativeControlState>().fixture_capabilities.lock().unwrap()
        .get(&crate::agent_provider::ProviderKind::Claude).cloned().unwrap();
    let response = execute(app.handle(), &ControlCaller::trusted(), "agent_capabilities", json!({"workspace_id":workspace,"provider":"claude"})).await.unwrap();
    assert_eq!(response["providers"][0]["provider"], "claude");
    assert_eq!(response["providers"][0]["capabilities"], serde_json::to_value(expected).unwrap());
    assert_eq!(response["providers"][0]["operations"]["supports_interrupt"], true);
    assert!(provider.calls.snapshot().is_empty(), "metadata must not launch a coding session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_reaches_native_provider_without_deleting_thread_or_pane() {
    let (app, provider, root, workspace) = fixture().await;
    let launched = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"launch-interrupt","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app,&launched).await;
    let thread = launched["thread_id"].as_str().unwrap();
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_interrupt", json!({
        "workspace_id":workspace,"thread_id":thread,"client_request_id":"interrupt-a",
    })).await.unwrap();
    let outcome = settle_operation(&app,&receipt).await;
    assert_eq!(outcome["state"], "succeeded", "{outcome}");
    assert_eq!(outcome["result"]["reached_provider"], true);
    assert!(app.state::<crate::state::AppStateStore>().agent_chat_pane_id_for_thread(thread).is_some());
    assert!(app.state::<crate::database::DatabaseStore>().get_agent_chat_session(thread).is_some());
    assert!(root.path().is_dir());
    assert_eq!(provider.calls.snapshot().iter().filter(|call|matches!(call,mock_agent_provider::MockCall::InterruptTurn(_, _))).count(),1);
    assert_eq!(provider.calls.snapshot().iter().filter(|call|matches!(call,mock_agent_provider::MockCall::StopSession(_))).count(),0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_shuts_down_native_session_and_preserves_pane_history_and_files() {
    use crate::agent_provider::AgentProvider;
    let (app, provider, root, workspace) = fixture().await;
    let launched = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"launch-stop","provider":"claude","permission_mode":"default","message":"Preserved fixture task",
    })).await.unwrap();
    let launched = settle_operation(&app,&launched).await;
    let thread = launched["thread_id"].as_str().unwrap();
    let args = json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"stop-a"});
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_stop", args.clone()).await.unwrap();
    let outcome = settle_operation(&app,&receipt).await;
    assert_eq!(outcome["state"], "succeeded", "{outcome}");
    assert_eq!(outcome["result"]["stopped"], true);
    assert!(!provider.has_session(&crate::agent_provider::ThreadId(thread.into())).await);
    let retry = execute(app.handle(), &ControlCaller::trusted(), "thread_stop", args).await.unwrap();
    assert_eq!(retry["operation_id"],receipt["operation_id"]);
    assert!(app.state::<crate::state::AppStateStore>().agent_chat_pane_id_for_thread(thread).is_some());
    assert!(root.path().is_dir());
    let page = execute(app.handle(), &ControlCaller::trusted(), "thread_read", json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    assert_eq!(page["messages"][0]["content"],"Preserved fixture task");
    assert_eq!(provider.calls.snapshot().iter().filter(|call|matches!(call,mock_agent_provider::MockCall::StopSession(_))).count(),1);
}

#[tokio::test]
async fn wait_is_bounded_and_never_mistakes_missing_runtime_for_completion() {
    let (app, provider, _root, workspace) = fixture().await;
    let db = app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session("wait-thread", &workspace, None, "claude").unwrap();
    let response = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", json!({
        "workspace_id":workspace,"thread_id":"wait-thread","timeout_ms":20,
    })).await.unwrap();
    assert_eq!(response["timed_out"],true);
    assert_eq!(response["status"]["phase"],"unknown");
    db.append_agent_chat_message("wait-thread", &json!({"type":"turn_completed","thread_id":"wait-thread","turn_id":"wait-turn","status":{"kind":"success"},"usage":null}).to_string()).unwrap();
    let response = execute(app.handle(), &ControlCaller::trusted(), "thread_wait", json!({
        "workspace_id":workspace,"thread_id":"wait-thread","turn_id":"wait-turn","timeout_ms":20,
    })).await.unwrap();
    assert_eq!(response["timed_out"],false);
    assert_eq!(response["settled"],true);
    assert!(provider.calls.snapshot().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_facade_uses_current_native_callback_and_retry_receipt() {
    use crate::agent_provider::{ProviderRuntimeEvent, RequestId, ThreadId, TurnId};
    let (app, provider, _root, workspace) = fixture().await;
    let launched = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"launch-approval","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app, &launched).await;
    assert_eq!(launched["state"], "succeeded");
    let thread = launched["thread_id"].as_str().unwrap();
    app.state::<NativeControlState>().observe_event(&ProviderRuntimeEvent::RequestOpened {
        thread_id:ThreadId(thread.into()),turn_id:TurnId("approval-turn".into()),
        request_id:RequestId("approval-request".into()),request_kind:"tool".into(),
        payload:json!({"fixture_private_payload":"must not be exposed"}),
        tool_use_id:None,subagent_id:None,
    });
    let status = execute(app.handle(), &ControlCaller::trusted(), "thread_status", json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    let args = json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"reply-once",
        "request_id":status["pending_approvals"][0]["request_id"],"decision":"allow_once"});
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_respond", args.clone()).await.unwrap();
    let outcome = settle_operation(&app, &receipt).await;
    assert_eq!(outcome["state"],"succeeded","{outcome}");
    let retry = execute(app.handle(), &ControlCaller::trusted(), "thread_respond", args.clone()).await.unwrap();
    assert_eq!(retry["operation_id"],receipt["operation_id"]);
    let mut stale = args;
    stale["client_request_id"] = json!("reply-to-retired");
    let retired = execute(app.handle(), &ControlCaller::trusted(), "thread_respond", stale).await.unwrap();
    assert_eq!(settle_operation(&app, &retired).await["state"],"failed");
    assert_eq!(provider.calls.snapshot().iter().filter(|c|matches!(c,mock_agent_provider::MockCall::RespondToRequest(_, _))).count(),1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervised_facade_does_not_send_to_live_full_access_worker() {
    let (app, provider, _root, workspace) = fixture().await;
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"launch-ceiling","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app, &receipt).await;
    assert_eq!(launched["state"],"succeeded");
    let thread = launched["thread_id"].as_str().unwrap();
    app.state::<NativeControlState>().record_worker_mode(thread,crate::agent_provider::ProviderKind::Claude,Some("bypassPermissions".into()));
    let mut caller = ControlCaller::trusted();
    caller.access = ControlAccess::Supervised;
    let receipt = execute(app.handle(), &caller, "thread_send", json!({
        "workspace_id":workspace,"thread_id":thread,"client_request_id":"blocked-full-worker","message":"Must not reach provider",
    })).await.unwrap();
    let outcome = settle_operation(&app, &receipt).await;
    assert_eq!(outcome["state"],"failed","{outcome}");
    assert_eq!(provider.calls.snapshot().iter().filter(|c|matches!(c,mock_agent_provider::MockCall::SendTurn(_, _))).count(),0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_approval_status_does_not_report_previous_turn_as_settled() {
    use crate::agent_provider::{ProviderRuntimeEvent, RequestId, ThreadId, TurnId};
    let (app, _provider, _root, workspace) = fixture().await;
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"launch-status","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app, &receipt).await;
    assert_eq!(launched["state"],"succeeded");
    let thread = launched["thread_id"].as_str().unwrap();
    app.state::<crate::database::DatabaseStore>().append_agent_chat_message(thread,&json!({
        "type":"turn_completed","thread_id":thread,"turn_id":"old-turn","status":{"kind":"success"},"usage":null,
    }).to_string()).unwrap();
    app.state::<NativeControlState>().observe_event(&ProviderRuntimeEvent::RequestOpened {
        thread_id:ThreadId(thread.into()),turn_id:TurnId("current-turn".into()),request_id:RequestId("current-request".into()),
        request_kind:"tool".into(),payload:json!({"private_payload":"not for status"}),tool_use_id:None,subagent_id:None,
    });
    let target = json!({"workspace_id":workspace,"thread_id":thread});
    let status = execute(app.handle(), &ControlCaller::trusted(), "thread_status", target.clone()).await.unwrap();
    assert_eq!(status["phase"],"waiting_approval");
    assert_eq!(status["settled"],false);
    assert_eq!(status["turn_id"],"current-turn");
    assert_eq!(status["pending_approvals"].as_array().unwrap().len(), 1);
    assert_eq!(status["pending_approvals"][0]["turn_id"], "current-turn");
    assert_eq!(status["pending_approvals"][0]["request_kind"], "tool");
    assert_ne!(status["pending_approvals"][0]["request_id"], "current-request");
    assert!(!status.to_string().contains("private_payload"));
    let mut wait = target;
    wait["timeout_ms"] = json!(20);
    assert_eq!(execute(app.handle(), &ControlCaller::trusted(), "thread_wait", wait).await.unwrap()["timed_out"],true);
}
