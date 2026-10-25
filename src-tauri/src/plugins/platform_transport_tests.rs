use super::*;
use std::time::Duration;

pub(crate) fn fixture() -> (tauri::App<tauri::test::MockRuntime>, tempfile::TempDir) {
    let app = tauri::test::mock_app();
    let root = tempfile::tempdir().unwrap();
    let platform = Arc::new(PluginPlatform {
        root: root.path().join("platform"),
        service: Arc::new(PlatformService::open(root.path().join("platform/state"))),
        packages: PlatformPackageStore::new(root.path().join("platform/packages")),
        management: ManagementStore::new(root.path().join("platform/management")),
        supervisor: PluginSupervisor::new(PluginRuntimeConfig {
            invocation_timeout: Duration::from_secs(1),
            ..Default::default()
        }),
        mutations: tokio::sync::Mutex::new(()),
    });
    let state = PluginPlatformState::default();
    assert!(state.platform.set(Ok(platform)).is_ok());
    app.manage(state);
    app.manage(crate::database::DatabaseStore::new_in_memory());
    app.manage(crate::state::AppStateStore::default());
    app.manage(crate::agent_control::NativeControlState::default());
    (app, root)
}
pub(crate) fn source(root: &Path, name: &str, version: &str, body: &str) -> PathBuf {
    let directory = root.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("codemux-plugin.json"),json!({
        "id":format!("test.{name}"),"name":name,"version":version,"apiVersion":1,"entry":"plugin.mjs",
        "capabilities":["storage","ui","ui.conversation-cards"],
        "actions":[{"id":"run","title":"Run","placements":["palette"],"agentCallable":true},
            {"id":"human","title":"Human only","placements":[],"agentCallable":false}],
        "views":[{"id":"card","title":"Card","entry":"view.html","placement":"conversation"}],"settings":[],
    }).to_string()).unwrap();
    std::fs::write(directory.join("plugin.mjs"), body).unwrap();
    std::fs::write(
        directory.join("view.html"),
        "<!doctype html><p>Versioned view</p>",
    )
    .unwrap();
    directory
}
async fn call(
    app: &tauri::App<tauri::test::MockRuntime>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    request(app.handle(), ControlCaller::trusted(), method, params).await
}
pub(crate) async fn install(app: &tauri::App<tauri::test::MockRuntime>, directory: &Path) -> Value {
    call(
        app,
        "install",
        json!({"source":{"kind":"local","directory":directory}}),
    )
    .await
    .unwrap()
}
pub(crate) async fn enable(
    app: &tauri::App<tauri::test::MockRuntime>,
    installation: &Value,
) -> Value {
    let backlog = call(
        app,
        "backlog",
        json!({"installationId":installation["installationId"]}),
    )
    .await
    .unwrap();
    let consented = call(app,"consent",json!({"installationId":installation["installationId"],"digest":installation["pendingUpdate"]["digest"].as_str().unwrap_or_else(||installation["digest"].as_str().unwrap()),
        "grants":{"capabilities":["storage","ui","ui.conversation-cards"],"projectIds":[]},
        "eventBacklog":{"policy":"skip","throughSequence":backlog["throughSequence"]}})).await.unwrap();
    call(
        app,
        "setEnabled",
        json!({"installationId":consented["installationId"],"enabled":true}),
    )
    .await
    .unwrap()
}
async fn invoke(
    app: &tauri::App<tauri::test::MockRuntime>,
    installation: &Value,
    operation: &str,
) -> Result<Value, String> {
    call(app,"invoke",json!({"installationId":installation["installationId"],"generation":installation["generation"],"handler":"action:run","operationId":operation})).await
}

#[tokio::test]
async fn platform_transport_stages_updates_and_preserves_data_and_card_history_across_reinstall() {
    let (app, root) = fixture();
    let directory = source(
        root.path(),
        "lifecycle",
        "1.0.0",
        r#"export function activate(ctx){ctx.handle('action:run',async()=>{const n=(await ctx.storage.get('n'))??0;await ctx.storage.set('n',n+1);return {data:{version:1,n:n+1},card:{viewId:'card',summary:'Recorded proposal',data:{id:1}}};});}"#,
    );
    let installed = install(&app, &directory).await;
    assert_eq!(installed["enabled"], false);
    assert!(invoke(&app, &installed, "before-consent").await.is_err());
    let active = enable(&app, &installed).await;
    let first = invoke(&app, &active, "first").await.unwrap();
    assert_eq!(first["data"]["n"], 1);
    source(
        root.path(),
        "lifecycle",
        "2.0.0",
        r#"export function activate(ctx){ctx.handle('action:run',async()=>({data:{version:2,n:await ctx.storage.get('n')}}));}"#,
    );
    let staged = call(
        &app,
        "refresh",
        json!({"installationId":active["installationId"]}),
    )
    .await
    .unwrap();
    assert_eq!(staged["digest"], active["digest"]);
    assert_eq!(staged["generation"], active["generation"]);
    assert_eq!(
        invoke(&app, &active, "still-v1").await.unwrap()["data"]["version"],
        1
    );
    assert!(call(&app,"consent",json!({"installationId":active["installationId"],"digest":"0".repeat(64),"grants":{"capabilities":[],"projectIds":[]}})).await.is_err());
    let updated = enable(&app, &staged).await;
    assert!(invoke(&app, &active, "stale").await.is_err());
    assert_eq!(invoke(&app, &updated, "v2").await.unwrap()["data"]["n"], 2);
    let view=call(&app,"readCardView",json!({"installationId":updated["installationId"],"generation":updated["generation"],"card":first["card"]})).await.unwrap();
    assert!(view["html"].as_str().unwrap().contains("Versioned view"));
    call(
        &app,
        "remove",
        json!({"installationId":updated["installationId"]}),
    )
    .await
    .unwrap();
    let reinstalled = enable(&app, &install(&app, &directory).await).await;
    assert_eq!(reinstalled["dataOwner"], active["dataOwner"]);
    assert_eq!(
        invoke(&app, &reinstalled, "retained").await.unwrap()["data"]["n"],
        2
    );
    call(
        &app,
        "remove",
        json!({"installationId":reinstalled["installationId"],"deleteData":true}),
    )
    .await
    .unwrap();
    let reset = enable(&app, &install(&app, &directory).await).await;
    assert_ne!(reset["dataOwner"], first["card"]["dataOwner"]);
    assert!(call(&app,"readCardView",json!({"installationId":reset["installationId"],"generation":reset["generation"],"card":first["card"]})).await.is_err());
    app.state::<PluginPlatformState>().shutdown().await;
}

#[tokio::test]
async fn platform_transport_cpu_fault_only_holds_affected_plugin_and_keeps_enabled_intent() {
    let (app, root) = fixture();
    let faulty = source(
        root.path(),
        "faulty",
        "1.0.0",
        "export function activate(ctx){ctx.handle('action:run',()=>{while(true){}});}",
    );
    let healthy = source(
        root.path(),
        "healthy",
        "1.0.0",
        "export function activate(ctx){ctx.handle('action:run',()=>({message:'healthy'}));}",
    );
    let faulty = enable(&app, &install(&app, &faulty).await).await;
    let healthy = enable(&app, &install(&app, &healthy).await).await;
    assert!(invoke(&app, &faulty, "hang")
        .await
        .unwrap_err()
        .contains("timeout"));
    let catalogue = call(&app, "catalogue", json!({})).await.unwrap();
    let faulty = catalogue["installations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["installationId"] == faulty["installationId"])
        .unwrap();
    assert_eq!(faulty["enabled"], true);
    assert_eq!(faulty["status"], "error");
    assert_eq!(
        invoke(&app, &healthy, "healthy").await.unwrap()["message"],
        "healthy"
    );
    let db = app.state::<crate::database::DatabaseStore>();
    db.set_setting("plugin-fault-core-check", "healthy")
        .unwrap();
    assert_eq!(
        db.get_setting("plugin-fault-core-check").as_deref(),
        Some("healthy")
    );
    app.state::<PluginPlatformState>().shutdown().await;
}

#[test]
fn platform_transport_image_results_keep_structured_cards_without_duplicate_base64_text() {
    let result = mcp_result(
        json!({"card":{"summary":"Durable summary"},"images":[{"mimeType":"image/png","data":"base64"}]}),
    );
    assert_eq!(
        result["structuredContent"]["card"]["summary"],
        "Durable summary"
    );
    assert_eq!(result["content"][1]["type"], "image");
    assert!(!result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("base64"));
}

#[tokio::test]
async fn platform_transport_recovery_validation_has_no_admin_side_effects() {
    let (app, root) = fixture();
    let directory = source(
        root.path(),
        "atomic",
        "1.0.0",
        "export function activate(ctx){ctx.handle('action:run',()=>({}));}",
    );
    let installed = install(&app, &directory).await;
    assert_eq!(installed["status"], "needs-consent");
    let active = enable(&app, &installed).await;
    let platform = app
        .state::<PluginPlatformState>()
        .get(app.handle())
        .unwrap();
    let id = active["installationId"].as_str().unwrap();
    let original = serde_json::to_value(platform.service.store.read(id).unwrap()).unwrap();
    let management = serde_json::to_value(platform.management.read(id).unwrap()).unwrap();
    for method in ["consent", "setEnabled", "resume"] {
        let mut params = json!({"installationId":id,"enabled":true,"digest":active["digest"],"grants":active["grants"],"eventBacklog":{"policy":"skip","throughSequence":999}});
        assert!(call(&app, method, params.clone())
            .await
            .unwrap_err()
            .contains("ahead"));
        assert_eq!(
            serde_json::to_value(platform.service.store.read(id).unwrap()).unwrap(),
            original
        );
        assert_eq!(
            serde_json::to_value(platform.management.read(id).unwrap()).unwrap(),
            management
        );
        params["eventBacklog"]["extra"] = json!(true);
        assert!(call(&app, method, params)
            .await
            .unwrap_err()
            .contains("invalid_input"));
        assert_eq!(
            serde_json::to_value(platform.service.store.read(id).unwrap()).unwrap(),
            original
        );
    }
    platform
        .service
        .store
        .update(id, |record| {
            record.event_cursor = 5;
            Ok(())
        })
        .unwrap();
    let stale = serde_json::to_value(platform.service.store.read(id).unwrap()).unwrap();
    assert!(call(
        &app,
        "resume",
        json!({"installationId":id,"eventBacklog":{"policy":"deliver","throughSequence":0}})
    )
    .await
    .unwrap_err()
    .contains("stale_backlog"));
    assert_eq!(
        serde_json::to_value(platform.service.store.read(id).unwrap()).unwrap(),
        stale
    );
    app.state::<PluginPlatformState>().shutdown().await;
}

#[tokio::test]
async fn platform_transport_interrupted_promotion_stays_unavailable_until_exact_review_recovers() {
    let (app, root) = fixture();
    let directory = source(
        root.path(),
        "partial",
        "1.0.0",
        "export function activate(ctx){ctx.handle('action:run',()=>({data:{version:1}}));}",
    );
    let active = enable(&app, &install(&app, &directory).await).await;
    source(
        root.path(),
        "partial",
        "2.0.0",
        "export function activate(ctx){ctx.handle('action:run',()=>({data:{version:2}}));}",
    );
    let staged = call(
        &app,
        "refresh",
        json!({"installationId":active["installationId"]}),
    )
    .await
    .unwrap();
    let id = active["installationId"].as_str().unwrap();
    let pointer = root
        .path()
        .join("platform/packages/active")
        .join(format!("{id}.json"));
    let saved = std::fs::read(&pointer).unwrap();
    std::fs::remove_file(&pointer).unwrap();
    std::fs::create_dir(&pointer).unwrap();
    assert!(call(&app,"consent",json!({"installationId":id,"digest":staged["pendingUpdate"]["digest"],"grants":active["grants"]})).await.is_err());
    let platform = app
        .state::<PluginPlatformState>()
        .get(app.handle())
        .unwrap();
    let partial = platform.installation(id).unwrap();
    assert_eq!(partial["status"], "error");
    assert_eq!(partial["enabled"], true);
    assert_ne!(partial["digest"], active["digest"]);
    assert!(invoke(&app, &partial, "partial")
        .await
        .unwrap_err()
        .contains("incomplete"));
    assert!(call(&app, "resume", json!({"installationId":id}))
        .await
        .is_err());
    std::fs::remove_dir(&pointer).unwrap();
    std::fs::write(&pointer, saved).unwrap();
    let recovered = enable(&app, &partial).await;
    assert_eq!(
        invoke(&app, &recovered, "recovered").await.unwrap()["data"]["version"],
        2
    );
    app.state::<PluginPlatformState>().shutdown().await;
}

#[tokio::test]
async fn platform_transport_agent_tool_never_bypasses_agent_callable_for_user_control_credentials()
{
    let (app, root) = fixture();
    let directory = source(
        root.path(),
        "agent-boundary",
        "1.0.0",
        "export function activate(ctx){ctx.handle('action:human',()=>({message:'forbidden'}));}",
    );
    let active = enable(&app, &install(&app, &directory).await).await;
    let error = agent_request(app.handle(),ControlCaller::trusted(),"plugin_invoke",json!({"installationId":active["installationId"],"generation":active["generation"],"actionId":"human","operationId":"agent","input":{"caller":{"kind":"client"}}})).await.unwrap_err();
    assert!(error.contains("not exposed to agents"));
    app.state::<PluginPlatformState>().shutdown().await;
}

#[test]
fn platform_transport_tree_shape_and_reference_bounds_match_native_rows() {
    let valid = json!({"nodes":[{"id":"p","kind":"project","projectId":"project"},{"id":"t","parentId":"p","kind":"thread","threadId":"thread","collapsedByDefault":false}]});
    assert!(validate_tree(&valid).is_ok());
    for nodes in [
        json!([{"id":"a","kind":"thread","threadId":"same"},{"id":"b","kind":"thread","threadId":"same"}]),
        json!([{"id":"a","kind":"group","title":"A","parentId":"missing"}]),
        json!([{"id":"a","kind":"group","title":"A","parentId":"a"}]),
        json!([{"id":"a","kind":"project","projectId":"p","collapsedByDefault":"yes"}]),
        json!([{"id":"a","kind":"thread","threadId":"t","title":"spoof native title"}]),
        json!((0..17).map(|i| if i==0 {json!({"id":format!("n{i}"),"kind":"group","title":"G"})} else {json!({"id":format!("n{i}"),"parentId":format!("n{}",i-1),"kind":"group","title":"G"})}).collect::<Vec<_>>()),
    ] { assert!(validate_tree(&json!({"nodes":nodes})).is_err()); }
}

#[test]
fn platform_transport_images_validate_private_path_size_and_signature() {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    std::fs::create_dir(&private).unwrap();
    std::fs::write(private.join("ok.png"), b"\x89PNG\r\n\x1a\npayload").unwrap();
    assert_eq!(
        read_result_images(&private, &json!([{"path":"ok.png","mimeType":"image/png"}])).unwrap()
            [0]["mimeType"],
        "image/png"
    );
    assert!(read_result_images(
        &private,
        &json!([{"path":"../ok.png","mimeType":"image/png"}])
    )
    .is_err());
    assert!(read_result_images(
        &private,
        &json!([{"path":"ok.png","mimeType":"image/jpeg"}])
    )
    .is_err());
    std::fs::write(private.join("large.png"), vec![0; 4 * 1024 * 1024 + 1]).unwrap();
    assert!(read_result_images(
        &private,
        &json!([{"path":"large.png","mimeType":"image/png"}])
    )
    .is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        std::fs::write(root.path().join("outside.png"), b"\x89PNG\r\n\x1a\nprivate").unwrap();
        symlink(root.path().join("outside.png"), private.join("leak.png")).unwrap();
        symlink(root.path(), private.join("escape")).unwrap();
        for path in ["leak.png", "escape/outside.png"] {
            assert!(
                read_result_images(&private, &json!([{"path":path,"mimeType":"image/png"}]))
                    .is_err()
            );
        }
    }
}

#[test]
fn platform_transport_public_manifest_normalizes_required_capabilities_for_review() {
    let manifest = super::super::platform_manifest::PlatformManifest::parse(br#"{"id":"test.alias","name":"Alias","version":"1.0.0","apiVersion":1,"entry":"plugin.mjs","requiredCapabilities":["storage"]}"#).unwrap();
    assert_eq!(
        public_manifest(&manifest)["capabilities"],
        json!(["storage"])
    );
}

#[tokio::test]
async fn platform_transport_event_only_observer_recovers_fast_committed_provenance_without_thread_read(
) {
    let (app, root) = fixture();
    let workspace = app
        .state::<crate::state::AppStateStore>()
        .create_empty_workspace_at_path(root.path().join("project"))
        .0;
    let project = super::super::platform_workflow::project_id(
        &app.state::<crate::state::AppStateStore>()
            .find_workspace(&workspace)
            .unwrap(),
    );
    let db = app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session("fast-thread", &workspace, None, "claude")
        .unwrap();
    let directory = source(root.path(),"observer","1.0.0","export function activate(ctx){ctx.events.onRunFinalized(async event=>{await ctx.storage.set('observed',event.effectsAllowed);});}");
    let manifest_path = directory.join("codemux-plugin.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["capabilities"]
        .as_array_mut()
        .unwrap()
        .push(json!("events.run-finalized"));
    std::fs::write(manifest_path, manifest.to_string()).unwrap();
    let installed = install(&app, &directory).await;
    let active = call(&app,"consent",json!({"installationId":installed["installationId"],"digest":installed["digest"],"grants":{"capabilities":["storage","ui","ui.conversation-cards","events.run-finalized"],"projectIds":[project]}})).await.unwrap();
    let active = call(
        &app,
        "setEnabled",
        json!({"installationId":active["installationId"],"enabled":true}),
    )
    .await
    .unwrap();
    db.append_agent_chat_message("fast-thread",&json!({"type":"turn_completed","thread_id":"fast-thread","turn_id":"fast-run","status":{"kind":"success"}}).to_string()).unwrap();
    let platform = app
        .state::<PluginPlatformState>()
        .get(app.handle())
        .unwrap();
    platform
        .service
        .events
        .recover_committed(app.handle())
        .unwrap();
    let drain = drain_events(app.handle(), &platform);
    let correlate = async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        db.control_append_user_message("fast-thread",&json!({"type":"user_message","text":"Human turn","message_id":"human","client_nonce":"human"}).to_string(),"fast-run",false).unwrap();
    };
    tokio::join!(drain, correlate);
    let record = platform
        .service
        .store
        .read(active["installationId"].as_str().unwrap())
        .unwrap();
    assert_eq!(record.storage.get("observed"), Some(&json!(true)));
    assert_eq!(record.acknowledged_deliveries.len(), 1);
    app.state::<PluginPlatformState>().shutdown().await;
}

#[tokio::test]
async fn platform_transport_explicit_ui_context_never_discloses_ungranted_lineage_or_becomes_caller(
) {
    let (app, root) = fixture();
    let workspace = app
        .state::<crate::state::AppStateStore>()
        .create_empty_workspace_at_path(root.path().join("context-project"))
        .0;
    app.state::<crate::database::DatabaseStore>()
        .upsert_agent_chat_session("context-thread", &workspace, None, "claude")
        .unwrap();
    let directory = source(root.path(),"context","1.0.0","export function activate(ctx){ctx.handle('action:run',(_,i)=>({data:{caller:i.caller,context:i.context}}));}");
    let active = enable(&app, &install(&app, &directory).await).await;
    let result = call(&app,"invoke",json!({"installationId":active["installationId"],"generation":active["generation"],"handler":"action:run","operationId":"context","context":{"threadId":"context-thread"}})).await.unwrap();
    assert_eq!(result["data"]["caller"], json!({"kind":"client"}));
    assert_eq!(
        result["data"]["context"],
        json!({"threadId":"context-thread","lineage":"unresolved"})
    );
    app.state::<PluginPlatformState>().shutdown().await;
}

#[tokio::test]
async fn platform_transport_native_tree_rejects_cross_project_parentage() {
    let (app, root) = fixture();
    let state = app.state::<crate::state::AppStateStore>();
    let first = state
        .create_empty_workspace_at_path(root.path().join("first-project"))
        .0;
    let second = state
        .create_empty_workspace_at_path(root.path().join("second-project"))
        .0;
    let first_project =
        super::super::platform_workflow::project_id(&state.find_workspace(&first).unwrap());
    let second_project =
        super::super::platform_workflow::project_id(&state.find_workspace(&second).unwrap());
    assert_ne!(first_project, second_project);
    app.state::<crate::database::DatabaseStore>()
        .upsert_agent_chat_session("other-project-thread", &second, None, "claude")
        .unwrap();
    let snapshot = json!({"nodes":[{"id":"p","kind":"project","projectId":first_project},{"id":"t","kind":"thread","threadId":"other-project-thread","parentId":"p"}]});
    let code = format!(
        "export function activate(ctx){{ctx.trees.register('tree',async()=>({snapshot}));}}"
    );
    let directory = source(root.path(), "tree-membership", "1.0.0", &code);
    let path = directory.join("codemux-plugin.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["capabilities"]
        .as_array_mut()
        .unwrap()
        .extend([json!("threads.read"), json!("ui.native-trees")]);
    manifest["nativeTrees"] = json!([{"id":"tree","title":"Tree"}]);
    std::fs::write(&path, manifest.to_string()).unwrap();
    let installed = install(&app, &directory).await;
    let active = call(&app,"consent",json!({"installationId":installed["installationId"],"digest":installed["digest"],"grants":{"capabilities":["storage","ui","ui.conversation-cards","threads.read","ui.native-trees"],"projectIds":[first_project,second_project]}})).await.unwrap();
    let active = call(
        &app,
        "setEnabled",
        json!({"installationId":active["installationId"],"enabled":true}),
    )
    .await
    .unwrap();
    assert!(call(&app,"readTree",json!({"installationId":active["installationId"],"generation":active["generation"],"treeId":"tree","operationId":"tree"})).await.unwrap_err().contains("different project"));
    app.state::<PluginPlatformState>().shutdown().await;
}

#[tokio::test]
async fn platform_transport_exact_consent_preserves_disabled_intent_on_install_and_update() {
    let (app, root) = fixture();
    let directory = source(
        root.path(),
        "intent",
        "1.0.0",
        "export function activate(ctx){ctx.handle('action:run',()=>({}));}",
    );
    let installed = install(&app, &directory).await;
    let approve = |installation: &Value| json!({"installationId":installation["installationId"],"digest":installation["pendingUpdate"]["digest"].as_str().unwrap_or_else(||installation["digest"].as_str().unwrap()),"grants":{"capabilities":["storage","ui","ui.conversation-cards"],"projectIds":[]}});
    let first = call(&app, "consent", approve(&installed)).await.unwrap();
    assert_eq!(first["enabled"], false);
    assert_eq!(first["status"], "disabled");
    source(
        root.path(),
        "intent",
        "2.0.0",
        "export function activate(ctx){ctx.handle('action:run',()=>({data:2}));}",
    );
    let staged = call(
        &app,
        "refresh",
        json!({"installationId":first["installationId"]}),
    )
    .await
    .unwrap();
    let updated = call(&app, "consent", approve(&staged)).await.unwrap();
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["status"], "disabled");
    assert_ne!(updated["digest"], first["digest"]);
    assert!(invoke(&app, &updated, "off")
        .await
        .unwrap_err()
        .contains("disabled"));
    app.state::<PluginPlatformState>().shutdown().await;
}

#[tokio::test]
async fn platform_transport_catalogue_exposes_retained_domain_record_after_partial_initial_install()
{
    let (app, root) = fixture();
    let directory = source(
        root.path(),
        "orphan",
        "1.0.0",
        "export function activate(){}",
    );
    let platform = app
        .state::<PluginPlatformState>()
        .get(app.handle())
        .unwrap();
    let snapshot = platform.packages.stage_local(&directory).unwrap();
    platform
        .service
        .register("orphan", "initial", snapshot.manifest, &snapshot.digest)
        .unwrap();
    let catalogue = call(&app, "catalogue", json!({})).await.unwrap();
    let row = catalogue["installations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["installationId"] == "orphan")
        .unwrap();
    assert!(row["recordIssue"]
        .as_str()
        .unwrap()
        .contains("metadata is missing"));
    assert_ne!(row["status"], "idle");
    assert!(platform.service.store.read("orphan").is_ok());
    app.state::<PluginPlatformState>().shutdown().await;
}
