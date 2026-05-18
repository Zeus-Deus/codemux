use crate::git::{BaseBranchDiff, CommitFileEntry, ConflictCheckResult, GitBranchInfo, GitDiffStat, GitFileStatus, GitLogEntry, MergeIntoBaseResult, MergeState, ResolverBranchInfo, WorktreeInfo};
use std::path::Path;

// Every command here shells out via `crate::git::*` → `Command::new("git")`.
// They MUST run on Tokio's blocking pool: a sync `#[tauri::command]` runs on
// the GTK main thread, and any wedged `git` subprocess (slow disk, stuck FS,
// network credential prompt) freezes the whole UI hard enough that even
// window-close requests can't be processed. The fix is uniform — async
// command + `spawn_blocking`. Frontend-side `invoke()` already returns a
// Promise either way, so no caller changes are needed.

#[tauri::command]
pub async fn check_is_git_repo(path: String) -> bool {
    tokio::task::spawn_blocking(move || crate::git::is_git_repo(Path::new(&path)))
        .await
        .unwrap_or(false)
}

#[tauri::command]
pub async fn init_git_repo(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || crate::git::git_init_repo(Path::new(&path)))
        .await
        .map_err(|e| format!("init_git_repo task join failed: {e}"))?
}

#[tauri::command]
pub async fn create_empty_repo(parent_dir: String, name: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let repo_path = Path::new(&parent_dir).join(&name);
        std::fs::create_dir_all(&repo_path)
            .map_err(|e| format!("Failed to create directory: {e}"))?;
        crate::git::git_init_repo(&repo_path)?;
        Ok(repo_path.display().to_string())
    })
    .await
    .map_err(|e| format!("create_empty_repo task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_git_status(path: String) -> Result<Vec<GitFileStatus>, String> {
    tokio::task::spawn_blocking(move || crate::git::git_status(Path::new(&path)))
        .await
        .map_err(|e| format!("get_git_status task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_git_diff(path: String, file: String, staged: bool) -> Result<String, String> {
    tokio::task::spawn_blocking(move || crate::git::git_diff(Path::new(&path), &file, staged))
        .await
        .map_err(|e| format!("get_git_diff task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_git_diff_stat(path: String) -> Result<GitDiffStat, String> {
    tokio::task::spawn_blocking(move || crate::git::git_diff_stat(Path::new(&path)))
        .await
        .map_err(|e| format!("get_git_diff_stat task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_stage_files(path: String, files: Vec<String>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::git_stage(Path::new(&path), &files))
        .await
        .map_err(|e| format!("git_stage_files task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_unstage_files(path: String, files: Vec<String>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::git_unstage(Path::new(&path), &files))
        .await
        .map_err(|e| format!("git_unstage_files task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_commit_changes(path: String, message: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::git_commit(Path::new(&path), &message))
        .await
        .map_err(|e| format!("git_commit_changes task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_push_changes(path: String, set_upstream: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::git_push(Path::new(&path), set_upstream))
        .await
        .map_err(|e| format!("git_push_changes task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_pull_changes(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::git_pull(Path::new(&path)))
        .await
        .map_err(|e| format!("git_pull_changes task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_fetch_changes(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::git_fetch(Path::new(&path)))
        .await
        .map_err(|e| format!("git_fetch_changes task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_fetch_prune(path: String) -> Result<(), String> {
    use std::time::Duration;

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new("git")
            .args(["fetch", "--prune"])
            .current_dir(&path)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("git fetch --prune failed: {}", stderr.trim()))
        }
        Ok(Err(e)) => Err(format!("Failed to run git fetch: {e}")),
        Err(_) => Err("git fetch timed out after 10 seconds".to_string()),
    }
}

#[tauri::command]
pub async fn git_amend_commit(path: String, message: Option<String>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::git::git_amend_commit(Path::new(&path), message.as_deref())
    })
    .await
    .map_err(|e| format!("git_amend_commit task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_undo_last_commit(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::git_undo_last_commit(Path::new(&path)))
        .await
        .map_err(|e| format!("git_undo_last_commit task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_stash_push(path: String, include_untracked: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::git::git_stash_push(Path::new(&path), include_untracked)
    })
    .await
    .map_err(|e| format!("git_stash_push task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_stash_pop(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::git_stash_pop(Path::new(&path)))
        .await
        .map_err(|e| format!("git_stash_pop task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_discard_file(path: String, file: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::git_discard_file(Path::new(&path), &file))
        .await
        .map_err(|e| format!("git_discard_file task join failed: {e}"))?
}

#[tauri::command]
pub async fn git_log_entries(path: String, count: usize) -> Result<Vec<GitLogEntry>, String> {
    tokio::task::spawn_blocking(move || crate::git::git_log(Path::new(&path), count))
        .await
        .map_err(|e| format!("git_log_entries task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_commit_files(path: String, hash: String) -> Result<Vec<CommitFileEntry>, String> {
    tokio::task::spawn_blocking(move || crate::git::get_commit_files(Path::new(&path), &hash))
        .await
        .map_err(|e| format!("get_commit_files task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_git_branch_info(path: String) -> Result<GitBranchInfo, String> {
    tokio::task::spawn_blocking(move || crate::git::git_branch_info(Path::new(&path)))
        .await
        .map_err(|e| format!("get_git_branch_info task join failed: {e}"))?
}

#[tauri::command]
pub async fn list_branches(path: String, remote: bool) -> Result<Vec<String>, String> {
    tokio::task::spawn_blocking(move || crate::git::git_list_branches(Path::new(&path), remote))
        .await
        .map_err(|e| format!("list_branches task join failed: {e}"))?
}

#[tauri::command]
pub async fn list_branches_detailed(path: String) -> Result<Vec<crate::git::BranchDetail>, String> {
    tokio::task::spawn_blocking(move || crate::git::git_list_branches_detailed(Path::new(&path)))
        .await
        .map_err(|e| format!("list_branches_detailed task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_base_branch_diff(path: String, base_branch: String) -> Result<BaseBranchDiff, String> {
    tokio::task::spawn_blocking(move || {
        crate::git::git_diff_base_branch(Path::new(&path), &base_branch)
    })
    .await
    .map_err(|e| format!("get_base_branch_diff task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_base_branch_file_diff(path: String, base_branch: String, file: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        crate::git::git_diff_base_branch_file(Path::new(&path), &base_branch, &file)
    })
    .await
    .map_err(|e| format!("get_base_branch_file_diff task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_default_branch(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || crate::git::git_default_branch(Path::new(&path)))
        .await
        .map_err(|e| format!("get_default_branch task join failed: {e}"))?
}

#[tauri::command]
pub async fn create_worktree(path: String, branch: String, new_branch: bool, base: Option<String>, pr_number: Option<u32>) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        crate::git::git_create_worktree(Path::new(&path), &branch, new_branch, base.as_deref(), pr_number)
    })
    .await
    .map_err(|e| format!("create_worktree task join failed: {e}"))?
}

#[tauri::command]
pub async fn remove_worktree(worktree_path: String, branch: Option<String>, force: Option<bool>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::git::git_remove_worktree(Path::new(&worktree_path), branch.as_deref(), force.unwrap_or(false))
    })
    .await
    .map_err(|e| format!("remove_worktree task join failed: {e}"))?
}

#[tauri::command]
pub async fn list_worktrees(path: String) -> Result<Vec<WorktreeInfo>, String> {
    tokio::task::spawn_blocking(move || crate::git::git_list_worktrees(Path::new(&path)))
        .await
        .map_err(|e| format!("list_worktrees task join failed: {e}"))?
}

#[tauri::command]
pub async fn merge_branch(path: String, source_branch: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || crate::git::merge_branch(Path::new(&path), &source_branch))
        .await
        .map_err(|e| format!("merge_branch task join failed: {e}"))?
}

/// Run a fallible blocking operation on Tokio's blocking pool with an outer
/// deadline. Used to wrap multi-step git command sequences whose sync
/// `#[tauri::command]` form would pin a runtime IPC thread and freeze the
/// whole app if any subprocess got stuck (see github.rs:79 for the same
/// root cause and fix on the Review tab).
///
/// Contract:
///   - Happy path: returns whatever the closure returned.
///   - Closure returns Err: passed through verbatim.
///   - Deadline elapses before closure returns: returns
///     `Err("<name> timed out after <secs> seconds")` immediately, freeing
///     the IPC dispatcher. The blocking task keeps running on its thread
///     until its current syscall returns — Tokio cannot abort blocking
///     tasks, but the UI is no longer wedged. True kill-on-cancel would
///     require rewriting the inner sequence on `tokio::process::Command`
///     with `kill_on_drop(true)`; tracked as a follow-up.
async fn run_blocking_with_timeout<F, T>(
    timeout: std::time::Duration,
    name: &'static str,
    op: F,
) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tokio::time::timeout(timeout, tokio::task::spawn_blocking(op)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => Err(format!("{name} task failed: {e}")),
        Err(_) => Err(format!(
            "{name} timed out after {} seconds",
            timeout.as_secs()
        )),
    }
}

#[tauri::command]
pub async fn merge_into_base(
    path: String,
    base_branch: String,
    delete_source_branch: bool,
) -> Result<MergeIntoBaseResult, String> {
    run_blocking_with_timeout(
        std::time::Duration::from_secs(120),
        "merge_into_base",
        move || crate::git::merge_into_base(Path::new(&path), &base_branch, delete_source_branch),
    )
    .await
}

#[tauri::command]
pub async fn complete_merge_into_base(
    path: String,
    base_branch: String,
    temp_branch: String,
    source_branch: String,
    delete_source_branch: bool,
) -> Result<(), String> {
    run_blocking_with_timeout(
        std::time::Duration::from_secs(60),
        "complete_merge_into_base",
        move || {
            crate::git::complete_merge_into_base(
                Path::new(&path),
                &base_branch,
                &temp_branch,
                &source_branch,
                delete_source_branch,
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn abort_merge_into_base(
    path: String,
    source_branch: String,
    temp_branch: String,
) -> Result<(), String> {
    run_blocking_with_timeout(
        std::time::Duration::from_secs(30),
        "abort_merge_into_base",
        move || crate::git::abort_merge_into_base(Path::new(&path), &source_branch, &temp_branch),
    )
    .await
}

#[tauri::command]
pub async fn get_merge_state(path: String) -> Result<MergeState, String> {
    tokio::task::spawn_blocking(move || crate::git::get_merge_state(Path::new(&path)))
        .await
        .map_err(|e| format!("get_merge_state task join failed: {e}"))?
}

#[tauri::command]
pub async fn check_merge_conflicts(path: String, target_branch: String) -> Result<ConflictCheckResult, String> {
    tokio::task::spawn_blocking(move || {
        crate::git::check_merge_conflicts(Path::new(&path), &target_branch)
    })
    .await
    .map_err(|e| format!("check_merge_conflicts task join failed: {e}"))?
}

#[tauri::command]
pub async fn resolve_conflict_ours(path: String, file: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::resolve_conflict_ours(Path::new(&path), &file))
        .await
        .map_err(|e| format!("resolve_conflict_ours task join failed: {e}"))?
}

#[tauri::command]
pub async fn resolve_conflict_theirs(path: String, file: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::git::resolve_conflict_theirs(Path::new(&path), &file)
    })
    .await
    .map_err(|e| format!("resolve_conflict_theirs task join failed: {e}"))?
}

#[tauri::command]
pub async fn mark_conflict_resolved(path: String, file: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::git::mark_conflict_resolved(Path::new(&path), &file)
    })
    .await
    .map_err(|e| format!("mark_conflict_resolved task join failed: {e}"))?
}

#[tauri::command]
pub async fn abort_merge(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::abort_merge(Path::new(&path)))
        .await
        .map_err(|e| format!("abort_merge task join failed: {e}"))?
}

#[tauri::command]
pub async fn continue_merge(path: String, message: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::continue_merge(Path::new(&path), &message))
        .await
        .map_err(|e| format!("continue_merge task join failed: {e}"))?
}

#[tauri::command]
pub async fn create_resolver_branch(path: String, target_branch: String) -> Result<ResolverBranchInfo, String> {
    tokio::task::spawn_blocking(move || {
        crate::git::create_resolver_branch(Path::new(&path), &target_branch)
    })
    .await
    .map_err(|e| format!("create_resolver_branch task join failed: {e}"))?
}

#[tauri::command]
pub async fn apply_resolution(path: String, temp_branch: String, original_branch: String, message: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::git::apply_resolution(Path::new(&path), &temp_branch, &original_branch, &message)
    })
    .await
    .map_err(|e| format!("apply_resolution task join failed: {e}"))?
}

#[tauri::command]
pub async fn abort_resolution(path: String, temp_branch: String, original_branch: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::git::abort_resolution(Path::new(&path), &temp_branch, &original_branch)
    })
    .await
    .map_err(|e| format!("abort_resolution task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_resolution_diff(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || crate::git::get_resolution_diff(Path::new(&path)))
        .await
        .map_err(|e| format!("get_resolution_diff task join failed: {e}"))?
}

#[tauri::command]
pub async fn resolve_conflicts_with_agent(
    path: String,
    cli: String,
    model: Option<String>,
    strategy: String,
    files: Vec<String>,
) -> Result<String, String> {
    crate::ai::resolve_conflicts_with_agent(
        Path::new(&path),
        &cli,
        model.as_deref(),
        &strategy,
        &files,
    )
    .await
}

#[tauri::command]
pub async fn git_clone_repo(url: String, target_dir: String) -> Result<String, String> {
    use std::time::Duration;

    let result = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new("git")
            .args(["clone", &url, &target_dir])
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) if output.status.success() => Ok(target_dir),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("git clone failed: {}", stderr.trim()))
        }
        Ok(Err(e)) => Err(format!("Failed to run git clone: {e}")),
        Err(_) => Err("git clone timed out after 120 seconds".to_string()),
    }
}

#[cfg(test)]
mod tests {
    //! Tests for `run_blocking_with_timeout`, the helper that wraps the
    //! merge-into-base Tauri commands. These encode the contract the freeze
    //! fix depends on: if any of them ever starts failing, the freeze is
    //! back.
    use super::run_blocking_with_timeout;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn happy_path_returns_inner_ok() {
        let res: Result<i32, String> = run_blocking_with_timeout(
            Duration::from_secs(5),
            "happy",
            || Ok(42),
        )
        .await;
        assert_eq!(res, Ok(42));
    }

    #[tokio::test]
    async fn inner_error_passes_through_verbatim() {
        let res: Result<(), String> = run_blocking_with_timeout(
            Duration::from_secs(5),
            "inner_err",
            || Err("real git error: working tree has uncommitted changes".to_string()),
        )
        .await;
        assert_eq!(
            res,
            Err("real git error: working tree has uncommitted changes".to_string())
        );
    }

    #[tokio::test]
    async fn timeout_returns_named_error_and_does_not_block_caller() {
        let start = Instant::now();
        let res: Result<(), String> = run_blocking_with_timeout(
            Duration::from_millis(100),
            "merge_into_base",
            || {
                std::thread::sleep(Duration::from_secs(1));
                Ok(())
            },
        )
        .await;
        let elapsed = start.elapsed();

        assert!(res.is_err(), "expected timeout Err, got: {res:?}");
        let err = res.unwrap_err();
        assert!(err.contains("merge_into_base"), "got: {err}");
        assert!(err.contains("timed out"), "got: {err}");
        assert!(err.contains("0 seconds"), "got: {err}");
        assert!(
            elapsed < Duration::from_millis(800),
            "wrapper must return promptly after timeout, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn inner_panic_becomes_task_failed_error() {
        let res: Result<(), String> = run_blocking_with_timeout(
            Duration::from_secs(5),
            "panicker",
            || panic!("simulated bug"),
        )
        .await;
        let err = res.unwrap_err();
        assert!(
            err.contains("panicker") && err.contains("task failed"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn runtime_remains_responsive_during_long_blocking_call() {
        let blocking = tokio::spawn(run_blocking_with_timeout(
            Duration::from_secs(5),
            "long_blocker",
            || {
                std::thread::sleep(Duration::from_secs(1));
                Ok::<(), String>(())
            },
        ));

        let parallel_start = Instant::now();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let parallel_elapsed = parallel_start.elapsed();

        assert!(
            parallel_elapsed < Duration::from_millis(500),
            "parallel async task should not be starved, took {parallel_elapsed:?}"
        );

        let _ = blocking.await;
    }
}
