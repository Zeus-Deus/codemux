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
fn resolve_repo(automation: &AutomationRecord) -> Result<ResolvedRepo, String> {
    if let Some(path) = &automation.project_path {
        let candidate = Path::new(path);
        if candidate.join(".git").exists() {
            return Ok(ResolvedRepo {
                path: candidate.to_path_buf(),
                base: None,
            });
        }
    }
    let remote = automation.project_remote.as_deref().ok_or_else(|| {
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

/// Clone `remote` into `clone_dir` if absent; otherwise `git fetch` it.
/// Git authenticates with the host's own credentials — Codemux injects
/// no token (matching Superset's clone behaviour).
fn ensure_clone(source: &CloneSource<'_>, clone_dir: &Path) -> Result<(), String> {
    if clone_dir.join(".git").exists() {
        if !clone_has_origin(clone_dir, source) {
            return Err("Automation checkout does not match the requested Git remote".into());
        }
        let output = crate::git::git_command()
            .arg("-C")
            .arg(clone_dir)
            .args(["fetch", "--prune", "origin"])
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
    let parent = clone_dir.parent().ok_or("Automation checkout has no parent directory")?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create clone directory: {error}"))?;
    // Publish only a complete clone, so a retry or a concurrent run never
    // mistakes a partially cloned directory for a usable Git checkout.
    let staging = tempfile::tempdir_in(parent)
        .map_err(|error| format!("Failed to create temporary clone directory: {error}"))?;
    let staging_clone = staging.path().join("repo");
    let output = crate::git::git_command()
        .current_dir(source.cwd)
        .args(["clone", "--"])
        .arg(source.clone_argument())
        .arg(&staging_clone)
        .output()
        .map_err(|error| format!("Failed to run git clone: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if let Err(error) = std::fs::rename(&staging_clone, clone_dir) {
        if !clone_has_origin(clone_dir, source) {
            return Err(format!("Failed to move automation clone into place: {error}"));
        }
    }
    Ok(())
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
    let repo = resolve_repo(automation).map_err(|error| (None, error))?;

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

    fn automation_with(
        project_path: Option<&str>,
        project_remote: Option<&str>,
    ) -> AutomationRecord {
        AutomationRecord {
            id: 1,
            server_id: None,
            name: "Test".to_string(),
            prompt: "do the thing".to_string(),
            agent: "claude".to_string(),
            schedule: "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY".to_string(),
            timezone: "UTC".to_string(),
            host_id: None,
            project_path: project_path.map(str::to_string),
            project_remote: project_remote.map(str::to_string),
            enabled: true,
            retention_limit: 10,
            last_run_at: None,
            next_run_at: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            deleted_at: None,
            dirty: false,
            trigger_type: Default::default(),
            webhook_id: None,
            webhook_token: None,
            webhook_signature: None,
            webhook_secret_ref: None,
        }
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
        let resolved = resolve_repo(&automation_with(Some(&path), None)).unwrap();
        assert_eq!(resolved.path, repo.path());
        // A local repo branches off its own HEAD.
        assert!(resolved.base.is_none());
    }

    #[test]
    fn resolve_repo_errors_without_a_path_or_remote() {
        // No project_path on this machine and no remote to clone.
        let result = resolve_repo(&automation_with(Some("/nonexistent/x"), None));
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
