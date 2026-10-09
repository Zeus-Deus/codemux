//! Native regression coverage with synthetic credentials and an isolated gh.
#![cfg(target_os = "linux")]

use codemux_lib::{git_provider, github, github_cache};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

static ENVIRONMENT: Mutex<()> = Mutex::new(());
const HEAD_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Fixture {
    root: tempfile::TempDir,
    path: Option<OsString>,
    token: Option<OsString>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let script = r#"#!/bin/sh
case "$1 $2" in
  'auth token') printf '%s' "$GH_TOKEN" ;;
  'config get') printf 'fixture-user' ;;
  'api rate_limit')
    reset=$(( $(date +%s) + 3600 ))
    printf '{"resources":{"core":{"limit":5000,"remaining":4900,"reset":%s},"graphql":{"limit":5000,"remaining":4900,"reset":%s}}}' "$reset" "$reset" ;;
  'pr view')
    printf 'head\n' >> .fixture-reads
    if [ -f .fixture-head-error ]; then cat .fixture-head-error >&2; exit 1; fi
    label=$(cat .fixture-label)
    head=''
    case "$*" in *headRefOid*) head=",\"headRefOid\":\"$(cat .fixture-head)\"" ;; esac
    printf '{"number":42,"url":"https://github.com/fixture/repo/pull/42","state":"OPEN","title":"%s","headRefName":"feature","baseRefName":"main","isDraft":false%s,"body":"fixture"}' "$label" "$head" ;;
  'pr diff')
    printf 'diff\n' >> .fixture-reads
    if [ -f .fixture-during-head ]; then cp .fixture-during-head .fixture-head; fi
    if [ -f .fixture-after-head-error ]; then cp .fixture-after-head-error .fixture-head-error; fi
    cat .fixture-patch ;;
  '--version ') printf 'gh fixture' ;;
  *) printf 'unsupported fixture command' >&2; exit 1 ;;
esac
"#;
        let gh = bin.join("gh");
        fs::write(&gh, script).unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::var_os("PATH");
        let token = std::env::var_os("GH_TOKEN");
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(path.as_deref().unwrap_or_default()));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        std::env::set_var("GH_TOKEN", format!("synthetic-freshness-{}", root.path().display()));
        Self { root, path, token }
    }

    fn repo(&self, name: &str) -> PathBuf {
        let repo = self.root.path().join(name);
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["remote", "add", "origin", "https://github-freshness.example.test/fixture/repo.git"]);
        fs::write(repo.join(".fixture-label"), "before").unwrap();
        fs::write(repo.join(".fixture-patch"), "old patch").unwrap();
        fs::write(repo.join(".fixture-head"), HEAD_A).unwrap();
        repo
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for (name, value) in [("PATH", &self.path), ("GH_TOKEN", &self.token)] {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }
}

fn git(repo: &Path, args: &[&str]) {
    let result = Command::new("git").args(args).current_dir(repo).output().unwrap();
    assert!(result.status.success(), "fixture git operation failed");
}

#[test]
fn native_detail_cache_does_not_cross_accounts() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("account-change");
    assert_eq!(github_cache::cached_get_pull_request(&repo, 42).unwrap().title, "before");
    fs::write(repo.join(".fixture-label"), "other account").unwrap();
    std::env::set_var("GH_TOKEN", "synthetic-freshness-account-b");
    assert_eq!(github_cache::cached_get_pull_request(&repo, 42).unwrap().title, "other account");
}

#[test]
fn native_detail_cache_does_not_cross_remote_repositories() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("remote-change");
    assert_eq!(github_cache::cached_get_pull_request(&repo, 42).unwrap().title, "before");
    git(&repo, &["remote", "set-url", "origin", "https://github-freshness.example.test/fixture/other.git"]);
    fs::write(repo.join(".fixture-label"), "other repository").unwrap();
    assert_eq!(github_cache::cached_get_pull_request(&repo, 42).unwrap().title, "other repository");
}

#[test]
fn review_diff_rechecks_the_remote_instead_of_reusing_a_previous_patch() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("updated-diff");
    let provider = git_provider::github_provider();
    assert_eq!(provider.pull_request_review_diff(&repo, 42).unwrap(), "old patch");
    fs::write(repo.join(".fixture-patch"), "new patch").unwrap();
    assert_eq!(provider.pull_request_review_diff(&repo, 42).unwrap(), "new patch");
}

#[test]
fn selected_pr_detail_exposes_the_head_used_for_review_drafts() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("detail-head");
    let pr = github::get_pull_request(&repo, 42).unwrap();
    assert_eq!(pr.head_ref_oid.as_deref(), Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
}

// Head-bound regressions also run in the dedicated exact-source helper harness.
#[test]
fn head_bound_diff_rejects_a_push_before_fetch_without_reading_the_patch() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("head-before-fetch");
    fs::write(repo.join(".fixture-head"), HEAD_B).unwrap();
    fs::write(repo.join(".fixture-patch"), "new head patch").unwrap();
    let result = github::get_pr_review_diff_for_head(&repo, 42, Some(HEAD_A));
    assert!(result.is_err(), "new-head content must not be returned for an old-head cache key: {result:?}");
    assert!(result.unwrap_err().contains("head changed"));
    assert_eq!(fs::read_to_string(repo.join(".fixture-reads")).unwrap(), "head\n");
}

#[test]
fn head_bound_diff_rejects_a_push_during_fetch() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("head-during-fetch");
    fs::write(repo.join(".fixture-during-head"), HEAD_B).unwrap();
    fs::write(repo.join(".fixture-patch"), "new head patch").unwrap();
    let result = github::get_pr_review_diff_for_head(&repo, 42, Some(HEAD_A));
    assert!(result.is_err(), "a patch fetched across a push must not escape the head guard: {result:?}");
    assert!(result.unwrap_err().contains("head changed"));
    assert_eq!(fs::read_to_string(repo.join(".fixture-reads")).unwrap(), "head\ndiff\nhead\n");
}

#[test]
fn head_bound_diff_rechecks_both_heads_and_patch_on_each_success() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("fresh-heads-and-patch");
    assert_eq!(github::get_pr_review_diff_for_head(&repo, 42, Some(HEAD_A)).unwrap(), "old patch");
    fs::write(repo.join(".fixture-head"), HEAD_B).unwrap();
    fs::write(repo.join(".fixture-patch"), "new patch").unwrap();
    assert_eq!(github::get_pr_review_diff_for_head(&repo, 42, Some(HEAD_B)).unwrap(), "new patch");
    assert_eq!(fs::read_to_string(repo.join(".fixture-reads")).unwrap(), "head\ndiff\nhead\nhead\ndiff\nhead\n");
}

#[test]
fn head_bound_diff_fails_closed_when_post_fetch_head_read_fails_and_keeps_cooldown() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("failed-head-verification");
    fs::write(repo.join(".fixture-after-head-error"), "API rate limit exceeded; Retry-After: 120").unwrap();
    let first = github::get_pr_review_diff_for_head(&repo, 42, Some(HEAD_A));
    assert!(first.unwrap_err().contains("rate limit"));
    let reads = fs::read_to_string(repo.join(".fixture-reads")).unwrap();
    assert_eq!(reads, "head\ndiff\nhead\n");
    assert!(github::get_pr_review_diff_for_head(&repo, 42, Some(HEAD_A)).is_err());
    assert_eq!(fs::read_to_string(repo.join(".fixture-reads")).unwrap(), reads,
        "fresh reads must not bypass failed-read caching or host cooldowns");
}

#[test]
fn head_bound_diff_without_a_sha_preserves_the_unversioned_provider_contract() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("omitted-head");
    fs::write(repo.join(".fixture-head-error"), "head unavailable").unwrap();
    assert_eq!(github::get_pr_review_diff_for_head(&repo, 42, None).unwrap(), "old patch");
    assert_eq!(fs::read_to_string(repo.join(".fixture-reads")).unwrap(), "diff\n");
}

#[test]
fn head_bound_diff_requires_a_nonempty_remote_head() {
    let _guard = ENVIRONMENT.lock().unwrap_or_else(|error| error.into_inner());
    let fixture = Fixture::new();
    let repo = fixture.repo("missing-head");
    fs::write(repo.join(".fixture-head"), "").unwrap();
    let result = github::get_pr_review_diff_for_head(&repo, 42, Some(HEAD_A));
    assert!(result.unwrap_err().contains("did not return"));
    assert_eq!(fs::read_to_string(repo.join(".fixture-reads")).unwrap(), "head\n");
}
