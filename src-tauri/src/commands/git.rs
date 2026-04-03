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
pub fn create_worktree(path: String, branch: String, new_branch: bool, base: Option<String>) -> Result<String, String> {
    crate::git::git_create_worktree(Path::new(&path), &branch, new_branch, base.as_deref())
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

#[tauri::command]
pub fn merge_into_base(path: String, base_branch: String, delete_source_branch: bool) -> Result<MergeIntoBaseResult, String> {
    crate::git::merge_into_base(Path::new(&path), &base_branch, delete_source_branch)
}

#[tauri::command]
pub fn complete_merge_into_base(
    path: String,
    base_branch: String,
    temp_branch: String,
    source_branch: String,
    delete_source_branch: bool,
) -> Result<(), String> {
    crate::git::complete_merge_into_base(
        Path::new(&path), &base_branch, &temp_branch, &source_branch, delete_source_branch,
    )
}

#[tauri::command]
pub fn abort_merge_into_base(
    path: String,
    source_branch: String,
    temp_branch: String,
) -> Result<(), String> {
    crate::git::abort_merge_into_base(Path::new(&path), &source_branch, &temp_branch)
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
