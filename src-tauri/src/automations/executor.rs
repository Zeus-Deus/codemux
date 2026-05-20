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

/// Create an isolated git worktree at `worktree_dir` on a new `branch`
/// off the project's current HEAD. Returns a flat error string on any
/// failure (path missing, not a git repo, branch clash, git absent).
pub fn prepare_worktree(
    project: &Path,
    worktree_dir: &Path,
    branch: &str,
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
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["worktree", "add"])
        .arg(worktree_dir)
        .arg("-b")
        .arg(branch)
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

/// Terminal outcome of executing a fire — written to the run row by
/// whichever scheduler owns the run.
pub struct FireOutcome {
    pub status: &'static str,
    pub workspace_id: Option<String>,
    pub error: Option<String>,
}

/// Run a short DB operation without holding the Tauri `State` borrow
/// across an await point.
fn with_db<R>(handle: &AppHandle, f: impl FnOnce(&DatabaseStore) -> R) -> R {
    let db: tauri::State<'_, DatabaseStore> = handle.state();
    f(&db)
}

/// Execute a fire — create the worktree, run the agent — and return the
/// terminal outcome. Tauri-free, so both the desktop scheduler task and
/// the `codemux-remote scheduler` loop call the same code.
pub async fn run_fire(automation: &AutomationRecord) -> FireOutcome {
    match run_inner(automation).await {
        Ok(workdir) => FireOutcome {
            status: "succeeded",
            workspace_id: Some(workdir),
            error: None,
        },
        Err((workdir, error)) => FireOutcome {
            status: "failed",
            workspace_id: workdir,
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
        outcome.error.as_deref(),
    );
}

/// Desktop scheduler entry point: mark the run started, execute it, and
/// record the outcome. The Tauri `State` borrow is re-acquired per DB
/// touch via `with_db`, never held across the agent-process await.
pub async fn execute_run(
    handle: AppHandle,
    automation: AutomationRecord,
    run: AutomationRunRecord,
) {
    with_db(&handle, |db| {
        let _ = db.mark_automation_run_started(run.id);
    });
    let outcome = run_fire(&automation).await;
    with_db(&handle, |db| apply_outcome(db, run.id, &outcome));
}

/// Fallible body of `execute_run`. On failure returns the workspace
/// path (when a worktree was created) alongside the error, so the run
/// row can still point at the partial workspace.
async fn run_inner(
    automation: &AutomationRecord,
) -> Result<String, (Option<String>, String)> {
    let project = automation
        .project_path
        .clone()
        .ok_or((None, "Automation has no project path set".to_string()))?;
    let project_path = PathBuf::from(&project);

    let branch = automation_branch(&automation.name, Utc::now());
    let worktree_dir =
        worktree_destination(&project_path, &branch).map_err(|error| (None, error))?;

    prepare_worktree(&project_path, &worktree_dir, &branch)
        .map_err(|error| (None, error))?;

    let workdir = worktree_dir.to_string_lossy().to_string();

    let (program, args) = agent_command(&automation.agent, &automation.prompt)
        .map_err(|error| (Some(workdir.clone()), error))?;

    let status = tokio::process::Command::new(&program)
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
        Ok(workdir)
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
        prepare_worktree(repo.path(), &worktree, "automation-test-1").unwrap();

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
        );
        assert!(result.is_err());
    }

    #[test]
    fn prepare_worktree_rejects_a_non_git_directory() {
        // An ordinary directory is not a repo — `git worktree add`
        // must fail rather than silently doing nothing.
        let plain = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let result =
            prepare_worktree(plain.path(), &dest.path().join("wt"), "automation-test-3");
        assert!(result.is_err());
    }
}
