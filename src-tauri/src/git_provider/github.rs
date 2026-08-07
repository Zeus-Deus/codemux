//! GitHub adapter — thin delegation to `crate::github` / `crate::github_cache`.
//!
//! Deliberately logic-free. Every method forwards to the function the
//! corresponding caller already used, including the choice of cached vs
//! uncached wrapper, so routing a call site through the trait cannot
//! change what GitHub users observe.

use std::path::Path;

use super::detect::ProviderKind;
use super::provider::{Capabilities, SourceControlProvider};
use crate::github::{
    self, CheckInfo, DeploymentInfo, GhStatus, GitHubIssue, IncomingPrItem, InlineReviewComment,
    PullRequestInfo, ReviewComment,
};
use crate::github_cache;

pub struct GitHubProvider;

impl SourceControlProvider for GitHubProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GitHub
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            has_pull_requests: true,
            has_checks: true,
            has_issues: true,
            has_inline_comments: true,
            has_review_threads: true,
            has_deployments: true,
            has_reviews: true,
            has_fork_pr_fetch: true,
        }
    }

    fn cli_available(&self) -> bool {
        github::gh_available()
    }

    fn auth_status(&self) -> GhStatus {
        github::check_gh_status()
    }

    fn branch_pull_request(&self, repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
        github::get_branch_pr(repo_path)
    }

    fn workspace_pull_request(&self, repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
        github::get_workspace_pr(repo_path)
    }

    fn list_pull_requests(
        &self,
        repo_path: &Path,
        state: &str,
    ) -> Result<Vec<PullRequestInfo>, String> {
        github_cache::cached_list_pull_requests(repo_path, state)
    }

    fn list_incoming_pull_requests(
        &self,
        repo_path: &Path,
        base_branch: &str,
    ) -> Result<Vec<IncomingPrItem>, String> {
        github::list_incoming_prs(repo_path, base_branch)
    }

    fn get_pull_request(&self, repo_path: &Path, number: u32) -> Result<PullRequestInfo, String> {
        github_cache::cached_get_pull_request(repo_path, number)
    }

    fn create_pull_request(
        &self,
        repo_path: &Path,
        title: &str,
        body: &str,
        base: Option<&str>,
        draft: bool,
    ) -> Result<PullRequestInfo, String> {
        github::create_pull_request(repo_path, title, body, base, draft)
    }

    fn merge_pull_request(
        &self,
        repo_path: &Path,
        number: u32,
        method: &str,
    ) -> Result<(), String> {
        github::merge_pull_request(repo_path, number, method)
    }

    fn pull_request_diff(
        &self,
        repo_path: &Path,
        number: u32,
        full: bool,
    ) -> Result<String, String> {
        github_cache::cached_get_pr_diff(repo_path, number, full)
    }

    fn pull_request_checks(&self, repo_path: &Path) -> Result<Vec<CheckInfo>, String> {
        github::get_pr_checks(repo_path)
    }

    fn pull_request_review_comments(
        &self,
        repo_path: &Path,
    ) -> Result<Vec<ReviewComment>, String> {
        github::get_pr_review_comments(repo_path)
    }

    fn pull_request_inline_comments(
        &self,
        repo_path: &Path,
        number: u32,
    ) -> Result<Vec<InlineReviewComment>, String> {
        github::get_pr_inline_comments(repo_path, number)
    }

    fn submit_pull_request_review(
        &self,
        repo_path: &Path,
        number: u32,
        event: &str,
        body: &str,
    ) -> Result<(), String> {
        github::submit_pr_review(repo_path, number, event, body)
    }

    fn pull_request_deployments(
        &self,
        repo_path: &Path,
        number: u32,
    ) -> Result<Vec<DeploymentInfo>, String> {
        github::get_pr_deployments(repo_path, number)
    }

    fn list_issues(
        &self,
        repo_path: &Path,
        search: Option<&str>,
    ) -> Result<Vec<GitHubIssue>, String> {
        github_cache::cached_list_issues(repo_path, search)
    }

    fn get_issue(&self, repo_path: &Path, number: u64) -> Result<GitHubIssue, String> {
        github_cache::cached_get_issue(repo_path, number)
    }

    fn get_issue_fresh(&self, repo_path: &Path, number: u64) -> Result<GitHubIssue, String> {
        github::get_github_issue(repo_path, number)
    }

    fn fork_pr_fetch_refspec(&self, number: u32, local_branch: &str) -> Option<String> {
        Some(format!("pull/{number}/head:{local_branch}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_the_full_github_capability_set() {
        let caps = GitHubProvider.capabilities();
        assert!(caps.has_pull_requests);
        assert!(caps.has_checks);
        assert!(caps.has_issues);
        assert!(caps.has_inline_comments);
        assert!(caps.has_review_threads);
        assert!(caps.has_deployments);
        assert!(caps.has_reviews);
        assert!(caps.has_fork_pr_fetch);
        assert!(GitHubProvider.is_implemented());
        assert_eq!(GitHubProvider.kind(), ProviderKind::GitHub);
    }

    #[test]
    fn fork_refspec_matches_the_refspec_the_worktree_path_has_always_used() {
        assert_eq!(
            GitHubProvider.fork_pr_fetch_refspec(42, "pr-42"),
            Some("pull/42/head:pr-42".to_string())
        );
    }
}
