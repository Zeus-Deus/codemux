//! Actual start command and production DB; inert MCP registry prevents sidecars.
use super::*;
use crate::{commands::agent_chat::{agent_chat_start_session,ensure_live_session,ProviderRegistry}, database::{DatabaseStore,AgentChatSessionConfig},observability::ObservabilityStore,state::AppStateStore,mcp::{registry::McpRegistry,McpConfigSource}};
use futures_util::StreamExt;
use tauri::Manager;
struct Fixture { dir: tempfile::TempDir, app: tauri::App<tauri::test::MockRuntime>, provider: Arc<GenericAcpProvider>, agent: String, thread: ThreadId, pane: String }
impl Fixture {
    async fn new(mode: &str) -> Self {
        let dir=tempfile::tempdir().unwrap();
        let db=DatabaseStore::new_in_memory();
        let agent=db.save_acp_agent(super::super::AcpAgentInput {id:None,name:"Synthetic start agent".into(),executable:super::super::test_python::executable(),args:vec!["-I".into(),format!("{}/tests/helpers/fake_custom_acp.py",env!("CARGO_MANIFEST_DIR"))],environment:std::collections::BTreeMap::from([
            ("FAKE_ACP_MODE".into(),Some(mode.into())),("FAKE_ACP_LOG".into(),Some(dir.path().join("wire.jsonl").to_string_lossy().into_owned())),
            ("HOME".into(),Some(dir.path().to_string_lossy().into_owned())),("TMPDIR".into(),Some(dir.path().to_string_lossy().into_owned()))]),enabled:true,auth_method:None}).unwrap();
        let state=AppStateStore::default();
        let workspace=state.create_empty_workspace_at_path(dir.path().to_owned());
        let pane=state.create_agent_chat_pane(&workspace.0,Some(ProviderKind::Acp),Some(dir.path().to_string_lossy().into_owned()),None,None).unwrap().0;
        let mcp=McpRegistry::new();
        mcp.insert_running_server_for_test("codemux-self",vec![McpConfigSource::Codemux]).await;
        let app=tauri::test::mock_builder().manage(db).manage(ObservabilityStore::default()).manage(state).manage(mcp).build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap();
        let provider=Arc::new(GenericAcpProvider::new(Arc::new(crate::commands::custom_acp::AppAcpStore(app.handle().clone()))));
        let registry=ProviderRegistry::new(); registry.set_acp(provider.clone()).await;
        app.manage(provider.clone()); app.manage(registry);
        Self {dir,app,provider,agent:agent.id,thread:ThreadId(format!("rereview-start-{mode}")),pane}
    }
    async fn start(&self, model: Option<&str>, effort: Option<&str>) -> Result<ThreadId,String> {
        let input=serde_json::from_value(json!({"thread_id":self.thread.0,"cwd":self.dir.path(),"additional_directories":[],"extra":{"acp_agent_id":self.agent},"model":model,"effort":effort})).unwrap();
        agent_chat_start_session(self.app.handle().clone(),self.pane.clone(),ProviderKind::Acp,input,None).await
    }
    async fn command_send(&self, input: Value) -> Result<TurnStartResult, String> {
        if self.app.try_state::<crate::commands::agent_chat::SubagentTracker>().is_none() {
            self.app.manage(crate::commands::agent_chat::SubagentTracker::default());
            self.app.manage(crate::commands::agent_chat::RunActivityTracker::default());
            self.app.manage(crate::commands::agent_chat::AgentChatChannelRegistry::default());
        }
        crate::commands::agent_chat::agent_chat_send_turn(self.app.handle().clone(), ProviderKind::Acp, serde_json::from_value(input).unwrap()).await
    }
    fn row(&self) -> crate::database::AgentChatSessionRecord { self.app.state::<DatabaseStore>().get_agent_chat_session(&self.thread.0).unwrap() }
}
async fn pending_control_cancelled_turn_settles_before_ack() {
    let mode = "ownership-held-control";
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let handle = f.app.handle().clone(); let thread = f.thread.clone();
    let setter = tokio::spawn(async move { crate::commands::agent_chat::agent_chat_set_model(handle, ProviderKind::Acp, thread, Some("vendor:model [1m]".into())).await });
    wait_marker(&f.dir.path().join("wire.jsonl.ready")).await;
    let mut events = f.provider.event_stream();
    let sent = f.command_send(json!({"thread_id":f.thread.0,"text":"must never run","model_override":"default","effort_override":" A effort "})).await.unwrap();
    let queued = f.command_send(json!({"thread_id":f.thread.0,"text":"queued healthy followup","model_override":"vendor:model [1m]","effort_override":" B effort "})).await.unwrap();
    assert!(queued.queued_id.is_some());
    assert!(tokio::time::timeout(Duration::from_secs(1), crate::commands::agent_chat::agent_chat_interrupt_turn(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some(sent.turn_id.clone()))).await.unwrap().unwrap());
    let terminal = tokio::time::timeout(Duration::from_secs(1), async { loop {
        if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. }) = events.next().await {
            if turn_id == sent.turn_id { break status; }
        }
    }}).await;
    // Keep ACK withheld until after the decisive completion observation.
    let held_wire = std::fs::read_to_string(f.dir.path().join("wire.jsonl")).unwrap();
    assert!(!held_wire.contains("fixture_model_ack"));
    assert!(!setter.is_finished(), "cancelling admission must not orphan the external setter");
    std::fs::write(f.dir.path().join("wire.jsonl.release"), b"release").unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(4), setter).await.unwrap().unwrap();
    let followup = tokio::time::timeout(Duration::from_secs(4), async { loop {
        if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. }) = events.next().await {
            if turn_id != sent.turn_id { break status; }
        }
    }}).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let live = f.provider.has_session(&f.thread).await;
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire = ownership_evidence(&f, mode, json!({"terminal":format!("{terminal:?}"),"changed":format!("{changed:?}"),"followup":format!("{followup:?}"),"stored":stored,"row":row,"live":live}));
    assert!(matches!(terminal, Ok(TurnStatus::Error { ref subtype, .. }) if subtype == "cancelled"), "canceled turn must terminally complete while external ACK is still withheld: {terminal:?}");
    assert!(changed.is_ok(), "external setter must independently settle after release");
    assert!(matches!(followup, Ok(TurnStatus::Success)), "old cancellation permit must not cancel queued owner: {followup:?}");
    assert!(live);
    assert!(!wire.iter().any(|v| v["method"] == "session/set_config_option" && (v["params"]["value"] == "default" || v["params"]["value"] == " A effort ")), "cancelled overrides must send no mutation RPC");
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/prompt").count(), 1);
    assert_eq!(stored.catalog.current_model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(row.model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(row.effort.as_deref(), Some(" B effort "));
    assert!(!stored.config_values.values().any(|v| v == &json!("default") || v == &json!(" A effort ")));
}
#[tokio::test]
async fn ownership_pending_control_cancel_terminal_before_withheld_ack_no_overrides() { pending_control_cancelled_turn_settles_before_ack().await; }

#[tokio::test]
async fn ownership_pending_control_stop_terminal_before_withheld_ack_no_overrides() {
    let mode = "ownership-held-control";
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let provider = f.provider.clone(); let thread = f.thread.clone();
    let setter = tokio::spawn(async move { provider.set_model(thread, "vendor:model [1m]".into()).await });
    wait_marker(&f.dir.path().join("wire.jsonl.ready")).await;
    let mut events = f.provider.event_stream();
    let sent = f.provider.send_turn(serde_json::from_value(json!({"thread_id":f.thread.0,"text":"must never run","model_override":"default","effort_override":" A effort "})).unwrap()).await.unwrap();
    let stopped = tokio::time::timeout(Duration::from_secs(2), f.provider.stop_session(f.thread.clone())).await;
    let terminal = tokio::time::timeout(Duration::from_secs(1), async { loop {
        if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. }) = events.next().await {
            if turn_id == sent.turn_id { break status; }
        }
    }}).await;
    let changed = tokio::time::timeout(Duration::from_secs(2), setter).await.unwrap().unwrap();
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let wire = ownership_evidence(&f, "ownership-held-control-stop", json!({"terminal":format!("{terminal:?}"),"stopped":format!("{stopped:?}"),"changed":format!("{changed:?}"),"stored":stored}));
    assert!(matches!(stopped, Ok(Ok(()))));
    assert!(matches!(terminal, Ok(TurnStatus::Error { ref subtype, .. }) if subtype == "cancelled"));
    assert!(changed.is_err(), "Stop must settle the independently-owned setter through transport teardown");
    assert!(!wire.iter().any(|v| v.get("fixture_model_ack").is_some() || v["method"] == "session/prompt"));
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/set_config_option").count(), 1);
    assert_eq!(stored.catalog.current_model.as_deref(), Some("default"));
    assert_eq!(serde_json::to_value(&stored).unwrap(), serde_json::to_value(&before).unwrap(), "Stop must preserve exact pre-mutation acknowledged intent");
}
#[tokio::test]
async fn ownership_pending_control_cancel_already_wired_model_settles_without_effort_override() {
    let mode = "ownership-held-control";
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let mut events = f.provider.event_stream();
    let sent = f.provider.send_turn(serde_json::from_value(json!({"thread_id":f.thread.0,"text":"must never run","model_override":"vendor:model [1m]","effort_override":" B effort "})).unwrap()).await.unwrap();
    wait_marker(&f.dir.path().join("wire.jsonl.ready")).await;
    f.provider.interrupt_turn(f.thread.clone(), Some(sent.turn_id.clone())).await.unwrap();
    // This turn owns an already-wired mutation. Do not drop its unpolled
    // JSON-RPC future: let its ACK safely reconcile, then deny unsent effort.
    std::fs::write(f.dir.path().join("wire.jsonl.release"), b"release").unwrap();
    let terminal = tokio::time::timeout(Duration::from_secs(4), async { loop {
        if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. }) = events.next().await {
            if turn_id == sent.turn_id { break status; }
        }
    }}).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire = ownership_evidence(&f, "ownership-held-control-owned", json!({"terminal":format!("{terminal:?}"),"stored":stored,"row":row}));
    assert!(matches!(terminal, Ok(TurnStatus::Error { ref subtype, .. }) if subtype == "cancelled"));
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/set_config_option").count(), 1, "only the already-wired model mutation may reach the child");
    assert!(!wire.iter().any(|v| v["method"] == "session/prompt"));
    assert_eq!(stored.catalog.current_model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(row.model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(row.effort.as_deref(), Some(" B effort "));
    // Binding reconciliation intentionally projects model ACK's advertised
    // effort into durable recovery state, even without an effort setter.
    assert_eq!(stored.config_values, HashMap::from([("model selector".into(), json!("vendor:model [1m]")), ("effort B".into(), json!(" B effort "))]), "only actual model ACK state may be reconciled; no canceled override intent");
}

async fn renewed_writer_override(cancelled: bool, legacy: bool) {
    let mode = if legacy { "renewed-writer-legacy" } else { "dynamic-renewed-writer" };
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let session = f.provider.session(&f.thread).await.unwrap();
    let gate = Arc::new(crate::json_rpc_child::CallbackWriteGate::default());
    *session.child.callback_write_gate.lock().unwrap() = Some((json!("writer admission callback"), gate.clone()));
    session.request("fixture/callback", json!({})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), gate.held.notified()).await.unwrap();
    assert!(session.state.lock().await.pending.is_empty(), "unsupported callback creates no approval");
    let mut events = f.provider.event_stream();
    let sent = f.command_send(json!({"thread_id":f.thread.0,"text":"writer admission turn","model_override":"vendor:model [1m]"})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), gate.request_waiting.notified()).await.unwrap();
    assert!(session.mutation_catalog_updates.lock().await.is_some(), "actual catalog request must own deferral while waiting on writer");
    let interrupted = session.interrupt(Some(sent.turn_id.clone()));
    tokio::pin!(interrupted);
    if cancelled {
        assert!(futures_util::poll!(&mut interrupted).is_pending(), "interrupt waits for actual callback owner, not control");
        assert!(session.state.lock().await.interrupted, "cancellation is published before callback writer releases");
    }
    let held_wire = std::fs::read_to_string(f.dir.path().join("wire.jsonl")).unwrap();
    assert!(!held_wire.contains("session/set_config_option") && !held_wire.contains("session/set_model"), "override is wholly unsent, not withheld ACK");
    let held = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    assert_eq!(serde_json::to_value(&held).unwrap(), serde_json::to_value(&before).unwrap());
    gate.release.notify_one();
    if cancelled { tokio::time::timeout(Duration::from_secs(2), &mut interrupted).await.unwrap().unwrap(); }
    let terminal = tokio::time::timeout(Duration::from_secs(4), async { loop {
        if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. }) = events.next().await {
            if turn_id == sent.turn_id { break status; }
        }
    }}).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let live = f.provider.has_session(&f.thread).await;
    let deferral_released = session.mutation_catalog_updates.lock().await.is_none();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let label = format!("renewed-writer-{}-{}", if legacy { "legacy" } else { "config" }, if cancelled { "cancel" } else { "control" });
    let wire = ownership_evidence(&f, &label, json!({"terminal":format!("{terminal:?}"),"stored":stored,"row":row,"live":live,"deferral_released":deferral_released}));
    assert!(wire.iter().any(|v| v["id"] == "writer admission callback" && v["error"]["code"] == -32601));
    assert!(live && deferral_released);
    if cancelled {
        assert!(matches!(terminal, Ok(TurnStatus::Error { ref subtype, .. }) if subtype == "cancelled"));
        assert!(!wire.iter().any(|v| matches!(v["method"].as_str(), Some("session/set_config_option" | "session/set_model" | "session/prompt"))), "cancelled wholly-unsent override must never reach first-byte admission");
        assert_eq!(serde_json::to_value(&stored).unwrap(), serde_json::to_value(&before).unwrap(), "no cancelled durable intent");
        assert_eq!(row.model.as_deref(), Some("default"));
    } else {
        assert!(matches!(terminal, Ok(TurnStatus::Success)));
        assert_eq!(wire.iter().filter(|v| v["method"] == if legacy { "session/set_model" } else { "session/set_config_option" }).count(), 1);
        assert_eq!(stored.catalog.current_model.as_deref(), Some("vendor:model [1m]"));
        assert_eq!(row.model.as_deref(), Some("vendor:model [1m]"));
    }
}
#[tokio::test]
async fn renewed_n2_cancelled_config_override_waiting_writer_is_never_sent() { renewed_writer_override(true, false).await; }
#[tokio::test]
async fn renewed_n2_cancelled_legacy_override_waiting_writer_is_never_sent() { renewed_writer_override(true, true).await; }
#[tokio::test]
async fn renewed_n2_non_cancelled_config_override_waiting_writer_settles() { renewed_writer_override(false, false).await; }
#[tokio::test]
async fn renewed_n2_non_cancelled_legacy_override_waiting_writer_settles() { renewed_writer_override(false, true).await; }

async fn n3_wholly_unsent_prompt(cancelled: bool) {
    let mut f = Fixture::new("dynamic-renewed-writer").await;
    f.thread.0.push_str(&format!("-{}", Uuid::new_v4()));
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let session = f.provider.session(&f.thread).await.unwrap();
    let callback = Arc::new(crate::json_rpc_child::CallbackWriteGate::default());
    *session.child.callback_write_gate.lock().unwrap() = Some((json!("writer admission callback"), callback.clone()));
    session.request("fixture/callback", json!({})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), callback.held.notified()).await.unwrap();
    assert!(session.state.lock().await.pending.is_empty());
    let prompt = Arc::new(crate::json_rpc_child::PromptWriteGate::default());
    *session.child.prompt_write_gate.lock().unwrap() = Some(prompt.clone());
    let mut events = f.provider.event_stream();
    let sent = f.command_send(json!({"thread_id":f.thread.0,"text":"N3 wholly unsent no override"})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), prompt.request_waiting.notified()).await.unwrap();
    let queued = f.command_send(json!({"thread_id":f.thread.0,"text":"N3 healthy queued successor"})).await.unwrap();
    assert!(queued.queued_id.is_some());
    let interrupted = crate::commands::agent_chat::agent_chat_interrupt_turn(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some(sent.turn_id.clone()));
    tokio::pin!(interrupted);
    if cancelled {
        assert!(futures_util::poll!(&mut interrupted).is_pending(), "Interrupt must wait for the real callback owner");
        assert!(session.state.lock().await.interrupted);
    }
    callback.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), prompt.held.notified()).await.unwrap();
    if cancelled { assert!(tokio::time::timeout(Duration::from_secs(2), &mut interrupted).await.unwrap().unwrap()); }
    // Interrupt has fully returned, but the actual prompt future still owns
    // no frame bytes and has not run its writer-time admission closure.
    let held_wire = std::fs::read_to_string(f.dir.path().join("wire.jsonl")).unwrap();
    assert!(!held_wire.contains("session/prompt") && !held_wire.contains("session/cancel"));
    prompt.release.notify_one();
    let mut terminals = Vec::new();
    let settled = tokio::time::timeout(Duration::from_secs(4), async {
        while terminals.len() < 2 {
            let event = events.next().await.unwrap();
            crate::commands::agent_chat::forward_event(f.app.handle(), event.clone());
            if let ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. } = event { terminals.push((turn_id, status)); }
        }
    }).await;
    let healthy = f.provider.has_session(&f.thread).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let persisted = crate::commands::agent_chat::agent_chat_list_messages(f.app.state(), f.thread.0.clone()).await.unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let label = if cancelled { "n3-unsent-cancel" } else { "n3-unsent-control" };
    let wire = ownership_evidence(&f, label, json!({"interrupt_completed_before_admission":cancelled,"held_wire":held_wire,"terminals":terminals,"settled":format!("{settled:?}"),"healthy":healthy,"stored":stored,"row":row,"persisted":persisted}));
    if cancelled {
        assert!(!wire.iter().any(|v| v["method"] == "session/prompt" && v["params"]["prompt"][0]["text"] == "N3 wholly unsent no override"), "N3: wholly-unsent session/prompt dispatched after completed Interrupt");
        assert!(!wire.iter().any(|v| v["method"] == "session/cancel"), "denied prompt must not send premature cancel or cancel its queued successor");
    }
    assert!(settled.is_ok() && healthy, "local cancellation must retain a healthy queue: {terminals:?}");
    assert_eq!(terminals.iter().filter(|(id,_)| id == &sent.turn_id).count(), 1);
    assert!(matches!(&terminals[0].1, TurnStatus::Error { subtype, .. } if cancelled && subtype == "cancelled") || (!cancelled && matches!(&terminals[0].1, TurnStatus::Success)));
    assert_ne!(terminals[1].0, sent.turn_id);
    assert!(matches!(&terminals[1].1, TurnStatus::Success), "stale cancellation permits must not poison the queued owner");
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/prompt").count(), if cancelled { 1 } else { 2 });
    assert_eq!(serde_json::to_value(&stored).unwrap(), serde_json::to_value(&before).unwrap());
    let durable_terminals: Vec<Value> = persisted.iter().map(|s| serde_json::from_str::<Value>(s).unwrap()).filter(|v| v["type"] == "turn_completed").collect();
    assert_eq!(durable_terminals.len(), 2, "actual bridge and DB cold replay must retain one terminal per turn");
}
#[tokio::test]
async fn n3_completed_interrupt_revokes_wholly_unsent_no_override_prompt() { n3_wholly_unsent_prompt(true).await; }
#[tokio::test]
async fn n3_non_cancelled_wholly_unsent_prompt_and_queue_remain_healthy() { n3_wholly_unsent_prompt(false).await; }

async fn n3_admitted_prompt_cancel(partial: bool) {
    let mut f = Fixture::new("renewed-prompt-hold").await;
    f.thread.0.push_str(&format!("-{}", Uuid::new_v4()));
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let session = f.provider.session(&f.thread).await.unwrap();
    let prompt = Arc::new(crate::json_rpc_child::PromptWriteGate { partial, ..Default::default() });
    let cancel = Arc::new(crate::json_rpc_child::CallbackWriteGate::default());
    if partial {
        *session.child.prompt_write_gate.lock().unwrap() = Some(prompt.clone());
        *session.child.callback_write_gate.lock().unwrap() = Some((json!("unused callback owner"), cancel.clone()));
    }
    let mut events = f.provider.event_stream();
    let sent = f.command_send(json!({"thread_id":f.thread.0,"text":"N3 admitted prompt must retain frame and ACK"})).await.unwrap();
    if partial {
        tokio::time::timeout(Duration::from_secs(2), prompt.held.notified()).await.unwrap();
        prompt.release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), prompt.partial_written.notified()).await.unwrap();
    } else { wait_marker(&f.dir.path().join("wire.jsonl.ready")).await; }
    assert!(tokio::time::timeout(Duration::from_secs(2), crate::commands::agent_chat::agent_chat_interrupt_turn(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some(sent.turn_id.clone()))).await.unwrap().unwrap());
    if partial {
        tokio::time::timeout(Duration::from_secs(2), cancel.cancel_waiting.notified()).await.unwrap();
        let held_wire = std::fs::read_to_string(f.dir.path().join("wire.jsonl")).unwrap();
        assert!(!held_wire.contains("session/prompt") && !held_wire.contains("session/cancel"), "the real frame has only a prefix; cancel must queue behind its retained writer");
        assert!(!f.dir.path().join("wire.jsonl.ready").exists());
        prompt.finish_frame.notify_one();
    }
    let terminal = tokio::time::timeout(Duration::from_secs(4), async { loop {
        let event = events.next().await.unwrap();
        crate::commands::agent_chat::forward_event(f.app.handle(), event.clone());
        if let ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. } = event { if turn_id == sent.turn_id { break status; } }
    }}).await;
    let healthy = f.provider.has_session(&f.thread).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let persisted = crate::commands::agent_chat::agent_chat_list_messages(f.app.state(), f.thread.0.clone()).await.unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let wire = ownership_evidence(&f, if partial { "n3-admitted-partial" } else { "n3-admitted-ack" }, json!({"partial":partial,"terminal":format!("{terminal:?}"),"healthy":healthy,"persisted":persisted,"stored":stored}));
    assert!(matches!(terminal, Ok(TurnStatus::Error { ref subtype, .. }) if subtype == "cancelled"), "owned cancellation ACK must settle normally: {terminal:?}");
    assert!(healthy);
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/prompt").count(), 1);
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/cancel").count(), 1);
    assert!(wire.iter().position(|v| v["method"] == "session/prompt").unwrap() < wire.iter().position(|v| v["method"] == "session/cancel").unwrap());
    assert_eq!(serde_json::to_value(&stored).unwrap(), serde_json::to_value(&before).unwrap());
    assert_eq!(persisted.iter().filter(|s| serde_json::from_str::<Value>(s).unwrap()["type"] == "turn_completed").count(), 1);
}
#[tokio::test]
async fn n3_admitted_partial_frame_cancellation_retains_writer_and_ack() { n3_admitted_prompt_cancel(true).await; }
#[tokio::test]
async fn n3_admitted_complete_frame_cancellation_retains_ack() { n3_admitted_prompt_cancel(false).await; }

#[derive(Debug, Default)]
struct AdmissionCheckpoint {
    preparing: Notify,
    release: Notify,
    commits: std::sync::atomic::AtomicUsize,
    aborts: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl TurnDispatchCheckpoint for AdmissionCheckpoint {
    async fn prepare(&self) { self.preparing.notify_one(); self.release.notified().await; }
    async fn commit(&self) { self.commits.fetch_add(1, Ordering::SeqCst); }
    async fn abort(&self) { self.aborts.fetch_add(1, Ordering::SeqCst); }
}
#[tokio::test]
async fn n3_writer_denial_aborts_already_committed_checkpoint() {
    let mut f = Fixture::new("dynamic-renewed-writer").await;
    f.thread.0.push_str(&format!("-{}", Uuid::new_v4()));
    f.start(None, None).await.unwrap();
    let session = f.provider.session(&f.thread).await.unwrap();
    let prompt = Arc::new(crate::json_rpc_child::PromptWriteGate::default());
    *session.child.prompt_write_gate.lock().unwrap() = Some(prompt.clone());
    let checkpoint = Arc::new(AdmissionCheckpoint::default());
    let mut input: SendTurnInput = serde_json::from_value(json!({"thread_id":f.thread.0,"text":"N3 revoked committed checkpoint"})).unwrap();
    input.turn_checkpoint = Some(checkpoint.clone());
    let mut events = f.provider.event_stream();
    let sent = f.provider.send_turn(input).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), checkpoint.preparing.notified()).await.unwrap();
    checkpoint.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), prompt.held.notified()).await.unwrap();
    assert_eq!(checkpoint.commits.load(Ordering::SeqCst), 1);
    f.provider.interrupt_turn(f.thread.clone(), Some(sent.turn_id.clone())).await.unwrap();
    prompt.release.notify_one();
    let terminal = tokio::time::timeout(Duration::from_secs(4), async { loop {
        if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. }) = events.next().await { if turn_id == sent.turn_id { break status; } }
    }}).await;
    let healthy = f.provider.has_session(&f.thread).await;
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let wire = ownership_evidence(&f, "n3-committed-checkpoint", json!({"terminal":format!("{terminal:?}"),"healthy":healthy,"commits":checkpoint.commits.load(Ordering::SeqCst),"aborts":checkpoint.aborts.load(Ordering::SeqCst)}));
    assert!(matches!(terminal, Ok(TurnStatus::Error { ref subtype, .. }) if subtype == "cancelled"));
    assert!(healthy);
    assert_eq!(checkpoint.aborts.load(Ordering::SeqCst), 1);
    assert!(!wire.iter().any(|v| matches!(v["method"].as_str(), Some("session/prompt" | "session/cancel"))));
}
#[tokio::test]
async fn ownership_pending_control_checkpoint_yield_rechecks_cancel_before_overrides() {
    let f = Fixture::new("ownership-held-control").await;
    f.start(None, None).await.unwrap();
    let checkpoint = Arc::new(AdmissionCheckpoint::default());
    let mut input: SendTurnInput = serde_json::from_value(json!({"thread_id":f.thread.0,"text":"never run","model_override":"vendor:model [1m]","effort_override":" B effort "})).unwrap();
    input.turn_checkpoint = Some(checkpoint.clone());
    let mut events = f.provider.event_stream();
    let sent = f.provider.send_turn(input).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), checkpoint.preparing.notified()).await.unwrap();
    f.provider.interrupt_turn(f.thread.clone(), Some(sent.turn_id.clone())).await.unwrap();
    checkpoint.release.notify_one();
    let terminal = tokio::time::timeout(Duration::from_secs(1), async { loop {
        if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. }) = events.next().await {
            if turn_id == sent.turn_id { break status; }
        }
    }}).await;
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire = ownership_evidence(&f, "ownership-held-control-checkpoint", json!({"terminal":format!("{terminal:?}")}));
    assert!(matches!(terminal, Ok(TurnStatus::Error { ref subtype, .. }) if subtype == "cancelled"));
    assert_eq!(checkpoint.commits.load(Ordering::SeqCst), 0);
    assert_eq!(checkpoint.aborts.load(Ordering::SeqCst), 1);
    assert!(!wire.iter().any(|v| v["method"] == "session/prompt" || v["method"] == "session/set_config_option"));
}

async fn prompt_excludes_pending_config(cancel: bool, stop: bool) {
    let f = Fixture::new("review4-slow-model").await;
    f.start(None, None).await.unwrap();
    let provider = f.provider.clone(); let thread = f.thread.clone();
    let setter = tokio::spawn(async move { provider.set_model(thread, "vendor:model [1m]".into()).await });
    wait_marker(&f.dir.path().join("wire.jsonl.ready")).await;
    let mut events = f.provider.event_stream();
    let sent = f.provider.send_turn(serde_json::from_value(json!({"thread_id":f.thread.0,"text":"pending config prompt"})).unwrap()).await.unwrap();
    // Give the actual dispatched worker/peer a chance to reveal the race.
    // The decisive assertion below is recorded ACK/prompt wire ordering.
    tokio::time::sleep(Duration::from_millis(250)).await;
    if cancel { tokio::time::timeout(Duration::from_secs(1), f.provider.interrupt_turn(f.thread.clone(), Some(sent.turn_id.clone()))).await.unwrap().unwrap(); }
    if stop {
        tokio::time::timeout(Duration::from_secs(2), f.provider.stop_session(f.thread.clone())).await.unwrap().unwrap();
    }
    std::fs::write(f.dir.path().join("wire.jsonl.release"), b"release").unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(4), setter).await.unwrap().unwrap();
    if !stop {
        let status = tokio::time::timeout(Duration::from_secs(4), async {loop {
            if let Some(ProviderRuntimeEvent::TurnCompleted {turn_id,status,..}) = events.next().await {if turn_id == sent.turn_id {break status;}}
        }}).await.unwrap();
        if cancel { assert!(matches!(status,TurnStatus::Error {..})); } else { assert!(matches!(status,TurnStatus::Success)); }
        f.provider.stop_session(f.thread.clone()).await.unwrap();
    }
    let wire = ownership_evidence(&f, if stop { "review4-stop" } else if cancel { "review4-interrupt" } else { "review4-send" }, json!({"changed":format!("{changed:?}")}));
    if cancel || stop {
        assert!(!wire.iter().any(|v| v["method"] == "session/prompt"), "cancellation while admission waits must never start a prompt");
    } else {
        assert!(changed.is_ok());
        let ack = wire.iter().position(|v| v.get("fixture_model_ack").is_some()).unwrap();
        let prompt = wire.iter().position(|v| v["method"] == "session/prompt").unwrap();
        assert!(ack < prompt, "prompt must be excluded until model ACK; wire={wire:?}");
        assert_eq!(wire.iter().find_map(|v|v.get("fixture_prompt_model")).unwrap(), &json!("vendor:model [1m]"));
    }
}
#[tokio::test]
async fn review4_prompt_waits_for_pending_model_ack() { prompt_excludes_pending_config(false, false).await; }
#[tokio::test]
async fn review4_interrupt_remains_responsive_during_pending_model_ack() { prompt_excludes_pending_config(true, false).await; }
#[tokio::test]
async fn review4_stop_remains_responsive_during_pending_model_ack() { prompt_excludes_pending_config(false, true).await; }
async fn wait_marker(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(4), async { while !path.exists() { tokio::time::sleep(Duration::from_millis(5)).await; } }).await.unwrap();
}
async fn combined_model_effort(queued: bool, invalid_effort: bool) {
    let mode = if queued { "dynamic-review5-queue" } else { "dynamic-ids" };
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let mut events = f.provider.event_stream();
    if queued {
        f.command_send(json!({"thread_id":f.thread.0,"text":"first held prompt"})).await.unwrap();
        wait_marker(&f.dir.path().join("wire.jsonl.ready")).await;
    }
    let effort = if invalid_effort { " A effort " } else { " B effort " };
    let sent = f.command_send(json!({"thread_id":f.thread.0,"text":"combined override","model_override":"vendor:model [1m]","effort_override":effort})).await;
    if queued { std::fs::write(f.dir.path().join("wire.jsonl.release"), b"release").unwrap(); }
    let expected_completions = usize::from(queued) + usize::from(sent.is_ok());
    let mut statuses = Vec::new();
    tokio::time::timeout(Duration::from_secs(4), async { while statuses.len() < expected_completions {
        if let Some(event) = events.next().await {
            crate::commands::agent_chat::forward_event(f.app.handle(), event.clone());
            if let ProviderRuntimeEvent::TurnCompleted { status, .. } = event { statuses.push(status); }
        }
    }}).await.unwrap();
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let healthy = f.provider.has_session(&f.thread).await;
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let wire = ownership_evidence(&f, if invalid_effort { "review5-invalid" } else if queued { "review5-queued" } else { "review5-direct" }, json!({"sent":format!("{sent:?}"),"statuses":statuses,"stored":stored,"row":row}));
    assert!(sent.is_ok(), "combined effort must be deferred until acknowledged replacement model: {sent:?}");
    if queued { assert!(sent.unwrap().queued_id.is_some()); }
    assert!(healthy, "local effort rejection must not close a healthy session");
    if invalid_effort {
        assert!(matches!(statuses.last(), Some(TurnStatus::Error { .. })));
        assert_eq!(wire.iter().filter(|v| v["method"] == "session/prompt").count(), usize::from(queued), "invalid followup may not prompt after the first held turn");
        assert!(!wire.iter().any(|v| v["method"] == "session/set_config_option" && v["params"]["value"] == " A effort "));
    } else {
        assert!(statuses.iter().all(|s| matches!(s, TurnStatus::Success)));
        assert_eq!(wire.iter().filter(|v| v["method"] == "session/prompt").count(), if queued { 2 } else { 1 });
        let setters: Vec<_> = wire.iter().filter(|v| v["method"] == "session/set_config_option").collect();
        assert_eq!(setters[0]["params"]["value"], json!("vendor:model [1m]"));
        assert_eq!(setters[1]["params"]["configId"], json!("effort B"));
        assert_eq!(setters[1]["params"]["value"], json!(" B effort "));
    }
    assert_eq!(stored.catalog.current_model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(stored.catalog.config_options[1].current_value, json!(" B effort "));
    assert_eq!(row.model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(row.effort.as_deref(), Some(" B effort "));
}
#[tokio::test]
async fn review5_direct_combined_override_uses_replacement_effort_catalog() { combined_model_effort(false, false).await; }
#[tokio::test]
async fn review5_queued_combined_override_uses_dispatch_effort_catalog() { combined_model_effort(true, false).await; }
#[tokio::test]
async fn review5_invalid_replacement_effort_never_prompts_or_persists_invalid_value() { combined_model_effort(false, true).await; }
#[tokio::test]
async fn ownership_pending_control_command_queue_invalid_effort_uses_ack_membership() { combined_model_effort(true, true).await; }
async fn synthetic_turn(f: &Fixture) -> TurnStatus {
    let mut events = f.provider.event_stream();
    let turn = f.provider.send_turn(serde_json::from_value(json!({"thread_id":f.thread.0,"text":"synthetic mode update"})).unwrap()).await.unwrap().turn_id;
    tokio::time::timeout(Duration::from_secs(4), async { loop {
        if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. }) = events.next().await { if turn_id == turn { break status; } }
    }}).await.unwrap()
}
async fn mode_order_and_recovery(mode: &str) {
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    crate::commands::custom_acp::acp_set_config(f.app.handle().clone(), f.app.state(), f.app.state(), f.thread.0.clone(), "opaque mode selector".into(), json!(" A mode ")).await.unwrap();
    let expected = if mode == "review2-cold" {
        assert!(matches!(synthetic_turn(&f).await, TurnStatus::Success)); " B mode "
    } else {
        crate::commands::custom_acp::acp_set_config(f.app.handle().clone(), f.app.state(), f.app.state(), f.thread.0.clone(), "opaque mode selector".into(), json!(" C mode ")).await.unwrap();
        if mode == "review2-before" { " C mode " } else { " D mode " }
    };
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let live = crate::commands::custom_acp::acp_thread_catalog(f.app.state(), f.app.state(), f.thread.0.clone()).await.unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let resumed = ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread).await;
    let restored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    ownership_evidence(&f, mode, json!({"stored":stored,"live":live,"restored":restored,"resume":format!("{resumed:?}")}));
    assert!(resumed.is_ok());
    assert!(live.live);
    assert_eq!(stored.catalog.config_options[0].current_value, json!(expected), "wire-latest mode must win");
    assert_eq!(stored.config_values["opaque mode selector"], json!(expected), "mode notification must reconcile persisted intent");
    assert_eq!(serde_json::to_value(live.catalog).unwrap(), serde_json::to_value(stored.catalog).unwrap());
    assert_eq!(restored.catalog.config_options[0].current_value, json!(expected), "cold recovery must not undo unsolicited mode");
    assert_eq!(restored.session_id, stored.session_id);
}
#[tokio::test]
async fn review2_mode_notification_reconciles_intent_on_cold_resume() { mode_order_and_recovery("review2-cold").await; }
#[tokio::test]
async fn review2_mode_after_ack_wins_live_and_cold() { mode_order_and_recovery("review2-after").await; }
#[tokio::test]
async fn review2_mode_before_ack_yields_to_newer_ack() { mode_order_and_recovery("review2-before").await; }
#[tokio::test]
async fn review2_startup_mode_after_response_survives_identity_install() {
    let f = Fixture::new("review2-startup").await;
    f.start(None, None).await.unwrap();
    let b = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    ownership_evidence(&f, "review2-startup", json!({"binding":b}));
    assert!(b.config_values.is_empty());
    assert_eq!(b.catalog.config_options[0].current_value, json!(" B mode "));
}
#[tokio::test]
async fn review2_unknown_mode_fails_closed_without_durable_mutation() {
    let f = Fixture::new("review2-invalid").await;
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let status = synthetic_turn(&f).await;
    let live = f.provider.has_session(&f.thread).await;
    let after = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    ownership_evidence(&f, "review2-invalid", json!({"before":before,"after":after,"live":live}));
    assert!(matches!(status, TurnStatus::Error { .. }), "unadvertised opaque mode must fail closed");
    assert!(!live);
    assert_eq!(serde_json::to_value(before).unwrap(), serde_json::to_value(after).unwrap());
}
#[tokio::test]
async fn review1_segment_completion_retains_whole_turn_output_limits() {
    for mode in ["review1-budget-text", "review1-budget-thought"] {
        let f = Fixture::new(mode).await;
        f.start(None, None).await.unwrap();
        let status = synthetic_turn(&f).await;
        f.provider.stop_session(f.thread.clone()).await.unwrap();
        ownership_evidence(&f, mode, json!({"status":status}));
        assert!(matches!(status, TurnStatus::Error { ref message, .. } if message.contains("assistant output limit exceeded")), "segment boundaries must not bypass the per-kind whole-turn cap: {status:?}");
    }
}
async fn transcript_segments(mode: &str) {
    let f = Fixture::new(mode).await;
    f.app.manage(crate::commands::agent_chat::SubagentTracker::default());
    f.app.manage(crate::commands::agent_chat::RunActivityTracker::default());
    f.app.manage(crate::commands::agent_chat::AgentChatChannelRegistry::default());
    f.start(None, None).await.unwrap();
    let mut events = f.provider.event_stream();
    let turn = f.provider.send_turn(serde_json::from_value(json!({"thread_id":f.thread.0,"text":"synthetic ordered transcript"})).unwrap()).await.unwrap().turn_id;
    let mut observed = Vec::new();
    let status = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let event = events.next().await.unwrap();
            crate::commands::agent_chat::forward_event(f.app.handle(), event.clone());
            if let ProviderRuntimeEvent::ItemCompleted { item, .. } = &event { observed.push(serde_json::to_value(item).unwrap()); }
            if let ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. } = event { if turn_id == turn { break status; } }
        }
    }).await;
    let persisted = crate::commands::agent_chat::agent_chat_list_messages(f.app.state(), f.thread.0.clone()).await.unwrap();
    let tail = crate::commands::agent_chat::agent_chat_list_messages_tail(f.app.state(), f.thread.0.clone(), 100).await.unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    ownership_evidence(&f, mode, json!({"items":observed,"persisted":persisted,"tail_total":tail.total_rows}));
    let mut expected = vec![json!({"kind":"assistant_text","text":"Before."}), json!({"kind":"tool_use","tool_name":"Synthetic tool","input":{},"tool_use_id":"ordered-tool"}),json!({"kind":"tool_result","tool_use_id":"ordered-tool","content":"result","is_error":false}), json!({"kind":"assistant_text","text":"After."})];
    if mode == "review1-alternating" { expected.extend([json!({"kind":"assistant_thinking","text":"Thought1."}),json!({"kind":"assistant_text","text":"Text2."}),json!({"kind":"assistant_thinking","text":"Thought2."})]); }
    assert!(matches!(status, Ok(TurnStatus::Success)));
    assert_eq!(observed, expected, "live completions must preserve actual content/tool boundaries");
    let hydrated: Vec<Value> = persisted.iter().map(|s| serde_json::from_str::<Value>(s).unwrap()).filter(|v| v["type"] == "item_completed").map(|v|v["item"].clone()).collect();
    assert_eq!(hydrated, expected, "actual bridge/database cold replay must keep identical item ordering");
    assert!(tail.complete);
    assert_eq!(tail.total_rows as usize, persisted.len());
    assert_eq!(tail.rows.len(), persisted.len());
    assert!(tail.rows.windows(2).all(|w| w[0].id < w[1].id));
}
#[tokio::test]
async fn review1_text_tool_text_live_completion_matches_db_replay() { transcript_segments("review1-tools").await; }
#[tokio::test]
async fn review1_alternating_thought_text_live_completion_matches_db_replay() { transcript_segments("review1-alternating").await; }
fn edit_definition(f: &Fixture, disable: bool) {
    let db = f.app.state::<DatabaseStore>();
    let old = db.acp_agents().unwrap().into_iter().find(|a| a.id == f.agent).unwrap();
    let mut args = old.args.clone(); if !disable { args.push("changed literal argument".into()); }
    db.save_acp_agent(super::super::AcpAgentInput { id: Some(old.id), name: old.name, executable: old.executable, args, environment: old.environment, enabled: !disable, auth_method: old.auth_method }).unwrap();
}
async fn repeat_live_start_after_edit(disable: bool) {
    let f = Fixture::new("resume").await;
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    edit_definition(&f, disable);
    let repeated = f.start(None, None).await;
    let live = crate::commands::custom_acp::acp_thread_catalog(f.app.state(), f.app.state(), f.thread.0.clone()).await.unwrap();
    let after = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let cold = f.start(None, None).await;
    let wire = ownership_evidence(&f, if disable { "review6-disabled" } else { "review6-edited" }, json!({"repeat":format!("{repeated:?}"),"cold":format!("{cold:?}"),"live":live,"before":before,"after":after}));
    assert!(repeated.is_ok(), "exact existing live binding must be idempotent after edit/disable: {repeated:?}");
    assert!(live.live);
    assert_eq!(serde_json::to_value(before).unwrap(), serde_json::to_value(after).unwrap());
    assert!(cold.is_err(), "cold launch must still reject changed/disabled definition");
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/new").count(), 1);
    assert_eq!(wire.iter().filter(|v| v.get("launch_argv").is_some()).count(), 1);
}
#[tokio::test]
async fn review6_live_repeat_start_after_launch_edit() { repeat_live_start_after_edit(false).await; }
#[tokio::test]
async fn review6_live_repeat_start_after_disable() { repeat_live_start_after_edit(true).await; }
#[tokio::test]
async fn review6_live_repeat_start_rejects_other_instance() {
    let mut f = Fixture::new("resume").await;
    f.start(None, None).await.unwrap();
    let original = f.agent.clone(); f.agent = Uuid::new_v4().to_string();
    let repeated = f.start(None, None).await;
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    assert!(repeated.unwrap_err().contains("another ACP instance"));
    assert_eq!(f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap().agent_id, original);
}
async fn renewed_startup_wire_mode(mode: &str, cold: bool, valid: bool) {
    let f = Fixture::new(mode).await;
    let mut events = f.provider.event_stream();
    let started = if cold {
        f.start(None, None).await.unwrap();
        let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
        assert!(before.config_values.is_empty());
        f.provider.stop_session(f.thread.clone()).await.unwrap();
        ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread).await
    } else { f.start(None, None).await.map(|_| ()) };
    let live = f.provider.has_session(&f.thread).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap();
    let row = started.as_ref().ok().map(|_| f.row());
    let catalog = if live { f.provider.live_catalog(&f.thread).await } else { None };
    use futures_util::FutureExt;
    let mut history = false;
    while let Some(Some(event)) = events.next().now_or_never() {
        history |= matches!(event, ProviderRuntimeEvent::ContentDelta { .. } | ProviderRuntimeEvent::ItemCompleted { .. } | ProviderRuntimeEvent::RequestOpened { .. });
    }
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire = ownership_evidence(&f, mode, json!({"started":format!("{started:?}"),"live":live,"stored":stored,"catalog":catalog,"row":row}));
    assert!(wire.iter().any(|v| v["id"] == "ownership barrier" && v["error"]["code"] == -32601));
    assert!(!wire.iter().any(|v| v["method"] == "session/set_config_option" || v["method"] == "session/set_model"));
    assert!(!history);
    if valid {
        assert!(started.is_ok(), "wire-valid Y followed by retiring response Z must remain healthy: {started:?}");
        assert!(live);
        let stored = stored.unwrap();
        assert_eq!(stored.session_id.as_deref(), Some("identical native/session ID"));
        assert!(stored.config_values.is_empty());
        assert_eq!(stored.catalog.config_options[0].current_value, json!(" Z mode "));
        assert_eq!(stored.catalog.config_options[0].options.len(), 1);
        assert_eq!(serde_json::to_value(&stored.catalog).unwrap(), serde_json::to_value(catalog.unwrap()).unwrap());
        assert!(row.unwrap().model.is_none());
    } else {
        assert!(started.is_err(), "matching malformed/unadvertised older mode must still fail closed");
        assert!(!live);
        if cold { assert!(stored.unwrap().catalog.config_options.is_empty()); } else { assert!(stored.is_none()); }
    }
}
#[derive(Default)]
pub(super) struct StartupModeGate { pub entered: Notify, pub release: Notify }
thread_local! {
    pub(super) static STARTUP_MODE_GATE: std::cell::RefCell<Option<Arc<StartupModeGate>>> = const { std::cell::RefCell::new(None) };
}
#[tokio::test]
async fn renewed_n1_startup_later_catalog_cannot_overtake_mode_replay() {
    let mode = "renewed-startup-new-replay";
    let f = Fixture::new(mode).await;
    let gate = Arc::new(StartupModeGate::default());
    STARTUP_MODE_GATE.with(|slot| *slot.borrow_mut() = Some(gate.clone()));
    let started = f.start(None, None);
    tokio::pin!(started);
    tokio::time::timeout(Duration::from_secs(4), async {
        tokio::select! {
            _ = gate.entered.notified() => {},
            result = &mut started => panic!("startup must reach replay gate before settling: {result:?}"),
        }
    }).await.unwrap();
    STARTUP_MODE_GATE.with(|slot| *slot.borrow_mut() = None);
    std::fs::write(f.dir.path().join("wire.jsonl.later"), b"release later catalog").unwrap();
    wait_marker(&f.dir.path().join("wire.jsonl.later-ready")).await;
    gate.release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(4), &mut started).await.unwrap();
    let live = f.provider.has_session(&f.thread).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    ownership_evidence(&f, mode, json!({"start":format!("{result:?}"),"live":live,"stored":stored}));
    assert!(result.is_ok(), "later catalog must queue behind wire-valid replay mode: {result:?}");
    assert!(live);
    assert_eq!(stored.unwrap().catalog.config_options[0].current_value, json!(" Z mode "));
}
#[tokio::test]
async fn renewed_n1_startup_cold_resume_mode_uses_wire_membership() { renewed_startup_wire_mode("renewed-startup-cold-resume", true, true).await; }
#[tokio::test]
async fn renewed_n1_startup_cold_load_mode_uses_wire_membership() { renewed_startup_wire_mode("renewed-startup-cold-load", true, true).await; }
#[tokio::test]
async fn renewed_n1_startup_new_mode_uses_wire_membership() { renewed_startup_wire_mode("renewed-startup-new-resume", false, true).await; }
#[tokio::test]
async fn renewed_n1_startup_old_malformed_mode_still_fails_closed() { renewed_startup_wire_mode("renewed-startup-malformed-load", true, false).await; }
#[tokio::test]
async fn renewed_n1_startup_old_unadvertised_mode_still_fails_closed() { renewed_startup_wire_mode("renewed-startup-invalid-resume", true, false).await; }

async fn startup_wire_latest(mode: &str, cold: bool) {
    let f = Fixture::new(mode).await;
    let mut events = f.provider.event_stream();
    f.start(None, None).await.unwrap();
    if cold {
        let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
        assert!(before.config_values.is_empty(), "cold resume must start with empty intent");
        assert!(before.catalog.current_model.is_none());
        assert!(f.row().model.is_none());
        f.provider.stop_session(f.thread.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(4), ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread)).await.unwrap().unwrap();
    }
    let live = crate::commands::custom_acp::acp_thread_catalog(f.app.state(), f.app.state(), f.thread.0.clone()).await.unwrap();
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let history = {
        use futures_util::FutureExt;
        let mut replayed = false;
        while let Some(Some(event)) = events.next().now_or_never() {
            replayed |= matches!(event, ProviderRuntimeEvent::ContentDelta { .. } | ProviderRuntimeEvent::ItemCompleted { .. });
        }
        replayed
    };
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let wire: Vec<Value> = std::fs::read_to_string(f.dir.path().join("wire.jsonl")).unwrap().lines().map(|s| serde_json::from_str(s).unwrap()).collect();
    assert!(!wire.iter().any(|v| v["method"] == "session/set_config_option" || v["method"] == "session/set_model"), "no explicit override or persisted intent may mask startup ordering");
    assert!(!history, "load history must remain suppressed");
    assert!(live.live);
    assert_eq!(stored.session_id.as_deref(), Some("identical native/session ID"));
    assert_eq!(live.catalog.config_options.iter().find(|o| o.id == "startup flag").unwrap().current_value, json!(true), "wire-later startup update D must survive C");
    assert_eq!(serde_json::to_value(&live.catalog).unwrap(), serde_json::to_value(&stored.catalog).unwrap(), "live and actual durable catalog must agree");
    assert_eq!(row.model.as_deref(), Some(" D model "), "actual shared start/resume DB projection must use D");
    assert_eq!(row.effort.as_deref(), Some(" D effort "));
    if cold {
        let method = if mode.ends_with("-load") { "session/load" } else { "session/resume" };
        let resume = wire.iter().find(|v| v["method"] == method).unwrap();
        assert_eq!(resume["params"]["sessionId"], json!("identical native/session ID"));
        assert_eq!(wire.iter().filter(|v| v["method"] == "session/new").count(), 1);
    }
}
#[tokio::test]
async fn final_startup_new_resume_caps_keeps_wire_latest() { startup_wire_latest("final-startup-new-resume", false).await; }
#[tokio::test]
async fn final_startup_new_load_caps_keeps_wire_latest() { startup_wire_latest("final-startup-new-load", false).await; }
#[tokio::test]
async fn final_startup_cold_resume_keeps_wire_latest() { startup_wire_latest("final-startup-cold-resume", true).await; }
#[tokio::test]
async fn final_startup_cold_load_keeps_wire_latest() { startup_wire_latest("final-startup-cold-load", true).await; }

// Failure-safe cleanup is exact-PID and limited to our credential-free peers.
impl Drop for Fixture {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Ok(log) = std::fs::read_to_string(self.dir.path().join("wire.jsonl")) {
            for entry in log.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
                if let Some(pid) = entry["owned_pid"].as_i64() {
                    let proc = PathBuf::from(format!("/proc/{pid}"));
                    let env = std::fs::read(proc.join("environ")).unwrap_or_default();
                    let owned = format!("FAKE_ACP_LOG={}", self.dir.path().join("wire.jsonl").display());
                    if env.split(|b| *b == 0).any(|v| v == owned.as_bytes()) {
                        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL); }
                    }
                }
            }
        }
    }
}
fn ownership_evidence(f: &Fixture, mode: &str, observed: Value) -> Vec<Value> {
    let wire = std::fs::read_to_string(f.dir.path().join("wire.jsonl")).unwrap();
    let messages: Vec<Value> = wire.lines().map(|s| serde_json::from_str(s).unwrap()).collect();
    #[cfg(target_os = "linux")]
    for pid in messages.iter().filter_map(|v| v["owned_pid"].as_u64()) {
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists(), "owned fixture PID {pid} must be reaped before assertions");
    }
    if let Some(root) = std::env::var_os("CODEMUX_ACP_TEST_EVIDENCE_DIR") {
        let root = PathBuf::from(root);
        assert!(root.is_absolute(), "test evidence root must be an explicit absolute path");
        let path = root.join(format!("{mode}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("wire.jsonl"), &wire).unwrap();
        std::fs::write(path.join("observed.json"), serde_json::to_vec_pretty(&observed).unwrap()).unwrap();
    }
    messages
}
async fn ownership_sparse_omitted_fields(mode: &str, model_change: bool) {
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    assert_eq!(before.catalog.current_model.as_deref(), Some("default"));
    assert_eq!(f.row().effort.as_deref(), Some(" E0 "));
    let changed = tokio::time::timeout(Duration::from_secs(4), async {
        if model_change {
            crate::commands::agent_chat::agent_chat_set_model(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some("vendor:model [1m]".into())).await
        } else {
            crate::commands::custom_acp::acp_set_config(f.app.handle().clone(), f.app.state(), f.app.state(), f.thread.0.clone(), "owned effort".into(), json!(" E1 ")).await.map(|_| ())
        }
    }).await.unwrap();
    let live = crate::commands::custom_acp::acp_thread_catalog(f.app.state(), f.app.state(), f.thread.0.clone()).await.unwrap();
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let resumed = tokio::time::timeout(Duration::from_secs(4), ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread)).await;
    let restored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let restored_row = f.row();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire = ownership_evidence(&f, mode, json!({"changed":format!("{changed:?}"),"resume":format!("{resumed:?}"),"live":live,"stored":stored,"row":row,"restored":restored,"restored_row":restored_row}));
    let ack = wire.iter().find_map(|v| v.get("fixture_ack")).unwrap();
    assert!(ack.get(if model_change { "configOptions" } else { "models" }).is_none(), "the tested field group must really be omitted");
    assert!(wire.iter().any(|v| v["id"] == "ownership barrier" && v["error"]["code"] == -32601), "D must be consumed before peer releases C");
    assert!(changed.is_ok(), "valid inverse sparse ACK must succeed: {changed:?}");
    assert!(live.live);
    assert_eq!(serde_json::to_value(&live.catalog).unwrap(), serde_json::to_value(&stored.catalog).unwrap());
    assert_eq!(stored.catalog.current_model.as_deref(), Some("vendor:model [1m]"), "config ACK omitting models must retain D's model");
    assert_eq!(stored.config_values["owned effort"], json!(" E1 "), "model ACK omitting controls must retain D's effort");
    assert_eq!(row.model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(row.effort.as_deref(), Some(" E1 "));
    assert!(matches!(resumed, Ok(Ok(()))), "accepted state must cold-recover without old restoration: {resumed:?}");
    assert_eq!(restored.session_id, before.session_id);
    assert_eq!(restored.revision, before.revision);
    assert_eq!(restored.catalog.current_model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(restored.config_values["owned effort"], json!(" E1 "));
    assert_eq!(restored_row.model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(restored_row.effort.as_deref(), Some(" E1 "));
    assert!(!wire.iter().any(|v| v["method"] == "session/set_config_option" && v["params"]["value"] == " E0 "), "no obsolete effort restoration");
    assert!(!wire.iter().any(|v| v["method"] == "session/set_model" && v["params"]["modelId"] == "default"), "no obsolete model restoration");
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/new").count(), 1);
    let method = if mode.ends_with("-load") { "session/load" } else { "session/resume" };
    assert_eq!(wire.iter().find(|v| v["method"] == method).unwrap()["params"]["sessionId"], json!("identical native/session ID"));
}
#[tokio::test]
async fn ownership_sparse_model_ack_preserves_effort_cold_resume() { ownership_sparse_omitted_fields("ownership-sparse-model-resume", true).await; }
#[tokio::test]
async fn ownership_sparse_model_ack_preserves_effort_cold_load() { ownership_sparse_omitted_fields("ownership-sparse-model-load", true).await; }
#[tokio::test]
async fn ownership_sparse_config_ack_preserves_model_cold_resume() { ownership_sparse_omitted_fields("ownership-sparse-config-resume", false).await; }
#[tokio::test]
async fn ownership_sparse_config_ack_preserves_model_cold_load() { ownership_sparse_omitted_fields("ownership-sparse-config-load", false).await; }
#[tokio::test]
async fn ownership_sparse_newer_ack_cannot_install_retired_model() {
    let mode = "ownership-sparse-retired-resume";
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let changed = crate::commands::agent_chat::agent_chat_set_model(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some("vendor:model [1m]".into())).await;
    let alive = f.provider.has_session(&f.thread).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    ownership_evidence(&f, mode, json!({"changed":format!("{changed:?}"),"alive":alive,"stored":stored,"row":row}));
    assert!(changed.is_err(), "a newer sparse ACK of B after D retired B is a protocol contradiction");
    assert!(!alive);
    assert_eq!(stored.catalog.current_model.as_deref(), Some(" D model "));
    assert_eq!(row.model.as_deref(), Some(" D model "));
    assert_eq!(row.effort.as_deref(), Some(" E1 "));
}

async fn ownership_queue_startup(mode: &str, accepts_foreign: bool) {
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    assert!(before.config_values.is_empty(), "no restoration intent may mask startup ordering");
    assert!(before.catalog.current_model.is_none());
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let mut events = f.provider.event_stream();
    let resumed = tokio::time::timeout(Duration::from_secs(4), ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread)).await;
    let live = crate::commands::custom_acp::acp_thread_catalog(f.app.state(), f.app.state(), f.thread.0.clone()).await.unwrap();
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let history = {
        use futures_util::FutureExt;
        let mut history = false;
        while let Some(Some(event)) = events.next().now_or_never() {
            history |= matches!(event, ProviderRuntimeEvent::ContentDelta { .. } | ProviderRuntimeEvent::ItemCompleted { .. } | ProviderRuntimeEvent::RequestOpened { .. });
        }
        history
    };
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire = ownership_evidence(&f, mode, json!({"resume":format!("{resumed:?}"),"live":live,"stored":stored,"row":row,"history":history}));
    let burst = wire.iter().find_map(|v| v.get("fixture_burst")).unwrap().as_array().unwrap();
    assert_eq!(stored.session_id, before.session_id);
    assert_eq!(stored.revision, before.revision);
    assert!(!history, "load history and inactive permission prompts must remain suppressed");
    assert!(!wire.iter().any(|v| v["method"] == "session/set_config_option" || v["method"] == "session/set_model"), "cold input and intent must be empty");
    let method = if mode.ends_with("-load") { "session/load" } else { "session/resume" };
    assert_eq!(wire.iter().find(|v| v["method"] == method).unwrap()["params"]["sessionId"], json!("identical native/session ID"));
    if accepts_foreign {
        assert_eq!(burst.iter().filter(|v| v["params"]["sessionId"] == "foreign native ID").count(), 65);
        assert_eq!(burst.iter().filter(|v| v["params"].get("sessionId").is_none()).count(), 1);
        assert_eq!(burst[0]["params"]["sessionId"], json!("identical native/session ID"));
        assert!(matches!(resumed, Ok(Ok(()))), "known-foreign startup traffic must not spend matching owner's budget: {resumed:?}");
        assert!(wire.iter().any(|v| v["id"] == "ownership barrier" && v["error"]["code"] == -32601), "callback must be serviced while startup RPC is pending");
        assert!(live.live);
        assert_eq!(serde_json::to_value(&live.catalog).unwrap(), serde_json::to_value(&stored.catalog).unwrap());
        assert_eq!(stored.catalog.config_options.iter().find(|o| o.id == "startup flag").unwrap().current_value, json!(true));
        assert_eq!(row.model.as_deref(), Some(" D model "));
        assert_eq!(row.effort.as_deref(), Some(" D effort "));
    } else {
        if !mode.contains("malformed") { assert_eq!(burst.len(), 65); }
        assert!(matches!(resumed, Ok(Err(_))), "matching malformed/overflow input must fail closed: {resumed:?}");
        assert!(!live.live);
        assert_eq!(serde_json::to_value(&stored).unwrap(), serde_json::to_value(&before).unwrap());
        assert!(row.model.is_none());
        assert!(row.effort.is_none());
    }
}
#[tokio::test]
async fn ownership_queue_known_foreign_burst_cold_resume() { ownership_queue_startup("ownership-queue-foreign-resume", true).await; }
#[tokio::test]
async fn ownership_queue_known_foreign_burst_cold_load() { ownership_queue_startup("ownership-queue-foreign-load", true).await; }
#[tokio::test]
async fn ownership_queue_matching_overflow_cold_resume() { ownership_queue_startup("ownership-queue-matching-resume", false).await; }
#[tokio::test]
async fn ownership_queue_matching_overflow_cold_load() { ownership_queue_startup("ownership-queue-matching-load", false).await; }
#[tokio::test]
async fn ownership_queue_matching_malformed_cold_resume() { ownership_queue_startup("ownership-queue-malformed-resume", false).await; }
#[tokio::test]
async fn ownership_queue_matching_malformed_cold_load() { ownership_queue_startup("ownership-queue-malformed-load", false).await; }
#[tokio::test]
async fn ownership_queue_unknown_new_identity_remains_bounded() {
    let mode = "ownership-queue-unknown-resume";
    let f = Fixture::new(mode).await;
    let started = tokio::time::timeout(Duration::from_secs(4), f.start(None, None)).await;
    let alive = f.provider.has_session(&f.thread).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire = ownership_evidence(&f, mode, json!({"start":format!("{started:?}"),"alive":alive,"stored":stored}));
    assert_eq!(wire.iter().find_map(|v| v.get("fixture_burst")).unwrap().as_array().unwrap().len(), 65);
    assert!(matches!(started, Ok(Err(_))), "unknown new-session queue must remain bounded: {started:?}");
    assert!(!alive);
    assert!(stored.is_none());
}

async fn coupled_mode_model_ack(mode: &str) {
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    crate::commands::custom_acp::acp_set_config(f.app.handle().clone(), f.app.state(), f.app.state(), f.thread.0.clone(), "coupled mode".into(), json!(" A mode ")).await.unwrap();
    let changed = crate::commands::agent_chat::agent_chat_set_model(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some("vendor:model [1m]".into())).await;
    let live = crate::commands::custom_acp::acp_thread_catalog(f.app.state(), f.app.state(), f.thread.0.clone()).await.unwrap();
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let resumed = tokio::time::timeout(Duration::from_secs(4), ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread)).await;
    let restored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire = ownership_evidence(&f, mode, json!({"changed":format!("{changed:?}"),"live":live,"stored":stored,"row":row,"resumed":format!("{resumed:?}"),"restored":restored}));
    let pair = wire.iter().find_map(|v| v.get("fixture_wire")).unwrap();
    assert_eq!(pair.as_array().unwrap().len(), 2);
    assert_eq!(pair[usize::from(mode.contains("before"))]["result"]["configOptions"].as_array().unwrap().iter().find(|o| o["category"] == "thought_level").unwrap()["id"], json!("effort B"));
    assert!(changed.is_ok(), "valid model/effort ACK with mode-only D must succeed: {changed:?}");
    assert!(live.live, "newly-advertised mode D must not close healthy peer");
    assert_eq!(stored.catalog.current_model.as_deref(), Some("vendor:model [1m]"), "mode-only D must not discard C's unrelated model ACK");
    assert_eq!(row.model.as_deref(), Some("vendor:model [1m]"));
    assert_eq!(row.effort.as_deref(), Some(" B effort "));
    assert_eq!(stored.catalog.config_options.iter().find(|o| o.category.as_deref() == Some("thought_level")).unwrap().id, "effort B");
    let expected = if mode.contains("before") { " A mode " } else { " Y mode " };
    assert_eq!(stored.config_values["coupled mode"], json!(expected));
    assert_eq!(serde_json::to_value(&live.catalog).unwrap(), serde_json::to_value(&stored.catalog).unwrap());
    assert!(matches!(resumed, Ok(Ok(()))), "wire-effective catalog must cold-recover: {resumed:?}");
    assert_eq!(serde_json::to_value(&restored.catalog).unwrap(), serde_json::to_value(&stored.catalog).unwrap());
    assert_eq!(restored.session_id, stored.session_id);
}
#[tokio::test]
async fn ownership_coupled_mode_after_config_ack_preserves_model_effort() { coupled_mode_model_ack("ownership-coupled-config-after").await; }

#[tokio::test]
async fn ownership_coupled_new_mode_after_config_ack_uses_ack_membership() { coupled_mode_model_ack("ownership-coupled-config-after-new-value").await; }
#[tokio::test]
async fn ownership_coupled_mode_before_config_ack_yields_to_newer_mode_model_effort() { coupled_mode_model_ack("ownership-coupled-config-before").await; }
#[tokio::test]
async fn ownership_coupled_mode_after_legacy_ack_preserves_model_effort() { coupled_mode_model_ack("ownership-coupled-legacy-after").await; }
#[tokio::test]
async fn ownership_coupled_new_mode_after_legacy_ack_uses_ack_membership() { coupled_mode_model_ack("ownership-coupled-legacy-after-new-value").await; }
#[tokio::test]
async fn ownership_coupled_mode_before_legacy_ack_yields_to_newer_mode_model_effort() { coupled_mode_model_ack("ownership-coupled-legacy-before").await; }

#[tokio::test]
async fn ownership_coupled_foreign_mode_burst_does_not_spend_mutation_budget() { coupled_mode_model_ack("ownership-coupled-config-after-new-value-foreign").await; }
#[tokio::test]
async fn ownership_coupled_matching_mode_mutation_budget_remains_bounded() {
    let mode = "ownership-coupled-config-after-overflow";
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(4), crate::commands::agent_chat::agent_chat_set_model(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some("vendor:model [1m]".into()))).await;
    let live = f.provider.has_session(&f.thread).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire = ownership_evidence(&f, mode, json!({"changed":format!("{changed:?}"),"live":live,"stored":stored}));
    assert_eq!(wire.iter().find_map(|v| v.get("fixture_burst")).unwrap().as_array().unwrap().len(), 65);
    assert!(matches!(changed, Ok(Err(_))), "matching mutation traffic remains bounded, even while ACK is withheld: {changed:?}");
    assert!(!live);
    assert_eq!(serde_json::to_value(&stored).unwrap(), serde_json::to_value(&before).unwrap());
}
async fn coupled_invalid_matching_mode(mode: &str) {
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let changed = crate::commands::agent_chat::agent_chat_set_model(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some("vendor:model [1m]".into())).await;
    let live = f.provider.has_session(&f.thread).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    ownership_evidence(&f, mode, json!({"changed":format!("{changed:?}"),"live":live,"stored":stored}));
    assert!(changed.is_err(), "a matching malformed/unadvertised mode must fail closed");
    assert!(!live);
    assert_eq!(stored.catalog.config_options.iter().find(|o| o.category.as_deref() == Some("mode")).unwrap().current_value, json!(" A mode "), "invalid mode must never mutate acknowledged state");
    assert!(!stored.config_values.values().any(|v| v == &json!(" unadvertised mode ") || v == &json!(true)));
}
#[tokio::test]
async fn ownership_coupled_matching_unadvertised_mode_after_ack_fails_closed() { coupled_invalid_matching_mode("ownership-coupled-config-after-invalid").await; }
#[tokio::test]
async fn ownership_coupled_matching_malformed_mode_after_ack_fails_closed() { coupled_invalid_matching_mode("ownership-coupled-legacy-after-malformed").await; }

async fn sparse_ack_retiring_model(mode: &str) {
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let before = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    assert!(before.catalog.config_options.iter().all(|o| o.category.as_deref() != Some("model")));
    assert!(before.catalog.capabilities.models.iter().any(|m| m.id == "vendor:model [1m]"));
    let changed = tokio::time::timeout(Duration::from_secs(4), crate::commands::agent_chat::agent_chat_set_model(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some("vendor:model [1m]".into()))).await.unwrap();
    let alive = f.provider.has_session(&f.thread).await;
    let live = crate::commands::custom_acp::acp_thread_catalog(f.app.state(), f.app.state(), f.thread.0.clone()).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let resumed = tokio::time::timeout(Duration::from_secs(4), ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread)).await;
    let restored = f.provider.catalog(&f.thread).await.unwrap();
    let restored_row = f.row();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    let wire: Vec<Value> = std::fs::read_to_string(f.dir.path().join("wire.jsonl")).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let messages = wire.iter().find_map(|v| v.get("fixture_wire")).unwrap();
    assert_eq!(messages[0]["result"], json!({}), "the actual child must return a sparse legacy ACK");
    assert_eq!(messages[1]["params"]["update"]["models"]["availableModels"], json!([{"modelId":" D model ","name":" D model "}]), "D must retire the requested B, not leave it available");
    assert!(changed.is_ok(), "valid sparse ACK must not be validated against wire-later D-only membership: {changed:?}");
    assert!(alive, "a healthy child must remain live");
    let live = live.unwrap();
    assert!(live.live);
    assert_eq!(serde_json::to_value(&live.catalog).unwrap(), serde_json::to_value(&stored.catalog).unwrap());
    assert_eq!(stored.session_id, before.session_id);
    assert_eq!(stored.revision, before.revision);
    assert_eq!(stored.catalog.current_model.as_deref(), Some(" D model "));
    assert_eq!(stored.catalog.capabilities.models.len(), 1);
    assert_eq!(row.model.as_deref(), Some(" D model "));
    assert_eq!(row.effort.as_deref(), Some(" D effort "));
    assert!(matches!(resumed, Ok(Ok(()))), "accepted D must cold-resume: {resumed:?}");
    assert_eq!(restored.current_model.as_deref(), Some(" D model "));
    assert_eq!(restored_row.model.as_deref(), Some(" D model "));
    assert_eq!(restored_row.effort.as_deref(), Some(" D effort "));
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/new").count(), 1);
    let method = if mode.ends_with("-load") { "session/load" } else { "session/resume" };
    assert_eq!(wire.iter().find(|v| v["method"] == method).unwrap()["params"]["sessionId"], json!("identical native/session ID"));
    assert_eq!(wire.iter().filter(|v| v["method"] == "session/set_model" && v["params"]["modelId"] == "vendor:model [1m]").count(), 1, "cold recovery must not restore retired B");
}
#[tokio::test]
async fn final_sparse_ack_retirement_cold_resume() { sparse_ack_retiring_model("final-sparse-resume").await; }
#[tokio::test]
async fn final_sparse_ack_retirement_cold_load() { sparse_ack_retiring_model("final-sparse-load").await; }

async fn sparse_invalid_catalog_fails(mode: &str) {
    let f = Fixture::new(mode).await;
    f.start(None, None).await.unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(4), crate::commands::agent_chat::agent_chat_set_model(f.app.handle().clone(), ProviderKind::Acp, f.thread.clone(), Some("vendor:model [1m]".into()))).await.unwrap();
    let alive = f.provider.has_session(&f.thread).await;
    let stored = f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    let row = f.row();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    assert!(changed.is_err(), "intrinsically invalid ACK or later update must fail closed");
    assert!(!alive, "invalid protocol must not keep a healthy binding");
    assert!(!stored.catalog.current_model.as_deref().unwrap().starts_with(" unadvertised"));
    assert!(!row.model.as_deref().unwrap().starts_with(" unadvertised"));
}
#[tokio::test]
async fn final_sparse_invalid_ack_is_checked_even_after_valid_later_update() { sparse_invalid_catalog_fails("final-sparse-invalid-ack").await; }
#[tokio::test]
async fn final_sparse_valid_full_ack_does_not_mask_invalid_later_update() { sparse_invalid_catalog_fails("final-sparse-invalid-update").await; }
#[tokio::test]
async fn rereview_start_command_persists_acknowledged_model_and_effort() {
    let f=Fixture::new("ack-start").await;
    let started=f.start(Some("vendor:model [1m]"),Some("requested effort")).await;
    let row=started.as_ref().ok().map(|_|f.row());
    let _=f.provider.stop_session(f.thread.clone()).await;
    let resumed=tokio::time::timeout(Duration::from_secs(4),ensure_live_session(f.app.handle(),ProviderKind::Acp,&f.thread)).await;
    let _=f.provider.stop_session(f.thread.clone()).await;
    assert!(started.is_ok(),"actual start command must execute: {started:?}");
    assert!(matches!(resumed,Ok(Ok(()))),"accepted startup state must cold-resume: {resumed:?}");
    let row=row.unwrap();
    assert_eq!(row.model.as_deref(),Some("default"),"history/recovery must use ACK, not requested model");
    assert_eq!(row.effort.as_deref(),Some(" Big "),"history/recovery must use ACK, not requested effort");
}
#[tokio::test]
async fn rereview_notification_replacement_projects_chat_row_and_cold_resume() {
    let f=Fixture::new("dynamic-notify-retired").await;
    f.start(Some("default"),Some(" A effort ")).await.unwrap();
    let mut events=f.provider.event_stream();
    let turn=f.provider.send_turn(serde_json::from_value(json!({"thread_id":f.thread.0,"text":"synthetic"})).unwrap()).await.unwrap().turn_id;
    let completed=tokio::time::timeout(Duration::from_secs(4),async {loop {if let Some(ProviderRuntimeEvent::TurnCompleted {turn_id,status,..})=events.next().await {if turn_id==turn {break status}}}}).await;
    let row=f.row();
    let stored=f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap().unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    // Exercise recovery even before row equality assertions (old A is now retired).
    let resumed=tokio::time::timeout(Duration::from_secs(4),ensure_live_session(f.app.handle(),ProviderKind::Acp,&f.thread)).await;
    let _=f.provider.stop_session(f.thread.clone()).await;
    assert!(matches!(completed,Ok(TurnStatus::Success)));
    assert!(matches!(resumed,Ok(Ok(()))),"latest accepted binding must survive auto-resume: {resumed:?}");
    assert_eq!(stored.catalog.current_model.as_deref(),Some("vendor:model [1m]"));
    assert_eq!(row.model.as_deref(),Some("vendor:model [1m]"));
    assert_eq!(row.effort.as_deref(),Some(" B effort "));
}
#[tokio::test]
async fn rereview_automatic_resume_ignores_legacy_stale_generic_row() {
    let f=Fixture::new("dynamic-ids").await;
    f.start(None,None).await.unwrap();
    f.provider.set_model(f.thread.clone(),"vendor:model [1m]".into()).await.unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    // Reproduce a previously saved, unreplayable generic row, not user intent.
    f.app.state::<DatabaseStore>().update_agent_chat_session_config(&f.thread.0,&AgentChatSessionConfig {model:Some(Some("default".into())),effort:Some(Some(" A effort ".into())),..Default::default()}).unwrap();
    let resumed=tokio::time::timeout(Duration::from_secs(4),ensure_live_session(f.app.handle(),ProviderKind::Acp,&f.thread)).await;
    let catalog=f.provider.catalog(&f.thread).await.unwrap();
    let _=f.provider.stop_session(f.thread.clone()).await;
    assert!(matches!(resumed,Ok(Ok(()))),"auto-resume must not replay stale generic overrides: {resumed:?}");
    assert_eq!(catalog.current_model.as_deref(),Some("vendor:model [1m]"));
}
