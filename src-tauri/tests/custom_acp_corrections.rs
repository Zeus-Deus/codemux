//! Real native commands + in-memory production DB + credential-free ACP child.
//! MockRuntime only: no production Builder, windows, servers or account setup.
#![cfg(unix)]
#[path = "helpers/custom_acp_python.rs"]
mod test_python;

use codemux_lib::{
    agent_provider::{
        custom_acp::{AcpAgentInput, GenericAcpProvider},
        AgentProvider, ProviderKind, ProviderRuntimeEvent, StartSessionInput, ThreadId, TurnStatus,
    },
    commands::{
        agent_chat::{
            agent_chat_set_model, agent_chat_set_permission_mode, ensure_live_session,
            ProviderRegistry,
        },
        custom_acp::{acp_catalog, AppAcpStore},
    },
    database::{AgentChatSessionConfig, DatabaseStore},
    observability::ObservabilityStore,
    state::AppStateStore,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tauri::Manager;
struct Fixture {
    dir: tempfile::TempDir,
    app: tauri::App<tauri::test::MockRuntime>,
    provider: Arc<GenericAcpProvider>,
    agent_id: String,
    thread: ThreadId,
}
impl Fixture {
    async fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = DatabaseStore::new_in_memory();
        let agent = db
            .save_acp_agent(AcpAgentInput {
                id: None,
                name: "Synthetic correction agent".into(),
                executable: test_python::executable(),
                args: vec!["-I".into(), format!(
                    "{}/tests/helpers/fake_custom_acp.py",
                    env!("CARGO_MANIFEST_DIR")
                )],
                environment: BTreeMap::from([
                    ("FAKE_ACP_MODE".into(), Some(mode.into())),
                    (
                        "FAKE_ACP_LOG".into(),
                        Some(dir.path().join("wire.jsonl").to_string_lossy().into_owned()),
                    ),
                ]),
                enabled: true,
                auth_method: None,
            })
            .unwrap();
        let app = tauri::test::mock_builder()
            .manage(db)
            .manage(ObservabilityStore::default())
            .manage(AppStateStore::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let provider = Arc::new(GenericAcpProvider::new(Arc::new(AppAcpStore(
            app.handle().clone(),
        ))));
        let registry = ProviderRegistry::new();
        registry.set_acp(provider.clone()).await;
        app.manage(provider.clone());
        app.manage(registry);
        let thread = ThreadId(format!("correction-{mode}"));
        let input: StartSessionInput = serde_json::from_value(json!({"thread_id":thread.0,"cwd":dir.path(),"additional_directories":[],"extra":{"acp_agent_id":agent.id}})).unwrap();
        let session = provider.start_session(input).await.unwrap();
        let db = app.state::<DatabaseStore>();
        db.upsert_agent_chat_session(&thread.0, "synthetic", dir.path().to_str(), "acp")
            .unwrap();
        db.set_agent_chat_sdk_session_id(&thread.0, &session.session_id.0)
            .unwrap();
        let catalog = provider.catalog(&thread).await.unwrap();
        let effort = catalog
            .config_options
            .iter()
            .find(|o| o.category.as_deref() == Some("thought_level"))
            .and_then(|o| o.current_value.as_str())
            .map(str::to_owned);
        db.update_agent_chat_session_config(
            &thread.0,
            &AgentChatSessionConfig {
                model: Some(catalog.current_model),
                effort: Some(effort),
                permission_mode: Some(Some("supervised".into())),
                ..Default::default()
            },
        )
        .unwrap();
        Self {
            dir,
            app,
            provider,
            agent_id: agent.id,
            thread,
        }
    }
    fn revise(&self) {
        let db = self.app.state::<DatabaseStore>();
        let mut config = db.acp_launch_config(&self.agent_id).unwrap();
        config.agent.args.push("revised literal arg".into());
        let old = config.agent.revision.clone();
        let revised = db
            .save_acp_agent(AcpAgentInput {
                id: Some(self.agent_id.clone()),
                name: config.agent.name,
                executable: config.agent.executable,
                args: config.agent.args,
                environment: config.agent.environment,
                enabled: true,
                auth_method: None,
            })
            .unwrap();
        assert_ne!(old, revised.revision);
    }
    fn row(&self) -> Value {
        serde_json::to_value(
            self.app
                .state::<DatabaseStore>()
                .get_agent_chat_session(&self.thread.0)
                .unwrap(),
        )
        .unwrap()
    }
    async fn resume(&self) {
        self.provider
            .stop_session(self.thread.clone())
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(4),
            ensure_live_session(self.app.handle(), ProviderKind::Acp, &self.thread),
        )
        .await
        .expect("resume must not deadlock")
        .expect("last accepted DB configuration must remain resumable");
        self.provider
            .stop_session(self.thread.clone())
            .await
            .unwrap();
    }
}
#[tokio::test]
async fn correction_revised_running_definition_keeps_live_catalog_readable() {
    let f = Fixture::new("resume").await;
    f.revise();
    let result = acp_catalog(f.app.state(), f.app.state(), f.thread.0.clone()).await;
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    assert!(
        result.is_ok(),
        "live catalog must precede cold revision checks: {result:?}"
    );
    assert!(
        acp_catalog(f.app.state(), f.app.state(), f.thread.0.clone())
            .await
            .is_err()
    );
    assert!(
        ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn correction_revised_running_definition_persists_live_config_without_rebinding() {
    let f = Fixture::new("resume").await;
    let original = f
        .app
        .state::<DatabaseStore>()
        .acp_binding(&f.thread.0)
        .unwrap()
        .unwrap();
    f.revise();
    let result = f
        .provider
        .set_config(f.thread.clone(), "boolean selector".into(), json!(true))
        .await;
    let alive = f.provider.has_session(&f.thread).await;
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    assert!(
        result.is_ok(),
        "established live binding update must not use launch admission: {result:?}"
    );
    assert!(alive);
    let stored = f
        .app
        .state::<DatabaseStore>()
        .acp_binding(&f.thread.0)
        .unwrap()
        .unwrap();
    assert_eq!(stored.revision, original.revision);
    assert_eq!(stored.session_id, original.session_id);
    assert_eq!(stored.cwd, original.cwd);
    assert_eq!(stored.config_values["boolean selector"], json!(true));
    assert!(
        ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn correction_start_racing_revision_is_rejected_at_final_admission() {
    racing_admission(false).await;
}
#[tokio::test]
async fn correction_start_racing_deletion_is_rejected_at_final_admission() {
    racing_admission(true).await;
}
async fn racing_admission(delete: bool) {
    let f = Fixture::new("gated-resume").await;
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let original = serde_json::to_value(
        f.app
            .state::<DatabaseStore>()
            .acp_binding(&f.thread.0)
            .unwrap(),
    )
    .unwrap();
    let input: StartSessionInput = serde_json::from_value(
        json!({"thread_id":f.thread.0,"cwd":f.dir.path(),"additional_directories":[]}),
    )
    .unwrap();
    let provider = f.provider.clone();
    let start = tokio::spawn(async move { provider.start_session(input).await });
    tokio::time::timeout(Duration::from_secs(4), async {
        while !f.dir.path().join("wire.jsonl.ready").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    if delete {
        f.app
            .state::<DatabaseStore>()
            .delete_acp_agent(&f.agent_id)
            .unwrap();
    } else {
        f.revise();
    }
    std::fs::write(f.dir.path().join("wire.jsonl.release"), "release").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(4), start)
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.is_err(),
        "a racing stale/new launch must not enter through live update: {result:?}"
    );
    assert!(!f.provider.has_session(&f.thread).await);
    assert_eq!(
        serde_json::to_value(
            f.app
                .state::<DatabaseStore>()
                .acp_binding(&f.thread.0)
                .unwrap()
        )
        .unwrap(),
        original
    );
    assert!(
        ensure_live_session(f.app.handle(), ProviderKind::Acp, &f.thread)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn correction_live_binding_update_rejects_every_identity_change() {
    let f = Fixture::new("resume").await;
    let db = f.app.state::<DatabaseStore>();
    let original = db.acp_binding(&f.thread.0).unwrap().unwrap();
    for field in [
        "thread",
        "agent",
        "revision",
        "cwd",
        "native",
        "missing-native",
    ] {
        let mut changed = original.clone();
        match field {
            "thread" => changed.thread_id.push_str("-other"),
            "agent" => changed.agent_id.push_str("-other"),
            "revision" => changed.revision.push_str("-other"),
            "cwd" => changed.cwd.push_str("/other"),
            "native" => changed.session_id = Some("other native ID".into()),
            _ => changed.session_id = None,
        }
        assert!(
            db.update_acp_binding(&changed).is_err(),
            "must reject {field}"
        );
        assert_eq!(
            serde_json::to_value(db.acp_binding(&f.thread.0).unwrap().unwrap()).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
    }
    f.provider.stop_session(f.thread.clone()).await.unwrap();
}
#[tokio::test]
async fn correction_cold_failed_model_is_not_acknowledged_without_a_live_session() {
    let f = Fixture::new("resume").await;
    let before = f.row();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        agent_chat_set_model(
            f.app.handle().clone(),
            ProviderKind::Acp,
            f.thread.clone(),
            Some("not advertised".into()),
        ),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    assert_eq!(f.row(), before);
    f.resume().await;
}

#[path = "helpers/mock_agent_provider.rs"]
#[allow(dead_code)]
mod builtin_fixture;
#[tokio::test]
async fn correction_builtin_cold_setters_keep_persist_always_apply_if_live_contract() {
    use builtin_fixture::{MockAgentProvider, MockCall};
    for kind in [
        ProviderKind::Claude,
        ProviderKind::Codex,
        ProviderKind::Cursor,
        ProviderKind::Grok,
        ProviderKind::OpenCode,
    ] {
        let db = DatabaseStore::new_in_memory();
        let thread = ThreadId(format!("builtin-{kind:?}"));
        db.upsert_agent_chat_session(&thread.0, "synthetic", None, "native")
            .unwrap();
        let provider = Arc::new(MockAgentProvider::new(kind));
        let registry = ProviderRegistry::new();
        match kind {
            ProviderKind::Claude => registry.set_claude(provider.clone()).await,
            ProviderKind::Codex => registry.set_codex(provider.clone()).await,
            ProviderKind::Cursor => registry.set_cursor(provider.clone()).await,
            ProviderKind::Grok => registry.set_grok(provider.clone()).await,
            _ => registry.set_opencode(provider.clone()).await,
        }
        let app = tauri::test::mock_builder()
            .manage(db)
            .manage(registry)
            .manage(ObservabilityStore::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        agent_chat_set_model(
            app.handle().clone(),
            kind,
            thread.clone(),
            Some("native model".into()),
        )
        .await
        .unwrap();
        agent_chat_set_permission_mode(
            app.handle().clone(),
            kind,
            thread.clone(),
            "native mode".into(),
        )
        .await
        .unwrap();
        let row = app
            .state::<DatabaseStore>()
            .get_agent_chat_session(&thread.0)
            .unwrap();
        assert_eq!(row.model.as_deref(), Some("native model"));
        assert_eq!(row.permission_mode.as_deref(), Some("native mode"));
        assert_eq!(
            provider.calls.snapshot(),
            vec![
                MockCall::SetModel(thread.clone(), "native model".into()),
                MockCall::SetPermissionMode(thread, "native mode".into())
            ]
        );
    }
}

#[tokio::test]
async fn correction_model_command_replacement_keeps_db_effort_resumable() {
    for mode in ["dynamic-ids", "dynamic-values", "dynamic-no-effort"] {
        let f = Fixture::new(mode).await;
        agent_chat_set_model(
            f.app.handle().clone(),
            ProviderKind::Acp,
            f.thread.clone(),
            Some("vendor:model [1m]".into()),
        )
        .await
        .unwrap();
        let row = f.row();
        f.resume().await;
        assert_eq!(row["model"], json!("vendor:model [1m]"));
        assert_eq!(
            row["effort"],
            if mode == "dynamic-no-effort" {
                Value::Null
            } else {
                json!(" B effort ")
            }
        );
    }
}

#[tokio::test]
async fn correction_revised_running_definition_accepts_notification_during_prompt() {
    use futures_util::StreamExt;
    let f = Fixture::new("notify-catalog").await;
    let original = f
        .app
        .state::<DatabaseStore>()
        .acp_binding(&f.thread.0)
        .unwrap()
        .unwrap();
    f.revise();
    let mut events = f.provider.event_stream();
    let turn = f
        .provider
        .send_turn(
            serde_json::from_value(json!({"thread_id":f.thread.0,"text":"synthetic"})).unwrap(),
        )
        .await
        .unwrap()
        .turn_id;
    let status = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(ProviderRuntimeEvent::TurnCompleted {
                turn_id, status, ..
            }) = events.next().await
            {
                if turn_id == turn {
                    break status;
                }
            }
        }
    })
    .await
    .unwrap();
    let alive = f.provider.has_session(&f.thread).await;
    let stored = f
        .app
        .state::<DatabaseStore>()
        .acp_binding(&f.thread.0)
        .unwrap()
        .unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    assert!(
        matches!(status, TurnStatus::Success),
        "definition edit must not turn a valid update into a failed turn: {status:?}"
    );
    assert!(alive);
    assert_eq!(stored.revision, original.revision);
    assert_eq!(
        stored
            .catalog
            .config_options
            .iter()
            .find(|o| o.id == "boolean selector")
            .unwrap()
            .current_value,
        json!(true)
    );
}

#[tokio::test]
async fn correction_new_start_racing_revision_or_deletion_has_no_binding() {
    for delete in [false, true] {
        let f = Fixture::new("gated-new").await;
        let thread = ThreadId("new-unbound-thread".into());
        let input:StartSessionInput=serde_json::from_value(json!({"thread_id":thread.0,"cwd":f.dir.path(),"additional_directories":[],"extra":{"acp_agent_id":f.agent_id}})).unwrap();
        let provider = f.provider.clone();
        let start = tokio::spawn(async move { provider.start_session(input).await });
        tokio::time::timeout(Duration::from_secs(4), async {
            while !f.dir.path().join("wire.jsonl.ready").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if delete {
            f.app
                .state::<DatabaseStore>()
                .delete_acp_agent(&f.agent_id)
                .unwrap();
        } else {
            f.revise();
        }
        std::fs::write(f.dir.path().join("wire.jsonl.release"), "release").unwrap();
        let result = tokio::time::timeout(Duration::from_secs(4), start)
            .await
            .unwrap()
            .unwrap();
        f.provider.stop_session(f.thread.clone()).await.unwrap();
        assert!(
            result.is_err(),
            "racing new launch must fail final admission: {result:?}"
        );
        assert!(f
            .app
            .state::<DatabaseStore>()
            .acp_binding(&thread.0)
            .unwrap()
            .is_none());
        assert!(!f.provider.has_session(&thread).await);
    }
}

#[tokio::test]
async fn correction_failed_model_validation_leaves_db_unchanged_and_resumable() {
    let f = Fixture::new("resume").await;
    let before = f.row();
    let result = agent_chat_set_model(
        f.app.handle().clone(),
        ProviderKind::Acp,
        f.thread.clone(),
        Some("not advertised".into()),
    )
    .await;
    assert!(result.is_err());
    let after = f.row();
    f.resume().await;
    assert_eq!(after, before);
}
#[tokio::test]
async fn correction_failed_model_rpc_leaves_db_unchanged_and_resumable() {
    let f = Fixture::new("reject-model").await;
    let before = f.row();
    assert!(agent_chat_set_model(
        f.app.handle().clone(),
        ProviderKind::Acp,
        f.thread.clone(),
        Some("vendor:model [1m]".into())
    )
    .await
    .is_err());
    let after = f.row();
    f.resume().await;
    assert_eq!(after, before);
}
#[tokio::test]
async fn correction_failed_permission_leaves_db_unchanged_and_resumable() {
    let f = Fixture::new("resume").await;
    let before = f.row();
    assert!(agent_chat_set_permission_mode(
        f.app.handle().clone(),
        ProviderKind::Acp,
        f.thread.clone(),
        "bypassPermissions".into()
    )
    .await
    .is_err());
    let after = f.row();
    f.resume().await;
    assert_eq!(after, before);
}
#[tokio::test]
async fn correction_model_persists_acknowledged_model_and_effort() {
    let f = Fixture::new("ack-model").await;
    f.app
        .state::<DatabaseStore>()
        .update_agent_chat_session_config(
            &f.thread.0,
            &AgentChatSessionConfig {
                effort: Some(Some("stale effort".into())),
                ..Default::default()
            },
        )
        .unwrap();
    agent_chat_set_model(
        f.app.handle().clone(),
        ProviderKind::Acp,
        f.thread.clone(),
        Some("vendor:model [1m]".into()),
    )
    .await
    .unwrap();
    let row = f.row();
    f.resume().await;
    assert_eq!(row["model"], json!("default"));
    assert_eq!(row["effort"], json!(" Big "));
}

async fn ordered_catalog_change(mode: &str, expected: &str) {
    let f = Fixture::new(mode).await;
    let changed = agent_chat_set_model(f.app.handle().clone(),ProviderKind::Acp,f.thread.clone(),Some("vendor:model [1m]".into())).await;
    let live = f.provider.catalog(&f.thread).await.unwrap();
    let row = f.row();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let resumed = tokio::time::timeout(Duration::from_secs(4),ensure_live_session(f.app.handle(),ProviderKind::Acp,&f.thread)).await;
    let restored = f.provider.catalog(&f.thread).await.unwrap();
    let _ = f.provider.stop_session(f.thread.clone()).await;
    assert!(changed.is_ok(),"setter must acknowledge a valid ordered catalog: {changed:?}");
    assert!(matches!(resumed,Ok(Ok(()))),"latest ordered state must cold-resume: {resumed:?}");
    assert_eq!(live.current_model.as_deref(),Some(expected),"wire-later catalog wins, not the drain-later application");
    assert_eq!(row["model"],json!(expected));
    assert_eq!(restored.current_model.as_deref(),Some(expected));
    assert_eq!(row["effort"],json!(if expected == " D model " {" D effort "} else {" C effort "}));
}
#[tokio::test]
async fn rereview_response_then_notification_keeps_latest_config_catalog() { ordered_catalog_change("ordered-config"," D model ").await; }
#[tokio::test]
async fn rereview_response_then_notification_keeps_latest_legacy_model_catalog() { ordered_catalog_change("ordered-legacy"," D model ").await; }
#[tokio::test]
async fn rereview_notification_then_response_keeps_latest_response_catalog() { ordered_catalog_change("ordered-before","vendor:model [1m]").await; }

async fn invalid_acknowledgement(mode: &str) {
    let f = Fixture::new(mode).await;
    let before = f.row();
    let binding_before = serde_json::to_value(f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap()).unwrap();
    let changed = agent_chat_set_model(f.app.handle().clone(),ProviderKind::Acp,f.thread.clone(),Some("vendor:model [1m]".into())).await;
    let after = f.row();
    let binding_after = serde_json::to_value(f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap()).unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let resumed = tokio::time::timeout(Duration::from_secs(4),ensure_live_session(f.app.handle(),ProviderKind::Acp,&f.thread)).await;
    let _ = f.provider.stop_session(f.thread.clone()).await;
    assert!(changed.is_err(),"unadvertised acknowledged membership must fail: {changed:?}");
    assert_eq!(after,before,"invalid ACK must not poison history/restart row");
    assert_eq!(binding_after,binding_before,"invalid ACK must not replace accepted durable binding");
    assert!(matches!(resumed,Ok(Ok(()))),"last valid ACK must remain resumable: {resumed:?}");
}
#[tokio::test]
async fn rereview_unadvertised_acknowledged_model_does_not_poison_resume() { invalid_acknowledgement("invalid-ack-model").await; }
#[tokio::test]
async fn rereview_unadvertised_acknowledged_effort_does_not_poison_resume() { invalid_acknowledgement("invalid-ack-effort").await; }
#[tokio::test]
async fn rereview_unadvertised_acknowledged_legacy_model_does_not_poison_resume() { invalid_acknowledgement("invalid-ack-legacy").await; }

#[tokio::test]
async fn rereview_thread_catalog_recovers_idle_live_authority_after_definition_change() {
    let f = Fixture::new("resume").await;
    f.revise();
    let wire_before = std::fs::read(f.dir.path().join("wire.jsonl")).unwrap();
    let binding_before = serde_json::to_value(f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap()).unwrap();
    // No lifecycle subscription/replay and no start: exactly a cold renderer read.
    let read = crate_read_catalog(&f).await;
    let legacy = acp_catalog(f.app.state(),f.app.state(),f.thread.0.clone()).await;
    let wire_after = std::fs::read(f.dir.path().join("wire.jsonl")).unwrap();
    let binding_after = serde_json::to_value(f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap()).unwrap();
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let cold = crate_read_catalog(&f).await;
    let read = read.unwrap();
    assert!(read.live);
    assert_eq!(read.catalog.agent_id,f.agent_id);
    assert_eq!(serde_json::to_value(legacy.unwrap()).unwrap(),serde_json::to_value(&read.catalog).unwrap());
    assert_eq!(wire_after,wire_before,"read must not send protocol/start/config calls");
    assert_eq!(binding_before,binding_after,"liveness must never be persisted");
    assert!(cold.is_err(),"cold changed revision must retain launch validation");
}
async fn crate_read_catalog(f: &Fixture) -> Result<codemux_lib::commands::custom_acp::AcpThreadCatalog,String> {
    codemux_lib::commands::custom_acp::acp_thread_catalog(f.app.state(),f.app.state(),f.thread.0.clone()).await
}
#[tokio::test]
async fn rereview_thread_catalog_cold_status_reads_without_launching() {
    let f = Fixture::new("resume").await;
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    let before = std::fs::read(f.dir.path().join("wire.jsonl")).unwrap();
    let binding_before = serde_json::to_value(f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap()).unwrap();
    let read = crate_read_catalog(&f).await.unwrap();
    assert!(!read.live);
    assert_eq!(read.catalog.agent_id,f.agent_id);
    assert_eq!(before,std::fs::read(f.dir.path().join("wire.jsonl")).unwrap());
    assert_eq!(binding_before,serde_json::to_value(f.app.state::<DatabaseStore>().acp_binding(&f.thread.0).unwrap()).unwrap());
    let serialized = serde_json::to_value(read).unwrap();
    assert_eq!(serialized["live"],json!(false));
    assert!(serialized.get("catalog").is_some());
}
#[tokio::test]
async fn rereview_thread_catalog_requires_exact_native_process_binding() {
    let f = Fixture::new("resume").await;
    let db = f.app.state::<DatabaseStore>();
    let mut newer = db.acp_binding(&f.thread.0).unwrap().unwrap();
    newer.session_id = Some("different native owner".into());
    db.save_acp_binding(&newer).unwrap();
    let read = crate_read_catalog(&f).await;
    f.provider.stop_session(f.thread.clone()).await.unwrap();
    assert!(!read.unwrap().live,"an old process cannot attest a different native binding");
}
