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
//! The pure pieces — `agent_command`, `automation_branch` — are unit
//! tested; `prepare_worktree` is covered against a real temp repo.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Manager};

use crate::database::{AutomationRecord, AutomationRunRecord, DatabaseStore};

/// Map an agent name to the headless CLI invocation that runs a single
/// prompt to completion. Pure.
pub fn agent_command(agent: &str, prompt: &str) -> Result<(String, Vec<String>), String> {
    match agent {
        "claude" => Ok((
            "claude".to_string(),
            vec!["--print".to_string(), prompt.to_string()],
        )),
        "codex" => Ok((
            "codex".to_string(),
            vec!["exec".to_string(), prompt.to_string()],
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
    let mut cmd = crate::execution::host_command("git");
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
    Ok(())
}

// ── Project repo resolution (the GitHub backbone) ──
//
// An automation run needs the project repo *on the machine the run
// executes on*. On the desktop / "This machine" that is `project_path`.
// On a separate host that path does not exist, so the host clones the
// project's git remote — see `docs/plans/automations-sync.md` Phase F.

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
    let clone_dir = automation_repos_dir()?.join(repo_dir_name(remote));
    ensure_clone(remote, &clone_dir)?;
    let base = default_remote_branch(&clone_dir)
        .unwrap_or_else(|| "origin/HEAD".to_string());
    Ok(ResolvedRepo {
        path: clone_dir,
        base: Some(base),
    })
}

/// Clone `remote` into `clone_dir` if absent; otherwise `git fetch` it.
/// Git authenticates with the host's own credentials — Codemux injects
/// no token (matching Superset's clone behaviour).
fn ensure_clone(remote: &str, clone_dir: &Path) -> Result<(), String> {
    if clone_dir.join(".git").exists() {
        let output = crate::execution::host_command("git")
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
    if let Some(parent) = clone_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create clone directory: {error}"))?;
    }
    let output = crate::execution::host_command("git")
        .args(["clone", remote])
        .arg(clone_dir)
        .output()
        .map_err(|error| format!("Failed to run git clone: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// `origin/<default-branch>` for a clone, resolved via `origin/HEAD`.
fn default_remote_branch(clone_dir: &Path) -> Option<String> {
    let output = crate::execution::host_command("git")
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
    let output = crate::execution::host_command("gh")
        .arg("pr")
        .arg("list")
        .args(["--head", branch])
        .args(["--json", "url"])
        .args(["--jq", ".[0].url // empty"])
        .current_dir(worktree)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
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

    let status = crate::execution::host_command_tokio(&program)
        .args(&args)
        .current_dir(&worktree_dir)
        .status()
        .await
        .map_err(|error| {
            (
                Some(workdir.clone()),
                format!("Failed to launch '{program}': {error}"),
            )
        })?;

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
        assert_eq!(args, vec!["--print".to_string(), "do the thing".to_string()]);
    }

    #[test]
    fn agent_command_maps_codex_to_exec_mode() {
        let (program, args) = agent_command("codex", "do the thing").unwrap();
        assert_eq!(program, "codex");
        assert_eq!(args, vec!["exec".to_string(), "do the thing".to_string()]);
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
        }
    }

    #[test]
    fn repo_dir_name_takes_the_final_segment() {
        assert_eq!(repo_dir_name("git@github.com:me/whatsapp-rag.git"), "whatsapp-rag");
        assert_eq!(repo_dir_name("https://github.com/me/my-repo"), "my-repo");
        assert_eq!(repo_dir_name("https://github.com/me/my-repo/"), "my-repo");
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
        ensure_clone(&remote_str, &clone_dir).unwrap();
        assert!(clone_dir.join(".git").is_dir());
        // Second call fetches an existing clone — must not error.
        ensure_clone(&remote_str, &clone_dir).unwrap();

        // The clone exposes a default remote branch.
        let base = default_remote_branch(&clone_dir);
        assert!(
            base.as_deref().map(|b| b.starts_with("origin/")).unwrap_or(false),
            "expected origin/<branch>, got {base:?}"
        );
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
