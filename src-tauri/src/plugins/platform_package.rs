//! Content-addressed packages. Staging/review never replaces active bytes or runs install scripts.
use super::platform_manifest::{
    safe_package_path, valid_identifier, PlatformManifest, MANIFEST_FILE,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

const MAX_ENTRIES: usize = 2_000;
const MAX_FILE_BYTES: u64 = 4 * 1_048_576;
const MAX_PACKAGE_BYTES: usize = 32 * 1_048_576;
const MAX_GIT_BYTES: u64 = 128 * 1_048_576;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlatformPackageSource {
    Local {
        directory: String,
    },
    Git {
        url: String,
        #[serde(rename = "ref")]
        git_ref: Option<String>,
        commit: String,
        subdir: Option<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSnapshot {
    pub directory: PathBuf,
    pub digest: String,
    pub manifest: PlatformManifest,
    pub source: PlatformPackageSource,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformPromotion {
    pub current: String,
    pub previous: Option<String>,
}
#[derive(Clone, Debug)]
pub struct PlatformPackageStore {
    root: PathBuf,
}
struct Collected {
    files: BTreeMap<String, Vec<u8>>,
    manifest: PlatformManifest,
    digest: String,
}

fn digest_files(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut hash = Sha256::new();
    for (name, bytes) in files {
        hash.update(format!("{}:{name}:{}:", name.len(), bytes.len()));
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}
fn collected(files: BTreeMap<String, Vec<u8>>) -> Result<Collected, String> {
    let manifest = PlatformManifest::parse(
        files
            .get(MANIFEST_FILE)
            .ok_or("codemux-plugin.json is required")?,
    )?;
    if manifest.api_version == 1 {
        for entry in std::iter::once(&manifest.entry).chain(manifest.views.iter().map(|v| &v.entry))
        {
            if !files.contains_key(entry) {
                return Err(format!("Declared entry {entry} is missing"));
            }
        }
    }
    let digest = digest_files(&files);
    Ok(Collected {
        files,
        manifest,
        digest,
    })
}
fn collect_local(root: &Path) -> Result<Collected, String> {
    if !root.is_absolute()
        || !std::fs::symlink_metadata(root).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
    {
        return Err("Choose an absolute, regular plugin directory".into());
    }
    fn walk(
        root: &Path,
        relative: &str,
        files: &mut BTreeMap<String, Vec<u8>>,
        count: &mut usize,
        total: &mut usize,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 64 {
            return Err("Plugin package is nested too deeply".into());
        }
        let directory = root.join(relative);
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "Plugin paths must be UTF-8")?;
            if name == ".git" {
                continue;
            }
            *count += 1;
            if *count > MAX_ENTRIES {
                return Err("Plugin package has too many entries".into());
            }
            entries.push(name);
        }
        entries.sort();
        for name in entries {
            let name = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            let path = safe_package_path(root, &name)?;
            let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.file_type().is_symlink() {
                return Err("Plugin packages cannot contain symbolic links".into());
            }
            if meta.is_dir() {
                walk(root, &name, files, count, total, depth + 1)?;
            } else if meta.is_file() {
                if meta.len() > MAX_FILE_BYTES || *total + meta.len() as usize > MAX_PACKAGE_BYTES {
                    return Err("Plugin snapshot exceeds its byte limit".into());
                }
                let mut options = std::fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                }
                let file = options.open(&path).map_err(|e| e.to_string())?;
                let opened = file.metadata().map_err(|e| e.to_string())?;
                if !opened.is_file() || opened.len() > MAX_FILE_BYTES {
                    return Err("Plugin source changed during snapshot".into());
                }
                let mut bytes = Vec::new();
                file.take(MAX_FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() as u64 != opened.len() {
                    return Err("Plugin source changed during snapshot".into());
                }
                *total += bytes.len();
                if *total > MAX_PACKAGE_BYTES {
                    return Err("Plugin package exceeds its byte limit".into());
                }
                files.insert(name, bytes);
            } else {
                return Err("Plugin packages contain only regular files/directories".into());
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    walk(root, "", &mut files, &mut 0, &mut 0, 0)?;
    collected(files)
}
fn write_files(snapshot: &Collected, destination: &Path, editable: bool) -> Result<(), String> {
    std::fs::create_dir(destination).map_err(|e| e.to_string())?;
    for (name, bytes) in &snapshot.files {
        let path = safe_package_path(destination, name)?;
        std::fs::create_dir_all(path.parent().ok_or("Invalid snapshot path")?)
            .map_err(|e| e.to_string())?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(if editable { 0o600 } else { 0o400 });
        }
        use std::io::Write;
        options
            .open(&path)
            .map_err(|e| e.to_string())?
            .write_all(bytes)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl PlatformPackageStore {
    /// Construction is side-effect free; callers can keep core startup independent of this store.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn snapshot_directory(&self, digest: &str) -> Result<PathBuf, String> {
        if !valid_digest(digest) {
            return Err("Invalid snapshot digest".into());
        }
        Ok(self.root.join("snapshots").join(digest))
    }
    pub fn data_directory(&self, installation_id: &str) -> Result<PathBuf, String> {
        if !valid_identifier(installation_id) {
            return Err("Invalid installation id".into());
        }
        let path = self.root.join("data").join(installation_id);
        private_dir(&path)?;
        Ok(path)
    }
    pub fn stage_local(&self, directory: &Path) -> Result<PlatformSnapshot, String> {
        let source = PlatformPackageSource::Local {
            directory: directory.to_string_lossy().into(),
        };
        self.stage(collect_local(directory)?, source)
    }
    fn stage(
        &self,
        snapshot: Collected,
        source: PlatformPackageSource,
    ) -> Result<PlatformSnapshot, String> {
        private_dir(&self.root)?;
        private_dir(&self.root.join("snapshots"))?;
        let destination = self.snapshot_directory(&snapshot.digest)?;
        if destination.exists() {
            self.verify(&snapshot.digest)?;
        } else {
            let staging = tempfile::Builder::new()
                .prefix(".stage-")
                .tempdir_in(self.root.join("snapshots"))
                .map_err(|e| e.to_string())?;
            let staged = staging.path().join("package");
            write_files(&snapshot, &staged, false)?;
            match std::fs::rename(&staged, &destination) {
                Ok(()) => (),
                Err(_) if destination.exists() => {
                    self.verify(&snapshot.digest)?;
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(PlatformSnapshot {
            directory: destination,
            digest: snapshot.digest,
            manifest: snapshot.manifest,
            source,
        })
    }
    pub fn verify(&self, digest: &str) -> Result<PlatformManifest, String> {
        let snapshot = collect_local(&self.snapshot_directory(digest)?)?;
        if snapshot.digest != digest {
            return Err("Installed snapshot changed; review and consent again".into());
        }
        Ok(snapshot.manifest)
    }
    pub fn active(&self, installation_id: &str) -> Result<Option<PlatformPromotion>, String> {
        if !valid_identifier(installation_id) {
            return Err("Invalid installation id".into());
        }
        let path = self
            .root
            .join("active")
            .join(format!("{installation_id}.json"));
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
            return Err("Unsafe or oversized plugin activation record retained".into());
        }
        let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
        if bytes.len() > 4096 {
            return Err("Oversized plugin activation record retained".into());
        }
        let promotion: PlatformPromotion = serde_json::from_slice(&bytes)
            .map_err(|_| "Unrecognized plugin activation record retained without modification")?;
        let digest =
            |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !digest(&promotion.current)
            || promotion
                .previous
                .as_deref()
                .is_some_and(|previous| !digest(previous))
        {
            return Err("Invalid plugin activation record retained without modification".into());
        }
        Ok(Some(promotion))
    }

    /// Only the reviewed digest is promoted; an update remains staged until this explicit step.
    /// The service must revoke the old runtime generation when changing its own active record.
    pub fn promote(
        &self,
        installation_id: &str,
        digest: &str,
        consent_digest: &str,
    ) -> Result<PlatformPromotion, String> {
        if !valid_identifier(installation_id) {
            return Err("Invalid installation id".into());
        }
        if digest != consent_digest {
            return Err("Consent must name this exact snapshot digest".into());
        }
        self.verify(digest)?;
        let old = self.active(installation_id)?;
        let promotion = PlatformPromotion {
            current: digest.into(),
            previous: old.and_then(|v| {
                if v.current == digest {
                    v.previous
                } else {
                    Some(v.current)
                }
            }),
        };
        self.write_promotion(installation_id, &promotion)?;
        Ok(promotion)
    }
    fn write_promotion(&self, id: &str, promotion: &PlatformPromotion) -> Result<(), String> {
        use std::io::Write;
        let directory = self.root.join("active");
        private_dir(&directory)?;
        let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(promotion).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(directory.join(format!("{id}.json")))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn rollback(
        &self,
        installation_id: &str,
        consent_digest: &str,
    ) -> Result<PlatformPromotion, String> {
        let active = self
            .active(installation_id)?
            .ok_or("No active plugin snapshot")?;
        let previous = active.previous.ok_or("No previous plugin snapshot")?;
        if previous != consent_digest {
            return Err("Rollback consent must name the exact previous digest".into());
        }
        self.verify(&previous)?;
        let promotion = PlatformPromotion {
            current: previous,
            previous: Some(active.current),
        };
        self.write_promotion(installation_id, &promotion)?;
        Ok(promotion)
    }
    pub async fn prepare_repair(
        &self,
        digest: &str,
        destination: &Path,
    ) -> Result<PathBuf, String> {
        if !destination.is_absolute() || destination.exists() {
            return Err("Repair destination must be a new absolute directory".into());
        }
        let snapshot = collect_local(&self.snapshot_directory(digest)?)?;
        if snapshot.digest != digest {
            return Err("Snapshot changed; no repair baseline accepted".into());
        }
        if let Some(parent) = destination.parent() {
            private_dir(parent)?;
        }
        write_files(&snapshot, destination, true)?;
        // Only our fixed git baseline commands execute. Plugin build/install scripts never do.
        for args in [
            vec!["init", "--quiet", "--template=", "--initial-branch=main"],
            vec!["add", "--all", "--force", "--", "."],
            vec![
                "-c",
                "user.name=Codemux",
                "-c",
                "user.email=plugin-repair@localhost",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "--no-verify",
                "-m",
                "Plugin repair baseline",
            ],
        ] {
            git_run(destination, &args, 16_000)
                .await
                .map_err(|e| format!("Repair files retained, but baseline failed: {e}"))?;
        }
        Ok(destination.into())
    }
    pub async fn stage_git(
        &self,
        url: &str,
        git_ref: Option<&str>,
        subdir: Option<&str>,
    ) -> Result<PlatformSnapshot, String> {
        validate_git_source(url, git_ref)?;
        if let Some(subdir) = subdir {
            safe_package_path(
                Path::new(if cfg!(windows) { "C:/git" } else { "/git" }),
                subdir,
            )?;
        }
        private_dir(&self.root)?;
        let temp = tempfile::Builder::new()
            .prefix(".git-fetch-")
            .tempdir_in(&self.root)
            .map_err(|e| e.to_string())?;
        git_run(
            temp.path(),
            &["init", "--bare", "--quiet", "--template="],
            16_000,
        )
        .await?;
        git_run(
            temp.path(),
            &[
                "fetch",
                "--depth=1",
                "--no-tags",
                "--no-recurse-submodules",
                "--",
                url,
                git_ref.unwrap_or("HEAD"),
            ],
            16_000,
        )
        .await?;
        let commit = String::from_utf8(
            git_run(
                temp.path(),
                &["rev-parse", "--verify", "FETCH_HEAD^{commit}"],
                256,
            )
            .await?,
        )
        .map_err(|_| "Invalid git revision")?
        .trim()
        .to_string();
        if ![40, 64].contains(&commit.len()) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Git source did not resolve to an immutable commit".into());
        }
        let tree = match subdir {
            Some(s) => format!("{commit}:{s}"),
            None => commit.clone(),
        };
        let archive = git_run(
            temp.path(),
            &["archive", "--format=tar", &tree],
            MAX_PACKAGE_BYTES + 4 * 1_048_576,
        )
        .await?;
        let mut files = BTreeMap::new();
        let mut total = 0;
        let mut entries = 0;
        for item in tar::Archive::new(archive.as_slice())
            .entries()
            .map_err(|e| e.to_string())?
        {
            let item = item.map_err(|e| e.to_string())?;
            entries += 1;
            if entries > MAX_ENTRIES {
                return Err("Git snapshot has too many entries".into());
            }
            let path = item.path().map_err(|e| e.to_string())?.into_owned();
            let name = path
                .to_str()
                .ok_or("Git snapshot paths must be UTF-8")?
                .trim_end_matches('/')
                .to_string();
            safe_package_path(temp.path(), &name)?;
            if item.header().entry_type().is_dir() {
                continue;
            }
            if !item.header().entry_type().is_file() {
                return Err("Git snapshot contains a link or special file".into());
            }
            let size = item.size();
            if size > MAX_FILE_BYTES || total + size as usize > MAX_PACKAGE_BYTES {
                return Err("Git snapshot exceeds its byte limit".into());
            }
            let mut bytes = Vec::new();
            item.take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            total += bytes.len();
            if bytes.len() as u64 != size || files.insert(name, bytes).is_some() {
                return Err("Invalid git snapshot".into());
            }
        }
        self.stage(
            collected(files)?,
            PlatformPackageSource::Git {
                url: url.into(),
                git_ref: git_ref.map(Into::into),
                commit,
                subdir: subdir.map(Into::into),
            },
        )
    }
}
pub fn validate_git_source(source: &str, git_ref: Option<&str>) -> Result<(), String> {
    let url = url::Url::parse(source).map_err(|_| "Invalid git source URL")?;
    if source.len() > 4096
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || git_ref.is_some_and(|s| {
            s.is_empty() || s.len() > 256 || s.starts_with('-') || s.contains(['\0', '\n', '\r'])
        })
    {
        return Err("Git source must be credential-free HTTPS, with a bounded revision".into());
    }
    Ok(())
}

async fn git_run(directory: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>, String> {
    git_run_bounded(directory, args, limit, true).await
}
/// Local workflow ref validation needs the same cancellation/deadline/output bounds,
/// without applying the small package-download quota to an existing project checkout.
pub(crate) async fn git_run_bounded(
    directory: &Path,
    args: &[&str],
    limit: usize,
    check_disk_budget: bool,
) -> Result<Vec<u8>, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.current_dir(directory)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in ["PATH", "SystemRoot", "WINDIR", "TMPDIR", "TMP", "TEMP"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "http.followRedirects=false",
            "-c",
            "http.lowSpeedLimit=1024",
            "-c",
            "http.lowSpeedTime=15",
            "-c",
            "pack.threads=1",
        ])
        .args(args);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = OwnedGitChild::new(cmd.spawn().map_err(|e| format!("Cannot start git: {e}"))?)?;
    let mut stdout = child.stdout.take().ok_or("Git stdout is unavailable")?;
    let task = async {
        let mut out = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = stdout.read(&mut buffer).await.map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            if out.len() + read > limit {
                return Err("Git output exceeds snapshot limit".into());
            }
            out.extend_from_slice(&buffer[..read]);
        }
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(
                "Git operation failed (credentials and install scripts are never invoked)".into(),
            );
        }
        Ok(out)
    };
    // A pack download is bounded by a deadline and checked disk budget, independently of stdout.
    let budget = async {
        if !check_disk_budget {
            std::future::pending::<()>().await;
        }
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        loop {
            tick.tick().await;
            let path = directory.to_owned();
            let over = tokio::task::spawn_blocking(move || directory_budget_exceeded(&path))
                .await
                .unwrap_or(true);
            if over {
                break;
            }
        }
    };
    let result = tokio::select! { result=tokio::time::timeout(Duration::from_secs(60),task)=>result.unwrap_or_else(|_|Err("Git operation exceeded its deadline".into())), _=budget=>Err("Git download exceeded its disk budget".into()) };
    child.kill_tree();
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    result
}
struct OwnedGitChild {
    child: tokio::process::Child,
    #[cfg(windows)]
    job: super::platform_runtime::windows_job::OwnedJob,
}
impl std::ops::Deref for OwnedGitChild {
    type Target = tokio::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}
impl std::ops::DerefMut for OwnedGitChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}
impl OwnedGitChild {
    fn new(child: tokio::process::Child) -> Result<Self, String> {
        #[cfg(windows)]
        let job = super::platform_runtime::windows_job::OwnedJob::attach(&child)?;
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
        })
    }
    fn kill_tree(&mut self) {
        // id() disappears after reaping. Never signal a saved PID that could have been reused.
        #[cfg(unix)]
        if let Some(pid) = self.child.id().filter(|p| *p > 1) {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        self.job.terminate();
        let _ = self.child.start_kill();
    }
}
impl Drop for OwnedGitChild {
    fn drop(&mut self) {
        self.kill_tree();
    }
}
fn directory_budget_exceeded(root: &Path) -> bool {
    let mut pending = vec![root.to_owned()];
    let mut size = 0;
    let mut count = 0;
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return true;
        };
        for entry in entries {
            count += 1;
            if count > 10_000 {
                return true;
            }
            let Ok(entry) = entry else {
                return true;
            };
            let Ok(meta) = entry.metadata() else {
                return true;
            };
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                size += meta.len();
                if size > MAX_GIT_BYTES {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path, code: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join(MANIFEST_FILE),r#"{"id":"example.test","name":"Test","version":"1.0.0","apiVersion":1,"entry":"main.mjs"}"#).unwrap();
        std::fs::write(root.join("main.mjs"), code).unwrap();
    }
    #[test]
    fn staging_consent_rollback_preserve_private_data_and_active_version() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fixture(&source, "export function activate(){}");
        let store = PlatformPackageStore::new(tmp.path().join("packages"));
        let one = store.stage_local(&source).unwrap();
        assert!(store.promote("install", &one.digest, "wrong").is_err());
        assert!(store.active("install").unwrap().is_none());
        store.promote("install", &one.digest, &one.digest).unwrap();
        let data = store.data_directory("install").unwrap();
        std::fs::write(data.join("keep"), "private").unwrap();
        fixture(&source, "throw new Error('broken update')");
        let two = store.stage_local(&source).unwrap();
        assert_ne!(one.digest, two.digest);
        assert_eq!(
            store.active("install").unwrap().unwrap().current,
            one.digest
        );
        store.promote("install", &two.digest, &two.digest).unwrap();
        store.rollback("install", &one.digest).unwrap();
        assert_eq!(
            store.active("install").unwrap().unwrap().current,
            one.digest
        );
        assert_eq!(
            std::fs::read_to_string(data.join("keep")).unwrap(),
            "private"
        );
    }
    #[test]
    fn sources_are_bounded_and_not_github_specific() {
        assert!(
            validate_git_source("https://git.example.org/team/plugin.git", Some("main")).is_ok()
        );
        for url in [
            "git@github.com:team/plugin.git",
            "http://example.org/repo",
            "https://user:secret@example.org/repo",
            "file:///tmp/repo",
        ] {
            assert!(validate_git_source(url, None).is_err());
        }
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path(), "code");
        std::fs::write(
            tmp.path().join("huge"),
            vec![0; (MAX_FILE_BYTES + 1) as usize],
        )
        .unwrap();
        assert!(collect_local(tmp.path()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_links_and_detects_modified_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fixture(&source, "code");
        std::os::unix::fs::symlink("main.mjs", source.join("link")).unwrap();
        assert!(collect_local(&source).is_err());
        std::fs::remove_file(source.join("link")).unwrap();
        let store = PlatformPackageStore::new(tmp.path().join("packages"));
        let one = store.stage_local(&source).unwrap();
        std::fs::write(one.directory.join("extra"), "tampered").unwrap();
        assert!(store.verify(&one.digest).is_err());
    }
    #[tokio::test]
    async fn repair_baseline_is_editable_and_ignores_plugin_install_scripts() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fixture(&source, "code");
        std::fs::write(
            source.join("package.json"),
            r#"{"scripts":{"install":"exit 99"}}"#,
        )
        .unwrap();
        let store = PlatformPackageStore::new(tmp.path().join("packages"));
        let one = store.stage_local(&source).unwrap();
        let repair = tmp.path().join("repair");
        store.prepare_repair(&one.digest, &repair).await.unwrap();
        std::fs::write(repair.join("main.mjs"), "edited").unwrap();
        assert!(store.verify(&one.digest).is_ok());
        assert!(repair.join(".git/HEAD").is_file());
    }
    #[test]
    fn future_activation_records_are_retained_before_promotion() {
        let root = tempfile::tempdir().unwrap();
        let store = PlatformPackageStore::new(root.path().join("packages"));
        std::fs::create_dir_all(root.path().join("packages/active")).unwrap();
        let path = root.path().join("packages/active/example.json");
        let future =
            serde_json::json!({"current":"a".repeat(64),"previous":null,"schemaVersion":99})
                .to_string();
        std::fs::write(&path, &future).unwrap();
        assert!(store.active("example").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), future);
    }
}
