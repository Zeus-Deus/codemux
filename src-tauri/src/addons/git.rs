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
        let _slot = self
            .slots
            .try_acquire()
            .map_err(|_| ProtocolError::new(ErrorCode::ResourceLimit, "Too many Git requests"))?;
        let bytes = run(&workspace.root, cancel).await?;
        let summary = parse(&bytes)?;
        *self.cache.lock().await = Some((key, Instant::now(), summary.clone()));
        Ok(summary)
    }
}
async fn run(root: &Path, cancel: &CancellationToken) -> Result<Vec<u8>> {
    let executable = which::which("git")
        .map_err(|_| ProtocolError::new(ErrorCode::NotAGitRepo, "Git is unavailable"))?;
    let empty = tempfile::tempdir().map_err(|_| {
        ProtocolError::new(
            ErrorCode::StorageUnavailable,
            "Cannot prepare Git operation",
        )
    })?;
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
        .env("GIT_PAGER", "")
        .env("LC_ALL", "C");
    cmd.args([
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
    ])
    .arg(format!("core.hooksPath={}", empty.path().display()))
    .args([
        "status",
        "--porcelain=v2",
        "--branch",
        "-z",
        "--untracked-files=normal",
        "--ignore-submodules=all",
    ]);
    let mut child = cmd
        .spawn()
        .map_err(|_| ProtocolError::new(ErrorCode::NotAGitRepo, "Cannot read Git status"))?;
    let mut output = child.stdout.take().unwrap().take(2 * 1024 * 1024 + 1);
    let mut bytes = Vec::new();
    let work = async {
        output
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ProtocolError::invalid("Cannot read Git output"))?;
        if bytes.len() > 2 * 1024 * 1024 {
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
                "This workspace is not a Git repository",
            ));
        }
        Ok(())
    };
    let result = tokio::select! {_ = cancel.cancelled()=>Err(ProtocolError::new(ErrorCode::ContextStale,"Workspace changed")),result=tokio::time::timeout(Duration::from_secs(5),work)=>result.unwrap_or_else(|_|Err(ProtocolError::new(ErrorCode::Timeout,"Git summary timed out")))};
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result?;
    Ok(bytes)
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
