use super::super::{
    platform_authority::{self, PlatformCaller},
    platform_store::PlatformGrants,
    platform_workflow,
};
use super::*;
use crate::{
    agent_control::{ControlAccess, ControlCaller, NativeControlState},
    database::DatabaseStore,
    state::AppStateStore,
};
use tauri::Manager;

fn manifest() -> PlatformManifest {
    PlatformManifest::parse(br#"{"id":"example.domain","name":"Domain test","version":"1.0.0","apiVersion":1,"entry":"plugin.mjs","capabilities":["storage","settings","workflow.query","threads.read","threads.send","threads.create","events.run-finalized","worktrees.create"],"settings":[{"key":"token","label":"Token","type":"secret"},{"key":"label","label":"Label","type":"string","default":"default"}]}"#).unwrap()
}
fn install(service: &PlatformService, id: &str, projects: Vec<String>) -> InstallationRecord {
    service
        .register(id, "initial", manifest(), "package-one")
        .unwrap();
    service
        .consent(
            id,
            "package-one",
            PlatformGrants {
                capabilities: manifest().requested_capabilities(),
                project_ids: projects,
            },
        )
        .unwrap()
}
fn caller(record: &InstallationRecord) -> PlatformCaller {
    PlatformCaller::new(
        record.installation_id.clone(),
        record.generation.clone(),
        ControlCaller::trusted(),
        None,
        None,
    )
    .with_operation_id("operation-one".into())
}
fn fixture() -> (
    tauri::App<tauri::test::MockRuntime>,
    tempfile::TempDir,
    String,
    String,
) {
    let app = tauri::test::mock_app();
    let root = tempfile::tempdir().unwrap();
    let state = AppStateStore::default();
    let workspace = state
        .create_empty_workspace_at_path(root.path().to_path_buf())
        .0;
    let project = platform_workflow::project_id(&state.find_workspace(&workspace).unwrap());
    app.manage(state);
    app.manage(DatabaseStore::new_in_memory());
    app.manage(NativeControlState::default());
    (app, root, workspace, project)
}

#[test]
fn platform_corrupt_future_records_do_not_replace_data_or_hide_healthy_siblings() {
    let root = tempfile::tempdir().unwrap();
    let service = PlatformService::open(root.path().join("platform"));
    let healthy = install(&service, "healthy", vec!["project".into()]);
    let records = service.store.root().join("installations");
    let future = br#"{"schemaVersion":99,"userData":"retain"}"#;
    std::fs::write(records.join("future.json"), future).unwrap();
    std::fs::write(records.join("corrupt.json"), b"{bad").unwrap();
    assert!(service
        .register("future", "new", manifest(), "other")
        .is_err());
    assert!(service
        .register("corrupt", "new", manifest(), "other")
        .is_err());
    assert_eq!(std::fs::read(records.join("future.json")).unwrap(), future);
    assert_eq!(
        std::fs::read(records.join("corrupt.json")).unwrap(),
        b"{bad"
    );
    assert_eq!(
        service
            .store
            .list()
            .iter()
            .filter(|(_, record)| record.is_ok())
            .count(),
        1
    );
    assert!(platform_authority::assert_active(
        &service.store.read("healthy").unwrap(),
        &caller(&healthy)
    )
    .is_ok());
}

#[test]
fn platform_updates_keep_intent_storage_and_receipt_identity_but_require_exact_consent() {
    let root = tempfile::tempdir().unwrap();
    let service = PlatformService::open(root.path().into());
    let record = install(&service, "sample", vec!["project".into()]);
    let old_caller = caller(&record);
    service
        .store
        .update("sample", |record| {
            record.storage.insert("plan".into(), json!({"step":2}));
            Ok(())
        })
        .unwrap();
    service.set_enabled("sample", false).unwrap();
    let updated = service
        .register("sample", "next", manifest(), "package-two")
        .unwrap();
    assert!(!updated.enabled);
    assert_eq!(updated.storage["plan"]["step"], 2);
    assert_eq!(updated.consented_digest.as_deref(), Some("package-one"));
    assert!(service
        .consent("sample", "package-one", updated.grants.clone())
        .unwrap_err()
        .contains("stale_digest"));
    assert!(platform_authority::assert_active(&updated, &old_caller)
        .unwrap_err()
        .contains("stale_generation"));
    let request = json!({"projectId":"project","text":"same","idempotencyKey":"effect"});
    let identity = |operation, principal, request: &Value| {
        platform_workflow::effect_identity(
            "sample",
            operation,
            principal,
            "effect",
            "threads.create",
            request,
        )
    };
    assert_eq!(
        identity("invocation", "local", &request),
        identity("invocation", "local", &request)
    );
    assert_ne!(
        identity("invocation", "local", &request).0,
        identity("other-invocation", "local", &request).0
    );
    assert_ne!(
        identity("invocation", "local", &request).0,
        identity("invocation", "other-client", &request).0
    );
    let changed = json!({"text":"changed"});
    assert_eq!(
        identity("invocation", "local", &request).0,
        identity("invocation", "local", &changed).0
    );
    assert_ne!(
        identity("invocation", "local", &request).1,
        identity("invocation", "local", &changed).1
    );
}

#[test]
fn platform_actor_scope_intersects_grants_and_outside_agent_never_inherits_target_identity() {
    let root = tempfile::tempdir().unwrap();
    let service = PlatformService::open(root.path().into());
    let record = install(&service, "sample", vec!["allowed".into()]);
    let scoped = PlatformCaller::new(
        "sample".into(),
        record.generation.clone(),
        ControlCaller::trusted(),
        Some(vec!["other".into()]),
        Some("ui-target".into()),
    );
    assert!(
        platform_authority::capability(&record, &scoped, "threads.read", Some("allowed")).is_err()
    );
    assert_eq!(scoped.source_thread_id(), Some("ui-target"));
    let outside = PlatformCaller::new(
        "sample".into(),
        record.generation.clone(),
        ControlCaller::outside("client".into(), "grant".into(), ControlAccess::ReadOnly),
        None,
        Some("forged-caller".into()),
    );
    assert_eq!(outside.source_thread_id(), None);
    assert!(
        platform_authority::capability(&record, &outside, "threads.send", Some("allowed")).is_err()
    );
    assert_eq!(
        outside.for_delivery("run-one".into()).control.access,
        ControlAccess::ReadOnly
    );
    assert_eq!(
        caller(&record)
            .for_delivery("run-one".into())
            .control
            .access,
        ControlAccess::Supervised
    );
}

#[test]
fn platform_settings_seal_values_bind_identity_and_preserve_unreadable_secrets() {
    let root = tempfile::tempdir().unwrap();
    let service = PlatformService::open(root.path().into());
    install(&service, "sample", vec![]);
    let updated = service
        .update_settings(
            "sample",
            BTreeMap::from([
                ("token".into(), json!("private-token")),
                ("label".into(), json!("label")),
            ]),
        )
        .unwrap();
    assert!(
        !std::fs::read_to_string(root.path().join("installations/sample.json"))
            .unwrap()
            .contains("private-token")
    );
    assert_eq!(
        service
            .open_secret("sample", "token", &updated.secrets["token"])
            .unwrap(),
        "private-token"
    );
    assert!(service
        .open_secret("another", "token", &updated.secrets["token"])
        .is_err());
    assert!(service
        .open_secret("sample", "other", &updated.secrets["token"])
        .is_err());
    assert_eq!(
        PlatformService::public_settings(&updated)["token"],
        json!({"hasSecret":true})
    );
    assert!(!PlatformService::public_settings(&updated)
        .to_string()
        .contains("private-token"));
    std::fs::rename(
        root.path().join("secrets.key"),
        root.path().join("retained-key"),
    )
    .unwrap();
    assert!(service
        .open_secret("sample", "token", &updated.secrets["token"])
        .is_err());
    assert!(service
        .update_settings(
            "sample",
            BTreeMap::from([("token".into(), json!("replacement"))])
        )
        .is_err());
    assert!(!root.path().join("secrets.key").exists());
    assert_eq!(
        service.store.read("sample").unwrap().secrets,
        updated.secrets
    );
}

#[tokio::test]
async fn platform_closed_threads_are_paged_and_transcripts_are_safe_and_project_scoped() {
    let (app, root, workspace, project) = fixture();
    let service = PlatformService::open(root.path().join("platform"));
    let record = install(&service, "sample", vec![project.clone()]);
    let caller = caller(&record);
    let db = app.state::<DatabaseStore>();
    for thread in ["closed-one", "closed-two"] {
        db.upsert_agent_chat_session(thread, &workspace, None, "claude")
            .unwrap();
        db.append_agent_chat_message(
            thread,
            &json!({"type":"user_message","text":"visible","message_id":thread}).to_string(),
        )
        .unwrap();
    }
    db.append_agent_chat_message(
        "closed-one",
        &json!({"type":"reasoning_delta","text":"private reasoning"}).to_string(),
    )
    .unwrap();
    db.append_agent_chat_message(
        "closed-one",
        &json!({"type":"tool_output","text":"private tool output"}).to_string(),
    )
    .unwrap();
    let first = service
        .dispatch(
            app.handle(),
            &caller,
            "threads.list",
            json!({"projectId":project,"limit":1}),
        )
        .await
        .unwrap();
    assert_eq!(first["threads"].as_array().unwrap().len(), 1);
    assert_eq!(first["nextCursor"], 1);
    let second = service
        .dispatch(
            app.handle(),
            &caller,
            "threads.list",
            json!({"projectId":project,"limit":1,"cursor":1}),
        )
        .await
        .unwrap();
    assert_ne!(first["threads"][0]["id"], second["threads"][0]["id"]);
    let read = service
        .dispatch(
            app.handle(),
            &caller,
            "threads.read",
            json!({"threadId":"closed-one"}),
        )
        .await
        .unwrap();
    assert!(read.to_string().contains("visible"));
    assert!(!read.to_string().contains("private reasoning"));
    assert!(!read.to_string().contains("private tool output"));
    assert!(service
        .dispatch(
            app.handle(),
            &caller,
            "threads.list",
            json!({"projectId":"ungranted"})
        )
        .await
        .unwrap_err()
        .contains("permission_denied"));
}

#[tokio::test]
async fn platform_committed_events_replay_same_identity_after_restart_until_ack_and_require_gap_choice(
) {
    let (app, root, workspace, project) = fixture();
    let platform_root = root.path().join("platform");
    let service = PlatformService::open(platform_root.clone());
    let record = install(&service, "sample", vec![project.clone()]);
    let caller = caller(&record);
    let db = app.state::<DatabaseStore>();
    db.upsert_agent_chat_session("thread", &workspace, None, "claude")
        .unwrap();
    assert!(service.events.next(&record, &caller).unwrap().is_none());
    db.append_agent_chat_message("thread",&json!({"type":"turn_completed","thread_id":"thread","turn_id":"run","status":{"kind":"success"}}).to_string()).unwrap();
    assert_eq!(
        service
            .events
            .recover_thread(&db, &project, "thread")
            .unwrap(),
        1
    );
    assert_eq!(
        service
            .events
            .recover_thread(&db, &project, "thread")
            .unwrap(),
        0
    );
    let event = service.events.next(&record, &caller).unwrap().unwrap();
    drop(service);
    let service = PlatformService::open(platform_root);
    let record = service.store.read("sample").unwrap();
    assert_eq!(
        service
            .events
            .next(&record, &caller)
            .unwrap()
            .unwrap()
            .delivery_id,
        event.delivery_id
    );
    service
        .dispatch(
            app.handle(),
            &caller,
            "events.ack",
            json!({"deliveryId":event.delivery_id}),
        )
        .await
        .unwrap();
    assert!(service
        .events
        .next(&service.store.read("sample").unwrap(), &caller)
        .unwrap()
        .is_none());
    service.set_enabled("sample", false).unwrap();
    db.append_agent_chat_message("thread",&json!({"type":"turn_completed","thread_id":"thread","turn_id":"next-run","status":{"kind":"error","subtype":"failed"}}).to_string()).unwrap();
    service
        .events
        .recover_thread(&db, &project, "thread")
        .unwrap();
    let enabled = service.set_enabled("sample", true).unwrap();
    let enabled_caller = super::tests::caller(&enabled);
    assert!(service
        .dispatch(app.handle(), &enabled_caller, "events.next", json!({}))
        .await
        .unwrap_err()
        .contains("backlog_choice_required"));
    assert_eq!(service.events.backlog(&enabled).unwrap()["count"], 1);
    service
        .events
        .recover(&service.store, "sample", "skip")
        .unwrap();
    assert!(service
        .events
        .next(&service.store.read("sample").unwrap(), &enabled_caller)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn platform_send_retries_across_generation_and_restart_recover_core_receipt_and_reject_changed_inputs(
) {
    let (app, root, workspace, project) = fixture();
    let platform_root = root.path().join("platform");
    let service = PlatformService::open(platform_root.clone());
    let record = install(&service, "sample", vec![project.clone()]);
    let caller = caller(&record);
    let db = app.state::<DatabaseStore>();
    db.upsert_agent_chat_session("target", &workspace, None, "claude")
        .unwrap();
    app.state::<AppStateStore>()
        .create_or_reuse_agent_chat_pane_with_selection(
            &workspace,
            Some(crate::agent_provider::ProviderKind::Claude),
            Some(root.path().to_string_lossy().into_owned()),
            Some(crate::presets::LaunchMode::NewTab),
            Some("target".into()),
            false,
        )
        .unwrap();
    let barrier = Arc::new(tokio::sync::Semaphore::new(0));
    *app.state::<NativeControlState>()
        .fixture_operation_barrier
        .lock()
        .unwrap() = Some(barrier);
    let request =
        json!({"threadId":"target","text":"line one\nline two","idempotencyKey":"stable"});
    let first = service
        .dispatch(app.handle(), &caller, "threads.send", request.clone())
        .await
        .unwrap();
    assert!(first["operation_id"].is_string());
    let retry = service
        .dispatch(app.handle(), &caller, "threads.send", request.clone())
        .await
        .unwrap();
    assert_eq!(first["operation_id"], retry["operation_id"]);
    let upgraded = service
        .register("sample", "revision-two", manifest(), "package-two")
        .unwrap();
    let upgraded = service
        .consent("sample", "package-two", upgraded.grants)
        .unwrap();
    let upgraded_caller = super::tests::caller(&upgraded);
    drop(service);
    let service = PlatformService::open(platform_root);
    let replay = service
        .dispatch(
            app.handle(),
            &upgraded_caller,
            "threads.send",
            request.clone(),
        )
        .await
        .unwrap();
    assert_eq!(first["operation_id"], replay["operation_id"]);
    let mut changed = request;
    changed["text"] = json!("changed");
    assert!(service
        .dispatch(app.handle(), &upgraded_caller, "threads.send", changed)
        .await
        .unwrap_err()
        .contains("idempotency_conflict"));
    let effect = service
        .dispatch(
            app.handle(),
            &upgraded_caller,
            "workflow.query",
            json!({"query":"effect","idempotencyKey":"stable"}),
        )
        .await
        .unwrap();
    assert_eq!(effect["operationId"], first["operation_id"]);
    let later = upgraded_caller
        .clone()
        .with_operation_id("later-click".into());
    assert!(service
        .dispatch(
            app.handle(),
            &later,
            "workflow.query",
            json!({"query":"effect","idempotencyKey":"stable"})
        )
        .await
        .unwrap_err()
        .contains("not_found"));
    // Simulate losing the plugin result write after the core receipt committed.
    service
        .store
        .update("sample", |record| {
            record.receipts.values_mut().next().unwrap().result = None;
            Ok(())
        })
        .unwrap();
    let operation = service
        .dispatch(
            app.handle(),
            &later,
            "workflow.query",
            json!({"query":"operation","operationId":first["operation_id"]}),
        )
        .await
        .unwrap();
    assert_eq!(operation["operationId"], first["operation_id"]);
    let mut other = later.clone();
    other.control.principal = "other-authenticated-client".into();
    assert!(service
        .dispatch(
            app.handle(),
            &other,
            "workflow.query",
            json!({"query":"operation","operationId":first["operation_id"]})
        )
        .await
        .unwrap_err()
        .contains("not owned"));
    service
        .store
        .update("sample", |record| {
            record
                .receipts
                .values_mut()
                .next()
                .unwrap()
                .required_capabilities = vec!["worktrees.create".into()];
            Ok(())
        })
        .unwrap();
    let mut grants = service.store.read("sample").unwrap().grants;
    grants
        .capabilities
        .retain(|capability| capability != "worktrees.create");
    let current = service.consent("sample", "package-two", grants).unwrap();
    let restricted = super::tests::caller(&current);
    for query in [
        json!({"query":"effect","idempotencyKey":"stable"}),
        json!({"query":"operation","operationId":first["operation_id"]}),
    ] {
        assert!(service
            .dispatch(app.handle(), &restricted, "workflow.query", query)
            .await
            .unwrap_err()
            .contains("permission_denied"));
    }
    assert_eq!(service.store.read("sample").unwrap().receipts.len(), 1);
}

#[tokio::test]
async fn platform_event_effect_budget_and_plugin_origin_survive_committed_recovery() {
    let (app, root, workspace, project) = fixture();
    let service = PlatformService::open(root.path().join("platform"));
    let record = install(&service, "sample", vec![project.clone()]);
    let caller = caller(&record);
    let db = app.state::<DatabaseStore>();
    for thread in ["source", "target"] {
        db.upsert_agent_chat_session(thread, &workspace, None, "claude")
            .unwrap();
        db.update_agent_chat_session_config(
            thread,
            &crate::database::AgentChatSessionConfig {
                permission_mode: Some(Some("default".into())),
                ..Default::default()
            },
        )
        .unwrap();
    }
    app.state::<AppStateStore>()
        .create_or_reuse_agent_chat_pane_with_selection(
            &workspace,
            Some(crate::agent_provider::ProviderKind::Claude),
            Some(root.path().to_string_lossy().into_owned()),
            Some(crate::presets::LaunchMode::NewTab),
            Some("target".into()),
            false,
        )
        .unwrap();
    db.control_append_user_message("source", &json!({"type":"user_message","text":"human work","message_id":"human","client_nonce":"human"}).to_string(), "human-run", false).unwrap();
    db.append_agent_chat_message("source", &json!({"type":"turn_completed","thread_id":"source","turn_id":"human-run","status":{"kind":"error","subtype":"interrupted"}}).to_string()).unwrap();
    service
        .events
        .recover_thread(&db, &project, "source")
        .unwrap();
    let event = service.events.next(&record, &caller).unwrap().unwrap();
    assert!(event.effects_allowed);
    assert_eq!(event.outcome, "interrupted");
    let event_caller = caller.for_delivery(event.delivery_id.clone());
    for key in ["one", "two", "three"] {
        *app.state::<NativeControlState>()
            .fixture_operation_barrier
            .lock()
            .unwrap() = Some(Arc::new(tokio::sync::Semaphore::new(0)));
        service
            .dispatch(
                app.handle(),
                &event_caller,
                "threads.send",
                json!({"threadId":"target","text":"follow-up","idempotencyKey":key}),
            )
            .await
            .unwrap();
    }
    assert!(service
        .dispatch(
            app.handle(),
            &event_caller,
            "threads.send",
            json!({"threadId":"target","text":"fourth","idempotencyKey":"four"})
        )
        .await
        .unwrap_err()
        .contains("at most three"));
    let stored = service.store.read("sample").unwrap();
    let command = stored.receipts.values().next().unwrap().command_key.clone();
    db.control_append_user_message("target", &json!({"type":"user_message","text":"follow-up","message_id":"plugin-message","client_nonce":command}).to_string(), "plugin-run", false).unwrap();
    db.append_agent_chat_message("target", &json!({"type":"turn_completed","thread_id":"target","turn_id":"plugin-run","status":{"kind":"success"}}).to_string()).unwrap();
    service
        .events
        .recover_thread(&db, &project, "target")
        .unwrap();
    service
        .dispatch(
            app.handle(),
            &event_caller,
            "events.ack",
            json!({"deliveryId":event.delivery_id}),
        )
        .await
        .unwrap();
    service
        .dispatch(
            app.handle(),
            &event_caller,
            "events.ack",
            json!({"deliveryId":event.delivery_id}),
        )
        .await
        .unwrap();
    assert!(service
        .events
        .next(&service.store.read("sample").unwrap(), &event_caller)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn platform_uncorrelated_runs_are_observable_without_automatic_effect_authority() {
    let (app, root, workspace, project) = fixture();
    let service = PlatformService::open(root.path().join("platform"));
    let record = install(&service, "sample", vec![project.clone()]);
    let caller = caller(&record);
    let db = app.state::<DatabaseStore>();
    db.upsert_agent_chat_session("thread", &workspace, None, "claude")
        .unwrap();
    db.append_agent_chat_message("thread", &json!({"type":"turn_completed","thread_id":"thread","turn_id":"legacy-run","status":{"kind":"success"}}).to_string()).unwrap();
    service
        .events
        .recover_thread(&db, &project, "thread")
        .unwrap();
    let event = service.events.next(&record, &caller).unwrap().unwrap();
    assert!(!event.effects_allowed);
    assert_eq!(event.run_id, "legacy-run");
}

#[test]
fn platform_admission_leases_end_when_an_invocation_is_dropped() {
    let root = tempfile::tempdir().unwrap();
    let service = PlatformService::open(root.path().into());
    let lease = service.begin_effect("operation").unwrap();
    assert!(service.begin_effect("operation").is_err());
    drop(lease);
    assert!(service.begin_effect("operation").is_ok());
}

#[tokio::test]
async fn platform_secret_alias_requires_a_secret_and_cancelled_scopes_cannot_write() {
    let (app, root, _, _) = fixture();
    let service = PlatformService::open(root.path().join("platform"));
    install(&service, "sample", vec![]);
    let current = service
        .update_settings(
            "sample",
            BTreeMap::from([("token".into(), json!("secret"))]),
        )
        .unwrap();
    let caller = caller(&current);
    assert_eq!(
        service
            .dispatch(app.handle(), &caller, "secrets.get", json!({"key":"token"}))
            .await
            .unwrap(),
        json!("secret")
    );
    assert!(service
        .dispatch(app.handle(), &caller, "secrets.get", json!({"key":"label"}))
        .await
        .unwrap_err()
        .contains("declared secret"));
    let signal = super::super::platform_runtime::RuntimeCancellation::default();
    let cancelled = caller.with_cancellation(signal.clone());
    signal.cancel();
    assert!(service
        .dispatch(
            app.handle(),
            &cancelled,
            "storage.set",
            json!({"key":"forbidden","value":true})
        )
        .await
        .unwrap_err()
        .contains("cancelled"));
    assert!(!service
        .store
        .read("sample")
        .unwrap()
        .storage
        .contains_key("forbidden"));
}

#[test]
fn platform_background_delivery_narrows_full_access_keeps_read_only_and_accepts_native_plan() {
    let root = tempfile::tempdir().unwrap();
    let service = PlatformService::open(root.path().join("platform"));
    let record = install(&service, "sample", vec![]);
    let background = caller(&record).for_delivery("delivery".into());
    assert_eq!(background.control.access, ControlAccess::Supervised);
    for provider in [
        crate::agent_provider::ProviderKind::Claude,
        crate::agent_provider::ProviderKind::Codex,
    ] {
        assert!(crate::agent_control::service::permission_ceiling(
            &background.control,
            provider,
            Some("plan")
        )
        .is_ok());
    }
    assert!(crate::agent_control::service::permission_ceiling(
        &background.control,
        crate::agent_provider::ProviderKind::Claude,
        Some("bypassPermissions")
    )
    .is_err());
    let mut read_only = caller(&record);
    read_only.control.access = ControlAccess::ReadOnly;
    assert_eq!(
        read_only.for_delivery("delivery".into()).control.access,
        ControlAccess::ReadOnly
    );
}
