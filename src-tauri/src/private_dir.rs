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
    let mut error = io::Error::new(
        io::ErrorKind::NotFound,
        "no per-user directory is available",
    );
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
    [dirs::runtime_dir(), dirs::cache_dir()]
        .into_iter()
        .flatten()
        .collect()
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
/// On Unix the part of the root that already exists is resolved first, and
/// it and each of its ancestors must be a directory owned by root or this
/// user that others cannot write to (a sticky directory such as `/tmp` is
/// fine, since others cannot rename what this user puts there). Otherwise
/// someone else could swap the folders below it after they pass their
/// checks. Only then are the missing folders created, one level at a time
/// and without following symlinks, each checked before the next.
fn private_dir_under(root: &Path, name: &str) -> io::Result<PathBuf> {
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not an absolute path", root.display()),
        ));
    }
    #[cfg(unix)]
    let root = verified_root(root)?;
    ensure_private_dir(&root.join(crate::APP_DIR_NAME), name)
}

/// Resolve and check the existing part of `root`, then create the rest.
#[cfg(unix)]
fn verified_root(root: &Path) -> io::Result<PathBuf> {
    use std::path::Component;
    let mut existing = root;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let (Some(parent), Some(Component::Normal(last))) =
                    (existing.parent(), existing.components().next_back())
                else {
                    return Err(error);
                };
                missing.push(last.to_owned());
                existing = parent;
            }
            Err(error) => return Err(error),
        }
    }
    let mut dir = std::fs::canonicalize(existing)?;
    for ancestor in dir.ancestors() {
        check_trusted_ancestor(ancestor)?;
    }
    for component in missing.into_iter().rev() {
        dir.push(component);
        create_dir_once(&dir)?;
        check_owned(&dir, 0o022)?;
    }
    Ok(dir)
}

/// `mkdir` one level, owner-only. An existing entry is left for the caller
/// to check; `mkdir` never follows a symlink in the last component.
#[cfg(unix)]
fn create_dir_once(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
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
/// Each level is created on its own and checked before anything is created
/// inside it; the caller vouches for the folder that holds `parent`.
pub(crate) fn ensure_private_dir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let dir = parent.join(name);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        create_dir_once(parent)?;
        // Tighten the parent before creating inside it, so nobody can swap
        // the child after its check.
        check_owned(parent, 0o002)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        create_dir_once(&dir)?;
        check_owned(&dir, 0o022)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir)?;
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
        format!(
            "{} is not a private directory owned by this user",
            path.display()
        ),
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

/// POSIX shell steps for creating or checking a private directory below
/// `$HOME` on the machine that will use it. Walk from `/` so a symlink or a
/// writable ancestor cannot redirect even the first `mkdir`. Sticky,
/// root/user-owned ancestors (such as `/tmp`) are safe to descend through.
/// Callers append their file operation to this script while `cmx_dir` names
/// the verified leaf. `parts` are internal, literal directory names.
pub(crate) fn shell_private_dir_guard(parts: &[&str], create: bool) -> String {
    let mut script = String::from(
        r#"umask 077
cmx_check_ancestor() {
  [ -d "$1" ] && [ ! -L "$1" ] || return 1
  [ -O "$1" ] || [ "$(find "$1" -prune -uid 0 -print)" = "$1" ] || return 1
  cmx_bad=$(find "$1" -prune \( -perm -020 -o -perm -002 \) ! -perm -1000 -print) || return 1
  [ -z "$cmx_bad" ]
}
cmx_check_owned() {
  [ -d "$1" ] && [ ! -L "$1" ] && [ -O "$1" ] || return 1
  cmx_bad=$(find "$1" -prune \( -perm -020 -o -perm -002 \) -print) || return 1
  [ -z "$cmx_bad" ]
}
cmx_check_chain() {
  case "$1" in /*) ;; *) return 1 ;; esac
  cmx_check_dir=/
  cmx_check_ancestor "$cmx_check_dir" || return 1
  cmx_check_rest=${1#/}
  while [ -n "$cmx_check_rest" ]; do
    cmx_check_part=${cmx_check_rest%%/*}
    case "$cmx_check_part" in ''|.|..) return 1 ;; esac
    cmx_check_dir="${cmx_check_dir%/}/$cmx_check_part"
    cmx_check_ancestor "$cmx_check_dir" || return 1
    case "$cmx_check_rest" in */*) cmx_check_rest=${cmx_check_rest#*/} ;; *) cmx_check_rest= ;; esac
  done
}
case "$HOME" in /*) ;; *) exit 1 ;; esac
cmx_source=/
cmx_check_ancestor "$cmx_source" || exit 1
cmx_uid=$(id -u) || exit 1
cmx_resolved=/
cmx_rest=${HOME#/}
while [ -n "$cmx_rest" ]; do
  cmx_part=${cmx_rest%%/*}
  case "$cmx_part" in ''|.|..) exit 1 ;; esac
  cmx_source="${cmx_source%/}/$cmx_part"
  if [ -L "$cmx_source" ]; then
    cmx_link_owner=$(find "$cmx_source" -prune \( -uid "$cmx_uid" -o -uid 0 \) -print) || exit 1
    [ "$cmx_link_owner" = "$cmx_source" ] || exit 1
  fi
  cmx_resolved=$(CDPATH= cd -P "$cmx_source" 2>/dev/null && pwd -P) || exit 1
  cmx_check_chain "$cmx_resolved" || exit 1
  case "$cmx_rest" in */*) cmx_rest=${cmx_rest#*/} ;; *) cmx_rest= ;; esac
done
cmx_dir=$cmx_resolved
cmx_check_owned "$cmx_dir" || exit 1
"#,
    );
    for part in parts {
        debug_assert!(part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')));
        script.push_str(&format!("cmx_dir=\"$cmx_dir/{part}\"\n"));
        if create {
            script.push_str("if [ ! -e \"$cmx_dir\" ] && [ ! -L \"$cmx_dir\" ]; then mkdir -m 700 \"$cmx_dir\" || cmx_check_owned \"$cmx_dir\" || exit 1; fi\n");
        }
        script.push_str("cmx_check_owned \"$cmx_dir\" || exit 1\n");
    }
    if create {
        script.push_str("chmod 700 \"$cmx_dir\" || exit 1\n");
    }
    script
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
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
        assert!(
            !shared.join("inner").exists(),
            "nothing is created under a refused parent"
        );

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
        assert!(
            create_private_file(&path).is_err(),
            "an existing file is never reused"
        );

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
    fn nothing_is_created_below_a_root_others_can_write() {
        let base = tempfile::tempdir().unwrap();
        let open = base.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(private_dir_under(&open.join("runtime/deeper"), "things").is_err());
        assert!(
            !open.join("runtime").exists(),
            "checks run before anything is created"
        );
    }

    #[test]
    fn missing_root_folders_are_created_owner_only() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("cache/nested");
        let dir = private_dir_under(&root, "things").unwrap();
        let base = std::fs::canonicalize(base.path()).unwrap();
        assert_eq!(
            dir,
            base.join("cache/nested")
                .join(crate::APP_DIR_NAME)
                .join("things")
        );
        assert_eq!(mode(&base.join("cache")), 0o700);
        assert_eq!(mode(&base.join("cache/nested")), 0o700);
    }

    #[test]
    fn a_symlink_planted_for_a_missing_root_folder_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        // The root's last folder already exists as a symlink: it resolves,
        // so the target is checked like any root. A symlink further down is
        // refused because the app folder must be a real directory.
        std::os::unix::fs::symlink(elsewhere.path(), base.path().join(crate::APP_DIR_NAME))
            .unwrap();
        assert!(private_dir_under(base.path(), "things").is_err());
        assert!(!elsewhere.path().join("things").exists());
    }

    #[test]
    fn a_symlinked_root_resolves_to_its_target() {
        let base = tempfile::tempdir().unwrap();
        let target = base.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let dir = private_dir_under(&link, "things").unwrap();
        assert_eq!(
            dir,
            std::fs::canonicalize(&target)
                .unwrap()
                .join(crate::APP_DIR_NAME)
                .join("things")
        );
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

    #[test]
    fn shell_guard_accepts_a_safe_directory_created_by_a_concurrent_process() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        // Simulate losing the mkdir race: the directory now exists, but the
        // mkdir command reports failure. The guard must verify that winner.
        let script = format!(
            "mkdir() {{ command mkdir \"$@\"; return 1; }}\n{}true\n",
            shell_private_dir_guard(&["agent-launch"], true)
        );
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env("HOME", &home_path)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(mode(&home_path.join("agent-launch")), 0o700);
    }

    #[test]
    fn shell_guard_uses_a_verified_target_for_a_linked_home() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let real_home = root.join("real-home");
        std::fs::create_dir(&real_home).unwrap();
        std::fs::set_permissions(&real_home, std::fs::Permissions::from_mode(0o700)).unwrap();
        let linked_home = root.join("linked-home");
        std::os::unix::fs::symlink(&real_home, &linked_home).unwrap();
        let script = format!(
            "{}printf data > \"$cmx_dir/probe\"\n",
            shell_private_dir_guard(&["agent-launch"], true)
        );
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env("HOME", &linked_home)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(real_home.join("agent-launch/probe")).unwrap(),
            "data"
        );
        assert_eq!(mode(&real_home.join("agent-launch")), 0o700);
    }

    #[test]
    fn shell_guard_rejects_a_linked_home_with_a_writable_target() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let unsafe_home = root.join("unsafe-home");
        std::fs::create_dir(&unsafe_home).unwrap();
        std::fs::set_permissions(&unsafe_home, std::fs::Permissions::from_mode(0o777)).unwrap();
        let linked_home = root.join("linked-home");
        std::os::unix::fs::symlink(&unsafe_home, &linked_home).unwrap();
        let script = format!(
            "{}printf data > \"$cmx_dir/probe\"\n",
            shell_private_dir_guard(&["agent-launch"], true)
        );
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("HOME", &linked_home)
            .status()
            .unwrap();
        assert!(!status.success());
        assert!(!unsafe_home.join("agent-launch").exists());
    }
}
