use crate::github::{
    CheckInfo, DeploymentInfo, GhStatus, GitHubIssue, InlineReviewComment, LinkedIssue,
    PullRequestInfo,
};
use crate::state::AppStateStore;
use std::path::{Path, PathBuf};
use tauri::State;

// Every command here that delegates to `crate::github::*` or
// `crate::github_cache::*` shells out via `Command::new("gh")` or
// `Command::new("git")`. They MUST run on Tokio's blocking pool: a sync
// `#[tauri::command]` runs on the GTK main thread, and any wedged
// subprocess freezes the whole UI hard enough that even window-close
// requests can't be processed. The fix is uniform — async command +
// `spawn_blocking`. Frontend `invoke()` returns a Promise either way.

#[tauri::command]
pub async fn check_gh_available() -> bool {
    tokio::task::spawn_blocking(crate::github::gh_available)
        .await
        .unwrap_or(false)
}

#[tauri::command]
pub async fn check_gh_status() -> GhStatus {
    tokio::task::spawn_blocking(crate::github::check_gh_status)
        .await
        .unwrap_or(GhStatus::NotInstalled)
}

#[tauri::command]
pub async fn check_github_repo(path: String) -> bool {
    // 10s timeout retained because this is the specific freeze the user hit
    // (sync `git remote -v` on the GTK main thread, May 2026). On wedged
    // git, falling back to "not a github repo" is acceptable; the perfect
    // answer is not worth a permanently pinned IPC.
    let path_buf = PathBuf::from(path);
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || crate::github::is_github_repo(&path_buf)),
    )
    .await
    {
        Ok(Ok(v)) => v,
        _ => false,
    }
}

#[tauri::command]
pub async fn get_branch_pull_request(path: String) -> Result<Option<PullRequestInfo>, String> {
    tokio::task::spawn_blocking(move || crate::github::get_branch_pr(Path::new(&path)))
        .await
        .map_err(|e| format!("get_branch_pull_request task join failed: {e}"))?
}

#[tauri::command]
pub async fn create_pull_request(
    path: String,
    title: String,
    body: String,
    base: Option<String>,
    draft: bool,
) -> Result<PullRequestInfo, String> {
    tokio::task::spawn_blocking(move || {
        crate::github::create_pull_request(
            Path::new(&path),
            &title,
            &body,
            base.as_deref(),
            draft,
        )
    })
    .await
    .map_err(|e| format!("create_pull_request task join failed: {e}"))?
}

#[tauri::command]
pub async fn list_pull_requests(path: String, state: String) -> Result<Vec<PullRequestInfo>, String> {
    tokio::task::spawn_blocking(move || {
        crate::github_cache::cached_list_pull_requests(Path::new(&path), &state)
    })
    .await
    .map_err(|e| format!("list_pull_requests task join failed: {e}"))?
}

/// Stage 5 — single PR detail (body + comments) by number, by repo
/// path. Path-based so the chat composer can call without a
/// workspace handle (mirrors `get_github_issue_by_path`).
#[tauri::command]
pub async fn get_github_pr_by_path(
    path: String,
    pr_number: u32,
) -> Result<PullRequestInfo, String> {
    tokio::task::spawn_blocking(move || {
        crate::github_cache::cached_get_pull_request(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_github_pr_by_path task join failed: {e}"))?
}

/// Stage 5 — PR diff. `full=false` → `--name-only`; `full=true` →
/// unified diff truncated at 100 KB. Cached separately per (number,
/// full) so flipping the toggle doesn't blow the other variant out
/// of the cache.
#[tauri::command]
pub async fn get_github_pr_diff_by_path(
    path: String,
    pr_number: u32,
    full: bool,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        crate::github_cache::cached_get_pr_diff(Path::new(&path), pr_number, full)
    })
    .await
    .map_err(|e| format!("get_github_pr_diff_by_path task join failed: {e}"))?
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
pub async fn merge_pull_request(path: String, pr_number: u32, method: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::github::merge_pull_request(Path::new(&path), pr_number, &method)
    })
    .await
    .map_err(|e| format!("merge_pull_request task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_pull_request_checks(path: String) -> Result<Vec<CheckInfo>, String> {
    tokio::task::spawn_blocking(move || crate::github::get_pr_checks(Path::new(&path)))
        .await
        .map_err(|e| format!("get_pull_request_checks task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_pr_review_comments(path: String) -> Result<Vec<crate::github::ReviewComment>, String> {
    tokio::task::spawn_blocking(move || crate::github::get_pr_review_comments(Path::new(&path)))
        .await
        .map_err(|e| format!("get_pr_review_comments task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_pr_inline_comments(path: String, pr_number: u32) -> Result<Vec<InlineReviewComment>, String> {
    tokio::task::spawn_blocking(move || {
        crate::github::get_pr_inline_comments(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pr_inline_comments task join failed: {e}"))?
}

#[tauri::command]
pub async fn submit_pr_review(path: String, pr_number: u32, event: String, body: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::github::submit_pr_review(Path::new(&path), pr_number, &event, &body)
    })
    .await
    .map_err(|e| format!("submit_pr_review task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_pr_deployments(path: String, pr_number: u32) -> Result<Vec<DeploymentInfo>, String> {
    tokio::task::spawn_blocking(move || {
        crate::github::get_pr_deployments(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pr_deployments task join failed: {e}"))?
}

// ── GitHub Issues ──

#[tauri::command]
pub async fn list_github_issues(
    state: State<'_, AppStateStore>,
    workspace_id: String,
    search: Option<String>,
) -> Result<Vec<GitHubIssue>, String> {
    let cwd = resolve_workspace_cwd(&state, &workspace_id)?;
    tokio::task::spawn_blocking(move || {
        crate::github_cache::cached_list_issues(Path::new(&cwd), search.as_deref())
    })
    .await
    .map_err(|e| format!("list_github_issues task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_github_issue(
    state: State<'_, AppStateStore>,
    workspace_id: String,
    issue_number: u64,
) -> Result<GitHubIssue, String> {
    let cwd = resolve_workspace_cwd(&state, &workspace_id)?;
    tokio::task::spawn_blocking(move || {
        crate::github_cache::cached_get_issue(Path::new(&cwd), issue_number)
    })
    .await
    .map_err(|e| format!("get_github_issue task join failed: {e}"))?
}

#[tauri::command]
pub async fn link_workspace_issue<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    issue_number: u64,
) -> Result<(), String> {
    let cwd = resolve_workspace_cwd(&state, &workspace_id)?;
    let issue = tokio::task::spawn_blocking(move || {
        crate::github::get_github_issue(Path::new(&cwd), issue_number)
    })
    .await
    .map_err(|e| format!("link_workspace_issue task join failed: {e}"))??;
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
pub fn unlink_workspace_issue<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    state.unlink_workspace_issue(&workspace_id);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub async fn refresh_workspace_issue<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
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

    let issue = tokio::task::spawn_blocking(move || {
        crate::github::get_github_issue(Path::new(&cwd), issue_number)
    })
    .await
    .map_err(|e| format!("refresh_workspace_issue task join failed: {e}"))??;
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
pub async fn refresh_workspace_pr<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
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

    let cwd_for_pr = cwd.clone();
    let pr_info = tokio::task::spawn_blocking(move || {
        crate::github::get_branch_pr(Path::new(&cwd_for_pr))
    })
    .await
    .map_err(|e| format!("refresh_workspace_pr task join failed: {e}"))??;
    // Decision matrix (mirrors the background pollers):
    //   - PR found & passes the historical-SHA gate → write fresh info
    //   - PR found but gated out (CLOSED/MERGED with diverged SHA) → clear
    //   - `gh pr view` returned nothing AND persisted state is historical
    //     → clear stale leftover from when the PR was active
    //   - `gh pr view` returned nothing AND persisted state is OPEN/DRAFT
    //     → preserve, protecting the fork-branch `gh pr checkout` case
    match pr_info.as_ref() {
        Some(pr) => {
            let cwd_for_head = cwd.clone();
            let head_sha = tokio::task::spawn_blocking(move || {
                crate::github::get_head_sha(Path::new(&cwd_for_head))
            })
            .await
            .ok()
            .flatten();
            if crate::github::should_show_branch_pr(pr, head_sha.as_deref()) {
                state.update_workspace_pr_info(
                    &workspace_id,
                    Some(pr.number),
                    Some(pr.display_state()),
                    Some(pr.url.clone()),
                );
            } else {
                state.update_workspace_pr_info(&workspace_id, None, None, None);
            }
        }
        None => {
            let persisted_state = {
                let snapshot = state.snapshot();
                snapshot
                    .workspaces
                    .iter()
                    .find(|w| w.workspace_id.0 == workspace_id)
                    .and_then(|w| w.pr_state.clone())
            };
            if persisted_state
                .as_deref()
                .map(crate::github::is_historical_pr_state)
                .unwrap_or(false)
            {
                state.update_workspace_pr_info(&workspace_id, None, None, None);
            }
        }
    }
    crate::state::emit_app_state(&app);
    Ok(())
}

/// List issues by repo path directly (no workspace needed — for use before workspace exists).
#[tauri::command]
pub async fn list_github_issues_by_path(
    path: String,
    search: Option<String>,
) -> Result<Vec<GitHubIssue>, String> {
    tokio::task::spawn_blocking(move || {
        crate::github_cache::cached_list_issues(Path::new(&path), search.as_deref())
    })
    .await
    .map_err(|e| format!("list_github_issues_by_path task join failed: {e}"))?
}

/// Get a single issue by repo path directly (no workspace needed).
#[tauri::command]
pub async fn get_github_issue_by_path(
    path: String,
    issue_number: u64,
) -> Result<GitHubIssue, String> {
    tokio::task::spawn_blocking(move || {
        crate::github_cache::cached_get_issue(Path::new(&path), issue_number)
    })
    .await
    .map_err(|e| format!("get_github_issue_by_path task join failed: {e}"))?
}

#[tauri::command]
pub fn suggest_issue_branch_name(issue_number: u64, issue_title: String) -> Result<String, String> {
    // Pure in-memory string formatting — safe on the main thread.
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
