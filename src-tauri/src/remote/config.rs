//! State-dir + manifest path resolution.
//!
//! The daemon's whole on-disk footprint lives under one directory,
//! by default `~/.local/share/codemux-remote/`. Everything is
//! addressable as `<state_dir>/<thing>`:
//!
//! ```text
//! <state_dir>/manifest.json   # endpoint + secret + pid + started_at
//! <state_dir>/codemux.db      # SQLite workspace registry
//! <state_dir>/workspaces/     # worktree storage
//! <state_dir>/serve.log       # daemonized-mode log
//! <state_dir>/logs/           # detached worktree-setup logs
//! ```

use std::path::PathBuf;

/// Default state directory for `codemux-remote serve`.
///
/// `XDG_DATA_HOME` / `~/.local/share/codemux-remote` on Linux.
/// `~/Library/Application Support/codemux-remote` on macOS.
/// Falls back to `~/.codemux-remote` if `dirs::data_dir()` returns
/// `None` (extremely unusual).
pub fn default_state_dir() -> PathBuf {
    if let Some(data_dir) = dirs::data_dir() {
        return data_dir.join("codemux-remote");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".codemux-remote");
    }
    // Last resort. Anyone hitting this has bigger problems than where
    // their state directory lives.
    PathBuf::from(".codemux-remote")
}

/// Create a private state directory, or verify an existing one. An older
/// default state directory is ours to tighten; an existing custom directory
/// may be shared with other tools, so its mode is never changed here.
pub fn ensure_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    ensure_private_dir_with_default(dir, &default_state_dir())
}

fn ensure_private_dir_with_default(
    dir: &std::path::Path,
    default: &std::path::Path,
) -> std::io::Result<()> {
    use cap_fs_ext::DirExt;
    use std::io::{Error, ErrorKind};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    if dir.as_os_str().is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "state directory cannot be empty",
        ));
    }

    match std::fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        Err(e) => return Err(e),
    }

    // Check and, for the known default only, chmod the opened directory.
    // Resolve the final component relative to its parent without following
    // a symlink, including when the supplied path has a trailing separator.
    let normalized: PathBuf = dir.components().collect();
    let name = normalized.file_name().ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidInput,
            "state directory must name a directory",
        )
    })?;
    let parent = normalized
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let parent = cap_std::fs::Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
    let opened = parent.open_dir_nofollow(name)?;
    let meta = opened.try_clone()?.into_std_file().metadata()?;
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            format!(
                "state directory {} is not owned by this user",
                dir.display()
            ),
        ));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        if dir != default {
            return Err(Error::new(ErrorKind::PermissionDenied, format!(
                "custom state directory {} is accessible to other users; choose a private directory or chmod it to 0700",
                dir.display()
            )));
        }
        opened
            .open(".")?
            .into_std()
            .set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Manifest file inside a given state-dir.
pub fn manifest_path(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("manifest.json")
}

/// SQLite workspace registry inside a given state-dir.
pub fn database_path(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("codemux.db")
}

/// Worktree storage root inside a given state-dir. The daemon checks
/// each new workspace's worktree out under
/// `<state_dir>/workspaces/<workspace_id>/`.
pub fn workspaces_root(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("workspaces")
}

/// Serve mode log file (only used when daemonized).
pub fn serve_log_path(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("serve.log")
}

/// Logs of worktree setup commands that `worktree create` ran detached
/// because no daemon was up to own them.
pub fn logs_dir(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn existing_custom_root_must_already_be_private() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("shared");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

        let error =
            ensure_private_dir_with_default(&root, &temp.path().join("default")).unwrap_err();
        assert!(error.to_string().contains("custom state directory"));
        assert_eq!(mode(&root), 0o755);

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        ensure_private_dir_with_default(&root, &temp.path().join("default")).unwrap();
        assert_eq!(mode(&root), 0o700);
    }

    #[test]
    fn older_default_root_is_tightened() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("default");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

        ensure_private_dir_with_default(&root, &root).unwrap();
        assert_eq!(mode(&root), 0o700);
    }

    #[test]
    fn state_dir_symlink_cannot_redirect_chmod() {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        let link = temp.path().join("default");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&shared, &link).unwrap();

        assert!(ensure_private_dir_with_default(&link, &link).is_err());
        assert_eq!(mode(&shared), 0o755);
    }
}
