//! Null object for a repository Codemux cannot serve.
//!
//! Exists so [`provider_for_path`](super::registry::provider_for_path)
//! can return a provider unconditionally: no caller has to unwrap an
//! `Option` or invent its own "no integration here" message. Every
//! method fails with the same sanitized sentence, and
//! [`is_implemented`](super::provider::SourceControlProvider::is_implemented)
//! returns false so gate-shaped callers can skip the work entirely
//! instead of collecting an error per tick.

use std::path::Path;

use super::detect::{DetectedProvider, ProviderKind};
use super::provider::{Capabilities, SourceControlProvider};
use crate::github::{
    CheckInfo, DeploymentInfo, GhStatus, GitHubIssue, IncomingPrItem, InlineReviewComment,
    PrsOverview, PullRequestInfo, ReviewComment,
};

pub struct UnsupportedProvider {
    kind: ProviderKind,
    /// Bare hostname. Never a URL — see [`DetectedProvider`], whose
    /// `host` is parsed with userinfo stripped, so nothing that reaches
    /// an error message here can carry a token or password.
    host: Option<String>,
}

impl UnsupportedProvider {
    pub fn from_detection(detected: &DetectedProvider) -> Self {
        Self {
            kind: detected.kind,
            host: detected.host.clone(),
        }
    }

    fn error(&self, operation: &str) -> String {
        let product = match self.kind {
            ProviderKind::Unknown => "This repository's host".to_string(),
            kind => kind.display_name().to_string(),
        };
        match &self.host {
            Some(host) => format!(
                "{product} ({host}) is not supported yet — cannot {operation}. \
                 Codemux currently integrates with GitHub and GitLab."
            ),
            None => format!(
                "{product} is not supported yet — cannot {operation}. \
                 Codemux currently integrates with GitHub and GitLab."
            ),
        }
    }

    fn err<T>(&self, operation: &str) -> Result<T, String> {
        Err(self.error(operation))
    }
}

impl SourceControlProvider for UnsupportedProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn is_implemented(&self) -> bool {
        false
    }

    fn cli_available(&self) -> bool {
        false
    }

    fn auth_status(&self) -> GhStatus {
        GhStatus::NotInstalled
    }

    fn branch_pull_request(&self, _repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
        self.err("look up the pull request for this branch")
    }

    fn workspace_pull_request(&self, _repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
        self.err("look up this workspace's pull request")
    }

    fn list_pull_requests(
        &self,
        _repo_path: &Path,
        _state: &str,
    ) -> Result<Vec<PullRequestInfo>, String> {
        self.err("list pull requests")
    }

    fn list_incoming_pull_requests(
        &self,
        _repo_path: &Path,
        _base_branch: &str,
    ) -> Result<Vec<IncomingPrItem>, String> {
        self.err("list incoming pull requests")
    }

    fn pull_requests_overview(&self, _repo_path: &Path) -> Result<PrsOverview, String> {
        self.err("list pull requests")
    }

    fn get_pull_request(
        &self,
        _repo_path: &Path,
        _number: u32,
    ) -> Result<PullRequestInfo, String> {
        self.err("open a pull request")
    }

    fn create_pull_request(
        &self,
        _repo_path: &Path,
        _title: &str,
        _body: &str,
        _base: Option<&str>,
        _draft: bool,
    ) -> Result<PullRequestInfo, String> {
        self.err("create a pull request")
    }

    fn merge_pull_request(
        &self,
        _repo_path: &Path,
        _number: u32,
        _method: &str,
        _delete_branch: bool,
        _commit_title: Option<&str>,
        _commit_body: Option<&str>,
    ) -> Result<(), String> {
        self.err("merge a pull request")
    }

    fn close_pull_request(&self, _repo_path: &Path, _number: u32) -> Result<(), String> {
        self.err("close a pull request")
    }

    fn reopen_pull_request(&self, _repo_path: &Path, _number: u32) -> Result<(), String> {
        self.err("reopen a pull request")
    }

    fn set_pull_request_ready(
        &self,
        _repo_path: &Path,
        _number: u32,
        _ready: bool,
    ) -> Result<(), String> {
        self.err("change a pull request's draft state")
    }

    fn update_pull_request(
        &self,
        _repo_path: &Path,
        _number: u32,
        _title: Option<&str>,
        _body: Option<&str>,
    ) -> Result<(), String> {
        self.err("edit a pull request")
    }

    fn request_pull_request_review(
        &self,
        _repo_path: &Path,
        _number: u32,
        _reviewer: &str,
    ) -> Result<(), String> {
        self.err("request a review")
    }

    fn check_log_excerpt(
        &self,
        _repo_path: &Path,
        _number: u32,
        _check_name: &str,
    ) -> Result<String, String> {
        self.err("read a check log")
    }

    fn pull_request_diff(
        &self,
        _repo_path: &Path,
        _number: u32,
        _full: bool,
    ) -> Result<String, String> {
        self.err("read a pull request diff")
    }

    fn pull_request_checks(
        &self,
        _repo_path: &Path,
        _number: Option<u32>,
    ) -> Result<Vec<CheckInfo>, String> {
        self.err("read pull request checks")
    }

    fn pull_request_review_comments(
        &self,
        _repo_path: &Path,
        _number: Option<u32>,
    ) -> Result<Vec<ReviewComment>, String> {
        self.err("read review comments")
    }

    fn pull_request_inline_comments(
        &self,
        _repo_path: &Path,
        _number: u32,
    ) -> Result<Vec<InlineReviewComment>, String> {
        self.err("read inline review comments")
    }

    fn submit_pull_request_review(
        &self,
        _repo_path: &Path,
        _number: u32,
        _event: &str,
        _body: &str,
    ) -> Result<(), String> {
        self.err("submit a review")
    }

    fn pull_request_deployments(
        &self,
        _repo_path: &Path,
        _number: u32,
    ) -> Result<Vec<DeploymentInfo>, String> {
        self.err("read deployments")
    }

    fn list_issues(
        &self,
        _repo_path: &Path,
        _search: Option<&str>,
    ) -> Result<Vec<GitHubIssue>, String> {
        self.err("list issues")
    }

    fn get_issue(&self, _repo_path: &Path, _number: u64) -> Result<GitHubIssue, String> {
        self.err("open an issue")
    }

    fn get_issue_fresh(&self, _repo_path: &Path, _number: u64) -> Result<GitHubIssue, String> {
        self.err("open an issue")
    }

    fn fork_pr_fetch_refspec(&self, _number: u32, _local_branch: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(kind: ProviderKind, host: &str) -> DetectedProvider {
        DetectedProvider {
            kind,
            host: Some(host.to_string()),
            base_url: Some(format!("https://{host}")),
            remote_name: Some("origin".to_string()),
        }
    }

    #[test]
    fn declares_nothing_and_reports_itself_unimplemented() {
        let provider = UnsupportedProvider::from_detection(&detected(
            ProviderKind::Bitbucket,
            "bitbucket.org",
        ));
        assert_eq!(provider.capabilities(), Capabilities::default());
        assert!(!provider.is_implemented());
        assert!(!provider.cli_available());
        assert_eq!(provider.fork_pr_fetch_refspec(3, "pr-3"), None);
    }

    #[test]
    fn errors_name_the_product_and_the_host() {
        let provider = UnsupportedProvider::from_detection(&detected(
            ProviderKind::Bitbucket,
            "bitbucket.example.com",
        ));
        let message = provider.list_pull_requests(Path::new("/tmp"), "open").unwrap_err();
        assert!(message.contains("Bitbucket"), "{message}");
        assert!(message.contains("bitbucket.example.com"), "{message}");
        assert!(message.contains("list pull requests"), "{message}");
    }

    #[test]
    fn an_unclassified_host_gets_a_generic_but_still_specific_error() {
        let provider = UnsupportedProvider::from_detection(&detected(
            ProviderKind::Unknown,
            "git.acme.internal",
        ));
        let message = provider.create_pull_request(Path::new("/tmp"), "t", "b", None, false).unwrap_err();
        assert!(message.contains("git.acme.internal"), "{message}");
        assert!(message.contains("create a pull request"), "{message}");
    }

    #[test]
    fn a_hostless_detection_still_produces_a_readable_error() {
        let provider = UnsupportedProvider::from_detection(&DetectedProvider::unknown());
        let message = provider.get_issue(Path::new("/tmp"), 1).unwrap_err();
        assert!(message.contains("not supported yet"), "{message}");
    }

    /// The host on a `DetectedProvider` is parsed with userinfo removed,
    /// so a remote carrying a token cannot leak through an error string.
    #[test]
    fn errors_cannot_echo_credentials() {
        let detected = crate::git_provider::detect::detect_from_remotes(
            &[crate::git_provider::detect::Remote {
                name: "origin".into(),
                url: "https://oauth2:secret-token-value@bitbucket.example.com/g/p.git".into(),
            }],
            None,
            &std::collections::HashMap::new(),
        );
        let provider = UnsupportedProvider::from_detection(&detected);
        for message in [
            provider
                .merge_pull_request(Path::new("/tmp"), 1, "squash", true, None, None)
                .unwrap_err(),
            provider.list_issues(Path::new("/tmp"), None).unwrap_err(),
            provider
                .pull_request_checks(Path::new("/tmp"), None)
                .unwrap_err(),
        ] {
            assert!(!message.contains("secret-token-value"), "{message}");
            assert!(!message.contains("oauth2"), "{message}");
            assert!(message.contains("bitbucket.example.com"), "{message}");
        }
    }
}
