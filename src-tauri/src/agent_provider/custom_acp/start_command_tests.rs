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
    fn row(&self) -> crate::database::AgentChatSessionRecord { self.app.state::<DatabaseStore>().get_agent_chat_session(&self.thread.0).unwrap() }
}
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
