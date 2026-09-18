use super::{workspace::Workspace, ErrorCode, ProtocolError, Result};
use serde::Serialize;
use std::{
    path::Path,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, Semaphore},
};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicts: u32,
    pub paths: Vec<String>,
    pub truncated: bool,
}
#[derive(Clone)]
pub struct Git {
    slots: Arc<Semaphore>,
    cache: Arc<Mutex<Option<((String, std::path::PathBuf), Instant, Summary)>>>,
}
impl Default for Git {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(2)),
            cache: Arc::new(Mutex::new(None)),
        }
    }
}
impl Git {
    pub async fn summary(
        &self,
        workspace: &Workspace,
        cancel: &CancellationToken,
    ) -> Result<Summary> {
        let key = (workspace.id.clone(), workspace.root.clone());
        if let Some((id, time, summary)) = &*self.cache.lock().await {
            if id == &key && time.elapsed() < Duration::from_secs(1) && !cancel.is_cancelled() {
                return Ok(summary.clone());
            }
        }
        let slot =
            self.slots.clone().try_acquire_owned().map_err(|_| {
                ProtocolError::new(ErrorCode::ResourceLimit, "Too many Git requests")
            })?;
        // The broker can drop its request future before our cancellation select
        // runs. Keep ownership (and the concurrency permit) in a separate task
        // until the child is explicitly killed and reaped.
        let job_cancel = cancel.child_token();
        let _cancel_on_drop = job_cancel.clone().drop_guard();
        let root = workspace.root.clone();
        let job = tokio::spawn(async move {
            let _slot = slot;
            run(&root, &job_cancel).await
        });
        let bytes = job
            .await
            .map_err(|_| ProtocolError::new(ErrorCode::PluginStopped, "Git operation stopped"))??;
        let summary = parse(&bytes)?;
        *self.cache.lock().await = Some((key, Instant::now(), summary.clone()));
        Ok(summary)
    }
}
// Repository configuration is untrusted: even `status` can execute clean filters.
// Discovery commands only read metadata. The status child receives a private Git
// directory with inert configuration, copied index/refs and a read-only object path.
const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const INDEX_LIMIT: usize = 128 * 1024 * 1024;

fn command(executable: &Path, root: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(executable);
    cmd.env_clear()
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(system) = std::env::var_os("SystemRoot") {
        cmd.env("SystemRoot", system);
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_PAGER", "")
        .env("LC_ALL", "C")
        .args([
            "--no-optional-locks",
            "--no-replace-objects",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "protocol.allow=never",
        ]);
    cmd
}

async fn capture(
    mut cmd: tokio::process::Command,
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
) -> Result<Vec<u8>> {
    let mut child = cmd
        .spawn()
        .map_err(|_| ProtocolError::new(ErrorCode::NotAGitRepo, "Cannot read Git metadata"))?;
    let mut output = child.stdout.take().unwrap().take(OUTPUT_LIMIT as u64 + 1);
    let mut bytes = Vec::new();
    let work = async {
        output
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ProtocolError::invalid("Cannot read Git output"))?;
        if bytes.len() > OUTPUT_LIMIT {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Git output limit",
            ));
        }
        let status = child
            .wait()
            .await
            .map_err(|_| ProtocolError::invalid("Git process failed"))?;
        if !status.success() {
            return Err(ProtocolError::new(
                ErrorCode::NotAGitRepo,
                "Cannot read this Git repository",
            ));
        }
        Ok(())
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => Err(ProtocolError::new(ErrorCode::ContextStale, "Workspace changed")),
        result = tokio::time::timeout_at(deadline, work) => result.unwrap_or_else(|_| Err(ProtocolError::new(ErrorCode::Timeout, "Git summary timed out")))
    };
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result?;
    Ok(bytes)
}

fn snapshot_error(_: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "Cannot prepare private Git metadata",
    )
}

async fn copy_metadata(
    source: &Path,
    target: &Path,
    remaining: &mut usize,
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
) -> Result<()> {
    let work = async {
        let metadata = match tokio::fs::symlink_metadata(source).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(snapshot_error(error)),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(snapshot_error("Git metadata is not a regular file"));
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.read(true);
        // A repository must not strand a blocking filesystem worker by swapping
        // an index/HEAD for a FIFO between metadata inspection and opening it.
        #[cfg(unix)]
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
        let file = options.open(source).await.map_err(snapshot_error)?;
        if !file.metadata().await.map_err(snapshot_error)?.is_file() {
            return Err(snapshot_error("Git metadata is not a regular file"));
        }
        let mut bytes = Vec::new();
        file.take(*remaining as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(snapshot_error)?;
        if bytes.len() > *remaining {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Git metadata snapshot limit",
            ));
        }
        *remaining -= bytes.len();
        tokio::fs::write(target, bytes)
            .await
            .map_err(snapshot_error)
    };
    tokio::select! {
        _ = cancel.cancelled() => Err(ProtocolError::new(ErrorCode::ContextStale, "Workspace changed")),
        result = tokio::time::timeout_at(deadline, work) => result.unwrap_or_else(|_| Err(ProtocolError::new(ErrorCode::Timeout, "Git summary timed out")))
    }
}

fn quote_config(value: &str) -> Result<String> {
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\u{8}'))
    {
        return Err(ProtocolError::invalid(
            "Unsupported Git configuration value",
        ));
    }
    Ok(format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
            .replace('\u{8}', "\\b")
    ))
}

fn safe_config(bytes: &[u8], format: &str) -> Result<String> {
    let mut config = format!("[core]\nrepositoryformatversion = 1\nbare = false\n[extensions]\nobjectformat = {format}\n");
    for record in bytes.split(|b| *b == 0).filter(|v| !v.is_empty()) {
        let record = std::str::from_utf8(record)
            .map_err(|_| ProtocolError::invalid("Unsupported Git configuration encoding"))?;
        let (key, value) = record.split_once('\n').unwrap_or((record, "true"));
        let Some((section, tail)) = key.split_once('.') else {
            continue;
        };
        if section == "core"
            && matches!(
                tail,
                "filemode" | "ignorecase" | "precomposeunicode" | "autocrlf" | "eol"
            )
        {
            config.push_str(&format!("[core]\n{tail} = {}\n", quote_config(value)?));
        } else if matches!(section, "branch" | "remote") {
            let Some((subsection, name)) = tail.rsplit_once('.') else {
                continue;
            };
            if (section == "branch" && matches!(name, "remote" | "merge"))
                || (section == "remote" && name == "fetch")
            {
                config.push_str(&format!(
                    "[{section} {}]\n{name} = {}\n",
                    quote_config(subsection)?,
                    quote_config(value)?
                ));
            }
        }
    }
    Ok(config)
}

async fn run(root: &Path, cancel: &CancellationToken) -> Result<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let executable = which::which("git")
        .map_err(|_| ProtocolError::new(ErrorCode::NotAGitRepo, "Git is unavailable"))?;
    let private = tempfile::tempdir().map_err(snapshot_error)?;
    let mut discover = command(&executable, root);
    discover.args([
        "rev-parse",
        "--path-format=absolute",
        "--absolute-git-dir",
        "--git-path",
        "objects",
        "--git-path",
        "shallow",
        "--show-object-format",
        "--show-toplevel",
    ]);
    let locations = capture(discover, cancel, deadline).await?;
    let locations = std::str::from_utf8(&locations)
        .map_err(|_| ProtocolError::invalid("Unsupported Git path encoding"))?;
    let locations: Vec<_> = locations.lines().collect();
    if locations.len() != 5 || !matches!(locations[3], "sha1" | "sha256") {
        return Err(ProtocolError::invalid(
            "Unsupported Git repository metadata",
        ));
    }
    let git_dir = Path::new(locations[0]);
    // A workspace rooted below a repository must not expand its read authority.
    if tokio::fs::canonicalize(locations[4])
        .await
        .map_err(snapshot_error)?
        != tokio::fs::canonicalize(root)
            .await
            .map_err(snapshot_error)?
    {
        return Err(ProtocolError::new(
            ErrorCode::NotAGitRepo,
            "Workspace is not a Git repository root",
        ));
    }
    let mut config_cmd = command(&executable, root);
    config_cmd.args(["config", "--null", "--list", "--includes"]);
    let config = safe_config(&capture(config_cmd, cancel, deadline).await?, locations[3])?;
    let mut refs_cmd = command(&executable, root);
    refs_cmd.args([
        "for-each-ref",
        "--format=%(objectname) %(refname)",
        "refs/heads/",
        "refs/remotes/",
    ]);
    let refs = capture(refs_cmd, cancel, deadline).await?;
    tokio::fs::create_dir(private.path().join("refs"))
        .await
        .map_err(snapshot_error)?;
    tokio::fs::create_dir(private.path().join("objects"))
        .await
        .map_err(snapshot_error)?;
    tokio::fs::write(private.path().join("config"), config)
        .await
        .map_err(snapshot_error)?;
    // `for-each-ref` supplies validated ref names; packed-refs avoids creating
    // filesystem paths from them (including Windows reserved basenames).
    tokio::fs::write(private.path().join("packed-refs"), refs)
        .await
        .map_err(snapshot_error)?;
    let mut head_limit = 4096;
    copy_metadata(
        &git_dir.join("HEAD"),
        &private.path().join("HEAD"),
        &mut head_limit,
        cancel,
        deadline,
    )
    .await?;
    let mut index_limit = INDEX_LIMIT;
    copy_metadata(
        &git_dir.join("index"),
        &private.path().join("index"),
        &mut index_limit,
        cancel,
        deadline,
    )
    .await?;
    let mut entries = tokio::fs::read_dir(git_dir).await.map_err(snapshot_error)?;
    while let Some(entry) = entries.next_entry().await.map_err(snapshot_error)? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.strip_prefix("sharedindex.").is_some_and(|hash| {
            matches!(hash.len(), 40 | 64) && hash.bytes().all(|b| b.is_ascii_hexdigit())
        }) {
            copy_metadata(
                &entry.path(),
                &private.path().join(name.as_ref()),
                &mut index_limit,
                cancel,
                deadline,
            )
            .await?;
        }
        if cancel.is_cancelled() || tokio::time::Instant::now() >= deadline {
            return Err(ProtocolError::new(
                ErrorCode::ContextStale,
                "Git metadata read interrupted",
            ));
        }
    }
    let mut shallow_limit = OUTPUT_LIMIT;
    copy_metadata(
        Path::new(locations[2]),
        &private.path().join("shallow"),
        &mut shallow_limit,
        cancel,
        deadline,
    )
    .await?;
    let mut status = command(&executable, root);
    status
        .env("GIT_DIR", private.path())
        .env("GIT_WORK_TREE", root)
        .env("GIT_OBJECT_DIRECTORY", locations[1])
        .args([
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ]);
    capture(status, cancel, deadline).await
}
fn parse(bytes: &[u8]) -> Result<Summary> {
    let mut result = Summary::default();
    let mut records = bytes.split(|b| *b == 0);
    while let Some(raw) = records.next() {
        let line = String::from_utf8_lossy(raw);
        let line = line.as_ref();
        if let Some(branch) = line.strip_prefix("# branch.head ") {
            result.branch = if branch == "(detached)" {
                None
            } else {
                Some(branch.into())
            };
            continue;
        }
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut parts = ab.split(' ');
            result.ahead = parts
                .next()
                .unwrap_or("+0")
                .trim_start_matches('+')
                .parse()
                .unwrap_or(0);
            result.behind = parts
                .next()
                .unwrap_or("-0")
                .trim_start_matches('-')
                .parse()
                .unwrap_or(0);
            continue;
        }
        let path = if let Some(path) = line.strip_prefix("? ") {
            result.untracked += 1;
            Some(path)
        } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
            let fields: Vec<_> = line
                .splitn(
                    if line.starts_with("1 ") {
                        9
                    } else if line.starts_with("2 ") {
                        10
                    } else {
                        11
                    },
                    ' ',
                )
                .collect();
            if fields.len() < 3 {
                return Err(ProtocolError::invalid("Malformed Git status"));
            }
            let xy = fields[1].as_bytes();
            if xy.len() != 2 {
                return Err(ProtocolError::invalid("Malformed Git status"));
            }
            if line.starts_with("u ") {
                result.conflicts += 1
            } else {
                if xy[0] != b'.' {
                    result.staged += 1
                }
                if xy[1] != b'.' {
                    result.unstaged += 1
                }
            }
            if line.starts_with("2 ") {
                records.next();
            }
            fields.last().copied()
        } else {
            None
        };
        if let Some(path) = path {
            if result.paths.len() < 500 {
                result.paths.push(path.into())
            } else {
                result.truncated = true
            }
        }
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Git fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn repository(root: &Path) {
        fixture_git(root, &["init", "-b", "main"]);
        std::fs::write(root.join("file"), "original\n").unwrap();
        std::fs::write(root.join(".gitattributes"), "file filter=fixture\n").unwrap();
        fixture_git(root, &["add", "."]);
        fixture_git(root, &["commit", "-m", "fixture"]);
    }
    #[tokio::test]
    async fn repository_programs_are_inert_and_normal_status_is_preserved() {
        let root = tempfile::tempdir().unwrap();
        repository(root.path());
        fixture_git(
            root.path(),
            &[
                "config",
                "filter.fixture.clean",
                "echo executed > filter-executed; cat",
            ],
        );
        std::fs::write(root.path().join("file"), "modified\n").unwrap();
        // Prove the fixture reaches Git's clean-filter execution path.
        fixture_git(root.path(), &["status", "--porcelain=v2"]);
        assert!(root.path().join("filter-executed").exists());
        std::fs::remove_file(root.path().join("filter-executed")).unwrap();
        let summary = parse(&run(root.path(), &CancellationToken::new()).await.unwrap()).unwrap();
        assert_eq!(summary.branch.as_deref(), Some("main"));
        assert_eq!(summary.unstaged, 1);
        assert_eq!(summary.paths, ["file"]);
        assert!(!root.path().join("filter-executed").exists());
        // Long-running process filters are independently disabled as well.
        fixture_git(
            root.path(),
            &[
                "config",
                "filter.fixture.process",
                "echo executed > process-executed; exit 1",
            ],
        );
        fixture_git(
            root.path(),
            &[
                "config",
                "core.fsmonitor",
                "echo executed > fsmonitor-executed",
            ],
        );
        fixture_git(root.path(), &["config", "core.hooksPath", ".hooks"]);
        run(root.path(), &CancellationToken::new()).await.unwrap();
        for name in ["filter-executed", "process-executed", "fsmonitor-executed"] {
            assert!(!root.path().join(name).exists());
        }
    }
    #[tokio::test]
    async fn snapshots_support_tracking_split_index_detached_and_linked_worktrees() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        repository(&repo);
        fixture_git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        fixture_git(
            &repo,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        );
        fixture_git(&repo, &["config", "branch.main.remote", "origin"]);
        fixture_git(&repo, &["config", "branch.main.merge", "refs/heads/main"]);
        std::fs::write(repo.join("staged"), "fixture").unwrap();
        fixture_git(&repo, &["add", "."]);
        fixture_git(&repo, &["commit", "-m", "ahead"]);
        fixture_git(&repo, &["update-index", "--split-index"]);
        std::fs::write(repo.join("file"), "modified\n").unwrap();
        let summary = parse(&run(&repo, &CancellationToken::new()).await.unwrap()).unwrap();
        assert_eq!((summary.ahead, summary.behind, summary.unstaged), (1, 0, 1));
        let linked = root.path().join("linked");
        fixture_git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        std::fs::write(linked.join("untracked"), "fixture").unwrap();
        let summary = parse(&run(&linked, &CancellationToken::new()).await.unwrap()).unwrap();
        assert_eq!(summary.branch, None);
        assert_eq!(summary.paths, ["untracked"]);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn dropped_broker_request_cancels_and_reaps_its_blocked_git_child() {
        let root = tempfile::tempdir().unwrap();
        repository(root.path());
        let config = root.path().join(".git/config");
        std::fs::remove_file(&config).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(config.as_os_str().as_bytes()).unwrap();
        // Block real Git during config discovery without running repository code.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let git = Git::default();
        let clone = git.clone();
        let context = CancellationToken::new();
        let token = context.clone();
        let workspace = Workspace {
            id: "fixture".into(),
            name: "Fixture".into(),
            root_name: "fixture".into(),
            location: "local",
            root: root.path().to_path_buf(),
        };
        let request = tokio::spawn(async move { clone.summary(&workspace, &token).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while git.slots.available_permits() == 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(git.slots.available_permits(), 1);
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(2), async {
            while git.slots.available_permits() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Git job must kill/reap before releasing its permit");
        assert!(
            !context.is_cancelled(),
            "Only the abandoned job is cancelled"
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_snapshot_rejects_fifos_and_symlinks_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("metadata");
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(source.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        for attempt in 0..2 {
            let result = tokio::time::timeout(
                Duration::from_millis(500),
                copy_metadata(
                    &source,
                    &root.path().join("copy"),
                    &mut 4096,
                    &CancellationToken::new(),
                    tokio::time::Instant::now() + Duration::from_secs(1),
                ),
            )
            .await
            .unwrap();
            assert!(result.is_err());
            if attempt == 0 {
                std::fs::remove_file(&source).unwrap();
                std::os::unix::fs::symlink("/dev/null", &source).unwrap();
            }
        }
    }
    #[tokio::test]
    async fn unborn_repository_and_cancelled_context_are_handled() {
        let root = tempfile::tempdir().unwrap();
        fixture_git(root.path(), &["init", "-b", "main"]);
        std::fs::write(root.path().join("new"), "fixture").unwrap();
        let summary = parse(&run(root.path(), &CancellationToken::new()).await.unwrap()).unwrap();
        assert_eq!(summary.branch.as_deref(), Some("main"));
        assert_eq!(summary.untracked, 1);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(run(root.path(), &cancel).await.is_err());
    }
    #[test]
    fn porcelain_paths_and_detached_head_are_preserved() {
        let summary=parse(b"# branch.head (detached)\0# branch.ab +2 -3\01 M. N... 100644 100644 100644 abc def path with spaces\0? new\0").unwrap();
        assert_eq!(summary.branch, None);
        assert_eq!(summary.ahead, 2);
        assert_eq!(summary.behind, 3);
        assert_eq!(summary.staged, 1);
        assert_eq!(summary.paths, vec!["path with spaces", "new"]);
    }
}
