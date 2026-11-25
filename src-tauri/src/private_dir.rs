//! Per-user directories for files Codemux hands to other processes.
//!
//! The shared temp directory is the wrong place for these on Unix: another
//! local user can create a fixed path there first, point it somewhere else
//! with a symlink, plant files in it, or read what lands in it.

use std::io;
use std::path::{Path, PathBuf};

/// `<root>/<app>/<name>`, created owner-only. The root is the user's runtime
/// directory (owner-only, cleared at logout) when the platform has one and it
/// passes [`private_dir_under`]'s checks, else the user's cache directory.
pub fn user_private_dir(name: &str) -> io::Result<PathBuf> {
    user_private_dir_in(&candidate_roots(), name)
}

/// The first root in `roots` that yields a safe `<root>/<app>/<name>`.
fn user_private_dir_in(roots: &[PathBuf], name: &str) -> io::Result<PathBuf> {
    let mut error = io::Error::new(io::ErrorKind::NotFound, "no per-user directory is available");
    for root in roots {
        match private_dir_under(root, name) {
            Ok(dir) => return Ok(dir),
            Err(next) => error = next,
        }
    }
    Err(error)
}

/// Never the shared temp dir: only the user's own runtime and cache dirs.
#[cfg_attr(test, allow(dead_code))]
fn production_roots() -> Vec<PathBuf> {
    [dirs::runtime_dir(), dirs::cache_dir()].into_iter().flatten().collect()
}

#[cfg(not(test))]
fn candidate_roots() -> Vec<PathBuf> {
    production_roots()
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

/// `<root>/<app>/<name>` once every directory on the way is safe.
///
/// On Unix the root is resolved and it and each of its ancestors must be a
/// directory owned by root or this user that others cannot write to (a
/// sticky directory such as `/tmp` is fine, since others cannot rename what
/// this user puts there). Otherwise someone else could swap the folders below
/// it after they pass their checks.
fn private_dir_under(root: &Path, name: &str) -> io::Result<PathBuf> {
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not an absolute path", root.display()),
        ));
    }
    #[cfg(unix)]
    let root = {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(root)?;
        let root = std::fs::canonicalize(root)?;
        for ancestor in root.ancestors() {
            check_trusted_ancestor(ancestor)?;
        }
        root
    };
    ensure_private_dir(&root.join(crate::APP_DIR_NAME), name)
}

#[cfg(unix)]
fn check_trusted_ancestor(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    let mode = meta.mode();
    if meta.is_dir()
        && (meta.uid() == 0 || meta.uid() == euid())
        && (mode & 0o022 == 0 || mode & 0o1000 != 0)
    {
        Ok(())
    } else {
        Err(not_private(path))
    }
}

/// Create `parent/name` and return it once it is safe to use.
///
/// On Unix both `parent` and the new directory must be real directories (not
/// symlinks) owned by this user. `parent` must not be world-writable and is
/// set to 0700, which clears the group write a umask of 002 leaves on folders
/// other code created. The new directory must not be group- or
/// world-writable, since others may already have put files in it; this code
/// always creates it 0700, and sets it to 0700 again in case it was widened.
pub(crate) fn ensure_private_dir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let dir = parent.join(name);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700);
        builder.create(&dir)?;
        // Tighten the parent first so nobody can swap the child after its
        // check.
        check_owned(parent, 0o002)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        check_owned(&dir, 0o022)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    builder.create(&dir)?;
    Ok(dir)
}

/// `path` is a real directory owned by this user with none of `forbidden`'s
/// mode bits set.
#[cfg(unix)]
fn check_owned(path: &Path, forbidden: u32) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() && meta.uid() == euid() && meta.mode() & forbidden == 0 {
        Ok(())
    } else {
        Err(not_private(path))
    }
}

#[cfg(unix)]
fn euid() -> u32 {
    // SAFETY: geteuid takes no arguments and returns the effective UID.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn not_private(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} is not a private directory owned by this user", path.display()),
    )
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
        let parent = root.path().join("app");
        let dir = parent.join("things");
        std::fs::create_dir_all(&dir).unwrap();
        // Group write on the app folder (umask 002) is cleared; a leaf that
        // only others can read is tightened.
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o775)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        ensure_private_dir(&parent, "things").unwrap();
        assert_eq!(mode(&parent), 0o700);
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn refuses_a_group_writable_leaf() {
        // Others in the group may already have put files in it.
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("things");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert!(ensure_private_dir(root.path(), "things").is_err());
        assert_eq!(mode(&dir), 0o770);
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
    fn a_root_others_can_write_falls_back_to_the_next_root() {
        let base = tempfile::tempdir().unwrap();
        let open = base.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        // A private root below a folder others can write to is no safer.
        let below_open = open.join("runtime");
        std::fs::create_dir(&below_open).unwrap();
        std::fs::set_permissions(&below_open, std::fs::Permissions::from_mode(0o700)).unwrap();
        let good = base.path().join("cache");

        assert!(private_dir_under(&open, "things").is_err());
        assert!(private_dir_under(&below_open, "things").is_err());
        let roots = [open.clone(), below_open.clone(), good.clone()];
        let dir = user_private_dir_in(&roots, "things").unwrap();
        let good = std::fs::canonicalize(&good).unwrap();
        assert_eq!(dir, good.join(crate::APP_DIR_NAME).join("things"));
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&good.join(crate::APP_DIR_NAME)), 0o700);
    }

    #[test]
    fn a_symlinked_root_resolves_to_its_target() {
        let base = tempfile::tempdir().unwrap();
        let target = base.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let dir = private_dir_under(&link, "things").unwrap();
        assert_eq!(dir, std::fs::canonicalize(&target).unwrap().join(crate::APP_DIR_NAME).join("things"));
    }

    #[test]
    fn relative_roots_and_no_roots_are_refused() {
        assert!(private_dir_under(Path::new("relative"), "things").is_err());
        assert!(user_private_dir_in(&[], "things").is_err());
    }

    #[test]
    fn production_roots_never_use_the_shared_temp_dir() {
        let temp = std::env::temp_dir();
        for root in production_roots() {
            assert_ne!(root, temp);
            assert!(root.is_absolute(), "{}", root.display());
        }
    }

    #[test]
    fn user_private_dir_is_owner_only() {
        let dir = user_private_dir("private-dir-test").unwrap();
        assert!(dir.ends_with(Path::new(crate::APP_DIR_NAME).join("private-dir-test")));
        assert_eq!(mode(&dir), 0o700);
    }
}
