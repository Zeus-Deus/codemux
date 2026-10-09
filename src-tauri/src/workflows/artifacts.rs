//! Pinned filesystem checkouts and explicit, hash-verified integration.
//!
//! The user's index, refs, worktrees, and dirty files are never used as a
//! staging area. Workers receive copies of one run baseline, including its
//! nonignored untracked files. A stopped writer must be sealed before its
//! changes can be applied to the source workspace.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::Mutex,
};

const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_CHECKOUT: u64 = 256 * 1024 * 1024;
pub const MAX_RETAINED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_RETAINED_NODES: usize = 1_000_000;

#[derive(Clone, Copy, Default)]
struct RetainedUsage {
    bytes: u64,
    nodes: usize,
}
#[derive(Default)]
struct QuotaState {
    retained: Option<RetainedUsage>,
    checkouts: BTreeMap<PathBuf, RetainedUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactManifest {
    pub attempt_id: String,
    pub digest: String,
    pub files: BTreeMap<String, FileStamp>,
    pub changed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceArtifact {
    pub run_id: String,
    pub attempt_id: String,
    pub kind: String,
    pub source: PathBuf,
    pub checkout: PathBuf,
    pub baseline_digest: String,
    pub scope: Vec<String>,
    pub sealed: Option<ArtifactManifest>,
    pub integrated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationReport {
    pub attempt_id: String,
    pub digest: String,
    pub changed_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPreview {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub binary: bool,
    pub truncated: bool,
    pub before_sha256: Option<String>,
    pub after_sha256: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Baseline {
    source: PathBuf,
    digest: String,
    files: BTreeMap<String, FileStamp>,
}

pub struct ArtifactStore {
    root: PathBuf,
    mutation: Mutex<()>,
    quota: Mutex<QuotaState>,
    retained_limit: u64,
}

fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 160
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("invalid artifact identity".into());
    }
    Ok(())
}

pub fn relative_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path.components().any(|c| c.as_os_str() == ".git")
    {
        return Err("path must stay inside the attempt checkout and outside .git".into());
    }
    Ok(path.to_path_buf())
}

pub fn in_scope(path: &str, scope: &[String]) -> bool {
    scope.iter().any(|prefix| {
        (prefix == "." && relative_path(path).is_ok())
            || path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with('/'))
    })
}

/// Reject symlink ancestors as well as the leaf. Host file tools never
/// resolve a worker-supplied path outside the copied checkout.
pub fn checked_path(root: &Path, value: &str) -> Result<PathBuf, String> {
    let relative = relative_path(value)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("symlinks are unavailable to managed file tools".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(current)
}

fn stamp(path: &Path) -> Result<Option<FileStamp>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("unsupported file type: {}", path.display()));
    }
    if metadata.len() > MAX_FILE {
        return Err("file exceeds workflow snapshot size limit".into());
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    Ok(Some(FileStamp {
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        size: bytes.len() as u64,
        executable,
    }))
}

fn digest(files: &BTreeMap<String, FileStamp>) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(files).map_err(|e| e.to_string())?)
    ))
}

fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn scan(root: &Path) -> Result<BTreeMap<String, FileStamp>, String> {
    let mut paths = vec![root.to_path_buf()];
    let mut files = BTreeMap::new();
    let mut bytes = 0;
    while let Some(directory) = paths.pop() {
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if metadata.file_type().is_symlink() {
                return Err("workflow snapshots reject symlinks".into());
            }
            if metadata.is_dir() {
                paths.push(path);
                continue;
            }
            let key = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("non-UTF8 workflow path")?
                .replace('\\', "/");
            let file = stamp(&path)?.ok_or("snapshot changed during scan")?;
            bytes += file.size;
            if bytes > MAX_CHECKOUT || files.len() >= 50_000 {
                return Err("checkout exceeds workflow snapshot limits".into());
            }
            files.insert(key, file);
        }
    }
    Ok(files)
}

fn copy_files(
    source: &Path,
    target: &Path,
    files: &BTreeMap<String, FileStamp>,
) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for (name, expected) in files {
        let from = checked_path(source, name)?;
        if stamp(&from)?.as_ref() != Some(expected) {
            return Err(format!("source drifted while copying {name}"));
        }
        let to = checked_path(target, name)?;
        fs::create_dir_all(to.parent().ok_or("missing parent")?).map_err(|e| e.to_string())?;
        fs::copy(&from, &to).map_err(|e| e.to_string())?;
        if stamp(&to)?.as_ref() != Some(expected) {
            return Err(format!("copied content mismatch: {name}"));
        }
    }
    Ok(())
}

fn metadata_usage(root: &Path) -> Result<RetainedUsage, String> {
    if !root.exists() {
        return Ok(RetainedUsage::default());
    }
    let mut usage = RetainedUsage::default();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            usage.nodes = usage.nodes.saturating_add(1);
            if usage.nodes > MAX_RETAINED_NODES {
                return Err("retained artifact node limit reached".into());
            }
            if metadata.file_type().is_symlink() {
                return Err("retained artifact store contains a symlink".into());
            }
            if metadata.is_dir() {
                directories.push(path);
            } else if metadata.is_file() {
                usage.bytes = usage.bytes.saturating_add(metadata.len());
            } else {
                return Err("retained artifact store contains an unsupported file type".into());
            }
        }
    }
    Ok(usage)
}

fn snapshot_usage(files: &BTreeMap<String, FileStamp>) -> RetainedUsage {
    let mut directories = BTreeSet::new();
    for name in files.keys() {
        let mut parent = Path::new(name).parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            directories.insert(path.to_path_buf());
            parent = path.parent();
        }
    }
    RetainedUsage {
        bytes: files.values().map(|file| file.size).sum(),
        nodes: files.len() + directories.len(),
    }
}

fn missing_parents(root: &Path, path: &Path) -> Result<usize, String> {
    let mut missing = 0;
    let mut parent = path.parent();
    while let Some(path) = parent {
        if path == root {
            break;
        }
        if !path.starts_with(root) {
            return Err("artifact path escaped its retained root".into());
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err("artifact parent is not a regular directory".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing += 1,
            Err(error) => return Err(error.to_string()),
        }
        parent = path.parent();
    }
    Ok(missing)
}

fn validate_manifest(
    artifact: &WorkspaceArtifact,
    manifest: &ArtifactManifest,
    baseline: &Baseline,
) -> Result<(), String> {
    let paths: BTreeSet<_> = manifest
        .files
        .keys()
        .chain(baseline.files.keys())
        .cloned()
        .collect();
    let changes: Vec<_> = paths
        .into_iter()
        .filter(|path| manifest.files.get(path) != baseline.files.get(path))
        .collect();
    if manifest.attempt_id != artifact.attempt_id
        || digest(&manifest.files)? != manifest.digest
        || digest(&baseline.files)? != baseline.digest
        || artifact.baseline_digest != baseline.digest
        || artifact.source != baseline.source
        || manifest.changed_paths != changes
        || changes.iter().any(|path| !in_scope(path, &artifact.scope))
    {
        return Err("artifact manifest identity, baseline, changes, or scope mismatch".into());
    }
    Ok(())
}

impl ArtifactStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            mutation: Mutex::new(()),
            quota: Mutex::new(QuotaState::default()),
            retained_limit: MAX_RETAINED_BYTES,
        }
    }

    // Only host-owned mutations adjust these cached counters. Failed writes
    // invalidate the cache so a bounded metadata census accounts for any
    // partial files before the next operation; ordinary calls do no full scan.
    fn quota_mutation<T>(
        &self,
        checkout: Option<&Path>,
        bytes: i64,
        nodes: i64,
        temporary_bytes: u64,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let mut quota = self
            .quota
            .lock()
            .map_err(|_| "artifact quota unavailable")?;
        let retained = match quota.retained {
            Some(value) => value,
            None => metadata_usage(&self.root)?,
        };
        quota.retained = Some(retained);
        let grow = bytes.max(0) as u64;
        let new_nodes = nodes.max(0) as usize;
        if retained
            .bytes
            .saturating_add(grow)
            .saturating_add(temporary_bytes)
            > self.retained_limit
            || retained.nodes.saturating_add(new_nodes) > MAX_RETAINED_NODES
        {
            return Err("retained workflow artifacts exceed the 4 GiB or node quota".into());
        }
        if let Some(checkout) = checkout {
            let value = if let Some(value) = quota.checkouts.get(checkout) {
                *value
            } else {
                metadata_usage(checkout)?
            };
            if value.bytes.saturating_add(grow) > MAX_CHECKOUT
                || value.nodes.saturating_add(new_nodes) > 100_000
            {
                return Err("attempt checkout exceeds workflow snapshot quota".into());
            }
            quota.checkouts.insert(checkout.into(), value);
        }
        match operation() {
            Ok(value) => {
                fn update(value: &mut RetainedUsage, bytes: i64, nodes: i64) {
                    value.bytes = value.bytes.saturating_add_signed(bytes);
                    value.nodes = value.nodes.saturating_add_signed(nodes as isize);
                }
                update(quota.retained.as_mut().unwrap(), bytes, nodes);
                if let Some(checkout) = checkout {
                    update(quota.checkouts.get_mut(checkout).unwrap(), bytes, nodes);
                }
                Ok(value)
            }
            Err(error) => {
                quota.retained = None;
                quota.checkouts.clear();
                Err(error)
            }
        }
    }

    fn save_quota<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), String> {
        let size = serde_json::to_vec_pretty(value)
            .map_err(|e| e.to_string())?
            .len() as u64;
        let old = fs::metadata(path).ok().map(|m| m.len()).unwrap_or(0);
        let nodes = missing_parents(&self.root, path)? + usize::from(!path.exists());
        self.quota_mutation(
            None,
            size as i64 - old as i64,
            nodes as i64,
            size.min(old),
            || save(path, value),
        )
    }
    fn attempt(&self, id: &str) -> Result<PathBuf, String> {
        valid_id(id)?;
        Ok(self.root.join("attempts").join(id))
    }
    fn baseline(&self, id: &str) -> Result<PathBuf, String> {
        valid_id(id)?;
        Ok(self.root.join("runs").join(id))
    }

    pub fn pin_run(&self, run_id: &str, source: &Path) -> Result<String, String> {
        let _lock = self
            .mutation
            .lock()
            .map_err(|_| "artifact store unavailable")?;
        let source = source.canonicalize().map_err(|e| e.to_string())?;
        self.pin_unlocked(run_id, &source)
            .map(|baseline| baseline.digest)
    }

    fn pin_unlocked(&self, run_id: &str, source: &Path) -> Result<Baseline, String> {
        let pinned = self.baseline(run_id)?;
        let baseline_path = pinned.join("baseline.json");
        let baseline: Baseline = if baseline_path.exists() {
            let value: Baseline = read(&baseline_path)?;
            if value.source != source {
                return Err("run workspace binding changed".into());
            }
            value
        } else {
            // Git's own ignore rules determine untracked inclusion. No
            // recursive scan of ignored runtime directories or personal data.
            let output = Command::new("git")
                .arg("-C")
                .arg(&source)
                .args([
                    "ls-files",
                    "-z",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                ])
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err("managed workflow workspaces must be Git repositories".into());
            }
            let mut files = BTreeMap::new();
            let mut bytes = 0;
            for raw in output.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
                let name = std::str::from_utf8(raw)
                    .map_err(|_| "non-UTF8 workflow path")?
                    .to_owned();
                let path = checked_path(&source, &name)?;
                if let Some(file) = stamp(&path)? {
                    bytes += file.size;
                    if bytes > MAX_CHECKOUT || files.len() >= 50_000 {
                        return Err("checkout exceeds workflow snapshot limits".into());
                    }
                    files.insert(name, file);
                }
            }
            let value = Baseline {
                source: source.to_path_buf(),
                digest: digest(&files)?,
                files,
            };
            let usage = snapshot_usage(&value.files);
            let checkout = pinned.join("checkout");
            let partial = metadata_usage(&checkout)?;
            let bytes = usage.bytes
                + serde_json::to_vec_pretty(&value)
                    .map_err(|e| e.to_string())?
                    .len() as u64;
            let nodes = usage.nodes
                + usize::from(!checkout.exists())
                + 1
                + missing_parents(&self.root, &checkout)?;
            self.quota_mutation(
                None,
                bytes as i64 - partial.bytes as i64,
                nodes as i64 - partial.nodes as i64,
                0,
                || {
                    copy_files(&source, &checkout, &value.files)?;
                    save(&baseline_path, &value)
                },
            )?;
            value
        };
        Ok(baseline)
    }

    pub fn prepare(
        &self,
        run_id: &str,
        source: &Path,
        attempt_id: &str,
        scope: Vec<String>,
    ) -> Result<WorkspaceArtifact, String> {
        self.prepare_with_seed(run_id, source, attempt_id, scope, None)
    }

    pub fn prepare_with_seed(
        &self,
        run_id: &str,
        source: &Path,
        attempt_id: &str,
        scope: Vec<String>,
        seed: Option<&str>,
    ) -> Result<WorkspaceArtifact, String> {
        let _lock = self
            .mutation
            .lock()
            .map_err(|_| "artifact store unavailable")?;
        for item in &scope {
            if item != "." {
                relative_path(item)?;
            }
        }
        let source = source.canonicalize().map_err(|e| e.to_string())?;
        let baseline = self.pin_unlocked(run_id, &source)?;
        let pinned = self.baseline(run_id)?;
        let attempt = self.attempt(attempt_id)?;
        if attempt.exists() {
            return Err(
                "attempt checkout already exists; do not dispatch duplicate writers".into(),
            );
        }
        let (input, files) = if let Some(seed) = seed {
            let previous = self.inspect(seed)?;
            let manifest = previous
                .sealed
                .ok_or("continuation artifact is not sealed")?;
            if previous.run_id != run_id
                || previous.baseline_digest != baseline.digest
                || manifest.attempt_id != previous.attempt_id
                || digest(&manifest.files)? != manifest.digest
                || manifest
                    .changed_paths
                    .iter()
                    .any(|path| !in_scope(path, &scope))
                || scan(&previous.checkout)? != manifest.files
            {
                return Err("continuation artifact identity, scope, or content mismatch".into());
            }
            (previous.checkout, manifest.files)
        } else {
            (pinned.join("checkout"), baseline.files.clone())
        };
        let artifact = WorkspaceArtifact {
            run_id: run_id.into(),
            attempt_id: attempt_id.into(),
            kind: "snapshot".into(),
            source,
            checkout: attempt.join("checkout"),
            baseline_digest: baseline.digest,
            scope,
            sealed: None,
            integrated: false,
        };
        let usage = snapshot_usage(&files);
        if usage.bytes > MAX_CHECKOUT || usage.nodes > 100_000 {
            return Err("attempt checkout exceeds workflow snapshot quota".into());
        }
        let bytes = usage.bytes
            + serde_json::to_vec_pretty(&artifact)
                .map_err(|e| e.to_string())?
                .len() as u64;
        let nodes = usage.nodes + 2 + missing_parents(&self.root, &artifact.checkout)?;
        self.quota_mutation(None, bytes as i64, nodes as i64, 0, || {
            copy_files(&input, &artifact.checkout, &files)?;
            save(&attempt.join("artifact.json"), &artifact)
        })?;
        self.quota
            .lock()
            .map_err(|_| "artifact quota unavailable")?
            .checkouts
            .insert(artifact.checkout.clone(), usage);
        Ok(artifact)
    }

    pub fn inspect(&self, attempt_id: &str) -> Result<WorkspaceArtifact, String> {
        read(&self.attempt(attempt_id)?.join("artifact.json"))
    }

    pub fn write_file(&self, attempt_id: &str, path: &str, content: &[u8]) -> Result<(), String> {
        let _lock = self
            .mutation
            .lock()
            .map_err(|_| "artifact store unavailable")?;
        let artifact = self.inspect(attempt_id)?;
        self.put_file_unlocked(&artifact, path, Some(content), None)
    }

    pub fn delete_file(&self, attempt_id: &str, path: &str) -> Result<(), String> {
        let _lock = self
            .mutation
            .lock()
            .map_err(|_| "artifact store unavailable")?;
        let artifact = self.inspect(attempt_id)?;
        self.put_file_unlocked(&artifact, path, None, None)
    }

    fn put_file_unlocked(
        &self,
        artifact: &WorkspaceArtifact,
        name: &str,
        content: Option<&[u8]>,
        permissions: Option<fs::Permissions>,
    ) -> Result<(), String> {
        if artifact.sealed.is_some() || !in_scope(name, &artifact.scope) {
            return Err("artifact writes require an unsealed declared task scope".into());
        }
        let path = checked_path(&artifact.checkout, name)?;
        let old = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => Some(metadata.len()),
            Ok(_) => return Err("managed file target is not a regular file".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        let size = content.map(|bytes| bytes.len() as u64).unwrap_or(0);
        if size > MAX_FILE {
            return Err("file exceeds workflow snapshot size limit".into());
        }
        if content.is_none() && old.is_none() {
            return Err("managed file does not exist".into());
        }
        let nodes = if content.is_some() {
            missing_parents(&self.root, &path)? as i64 + i64::from(old.is_none())
        } else {
            -1
        };
        self.quota_mutation(
            Some(&artifact.checkout),
            size as i64 - old.unwrap_or(0) as i64,
            nodes,
            0,
            || {
                if let Some(content) = content {
                    fs::create_dir_all(path.parent().ok_or("missing parent")?)
                        .map_err(|e| e.to_string())?;
                    fs::write(&path, content).map_err(|e| e.to_string())?;
                    if let Some(permissions) = permissions {
                        fs::set_permissions(&path, permissions).map_err(|e| e.to_string())?;
                    }
                } else {
                    fs::remove_file(&path).map_err(|e| e.to_string())?;
                }
                Ok(())
            },
        )
    }

    pub fn preview(
        &self,
        attempt_id: &str,
        expected_digest: &str,
        path: &str,
    ) -> Result<ArtifactPreview, String> {
        use std::io::Read;
        fn text(
            path: &Path,
            expected: Option<&FileStamp>,
        ) -> Result<(Option<String>, bool, bool), String> {
            let Some(expected) = expected else {
                return Ok((None, false, false));
            };
            if stamp(path)?.as_ref() != Some(expected) {
                return Err("preview content does not match its accepted hash".into());
            }
            let mut bytes = Vec::new();
            fs::File::open(path)
                .map_err(|e| e.to_string())?
                .take(65_536)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            let truncated = expected.size > bytes.len() as u64;
            if bytes.contains(&0) {
                return Ok((None, true, truncated));
            }
            match std::str::from_utf8(&bytes) {
                Ok(value) => Ok((Some(value.to_owned()), false, truncated)),
                Err(error) if truncated && error.error_len().is_none() => Ok((
                    Some(
                        std::str::from_utf8(&bytes[..error.valid_up_to()])
                            .map_err(|e| e.to_string())?
                            .to_owned(),
                    ),
                    false,
                    true,
                )),
                Err(_) => Ok((None, true, truncated)),
            }
        }
        let _lock = self
            .mutation
            .lock()
            .map_err(|_| "artifact store unavailable")?;
        let artifact = self.inspect(attempt_id)?;
        let manifest = artifact
            .sealed
            .as_ref()
            .ok_or("artifact is not sealed after verified stop")?;
        if manifest.digest != expected_digest
            || manifest.attempt_id != artifact.attempt_id
            || digest(&manifest.files)? != manifest.digest
            || !manifest.changed_paths.iter().any(|changed| changed == path)
        {
            return Err(
                "preview requires an accepted changed path and exact artifact digest".into(),
            );
        }
        let baseline_dir = self.baseline(&artifact.run_id)?;
        let baseline: Baseline = read(&baseline_dir.join("baseline.json"))?;
        validate_manifest(&artifact, &manifest, &baseline)?;
        let before_path = checked_path(&baseline_dir.join("checkout"), path)?;
        let after_path = checked_path(&artifact.checkout, path)?;
        let before_stamp = baseline.files.get(path);
        let after_stamp = manifest.files.get(path);
        let (before, before_binary, before_truncated) = text(&before_path, before_stamp)?;
        let (after, after_binary, after_truncated) = text(&after_path, after_stamp)?;
        Ok(ArtifactPreview {
            path: path.into(),
            before,
            after,
            binary: before_binary || after_binary,
            truncated: before_truncated || after_truncated,
            before_sha256: before_stamp.map(|stamp| stamp.sha256.clone()),
            after_sha256: after_stamp.map(|stamp| stamp.sha256.clone()),
        })
    }

    /// Callers establish dependency ownership and generation before import.
    pub fn import_dependency(
        &self,
        target_id: &str,
        dependency_id: &str,
    ) -> Result<Vec<String>, String> {
        let _lock = self
            .mutation
            .lock()
            .map_err(|_| "artifact store unavailable")?;
        let target = self.inspect(target_id)?;
        let dependency = self.inspect(dependency_id)?;
        if target.sealed.is_some()
            || target.run_id != dependency.run_id
            || target.baseline_digest != dependency.baseline_digest
        {
            return Err("dependency checkout does not belong to this writable run baseline".into());
        }
        let manifest = dependency
            .sealed
            .as_ref()
            .ok_or("dependency artifact is unsealed")?;
        if scan(&dependency.checkout)? != manifest.files {
            return Err("dependency artifact changed after acceptance".into());
        }
        let baseline: Baseline = read(&self.baseline(&target.run_id)?.join("baseline.json"))?;
        validate_manifest(&dependency, manifest, &baseline)?;
        for name in &manifest.changed_paths {
            if !in_scope(name, &target.scope) {
                return Err("dependency changes exceed this task's write scope".into());
            }
            let current = stamp(&checked_path(&target.checkout, name)?)?;
            if current != baseline.files.get(name).cloned()
                && current != manifest.files.get(name).cloned()
            {
                return Err(format!("dependency patch conflicts at {name}"));
            }
        }
        for name in &manifest.changed_paths {
            if let Some(expected) = manifest.files.get(name) {
                let origin = checked_path(&dependency.checkout, name)?;
                if stamp(&origin)?.as_ref() != Some(expected) {
                    return Err("dependency content changed while copying".into());
                }
                let bytes = fs::read(&origin).map_err(|e| e.to_string())?;
                if format!("{:x}", Sha256::digest(&bytes)) != expected.sha256 {
                    return Err("dependency content changed while reading".into());
                }
                self.put_file_unlocked(
                    &target,
                    name,
                    Some(&bytes),
                    Some(
                        fs::metadata(origin)
                            .map_err(|e| e.to_string())?
                            .permissions(),
                    ),
                )?;
            } else if checked_path(&target.checkout, name)?.exists() {
                self.put_file_unlocked(&target, name, None, None)?;
            }
        }
        Ok(manifest.changed_paths.clone())
    }

    /// Only call after the driver has proved quiescence. Sealing is not IPC.
    pub fn seal(&self, attempt_id: &str) -> Result<ArtifactManifest, String> {
        let _lock = self
            .mutation
            .lock()
            .map_err(|_| "artifact store unavailable")?;
        let mut artifact = self.inspect(attempt_id)?;
        if let Some(manifest) = artifact.sealed {
            return Ok(manifest);
        }
        let baseline: Baseline = read(&self.baseline(&artifact.run_id)?.join("baseline.json"))?;
        let files = scan(&artifact.checkout)?;
        let paths: BTreeSet<_> = files.keys().chain(baseline.files.keys()).cloned().collect();
        let changed_paths: Vec<_> = paths
            .into_iter()
            .filter(|path| files.get(path) != baseline.files.get(path))
            .collect();
        if changed_paths
            .iter()
            .any(|path| !in_scope(path, &artifact.scope))
        {
            return Err("attempt changed files outside its declared write scope".into());
        }
        let manifest = ArtifactManifest {
            attempt_id: attempt_id.into(),
            digest: digest(&files)?,
            files,
            changed_paths,
        };
        artifact.sealed = Some(manifest.clone());
        self.save_quota(&self.attempt(attempt_id)?.join("artifact.json"), &artifact)?;
        Ok(manifest)
    }

    pub fn integrate(
        &self,
        attempt_id: &str,
        expected_digest: &str,
    ) -> Result<IntegrationReport, String> {
        let _lock = self
            .mutation
            .lock()
            .map_err(|_| "artifact store unavailable")?;
        let mut artifact = self.inspect(attempt_id)?;
        let accepted = artifact
            .sealed
            .as_ref()
            .ok_or("artifact is not sealed after verified stop")?
            .clone();
        if accepted.digest != expected_digest {
            return Err("accepted artifact digest mismatch".into());
        }
        let baseline: Baseline = read(&self.baseline(&artifact.run_id)?.join("baseline.json"))?;
        validate_manifest(&artifact, &accepted, &baseline)?;
        if artifact.integrated {
            return Ok(IntegrationReport {
                attempt_id: attempt_id.into(),
                digest: accepted.digest,
                changed_paths: accepted.changed_paths,
            });
        }
        if scan(&artifact.checkout)? != accepted.files {
            return Err("artifact changed after acceptance".into());
        }
        let mut before = Vec::new();
        for path in &accepted.changed_paths {
            if !in_scope(path, &artifact.scope) {
                return Err("artifact write scope mismatch".into());
            }
            let destination = checked_path(&artifact.source, path)?;
            let current = stamp(&destination)?;
            // A previous interrupted apply may have installed this exact
            // postimage already. Retry only its remaining baseline paths.
            if current == accepted.files.get(path).cloned() {
                continue;
            }
            if current != baseline.files.get(path).cloned() {
                return Err(format!(
                    "source conflict at {path}; preserve the user's changes"
                ));
            }
            before.push((
                path.clone(),
                if destination.exists() {
                    Some(fs::read(&destination).map_err(|e| e.to_string())?)
                } else {
                    None
                },
                fs::metadata(&destination).ok().map(|m| m.permissions()),
            ));
        }
        let transaction = self
            .attempt(attempt_id)?
            .join(format!("integration-{}", uuid::Uuid::new_v4()));
        // Keep rollback evidence on disk even if the app exits unexpectedly.
        let journal = serde_json::json!({"attempt_id":attempt_id,"digest":expected_digest,"state":"applying","paths":before.iter().map(|(path,_,_)|path).collect::<Vec<_>>()});
        let journal_size = serde_json::to_vec_pretty(&journal)
            .map_err(|e| e.to_string())?
            .len() as u64;
        let backup_size = before
            .iter()
            .filter_map(|(_, bytes, _)| bytes.as_ref())
            .map(|bytes| bytes.len() as u64)
            .sum::<u64>();
        let backup_nodes = before
            .iter()
            .filter(|(_, bytes, _)| bytes.is_some())
            .count()
            + 2;
        let final_metadata_size = serde_json::to_vec_pretty(&artifact)
            .map_err(|e| e.to_string())?
            .len() as u64;
        self.quota_mutation(
            None,
            (journal_size + backup_size) as i64,
            backup_nodes as i64,
            final_metadata_size.max(journal_size),
            || {
                fs::create_dir_all(&transaction).map_err(|e| e.to_string())?;
                save(&transaction.join("journal.json"), &journal)?;
                for (index, (_, bytes, _)) in before.iter().enumerate() {
                    if let Some(bytes) = bytes {
                        fs::write(transaction.join(format!("before-{index}")), bytes)
                            .map_err(|e| e.to_string())?;
                    }
                }
                Ok(())
            },
        )?;
        let applied = (|| -> Result<(), String> {
            for (path, _, _) in &before {
                let destination = checked_path(&artifact.source, path)?;
                // Recheck immediately before each mutation; other editors do
                // not participate in CodeMux's integration mutex.
                let current = stamp(&destination)?;
                if current == accepted.files.get(path).cloned() {
                    continue;
                }
                if current != baseline.files.get(path).cloned() {
                    return Err(format!("source conflict at {path}"));
                }
                match accepted.files.get(path) {
                    Some(expected) => {
                        let origin = checked_path(&artifact.checkout, path)?;
                        let bytes = fs::read(&origin).map_err(|e| e.to_string())?;
                        if format!("{:x}", Sha256::digest(&bytes)) != expected.sha256 {
                            return Err("artifact content changed during integration".into());
                        }
                        fs::create_dir_all(destination.parent().ok_or("missing parent")?)
                            .map_err(|e| e.to_string())?;
                        let temporary =
                            destination.with_extension(format!("codemux-{}", uuid::Uuid::new_v4()));
                        fs::write(&temporary, &bytes).map_err(|e| e.to_string())?;
                        fs::set_permissions(
                            &temporary,
                            fs::metadata(&origin)
                                .map_err(|e| e.to_string())?
                                .permissions(),
                        )
                        .map_err(|e| e.to_string())?;
                        fs::rename(temporary, &destination).map_err(|e| e.to_string())?;
                    }
                    None => fs::remove_file(&destination).map_err(|e| e.to_string())?,
                }
            }
            Ok(())
        })();
        if let Err(error) = applied {
            // Restore only paths still matching our just-applied content;
            // never overwrite an external editor's intervening update.
            let mut rollback_error = false;
            for (path, bytes, permissions) in before.iter().rev() {
                let destination = checked_path(&artifact.source, path)?;
                if stamp(&destination)? != accepted.files.get(path).cloned() {
                    continue;
                }
                let result = match bytes {
                    Some(bytes) => fs::write(&destination, bytes),
                    None => fs::remove_file(&destination),
                };
                if result.is_err() {
                    rollback_error = true;
                }
                if let Some(permissions) = permissions {
                    if fs::set_permissions(destination, permissions.clone()).is_err() {
                        rollback_error = true;
                    }
                }
            }
            return Err(format!(
                "integration failed: {error}; rollback evidence at {}{}",
                transaction.display(),
                if rollback_error {
                    "; rollback incomplete"
                } else {
                    ""
                }
            ));
        }
        artifact.integrated = true;
        self.save_quota(&self.attempt(attempt_id)?.join("artifact.json"), &artifact)?;
        self.save_quota(
            &transaction.join("journal.json"),
            &serde_json::json!({"attempt_id":attempt_id,"digest":expected_digest,"state":"integrated"}),
        )?;
        Ok(IntegrationReport {
            attempt_id: attempt_id.into(),
            digest: accepted.digest,
            changed_paths: accepted.changed_paths,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success());
        fs::write(dir.path().join("tracked.txt"), "base").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "tracked.txt"])
            .status()
            .unwrap()
            .success());
        dir
    }

    #[test]
    fn workflow_artifacts_continue_waiting_writes_and_import_dependency_changes() {
        let source = workspace();
        let storage = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(storage.path().into());
        store.pin_run("run", source.path()).unwrap();
        let parent = store
            .prepare("run", source.path(), "parent", vec!["tracked.txt".into()])
            .unwrap();
        fs::write(parent.checkout.join("tracked.txt"), "parent progress").unwrap();
        store.seal("parent").unwrap();
        let resumed = store
            .prepare_with_seed(
                "run",
                source.path(),
                "resumed",
                vec!["tracked.txt".into()],
                Some("parent"),
            )
            .unwrap();
        assert_eq!(
            fs::read_to_string(resumed.checkout.join("tracked.txt")).unwrap(),
            "parent progress"
        );
        let child = store
            .prepare("run", source.path(), "child", vec!["child.txt".into()])
            .unwrap();
        fs::write(child.checkout.join("child.txt"), "child patch").unwrap();
        store.seal("child").unwrap();
        assert!(store.import_dependency("resumed", "child").is_err());
        let integrator = store
            .prepare(
                "run",
                source.path(),
                "integrator",
                vec!["tracked.txt".into(), "child.txt".into()],
            )
            .unwrap();
        store.import_dependency("integrator", "parent").unwrap();
        store.import_dependency("integrator", "child").unwrap();
        assert_eq!(
            fs::read_to_string(integrator.checkout.join("tracked.txt")).unwrap(),
            "parent progress"
        );
        assert_eq!(
            fs::read_to_string(integrator.checkout.join("child.txt")).unwrap(),
            "child patch"
        );
    }

    #[test]
    fn workflow_artifacts_preview_is_bounded_and_hash_verified() {
        let source = workspace();
        let storage = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(storage.path().into());
        let artifact = store
            .prepare(
                "run",
                source.path(),
                "preview",
                vec!["tracked.txt".into(), "new.txt".into()],
            )
            .unwrap();
        fs::write(artifact.checkout.join("tracked.txt"), "replacement").unwrap();
        fs::write(artifact.checkout.join("new.txt"), "é".repeat(40_000)).unwrap();
        let accepted = store.seal("preview").unwrap();
        let preview = store
            .preview("preview", &accepted.digest, "tracked.txt")
            .unwrap();
        assert_eq!(preview.before.as_deref(), Some("base"));
        assert_eq!(preview.after.as_deref(), Some("replacement"));
        assert!(!preview.binary);
        assert!(!preview.truncated);
        let long = store
            .preview("preview", &accepted.digest, "new.txt")
            .unwrap();
        assert!(long.truncated);
        assert!(!long.binary);
        assert!(long.after.unwrap().len() <= 65_536);
        assert!(store.preview("preview", "forged", "tracked.txt").is_err());
        assert!(store
            .preview("preview", &accepted.digest, "../outside")
            .is_err());
        fs::write(artifact.checkout.join("tracked.txt"), "tampered").unwrap();
        assert!(store
            .preview("preview", &accepted.digest, "tracked.txt")
            .is_err());
    }

    #[test]
    fn workflow_artifacts_workspace_scope_and_manifest_fences() {
        let source = workspace();
        let storage = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(storage.path().into());
        let artifact = store
            .prepare("run", source.path(), "all", vec![".".into()])
            .unwrap();
        fs::create_dir_all(artifact.checkout.join("src")).unwrap();
        fs::write(artifact.checkout.join("src/new.txt"), "accepted").unwrap();
        let accepted = store.seal("all").unwrap();
        assert!(in_scope("src/new.txt", &[".".into()]));
        assert!(!in_scope("../outside", &[".".into()]));
        assert!(store
            .preview("all", &accepted.digest, "src/new.txt")
            .is_ok());
        let mut forged = store.inspect("all").unwrap();
        forged.sealed.as_mut().unwrap().changed_paths.clear();
        save(
            &store.attempt("all").unwrap().join("artifact.json"),
            &forged,
        )
        .unwrap();
        assert!(store.integrate("all", &accepted.digest).is_err());
        assert!(!source.path().join("src/new.txt").exists());
    }

    #[test]
    fn workflow_artifacts_retry_partial_integration_preserves_postimages() {
        let source = workspace();
        let storage = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(storage.path().into());
        let artifact = store
            .prepare("run", source.path(), "partial", vec![".".into()])
            .unwrap();
        store
            .write_file("partial", "tracked.txt", b"accepted")
            .unwrap();
        store
            .write_file("partial", "new.txt", b"remaining")
            .unwrap();
        let accepted = store.seal("partial").unwrap();
        // Represents a crash after one of the source renames but before the
        // durable integrated flag was committed.
        fs::copy(
            artifact.checkout.join("tracked.txt"),
            source.path().join("tracked.txt"),
        )
        .unwrap();
        store.integrate("partial", &accepted.digest).unwrap();
        assert_eq!(
            fs::read_to_string(source.path().join("tracked.txt")).unwrap(),
            "accepted"
        );
        assert_eq!(
            fs::read_to_string(source.path().join("new.txt")).unwrap(),
            "remaining"
        );
    }

    #[test]
    fn workflow_artifacts_cached_quota_bounds_copies_and_host_writes() {
        let source = workspace();
        fs::write(source.path().join("bulk.bin"), vec![b'x'; 4096]).unwrap();
        let storage = tempfile::tempdir().unwrap();
        let store = ArtifactStore {
            retained_limit: 16_384,
            ..ArtifactStore::new(storage.path().into())
        };
        store.pin_run("run", source.path()).unwrap();
        let artifact = store
            .prepare("run", source.path(), "first", vec![".".into()])
            .unwrap();
        store.write_file("first", "src/new.txt", b"hello").unwrap();
        store.write_file("first", "src/new.txt", b"hi").unwrap();
        store.delete_file("first", "src/new.txt").unwrap();
        let quota = store.quota.lock().unwrap();
        let expected = metadata_usage(storage.path()).unwrap();
        assert_eq!(quota.retained.unwrap().bytes, expected.bytes);
        assert_eq!(quota.retained.unwrap().nodes, expected.nodes);
        drop(quota);
        assert!(store
            .write_file("first", "too-big.txt", &vec![b'y'; 16_384])
            .unwrap_err()
            .contains("quota"));
        assert!(!artifact.checkout.join("too-big.txt").exists());
        let mut rejected = false;
        for n in 0..10 {
            if store
                .prepare("run", source.path(), &format!("copy{n}"), vec![".".into()])
                .is_err()
            {
                rejected = true;
                break;
            }
        }
        assert!(rejected);
        assert!(metadata_usage(storage.path()).unwrap().bytes <= 16_384);
    }
    #[test]
    fn workflow_artifacts_pin_dirty_and_untracked_and_reject_drift() {
        let source = workspace();
        let storage = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(storage.path().into());
        fs::write(source.path().join("tracked.txt"), "dirty").unwrap();
        fs::write(source.path().join("new.txt"), "untracked").unwrap();
        let a = store
            .prepare("run", source.path(), "first", vec!["tracked.txt".into()])
            .unwrap();
        fs::write(source.path().join("new.txt"), "later").unwrap();
        let b = store
            .prepare("run", source.path(), "second", vec!["tracked.txt".into()])
            .unwrap();
        assert_eq!(
            fs::read_to_string(b.checkout.join("new.txt")).unwrap(),
            "untracked"
        );
        fs::write(a.checkout.join("tracked.txt"), "result").unwrap();
        let accepted = store.seal("first").unwrap();
        fs::write(source.path().join("tracked.txt"), "external editor").unwrap();
        assert!(store.integrate("first", &accepted.digest).is_err());
        assert_eq!(
            fs::read_to_string(source.path().join("tracked.txt")).unwrap(),
            "external editor"
        );
    }
    #[test]
    fn workflow_artifacts_require_seal_scope_hash_and_integrate_once() {
        let source = workspace();
        let storage = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(storage.path().into());
        let a = store
            .prepare("run", source.path(), "writer", vec!["tracked.txt".into()])
            .unwrap();
        assert!(store.integrate("writer", "forged").is_err());
        fs::write(a.checkout.join("tracked.txt"), "accepted").unwrap();
        let seal = store.seal("writer").unwrap();
        assert!(store.integrate("writer", "forged").is_err());
        store.integrate("writer", &seal.digest).unwrap();
        store.integrate("writer", &seal.digest).unwrap();
        assert_eq!(
            fs::read_to_string(source.path().join("tracked.txt")).unwrap(),
            "accepted"
        );
        assert!(store
            .prepare("run", source.path(), "writer", vec![])
            .is_err());
    }
    #[cfg(unix)]
    #[test]
    fn workflow_artifacts_deny_traversal_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(checked_path(root.path(), "../escape").is_err());
        assert!(checked_path(root.path(), ".git/config").is_err());
        assert!(checked_path(root.path(), "escape/target").is_err());
    }
}
