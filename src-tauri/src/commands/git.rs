use crate::git::{GitBranchInfo, GitDiffStat, GitFileStatus, GitLogEntry, WorktreeInfo};
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
