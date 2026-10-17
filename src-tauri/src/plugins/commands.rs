//! Tauri commands for plugins. Desktop-only: the web remote denies every
//! `plugins_*` command before dispatch (web_remote/dispatch.rs).

use super::{
    catalog::{CatalogCache, CatalogView},
    install::{self, Existing},
    registry::{Registry, SafeMode, SafeModeStatus, REGISTRY_FILE},
    watch, DiskPluginEntry, InstallMarker,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

#[derive(Default)]
pub struct PluginsState {
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    /// Opened on first use; an open error is kept and puts the launch in
    /// safe mode instead of failing every call.
    registry: OnceLock<Result<Registry, String>>,
    safe_mode: OnceLock<SafeMode>,
    catalog: CatalogCache,
    /// One install/update/remove at a time.
    installing: tokio::sync::Mutex<()>,
}

impl PluginsState {
    fn registry(&self) -> &Result<Registry, String> {
        self.registry.get_or_init(|| {
            let dir = super::app_data_dir().ok_or("App data directory is unavailable")?;
            Registry::open(&dir.join(REGISTRY_FILE))
        })
    }

    /// The catalog without the network: the persisted copy from the last
    /// fetch, else the bundled one.
    fn catalog_now(&self) -> super::catalog::Catalog {
        if let Ok(registry) = self.registry() {
            if let Ok(Some(json)) = registry.meta(CATALOG_META_KEY) {
                if let Ok(saved) = serde_json::from_str::<SavedCatalog>(&json) {
                    self.catalog.seed(saved.catalog, saved.fetched_at);
                }
            }
        }
        self.catalog.current().catalog
    }

    /// The catalog, re-fetched when due. Every caller goes through here, so
    /// a successful fetch is always persisted, and a changed blocklist always
    /// makes the renderer rescan, which disables a newly blocked plugin at
    /// once (whoever triggered the fetch: Settings, install, the startup
    /// check).
    async fn catalog_fresh<R: Runtime>(&self, app: &AppHandle<R>, force: bool) -> CatalogView {
        let before = self.catalog_now().blocked;
        let view = self.catalog.get(force).await;
        if let (Some((catalog, fetched_at)), Ok(registry)) = (self.catalog.fetched(), self.registry()) {
            if let Ok(json) = serde_json::to_string(&SavedCatalog { catalog, fetched_at }) {
                let _ = registry.set_meta(CATALOG_META_KEY, &json);
            }
        }
        if view.catalog.blocked != before {
            let _ = app.emit_to("main", watch::CHANGED_EVENT, watch::Changed { id: None });
        }
        view
    }

    fn safe_mode(&self) -> &SafeMode {
        self.safe_mode.get_or_init(|| {
            let error = self.registry().as_ref().err().cloned();
            let dir = super::app_data_dir().unwrap_or_default();
            SafeMode::detect(&dir, error.as_deref())
        })
    }
}

const CATALOG_META_KEY: &str = "catalog";

#[derive(Serialize, serde::Deserialize)]
struct SavedCatalog {
    catalog: super::catalog::Catalog,
    fetched_at: u64,
}

fn gui<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    crate::ensure_gui_mode(app).map_err(|_| "Plugins are available only in the desktop app".to_string())
}

pub(crate) fn root() -> Result<PathBuf, String> {
    let root = super::default_root().ok_or("App data directory is unavailable")?;
    std::fs::create_dir_all(&root).map_err(|e| format!("Cannot create the plugins folder: {e}"))?;
    Ok(root)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginsRoot {
    pub path: String,
    /// `CODEMUX_DISABLE_PLUGINS=1`: the renderer loads nothing.
    pub disabled_by_env: bool,
}

#[tauri::command]
pub fn plugins_root<R: Runtime>(app: AppHandle<R>) -> Result<PluginsRoot, String> {
    gui(&app)?;
    Ok(PluginsRoot {
        path: root()?.to_string_lossy().to_string(),
        disabled_by_env: super::disabled_by_env(),
    })
}

#[tauri::command]
pub async fn plugins_list<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
) -> Result<Vec<DiskPluginEntry>, String> {
    gui(&app)?;
    let root = root()?;
    let catalog = state.catalog_now();
    // Tidy what an interrupted install left behind, but never while an
    // install, update or remove is running: its staging folder and backup
    // are live then. Skipped now, it runs on a later listing.
    if let Ok(_quiet) = state.installing.try_lock() {
        let root = root.clone();
        tauri::async_runtime::spawn_blocking(move || install::recover(&root))
            .await
            .map_err(|e| e.to_string())?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut entries = super::list(&root).map_err(|e| e.to_string())?;
        // Blocked ids or commits are disabled at load, with the reason shown.
        for entry in &mut entries {
            let sha = entry.marker.as_ref().map(|m| m.sha.as_str());
            if let Some(reason) = catalog.blocked_reason(&entry.id, sha) {
                entry.error.get_or_insert(format!("Blocked by the Codemux catalog: {reason}"));
            }
        }
        Ok(entries)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn plugins_read_source<R: Runtime>(app: AppHandle<R>, id: String) -> Result<String, String> {
    gui(&app)?;
    let root = root()?;
    tauri::async_runtime::spawn_blocking(move || super::read_source(&root, &id))
        .await
        .map_err(|e| e.to_string())?
}

/// Start the root watcher (idempotent). Emits `plugins_changed { id }`.
#[tauri::command]
pub fn plugins_watch<R: Runtime>(app: AppHandle<R>, state: State<'_, PluginsState>) -> Result<(), String> {
    gui(&app)?;
    let mut slot = state.watcher.lock().unwrap();
    if slot.is_some() {
        return Ok(());
    }
    let root = root()?;
    let handle = app.clone();
    let watcher = watch::start(root, move |changes| {
        for change in changes {
            let _ = handle.emit_to("main", watch::CHANGED_EVENT, change);
        }
    })
    .map_err(|e| format!("Cannot watch the plugins folder: {e}"))?;
    *slot = Some(watcher);
    // Re-check the catalog's blocklist in the background, never during
    // startup. A new block makes the renderer rescan, which disables it.
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let state = handle.state::<PluginsState>();
        let _ = state.catalog_fresh(&handle, false).await;
    });
    Ok(())
}

/// Open the plugins folder in the system file manager.
#[tauri::command]
pub fn plugins_reveal_root<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    gui(&app)?;
    let root = root()?;
    tauri_plugin_opener::open_path(root.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn plugins_get_decisions<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
) -> Result<BTreeMap<String, bool>, String> {
    gui(&app)?;
    match state.registry() {
        Ok(registry) => registry.decisions(),
        // Safe mode reports the problem; an empty map lets bundled plugins
        // fall back to their defaults.
        Err(_) => Ok(BTreeMap::new()),
    }
}

/// Written only by Settings → Plugins. The SDK has no verb for this, so one
/// plugin can't switch another.
#[tauri::command]
pub fn plugins_set_enabled<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
    id: String,
    on: bool,
) -> Result<(), String> {
    gui(&app)?;
    state.registry().as_ref().map_err(|e| e.clone())?.set_enabled(&id, on)
}

#[tauri::command]
pub fn plugins_safe_mode_status<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
) -> Result<SafeModeStatus, String> {
    gui(&app)?;
    Ok(state.safe_mode().status())
}

/// `phase`: `"loading"` before activation, `"ok"` once the app has stayed up.
#[tauri::command]
pub fn plugins_startup_checkpoint<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
    phase: String,
) -> Result<(), String> {
    gui(&app)?;
    let safe = state.safe_mode();
    match phase.as_str() {
        "loading" => safe.begin_loading(),
        "ok" => safe.startup_ok(),
        other => Err(format!("unknown checkpoint {other}")),
    }
}

#[tauri::command]
pub fn plugins_resume<R: Runtime>(app: AppHandle<R>, state: State<'_, PluginsState>) -> Result<(), String> {
    gui(&app)?;
    state.safe_mode().resume();
    Ok(())
}

/// Turn the chat event tap on (first `host.chat.onEvent` subscriber) or off
/// (last one gone). See `plugins/tap.rs`.
#[tauri::command]
pub fn plugins_set_event_tap<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<(), String> {
    gui(&app)?;
    super::tap::set_enabled(on);
    Ok(())
}

// ── catalog, install, update, remove ──────────────────────────────────

/// The catalog (bundled copy until the first fetch; re-fetched every 24 h).
#[tauri::command]
pub async fn plugins_catalog<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
    refresh: Option<bool>,
) -> Result<CatalogView, String> {
    gui(&app)?;
    Ok(state.catalog_fresh(&app, refresh.unwrap_or(false)).await)
}

/// What the install dialog shows before anything is written.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstallPreview {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub source: String,
    pub repo: String,
    pub sha: String,
    pub subdir: Option<String>,
    pub tier: String,
    /// Set when this replaces an existing install (the installed commit).
    pub replaces: Option<String>,
}

struct Plan {
    preview: InstallPreview,
    marker: InstallMarker,
    files: Vec<(String, Vec<u8>)>,
}

fn blocked_check(catalog: &super::catalog::Catalog, id: &str, sha: &str) -> Result<(), String> {
    match catalog.blocked_reason(id, Some(sha)) {
        Some(reason) => Err(format!("Blocked by the Codemux catalog: {reason}")),
        None => Ok(()),
    }
}

fn plan_git(root: &std::path::Path, catalog: &super::catalog::Catalog, url: &str, git_ref: Option<&str>) -> Result<Plan, String> {
    let repo = install::parse_repo(url)?;
    let git = install::Git::new()?;
    let tree = if git_ref.is_some() { None } else { repo.tree.as_deref() };
    let fetched = install::fetch_plugin(&git, &format!("{}.git", repo.repo), tree, git_ref, None)?;
    let id = install::folder_id(&fetched.meta, &repo.name)?;
    blocked_check(catalog, &id, &fetched.sha)?;
    finish_plan(root, id, "git", repo.repo, "unverified".into(), None, fetched)
}

fn plan_catalog(root: &std::path::Path, catalog: &super::catalog::Catalog, id: &str) -> Result<Plan, String> {
    let entry = catalog.entry(id).ok_or_else(|| format!("\"{id}\" is not in the plugin catalog"))?;
    blocked_check(catalog, &entry.id, &entry.sha)?;
    let git = install::Git::new()?;
    let fetched = install::fetch_plugin(&git, &format!("{}.git", entry.repo), None, Some(&entry.sha), entry.subdir.as_deref())?;
    if fetched.meta.id.as_deref().is_some_and(|found| found != entry.id) {
        return Err(format!(
            "The plugin at that commit says its id is \"{}\", not \"{}\"",
            fetched.meta.id.as_deref().unwrap_or(""),
            entry.id
        ));
    }
    let mut fetched = fetched;
    fetched.meta.name.get_or_insert_with(|| entry.name.clone());
    if !entry.description.is_empty() {
        fetched.meta.description.get_or_insert_with(|| entry.description.clone());
    }
    finish_plan(root, entry.id.clone(), "catalog", entry.repo.clone(), entry.tier.clone(), Some(entry.id.clone()), fetched)
}

fn finish_plan(
    root: &std::path::Path,
    id: String,
    source: &str,
    repo: String,
    tier: String,
    catalog_id: Option<String>,
    fetched: install::Fetched,
) -> Result<Plan, String> {
    let installed = match install::existing(root, &id)? {
        Existing::Installed(marker) => Some(marker),
        Existing::Local => {
            return Err(format!("A Local plugin folder named \"{id}\" already exists. Remove it first."))
        }
        Existing::Link => return Err(format!("\"{id}\" is a linked folder. Remove the link first.")),
        Existing::None => None,
    };
    let name = fetched.meta.name.clone().unwrap_or_else(|| id.clone());
    let marker = InstallMarker {
        source: source.into(),
        repo: repo.clone(),
        sha: fetched.sha.clone(),
        catalog_id,
        tier: tier.clone(),
        installed_at: install::now_rfc3339(),
        subdir: fetched.subdir.clone(),
        git_ref: fetched.git_ref.clone(),
        name: Some(name.clone()),
        description: fetched.meta.description.clone(),
    };
    // Only an update of the same plugin keeps the user's on/off decision.
    if let Some(installed) = &installed {
        install::check_replacement(&id, installed, &marker)?;
    }
    let replaces = installed.map(|installed| installed.sha);
    Ok(Plan {
        preview: InstallPreview {
            id,
            name,
            description: fetched.meta.description.clone(),
            version: fetched.meta.version.clone(),
            source: source.into(),
            repo,
            sha: fetched.sha,
            subdir: fetched.subdir,
            tier,
            replaces,
        },
        marker,
        files: fetched.files,
    })
}

/// Where to install from: a GitHub URL (optionally a ref) or a catalog id.
#[derive(serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum InstallSource {
    Git { url: String, git_ref: Option<String> },
    Catalog { id: String },
    /// The update of an installed plugin: same source, newest commit
    /// (catalog: the catalog's pinned commit).
    Update { id: String },
}

fn plan_for(root: &std::path::Path, catalog: &super::catalog::Catalog, source: &InstallSource) -> Result<Plan, String> {
    match source {
        InstallSource::Git { url, git_ref } => plan_git(root, catalog, url, git_ref.as_deref()),
        InstallSource::Catalog { id } => plan_catalog(root, catalog, id),
        InstallSource::Update { id } => {
            let marker = match install::existing(root, id)? {
                Existing::Installed(marker) => marker,
                _ => return Err(format!("\"{id}\" was not installed from the catalog or GitHub")),
            };
            match (marker.source.as_str(), marker.catalog_id.as_deref()) {
                ("catalog", Some(catalog_id)) => plan_catalog(root, catalog, catalog_id),
                _ => {
                    let repo = install::parse_repo(&marker.repo)?;
                    let git = install::Git::new()?;
                    let fetched = install::fetch_plugin(
                        &git,
                        &format!("{}.git", repo.repo),
                        None,
                        Some(marker.git_ref.as_deref().unwrap_or("HEAD")),
                        marker.subdir.as_deref(),
                    )?;
                    blocked_check(catalog, id, &fetched.sha)?;
                    let mut plan = finish_plan(root, id.clone(), "git", repo.repo, marker.tier.clone(), None, fetched)?;
                    plan.marker.git_ref = marker.git_ref.clone();
                    Ok(plan)
                }
            }
        }
    }
}

/// Resolve the source to one commit and describe it, without writing.
#[tauri::command]
pub async fn plugins_preview_install<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
    source: InstallSource,
) -> Result<InstallPreview, String> {
    gui(&app)?;
    let root = root()?;
    let catalog = state.catalog_fresh(&app, false).await.catalog;
    tauri::async_runtime::spawn_blocking(move || plan_for(&root, &catalog, &source).map(|p| p.preview))
        .await
        .map_err(|e| e.to_string())?
}

/// Install exactly the previewed commit. The plugin starts off unless
/// `enable` (the dialog's "Install and turn on"). A different commit than the
/// preview showed (the branch moved) is refused, so the dialog never lies.
#[tauri::command]
pub async fn plugins_install<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
    source: InstallSource,
    expected_sha: String,
    enable: bool,
) -> Result<InstallPreview, String> {
    gui(&app)?;
    let _one = state.installing.lock().await;
    let root = root()?;
    let catalog = state.catalog_fresh(&app, false).await.catalog;
    let plan = {
        let (root, catalog, source) = (root.clone(), catalog.clone(), source.clone());
        tauri::async_runtime::spawn_blocking(move || plan_for(&root, &catalog, &source))
            .await
            .map_err(|e| e.to_string())??
    };
    if plan.preview.sha != expected_sha {
        return Err("The plugin changed since the preview; review the new commit and try again".into());
    }
    let id = plan.preview.id.clone();
    let is_update = plan.preview.replaces.is_some();
    {
        let (root, id, files, marker) = (root.clone(), id.clone(), plan.files, plan.marker);
        tauri::async_runtime::spawn_blocking(move || install::write_install(&root, &id, &files, &marker))
            .await
            .map_err(|e| e.to_string())??;
    }
    // A new install starts off (or on, from "Install and turn on"); an update
    // keeps the user's decision.
    if let Ok(registry) = state.registry() {
        if !is_update || enable {
            registry.set_enabled(&id, enable)?;
        }
    }
    let _ = app.emit_to("main", watch::CHANGED_EVENT, watch::Changed { id: Some(id) });
    Ok(plan.preview)
}

/// Remove an installed or Local disk plugin. A linked folder is unlinked,
/// never its target. Bundled plugins have no folder and can't be removed.
#[tauri::command]
pub async fn plugins_remove<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
    id: String,
) -> Result<(), String> {
    gui(&app)?;
    let _one = state.installing.lock().await;
    let root = root()?;
    {
        let (root, id) = (root.clone(), id.clone());
        tauri::async_runtime::spawn_blocking(move || super::paths::remove_plugin_dir(&root, &id))
            .await
            .map_err(|e| e.to_string())??;
    }
    if let Ok(registry) = state.registry() {
        registry.forget(&id)?;
    }
    let _ = app.emit_to("main", watch::CHANGED_EVENT, watch::Changed { id: Some(id) });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(root: &std::path::Path, source: &str, repo: &str) -> Result<Plan, String> {
        finish_plan(
            root,
            "hello".into(),
            source,
            repo.into(),
            "unverified".into(),
            (source == "catalog").then(|| "hello".to_string()),
            install::Fetched {
                sha: "a".repeat(40),
                git_ref: None,
                subdir: None,
                files: vec![(super::super::ENTRY_FILE.into(), b"original".to_vec())],
                meta: install::ScannedMeta { id: Some("hello".into()), ..Default::default() },
            },
        )
    }

    #[test]
    fn plugin_install_identity_is_checked_in_the_preview() {
        let repo = "https://github.com/owner/hello";
        for (source, other) in [("git", "catalog"), ("catalog", "git")] {
            let root = tempfile::tempdir().unwrap();
            let fresh = plan(root.path(), source, repo).unwrap();
            assert_eq!(fresh.preview.replaces, None);
            install::write_install(root.path(), "hello", &fresh.files, &fresh.marker).unwrap();
            let update = plan(root.path(), source, "https://github.com/Owner/Hello").unwrap();
            assert_eq!(update.preview.replaces.as_deref(), Some(fresh.marker.sha.as_str()));

            let error = plan(root.path(), other, repo).err().expect("another source must not be an update");
            assert!(error.contains("different repository or source") && error.contains("Remove it first"), "{error}");

            let error = plan(root.path(), source, "https://github.com/fork/hello").err().unwrap();
            assert!(error.contains("different repository or source"), "{error}");
        }
    }
}
