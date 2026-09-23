//! Trusted desktop management surface. None of these commands is callable remotely.
use crate::addons::{
    self,
    manager::{Installation, Manager, Status, UiEvent},
    ErrorCode, ProtocolError, Result,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{ipc::Channel, Manager as _, Runtime, State};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
/// A failed open. `root` is set when the add-on registry itself could not be
/// opened, which the reset action can recover without a restart.
struct Failure {
    error: ProtocolError,
    root: Option<PathBuf>,
    cause: String,
}
#[derive(Default)]
pub struct AddonState {
    manager: Mutex<Option<std::result::Result<Arc<Manager>, Failure>>>,
    subscription: Mutex<Option<CancellationToken>>,
    reviews: Arc<addons::lifecycle::Reviews>,
    development: addons::development::Development,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    paused: bool,
    installed: Vec<InventoryItem>,
    error: Option<String>,
    warnings: Vec<String>,
    developer_mode: bool,
    development_package: Option<String>,
    /// Plugin ID -> declared credential ID -> state. Never contains a secret.
    credential_states: HashMap<String, BTreeMap<String, addons::credentials::CredentialState>>,
    registry_error: Option<RegistryError>,
    interrupted_activations: Vec<String>,
}
/// An installation plus values derived from the running app and the cached
/// catalog. Nothing here is downloaded or activated to build it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItem {
    #[serde(flatten)]
    installation: Installation,
    update_available: Option<String>,
    catalog: Option<addons::catalog::Listing>,
    compatibility: addons::manager::Compatibility,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryError {
    path: String,
    cause: String,
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
                let fail = |error: ProtocolError| Failure {
                    cause: error.message.clone(),
                    error,
                    root: None,
                };
                let root = dirs::data_dir()
                    .ok_or_else(|| {
                        fail(ProtocolError::new(
                            ErrorCode::StorageUnavailable,
                            "App data directory is unavailable",
                        ))
                    })?
                    .join(crate::APP_DIR_NAME)
                    .join("addons-v1");
                let filename = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
                    "codemux-addon-host-linux-x64"
                } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
                    "codemux-addon-host-windows-x64.exe"
                } else {
                    return Err(fail(ProtocolError::new(
                        ErrorCode::IncompatibleApi,
                        "Add-ons are not supported on this platform",
                    )));
                };
                let host = app
                    .path()
                    .resource_dir()
                    .map_err(|_| {
                        fail(ProtocolError::new(
                            ErrorCode::PluginStopped,
                            "App resources are unavailable",
                        ))
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
                Manager::open(root.clone(), host).map_err(|cause| Failure {
                    error: ProtocolError::new(
                        ErrorCode::StorageUnavailable,
                        format!(
                            "The add-on registry at {} could not be opened: {}. Reset it to start with no add-ons; the current files are kept as a backup.",
                            root.display(),
                            cause.message.trim_end_matches('.')
                        ),
                    ),
                    root: Some(root),
                    cause: cause.message,
                })
            })();
            if let Ok(opened) = &result {
                opened.start_catalog_recheck();
            }
            *manager = Some(result);
        }
        match manager.as_ref().unwrap() {
            Ok(opened) => Ok(opened.clone()),
            Err(failure) => Err(failure.error.clone()),
        }
    }
    /// Forget a failed open so an explicit user action retries it.
    fn retry(&self) {
        let mut manager = self.manager.lock().unwrap();
        if matches!(manager.as_ref(), Some(Err(_))) {
            *manager = None;
        }
    }
    fn registry_error(&self) -> Option<RegistryError> {
        match self.manager.lock().unwrap().as_ref() {
            Some(Err(Failure {
                root: Some(root),
                cause,
                ..
            })) => Some(RegistryError {
                path: root.display().to_string(),
                cause: cause.clone(),
            }),
            _ => None,
        }
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
    let unavailable = |error: ProtocolError, registry_error| Inventory {
        developer_mode: false,
        development_package: None,
        paused: true,
        installed: vec![],
        error: Some(error.message),
        warnings: vec![],
        registry_error,
        interrupted_activations: vec![],
        credential_states: HashMap::new(),
    };
    match state.get(&app) {
        Ok(manager) => match manager.list() {
            Ok(installed) => {
                let mut warnings = manager
                    .cleanup_warnings()
                    .unwrap_or_else(|error| vec![error.message]);
                let mut credential_states = HashMap::new();
                for installation in &installed {
                    match manager.credential_states(installation) {
                        Ok(states) => {
                            credential_states.insert(installation.manifest.id.clone(), states);
                        }
                        Err(error) => warnings.push(error.message),
                    }
                }
                warnings.dedup();
                Inventory {
                    developer_mode: state.development.enabled(),
                    development_package: state.development.package(),
                    paused: manager.paused(),
                    installed: installed
                        .into_iter()
                        .map(|installation| {
                            let (catalog, update_available) =
                                manager.catalog_status(&installation);
                            InventoryItem {
                                compatibility: installation.compatibility(),
                                installation,
                                update_available,
                                catalog,
                            }
                        })
                        .collect(),
                    error: None,
                    warnings,
                    registry_error: None,
                    interrupted_activations: manager.interrupted_activations(),
                    credential_states,
                }
            }
            Err(error) => unavailable(error, None),
        },
        Err(error) => unavailable(error, state.registry_error()),
    }
}
/// Move an unreadable add-on registry aside and open a fresh one without a
/// restart. Returns the backup folder, which keeps every previous file.
#[tauri::command]
pub fn addon_registry_reset<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
) -> Result<String> {
    crate::ensure_gui_mode(&app).map_err(|_| {
        ProtocolError::new(
            ErrorCode::RemoteUnsupported,
            "Add-ons are available only in the desktop app",
        )
    })?;
    let backup = {
        let mut manager = state.manager.lock().unwrap();
        let Some(Err(Failure {
            root: Some(root), ..
        })) = manager.as_ref()
        else {
            return Err(ProtocolError::invalid(
                "The add-on registry has not failed to open; nothing was reset",
            ));
        };
        let backup = Manager::reset_registry(root)?;
        *manager = None;
        backup
    };
    // The move already happened; never lose where the previous files went.
    state.get(&app).map_err(|error| {
        ProtocolError::new(
            error.data.code,
            format!(
                "{} The previous add-on files were moved to {}.",
                error.message,
                backup.display()
            ),
        )
    })?;
    Ok(backup.display().to_string())
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
    let events = manager.events.subscribe();
    tauri::async_runtime::spawn(forward(manager, events, cancel, move |event| {
        channel.send(event).is_ok()
    }));
    Ok(())
}
/// Forward add-on events to one webview. A slow receiver is a transport
/// condition, not a user pause: missed events are replaced with current state.
async fn forward(
    manager: Arc<Manager>,
    mut events: broadcast::Receiver<UiEvent>,
    cancel: CancellationToken,
    send: impl Fn(UiEvent) -> bool,
) {
    loop {
        let delivered = tokio::select! {
            _ = cancel.cancelled() => break,
            event = events.recv() => match event {
                Ok(event) => send(event),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Subscribe first, so everything after the snapshot follows it.
                    events = manager.events.subscribe();
                    manager.resync_events().await.into_iter().all(&send)
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        };
        if !delivered {
            manager.change_workspace(None).await;
            break;
        }
    }
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
    manager
        .save_credential(&installation, &credential_id, value, session_only)
        .await
}
/// Removes a saved credential; requests to its origin become unauthenticated.
/// Returns cleanup warnings, like removal, when the OS store is locked.
#[tauri::command]
pub async fn addon_credential_clear<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
    credential_id: String,
) -> Result<Vec<String>> {
    let manager = state.get(&app)?;
    let operation = manager.operation(&id).await;
    let _lock = operation.lock().await;
    let installation = manager.installation(&id)?;
    manager.clear_credential(&installation, &credential_id).await
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
        Status::BlockedDisabled | Status::IncompatibleDisabled | Status::Removing
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
    // Resuming also retries an add-on registry that failed to open.
    state.retry();
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
pub async fn addon_check_update<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AddonState>,
    id: String,
) -> Result<addons::catalog::UpdateCheck> {
    state.reviews.check_update(&state.get(&app)?, &id).await
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
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn a_lagging_subscriber_resynchronizes_without_pausing_add_ons() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), "unused".into()).unwrap();
        let events = manager.events.subscribe();
        // Overflow the bounded broadcast buffer before the forwarder reads.
        for _ in 0..40 {
            assert!(manager.events.send(UiEvent::Inventory).is_ok());
        }
        let received = Arc::new(Mutex::new(0usize));
        let counter = received.clone();
        let cancel = CancellationToken::new();
        let forwarder = tokio::spawn(forward(
            manager.clone(),
            events,
            cancel.clone(),
            move |_| {
                *counter.lock().unwrap() += 1;
                true
            },
        ));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while *received.lock().unwrap() == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("resynchronized state is delivered");
        // Forwarding continues after the lag.
        let before = *received.lock().unwrap();
        assert!(manager.events.send(UiEvent::Inventory).is_ok());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while *received.lock().unwrap() == before {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("later events still arrive");
        cancel.cancel();
        forwarder.await.unwrap();
        assert!(!manager.paused());
        drop(manager);
        assert!(!Manager::open(root.path().into(), "unused".into())
            .unwrap()
            .paused());
    }
}
