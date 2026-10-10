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
