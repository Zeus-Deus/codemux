//! Per-user directories for files Codemux hands to other processes.
//!
//! The shared temp directory is the wrong place for these on Unix: another
//! local user can create a fixed path there first, point it somewhere else
//! with a symlink, plant files in it, or read what lands in it.

use std::io;
use std::path::{Path, PathBuf};

/// `<root>/<app>/<name>`, created owner-only. The root is the user's runtime
/// directory (owner-only, cleared at logout) when the platform has one and it
/// passes the checks in [`ensure_private_dir`], else the user's cache
/// directory.
pub fn user_private_dir(name: &str) -> io::Result<PathBuf> {
    let mut error = io::Error::new(io::ErrorKind::NotFound, "no per-user directory is available");
    for root in candidate_roots() {
        match ensure_private_dir(&root.join(crate::APP_DIR_NAME), name) {
            Ok(dir) => return Ok(dir),
            Err(next) => error = next,
        }
    }
    Err(error)
}

#[cfg(not(test))]
fn candidate_roots() -> Vec<PathBuf> {
    [dirs::runtime_dir(), dirs::cache_dir()].into_iter().flatten().collect()
}

// Tests never touch the user's real runtime or cache directory. One root per
// user, reused across runs, so test runs do not pile up folders.
#[cfg(test)]
fn candidate_roots() -> Vec<PathBuf> {
    #[cfg(unix)]
    // SAFETY: geteuid takes no arguments and returns the effective UID.
    let user = unsafe { libc::geteuid() }.to_string();
    #[cfg(not(unix))]
    let user = "user";
    vec![std::env::temp_dir().join(format!("codemux-test-private-{user}"))]
}

/// Create `parent/name` and return it once it is safe to use.
///
/// On Unix both `parent` and the new directory must be real directories (not
/// symlinks) owned by this user and not world-writable, and the new directory
/// is set to 0700 even when it already existed. Group write is tolerated
/// because a umask of 002 with a per-user group gives every directory the
/// user creates that mode.
pub(crate) fn ensure_private_dir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let dir = parent.join(name);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700);
        builder.create(&dir)?;
        check_owned(parent)?;
        check_owned(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    builder.create(&dir)?;
    Ok(dir)
}

#[cfg(unix)]
fn check_owned(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    // SAFETY: geteuid takes no arguments and returns the effective UID.
    let uid = unsafe { libc::geteuid() };
    if meta.is_dir() && meta.uid() == uid && meta.mode() & 0o002 == 0 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a private directory owned by this user", path.display()),
        ))
    }
}

/// Create a new owner-only file at `path`, failing if anything (including a
/// symlink) is already there.
pub(crate) fn create_private_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn creates_owner_only_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = ensure_private_dir(&root.path().join("app"), "things").unwrap();
        assert_eq!(dir, root.path().join("app/things"));
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&root.path().join("app")), 0o700);
    }

    #[test]
    fn tightens_an_existing_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("things");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o775)).unwrap();
        ensure_private_dir(root.path(), "things").unwrap();
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn refuses_a_shared_or_symlinked_directory() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        // A world-writable leaf or parent is refused.
        assert!(ensure_private_dir(root.path(), "shared").is_err());
        assert_eq!(mode(&shared), 0o777, "a refused directory is left alone");
        assert!(ensure_private_dir(&shared, "inner").is_err());

        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), root.path().join("link")).unwrap();
        assert!(ensure_private_dir(root.path(), "link").is_err());
    }

    #[test]
    fn private_file_is_0600_and_never_follows_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file");
        drop(create_private_file(&path).unwrap());
        assert_eq!(mode(&path), 0o600);
        assert!(create_private_file(&path).is_err(), "an existing file is never reused");

        let target = root.path().join("target");
        std::fs::write(&target, "keep").unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(create_private_file(&link).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    }

    #[test]
    fn user_private_dir_is_owner_only() {
        let dir = user_private_dir("private-dir-test").unwrap();
        assert!(dir.ends_with(Path::new(crate::APP_DIR_NAME).join("private-dir-test")));
        assert_eq!(mode(&dir), 0o700);
    }
}
