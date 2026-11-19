//! Prepare a two-tree checkout away from the live files. Only publish the
//! entries Git actually changed, keeping moved files reachable by open handles.

use super::{checkout_config, git_command, git_path, path_from_git_bytes, run_git_with_index};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use tempfile::{NamedTempFile, TempDir, TempPath};

type Entries = BTreeMap<PathBuf, Vec<u8>>;

// Mode and object ID, keyed by the exact Git spelling. Skip-worktree entries
// have no file to install or remove.
fn entries(worktree: &Path, git_dir: &Path, index: &Path) -> Result<Entries, String> {
    let out = git_command()
        .current_dir(worktree)
        .env("GIT_DIR", git_dir)
        .env("GIT_WORK_TREE", worktree)
        .env("GIT_INDEX_FILE", index)
        .args(["ls-files", "--stage", "-t", "-z"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let mut entries = Entries::new();
    for record in out
        .stdout
        .split(|&b| b == 0)
        .filter(|r| !r.is_empty() && !r.starts_with(b"S "))
    {
        let tab = record
            .iter()
            .position(|&b| b == b'\t')
            .ok_or("invalid git ls-files record")?;
        let path = path_from_git_bytes(record[tab + 1..].to_vec())
            .ok_or("cannot represent checkout path")?;
        entries.insert(path, record[2..tab].to_vec());
    }
    Ok(entries)
}

// Git runs filter commands at the worktree root. While checkout output is
// private, relative scripts and repository configuration still live here.
fn configure_checkout_filters(
    checkout: &Path,
    merge: &mut std::process::Command,
) -> Result<(), String> {
    let out = git_command()
        .current_dir(checkout)
        .args([
            "config",
            "--null",
            "--get-regexp",
            r"^filter\..*\.(smudge|process)$",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.code() == Some(1) && out.stderr.is_empty() {
        return Ok(());
    }
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    merge.env("CODEMUX_FILTER_WORKTREE", checkout);
    for (n, setting) in out
        .stdout
        .split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .enumerate()
    {
        let Some(i) = setting.iter().position(|&b| b == b'\n') else {
            continue;
        };
        let env = format!("CODEMUX_CHECKOUT_FILTER_{n}");
        let mut arg = b"--config-env=".to_vec();
        arg.extend_from_slice(&setting[..i]);
        arg.push(b'=');
        arg.extend_from_slice(env.as_bytes());
        let command = &setting[i + 1..];
        let mut wrapped = Vec::new();
        if !command.is_empty() {
            wrapped.extend_from_slice(
                b"cd \"$CODEMUX_FILTER_WORKTREE\" && export GIT_WORK_TREE=\"$PWD\" && ",
            );
            wrapped.extend_from_slice(command);
        }
        // Keep configured command lines out of the checkout process's argv.
        merge
            .arg(path_from_git_bytes(arg).ok_or("cannot represent filter name")?)
            .env(
                env,
                path_from_git_bytes(wrapped).ok_or("cannot represent filter command")?,
            );
    }
    Ok(())
}

// During checkout Git reads tracked attributes from the resulting index and
// falls back to local .gitattributes on disk. Mirror only those local files
// along affected paths, without walking the rest of the user's checkout.
fn copy_local_attributes(
    checkout: &Path,
    tree: &Path,
    before: &Entries,
    old: &str,
    new: &str,
) -> Result<(), String> {
    let out = git_command()
        .current_dir(checkout)
        .args(["diff", "--name-only", "--no-renames", "-z", old, new, "--"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let mut visited = HashSet::new();
    for name in out.stdout.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let path = path_from_git_bytes(name.to_vec()).ok_or("cannot represent checkout path")?;
        for dir in path.ancestors().skip(1) {
            if !visited.insert(dir.to_owned()) {
                break;
            }
            let rel = dir.join(".gitattributes");
            if before.contains_key(&rel) {
                continue;
            }
            if !parents_are_directories(checkout, &rel).map_err(|e| e.to_string())? {
                continue;
            }
            let source = checkout.join(&rel);
            match fs::symlink_metadata(&source) {
                Ok(meta) if meta.is_file() => {
                    fs::create_dir_all(tree.join(dir))
                        .and_then(|()| fs::copy(source, tree.join(rel)))
                        .map_err(|e| format!("could not prepare checkout attributes ({e})"))?;
                }
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(format!("could not inspect {} ({e})", source.display())),
            }
        }
    }
    Ok(())
}

// Fingerprint the actual checkout representation, including smudged bytes.
#[derive(PartialEq, Eq)]
enum FileState {
    Regular([u8; 32], fs::Permissions),
    Symlink(std::ffi::OsString),
}

impl FileState {
    fn read(path: &Path) -> io::Result<Self> {
        let meta = fs::symlink_metadata(path)?;
        if meta.is_symlink() {
            return Ok(Self::Symlink(fs::read_link(path)?.into_os_string()));
        }
        if !meta.is_file() {
            return Err(collision(path));
        }
        let mut options = fs::File::options();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(collision(path));
        }
        let mut hash = Sha256::new();
        io::copy(&mut file, &mut hash)?;
        Ok(Self::Regular(hash.finalize().into(), meta.permissions()))
    }
}

// A missing tracked file is a meaningful state on both sides of validation.
fn file_at(root: &Path, rel: &Path) -> io::Result<Option<FileState>> {
    if !parents_are_directories(root, rel)? {
        return Ok(None);
    }
    match FileState::read(&root.join(rel)) {
        Ok(state) => Ok(Some(state)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

enum Original {
    Gitlink,
    Tracked(Option<FileState>),
}

enum Undo {
    RestoreFile(PathBuf, PathBuf),
    UninstallFile {
        dest: PathBuf,
        prepared: PathBuf,
        state: FileState,
    },
    CreateDir(PathBuf, fs::Permissions),
    RemoveDir(PathBuf),
}

pub(super) struct PreparedCheckout {
    container: TempDir,
    recovery: PathBuf,
    index: TempPath,
    remove: Vec<(PathBuf, Original)>,
    install: Vec<(PathBuf, bool)>,
    undo: Vec<Undo>,
}

fn private_dir(parent: &Path, prefix: &str) -> io::Result<TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(parent)
}

#[cfg(unix)]
fn create_recovery_root(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    fs::DirBuilder::new().mode(0o700).create(path)?;
    // On macOS an inherited allow ACL can override the directory's mode.
    #[cfg(target_os = "macos")]
    if !std::process::Command::new("/bin/chmod")
        .arg("-N")
        .arg(path)
        .status()?
        .success()
    {
        return Err(io::Error::other(
            "could not clear inherited recovery permissions",
        ));
    }
    if path.metadata()?.permissions().mode() & 0o777 != 0o700 {
        return Err(io::Error::other(
            "recovery directory must be accessible only to its owner",
        ));
    }
    Ok(())
}

// Reserve recovery with a real directory rename before the ref transaction.
// Starting on the checkout side keeps its filename semantics.
fn checkout_recovery(checkout: &Path, git_dir: &Path) -> io::Result<(TempDir, PathBuf)> {
    let common = git_path(checkout, "--git-common-dir")
        .ok_or_else(|| io::Error::other("could not locate common Git directory"))?;
    let common = dunce::canonicalize(common)?;
    if common != dunce::canonicalize(git_dir)? && common.starts_with(dunce::canonicalize(checkout)?)
    {
        return Err(io::Error::other(
            "common Git directory is inside the linked checkout",
        ));
    }
    let container = private_dir(&common, "codemux-recovery-")?;
    let recovery = container.path().join("files");
    let local = private_dir(checkout, ".codemux-checkout-")?;
    let source = local.path().join("files");
    #[cfg(unix)]
    create_recovery_root(&source)?;
    #[cfg(windows)]
    clack_private_fs::create_dir(&source)?;
    // Incoming files should inherit the checkout's ordinary ACLs when Git
    // prepares them. Only the enclosing recovery root needs to be private.
    let tree = local.path().join("tree");
    fs::create_dir(&tree)?;
    fs::rename(tree, source.join("tree"))?;
    fs::create_dir(source.join("original"))?;
    fs::rename(source, &recovery)?;
    Ok((container, recovery))
}

impl PreparedCheckout {
    pub(super) fn new(checkout: &Path, lock: &Path, old: &str, new: &str) -> Result<Self, String> {
        // Git may clear skip-worktree when a path is present on disk. The
        // empty preparation tree cannot reproduce that live sparse decision.
        if checkout_config(checkout, "core.sparseCheckout", true)?
            .is_some_and(|v| v.trim_end() == "true")
        {
            return Err(
                "cannot atomically update a base checkout with sparse checkout enabled".into(),
            );
        }
        let git_dir =
            git_path(checkout, "--git-dir").ok_or("could not locate checkout git directory")?;
        let (container, recovery) = checkout_recovery(checkout, &git_dir).map_err(|e| {
            format!("could not reserve a recovery directory on the checkout filesystem ({e})")
        })?;
        let tree = recovery.join("tree");
        // --index-output leaves the original copy available for the live
        // dry run. Keep the result on the index filesystem for atomic rename.
        let index = NamedTempFile::new_in(lock.parent().ok_or("missing index directory")?)
            .map_err(|e| e.to_string())?
            .into_temp_path();
        let before = entries(checkout, &git_dir, lock)?;
        let mut merge = git_command();
        merge
            .current_dir(&tree)
            .env("GIT_DIR", &git_dir)
            .env("GIT_WORK_TREE", &tree)
            .env("GIT_INDEX_FILE", lock);
        configure_checkout_filters(checkout, &mut merge)?;
        copy_local_attributes(checkout, &tree, &before, old, new)?;
        let out = merge
            .args([
                "read-tree",
                "-u",
                "-m",
                "--no-recurse-submodules",
                "--index-output",
            ])
            .arg(&index)
            .args([old, new])
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!(
                "git read-tree failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let after = entries(&tree, &git_dir, &index)?;
        let changed = |from: &Entries, to: &Entries| -> Vec<_> {
            from.iter()
                .filter(|(p, entry)| to.get(*p) != Some(entry))
                .map(|(p, entry)| (p.clone(), entry.starts_with(b"160000 ")))
                .collect()
        };
        let remove = changed(&before, &after);
        let install = changed(&after, &before);
        if remove
            .iter()
            .chain(&install)
            .any(|(p, gitlink)| *gitlink && checkout.join(p).join(".git").exists())
            && checkout_config(checkout, "submodule.recurse", true)?
                .is_some_and(|v| v.trim_end() == "true")
        {
            return Err(
                "cannot atomically update a populated submodule recursively in the base checkout"
                    .into(),
            );
        }
        // Capture the outgoing bytes before the final live dry run. The
        // index lock excludes Git writers, but editors need no such lock.
        let remove = remove
            .into_iter()
            .map(|(rel, gitlink)| {
                let original = if gitlink {
                    Original::Gitlink
                } else {
                    Original::Tracked(file_at(checkout, &rel)?)
                };
                Ok((rel, original))
            })
            .collect::<io::Result<_>>()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            remove,
            install,
            container,
            recovery,
            index,
            undo: Vec::new(),
        })
    }

    pub(super) fn install(&mut self, checkout: &Path, lock: &Path) -> Result<(), String> {
        self.install_files(checkout).map_err(|e| e.to_string())?;
        // Moving a file can change its stat data (notably ctime). Refresh
        // clean entries without staging the user's unrelated dirty files.
        run_git_with_index(
            checkout,
            &self.index,
            &["update-index", "-q", "--ignore-submodules", "--refresh"],
        )?;
        // Validate the actual objects we took out of the checkout, even if
        // an editor wrote through an open handle while installation ran.
        let backups = self.recovery.join("original");
        for (rel, original) in &self.remove {
            if let Original::Tracked(expected) = original {
                let actual = file_at(&backups, rel).map_err(|e| e.to_string())?;
                if actual != *expected {
                    return Err(changed(&checkout.join(rel)).to_string());
                }
            }
        }
        fs::rename(&self.index, lock)
            .map_err(|e| format!("could not prepare the checkout index ({e})"))
    }

    fn install_files(&mut self, checkout: &Path) -> io::Result<()> {
        let needed_dirs: HashSet<_> = self
            .install
            .iter()
            .flat_map(|(rel, _)| rel.ancestors().skip(1).map(|p| checkout.join(p)))
            .collect();
        for (rel, original) in &self.remove {
            let dest = checkout.join(rel);
            // An absent parent (including a symlink in its place) makes
            // this tracked leaf absent. Never walk through that symlink.
            if !parents_are_directories(checkout, rel)? {
                continue;
            }
            let meta = match fs::symlink_metadata(&dest) {
                Ok(meta) => meta,
                Err(e) if e.kind() == ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            if matches!(original, Original::Gitlink) {
                if !meta.is_dir() {
                    return Err(collision(&dest));
                }
                // Git leaves populated submodules in the checkout.
                remove_empty_dir(&dest, &mut self.undo)?;
            } else if meta.is_dir() {
                return Err(collision(&dest));
            } else {
                let backup = self.recovery.join("original").join(rel);
                fs::create_dir_all(backup.parent().unwrap())?;
                fs::rename(&dest, &backup)?;
                self.undo.push(Undo::RestoreFile(backup, dest.clone()));
            }
            for parent in dest.ancestors().skip(1).take_while(|p| *p != checkout) {
                if needed_dirs.contains(parent) || !remove_empty_dir(parent, &mut self.undo)? {
                    break;
                }
            }
        }
        for (rel, gitlink) in &self.install {
            let dest = checkout.join(rel);
            let mut dir = checkout.to_owned();
            for part in rel.parent().unwrap_or(Path::new("")).components() {
                dir.push(part);
                ensure_dir(&dir, &mut self.undo)?;
            }
            if *gitlink {
                ensure_dir(&dest, &mut self.undo)?;
                continue;
            }
            if fs::symlink_metadata(&dest).is_ok_and(|m| m.is_dir()) {
                remove_empty_dir(&dest, &mut self.undo)?;
            }
            // The no-replace operation itself protects ignored and untracked
            // files, including files created after Git's validation.
            let prepared = self.recovery.join("tree").join(rel);
            let state = FileState::read(&prepared)?;
            move_noclobber(prepared.clone(), &dest).map_err(|e| {
                if e.kind() == ErrorKind::AlreadyExists || dest.is_dir() {
                    collision(&dest)
                } else {
                    e
                }
            })?;
            self.undo.push(Undo::UninstallFile {
                dest,
                prepared,
                state,
            });
        }
        Ok(())
    }

    fn retain(&mut self, path: PathBuf) -> String {
        self.container.disable_cleanup(true);
        log::warn!("Checkout files retained for saves through open handles at {path:?}");
        if let Some(absolute) = path.to_str() {
            return absolute.to_owned();
        }
        // Only the common directory can contain non-UTF-8 bytes. All names
        // below it, including tempfile's random suffix, are generated as ASCII.
        let common = self
            .container
            .path()
            .parent()
            .expect("absolute recovery container");
        path.strip_prefix(common)
            .expect("retained path is inside the recovery container")
            .to_str()
            .expect("generated recovery path is ASCII")
            .to_owned()
    }

    pub(super) fn keep_originals(mut self) -> Option<String> {
        if self.undo.iter().any(|u| matches!(u, Undo::RestoreFile(..))) {
            // Installation has consumed the incoming files. Discard any
            // remaining preparation data, such as mirrored .gitattributes.
            let _ = fs::remove_dir_all(self.recovery.join("tree"));
            Some(self.retain(self.recovery.join("original")))
        } else {
            None
        }
    }

    pub(super) fn rollback(&mut self, error: String) -> String {
        let mut failure = None;
        let mut preserve = HashSet::new();
        let mut uninstalled = false;
        while let Some(action) = self.undo.pop() {
            let result = match action {
                Undo::RestoreFile(_, dest) if preserve.contains(&dest) => continue,
                Undo::RestoreFile(backup, dest) => move_noclobber(backup, &dest),
                Undo::UninstallFile {
                    dest,
                    prepared,
                    state,
                } => {
                    let result = uninstall_file(&dest, &prepared, state);
                    if result.is_ok() {
                        uninstalled = true;
                    } else {
                        // Also preserve a deletion if the user removed the
                        // installed file before we could roll it back.
                        preserve.insert(dest);
                    }
                    result
                }
                Undo::CreateDir(dir, mode) => match fs::create_dir(&dir) {
                    Ok(()) => fs::set_permissions(dir, mode),
                    Err(e)
                        if e.kind() == ErrorKind::AlreadyExists
                            && fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) =>
                    {
                        Ok(())
                    }
                    Err(e) => Err(e),
                },
                Undo::RemoveDir(dir) => match fs::remove_dir(dir) {
                    // Another process added a file here. Leave its directory.
                    Err(e) if e.kind() == ErrorKind::DirectoryNotEmpty => Ok(()),
                    result => result,
                },
            };
            if let Err(e) = result {
                failure.get_or_insert(e);
            }
        }
        if let Some(e) = failure {
            let path = self.retain(self.recovery.clone());
            return format!(
                "{error}. Could not restore all working files ({e}); checkout recovery files remain at {}",
                super::describe_recovery_dir(&path)
            );
        }
        if uninstalled {
            // These files were briefly live too. Their handles can outlast
            // a successful rollback just as they can outlast a commit.
            let path = self.retain(self.recovery.join("tree"));
            return format!(
                "{error}. Checkout recovery files remain at {}",
                super::describe_recovery_dir(&path)
            );
        }
        error
    }
}

fn changed(path: &Path) -> io::Error {
    io::Error::other(format!(
        "working tree file '{}' changed during checkout",
        path.display()
    ))
}

// Take the installed object back before comparing it. Checking the live
// path and then unlinking it could discard another save between the two.
fn uninstall_file(dest: &Path, prepared: &Path, expected: FileState) -> io::Result<()> {
    move_noclobber(dest.to_owned(), prepared)?;
    let check = FileState::read(prepared).and_then(|actual| {
        if actual == expected {
            Ok(())
        } else {
            Err(changed(dest))
        }
    });
    if check.is_err() {
        move_noclobber(prepared.to_owned(), dest)?;
    }
    check
}

fn collision(path: &Path) -> io::Error {
    io::Error::new(
        ErrorKind::AlreadyExists,
        format!(
            "Untracked working tree file '{}' would be overwritten by merge.",
            path.display()
        ),
    )
}

fn parents_are_directories(root: &Path, path: &Path) -> io::Result<bool> {
    let mut dir = root.to_owned();
    for part in path.parent().unwrap_or(Path::new("")).components() {
        dir.push(part);
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

fn ensure_dir(dir: &Path, undo: &mut Vec<Undo>) -> io::Result<()> {
    match fs::create_dir(dir) {
        Ok(()) => {
            undo.push(Undo::RemoveDir(dir.to_owned()));
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(dir)?.is_dir() {
                Ok(())
            } else {
                Err(collision(dir))
            }
        }
        Err(e) => Err(e),
    }
}

fn remove_empty_dir(dir: &Path, undo: &mut Vec<Undo>) -> io::Result<bool> {
    let mode = fs::symlink_metadata(dir)?.permissions();
    match fs::remove_dir(dir) {
        Ok(()) => {
            undo.push(Undo::CreateDir(dir.to_owned(), mode));
            Ok(true)
        }
        Err(e) if e.kind() == ErrorKind::DirectoryNotEmpty => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(not(windows))]
fn move_noclobber(source: PathBuf, dest: &Path) -> io::Result<()> {
    let mut source = TempPath::try_from_path(source)?;
    source.disable_cleanup(true);
    source.persist_noclobber(dest).map_err(|e| e.error)
}

#[cfg(windows)]
fn move_noclobber(source: PathBuf, dest: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    // These are real checkout files. TempPath::persist_noclobber on Windows
    // resets their attributes, including read-only and hidden, before moving.
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let source = wide(&source);
    let dest = wide(dest);
    // SAFETY: Both paths are NUL-terminated and live through the call. Zero
    // flags disallow both replacing a destination and copying across volumes.
    if unsafe { MoveFileExW(source.as_ptr(), dest.as_ptr(), 0) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
