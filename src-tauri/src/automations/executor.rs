//! Fire executor — turns a fired automation run into a real agent run.
//!
//! When `scheduler::tick` records a fire, the desktop scheduler task
//! hands the run here. The executor:
//!
//! 1. creates an isolated git worktree off the automation's project,
//!    on a fresh per-fire branch;
//! 2. spawns the chosen agent headlessly with the prompt;
//! 3. records the terminal `succeeded` / `failed` status (and the
//!    workspace path) back on the run row.
//!
//! A webhook delivery takes the same path through `dispatch_webhook`,
//! with the prompt already rendered from the request.
//!
//! The pure pieces — `agent_command`, `automation_branch` — are unit
//! tested; `prepare_worktree` is covered against a real temp repo.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::database::{AutomationRecord, AutomationRunRecord, DatabaseStore};

/// Map an agent name to the headless CLI invocation that runs a single
/// prompt to completion. Pure.
///
/// `--` ends option parsing, so a prompt that starts with `-` (a webhook
/// body of `--help`, say) is read as the prompt and not as a flag.
pub fn agent_command(agent: &str, prompt: &str) -> Result<(String, Vec<String>), String> {
    match agent {
        "claude" => Ok((
            "claude".to_string(),
            vec!["--print".to_string(), "--".to_string(), prompt.to_string()],
        )),
        "codex" => Ok((
            "codex".to_string(),
            vec!["exec".to_string(), "--".to_string(), prompt.to_string()],
        )),
        other => Err(format!(
            "Automation agent '{other}' is not supported (expected 'claude' or 'codex')"
        )),
    }
}

/// Build the per-fire branch name: a slug of the automation name plus a
/// UTC timestamp, so every run lands on its own branch.
pub fn automation_branch(name: &str, at: DateTime<Utc>) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    let slug: String = slug.chars().take(32).collect();
    let slug = if slug.is_empty() {
        "automation".to_string()
    } else {
        slug
    };
    format!("automation-{slug}-{}", at.format("%Y%m%d-%H%M%S"))
}

/// Compute the worktree directory for a fire — `~/.codemux/worktrees/
/// <project>/<branch>`, the same layout push-to-host uses.
fn worktree_destination(project: &Path, branch: &str) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let project_name = project
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());
    Ok(home
        .join(".codemux")
        .join("worktrees")
        .join(project_name)
        .join(branch))
}

/// Create an isolated git worktree at `worktree_dir` on a new `branch`.
///
/// `base` is the ref the branch starts from: `None` → the repo's
/// current `HEAD` (the local case), `Some(ref)` → that ref (a
/// freshly-fetched `origin/<default>` for a host clone). Returns a flat
/// error string on any failure (path missing, not a git repo, branch
/// clash, git absent).
pub fn prepare_worktree(
    project: &Path,
    worktree_dir: &Path,
    branch: &str,
    base: Option<&str>,
) -> Result<(), String> {
    if !project.is_dir() {
        return Err(format!(
            "Project path does not exist: {}",
            project.display()
        ));
    }
    if let Some(parent) = worktree_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create worktree parent directory: {error}"))?;
    }
    let mut cmd = crate::git::git_command();
    cmd.arg("-C")
        .arg(project)
        .args(["worktree", "add"])
        .arg(worktree_dir)
        .arg("-b")
        .arg(branch);
    if let Some(base_ref) = base {
        cmd.arg(base_ref);
    }
    let output = cmd
        .output()
        .map_err(|error| format!("Failed to run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    crate::git::refresh_new_worktree_index(worktree_dir);
    Ok(())
}

// ── Project repo resolution (the GitHub backbone) ──
//
// An automation run needs the project repo *on the machine the run
// executes on*. On the desktop / "This machine" that is `project_path`.
// On a separate host that path does not exist, so the host clones the
// project's git remote.

/// A project repository resolved to a usable path on this machine, plus
/// the ref a new worktree should branch off.
struct ResolvedRepo {
    path: PathBuf,
    /// `None` → branch off the repo's current `HEAD` (the local case).
    /// `Some(ref)` → branch off this ref — a freshly-fetched
    /// `origin/<default>` for a host clone.
    base: Option<String>,
}

/// Host-local directory holding clones of automation projects.
fn automation_repos_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    Ok(home.join(".codemux").join("automation-repos"))
}

/// A filesystem-safe directory name for a clone, from the remote URL's
/// final path segment (`git@github.com:me/whatsapp-rag.git` →
/// `whatsapp-rag`).
fn repo_dir_name(remote_url: &str) -> String {
    let trimmed = remote_url.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let segment = trimmed.rsplit(['/', ':']).next().unwrap_or("project");
    let slug: String = segment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "project".to_string()
    } else {
        slug.to_string()
    }
}

/// Resolve the automation's project to a local git repo on this machine.
///
/// Local case — `project_path` is here and is a git repo: use it, and
/// branch off its current `HEAD`. Host case — clone, or `git fetch` an
/// existing clone of, `project_remote` into `~/.codemux/
/// automation-repos/`, and branch off the freshly-fetched
/// `origin/<default>`.
fn resolve_repo(project_path: Option<&str>, project_remote: Option<&str>) -> Result<ResolvedRepo, String> {
    if let Some(path) = project_path {
        let candidate = Path::new(path);
        if candidate.join(".git").exists() {
            return Ok(ResolvedRepo {
                path: candidate.to_path_buf(),
                base: None,
            });
        }
    }
    let remote = project_remote.ok_or_else(|| {
        "This automation has no project repo on this machine and no git \
         remote to clone from. Pick a project that has a git remote."
            .to_string()
    })?;
    let clone_cwd = std::env::current_dir()
        .map_err(|error| format!("Failed to determine the Git clone directory: {error}"))?;
    let clone_dir = resolve_clone(remote, &automation_repos_dir()?, &clone_cwd)?;
    let base = default_remote_branch(&clone_dir)
        .unwrap_or_else(|| "origin/HEAD".to_string());
    Ok(ResolvedRepo {
        path: clone_dir,
        base: Some(base),
    })
}

struct CloneSource<'a> {
    remote: &'a str,
    cwd: &'a Path,
    local_path: Option<PathBuf>,
}

impl<'a> CloneSource<'a> {
    fn new(remote: &'a str, cwd: &'a Path) -> Self {
        Self { remote, cwd, local_path: absolute_local_remote(remote, cwd) }
    }

    fn cache_key(&self) -> &[u8] {
        self.local_path.as_deref()
            .map(|path| path.as_os_str().as_encoded_bytes())
            .unwrap_or_else(|| self.remote.as_bytes())
    }

    fn clone_argument(&self) -> &std::ffi::OsStr {
        #[cfg(windows)]
        if let Some(local_path) = &self.local_path {
            let remote = Path::new(self.remote);
            // Git can save drive-relative and drive-less rooted paths
            // verbatim. Make them independent of the fetch working directory.
            if !remote.is_absolute()
                && (remote.has_root() || matches!(remote.components().next(), Some(std::path::Component::Prefix(_))))
            {
                return local_path.as_os_str();
            }
        }
        std::ffi::OsStr::new(self.remote)
    }
}

/// Resolve the spelling Git stores for a local clone.
fn absolute_local_remote(remote: &str, clone_cwd: &Path) -> Option<PathBuf> {
    if remote.is_empty() {
        return None;
    }
    let path = Path::new(remote);
    let has_prefix = matches!(path.components().next(), Some(std::path::Component::Prefix(_)));
    let absolute = if has_prefix && !path.is_absolute() {
        // A one-letter remote helper can look like a Windows drive prefix.
        if remote.split_once(':').is_some_and(|(_, address)| address.starts_with(':')) {
            return None;
        }
        // Windows drive-relative paths use the working directory of that drive.
        std::path::absolute(path).ok()?
    } else {
        clone_cwd.join(path)
    };
    // Git tries local repository paths before handing a colon-containing
    // argument to a transport. An insteadOf rule can rewrite that argument
    // even when the colon follows a slash.
    if !path.is_absolute() && remote.contains(':')
        && !git_clone_finds_local_source(&absolute, clone_cwd)
    {
        return None;
    }
    Some(absolute)
}

fn git_clone_finds_local_source(path: &Path, clone_cwd: &Path) -> bool {
    let candidate = |suffix: &str| {
        let mut candidate = path.as_os_str().to_os_string();
        candidate.push(suffix);
        PathBuf::from(candidate)
    };
    // Match Git clone's local lookup suffixes and let Git validate each
    // existing candidate as a repository directory or gitfile.
    ["/.git", "", ".git/.git", ".git"].into_iter().any(|suffix| {
        let path = candidate(suffix);
        path.exists() && crate::git::git_command()
            .current_dir(clone_cwd)
            .args(["rev-parse", "--resolve-git-dir"])
            .arg(&path)
            .output()
            .is_ok_and(|output| output.status.success())
    }) || [".bundle", ""].into_iter().any(|suffix| candidate(suffix).is_file())
}

fn clone_has_origin(clone_dir: &Path, source: &CloneSource<'_>) -> bool {
    let remote = source.remote;
    if remote.is_empty() || !clone_dir.join(".git").is_dir() {
        return false;
    }
    let Ok(output) = crate::git::git_command()
        .arg("-C")
        .arg(clone_dir)
        .args(["config", "--null", "--get-all", "remote.origin.url"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let Ok(origin) = std::str::from_utf8(&output.stdout) else {
        return false;
    };
    let Some(origin) = origin.strip_suffix('\0').filter(|origin| !origin.contains('\0')) else {
        return false;
    };
    let Some(absolute_remote) = source.local_path.as_deref() else {
        return origin == remote;
    };
    let origin_path = Path::new(origin);
    if !origin_path.is_absolute() {
        return false;
    }
    // Git can find a repository by appending .git even when the literal
    // path is absent or points to a different directory. Compare the
    // saved spelling, preserving trailing separators and parent components.
    if origin_path.as_os_str() == absolute_remote.as_os_str() {
        return true;
    }
    let remote_path = Path::new(remote);
    let is_plain_relative = !remote_path.has_root()
        && !matches!(remote_path.components().next(), Some(std::path::Component::Prefix(_)));
    #[cfg(windows)]
    let (origin_with_slashes, remote_with_slashes) = (origin.replace('\\', "/"), remote.replace('\\', "/"));
    #[cfg(windows)]
    let (origin, remote) = (origin_with_slashes.as_str(), remote_with_slashes.as_str());
    #[cfg(windows)]
    if absolute_remote.to_str().is_some_and(|path| origin == path.replace('\\', "/")) {
        return true;
    }
    // Git can use PWD's alias for the clone directory. Compare only that
    // directory when the source argument is identical. Canonicalizing the
    // full path would skip Git's repository lookup for the source itself.
    is_plain_relative
        && origin.strip_suffix(remote).filter(|path| path.ends_with('/')).is_some_and(|directory| {
            Path::new(directory).canonicalize().is_ok_and(|origin_cwd| {
                source.cwd.canonicalize().is_ok_and(|cwd| cwd == origin_cwd)
            })
        })
}

fn resolve_clone(remote: &str, repos_dir: &Path, clone_cwd: &Path) -> Result<PathBuf, String> {
    let source = CloneSource::new(remote, clone_cwd);
    let name = repo_dir_name(remote);
    let legacy = repos_dir.join(&name);
    // Keep existing clones and their worktrees usable when their origin matches.
    let clone_dir = if clone_has_origin(&legacy, &source) {
        legacy
    } else {
        let cache_key = source.cache_key();
        let digest = Sha256::digest(cache_key);
        let current = repos_dir.join(format!("{:.40}-{digest:x}", name));
        // Earlier versions hashed the relative argument. Reuse such a clone
        // only if it belongs to this invocation's resolved source.
        let previous = (cache_key != remote.as_bytes()).then(|| {
            let previous_digest = Sha256::digest(remote.as_bytes());
            repos_dir.join(format!("{:.40}-{previous_digest:x}", name))
        });
        previous.filter(|path| clone_has_origin(path, &source)).unwrap_or(current)
    };
    ensure_clone(&source, &clone_dir)?;
    Ok(clone_dir)
}

// Store clone progress in the length of the per-checkout lock file.
const CLONE_IN_PROGRESS: u64 = 1;
const CLONE_FAILED: u64 = 2;

fn record_clone_state(lock: &std::fs::File, state: u64) -> std::io::Result<()> {
    lock.set_len(state).and_then(|_| lock.sync_data())
}

/// Clone `remote` into `clone_dir` if absent; otherwise `git fetch` it.
/// Git authenticates with the host's own credentials — Codemux injects
/// no token (matching Superset's clone behaviour).
fn ensure_clone(source: &CloneSource<'_>, clone_dir: &Path) -> Result<(), String> {
    let parent = clone_dir.parent().ok_or("Automation checkout has no parent directory")?;
    let name = clone_dir.file_name().ok_or("Automation checkout has no directory name")?;
    let locks = parent.join(".locks");
    let recovery = parent.join(".failed-checkouts").join(name);
    std::fs::create_dir_all(&locks)
        .map_err(|error| format!("Failed to create automation checkout lock directory: {error}"))?;
    let lock_path = locks.join(name);
    // Keep the lock file in place so concurrent runs always lock the same file.
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(&lock_path)
        .map_err(|error| format!("Failed to open automation checkout lock: {error}"))?;
    lock.lock().map_err(|error| format!("Failed to lock automation checkout: {error}"))?;
    let state = lock.metadata().map_err(|error| format!("Failed to read automation checkout lock: {error}"))?.len();
    match state {
        0 => {}
        CLONE_FAILED => {
            preserve_failed_clone(clone_dir, &recovery)?;
            record_clone_state(&lock, 0)
                .map_err(|error| format!("Failed to clear the automation checkout's incomplete marker: {error}"))?;
        }
        _ => {
            return Err(format!(
                "Git did not report completion for the interrupted automation clone at {}. Its incomplete marker is {}",
                clone_dir.display(),
                lock_path.display(),
            ));
        }
    }

    if clone_dir.join(".git").exists() {
        if !clone_has_origin(clone_dir, source) {
            return Err("Automation checkout does not match the requested Git remote".into());
        }
        let mut command = crate::git::git_command();
        command
            .arg("-C")
            .arg(clone_dir)
            .args(["fetch", "--prune", "origin"]);
        let _ssh = crate::ssh_policy::unattended_git(&mut command, None)
            .map_err(|error| format!("Failed to prepare unattended Git SSH: {error}"))?;
        let output = command
            .output()
            .map_err(|error| format!("Failed to run git fetch: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git fetch failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        return Ok(());
    }
    match std::fs::symlink_metadata(clone_dir) {
        Ok(metadata) => {
            if !metadata.is_dir() || clone_dir.read_dir()
                .map_err(|error| format!("Failed to inspect automation checkout: {error}"))?
                .next().is_some()
            {
                return Err(format!("Automation checkout path is already occupied: {}", clone_dir.display()));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Failed to inspect automation checkout: {error}")),
    }
    let mut command = crate::git::git_command();
    // Initialize .git at its final location so clone and fetch see the same
    // destination-scoped Git config.
    command
        .current_dir(source.cwd)
        .args(["clone", "--"])
        .arg(source.clone_argument())
        .arg(clone_dir);
    let _ssh = crate::ssh_policy::unattended_git(&mut command, Some(clone_dir))
        .map_err(|error| format!("Failed to prepare unattended Git SSH: {error}"))?;
    record_clone_state(&lock, CLONE_IN_PROGRESS)
        .map_err(|error| format!("Failed to mark automation checkout incomplete: {error}"))?;
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let result = match child {
        Ok(child) => {
            // A failed wait can leave Git running. Keep the marker in that
            // case so a later run preserves the checkout.
            let output = child.wait_with_output()
                .map_err(|error| format!("Failed to wait for git clone: {error}"))?;
            if output.status.success() {
                Ok(())
            } else {
                let mut message = format!("git clone failed: {}", String::from_utf8_lossy(&output.stderr).trim());
                record_clone_state(&lock, CLONE_FAILED)
                    .map_err(|error| format!("{message}\nFailed to record the clone failure: {error}"))?;
                match preserve_failed_clone(clone_dir, &recovery) {
                    Ok(Some(saved)) => message.push_str(&format!(
                        "\nThe incomplete checkout was moved to {}.",
                        saved.display(),
                    )),
                    Ok(None) => {}
                    Err(error) => return Err(format!("{message}\n{error}")),
                }
                Err(message)
            }
        }
        Err(error) => Err(format!("Failed to run git clone: {error}")),
    };
    record_clone_state(&lock, 0)
        .map_err(|error| format!("Failed to clear the automation checkout's incomplete marker: {error}"))?;
    result
}

/// Called under the checkout lock after Git has reported failure. Preserve
/// one failed checkout per destination so a retry can use the original path.
/// If it fails again, keep both copies and block further clones until the
/// saved checkout has been moved elsewhere.
fn preserve_failed_clone(clone_dir: &Path, recovery: &Path) -> Result<Option<PathBuf>, String> {
    match std::fs::remove_dir(clone_dir) {
        Ok(()) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
        Err(error) => {
            return Err(format!("Failed to inspect the incomplete checkout at {}: {error}", clone_dir.display()));
        }
    }
    let saved = recovery.join("checkout");
    match std::fs::symlink_metadata(&saved) {
        Ok(_) => {
            return Err(format!(
                "Automation checkout recovery is full. Move the saved checkout at {} elsewhere before retrying. The latest failed checkout is at {}.",
                saved.display(),
                clone_dir.display(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("Failed to inspect the recovery path at {}: {error}", saved.display()));
        }
    }
    std::fs::create_dir_all(recovery)
        .map_err(|error| format!("Failed to create the recovery directory at {}: {error}", recovery.display()))?;
    std::fs::rename(clone_dir, &saved)
        .map_err(|error| format!("Failed to preserve the incomplete checkout at {}: {error}", clone_dir.display()))?;
    Ok(Some(saved))
}

/// `origin/<default-branch>` for a clone, resolved via `origin/HEAD`.
fn default_remote_branch(clone_dir: &Path) -> Option<String> {
    let output = crate::git::git_command()
        .arg("-C")
        .arg(clone_dir)
        .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let reference = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if reference.is_empty() {
        None
    } else {
        Some(reference)
    }
}

/// Best-effort: the URL of a PR for `branch`, via `gh pr list`. `None`
/// when `gh` is absent, unauthenticated, or no PR exists.
fn detect_pr_url(worktree: &Path, branch: &str) -> Option<String> {
    let output = crate::github_budget::run(
        worktree,
        &[
            "pr",
            "list",
            "--head",
            branch,
            "--limit",
            "1",
            "--json",
            "url",
            "--jq",
            ".[0].url // empty",
        ],
        std::time::Duration::from_secs(10),
    )
    .ok()?;
    let url = output.trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

/// Terminal outcome of executing a fire — written to the run row by
/// whichever scheduler owns the run.
pub struct FireOutcome {
    pub status: &'static str,
    pub workspace_id: Option<String>,
    pub branch: Option<String>,
    pub pr_url: Option<String>,
    pub error: Option<String>,
}

/// Run a short DB operation without holding the Tauri `State` borrow
/// across an await point.
fn with_db<RT: tauri::Runtime, R>(handle: &AppHandle<RT>, f: impl FnOnce(&DatabaseStore) -> R) -> R {
    let db: tauri::State<'_, DatabaseStore> = handle.state();
    f(&db)
}

/// Execute a fire — resolve the repo, create the worktree, run the
/// agent — and return the terminal outcome. Tauri-free, so the desktop
/// scheduler task and the `codemux-remote scheduler` loop share it.
pub async fn run_fire(automation: &AutomationRecord) -> FireOutcome {
    // The branch is deterministic and known up front, so it is recorded
    // on the run whatever the outcome.
    let branch = automation_branch(&automation.name, Utc::now());
    match run_inner(automation, &branch).await {
        Ok((workdir, pr_url)) => FireOutcome {
            status: "succeeded",
            workspace_id: Some(workdir),
            branch: Some(branch),
            pr_url,
            error: None,
        },
        Err((workdir, error)) => FireOutcome {
            status: "failed",
            workspace_id: workdir,
            branch: Some(branch),
            pr_url: None,
            error: Some(error),
        },
    }
}

/// Write a [`FireOutcome`] to its run row. Shared by every scheduler.
pub fn apply_outcome(db: &DatabaseStore, run_id: i64, outcome: &FireOutcome) {
    if outcome.status == "failed" {
        eprintln!(
            "[codemux::automations] run {run_id} failed: {}",
            outcome.error.as_deref().unwrap_or("unknown error")
        );
    }
    let _ = db.finish_automation_run(
        run_id,
        outcome.status,
        outcome.workspace_id.as_deref(),
        outcome.branch.as_deref(),
        outcome.pr_url.as_deref(),
        outcome.error.as_deref(),
    );
}

/// Desktop scheduler entry point: mark the run started, execute it, and
/// record the outcome. The Tauri `State` borrow is re-acquired per DB
/// touch via `with_db`, never held across the agent-process await.
pub async fn execute_run<RT: tauri::Runtime>(
    handle: AppHandle<RT>,
    automation: AutomationRecord,
    run: AutomationRunRecord,
) {
    with_db(&handle, |db| {
        let _ = db.mark_automation_run_started(run.id);
    });
    let outcome = run_fire(&automation).await;
    with_db(&handle, |db| apply_outcome(db, run.id, &outcome));
}

/// A webhook delivery that started a run, as reported to the sender.
pub struct DispatchedWebhook {
    pub run_id: i64,
    /// Placeholders that had no value in the request.
    pub missing: Vec<String>,
}

/// Webhook entry point for the HTTP route: validate the request with
/// `webhook::accept_delivery`, then announce and execute the run in the
/// background, like a scheduled fire. Returns once the run is recorded, so
/// the route can answer the sender without waiting for the agent.
pub fn dispatch_webhook<RT: tauri::Runtime>(
    handle: &AppHandle<RT>,
    webhook_id: &str,
    token: &str,
    request: &super::webhook::WebhookRequest,
) -> Result<DispatchedWebhook, super::webhook::DeliveryRejection> {
    use tauri::Emitter;

    let accepted = with_db(handle, |db| {
        super::webhook::accept_delivery(db, webhook_id, token, request, Utc::now())
    })?;
    if let Err(error) = handle.emit("automations://fire", &accepted.run) {
        eprintln!("[codemux::automations] failed to emit fire event: {error}");
    }
    let dispatched = DispatchedWebhook {
        run_id: accepted.run.id,
        missing: accepted.missing,
    };
    tauri::async_runtime::spawn(execute_run(
        handle.clone(),
        accepted.automation,
        accepted.run,
    ));
    Ok(dispatched)
}

/// Fallible body of `execute_run`. On success returns the worktree path
/// and any PR URL detected; on failure returns the worktree path (when
/// one was created) alongside the error, so the run row can still point
/// at the partial workspace.
async fn run_inner(
    automation: &AutomationRecord,
    branch: &str,
) -> Result<(String, Option<String>), (Option<String>, String)> {
    // Resolve the project to a repo on *this* machine — the local path
    // if it is here, otherwise a clone of the git remote.
    let project_path = automation.project_path.clone();
    let project_remote = automation.project_remote.clone();
    let repo = tokio::task::spawn_blocking(move || {
        resolve_repo(project_path.as_deref(), project_remote.as_deref())
    })
        .await
        .map_err(|error| (None, format!("Automation repo resolution failed: {error}")))?
        .map_err(|error| (None, error))?;

    let worktree_dir =
        worktree_destination(&repo.path, branch).map_err(|error| (None, error))?;

    prepare_worktree(&repo.path, &worktree_dir, branch, repo.base.as_deref())
        .map_err(|error| (None, error))?;

    let workdir = worktree_dir.to_string_lossy().to_string();

    let (program, args) = agent_command(&automation.agent, &automation.prompt)
        .map_err(|error| (Some(workdir.clone()), error))?;

    let launch_failed = |error: std::io::Error| {
        (
            Some(workdir.clone()),
            format!("Failed to launch '{program}': {error}"),
        )
    };
    // An unattended run is an agent process: it and its tools are refused
    // admin commands on the control socket.
    let mut command = crate::execution::host_command_tokio(&program);
    if program == "claude" {
        crate::claude_env::apply_claude_cli_env(&mut command, &worktree_dir);
    }
    let mut child = command
        .args(&args)
        .current_dir(&worktree_dir)
        .env(crate::control_identity::AGENT_PROCESS_ENV, "automation")
        .spawn()
        .map_err(launch_failed)?;
    if let Some(pid) = child.id() {
        crate::control_identity::register_agent_process(pid);
    }
    let status = child.wait().await.map_err(launch_failed)?;

    if status.success() {
        // Best-effort: if the agent pushed a branch and opened a PR,
        // record the URL so run history reads like a PR list.
        let pr_url = detect_pr_url(&worktree_dir, branch);
        Ok((workdir, pr_url))
    } else {
        Err((
            Some(workdir),
            format!("Agent '{program}' exited with {status}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn agent_command_maps_claude_to_print_mode() {
        let (program, args) = agent_command("claude", "do the thing").unwrap();
        assert_eq!(program, "claude");
        assert_eq!(args, ["--print", "--", "do the thing"]);
    }

    #[test]
    fn agent_command_maps_codex_to_exec_mode() {
        let (program, args) = agent_command("codex", "do the thing").unwrap();
        assert_eq!(program, "codex");
        assert_eq!(args, ["exec", "--", "do the thing"]);
    }

    #[test]
    fn agent_command_keeps_a_dash_prompt_after_the_option_terminator() {
        for agent in ["claude", "codex"] {
            let (_, args) = agent_command(agent, "--help").unwrap();
            assert_eq!(args[args.len() - 2..], ["--", "--help"]);
        }
    }

    #[test]
    fn agent_command_rejects_an_unknown_agent() {
        assert!(agent_command("gpt-9000", "x").is_err());
    }

    #[test]
    fn automation_branch_slugifies_and_timestamps() {
        let at = Utc.with_ymd_and_hms(2026, 1, 2, 9, 7, 0).unwrap();
        assert_eq!(
            automation_branch("Daily Issue Triage!", at),
            "automation-daily-issue-triage-20260102-090700"
        );
    }

    #[test]
    fn automation_branch_falls_back_when_name_has_no_word_chars() {
        let at = Utc.with_ymd_and_hms(2026, 1, 2, 9, 7, 0).unwrap();
        assert_eq!(
            automation_branch("***", at),
            "automation-automation-20260102-090700"
        );
    }

    #[test]
    fn prepare_worktree_creates_an_isolated_branch() {
        // A real temp git repo with one commit, then a worktree add.
        let repo = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["commit", "--allow-empty", "-q", "-m", "init"]);

        let dest = tempfile::tempdir().unwrap();
        let worktree = dest.path().join("wt");
        prepare_worktree(repo.path(), &worktree, "automation-test-1", None).unwrap();

        // A worktree has a `.git` file (not directory) pointing back at
        // the main repo.
        assert!(worktree.is_dir());
        assert!(worktree.join(".git").exists());
    }

    #[test]
    fn prepare_worktree_rejects_a_missing_project() {
        let dest = tempfile::tempdir().unwrap();
        let result = prepare_worktree(
            Path::new("/nonexistent/codemux/project"),
            &dest.path().join("wt"),
            "automation-test-2",
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn prepare_worktree_rejects_a_non_git_directory() {
        // An ordinary directory is not a repo — `git worktree add`
        // must fail rather than silently doing nothing.
        let plain = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let result = prepare_worktree(
            plain.path(),
            &dest.path().join("wt"),
            "automation-test-3",
            None,
        );
        assert!(result.is_err());
    }

    // ── Phase F: repo resolution (the GitHub backbone) ──

    /// Run `git` in `dir`, asserting success.
    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed in {}", dir.display());
    }

    /// A temp git repo with one commit, returned as a held `TempDir`.
    fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["commit", "--allow-empty", "-q", "-m", "init"]);
        dir
    }

    fn create_bare_remote_with_marker(directory: &Path, marker: &str) {
        let source = temp_repo();
        std::fs::write(source.path().join(marker), marker).unwrap();
        git(source.path(), &["add", "--", marker]);
        git(source.path(), &["commit", "-qm", "Add marker"]);
        std::fs::create_dir(directory).unwrap();
        git(
            directory,
            &["clone", "--bare", "-q", source.path().to_str().unwrap(), "repo.git"],
        );
    }

    #[cfg(unix)]
    fn install_post_checkout_hook(root: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let hooks = root.join("hooks");
        std::fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        std::fs::write(&hook, script).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(root, &["config", "--global", "core.hooksPath", hooks.to_str().unwrap()]);
        hook
    }

    #[test]
    fn repo_dir_name_takes_the_final_segment() {
        assert_eq!(repo_dir_name("git@github.com:me/whatsapp-rag.git"), "whatsapp-rag");
        assert_eq!(repo_dir_name("https://github.com/me/my-repo"), "my-repo");
        assert_eq!(repo_dir_name("https://github.com/me/my-repo/"), "my-repo");
        assert_eq!(repo_dir_name(r"C:\work\repo.git"), "work-repo");
        // No usable segment → a stable fallback.
        assert_eq!(repo_dir_name("////"), "project");
    }

    #[test]
    fn ensure_clone_clones_then_fetches_idempotently() {
        // A source repo, mirror-cloned to a bare "remote".
        let source = temp_repo();
        let remote_dir = tempfile::tempdir().unwrap();
        let remote = remote_dir.path().join("bare.git");
        let ok = std::process::Command::new("git")
            .args(["clone", "--bare", "-q"])
            .arg(source.path())
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "bare clone failed");

        let dest = tempfile::tempdir().unwrap();
        let clone_dir = dest.path().join("clone");
        let remote_str = remote.to_string_lossy().to_string();

        // First call clones.
        let source = CloneSource::new(&remote_str, remote_dir.path());
        ensure_clone(&source, &clone_dir).unwrap();
        assert!(clone_dir.join(".git").is_dir());
        // Second call fetches an existing clone — must not error.
        ensure_clone(&source, &clone_dir).unwrap();

        // The clone exposes a default remote branch.
        let base = default_remote_branch(&clone_dir);
        assert!(
            base.as_deref().map(|b| b.starts_with("origin/")).unwrap_or(false),
            "expected origin/<branch>, got {base:?}"
        );
    }

    #[test]
    fn resolve_clone_retries_after_git_reports_failure() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("source");
        let remote = source_dir.join("repo.git");
        let remote = remote.to_str().unwrap();
        let cache = root.path().join("cache");
        let error = resolve_clone(remote, &cache, root.path()).unwrap_err();
        assert!(error.starts_with("git clone failed:"), "{error}");

        create_bare_remote_with_marker(&source_dir, "retry.txt");
        let checkout = resolve_clone(remote, &cache, root.path()).unwrap();
        assert!(checkout.join("retry.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_clone_retries_and_preserves_files_after_checkout_failure() {
        if crate::test_isolation::isolated(
            "automations::executor::tests::resolve_clone_retries_and_preserves_files_after_checkout_failure",
        ) {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        create_bare_remote_with_marker(&source, "tracked.txt");
        install_post_checkout_hook(root.path(), r#"#!/bin/sh
if [ ! -f "$0.ran" ]; then
    : > "$0.ran"
    printf '%s' 'first checkout data' > first-attempt.txt
    printf '%s\n' 'fixture checkout hook failed' >&2
    exit 1
fi
"#);

        let remote = source.join("repo.git");
        let remote = remote.to_str().unwrap();
        let cache = root.path().join("cache");
        let error = resolve_clone(remote, &cache, root.path()).unwrap_err();
        assert!(error.contains("fixture checkout hook failed"), "{error}");

        let checkout = resolve_clone(remote, &cache, root.path()).unwrap();
        assert!(checkout.join("tracked.txt").is_file());
        assert!(!checkout.join("first-attempt.txt").exists());

        let failed_checkout = cache.join(".failed-checkouts")
            .join(checkout.file_name().unwrap()).join("checkout");
        assert!(failed_checkout.join(".git").is_dir());
        assert!(failed_checkout.join("tracked.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(failed_checkout.join("first-attempt.txt")).unwrap(),
            "first checkout data",
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_clone_bounds_repeated_checkout_failures_and_resumes_after_archive_move() {
        if crate::test_isolation::isolated(
            "automations::executor::tests::resolve_clone_bounds_repeated_checkout_failures_and_resumes_after_archive_move",
        ) {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        create_bare_remote_with_marker(&source, "tracked.txt");
        let hook = install_post_checkout_hook(root.path(), r#"#!/bin/sh
printf '%s\n' checkout >> "$0.attempts"
if [ ! -f "$0.allow" ]; then
    cp "$0.attempts" failed-attempts.txt
    printf '%s\n' 'fixture checkout hook failed' >&2
    exit 1
fi
"#);

        let remote = source.join("repo.git");
        let remote = remote.to_str().unwrap();
        let cache = root.path().join("cache");
        let name = format!("repo-{:x}", Sha256::digest(remote.as_bytes()));
        let checkout = cache.join(&name);
        let archives = cache.join(".failed-checkouts");
        let slot = archives.join(&name);
        let archived_checkout = slot.join("checkout");
        let resolve = || resolve_clone(remote, &cache, root.path());

        let first_error = resolve().unwrap_err();
        assert!(first_error.contains("fixture checkout hook failed"), "{first_error}");
        assert!(!checkout.exists());
        let second_error = resolve().unwrap_err();
        assert!(second_error.contains("fixture checkout hook failed"), "{second_error}");

        for _ in 0..2 {
            let error = resolve().unwrap_err();
            assert!(error.contains("recovery is full"), "{error}");
            assert!(error.contains(checkout.to_str().unwrap()), "{error}");
            assert!(error.contains(archived_checkout.to_str().unwrap()), "{error}");
            assert!(!error.contains("fixture checkout hook failed"), "{error}");
        }
        let attempts = hook.with_extension("attempts");
        assert_eq!(std::fs::read_to_string(&attempts).unwrap(), "checkout\ncheckout\n");
        assert_eq!(archives.read_dir().unwrap().count(), 1);
        assert_eq!(std::fs::metadata(cache.join(".locks").join(&name)).unwrap().len(), CLONE_FAILED);
        let failed_attempts = |path: &Path| std::fs::read_to_string(path.join("failed-attempts.txt")).unwrap();
        assert_eq!(failed_attempts(&archived_checkout), "checkout\n");
        assert_eq!(failed_attempts(&checkout), "checkout\ncheckout\n");

        let saved_first = root.path().join("retained-first");
        std::fs::rename(&slot, &saved_first).unwrap();
        std::fs::write(hook.with_extension("allow"), b"").unwrap();
        assert_eq!(resolve().unwrap(), checkout);
        assert!(checkout.join("tracked.txt").is_file());
        assert!(!checkout.join("failed-attempts.txt").exists());
        assert_eq!(failed_attempts(&saved_first.join("checkout")), "checkout\n");
        assert_eq!(failed_attempts(&archived_checkout), "checkout\ncheckout\n");
        assert_eq!(std::fs::read_to_string(attempts).unwrap(), "checkout\ncheckout\ncheckout\n");
    }

    #[test]
    fn ensure_clone_recovers_a_recorded_failure_on_retry() {
        let root = tempfile::tempdir().unwrap();
        let source = temp_repo();
        let source = CloneSource::new(source.path().to_str().unwrap(), root.path());
        let checkout = root.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::write(checkout.join("leftover.txt"), "failed clone file").unwrap();
        let locks = root.path().join(".locks");
        std::fs::create_dir(&locks).unwrap();
        let marker = std::fs::File::create(locks.join("checkout")).unwrap();
        record_clone_state(&marker, CLONE_FAILED).unwrap();
        drop(marker);

        ensure_clone(&source, &checkout).unwrap();
        assert!(checkout.join(".git").is_dir());
        assert!(!checkout.join("leftover.txt").exists());
        let saved = root.path().join(".failed-checkouts/checkout/checkout/leftover.txt");
        assert_eq!(std::fs::read_to_string(saved).unwrap(), "failed clone file");
    }

    #[test]
    fn ensure_clone_preserves_a_checkout_when_completion_is_unknown() {
        let root = tempfile::tempdir().unwrap();
        let source = temp_repo();
        let source = CloneSource::new(source.path().to_str().unwrap(), root.path());
        let checkout = root.path().join("checkout");
        ensure_clone(&source, &checkout).unwrap();
        let marker = std::fs::File::create(root.path().join(".locks/checkout")).unwrap();
        record_clone_state(&marker, CLONE_IN_PROGRESS).unwrap();
        drop(marker);

        let error = ensure_clone(&source, &checkout).unwrap_err();
        assert!(error.contains("Git did not report completion"), "{error}");
        assert!(checkout.join(".git").is_dir());
    }

    #[test]
    fn resolve_clone_serializes_concurrent_requests_for_the_same_checkout() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("source");
        create_bare_remote_with_marker(&source_dir, "shared.txt");
        let remote = source_dir.join("repo.git");
        let remote = remote.to_str().unwrap();
        let cache = root.path().join("cache");
        let start = std::sync::Barrier::new(2);
        let resolve = || {
            start.wait();
            resolve_clone(remote, &cache, root.path()).unwrap()
        };

        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(&resolve);
            let second = resolve();
            (first.join().unwrap(), second)
        });
        assert_eq!(first, second);
        assert!(first.join("shared.txt").is_file());
    }

    #[test]
    fn resolve_clone_reuses_a_relative_bare_remote_without_the_git_suffix() {
        let source = temp_repo();
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &[
            "clone", "--bare", "-q", source.path().to_str().unwrap(), "project.git",
        ]);
        assert!(!root.path().join("project").exists());
        let cache = root.path().join("cache");

        let checkout = resolve_clone("project", &cache, root.path()).unwrap();
        assert_eq!(resolve_clone("project", &cache, root.path()).unwrap(), checkout);
        assert!(!clone_has_origin(&checkout, &CloneSource::new("project", &cache)));
        assert!(!clone_has_origin(&checkout, &CloneSource::new("project/", root.path())));
    }

    #[test]
    fn resolve_clone_preserves_git_url_rewrites() {
        if crate::test_isolation::isolated(
            "automations::executor::tests::resolve_clone_preserves_git_url_rewrites",
        ) {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let rewritten = root.path().join("rewritten");
        create_bare_remote_with_marker(&rewritten, "rewritten.txt");
        let url = url::Url::from_directory_path(&rewritten).unwrap();
        git(root.path(), &["config", "--global", &format!("url.{url}.insteadOf"), "company/git:org/"]);
        let remote = "company/git:org/repo.git";
        let cache = root.path().join("cache");

        let aliased = resolve_clone(remote, &cache, root.path()).unwrap();
        assert!(aliased.join("rewritten.txt").is_file());
        assert_eq!(resolve_clone(remote, &cache, root.path()).unwrap(), aliased);

        // Git prefers a literal repository with this spelling over the
        // rewrite, even if the rewritten remote already has a checkout.
        #[cfg(unix)]
        {
            let shadow = root.path().join("shadow");
            std::fs::create_dir_all(shadow.join("company")).unwrap();
            create_bare_remote_with_marker(&shadow.join("company/git:org"), "literal.txt");
            let literal = resolve_clone(remote, &cache, &shadow).unwrap();
            assert_ne!(literal, aliased);
            assert!(literal.join("literal.txt").is_file());
            assert_eq!(resolve_clone(remote, &cache, &shadow).unwrap(), literal);
            assert_eq!(resolve_clone(remote, &cache, root.path()).unwrap(), aliased);
        }
    }

    #[test]
    fn resolve_clone_uses_destination_git_config_for_clone_and_fetch() {
        if crate::test_isolation::isolated(
            "automations::executor::tests::resolve_clone_uses_destination_git_config_for_clone_and_fetch",
        ) {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let fallback = root.path().join("fallback");
        std::fs::create_dir(&fallback).unwrap();
        create_bare_remote_with_marker(&fallback.join("project"), "fallback.txt");
        let scoped = root.path().join("scoped");
        create_bare_remote_with_marker(&scoped, "scoped.txt");

        let remote = "company:project/repo.git";
        let cache = root.path().join("cache");
        let checkout_name = format!("repo-{:x}", Sha256::digest(remote.as_bytes()));
        let fallback_url = url::Url::from_directory_path(&fallback).unwrap();
        git(root.path(), &[
            "config", "--global", &format!("url.{fallback_url}.insteadOf"), "company:",
        ]);
        let scoped_config = root.path().join("scoped.gitconfig");
        let scoped_url = url::Url::from_directory_path(&scoped).unwrap();
        git(root.path(), &[
            "config", "--file", scoped_config.to_str().unwrap(),
            &format!("url.{scoped_url}.insteadOf"), "company:project/",
        ]);
        git(root.path(), &[
            "config", "--global", &format!("includeIf.gitdir:**/cache/{checkout_name}/.git.path"),
            scoped_config.to_str().unwrap(),
        ]);

        let checkout = resolve_clone(remote, &cache, root.path()).unwrap();
        assert_eq!(checkout, cache.join(&checkout_name));
        assert!(checkout.join("scoped.txt").is_file());
        assert!(!checkout.join("fallback.txt").exists());

        let publisher = root.path().join("publisher");
        git(root.path(), &[
            "clone", "-q", "--", scoped.join("repo.git").to_str().unwrap(),
            publisher.to_str().unwrap(),
        ]);
        std::fs::write(publisher.join("fetched.txt"), "new scoped commit").unwrap();
        git(&publisher, &["add", "fetched.txt"]);
        git(&publisher, &["commit", "-qm", "Advance scoped remote"]);
        git(&publisher, &["push", "-q", "origin", "HEAD"]);

        assert_eq!(resolve_clone(remote, &cache, root.path()).unwrap(), checkout);
        let branch = default_remote_branch(&checkout).unwrap();
        git(&checkout, &["cat-file", "-e", &format!("{branch}:fetched.txt")]);
    }

    #[cfg(windows)]
    #[test]
    fn resolve_clone_reuses_a_legacy_checkout_for_a_windows_path() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("source");
        create_bare_remote_with_marker(&source_dir, "legacy.txt");
        let remote = source_dir.join("repo.git");
        let remote = remote.to_str().unwrap().replace('/', "\\");
        let cache = root.path().join("cache");
        std::fs::create_dir(&cache).unwrap();
        let legacy = cache.join(repo_dir_name(&remote));
        assert_ne!(legacy, cache.join("repo"));
        git(root.path(), &["clone", "-q", "--", &remote, legacy.to_str().unwrap()]);

        assert_eq!(resolve_clone(&remote, &cache, root.path()).unwrap(), legacy);
        assert!(legacy.join("legacy.txt").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn resolve_clone_can_fetch_a_windows_drive_relative_remote() {
        if crate::test_isolation::isolated(
            "automations::executor::tests::resolve_clone_can_fetch_a_windows_drive_relative_remote",
        ) {
            return;
        }
        let cwd = std::env::current_dir().unwrap();
        let source_dir = cwd.join("source");
        create_bare_remote_with_marker(&source_dir, "local.txt");
        let drive = cwd.components().next().unwrap();
        let remote = format!("{}source\\repo.git", drive.as_os_str().to_str().unwrap());
        assert!(!Path::new(&remote).is_absolute());
        let cache = cwd.join("cache");

        let checkout = resolve_clone(&remote, &cache, &cwd).unwrap();
        assert_eq!(resolve_clone(&remote, &cache, &cwd).unwrap(), checkout);
        assert!(checkout.join("local.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_clone_separates_bare_repos_when_their_suffixless_paths_alias() {
        let root = tempfile::tempdir().unwrap();
        let shared_directory = root.path().join("shared");
        std::fs::create_dir(&shared_directory).unwrap();
        for (directory, marker) in [("first", "first.txt"), ("second", "second.txt")] {
            let directory = root.path().join(directory);
            create_bare_remote_with_marker(&directory, marker);
            std::os::unix::fs::symlink(&shared_directory, directory.join("repo")).unwrap();
        }

        let cache = root.path().join("cache");
        let legacy = cache.join("repo");
        ensure_clone(&CloneSource::new("first/repo", root.path()), &legacy).unwrap();
        assert!(legacy.join("first.txt").is_file());

        let second = resolve_clone("second/repo", &cache, root.path()).unwrap();
        assert_ne!(second, legacy);
        assert!(second.join("second.txt").is_file());
        assert!(!second.join("first.txt").exists());
        assert_eq!(resolve_clone("first/repo", &cache, root.path()).unwrap(), legacy);
        assert_eq!(resolve_clone("second/repo", &cache, root.path()).unwrap(), second);
    }

    #[test]
    fn absolute_local_remote_preserves_remote_helper_syntax() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolute_local_remote("x::repository", &cwd), None);
    }

    #[test]
    fn resolve_clone_separates_relative_remotes_in_different_working_directories() {
        let root = tempfile::tempdir().unwrap();
        let first_cwd = root.path().join("first");
        let second_cwd = root.path().join("second");
        create_bare_remote_with_marker(&first_cwd, "first.txt");
        create_bare_remote_with_marker(&second_cwd, "second.txt");
        let cache = root.path().join("cache");

        let first = resolve_clone("repo", &cache, &first_cwd).unwrap();
        let second = resolve_clone("repo", &cache, &second_cwd).unwrap();
        assert_ne!(first, second);
        assert!(first.join("first.txt").is_file());
        assert!(second.join("second.txt").is_file());
        assert_eq!(resolve_clone("repo", &cache, &first_cwd).unwrap(), first);
        assert_eq!(resolve_clone("repo", &cache, &second_cwd).unwrap(), second);
    }

    #[test]
    fn resolve_clone_reuses_a_matching_checkout_keyed_by_the_raw_relative_remote() {
        let root = tempfile::tempdir().unwrap();
        let first_cwd = root.path().join("first");
        let second_cwd = root.path().join("second");
        create_bare_remote_with_marker(&first_cwd, "first.txt");
        create_bare_remote_with_marker(&second_cwd, "second.txt");
        let cache = root.path().join("cache");
        let old_checkout = cache.join(format!("repo-{:x}", Sha256::digest(b"repo")));
        ensure_clone(&CloneSource::new("repo", &first_cwd), &old_checkout).unwrap();

        let second = resolve_clone("repo", &cache, &second_cwd).unwrap();
        assert_ne!(second, old_checkout);
        assert!(second.join("second.txt").is_file());
        assert_eq!(resolve_clone("repo", &cache, &first_cwd).unwrap(), old_checkout);
        assert!(old_checkout.join("first.txt").is_file());
    }

    #[test]
    fn remote_automations_keep_same_named_repositories_separate() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first/repo");
        let second = root.path().join("second/repo");
        for (path, marker) in [(&first, "first.txt"), (&second, "second.txt")] {
            std::fs::create_dir_all(path).unwrap();
            git(path, &["init", "-q"]);
            git(path, &["config", "user.email", "t@example.com"]);
            git(path, &["config", "user.name", "Test"]);
            std::fs::write(path.join(marker), marker).unwrap();
            git(path, &["add", marker]);
            git(path, &["commit", "-qm", "Add marker"]);
        }
        let first = first.to_str().unwrap();
        let second = second.to_str().unwrap();
        let cache = root.path().join("cache");
        let legacy = cache.join("repo");
        ensure_clone(&CloneSource::new(first, root.path()), &legacy).unwrap();

        let second_clone = resolve_clone(second, &cache, root.path()).unwrap();
        assert_ne!(second_clone, legacy);
        assert!(second_clone.join("second.txt").is_file());
        assert_eq!(resolve_clone(second, &cache, root.path()).unwrap(), second_clone);
        assert_eq!(resolve_clone(first, &cache, root.path()).unwrap(), legacy);
        assert!(legacy.join("first.txt").is_file());
    }

    #[test]
    fn resolve_repo_uses_a_local_project_path_when_present() {
        let repo = temp_repo();
        let path = repo.path().to_string_lossy().to_string();
        let resolved = resolve_repo(Some(&path), None).unwrap();
        assert_eq!(resolved.path, repo.path());
        // A local repo branches off its own HEAD.
        assert!(resolved.base.is_none());
    }

    #[test]
    fn resolve_repo_errors_without_a_path_or_remote() {
        // No project_path on this machine and no remote to clone.
        let result = resolve_repo(Some("/nonexistent/x"), None);
        assert!(result.is_err());
    }

    #[test]
    fn prepare_worktree_branches_off_an_explicit_base() {
        let repo = temp_repo();
        // A second commit on a side branch to use as the base.
        git(repo.path(), &["checkout", "-q", "-b", "side"]);
        git(repo.path(), &["commit", "--allow-empty", "-q", "-m", "side commit"]);
        git(repo.path(), &["checkout", "-q", "-"]);

        let dest = tempfile::tempdir().unwrap();
        let worktree = dest.path().join("wt");
        prepare_worktree(repo.path(), &worktree, "automation-based", Some("side"))
            .unwrap();
        assert!(worktree.join(".git").exists());

        // The worktree's HEAD descends from `side` — its tip commit
        // matches `side`'s tip.
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(&worktree)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let side = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["rev-parse", "side"])
            .output()
            .unwrap();
        assert_eq!(head.stdout, side.stdout);
    }
}
