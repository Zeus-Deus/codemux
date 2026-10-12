//! Typed entry points sharing the process-wide GitHub read cache.
//!
//! Do not cache parsed values again here. The native coordinator already
//! coalesces and bounds responses by host, credential and repository. A
//! second path/number-only cache outlived refreshes and served one account's
//! or remote's data to another, while hiding the live detail polling cadence.

use std::path::Path;

use crate::github::{self, GitHubIssue, PullRequestInfo};

pub fn cached_list_issues(
    repo_path: &Path,
    search: Option<&str>,
) -> Result<Vec<GitHubIssue>, String> {
    github::list_github_issues(repo_path, search)
}

pub fn cached_get_issue(repo_path: &Path, number: u64) -> Result<GitHubIssue, String> {
    github::get_github_issue(repo_path, number)
}

pub fn cached_list_pull_requests(
    repo_path: &Path,
    state: &str,
) -> Result<Vec<PullRequestInfo>, String> {
    github::list_pull_requests(repo_path, state)
}

pub fn cached_get_pull_request(repo_path: &Path, number: u32) -> Result<PullRequestInfo, String> {
    github::get_pull_request(repo_path, number)
}

pub fn cached_get_pr_diff(repo_path: &Path, number: u32, full: bool) -> Result<String, String> {
    github::get_pr_diff(repo_path, number, full)
}
