//! Trusted desktop management surface. None of these commands is callable remotely.
use crate::addons::{
    self,
    manager::{Installation, Manager, Status, UiEvent},
    ErrorCode, ProtocolError, Result,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::{ipc::Channel, Manager as _, Runtime, State};
use tokio_util::sync::CancellationToken;
#[derive(Default)]
pub struct AddonState {
    manager: Mutex<Option<Result<Arc<Manager>>>>,
    subscription: Mutex<Option<CancellationToken>>,
    reviews: Arc<addons::lifecycle::Reviews>,
    development: addons::development::Development,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    paused: bool,
    installed: Vec<Installation>,
    error: Option<String>,
    warnings: Vec<String>,
    developer_mode: bool,
    development_package: Option<String>,
}
impl AddonState {
    pub fn get<R: Runtime>(&self, app: &tauri::AppHandle<R>) -> Result<Arc<Manager>> {
        crate::ensure_gui_mode(app).map_err(|_| {
            ProtocolError::new(
                ErrorCode::RemoteUnsupported,
                "Add-ons are available only in the desktop app",
            )
        })?;
        let mut manager = self.manager.lock().unwrap();
        if manager.is_none() {
            let result = (|| {
                let root = dirs::data_dir()
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::StorageUnavailable,
                            "App data directory is unavailable",
                        )
                    })?
                    .join(crate::APP_DIR_NAME)
                    .join("addons-v1");
                let filename = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
                    "codemux-addon-host-linux-x64"
                } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
                    "codemux-addon-host-windows-x64.exe"
                } else {
                    return Err(ProtocolError::new(
                        ErrorCode::IncompatibleApi,
                        "Add-ons are not supported on this platform",
                    ));
                };
                let host = app
                    .path()
                    .resource_dir()
                    .map_err(|_| {
                        ProtocolError::new(
                            ErrorCode::PluginStopped,
                            "App resources are unavailable",
                        )
                    })?
                    .join("binaries")
                    .join(filename);
                #[cfg(debug_assertions)]
                let host = if host.is_file() {
                    host
                } else {
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("binaries")
                        .join(filename)
                };
                Manager::open(root, host)
            })();
            *manager = Some(result);
        }
        manager.as_ref().unwrap().clone()
    }
    pub fn existing(&self) -> Option<Arc<Manager>> {
        self.manager
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .cloned()
    }
}
#[tauri::command]
pub fn addon_inventory<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
) -> Inventory {
    match state.get(&app) {
        Ok(manager) => match manager.list() {
            Ok(installed) => Inventory {
                developer_mode: state.development.enabled(),
                development_package: state.development.package(),
                paused: manager.paused(),
                installed,
                error: None,
                warnings: manager
                    .cleanup_warnings()
                    .unwrap_or_else(|error| vec![error.message]),
            },
            Err(error) => Inventory {
                developer_mode: false,
                development_package: None,
                paused: true,
                installed: vec![],
                error: Some(error.message),
                warnings: vec![],
            },
        },
        Err(error) => Inventory {
            developer_mode: false,
            development_package: None,
            paused: true,
            installed: vec![],
            error: Some(error.message),
            warnings: vec![],
        },
    }
}
#[tauri::command]
pub async fn addon_subscribe<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    channel: Channel<UiEvent>,
) -> Result<()> {
    let manager = state.get(&app)?;
    let cancel = CancellationToken::new();
    if let Some(old) = state.subscription.lock().unwrap().replace(cancel.clone()) {
        old.cancel();
    }
    manager.change_workspace(None).await;
    let mut events = manager.events.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {_ = cancel.cancelled()=>break,event=events.recv()=>{match event{Ok(event)=>if channel.send(event).is_err(){manager.change_workspace(None).await;break},Err(_)=>{manager.pause_all().await;break}}}}
        }
    });
    Ok(())
}
#[tauri::command]
pub async fn addon_pause_all<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
) -> Result<()> {
    state.development.stop(None);
    state.get(&app)?.pause_all().await;
    Ok(())
}
#[tauri::command]
pub async fn addon_disable<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
) -> Result<()> {
    let manager = state.get(&app)?;
    state.development.stop(Some(&id));
    let operation = manager.operation(&id).await;
    let _lock = operation.lock().await;
    let mut installation = manager.installation(&id)?;
    installation.desired_enabled = false;
    installation.status = Status::InstalledDisabled;
    manager.save(&installation)?;
    manager.stop(&id, None).await;
    Ok(())
}
#[tauri::command]
pub async fn addon_context_changed<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    workspaces: State<'_, crate::state::AppStateStore>,
    workspace_id: Option<String>,
) -> Result<()> {
    let workspace = workspace_id
        .as_deref()
        .map(|id| addons::workspace::current(&workspaces, id))
        .transpose();
    let manager = state.get(&app)?;
    // Invalidate even when the new target is remote or unavailable.
    manager
        .change_workspace(workspace.clone().ok().flatten())
        .await;
    workspace.map(|_| ())
}
#[tauri::command]
pub async fn addon_composer_register<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    workspaces: State<'_, crate::state::AppStateStore>,
    flags: State<'_, crate::observability::ObservabilityStore>,
    composer_id: String,
    workspace_id: String,
) -> Result<()> {
    if !flags.agent_chat_enabled() {
        return Err(ProtocolError::new(
            ErrorCode::NoComposer,
            "Chat GUI is disabled",
        ));
    }
    let workspace = addons::workspace::current(&workspaces, &workspace_id)?;
    state
        .get(&app)?
        .contexts
        .lock()
        .await
        .register_composer(composer_id, workspace.id)
}
#[tauri::command]
pub async fn addon_composer_closed<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    composer_id: String,
) -> Result<()> {
    state
        .get(&app)?
        .contexts
        .lock()
        .await
        .revoke_composer(&composer_id);
    Ok(())
}
#[tauri::command]
pub async fn addon_execute<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    workspaces: State<'_, crate::state::AppStateStore>,
    id: String,
    command: String,
    kind: String,
    workspace_id: Option<String>,
    composer_id: Option<String>,
) -> Result<()> {
    let manager = state.get(&app)?;
    let workspace = workspace_id
        .as_deref()
        .map(|id| addons::workspace::current(&workspaces, id))
        .transpose()?;
    let running = manager.ensure_active(&id).await?;
    let context = manager
        .context_handle(&running, workspace, composer_id)
        .await?;
    manager.execute(&running, &command, &kind, &context).await
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mounted {
    pub view_id: String,
    pub generation: String,
}
#[tauri::command]
pub async fn addon_mount<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    workspaces: State<'_, crate::state::AppStateStore>,
    id: String,
    view: String,
    kind: String,
    workspace_id: String,
    composer_id: Option<String>,
) -> Result<Mounted> {
    let manager = state.get(&app)?;
    let workspace = addons::workspace::current(&workspaces, &workspace_id)?;
    let running = manager.ensure_active(&id).await?;
    let context = manager
        .context_handle(&running, Some(workspace), composer_id)
        .await?;
    let view_id = manager.mount(&running, &view, &kind, &context).await?;
    Ok(Mounted {
        view_id,
        generation: running.generation().into(),
    })
}
#[tauri::command]
pub async fn addon_unmount<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
    generation: String,
    view_id: String,
) -> Result<()> {
    let manager = state.get(&app)?;
    let running = manager.running(&id, &generation).await?;
    manager.unmount(&running, &view_id).await
}
#[tauri::command]
pub async fn addon_ui_link<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
    generation: String,
    view_id: String,
    node_id: String,
    url: String,
) -> Result<()> {
    let manager = state.get(&app)?;
    let running = manager.running(&id, &generation).await?;
    manager.ui_link(&running, &view_id, &node_id, &url).await
}
#[tauri::command]
pub async fn addon_ui_event<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
    generation: String,
    view_id: String,
    node_id: String,
    event: String,
    callback_id: String,
    value: Value,
) -> Result<()> {
    let manager = state.get(&app)?;
    let running = manager.running(&id, &generation).await?;
    manager
        .ui_event(&running, &view_id, &node_id, &event, &callback_id, value)
        .await
}
#[tauri::command]
pub async fn addon_ui_ack<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
    generation: String,
    view_id: String,
    revision: u64,
) -> Result<()> {
    let manager = state.get(&app)?;
    let running = manager.running(&id, &generation).await?;
    manager.acknowledge(&running, &view_id, revision).await
}
#[tauri::command]
pub async fn addon_effect_claim<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    request_id: String,
    generation: String,
) -> Result<()> {
    state
        .get(&app)?
        .claim_effect(&request_id, &generation)
        .await
}
#[tauri::command]
pub async fn addon_effect_result<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    request_id: String,
    generation: String,
    value: Value,
    error: Option<ProtocolError>,
) -> Result<()> {
    let manager = state.get(&app)?;
    manager
        .effect_result(&request_id, &generation, error.map_or(Ok(value), Err))
        .await
}
#[tauri::command]
pub async fn addon_settings_set<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
    settings: Value,
) -> Result<()> {
    state.get(&app)?.set_settings(&id, settings).await
}
#[tauri::command]
pub async fn addon_credential_set<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
    credential_id: String,
    value: String,
    session_only: bool,
) -> Result<()> {
    let manager = state.get(&app)?;
    let operation = manager.operation(&id).await;
    let _lock = operation.lock().await;
    let installation = manager.installation(&id)?;
    if !installation
        .manifest
        .credentials
        .iter()
        .any(|c| c.id == credential_id)
    {
        return Err(ProtocolError::new(
            ErrorCode::PermissionDenied,
            "Credential was not declared",
        ));
    }
    let origin = &installation
        .manifest
        .credentials
        .iter()
        .find(|c| c.id == credential_id)
        .unwrap()
        .origin;
    let credential_id = super::super::addons::credentials::Credentials::key(&credential_id, origin);
    // Persist the host-owned index before the non-cancellable OS write. If
    // this IPC task is dropped, uninstall/restart can still find the credential.
    if !session_only {
        manager.record_credential(&installation.installation_id, &credential_id)?;
    }
    manager
        .credentials
        .set(
            &installation.installation_id,
            &credential_id,
            value,
            session_only,
        )
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn addon_import_review<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    path: String,
) -> Result<addons::lifecycle::Review> {
    let manager = state.get(&app)?;
    let reviews = state.reviews.clone();
    tokio::task::spawn_blocking(move || {
        reviews.prepare_local(&manager, std::path::Path::new(&path))
    })
    .await
    .map_err(|_| ProtocolError::invalid("Package validation failed"))?
}
#[tauri::command]
pub async fn addon_accept_review<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    token: String,
    enable: bool,
    replace_source: bool,
    restore_data: Option<bool>,
) -> Result<Installation> {
    let manager = state.get(&app)?;
    let path = state.reviews.development_path(&token);
    if path.is_some() && !state.development.enabled() {
        return Err(ProtocolError::invalid("Developer mode is off"));
    }
    let installation = state
        .reviews
        .accept_with_data(
            &manager,
            &token,
            enable,
            replace_source,
            restore_data.unwrap_or(false),
        )
        .await?;
    if let Some(path) = path {
        if enable {
            state.development.watch(
                manager,
                state.reviews.clone(),
                path,
                installation.manifest.id.clone(),
                installation.source.clone(),
            )?;
        }
    }
    Ok(installation)
}
#[tauri::command]
pub fn addon_cancel_review(state: State<'_, AddonState>, token: String) {
    state.reviews.cancel(&token)
}
#[tauri::command]
pub async fn addon_remove<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
    keep_data: bool,
) -> Result<Vec<String>> {
    state.development.stop(Some(&id));
    state.get(&app)?.remove(&id, keep_data).await
}
#[tauri::command]
pub fn addon_developer_mode<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    enabled: bool,
) -> Result<()> {
    state.get(&app)?;
    state.development.set_enabled(enabled);
    Ok(())
}
#[tauri::command]
pub async fn addon_development_review<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    path: String,
) -> Result<addons::lifecycle::Review> {
    let manager = state.get(&app)?;
    if !state.development.enabled() {
        return Err(ProtocolError::invalid(
            "Enable Developer mode before selecting a package",
        ));
    }
    state
        .reviews
        .prepare_development(&manager, std::path::Path::new(&path))
}
#[tauri::command]
pub fn addon_development_reload<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
) -> Result<()> {
    state.get(&app)?;
    state.development.reload()
}
#[tauri::command]
pub async fn addon_retry_cleanup<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
) -> Result<Vec<String>> {
    state.get(&app)?.retry_cleanup().await
}
#[tauri::command]
pub async fn addon_rollback<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
) -> Result<()> {
    state.get(&app)?.rollback(&id).await
}
#[tauri::command]
pub async fn addon_enable<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
) -> Result<()> {
    let manager = state.get(&app)?;
    let operation = manager.operation(&id).await;
    let _lock = operation.lock().await;
    let mut installation = manager.installation(&id)?;
    if matches!(
        installation.status,
        Status::BlockedDisabled | Status::IncompatibleDisabled
    ) {
        return Err(ProtocolError::new(
            ErrorCode::PermissionDenied,
            "This release cannot be enabled",
        ));
    }
    installation
        .grant
        .as_ref()
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::PermissionDenied, "Permission review required")
        })?
        .check(
            &installation.installation_id,
            &installation.manifest,
            &installation.source,
            &installation.digest,
        )?;
    installation.desired_enabled = true;
    installation.status = Status::EnabledIdle;
    installation.failure = None;
    manager.save(&installation)
}
#[tauri::command]
pub fn addon_resume<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
) -> Result<()> {
    state.get(&app)?.resume()
}
#[tauri::command]
pub async fn addon_catalog<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    refresh: bool,
) -> Result<addons::catalog::Browse> {
    state.get(&app)?.browse(refresh).await
}
#[tauri::command]
pub async fn addon_catalog_review<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    target: String,
) -> Result<addons::lifecycle::Review> {
    state
        .reviews
        .prepare_catalog(&state.get(&app)?, &target)
        .await
}
#[tauri::command]
pub fn addon_settings_get<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
) -> Result<Value> {
    let manager = state.get(&app)?;
    manager.settings(&manager.installation(&id)?)
}
/// Bounded, sanitized log activity for the Settings detail view.
#[tauri::command]
pub fn addon_diagnostics<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
) -> Result<addons::manager::Diagnostics> {
    state.get(&app)?.diagnostics(&id)
}
