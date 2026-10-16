//! Path safety for plugin ids. The renderer only ever passes an id (a single
//! folder name), never a path, and every resolved path is re-checked against
//! the canonical root so a symlink or `..` can't reach outside it.

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum PluginPathError {
    InvalidId(String),
    Missing(String),
    Outside(String),
}

impl std::fmt::Display for PluginPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(f, "\"{id}\" is not a valid plugin id"),
            Self::Missing(id) => write!(f, "Plugin \"{id}\" is not installed"),
            Self::Outside(id) => write!(f, "Plugin \"{id}\" resolves outside the plugins folder"),
        }
    }
}

/// `[a-z0-9-]{1,64}`: one path segment, no dots, no separators.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `<root>/<id>` without touching the filesystem; the id is validated.
pub fn plugin_dir(root: &Path, id: &str) -> Result<PathBuf, PluginPathError> {
    if !valid_id(id) {
        return Err(PluginPathError::InvalidId(id.to_string()));
    }
    Ok(root.join(id))
}

/// `<root>/<id>/plugin.js`, canonicalized and required to stay inside the
/// canonical root. A linked plugin folder may point anywhere, so containment
/// is checked on the folder's own entry, which is a direct child of root.
pub fn contained_entry(root: &Path, id: &str) -> Result<PathBuf, PluginPathError> {
    let dir = plugin_dir(root, id)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|_| PluginPathError::Missing(id.to_string()))?;
    // The folder entry itself (not its target) must be a direct child of the
    // root. `symlink_metadata` doesn't follow the link.
    std::fs::symlink_metadata(&dir).map_err(|_| PluginPathError::Missing(id.to_string()))?;
    let entry = dir.join(super::ENTRY_FILE);
    let canonical_entry = entry
        .canonicalize()
        .map_err(|_| PluginPathError::Missing(id.to_string()))?;
    // A plain folder must keep its entry inside the root. A linked folder
    // (developer checkout) is the user's own explicit link: its target may
    // live elsewhere, but the entry name may not escape that target.
    let is_link = std::fs::symlink_metadata(&dir)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    let base = if is_link {
        dir.canonicalize()
            .map_err(|_| PluginPathError::Missing(id.to_string()))?
    } else {
        canonical_root.join(id)
    };
    if canonical_entry.parent() != Some(base.as_path()) {
        return Err(PluginPathError::Outside(id.to_string()));
    }
    if !is_link && !canonical_entry.starts_with(&canonical_root) {
        return Err(PluginPathError::Outside(id.to_string()));
    }
    Ok(canonical_entry)
}

/// Remove `<root>/<id>`. A symlinked folder is unlinked, never its target.
pub fn remove_plugin_dir(root: &Path, id: &str) -> Result<(), String> {
    let dir = plugin_dir(root, id).map_err(|e| e.to_string())?;
    let canonical_root = root
        .canonicalize()
        .map_err(|_| PluginPathError::Missing(id.to_string()).to_string())?;
    let meta = std::fs::symlink_metadata(&dir)
        .map_err(|_| PluginPathError::Missing(id.to_string()).to_string())?;
    // The parent of the folder entry must be the root itself.
    let parent = dir
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .ok_or_else(|| PluginPathError::Outside(id.to_string()).to_string())?;
    if parent != canonical_root {
        return Err(PluginPathError::Outside(id.to_string()).to_string());
    }
    if meta.file_type().is_symlink() {
        #[cfg(windows)]
        {
            // A directory junction/symlink on Windows is removed with remove_dir.
            return std::fs::remove_dir(&dir)
                .or_else(|_| std::fs::remove_file(&dir))
                .map_err(|e| format!("Cannot remove plugin link: {e}"));
        }
        #[cfg(not(windows))]
        return std::fs::remove_file(&dir).map_err(|e| format!("Cannot remove plugin link: {e}"));
    }
    if !meta.is_dir() {
        return Err(format!("\"{id}\" is not a plugin folder"));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Cannot remove plugin: {e}"))
}
