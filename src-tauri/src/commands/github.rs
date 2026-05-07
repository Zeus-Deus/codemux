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
    crate::github_cache::cached_list_pull_requests(Path::new(&path), &state)
}

/// Stage 5 — single PR detail (body + comments) by number, by repo
/// path. Path-based so the chat composer can call without a
/// workspace handle (mirrors `get_github_issue_by_path`).
#[tauri::command]
pub fn get_github_pr_by_path(
    path: String,
    pr_number: u32,
) -> Result<PullRequestInfo, String> {
    crate::github_cache::cached_get_pull_request(Path::new(&path), pr_number)
}

/// Stage 5 — PR diff. `full=false` → `--name-only`; `full=true` →
/// unified diff truncated at 100 KB. Cached separately per (number,
/// full) so flipping the toggle doesn't blow the other variant out
/// of the cache.
#[tauri::command]
pub fn get_github_pr_diff_by_path(
    path: String,
    pr_number: u32,
    full: bool,
) -> Result<String, String> {
    crate::github_cache::cached_get_pr_diff(Path::new(&path), pr_number, full)
}

#[tauri::command]
pub async fn list_incoming_prs(
    path: String,
    base_branch: String,
) -> Result<Vec<crate::github::IncomingPrItem>, String> {
    // Run the blocking `gh pr list` shell-out on the blocking pool so it
    // never holds up the IPC runtime. On a repo with thousands of PRs
    // the call can take several seconds; a sync `#[tauri::command]`
    // would pin a runtime thread for that whole duration and was the
    // root cause of the Review-tab freeze the user reported.
    tokio::task::spawn_blocking(move || {
        crate::github::list_incoming_prs(Path::new(&path), &base_branch)
    })
    .await
    .map_err(|e| format!("list_incoming_prs task join failed: {e}"))?
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
    crate::github_cache::cached_list_issues(Path::new(&cwd), search.as_deref())
}

#[tauri::command]
pub fn get_github_issue(
    state: State<'_, AppStateStore>,
    workspace_id: String,
    issue_number: u64,
) -> Result<GitHubIssue, String> {
    let cwd = resolve_workspace_cwd(&state, &workspace_id)?;
    crate::github_cache::cached_get_issue(Path::new(&cwd), issue_number)
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

#[tauri::command]
pub fn refresh_workspace_pr(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    let cwd = {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found: {workspace_id}"))?;
        // Match frontend logic: worktree_path ?? cwd (pr-panel.tsx:328)
        ws.worktree_path.clone().unwrap_or_else(|| ws.cwd.clone())
    };

    let pr_info = crate::github::get_branch_pr(Path::new(&cwd))?;
    // Only overwrite if we found a PR — don't clear a pr_number that was
    // explicitly set during PR-checkout (fork branches where gh can't resolve).
    if pr_info.is_some() {
        let pr_number = pr_info.as_ref().map(|p| p.number);
        let pr_state = pr_info.as_ref().map(|p| p.display_state());
        let pr_url = pr_info.as_ref().map(|p| p.url.clone());
        state.update_workspace_pr_info(&workspace_id, pr_number, pr_state, pr_url);
    }
    crate::state::emit_app_state(&app);
    Ok(())
}

/// List issues by repo path directly (no workspace needed — for use before workspace exists).
#[tauri::command]
pub fn list_github_issues_by_path(
    path: String,
    search: Option<String>,
) -> Result<Vec<GitHubIssue>, String> {
    crate::github_cache::cached_list_issues(Path::new(&path), search.as_deref())
}

/// Get a single issue by repo path directly (no workspace needed).
#[tauri::command]
pub fn get_github_issue_by_path(
    path: String,
    issue_number: u64,
) -> Result<GitHubIssue, String> {
    crate::github_cache::cached_get_issue(Path::new(&path), issue_number)
}

#[tauri::command]
pub fn suggest_issue_branch_name(issue_number: u64, issue_title: String) -> Result<String, String> {
    Ok(crate::github::suggest_branch_name(issue_number, &issue_title))
}

/// Resolve workspace cwd from workspace_id (or fall back to project_root).
/// Validates the returned path is inside a git repository to avoid using
/// stale or incorrect project_root values from ghost workspaces.
fn resolve_workspace_cwd(state: &AppStateStore, workspace_id: &str) -> Result<String, String> {
    let snapshot = state.snapshot();
    let ws = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .ok_or_else(|| format!("No workspace found: {workspace_id}"))?;

    // Prefer project_root (main repo) since `gh issue` needs the main repo, not a worktree.
    // But validate it's actually a git repo — ghost workspaces can have stale project_root.
    if let Some(ref root) = ws.project_root {
        if crate::config::workspace_config::find_git_root(std::path::Path::new(root)).is_some() {
            return Ok(root.clone());
        }
    }

    // Fall back to cwd if project_root is missing or invalid
    let cwd = &ws.cwd;
    if crate::config::workspace_config::find_git_root(std::path::Path::new(cwd)).is_some() {
        return Ok(cwd.clone());
    }

    Err(format!(
        "Workspace {workspace_id} has no valid git repository path (project_root: {:?}, cwd: {})",
        ws.project_root, ws.cwd
    ))
}
