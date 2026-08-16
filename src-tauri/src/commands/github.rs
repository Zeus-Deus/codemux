use crate::git_provider::{self, SourceControlProvider};
use crate::github::{
    CheckInfo, DeploymentInfo, GhStatus, GitHubIssue, InlineReviewComment, LinkedIssue,
    PullRequestInfo,
};
use crate::state::AppStateStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;

// Every command here resolves a provider and shells out via
// `Command::new("gh")` / `Command::new("git")`. They MUST run on Tokio's
// blocking pool: a sync `#[tauri::command]` runs on the GTK main thread,
// and any wedged subprocess freezes the whole UI hard enough that even
// window-close requests can't be processed. The fix is uniform — async
// command + `spawn_blocking`. Frontend `invoke()` returns a Promise
// either way.
//
// The command *names* stay GitHub-shaped (they are the stable Tauri
// boundary), but the implementations go through the provider registry so
// a second product only has to add an adapter. Data commands use the
// advisory resolution deliberately: see `git_provider::registry`.

/// Provider for a repo path, resolved on the blocking pool along with
/// the call itself — detection is a subprocess too.
fn provider_at(path: &str) -> Arc<dyn SourceControlProvider> {
    git_provider::provider_for_path_or_default(Path::new(path))
}

#[tauri::command]
pub async fn check_gh_available() -> bool {
    tokio::task::spawn_blocking(|| git_provider::github_provider().cli_available())
        .await
        .unwrap_or(false)
}

#[tauri::command]
pub async fn check_gh_status() -> GhStatus {
    tokio::task::spawn_blocking(|| git_provider::github_provider().auth_status())
        .await
        .unwrap_or(GhStatus::NotInstalled)
}

/// Preflight for the PR / issue UI: is there a hosting integration that
/// can serve this checkout? Strict resolution — an unclassified host
/// answers `false` here exactly as a non-`github.com` remote used to.
#[tauri::command]
pub async fn check_github_repo(path: String) -> bool {
    // 10s timeout retained because this is the specific freeze the user hit
    // (sync `git remote -v` on the GTK main thread, May 2026). On wedged
    // git, falling back to "no integration" is acceptable; the perfect
    // answer is not worth a permanently pinned IPC.
    let path_buf = PathBuf::from(path);
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            git_provider::repo_has_supported_provider(&path_buf)
        }),
    )
    .await
    {
        Ok(Ok(v)) => v,
        _ => false,
    }
}

#[tauri::command]
pub async fn get_branch_pull_request(path: String) -> Result<Option<PullRequestInfo>, String> {
    tokio::task::spawn_blocking(move || provider_at(&path).branch_pull_request(Path::new(&path)))
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
        provider_at(&path).create_pull_request(
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
        provider_at(&path).list_pull_requests(Path::new(&path), &state)
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
        provider_at(&path).get_pull_request(Path::new(&path), pr_number)
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
        provider_at(&path).pull_request_diff(Path::new(&path), pr_number, full)
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
        provider_at(&path).list_incoming_pull_requests(Path::new(&path), &base_branch)
    })
    .await
    .map_err(|e| format!("list_incoming_prs task join failed: {e}"))?
}

/// Every open pull request in one repository, for the Pull Requests
/// page. One call per project root; the page fans out and merges.
///
/// Same blocking-pool discipline as `list_incoming_prs`, and for the
/// same reason: this one asks for the CI rollup too, which is the
/// slowest field `gh pr list` serves.
#[tauri::command]
pub async fn list_prs_overview(path: String) -> Result<crate::github::PrsOverview, String> {
    tokio::task::spawn_blocking(move || provider_at(&path).pull_requests_overview(Path::new(&path)))
        .await
        .map_err(|e| format!("list_prs_overview task join failed: {e}"))?
}

#[tauri::command]
pub async fn merge_pull_request(
    path: String,
    pr_number: u32,
    method: String,
    delete_branch: Option<bool>,
    commit_title: Option<String>,
    commit_body: Option<String>,
) -> Result<(), String> {
    // `delete_branch` is optional so an older caller that only passes
    // (path, number, method) keeps the previous behaviour instead of
    // silently flipping to "keep the branch".
    let delete_branch = delete_branch.unwrap_or(true);
    tokio::task::spawn_blocking(move || {
        provider_at(&path).merge_pull_request(
            Path::new(&path),
            pr_number,
            &method,
            delete_branch,
            commit_title.as_deref(),
            commit_body.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("merge_pull_request task join failed: {e}"))?
}

#[tauri::command]
pub async fn close_pull_request(path: String, pr_number: u32) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_at(&path).close_pull_request(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("close_pull_request task join failed: {e}"))?
}

#[tauri::command]
pub async fn reopen_pull_request(path: String, pr_number: u32) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_at(&path).reopen_pull_request(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("reopen_pull_request task join failed: {e}"))?
}

#[tauri::command]
pub async fn set_pr_ready(path: String, pr_number: u32, ready: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_at(&path).set_pull_request_ready(Path::new(&path), pr_number, ready)
    })
    .await
    .map_err(|e| format!("set_pr_ready task join failed: {e}"))?
}

#[tauri::command]
pub async fn update_pull_request(
    path: String,
    pr_number: u32,
    title: Option<String>,
    body: Option<String>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_at(&path).update_pull_request(
            Path::new(&path),
            pr_number,
            title.as_deref(),
            body.as_deref(),
        )
    })
    .await
    .map_err(|e| format!("update_pull_request task join failed: {e}"))?
}

#[tauri::command]
pub async fn request_pr_review(
    path: String,
    pr_number: u32,
    reviewer: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_at(&path).request_pull_request_review(Path::new(&path), pr_number, &reviewer)
    })
    .await
    .map_err(|e| format!("request_pr_review task join failed: {e}"))?
}

/// Best-effort log tail for a failing check. Never an error the UI has
/// to show: an unavailable excerpt resolves to an empty string so the
/// failing-check card renders without it.
#[tauri::command]
pub async fn get_check_log_excerpt(
    path: String,
    pr_number: u32,
    check_name: String,
) -> Result<String, String> {
    let excerpt = tokio::task::spawn_blocking(move || {
        provider_at(&path).check_log_excerpt(Path::new(&path), pr_number, &check_name)
    })
    .await
    .map_err(|e| format!("get_check_log_excerpt task join failed: {e}"))?;
    Ok(excerpt.unwrap_or_default())
}

#[tauri::command]
pub async fn get_pull_request_checks(
    path: String,
    pr_number: Option<u32>,
) -> Result<Vec<CheckInfo>, String> {
    // Optional so the panel keeps its "checks for the branch I'm on"
    // call unchanged; the page passes the selected PR's number because
    // the checkout it reads from is usually standing somewhere else.
    tokio::task::spawn_blocking(move || {
        provider_at(&path).pull_request_checks(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pull_request_checks task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_pr_review_comments(
    path: String,
    pr_number: Option<u32>,
) -> Result<Vec<crate::github::ReviewComment>, String> {
    tokio::task::spawn_blocking(move || {
        provider_at(&path).pull_request_review_comments(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pr_review_comments task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_pr_inline_comments(path: String, pr_number: u32) -> Result<Vec<InlineReviewComment>, String> {
    tokio::task::spawn_blocking(move || {
        provider_at(&path).pull_request_inline_comments(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pr_inline_comments task join failed: {e}"))?
}

#[tauri::command]
pub async fn submit_pr_review(path: String, pr_number: u32, event: String, body: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_at(&path).submit_pull_request_review(Path::new(&path), pr_number, &event, &body)
    })
    .await
    .map_err(|e| format!("submit_pr_review task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_pr_deployments(path: String, pr_number: u32) -> Result<Vec<DeploymentInfo>, String> {
    tokio::task::spawn_blocking(move || {
        provider_at(&path).pull_request_deployments(Path::new(&path), pr_number)
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
        provider_at(&cwd).list_issues(Path::new(&cwd), search.as_deref())
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
        provider_at(&cwd).get_issue(Path::new(&cwd), issue_number)
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
        provider_at(&cwd).get_issue_fresh(Path::new(&cwd), issue_number)
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
        provider_at(&cwd).get_issue_fresh(Path::new(&cwd), issue_number)
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
    let (detected, lookup) = tokio::task::spawn_blocking(move || {
        // Advisory resolution, like every other data command here — a
        // manual refresh must keep working on a host detection can't name
        // but the CLI can resolve.
        let detected = git_provider::detect_provider(Path::new(&cwd_for_pr));
        let lookup = git_provider::provider_for_detection_or_default(&detected)
            .workspace_pull_request(Path::new(&cwd_for_pr));
        (detected, lookup)
    })
    .await
    .map_err(|e| format!("refresh_workspace_pr task join failed: {e}"))?;
    state.update_workspace_provider_kind(
        &workspace_id,
        git_provider::provider_kind_field(&detected),
    );
    // Decision matrix, shared with both background pollers via
    // `branch_pr_outcome`: match → write, successful empty → clear, lookup
    // error (or detached HEAD) → leave the stored info untouched. A failed
    // lookup is deliberately not an `Err` back to the frontend: a manual
    // refresh during a rebase should be a no-op, not an error toast.
    match crate::github::branch_pr_outcome(lookup) {
        crate::github::BranchPrOutcome::Write(pr) => {
            state.update_workspace_pr_info(
                &workspace_id,
                Some(pr.number),
                Some(pr.display_state()),
                Some(pr.url),
                pr.head_branch,
            );
        }
        crate::github::BranchPrOutcome::Clear => {
            state.update_workspace_pr_info(&workspace_id, None, None, None, None);
        }
        crate::github::BranchPrOutcome::Preserve => {}
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
        provider_at(&path).list_issues(Path::new(&path), search.as_deref())
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
        provider_at(&path).get_issue(Path::new(&path), issue_number)
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
