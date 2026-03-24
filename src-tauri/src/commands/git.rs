use crate::git::{ConflictCheckResult, GitBranchInfo, GitDiffStat, GitFileStatus, GitLogEntry, MergeState, ResolverBranchInfo, WorktreeInfo};
use std::path::Path;

#[tauri::command]
pub fn get_git_status(path: String) -> Result<Vec<GitFileStatus>, String> {
    let p = Path::new(&path);
    eprintln!("[git_status] path={}, exists={}, is_dir={}", path, p.exists(), p.is_dir());
    let result = crate::git::git_status(p);
    match &result {
        Ok(files) => eprintln!("[git_status] found {} files", files.len()),
        Err(e) => eprintln!("[git_status] error: {}", e),
    }
    result
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
pub fn git_discard_file(path: String, file: String) -> Result<(), String> {
    crate::git::git_discard_file(Path::new(&path), &file)
}

#[tauri::command]
pub fn git_log_entries(path: String, count: usize) -> Result<Vec<GitLogEntry>, String> {
    crate::git::git_log(Path::new(&path), count)
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
pub fn create_worktree(path: String, branch: String, new_branch: bool, base: Option<String>) -> Result<String, String> {
    crate::git::git_create_worktree(Path::new(&path), &branch, new_branch, base.as_deref())
}

#[tauri::command]
pub fn remove_worktree(worktree_path: String, branch: Option<String>) -> Result<(), String> {
    crate::git::git_remove_worktree(Path::new(&worktree_path), branch.as_deref())
}

#[tauri::command]
pub fn list_worktrees(path: String) -> Result<Vec<WorktreeInfo>, String> {
    crate::git::git_list_worktrees(Path::new(&path))
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
