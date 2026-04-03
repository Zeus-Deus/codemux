use crate::github::{
    CheckInfo, DeploymentInfo, GhStatus, GitHubIssue, InlineReviewComment, LinkedIssue,
    PullRequestInfo,
};
use crate::state::AppStateStore;
use std::path::Path;
use tauri::State;

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

#[tauri::command]
pub fn get_pr_inline_comments(path: String, pr_number: u32) -> Result<Vec<InlineReviewComment>, String> {
    crate::github::get_pr_inline_comments(Path::new(&path), pr_number)
}

#[tauri::command]
pub fn submit_pr_review(path: String, pr_number: u32, event: String, body: String) -> Result<(), String> {
    crate::github::submit_pr_review(Path::new(&path), pr_number, &event, &body)
}

#[tauri::command]
pub fn get_pr_deployments(path: String, pr_number: u32) -> Result<Vec<DeploymentInfo>, String> {
    crate::github::get_pr_deployments(Path::new(&path), pr_number)
}

// ── GitHub Issues ──

#[tauri::command]
pub fn list_github_issues(
    state: State<'_, AppStateStore>,
    workspace_id: String,
    search: Option<String>,
) -> Result<Vec<GitHubIssue>, String> {
    let cwd = resolve_workspace_cwd(&state, &workspace_id)?;
    crate::github::list_github_issues(Path::new(&cwd), search.as_deref())
}

#[tauri::command]
pub fn get_github_issue(
    state: State<'_, AppStateStore>,
    workspace_id: String,
    issue_number: u64,
) -> Result<GitHubIssue, String> {
    let cwd = resolve_workspace_cwd(&state, &workspace_id)?;
    crate::github::get_github_issue(Path::new(&cwd), issue_number)
}

#[tauri::command]
pub fn link_workspace_issue(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    issue_number: u64,
) -> Result<(), String> {
    let cwd = resolve_workspace_cwd(&state, &workspace_id)?;
    let issue = crate::github::get_github_issue(Path::new(&cwd), issue_number)?;
    let linked = LinkedIssue {
        number: issue.number,
        title: issue.title,
        state: issue.state,
        labels: issue.labels,
    };
    state.link_workspace_issue(&workspace_id, linked);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn unlink_workspace_issue(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    state.unlink_workspace_issue(&workspace_id);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn refresh_workspace_issue(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    let (cwd, issue_number) = {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found: {workspace_id}"))?;
        let num = ws
            .linked_issue
            .as_ref()
            .ok_or_else(|| "No linked issue on this workspace".to_string())?
            .number;
        (ws.cwd.clone(), num)
    };

    let issue = crate::github::get_github_issue(Path::new(&cwd), issue_number)?;
    let linked = LinkedIssue {
        number: issue.number,
        title: issue.title,
        state: issue.state,
        labels: issue.labels,
    };
    state.link_workspace_issue(&workspace_id, linked);
    crate::state::emit_app_state(&app);
    Ok(())
}

/// List issues by repo path directly (no workspace needed — for use before workspace exists).
#[tauri::command]
pub fn list_github_issues_by_path(
    path: String,
    search: Option<String>,
) -> Result<Vec<GitHubIssue>, String> {
    crate::github::list_github_issues(Path::new(&path), search.as_deref())
}

/// Get a single issue by repo path directly (no workspace needed).
#[tauri::command]
pub fn get_github_issue_by_path(
    path: String,
    issue_number: u64,
) -> Result<GitHubIssue, String> {
    crate::github::get_github_issue(Path::new(&path), issue_number)
}

#[tauri::command]
pub fn suggest_issue_branch_name(issue_number: u64, issue_title: String) -> Result<String, String> {
    Ok(crate::github::suggest_branch_name(issue_number, &issue_title))
}

/// Resolve workspace cwd from workspace_id (or fall back to project_root).
fn resolve_workspace_cwd(state: &AppStateStore, workspace_id: &str) -> Result<String, String> {
    let snapshot = state.snapshot();
    let ws = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .ok_or_else(|| format!("No workspace found: {workspace_id}"))?;
    // Prefer project_root (main repo) since `gh issue` needs the main repo, not a worktree.
    Ok(ws
        .project_root
        .clone()
        .unwrap_or_else(|| ws.cwd.clone()))
}
