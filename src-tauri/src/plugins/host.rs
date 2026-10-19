//! Desktop plugin resources. Tokens identify one loaded owner; paths always
//! refer to this machine, independently of the selected workspace.
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};
use tauri::{AppHandle, Runtime, State};
use tokio::sync::{watch, Notify};

#[path = "host_data.rs"]
mod data;
#[path = "host_process.rs"]
pub mod process;

#[derive(Default)]
pub struct PluginHostState {
    epoch: AtomicU64,
    owners: Mutex<HashMap<String, Arc<HostOwner>>>,
    retired: Mutex<Vec<Arc<HostOwner>>>,
}
struct HostOwner {
    plugin: String,
    epoch: u64,
    cancel: watch::Sender<bool>,
    calls: AtomicUsize,
    changed: Notify,
}
impl HostOwner {
    async fn wait_idle(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.calls.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}
pub(super) struct RunPermit(Arc<HostOwner>);
impl RunPermit {
    pub fn cancellation(&self) -> watch::Receiver<bool> {
        self.0.cancel.subscribe()
    }
}
impl Drop for RunPermit {
    fn drop(&mut self) {
        self.0.calls.fetch_sub(1, Ordering::AcqRel);
        self.0.changed.notify_waiters();
    }
}
impl PluginHostState {
    fn acquire(&self, page_epoch: u64, plugin: &str, token: &str) -> Result<(), String> {
        if !super::valid_id(plugin) || token.is_empty() || token.len() > 128 {
            return Err("Invalid plugin owner".into());
        }
        let mut owners = self.owners.lock().unwrap();
        if page_epoch != self.epoch.load(Ordering::Acquire) {
            return Err("Plugin page is no longer active".into());
        }
        if let Some(owner) = owners.get(token) {
            return if owner.plugin == plugin {
                Ok(())
            } else {
                Err("Owner token belongs to another plugin".into())
            };
        }
        if owners.len() >= 1024 {
            return Err("Too many plugin owners".into());
        }
        owners.insert(
            token.into(),
            Arc::new(HostOwner {
                plugin: plugin.into(),
                epoch: self.epoch.load(Ordering::Acquire),
                cancel: watch::channel(false).0,
                calls: AtomicUsize::new(0),
                changed: Notify::new(),
            }),
        );
        self.retired
            .lock()
            .unwrap()
            .retain(|owner| owner.calls.load(Ordering::Acquire) != 0);
        Ok(())
    }
    fn begin(&self, plugin: &str, token: &str) -> Result<RunPermit, String> {
        let owners = self.owners.lock().unwrap();
        let owner = owners
            .get(token)
            .filter(|owner| {
                owner.plugin == plugin
                    && owner.epoch == self.epoch.load(Ordering::Acquire)
                    && !*owner.cancel.borrow()
            })
            .ok_or("Plugin host owner is no longer active")?;
        if owner.calls.load(Ordering::Acquire) >= 32 {
            return Err("Too many concurrent plugin calls".into());
        }
        owner.calls.fetch_add(1, Ordering::AcqRel);
        Ok(RunPermit(owner.clone()))
    }
    async fn release(&self, plugin: &str, token: &str) {
        let owner = {
            let mut owners = self.owners.lock().unwrap();
            if owners
                .get(token)
                .is_some_and(|owner| owner.plugin == plugin)
            {
                let owner = owners.remove(token).unwrap();
                owner.cancel.send_replace(true);
                self.retired.lock().unwrap().push(owner.clone());
                Some(owner)
            } else {
                None
            }
        };
        if let Some(owner) = owner {
            owner.wait_idle().await;
            self.retired
                .lock()
                .unwrap()
                .retain(|owner| owner.calls.load(Ordering::Acquire) != 0);
        }
    }
    pub fn invalidate_page(&self, epoch: u64) {
        let mut owners = self.owners.lock().unwrap();
        self.epoch.store(epoch, Ordering::Release);
        let mut retired = self.retired.lock().unwrap();
        for (_, owner) in owners.drain() {
            owner.cancel.send_replace(true);
            retired.push(owner);
        }
        retired.retain(|owner| owner.calls.load(Ordering::Acquire) != 0);
    }
    pub async fn shutdown(&self) {
        self.invalidate_page(u64::MAX);
        let owners = self.retired.lock().unwrap().clone();
        // A blocking filesystem operation on a dead mount cannot be cancelled.
        // Cancel children immediately, but bound the overall app-exit wait.
        if tokio::time::timeout(std::time::Duration::from_secs(3), async {
            for owner in owners {
                owner.wait_idle().await;
            }
        })
        .await
        .is_err()
        {
            log::warn!("Plugin host shutdown timed out waiting for blocking calls");
        }
    }
}

fn workspace_context(state: &crate::state::AppStateStore, id: Option<&str>) -> Result<(), String> {
    let Some(id) = id else {
        return Ok(());
    };
    state.with_canonical_snapshot(|snapshot| {
        let workspace = snapshot.workspaces.iter().find(|w| w.workspace_id.0 == id)
            .ok_or("Workspace is not open")?;
        if workspace.host_id.is_some() || workspace.remote_cwd.is_some() {
            Err("Plugin host process and filesystem APIs operate locally; remote workspaces are unsupported".into())
        } else { Ok(()) }
    })
}

#[tauri::command]
pub fn plugins_host_acquire<R: Runtime>(
    app: AppHandle<R>,
    host: State<'_, PluginHostState>,
    plugin_id: String,
    token: String,
    page_epoch: u64,
) -> Result<(), String> {
    super::commands::gui(&app)?;
    host.acquire(page_epoch, &plugin_id, &token)
}
#[tauri::command]
pub async fn plugins_host_release<R: Runtime>(
    app: AppHandle<R>,
    host: State<'_, PluginHostState>,
    plugin_id: String,
    token: String,
) -> Result<(), String> {
    super::commands::gui(&app)?;
    host.release(&plugin_id, &token).await;
    Ok(())
}
#[tauri::command]
pub async fn plugins_process_run<R: Runtime>(
    app: AppHandle<R>,
    host: State<'_, PluginHostState>,
    state: State<'_, crate::state::AppStateStore>,
    plugin_id: String,
    token: String,
    input: process::RunInput,
) -> Result<process::RunResult, String> {
    super::commands::gui(&app)?;
    workspace_context(&state, input.workspace_id.as_deref())?;
    let permit = host.begin(&plugin_id, &token)?;
    // The supervisor outlives cancellation of the renderer's IPC future.
    tokio::spawn(process::run(input, permit))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn plugins_process_which<R: Runtime>(
    app: AppHandle<R>,
    host: State<'_, PluginHostState>,
    plugin_id: String,
    token: String,
    command: String,
) -> Result<Option<String>, String> {
    super::commands::gui(&app)?;
    let _permit = host.begin(&plugin_id, &token)?;
    tokio::task::spawn_blocking(move || process::which(&command))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn plugins_fs<R: Runtime>(
    app: AppHandle<R>,
    host: State<'_, PluginHostState>,
    state: State<'_, crate::state::AppStateStore>,
    plugin_id: String,
    token: String,
    workspace_id: Option<String>,
    operation: data::FsOperation,
) -> Result<serde_json::Value, String> {
    super::commands::gui(&app)?;
    workspace_context(&state, workspace_id.as_deref())?;
    let permit = host.begin(&plugin_id, &token)?;
    tokio::task::spawn_blocking(move || {
        let cancellation = permit.cancellation();
        let _permit = permit;
        if *cancellation.borrow() {
            return Err("Plugin has been disposed".into());
        }
        data::execute(&plugin_id, operation)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn plugin_host_owner_cancels_and_waits_and_rejects_stale_calls() {
        let host = Arc::new(PluginHostState::default());
        host.acquire(0, "demo", "one").unwrap();
        let permit = host.begin("demo", "one").unwrap();
        let mut cancelled = permit.cancellation();
        assert!(host.begin("other", "one").is_err());
        let copy = host.clone();
        let release = tokio::spawn(async move { copy.release("demo", "one").await });
        cancelled.changed().await.unwrap();
        assert!(*cancelled.borrow());
        assert!(!release.is_finished());
        drop(permit);
        release.await.unwrap();
        assert!(host.begin("demo", "one").is_err());
        host.acquire(0, "demo", "two").unwrap();
        host.release("demo", "one").await;
        assert!(host.begin("demo", "two").is_ok());
        host.invalidate_page(1);
        assert!(host.begin("demo", "two").is_err());
        assert!(host.acquire(0, "demo", "late").is_err());
        host.acquire(1, "demo", "fresh").unwrap();
    }
    #[tokio::test]
    async fn plugin_host_shutdown_is_bounded_when_a_filesystem_call_is_stuck() {
        let host = PluginHostState::default();
        host.acquire(0, "demo", "one").unwrap();
        let permit = host.begin("demo", "one").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(4), host.shutdown())
            .await
            .expect("app exit must not wait forever for a blocking filesystem call");
        assert!(*permit.cancellation().borrow());
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    #[test]
    fn plugin_host_context_is_explicit_and_does_not_follow_active_workspace() {
        let state = crate::state::AppStateStore::default();
        let local = state.snapshot().active_workspace_id.0;
        let remote = state.create_workspace_at_path(std::env::temp_dir()).0;
        let mut snapshot = state.snapshot();
        snapshot
            .workspaces
            .iter_mut()
            .find(|ws| ws.workspace_id.0 == remote)
            .unwrap()
            .host_id = Some(42);
        snapshot.active_workspace_id = crate::state::WorkspaceId(remote.clone());
        state.replace_snapshot(snapshot);
        assert!(workspace_context(&state, None).is_ok());
        assert!(workspace_context(&state, Some(&local)).is_ok());
        assert!(workspace_context(&state, Some(&remote)).is_err());
        assert!(workspace_context(&state, Some("closed-workspace")).is_err());
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OsInfo {
    platform: &'static str,
    arch: &'static str,
    home_dir: Option<String>,
    config_dir: Option<String>,
    data_dir: Option<String>,
    local_data_dir: Option<String>,
}
fn os_info() -> Result<OsInfo, String> {
    if !matches!(std::env::consts::OS, "linux" | "windows" | "macos")
        || !matches!(std::env::consts::ARCH, "x86_64" | "aarch64")
    {
        return Err("Unsupported platform/architecture for plugin OS info".into());
    }
    let text = |p: Option<std::path::PathBuf>| p.map(|p| p.to_string_lossy().into_owned());
    Ok(OsInfo {
        platform: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        home_dir: text(dirs::home_dir()),
        config_dir: text(dirs::config_dir()),
        data_dir: text(dirs::data_dir()),
        local_data_dir: text(dirs::data_local_dir()),
    })
}
#[tauri::command]
pub fn plugins_os_info<R: Runtime>(
    app: AppHandle<R>,
    host: State<'_, PluginHostState>,
    plugin_id: String,
    token: String,
) -> Result<OsInfo, String> {
    super::commands::gui(&app)?;
    let _permit = host.begin(&plugin_id, &token)?;
    os_info()
}

#[cfg(test)]
mod os_tests {
    use super::*;
    #[test]
    fn plugin_host_os_literals_and_known_dirs_match_native_resolution() {
        let value = os_info().unwrap();
        assert_eq!(value.platform, std::env::consts::OS);
        assert_eq!(value.arch, std::env::consts::ARCH);
        assert_eq!(
            value.home_dir,
            dirs::home_dir().map(|p| p.to_string_lossy().into_owned())
        );
        assert_eq!(
            value.config_dir,
            dirs::config_dir().map(|p| p.to_string_lossy().into_owned())
        );
        assert_eq!(
            value.data_dir,
            dirs::data_dir().map(|p| p.to_string_lossy().into_owned())
        );
        assert_eq!(
            value.local_data_dir,
            dirs::data_local_dir().map(|p| p.to_string_lossy().into_owned())
        );
    }
}
