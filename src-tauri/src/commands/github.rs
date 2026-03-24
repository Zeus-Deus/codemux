use crate::github::{CheckInfo, GhStatus, PullRequestInfo};
use std::path::Path;

#[tauri::command]
pub fn check_gh_available() -> bool {
    crate::github::gh_available()
}

#[tauri::command]
pub fn check_gh_status() -> GhStatus {
    crate::github::check_gh_status()
}

#[tauri::command]
pub fn check_github_repo(path: String) -> bool {
    crate::github::is_github_repo(Path::new(&path))
}

#[tauri::command]
pub fn get_branch_pull_request(path: String) -> Result<Option<PullRequestInfo>, String> {
    crate::github::get_branch_pr(Path::new(&path))
}

#[tauri::command]
pub fn create_pull_request(
    path: String,
    title: String,
    body: String,
    base: Option<String>,
    draft: bool,
) -> Result<PullRequestInfo, String> {
    crate::github::create_pull_request(Path::new(&path), &title, &body, base.as_deref(), draft)
}

#[tauri::command]
pub fn list_pull_requests(path: String, state: String) -> Result<Vec<PullRequestInfo>, String> {
    crate::github::list_pull_requests(Path::new(&path), &state)
}

#[tauri::command]
pub fn merge_pull_request(path: String, pr_number: u32, method: String) -> Result<(), String> {
    crate::github::merge_pull_request(Path::new(&path), pr_number, &method)
}

#[tauri::command]
pub fn get_pull_request_checks(path: String) -> Result<Vec<CheckInfo>, String> {
    crate::github::get_pr_checks(Path::new(&path))
}

#[tauri::command]
pub fn get_pr_review_comments(path: String) -> Result<Vec<crate::github::ReviewComment>, String> {
    crate::github::get_pr_review_comments(Path::new(&path))
}
