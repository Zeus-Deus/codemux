//! Full-authority renderer plugins: discovery, source reads, file watching,
//! enable decisions and safe mode. Plugin code itself runs in the webview
//! (`src/contrib/`); this module only owns the files and the persisted state.
//!
//! Layout under `<data_dir>/<APP_DIR_NAME>/`:
//!
//! ```text
//! plugins/<id>/plugin.js              one plain ESM entry per plugin
//! plugins/<id>/.codemux-plugin.json   install marker (absent = Local)
//! plugins.sqlite                      decisions(id, enabled), meta(key, value)
//! plugins.loading                     unclean-start marker (safe mode)
//! ```

pub mod catalog;
pub mod commands;
pub mod host;
pub mod mcp;
pub mod install;
pub mod paths;
pub mod registry;
pub mod tap;
pub mod watch;
pub mod platform;
pub mod platform_authority;
pub mod platform_events;
pub mod platform_management;
pub mod platform_manifest;
pub mod platform_package;
pub mod platform_runtime;
pub mod platform_store;
pub mod platform_transport;
pub mod platform_workflow;
#[cfg(test)]
mod platform_acceptance_tests;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use paths::{valid_id, PluginPathError};

/// Largest plugin source the renderer will evaluate. A file above this is
/// refused rather than truncated: half a module can still parse.
pub const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
pub const ENTRY_FILE: &str = "plugin.js";
pub const MARKER_FILE: &str = ".codemux-plugin.json";

/// `CODEMUX_DISABLE_PLUGINS=1` loads no plugin at all, bundled ones included.
pub fn disabled_by_env() -> bool {
    std::env::var("CODEMUX_DISABLE_PLUGINS")
        .map(|v| {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

/// `<data_dir>/<APP_DIR_NAME>`; the plugins root and its registry live here.
pub fn app_data_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join(crate::APP_DIR_NAME))
}

pub fn default_root() -> Option<PathBuf> {
    Some(app_data_dir()?.join("plugins"))
}

/// Where an installed plugin came from. Absent marker = a Local folder.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallMarker {
    /// `"catalog"` or `"git"`.
    pub source: String,
    pub repo: String,
    pub sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    /// `"official"` or `"unverified"`.
    pub tier: String,
    pub installed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdir: Option<String>,
    /// Branch or tag a Git install follows for updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One folder in the plugins root that carries a `plugin.js`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiskPluginEntry {
    /// Folder name. The plugin's `id` must equal it.
    pub id: String,
    /// Absolute `plugin.js` path, for Reveal and error rows.
    pub file: String,
    pub dir: String,
    pub marker: Option<InstallMarker>,
    /// Set when the folder can't be loaded at all (bad name, unreadable marker).
    pub error: Option<String>,
}

/// List plugin folders, sorted by name so first-wins is stable across launches.
/// Dot-folders (staging, backups) and folders without `plugin.js` are skipped.
pub fn list(root: &Path) -> std::io::Result<Vec<DiskPluginEntry>> {
    let read = match std::fs::read_dir(root) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let dir = entry.path();
        // A symlinked folder is allowed (linked dev checkout); file_type()
        // doesn't follow links, metadata() does.
        if !std::fs::metadata(&dir).map(|m| m.is_dir()).unwrap_or(false) {
            continue;
        }
        let file = dir.join(ENTRY_FILE);
        if !file.is_file() {
            continue;
        }
        let mut error = None;
        if !valid_id(&name) {
            error = Some(format!(
                "Folder name \"{name}\" is not a valid plugin id (lowercase letters, digits and dashes, at most 64)"
            ));
        }
        let marker = match read_marker(&dir) {
            Ok(marker) => marker,
            Err(cause) => {
                error.get_or_insert(cause);
                None
            }
        };
        out.push(DiskPluginEntry {
            id: name,
            file: file.to_string_lossy().to_string(),
            dir: dir.to_string_lossy().to_string(),
            marker,
            error,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

pub fn read_marker(dir: &Path) -> Result<Option<InstallMarker>, String> {
    let path = dir.join(MARKER_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("Install marker is unreadable: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Install marker is unreadable: {error}")),
    }
}

/// Read `<root>/<id>/plugin.js` in full, or refuse. Never truncates.
pub fn read_source(root: &Path, id: &str) -> Result<String, String> {
    let file = paths::contained_entry(root, id).map_err(|e| e.to_string())?;
    let meta = std::fs::metadata(&file).map_err(|e| format!("Cannot read {ENTRY_FILE}: {e}"))?;
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "{ENTRY_FILE} is {} bytes; the limit is {MAX_SOURCE_BYTES} bytes",
            meta.len()
        ));
    }
    let bytes = std::fs::read(&file).map_err(|e| format!("Cannot read {ENTRY_FILE}: {e}"))?;
    // Re-check after the read: the file may have grown between stat and read.
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(format!("{ENTRY_FILE} exceeds {MAX_SOURCE_BYTES} bytes"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{ENTRY_FILE} is not UTF-8 text"))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod install_tests;
