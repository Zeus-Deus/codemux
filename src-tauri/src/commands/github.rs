use crate::git_provider::{self, Operation, SourceControlProvider};
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

/// The single choke point for per-operation capabilities.
///
/// Every review command resolves its provider through this instead of
/// [`provider_at`], so an operation a host has not declared is refused
/// *here* rather than attempted and failed at the server. That is what
/// makes a blank declaration safe: an unverified host is read-only by
/// construction, not by everyone remembering to check.
///
/// The refusal names both the host and the operation, because the two
/// audiences differ — the user needs to know which product can't do it,
/// and whoever fills in a new adapter's declarations needs to know which
/// line to add.
fn provider_for(path: &str, op: Operation) -> Result<Arc<dyn SourceControlProvider>, String> {
    let provider = provider_at(path);
    check_operation(provider.as_ref(), op)?;
    Ok(provider)
}

/// The refusal itself, split out so it can be tested without a
/// repository on disk — the interesting part is the decision and the
/// sentence, not the path resolution around them.
pub(crate) fn check_operation(
    provider: &dyn SourceControlProvider,
    op: Operation,
) -> Result<(), String> {
    if provider.operations().allows(op) {
        return Ok(());
    }
    Err(format!(
        "{} does not declare this operation — Codemux cannot {} here. \
         Open it in the browser instead.",
        provider.kind().display_name(),
        op.describe(),
    ))
}

/// Which operation a submitted verdict actually is.
///
/// One command carries three of them, and they are declared separately
/// precisely because GitLab serves two and refuses the third.
fn verdict_operation(event: &str) -> Operation {
    match event {
        "approve" => Operation::Approve,
        "request-changes" => Operation::RequestChanges,
        _ => Operation::Comment,
    }
}

/// GitHub requires `body` on the two verdicts that are only words: a
/// review whose event is `COMMENT` or `REQUEST_CHANGES` and whose body is
/// empty is a 422, and a raw 422 in a toast tells a reviewer nothing
/// about what to do. Approving stays wordless — the approval *is* the
/// statement.
///
/// Line notes do not stand in for the body. They hang off the review;
/// they are not it, and the host rejects the call either way.
pub(crate) fn require_review_body(event: &str, body: &str) -> Result<(), String> {
    if !body.trim().is_empty() || event == "approve" {
        return Ok(());
    }
    Err(match event {
        "request-changes" => {
            "Requesting changes needs a message saying what has to change.".to_string()
        }
        _ => "A comment review needs a message.".to_string(),
    })
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
    tokio::task::spawn_blocking(move || provider_for(&path, Operation::ListRead)?.branch_pull_request(Path::new(&path)))
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
        provider_for(&path, Operation::Comment)?.create_pull_request(
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
        provider_for(&path, Operation::ListRead)?.list_pull_requests(Path::new(&path), &state)
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
        provider_for(&path, Operation::ListRead)?.get_pull_request(Path::new(&path), pr_number)
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
        provider_for(&path, Operation::ListRead)?.pull_request_diff(Path::new(&path), pr_number, full)
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
        provider_for(&path, Operation::ListRead)?.list_incoming_pull_requests(Path::new(&path), &base_branch)
    })
    .await
    .map_err(|e| format!("list_incoming_prs task join failed: {e}"))?
}

/// Every open pull request in one repository, for the Pull Requests
/// page. One call per project root; the page fans out and merges.
///
/// Same blocking-pool discipline as `list_incoming_prs`. This one is
/// deliberately the *cheap* half — the CI rollup and the line counts
/// come from `list_prs_overview_stats` behind it, so the list can be on
/// screen before the host has finished computing them.
#[tauri::command]
pub async fn list_prs_overview(path: String) -> Result<crate::github::PrsOverview, String> {
    tokio::task::spawn_blocking(move || provider_for(&path, Operation::ListRead)?.pull_requests_overview(Path::new(&path)))
        .await
        .map_err(|e| format!("list_prs_overview task join failed: {e}"))?
}

/// The slow half of the same rows: CI rollup and line counts by number.
///
/// Gated on `ChecksStatus` rather than `ListRead`: a host that can list
/// pull requests but has not declared check status has nothing to say
/// here, and should say so instead of shelling out.
#[tauri::command]
pub async fn list_prs_overview_stats(
    path: String,
) -> Result<Vec<crate::github::PrOverviewStats>, String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::ChecksStatus)?
            .pull_requests_overview_stats(Path::new(&path))
    })
    .await
    .map_err(|e| format!("list_prs_overview_stats task join failed: {e}"))?
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
        provider_for(&path, Operation::MergeWithStrategies)?.merge_pull_request(
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
        provider_for(&path, Operation::DraftReadyCloseReopen)?.close_pull_request(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("close_pull_request task join failed: {e}"))?
}

#[tauri::command]
pub async fn reopen_pull_request(path: String, pr_number: u32) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::DraftReadyCloseReopen)?.reopen_pull_request(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("reopen_pull_request task join failed: {e}"))?
}

#[tauri::command]
pub async fn set_pr_ready(path: String, pr_number: u32, ready: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::DraftReadyCloseReopen)?.set_pull_request_ready(Path::new(&path), pr_number, ready)
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
        provider_for(&path, Operation::Comment)?.update_pull_request(
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
        provider_for(&path, Operation::Comment)?.request_pull_request_review(Path::new(&path), pr_number, &reviewer)
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
        provider_for(&path, Operation::ChecksStatus)?.check_log_excerpt(Path::new(&path), pr_number, &check_name)
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
        provider_for(&path, Operation::ChecksStatus)?.pull_request_checks(Path::new(&path), pr_number)
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
        provider_for(&path, Operation::ListRead)?.pull_request_review_comments(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pr_review_comments task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_pr_inline_comments(path: String, pr_number: u32) -> Result<Vec<InlineReviewComment>, String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::ListRead)?.pull_request_inline_comments(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pr_inline_comments task join failed: {e}"))?
}

/// Conversation threads with their resolution state.
///
/// `ListRead`, not a thread operation of its own: reading who said what
/// is reading. The two thread *operations* gate the reply box and the
/// resolve button, so a host that serves threads but cannot be written to
/// still shows them — read-only, which is exactly right.
#[tauri::command]
pub async fn get_pr_review_threads(
    path: String,
    pr_number: u32,
) -> Result<Vec<crate::github::PrReviewThread>, String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::ListRead)?.pull_request_review_threads(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pr_review_threads task join failed: {e}"))?
}

/// Reply into a thread. Both ids travel because the two hosts address
/// the same act differently — see `SourceControlProvider::reply_to_review_thread`.
#[tauri::command]
pub async fn reply_to_pr_thread(
    path: String,
    pr_number: u32,
    thread_id: String,
    root_comment_id: Option<u64>,
    body: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::ThreadReply)?.reply_to_review_thread(
            Path::new(&path),
            pr_number,
            &thread_id,
            root_comment_id,
            &body,
        )
    })
    .await
    .map_err(|e| format!("reply_to_pr_thread task join failed: {e}"))?
}

/// Resolve or unresolve a thread — one command both ways, because the
/// button is one button whose label flips.
#[tauri::command]
pub async fn set_pr_thread_resolved(
    path: String,
    pr_number: u32,
    thread_id: String,
    resolved: bool,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::ThreadResolve)?.set_review_thread_resolved(
            Path::new(&path),
            pr_number,
            &thread_id,
            resolved,
        )
    })
    .await
    .map_err(|e| format!("set_pr_thread_resolved task join failed: {e}"))?
}

#[tauri::command]
pub async fn submit_pr_review(path: String, pr_number: u32, event: String, body: String) -> Result<(), String> {
    require_review_body(&event, &body)?;
    tokio::task::spawn_blocking(move || {
        provider_for(&path, verdict_operation(&event))?.submit_pull_request_review(Path::new(&path), pr_number, &event, &body)
    })
    .await
    .map_err(|e| format!("submit_pr_review task join failed: {e}"))?
}

/// The complete diff for the Code tab. Uncapped where the prompt-facing
/// `get_github_pr_diff_by_path` is capped, because a reviewer writing
/// notes has to be looking at all of it.
#[tauri::command]
pub async fn get_pr_review_diff(path: String, pr_number: u32) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::ListRead)?.pull_request_review_diff(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pr_review_diff task join failed: {e}"))?
}

/// Post one line comment immediately. `commit_id` must be the head the
/// diff on screen came from — see `github::add_pr_inline_comment`.
#[tauri::command]
pub async fn add_pr_inline_comment(
    path: String,
    pr_number: u32,
    comment: crate::github::PrDraftComment,
    commit_id: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::LineComments)?.add_inline_comment(Path::new(&path), pr_number, &comment, &commit_id)
    })
    .await
    .map_err(|e| format!("add_pr_inline_comment task join failed: {e}"))?
}

/// The whole pending review — verdict, body and every line note — as one
/// request. Empty `comments` falls through to the plain review path.
#[tauri::command]
pub async fn submit_pr_review_with_comments(
    path: String,
    pr_number: u32,
    event: String,
    body: String,
    comments: Vec<crate::github::PrDraftComment>,
    commit_id: String,
) -> Result<(), String> {
    require_review_body(&event, &body)?;
    tokio::task::spawn_blocking(move || {
        // A pending review with line notes is two declarations: the
        // verdict, and the line comments carrying it.
        let provider = provider_for(&path, verdict_operation(&event))?;
        if !comments.is_empty() && !provider.operations().allows(Operation::LineComments) {
            provider_for(&path, Operation::LineComments)?;
        }
        provider.submit_review_with_comments(
            Path::new(&path),
            pr_number,
            &event,
            &body,
            &comments,
            &commit_id,
        )
    })
    .await
    .map_err(|e| format!("submit_pr_review_with_comments task join failed: {e}"))?
}

#[tauri::command]
pub async fn get_pr_deployments(path: String, pr_number: u32) -> Result<Vec<DeploymentInfo>, String> {
    tokio::task::spawn_blocking(move || {
        provider_for(&path, Operation::ListRead)?.pull_request_deployments(Path::new(&path), pr_number)
    })
    .await
    .map_err(|e| format!("get_pr_deployments task join failed: {e}"))?
}

/// The host's history of one pull request, oldest first.
///
/// No "opened" row and no checks row: the first is synthesized by the
/// caller from the pull request it already holds, and the second is not a
/// timeline event at any host — it is the live checks query, rendered
/// last. Putting either here would mean re-fetching data the surface has.
#[tauri::command]
pub async fn get_pr_timeline(
    path: String,
    pr_number: u32,
) -> Result<Vec<crate::github::PrTimelineEvent>, String> {
    tokio::task::spawn_blocking(move || {
        let mut events =
            provider_for(&path, Operation::Timeline)?.pull_request_timeline(Path::new(&path), pr_number)?;
        // One ordering for both hosts. Undated events sort first rather
        // than being dropped — an event with no timestamp is still an
        // event that happened.
        events.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(events)
    })
    .await
    .map_err(|e| format!("get_pr_timeline task join failed: {e}"))?
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_provider::detect::DetectedProvider;
    use crate::git_provider::github::GitHubProvider;
    use crate::git_provider::gitlab::GitLabProvider;
    use crate::git_provider::unsupported::UnsupportedProvider;
    use crate::git_provider::ProviderKind;

    fn detected(kind: ProviderKind) -> DetectedProvider {
        DetectedProvider {
            kind,
            host: Some("git.acme.test".to_string()),
            base_url: Some("https://git.acme.test".to_string()),
            remote_name: Some("origin".to_string()),
        }
    }

    /// The footnote's rule, enforced: a host that declares nothing gets
    /// every operation refused, and the refusal names the host and what
    /// was refused so neither the user nor the next adapter author has to
    /// guess which declaration is missing.
    #[test]
    fn an_undeclared_host_refuses_every_operation_by_name() {
        for kind in [ProviderKind::Bitbucket, ProviderKind::AzureDevOps] {
            let provider = UnsupportedProvider::from_detection(&detected(kind));
            for op in Operation::ALL {
                let err = check_operation(&provider, op)
                    .expect_err("an undeclared operation must be refused");
                assert!(
                    err.contains(kind.display_name()),
                    "{kind:?}/{op:?} refusal must name the host: {err}"
                );
                assert!(
                    err.contains(op.describe()),
                    "{kind:?}/{op:?} refusal must name the operation: {err}"
                );
            }
        }
    }

    /// An unclassified checkout is served by the advisory GitHub route,
    /// so it must not be caught by the guard — a self-hosted GitHub on a
    /// bespoke domain has always worked through these commands.
    #[test]
    fn github_permits_every_operation() {
        for op in Operation::ALL {
            assert!(check_operation(&GitHubProvider, op).is_ok(), "{op:?}");
        }
    }

    /// GitLab withholds exactly two, for two different reasons: it has
    /// no request-changes verdict, and this build cannot post its line
    /// comments (see `GitLabProvider::operations`).
    #[test]
    fn gitlab_withholds_request_changes_and_line_comments() {
        let provider = GitLabProvider::from_detection(&detected(ProviderKind::GitLab));
        for op in Operation::ALL {
            let result = check_operation(&provider, op);
            if matches!(op, Operation::RequestChanges | Operation::LineComments) {
                let err = result.expect_err("GitLab does not declare this");
                assert!(err.contains("GitLab"), "{err}");
                assert!(err.contains(op.describe()), "{err}");
            } else {
                assert!(result.is_ok(), "{op:?} should be served by GitLab");
            }
        }
    }

    /// The declaration must match what the adapter can actually do — a
    /// capability that says yes where the method says no is the failure
    /// these declarations exist to prevent.
    #[test]
    fn gitlab_does_not_declare_an_operation_its_methods_refuse() {
        use crate::git_provider::SourceControlProvider as _;
        let provider = GitLabProvider::from_detection(&detected(ProviderKind::GitLab));
        let comment = crate::github::PrDraftComment {
            file: "src/a.rs".into(),
            body: "note".into(),
            side: "RIGHT".into(),
            line: 12,
            start_line: None,
        };
        // Both line-comment routes refuse, so the declaration must too.
        assert!(provider
            .add_inline_comment(std::path::Path::new("/tmp"), 1, &comment, "abc")
            .is_err());
        assert!(!provider.operations().line_comments);
    }

    /// Replying and resolving are declarations like any other, and they
    /// are in `ALL` — which is what makes the refusal test above cover
    /// them without anyone remembering to add a case.
    #[test]
    fn the_thread_operations_are_declared_by_the_hosts_that_serve_them() {
        assert!(Operation::ALL.contains(&Operation::ThreadReply));
        assert!(Operation::ALL.contains(&Operation::ThreadResolve));

        let gitlab = GitLabProvider::from_detection(&detected(ProviderKind::GitLab));
        let unsupported = UnsupportedProvider::from_detection(&detected(ProviderKind::Bitbucket));

        for op in [Operation::ThreadReply, Operation::ThreadResolve] {
            // Both hosts have a real route for these; GitLab's is the
            // discussions API it already reads threads from.
            assert!(check_operation(&GitHubProvider, op).is_ok(), "{op:?}");
            assert!(check_operation(&gitlab, op).is_ok(), "{op:?}");

            let err = check_operation(&unsupported, op).expect_err("must be refused");
            assert!(err.contains("Bitbucket"), "{err}");
            assert!(err.contains(op.describe()), "{err}");
        }
    }

    /// GitHub 422s a `COMMENT` or `REQUEST_CHANGES` review with no body,
    /// and a raw 422 in a toast tells a reviewer nothing. The refusal
    /// happens here, before the request, and says what to do about it.
    #[test]
    fn a_wordless_comment_or_request_for_changes_is_refused_here() {
        for event in ["comment", "request-changes", ""] {
            for body in ["", "   ", "\n\t"] {
                let err = require_review_body(event, body)
                    .expect_err("an empty body must be refused for this verdict");
                assert!(
                    err.contains("message"),
                    "the refusal must name what is missing: {err}"
                );
            }
            assert!(require_review_body(event, "Looks off to me.").is_ok());
        }
    }

    /// An approval is a statement on its own; requiring words for it
    /// would be this app inventing a rule GitHub does not have.
    #[test]
    fn approving_stays_wordless() {
        assert!(require_review_body("approve", "").is_ok());
        assert!(require_review_body("approve", "   ").is_ok());
        assert!(require_review_body("approve", "Nice.").is_ok());
    }

    /// The three verdicts one command carries are three declarations.
    #[test]
    fn verdicts_map_to_their_own_operations() {
        assert_eq!(verdict_operation("approve"), Operation::Approve);
        assert_eq!(verdict_operation("request-changes"), Operation::RequestChanges);
        assert_eq!(verdict_operation("comment"), Operation::Comment);
        // Anything unrecognised is the least privileged of the three.
        assert_eq!(verdict_operation(""), Operation::Comment);
    }
}
