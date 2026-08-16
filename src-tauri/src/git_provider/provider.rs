//! The operation surface Codemux needs from a git hosting product.
//!
//! The method set is not aspirational — it is exactly what the PR panel,
//! the review tab, the composer's issue/PR pickers, and the two workspace
//! pollers already call on `crate::github`. Signatures reuse the structs
//! those callers already speak (`PullRequestInfo`, `GitHubIssue`, …)
//! rather than introducing a parallel type hierarchy: a second provider's
//! job is to populate the same shapes, not to invent new ones.
//!
//! Every method is synchronous because every implementation shells out to
//! a CLI. Callers keep doing what they do today — wrap the call in
//! `spawn_blocking` so a wedged subprocess can never pin the UI thread.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::detect::ProviderKind;
use crate::github::{
    CheckInfo, DeploymentInfo, GhStatus, GitHubIssue, IncomingPrItem, InlineReviewComment,
    PullRequestInfo, ReviewComment,
};

/// What a provider can actually do, declared statically so the UI can
/// hide controls instead of offering a button that always errors.
///
/// Defaults to all-false; an implementation flips only what it serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Capabilities {
    /// Pull/merge requests at all — list, view, create, merge.
    pub has_pull_requests: bool,
    /// CI status attached to a pull request.
    pub has_checks: bool,
    /// An issue tracker Codemux can read and link a workspace to.
    pub has_issues: bool,
    /// Per-file, per-line review comments.
    pub has_inline_comments: bool,
    /// Conversation-level review threads (approve / request changes).
    pub has_review_threads: bool,
    /// Deployment environments reported against a pull request.
    pub has_deployments: bool,
    /// A submit-review verb distinct from a plain comment.
    pub has_reviews: bool,
    /// The head branch of a cross-repository pull request can be fetched
    /// into a local worktree from the base repo's remote.
    pub has_fork_pr_fetch: bool,
}

/// Contract every hosting integration satisfies.
///
/// Implementations are stateless handles held behind an `Arc` by the
/// registry; they carry no per-repository state, so every method takes
/// the repository path it should operate on.
pub trait SourceControlProvider: Send + Sync {
    /// Which product this implementation serves.
    fn kind(&self) -> ProviderKind;

    fn capabilities(&self) -> Capabilities;

    /// False for the null object. This is the gate that used to be
    /// `github::is_github_repo`: "does a working integration exist for
    /// this checkout?" — separate from whether the user is authenticated.
    fn is_implemented(&self) -> bool {
        true
    }

    /// Is the provider's CLI installed? Cheap; no auth, no network.
    fn cli_available(&self) -> bool;

    /// Installed + logged in, as one tri-state.
    fn auth_status(&self) -> GhStatus;

    // ── Pull requests ──

    /// The PR for the branch currently checked out, if any. `Ok(None)`
    /// is authoritative ("this branch has no PR"); `Err` means the
    /// lookup failed and the caller must preserve what it already knew.
    fn branch_pull_request(&self, repo_path: &Path) -> Result<Option<PullRequestInfo>, String>;

    /// Like [`branch_pull_request`](Self::branch_pull_request) but with
    /// the side-branch fallback the sidebar badge uses.
    fn workspace_pull_request(&self, repo_path: &Path) -> Result<Option<PullRequestInfo>, String>;

    fn list_pull_requests(
        &self,
        repo_path: &Path,
        state: &str,
    ) -> Result<Vec<PullRequestInfo>, String>;

    /// PRs opened *against* `base_branch` — the review tab's inbox.
    fn list_incoming_pull_requests(
        &self,
        repo_path: &Path,
        base_branch: &str,
    ) -> Result<Vec<IncomingPrItem>, String>;

    fn get_pull_request(&self, repo_path: &Path, number: u32) -> Result<PullRequestInfo, String>;

    fn create_pull_request(
        &self,
        repo_path: &Path,
        title: &str,
        body: &str,
        base: Option<&str>,
        draft: bool,
    ) -> Result<PullRequestInfo, String>;

    /// `commit_title` / `commit_body` are the merge-commit subject and
    /// body the merge sheet collects; products that cannot set them (or
    /// strategies that have no merge commit) ignore them.
    fn merge_pull_request(
        &self,
        repo_path: &Path,
        number: u32,
        method: &str,
        delete_branch: bool,
        commit_title: Option<&str>,
        commit_body: Option<&str>,
    ) -> Result<(), String>;

    fn close_pull_request(&self, repo_path: &Path, number: u32) -> Result<(), String>;

    fn reopen_pull_request(&self, repo_path: &Path, number: u32) -> Result<(), String>;

    /// Flip draft ↔ ready-for-review.
    fn set_pull_request_ready(
        &self,
        repo_path: &Path,
        number: u32,
        ready: bool,
    ) -> Result<(), String>;

    /// Edit title and/or body. `None` leaves a field untouched.
    fn update_pull_request(
        &self,
        repo_path: &Path,
        number: u32,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<(), String>;

    fn request_pull_request_review(
        &self,
        repo_path: &Path,
        number: u32,
        reviewer: &str,
    ) -> Result<(), String>;

    /// Best-effort tail of a failing check's log. An empty string means
    /// "nothing to show" and is not an error — the card renders without
    /// the excerpt.
    fn check_log_excerpt(
        &self,
        repo_path: &Path,
        number: u32,
        check_name: &str,
    ) -> Result<String, String>;

    /// `full = false` yields a name-only diff; `true` a unified diff.
    fn pull_request_diff(
        &self,
        repo_path: &Path,
        number: u32,
        full: bool,
    ) -> Result<String, String>;

    /// CI checks for the current branch's PR.
    fn pull_request_checks(&self, repo_path: &Path) -> Result<Vec<CheckInfo>, String>;

    /// Conversation-level review threads for the current branch's PR.
    fn pull_request_review_comments(&self, repo_path: &Path)
        -> Result<Vec<ReviewComment>, String>;

    fn pull_request_inline_comments(
        &self,
        repo_path: &Path,
        number: u32,
    ) -> Result<Vec<InlineReviewComment>, String>;

    fn submit_pull_request_review(
        &self,
        repo_path: &Path,
        number: u32,
        event: &str,
        body: &str,
    ) -> Result<(), String>;

    fn pull_request_deployments(
        &self,
        repo_path: &Path,
        number: u32,
    ) -> Result<Vec<DeploymentInfo>, String>;

    // ── Issues ──

    fn list_issues(&self, repo_path: &Path, search: Option<&str>)
        -> Result<Vec<GitHubIssue>, String>;

    fn get_issue(&self, repo_path: &Path, number: u64) -> Result<GitHubIssue, String>;

    /// Same as [`get_issue`](Self::get_issue) but bypassing any TTL
    /// cache. The linked-issue card on a workspace is written from this:
    /// it is the stored copy other UI reads, so it must not be seeded
    /// from a minute-old list.
    fn get_issue_fresh(&self, repo_path: &Path, number: u64) -> Result<GitHubIssue, String>;

    // ── Git-level integration ──

    /// Refspec that fetches a cross-repository PR's head into a local
    /// branch, for `git fetch <remote> <refspec>`. `None` when the
    /// product exposes no such ref, in which case the caller simply
    /// skips the fetch attempt.
    fn fork_pr_fetch_refspec(&self, number: u32, local_branch: &str) -> Option<String>;
}
