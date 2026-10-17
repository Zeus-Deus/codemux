//! Installing plugins from GitHub at a pinned commit.
//!
//! Nothing from the repository runs: no checkout, no hooks, no scripts, no
//! npm. The installer makes a private bare repository, fetches exactly one
//! commit (shallow and blobless, over https only), lists the plugin folder's
//! tree and reads its regular files' blobs. Those files go to
//! `<root>/.staging-<id>.<n>`, the install marker is written, and the staging
//! folder is renamed over `<root>/<id>`. A crash in between leaves only
//! dot-folders, which [`recover`] tidies on the next listing.

use super::{InstallMarker, ENTRY_FILE, MARKER_FILE, MAX_SOURCE_BYTES};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Files copied from the plugin folder (non-recursive), and their total size.
pub const MAX_FILES: usize = 64;
pub const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(90);

// ── URLs ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoUrl {
    /// `https://github.com/<owner>/<repo>`, normalised.
    pub repo: String,
    pub owner: String,
    pub name: String,
    /// Whatever followed `/tree/` in the URL (ref and maybe a folder).
    pub tree: Option<String>,
}

fn segment_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Accepts `https://github.com/<owner>/<repo>` (optionally `.git`, a trailing
/// slash, or `/tree/<ref>[/<folder>]`). Nothing else: no other host, no
/// credentials, no ports, no query.
pub fn parse_repo(input: &str) -> Result<RepoUrl, String> {
    let bad = || "Enter a GitHub repository URL like https://github.com/owner/repo".to_string();
    let url = url::Url::parse(input.trim()).map_err(|_| bad())?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(bad());
    }
    let parts: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    if parts.len() < 2 {
        return Err(bad());
    }
    let owner = parts[0];
    let name = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if !segment_ok(owner) || !segment_ok(name) {
        return Err(bad());
    }
    let tree = match parts.get(2) {
        None => None,
        Some(&"tree") if parts.len() > 3 => Some(parts[3..].join("/")),
        Some(_) => return Err(bad()),
    };
    if let Some(tree) = &tree {
        check_subdir(tree)?;
    }
    Ok(RepoUrl {
        repo: format!("https://github.com/{owner}/{name}"),
        owner: owner.into(),
        name: name.into(),
        tree,
    })
}

/// A folder inside the repository: relative, forward slashes, no `.`/`..`.
pub fn check_subdir(subdir: &str) -> Result<(), String> {
    let ok = !subdir.is_empty()
        && subdir.len() <= 400
        && !subdir.starts_with('/')
        && !subdir.contains('\\')
        && subdir
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != ".." && !s.chars().any(char::is_control));
    if ok {
        Ok(())
    } else {
        Err(format!("\"{subdir}\" is not a folder inside the repository"))
    }
}

fn is_sha(value: &str) -> bool {
    super::catalog::is_sha(value)
}

// ── git ───────────────────────────────────────────────────────────────

/// A private bare repository driven with a locked-down git: no system or
/// global config, no prompts, no credential helpers, https only.
pub struct Git {
    exe: PathBuf,
    dir: tempfile::TempDir,
    /// Tests fetch from a local bare repository over `file://`.
    allow_file: bool,
}

impl Git {
    pub fn new() -> Result<Self, String> {
        let exe = which::which("git").map_err(|_| "Git is required to install plugins".to_string())?;
        Self::with(exe, false)
    }

    pub(crate) fn with(exe: PathBuf, allow_file: bool) -> Result<Self, String> {
        let dir = tempfile::Builder::new()
            .prefix("codemux-plugin-fetch-")
            .tempdir()
            .map_err(|e| format!("Cannot create a temporary folder: {e}"))?;
        let git = Self { exe, dir, allow_file };
        git.run(&["init", "--bare", "-q", "--template="], None, 4096)?;
        Ok(git)
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.env_clear();
        for key in [
            "PATH", "SystemRoot", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY",
            "all_proxy", "NO_PROXY", "no_proxy", "SSL_CERT_FILE", "SSL_CERT_DIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        cmd.env("GIT_DIR", self.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("LC_ALL", "C")
            .current_dir(self.dir.path())
            .args(["-c", "protocol.allow=never"])
            .args([
                "-c",
                if self.allow_file { "protocol.file.allow=always" } else { "protocol.https.allow=always" },
            ])
            .args(["-c", "credential.helper=", "-c", "core.askPass=", "-c", "fetch.fsckObjects=true"]);
        cmd
    }

    /// Run git with an overall deadline and an output cap.
    fn run(&self, args: &[&str], stdin: Option<&[u8]>, max_out: u64) -> Result<Vec<u8>, String> {
        let mut child = self
            .command()
            .args(args)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Cannot run git: {e}"))?;
        if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let input = input.to_vec();
            std::thread::spawn(move || {
                let _ = pipe.write_all(&input);
            });
        }
        let mut out_pipe = child.stdout.take().unwrap();
        let mut err_pipe = child.stderr.take().unwrap();
        let out = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = (&mut out_pipe).take(max_out + 1).read_to_end(&mut buf);
            buf
        });
        let err = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = (&mut err_pipe).take(16 * 1024).read_to_end(&mut buf);
            buf
        });
        let deadline = Instant::now() + GIT_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Git took too long; check your connection and try again".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let out = out.join().unwrap_or_default();
        let err = String::from_utf8_lossy(&err.join().unwrap_or_default()).to_string();
        if out.len() as u64 > max_out {
            let _ = child.kill();
            return Err("The plugin is larger than the install limit".into());
        }
        if !status.success() {
            let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("git failed").trim();
            if line.contains("could not read Username")
                || line.contains("Repository not found")
                || line.contains("not found")
            {
                return Err("Repository not found, or it isn't public".into());
            }
            return Err(format!("Git: {line}"));
        }
        Ok(out)
    }

    fn set_origin(&self, url: &str) -> Result<(), String> {
        for args in [
            ["config", "core.repositoryformatversion", "1"].as_slice(),
            &["config", "extensions.partialClone", "origin"],
            &["config", "remote.origin.url", url],
            &["config", "remote.origin.promisor", "true"],
            &["config", "remote.origin.partialclonefilter", "blob:none"],
        ] {
            self.run(args, None, 4096)?;
        }
        Ok(())
    }

    /// Ref names on the remote, for resolving `/tree/<ref>/<folder>`.
    fn remote_refs(&self, url: &str) -> Result<Vec<String>, String> {
        let out = self.run(&["ls-remote", "--heads", "--tags", url], None, 8 * 1024 * 1024)?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|l| l.split('\t').nth(1))
            .filter_map(|r| r.strip_prefix("refs/heads/").or_else(|| r.strip_prefix("refs/tags/")))
            .map(|r| r.trim_end_matches("^{}").to_string())
            .collect())
    }

    /// Fetch one commit (by sha, branch, tag or `HEAD`) and return its sha.
    fn fetch(&self, want: &str) -> Result<String, String> {
        let refspec = format!("+{want}:refs/codemux/want");
        self.run(
            &[
                "fetch", "-q", "--no-tags", "--no-write-fetch-head", "--depth", "1", "--filter=blob:none",
                "origin", &refspec,
            ],
            None,
            1024 * 1024,
        )?;
        let out = self.run(&["rev-parse", "--verify", "refs/codemux/want^{commit}"], None, 4096)?;
        let sha = String::from_utf8_lossy(&out).trim().to_string();
        if !is_sha(&sha) {
            return Err("Git returned an unexpected commit id".into());
        }
        if is_sha(want) && sha != want {
            return Err("The fetched commit doesn't match the requested one".into());
        }
        Ok(sha)
    }

    /// `(mode, oid, name)` for the entries of one folder (non-recursive).
    fn list_folder(&self, sha: &str, folder: Option<&str>) -> Result<Vec<(String, String, String)>, String> {
        let spec = match folder {
            Some(f) => format!("{sha}:{f}"),
            None => sha.to_string(),
        };
        let out = self.run(&["ls-tree", "-z", &spec], None, 4 * 1024 * 1024)?;
        Ok(out
            .split(|b| *b == 0)
            .filter(|e| !e.is_empty())
            .filter_map(|entry| {
                let entry = String::from_utf8_lossy(entry);
                let (meta, name) = entry.split_once('\t')?;
                let mut meta = meta.split(' ');
                let mode = meta.next()?.to_string();
                let _kind = meta.next()?;
                let oid = meta.next()?.to_string();
                Some((mode, oid, name.to_string()))
            })
            .collect())
    }

    fn read_blobs(&self, oids: &[String]) -> Result<Vec<Vec<u8>>, String> {
        let input = oids.join("\n") + "\n";
        let out = self.run(&["cat-file", "--batch"], Some(input.as_bytes()), MAX_TOTAL_BYTES + 64 * 1024)?;
        let mut blobs = Vec::new();
        let mut rest = out.as_slice();
        for oid in oids {
            let nl = rest.iter().position(|b| *b == b'\n').ok_or("Git returned a short blob list")?;
            let header = String::from_utf8_lossy(&rest[..nl]).to_string();
            let mut parts = header.split(' ');
            if parts.next() != Some(oid.as_str()) || parts.next() != Some("blob") {
                return Err(format!("Git could not read {oid}"));
            }
            let size: usize = parts.next().and_then(|s| s.parse().ok()).ok_or("Bad blob header")?;
            rest = &rest[nl + 1..];
            if rest.len() < size + 1 {
                return Err("Git returned a truncated blob".into());
            }
            blobs.push(rest[..size].to_vec());
            rest = &rest[size + 1..];
        }
        Ok(blobs)
    }
}

// ── fetching a plugin ─────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScannedMeta {
    pub id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
}

/// Best-effort, static read of the default export's `id`, `name`,
/// `description` and `version` string literals. Nothing is evaluated. The
/// loader still checks `plugin.id` equals the folder name.
pub fn scan_meta(source: &str) -> ScannedMeta {
    let start = ["export default", "definePlugin("]
        .iter()
        .filter_map(|m| source.rfind(m))
        .max()
        .unwrap_or(0);
    let field = |name: &str, max: usize| -> Option<String> {
        let re = regex::Regex::new(&format!(r#"\b{name}\s*:\s*["']([^"'\n\\]{{1,{max}}})["']"#)).ok()?;
        re.captures(&source[start..])
            .or_else(|| re.captures(source))
            .map(|c| c[1].trim().to_string())
    };
    ScannedMeta {
        id: field("id", 64).filter(|id| super::valid_id(id)),
        name: field("name", 80),
        description: field("description", 240),
        version: field("version", 40),
    }
}

pub struct Fetched {
    pub sha: String,
    /// The branch or tag the commit was resolved from (for updates); `None`
    /// for a pinned sha or the default branch.
    pub git_ref: Option<String>,
    /// Folder inside the repository that holds `plugin.js`; `None` = root.
    pub subdir: Option<String>,
    pub files: Vec<(String, Vec<u8>)>,
    pub meta: ScannedMeta,
}

/// Resolve `url` (+ optional ref/folder) to one commit and read the plugin
/// folder's regular files. Symlinks, submodules, sub-folders and any
/// `.codemux-plugin.json` in the repository are skipped.
pub fn fetch_plugin(
    git: &Git,
    fetch_url: &str,
    tree: Option<&str>,
    want: Option<&str>,
    subdir: Option<&str>,
) -> Result<Fetched, String> {
    git.set_origin(fetch_url)?;
    // `/tree/<ref>/<folder>`: the ref may itself contain slashes, so pick the
    // longest remote ref that prefixes it (a leading sha also works).
    let (want, subdir): (String, Option<String>) = match (tree, want) {
        (Some(tree), None) => {
            let first = tree.split('/').next().unwrap_or("");
            if is_sha(first) {
                let rest = tree[first.len()..].trim_start_matches('/');
                (first.to_string(), (!rest.is_empty()).then(|| rest.to_string()))
            } else {
                let refs = git.remote_refs(fetch_url)?;
                let best = refs
                    .iter()
                    .filter(|r| tree == r.as_str() || tree.starts_with(&format!("{r}/")))
                    .max_by_key(|r| r.len())
                    .ok_or_else(|| format!("No branch or tag matches \"{tree}\""))?;
                let rest = tree[best.len()..].trim_start_matches('/');
                (best.clone(), (!rest.is_empty()).then(|| rest.to_string()))
            }
        }
        (_, Some(want)) => (want.to_string(), subdir.map(str::to_string)),
        (None, None) => ("HEAD".to_string(), subdir.map(str::to_string)),
    };
    if let Some(s) = &subdir {
        check_subdir(s)?;
    }
    let sha = git.fetch(&want)?;
    let git_ref = (!is_sha(&want) && want != "HEAD").then(|| want.clone());

    // Which folder holds plugin.js: the given one, else root, else codemux/.
    let candidates: Vec<Option<String>> = match &subdir {
        Some(s) => vec![Some(s.clone())],
        None => vec![None, Some("codemux".into())],
    };
    let mut chosen = None;
    for folder in candidates {
        let entries = match git.list_folder(&sha, folder.as_deref()) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        if entries.iter().any(|(mode, _, name)| name == ENTRY_FILE && mode.starts_with("100")) {
            chosen = Some((folder, entries));
            break;
        }
    }
    let (folder, entries) = chosen.ok_or_else(|| match &subdir {
        Some(s) => format!("No {ENTRY_FILE} in \"{s}\" at that commit"),
        None => format!("No {ENTRY_FILE} at the repository root or in codemux/"),
    })?;

    // Regular files only (100644 / 100755), never links (120000), trees or
    // submodules (160000), and never a marker the repository brought along.
    let files: Vec<(String, String)> = entries
        .into_iter()
        .filter(|(mode, _, name)| (mode == "100644" || mode == "100755") && name != MARKER_FILE)
        .filter(|(_, _, name)| !name.contains('/') && !name.contains('\\') && name != "." && name != "..")
        .map(|(_, oid, name)| (name, oid))
        .collect();
    if files.len() > MAX_FILES {
        return Err(format!("The plugin folder has more than {MAX_FILES} files"));
    }
    let blobs = git.read_blobs(&files.iter().map(|(_, oid)| oid.clone()).collect::<Vec<_>>())?;
    let total: u64 = blobs.iter().map(|b| b.len() as u64).sum();
    if total > MAX_TOTAL_BYTES {
        return Err("The plugin is larger than the install limit".into());
    }
    let files: Vec<(String, Vec<u8>)> = files.into_iter().map(|(n, _)| n).zip(blobs).collect();
    let entry = files
        .iter()
        .find(|(n, _)| n == ENTRY_FILE)
        .ok_or_else(|| format!("No {ENTRY_FILE} found"))?;
    if entry.1.len() as u64 > MAX_SOURCE_BYTES {
        return Err(format!("{ENTRY_FILE} is larger than {MAX_SOURCE_BYTES} bytes"));
    }
    let meta = scan_meta(std::str::from_utf8(&entry.1).map_err(|_| format!("{ENTRY_FILE} is not UTF-8"))?);
    Ok(Fetched { sha, git_ref, subdir: folder, files, meta })
}

// ── writing ───────────────────────────────────────────────────────────

fn unique_suffix() -> String {
    format!(
        "{}{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

/// What already sits at `<root>/<id>`.
#[derive(Debug, PartialEq, Eq)]
pub enum Existing {
    None,
    Installed(InstallMarker),
    Local,
    Link,
}

pub fn existing(root: &Path, id: &str) -> Result<Existing, String> {
    let dir = super::paths::plugin_dir(root, id).map_err(|e| e.to_string())?;
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Existing::None),
        Err(e) => Err(e.to_string()),
        Ok(m) if m.file_type().is_symlink() => Ok(Existing::Link),
        Ok(m) if !m.is_dir() => Err(format!("\"{id}\" exists and is not a folder")),
        Ok(_) => Ok(match super::read_marker(&dir)? {
            Some(marker) => Existing::Installed(marker),
            None => Existing::Local,
        }),
    }
}

/// Write `files` + marker into a staging folder and swap it over
/// `<root>/<id>`. Refuses to replace a Local folder or a linked folder:
/// those are the user's own code.
pub fn write_install(
    root: &Path,
    id: &str,
    files: &[(String, Vec<u8>)],
    marker: &InstallMarker,
) -> Result<(), String> {
    match existing(root, id)? {
        Existing::Local => {
            return Err(format!(
                "A Local plugin folder named \"{id}\" already exists. Remove it first."
            ))
        }
        Existing::Link => {
            return Err(format!("\"{id}\" is a linked folder. Remove the link first."))
        }
        Existing::None | Existing::Installed(_) => {}
    }
    let dest = super::paths::plugin_dir(root, id).map_err(|e| e.to_string())?;
    let suffix = unique_suffix();
    let staging = root.join(format!(".staging-{id}.{suffix}"));
    std::fs::create_dir(&staging).map_err(|e| format!("Cannot create the install folder: {e}"))?;
    let write = || -> Result<(), String> {
        for (name, bytes) in files {
            if name.contains('/') || name.contains('\\') || name == MARKER_FILE || name.starts_with("..") {
                continue;
            }
            std::fs::write(staging.join(name), bytes).map_err(|e| format!("Cannot write {name}: {e}"))?;
        }
        let json = serde_json::to_vec_pretty(marker).map_err(|e| e.to_string())?;
        std::fs::write(staging.join(MARKER_FILE), json).map_err(|e| format!("Cannot write the marker: {e}"))
    };
    if let Err(error) = write() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    if dest.exists() {
        let backup = root.join(format!(".backup-{id}.{suffix}"));
        std::fs::rename(&dest, &backup).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging);
            format!("Cannot replace the installed copy: {e}")
        })?;
        if let Err(e) = std::fs::rename(&staging, &dest) {
            let _ = std::fs::rename(&backup, &dest);
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("Cannot finish the install: {e}"));
        }
        let _ = std::fs::remove_dir_all(&backup);
    } else if let Err(e) = std::fs::rename(&staging, &dest) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("Cannot finish the install: {e}"));
    }
    Ok(())
}

/// Tidy what an interrupted install left: drop staging folders, and put a
/// backup back when its plugin folder is missing (else drop the backup).
pub fn recover(root: &Path) {
    let Ok(read) = std::fs::read_dir(root) else { return };
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        // Never follow a link while tidying.
        let is_dir = std::fs::symlink_metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        if name.starts_with(".staging-") {
            let _ = std::fs::remove_dir_all(&path);
        } else if let Some(rest) = name.strip_prefix(".backup-") {
            let id = rest.split('.').next().unwrap_or("");
            if !super::valid_id(id) {
                continue;
            }
            let dest = root.join(id);
            if std::fs::symlink_metadata(&dest).is_err() {
                let _ = std::fs::rename(&path, &dest);
            } else {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

/// The folder id for a git install: the plugin's own id when the source
/// names one, else the repository name made into a valid id.
pub fn folder_id(meta: &ScannedMeta, repo_name: &str) -> Result<String, String> {
    if let Some(id) = &meta.id {
        return Ok(id.clone());
    }
    let mut id: String = repo_name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    id = id.trim_matches('-').chars().take(64).collect();
    if super::valid_id(&id) {
        Ok(id)
    } else {
        Err("Couldn't work out the plugin's id".into())
    }
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
