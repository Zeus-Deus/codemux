//! In-process cache for `gh issue list` / `gh issue view` results.
//!
//! Stage 4 — composer's `+ → GitHub Issue…` and `@issue:` popups call
//! these wrappers instead of `crate::github::*` directly. The list TTL
//! is intentionally short (open-issues drift often) while detail TTL
//! is longer (a single-issue view is stable enough to reuse across a
//! few popup interactions).
//!
//! The cache is process-local (no IPC), thread-safe via `Mutex`, and
//! best-effort: a poisoned mutex falls through to a fresh fetch
//! rather than propagating the panic.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::github::{self, GitHubIssue, PullRequestInfo};

/// 60s — open-issues lists shift as people file/close issues. Short
/// enough that a refresh is one second of waiting away.
pub const LIST_TTL: Duration = Duration::from_secs(60);
/// 5 min — single-issue detail (body + comments). Re-fetch boundary
/// chosen so a user can flip between popup → chip → re-open without
/// hammering `gh`.
pub const DETAIL_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct CacheEntry<T> {
    value: T,
    fetched_at: Instant,
}

impl<T> CacheEntry<T> {
    fn fresh(value: T) -> Self {
        Self {
            value,
            fetched_at: Instant::now(),
        }
    }

    fn is_fresh(&self, ttl: Duration) -> bool {
        self.fetched_at.elapsed() < ttl
    }
}

/// Key on `(repo_path, search_query)` so two queries against the same
/// repo don't collide. `None` (open issues) renders to a sentinel
/// suffix to keep the key stringly-typed without an enum.
type ListKey = String;

fn list_key(repo_path: &Path, search: Option<&str>) -> ListKey {
    format!("{}|{}", repo_path.display(), search.unwrap_or(""))
}

static ISSUE_LIST_CACHE: LazyLock<Mutex<HashMap<ListKey, CacheEntry<Vec<GitHubIssue>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static ISSUE_DETAIL_CACHE: LazyLock<Mutex<HashMap<u64, CacheEntry<GitHubIssue>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Stage 5 — PR list cache, keyed by `(repo_path, state)` since
/// `gh pr list` takes a state filter (open/closed/merged/all).
type PrListKey = String;

fn pr_list_key(repo_path: &Path, state: &str) -> PrListKey {
    format!("{}|{}", repo_path.display(), state)
}

static PR_LIST_CACHE: LazyLock<Mutex<HashMap<PrListKey, CacheEntry<Vec<PullRequestInfo>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static PR_DETAIL_CACHE: LazyLock<Mutex<HashMap<u32, CacheEntry<PullRequestInfo>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Diffs are bigger and shift more often than the PR header; key by
/// `(number, full)` so a request for a name-only diff doesn't return
/// a previously-cached full diff (or vice versa). The string is the
/// diff body verbatim.
type PrDiffKey = (u32, bool);

static PR_DIFF_CACHE: LazyLock<Mutex<HashMap<PrDiffKey, CacheEntry<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Cached wrapper around `github::list_github_issues`. Returns the
/// cached vec when within `LIST_TTL`; otherwise falls through to gh.
/// Errors are NOT cached — the next call after an auth failure
/// retries the gh roundtrip.
pub fn cached_list_issues(
    repo_path: &Path,
    search: Option<&str>,
) -> Result<Vec<GitHubIssue>, String> {
    let key = list_key(repo_path, search);

    if let Ok(cache) = ISSUE_LIST_CACHE.lock() {
        if let Some(entry) = cache.get(&key) {
            if entry.is_fresh(LIST_TTL) {
                return Ok(entry.value.clone());
            }
        }
    }

    let fresh = github::list_github_issues(repo_path, search)?;
    if let Ok(mut cache) = ISSUE_LIST_CACHE.lock() {
        cache.insert(key, CacheEntry::fresh(fresh.clone()));
    }
    Ok(fresh)
}

/// Cached wrapper around `github::get_github_issue`. Detail TTL is
/// longer than list TTL because a single-issue detail rarely changes
/// during a user's chat-popup interaction.
pub fn cached_get_issue(repo_path: &Path, number: u64) -> Result<GitHubIssue, String> {
    if let Ok(cache) = ISSUE_DETAIL_CACHE.lock() {
        if let Some(entry) = cache.get(&number) {
            if entry.is_fresh(DETAIL_TTL) {
                return Ok(entry.value.clone());
            }
        }
    }

    let fresh = github::get_github_issue(repo_path, number)?;
    if let Ok(mut cache) = ISSUE_DETAIL_CACHE.lock() {
        cache.insert(number, CacheEntry::fresh(fresh.clone()));
    }
    Ok(fresh)
}

/// Stage 5 — cached PR list. Same TTL as issue lists since both
/// shift on roughly the same human cadence (PRs file/merge/close).
pub fn cached_list_pull_requests(
    repo_path: &Path,
    state: &str,
) -> Result<Vec<PullRequestInfo>, String> {
    let key = pr_list_key(repo_path, state);

    if let Ok(cache) = PR_LIST_CACHE.lock() {
        if let Some(entry) = cache.get(&key) {
            if entry.is_fresh(LIST_TTL) {
                return Ok(entry.value.clone());
            }
        }
    }

    let fresh = github::list_pull_requests(repo_path, state)?;
    if let Ok(mut cache) = PR_LIST_CACHE.lock() {
        cache.insert(key, CacheEntry::fresh(fresh.clone()));
    }
    Ok(fresh)
}

/// Cached single-PR detail (body + comments). Detail TTL matches
/// issues.
pub fn cached_get_pull_request(
    repo_path: &Path,
    number: u32,
) -> Result<PullRequestInfo, String> {
    if let Ok(cache) = PR_DETAIL_CACHE.lock() {
        if let Some(entry) = cache.get(&number) {
            if entry.is_fresh(DETAIL_TTL) {
                return Ok(entry.value.clone());
            }
        }
    }

    let fresh = github::get_pull_request(repo_path, number)?;
    if let Ok(mut cache) = PR_DETAIL_CACHE.lock() {
        cache.insert(number, CacheEntry::fresh(fresh.clone()));
    }
    Ok(fresh)
}

/// Cached diff body. Keyed on `(number, full)` so the name-only +
/// full variants don't collide. Detail TTL — diffs change at most
/// when the head SHA does, which is on a human cadence too.
pub fn cached_get_pr_diff(
    repo_path: &Path,
    number: u32,
    full: bool,
) -> Result<String, String> {
    let key = (number, full);
    if let Ok(cache) = PR_DIFF_CACHE.lock() {
        if let Some(entry) = cache.get(&key) {
            if entry.is_fresh(DETAIL_TTL) {
                return Ok(entry.value.clone());
            }
        }
    }

    let fresh = github::get_pr_diff(repo_path, number, full)?;
    if let Ok(mut cache) = PR_DIFF_CACHE.lock() {
        cache.insert(key, CacheEntry::fresh(fresh.clone()));
    }
    Ok(fresh)
}

/// Stage 5 companion to `invalidate_issue_cache`. `None` drops all
/// PR-side caches; `Some(n)` drops detail + diff entries for PR #n
/// (both the name-only and full diff variants) but leaves the list
/// cache alone — list rows shift independently of single-PR head SHA
/// changes.
pub fn invalidate_pr_cache(number: Option<u32>) {
    if let Ok(mut cache) = PR_DETAIL_CACHE.lock() {
        match number {
            Some(n) => {
                cache.remove(&n);
            }
            None => cache.clear(),
        }
    }
    if let Ok(mut cache) = PR_DIFF_CACHE.lock() {
        match number {
            Some(n) => {
                cache.retain(|(k, _), _| *k != n);
            }
            None => cache.clear(),
        }
    }
    if number.is_none() {
        if let Ok(mut cache) = PR_LIST_CACHE.lock() {
            cache.clear();
        }
    }
}

/// Drop one or all cached detail entries. `None` clears everything;
/// `Some(n)` clears just issue #n. Tests use the `None` form between
/// cases so they can assert on cache hit vs miss without leaking state.
pub fn invalidate_issue_cache(number: Option<u64>) {
    if let Ok(mut cache) = ISSUE_DETAIL_CACHE.lock() {
        match number {
            Some(n) => {
                cache.remove(&n);
            }
            None => cache.clear(),
        }
    }
    if number.is_none() {
        if let Ok(mut cache) = ISSUE_LIST_CACHE.lock() {
            cache.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::IssueState;

    fn fixture_issue(number: u64) -> GitHubIssue {
        GitHubIssue {
            number,
            title: format!("Issue #{number}"),
            state: IssueState::Open,
            labels: Vec::new(),
            assignees: Vec::new(),
            url: format!("https://github.com/u/r/issues/{number}"),
            body: None,
            comments: Vec::new(),
            total_comments: 0,
            updated_at: None,
        }
    }

    #[test]
    fn cache_entry_freshness_window() {
        let entry = CacheEntry::fresh(fixture_issue(1));
        assert!(entry.is_fresh(Duration::from_secs(60)));
        // Zero-duration TTL is always stale (elapsed > 0 immediately
        // after construction).
        assert!(!entry.is_fresh(Duration::from_nanos(0)));
    }

    #[test]
    fn list_key_disambiguates_path_and_query() {
        let k1 = list_key(Path::new("/a"), None);
        let k2 = list_key(Path::new("/a"), Some("bug"));
        let k3 = list_key(Path::new("/b"), None);
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_eq!(list_key(Path::new("/a"), None), k1);
    }

    #[test]
    fn detail_cache_round_trip() {
        // Per-test unique key so parallel test runs don't stomp.
        let unique_num: u64 = 9_999_001;
        let issue = fixture_issue(unique_num);
        ISSUE_DETAIL_CACHE
            .lock()
            .unwrap()
            .insert(unique_num, CacheEntry::fresh(issue.clone()));
        let entry = ISSUE_DETAIL_CACHE
            .lock()
            .unwrap()
            .get(&unique_num)
            .cloned();
        assert!(entry.is_some());
        let cached = entry.unwrap();
        assert_eq!(cached.value.number, unique_num);
        invalidate_issue_cache(Some(unique_num));
        assert!(ISSUE_DETAIL_CACHE.lock().unwrap().get(&unique_num).is_none());
    }

    #[test]
    fn targeted_invalidation_keeps_other_detail_entries() {
        // Two unrelated issue numbers — invalidating one must not
        // touch the other. Unique constants keep the test isolated
        // from parallel siblings.
        let keep: u64 = 9_999_011;
        let drop: u64 = 9_999_012;
        ISSUE_DETAIL_CACHE
            .lock()
            .unwrap()
            .insert(keep, CacheEntry::fresh(fixture_issue(keep)));
        ISSUE_DETAIL_CACHE
            .lock()
            .unwrap()
            .insert(drop, CacheEntry::fresh(fixture_issue(drop)));
        invalidate_issue_cache(Some(drop));
        assert!(ISSUE_DETAIL_CACHE.lock().unwrap().contains_key(&keep));
        assert!(!ISSUE_DETAIL_CACHE.lock().unwrap().contains_key(&drop));
        // Cleanup so we don't leak into parallel tests.
        ISSUE_DETAIL_CACHE.lock().unwrap().remove(&keep);
    }

    #[test]
    fn global_invalidation_clears_a_planted_list_entry() {
        // Use a unique path so other parallel tests' global
        // invalidations don't race us between insert and assert.
        // Hold the lock across insert + read so no other thread can
        // clear it mid-flight.
        let key = list_key(
            Path::new("/codemux-test/global-invalidation-fixture-z9k"),
            None,
        );
        {
            let mut cache = ISSUE_LIST_CACHE.lock().unwrap();
            cache.insert(key.clone(), CacheEntry::fresh(vec![fixture_issue(1)]));
            assert!(cache.contains_key(&key));
        }
        invalidate_issue_cache(None);
        assert!(!ISSUE_LIST_CACHE.lock().unwrap().contains_key(&key));
    }
}
