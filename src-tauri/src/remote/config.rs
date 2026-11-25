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
/// default state directory can be tightened if other users could read but
/// not modify it. An existing custom directory may be shared with other
/// tools, so its mode is never changed here.
pub fn ensure_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    open_private_dir(dir).map(|_| ())
}

/// Verify an existing state directory without creating one. Commands that
/// inspect an existing daemon use this before trusting a manifest from disk.
pub fn private_dir_exists(dir: &std::path::Path) -> std::io::Result<bool> {
    use std::io::{Error, ErrorKind};

    if dir.as_os_str().is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "state directory cannot be empty",
        ));
    }

    match std::fs::symlink_metadata(dir) {
        Ok(_) => verify_private_dir_with_default(dir, &default_state_dir()).map(|_| true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Return the verified directory handle for operations that must stay inside
/// the checked state directory even if its pathname changes afterward.
pub(crate) fn open_private_dir(dir: &std::path::Path) -> std::io::Result<cap_std::fs::Dir> {
    open_private_dir_with_default(dir, &default_state_dir())
}

#[cfg(test)]
fn ensure_private_dir_with_default(
    dir: &std::path::Path,
    default: &std::path::Path,
) -> std::io::Result<()> {
    open_private_dir_with_default(dir, default).map(|_| ())
}

fn open_private_dir_with_default(
    dir: &std::path::Path,
    default: &std::path::Path,
) -> std::io::Result<cap_std::fs::Dir> {
    use std::io::{Error, ErrorKind};
    use std::os::unix::fs::DirBuilderExt;

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

    verify_private_dir_with_default(dir, default)
}

fn verify_private_dir_with_default(
    dir: &std::path::Path,
    default: &std::path::Path,
) -> std::io::Result<cap_std::fs::Dir> {
    use cap_fs_ext::DirExt;
    use std::io::{Error, ErrorKind};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    // SQLite opens the registry and its WAL by pathname. A private leaf is
    // insufficient if another user can replace an ancestor between this
    // check and SQLite's open. Walk every symlink hop, including targets
    // hidden by later symlinks, before using that pathname.
    ensure_stable_ancestors(dir)?;

    // Check and, for a nonwritable known default only, chmod the opened
    // directory.
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
                "custom state directory {} is accessible to other users; use a fresh private state directory instead. To recover data, see https://github.com/Zeus-Deus/codemux/blob/main/docs/security/remote-state-recovery.md",
                dir.display()
            )));
        }
        // A formerly readable default root is safe to tighten when its
        // parent is stable. A writable one could contain planted entries,
        // so it needs user inspection before any automatic migration.
        if meta.permissions().mode() & 0o022 != 0 {
            return Err(Error::new(ErrorKind::PermissionDenied, format!(
                "default state directory {} was writable by other users; do not reuse it. Stop the remote service through its service manager, isolate the old directory, and start with a fresh private state directory. Migrate only trusted workspace records; see https://github.com/Zeus-Deus/codemux/blob/main/docs/security/remote-state-recovery.md",
                dir.display()
            )));
        }
        opened
            .open(".")?
            .into_std()
            .set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(opened)
}

fn ensure_stable_ancestors(dir: &std::path::Path) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    let mut candidate = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()?.join(dir)
    };
    let uid = unsafe { libc::geteuid() };
    for _ in 0..40 {
        let mut prefix = std::path::PathBuf::new();
        let mut components = candidate.components();
        let mut next = None;
        while let Some(component) = components.next() {
            match component {
                Component::CurDir => continue,
                Component::ParentDir => {
                    prefix.pop();
                    continue;
                }
                _ => prefix.push(component.as_os_str()),
            }
            if prefix == std::path::Path::new("/") {
                continue;
            }
            let meta = std::fs::symlink_metadata(&prefix)?;
            if meta.uid() != uid && meta.uid() != 0 {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    format!("state directory ancestor {} is owned by another user", prefix.display()),
                ));
            }
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&prefix)?;
                let mut resolved = if target.is_absolute() {
                    target
                } else {
                    prefix.parent().unwrap().join(target)
                };
                resolved.push(components.as_path());
                next = Some(resolved);
                break;
            }
            // A sticky directory protects an entry owned by this user (or
            // root), as /tmp does for a user-owned temporary directory.
            if !components.as_path().as_os_str().is_empty()
                && meta.is_dir()
                && meta.mode() & 0o022 != 0
                && meta.mode() & 0o1000 == 0
            {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    format!("state directory ancestor {} can be replaced by another user", prefix.display()),
                ));
            }
        }
        match next {
            Some(resolved) => candidate = resolved,
            None => return Ok(()),
        }
    }
    Err(Error::new(ErrorKind::InvalidInput, "too many state directory symlinks"))
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
pub(crate) fn private_test_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn checking_existing_state_does_not_create_or_trust_shared_roots() {
        let temp = private_test_dir();
        let state = temp.path().join("state");
        assert!(!private_dir_exists(&state).unwrap());
        assert!(!state.exists());

        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(private_dir_exists(&state).is_err());
        assert_eq!(mode(&state), 0o777);

        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(private_dir_exists(&state).unwrap());
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
        assert!(error.to_string().contains("fresh private state directory"));
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
    fn writable_older_default_root_requires_manual_migration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("default");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o775)).unwrap();

        let error = ensure_private_dir_with_default(&root, &root).unwrap_err();
        assert!(error.to_string().contains("do not reuse it"), "{error}");
        assert!(error.to_string().contains("fresh private state directory"), "{error}");
        assert!(error.to_string().contains("Migrate only trusted workspace records"), "{error}");
        assert!(error.to_string().contains("remote-state-recovery.md"), "{error}");
        assert_eq!(mode(&root), 0o775);
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

    #[test]
    fn private_leaf_under_replaceable_ancestor_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        let state = shared.join("private");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();

        let error = ensure_private_dir_with_default(&state, &temp.path().join("default"))
            .unwrap_err();
        assert!(error.to_string().contains("can be replaced"), "{error}");
        assert_eq!(mode(&state), 0o700);
    }

    #[test]
    fn symlink_into_replaceable_ancestor_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        let state = shared.join("private");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = temp.path().join("linked-state");
        std::os::unix::fs::symlink(&state, &link).unwrap();

        let error = ensure_private_dir_with_default(&link, &temp.path().join("default"))
            .unwrap_err();
        assert!(error.to_string().contains("can be replaced"), "{error}");
    }

    #[test]
    fn intermediate_symlink_chain_cannot_hide_a_replaceable_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let safe = temp.path().join("safe");
        let shared = temp.path().join("shared");
        let target = temp.path().join("target");
        let state = target.join("state");
        for dir in [&safe, &shared, &state] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&target, shared.join("relay")).unwrap();
        std::os::unix::fs::symlink(shared.join("relay"), safe.join("link")).unwrap();

        let via_chain = safe.join("link/state");
        let error = ensure_private_dir_with_default(&via_chain, &temp.path().join("default"))
            .unwrap_err();
        assert!(error.to_string().contains("can be replaced"), "{error}");
    }
}
