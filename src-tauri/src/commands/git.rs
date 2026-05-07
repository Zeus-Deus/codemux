use crate::git::{BaseBranchDiff, CommitFileEntry, ConflictCheckResult, GitBranchInfo, GitDiffStat, GitFileStatus, GitLogEntry, MergeIntoBaseResult, MergeState, ResolverBranchInfo, WorktreeInfo};
use std::path::Path;

#[tauri::command]
pub fn check_is_git_repo(path: String) -> bool {
    crate::git::is_git_repo(Path::new(&path))
}

#[tauri::command]
pub fn init_git_repo(path: String) -> Result<String, String> {
    crate::git::git_init_repo(Path::new(&path))
}

#[tauri::command]
pub fn create_empty_repo(parent_dir: String, name: String) -> Result<String, String> {
    let repo_path = Path::new(&parent_dir).join(&name);
    std::fs::create_dir_all(&repo_path)
        .map_err(|e| format!("Failed to create directory: {e}"))?;
    crate::git::git_init_repo(&repo_path)?;
    Ok(repo_path.display().to_string())
}

#[tauri::command]
pub fn get_git_status(path: String) -> Result<Vec<GitFileStatus>, String> {
    crate::git::git_status(Path::new(&path))
}

#[tauri::command]
pub fn get_git_diff(path: String, file: String, staged: bool) -> Result<String, String> {
    crate::git::git_diff(Path::new(&path), &file, staged)
}

#[tauri::command]
pub fn get_git_diff_stat(path: String) -> Result<GitDiffStat, String> {
    crate::git::git_diff_stat(Path::new(&path))
}

#[tauri::command]
pub fn git_stage_files(path: String, files: Vec<String>) -> Result<(), String> {
    crate::git::git_stage(Path::new(&path), &files)
}

#[tauri::command]
pub fn git_unstage_files(path: String, files: Vec<String>) -> Result<(), String> {
    crate::git::git_unstage(Path::new(&path), &files)
}

#[tauri::command]
pub fn git_commit_changes(path: String, message: String) -> Result<(), String> {
    crate::git::git_commit(Path::new(&path), &message)
}

#[tauri::command]
pub fn git_push_changes(path: String, set_upstream: bool) -> Result<(), String> {
    crate::git::git_push(Path::new(&path), set_upstream)
}

#[tauri::command]
pub fn git_pull_changes(path: String) -> Result<(), String> {
    crate::git::git_pull(Path::new(&path))
}

#[tauri::command]
pub fn git_fetch_changes(path: String) -> Result<(), String> {
    crate::git::git_fetch(Path::new(&path))
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
pub fn git_stash_push(path: String, include_untracked: bool) -> Result<(), String> {
    crate::git::git_stash_push(Path::new(&path), include_untracked)
}

#[tauri::command]
pub fn git_stash_pop(path: String) -> Result<(), String> {
    crate::git::git_stash_pop(Path::new(&path))
}

#[tauri::command]
pub fn git_discard_file(path: String, file: String) -> Result<(), String> {
    crate::git::git_discard_file(Path::new(&path), &file)
}

#[tauri::command]
pub fn git_log_entries(path: String, count: usize) -> Result<Vec<GitLogEntry>, String> {
    crate::git::git_log(Path::new(&path), count)
}

#[tauri::command]
pub fn get_commit_files(path: String, hash: String) -> Result<Vec<CommitFileEntry>, String> {
    crate::git::get_commit_files(Path::new(&path), &hash)
}

#[tauri::command]
pub fn get_git_branch_info(path: String) -> Result<GitBranchInfo, String> {
    crate::git::git_branch_info(Path::new(&path))
}

#[tauri::command]
pub fn list_branches(path: String, remote: bool) -> Result<Vec<String>, String> {
    crate::git::git_list_branches(Path::new(&path), remote)
}

#[tauri::command]
pub fn list_branches_detailed(path: String) -> Result<Vec<crate::git::BranchDetail>, String> {
    crate::git::git_list_branches_detailed(Path::new(&path))
}

#[tauri::command]
pub fn get_base_branch_diff(path: String, base_branch: String) -> Result<BaseBranchDiff, String> {
    crate::git::git_diff_base_branch(Path::new(&path), &base_branch)
}

#[tauri::command]
pub fn get_base_branch_file_diff(path: String, base_branch: String, file: String) -> Result<String, String> {
    crate::git::git_diff_base_branch_file(Path::new(&path), &base_branch, &file)
}

#[tauri::command]
pub fn get_default_branch(path: String) -> Result<String, String> {
    crate::git::git_default_branch(Path::new(&path))
}

#[tauri::command]
pub fn create_worktree(path: String, branch: String, new_branch: bool, base: Option<String>, pr_number: Option<u32>) -> Result<String, String> {
    crate::git::git_create_worktree(Path::new(&path), &branch, new_branch, base.as_deref(), pr_number)
}

#[tauri::command]
pub fn remove_worktree(worktree_path: String, branch: Option<String>, force: Option<bool>) -> Result<(), String> {
    crate::git::git_remove_worktree(Path::new(&worktree_path), branch.as_deref(), force.unwrap_or(false))
}

#[tauri::command]
pub fn list_worktrees(path: String) -> Result<Vec<WorktreeInfo>, String> {
    crate::git::git_list_worktrees(Path::new(&path))
}

#[tauri::command]
pub fn merge_branch(path: String, source_branch: String) -> Result<String, String> {
    crate::git::merge_branch(Path::new(&path), &source_branch)
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
pub fn get_merge_state(path: String) -> Result<MergeState, String> {
    crate::git::get_merge_state(Path::new(&path))
}

#[tauri::command]
pub fn check_merge_conflicts(path: String, target_branch: String) -> Result<ConflictCheckResult, String> {
    crate::git::check_merge_conflicts(Path::new(&path), &target_branch)
}

#[tauri::command]
pub fn resolve_conflict_ours(path: String, file: String) -> Result<(), String> {
    crate::git::resolve_conflict_ours(Path::new(&path), &file)
}

#[tauri::command]
pub fn resolve_conflict_theirs(path: String, file: String) -> Result<(), String> {
    crate::git::resolve_conflict_theirs(Path::new(&path), &file)
}

#[tauri::command]
pub fn mark_conflict_resolved(path: String, file: String) -> Result<(), String> {
    crate::git::mark_conflict_resolved(Path::new(&path), &file)
}

#[tauri::command]
pub fn abort_merge(path: String) -> Result<(), String> {
    crate::git::abort_merge(Path::new(&path))
}

#[tauri::command]
pub fn continue_merge(path: String, message: String) -> Result<(), String> {
    crate::git::continue_merge(Path::new(&path), &message)
}

#[tauri::command]
pub fn create_resolver_branch(path: String, target_branch: String) -> Result<ResolverBranchInfo, String> {
    crate::git::create_resolver_branch(Path::new(&path), &target_branch)
}

#[tauri::command]
pub fn apply_resolution(path: String, temp_branch: String, original_branch: String, message: String) -> Result<(), String> {
    crate::git::apply_resolution(Path::new(&path), &temp_branch, &original_branch, &message)
}

#[tauri::command]
pub fn abort_resolution(path: String, temp_branch: String, original_branch: String) -> Result<(), String> {
    crate::git::abort_resolution(Path::new(&path), &temp_branch, &original_branch)
}

#[tauri::command]
pub fn get_resolution_diff(path: String) -> Result<String, String> {
    crate::git::get_resolution_diff(Path::new(&path))
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
        // Closure returns Ok quickly → wrapper returns the same Ok unchanged.
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
        // Closure returns Err → wrapper passes the exact same Err through,
        // not a "task failed" wrapping. (Critical for surfacing real git
        // errors like "uncommitted changes" to the UI.)
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
        // This is the test that proves the freeze is fixed: a closure that
        // would block far longer than the deadline must NOT pin the caller.
        // We assert (a) the wrapper returns Err with the timeout message
        // mentioning the operation name and the deadline in seconds, and
        // (b) the call returns within a small bounded wall-clock window.
        // Without `tokio::time::timeout` the test would hang for 30s.
        let start = Instant::now();
        // Inner sleep (1s) > deadline (100ms), so timeout MUST fire. Kept
        // short on purpose: tokio's runtime waits for blocking threads on
        // shutdown, so a long inner sleep would inflate the CI suite by
        // that duration even though the assertion already passed.
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
        assert!(
            err.contains("merge_into_base"),
            "timeout error should name the operation, got: {err}"
        );
        assert!(
            err.contains("timed out"),
            "timeout error should say 'timed out', got: {err}"
        );
        assert!(
            err.contains("0 seconds"),
            "timeout error should report the configured deadline (0s for 100ms), got: {err}"
        );
        // The wrapper must return promptly after the 100ms deadline; if the
        // freeze regression came back, this would be ≥1s.
        assert!(
            elapsed < Duration::from_millis(800),
            "wrapper must return promptly after timeout, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn inner_panic_becomes_task_failed_error() {
        // If the inner closure panics, spawn_blocking yields a JoinError;
        // the wrapper must convert it into a `<name> task failed: <err>`
        // string instead of unwinding the runtime task. This matters
        // because a panicking git helper would otherwise crash the IPC
        // worker and the user would see an opaque IPC failure.
        let res: Result<(), String> = run_blocking_with_timeout(
            Duration::from_secs(5),
            "panicker",
            || panic!("simulated bug"),
        )
        .await;
        let err = res.unwrap_err();
        assert!(
            err.contains("panicker") && err.contains("task failed"),
            "panic should surface as '<name> task failed: ...', got: {err}"
        );
    }

    #[tokio::test]
    async fn runtime_remains_responsive_during_long_blocking_call() {
        // The whole point of moving the work off the IPC thread: while
        // a long blocking call is in flight, OTHER async tasks can still
        // make progress. We start a 1s blocking task on the wrapper and
        // a parallel async sleep of 50ms; the parallel sleep must finish
        // promptly, well before the blocking task does.
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
            "parallel async task should not be starved by blocking work, took {parallel_elapsed:?}"
        );

        // Drain the blocking task so the test's runtime shuts down cleanly.
        let _ = blocking.await;
    }
}
