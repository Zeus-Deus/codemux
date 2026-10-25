//! The UI and agent transports share one application-owned plugin platform.
use super::{
    platform::PlatformService,
    platform_authority::{self, PlatformCaller},
    platform_management::{
        installation_id, ManagementRecord, ManagementStore, PackageReview, PreviousConfiguration,
    },
    platform_manifest::{safe_package_path, valid_identifier, PLATFORM_CAPABILITIES},
    platform_package::{PlatformPackageSource, PlatformPackageStore, PlatformSnapshot},
    platform_runtime::{
        PluginExecutionInput, PluginHostCallback, PluginRuntimeConfig, PluginRuntimeError,
        PluginSupervisor, RuntimeCancellation,
    },
    platform_store::{InstallationRecord, PlatformGrants},
};
use crate::{
    agent_control::{ControlAccess, ControlCaller},
    agent_provider::message::CreatedBy,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::Notify;

const MAX_REQUEST_BYTES: usize = 128_000;
const MAX_VIEW_BYTES: u64 = 1_000_000;

#[derive(Default)]
pub struct PluginPlatformState {
    platform: OnceLock<Result<Arc<PluginPlatform>, String>>,
    wake: Arc<Notify>,
    started: std::sync::atomic::AtomicBool,
    stopping: std::sync::atomic::AtomicBool,
    recovery_needed: std::sync::atomic::AtomicBool,
    committed_threads: std::sync::Mutex<std::collections::BTreeSet<String>>,
}
pub struct PluginPlatform {
    root: PathBuf,
    service: Arc<PlatformService>,
    packages: PlatformPackageStore,
    management: ManagementStore,
    supervisor: PluginSupervisor,
    mutations: tokio::sync::Mutex<()>,
}

impl PluginPlatformState {
    fn get<R: Runtime>(&self, app: &AppHandle<R>) -> Result<Arc<PluginPlatform>, String> {
        self.platform
            .get_or_init(|| {
                let root = super::app_data_dir()
                    .ok_or("Plugin data directory is unavailable")?
                    .join("plugin-platform");
                let resources = app.path().resource_dir().ok();
                let config = PluginRuntimeConfig {
                    executable: super::platform_runtime::resolve_plugin_host(resources.as_deref()),
                    ..Default::default()
                };
                Ok(Arc::new(PluginPlatform {
                    service: Arc::new(PlatformService::open(root.join("state"))),
                    packages: PlatformPackageStore::new(root.join("packages")),
                    management: ManagementStore::new(root.join("management")),
                    supervisor: PluginSupervisor::new(config),
                    root,
                    mutations: tokio::sync::Mutex::new(()),
                }))
            })
            .clone()
    }
    pub async fn shutdown(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.wake.notify_one();
        if let Some(Ok(platform)) = self.platform.get() {
            platform.supervisor.shutdown().await;
        }
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn field<'a>(params: &'a Value, name: &str, max: usize) -> Result<&'a str, String> {
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= max)
        .ok_or_else(|| format!("invalid_input: {name} is required and must be bounded text"))
}
fn identifier<'a>(params: &'a Value, name: &str) -> Result<&'a str, String> {
    let id = field(params, name, 128)?;
    valid_identifier(id)
        .then_some(id)
        .ok_or_else(|| format!("invalid_input: Invalid {name}"))
}
fn admin<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller) -> Result<(), String> {
    crate::agent_control::service::authorize(app, caller).map_err(|e| e.to_string())?;
    if caller.access != ControlAccess::FullAccess || caller.created_by != CreatedBy::User {
        return Err(
            "permission_denied: Plugin management requires a person with full application access"
                .into(),
        );
    }
    Ok(())
}
fn source_json(source: &PlatformPackageSource) -> Value {
    serde_json::to_value(source).unwrap_or(Value::Null)
}

fn public_manifest(manifest: &super::platform_manifest::PlatformManifest) -> Value {
    let mut value = serde_json::to_value(manifest).unwrap_or(Value::Null);
    let required: std::collections::BTreeSet<_> = manifest
        .capabilities
        .iter()
        .chain(&manifest.required_capabilities)
        .cloned()
        .collect();
    value["capabilities"] = json!(required);
    value
}

impl PluginPlatform {
    fn visible(&self, meta: &ManagementRecord, record: &InstallationRecord) -> Value {
        let compatibility = record.manifest.compatibility();
        let status = if !compatibility.available {
            "incompatible"
        } else if record.consented_digest.as_deref() != Some(record.digest.as_str()) {
            "needs-consent"
        } else if meta.current.digest != record.digest {
            "error"
        } else if !record.enabled {
            "disabled"
        } else if record
            .unavailable_reason
            .as_ref()
            .is_some_and(|e| e.starts_with("quarantined:"))
        {
            "quarantined"
        } else if record.unavailable_reason.is_some() {
            "error"
        } else {
            "idle"
        };
        json!({
            "installationId":record.installation_id, "generation":record.generation,
            "dataOwner":meta.data_owner.as_deref().unwrap_or(&record.installation_id), "manifest":public_manifest(&record.manifest),
            "source":source_json(&meta.current.source), "digest":record.digest,
            "enabled":record.enabled, "consentedDigest":record.consented_digest,
            "status":status, "error":if meta.current.digest != record.digest {
                Some("Plugin package promotion is incomplete; review the retained snapshot again".to_string())
            } else { record.unavailable_reason.clone() }, "observerError":record.delivery_error,
            "compatibility":compatibility, "grants":record.grants,
            "settings":PlatformService::public_settings(record), "updatedAt":meta.updated_at,
            "pendingUpdate":meta.pending_update.as_ref().map(|p|json!({"kind":p.kind.as_deref().unwrap_or("upstream"),
                "manifest":public_manifest(&p.manifest),"source":source_json(&p.source),"digest":p.digest,
                "generation":format!("pending-{}",p.digest),"updatedAt":meta.updated_at})),
            "recovery":{"activeSource":if meta.publisher_source.is_some(){"local-repair"}else{"upstream"},
                "upstreamSource":meta.publisher_source.as_ref().map(source_json),
                "previous":meta.previous.as_ref().map(|p|json!({"version":p.package.manifest.version,"digest":p.package.digest,"source":source_json(&p.package.source)})),
                "repair":meta.repair_directory.as_ref().map(|directory|json!({"workspaceId":format!("repair-{}",record.installation_id),
                    "directory":directory,"baseDigest":record.digest,"baseGeneration":record.generation,"draft":repair_draft(record)}))}
        })
    }
    fn installation(&self, id: &str) -> Result<Value, String> {
        let meta = self.management.read(id)?;
        if meta.removed {
            return Err("not_found: Plugin was removed".into());
        }
        Ok(self.visible(&meta, &self.service.store.read(id)?))
    }
    fn catalogue<R: Runtime>(&self, app: &AppHandle<R>) -> Value {
        let mut installations = Vec::new();
        let mut records: BTreeMap<_, _> = self.management.list().into_iter().collect();
        for id in self.service.store.ids() {
            records.entry(id).or_insert_with(|| Err("unavailable: Package review metadata is missing; reinstall the same source to recover retained plugin data".into()));
        }
        for (id, meta) in records {
            match meta.and_then(|meta| {
                if meta.removed { return Ok(None); }
                let record = self.service.store.read(&id)?;
                Ok(Some(self.visible(&meta, &record)))
            }) {
                Ok(Some(record)) => installations.push(record),
                Ok(None) => {},
                Err(error) => installations.push(json!({"installationId":id,"generation":"unavailable","dataOwner":id,
                    "recordIssue":error,"manifest":{"id":id,"name":"Unavailable plugin","version":"0.0.0","apiVersion":0,
                    "entry":"","capabilities":[],"actions":[],"views":[],"settings":[]},
                    "source":{"kind":"local","directory":""},"digest":"","enabled":false,"consentedDigest":null,
                    "status":"incompatible","error":error,"grants":{"capabilities":[],"projectIds":[]},"settings":{},"updatedAt":""})),
            }
        }
        json!({"executionDisabled":execution_disabled(app),"installations":installations,
            "recoverySupported":true,"changesSupported":true,"conversationCardsSupported":true,
            "apiVersion":1,"capabilities":PLATFORM_CAPABILITIES,
            "projectOptions":super::platform_workflow::project_options(app)})
    }
    async fn stage(&self, source: &Value) -> Result<PlatformSnapshot, String> {
        match field(source, "kind", 32)? {
            "local" => {
                let packages = self.packages.clone();
                let path = PathBuf::from(field(source, "directory", 4096)?);
                tokio::task::spawn_blocking(move || packages.stage_local(&path))
                    .await
                    .map_err(|_| "Plugin snapshot worker failed")?
            }
            "git" => {
                self.packages
                    .stage_git(
                        field(source, "url", 4096)?,
                        source.get("ref").and_then(Value::as_str),
                        source.get("subdir").and_then(Value::as_str),
                    )
                    .await
            }
            _ => Err("invalid_source: Choose a local directory or HTTPS Git repository".into()),
        }
    }
    async fn stage_source(
        &self,
        source: &PlatformPackageSource,
    ) -> Result<PlatformSnapshot, String> {
        self.stage(&source_json(source)).await
    }
    async fn install<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        caller: &ControlCaller,
        source: &Value,
    ) -> Result<Value, String> {
        admin(app, caller)?;
        let snapshot = self.stage(source).await?;
        admin(app, caller)?;
        let id = installation_id(&snapshot.manifest, &snapshot.source)?;
        let _mutation = self.mutations.lock().await;
        admin(app, caller)?;
        let review = PackageReview::from(snapshot);
        // Missing state is distinct from corrupt, unreadable, or future state.
        let previous = self.management.read_optional(&id)?;
        if let Some(mut meta) = previous.filter(|m| !m.removed) {
            if meta.current.digest != review.digest {
                meta.pending_update = Some(review);
                meta.pending_configuration = None;
                meta.updated_at = now();
                self.management.update(&id, |_| Ok((meta, ())))?;
            }
        } else {
            let mut meta = self.management.read(&id).unwrap_or(ManagementRecord {
                schema_version: 1,
                installation_id: id.clone(),
                data_owner: Some(uuid::Uuid::new_v4().to_string()),
                current: review.clone(),
                pending_update: None,
                pending_configuration: None,
                previous: None,
                repair_directory: None,
                publisher_source: None,
                removed: false,
                updated_at: now(),
            });
            meta.current = review.clone();
            meta.pending_update = None;
            meta.removed = false;
            meta.updated_at = now();
            self.service.register(
                &id,
                &uuid::Uuid::new_v4().to_string(),
                review.manifest.clone(),
                &review.digest,
            )?;
            self.service.set_enabled(&id, false)?;
            self.management.update(&id, |_| Ok((meta, ())))?;
        }
        changed(app, &id);
        self.installation(&id)
    }
    async fn consent<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        caller: &ControlCaller,
        params: &Value,
    ) -> Result<Value, String> {
        admin(app, caller)?;
        let id = identifier(params, "installationId")?;
        let digest = field(params, "digest", 64)?;
        let grants: PlatformGrants = serde_json::from_value(
            params
                .get("grants")
                .cloned()
                .ok_or("invalid_input: grants required")?,
        )
        .map_err(|_| "invalid_input: Invalid project/capability grants")?;
        let _mutation = self.mutations.lock().await;
        admin(app, caller)?;
        let mut meta = self.management.read(id)?;
        if meta.removed {
            return Err("not_found: Plugin was removed".into());
        }
        let review = meta
            .pending_update
            .clone()
            .unwrap_or_else(|| meta.current.clone());
        if review.digest != digest {
            return Err("stale_digest: Review the exact pending plugin snapshot".into());
        }
        platform_authority::validate_grants(&review.manifest, &grants)?;
        if !review.manifest.compatibility().available {
            return Err("incompatible: Required plugin API features are unsupported".into());
        }
        self.packages.verify(digest)?;
        let old = self.service.store.read(id)?;
        let restore = meta.pending_configuration.clone();
        // Reject stale/invalid choices atomically before stopping code or changing review state.
        // The durable hold also makes every partial promotion failure unavailable on restart.
        self.begin_transition(id, params)?;
        if old.digest != digest {
            // Save rollback configuration before revoking the old generation. Plugin data and
            // effect receipts are deliberately never copied back during code rollback.
            meta.previous = Some(PreviousConfiguration::capture(meta.current.clone(), &old));
            self.management.update(id, |_| Ok((meta.clone(), ())))?;
        }
        self.supervisor.stop(id).await;
        admin(app, caller)?;
        self.service.register(
            id,
            &uuid::Uuid::new_v4().to_string(),
            review.manifest.clone(),
            digest,
        )?;
        // Registration can require a new backlog choice for a changed digest. Keep
        // the reviewed decision while the durable transition hold still blocks code.
        let latest = self.service.events.latest()?;
        self.service
            .store
            .update(id, |record| apply_recovery(record, params, latest))?;
        self.service.consent_pending(id, digest, grants)?;
        if let Some(restore) = restore {
            self.service.store.update(id, |record| {
                record.settings = record
                    .manifest
                    .settings
                    .iter()
                    .filter(|s| s.kind != "secret")
                    .filter_map(|s| {
                        old.settings
                            .get(&s.key)
                            .or_else(|| restore.settings.get(&s.key))
                            .filter(|v| match s.kind.as_str() {
                                "string" => v.is_string(),
                                "number" => v.is_number(),
                                "boolean" => v.is_boolean(),
                                _ => false,
                            })
                            .map(|v| (s.key.clone(), v.clone()))
                    })
                    .collect();
                record.secrets = record
                    .manifest
                    .settings
                    .iter()
                    .filter(|s| s.kind == "secret")
                    .filter_map(|s| {
                        old.secrets
                            .get(&s.key)
                            .or_else(|| restore.secrets.get(&s.key))
                            .map(|v| (s.key.clone(), v.clone()))
                    })
                    .collect();
                Ok(())
            })?;
        }
        self.packages.promote(id, digest, digest)?;
        if review.kind.as_deref() == Some("local-repair") && meta.publisher_source.is_none() {
            meta.publisher_source = Some(meta.current.source.clone());
        }
        if review.kind.as_deref() == Some("upstream") {
            meta.publisher_source = None;
        }
        meta.current = review;
        meta.pending_update = None;
        meta.pending_configuration = None;
        meta.updated_at = now();
        self.management.update(id, |_| Ok((meta, ())))?;
        self.finish_transition(id, old.enabled)?;
        self.supervisor.resume(id).await;
        changed(app, id);
        self.installation(id)
    }
    fn begin_transition(&self, id: &str, params: &Value) -> Result<(), String> {
        validate_recovery(params)?;
        let latest = self.service.events.latest()?;
        self.service.update_authority(id, |record| {
            apply_recovery(record, params, latest)?;
            record.unavailable_reason = Some(
                "Plugin management change is incomplete; review or resume it in Settings".into(),
            );
            Ok(())
        })?;
        Ok(())
    }
    fn finish_transition(&self, id: &str, enabled: bool) -> Result<(), String> {
        self.service
            .events
            .clear_empty_gap(&self.service.store, id)?;
        self.service.update_authority(id, |record| {
            record.enabled = enabled;
            record.unavailable_reason = None;
            Ok(())
        })?;
        Ok(())
    }

    fn admit_invocation<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        control: &ControlCaller,
        id: &str,
        generation: &str,
        handler: &str,
    ) -> Result<InstallationRecord, String> {
        crate::agent_control::service::authorize(app, control).map_err(|e| e.to_string())?;
        if execution_disabled(app) {
            return Err("disabled: Plugin execution is paused for this launch".into());
        }
        let meta = self.management.read(id)?;
        if meta.removed {
            return Err("not_found: Plugin was removed".into());
        }
        let record = self.service.store.read(id)?;
        if meta.current.digest != record.digest {
            return Err(
                "unavailable: Plugin package review is incomplete; review or repair it in Settings"
                    .into(),
            );
        }
        let caller = PlatformCaller::new(id.into(), generation.into(), control.clone(), None, None);
        platform_authority::assert_active(&record, &caller)?;
        if let Some(action) = handler.strip_prefix("action:") {
            let action = record
                .manifest
                .actions
                .iter()
                .find(|a| a.id == action)
                .ok_or("not_found: Plugin action is not declared")?;
            if control.created_by == CreatedBy::Agent && !action.agent_callable {
                return Err("permission_denied: This action is not exposed to agents".into());
            }
        } else if let Some(status) = handler.strip_prefix("status:") {
            if control.created_by != CreatedBy::User
                || !record.manifest.thread_status.iter().any(|s| s.id == status)
            {
                return Err("permission_denied: Undeclared thread status".into());
            }
            platform_authority::capability(&record, &caller, "ui", None)?;
        } else if let Some(tree) = handler.strip_prefix("tree:") {
            if control.created_by != CreatedBy::User
                || !record.manifest.native_trees.iter().any(|s| s.id == tree)
            {
                return Err("permission_denied: Undeclared native tree".into());
            }
            platform_authority::capability(&record, &caller, "ui.native-trees", None)?;
        } else if let Some(view) = handler.strip_prefix("view:") {
            let (id, name) = view
                .split_once(':')
                .ok_or("invalid_input: View handler required")?;
            if control.created_by != CreatedBy::User
                || !valid_identifier(name)
                || !record.manifest.views.iter().any(|v| v.id == id)
            {
                return Err("permission_denied: Undeclared view handler".into());
            }
            platform_authority::capability(&record, &caller, "ui", None)?;
        } else if handler == "event:run.finalized" {
            if control.created_by != CreatedBy::System {
                return Err("permission_denied: Only the host delivers committed events".into());
            }
            platform_authority::capability(&record, &caller, "events.run-finalized", None)?;
        } else {
            return Err("invalid_input: Unsupported plugin handler".into());
        }
        Ok(record)
    }

    async fn invoke<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        control: ControlCaller,
        params: &Value,
        event_delivery: Option<String>,
    ) -> Result<Value, String> {
        let id = identifier(params, "installationId")?.to_string();
        let generation = identifier(params, "generation")?.to_string();
        let handler = field(params, "handler", 384)?.to_string();
        let operation_id = field(params, "operationId", 128)?.to_string();
        if !valid_identifier(&operation_id) {
            return Err("invalid_input: Invalid operation identity".into());
        }
        let record = self.admit_invocation(app, &control, &id, &generation, &handler)?;
        let packages = self.packages.clone();
        let digest = record.digest.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || packages.verify(&digest))
            .await
            .map_err(|_| "unavailable: Plugin verification worker failed")?
        {
            let _ = self.service.mark_unavailable(&id, Some(error.clone()));
            changed(app, &id);
            return Err(error);
        }
        self.admit_invocation(app, &control, &id, &generation, &handler)?;
        let mut context = params.get("context").filter(|v| !v.is_null()).cloned();
        if let Some(context) = &mut context {
            let object = context
                .as_object_mut()
                .ok_or("invalid_input: Plugin action context must be an object")?;
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "projectId" | "threadId" | "messageId" | "selection"
                )
            }) {
                return Err("invalid_input: Unknown action context fields".into());
            }
            for (key, value) in object.iter() {
                if !value
                    .as_str()
                    .is_some_and(|v| v.len() <= if key == "selection" { 64_000 } else { 4096 })
                {
                    return Err("invalid_input: Invalid action context".into());
                }
            }
            if let Some(thread) = object
                .get("threadId")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                let project = super::platform_workflow::thread_project(app, &thread)?;
                let context_owner = PlatformCaller::new(
                    id.clone(),
                    generation.clone(),
                    control.clone(),
                    None,
                    None,
                );
                let readable = self
                    .service
                    .authorize(app, &context_owner, "threads.read", Some(&project))
                    .is_ok();
                if object
                    .get("projectId")
                    .is_some_and(|v| v.as_str() != Some(project.as_str()))
                {
                    return Err("invalid_input: Thread and project context disagree".into());
                }
                // Explicit target context is caller-supplied presentation data, not
                // a grant or source identity. Enrich it only with readable metadata.
                if readable {
                    object.insert("projectId".into(), json!(project));
                    self.add_lineage(app, &context_owner, &thread, object);
                } else {
                    object.insert("lineage".into(), json!("unresolved"));
                }
            }
        }
        let mut caller_json = json!({"kind":match control.created_by{CreatedBy::Agent=>"agent",CreatedBy::System=>"background",CreatedBy::User=>"client"}});
        if let Some(thread) = control.thread_id() {
            caller_json["threadId"] = json!(thread);
            if let Some(object) = caller_json.as_object_mut() {
                let caller_owner = PlatformCaller::new(
                    id.clone(),
                    generation.clone(),
                    control.clone(),
                    None,
                    None,
                );
                self.add_lineage(app, &caller_owner, thread, object);
            }
        }
        let target_thread = context
            .as_ref()
            .and_then(|v| v["threadId"].as_str())
            .map(str::to_owned);
        if handler.starts_with("status:") && target_thread.is_none() {
            return Err("invalid_input: A thread status requires a target thread".into());
        }
        let event_projects = if event_delivery.is_some() {
            Some(vec![context
                .as_ref()
                .and_then(|context| context["projectId"].as_str())
                .ok_or("invalid_input: A committed event requires its project context")?
                .to_string()])
        } else {
            None
        };
        let owner = PlatformCaller::new(
            id.clone(),
            generation.clone(),
            control.clone(),
            event_projects,
            target_thread,
        )
        .with_operation_id(operation_id.clone());
        let owner = if let Some(delivery) = event_delivery {
            owner.for_delivery(delivery)
        } else {
            owner
        };
        let platform = self.clone();
        let host_app = app.clone();
        let owner_for_host = owner.clone();
        let host: PluginHostCallback = Arc::new(move |call| {
            let platform = platform.clone();
            let app = host_app.clone();
            let owner = owner_for_host.clone();
            Box::pin(async move {
                if call.cancellation.is_cancelled() {
                    return Err(PluginRuntimeError::new(
                        "cancelled",
                        "Plugin invocation ended",
                    ));
                }
                let owner = owner.with_cancellation(call.cancellation.clone());
                let input = call.input;
                // The domain's receipt identity includes authenticated principal and this
                // host-captured operation id; never rewrite an author's stable effect key.
                if call.method == "ui.invalidate" {
                    return platform
                        .invalidate(&app, &owner, &input)
                        .map_err(runtime_error);
                }
                tokio::select! {biased;
                    _=call.cancellation.cancelled()=>Err(PluginRuntimeError::new("cancelled","Plugin invocation ended")),
                    result=platform.service.dispatch(&app,&owner,&call.method,input)=>result.map_err(runtime_error),
                }
            })
        });
        let result = self
            .supervisor
            .invoke(
                PluginExecutionInput {
                    installation_id: id.clone(),
                    generation: generation.clone(),
                    directory: self.packages.snapshot_directory(&record.digest)?,
                    entry: record.manifest.entry.clone(),
                    data_directory: self.service.store.data_directory(&id)?,
                    handler: handler.clone(),
                    operation_id,
                    input: params.get("input").cloned().unwrap_or(Value::Null),
                    caller: caller_json,
                    context,
                    capabilities: record.grants.capabilities.clone(),
                },
                host,
                RuntimeCancellation::default(),
            )
            .await;
        match result {
            Ok(result) => {
                self.admit_invocation(app, &control, &id, &generation, &handler)?;
                let value = self.result(app, &owner, &record, result).await?;
                self.admit_invocation(app, &control, &id, &generation, &handler)?;
                Ok(value)
            }
            Err(error) => {
                if matches!(
                    error.reason.as_str(),
                    "crashed"
                        | "start_failed"
                        | "timeout"
                        | "invalid_result"
                        | "quarantined"
                        | "protocol_error"
                ) {
                    // Revoke only the affected installation; the enabled decision survives.
                    let _ = self.service.mark_unavailable(&id, Some(error.to_string()));
                    changed(app, &id);
                }
                Err(error.to_string())
            }
        }
    }
    fn invalidate<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        caller: &PlatformCaller,
        input: &Value,
    ) -> Result<Value, String> {
        let record = self.service.authorize(app, caller, "ui.changes", None)?;
        let topics = input
            .get("topics")
            .and_then(Value::as_array)
            .filter(|v| v.len() <= 64)
            .ok_or("invalid_input: Bounded contribution topics required")?;
        for topic in topics {
            let topic = topic.as_str().ok_or("invalid_input: Topic must be text")?;
            let valid = if let Some(id) = topic.strip_prefix("tree:") {
                record
                    .grants
                    .capabilities
                    .iter()
                    .any(|c| c == "ui.native-trees")
                    && record.manifest.native_trees.iter().any(|v| v.id == id)
            } else if let Some(id) = topic.strip_prefix("view:") {
                record.grants.capabilities.iter().any(|c| c == "ui")
                    && record.manifest.views.iter().any(|v| v.id == id)
            } else if let Some(id) = topic.strip_prefix("status:") {
                record.grants.capabilities.iter().any(|c| c == "ui")
                    && record.manifest.thread_status.iter().any(|v| v.id == id)
            } else {
                false
            };
            if !valid {
                return Err(
                    "permission_denied: Topic does not name an authorized contribution".into(),
                );
            }
        }
        for topic in topics {
            let _=app.emit("plugins_platform_changed",json!({"installationId":caller.installation_id(),"generation":caller.generation(),"topic":topic}));
        }
        Ok(Value::Null)
    }

    async fn result<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        owner: &PlatformCaller,
        record: &InstallationRecord,
        raw: Value,
    ) -> Result<Value, String> {
        if raw.is_null() {
            return Ok(json!({}));
        }
        let raw = raw
            .as_object()
            .ok_or("invalid_result: Plugin result must be an object")?;
        if serde_json::to_vec(raw)
            .map_err(|_| "invalid_result: Cannot encode plugin result")?
            .len()
            > MAX_REQUEST_BYTES
        {
            return Err("invalid_result: Plugin result exceeds 128 KiB".into());
        }
        let mut result = serde_json::Map::new();
        if let Some(message) = raw.get("message") {
            if !message.as_str().is_some_and(|s| s.chars().count() <= 4000) {
                return Err("invalid_result: Plugin message exceeds its limit".into());
            }
            result.insert("message".into(), message.clone());
        }
        if let Some(data) = raw.get("data") {
            result.insert("data".into(), data.clone());
        }
        if let Some(card) = raw.get("card").filter(|v| !v.is_null()) {
            platform_authority::capability(record, owner, "ui", None)?;
            platform_authority::capability(record, owner, "ui.conversation-cards", None)?;
            let id = identifier(card, "viewId")?;
            if !record
                .manifest
                .views
                .iter()
                .any(|v| v.id == id && v.placement == "conversation")
            {
                return Err(
                    "invalid_result: Card view must be a declared conversation view".into(),
                );
            }
            let summary = field(card, "summary", 4000)?;
            if summary.chars().count() > 1000
                || serde_json::to_vec(card.get("data").unwrap_or(&Value::Null))
                    .map_err(|_| "invalid_result: Invalid card data")?
                    .len()
                    > 2048
            {
                return Err("invalid_result: Conversation card exceeds its reference limit".into());
            }
            let meta = self.management.read(&record.installation_id)?;
            let mut reference = json!({"viewId":id,"summary":summary,"installationId":record.installation_id,
                "generation":record.generation,"dataOwner":meta.data_owner.as_deref().unwrap_or(&record.installation_id)});
            if let Some(data) = card.get("data") {
                reference["data"] = data.clone();
            }
            result.insert("card".into(), reference);
        }
        let mut unsupported = raw
            .get("unsupportedIntentCount")
            .and_then(Value::as_u64)
            .filter(|count| *count <= 32)
            .unwrap_or(0);
        let mut intents = Vec::new();
        if let Some(requests) = raw.get("intents") {
            let requests = requests
                .as_array()
                .filter(|a| a.len() <= 32)
                .ok_or("invalid_result: Bounded client intents required")?;
            platform_authority::capability(record, owner, "ui", None)?;
            for intent in requests {
                let valid = match intent["type"].as_str() {
                    Some("composer.insert") => {
                        intent["text"].as_str().is_some_and(|s| s.len() <= 64000)
                    }
                    Some("preview.open") => intent["url"]
                        .as_str()
                        .filter(|s| s.len() <= 4096)
                        .and_then(|s| url::Url::parse(s).ok())
                        .is_some_and(|u| {
                            matches!(u.scheme(), "http" | "https")
                                && u.username().is_empty()
                                && u.password().is_none()
                        }),
                    Some("view.open") => intent["viewId"].as_str().is_some_and(|id| {
                        record
                            .manifest
                            .views
                            .iter()
                            .any(|v| v.id == id && v.placement != "conversation")
                    }),
                    Some("thread.reveal") => {
                        if let Some(id) = intent["threadId"].as_str().filter(|s| s.len() <= 4096) {
                            self.service
                                .dispatch(app, owner, "threads.metadata", json!({"threadId":id}))
                                .await
                                .is_ok()
                        } else {
                            false
                        }
                    }
                    _ => {
                        unsupported += 1;
                        false
                    }
                };
                if valid {
                    intents.push(intent.clone());
                } else if matches!(
                    intent["type"].as_str(),
                    Some("composer.insert" | "preview.open" | "view.open" | "thread.reveal")
                ) {
                    return Err(
                        "invalid_result: Plugin returned an invalid or unauthorized client intent"
                            .into(),
                    );
                }
            }
        }
        if !intents.is_empty() {
            result.insert("intents".into(), json!(intents));
        }
        if unsupported > 0 {
            result.insert("unsupportedIntentCount".into(), json!(unsupported));
        }
        if let Some(images) = raw.get("images").filter(|v| !v.is_null()) {
            let directory = self.service.store.data_directory(&record.installation_id)?;
            let images = images.clone();
            let images =
                tokio::task::spawn_blocking(move || read_result_images(&directory, &images))
                    .await
                    .map_err(|_| "invalid_result: Image reader failed")??;
            result.insert("images".into(), images);
        }
        Ok(Value::Object(result))
    }

    fn read_view<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        control: &ControlCaller,
        params: &Value,
        card: bool,
    ) -> Result<Value, String> {
        let id = identifier(params, "installationId")?;
        let generation = identifier(params, "generation")?;
        let meta = self.management.read(id)?;
        let record = self.service.store.read(id)?;
        if meta.removed || meta.current.digest != record.digest {
            return Err("not_found: Plugin view is unavailable".into());
        }
        let owner = PlatformCaller::new(id.into(), generation.into(), control.clone(), None, None);
        crate::agent_control::service::authorize(app, control).map_err(|e| e.to_string())?;
        if execution_disabled(app) {
            return Err("disabled: Plugin execution is paused for this launch".into());
        }
        platform_authority::capability(&record, &owner, "ui", None)?;
        let view_id = if card {
            platform_authority::capability(&record, &owner, "ui.conversation-cards", None)?;
            let reference = params
                .get("card")
                .ok_or("invalid_input: Card reference required")?;
            if reference["dataOwner"].as_str() != Some(meta.data_owner.as_deref().unwrap_or(id))
                || reference["installationId"].as_str() != Some(id)
            {
                return Err(
                    "permission_denied: Card belongs to a different plugin data generation".into(),
                );
            }
            identifier(reference, "viewId")?
        } else {
            identifier(params, "viewId")?
        };
        let view = record
            .manifest
            .views
            .iter()
            .find(|v| v.id == view_id && ((v.placement == "conversation") == card))
            .ok_or("not_found: Plugin view is not declared at this placement")?;
        self.packages.verify(&record.digest)?;
        let directory = self.packages.snapshot_directory(&record.digest)?;
        let path = safe_package_path(&directory, &view.entry)?;
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| "unavailable: Plugin view file is missing")?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_VIEW_BYTES
        {
            return Err("unavailable: Unsafe or oversized plugin view".into());
        }
        let html = std::fs::read_to_string(path)
            .map_err(|_| "unavailable: Plugin view must be UTF-8 HTML")?;
        if html.len() as u64 > MAX_VIEW_BYTES {
            return Err("unavailable: Plugin view exceeds its limit".into());
        }
        Ok(json!({"html":html}))
    }

    async fn read_tree<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        control: ControlCaller,
        params: &Value,
    ) -> Result<Value, String> {
        let id = identifier(params, "installationId")?;
        let generation = identifier(params, "generation")?;
        let tree = identifier(params, "treeId")?;
        let input = json!({"installationId":id,"generation":generation,"handler":format!("tree:{tree}"),
            "operationId":field(params,"operationId",128)?,"input":{}});
        let result = self.invoke(app, control.clone(), &input, None).await?;
        let snapshot = result
            .get("data")
            .ok_or("invalid_result: Tree handler must return a data snapshot")?;
        let nodes = validate_tree(snapshot)?;
        let owner = PlatformCaller::new(id.into(), generation.into(), control.clone(), None, None);
        self.service.authorize(app, &owner, "threads.read", None)?;
        let mut projects = BTreeMap::new();
        for node in nodes {
            let project = match node["kind"].as_str() {
                Some("thread") => Some(super::platform_workflow::thread_project(
                    app,
                    field(node, "threadId", 4096)?,
                )?),
                Some("project") => {
                    let project = field(node, "projectId", 4096)?;
                    if !super::platform_workflow::recovery_workspaces(app)
                        .iter()
                        .any(|(_, id)| id == project)
                    {
                        return Err("permission_denied: Tree project is unavailable".into());
                    }
                    Some(project.to_string())
                }
                _ => None,
            };
            if let Some(project) = project {
                self.service
                    .authorize(app, &owner, "threads.read", Some(&project))?;
                projects.insert(field(node, "id", 128)?, project);
            }
        }
        let parents: BTreeMap<_, _> = nodes
            .iter()
            .map(|node| {
                (
                    node["id"].as_str().unwrap_or_default(),
                    node.get("parentId").and_then(Value::as_str),
                )
            })
            .collect();
        for node in nodes {
            let mut parent = node.get("parentId").and_then(Value::as_str);
            while let Some(id) = parent {
                if let (Some(project), Some(ancestor_project)) = (
                    projects.get(node["id"].as_str().unwrap_or_default()),
                    projects.get(id),
                ) {
                    if project != ancestor_project {
                        return Err(
                            "invalid_result: Tree reference is nested under a different project"
                                .into(),
                        );
                    }
                }
                parent = parents.get(id).copied().flatten();
            }
        }
        self.admit_invocation(app, &control, id, generation, &format!("tree:{tree}"))?;
        Ok(json!({"nodes":nodes}))
    }
    fn add_lineage<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        owner: &PlatformCaller,
        thread: &str,
        object: &mut serde_json::Map<String, Value>,
    ) {
        let db = app.state::<crate::database::DatabaseStore>();
        let mut cursor = thread.to_string();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..64 {
            let readable =
                super::platform_workflow::thread_project(app, &cursor).and_then(|project| {
                    self.service
                        .authorize(app, owner, "threads.read", Some(&project))
                        .map(|_| ())
                });
            if !seen.insert(cursor.clone()) || readable.is_err() {
                object.insert("lineage".into(), json!("unresolved"));
                return;
            }
            match db.delegation_task_for_child(&cursor) {
                Ok(Some(task)) => cursor = task.parent_thread_id,
                Ok(None) => {
                    if cursor != thread {
                        object.insert("subagentOf".into(), json!(cursor));
                    }
                    return;
                }
                Err(_) => {
                    object.insert("lineage".into(), json!("unresolved"));
                    return;
                }
            }
        }
        object.insert("lineage".into(), json!("unresolved"));
    }
}

fn validate_tree(snapshot: &Value) -> Result<&Vec<Value>, String> {
    let nodes = snapshot["nodes"]
        .as_array()
        .filter(|nodes| nodes.len() <= 2000)
        .ok_or("invalid_result: Tree requires at most 2000 nodes")?;
    if !snapshot.as_object().is_some_and(|object| object.len() == 1)
        || snapshot.to_string().len() > 128000
    {
        return Err("invalid_result: Invalid or oversized tree snapshot".into());
    }
    let mut parents = BTreeMap::new();
    let mut references = std::collections::BTreeSet::new();
    for node in nodes {
        let node_id = identifier(node, "id")?;
        let kind = field(node, "kind", 16)?;
        let specific = match kind {
            "group" => {
                field(node, "title", 200)?;
                "title"
            }
            "thread" => {
                field(node, "threadId", 4096)?;
                "threadId"
            }
            "project" => {
                field(node, "projectId", 4096)?;
                "projectId"
            }
            _ => return Err("invalid_result: Unknown tree node kind".into()),
        };
        if !node.as_object().is_some_and(|object| {
            object.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "id" | "kind" | "parentId" | "collapsedByDefault"
                ) || key == specific
            })
        }) || node
            .get("collapsedByDefault")
            .is_some_and(|v| !v.is_boolean())
        {
            return Err("invalid_result: Invalid tree node shape".into());
        }
        if kind != "group" && !references.insert((kind, field(node, specific, 4096)?)) {
            return Err("invalid_result: Duplicate tree reference".into());
        }
        let parent = node
            .get("parentId")
            .map(|_| identifier(node, "parentId"))
            .transpose()?;
        if parents.insert(node_id, parent).is_some() {
            return Err("invalid_result: Duplicate tree node".into());
        }
    }
    for node in parents.keys() {
        let mut next = Some(*node);
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = next {
            if seen.len() >= 16 || !seen.insert(id) {
                return Err("invalid_result: Tree is cyclic or exceeds 16 levels".into());
            }
            next = *parents
                .get(id)
                .ok_or("invalid_result: Tree parent is missing")?;
        }
    }
    Ok(nodes)
}

fn read_result_images(directory: &Path, images: &Value) -> Result<Value, String> {
    use base64::Engine;
    use std::io::Read;
    let images = images
        .as_array()
        .filter(|v| v.len() <= 4)
        .ok_or("invalid_result: At most four result images are supported")?;
    let root = directory
        .canonicalize()
        .map_err(|_| "invalid_result: Plugin data directory is unavailable")?;
    let mut output = Vec::new();
    for image in images {
        let file = open_private_image(&root, field(image, "path", 512)?)?;
        let metadata = file
            .metadata()
            .map_err(|_| "invalid_result: Cannot inspect plugin image")?;
        if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
            return Err("invalid_result: Image exceeds 4 MiB".into());
        }
        let mut bytes = Vec::new();
        file.take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "invalid_result: Image read failed")?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("invalid_result: Image exceeds 4 MiB".into());
        }
        let mime = field(image, "mimeType", 32)?;
        if !match mime {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
            _ => false,
        } {
            return Err(
                "invalid_result: Image signature does not match its supported MIME type".into(),
            );
        }
        output.push(
            json!({"mimeType":mime,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}),
        );
    }
    Ok(json!(output))
}

/// Resolve each component relative to an already-open private directory. A worker
/// swapping an intermediate directory for a symlink cannot redirect the read.
#[cfg(unix)]
fn open_private_image(root: &Path, relative: &str) -> Result<std::fs::File, String> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        },
    };
    safe_package_path(root, relative)?;
    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)
        .map_err(|_| "invalid_result: Private image directory is unavailable")?;
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        let name = CString::new(part).map_err(|_| "invalid_result: Invalid image path")?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if parts.peek().is_some() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(
                "invalid_result: Image must be a private regular file without symlinks".into(),
            );
        }
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        if parts.peek().is_none() {
            return Ok(file);
        }
        directory = file;
    }
    Err("invalid_result: Invalid image path".into())
}
#[cfg(not(unix))]
fn open_private_image(root: &Path, relative: &str) -> Result<std::fs::File, String> {
    let path = safe_package_path(root, relative)?;
    let mut component = root.to_path_buf();
    for part in relative.split('/') {
        component.push(part);
        if std::fs::symlink_metadata(&component)
            .map_err(|_| "invalid_result: Image unavailable")?
            .file_type()
            .is_symlink()
        {
            return Err("invalid_result: Image path must not contain symlinks".into());
        }
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "invalid_result: Image unavailable")?;
    if !canonical.starts_with(root) {
        return Err("invalid_result: Image escapes private data".into());
    }
    std::fs::File::open(canonical).map_err(|_| "invalid_result: Cannot open image".into())
}

fn runtime_error(message: String) -> PluginRuntimeError {
    let (reason, text) = message
        .split_once(": ")
        .unwrap_or(("operation_failed", message.as_str()));
    let mut error = PluginRuntimeError::new(reason, text);
    if reason == "idempotency_conflict" {
        error.code = Some(reason.into());
    }
    error
}
fn repair_draft(record: &InstallationRecord) -> String {
    format!("Repair plugin {} ({}) against Codemux plugin API v1. Keep data formats compatible, preserve history, and test the package before review. The editable copy is separate from running code. Settings, secrets and plugin data are intentionally omitted.",record.manifest.name,record.manifest.version)
}
fn changed<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let _ = app.emit(
        "plugins_platform_catalogue_changed",
        json!({"installationId":id}),
    );
}

/// Native callers pass established identity. The JSON request never carries authority.
pub fn request<'a, R: Runtime>(
    app: &'a AppHandle<R>,
    caller: ControlCaller,
    method: &'a str,
    params: Value,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send + 'a>> {
    Box::pin(request_inner(app, caller, method, params))
}

async fn request_inner<R: Runtime>(
    app: &AppHandle<R>,
    caller: ControlCaller,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    if !params.is_object()
        || serde_json::to_vec(&params)
            .map_err(|_| "invalid_input: Invalid request")?
            .len()
            > MAX_REQUEST_BYTES
    {
        return Err("invalid_input: Plugin request must be a bounded JSON object".into());
    }
    crate::agent_control::service::authorize(app, &caller).map_err(|e| e.to_string())?;
    let state = app
        .try_state::<PluginPlatformState>()
        .ok_or("unavailable: Plugin platform is not registered")?;
    let platform = match state.get(app) {
        Ok(platform) => platform,
        Err(error) if method == "catalogue" => {
            return Ok(json!({"installations":[],"unavailable":{"message":error}}))
        }
        Err(error) => return Err(error),
    };
    if method == "catalogue" {
        return Ok(platform.catalogue(app));
    }
    if method == "invoke" {
        return platform.invoke(app, caller, &params, None).await;
    }
    if method == "readView" || method == "readCardView" {
        return platform.read_view(app, &caller, &params, method == "readCardView");
    }
    if method == "readTree" {
        return platform.read_tree(app, caller, &params).await;
    }
    admin(app, &caller)?;
    if method == "install" {
        return platform
            .install(
                app,
                &caller,
                params
                    .get("source")
                    .ok_or("invalid_input: source required")?,
            )
            .await;
    }
    if method == "consent" {
        validate_recovery(&params)?;
        let recovery = platform.clone();
        let recovery_app = app.clone();
        tokio::task::spawn_blocking(move || {
            recovery.service.events.recover_committed(&recovery_app)
        })
        .await
        .map_err(|_| "Plugin event recovery failed")??;
        let result = platform.consent(app, &caller, &params).await?;
        state.wake.notify_one();
        return Ok(result);
    }
    let id = identifier(&params, "installationId")?.to_string();
    if method == "backlog" {
        platform.service.events.recover_committed(app)?;
        return platform
            .service
            .events
            .backlog(&platform.service.store.read(&id)?);
    }
    if method == "refresh" {
        let meta = platform.management.read(&id)?;
        if meta.removed {
            return Err("not_found: Plugin was removed".into());
        }
        let mut review = PackageReview::from(
            platform
                .stage_source(
                    meta.publisher_source
                        .as_ref()
                        .unwrap_or(&meta.current.source),
                )
                .await?,
        );
        admin(app, &caller)?;
        let _mutation = platform.mutations.lock().await;
        if review.manifest.id != meta.current.manifest.id {
            return Err("invalid_source: Publisher update changed the plugin identity".into());
        }
        review.kind = Some("upstream".into());
        platform.management.update(&id, |previous| {
            let mut current = previous.ok_or("not_found: Plugin was removed")?;
            if current.current.digest != meta.current.digest || current.removed {
                return Err("stale_generation: Plugin changed while checking source".into());
            }
            current.pending_update = (review.digest != current.current.digest).then_some(review);
            current.pending_configuration = None;
            current.updated_at = now();
            Ok((current, ()))
        })?;
        changed(app, &id);
        return platform.installation(&id);
    }
    if method == "prepareRepair" {
        let meta = platform.management.read(&id)?;
        let record = platform.service.store.read(&id)?;
        if meta.removed {
            return Err("not_found: Plugin was removed".into());
        }
        let directory =
            platform
                .root
                .join("repairs")
                .join(format!("{}-{}", id, uuid::Uuid::new_v4()));
        platform
            .packages
            .prepare_repair(&record.digest, &directory)
            .await?;
        admin(app, &caller)?;
        platform.management.update(&id, |previous| {
            let mut meta = previous.ok_or("not_found: Plugin was removed")?;
            if meta.current.digest != record.digest || meta.removed {
                return Err("stale_generation: Plugin changed during repair preparation".into());
            }
            meta.repair_directory = Some(directory.to_string_lossy().into());
            meta.updated_at = now();
            Ok((meta, ()))
        })?;
        changed(app, &id);
        return Ok(
            json!({"workspaceId":format!("repair-{id}"),"directory":directory,"baseDigest":record.digest,"baseGeneration":record.generation,"draft":repair_draft(&record)}),
        );
    }
    if method == "stageRepair" {
        let meta = platform.management.read(&id)?;
        if field(&params, "workspaceId", 256)? != format!("repair-{id}")
            || field(&params, "baseDigest", 64)? != meta.current.digest
        {
            return Err(
                "stale_generation: Repair baseline does not match this installation".into(),
            );
        }
        let directory = meta
            .repair_directory
            .as_ref()
            .ok_or("not_found: No repair workspace")?;
        let mut review = PackageReview::from(
            platform
                .stage(&json!({"kind":"local","directory":directory}))
                .await?,
        );
        admin(app, &caller)?;
        if review.manifest.id != meta.current.manifest.id {
            return Err("invalid_source: Local repair changed the plugin identity".into());
        }
        review.kind = Some("local-repair".into());
        platform.management.update(&id, |previous| {
            let mut current = previous.ok_or("not_found: Plugin was removed")?;
            if current.current.digest != meta.current.digest || current.removed {
                return Err("stale_generation: Plugin changed during repair".into());
            }
            current.pending_update = Some(review);
            current.pending_configuration = None;
            current.updated_at = now();
            Ok((current, ()))
        })?;
        changed(app, &id);
        return platform.installation(&id);
    }
    let _mutation = platform.mutations.lock().await;
    admin(app, &caller)?;
    let meta = platform.management.read(&id)?;
    if meta.removed {
        return Err("not_found: Plugin was removed".into());
    }
    match method {
        "discardPendingUpdate" => {
            if platform.service.store.read(&id)?.digest != meta.current.digest {
                return Err(
                    "unavailable: Complete the retained snapshot review before discarding it"
                        .into(),
                );
            }
            platform.management.update(&id, |previous| {
                let mut meta = previous.ok_or("not_found: Plugin was removed")?;
                meta.pending_update = None;
                meta.pending_configuration = None;
                meta.updated_at = now();
                Ok((meta, ()))
            })?;
        }
        "stagePrevious" => {
            if params["acknowledgeCurrentData"] != true {
                return Err(
                    "invalid_input: Acknowledge that rollback uses current plugin data".into(),
                );
            }
            let previous = meta
                .previous
                .clone()
                .ok_or("not_found: No previous plugin code")?;
            if field(&params, "expectedDigest", 64)? != previous.package.digest {
                return Err("stale_digest: Previous version changed; review again".into());
            }
            platform.packages.verify(&previous.package.digest)?;
            platform.management.update(&id, |current| {
                let mut meta = current.ok_or("not_found: Plugin was removed")?;
                let mut package = previous.package.clone();
                package.kind = Some("previous".into());
                meta.pending_update = Some(package);
                meta.pending_configuration = Some(previous);
                meta.updated_at = now();
                Ok((meta, ()))
            })?;
        }
        "setEnabled" => {
            validate_recovery(&params)?;
            let enabled = params["enabled"]
                .as_bool()
                .ok_or("invalid_input: enabled must be boolean")?;
            if enabled {
                platform.begin_transition(&id, &params)?;
            } else {
                // Validate even an irrelevant choice before changing enabled intent.
                apply_recovery(
                    &mut platform.service.store.read(&id)?,
                    &params,
                    platform.service.events.latest()?,
                )?;
                platform.service.set_enabled(&id, false)?;
            }
            platform.supervisor.stop(&id).await;
            admin(app, &caller)?;
            if enabled {
                platform.finish_transition(&id, true)?;
                platform.supervisor.resume(&id).await;
                state.wake.notify_one();
            }
        }
        "resume" => {
            validate_recovery(&params)?;
            platform.begin_transition(&id, &params)?;
            platform.supervisor.stop(&id).await;
            admin(app, &caller)?;
            // A resume cannot complete a mismatched package/domain promotion.
            let record = platform.service.store.read(&id)?;
            if record.digest != meta.current.digest {
                return Err(
                    "unavailable: Review the retained pending snapshot to complete promotion"
                        .into(),
                );
            }
            platform.finish_transition(&id, record.enabled)?;
            platform.supervisor.resume(&id).await;
            state.wake.notify_one();
        }
        "updateSettings" => {
            let values: BTreeMap<String, Value> = serde_json::from_value(params["values"].clone())
                .map_err(|_| "invalid_input: Setting values required")?;
            platform.service.update_settings(&id, values)?;
            platform.supervisor.resume(&id).await;
            state.wake.notify_one();
        }
        "remove" => {
            platform.service.set_enabled(&id, false)?;
            platform.supervisor.stop(&id).await;
            admin(app, &caller)?;
            if params
                .get("deleteData")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                platform.management.update(&id, |previous| {
                    let mut meta = previous.ok_or("not_found: Plugin was removed")?;
                    meta.data_owner = Some(uuid::Uuid::new_v4().to_string());
                    meta.previous = None;
                    meta.pending_configuration = None;
                    meta.updated_at = now();
                    Ok((meta, ()))
                })?;
                let directory = platform.service.store.root().join("data").join(&id);
                remove_owned_directory(&directory)?;
                platform.service.store.update(&id, |record| {
                    record.storage.clear();
                    record.settings.clear();
                    record.secrets.clear();
                    record.consented_digest = None;
                    Ok(())
                })?;
            }
            platform.management.update(&id, |previous| {
                let mut meta = previous.ok_or("not_found: Plugin was removed")?;
                meta.removed = true;
                meta.pending_update = None;
                meta.pending_configuration = None;
                if params["deleteData"] == true {
                    meta.previous = None;
                }
                meta.updated_at = now();
                Ok((meta, ()))
            })?;
            changed(app, &id);
            return Ok(Value::Null);
        }
        _ => return Err("invalid_input: Unknown plugin platform method".into()),
    }
    changed(app, &id);
    platform.installation(&id)
}

fn apply_recovery(
    record: &mut InstallationRecord,
    params: &Value,
    latest: i64,
) -> Result<(), String> {
    validate_recovery(params)?;
    if let Some(policy) = params.get("eventBacklog").filter(|v| !v.is_null()) {
        let through = policy["throughSequence"]
            .as_i64()
            .ok_or("invalid_input: Event position required")?;
        if through > latest {
            return Err("invalid_input: Event position is ahead of committed history".into());
        }
        if through < record.event_cursor {
            return Err("stale_backlog: Missed runs changed; review again".into());
        }
        if policy["policy"] == "skip" {
            record.event_cursor = through;
        }
        record.event_backlog_choice_required = false;
        record.delivery_error = None;
    }
    Ok(())
}
fn validate_recovery(params: &Value) -> Result<(), String> {
    if let Some(policy) = params.get("eventBacklog").filter(|v| !v.is_null()) {
        if !policy.as_object().is_some_and(|object| {
            object.len() == 2
                && object.contains_key("policy")
                && object.contains_key("throughSequence")
        }) || !matches!(policy["policy"].as_str(), Some("skip" | "deliver"))
            || policy["throughSequence"].as_i64().is_none_or(|v| v < 0)
        {
            return Err("invalid_input: Missed-event choice must name a reviewed position and deliver or skip".into());
        }
    }
    Ok(())
}
fn remove_owned_directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(path)
            .map_err(|_| "storage_failed: Cannot remove plugin data link".into()),
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path)
            .map_err(|_| "storage_failed: Cannot remove plugin data directory".into()),
        Ok(_) => Err("storage_failed: Plugin data directory is not a directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("storage_failed: Cannot inspect plugin data directory".into()),
    }
}

#[tauri::command]
pub async fn plugins_platform_request<R: Runtime>(
    app: AppHandle<R>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    request(&app, ControlCaller::trusted(), &method, params).await
}

fn execution_disabled<R: Runtime>(app: &AppHandle<R>) -> bool {
    super::disabled_by_env()
        || app
            .try_state::<super::commands::PluginsState>()
            .is_some_and(|state| state.execution_paused())
}

/// Called only after a transcript row committed; no plugin code or journal I/O runs on the
/// provider's event-forwarding path. Recovery also runs once after providers are available.
pub fn committed<R: Runtime>(app: &AppHandle<R>, thread: &str) {
    let Some(state) = app.try_state::<PluginPlatformState>() else {
        return;
    };
    if let Ok(mut threads) = state.committed_threads.lock() {
        if threads.len() < 1024 {
            threads.insert(thread.into());
        } else {
            state
                .recovery_needed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    state.wake.notify_one();
}
pub fn wake<R: Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<PluginPlatformState>() {
        state.wake.notify_one();
    }
}
pub fn start<R: Runtime>(app: AppHandle<R>) {
    use std::sync::atomic::Ordering;
    let Some(state) = app.try_state::<PluginPlatformState>() else {
        return;
    };
    if state.started.swap(true, Ordering::SeqCst) {
        return;
    }
    let notify = state.wake.clone();
    drop(state);
    tauri::async_runtime::spawn(async move {
        let mut startup = true;
        let mut last_full = tokio::time::Instant::now();
        loop {
            let state = app.state::<PluginPlatformState>();
            if state.stopping.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(platform) = state.get(&app) {
                let installed = platform.management.list().into_iter().any(|(_, r)| {
                    r.is_ok_and(|r| {
                        !r.removed
                            && r.current
                                .manifest
                                .requested_capabilities()
                                .iter()
                                .any(|capability| capability == "events.run-finalized")
                    })
                });
                if installed {
                    let threads = state
                        .committed_threads
                        .lock()
                        .map(|mut set| std::mem::take(&mut *set))
                        .unwrap_or_default();
                    let recovered = platform.clone();
                    let recovery_app = app.clone();
                    let full = startup
                        || state.recovery_needed.swap(false, Ordering::SeqCst)
                        || last_full.elapsed() >= std::time::Duration::from_secs(30);
                    if full {
                        last_full = tokio::time::Instant::now();
                    }
                    let recovery = tokio::task::spawn_blocking(move || -> Result<(), String> {
                        if full {
                            recovered.service.events.recover_committed(&recovery_app)?;
                        } else {
                            let db = recovery_app.state::<crate::database::DatabaseStore>();
                            for thread in threads {
                                if let Ok(project) =
                                    super::platform_workflow::thread_project(&recovery_app, &thread)
                                {
                                    recovered
                                        .service
                                        .events
                                        .recover_thread(&db, &project, &thread)?;
                                }
                            }
                        }
                        Ok(())
                    })
                    .await;
                    if let Err(error) = recovery
                        .unwrap_or_else(|_| Err("Plugin event recovery worker failed".into()))
                    {
                        state.recovery_needed.store(true, Ordering::SeqCst);
                        eprintln!("[plugins] {error}");
                    } else {
                        startup = false;
                        if !execution_disabled(&app) {
                            drain_events(&app, &platform).await;
                        }
                    }
                } else if let Ok(mut threads) = state.committed_threads.lock() {
                    threads.clear();
                }
            }
            // Catch late native input provenance and retry transient journal failures even
            // without another provider event. No sleep or scan runs on core persistence.
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(30), notify.notified()).await;
        }
    });
}
async fn drain_events<R: Runtime>(app: &AppHandle<R>, platform: &Arc<PluginPlatform>) {
    use futures_util::StreamExt;
    let installations = platform
        .management
        .list()
        .into_iter()
        .filter_map(|(id, meta)| meta.is_ok_and(|meta| !meta.removed).then_some(id));
    futures_util::stream::iter(installations)
        .for_each_concurrent(4, |id| async move {
            drain_installation(app, platform, id).await;
        })
        .await;
}
async fn drain_installation<R: Runtime>(
    app: &AppHandle<R>,
    platform: &Arc<PluginPlatform>,
    id: String,
) {
    let mut delivered = 0;
    let mut busy_retries = 0;
    while delivered < 16 {
        if execution_disabled(app)
            || app
                .state::<PluginPlatformState>()
                .stopping
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let Ok(record) = platform.service.store.read(&id) else {
            break;
        };
        if !record.enabled
            || record.unavailable_reason.is_some()
            || !record.manifest.compatibility().available
            || record.consented_digest.as_deref() != Some(record.digest.as_str())
            || record.event_backlog_choice_required
            || record.delivery_error.is_some()
            || !record
                .grants
                .capabilities
                .iter()
                .any(|c| c == "events.run-finalized")
        {
            break;
        }
        let mut control = ControlCaller::trusted();
        control.created_by = CreatedBy::System;
        control.principal = format!("plugin-background:{id}");
        let owner = PlatformCaller::new(
            id.clone(),
            record.generation.clone(),
            control.clone(),
            None,
            None,
        );
        let mut event = match platform.service.events.next(&record, &owner) {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(error) => {
                let _ = platform
                    .service
                    .events
                    .pause(&platform.service.store, &id, error);
                break;
            }
        };
        if !event.effects_allowed {
            // A fast provider can commit completion before send_turn persists the
            // native input correlation. Give that committed provenance one more
            // recovery pass before delivering an observation without effect rights.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let recovered = platform.clone();
            let recovery_app = app.clone();
            let thread = event.thread_id.clone();
            let project = event.project_id.clone();
            let recovery = tokio::task::spawn_blocking(move || {
                recovered.service.events.recover_thread(
                    &recovery_app.state::<crate::database::DatabaseStore>(),
                    &project,
                    &thread,
                )
            })
            .await;
            if !recovery.is_ok_and(|result| result.is_ok()) {
                break;
            }
            let Ok(current) = platform.service.store.read(&id) else {
                break;
            };
            if current.generation != record.generation {
                break;
            }
            event = match platform.service.events.next(&current, &owner) {
                Ok(Some(event)) => event,
                _ => break,
            };
        }
        let input = json!({"installationId":id,"generation":record.generation,"handler":"event:run.finalized",
                "operationId":format!("event-{}",super::platform_store::digest(&json!(event.delivery_id))),
                "input":event,"context":{"threadId":event.thread_id,"projectId":event.project_id}});
        match platform
            .invoke(app, control, &input, Some(event.delivery_id.clone()))
            .await
        {
            Ok(_) => {
                busy_retries = 0;
                let delivery_owner = owner.for_delivery(event.delivery_id.clone());
                if let Err(error) = platform
                    .service
                    .dispatch(
                        app,
                        &delivery_owner,
                        "events.ack",
                        json!({"deliveryId":event.delivery_id}),
                    )
                    .await
                {
                    let _ = platform
                        .service
                        .events
                        .pause(&platform.service.store, &id, error);
                    changed(app, &id);
                    break;
                }
                delivered += 1;
                if delivered == 16 {
                    app.state::<PluginPlatformState>().wake.notify_one();
                }
            }
            // Only this supervisor admission error proves the handler never ran.
            // Handler/host failures use operation_failed even if their text says busy.
            Err(error) if error == "busy: Plugin invocation queue is full" => {
                if busy_retries == 5 {
                    // The periodic sweep will retry the unchanged durable cursor.
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50 << busy_retries)).await;
                busy_retries += 1;
            }
            Err(error) => {
                let _ = platform
                    .service
                    .events
                    .pause(&platform.service.store, &id, error);
                changed(app, &id);
                break;
            }
        }
    }
}

pub async fn agent_request<R: Runtime>(
    app: &AppHandle<R>,
    mut caller: ControlCaller,
    name: &str,
    mut input: Value,
) -> Result<Value, String> {
    if name == "plugin_list" {
        if !input.as_object().is_some_and(|v| v.is_empty()) {
            return Err("invalid_input: plugin_list accepts an empty object".into());
        }
        let catalogue = request(app, caller, "catalogue", json!({})).await?;
        let plugins=catalogue["installations"].as_array().into_iter().flatten().filter(|installation|{
            installation["enabled"]==true&&installation["status"]=="idle"&&catalogue["executionDisabled"]!=true
        }).filter_map(|installation|{
            let actions:Vec<_>=installation["manifest"]["actions"].as_array()?.iter().filter(|a|a["agentCallable"]==true)
                .map(|a|json!({"id":a["id"],"title":a["title"],"description":a["description"]})).collect();
            (!actions.is_empty()).then(||json!({"installationId":installation["installationId"],"generation":installation["generation"],
                "name":installation["manifest"]["name"],"version":installation["manifest"]["version"],"actions":actions}))
        }).collect::<Vec<_>>();
        return Ok(json!({"plugins":plugins}));
    }
    if name != "plugin_invoke" {
        return Err("invalid_input: Unknown plugin agent tool".into());
    }
    // Every invocation through an agent tool observes agentCallable, including
    // outside MCP clients whose generic control credential represents a user.
    caller.created_by = CreatedBy::Agent;
    let action = identifier(&input, "actionId")?.to_string();
    let object = input
        .as_object_mut()
        .ok_or("invalid_input: Object required")?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "installationId" | "generation" | "actionId" | "operationId" | "input" | "context"
        )
    }) {
        return Err("invalid_input: Unknown plugin invocation fields".into());
    }
    object.remove("actionId");
    object.insert("handler".into(), json!(format!("action:{action}")));
    if let Some(thread) = caller.thread_id() {
        let context = object.entry("context").or_insert_with(|| json!({}));
        let context = context
            .as_object_mut()
            .ok_or("invalid_input: Context must be an object")?;
        context.entry("threadId").or_insert_with(|| json!(thread));
    }
    request(app, caller, "invoke", input).await
}

pub fn mcp_result(mut value: Value) -> Value {
    let images = value
        .as_object_mut()
        .and_then(|o| o.remove("images"))
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let mut content = vec![json!({"type":"text","text":value.to_string()})];
    for image in images {
        content.push(json!({"type":"image","mimeType":image["mimeType"],"data":image["data"]}));
    }
    json!({"isError":false,"content":content,"structuredContent":value})
}

pub struct PluginToolsHost<R: Runtime>(pub AppHandle<R>);
impl<R: Runtime> crate::mcp::registry::HostTools for PluginToolsHost<R> {
    fn tools_for(
        &self,
        _caller: &crate::mcp::registry::HostCaller,
    ) -> Vec<crate::mcp::runtime::McpTool> {
        crate::agent_control::catalog::plugin_tools()
            .into_iter()
            .map(|tool| crate::mcp::runtime::McpTool {
                name: tool.name.into(),
                prefixed_name: crate::mcp::runtime::prefix_tool_name("codemux", tool.name),
                description: Some(tool.description.into()),
                input_schema: tool.input_schema,
                server_id: "codemux".into(),
            })
            .collect()
    }
    fn call(
        &self,
        _caller: &crate::mcp::registry::HostCaller,
        _name: &str,
        _args: &Value,
    ) -> Option<Value> {
        None
    }
    fn call_async<'a>(
        &'a self,
        caller: &'a crate::mcp::registry::HostCaller,
        name: &'a str,
        args: &'a Value,
        _call_id: Option<&'a str>,
        _run_id: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Value>> + Send + 'a>> {
        Box::pin(async move {
            let raw = name.strip_prefix("mcp__codemux__")?;
            if !matches!(raw, "plugin_list" | "plugin_invoke") {
                return None;
            }
            let Some(control) = caller.control.clone() else {
                return Some(
                    json!({"isError":true,"content":[{"type":"text","text":"A host-verified plugin caller is required"}]}),
                );
            };
            Some(
                match agent_request(&self.0, control, raw, args.clone()).await {
                    Ok(value) => mcp_result(value),
                    Err(error) => json!({"isError":true,"content":[{"type":"text","text":error}]}),
                },
            )
        })
    }
}

#[cfg(test)]
#[path = "platform_transport_tests.rs"]
pub(crate) mod tests;
