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
    PrOverviewStats, PrReviewThread, PrTimelineEvent, PrsOverview, PullRequestInfo, ReviewComment,
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

/// One reviewable operation, as the review surfaces think of them.
///
/// Deliberately coarser than the trait's method list: this is the
/// vocabulary the *UI* gates on, so `approve` and `request_changes` are
/// two operations even though both go through
/// [`submit_pull_request_review`](SourceControlProvider::submit_pull_request_review),
/// and every read path collapses into `ListRead` because no surface
/// offers "list" without "read".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    ListRead,
    Comment,
    Approve,
    RequestChanges,
    LineComments,
    MergeWithStrategies,
    DraftReadyCloseReopen,
    ChecksStatus,
    Timeline,
    ThreadReply,
    ThreadResolve,
}

impl Operation {
    /// The verb phrase that completes "… cannot <op> on this host".
    pub fn describe(self) -> &'static str {
        match self {
            Operation::ListRead => "list or read pull requests",
            Operation::Comment => "comment on a pull request",
            Operation::Approve => "approve a pull request",
            Operation::RequestChanges => "request changes on a pull request",
            Operation::LineComments => "leave line comments on a diff",
            Operation::MergeWithStrategies => "merge a pull request",
            Operation::DraftReadyCloseReopen => "change a pull request's draft or open state",
            Operation::ChecksStatus => "read check or pipeline status",
            Operation::Timeline => "read a pull request's timeline",
            Operation::ThreadReply => "reply to a review thread",
            Operation::ThreadResolve => "resolve or unresolve a review thread",
        }
    }

    /// Every operation, for exhaustive tests and for [`OperationCapabilities::all`].
    pub const ALL: [Operation; 11] = [
        Operation::ListRead,
        Operation::Comment,
        Operation::Approve,
        Operation::RequestChanges,
        Operation::LineComments,
        Operation::MergeWithStrategies,
        Operation::DraftReadyCloseReopen,
        Operation::ChecksStatus,
        Operation::Timeline,
        Operation::ThreadReply,
        Operation::ThreadResolve,
    ];
}

/// Per-operation declarations, from which the UI renders.
///
/// The point of the split from [`Capabilities`] is the difference between
/// "this host has inline comments as a concept" and "this build of
/// Codemux can perform the inline-comment operation against it". Only the
/// second one may draw a button.
///
/// Defaults to all-false, and that default is the *correct* answer for a
/// host nobody has verified: the command layer refuses undeclared
/// operations, so a wrong declaration is worse than no declaration —
/// an undeclared host degrades to read-only-with-a-sentence, while a
/// wrongly declared one draws controls that fail at the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OperationCapabilities {
    pub list_read: bool,
    pub comment: bool,
    pub approve: bool,
    pub request_changes: bool,
    pub line_comments: bool,
    pub merge_with_strategies: bool,
    pub draft_ready_close_reopen: bool,
    pub checks_status: bool,
    pub timeline: bool,
    /// Post a reply into an existing review thread.
    pub thread_reply: bool,
    /// Flip a review thread's resolved state, both ways.
    pub thread_resolve: bool,
}

impl OperationCapabilities {
    /// Everything declared — a host verified against its real CLI.
    pub fn all() -> Self {
        Self {
            list_read: true,
            comment: true,
            approve: true,
            request_changes: true,
            line_comments: true,
            merge_with_strategies: true,
            draft_ready_close_reopen: true,
            checks_status: true,
            timeline: true,
            thread_reply: true,
            thread_resolve: true,
        }
    }

    pub fn allows(&self, op: Operation) -> bool {
        match op {
            Operation::ListRead => self.list_read,
            Operation::Comment => self.comment,
            Operation::Approve => self.approve,
            Operation::RequestChanges => self.request_changes,
            Operation::LineComments => self.line_comments,
            Operation::MergeWithStrategies => self.merge_with_strategies,
            Operation::DraftReadyCloseReopen => self.draft_ready_close_reopen,
            Operation::ChecksStatus => self.checks_status,
            Operation::Timeline => self.timeline,
            Operation::ThreadReply => self.thread_reply,
            Operation::ThreadResolve => self.thread_resolve,
        }
    }
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

    /// Which review operations this adapter declares it can perform.
    ///
    /// No default implementation on purpose. A new adapter has to make
    /// the claim explicitly, in one place, having actually checked its
    /// CLI — which is exactly the discipline the blank Bitbucket and
    /// Azure columns exist to enforce.
    fn operations(&self) -> OperationCapabilities;

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

    /// Every open pull request in this repository, with the viewer
    /// relation (who is asking, who is asked). This is what the Pull
    /// Requests page lists, and it is the call whose latency the user
    /// watches — so it carries only what the host serves cheaply.
    ///
    /// A provider that can answer the CI rollup for free (GitLab's merge
    /// request list already carries `head_pipeline`) fills `checks` here
    /// and the page never makes the second call. A provider that cannot
    /// leaves it `None`, meaning "not measured yet".
    fn pull_requests_overview(&self, repo_path: &Path) -> Result<PrsOverview, String>;

    /// The expensive half of the same rows: CI rollup and line counts,
    /// keyed by number so the page can merge them into a list it has
    /// already painted. Only called when `pull_requests_overview` left
    /// `checks` unanswered.
    fn pull_requests_overview_stats(
        &self,
        repo_path: &Path,
    ) -> Result<Vec<PrOverviewStats>, String>;

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

    /// The complete unified diff, for the surface that renders it.
    ///
    /// Separate from `pull_request_diff` because that one is capped for
    /// prompts: a review has to see every hunk, or notes get written
    /// against a file the reviewer was never shown.
    fn pull_request_review_diff(
        &self,
        repo_path: &Path,
        number: u32,
    ) -> Result<String, String>;

    /// Post one line comment now, outside any pending review.
    fn add_inline_comment(
        &self,
        repo_path: &Path,
        number: u32,
        comment: &crate::github::PrDraftComment,
        commit_id: &str,
    ) -> Result<(), String>;

    /// Submit a verdict and its line notes as a single request.
    fn submit_review_with_comments(
        &self,
        repo_path: &Path,
        number: u32,
        event: &str,
        body: &str,
        comments: &[crate::github::PrDraftComment],
        commit_id: &str,
    ) -> Result<(), String>;

    /// CI checks for a pull request. `number = None` means the current
    /// branch's PR — the panel's case; the Pull Requests page passes the
    /// number of whichever PR is selected, which is usually not this
    /// checkout's.
    fn pull_request_checks(
        &self,
        repo_path: &Path,
        number: Option<u32>,
    ) -> Result<Vec<CheckInfo>, String>;

    /// Conversation-level review threads. `number = None` means the
    /// current branch's PR; the page passes the selected one.
    fn pull_request_review_comments(
        &self,
        repo_path: &Path,
        number: Option<u32>,
    ) -> Result<Vec<ReviewComment>, String>;

    fn pull_request_inline_comments(
        &self,
        repo_path: &Path,
        number: u32,
    ) -> Result<Vec<InlineReviewComment>, String>;

    /// Conversation threads on the diff, with their resolution state.
    ///
    /// Distinct from [`pull_request_inline_comments`](Self::pull_request_inline_comments),
    /// which is a flat list of comments and cannot answer "is this still
    /// open?" — the two coexist because the flat list is what the review
    /// *summaries* are grouped from.
    fn pull_request_review_threads(
        &self,
        repo_path: &Path,
        number: u32,
    ) -> Result<Vec<PrReviewThread>, String>;

    /// Reply into a thread.
    ///
    /// Both ids because the two hosts address the same act differently:
    /// GitLab replies to the discussion (`thread_id`), GitHub to the
    /// thread's first comment (`root_comment_id`, which the thread
    /// payload carries as `database_id`). An implementation uses the one
    /// it needs and errors clearly if that one is absent, rather than
    /// silently posting a top-level comment instead.
    fn reply_to_review_thread(
        &self,
        repo_path: &Path,
        number: u32,
        thread_id: &str,
        root_comment_id: Option<u64>,
        body: &str,
    ) -> Result<(), String>;

    /// Resolve (`true`) or unresolve (`false`) a thread.
    fn set_review_thread_resolved(
        &self,
        repo_path: &Path,
        number: u32,
        thread_id: &str,
        resolved: bool,
    ) -> Result<(), String>;

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

    /// The host's own history of a pull request, oldest first.
    ///
    /// Excludes the "opened" event: no host serves one, and the caller
    /// already holds the pull request it would be synthesized from.
    fn pull_request_timeline(
        &self,
        repo_path: &Path,
        number: u32,
    ) -> Result<Vec<PrTimelineEvent>, String>;

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
