//! GitLab adapter — the `glab` CLI.
//!
//! Where the GitHub adapter delegates to `crate::github`, this one owns
//! its logic: there is no pre-existing GitLab code to forward to. The
//! shape of that logic is nonetheless copied deliberately from the gh
//! path, because the callers are shared —
//!
//! - every read is gated on "CLI installed + authenticated" and returns
//!   `Err` when it cannot answer, so the pollers' preserve-vs-clear
//!   matrix (`github::branch_pr_outcome`) keeps working: only a
//!   successful *empty* result may clear a stored association;
//! - results are mapped onto the same structs the UI already speaks
//!   (`PullRequestInfo`, `GitHubIssue`, …) with the same vocabulary for
//!   states, check buckets and review decisions, so no component needs
//!   a GitLab branch;
//! - every subprocess runs with the keep-DBus environment sanitiser,
//!   because `glab` stores its token in the OS keyring exactly like `gh`
//!   and a nulled `DBUS_SESSION_BUS_ADDRESS` makes a logged-in user look
//!   logged out.
//!
//! Reads go through `glab api` rather than the porcelain subcommands.
//! Two reasons: the porcelain's `--output json` is glab's own view of a
//! merge request (a Go struct that has changed shape between releases),
//! while `glab api` returns the documented REST payload; and several of
//! the things Codemux needs — discussions, pipelines, per-file diffs —
//! have no porcelain equivalent at all. `glab api` still resolves the
//! host, the token and the project from the checkout's remote, which is
//! the whole reason to shell out to it instead of speaking HTTP here.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use serde_json::Value;

use super::cache::{TtlCache, DETAIL_TTL, LIST_TTL};
use super::detect::{run_git, DetectedProvider, ProviderKind};
use super::exec::{run_timed, TimedFailure};
use super::provider::{Capabilities, SourceControlProvider};
use crate::execution::{sanitize_gui_env_std, sanitize_gui_env_std_keep_dbus};
use crate::github::{
    CheckInfo, DeploymentInfo, GhStatus, GitHubIssue, IncomingPrItem, InlineReviewComment,
    IssueComment, IssueState, PrOverviewItem, PrsOverview, PullRequestInfo, ReviewComment,
    MAX_ISSUE_BODY_BYTES,
    MAX_ISSUE_COMMENTS, MAX_PR_DIFF_BYTES,
};

/// Matches `github::ISSUE_FETCH_TIMEOUT` — the deadline on a single
/// detail-shaped call, past which a wedged CLI must surface as an error
/// rather than pin the blocking pool.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Matches `github::INCOMING_PRS_TIMEOUT`. List queries against a busy
/// project are legitimately slower than a single-object fetch.
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// Page size for list queries, chosen to match the gh path's `--limit`
/// values (50 for a UI list, 100 for the branch lookup where a reused
/// branch name's older merge requests must not page out the newest).
const LIST_PAGE_SIZE: u32 = 50;
const BRANCH_LOOKUP_PAGE_SIZE: u32 = 100;

// ── Provider ────────────────────────────────────────────────────

/// Stateless handle; the detection fields are the only per-repository
/// context it carries, and they exist so error copy and the auth probe
/// can name the instance the checkout actually points at.
pub struct GitLabProvider {
    /// Bare hostname (no port, no userinfo) — see [`DetectedProvider`].
    host: Option<String>,
    /// Web origin, carrying a port when an http(s) remote named one.
    base_url: Option<String>,
}

impl GitLabProvider {
    pub fn from_detection(detected: &DetectedProvider) -> Self {
        Self {
            host: detected.host.clone(),
            base_url: detected.base_url.clone(),
        }
    }

    /// The host as `glab` spells it in its config: `host[:port]`.
    ///
    /// `DetectedProvider::host` drops the port, but a self-hosted
    /// instance on a non-default port is a whole separate entry in
    /// glab's hosts file (`localhost:8929`), so the port has to come
    /// back from the web origin. An SSH-only remote never produced one,
    /// in which case the bare host is returned and the auth probe falls
    /// back to prefix matching.
    fn auth_host(&self) -> Option<String> {
        if let Some(base_url) = &self.base_url {
            let authority = base_url
                .split_once("://")
                .map_or(base_url.as_str(), |(_, rest)| rest)
                .trim_end_matches('/');
            if !authority.is_empty() {
                return Some(authority.to_string());
            }
        }
        self.host.clone()
    }

    /// The gate every read applies before spending a network round trip,
    /// mirroring the `gh_available()` + `check_gh_status()` preamble on
    /// the GitHub path. `Err` here is a *failure to answer*, never an
    /// authoritative "no data".
    ///
    /// Memoized per instance, success only: a ready verdict costs two
    /// subprocesses (`which` + an auth round trip that may reach the
    /// keyring), and the detail view alone runs four API calls that
    /// would each pay for it. A signed-out user is never memoized, so
    /// the actionable "glab auth login" message is always the first
    /// thing they see; the cost of the reverse case is that a user who
    /// signs out mid-session gets GitLab's own auth error for up to
    /// [`READY_TTL`] instead of ours.
    fn require_ready(&self) -> Result<(), String> {
        let key = self.auth_host().unwrap_or_default();
        READY_CACHE.get_or_fetch(&key, READY_TTL, || match self.auth_status() {
            GhStatus::NotInstalled => Err(NOT_INSTALLED.to_string()),
            GhStatus::NotAuthenticated => Err(NOT_AUTHENTICATED.to_string()),
            GhStatus::Authenticated { .. } => Ok(()),
        })
    }

    fn api(&self, repo_path: &Path, endpoint: &str, timeout: Duration) -> Result<Value, String> {
        self.require_ready()?;
        run_glab_json(repo_path, &["api", endpoint], timeout)
    }
}

const NOT_INSTALLED: &str = "glab CLI is not installed";
const NOT_AUTHENTICATED: &str = "glab CLI is not authenticated. Run: glab auth login";

// ── Subprocess plumbing ─────────────────────────────────────────

fn glab_available() -> bool {
    let mut cmd = crate::execution::host_command("which");
    cmd.arg("glab");
    sanitize_gui_env_std(&mut cmd);
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

/// A `glab` invocation, with the keyring-safe environment every call
/// needs: glab pulls its token from the user's secret-service keyring, so
/// a nulled `DBUS_SESSION_BUS_ADDRESS` makes a logged-in user read as
/// logged out. See `auth_status`.
fn glab_command(args: &[&str]) -> std::process::Command {
    let mut cmd = crate::execution::host_command("glab");
    cmd.args(args);
    sanitize_gui_env_std_keep_dbus(&mut cmd);
    cmd
}

/// Map a runner failure onto the message shape glab's callers expect.
fn glab_failure(failure: TimedFailure, timeout: Duration) -> String {
    match failure {
        TimedFailure::Spawn(e) => format!("Failed to run glab: {e}"),
        TimedFailure::Wait(e) => format!("Failed to wait for glab: {e}"),
        TimedFailure::Timeout => {
            format!("glab command timed out after {}s", timeout.as_secs())
        }
    }
}

/// Run `glab` with a deadline, mirroring `github::run_gh_timed`.
///
/// A non-zero exit is an `Err` even when stdout parsed — the pollers key
/// their preserve-vs-clear decision on exactly that distinction.
fn run_glab_timed(repo_path: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = glab_command(args);
    cmd.current_dir(repo_path);
    let output = run_timed(cmd, timeout).map_err(|e| glab_failure(e, timeout))?;

    if output.success {
        return Ok(output.stdout.trim_end().to_string());
    }
    Err(format!(
        "glab {} failed: {}",
        args.first().unwrap_or(&""),
        // glab prints API errors as a JSON body on stdout and decoration
        // on stderr; prefer whichever is non-empty so the message is not
        // just "glab api failed: ".
        first_non_empty(&output.stderr, &output.stdout),
    ))
}

fn first_non_empty(primary: &str, fallback: &str) -> String {
    let primary = primary.trim();
    if !primary.is_empty() {
        return primary.to_string();
    }
    fallback.trim().to_string()
}

fn run_glab_json(repo_path: &Path, args: &[&str], timeout: Duration) -> Result<Value, String> {
    let output = run_glab_timed(repo_path, args, timeout)?;
    if output.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&output).map_err(|e| format!("Failed to parse glab JSON: {e}"))
}

// ── Auth status parsing ─────────────────────────────────────────

/// What `glab auth status` said about one instance.
///
/// Tri-state on purpose. `glab auth status` prints human text, not
/// JSON, and its layout is not a stable interface — so "the output did
/// not look like anything we recognise" has to be distinguishable from
/// "we read it and the user is logged out", or an upstream cosmetic
/// change would silently log every user out of their GitLab UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthOutcome {
    Authenticated { username: String },
    NotAuthenticated,
    /// Nothing host-block-shaped was found. Callers fall back to the
    /// process exit status, which is the signal that does not depend on
    /// wording.
    Unknown,
}

/// Parse `glab auth status` output, optionally scoped to one instance.
///
/// The output is a sequence of blocks, each introduced by an unindented
/// hostname line and followed by indented detail lines, one of which
/// reads `✓ Logged in to <host> as <user> (<source>)` when that
/// instance has a working token. Anything else in a block — including
/// glab's trailing error banner, which is indented and therefore lands
/// inside the last block — is ignored.
///
/// `want_host` is matched case-insensitively, first exactly and then
/// against the block host with its port removed, because an SSH-only
/// remote cannot tell us which port the web UI listens on.
pub(crate) fn parse_auth_status(text: &str, want_host: Option<&str>) -> AuthOutcome {
    let mut blocks: Vec<(String, Option<String>)> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let is_block_header = !line.starts_with(char::is_whitespace);
        if is_block_header {
            blocks.push((trimmed.to_ascii_lowercase(), None));
            continue;
        }
        if let Some(user) = logged_in_username(trimmed) {
            if let Some(block) = blocks.last_mut() {
                if block.1.is_none() {
                    block.1 = Some(user);
                }
            }
        }
    }

    if blocks.is_empty() {
        return AuthOutcome::Unknown;
    }

    let matched = match want_host {
        Some(want) => {
            let want = want.to_ascii_lowercase();
            blocks
                .iter()
                .find(|(host, _)| *host == want)
                .or_else(|| {
                    blocks.iter().find(|(host, _)| {
                        host.split(':').next().unwrap_or(host) == want.split(':').next().unwrap_or(&want)
                    })
                })
        }
        // Unscoped: any logged-in instance answers the question, since
        // there is no particular one we were asked about.
        None => blocks
            .iter()
            .find(|(_, user)| user.is_some())
            .or_else(|| blocks.first()),
    };

    match matched {
        Some((_, Some(user))) => AuthOutcome::Authenticated {
            username: user.clone(),
        },
        // A block we found and read, or a host glab has no block for at
        // all: either way there is no usable token for it.
        Some((_, None)) | None => AuthOutcome::NotAuthenticated,
    }
}

/// Turn one `glab auth status` run into the tri-state the trait returns.
///
/// Which signal is authoritative depends on whether the probe was
/// host-scoped:
///
/// - **Scoped** (`--hostname`): the exit status is about that one
///   instance, so a non-zero exit is a real "not logged in here".
/// - **Unscoped**: the exit status is an AND across every configured
///   instance, so one stale entry makes glab exit non-zero even while
///   another host is perfectly logged in. The printed report is the only
///   thing that can tell them apart, so it is read first and the exit
///   code is only the fallback.
///
/// Pure so the multi-host case is testable without a live `glab`.
pub(crate) fn auth_verdict(text: &str, want_host: Option<&str>, exit_success: bool) -> GhStatus {
    let outcome = parse_auth_status(text, want_host);

    if !exit_success {
        if want_host.is_none() {
            if let AuthOutcome::Authenticated { username } = outcome {
                return GhStatus::Authenticated { username };
            }
        }
        return GhStatus::NotAuthenticated;
    }

    match outcome {
        AuthOutcome::Authenticated { username } => GhStatus::Authenticated { username },
        AuthOutcome::NotAuthenticated => GhStatus::NotAuthenticated,
        // A zero exit is the authoritative answer; unparseable wording
        // only costs us the username, which the gh path also leaves
        // empty when it cannot find one.
        AuthOutcome::Unknown => GhStatus::Authenticated {
            username: String::new(),
        },
    }
}

/// Username out of a `Logged in to <host> as <user> (<source>)` line.
fn logged_in_username(line: &str) -> Option<String> {
    if !line.contains("Logged in to") {
        return None;
    }
    let after = line.split(" as ").nth(1)?;
    let user = after.split_whitespace().next()?;
    // Defensive: an empty or parenthesised token means the wording
    // moved and we did not actually capture a name.
    if user.is_empty() || user.starts_with('(') {
        return None;
    }
    Some(user.to_string())
}

// ── State / status vocabularies ─────────────────────────────────

/// GitLab merge-request state → the string the UI already renders.
///
/// The UI's vocabulary is gh's: uppercase `OPEN` / `MERGED` / `CLOSED`,
/// with draft carried separately on `is_draft` and folded in by
/// `PullRequestInfo::display_state`. GitLab has no draft *state* either
/// — it is a boolean on the merge request — so the two models line up
/// exactly and nothing has to be invented here.
pub(crate) fn normalize_mr_state(state: &str) -> String {
    match state.trim().to_ascii_lowercase().as_str() {
        "opened" | "open" | "reopened" => "OPEN".to_string(),
        "merged" => "MERGED".to_string(),
        "closed" | "locked" => "CLOSED".to_string(),
        // Never silently claim OPEN for a state a future GitLab adds:
        // an unrecognised value is passed through uppercased, which the
        // UI renders literally and `is_historical_pr_state` treats as
        // non-terminal (the safe side — nothing gets archived by
        // accident).
        other => other.to_ascii_uppercase(),
    }
}

/// GitLab job / commit-status state → the bucket vocabulary the checks
/// UI understands (`pass` / `fail` / `pending` / `skipping` / `cancel`,
/// the same set gh's `bucket` field emits).
pub(crate) fn check_bucket(status: &str) -> &'static str {
    match status.trim().to_ascii_lowercase().as_str() {
        "success" | "passed" => "pass",
        "failed" => "fail",
        "canceled" | "cancelled" => "cancel",
        "skipped" | "manual" => "skipping",
        _ => "pending",
    }
}

// ── JSON mapping ────────────────────────────────────────────────

/// Merge request JSON → `PullRequestInfo`.
///
/// `number` is the project-scoped `iid`, not the instance-wide `id`:
/// the iid is what the web UI, the `refs/merge-requests/<n>/head` ref
/// and every user-facing "!123" reference mean.
pub(crate) fn parse_mr_json(v: &Value) -> PullRequestInfo {
    PullRequestInfo {
        number: v["iid"].as_u64().unwrap_or(0) as u32,
        url: v["web_url"].as_str().unwrap_or("").to_string(),
        state: normalize_mr_state(v["state"].as_str().unwrap_or("opened")),
        title: v["title"].as_str().unwrap_or("").to_string(),
        head_branch: v["source_branch"].as_str().map(|s| s.to_string()),
        base_branch: v["target_branch"].as_str().map(|s| s.to_string()),
        // `work_in_progress` is the pre-14.0 spelling, still emitted.
        is_draft: v["draft"]
            .as_bool()
            .or_else(|| v["work_in_progress"].as_bool())
            .unwrap_or(false),
        mergeable: mergeable_label(v),
        // Diff stats are not on the REST merge-request payload at all;
        // the detail path fills them in separately. List rows leave them
        // empty rather than pay a round trip per row.
        additions: None,
        deletions: None,
        review_decision: None,
        checks_passing: None,
        updated_at: v["updated_at"].as_str().map(|s| s.to_string()),
        head_ref_oid: v["sha"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        // Cross-project merge requests differ from the target project by
        // `source_project_id`; the owning namespace is not on this
        // payload, and the branch lookup does not need it (GitLab scopes
        // `source_branch` to the target project's merge requests, so the
        // fork ambiguity gh has to disambiguate client-side cannot
        // arise here).
        head_repository_owner: None,
        body: None,
        comments: Vec::new(),
        total_comments: 0,
        author: v["author"]["username"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        merge_state_status: merge_state_label(v),
        // GitLab reports this as a string ("3"), and as "3+" once the
        // diff exceeds the project's limit — take the leading digits.
        changed_files: v["changes_count"].as_str().and_then(|s| {
            s.trim_end_matches('+')
                .parse::<u32>()
                .ok()
        }),
        merged_by: v["merged_by"]["username"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        merged_at: v["merged_at"].as_str().map(|s| s.to_string()),
        review_requests: v["reviewers"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| r["username"].as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default(),
        // Approvals are a separate endpoint on GitLab; the aggregate
        // already arrives via `review_decision` on the detail path, and
        // paying a round trip per list row for per-reviewer verdicts is
        // not worth it.
        latest_reviews: Vec::new(),
    }
}

/// `mergeStateStatus` in gh's vocabulary, so the action bar can name one
/// blocking reason regardless of which product answered.
fn merge_state_label(v: &Value) -> Option<String> {
    if v["has_conflicts"].as_bool() == Some(true) {
        return Some("DIRTY".to_string());
    }
    match v["detailed_merge_status"]
        .as_str()
        .or_else(|| v["merge_status"].as_str())?
    {
        "mergeable" | "can_be_merged" => Some("CLEAN".to_string()),
        "conflict" | "broken_status" | "cannot_be_merged" | "cannot_be_merged_recheck" => {
            Some("DIRTY".to_string())
        }
        "ci_still_running" | "ci_must_pass" => Some("UNSTABLE".to_string()),
        "not_approved" | "blocked_status" | "draft_status" | "discussions_not_resolved" => {
            Some("BLOCKED".to_string())
        }
        "need_rebase" => Some("BEHIND".to_string()),
        _ => Some("UNKNOWN".to_string()),
    }
}

/// `mergeable` in gh's vocabulary (`MERGEABLE` / `CONFLICTING` /
/// `UNKNOWN`), which is what the attachment block prints.
fn mergeable_label(v: &Value) -> Option<String> {
    if v["has_conflicts"].as_bool() == Some(true) {
        return Some("CONFLICTING".to_string());
    }
    match v["detailed_merge_status"]
        .as_str()
        .or_else(|| v["merge_status"].as_str())?
    {
        "mergeable" | "can_be_merged" => Some("MERGEABLE".to_string()),
        "conflict" | "broken_status" | "cannot_be_merged" | "cannot_be_merged_recheck" => {
            Some("CONFLICTING".to_string())
        }
        _ => Some("UNKNOWN".to_string()),
    }
}

fn parse_incoming_mr_json(v: &Value) -> IncomingPrItem {
    IncomingPrItem {
        number: v["iid"].as_u64().unwrap_or(0) as u32,
        title: v["title"].as_str().unwrap_or("").to_string(),
        author: v["author"]["username"].as_str().unwrap_or("").to_string(),
        head_branch: v["source_branch"].as_str().map(|s| s.to_string()),
        is_draft: v["draft"]
            .as_bool()
            .or_else(|| v["work_in_progress"].as_bool())
            .unwrap_or(false),
        additions: None,
        deletions: None,
        review_decision: None,
        // Same trade the gh path makes with `statusCheckRollup`: an
        // aggregate CI state per row costs a request per row, so the
        // overview ships without a dot and the detail view fetches
        // checks when the user opens one.
        checks_status: None,
        updated_at: v["updated_at"].as_str().map(|s| s.to_string()),
        url: v["web_url"].as_str().unwrap_or("").to_string(),
    }
}

/// A merge request's pipeline reduced to the page's four-word CI
/// vocabulary.
///
/// Only what the list endpoint already carries is read. A per-MR
/// pipeline lookup would be a request per row, which is exactly the cost
/// the reduced rollup exists to avoid.
fn mr_rollup_state(v: &Value) -> String {
    let status = v["head_pipeline"]["status"]
        .as_str()
        .or_else(|| v["pipeline"]["status"].as_str())
        .unwrap_or("");
    match status {
        "success" | "manual" => "passing".to_string(),
        "failed" => "failing".to_string(),
        "running" | "pending" | "created" | "waiting_for_resource" | "preparing" | "scheduled" => {
            "pending".to_string()
        }
        // canceled / skipped / absent: there is nothing to report, and a
        // grey dot that means "cancelled" and a grey dot that means "no
        // pipeline" are the same dot.
        _ => "none".to_string(),
    }
}

/// Approval state in the vocabulary the row labels read.
///
/// GitLab has no request-changes verdict (see the capability table), so
/// this is a two-state answer at most. `approved` is only present on
/// some instances/tiers, so it is read defensively and the merge-status
/// hint is the fallback — neither is invented when GitLab is silent.
fn mr_review_decision(v: &Value) -> Option<String> {
    if v["approved"].as_bool() == Some(true) {
        return Some("APPROVED".to_string());
    }
    match v["detailed_merge_status"].as_str() {
        Some("not_approved") => Some("REVIEW_REQUIRED".to_string()),
        _ => None,
    }
}

fn parse_overview_mr_json(v: &Value) -> PrOverviewItem {
    let reviewers = v["reviewers"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    r["username"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();

    PrOverviewItem {
        number: v["iid"].as_u64().unwrap_or(0) as u32,
        title: v["title"].as_str().unwrap_or("").to_string(),
        author: v["author"]["username"].as_str().unwrap_or("").to_string(),
        head_branch: v["source_branch"].as_str().map(|s| s.to_string()),
        is_draft: v["draft"]
            .as_bool()
            .or_else(|| v["work_in_progress"].as_bool())
            .unwrap_or(false),
        // Line counts are a diff-stat call per merge request on GitLab;
        // the row simply omits them rather than paying 50 round trips.
        additions: None,
        deletions: None,
        review_decision: mr_review_decision(v),
        checks: mr_rollup_state(v),
        review_requested_from: reviewers,
        updated_at: v["updated_at"].as_str().map(|s| s.to_string()),
        url: v["web_url"].as_str().unwrap_or("").to_string(),
    }
}

fn parse_issue_json(v: &Value) -> GitHubIssue {
    let labels = v["labels"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let assignees = v["assignees"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a["username"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    GitHubIssue {
        number: v["iid"].as_u64().unwrap_or(0),
        title: v["title"].as_str().unwrap_or("").to_string(),
        state: match v["state"].as_str().unwrap_or("opened") {
            "closed" => IssueState::Closed,
            _ => IssueState::Open,
        },
        labels,
        assignees,
        url: v["web_url"].as_str().unwrap_or("").to_string(),
        body: None,
        comments: Vec::new(),
        total_comments: 0,
        updated_at: v["updated_at"].as_str().map(|s| s.to_string()),
    }
}

/// Truncate a body at `MAX_ISSUE_BODY_BYTES` on a char boundary, with
/// the same marker the gh path appends.
fn truncate_body(body: &str) -> String {
    if body.len() <= MAX_ISSUE_BODY_BYTES {
        return body.to_string();
    }
    let mut end = MAX_ISSUE_BODY_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…\n\n[Body truncated at 50KB]", &body[..end])
}

/// `notes` payload → the capped comment list plus the true total.
///
/// System notes ("changed the description", "added 1 commit") and
/// diff-anchored notes are dropped: the first are timeline noise the gh
/// path never had, and the second belong to the inline-comment surface.
pub(crate) fn parse_notes(v: &Value) -> (Vec<IssueComment>, u32) {
    let Some(arr) = v.as_array() else {
        return (Vec::new(), 0);
    };
    let conversation: Vec<&Value> = arr
        .iter()
        .filter(|n| n["system"].as_bool() != Some(true))
        .filter(|n| n["position"].is_null())
        .filter(|n| !n["body"].as_str().unwrap_or("").is_empty())
        .collect();

    let total = conversation.len() as u32;
    let comments = conversation
        .into_iter()
        .take(MAX_ISSUE_COMMENTS)
        .map(|n| IssueComment {
            author: n["author"]["username"].as_str().unwrap_or("").to_string(),
            body: n["body"].as_str().unwrap_or("").to_string(),
            created_at: n["created_at"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    (comments, total)
}

/// Split a `discussions` payload into the two surfaces the review tab
/// renders: conversation threads and diff-anchored comments.
///
/// GitLab models both as notes inside a discussion, distinguished by the
/// presence of a `position`. GitHub models them as two separate API
/// resources, which is why the trait has two methods; the split happens
/// here so one request serves both.
///
/// Threading: GitHub gives every inline comment a `pull_request_review_id`
/// (the review it was submitted with) and an `in_reply_to_id`. GitLab has
/// no review object, but a discussion is the same grouping, so the first
/// note's id stands in for the review id and every later note in the same
/// discussion is marked as replying to it. That is exactly what the review
/// tab groups on.
pub(crate) fn split_discussions(v: &Value) -> (Vec<ReviewComment>, Vec<InlineReviewComment>) {
    let mut threads = Vec::new();
    let mut inline = Vec::new();

    let Some(discussions) = v.as_array() else {
        return (threads, inline);
    };

    for discussion in discussions {
        let Some(notes) = discussion["notes"].as_array() else {
            continue;
        };
        let root_id = notes.first().and_then(|n| n["id"].as_u64());

        for (index, note) in notes.iter().enumerate() {
            if note["system"].as_bool() == Some(true) {
                continue;
            }
            let body = note["body"].as_str().unwrap_or("").to_string();
            if body.is_empty() {
                continue;
            }
            let id = note["id"].as_u64().unwrap_or(0);
            let author = note["author"]["username"].as_str().unwrap_or("").to_string();
            let created_at = note["created_at"].as_str().unwrap_or("").to_string();
            let position = &note["position"];

            if position.is_null() {
                threads.push(ReviewComment {
                    id,
                    author,
                    body,
                    // GitLab CE has no per-review verdict on a note; an
                    // approval is a separate object (see
                    // `review_decision`), so every note is a comment.
                    state: "COMMENTED".to_string(),
                    created_at,
                });
            } else {
                inline.push(InlineReviewComment {
                    id,
                    author,
                    body,
                    // A note on a deleted line has only `old_path`.
                    path: position["new_path"]
                        .as_str()
                        .or_else(|| position["old_path"].as_str())
                        .unwrap_or("")
                        .to_string(),
                    line: position["new_line"]
                        .as_u64()
                        .or_else(|| position["old_line"].as_u64())
                        .map(|n| n as u32),
                    created_at,
                    in_reply_to_id: if index == 0 { None } else { root_id },
                    pull_request_review_id: root_id,
                });
            }
        }
    }

    (threads, inline)
}

/// Pipeline jobs (or, for an externally reported pipeline, the commit
/// statuses that stand in for them) → the checks list.
pub(crate) fn parse_checks(jobs: &Value) -> Vec<CheckInfo> {
    let Some(arr) = jobs.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|job| {
            let status = job["status"].as_str().unwrap_or("pending");
            CheckInfo {
                name: job["name"].as_str().unwrap_or("").to_string(),
                status: status.to_string(),
                conclusion: Some(check_bucket(status).to_string()),
                // Not derived from started/finished for the same reason
                // the gh path leaves it empty: the UI formats its own
                // elapsed time when it wants one.
                elapsed_time: None,
                detail_url: job["web_url"]
                    .as_str()
                    .or_else(|| job["target_url"].as_str())
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty()),
                started_at: job["started_at"].as_str().map(|s| s.to_string()),
                completed_at: job["finished_at"].as_str().map(|s| s.to_string()),
            }
        })
        .collect()
}

/// Pick the merge request that represents a branch.
///
/// Open beats historical, then most recently updated wins — the same
/// preference `github::select_branch_pr` applies, and for the same
/// reason: a branch name gets reused, and the newest merge request on it
/// is the one the user is looking at.
pub(crate) fn select_branch_mr(rows: &[Value]) -> Option<PullRequestInfo> {
    let mut best: Option<PullRequestInfo> = None;
    for row in rows {
        let candidate = parse_mr_json(row);
        let better = match &best {
            None => true,
            Some(current) => {
                let candidate_open = candidate.state == "OPEN";
                let current_open = current.state == "OPEN";
                if candidate_open != current_open {
                    candidate_open
                } else {
                    candidate.updated_at > current.updated_at
                }
            }
        };
        if better {
            best = Some(candidate);
        }
    }
    best
}

// ── Caches ──────────────────────────────────────────────────────
//
// Same TTLs as the GitHub path (see `super::cache`). Keys are
// repository-path-first so one repository's entries can be dropped
// without touching another's.

/// How long a *successful* readiness verdict is reused. Short enough
/// that a session-long staleness cannot build up, long enough that a
/// burst of calls from one panel render pays for it once.
const READY_TTL: Duration = Duration::from_secs(60);

/// Keyed by instance (`host[:port]`), because two self-hosted GitLabs
/// are two separate logins.
static READY_CACHE: LazyLock<TtlCache<()>> = LazyLock::new(TtlCache::new);

static MR_LIST_CACHE: LazyLock<TtlCache<Vec<PullRequestInfo>>> = LazyLock::new(TtlCache::new);
static MR_DETAIL_CACHE: LazyLock<TtlCache<PullRequestInfo>> = LazyLock::new(TtlCache::new);
static MR_DIFF_CACHE: LazyLock<TtlCache<String>> = LazyLock::new(TtlCache::new);
static ISSUE_LIST_CACHE: LazyLock<TtlCache<Vec<GitHubIssue>>> = LazyLock::new(TtlCache::new);
static ISSUE_DETAIL_CACHE: LazyLock<TtlCache<GitHubIssue>> = LazyLock::new(TtlCache::new);

fn cache_key(repo_path: &Path, parts: &[&str]) -> String {
    format!("{}|{}", repo_path.display(), parts.join("|"))
}

/// Drop every cached GitLab read for one repository, or all of them when
/// `None`. Called after a write (create / merge) so the next read cannot
/// serve a value from before it.
pub fn invalidate_cache(repo_path: Option<&Path>) {
    match repo_path {
        Some(path) => {
            let prefix = format!("{}|", path.display());
            MR_LIST_CACHE.invalidate_prefix(&prefix);
            MR_DETAIL_CACHE.invalidate_prefix(&prefix);
            MR_DIFF_CACHE.invalidate_prefix(&prefix);
            ISSUE_LIST_CACHE.invalidate_prefix(&prefix);
            ISSUE_DETAIL_CACHE.invalidate_prefix(&prefix);
        }
        None => {
            MR_LIST_CACHE.clear();
            MR_DETAIL_CACHE.clear();
            MR_DIFF_CACHE.clear();
            ISSUE_LIST_CACHE.clear();
            ISSUE_DETAIL_CACHE.clear();
        }
    }
}

// ── Trait implementation ────────────────────────────────────────

impl SourceControlProvider for GitLabProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GitLab
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            has_pull_requests: true,
            has_checks: true,
            has_issues: true,
            has_inline_comments: true,
            has_review_threads: true,
            // GitLab's environments/deployments model does not answer
            // "which deployments belong to this merge request" the way
            // the panel asks it, so this is not served at all rather
            // than served wrongly.
            has_deployments: false,
            has_reviews: true,
            has_fork_pr_fetch: true,
        }
    }

    fn cli_available(&self) -> bool {
        glab_available()
    }

    fn auth_status(&self) -> GhStatus {
        if !glab_available() {
            return GhStatus::NotInstalled;
        }

        let host = self.auth_host();
        let mut args: Vec<&str> = vec!["auth", "status"];
        // Scope to this checkout's instance. Without it the exit status
        // is an AND across every configured instance — one stale
        // gitlab.com entry would report a working self-hosted login as
        // failed.
        if let Some(host) = &host {
            args.push("--hostname");
            args.push(host);
        }

        // Bounded like every other shell-out: an instance that accepts
        // the connection and never answers would otherwise hang the
        // caller, which for the workspace poller is the whole tick.
        let Ok(output) = run_timed(glab_command(&args), CALL_TIMEOUT) else {
            return GhStatus::NotAuthenticated;
        };

        // glab writes the report to stderr; read both streams so a
        // future move to stdout does not break this.
        let text = format!("{}{}", output.stderr, output.stdout);
        auth_verdict(&text, host.as_deref(), output.success)
    }

    fn branch_pull_request(&self, repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
        let Some(branch) = run_git(repo_path, &["branch", "--show-current"]) else {
            return Err("detached HEAD: no branch to resolve a merge request for".to_string());
        };
        let endpoint = format!(
            "projects/:id/merge_requests?source_branch={}&state=all&per_page={}&order_by=updated_at&sort=desc",
            urlencoding::encode(&branch),
            BRANCH_LOOKUP_PAGE_SIZE,
        );
        let value = self.api(repo_path, &endpoint, CALL_TIMEOUT)?;
        let rows = value
            .as_array()
            .ok_or_else(|| "Expected JSON array from merge request list".to_string())?;
        Ok(select_branch_mr(rows))
    }

    fn workspace_pull_request(&self, repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
        // No side-branch fallback: the gh path's reflog scan exists to
        // work around `gh pr list --head` not matching fork branches,
        // and the branch query here is already project-scoped and exact.
        self.branch_pull_request(repo_path)
    }

    fn list_pull_requests(
        &self,
        repo_path: &Path,
        state: &str,
    ) -> Result<Vec<PullRequestInfo>, String> {
        let gitlab_state = match state.to_ascii_lowercase().as_str() {
            "open" | "opened" => "opened",
            "closed" => "closed",
            "merged" => "merged",
            _ => "all",
        };
        let key = cache_key(repo_path, &["mr-list", gitlab_state]);
        MR_LIST_CACHE.get_or_fetch(&key, LIST_TTL, || {
            let endpoint = format!(
                "projects/:id/merge_requests?state={gitlab_state}&per_page={LIST_PAGE_SIZE}&order_by=updated_at&sort=desc"
            );
            let value = self.api(repo_path, &endpoint, LIST_TIMEOUT)?;
            let rows = value
                .as_array()
                .ok_or_else(|| "Expected JSON array from merge request list".to_string())?;
            Ok(rows.iter().map(parse_mr_json).collect())
        })
    }

    fn list_incoming_pull_requests(
        &self,
        repo_path: &Path,
        base_branch: &str,
    ) -> Result<Vec<IncomingPrItem>, String> {
        let endpoint = format!(
            "projects/:id/merge_requests?target_branch={}&state=opened&per_page={}&order_by=updated_at&sort=desc",
            urlencoding::encode(base_branch),
            LIST_PAGE_SIZE,
        );
        let value = self.api(repo_path, &endpoint, LIST_TIMEOUT)?;
        let rows = value
            .as_array()
            .ok_or_else(|| "Expected JSON array from merge request list".to_string())?;
        Ok(rows.iter().map(parse_incoming_mr_json).collect())
    }

    fn pull_requests_overview(&self, repo_path: &Path) -> Result<PrsOverview, String> {
        let endpoint = format!(
            "projects/:id/merge_requests?state=opened&per_page={LIST_PAGE_SIZE}&order_by=updated_at&sort=desc"
        );
        let value = self.api(repo_path, &endpoint, LIST_TIMEOUT)?;
        let rows = value
            .as_array()
            .ok_or_else(|| "Expected JSON array from merge request list".to_string())?;
        let viewer = match self.auth_status() {
            GhStatus::Authenticated { username } if !username.is_empty() => Some(username),
            _ => None,
        };
        Ok(PrsOverview {
            viewer,
            items: rows.iter().map(parse_overview_mr_json).collect(),
        })
    }

    fn get_pull_request(&self, repo_path: &Path, number: u32) -> Result<PullRequestInfo, String> {
        let key = cache_key(repo_path, &["mr", &number.to_string()]);
        MR_DETAIL_CACHE.get_or_fetch(&key, DETAIL_TTL, || {
            let value = self.api(
                repo_path,
                &format!("projects/:id/merge_requests/{number}"),
                CALL_TIMEOUT,
            )?;
            let mut mr = parse_mr_json(&value);

            if let Some(body) = value["description"].as_str() {
                mr.body = Some(truncate_body(body));
            }

            let notes = self.api(
                repo_path,
                &format!("projects/:id/merge_requests/{number}/notes?per_page=100&sort=asc&order_by=created_at"),
                CALL_TIMEOUT,
            )?;
            let (comments, total) = parse_notes(&notes);
            mr.comments = comments;
            mr.total_comments = total;

            // Both of these are decoration on an otherwise complete
            // merge request, and each costs its own round trip, so a
            // failure leaves the field empty instead of failing the
            // fetch.
            mr.review_decision = self.review_decision(repo_path, number);
            if let Some((additions, deletions)) = self.diff_stats(repo_path, &value) {
                mr.additions = Some(additions);
                mr.deletions = Some(deletions);
            }

            Ok(mr)
        })
    }

    fn create_pull_request(
        &self,
        repo_path: &Path,
        title: &str,
        body: &str,
        base: Option<&str>,
        draft: bool,
    ) -> Result<PullRequestInfo, String> {
        self.require_ready()?;
        let Some(source_branch) = run_git(repo_path, &["branch", "--show-current"]) else {
            return Err("detached HEAD: no branch to open a merge request from".to_string());
        };
        let target_branch = match base {
            Some(base) => base.to_string(),
            None => self.default_branch(repo_path)?,
        };
        // GitLab has no draft flag on create; a `Draft:` title prefix is
        // the documented mechanism and is what sets `draft: true` on the
        // returned object.
        let title = if draft && !title.trim_start().to_ascii_lowercase().starts_with("draft:") {
            format!("Draft: {title}")
        } else {
            title.to_string()
        };

        let value = run_glab_json(
            repo_path,
            &[
                "api",
                "--method",
                "POST",
                "projects/:id/merge_requests",
                "-f",
                &format!("source_branch={source_branch}"),
                "-f",
                &format!("target_branch={target_branch}"),
                "-f",
                &format!("title={title}"),
                "-f",
                &format!("description={body}"),
            ],
            CALL_TIMEOUT,
        )?;
        invalidate_cache(Some(repo_path));
        Ok(parse_mr_json(&value))
    }

    fn merge_pull_request(
        &self,
        repo_path: &Path,
        number: u32,
        method: &str,
        delete_branch: bool,
        commit_title: Option<&str>,
        commit_body: Option<&str>,
    ) -> Result<(), String> {
        self.require_ready()?;
        let number_str = number.to_string();
        let mut args: Vec<&str> = vec![
            "mr",
            "merge",
            &number_str,
            "--yes",
            // `glab mr merge` defaults `--auto-merge` to true, which
            // queues the merge behind the pipeline instead of
            // performing it. The GitHub path merges now, so this must
            // be turned off explicitly or the button would appear to do
            // nothing on any project with CI.
            "--auto-merge=false",
        ];
        if delete_branch {
            // Mirrors `gh pr merge --delete-branch`.
            args.push("--remove-source-branch");
        }
        match method {
            "squash" => args.push("--squash"),
            "rebase" => args.push("--rebase"),
            _ => {}
        }
        // glab spells the squash-commit subject `--squash-message` and
        // the merge-commit subject `--message`; neither applies to a
        // rebase, which produces no commit of its own.
        let title = commit_title.map(str::trim).filter(|s| !s.is_empty());
        let body = commit_body.map(str::trim).filter(|s| !s.is_empty());
        let combined = match (title, body) {
            (Some(t), Some(b)) => Some(format!("{t}\n\n{b}")),
            (Some(t), None) => Some(t.to_string()),
            _ => None,
        };
        if let Some(message) = combined.as_deref() {
            match method {
                "squash" => {
                    args.push("--squash-message");
                    args.push(message);
                }
                "rebase" => {}
                _ => {
                    args.push("--message");
                    args.push(message);
                }
            }
        }
        run_glab_timed(repo_path, &args, LIST_TIMEOUT)?;
        invalidate_cache(Some(repo_path));
        Ok(())
    }

    fn close_pull_request(&self, repo_path: &Path, number: u32) -> Result<(), String> {
        self.require_ready()?;
        run_glab_timed(repo_path, &["mr", "close", &number.to_string()], CALL_TIMEOUT)?;
        invalidate_cache(Some(repo_path));
        Ok(())
    }

    fn reopen_pull_request(&self, repo_path: &Path, number: u32) -> Result<(), String> {
        self.require_ready()?;
        run_glab_timed(
            repo_path,
            &["mr", "reopen", &number.to_string()],
            CALL_TIMEOUT,
        )?;
        invalidate_cache(Some(repo_path));
        Ok(())
    }

    /// GitLab has no draft flag: draft state is a `Draft:` title prefix,
    /// which is also how `create_pull_request` sets it. Going ready
    /// strips the prefix; going back to draft restores it.
    fn set_pull_request_ready(
        &self,
        repo_path: &Path,
        number: u32,
        ready: bool,
    ) -> Result<(), String> {
        self.require_ready()?;
        let number_str = number.to_string();
        let args: Vec<&str> = if ready {
            vec!["mr", "update", &number_str, "--ready"]
        } else {
            vec!["mr", "update", &number_str, "--draft"]
        };
        run_glab_timed(repo_path, &args, CALL_TIMEOUT)?;
        invalidate_cache(Some(repo_path));
        Ok(())
    }

    fn update_pull_request(
        &self,
        repo_path: &Path,
        number: u32,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<(), String> {
        self.require_ready()?;
        let number_str = number.to_string();
        let mut args: Vec<&str> = vec!["mr", "update", &number_str];
        if let Some(title) = title {
            args.push("--title");
            args.push(title);
        }
        if let Some(body) = body {
            args.push("--description");
            args.push(body);
        }
        if args.len() == 3 {
            return Ok(());
        }
        run_glab_timed(repo_path, &args, CALL_TIMEOUT)?;
        invalidate_cache(Some(repo_path));
        Ok(())
    }

    fn request_pull_request_review(
        &self,
        repo_path: &Path,
        number: u32,
        reviewer: &str,
    ) -> Result<(), String> {
        let reviewer = reviewer.trim();
        if reviewer.is_empty() {
            return Err("A reviewer name is required.".to_string());
        }
        self.require_ready()?;
        run_glab_timed(
            repo_path,
            &["mr", "update", &number.to_string(), "--reviewer", reviewer],
            CALL_TIMEOUT,
        )?;
        invalidate_cache(Some(repo_path));
        Ok(())
    }

    /// GitLab job traces are a per-job endpoint, and mapping a check
    /// name back to a job id costs two more round trips than the card
    /// is worth. The UI renders the failing-check card without an
    /// excerpt when this is empty, which is the same path a GitHub run
    /// with no matching log takes.
    fn check_log_excerpt(
        &self,
        _repo_path: &Path,
        _number: u32,
        _check_name: &str,
    ) -> Result<String, String> {
        Ok(String::new())
    }

    fn pull_request_diff(
        &self,
        repo_path: &Path,
        number: u32,
        full: bool,
    ) -> Result<String, String> {
        let key = cache_key(repo_path, &["diff", &number.to_string(), &full.to_string()]);
        MR_DIFF_CACHE.get_or_fetch(&key, DETAIL_TTL, || {
            if !full {
                // Name-only: the per-file payload without the patch
                // bodies, matching `gh pr diff --name-only`.
                let value = self.api(
                    repo_path,
                    &format!("projects/:id/merge_requests/{number}/diffs?per_page=100"),
                    CALL_TIMEOUT,
                )?;
                let files: Vec<&str> = value
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|f| {
                                f["new_path"].as_str().or_else(|| f["old_path"].as_str())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return Ok(files.join("\n"));
            }

            self.require_ready()?;
            let output = run_glab_timed(
                repo_path,
                &["mr", "diff", &number.to_string(), "--raw"],
                LIST_TIMEOUT,
            )?;
            if output.len() <= MAX_PR_DIFF_BYTES {
                return Ok(output);
            }
            let mut end = MAX_PR_DIFF_BYTES;
            while end > 0 && !output.is_char_boundary(end) {
                end -= 1;
            }
            Ok(format!(
                "{}\n\n[Diff truncated at {}KB — use `glab mr diff {}` for the full patch]",
                &output[..end],
                MAX_PR_DIFF_BYTES / 1024,
                number,
            ))
        })
    }

    fn pull_request_checks(
        &self,
        repo_path: &Path,
        number: Option<u32>,
    ) -> Result<Vec<CheckInfo>, String> {
        let number = match number {
            Some(number) => number,
            None => match self.branch_pull_request(repo_path)? {
                Some(mr) => mr.number,
                None => return Ok(Vec::new()),
            },
        };

        let pipelines = self.api(
            repo_path,
            &format!(
                "projects/:id/merge_requests/{number}/pipelines?per_page=1&order_by=id&sort=desc"
            ),
            CALL_TIMEOUT,
        )?;
        let Some(pipeline) = pipelines.as_array().and_then(|arr| arr.first()) else {
            return Ok(Vec::new());
        };
        let Some(pipeline_id) = pipeline["id"].as_u64() else {
            return Ok(Vec::new());
        };

        let jobs = self.api(
            repo_path,
            &format!("projects/:id/pipelines/{pipeline_id}/jobs?per_page=100"),
            CALL_TIMEOUT,
        )?;
        let checks = parse_checks(&jobs);
        if !checks.is_empty() {
            return Ok(checks);
        }

        // A pipeline with no jobs is an externally reported one (a CI
        // system posting commit statuses rather than running GitLab CI).
        // Its "jobs" are the commit statuses on the pipeline's sha, and
        // they carry the same name/state/target_url triple.
        let Some(sha) = pipeline["sha"].as_str() else {
            return Ok(Vec::new());
        };
        let statuses = self.api(
            repo_path,
            &format!("projects/:id/repository/commits/{sha}/statuses?per_page=100"),
            CALL_TIMEOUT,
        )?;
        Ok(parse_checks(&statuses))
    }

    fn pull_request_review_comments(
        &self,
        repo_path: &Path,
        number: Option<u32>,
    ) -> Result<Vec<ReviewComment>, String> {
        let number = match number {
            Some(number) => number,
            None => match self.branch_pull_request(repo_path)? {
                Some(mr) => mr.number,
                None => return Ok(Vec::new()),
            },
        };
        let discussions = self.discussions(repo_path, number)?;
        Ok(split_discussions(&discussions).0)
    }

    fn pull_request_inline_comments(
        &self,
        repo_path: &Path,
        number: u32,
    ) -> Result<Vec<InlineReviewComment>, String> {
        let discussions = self.discussions(repo_path, number)?;
        Ok(split_discussions(&discussions).1)
    }

    fn submit_pull_request_review(
        &self,
        repo_path: &Path,
        number: u32,
        event: &str,
        body: &str,
    ) -> Result<(), String> {
        if event == "request-changes" {
            // GitLab CE has no request-changes verb: reviewer states are
            // a paid feature and unapproving is not the same act. Refuse
            // before spending a round trip, and refuse rather than
            // silently downgrade it to a comment — which would tell the
            // author their changes were accepted.
            return Err(
                "GitLab has no request-changes review verb — leave a comment instead.".to_string(),
            );
        }
        // An approval is an act in its own right, so an empty body is
        // fine there. Every other verb *is* the comment, and posting
        // nothing while returning Ok would tell the UI a review was
        // submitted that no one will ever see.
        if event != "approve" && body.trim().is_empty() {
            return Err("A review comment cannot be empty.".to_string());
        }
        self.require_ready()?;
        // Anything else ("comment", or an unknown verb) is the body
        // alone, which is what a plain comment is on the gh path too.
        if event == "approve" {
            run_glab_timed(
                repo_path,
                &[
                    "api",
                    "--method",
                    "POST",
                    &format!("projects/:id/merge_requests/{number}/approve"),
                ],
                CALL_TIMEOUT,
            )?;
        }

        if !body.is_empty() {
            run_glab_timed(
                repo_path,
                &[
                    "api",
                    "--method",
                    "POST",
                    &format!("projects/:id/merge_requests/{number}/notes"),
                    "-f",
                    &format!("body={body}"),
                ],
                CALL_TIMEOUT,
            )?;
        }
        invalidate_cache(Some(repo_path));
        Ok(())
    }

    fn pull_request_deployments(
        &self,
        _repo_path: &Path,
        _number: u32,
    ) -> Result<Vec<DeploymentInfo>, String> {
        Err(self.unsupported("list deployments for a merge request"))
    }

    fn list_issues(
        &self,
        repo_path: &Path,
        search: Option<&str>,
    ) -> Result<Vec<GitHubIssue>, String> {
        let key = cache_key(repo_path, &["issues", search.unwrap_or("")]);
        ISSUE_LIST_CACHE.get_or_fetch(&key, LIST_TTL, || {
            // Mirrors the gh path: a search spans every state and
            // returns fewer rows; the default list is open issues only.
            let endpoint = match search {
                Some(query) => format!(
                    "projects/:id/issues?search={}&state=all&per_page=20&order_by=updated_at&sort=desc",
                    urlencoding::encode(query)
                ),
                None => format!(
                    "projects/:id/issues?state=opened&per_page={LIST_PAGE_SIZE}&order_by=updated_at&sort=desc"
                ),
            };
            let value = self.api(repo_path, &endpoint, CALL_TIMEOUT)?;
            let rows = value
                .as_array()
                .ok_or_else(|| "Expected JSON array from issue list".to_string())?;
            Ok(rows.iter().map(parse_issue_json).collect())
        })
    }

    fn get_issue(&self, repo_path: &Path, number: u64) -> Result<GitHubIssue, String> {
        let key = cache_key(repo_path, &["issue", &number.to_string()]);
        ISSUE_DETAIL_CACHE.get_or_fetch(&key, DETAIL_TTL, || self.get_issue_fresh(repo_path, number))
    }

    fn get_issue_fresh(&self, repo_path: &Path, number: u64) -> Result<GitHubIssue, String> {
        let value = self.api(
            repo_path,
            &format!("projects/:id/issues/{number}"),
            CALL_TIMEOUT,
        )?;
        let mut issue = parse_issue_json(&value);
        if let Some(body) = value["description"].as_str() {
            issue.body = Some(truncate_body(body));
        }
        let notes = self.api(
            repo_path,
            &format!("projects/:id/issues/{number}/notes?per_page=100&sort=asc&order_by=created_at"),
            CALL_TIMEOUT,
        )?;
        let (comments, total) = parse_notes(&notes);
        issue.comments = comments;
        issue.total_comments = total;
        Ok(issue)
    }

    fn fork_pr_fetch_refspec(&self, number: u32, local_branch: &str) -> Option<String> {
        // GitLab's counterpart to `pull/<n>/head`. Unlike GitHub's it is
        // not resolvable without the `refs/` prefix, because
        // `merge-requests/…` is not one of the prefixes git's ref
        // disambiguation searches.
        Some(format!("refs/merge-requests/{number}/head:{local_branch}"))
    }
}

// ── Helpers that need `self` ────────────────────────────────────

impl GitLabProvider {
    fn unsupported(&self, operation: &str) -> String {
        match &self.host {
            Some(host) => format!("GitLab ({host}) cannot {operation}."),
            None => format!("GitLab cannot {operation}."),
        }
    }

    fn discussions(&self, repo_path: &Path, number: u32) -> Result<Value, String> {
        self.api(
            repo_path,
            &format!("projects/:id/merge_requests/{number}/discussions?per_page=100"),
            CALL_TIMEOUT,
        )
    }

    fn default_branch(&self, repo_path: &Path) -> Result<String, String> {
        let value = self.api(repo_path, "projects/:id", CALL_TIMEOUT)?;
        value["default_branch"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "Project has no default branch".to_string())
    }

    /// `APPROVED` / `REVIEW_REQUIRED` in the vocabulary the review header
    /// renders. `None` when the instance does not serve the approvals
    /// endpoint, which leaves the header on its neutral default.
    fn review_decision(&self, repo_path: &Path, number: u32) -> Option<String> {
        let value = self
            .api(
                repo_path,
                &format!("projects/:id/merge_requests/{number}/approvals"),
                CALL_TIMEOUT,
            )
            .ok()?;
        match value["approved"].as_bool() {
            Some(true) => Some("APPROVED".to_string()),
            Some(false) => Some("REVIEW_REQUIRED".to_string()),
            None => None,
        }
    }

    /// Additions/deletions for a merge request.
    ///
    /// The REST payload has no diff stats — only GraphQL exposes them —
    /// and GraphQL needs the project's full path, which the merge
    /// request carries in `references.full` (`group/project!12`). Best
    /// effort: any failure just leaves the counts empty.
    fn diff_stats(&self, repo_path: &Path, mr: &Value) -> Option<(u32, u32)> {
        let reference = mr["references"]["full"].as_str()?;
        let full_path = reference.split('!').next()?;
        if full_path.is_empty() {
            return None;
        }
        let iid = mr["iid"].as_u64()?;
        let query = format!(
            "{{project(fullPath:\"{full_path}\"){{mergeRequest(iid:\"{iid}\"){{diffStatsSummary{{additions deletions}}}}}}}}"
        );
        let value = run_glab_json(
            repo_path,
            &["api", "graphql", "-f", &format!("query={query}")],
            CALL_TIMEOUT,
        )
        .ok()?;
        let summary = &value["data"]["project"]["mergeRequest"]["diffStatsSummary"];
        Some((
            summary["additions"].as_u64()? as u32,
            summary["deletions"].as_u64()? as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> GitLabProvider {
        GitLabProvider::from_detection(&DetectedProvider {
            kind: ProviderKind::GitLab,
            host: Some("gitlab.example.test".to_string()),
            base_url: Some("https://gitlab.example.test".to_string()),
            remote_name: Some("origin".to_string()),
        })
    }

    // ── Capabilities / URLs / refspec ──

    #[test]
    fn declares_only_what_it_serves() {
        let caps = provider().capabilities();
        assert!(caps.has_pull_requests);
        assert!(caps.has_checks);
        assert!(caps.has_issues);
        assert!(caps.has_inline_comments);
        assert!(caps.has_review_threads);
        assert!(caps.has_reviews);
        assert!(caps.has_fork_pr_fetch);
        // Not served, so the UI can hide the section instead of
        // rendering an error into it.
        assert!(!caps.has_deployments);
        assert!(provider().is_implemented());
        assert_eq!(provider().kind(), ProviderKind::GitLab);
    }

    #[test]
    fn deployments_are_refused_with_a_reason() {
        let error = provider()
            .pull_request_deployments(Path::new("/tmp"), 1)
            .unwrap_err();
        assert!(error.contains("GitLab"), "{error}");
        assert!(error.contains("gitlab.example.test"), "{error}");
    }

    #[test]
    fn fork_refspec_is_the_merge_request_head_ref() {
        assert_eq!(
            provider().fork_pr_fetch_refspec(42, "mr-42"),
            Some("refs/merge-requests/42/head:mr-42".to_string())
        );
    }

    #[test]
    fn auth_host_carries_the_port_the_bare_hostname_dropped() {
        let with_port = GitLabProvider::from_detection(&DetectedProvider {
            kind: ProviderKind::GitLab,
            host: Some("localhost".to_string()),
            base_url: Some("http://localhost:8929".to_string()),
            remote_name: None,
        });
        assert_eq!(with_port.auth_host().as_deref(), Some("localhost:8929"));

        // An SSH-only remote yields no web port; the bare host is the
        // best available and prefix matching covers the rest.
        let ssh_only = GitLabProvider::from_detection(&DetectedProvider {
            kind: ProviderKind::GitLab,
            host: Some("git.acme.test".to_string()),
            base_url: Some("https://git.acme.test".to_string()),
            remote_name: None,
        });
        assert_eq!(ssh_only.auth_host().as_deref(), Some("git.acme.test"));
    }

    // ── Auth status parsing ──

    /// Verbatim `glab auth status` output with two configured
    /// instances, one broken. Reproduced from a live run so the parser
    /// is tested against the real layout, glyphs and trailing banner.
    const MULTI_HOST_OUTPUT: &str = "\
gitlab.com
  x gitlab.com: API call failed: GET https://gitlab.com/api/v4/user: 401 {message: 401 Unauthorized}
  ✓ Git operations for gitlab.com configured to use ssh protocol.
  ! No token found (checked config file, keyring, and environment variables).
localhost:8929
  ✓ Logged in to localhost:8929 as root (keyring)
  ✓ Git operations for localhost:8929 configured to use ssh protocol.
  ✓ Token found in operating system keyring: **************************

   ERROR

  X could not authenticate to one or more of the configured GitLab instances.
";

    #[test]
    fn multi_host_output_answers_per_host() {
        assert_eq!(
            parse_auth_status(MULTI_HOST_OUTPUT, Some("localhost:8929")),
            AuthOutcome::Authenticated {
                username: "root".to_string()
            }
        );
        // The broken instance is read, not guessed at.
        assert_eq!(
            parse_auth_status(MULTI_HOST_OUTPUT, Some("gitlab.com")),
            AuthOutcome::NotAuthenticated
        );
    }

    #[test]
    fn a_host_glab_has_never_heard_of_is_not_authenticated() {
        assert_eq!(
            parse_auth_status(MULTI_HOST_OUTPUT, Some("git.acme.test")),
            AuthOutcome::NotAuthenticated
        );
    }

    #[test]
    fn a_portless_host_matches_the_configured_instance_on_that_host() {
        // What an SSH-only remote produces: detection knows the host but
        // not the web port glab keyed its config on.
        assert_eq!(
            parse_auth_status(MULTI_HOST_OUTPUT, Some("localhost")),
            AuthOutcome::Authenticated {
                username: "root".to_string()
            }
        );
    }

    #[test]
    fn host_matching_ignores_case() {
        assert_eq!(
            parse_auth_status(MULTI_HOST_OUTPUT, Some("LocalHost:8929")),
            AuthOutcome::Authenticated {
                username: "root".to_string()
            }
        );
    }

    #[test]
    fn an_unscoped_probe_takes_any_logged_in_instance() {
        assert_eq!(
            parse_auth_status(MULTI_HOST_OUTPUT, None),
            AuthOutcome::Authenticated {
                username: "root".to_string()
            }
        );
    }

    /// `glab auth status` with no `--hostname` exits non-zero when *any*
    /// configured instance fails, and the fixture has exactly that shape:
    /// a stale gitlab.com plus a working localhost. Trusting the exit
    /// code alone reported a logged-in user as signed out.
    #[test]
    fn an_unscoped_probe_reads_the_report_rather_than_the_failing_exit_code() {
        assert_eq!(
            auth_verdict(MULTI_HOST_OUTPUT, None, false),
            GhStatus::Authenticated {
                username: "root".to_string()
            }
        );
        // A host-scoped probe is about one instance, so there the exit
        // code *is* the answer.
        assert_eq!(
            auth_verdict(MULTI_HOST_OUTPUT, Some("localhost:8929"), false),
            GhStatus::NotAuthenticated
        );
    }

    #[test]
    fn an_unscoped_probe_with_nothing_logged_in_stays_negative() {
        let output = "gitlab.com\n  ! No token found.\n";
        assert_eq!(auth_verdict(output, None, false), GhStatus::NotAuthenticated);
        // Unparseable output plus a failing exit is not a login either.
        assert_eq!(
            auth_verdict("panic: runtime error", None, false),
            GhStatus::NotAuthenticated
        );
    }

    #[test]
    fn a_zero_exit_with_unreadable_wording_still_counts_as_signed_in() {
        // Losing the username to a reworded banner is acceptable;
        // logging every user out over it is not.
        assert_eq!(
            auth_verdict("", Some("gitlab.example.test"), true),
            GhStatus::Authenticated {
                username: String::new()
            }
        );
    }

    #[test]
    fn every_host_logged_out_is_not_authenticated() {
        let output = "\
gitlab.com
  ! No token found (checked config file, keyring, and environment variables).
";
        assert_eq!(
            parse_auth_status(output, Some("gitlab.com")),
            AuthOutcome::NotAuthenticated
        );
        assert_eq!(parse_auth_status(output, None), AuthOutcome::NotAuthenticated);
    }

    #[test]
    fn garbage_is_unknown_rather_than_a_verdict() {
        // Nothing block-shaped: no verdict may be inferred, and nothing
        // may panic. The caller falls back to the exit status.
        for garbage in ["", "   \n\t  \n", "  panic: runtime error\n  goroutine 1 [running]:\n"] {
            assert_eq!(
                parse_auth_status(garbage, Some("gitlab.com")),
                AuthOutcome::Unknown,
                "{garbage:?}"
            );
            assert_eq!(parse_auth_status(garbage, None), AuthOutcome::Unknown);
        }
    }

    #[test]
    fn a_reworded_login_line_degrades_to_not_authenticated_not_a_panic() {
        // The block is found, the login line is unrecognisable. Losing
        // the username is acceptable; crashing is not.
        let output = "gitlab.example.test\n  ✓ Authenticated as root\n";
        assert_eq!(
            parse_auth_status(output, Some("gitlab.example.test")),
            AuthOutcome::NotAuthenticated
        );
    }

    #[test]
    fn a_login_line_with_no_username_token_is_not_mistaken_for_one() {
        let output = "gitlab.example.test\n  ✓ Logged in to gitlab.example.test as (keyring)\n";
        assert_eq!(
            parse_auth_status(output, Some("gitlab.example.test")),
            AuthOutcome::NotAuthenticated
        );
    }

    // ── State normalisation ──

    #[test]
    fn merge_request_states_map_onto_the_ui_vocabulary() {
        assert_eq!(normalize_mr_state("opened"), "OPEN");
        assert_eq!(normalize_mr_state("reopened"), "OPEN");
        assert_eq!(normalize_mr_state("merged"), "MERGED");
        assert_eq!(normalize_mr_state("closed"), "CLOSED");
        assert_eq!(normalize_mr_state("locked"), "CLOSED");
        // Case and whitespace are not load-bearing.
        assert_eq!(normalize_mr_state(" Opened "), "OPEN");
        // An unknown state is passed through, never coerced to OPEN.
        assert_eq!(normalize_mr_state("quantum"), "QUANTUM");
    }

    #[test]
    fn draft_is_a_flag_not_a_state_and_display_state_folds_it_in() {
        let draft = parse_mr_json(&serde_json::json!({
            "iid": 3,
            "state": "opened",
            "title": "Draft: wip",
            "draft": true,
        }));
        assert_eq!(draft.state, "OPEN");
        assert!(draft.is_draft);
        assert_eq!(draft.display_state(), "DRAFT");

        // Pre-14.0 payloads spell it `work_in_progress`.
        let legacy = parse_mr_json(&serde_json::json!({
            "iid": 4,
            "state": "opened",
            "work_in_progress": true,
        }));
        assert!(legacy.is_draft);

        let merged = parse_mr_json(&serde_json::json!({
            "iid": 5,
            "state": "merged",
        }));
        assert_eq!(merged.display_state(), "MERGED");
        assert!(crate::github::is_historical_pr_state(&merged.state));
    }

    #[test]
    fn merge_request_fields_land_where_the_ui_reads_them() {
        let mr = parse_mr_json(&serde_json::json!({
            "id": 900,
            "iid": 12,
            "state": "opened",
            "title": "Add auth",
            "web_url": "http://localhost:8929/root/app/-/merge_requests/12",
            "source_branch": "feature/auth",
            "target_branch": "main",
            "updated_at": "2026-08-07T11:22:41.756Z",
            "sha": "be1b4c6",
            "has_conflicts": false,
            "detailed_merge_status": "mergeable",
            "author": {"username": "root"},
        }));
        // The project-scoped iid, not the instance-wide id.
        assert_eq!(mr.number, 12);
        assert_eq!(mr.url, "http://localhost:8929/root/app/-/merge_requests/12");
        assert_eq!(mr.head_branch.as_deref(), Some("feature/auth"));
        assert_eq!(mr.base_branch.as_deref(), Some("main"));
        assert_eq!(mr.head_ref_oid.as_deref(), Some("be1b4c6"));
        assert_eq!(mr.mergeable.as_deref(), Some("MERGEABLE"));
        assert_eq!(mr.author.as_deref(), Some("root"));
    }

    #[test]
    fn conflicts_surface_in_the_vocabulary_the_attachment_block_prints() {
        let conflicting = parse_mr_json(&serde_json::json!({
            "iid": 1, "has_conflicts": true, "detailed_merge_status": "mergeable",
        }));
        assert_eq!(conflicting.mergeable.as_deref(), Some("CONFLICTING"));

        let checking = parse_mr_json(&serde_json::json!({
            "iid": 1, "detailed_merge_status": "checking",
        }));
        assert_eq!(checking.mergeable.as_deref(), Some("UNKNOWN"));

        let unknown = parse_mr_json(&serde_json::json!({"iid": 1}));
        assert_eq!(unknown.mergeable, None);
    }

    #[test]
    fn branch_selection_prefers_an_open_merge_request_then_the_newest() {
        let rows = vec![
            serde_json::json!({"iid": 1, "state": "merged", "updated_at": "2026-08-07T12:00:00Z"}),
            serde_json::json!({"iid": 2, "state": "opened", "updated_at": "2026-08-01T09:00:00Z"}),
            serde_json::json!({"iid": 3, "state": "closed", "updated_at": "2026-08-09T09:00:00Z"}),
        ];
        assert_eq!(select_branch_mr(&rows).unwrap().number, 2);

        // With nothing open, the most recently updated historical one
        // wins — a reused branch name must not resurrect an old MR.
        let historical = vec![
            serde_json::json!({"iid": 1, "state": "merged", "updated_at": "2026-08-07T12:00:00Z"}),
            serde_json::json!({"iid": 3, "state": "closed", "updated_at": "2026-08-09T09:00:00Z"}),
        ];
        assert_eq!(select_branch_mr(&historical).unwrap().number, 3);

        assert!(select_branch_mr(&[]).is_none());
    }

    // ── Checks ──

    #[test]
    fn pipeline_jobs_map_onto_the_check_buckets_the_icons_understand() {
        let jobs = serde_json::json!([
            {"name": "build", "status": "running", "web_url": "http://ci/1",
             "started_at": "2026-08-07T11:23:36.159Z", "finished_at": null},
            {"name": "lint", "status": "success", "web_url": "http://ci/2"},
            {"name": "test", "status": "failed"},
            {"name": "deploy", "status": "manual"},
            {"name": "stale", "status": "canceled"},
            {"name": "future", "status": "waiting_for_resource"},
        ]);
        let checks = parse_checks(&jobs);
        assert_eq!(checks.len(), 6);

        assert_eq!(checks[0].name, "build");
        assert_eq!(checks[0].status, "running");
        assert_eq!(checks[0].conclusion.as_deref(), Some("pending"));
        assert_eq!(checks[0].detail_url.as_deref(), Some("http://ci/1"));
        assert_eq!(
            checks[0].started_at.as_deref(),
            Some("2026-08-07T11:23:36.159Z")
        );
        assert_eq!(checks[0].completed_at, None);

        assert_eq!(checks[1].conclusion.as_deref(), Some("pass"));
        assert_eq!(checks[2].conclusion.as_deref(), Some("fail"));
        assert_eq!(checks[3].conclusion.as_deref(), Some("skipping"));
        assert_eq!(checks[4].conclusion.as_deref(), Some("cancel"));
        // An unrecognised state reads as still-running, never as passed.
        assert_eq!(checks[5].conclusion.as_deref(), Some("pending"));
    }

    #[test]
    fn commit_statuses_stand_in_for_jobs_on_an_external_pipeline() {
        // Externally reported pipelines have no jobs; their commit
        // statuses carry the same triple under `target_url`.
        let statuses = serde_json::json!([
            {"name": "build", "status": "running", "target_url": "http://ci.example/build"},
        ]);
        let checks = parse_checks(&statuses);
        assert_eq!(checks[0].detail_url.as_deref(), Some("http://ci.example/build"));
    }

    #[test]
    fn a_non_array_checks_payload_is_empty_not_an_error() {
        assert!(parse_checks(&Value::Null).is_empty());
        assert!(parse_checks(&serde_json::json!({"message": "403 Forbidden"})).is_empty());
    }

    // ── Discussions ──

    fn discussions_fixture() -> Value {
        serde_json::json!([
            {
                "id": "aaa",
                "notes": [
                    {"id": 1, "body": "Overall looks good", "system": false,
                     "author": {"username": "root"}, "created_at": "2026-08-07T11:24:05.556Z"},
                    {"id": 9, "body": "agreed", "system": false,
                     "author": {"username": "dev"}, "created_at": "2026-08-07T11:25:00.000Z"}
                ]
            },
            {
                "id": "bbb",
                "notes": [
                    {"id": 3, "body": "Inline nit", "system": false,
                     "author": {"username": "root"}, "created_at": "2026-08-07T11:26:00.000Z",
                     "position": {"new_path": "a.txt", "old_path": "a.txt", "new_line": 2}},
                    {"id": 4, "body": "fixed", "system": false,
                     "author": {"username": "dev"}, "created_at": "2026-08-07T11:27:00.000Z",
                     "position": {"new_path": "a.txt", "old_path": "a.txt", "new_line": 2}}
                ]
            },
            {
                "id": "ccc",
                "notes": [
                    {"id": 5, "body": "changed the description", "system": true,
                     "author": {"username": "root"}, "created_at": "2026-08-07T11:28:00.000Z"},
                    {"id": 6, "body": "", "system": false,
                     "author": {"username": "root"}, "created_at": "2026-08-07T11:29:00.000Z"}
                ]
            }
        ])
    }

    #[test]
    fn discussions_split_by_whether_a_note_is_anchored_to_the_diff() {
        let (threads, inline) = split_discussions(&discussions_fixture());

        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].id, 1);
        assert_eq!(threads[0].author, "root");
        assert_eq!(threads[0].state, "COMMENTED");
        assert_eq!(threads[1].id, 9);

        assert_eq!(inline.len(), 2);
        assert_eq!(inline[0].path, "a.txt");
        assert_eq!(inline[0].line, Some(2));
    }

    #[test]
    fn a_discussion_stands_in_for_a_review_so_replies_thread_under_it() {
        let (_, inline) = split_discussions(&discussions_fixture());
        // The first note of a discussion is the thread root; later notes
        // point back at it, which is what the review tab groups on.
        assert_eq!(inline[0].in_reply_to_id, None);
        assert_eq!(inline[0].pull_request_review_id, Some(3));
        assert_eq!(inline[1].in_reply_to_id, Some(3));
        assert_eq!(inline[1].pull_request_review_id, Some(3));
    }

    #[test]
    fn system_notes_and_empty_bodies_never_reach_either_surface() {
        let (threads, inline) = split_discussions(&discussions_fixture());
        assert!(!threads.iter().any(|t| t.id == 5 || t.id == 6));
        assert!(!inline.iter().any(|c| c.id == 5 || c.id == 6));
    }

    #[test]
    fn an_inline_note_on_a_removed_line_falls_back_to_the_old_side() {
        let value = serde_json::json!([{
            "id": "x",
            "notes": [{"id": 1, "body": "gone", "system": false,
                       "author": {"username": "root"}, "created_at": "t",
                       "position": {"new_path": null, "old_path": "a.txt",
                                    "new_line": null, "old_line": 7}}]
        }]);
        let (_, inline) = split_discussions(&value);
        assert_eq!(inline[0].path, "a.txt");
        assert_eq!(inline[0].line, Some(7));
    }

    #[test]
    fn a_non_array_discussions_payload_yields_two_empty_lists() {
        let (threads, inline) = split_discussions(&Value::Null);
        assert!(threads.is_empty());
        assert!(inline.is_empty());
    }

    // ── Notes / issues ──

    #[test]
    fn notes_drop_system_entries_and_cap_at_the_prompt_budget() {
        let mut notes: Vec<Value> = (0..30)
            .map(|i| {
                serde_json::json!({
                    "id": i, "body": format!("comment {i}"), "system": false,
                    "author": {"username": "root"}, "created_at": "t"
                })
            })
            .collect();
        notes.push(serde_json::json!({
            "id": 99, "body": "added 1 commit", "system": true,
            "author": {"username": "root"}, "created_at": "t"
        }));
        notes.push(serde_json::json!({
            "id": 100, "body": "diff note", "system": false,
            "author": {"username": "root"}, "created_at": "t",
            "position": {"new_path": "a.txt", "new_line": 1}
        }));

        let (comments, total) = parse_notes(&Value::Array(notes));
        // The total counts conversation notes only — the system entry
        // and the diff-anchored note belong to other surfaces.
        assert_eq!(total, 30);
        assert_eq!(comments.len(), MAX_ISSUE_COMMENTS);
        assert_eq!(comments[0].author, "root");
        assert_eq!(comments[0].body, "comment 0");
    }

    #[test]
    fn issue_fields_land_where_the_ui_reads_them() {
        let issue = parse_issue_json(&serde_json::json!({
            "id": 700,
            "iid": 1,
            "title": "First issue",
            "state": "opened",
            "labels": ["bug", "ui"],
            "assignees": [{"username": "root"}],
            "web_url": "http://localhost:8929/root/app/-/issues/1",
            "updated_at": "2026-08-07T11:25:42.620Z",
        }));
        assert_eq!(issue.number, 1);
        assert_eq!(issue.state, IssueState::Open);
        assert_eq!(issue.labels, vec!["bug", "ui"]);
        assert_eq!(issue.assignees, vec!["root"]);

        let closed = parse_issue_json(&serde_json::json!({"iid": 2, "state": "closed"}));
        assert_eq!(closed.state, IssueState::Closed);
    }

    #[test]
    fn a_long_body_is_truncated_on_a_char_boundary() {
        let body = "é".repeat(MAX_ISSUE_BODY_BYTES);
        let truncated = truncate_body(&body);
        assert!(truncated.ends_with("[Body truncated at 50KB]"));
        // Truncation on a byte index inside a multi-byte char would have
        // panicked before reaching here.
        assert!(truncated.len() < body.len() + 64);

        let short = "fits";
        assert_eq!(truncate_body(short), short);
    }

    // ── Query encoding ──

    /// Guards the property the endpoints depend on: a branch name with a
    /// `/`, or a search with a space or `&`, must survive as one query
    /// parameter rather than splitting the endpoint `glab api` receives.
    #[test]
    fn query_values_are_encoded_so_a_slashed_branch_stays_one_parameter() {
        assert_eq!(urlencoding::encode("feature/auth"), "feature%2Fauth");
        assert_eq!(urlencoding::encode("fix bug & more"), "fix%20bug%20%26%20more");
        assert_eq!(urlencoding::encode("é"), "%C3%A9");
    }

    #[test]
    fn request_changes_is_refused_rather_than_downgraded() {
        let error = provider()
            .submit_pull_request_review(Path::new("/tmp"), 1, "request-changes", "no")
            .unwrap_err();
        assert!(error.contains("request-changes"), "{error}");
    }

    /// A comment-shaped verb with nothing to say posts nothing. Returning
    /// `Ok` for that told the review tab a review had been submitted that
    /// would never appear on the merge request.
    #[test]
    fn a_comment_with_no_body_is_refused_instead_of_silently_doing_nothing() {
        for body in ["", "   \n\t "] {
            for event in ["comment", "some-future-verb"] {
                let error = provider()
                    .submit_pull_request_review(Path::new("/tmp"), 1, event, body)
                    .unwrap_err();
                assert!(error.contains("empty"), "{event}/{body:?}: {error}");
            }
        }
    }
}
