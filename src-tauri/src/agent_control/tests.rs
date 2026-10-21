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

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn review_r1_human_activity_disposes_unsent_resumes_but_keeps_task_results() {
    let (app,provider,_root,workspace)=fixture().await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"r1-resume-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&receipt).await;
    let thread=launched["thread_id"].as_str().unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    let mut snapshots=vec![];
    for (id,origin) in [("stale-usage","usage_resume"),("stale-restart","restart_resume"),("task-result","delegation")] {
        let payload=json!({"provider":"claude","origin":origin,"input":{"thread_id":thread,"text":id,"client_nonce":id},"provenance":{"created_by":"system","creation_source":"codemux"},"caller":null});
        db.admit_message_delivery(id,thread,id,&payload,"old",0,None,false).unwrap();
        snapshots.push(db.message_delivery(id).unwrap().unwrap());
    }
    crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),crate::agent_provider::ProviderKind::Claude,serde_json::from_value(json!({"thread_id":thread,"text":"new task","client_nonce":"new-human-task"})).unwrap()).await.unwrap();
    for id in ["stale-usage","stale-restart"] { assert_eq!(db.message_delivery(id).unwrap().unwrap().state,"cancelled"); }
    assert_eq!(db.message_delivery("task-result").unwrap().unwrap().state,"pending");
    for row in snapshots { super::delivery::dispatch(app.handle(),row).await; }
    let sends:Vec<_>=provider.calls.snapshot().into_iter().filter_map(|call|match call {mock_agent_provider::MockCall::SendTurn(_,text)=>Some(text),_=>None}).collect();
    assert_eq!(sends,vec!["new task","task-result"]);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn review_r1_merged_saved_queue_sends_with_original_delivery_identity() {
    use crate::agent_provider::{message::*,types::MessageDelivery,ProviderKind,ThreadId};
    for restart_replay in [false,true] {
        let (app,provider,_root,workspace)=fixture().await;
        let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"r1-merge-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
        let launched=settle_operation(&app,&receipt).await;
        let survivor=launched["thread_id"].as_str().unwrap();
        let db=app.state::<crate::database::DatabaseStore>();
        db.upsert_agent_chat_session("merge-source",&workspace,None,"claude").unwrap();
        let provenance=MessageProvenance {message_id:Some("merge-message".into()),delivery_id:Some("merge-delivery".into()),origin:Some(crate::commands::agent_chat::TurnOrigin::Controller),..Default::default()};
        db.admit_message_delivery("merge-delivery","merge-source","merge-message",&json!({"provider":"claude","origin":"controller","input":{"thread_id":"merge-source","text":"saved task"},"provenance":provenance,"caller":null}),"old",0,None,true).unwrap();
        db.finish_message_delivery("merge-delivery","queued",Some(&json!({"queued_id":"merge-queue"})),None).unwrap();
        db.insert_queued_turn(&crate::database::queued_turns::QueuedTurnRecord {thread_id:"merge-source".into(),queued_id:"merge-queue".into(),position:0,text:"saved task".into(),prompt:None,provenance,skill_ids:vec![],skill_text:None,include_plugins:false,images:vec![],client_nonce:Some("merge-message".into()),model_override:None,effort_override:None,permission_mode_override:None,epoch:"old".into()}).unwrap();
        db.merge_reopened_agent_chat_session("merge-source",survivor).unwrap();
        if restart_replay { db.recover_message_deliveries("new").unwrap(); }
        crate::commands::queued_turns::send_saved_queued_turn(app.handle().clone(),ProviderKind::Claude,ThreadId(survivor.into()),"merge-queue".into(),MessageDelivery::Queue,!restart_replay).await.unwrap();
        assert!(db.get_queued_turn(survivor,"merge-queue").is_none());
        let row=db.message_delivery("merge-delivery").unwrap().unwrap();
        assert_eq!(row.state,"accepted"); assert_eq!(row.message_id,"merge-message");
        assert_eq!(provider.calls.snapshot().into_iter().filter(|call|matches!(call,mock_agent_provider::MockCall::SendTurn(..))).count(),1);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn review_r2_cursor_unanswered_prompts_settle_without_native_acceptance() {
    use crate::agent_provider::{AgentProvider,ProviderKind,ProviderRuntimeEvent,SendTurnInput,ThreadId};
    use futures_util::StreamExt;
    use std::os::unix::fs::PermissionsExt;
    for interrupt in [false,true] {
        let root=tempfile::tempdir().unwrap();
        let binary=root.path().join("peer.py");
        std::fs::write(&binary,include_str!("../../tests/helpers/agent_control_acp_peer.py")).unwrap();
        std::fs::set_permissions(&binary,std::fs::Permissions::from_mode(0o700)).unwrap();
        let provider=crate::agent_provider::cursor::CursorAgentProvider::new(crate::agent_provider::cursor::CursorProviderConfig {binary,event_channel_capacity:64,mcp_registry:None});
        let thread=ThreadId(format!("review-r2-cursor-{interrupt}"));
        provider.start_session(serde_json::from_value(json!({"thread_id":thread,"cwd":root.path(),"permission_mode":"ask","additional_directories":[],"env":{"CODEMUX_AGENT_CONTROL_FIXTURE_ROOT":root.path()}})).unwrap()).await.unwrap();
        let app=tauri::test::mock_app();
        app.manage(crate::database::DatabaseStore::new_in_memory());
        app.manage(crate::commands::agent_chat::SubagentTracker::default());
    app.manage(crate::commands::agent_chat::RunActivityTracker::default());
        app.manage(crate::commands::agent_chat::AgentChatChannelRegistry::default());
        let db=app.state::<crate::database::DatabaseStore>();
        db.upsert_agent_chat_session(&thread.0,"workspace",None,"cursor").unwrap();
        db.admit_message_delivery("unanswered",&thread.0,"unanswered",&json!({"origin":"controller","provider":"cursor"}),"epoch",0,None,true).unwrap();
        let mut input:SendTurnInput=serde_json::from_value(json!({"thread_id":thread,"text":if interrupt {"review-r2-unanswered-prompt"} else {"review-r2-exit-prompt"}})).unwrap();
        input.turn_checkpoint=Some(Arc::new(super::delivery::DeliveryCheckpoint {app:app.handle().clone(),id:"unanswered".into(),thread:thread.0.clone(),provider:ProviderKind::Cursor,inner:None,release_queue:false,admitted_generation:0,resumed_ids:vec![]}));
        let mut events=provider.event_stream();
        let sent=provider.send_turn(input).await.unwrap();
        if interrupt {
            tokio::time::timeout(std::time::Duration::from_secs(3),async {
                while !root.path().join("prompt-ready").exists() {tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
            }).await.unwrap();
            provider.interrupt_turn(thread.clone(),Some(sent.turn_id)).await.unwrap();
        }
        let completed=tokio::time::timeout(std::time::Duration::from_secs(8),async {
            while !matches!(events.next().await,Some(ProviderRuntimeEvent::TurnCompleted {..})) {}
        }).await;
        provider.stop_session(thread.clone()).await.unwrap();
        assert!(completed.is_ok(),"an unanswered interrupted prompt must have a cancellation deadline");
        let row=db.message_delivery("unanswered").unwrap().unwrap();
        assert!(row.attempted);
        assert_eq!(row.state,"held_unknown");
        assert_eq!(db.thread_message_delivery_attention(&thread.0).unwrap().len(),1);
        assert!(!db.has_inflight_message_deliveries(&thread.0).unwrap());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn review_r1_acp_local_rpc_refusal_is_not_native_acceptance() {
    use crate::agent_provider::{AgentProvider,ProviderKind,ProviderRuntimeEvent,SendTurnInput,ThreadId,TurnDispatchCheckpoint};
    use futures_util::StreamExt;
    use std::{os::unix::fs::PermissionsExt,sync::atomic::{AtomicBool,AtomicUsize,Ordering}};
    #[derive(Debug,Default)]
    struct Refusal { calls:AtomicUsize, accepted:AtomicBool }
    #[async_trait::async_trait]
    impl TurnDispatchCheckpoint for Refusal {
    fn native_stop_generation(&self) -> Result<u64, crate::agent_provider::ProviderError> { Ok(0) }

        fn authorize_dispatch(&self)->Result<(),crate::agent_provider::ProviderError> {
            if self.calls.fetch_add(1,Ordering::SeqCst)==0 {Ok(())}
            else {Err(crate::agent_provider::ProviderError::ValidationError {message:"refused at final prompt boundary".into()})}
        }
        async fn accepted(&self,_:&crate::agent_provider::TurnId) { self.accepted.store(true,Ordering::SeqCst); }
        async fn prepare(&self) {}
        async fn commit(&self) {}
        async fn abort(&self) {}
    }
    let root=tempfile::tempdir().unwrap();
    let binary=root.path().join("peer.py");
    std::fs::write(&binary,include_str!("../../tests/helpers/agent_control_acp_peer.py")).unwrap();
    std::fs::set_permissions(&binary,std::fs::Permissions::from_mode(0o700)).unwrap();
    let rig=root.path().join("rig");std::fs::create_dir(&rig).unwrap();
    let provider=crate::agent_provider::cursor::CursorAgentProvider::new(crate::agent_provider::cursor::CursorProviderConfig {binary,event_channel_capacity:64,mcp_registry:None});
    let thread=ThreadId("review-r1-local-rpc-refusal".into());
    provider.start_session(serde_json::from_value(json!({"thread_id":thread,"cwd":root.path(),"permission_mode":"ask","additional_directories":[],"env":{"CODEMUX_AGENT_CONTROL_FIXTURE_ROOT":rig}})).unwrap()).await.unwrap();
    let checkpoint=Arc::new(Refusal::default());
    let mut input:SendTurnInput=serde_json::from_value(json!({"thread_id":thread,"text":"must not prompt"})).unwrap();
    input.turn_checkpoint=Some(checkpoint.clone());
    let mut events=provider.event_stream();
    provider.send_turn(input).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3),async {
        while !matches!(events.next().await,Some(ProviderRuntimeEvent::TurnCompleted {..})) {}
    }).await.unwrap();
    let wire=std::fs::read_to_string(rig.join("peer-wire.jsonl")).unwrap();
    provider.stop_session(thread).await.unwrap();
    assert_eq!(checkpoint.calls.load(Ordering::SeqCst),2);
    assert!(!checkpoint.accepted.load(Ordering::SeqCst));
    assert!(!wire.contains("session/prompt"));
    assert_eq!(provider.kind(),ProviderKind::Cursor);
}

#[tokio::test]
async fn review_r3_grok_terminal_survives_interrupt_during_standard_response_grace() {
    use crate::agent_provider::{AgentProvider, ProviderKind, ProviderRuntimeEvent, SendTurnInput, ThreadId, TurnStatus};
    use futures_util::StreamExt;
    use std::time::Duration;

    let binary = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap()
        .join(format!("fake_grok_acp{}", std::env::consts::EXE_SUFFIX));
    assert!(binary.is_file(), "build the fake_grok_acp test fixture first");
    let provider = crate::agent_provider::grok::GrokAgentProvider::new(
        crate::agent_provider::grok::GrokProviderConfig { binary, event_channel_capacity: 128, mcp_registry: None },
    );
    let root = tempfile::tempdir().unwrap();
    let thread = ThreadId("review-r3-grok-terminal".into());
    provider.start_session(serde_json::from_value(json!({
        "thread_id": thread, "cwd": root.path(), "model": "grok-4.6", "effort": "high",
        "fresh_session": true, "permission_mode": "agent", "additional_directories": []
    })).unwrap()).await.unwrap();
    let app = tauri::test::mock_app();
    app.manage(crate::database::DatabaseStore::new_in_memory());
    app.manage(crate::commands::agent_chat::SubagentTracker::default());
    app.manage(crate::commands::agent_chat::RunActivityTracker::default());
    app.manage(crate::commands::agent_chat::AgentChatChannelRegistry::default());
    let db = app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session(&thread.0, "workspace", None, "grok").unwrap();
    db.admit_message_delivery("grok-terminal", &thread.0, "grok-terminal",
        &json!({"origin": "controller", "provider": "grok"}), "epoch", 0, None, true).unwrap();
    let mut input: SendTurnInput = serde_json::from_value(json!({
        "thread_id": thread, "text": "process boundary omitted-standard"
    })).unwrap();
    input.turn_checkpoint = Some(Arc::new(super::delivery::DeliveryCheckpoint {
        app: app.handle().clone(), id: "grok-terminal".into(), thread: thread.0.clone(),
        provider: ProviderKind::Grok, inner: None, release_queue: false, admitted_generation: 0, resumed_ids: vec![],
    }));
    let mut events = provider.event_stream();
    let sent = provider.send_turn(input).await.unwrap();
    // This fixture marker follows the durable terminal on the same ordered
    // notification stream. No standard session/prompt response will arrive.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match events.next().await {
                Some(ProviderRuntimeEvent::ContextCompactionChanged { active: false, .. }) => break,
                Some(ProviderRuntimeEvent::TurnCompleted { .. }) => panic!("terminal grace ended before interrupt"),
                Some(_) => {}
                None => panic!("provider stream closed"),
            }
        }
    }).await.unwrap();
    provider.interrupt_turn(thread.clone(), Some(sent.turn_id)).await.unwrap();
    let mut usage_events = vec![];
    let (status, usage) = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            match events.next().await {
                Some(event @ ProviderRuntimeEvent::UsageRecorded { .. }) => usage_events.push(event),
                Some(ProviderRuntimeEvent::TurnCompleted { status, usage, .. }) => break (status, usage),
                Some(_) => {}
                None => panic!("provider stream closed"),
            }
        }
    }).await.expect("cancellation must settle before the 30-second response grace");
    let row = db.message_delivery("grok-terminal").unwrap().unwrap();
    let attention = db.thread_message_delivery_attention(&thread.0).unwrap();
    db.recover_message_deliveries("restart").unwrap();
    let recovered = db.message_delivery("grok-terminal").unwrap().unwrap();

    // Reusing the same session also catches unnecessary child shutdown and
    // stale cancellation leaking into the next prompt.
    let next = provider.send_turn(serde_json::from_value(json!({
        "thread_id": thread, "text": "process boundary follow-up"
    })).unwrap()).await;
    let next_status = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match events.next().await {
                Some(ProviderRuntimeEvent::TurnCompleted { status, .. }) => break status,
                Some(_) => {}
                None => panic!("provider stream closed before follow-up completed"),
            }
        }
    }).await;
    provider.stop_session(thread).await.unwrap();

    assert_eq!(row.state, "accepted", "durable terminal proves native acceptance: {status:?}");
    assert!(row.attempted);
    assert!(attention.is_empty());
    assert_eq!(recovered.state, "accepted");
    assert!(matches!(status, TurnStatus::Error { ref subtype, ref message }
        if subtype == "grok_acp" && message == "durable fallback must lose to standard response"));
    assert_eq!(usage_events.len(), 1);
    assert!(matches!(&usage_events[0], ProviderRuntimeEvent::UsageRecorded {
        input_tokens: 800, output_tokens: 80, cost_usd: Some(cost), ..
    } if (*cost - 0.9).abs() < f64::EPSILON));
    let usage = usage.expect("durable terminal usage");
    assert_eq!(usage.total_cost_usd, Some(0.9));
    assert_eq!(usage.duration_ms, 321);
    assert_eq!(usage.num_turns, 3);
    assert!(next.is_ok(), "healthy child must accept follow-up: {next:?}");
    assert!(matches!(next_status, Ok(TurnStatus::Success)), "follow-up must complete on the same child: {next_status:?}");
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
    use crate::agent_provider::{ProviderRuntimeEvent, RequestId, SessionStatus, SubagentSnapshot, SubagentStatus, ThreadId, TurnId, TurnStatus,
    };
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
    fn native_stop_generation(&self) -> Result<u64, crate::agent_provider::ProviderError> { Ok(0) }

    fn authorize_dispatch(&self) -> Result<(), crate::agent_provider::ProviderError> { self.inner.authorize_dispatch() }
    async fn accepted(&self, turn: &crate::agent_provider::TurnId) { self.inner.accepted(turn).await; }
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
    let inner = Arc::new(crate::agent_provider::cursor::CursorAgentProvider::new(crate::agent_provider::cursor::CursorProviderConfig {binary,event_channel_capacity:256, mcp_registry: None }));
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
        permission_mode:Some("ask".into()),effort:None,context_window:None,fast_mode:false,service_tier:None,additional_directories:vec![],
        env:Some(std::collections::HashMap::from([("CODEMUX_AGENT_CONTROL_FIXTURE_ROOT".into(),rig.to_string_lossy().into_owned())])),
        workspace_id:None,remote:None,extra:serde_json::Value::Null,recorded_usage_baseline:None,
    }, Some(thread.into())).await.unwrap();
    let first = agent_chat::send_desktop_turn(app.handle().clone(), ProviderKind::Cursor, serde_json::from_value(json!({
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
    async fn set_model(&self, _: crate::agent_provider::ThreadId, m: String) -> Result<(), crate::agent_provider::ProviderError> { self.session.set_model(m).await }
    async fn set_permission_mode(&self, t: crate::agent_provider::ThreadId, m: String) -> Result<(), crate::agent_provider::ProviderError> { self.metadata.set_permission_mode(t,m).await }
    async fn stop_session(&self, _: crate::agent_provider::ThreadId) -> Result<(), crate::agent_provider::ProviderError> { self.session.shutdown().await }
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
    let a = agent_chat::send_desktop_turn(app.handle().clone(),ProviderKind::OpenCode,serde_json::from_value(json!({"thread_id":thread,"text":"recovery-a","client_nonce":"recovery-a-nonce"})).unwrap()).await.unwrap();
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
    let send = tokio::spawn(async move {agent_chat::send_desktop_turn(send_app,ProviderKind::Claude,serde_json::from_value(json!({"thread_id":send_thread,"text":"immediate visible","client_nonce":"immediate-nonce"})).unwrap()).await.unwrap()});
    tokio::time::timeout(std::time::Duration::from_secs(5),entered.notified()).await.unwrap();
    // Scheduling-only synthetic completion before the mock send's ACK. The
    // real command must still correlate its late visible envelope by ID.
    agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::TurnCompleted {thread_id:ThreadId(thread.into()),turn_id:TurnId("mock-turn".into()),status:TurnStatus::Success,usage:None});
    release.notify_one();let result=send.await.unwrap();assert_eq!(result.turn_id.0,"mock-turn");
    agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::TurnQueued {provenance: Default::default(),thread_id:ThreadId(thread.into()),queued_id:"cancel-me".into(),text:"must not replay".into(),client_nonce:None});
    agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::QueuedTurnCancelled {thread_id:ThreadId(thread.into()),queued_id:"cancel-me".into()});
    agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::QueuedTurnDispatched {thread_id:ThreadId(thread.into()),queued_id:"late-steer".into(),turn_id:TurnId("mock-turn".into()),text:"late guidance".into(),steered:true});
    let snapshot=std::env::var_os("CODEMUX_MARIO_RECEIPT").map(std::path::PathBuf::from).map(|p|p.with_file_name("review-immediate-steered.sqlite")).unwrap_or_else(||root.path().join("snapshot.sqlite"));
    let db=app.state::<crate::database::DatabaseStore>().control_fixture_snapshot(&snapshot);
    let rows:Vec<serde_json::Value>=db.list_agent_chat_messages(thread).iter().map(|p|serde_json::from_str(p).unwrap()).collect();
    let user=rows.iter().find(|r|r["type"]=="user_message" && r["client_nonce"]=="immediate-nonce").unwrap();
    for (field,expected) in [("type",json!("user_message")),("thread_id",json!(thread)),("text",json!("immediate visible")),("client_nonce",json!("immediate-nonce")),("message_id",json!("immediate-nonce")),("created_by",json!("user")),("creation_source",json!("desktop"))] {
        assert_eq!(user[field],expected,"Envelope {field} must preserve its meaning");
    }
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
    use crate::agent_provider::{ProviderRuntimeEvent, SubagentSnapshot, SubagentStatus, ThreadId, TurnId, TurnStatus,
    };
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
    use crate::agent_provider::{AgentProvider, ProviderKind, ThreadId};
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
    let row = app.state::<crate::database::DatabaseStore>().message_delivery(send["delivery_id"].as_str().unwrap()).unwrap().unwrap();
    assert_eq!(row.state, "preparing");
    super::delivery::dispatch(app.handle(), row).await;
    assert_eq!(provider.calls.snapshot().iter().filter(|c| matches!(c, mock_agent_provider::MockCall::SendTurn(_, _))).count(), 0, "repair worker must not steal the original admission fence");
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
    use crate::agent_provider::{ProviderRuntimeEvent, RequestId, ThreadId, TurnId};
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
async fn orchestration_message_controller_retries_and_persists_caller_provenance() {
    let (app, provider, _root, workspace) = fixture().await;
    let launched = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"launch-send","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app,&launched).await;
    assert_eq!(launched["state"], "succeeded");
    let thread = launched["thread_id"].as_str().unwrap();
    app.state::<crate::database::DatabaseStore>().upsert_agent_chat_usage_resume(thread,"claude",Some(9999999999999)).unwrap();
    let args = json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"send-a","message":"Synthetic follow-up","delivery":"queue"});
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_send", args.clone()).await.unwrap();
    let delivered = settle_operation(&app,&receipt).await;
    assert_eq!(delivered["state"], "succeeded", "{delivered}");
    let retry = execute(app.handle(), &ControlCaller::trusted(), "thread_send", args.clone()).await.unwrap();
    assert_eq!(retry["operation_id"], receipt["operation_id"]);
    let transcript = execute(app.handle(), &ControlCaller::trusted(), "thread_read", json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    assert_eq!(transcript["messages"][0]["content"], "Synthetic follow-up");
    assert_eq!(provider.calls.snapshot().iter().filter(|call| matches!(call,mock_agent_provider::MockCall::SendTurn(_, _))).count(),1);
    let mut changed=args.clone(); changed["message"]=json!("different payload");
    assert_eq!(execute(app.handle(),&ControlCaller::trusted(),"thread_send",changed).await.unwrap_err().code,"request_key_conflict");
    let db=app.state::<crate::database::DatabaseStore>();
    let messages:Vec<serde_json::Value>=db.list_agent_chat_messages(thread).iter().map(|p|serde_json::from_str(p).unwrap()).filter(|p:&serde_json::Value|p["type"]=="user_message").collect();
    assert_eq!(messages.len(),1);
    assert_eq!(messages[0]["created_by"],"user");
    assert_eq!(messages[0]["creation_source"],"mcp");
    assert_eq!(messages[0]["input_intent"],"turn_start");
    assert!(messages[0]["message_id"].is_string());

    assert!(db.get_agent_chat_usage_resume(thread).is_some(),"Controller input must not reset human activity or resume budgets");

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

/// Listing reads caches only. With every catalog cold it reports each
/// built-in instance undiscovered and starts nothing: no OpenCode server, no
/// CLI health check, no catalog harvest. A cached catalog is listed as is.
#[tokio::test]
async fn all_provider_listing_with_cold_caches_starts_nothing() {
    use crate::agent_provider::ProviderKind::{self, *};
    let (app, _provider, _root, workspace) = fixture().await;
    app.state::<NativeControlState>().fixture_capabilities.lock().unwrap().clear();
    let registry = app.state::<crate::commands::agent_chat::ProviderRegistry>();
    let mock = |kind: ProviderKind| Arc::new(mock_agent_provider::MockAgentProvider::new(kind));
    registry.set_codex(mock(Codex)).await;
    registry.set_cursor(mock(Cursor)).await;
    registry.set_grok(mock(Grok)).await;
    registry.set_opencode(mock(OpenCode)).await;
    let opencode = Arc::new(crate::agent_provider::opencode::OpenCodeServerManager::new());
    app.manage(opencode.clone());
    app.manage(Arc::new(crate::agent_provider::claude::capabilities::ClaudeCapabilityCache::new()));
    app.manage(Arc::new(crate::agent_provider::codex::capabilities::CodexCapabilityCache::new()));
    app.manage(Arc::new(crate::agent_provider::cursor::capabilities::CursorCapabilityCache::new()));
    app.manage(Arc::new(crate::agent_provider::grok::capabilities::GrokCapabilityCache::new()));
    let trusted = ControlCaller::trusted();
    let list = || execute(app.handle(), &trusted, "agent_capabilities", json!({"workspace_id":workspace}));

    let cold = list().await.unwrap();
    let providers = cold["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 5, "{cold}");
    for entry in providers {
        assert_eq!(entry["discovery_error"]["code"], "catalog_not_discovered", "{entry}");
    }
    assert!(!opencode.is_running_for_tests().await, "listing started an OpenCode server");

    // An OpenCode 1 catalog the composer read earlier: listed, with the reason
    // its chats cannot collaborate, still without a server.
    let catalog = crate::agent_provider::opencode::capabilities::build_capabilities(vec![]);
    opencode.remember_catalog(crate::agent_provider::opencode::api::OpenCodeApi::Legacy, catalog);
    let warm = list().await.unwrap();
    let entry = warm["providers"].as_array().unwrap().iter().find(|p| p["provider"] == "opencode").unwrap();
    assert_eq!(entry["discovery_error"], serde_json::Value::Null, "{entry}");
    assert_eq!(entry["collaboration_unavailable"], crate::agent_provider::opencode::session::LEGACY_COLLABORATION);
    assert!(!opencode.is_running_for_tests().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_reaches_native_provider_without_deleting_thread_or_pane() {
    use crate::agent_provider::{ProviderRuntimeEvent, SessionStatus, ThreadId};
    let (app, provider, root, workspace) = fixture().await;
    let launched = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id":workspace,"client_request_id":"launch-interrupt","provider":"claude","permission_mode":"default",
    })).await.unwrap();
    let launched = settle_operation(&app,&launched).await;
    let thread = launched["thread_id"].as_str().unwrap();
    // Controller interrupt selects an existing run; an idle session has none.
    let idle = execute(app.handle(), &ControlCaller::trusted(), "thread_interrupt", json!({
        "workspace_id":workspace,"thread_id":thread,"client_request_id":"interrupt-idle",
    })).await.unwrap();
    assert_eq!(settle_operation(&app, &idle).await["result"]["reached_provider"], false);
    assert!(provider.calls.snapshot().iter().all(|call| !matches!(call, mock_agent_provider::MockCall::InterruptTurn(_, _))));
    let turn = crate::commands::agent_chat::send_desktop_turn(app.handle().clone(), crate::agent_provider::ProviderKind::Claude,
        serde_json::from_value(json!({"thread_id":thread,"text":"Interrupt this run"})).unwrap()).await.unwrap();
    crate::commands::agent_chat::forward_event(app.handle(), ProviderRuntimeEvent::SessionStateChanged {
        thread_id: ThreadId(thread.into()), status: SessionStatus::Running {active_turn: turn.turn_id.clone()},
    });
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
    assert!(provider.calls.snapshot().iter().any(|call| matches!(call,mock_agent_provider::MockCall::InterruptTurn(_, Some(id)) if id == &turn.turn_id)));
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


#[tokio::test]
async fn workspace_chat_launch_shares_native_start_without_selecting_workspace() {
    let (app, provider, root, parent) = fixture().await;
    std::fs::create_dir(root.path().join("worktree")).unwrap();
    let target = app.state::<crate::state::AppStateStore>().create_empty_workspace_at_path(root.path().join("worktree"));
    app.state::<crate::state::AppStateStore>().activate_workspace(&parent);
    let caller = ControlCaller { admitted: None, identity: super::CallerPrincipal::LocalController, created_by: crate::agent_provider::message::CreatedBy::Agent, principal: "thread:parent".into(), access: ControlAccess::FullAccess, grant_id: None };
    let prepared = service::PreparedWorkspaceChat::prepare(app.handle(), &caller,
        crate::agent_provider::ProviderKind::Claude, "Fix sidebar tests".into(), "Self-contained fixture brief".into(), None, None,
    ).await.unwrap();
    let launched = prepared.materialize(app.handle(), &caller, target.0.clone(), "prepared-workspace-child").await.unwrap();
    let thread = launched["thread_id"].as_str().unwrap();
    assert_eq!(app.state::<crate::state::AppStateStore>().snapshot().active_workspace_id.0, parent);
    let record = app.state::<crate::database::DatabaseStore>().get_agent_chat_session(thread).unwrap();
    assert_eq!(record.workspace_id, target.0);
    assert_eq!(record.title.as_deref(), Some("Fix sidebar tests"));
    assert_eq!(record.permission_mode.as_deref(), Some("bypassPermissions"));
    assert!(provider.calls.snapshot().iter().any(|call| matches!(call,
        mock_agent_provider::MockCall::SendTurn(id, text) if id.0 == thread && text == "Self-contained fixture brief")));
    assert_eq!(provider.start_inputs()[0].permission_mode.as_deref(), Some("bypassPermissions"));
}

#[tokio::test]
async fn workspace_chat_launch_validates_models_and_keeps_permission_ceiling() {
    let (app, provider, _root, _workspace) = fixture().await;
    let caller = ControlCaller::trusted();
    let invalid = service::PreparedWorkspaceChat::prepare(app.handle(), &caller,
        crate::agent_provider::ProviderKind::Claude, "Fixture".into(), "Brief".into(), Some("not-advertised".into()), None,
    ).await.err().unwrap();
    assert_eq!(invalid.code, "unsupported_model");
    let invalid = service::PreparedWorkspaceChat::prepare(app.handle(), &caller,
        crate::agent_provider::ProviderKind::Claude, "Fixture".into(), "Brief".into(), None, Some("high".into()),
    ).await.err().unwrap();
    assert_eq!(invalid.code, "unsupported_effort");
    let supervised = ControlCaller { access: ControlAccess::Supervised, ..caller };
    let denied = service::PreparedWorkspaceChat::prepare(app.handle(), &supervised,
        crate::agent_provider::ProviderKind::Claude, "Fixture".into(), "Brief".into(), None, None,
    ).await.err().unwrap();
    assert_eq!(denied.code, "unsupported_permission_ceiling");
    assert!(provider.calls.snapshot().is_empty());
}


#[tokio::test]
async fn workspace_chat_permission_changes_refresh_the_host_tool_catalog() {
    let (app, _provider, _root, workspace) = fixture().await;
    app.state::<crate::database::DatabaseStore>().upsert_agent_chat_session("parent", &workspace, None, "claude").unwrap();
    let mut updates = app.state::<crate::mcp::registry::McpRegistry>().subscribe_status();
    crate::commands::agent_chat::agent_chat_set_permission_mode(app.handle().clone(),
        crate::agent_provider::ProviderKind::Claude, crate::agent_provider::ThreadId("parent".into()), "default".into(),
    ).await.unwrap();
    assert_eq!(updates.try_recv().unwrap().id, "codemux-host");
}


#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_accept_then_lost_ack_does_not_dispatch_again() {
    let (app,provider,_root,workspace)=fixture().await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"fault-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&receipt).await;
    let thread=launched["thread_id"].as_str().unwrap();
    provider.fail_next_send_after_accept(crate::agent_provider::ProviderError::RpcError {message:"fixture: accepted bytes, ACK dropped".into()});
    let args=json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"fault-send","message":"Review the fixture"});
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_send",args.clone()).await.unwrap();
    let outcome=settle_operation(&app,&receipt).await;
    assert_eq!(outcome["state"],"needs_attention","{outcome}");
    assert_eq!(outcome["dispatch_disposition"],"held_unknown");
    let retry=execute(app.handle(),&ControlCaller::trusted(),"thread_send",args).await.unwrap();
    assert_eq!(retry["operation_id"],receipt["operation_id"]);
    let db=app.state::<crate::database::DatabaseStore>();
    let row=db.message_delivery(receipt["delivery_id"].as_str().unwrap()).unwrap().unwrap();
    super::delivery::dispatch(app.handle(),row).await;
    assert_eq!(provider.calls.snapshot().iter().filter(|call|matches!(call,mock_agent_provider::MockCall::SendTurn(_, _))).count(),1);
    let status=execute(app.handle(),&ControlCaller::trusted(),"thread_status",json!({"workspace_id":workspace,"thread_id":thread})).await.unwrap();
    assert_eq!(status["phase"],"needs_attention");
    assert!(!status["deliveries_needing_attention"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_pre_send_failure_retries_original_identity() {
    let (app,provider,_root,workspace)=fixture().await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"retry-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&receipt).await;
    let thread=launched["thread_id"].as_str().unwrap();
    provider.fail_next_send(crate::agent_provider::ProviderError::ValidationError {message:"fixture: definitely before bytes".into()});
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_send",json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"retry-send","message":"Review the fixture"})).await.unwrap();
    let id=receipt["delivery_id"].as_str().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let row=app.state::<crate::database::DatabaseStore>().message_delivery(id).unwrap().unwrap();
            if row.state=="pending" && row.error.is_some() { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let row=app.state::<crate::database::DatabaseStore>().message_delivery(id).unwrap().unwrap();
    let message_id=row.message_id.clone(); let attempt=row.attempt_id.clone();
    super::delivery::dispatch(app.handle(),row).await;
    let delivered=settle_operation(&app,&receipt).await;
    assert_eq!(delivered["state"],"succeeded","{delivered}");
    let row=app.state::<crate::database::DatabaseStore>().message_delivery(id).unwrap().unwrap();
    assert_eq!(row.message_id,message_id);assert_eq!(row.attempt_id,attempt);
    assert_eq!(provider.calls.snapshot().iter().filter(|call|matches!(call,mock_agent_provider::MockCall::SendTurn(_, _))).count(),1);
}

#[tokio::test]
async fn orchestration_message_queue_edit_promotion_preserves_live_and_persisted_provenance() {
    use crate::agent_provider::{message::*,ProviderRuntimeEvent,ThreadId,TurnId};
    let (app,_provider,_root,workspace)=fixture().await;
    let db=app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session("receiver",&workspace,None,"claude").unwrap();
    let events=mario_channel(app.handle(),"receiver");
    let provenance=MessageProvenance {message_id:Some("stable-queued".into()),created_by:CreatedBy::Agent,
        creation_source:CreationSource::Provider,sender_thread_id:Some("sender".into()),sender_title:Some("Review agent".into()),..Default::default()};
    crate::commands::agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::TurnQueued {
        provenance,thread_id:ThreadId("receiver".into()),queued_id:"queue".into(),client_nonce:Some("nonce".into()),text:"Review".into(),
    });
    crate::commands::agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::QueuedTurnEdited {
        thread_id:ThreadId("receiver".into()),queued_id:"queue".into(),text:"Edited review".into(),
    });
    assert_eq!(db.get_queued_turn("receiver","queue").unwrap().provenance.created_by,CreatedBy::Agent);
    crate::commands::agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::QueuedTurnDispatched {
        thread_id:ThreadId("receiver".into()),queued_id:"queue".into(),turn_id:TurnId("active".into()),text:"Edited review".into(),steered:true,
    });
    let rows:Vec<serde_json::Value>=db.list_agent_chat_messages("receiver").iter().map(|p|serde_json::from_str(p).unwrap()).filter(|p:&serde_json::Value|p["type"]=="user_message").collect();
    assert_eq!(rows.len(),1);let row=&rows[0];
    assert_eq!(row["message_id"],"stable-queued");assert_eq!(row["sender_title"],"Review agent");
    assert_eq!(row["created_by"],"agent");assert_eq!(row["input_intent"],"promoted_queued_to_steer");
    assert_eq!(row["text"],"Edited review");
    let live=events.lock().unwrap();let live=live.iter().find(|event|event["event"]["type"]=="user_message").unwrap();
    for field in ["message_id","created_by","creation_source","sender_thread_id","sender_title","input_intent","text"] {
        assert_eq!(live["event"][field],row[field],"Live and persisted {field} must agree");
    }
    assert!(db.list_queued_turns("receiver").is_empty());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_saved_resend_keeps_agent_identity_and_text_split() {
    use crate::agent_provider::message::*;
    let (app,provider,_root,workspace)=fixture().await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"saved-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&receipt).await;let thread=launched["thread_id"].as_str().unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_usage_resume(thread,"claude",Some(9999999999999)).unwrap();
    db.control_record_queue_admission(thread,"saved").unwrap();
    db.insert_queued_turn(&crate::database::queued_turns::QueuedTurnRecord {
        provenance:MessageProvenance {message_id:Some("saved-agent-message".into()),created_by:CreatedBy::Agent,creation_source:CreationSource::Provider,
            sender_thread_id:Some("sender".into()),sender_title:Some("Review agent".into()),origin:Some(crate::commands::agent_chat::TurnOrigin::Delegation),..Default::default()},
        thread_id:thread.into(),queued_id:"saved".into(),position:0,text:"Visible brief".into(),prompt:Some("Visible brief\nAgent-only footer".into()),
        skill_ids:vec![],skill_text:None,include_plugins:false,images:vec![],client_nonce:Some("original-nonce".into()),
        model_override:None,effort_override:None,permission_mode_override:None,epoch:"previous-run".into(),
    }).unwrap();
    crate::commands::queued_turns::agent_chat_send_saved_queued_turn(app.handle().clone(),crate::agent_provider::ProviderKind::Claude,crate::agent_provider::ThreadId(thread.into()),"saved".into(),crate::agent_provider::types::MessageDelivery::Queue).await.unwrap();
    let rows:Vec<serde_json::Value>=db.list_agent_chat_messages(thread).iter().map(|p|serde_json::from_str(p).unwrap()).filter(|p:&serde_json::Value|p["type"]=="user_message").collect();
    assert_eq!(rows.len(),1);assert_eq!(rows[0]["message_id"],"saved-agent-message");assert_eq!(rows[0]["created_by"],"agent");
    assert_eq!(rows[0]["input_intent"],"queued_turn");assert_eq!(rows[0]["text"],"Visible brief");assert_eq!(rows[0]["agent_text"],"Visible brief\nAgent-only footer");
    assert!(db.get_agent_chat_usage_resume(thread).is_some());assert!(db.list_queued_turns(thread).is_empty());
    assert!(db.control_unresolved_queued_ids(thread).unwrap().is_empty(), "legacy replay must retire its old native queue admission");
    assert!(provider.calls.snapshot().iter().any(|call|matches!(call,mock_agent_provider::MockCall::SendTurn(_,text) if text=="Visible brief\nAgent-only footer")));
}


#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_only_interactive_sources_reset_human_activity() {
    let (app,_provider,_root,workspace)=fixture().await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"source-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&receipt).await;let thread=launched["thread_id"].as_str().unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    for source in ["desktop","web","phone"] {
        db.upsert_agent_chat_usage_resume(thread,"claude",Some(9999999999999)).unwrap();
        let command=serde_json::from_value(json!({"thread_id":thread,"text":format!("Human {source}"),"client_nonce":format!("human-{source}"),"input_source":source})).unwrap();
        crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),crate::agent_provider::ProviderKind::Claude,command).await.unwrap();
        assert!(db.get_agent_chat_usage_resume(thread).is_none());
        let row=db.list_agent_chat_messages(thread).into_iter().filter_map(|p|serde_json::from_str::<serde_json::Value>(&p).ok()).find(|p|p["message_id"]==format!("human-{source}")).unwrap();
        assert_eq!(row["created_by"],"user");assert_eq!(row["creation_source"],"desktop","client input_source cannot override desktop IPC");
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_stop_queue_release_rearms_retained_checkpoint() {
    use crate::agent_provider::{ProviderKind, ThreadId, TurnDispatchCheckpoint};
    for send_now in [false,true] {
        let (app,_provider,_root,workspace)=fixture().await;
        let launched=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"release-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
        let launched=settle_operation(&app,&launched).await;
        let thread=launched["thread_id"].as_str().unwrap();
        let db=app.state::<crate::database::DatabaseStore>();
        db.admit_message_delivery("queued-b",thread,"b",&json!({"origin":"user"}),"epoch",0,None,true).unwrap();
        db.finish_message_delivery("queued-b","queued",Some(&json!({"queued_id":"b"})),None).unwrap();
        let checkpoint=super::delivery::DeliveryCheckpoint {app:app.handle().clone(),id:"queued-b".into(),thread:thread.into(),provider:ProviderKind::Claude,inner:None,release_queue:false,resumed_ids:vec![],admitted_generation:0};
        db.stop_message_deliveries(thread).unwrap();
        assert!(checkpoint.authorize_dispatch().is_err());
        if send_now {
            // The mock has no queue implementation; the command must still
            // re-arm before handing control to the provider's release path.
            let _=crate::commands::agent_chat::agent_chat_send_queued_turn_now(app.handle().clone(),ProviderKind::Claude,ThreadId(thread.into()),"b".into(),None).await;
        } else {
            crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),ProviderKind::Claude,serde_json::from_value(json!({"thread_id":thread,"text":"C","client_nonce":"c"})).unwrap()).await.unwrap();
        }
        checkpoint.authorize_dispatch().unwrap();
        db.finish_message_delivery("queued-b","accepted",Some(&json!({"turn_id":"b-run"})),None).unwrap();
        assert_eq!(db.message_delivery("queued-b").unwrap().unwrap().state,"accepted");
    }
}

#[tokio::test]
async fn orchestration_message_held_queue_and_unknown_delivery_do_not_prevent_wait_settling() {
    use crate::agent_provider::{ProviderRuntimeEvent,ThreadId,TurnId,TurnStatus};
    let (app,_provider,_root,workspace)=fixture().await;
    let thread="held-settled";
    let db=app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session(thread,&workspace,None,"claude").unwrap();
    db.admit_message_delivery("unknown",thread,"unknown",&json!({"origin":"controller"}),"epoch",0,None,true).unwrap();
    db.message_delivery_submitting("unknown").unwrap();db.fail_message_delivery("unknown","ACK lost").unwrap();
    db.admit_message_delivery("human",thread,"human",&json!({"origin":"user"}),"epoch",0,None,true).unwrap();
    db.finish_message_delivery("human","queued",Some(&json!({"queued_id":"held-b"})),None).unwrap();
    crate::commands::agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::TurnQueued {thread_id:ThreadId(thread.into()),queued_id:"held-b".into(),client_nonce:None,text:"B".into(),provenance:Default::default()});
    db.stop_message_deliveries(thread).unwrap();
    crate::commands::agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::TurnCompleted {thread_id:ThreadId(thread.into()),turn_id:TurnId("a".into()),status:TurnStatus::Error {subtype:"interrupted".into(),message:"Stopped".into()},usage:None});
    let result=execute(app.handle(),&ControlCaller::trusted(),"thread_wait",json!({"workspace_id":workspace,"thread_id":thread,"timeout_ms":1})).await.unwrap();
    assert_eq!(result["settled"],true,"{result}");
    assert_eq!(result["status"]["phase"],"needs_attention");
    assert_eq!(result["status"]["deliveries_needing_attention"].as_array().unwrap().len(),1);
}

#[tokio::test]
async fn orchestration_message_definite_error_survives_attention_and_cold_status() {
    use crate::agent_provider::{ProviderRuntimeEvent,ThreadId,TurnId,TurnStatus};
    let (app,_provider,_root,workspace)=fixture().await;
    let thread="error-with-held-delivery";
    let db=app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session(thread,&workspace,None,"claude").unwrap();
    db.admit_message_delivery("held",thread,"held",&json!({"origin":"controller"}),"epoch",0,None,true).unwrap();
    db.refuse_message_delivery("held","permission_ceiling: refused before send").unwrap();
    crate::commands::agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::TurnCompleted {
        thread_id:ThreadId(thread.into()),turn_id:TurnId("failed-turn".into()),status:TurnStatus::Error {subtype:"provider_error".into(),message:"Definite failure".into()},usage:None,
    });
    for cold in [false,true] {
        if cold { app.unmanage::<NativeControlState>().unwrap(); app.manage(NativeControlState::default()); }
        let result=execute(app.handle(),&ControlCaller::trusted(),"thread_wait",json!({"workspace_id":workspace,"thread_id":thread,"timeout_ms":1})).await.unwrap();
        assert_eq!(result["status"]["phase"],"error","{result}");
        assert_eq!(result["settled"],true,"{result}");
        assert_eq!(result["status"]["deliveries_needing_attention"].as_array().unwrap().len(),1);
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_controller_interrupt_selects_current_run_without_stop_fence() {
    use crate::agent_provider::{ProviderRuntimeEvent, SessionStatus, ThreadId, TurnId};
    let (app,provider,_root,workspace)=fixture().await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"interrupt-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&receipt).await; let thread=launched["thread_id"].as_str().unwrap();
    crate::commands::agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::SessionStateChanged {thread_id:ThreadId(thread.into()),status:SessionStatus::Running {active_turn:TurnId("selected".into())}});
    let db=app.state::<crate::database::DatabaseStore>();
    db.admit_message_delivery("pending",thread,"pending",&json!({"origin":"controller"}),"epoch",0,None,false).unwrap();
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_interrupt",json!({"workspace_id":workspace,"thread_id":thread,"client_request_id":"interrupt"})).await.unwrap();
    assert_eq!(settle_operation(&app,&receipt).await["state"],"succeeded");
    assert_eq!(db.delivery_generation(thread).unwrap().0,0);
    assert_eq!(db.message_delivery("pending").unwrap().unwrap().state,"pending");
    assert!(provider.calls.snapshot().iter().any(|call|matches!(call,mock_agent_provider::MockCall::InterruptTurn(_,Some(turn)) if turn.0=="selected")));
    db.stop_message_deliveries(thread).unwrap();
    let input=serde_json::from_value(json!({"thread_id":thread,"text":"Resume","client_nonce":"new-resume"})).unwrap();
    crate::commands::agent_chat::send_turn_with_origin(app.handle().clone(),crate::agent_provider::ProviderKind::Claude,input,crate::commands::agent_chat::TurnOrigin::UsageResume).await.unwrap();
}

#[tokio::test]
async fn orchestration_message_controller_checks_authority_and_binding_before_admission() {
    let (app,_provider,_root,workspace)=fixture().await;
    let db=app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session("unbound",&workspace,None,"claude").unwrap();
    let args=json!({"workspace_id":workspace,"thread_id":"unbound","client_request_id":"must-not-admit","message":"brief"});
    let error=execute(app.handle(),&ControlCaller::trusted(),"thread_send",args.clone()).await.unwrap_err();
    assert_eq!(error.code,"thread_not_bound");
    // Calling the service itself exercises its own pre-admission guard.
    app.manage(crate::mcp_connector::McpConnectorState::default());
    app.manage(crate::web_remote::WebRemoteState::default());
    let caller=ControlCaller::outside("revoked-client".into(),"missing-grant".into(),ControlAccess::FullAccess);
    assert!(super::service::thread_send(app.handle(),&caller,args).await.is_err());
    assert!(db.pending_message_deliveries().unwrap().is_empty());
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_idle_nonsteering_provider_starts_but_busy_steer_is_rejected() {
    use crate::agent_provider::{AgentProvider,ProviderKind};
    let (app,claude,_root,workspace)=fixture().await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"idle-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&receipt).await; let thread=launched["thread_id"].as_str().unwrap();
    let provider=Arc::new(mock_agent_provider::MockAgentProvider::new(ProviderKind::Cursor));
    provider.start_session(claude.start_inputs()[0].clone()).await.unwrap();
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_cursor(provider.clone()).await;
    app.state::<crate::database::DatabaseStore>().upsert_agent_chat_session(thread,&workspace,None,"cursor").unwrap();
    for busy in [false,true] {
        provider.set_turn_active(thread,busy);
        let result=crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),ProviderKind::Cursor,serde_json::from_value(json!({"thread_id":thread,"text":"brief","delivery":"steer","client_nonce":format!("steer-{busy}")})).unwrap()).await;
        assert_eq!(result.is_ok(),!busy,"{result:?}");
    }
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_remote_result_retry_uses_first_admitted_payload() {
    let (app,provider,_root,workspace)=fixture().await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"remote-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&receipt).await; let thread=launched["thread_id"].as_str().unwrap();
    provider.fail_next_send(crate::agent_provider::ProviderError::ValidationError {message:"before bytes".into()});
    // Exercise canonical retry rendering at intake. The actual remote journal
    // guard is separately covered by delegation integration tests.
    let make=|text:&str|serde_json::from_value(json!({"thread_id":thread,"text":text,"client_nonce":"delegation:remote-id"})).unwrap();
    let mut context=super::delivery::SendContext::new(crate::commands::agent_chat::TurnOrigin::RemoteDelegation);
    // A missing remote journal task rejects before send but after admission.
    let _=crate::commands::agent_chat::send_turn_with_context(app.handle().clone(),crate::agent_provider::ProviderKind::Claude,make("first rendering"),context.clone()).await;
    context.provenance.sender_title=Some("later mutable title".into());
    let error=crate::commands::agent_chat::send_turn_with_context(app.handle().clone(),crate::agent_provider::ProviderKind::Claude,make("changed rendering"),context).await.unwrap_err();
    assert!(!error.contains("message_identity_conflict"),"{error}");
    let row=app.state::<crate::database::DatabaseStore>().message_delivery_for_message(thread,"delegation:remote-id").unwrap().unwrap();
    assert_eq!(row.payload["input"]["text"],"first rendering");
}

#[cfg(unix)]
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_real_acp_configuration_failure_is_definitely_before_send() {
    use crate::agent_provider::{AgentProvider,ProviderKind,StartSessionInput,ThreadId};
    use std::os::unix::fs::PermissionsExt;
    let peer_root=tempfile::tempdir().unwrap(); let binary=peer_root.path().join("peer.py");
    std::fs::write(&binary,include_str!("../../tests/helpers/agent_control_acp_peer.py")).unwrap();
    std::fs::set_permissions(&binary,std::fs::Permissions::from_mode(0o700)).unwrap();
    let rig=peer_root.path().join("rig");std::fs::create_dir(&rig).unwrap();
    let provider=Arc::new(crate::agent_provider::cursor::CursorAgentProvider::new(crate::agent_provider::cursor::CursorProviderConfig {binary,event_channel_capacity:128,mcp_registry:None}));
    let thread="pre-config-failure";
    let (app,root,_workspace,pane)=super::guard_tests::fixture(provider.clone(),thread).await;
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_cursor(provider.clone()).await;
    crate::commands::agent_chat::agent_chat_start_session(app.handle().clone(),pane,ProviderKind::Cursor,StartSessionInput {
        thread_id:ThreadId(thread.into()),cwd:root.path().canonicalize().unwrap(),model:None,resume_cursor:None,fresh_session:true,permission_mode:Some("ask".into()),effort:None,context_window:None,fast_mode:false,service_tier:None,additional_directories:vec![],env:Some(std::collections::HashMap::from([("CODEMUX_AGENT_CONTROL_FIXTURE_ROOT".into(),rig.to_string_lossy().into_owned())])),workspace_id:None,remote:None,extra:serde_json::Value::Null,recorded_usage_baseline:None,
    },Some(thread.into())).await.unwrap();
    let result=crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),ProviderKind::Cursor,serde_json::from_value(json!({"thread_id":thread,"text":"must not dispatch","model_override":"review-fatal","client_nonce":"config-failure"})).unwrap()).await;
    assert!(result.is_err(),"{result:?}");
    let db=app.state::<crate::database::DatabaseStore>();
    let row=db.message_delivery_for_message(thread,"config-failure").unwrap().unwrap();
    assert!(!row.attempted,"configuration failure must precede dispatch authorization");
    assert_eq!(row.state,"rejected");assert!(db.thread_message_delivery_attention(thread).unwrap().is_empty());
    let wire=std::fs::read_to_string(rig.join("peer-wire.jsonl")).unwrap();
    assert!(wire.contains("session/set_config_option"));assert!(!wire.contains("session/prompt"));
    provider.stop_session(ThreadId(thread.into())).await.unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_unbound_controller_delivery_retries_when_pane_is_rebound() {
    use crate::agent_provider::ProviderKind;
    let (app,provider,root,workspace)=fixture().await;
    let launch=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"rebind-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&launch).await; let thread=launched["thread_id"].as_str().unwrap();
    let state=app.state::<crate::state::AppStateStore>();
    let pane=state.agent_chat_pane_id_for_thread(thread).unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    let context=super::delivery::SendContext::controller(&ControlCaller::trusted());
    let payload=serde_json::to_value(super::delivery::DeliveryPayload {provider:ProviderKind::Claude,input:serde_json::from_value(json!({"thread_id":thread,"text":"retry after reopen","client_nonce":"rebind-message"})).unwrap(),origin:context.origin,provenance:context.provenance,caller:Some(ControlCaller::trusted()),pr_watch:None}).unwrap();
    db.admit_message_delivery("rebind",thread,"rebind-message",&payload,"epoch",0,None,false).unwrap();
    state.close_pane(&pane).unwrap();
    super::delivery::dispatch(app.handle(),db.message_delivery("rebind").unwrap().unwrap()).await;
    let pending=db.message_delivery("rebind").unwrap().unwrap();
    assert_eq!(pending.state,"pending"); assert!(!pending.attempted);
    state.create_agent_chat_pane(&workspace,Some(ProviderKind::Claude),Some(root.path().to_string_lossy().into_owned()),None,Some(thread.into())).unwrap();
    super::delivery::dispatch(app.handle(),pending).await;
    assert_eq!(db.message_delivery("rebind").unwrap().unwrap().state,"accepted");
    assert_eq!(provider.calls.snapshot().iter().filter(|call|matches!(call,mock_agent_provider::MockCall::SendTurn(_, _))).count(),1);
}

#[tokio::test]
async fn orchestration_message_mock_adapter_aborts_checkpoint_on_rejection_and_lost_ack() {
    use crate::agent_provider::{AgentProvider,ProviderError,ProviderKind,SendTurnInput,TurnDispatchCheckpoint};
    #[derive(Debug)]
    struct Checkpoint { reject:bool, aborts:std::sync::atomic::AtomicUsize }
    #[async_trait::async_trait]
    impl TurnDispatchCheckpoint for Checkpoint {
    fn native_stop_generation(&self) -> Result<u64, crate::agent_provider::ProviderError> { Ok(0) }

        fn authorize_dispatch(&self)->Result<(),ProviderError> {
            if self.reject { Err(ProviderError::ValidationError {message:"authorization rejected".into()}) } else { Ok(()) }
        }
        async fn prepare(&self) {}
        async fn commit(&self) {}
        async fn abort(&self) {self.aborts.fetch_add(1,std::sync::atomic::Ordering::SeqCst);}
    }
    for reject in [false,true] {
        let provider=mock_agent_provider::MockAgentProvider::new(ProviderKind::Claude);
        let checkpoint=Arc::new(Checkpoint {reject,aborts:Default::default()});
        let mut input:SendTurnInput=serde_json::from_value(json!({"thread_id":"thread","text":"brief","model_override":null})).unwrap();
        input.turn_checkpoint=Some(checkpoint.clone());
        if !reject {provider.fail_next_send_after_accept(ProviderError::RpcError {message:"ACK lost".into()});}
        assert!(provider.send_turn(input).await.is_err());
        assert_eq!(checkpoint.aborts.load(std::sync::atomic::Ordering::SeqCst),1);
    }
}


#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_wait_includes_admitted_work_and_retry_backoff() {
    let (app,_provider,_root,workspace)=fixture().await;
    let launched=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"wait-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&launched).await;let thread=launched["thread_id"].as_str().unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    db.admit_message_delivery("admitted",thread,"admitted",&json!({"origin":"controller"}),"epoch",0,None,false).unwrap();
    for state in ["pending","preparing","pending","submitting"] {
        if state=="preparing" { assert!(db.claim_message_delivery("admitted").unwrap()); }
        if state=="pending" && db.message_delivery("admitted").unwrap().unwrap().state=="preparing" { db.fail_message_delivery("admitted","transient pre-send failure").unwrap(); }
        if state=="submitting" { db.claim_message_delivery("admitted").unwrap();db.message_delivery_submitting("admitted").unwrap(); }
        let waited=execute(app.handle(),&ControlCaller::trusted(),"thread_wait",json!({"workspace_id":workspace,"thread_id":thread,"timeout_ms":1})).await.unwrap();
        assert_eq!(waited["settled"],false,"{state}: {waited}");assert_eq!(waited["timed_out"],true);
    }
    db.finish_message_delivery("admitted","accepted",None,None).unwrap();
    let waited=execute(app.handle(),&ControlCaller::trusted(),"thread_wait",json!({"workspace_id":workspace,"thread_id":thread,"timeout_ms":1})).await.unwrap();
    assert_eq!(waited["settled"],true);
}

#[tokio::test]
async fn orchestration_message_disposed_wake_emits_no_warning_or_draft_restoration() {
    use crate::agent_provider::{ProviderRuntimeEvent,ThreadId};
    let (app,_provider,_root,workspace)=fixture().await;
    let db=app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session("receiver",&workspace,None,"claude").unwrap();
    let events=mario_channel(app.handle(),"receiver");
    db.admit_message_delivery("wake","receiver","wake",&json!({"origin":"pr_watch"}),"epoch",0,None,true).unwrap();
    db.finish_message_delivery("wake","queued",Some(&json!({"queued_id":"wake-queue"})),None).unwrap();
    db.stop_message_deliveries("receiver").unwrap();
    crate::commands::agent_chat::forward_event(app.handle(),ProviderRuntimeEvent::QueuedTurnCancelled {thread_id:ThreadId("receiver".into()),queued_id:"wake-queue".into()});
    let events=events.lock().unwrap();
    assert!(events.iter().any(|e|e["event"]["type"]=="queued_turn_disposed"));
    assert!(!events.iter().any(|e|matches!(e["event"]["type"].as_str(),Some("runtime_warning"|"queued_turn_cancelled"))));
}

// The peer is a Python script made executable with Unix permissions.
#[cfg(unix)]
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_real_acp_machine_send_cannot_release_stopped_explicit_queue() {
    use crate::agent_provider::{AgentProvider,ProviderKind,StartSessionInput,ThreadId};
    use std::os::unix::fs::PermissionsExt;
    let peer=tempfile::tempdir().unwrap(); let binary=peer.path().join("peer.py");
    std::fs::write(&binary,include_str!("../../tests/helpers/agent_control_acp_peer.py")).unwrap();
    std::fs::set_permissions(&binary,std::fs::Permissions::from_mode(0o700)).unwrap();
    let rig=peer.path().join("rig");std::fs::create_dir(&rig).unwrap();
    let provider=Arc::new(crate::agent_provider::cursor::CursorAgentProvider::new(crate::agent_provider::cursor::CursorProviderConfig {binary,event_channel_capacity:128,mcp_registry:None}));
    let thread="acp-held-explicit";
    let (app,root,_workspace,pane)=super::guard_tests::fixture(provider.clone(),thread).await;
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_cursor(provider.clone()).await;
    crate::commands::agent_chat::agent_chat_start_session(app.handle().clone(),pane,ProviderKind::Cursor,StartSessionInput {
        thread_id:ThreadId(thread.into()),cwd:root.path().canonicalize().unwrap(),model:None,resume_cursor:None,fresh_session:true,permission_mode:Some("ask".into()),effort:None,context_window:None,fast_mode:false,service_tier:None,additional_directories:vec![],env:Some(std::collections::HashMap::from([("CODEMUX_AGENT_CONTROL_FIXTURE_ROOT".into(),rig.to_string_lossy().into_owned())])),workspace_id:None,remote:None,extra:serde_json::Value::Null,recorded_usage_baseline:None,
    },Some(thread.into())).await.unwrap();
    let send=|text:&str|serde_json::from_value(json!({"thread_id":thread,"text":text,"client_nonce":text,"delivery":"queue"})).unwrap();
    crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),ProviderKind::Cursor,send("review-hold")).await.unwrap();
    let b=crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),ProviderKind::Cursor,send("held-B")).await.unwrap();
    assert!(b.queued_id.is_some());
    crate::commands::agent_chat::agent_chat_interrupt_turn(app.handle().clone(),ProviderKind::Cursor,ThreadId(thread.into()),None).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3),async {
        while provider.turn_active(&ThreadId(thread.into())).await { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
    }).await.unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    assert_eq!(db.message_delivery_for_message(thread,"held-B").unwrap().unwrap().state,"held");
    crate::commands::agent_chat::send_turn_with_context(app.handle().clone(),ProviderKind::Cursor,send("machine-C"),super::delivery::SendContext::controller(&ControlCaller::trusted())).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3),async {
        while provider.turn_active(&ThreadId(thread.into())).await { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
    }).await.unwrap();
    let b=db.message_delivery_for_message(thread,"held-B").unwrap().unwrap();
    assert_eq!(b.state,"held");assert!(!b.attempted);
    // A fresh human action re-arms B, which is still in the adapter queue.
    crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),ProviderKind::Cursor,send("human-D")).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3),async {
        while !db.message_delivery_for_message(thread,"held-B").unwrap().unwrap().attempted { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
    }).await.unwrap();
    provider.stop_session(ThreadId(thread.into())).await.unwrap();
}


#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_controller_queue_release_refreshes_only_stop_fence() {
    use crate::agent_provider::{ProviderKind,TurnDispatchCheckpoint};
    let (app,_provider,_root,workspace)=fixture().await;
    let launched=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"retained-guard-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&launched).await;let thread=launched["thread_id"].as_str().unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    db.admit_message_delivery("retained-B",thread,"retained-B",&json!({"origin":"controller"}),"epoch",0,None,true).unwrap();
    db.finish_message_delivery("retained-B","queued",Some(&json!({"queued_id":"B"})),None).unwrap();
    let control=NativeControlGuard {caller:ControlCaller::trusted(),workspace_id:workspace.clone(),thread_id:thread.into(),provider:ProviderKind::Claude,admission:None}.retain_admission(app.handle());
    let checkpoint=super::delivery::DeliveryCheckpoint {
        app:app.handle().clone(),id:"retained-B".into(),thread:thread.into(),provider:ProviderKind::Claude,
        release_queue:false,resumed_ids:vec![],admitted_generation:0,inner:Some(Arc::new(super::GuardedTurnCheckpoint {app:app.handle().clone(),control,inner:None})),
    };
    let gate=app.state::<NativeControlState>().thread_gate(thread);
    gate.cancel();db.stop_message_deliveries(thread).unwrap();
    assert!(checkpoint.authorize_dispatch().is_err());
    crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),ProviderKind::Claude,serde_json::from_value(json!({"thread_id":thread,"text":"human C","client_nonce":"release-C"})).unwrap()).await.unwrap();
    checkpoint.authorize_dispatch().unwrap();
    // A later provider restart/rebind is not a human release of another Stop.
    gate.cancel();
    assert!(checkpoint.authorize_dispatch().is_err());
}

#[tokio::test]
async fn orchestration_message_saved_replay_rechecks_revoked_caller_before_rearm() {
    use crate::agent_provider::{message::*,types::MessageDelivery,ProviderKind,ThreadId};
    for release in [false,true] {
        let (app,provider,_root,workspace)=fixture().await;
        app.manage(crate::mcp_connector::McpConnectorState::default());
        app.manage(crate::web_remote::WebRemoteState::default());
        let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"saved-revoked-launch","provider":"claude","permission_mode":"default"})).await.unwrap();
        let launched=settle_operation(&app,&receipt).await;let thread=launched["thread_id"].as_str().unwrap();
        let db=app.state::<crate::database::DatabaseStore>();
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port=listener.local_addr().unwrap().port();drop(listener);
        crate::web_remote::web_remote_set_config(app.handle().clone(),Some(port),None,Some("loopback".into()),Some(false),Some(false),Some(false),Some(true)).await.unwrap();
        crate::web_remote::web_remote_enable(app.handle().clone()).await.unwrap();
        crate::mcp_connector::agent_connector_set_config(app.handle().clone(),true,None).unwrap();
        let now=chrono::Utc::now().timestamp_millis();
        db.mcp_register_client(crate::database::mcp_connector::NewMcpClient {id:"revoked-client".into(),name:"Synthetic replay client".into(),redirects:vec!["https://client.example/cb".into()],created_at:now},now).unwrap();
        db.mcp_mint_grant(crate::database::mcp_connector::NewMcpGrant {id:"revoked-grant".into(),client_id:"revoked-client".into(),token_hash:"synthetic-hash".into(),redirect_uri:"https://client.example/cb".into(),resource:format!("http://127.0.0.1:{port}/mcp"),access:"full_access".into(),created_at:now,expires_at:now+60_000}).unwrap();
        let caller=ControlCaller::outside("revoked-client".into(),"revoked-grant".into(),ControlAccess::FullAccess);
        crate::mcp_connector::validate_caller(app.handle(),&caller).unwrap();
        let provenance=MessageProvenance {message_id:Some("B".into()),delivery_id:Some("B".into()),origin:Some(crate::commands::agent_chat::TurnOrigin::Controller),created_by:CreatedBy::User,creation_source:CreationSource::Mcp,..Default::default()};
        db.admit_message_delivery("B",thread,"B",&json!({"provider":"claude","origin":"controller","input":{"thread_id":thread,"text":"must not send"},"provenance":provenance,"caller":caller}),"old",0,None,true).unwrap();
        db.finish_message_delivery("B","queued",Some(&json!({"queued_id":"saved-B"})),None).unwrap();
        db.control_record_queue_admission(thread,"saved-B").unwrap();
        db.insert_queued_turn(&crate::database::queued_turns::QueuedTurnRecord {thread_id:thread.into(),queued_id:"saved-B".into(),position:0,text:"must not send".into(),prompt:None,provenance,skill_ids:vec![],skill_text:None,include_plugins:false,images:vec![],client_nonce:Some("B".into()),model_override:None,effort_override:None,permission_mode_override:None,epoch:"old".into()}).unwrap();
        crate::mcp_connector::agent_connector_revoke(app.handle().clone(),"revoked-client".into()).unwrap();
        db.recover_message_deliveries("new").unwrap();
        let calls=provider.calls.snapshot().len();
        let result=crate::commands::queued_turns::send_saved_queued_turn(app.handle().clone(),ProviderKind::Claude,ThreadId(thread.into()),"saved-B".into(),MessageDelivery::Queue,release).await;
        assert!(result.is_err(),"revoked caller replay was allowed: {result:?}");
        assert_eq!(provider.calls.snapshot().len(),calls);
        assert!(db.get_queued_turn(thread,"saved-B").is_some());
        let row=db.message_delivery("B").unwrap().unwrap();
        assert_eq!(row.state,"held");assert!(!row.attempted);
        assert!(row.error.unwrap().contains("grant_revoked"));
        assert_eq!(db.thread_message_delivery_attention(thread).unwrap().len(),1);
        if !release {
            let mut next=db.get_queued_turn(thread,"saved-B").unwrap();
            next.queued_id="saved-D".into();next.position=1;next.text="allowed saved D".into();
            next.client_nonce=Some("saved-D".into());
            next.provenance=MessageProvenance {message_id:Some("saved-D".into()),..Default::default()};
            db.insert_queued_turn(&next).unwrap();
            crate::commands::restart_resume::test_drain_saved_queue(app.handle(),&ThreadId(thread.into())).await;
            assert!(db.get_queued_turn(thread,"saved-B").is_some());
            assert!(db.get_queued_turn(thread,"saved-D").is_none(),"a refused saved item must not block later replay");
            assert!(provider.calls.snapshot().iter().any(|call|matches!(call,mock_agent_provider::MockCall::SendTurn(_,text) if text=="allowed saved D")));
        }
        crate::web_remote::web_remote_disable(app.handle().clone()).unwrap();
    }
}

#[tokio::test]
async fn orchestration_message_stop_with_disposed_queue_and_inactive_session_settles() {
    use crate::agent_provider::{ProviderKind,ThreadId};
    let (app,_provider,_root,_workspace)=fixture().await;
    let cursor=Arc::new(crate::agent_provider::cursor::CursorAgentProvider::new(crate::agent_provider::cursor::CursorProviderConfig {binary:"unused-synthetic-peer".into(),event_channel_capacity:16,mcp_registry:None}));
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_cursor(cursor).await;
    let db=app.state::<crate::database::DatabaseStore>();
    // A late provider ACK can outlive a deleted/inactive persisted session.
    db.admit_message_delivery("wake","inactive","wake",&json!({"origin":"pr_watch"}),"old",0,None,true).unwrap();
    db.stop_message_deliveries("inactive").unwrap();
    db.finish_message_delivery("wake","queued",Some(&json!({"queued_id":"late-wake"})),None).unwrap();
    assert_eq!(db.disposed_delivery_queue_ids("inactive").unwrap(),vec!["late-wake"]);
    let reached=crate::commands::agent_chat::agent_chat_interrupt_turn(app.handle().clone(),ProviderKind::Cursor,ThreadId("inactive".into()),None).await.unwrap();
    assert!(!reached);
    assert!(!app.state::<crate::commands::agent_chat::SubagentTracker>().delegated_work_holding_turn("inactive"));
}

#[tokio::test]
async fn orchestration_message_failed_human_send_restores_durable_stop_hold() {
    use crate::agent_provider::ProviderKind;
    let (app,provider,_root,workspace)=fixture().await;
    let launched=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"failed-release","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&launched).await;let thread=launched["thread_id"].as_str().unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    db.admit_message_delivery("B",thread,"B",&json!({"origin":"controller"}),"epoch",0,None,true).unwrap();
    db.finish_message_delivery("B","queued",Some(&json!({"queued_id":"B-queue"})),None).unwrap();
    db.stop_message_deliveries(thread).unwrap();
    provider.fail_next_send(crate::agent_provider::ProviderError::ValidationError {message:"provider rejected human D".into()});
    let result=crate::commands::agent_chat::send_desktop_turn(app.handle().clone(),ProviderKind::Claude,serde_json::from_value(json!({"thread_id":thread,"text":"D","client_nonce":"D"})).unwrap()).await;
    assert!(result.is_err());
    let row=db.message_delivery("B").unwrap().unwrap();
    assert_eq!(row.state,"held");assert_eq!(row.hold_reason.as_deref(),Some("stop"));assert!(!row.attempted);
    db.recover_message_deliveries("next-epoch").unwrap();
    assert!(db.rearm_saved_message_delivery("B",thread,"B-queue",false).is_err());
}

#[tokio::test]
async fn orchestration_message_lost_ack_warns_only_on_held_unknown_transition() {
    use crate::agent_provider::ProviderKind;
    let (app,provider,_root,workspace)=fixture().await;
    let launched=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"warn-once","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&launched).await;let thread=launched["thread_id"].as_str().unwrap();
    let events=mario_channel(app.handle(),thread);
    provider.fail_next_send_after_accept(crate::agent_provider::ProviderError::RpcError {message:"ACK lost".into()});
    let result=crate::commands::agent_chat::send_turn_with_context(app.handle().clone(),ProviderKind::Claude,serde_json::from_value(json!({"thread_id":thread,"text":"controller B","client_nonce":"B"})).unwrap(),super::delivery::SendContext::controller(&ControlCaller::trusted())).await;
    assert!(result.is_err());
    assert_eq!(events.lock().unwrap().iter().filter(|e|e["event"]["type"]=="runtime_warning" && e["event"]["message"].as_str().is_some_and(|s|s.contains("Message delivery needs attention"))).count(),1);
}

#[tokio::test]
async fn orchestration_message_restart_drain_stops_after_thread_wide_saved_failure() {
    use crate::agent_provider::{message::*,ThreadId};
    let (app,provider,_root,workspace)=fixture().await;
    let launched=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"saved-wide-failure","provider":"claude","permission_mode":"default"})).await.unwrap();
    let launched=settle_operation(&app,&launched).await;let thread=launched["thread_id"].as_str().unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    let provenance=MessageProvenance {message_id:Some("B".into()),delivery_id:Some("B".into()),origin:Some(crate::commands::agent_chat::TurnOrigin::Controller),..Default::default()};
    db.admit_message_delivery("B",thread,"B",&json!({"provider":"claude","origin":"controller","input":{"thread_id":thread,"text":"B"},"provenance":provenance,"caller":null}),"old",0,None,true).unwrap();
    db.finish_message_delivery("B","queued",Some(&json!({"queued_id":"saved-B"})),None).unwrap();
    for (pos,id,meta) in [(0,"saved-B",provenance),(1,"saved-D",MessageProvenance::default())] {
        db.insert_queued_turn(&crate::database::queued_turns::QueuedTurnRecord {thread_id:thread.into(),queued_id:id.into(),position:pos,text:id.into(),prompt:None,provenance:meta,skill_ids:vec![],skill_text:None,include_plugins:false,images:vec![],client_nonce:Some(id.into()),model_override:None,effort_override:None,permission_mode_override:None,epoch:"old".into()}).unwrap();
    }
    provider.fail_next_send(crate::agent_provider::ProviderError::ValidationError {message:"Thread is unbound".into()});
    crate::commands::restart_resume::test_drain_saved_queue(app.handle(),&ThreadId(thread.into())).await;
    assert_eq!(db.message_delivery("B").unwrap().unwrap().hold_reason.as_deref(),Some("recovery"));
    assert!(db.get_queued_turn(thread,"saved-D").is_some());
    assert!(!provider.calls.snapshot().iter().any(|call|matches!(call,mock_agent_provider::MockCall::SendTurn(_,text) if text=="saved-D")));
}

#[tokio::test]
async fn orchestration_message_queue_promotion_checks_authority_before_recording_submission() {
    use crate::agent_provider::{ProviderKind,TurnDispatchCheckpoint};
    #[derive(Debug)]
    struct Authority(std::sync::atomic::AtomicBool);
    #[async_trait::async_trait]
    impl TurnDispatchCheckpoint for Authority {
    fn native_stop_generation(&self) -> Result<u64, crate::agent_provider::ProviderError> { Ok(0) }

        fn authorize_dispatch(&self)->Result<(),crate::agent_provider::ProviderError> {
            if self.0.load(std::sync::atomic::Ordering::SeqCst) { Err(crate::agent_provider::ProviderError::ValidationError {message:"grant_revoked".into()}) } else { Ok(()) }
        }
        async fn prepare(&self) {}
        async fn commit(&self) {}
        async fn abort(&self) {}
    }
    let (app,_provider,_root,workspace)=fixture().await;
    let db=app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session("receiver",&workspace,None,"claude").unwrap();
    app.state::<crate::state::AppStateStore>().create_agent_chat_pane(&workspace,Some(ProviderKind::Claude),None,None,Some("receiver".into())).unwrap();
    db.admit_message_delivery("B","receiver","B",&json!({"origin":"controller"}),"epoch",0,None,true).unwrap();
    db.finish_message_delivery("B","queued",Some(&json!({"queued_id":"B-queue"})),None).unwrap();
    let authority=Arc::new(Authority(std::sync::atomic::AtomicBool::new(true)));
    let checkpoint=super::delivery::DeliveryCheckpoint {app:app.handle().clone(),id:"B".into(),thread:"receiver".into(),provider:ProviderKind::Claude,inner:Some(authority.clone()),release_queue:false,admitted_generation:0,resumed_ids:vec![]};
    assert!(checkpoint.authorize_dispatch().is_err());
    for revoked in [true,false] {
        db.rearm_promoted_message_delivery("receiver","B-queue").unwrap();
        authority.0.store(revoked,std::sync::atomic::Ordering::SeqCst);
        let result=checkpoint.authorize_queue_promotion();
        assert_eq!(result.is_err(),revoked);
        let row=db.message_delivery("B").unwrap().unwrap();
        assert!(!row.attempted,"promotion preflight must never claim provider acceptance");
        assert_eq!(row.state,if revoked {"held"} else {"queued"});
    }
    checkpoint.authorize_dispatch().unwrap();
    assert!(db.message_delivery("B").unwrap().unwrap().attempted);
}

async fn native_caller_fixture(mode:&str)->(tauri::App<tauri::test::MockRuntime>, Arc<mock_agent_provider::MockAgentProvider>, tempfile::TempDir, String, String, Arc<crate::delegation::authority::NativeAuthority>, crate::delegation::app::NativeActor) {
    native_caller_provider_fixture(crate::agent_provider::ProviderKind::Claude, mode).await
}

async fn native_caller_provider_fixture(kind:crate::agent_provider::ProviderKind, mode:&str)->(tauri::App<tauri::test::MockRuntime>, Arc<mock_agent_provider::MockAgentProvider>, tempfile::TempDir, String, String, Arc<crate::delegation::authority::NativeAuthority>, crate::delegation::app::NativeActor) {
    let (app, mut provider, root, workspace)=fixture().await;
    app.manage(crate::commands::delegation::DelegationState::default());
    if kind == crate::agent_provider::ProviderKind::Codex {
        provider = Arc::new(mock_agent_provider::MockAgentProvider::new(kind));
        app.state::<crate::commands::agent_chat::ProviderRegistry>().set_codex(provider.clone()).await;
        app.state::<NativeControlState>().fixture_capabilities.lock().unwrap().insert(kind, crate::agent_provider::codex::capabilities::build_capabilities(vec![]));
    }
    provider.enable_session_listing();
    let launched=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"native-caller-launch","provider":kind,"permission_mode":mode})).await.unwrap();
    let receipt=settle_operation(&app,&launched).await;
    assert_eq!(receipt["state"],"succeeded","{receipt}");
    let thread=receipt["thread_id"].as_str().unwrap().to_string();
    let authority=crate::delegation::authority::NativeAuthority::with_policy(&thread,kind,Some(mode));
    let generation=authority.begin(0);assert!(authority.bind(generation,"native-run"));
    let actor=crate::delegation::app::NativeActor {thread_id:thread.clone(),provider:kind,workspace_id:Some(workspace.clone()),session_id:format!("mock-{thread}"),permission_mode:Some(mode.into()),permit:authority.capture()};
    (app,provider,root,workspace,thread,authority,actor)
}

#[tokio::test]
async fn native_callers_reject_spoofed_arguments_and_replaced_session(){
    let (app,_provider,_root,_workspace,thread,authority,actor)=native_caller_fixture("default").await;
    for field in ["caller","_meta","sender_thread_id","authority_generation"]{
        let mut args=json!({"thread_id":thread,"message":"hello from this authenticated chat","client_request_id":format!("spoof-{field}")});
        args[field]=json!({"thread_id":"someone-else","access":"full_access"});
        let error=super::native::call(app.handle().clone(),actor.clone(),"thread_send".into(),args).await.unwrap_err();
        assert!(error.contains("unknown field"),"{error}");
    }
    let caller=super::native::qualify(app.handle(),&actor).await.unwrap();
    let _replacement=crate::delegation::authority::NativeAuthority::new(&thread);
    assert!(service::authorize(app.handle(),&caller).is_err());
    assert!(actor.permit.admit().is_err());
    authority.revoke();
}

#[tokio::test]
async fn native_callers_plan_ceiling_and_dispatch_downgrade(){
    use crate::agent_provider::ProviderKind;
    let (app,_provider,_root,_workspace,thread,_authority,actor)=native_caller_fixture("plan").await;
    let caller=super::native::qualify(app.handle(),&actor).await.unwrap();
    assert!(service::permission_ceiling(&caller,ProviderKind::Claude,Some("bypassPermissions")).is_err());
    assert!(service::permission_ceiling(&caller,ProviderKind::Codex,Some("danger-full-access")).is_err());
    assert_eq!(caller.ceiling().unwrap().inherited(ProviderKind::Claude).unwrap(),"plan");
    assert_eq!(caller.ceiling().unwrap().inherited(ProviderKind::Codex).unwrap(),"read-only");
    let error=super::native::call(app.handle().clone(),actor.clone(),"thread_launch".into(),json!({"provider":"claude","permission_mode":"bypassPermissions","title":"Escalation"})).await.unwrap_err();
    assert!(error.contains("permission_ceiling"),"{error}");
    let error=super::native::call(app.handle().clone(),actor.clone(),"thread_launch".into(),json!({"provider":"claude","worktree":{"strategy":"new"}})).await.unwrap_err();
    assert!(error.contains("unsupported_operation"),"{error}");
    assert!(service::thread_respond(app.handle(),&caller,json!({})).await.unwrap_err().to_string().contains("cannot grant"));
    let (app,_,_root,workspace,thread,_authority,actor)=native_caller_fixture("bypassPermissions").await;
    let caller=super::native::qualify(app.handle(),&actor).await.unwrap();
    let guard=NativeControlGuard {caller,workspace_id:workspace,thread_id:thread.clone(),provider:ProviderKind::Claude,admission:None}.retain_admission(app.handle());
    _authority.set_policy(ProviderKind::Claude,Some("plan"));
    assert!(guard.verify(app.handle()).unwrap_err().contains("permission_ceiling"),"per-turn downgrade is effective even before the persisted session mode changes");
    _authority.set_policy(ProviderKind::Claude,Some("bypassPermissions"));
    app.state::<crate::database::DatabaseStore>().update_agent_chat_session_config(&thread,&crate::database::AgentChatSessionConfig{permission_mode:Some(Some("plan".into())),..Default::default()}).unwrap();
    assert!(guard.verify(app.handle()).unwrap_err().contains("permission_ceiling"));
}

#[tokio::test]
async fn native_callers_null_launch_options_inherit_effective_ceiling(){
    let (app,_,_root,_workspace,_thread,_authority,actor)=native_caller_fixture("plan").await;
    for instance in [serde_json::Value::Null,json!("claude")] {
        let receipt=super::native::call(app.handle().clone(),actor.clone(),"thread_launch".into(),json!({"workspace_id":null,"provider":null,"provider_instance_id":instance,"permission_mode":null,"client_request_id":null,"title":"Inherited Plan child"})).await.unwrap();
        let outcome=tokio::time::timeout(std::time::Duration::from_secs(15),async {
            loop {
                let status=super::native::call(app.handle().clone(),actor.clone(),"operation_status".into(),json!({"operation_id":receipt["operation_id"]})).await.unwrap();
                if !matches!(status["state"].as_str(),Some("accepted"|"running")) {break status;}
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(outcome["state"],"succeeded","{outcome}");
        let child=outcome["thread_id"].as_str().unwrap();
        assert_eq!(app.state::<crate::database::DatabaseStore>().get_agent_chat_session(child).unwrap().permission_mode.as_deref(),Some("plan"));
    }
}

#[tokio::test]
async fn native_callers_catalog_is_mode_independent_and_toggle_controls_new_sessions(){
    use crate::mcp::registry::{HostCaller,HostTools};
    let (app,_,_root,workspace,thread,_authority,_actor)=native_caller_fixture("plan").await;
    let host=super::native::OrchestrationHost(app.handle().clone());
    let mut caller=HostCaller{control:None,thread_id:thread,provider:crate::agent_provider::ProviderKind::Claude,workspace_id:Some(workspace),permission_mode:Some("plan".into())};
    for provider in [crate::agent_provider::ProviderKind::Claude,crate::agent_provider::ProviderKind::Codex]{caller.provider=provider;for mode in ["plan","read-only","workspace-write","danger-full-access"]{caller.permission_mode=Some(mode.into());let tools=host.tools_for(&caller);assert!(tools.iter().any(|t|t.name=="thread_send"));assert!(tools.iter().any(|t|t.name=="thread_wait"));assert!(!tools.iter().any(|t|t.name=="thread_respond"));}}
    let capabilities=super::native::call(app.handle().clone(),_actor,"orchestrator_capabilities".into(),json!({})).await.unwrap();
    assert!(capabilities["providers"][0]["allowed_permission_modes"].as_array().unwrap().contains(&json!("plan")));
    app.state::<crate::database::DatabaseStore>().set_setting("agents.cross_provider_delegation","false").unwrap();
    assert!(host.tools_for(&caller).is_empty());
}

#[tokio::test]
async fn native_callers_project_inventory_bounded_reads_and_idle_wait_use_canonical_context(){
    let (app,_,_root,workspace,thread,_authority,actor)=native_caller_fixture("bypassPermissions").await;
    let db=app.state::<crate::database::DatabaseStore>();
    db.set_agent_chat_title(&thread,"Native coordinator fixture").unwrap();
    db.append_agent_chat_message(&thread,&json!({"type":"user_message","thread_id":thread,"text":"Visible bounded message"}).to_string()).unwrap();
    let list=super::native::call(app.handle().clone(),actor.clone(),"thread_list".into(),json!({})).await.unwrap();
    assert!(list["threads"].as_array().unwrap().iter().any(|t|t["thread_id"]==thread&&t["workspace_id"]==workspace));
    let search=super::native::call(app.handle().clone(),actor.clone(),"thread_search".into(),json!({"query":"Native coordinator"})).await.unwrap();
    assert!(search["matches"].as_array().unwrap().iter().any(|t|t["thread_id"]==thread));
    let first=super::native::call(app.handle().clone(),actor.clone(),"thread_read".into(),json!({"thread_id":thread,"max_chars":7})).await.unwrap();
    assert_eq!(first["messages"][0]["content"],"Visible");assert_eq!(first["messages"][0]["truncated"],true);
    let second=super::native::call(app.handle().clone(),actor.clone(),"thread_read".into(),json!({"thread_id":thread,"cursor":first["next_cursor"],"text_offset":first["next_text_offset"]})).await.unwrap();
    assert_eq!(second["messages"][0]["content"]," bounded message");
    let wait=super::native::call(app.handle().clone(),actor,"thread_wait".into(),json!({"thread_id":thread,"timeout_ms":1})).await.unwrap();
    assert_eq!(wait["timed_out"],false);assert_eq!(wait["settled"],true);
}

#[tokio::test]
async fn native_callers_superseded_wait_releases_delivery_without_cancelling_task(){
    use crate::mcp::registry::{HostCaller,HostTools};
    let (app,_,_root,workspace,thread,authority,actor)=native_caller_fixture("bypassPermissions").await;
    let control=super::native::qualify(app.handle(),&actor).await.unwrap();
    let handle=app.handle().clone();let parent=thread.clone();
    let job=tokio::spawn(async move {
        let host=crate::commands::delegation::DelegationHost::with_install_check(handle,|_|true);
        let caller=HostCaller{control:Some(control),thread_id:parent,provider:crate::agent_provider::ProviderKind::Claude,workspace_id:Some(workspace),permission_mode:Some("bypassPermissions".into())};
        host.call_async(&caller,"mcp__codemux__delegate_task",&json!({"provider":"claude","title":"Waiting child","task":"Complete this isolated mock child task independently.","mode":"wait","wait_ms":20000}),None,None).await.unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(3),async{loop{if !app.state::<crate::database::DatabaseStore>().delegation_tasks(Some(&thread)).unwrap().is_empty(){break;}tokio::task::yield_now().await;}}).await.unwrap();
    authority.invalidate();
    let result=tokio::time::timeout(std::time::Duration::from_secs(3),job).await.unwrap().unwrap();
    assert_eq!(result["isError"],true);
    let tasks=app.state::<crate::database::DatabaseStore>().delegation_tasks(Some(&thread)).unwrap();
    assert_eq!(tasks.len(),1);assert!(tasks[0].wait_until.is_none());assert!(!tasks[0].status.terminal());
}

#[tokio::test]
async fn native_callers_toggle_is_enforced_at_dispatch_but_suggestions_still_work() {
    let (app,_,_root,_workspace,thread,authority,mut actor)=native_caller_fixture("bypassPermissions").await;
    app.state::<crate::database::DatabaseStore>().set_setting("agents.cross_provider_delegation","false").unwrap();
    let error=native::call(app.handle().clone(),actor.clone(),"thread_list".into(),json!({})).await.unwrap_err();
    assert!(error.contains("orchestration_disabled"),"{error}");
    authority.set_policy(crate::agent_provider::ProviderKind::Claude,Some("unmapped"));
    actor.permission_mode=Some("unmapped".into());
    let result=native::call(app.handle().clone(),actor,"suggest_tasks".into(),json!({"tasks":[{"title":"Review","task":"Review the parser and run its focused regression tests."}]})).await.unwrap();
    assert!(!result.to_string().contains("orchestration_disabled"));
    assert!(app.state::<crate::database::DatabaseStore>().get_agent_chat_session(&thread).is_some());
}

#[tokio::test]
async fn native_callers_validate_every_explicit_delegate_mode_against_catalog() {
    use crate::agent_provider::ProviderKind;
    use crate::mcp::registry::{HostCaller,HostTools};
    for (parent_mode,child_mode,expected_plan) in [("plan","plan",true),("bypassPermissions","read-only",false),("bypassPermissions","invented",false),("bypassPermissions","workspace-write",false)] {
        let (app,_,_root,workspace,thread,_authority,actor)=native_caller_fixture(parent_mode).await;
        let mut caps=crate::agent_provider::codex::capabilities::build_capabilities(vec![]);
        caps.permission_modes.retain(|m| m.value!="workspace-write");
        app.state::<NativeControlState>().fixture_capabilities.lock().unwrap().insert(ProviderKind::Codex,caps);
        let caller=HostCaller {control:Some(native::qualify(app.handle(),&actor).await.unwrap()),thread_id:thread.clone(),provider:ProviderKind::Claude,workspace_id:Some(workspace),permission_mode:Some(parent_mode.into())};
        let host=crate::commands::delegation::DelegationHost::with_install_check(app.handle().clone(),|_|true);
        let result=host.call_async(&caller,"mcp__codemux__delegate_task",&json!({"provider":"codex","permission_mode":child_mode,"task":"Compute 17 plus 18 without using tools.","title":"Policy child"}),None,None).await.unwrap();
        if matches!(child_mode,"invented"|"workspace-write") {
            assert_eq!(result["isError"],true,"{result}");
            assert!(result.to_string().contains("unsupported_permission_mode"),"{result}");
            assert!(app.state::<crate::database::DatabaseStore>().delegation_tasks(Some(&thread)).unwrap().is_empty());
        } else {
            assert_ne!(result["isError"],true,"{result}");
            let task=app.state::<crate::database::DatabaseStore>().delegation_tasks(Some(&thread)).unwrap().remove(0);
            assert_eq!(task.permission_mode.as_deref(),Some("read-only"));
            assert_eq!(task.plan_mode,expected_plan);
        }
    }
}

#[tokio::test]
async fn native_callers_admitted_message_survives_sender_pane_change_and_human_release() {
    let (app,provider,_root,workspace,thread,authority,actor)=native_caller_fixture("default").await;
    let target=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"receiver","provider":"claude","permission_mode":"default"})).await.unwrap();
    let target=settle_operation(&app,&target).await;
    let receiver=target["thread_id"].as_str().unwrap();
    let barrier=Arc::new(tokio::sync::Semaphore::new(0));
    *app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap()=Some(barrier.clone());
    let receipt=native::call(app.handle().clone(),actor,"thread_send".into(),json!({"thread_id":receiver,"message":"Accepted before pane closed","client_request_id":"accepted-pane-change"})).await.unwrap();
    let id=receipt["delivery_id"].as_str().unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    let mut caller:ControlCaller=serde_json::from_value(db.message_delivery(id).unwrap().unwrap().payload["caller"].clone()).unwrap();
    caller.admitted=Some(AdmittedAuthority::Delivery{delivery_id:id.into()});
    let pane=app.state::<crate::state::AppStateStore>().agent_chat_pane_id_for_thread(&thread).unwrap();
    app.state::<crate::state::AppStateStore>().close_pane(&pane).unwrap();authority.invalidate();
    service::authorize(app.handle(),&caller).unwrap();
    let gate=app.state::<NativeControlState>().thread_gate(receiver);
    gate.cancel();
    db.stop_message_deliveries(receiver).unwrap();
    let held=db.message_delivery(id).unwrap().unwrap();
    assert_eq!(held.state,"held");
    assert_eq!(held.hold_reason.as_deref(),Some("stop"));
    assert!(!held.attempted);
    assert!(service::authorize(app.handle(),&caller).is_err());
    assert_eq!(db.resume_message_delivery_thread(receiver).unwrap(),vec![id]);
    assert_eq!(db.message_delivery(id).unwrap().unwrap().state,"preparing");
    service::authorize(app.handle(),&caller).unwrap();
    barrier.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(5),async {loop {if provider.calls.snapshot().iter().any(|c|matches!(c,mock_agent_provider::MockCall::SendTurn(t,_) if t.0==receiver)){break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
    assert_ne!(db.message_delivery(id).unwrap().unwrap().state,"rejected");
}

#[tokio::test]
async fn native_callers_batch_partial_retry_preserves_receipts_and_never_takes_focus() {
    let (app,_,_root,workspace,thread,_authority,actor)=native_caller_fixture("default").await;
    let before=app.state::<crate::state::AppStateStore>().snapshot();
    let args=json!({"workspace_id":workspace,"threads":[{"title":"First","select":true},{"title":"Bad","permission_mode":"invented"}]});
    let first=native::call(app.handle().clone(),actor.clone(),"create_threads".into(),args.clone()).await.unwrap();
    assert_eq!(first["failed_index"],1,"{first}");assert_eq!(first["threads"].as_array().unwrap().len(),1);
    let retry=native::call(app.handle().clone(),actor,"create_threads".into(),args).await.unwrap();
    assert_eq!(first["client_request_id"],retry["client_request_id"]);
    assert_eq!(first["threads"][0]["operation_id"],retry["threads"][0]["operation_id"]);
    let receipt=tokio::time::timeout(std::time::Duration::from_secs(10),async {loop {
        let receipt=app.state::<crate::database::DatabaseStore>().control_operation(&format!("thread:{thread}"),first["threads"][0]["operation_id"].as_str().unwrap(),&app.state::<NativeControlState>().epoch).unwrap().unwrap();
        if !matches!(receipt.state.as_str(),"accepted"|"running") {break receipt;}
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }}).await.unwrap();
    assert_eq!(receipt.state,"succeeded");
    let after=app.state::<crate::state::AppStateStore>().snapshot();
    let before_workspace=before.workspaces.iter().find(|w|w.workspace_id.0==workspace).unwrap();
    let after_workspace=after.workspaces.iter().find(|w|w.workspace_id.0==workspace).unwrap();
    assert_eq!(before_workspace.active_surface_id,after_workspace.active_surface_id);
    assert_eq!(before_workspace.surfaces.iter().find(|s|s.surface_id==before_workspace.active_surface_id).unwrap().active_pane_id,after_workspace.surfaces.iter().find(|s|s.surface_id==after_workspace.active_surface_id).unwrap().active_pane_id);
    assert!(app.state::<crate::state::AppStateStore>().agent_chat_pane_id_for_thread(&thread).is_some());
}

#[tokio::test]
async fn native_callers_agent_ping_pong_stops_at_24_hops() {
    let (app,_provider,_root,workspace,a,authority_a,actor_a)=native_caller_fixture("default").await;
    let launch=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"ping-b","provider":"claude","permission_mode":"default"})).await.unwrap();
    let b=settle_operation(&app,&launch).await["thread_id"].as_str().unwrap().to_string();
    let authority_b=crate::delegation::authority::NativeAuthority::with_policy(&b,crate::agent_provider::ProviderKind::Claude,Some("default"));
    let generation=authority_b.begin(0);authority_b.bind(generation,"ping-b-run");
    let actor_b=crate::delegation::app::NativeActor {thread_id:b.clone(),session_id:format!("mock-{b}"),permit:authority_b.capture(),..actor_a.clone()};
    let root=app.state::<crate::database::DatabaseStore>().orchestration_context(&a).unwrap().root_id;
    for hop in 0..=24 {
        let (sender,target)=if hop%2==0 {(actor_a.clone(),&b)} else {(actor_b.clone(),&a)};
        let barrier=Arc::new(tokio::sync::Semaphore::new(0));
        *app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap()=Some(barrier.clone());
        let result=native::call(app.handle().clone(),sender,"thread_send".into(),json!({"thread_id":target,"message":format!("Ping hop {hop}"),"client_request_id":format!("hop-{hop}")})).await;
        if hop==24 {assert!(result.unwrap_err().contains("resource_limit"));break;}
        let receipt=result.unwrap();let id=receipt["delivery_id"].as_str().unwrap();
        app.state::<crate::database::DatabaseStore>().claim_message_delivery(id).unwrap();
        app.state::<crate::database::DatabaseStore>().message_delivery_submitting(id).unwrap();
        app.state::<crate::database::DatabaseStore>().finish_message_delivery(id,"accepted",None,None).unwrap();
        barrier.add_permits(1);
    }
    assert_eq!(app.state::<crate::database::DatabaseStore>().orchestration_context(&a).unwrap().root_id,root);
    drop((authority_a,authority_b));
}

#[tokio::test]
async fn native_callers_task_cancel_checks_ownership_before_target_ceiling() {
    let (app,_,_root,_workspace,_thread,_authority,actor)=native_caller_fixture("plan").await;
    let mut task=crate::database::delegation_tasks::TaskRecord::new("foreign-task".into(),"foreign-parent".into(),None,"codex".into(),"Foreign".into(),"A foreign task brief".into(),1);
    task.permission_mode=Some("danger-full-access".into());
    app.state::<crate::database::DatabaseStore>().admit_delegation_task(&task,None,&json!({})).unwrap();
    let error=native::call(app.handle().clone(),actor,"task_cancel".into(),json!({"task_id":"foreign-task"})).await.unwrap_err();
    assert_eq!(error,"Task is not owned by this parent");
}

#[tokio::test]
async fn native_callers_self_tools_restore_full_access_and_keep_lower_modes_read_only() {
    for mode in ["bypassPermissions","plan"] {
        let (app,_,_root,_workspace,_thread,_authority,actor)=native_caller_fixture(mode).await;
        app.state::<crate::database::DatabaseStore>().set_setting("agents.cross_provider_delegation","false").unwrap();
        let error=native::call(app.handle().clone(),actor,"terminal_write".into(),json!({"pane_id":"missing","text":"fixture"})).await.unwrap_err();
        if mode=="plan" {assert!(error.contains("provider_control_denied"),"{error}");}
        else {assert!(!error.contains("provider_control_denied") && !error.contains("orchestration_disabled"),"{error}");}
    }
}

#[tokio::test]
async fn native_callers_workspace_launch_routes_and_keeps_admitted_thread_authority() {
    let (app,provider,root,workspace,thread,_authority,actor)=native_caller_fixture("bypassPermissions").await;
    app.state::<crate::database::DatabaseStore>().set_setting("agents.cross_provider_delegation","false").unwrap();
    let result=native::call(app.handle().clone(),actor.clone(),"workspace_launch".into(),json!({"title":"","message":"A complete fixture brief"})).await.unwrap();
    assert_eq!(result["isError"],true);assert!(result.to_string().contains("title"));
    assert!(!result.to_string().contains("unsupported_operation"));
    // The host admits the same authenticated principal before handing startup
    // to PreparedWorkspaceChat; pane changes cannot revoke accepted startup.
    app.state::<crate::database::DatabaseStore>().set_setting("agents.cross_provider_delegation","true").unwrap();
    let mut caller=native::qualify(app.handle(),&actor).await.unwrap();
    let admission=app.state::<crate::database::DatabaseStore>().admit_control_operation(&caller.principal,"workspace-launch","workspace_launch","payload",&app.state::<NativeControlState>().epoch,Some(&workspace),None).unwrap();
    caller.admitted=Some(AdmittedAuthority::Operation {operation_id:admission.receipt.operation_id});
    app.state::<crate::database::DatabaseStore>().set_setting("agents.cross_provider_delegation","false").unwrap();
    let pane=app.state::<crate::state::AppStateStore>().agent_chat_pane_id_for_thread(&thread).unwrap();
    app.state::<crate::state::AppStateStore>().close_pane(&pane).unwrap();
    let child_workspace=app.state::<crate::state::AppStateStore>().create_empty_workspace_at_path(root.path().into());
    let prepared=service::PreparedWorkspaceChat::prepare(app.handle(),&caller,crate::agent_provider::ProviderKind::Claude,"Independent workspace".into(),"Work independently on this fixture".into(),None,None).await.unwrap();
    let launched=prepared.materialize(app.handle(),&caller,child_workspace.0,"independent-child").await.unwrap();
    let child=launched["thread_id"].as_str().unwrap();
    assert!(provider.calls.snapshot().iter().any(|c|matches!(c,mock_agent_provider::MockCall::SendTurn(t,_) if t.0==child)));
}

#[test]
fn orchestration_message_stale_caller_fences_hold_without_definitive_rejection() {
    let db=crate::database::DatabaseStore::new_in_memory();
    db.admit_message_delivery("stale","target","stale",&json!({"origin":"controller"}),"epoch",0,None,false).unwrap();
    for _ in 0..4 {
        db.claim_message_delivery("stale").unwrap();
        assert_eq!(db.fail_message_delivery("stale","stale_caller: generation fence changed").unwrap(),"held");
        db.retry_message_delivery("target","stale").unwrap();
    }
}

#[tokio::test]
async fn native_callers_rearmed_queued_agent_message_keeps_its_identity_and_authority() {
    use crate::agent_provider::{ProviderKind, TurnDispatchCheckpoint};
    let (app,_,_root,workspace,thread,authority,actor)=native_caller_fixture("default").await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"rearm-target","provider":"claude","permission_mode":"default"})).await.unwrap();
    let receiver=settle_operation(&app,&receipt).await["thread_id"].as_str().unwrap().to_string();
    let mut caller=native::qualify(app.handle(),&actor).await.unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    let payload=json!({"origin":"controller","caller":caller});
    db.admit_message_delivery("queued-agent",&receiver,"original-message",&payload,"epoch",0,None,false).unwrap();
    db.finish_message_delivery("queued-agent","queued",Some(&json!({"queued_id":"original-queue"})),None).unwrap();
    caller.admitted=Some(AdmittedAuthority::Delivery{delivery_id:"queued-agent".into()});
    let control=NativeControlGuard{caller,workspace_id:workspace,thread_id:receiver.clone(),provider:ProviderKind::Claude,admission:None}.retain_admission(app.handle());
    let checkpoint=super::delivery::DeliveryCheckpoint{
        app:app.handle().clone(),id:"queued-agent".into(),thread:receiver.clone(),provider:ProviderKind::Claude,
        release_queue:false,resumed_ids:vec![],admitted_generation:0,
        inner:Some(Arc::new(super::GuardedTurnCheckpoint{app:app.handle().clone(),control,inner:None})),
    };
    let pane=app.state::<crate::state::AppStateStore>().agent_chat_pane_id_for_thread(&thread).unwrap();
    app.state::<crate::state::AppStateStore>().close_pane(&pane).unwrap();
    authority.invalidate();
    let gate=app.state::<NativeControlState>().thread_gate(&receiver);
    gate.cancel();db.stop_message_deliveries(&receiver).unwrap();
    let held=db.message_delivery("queued-agent").unwrap().unwrap();
    assert_eq!(held.state,"held");assert_eq!(held.hold_reason.as_deref(),Some("stop"));assert!(!held.attempted);
    assert!(checkpoint.authorize_dispatch().is_err());
    assert_eq!(db.resume_message_delivery_thread(&receiver).unwrap(),vec!["queued-agent"]);
    let rearmed=db.message_delivery("queued-agent").unwrap().unwrap();
    assert_eq!(rearmed.state,"queued");assert_eq!(rearmed.message_id,"original-message");assert_eq!(rearmed.queued_id.as_deref(),Some("original-queue"));
    checkpoint.authorize_dispatch().unwrap();
    assert!(db.message_delivery("queued-agent").unwrap().unwrap().attempted);
    gate.cancel();assert!(checkpoint.authorize_dispatch().is_err());
}

#[tokio::test]
async fn native_callers_saved_agent_send_rearms_after_sender_pane_closes() {
    use crate::agent_provider::{message::*, types::MessageDelivery, ProviderKind, ThreadId};
    let (app,provider,_root,workspace,thread,authority,actor)=native_caller_fixture("default").await;
    let receipt=execute(app.handle(),&ControlCaller::trusted(),"thread_launch",json!({"workspace_id":workspace,"client_request_id":"saved-agent-target","provider":"claude","permission_mode":"default"})).await.unwrap();
    let receiver=settle_operation(&app,&receipt).await["thread_id"].as_str().unwrap().to_string();
    let caller=native::qualify(app.handle(),&actor).await.unwrap();
    let db=app.state::<crate::database::DatabaseStore>();
    let provenance=MessageProvenance{message_id:Some("saved-agent-message".into()),delivery_id:Some("saved-agent-delivery".into()),origin:Some(crate::commands::agent_chat::TurnOrigin::Controller),created_by:CreatedBy::Agent,creation_source:CreationSource::Provider,sender_thread_id:Some(thread.clone()),..Default::default()};
    let payload=json!({"provider":"claude","origin":"controller","input":{"thread_id":receiver,"text":"Retained agent report"},"provenance":provenance,"caller":caller});
    db.admit_message_delivery("saved-agent-delivery",&receiver,"saved-agent-message",&payload,"old",0,None,false).unwrap();
    db.finish_message_delivery("saved-agent-delivery","queued",Some(&json!({"queued_id":"saved-agent-queue"})),None).unwrap();
    db.insert_queued_turn(&crate::database::queued_turns::QueuedTurnRecord{thread_id:receiver.clone(),queued_id:"saved-agent-queue".into(),position:0,text:"Retained agent report".into(),prompt:None,provenance,skill_ids:vec![],skill_text:None,include_plugins:false,images:vec![],client_nonce:Some("saved-agent-message".into()),model_override:None,effort_override:None,permission_mode_override:None,epoch:"old".into()}).unwrap();
    db.stop_message_deliveries(&receiver).unwrap();
    let pane=app.state::<crate::state::AppStateStore>().agent_chat_pane_id_for_thread(&thread).unwrap();
    app.state::<crate::state::AppStateStore>().close_pane(&pane).unwrap();authority.invalidate();
    crate::commands::queued_turns::send_saved_queued_turn(app.handle().clone(),ProviderKind::Claude,ThreadId(receiver.clone()),"saved-agent-queue".into(),MessageDelivery::Queue,true).await.unwrap();
    let row=db.message_delivery("saved-agent-delivery").unwrap().unwrap();
    assert_eq!(row.message_id,"saved-agent-message");assert!(row.attempted);assert_eq!(row.state,"accepted");
    assert_eq!(provider.calls.snapshot().iter().filter(|c|matches!(c,mock_agent_provider::MockCall::SendTurn(t,text) if t.0==receiver && text=="Retained agent report")).count(),1);
}

#[tokio::test]
async fn native_callers_queued_human_keeps_active_turn_authority_until_stop() {
    let (app,_,_root,_workspace,thread,authority,actor)=native_caller_fixture("default").await;
    let caller=native::qualify(app.handle(),&actor).await.unwrap();
    crate::delegation::authority::NativeAuthority::user_activity_thread(&thread,false);
    service::authorize(app.handle(),&caller).unwrap();
    caller.admit(||Ok(())).unwrap();
    actor.permit.identity().unwrap();
    authority.invalidate();
    assert!(service::authorize(app.handle(),&caller).is_err());
    assert!(caller.admit(||Ok(())).is_err());
}

#[tokio::test]
async fn native_callers_round2_stop_revokes_live_descendants_before_interrupts() {
    use crate::commands::delegation::{on_thread_stopped, StopKind};
    use crate::database::delegation_tasks::{TaskRecord, TaskStatus};
    let (app, _, _root, workspace, grandchild, _authority, actor) = native_caller_fixture("default").await;
    let db = app.state::<crate::database::DatabaseStore>();
    for (parent, child, id) in [("stopped-parent", "stopped-child", "first"), ("stopped-child", grandchild.as_str(), "second")] {
        let mut task = TaskRecord::new(id.into(), parent.into(), Some(child.into()), "claude".into(), "Nested task".into(), "A complete nested task brief".into(), 1);
        task.workspace_id = workspace.clone();
        task.permission_mode = Some("default".into());
        // Published tasks may still own a live native turn. Revocation must
        // precede the terminal-status shortcut as well as provider waits.
        task.status = TaskStatus::Completed;
        db.admit_delegation_task(&task, None, &json!({})).unwrap();
    }
    let gate = app.state::<NativeControlState>().thread_gate(&grandchild);
    let generation = gate.generation();
    let tasks_before = db.delegation_tasks(None).unwrap().len();
    let chats_before = db.control_thread_ids(&workspace, None, 100).unwrap();
    on_thread_stopped(app.handle(), "stopped-parent", StopKind::Stop);
    // No yield or provider acknowledgement between Stop and these calls.
    for (name, args) in [
        ("delegate_task", json!({"provider":"claude","title":"Late child","task":"This task must never be admitted after Stop","client_request_id":"late-task"})),
        ("thread_launch", json!({"title":"Late chat","client_request_id":"late-launch"})),
        ("thread_send", json!({"thread_id":grandchild,"message":"This message must never be admitted","client_request_id":"late-send"})),
    ] {
        let error = native::call(app.handle().clone(), actor.clone(), name.into(), args).await.unwrap_err();
        assert!(error.contains("stale_caller"), "{name}: {error}");
    }
    assert!(gate.verify(generation).is_err());
    assert_eq!(db.delegation_tasks(None).unwrap().len(), tasks_before);
    assert_eq!(db.control_thread_ids(&workspace, None, 100).unwrap(), chats_before);
    for key in ["late-launch", "late-send"] { assert!(db.control_operation_by_key(&format!("thread:{grandchild}"), key, if key == "late-launch" { "thread_launch" } else { "thread_send" }, "unused", &app.state::<NativeControlState>().epoch).unwrap().is_none()); }
}

#[tokio::test]
async fn native_callers_round2_close_revokes_tab_child_before_interrupt() {
    use crate::commands::delegation::{on_thread_stopped, DelegationState, StopKind, Task};
    let (app, _, _root, workspace, child, _authority, actor) = native_caller_fixture("default").await;
    use crate::delegation::{journal::Journal, DelegateTaskInput, Grant, Parent};
    let dir = tempfile::tempdir().unwrap();
    let journal = Arc::new(Journal::open(dir.path()).unwrap());
    let parent = Parent { thread_id:child.clone(), workspace_id:workspace, provider:crate::agent_provider::ProviderKind::Claude, label:"Tab child".into(), permission_mode:"default".into(), event_id:None };
    let grant: Grant = serde_json::from_value(json!({"id":"grant","host_id":4,"host_name":"Fixture host","ssh_target":"fixture-host","workspace_path":"/fixture/checkout","workspace_name":"Checkout","provider":"codex","permission_mode":"danger-full-access","enabled":true,"created_at":"fixture"})).unwrap();
    let input = DelegateTaskInput { target_id:grant.id.clone(), prompt:"Existing SSH task".into(), title:None, model:None, effort:None, client_request_id:"ssh-child-close".into() };
    journal.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = journal.admit(&parent, &grant, "checkout", &input).unwrap();
    app.manage(journal.clone());
    app.state::<DelegationState>().reserve_worktree(&child, Task::new("closed-parent", crate::agent_provider::ProviderKind::Claude, "Tab child", "A tab child task", 1));
    let gate = app.state::<NativeControlState>().thread_gate(&child);
    let generation = gate.generation();
    let _dispatch = gate.dispatch.clone().lock_owned().await;
    on_thread_stopped(app.handle(), "closed-parent", StopKind::Close);
    assert!(actor.permit.identity().is_err());
    assert!(gate.verify(generation).is_err());
    let error = native::call(app.handle().clone(), actor, "thread_launch".into(), json!({"client_request_id":"closed-launch"})).await.unwrap_err();
    assert!(error.contains("stale_caller"), "{error}");
    assert!(gate.verify(generation).is_err());
    let retained = journal.task(&task.id).unwrap();
    assert!(!retained.cancel_requested, "Close must leave the tab child's SSH work running");
    assert_eq!(retained.status, task.status);
    assert_eq!(retained.wake_state, task.wake_state);
}

#[tokio::test]
async fn native_callers_round2_long_batch_keys_preserve_entry_receipts() {
    for key in ["k".repeat(128), "é".repeat(64)] {
        let (app, _, _root, workspace, _, _authority, actor) = native_caller_fixture("default").await;
        let args = json!({"workspace_id":workspace,"client_request_id":key,"threads":[{"title":"First"},{"title":"Second"}]});
        let first = native::call(app.handle().clone(), actor.clone(), "create_threads".into(), args.clone()).await.unwrap();
        assert!(first.get("error").is_none(), "{first}");
        assert_eq!(first["threads"].as_array().unwrap().len(), 2);
        let retry = native::call(app.handle().clone(), actor, "create_threads".into(), args).await.unwrap();
        assert_eq!(first["client_request_id"], key);
        for index in 0..2 { assert_eq!(first["threads"][index]["operation_id"], retry["threads"][index]["operation_id"]); }
    }
}

#[tokio::test]
async fn native_callers_round2_workspace_launch_inherits_lower_modes_and_keeps_plan_refusal() {
    use crate::agent_provider::ProviderKind;
    use crate::mcp::registry::{HostCaller, HostTools};
    for (kind, mode) in [(ProviderKind::Claude,"default"),(ProviderKind::Claude,"acceptEdits"),(ProviderKind::Codex,"workspace-write"),(ProviderKind::Claude,"plan"),(ProviderKind::Codex,"plan")] {
        let (app, provider, root, workspace, thread, _authority, actor) = native_caller_provider_fixture(kind, mode).await;
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").current_dir(root.path()).args(args).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        };
        git(&["init","-b","main"]);
        git(&["-c","user.name=Test","-c","user.email=test@example.com","-c","commit.gpgsign=false","commit","--allow-empty","-m","base"]);
        app.manage(crate::terminal::PtyState::default());
        app.manage(crate::presets::PresetStoreState::new(&app.state::<crate::database::DatabaseStore>()));
        let control = native::qualify(app.handle(), &actor).await.unwrap();
        let caller = HostCaller {control:Some(control),thread_id:thread.clone(),provider:kind,workspace_id:Some(workspace),permission_mode:Some(mode.into())};
        let host = crate::commands::workspace_launch::WorkspaceLaunchHost::with_install_check(app.handle().clone(), |_|true);
        let offered = host.tools_for(&caller).iter().any(|t|t.name=="workspace_launch");
        let result = host.call(&caller,"mcp__codemux__workspace_launch",&json!({"title":"Lower mode workspace","message":"Work on this self-contained fixture","branch":"lower-mode-fixture"})).unwrap();
        if mode == "plan" {
            assert!(!offered);
            assert_eq!(result["isError"],true);
            assert!(result.to_string().contains("Plan mode"));
            continue;
        }
        assert!(offered, "{kind:?} {mode}");
        assert_eq!(result["isError"],false,"{result}");
        let child = tokio::time::timeout(std::time::Duration::from_secs(10), async { loop {
            if let Some(child) = provider.calls.snapshot().iter().find_map(|c| match c {mock_agent_provider::MockCall::SendTurn(t,_) if t.0!=thread => Some(t.0.clone()), _ => None}) {break child;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }}).await.unwrap();
        let input = provider.start_inputs().into_iter().find(|i|i.thread_id.0==child).unwrap();
        let launched_ceiling = super::permissions::Ceiling::for_mode(kind,input.permission_mode.as_deref()).unwrap();
        assert!(caller.control.as_ref().unwrap().ceiling().unwrap().permits(launched_ceiling));
        assert_eq!(input.permission_mode.as_deref(),Some(mode));
        assert_eq!(app.state::<crate::database::DatabaseStore>().get_agent_chat_session(&child).unwrap().permission_mode,input.permission_mode);
        let db = app.state::<crate::database::DatabaseStore>();
        let parent_context = db.orchestration_context(&thread).unwrap();
        let child_context = db.orchestration_context(&child).unwrap();
        assert_eq!(child_context.root_id, parent_context.root_id);
        assert_eq!(child_context.depth, parent_context.depth + 1);
        for i in 1..6 {
            let target = format!("other-launch-{i}");
            db.admit_control_operation_for_thread(&caller.control.as_ref().unwrap().principal, &target, "thread_launch", &target, &app.state::<NativeControlState>().epoch, Some(caller.workspace_id.as_deref().unwrap()), Some(&target), Some((&thread, 0))).unwrap();
            db.orchestration_launch_finished(&target).unwrap();
        }
        let refused = host.call(&caller, "mcp__codemux__workspace_launch", &json!({"title":"Over budget", "message":"This must fail before starting"})).unwrap();
        assert_eq!(refused["isError"], true, "{refused}");
        assert!(refused.to_string().contains("6 new-work admissions"), "{refused}");
        // Only this test's checkout is removed; its repository is a TempDir.
        git(&["worktree","remove","--force",input.cwd.to_str().unwrap()]);
        std::fs::remove_dir(input.cwd.parent().unwrap()).unwrap();
    }
}

#[tokio::test]
async fn native_callers_round3_workspace_provider_listing_matches_ceiling_dispatch() {
    use crate::agent_provider::ProviderKind;
    use crate::mcp::registry::{HostCaller, HostTools};
    for mode in ["default", "acceptEdits", "bypassPermissions"] {
        let (app, _, root, workspace, thread, _authority, actor) = native_caller_fixture(mode).await;
        for args in [vec!["init", "-b", "main"], vec!["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "base"]] {
            assert!(std::process::Command::new("git").current_dir(root.path()).args(args).output().unwrap().status.success());
        }
        let mut control = native::qualify(app.handle(), &actor).await.unwrap();
        // Discovery must use the qualified ceiling, even if the callback's
        // cached mode is broader than the effective turn policy.
        let caller = HostCaller {control:Some(control.clone()), thread_id:thread, provider:ProviderKind::Claude, workspace_id:Some(workspace), permission_mode:Some("bypassPermissions".into())};
        let host = crate::commands::workspace_launch::WorkspaceLaunchHost::with_install_check(app.handle().clone(), |_|true);
        let tools = host.tools_for(&caller);
        let offered = tools[0].input_schema["properties"]["provider"]["enum"].as_array().unwrap();
        assert_eq!(offered.contains(&json!("opencode")), mode == "bypassPermissions");
        assert!(offered.contains(&json!("cursor")) && offered.contains(&json!("grok")));
        if mode != "bypassPermissions" {
            let denied = host.call(&caller, "mcp__codemux__workspace_launch", &json!({"title":"Denied provider", "message":"A complete brief", "provider":"opencode"})).unwrap();
            assert_eq!(denied["isError"], true, "{denied}");
            assert!(denied.to_string().contains("permission"), "{denied}");
        } else {
            let opencode = Arc::new(mock_agent_provider::MockAgentProvider::new(ProviderKind::OpenCode));
            app.state::<crate::commands::agent_chat::ProviderRegistry>().set_opencode(opencode.clone()).await;
            app.state::<NativeControlState>().fixture_capabilities.lock().unwrap().insert(ProviderKind::OpenCode, crate::agent_provider::opencode::capabilities::opencode_stage1_placeholder());
            // Exercise actual preparation and startup, not just the enum.
            let admitted = app.state::<crate::database::DatabaseStore>().admit_control_operation(&control.principal, "opencode-workspace", "workspace_launch", "payload", &app.state::<NativeControlState>().epoch, None, Some("opencode-child")).unwrap();
            control.admitted = Some(AdmittedAuthority::Operation {operation_id:admitted.receipt.operation_id});
            let prepared = service::PreparedWorkspaceChat::prepare(app.handle(), &control, ProviderKind::OpenCode, "OpenCode workspace".into(), "A complete OpenCode brief".into(), None, None).await.unwrap();
            let launched = prepared.materialize(app.handle(), &control, caller.workspace_id.clone().unwrap(), "opencode-child").await.unwrap();
            let child = launched["thread_id"].as_str().unwrap();
            let start = opencode.start_inputs().into_iter().find(|input| input.thread_id.0 == child).unwrap();
            assert_eq!(start.permission_mode, None);
            assert!(opencode.calls.snapshot().iter().any(|call|matches!(call,mock_agent_provider::MockCall::SendTurn(t,_) if t.0==child)));
        }
    }
}

#[tokio::test]
async fn native_callers_round2_dispatch_stop_fence_survives_late_callback_capture() {
    let (app, _, _root, workspace, thread, authority, actor) = native_caller_fixture("default").await;
    let db = app.state::<crate::database::DatabaseStore>();
    // Model the interval after durable Stop but before native invalidation.
    // Capturing callback identity now must retain the turn's old generation.
    db.stop_message_deliveries(&thread).unwrap();
    db.resume_message_delivery_thread(&thread).unwrap();
    actor.permit.identity().unwrap().verify(&thread).unwrap();
    for (name,args) in [
        ("delegate_task",json!({"provider":"claude","task":"Never admit work from a stopped native turn"})),
        ("thread_launch",json!({"client_request_id":"late-launch"})),
        ("thread_send",json!({"thread_id":thread,"message":"Never admit this message","client_request_id":"late-send"})),
    ] {
        let error = native::call(app.handle().clone(),actor.clone(),name.into(),args).await.unwrap_err();
        assert!(error.contains("stale_caller"),"{name}: {error}");
    }
    assert!(db.delegation_tasks(None).unwrap().is_empty());
    assert_eq!(db.control_thread_ids(&workspace,None,100).unwrap(),vec![thread.clone()]);
    let generation = authority.begin(db.delivery_generation(&thread).unwrap().0);
    assert!(authority.bind(generation,"new-human-turn"));
    let actor = crate::delegation::app::NativeActor {permit:authority.capture(),..actor};
    let receipt = native::call(app.handle().clone(),actor,"thread_launch".into(),json!({"client_request_id":"fresh-launch"})).await.unwrap();
    assert_eq!(receipt["state"],"accepted");
}

async fn settle_native(app: &tauri::App<tauri::test::MockRuntime>, actor: &crate::delegation::app::NativeActor, receipt: &serde_json::Value) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let status = super::native::call(app.handle().clone(), actor.clone(), "operation_status".into(), json!({"operation_id":receipt["operation_id"]})).await.unwrap();
            if !matches!(status["state"].as_str(), Some("accepted" | "running")) { break status; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_launch_without_workspace_gets_its_own_titled_child_workspace() {
    if crate::state::run_with_isolated_layout("agent_control::tests::native_launch_without_workspace_gets_its_own_titled_child_workspace") { return; }
    use crate::agent_provider::ProviderKind;
    let (app, _, root, workspace, _thread, _authority, actor) = native_caller_fixture("bypassPermissions").await;
    let state = app.state::<crate::state::AppStateStore>();
    // The caller sits in a linked worktree; the project's protected main
    // checkout is open and already holds an unrelated chat.
    let main_dir = tempfile::tempdir().unwrap();
    let main_root = main_dir.path().display().to_string();
    let main = state.create_empty_workspace_at_path(main_dir.path().to_path_buf()).0;
    state.set_workspace_protected(&main, true);
    state.set_workspace_project_root(&main, main_root.clone());
    state.create_agent_chat_pane(&main, Some(ProviderKind::Claude), None, Some(crate::presets::LaunchMode::NewTab), Some("old-root-chat".into())).unwrap();
    state.set_workspace_project_root(&workspace, main_root.clone());
    state.set_workspace_project_identity(&workspace, Some("project-uid".into()), Some("worktree".into()));
    state.set_workspace_worktree(&workspace, root.path().display().to_string(), "Coordinator".into());
    assert!(state.activate_workspace(&workspace));
    let root_before = serde_json::to_value(state.find_workspace(&main).unwrap()).unwrap();
    let count_before = state.snapshot().workspaces.len();

    let mut children = vec![];
    for (key, title) in [("ci-review-a", "CI speed review — A"), ("ci-review-b", "CI speed review — B")] {
        let args = json!({"client_request_id":key,"provider":"claude","title":title});
        let receipt = super::native::call(app.handle().clone(), actor.clone(), "thread_launch".into(), args.clone()).await.unwrap();
        let outcome = settle_native(&app, &actor, &receipt).await;
        assert_eq!(outcome["state"], "succeeded", "{outcome}");
        let child = receipt["workspace_id"].as_str().unwrap().to_string();
        assert_eq!(outcome["workspace_id"], child);
        assert_ne!(child, workspace);
        assert_ne!(child, main);
        // A retry reuses the receipt and creates nothing new.
        let retry = super::native::call(app.handle().clone(), actor.clone(), "thread_launch".into(), args).await.unwrap();
        assert_eq!(retry["operation_id"], receipt["operation_id"]);
        assert_eq!(retry["workspace_id"], child);

        let ws = state.find_workspace(&child).unwrap();
        assert_eq!(ws.parent_workspace_id.as_deref(), Some(workspace.as_str()));
        assert_eq!(ws.cwd, root.path().display().to_string());
        assert_eq!(ws.worktree_path, None);
        assert_eq!(ws.project_root.as_deref(), Some(main_root.as_str()));
        assert_eq!(ws.project_uid.as_deref(), Some("project-uid"));
        assert!(!ws.protected);
        assert_eq!(ws.title, title);
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.tabs[0].title, title);
        assert_eq!(ws.surfaces[0].title, title);
        // Its chat is selected inside it, so opening it shows the chat.
        assert_eq!(ws.active_tab_id, ws.tabs[0].tab_id);
        assert_eq!(ws.active_surface_id, ws.surfaces[0].surface_id);
        assert_eq!(ws.tabs[0].surface_id.as_ref(), Some(&ws.active_surface_id));
        let thread = outcome["thread_id"].as_str().unwrap();
        let record = app.state::<crate::database::DatabaseStore>().get_agent_chat_session(thread).unwrap();
        assert_eq!(record.workspace_id, child);
        assert_eq!(record.title.as_deref(), Some(title));
        children.push(child);
    }
    assert_eq!(state.snapshot().workspaces.len(), count_before + 2);
    assert_eq!(state.snapshot().active_workspace_id.0, workspace, "launches never take focus");
    assert_eq!(serde_json::to_value(state.find_workspace(&main).unwrap()).unwrap(), root_before, "the main checkout is untouched");

    // A batch without a workspace gets one child workspace per chat.
    let batch = super::native::call(app.handle().clone(), actor.clone(), "create_threads".into(), json!({"client_request_id":"batch-1","threads":[{"provider":"claude","title":"Batch one"},{"provider":"claude","message":"Look at the parser\nand more"}]})).await.unwrap();
    let receipts = batch["threads"].as_array().unwrap();
    assert_eq!(receipts.len(), 2, "{batch}");
    for (receipt, title) in receipts.iter().zip(["Batch one", "Look at the parser"]) {
        assert_eq!(settle_native(&app, &actor, receipt).await["state"], "succeeded");
        let ws = state.find_workspace(receipt["workspace_id"].as_str().unwrap()).unwrap();
        assert_eq!(ws.parent_workspace_id.as_deref(), Some(workspace.as_str()));
        assert_eq!(ws.title, title);
    }
    assert_eq!(state.snapshot().workspaces.len(), count_before + 4);

    // An explicit workspace keeps the tab placement, titled as requested.
    let tabs_before = state.find_workspace(&workspace).unwrap().tabs.len();
    let receipt = super::native::call(app.handle().clone(), actor.clone(), "thread_launch".into(), json!({"workspace_id":workspace,"provider":"claude","title":"Explicit tab"})).await.unwrap();
    assert_eq!(settle_native(&app, &actor, &receipt).await["state"], "succeeded");
    assert_eq!(receipt["workspace_id"], workspace);
    let ws = state.find_workspace(&workspace).unwrap();
    assert_eq!(ws.tabs.len(), tabs_before + 1);
    assert_eq!(ws.tabs.last().unwrap().title, "Explicit tab");
    assert_eq!(state.snapshot().workspaces.len(), count_before + 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_children_get_child_workspaces_that_close_with_the_parent() {
    if crate::state::run_with_isolated_layout("agent_control::tests::delegated_children_get_child_workspaces_that_close_with_the_parent") { return; }
    use crate::mcp::registry::{HostCaller, HostTools};
    let (app, _, root, workspace, thread, _authority, actor) = native_caller_fixture("bypassPermissions").await;
    app.manage(crate::terminal::PtyState::default());
    let state = app.state::<crate::state::AppStateStore>();
    let db = app.state::<crate::database::DatabaseStore>();
    let parent_tabs = state.find_workspace(&workspace).unwrap().tabs.len();
    let control = super::native::qualify(app.handle(), &actor).await.unwrap();
    let host = crate::commands::delegation::DelegationHost::with_install_check(app.handle().clone(), |_| true);
    let caller = HostCaller { control: Some(control), thread_id: thread.clone(), provider: crate::agent_provider::ProviderKind::Claude, workspace_id: Some(workspace.clone()), permission_mode: Some("bypassPermissions".into()) };
    let args = json!({"provider":"claude","title":"Review the parser","task":"Review the parser module and report any defects you find.","client_request_id":"delegate-child-1"});
    let receipt = host.call_async(&caller, "mcp__codemux__delegate_task", &args, None, None).await.unwrap();
    let retry = host.call_async(&caller, "mcp__codemux__delegate_task", &args, None, None).await.unwrap();
    assert_eq!(retry["task_id"], receipt["task_id"], "{retry}");
    let task = db.delegation_tasks(Some(&thread)).unwrap().pop().unwrap();
    let child_thread = task.child_thread_id.clone().unwrap();
    let child = task.workspace_id.clone();
    assert_eq!(db.delegation_tasks(Some(&thread)).unwrap().len(), 1);

    // Its own workspace, nested under the parent's, in the same folder.
    let ws = state.find_workspace(&child).unwrap();
    assert_ne!(child, workspace);
    assert_eq!(ws.parent_workspace_id.as_deref(), Some(workspace.as_str()));
    assert_eq!(ws.title, "Review the parser");
    assert_eq!(ws.cwd, root.path().display().to_string());
    assert_eq!(ws.worktree_path, None);
    assert_eq!(ws.tabs.len(), 1);
    assert_eq!(ws.tabs[0].title, "Claude · Review the parser");
    let pane = state.agent_chat_pane_id_for_thread(&child_thread).unwrap();
    assert_eq!(state.workspace_id_for_pane(&pane).as_deref(), Some(child.as_str()));
    assert_eq!(state.find_workspace(&workspace).unwrap().tabs.len(), parent_tabs, "no tab in the parent's workspace");

    // An independent launch is not owned and survives the parent's close.
    let launch = super::native::call(app.handle().clone(), actor.clone(), "thread_launch".into(), json!({"provider":"claude","title":"Independent"})).await.unwrap();
    assert_eq!(settle_native(&app, &actor, &launch).await["state"], "succeeded");
    let independent = launch["workspace_id"].as_str().unwrap().to_string();

    crate::commands::workspace::close_workspace_with_worktree_impl(
        app.handle().clone(), &state, &db, workspace.clone(), false, None, None,
    ).await.unwrap();
    assert!(state.find_workspace(&child).is_none(), "the delegated child closes with its parent's workspace");
    assert!(state.find_workspace(&independent).is_some(), "an independent launch stays");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !db.delegation_task_for_child(&child_thread).unwrap().unwrap().status.terminal() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
}

#[tokio::test]
async fn native_launch_catalog_steers_delegation_to_delegate_task() {
    use crate::mcp::registry::{HostCaller, HostTools};
    let (app, _, _root, workspace, thread, _authority, _actor) = native_caller_fixture("bypassPermissions").await;
    let host = super::native::OrchestrationHost(app.handle().clone());
    let caller = HostCaller { control: None, thread_id: thread, provider: crate::agent_provider::ProviderKind::Claude, workspace_id: Some(workspace), permission_mode: Some("bypassPermissions".into()) };
    let tools = host.tools_for(&caller);
    for name in ["thread_launch", "create_threads"] {
        let description = tools.iter().find(|t| t.name == name).unwrap().description.clone().unwrap();
        assert!(description.starts_with("If the user asks to delegate work or wants results back, use delegate_task instead."), "{description}");
        assert!(description.contains("nested under yours"), "{description}");
        assert!(!description.contains("project-root"), "{description}");
    }
}

#[tokio::test]
async fn all_provider_target_validation_precedes_admission_and_preserves_instance_catalog() {
    let (app, _, _root, workspace, _thread, _authority, actor) =
        native_caller_fixture("bypassPermissions").await;
    let db = app.state::<crate::database::DatabaseStore>();
    db.set_setting(
        "hermes.installation",
        "/missing/orchestration-fixture-hermes",
    )
    .unwrap();
    let before = db.control_thread_ids(&workspace, None, 100).unwrap();
    for target in [
        json!({"instance_id":"acp:missing"}),
        json!({"instance_id":"hermes:missing"}),
        json!({"instance_id":"claude:missing"}),
        json!({"instance_id":"claude","model":"unadvertised-model"}),
        json!({"instance_id":"claude","options":{"executable":"/bin/sh"}}),
    ] {
        let result = super::native::call(
            app.handle().clone(),
            actor.clone(),
            "thread_launch".into(),
            json!({"target":target,"title":"Unavailable selection"}),
        )
        .await;
        assert!(result.is_err(), "{result:?}");
        assert_eq!(
            db.control_thread_ids(&workspace, None, 100).unwrap(),
            before,
            "a readiness rejection must not publish a child pane/session"
        );
    }
}

#[tokio::test]
async fn all_provider_host_catalog_uses_the_same_tools_in_every_mode() {
    use crate::agent_provider::ProviderKind;
    use crate::mcp::registry::{HostCaller, HostTools};
    let (app, _, _root, workspace, thread, _authority, _actor) =
        native_caller_fixture("default").await;
    let host = super::native::OrchestrationHost(app.handle().clone());
    for provider in [
        ProviderKind::OpenCode,
        ProviderKind::Cursor,
        ProviderKind::Grok,
        ProviderKind::Hermes,
        ProviderKind::Acp,
    ] {
        for mode in [
            None,
            Some("plan"),
            Some("default"),
            Some("agent"),
            Some("supervised"),
        ] {
            let caller = HostCaller {
                control: None,
                thread_id: thread.clone(),
                provider,
                workspace_id: Some(workspace.clone()),
                permission_mode: mode.map(str::to_string),
            };
            let names: Vec<_> = host
                .tools_for(&caller)
                .into_iter()
                .map(|t| t.name)
                .collect();
            for name in [
                "orchestrator_capabilities",
                "thread_send",
                "thread_launch",
                "thread_wait",
                "thread_interrupt",
                "create_threads",
            ] {
                assert!(
                    names.iter().any(|n| n == name),
                    "{provider:?} {mode:?}: {names:?}"
                );
            }
            assert!(!names.iter().any(|n| n == "thread_respond"));
        }
    }
}

#[tokio::test]
async fn all_provider_task_retry_keeps_the_admitted_selection_when_target_is_unavailable() {
    use crate::mcp::registry::{HostCaller, HostTools};
    let (app, _, _root, workspace, thread, _authority, actor) =
        native_caller_fixture("bypassPermissions").await;
    let caller = super::native::qualify(app.handle(), &actor).await.unwrap();
    let db = app.state::<crate::database::DatabaseStore>();
    let args = json!({"target":{"instance_id":"acp:removed-instance","model":null,"options":{}},"title":"Already admitted worker","task":"Review this previously admitted task without launching another worker","client_request_id":"admitted-selection","permission_mode":"supervised"});
    let mut record = crate::database::delegation_tasks::TaskRecord::new(
        "existing-selection-task".into(),
        thread.clone(),
        Some("existing-selection-child".into()),
        "acp".into(),
        args["title"].as_str().unwrap().into(),
        args["task"].as_str().unwrap().into(),
        1,
    );
    record.workspace_id = workspace.clone();
    record.permission_mode = Some("supervised".into());
    let payload = json!({"provider":"acp","title":args["title"],"task":args["task"],"model":null,"effort":null,"role":null,"mode":"async","wait_ms":20000,"permission_mode":"supervised","target":args["target"]});
    db.admit_delegation_task(&record, Some("admitted-selection"), &payload)
        .unwrap();
    let host = crate::commands::delegation::DelegationHost::with_install_check(
        app.handle().clone(),
        |_| false,
    );
    let host_caller = HostCaller {
        control: Some(caller),
        thread_id: thread,
        provider: crate::agent_provider::ProviderKind::Claude,
        workspace_id: Some(workspace),
        permission_mode: Some("bypassPermissions".into()),
    };
    let response = host
        .call_async(
            &host_caller,
            "mcp__codemux__delegate_task",
            &args,
            None,
            None,
        )
        .await
        .unwrap();
    assert_ne!(response["isError"], true, "{response}");
    assert!(response.to_string().contains("existing-selection-task"));
    let mut conflict = args;
    conflict["task"] = json!("This is a different task with the same retry key");
    let response = host
        .call_async(
            &host_caller,
            "mcp__codemux__delegate_task",
            &conflict,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(response["isError"], true);
    assert!(response.to_string().contains("client_request_id_conflict"));
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn all_provider_queued_launch_keeps_delivery_authority_after_receipt_success() {
    let (app,provider,_root,_workspace,_thread,_authority,actor)=native_caller_fixture("default").await;
    provider.queue_next_send();
    let caller=native::qualify(app.handle(),&actor).await.unwrap();
    let receipt=native::call(app.handle().clone(),actor,"thread_launch".into(),json!({"client_request_id":"deferred-launch","title":"Queued worker","message":"Run after the launch receipt succeeds"})).await.unwrap();
    let launched=tokio::time::timeout(std::time::Duration::from_secs(15),async {
        loop {
            let status=service::operation_status(app.handle(),&caller,json!({"operation_id":receipt["operation_id"]})).unwrap();
            if !matches!(status["state"].as_str(),Some("accepted"|"running")){break status;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(launched["state"],"succeeded");
    let thread=launched["thread_id"].as_str().unwrap();
    let input=provider.take_queued_input();
    let checkpoint=input.turn_checkpoint.unwrap();
    checkpoint.prepare().await;
    checkpoint.verify_dispatch().expect("a succeeded launch does not revoke its queued first message");
    app.state::<NativeControlState>().thread_gate(thread).cancel();
    app.state::<crate::database::DatabaseStore>().stop_message_deliveries(thread).unwrap();
    assert!(checkpoint.verify_dispatch().is_err(),"Stop still revokes the queued first message");
    checkpoint.abort().await;
    assert!(provider.calls.snapshot().iter().all(|c|!matches!(c,mock_agent_provider::MockCall::SendTurn(..))));
}

/// Hermes runs a configured profile. workspace_launch carries the profile
/// through preparation: a Hermes chat keeps its own, a full-access chat of
/// another provider names one, and lower ceilings are never offered Hermes.
#[tokio::test]
async fn all_provider_workspace_launch_carries_hermes_profiles() {
    use crate::agent_provider::ProviderKind;
    use crate::mcp::registry::{HostCaller, HostTools};
    let (app, claude, root, workspace) = fixture().await;
    claude.enable_session_listing();
    app.manage(crate::commands::delegation::DelegationState::default());
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git").current_dir(root.path()).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    };
    git(&["init", "-b", "main"]);
    git(&["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "base"]);
    app.manage(crate::terminal::PtyState::default());
    app.manage(crate::presets::PresetStoreState::new(&app.state::<crate::database::DatabaseStore>()));
    // A configured Hermes with a default and a `coder` profile; no process runs.
    let hermes_home = tempfile::tempdir().unwrap();
    let installation = hermes_home.path().join("hermes");
    std::fs::write(&installation, "#!/bin/sh\n").unwrap();
    let hermes_root = hermes_home.path().join("root");
    std::fs::create_dir_all(hermes_root.join("profiles/coder")).unwrap();
    for home in [hermes_root.clone(), hermes_root.join("profiles/coder")] {
        std::fs::write(home.join("config.yaml"), "model: {provider: custom}\n").unwrap();
    }
    let db = app.state::<crate::database::DatabaseStore>();
    db.set_setting("hermes.installation", installation.to_str().unwrap()).unwrap();
    db.set_setting("hermes.root", hermes_root.to_str().unwrap()).unwrap();
    let profiles = super::selection::profiles(&db).unwrap();
    let coder = profiles.iter().find(|p| p.id == "coder").unwrap().clone();
    let runtime = Arc::new(crate::agent_provider::hermes::HermesProvider::new(Arc::new(crate::database::DatabaseStore::new_in_memory())));
    let catalog = json!({
        "models":{"currentModelId":"openai:fixture","availableModels":[{"modelId":"openai:fixture","name":"Fixture"}]},
        "modes":{"currentModeId":"default","availableModes":[{"id":"default","name":"Default"},{"id":"dont_ask","name":"Don't ask"}]},
    });
    for profile in &profiles {
        runtime.remember_catalog_for_test(profile, catalog.clone()).await;
    }
    app.manage(runtime);
    let hermes = Arc::new(mock_agent_provider::MockAgentProvider::new(ProviderKind::Hermes));
    hermes.enable_session_listing();
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_hermes(hermes.clone()).await;
    let mut caps = crate::agent_provider::claude::capabilities::claude_fallback_capabilities();
    caps.permission_modes = vec![crate::agent_provider::PermissionModeOption {value:"dont_ask".into(),label:"Don't ask".into(),description:String::new(),is_default:true}];
    app.state::<NativeControlState>().fixture_capabilities.lock().unwrap().insert(ProviderKind::Hermes, caps);

    let caller = |kind: ProviderKind, mode: &'static str| {
        let (app, workspace, coder, cwd) = (&app, workspace.clone(), coder.clone(), root.path().to_path_buf());
        async move {
            let launched = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({"workspace_id":workspace,"client_request_id":format!("hermes-caller-{kind:?}-{mode}"),"provider":kind,"permission_mode":mode})).await.unwrap();
            let receipt = settle_operation(app, &launched).await;
            assert_eq!(receipt["state"], "succeeded", "{receipt}");
            let thread = receipt["thread_id"].as_str().unwrap().to_string();
            let app = app.handle();
            if kind == ProviderKind::Hermes {
                app.state::<crate::database::DatabaseStore>().save_hermes_binding(&crate::agent_provider::hermes::binding::Binding {
                    schema_version: 1, profile: coder, thread_id: thread.clone(), workspace_id: Some(workspace.clone()), cwd,
                    acp_session_id: None, current_native_id: None, root_native_id: None, model_override: None,
                    resolved_model: None, permission_mode: Some(mode.into()), compatibility: String::new(), cleanup_pending: false,
                }).unwrap();
            }
            let authority = crate::delegation::authority::NativeAuthority::with_policy(&thread, kind, Some(mode));
            let generation = authority.begin(0);
            assert!(authority.bind(generation, "native-run"));
            let actor = crate::delegation::app::NativeActor {thread_id:thread.clone(),provider:kind,workspace_id:Some(workspace.clone()),session_id:format!("mock-{thread}"),permission_mode:Some(mode.into()),permit:authority.capture()};
            let control = native::qualify(app, &actor).await.unwrap();
            (HostCaller {control:Some(control),thread_id:thread,provider:kind,workspace_id:Some(workspace),permission_mode:Some(mode.into())}, authority)
        }
    };
    let host = crate::commands::workspace_launch::WorkspaceLaunchHost::with_install_check(app.handle().clone(), |_| true);
    let offered = |caller: &HostCaller| host.tools_for(caller)[0].input_schema["properties"]["provider"]["enum"].as_array().unwrap().contains(&json!("hermes"));
    let launched_profile = |parent: String| {
        let hermes = hermes.clone();
        async move {
            let input = tokio::time::timeout(std::time::Duration::from_secs(10), async { loop {
                let child = hermes.calls.snapshot().iter().find_map(|c| match c {mock_agent_provider::MockCall::SendTurn(t,_) if t.0 != parent => Some(t.0.clone()), _ => None});
                if let Some(input) = child.and_then(|child| hermes.start_inputs().into_iter().find(|i| i.thread_id.0 == child)) { break input; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }}).await.expect("the Hermes workspace chat started");
            hermes.calls.0.lock().unwrap().clear();
            input
        }
    };
    let cleanup = |input: &crate::agent_provider::StartSessionInput| {
        git(&["worktree", "remove", "--force", input.cwd.to_str().unwrap()]);
        let _ = std::fs::remove_dir(input.cwd.parent().unwrap());
    };

    // A full-access Hermes chat omits provider and keeps its own profile.
    let (hermes_caller, _a) = caller(ProviderKind::Hermes, "dont_ask").await;
    assert!(offered(&hermes_caller));
    let result = host.call(&hermes_caller, "mcp__codemux__workspace_launch", &json!({"title":"Hermes workspace","message":"A complete Hermes brief","branch":"hermes-own-profile"})).unwrap();
    assert_eq!(result["isError"], false, "{result}");
    let input = launched_profile(hermes_caller.thread_id.clone()).await;
    assert_eq!(input.extra["hermes_profile"]["id"], "coder");
    assert_eq!(input.permission_mode.as_deref(), Some("dont_ask"));
    cleanup(&input);

    // A lower ceiling is never offered Hermes and cannot name a profile.
    let (lower, _b) = caller(ProviderKind::Claude, "default").await;
    assert!(!offered(&lower));
    let refused = host.call(&lower, "mcp__codemux__workspace_launch", &json!({"title":"Lower Hermes","message":"A complete brief","target":{"instance_id":"hermes:coder"}})).unwrap();
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(refused.to_string().contains("permission"), "{refused}");

    // A full-access chat of another provider targets a configured profile.
    let (full, _c) = caller(ProviderKind::Claude, "bypassPermissions").await;
    assert!(offered(&full));
    let missing = host.call(&full, "mcp__codemux__workspace_launch", &json!({"title":"No profile","message":"A complete brief","provider":"hermes"})).unwrap();
    assert!(missing.to_string().contains("target.instance_id"), "{missing}");
    let result = host.call(&full, "mcp__codemux__workspace_launch", &json!({"title":"Targeted Hermes","message":"A complete brief","branch":"hermes-targeted","target":{"instance_id":"hermes:default"}})).unwrap();
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(result["content"][0]["text"].as_str().map(|t| t.contains("\"provider\":\"hermes\"")), Some(true), "{result}");
    let input = launched_profile(full.thread_id.clone()).await;
    assert_eq!(input.extra["hermes_profile"]["id"], "default");
    assert_eq!(input.permission_mode.as_deref(), Some("dont_ask"));
    cleanup(&input);
}
