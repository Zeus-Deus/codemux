use crate::execution::{sanitize_gui_env_std, sanitize_gui_env_std_keep_dbus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestInfo {
    pub number: u32,
    pub url: String,
    pub state: String,
    pub title: String,
    #[serde(alias = "headRefName", default)]
    pub head_branch: Option<String>,
    #[serde(alias = "baseRefName", default)]
    pub base_branch: Option<String>,
    #[serde(alias = "isDraft", default)]
    pub is_draft: bool,
    #[serde(default)]
    pub mergeable: Option<String>,
    #[serde(default)]
    pub additions: Option<u32>,
    #[serde(default)]
    pub deletions: Option<u32>,
    #[serde(alias = "reviewDecision", default)]
    pub review_decision: Option<String>,
    #[serde(default)]
    pub checks_passing: Option<bool>,
    #[serde(alias = "updatedAt", default)]
    pub updated_at: Option<String>,
    /// When the pull request was opened. One string field, added for the
    /// timeline's synthesized "opened" row — the commit *count* that row
    /// also shows is deliberately not fetched here, because `commits` is
    /// an array and this struct is refreshed every 2.5s.
    #[serde(alias = "createdAt", default)]
    pub created_at: Option<String>,
    /// Head commit SHA reported by GitHub. Kept as useful PR metadata, but
    /// association is branch/repository based: a local worktree may
    /// legitimately be behind the final remote PR head after review commits
    /// land, so exact SHA equality is not a valid identity check.
    #[serde(alias = "headRefOid", default)]
    pub head_ref_oid: Option<String>,
    /// Login of the repository that owns the PR's head branch (the fork
    /// owner for a cross-repository PR, the base repo's owner otherwise).
    /// Populated by the branch-PR list query only, where it disambiguates a
    /// fork-tracking local branch from a same-named branch in the upstream
    /// repo. Not a serde alias: gh emits this as a nested
    /// `{"login": …}` object, so `parse_pr_json` flattens it by hand.
    #[serde(default)]
    pub head_repository_owner: Option<String>,
    /// Stage 5 — populated by `get_pull_request` only; list paths
    /// leave it `None` so a list query stays cheap. Body is truncated
    /// to 50 KB at a char boundary, mirroring the issue path.
    #[serde(default)]
    pub body: Option<String>,
    /// Stage 5 — first `MAX_ISSUE_COMMENTS` PR conversation comments
    /// (review threads ship via the existing `get_pr_review_comments`
    /// path). Reusing `IssueComment` because the gh JSON shape is
    /// identical: `{author: {login}, body, createdAt}`.
    #[serde(default)]
    pub comments: Vec<IssueComment>,
    /// Total comment count on the PR — equal to `comments.len()`
    /// when under the cap, greater when truncated.
    #[serde(default, rename = "totalComments")]
    pub total_comments: u32,
    /// Author login. Useful for chip tooltips + injection header.
    #[serde(default)]
    pub author: Option<String>,
    /// gh's `mergeStateStatus`: CLEAN / BLOCKED / DIRTY / BEHIND /
    /// UNSTABLE / HAS_HOOKS / UNKNOWN. `mergeable` alone cannot tell
    /// "conflicts with base" from "a required check is still running",
    /// and the action bar has to name the blocking reason in words.
    #[serde(alias = "mergeStateStatus", default)]
    pub merge_state_status: Option<String>,
    /// File count for the meta row ("8 files").
    #[serde(alias = "changedFiles", default)]
    pub changed_files: Option<u32>,
    /// Login that merged it — the merged-elsewhere drift notice names
    /// a person, not an event.
    #[serde(alias = "mergedBy", default)]
    pub merged_by: Option<String>,
    #[serde(alias = "mergedAt", default)]
    pub merged_at: Option<String>,
    /// Logins with a review *requested* but not yet given. Drives the
    /// "Nobody is reviewing this yet" vs. pending-chips branch.
    #[serde(alias = "reviewRequests", default)]
    pub review_requests: Vec<String>,
    /// One entry per reviewer who has actually submitted a verdict.
    #[serde(alias = "latestReviews", default)]
    pub latest_reviews: Vec<PrReviewSummary>,
}

/// A reviewer's most recent verdict, flattened from gh's
/// `latestReviews[].author.login` + `.state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrReviewSummary {
    pub author: String,
    /// APPROVED / CHANGES_REQUESTED / COMMENTED / PENDING / DISMISSED.
    pub state: String,
}

impl PullRequestInfo {
    /// State string that feeds the workspace `pr_state` field (and the
    /// sidebar PR-status icon). Collapses `is_draft: true` into a "DRAFT"
    /// label so the sidebar can pick the muted draft icon without needing
    /// a separate `is_draft` column on the workspace.
    pub fn display_state(&self) -> String {
        if self.is_draft {
            "DRAFT".to_string()
        } else {
            self.state.clone()
        }
    }
}

/// True for terminal PR states.
pub fn is_historical_pr_state(state: &str) -> bool {
    matches!(state, "CLOSED" | "MERGED")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingPrItem {
    pub number: u32,
    pub title: String,
    pub author: String,
    #[serde(alias = "headRefName", default)]
    pub head_branch: Option<String>,
    #[serde(alias = "isDraft", default)]
    pub is_draft: bool,
    #[serde(default)]
    pub additions: Option<u32>,
    #[serde(default)]
    pub deletions: Option<u32>,
    #[serde(alias = "reviewDecision", default)]
    pub review_decision: Option<String>,
    /// Summarized from statusCheckRollup: "success", "failure", "pending", or None
    pub checks_status: Option<String>,
    #[serde(alias = "updatedAt", default)]
    pub updated_at: Option<String>,
    pub url: String,
}

/// One row of the Pull Requests page.
///
/// Deliberately not `IncomingPrItem` with fields bolted on: the incoming
/// list answers "what is aimed at my branch", this answers "what wants
/// something from me", and the two differ in exactly the fields that
/// cost money to fetch. Keeping them apart means the cheap call stays
/// cheap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrOverviewItem {
    pub number: u32,
    pub title: String,
    pub author: String,
    pub head_branch: Option<String>,
    pub is_draft: bool,
    /// Line counts, when the host served them cheaply. `None` means "not
    /// measured yet", and the row draws nothing rather than a zero.
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    /// APPROVED / CHANGES_REQUESTED / REVIEW_REQUIRED, when the product
    /// has such a verdict. GitLab has no request-changes concept.
    pub review_decision: Option<String>,
    /// The rollup reduced host-side to one of `passing` / `failing` /
    /// `pending` / `none`. The raw per-check array is 50 rows × N checks
    /// of JSON the page would only ever collapse to a colour anyway.
    ///
    /// `None` is a fourth answer, and a load-bearing one: *nobody has
    /// asked yet*. It is not `none` (this pull request has no checks) and
    /// it is not a colour — a row carrying `None` is waiting on the stats
    /// call, and the surfaces that judge a pull request by its CI (the
    /// badge, the toast, the "ready to merge" label) must all decline to
    /// judge until it becomes a word.
    pub checks: Option<String>,
    /// Logins this pull request is waiting on, so the page can group by
    /// "needs your review" without a second search query.
    pub review_requested_from: Vec<String>,
    pub updated_at: Option<String>,
    pub url: String,
}

/// The second half of a row, fetched separately because it is the half
/// that costs seconds.
///
/// `statusCheckRollup` makes GitHub compute an aggregate CI state per
/// pull request server-side, and `additions`/`deletions` make it compute
/// a diff stat per pull request; both are per-row work on top of a list
/// that is otherwise a single indexed read. Splitting them out is what
/// lets the listing paint while they are still being computed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrOverviewStats {
    pub number: u32,
    /// `passing` / `failing` / `pending` / `none`.
    pub checks: String,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

/// The overview for one repository root, plus who is asking.
///
/// `viewer` rides along with the rows rather than being resolved
/// separately by the caller because the grouping is meaningless without
/// it: a page that can't tell your PRs from everyone else's is just an
/// unsorted list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrsOverview {
    /// Signed-in account for this checkout's host, when the CLI names
    /// one. `None` ⇒ the page can still list, but cannot group.
    pub viewer: Option<String>,
    pub items: Vec<PrOverviewItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckInfo {
    pub name: String,
    #[serde(alias = "state")]
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(alias = "elapsedTime", default)]
    pub elapsed_time: Option<String>,
    #[serde(alias = "detailUrl", default)]
    pub detail_url: Option<String>,
    #[serde(alias = "startedAt", default)]
    pub started_at: Option<String>,
    #[serde(alias = "completedAt", default)]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub state: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum GhStatus {
    NotInstalled,
    NotAuthenticated,
    Authenticated { username: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineReviewComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub path: String,
    pub line: Option<u32>,
    pub created_at: String,
    pub in_reply_to_id: Option<u64>,
    pub pull_request_review_id: Option<u64>,
}

/// One comment inside a review thread.
///
/// `id` is a string because the two hosts' id spaces are not both
/// numbers: GitHub's is an opaque GraphQL node id, GitLab's is a note
/// number. Nothing but React keys and equality is done with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrThreadComment {
    pub id: String,
    /// The same comment's REST id, when the host has one.
    ///
    /// Two jobs, both load-bearing: it is the key the UI dedupes on (an
    /// inline comment already shown inside a thread must not be drawn a
    /// second time by the flat comment list), and on GitHub it is the
    /// resource the reply endpoint addresses.
    pub database_id: Option<u64>,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// A conversation anchored to a diff — the unit a reviewer replies to
/// and resolves.
///
/// REST's `pulls/{n}/comments` carries none of this: it is a flat list of
/// comments with no thread identity and no resolution state, which is why
/// the GitHub implementation of this one type goes through GraphQL while
/// everything else on the adapter stays on `gh`'s REST surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrReviewThread {
    /// The id the reply and resolve calls address.
    pub id: String,
    pub is_resolved: bool,
    /// The lines this thread was written against are no longer in the
    /// diff. Labelled, never hidden — an outdated objection is still an
    /// objection.
    pub is_outdated: bool,
    /// Whether this host can resolve *this* thread.
    ///
    /// Not a per-host constant: GitLab only resolves discussions anchored
    /// to a diff, and a plain merge-request comment there has no
    /// resolution state at all. Rendering a Resolve button on one would
    /// be a control that answers a click with a 400.
    pub is_resolvable: bool,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub comments: Vec<PrThreadComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentInfo {
    pub id: u64,
    pub environment: String,
    pub state: String,
    pub url: Option<String>,
    pub created_at: String,
}

// ── PR timeline ──
//
// The host's own history of a pull request, mapped to the handful of
// shapes the timeline rail actually draws. Two rules govern this model:
//
// 1. Every variant here is one the UI can render *specifically* — there
//    is no variant that exists only to be turned back into a string.
// 2. Anything else becomes [`PrTimelineEventKind::Other`] carrying a
//    human label, and is drawn as a plain one-liner. A host that grows a
//    new event type must never make an entry vanish (the reviewer would
//    be reading an incomplete history without being told) and must never
//    make the deserializer fail (which would blank the whole tab).

/// What happened, in the vocabulary the rail draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrTimelineEventKind {
    /// Synthesized from the PR itself: the API's stream has no "opened".
    Opened { commits: Option<u32> },
    Commented { body: String },
    Reviewed {
        /// APPROVED / CHANGES_REQUESTED / COMMENTED.
        verdict: String,
        body: String,
        /// `path:line` of the review's first inline note, when the
        /// payload already carried one. Never fetched for: one extra
        /// request per review to decorate a card is not a trade the
        /// 30s poll can afford.
        anchor: Option<String>,
    },
    Committed { sha: String, message: String },
    HeadRefForcePushed { sha: Option<String> },
    Merged { sha: Option<String> },
    Closed,
    Reopened,
    ReviewRequested { reviewer: Option<String> },
    Renamed { from: String, to: String },
    /// Anything the host has that this build does not draw specifically.
    Other { label: String },
}

/// One row of the host's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrTimelineEvent {
    /// Stable within one payload — the React key, so a poll that returns
    /// the same event twice updates a row instead of duplicating it.
    pub id: String,
    pub actor: Option<String>,
    /// ISO-8601. `None` sorts to the top rather than being dropped.
    pub created_at: Option<String>,
    #[serde(flatten)]
    pub kind: PrTimelineEventKind,
}

/// Turn a raw event type into the label a one-liner shows.
///
/// `head_ref_deleted` → "head ref deleted". Underscores become spaces
/// because a raw API token in a sentence reads like a leaked internal.
fn humanize_event(raw: &str) -> String {
    raw.replace('_', " ")
}

/// `gh api --paginate` on an array endpoint emits one JSON array per
/// page, concatenated. `serde_json::from_str` sees the first array and
/// then trailing characters, so a two-page timeline would either error or
/// silently lose every page but the first — read the stream instead and
/// flatten it.
fn parse_paginated_array(json: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let stream = serde_json::Deserializer::from_str(json).into_iter::<serde_json::Value>();
    for value in stream.flatten() {
        match value {
            serde_json::Value::Array(items) => out.extend(items),
            other => out.push(other),
        }
    }
    out
}

/// Map one raw GitHub timeline row.
///
/// Public within the crate so the mapping can be tested against recorded
/// payloads without a `gh` on PATH — the parsing is the part that breaks,
/// not the subprocess call around it.
pub(crate) fn map_github_timeline_event(
    raw: &serde_json::Value,
    index: usize,
) -> Option<PrTimelineEvent> {
    let event = raw["event"].as_str().unwrap_or("");

    // `commented` rows carry the author under `user`; the rest under
    // `actor`. A commit row has neither and names its author inline.
    let actor = raw["actor"]["login"]
        .as_str()
        .or_else(|| raw["user"]["login"].as_str())
        .or_else(|| raw["author"]["name"].as_str())
        .or_else(|| raw["committer"]["name"].as_str())
        .map(|s| s.to_string());

    let created_at = raw["created_at"]
        .as_str()
        .or_else(|| raw["submitted_at"].as_str())
        .or_else(|| raw["author"]["date"].as_str())
        .or_else(|| raw["committer"]["date"].as_str())
        .map(|s| s.to_string());

    // Ids are per-resource and a commit row has a `sha` instead, so the
    // index is folded in: two events can share neither.
    let id = raw["id"]
        .as_u64()
        .map(|n| n.to_string())
        .or_else(|| raw["sha"].as_str().map(|s| s.to_string()))
        .or_else(|| raw["node_id"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| format!("{event}-{index}"));
    let id = format!("{id}:{index}");

    let kind = match event {
        "commented" => {
            let body = raw["body"].as_str().unwrap_or("").trim().to_string();
            // A comment with no text is a deletion artefact, not history.
            if body.is_empty() {
                return None;
            }
            PrTimelineEventKind::Commented { body }
        }
        "reviewed" => PrTimelineEventKind::Reviewed {
            verdict: raw["state"]
                .as_str()
                .unwrap_or("COMMENTED")
                .to_uppercase(),
            body: raw["body"].as_str().unwrap_or("").trim().to_string(),
            anchor: None,
        },
        "committed" => PrTimelineEventKind::Committed {
            sha: raw["sha"].as_str().unwrap_or("").to_string(),
            message: raw["message"]
                .as_str()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .to_string(),
        },
        "head_ref_force_pushed" => PrTimelineEventKind::HeadRefForcePushed {
            sha: raw["commit_id"].as_str().map(|s| s.to_string()),
        },
        "merged" => PrTimelineEventKind::Merged {
            sha: raw["commit_id"].as_str().map(|s| s.to_string()),
        },
        "closed" => PrTimelineEventKind::Closed,
        "reopened" => PrTimelineEventKind::Reopened,
        "review_requested" => PrTimelineEventKind::ReviewRequested {
            reviewer: raw["requested_reviewer"]["login"]
                .as_str()
                .or_else(|| raw["requested_team"]["name"].as_str())
                .map(|s| s.to_string()),
        },
        "renamed" => PrTimelineEventKind::Renamed {
            from: raw["rename"]["from"].as_str().unwrap_or("").to_string(),
            to: raw["rename"]["to"].as_str().unwrap_or("").to_string(),
        },
        // Includes the empty string: a row with no `event` at all is
        // still a row, and saying "unknown event" is more honest than
        // dropping it.
        other => PrTimelineEventKind::Other {
            label: humanize_event(if other.is_empty() { "unknown event" } else { other }),
        },
    };

    Some(PrTimelineEvent {
        id,
        actor,
        created_at,
        kind,
    })
}

/// Map a whole raw GitHub timeline payload.
pub(crate) fn map_github_timeline(rows: &[serde_json::Value]) -> Vec<PrTimelineEvent> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, raw)| map_github_timeline_event(raw, i))
        .collect()
}

/// The host's history of one pull request.
///
/// `GET /repos/{owner}/{repo}/issues/{n}/timeline` with a plain Accept
/// header — the preview header this endpoint once needed is long retired,
/// and sending it now is a way to get a 415 from GitHub Enterprise.
///
/// The stream has no "opened" event, so the caller synthesizes one from
/// the pull request it already holds; doing it here would cost a second
/// request for data every caller already has.
pub fn get_pr_timeline(repo_path: &Path, pr_number: u32) -> Result<Vec<PrTimelineEvent>, String> {
    let nwo = get_repo_nwo(repo_path)?;
    let endpoint = format!("repos/{nwo}/issues/{pr_number}/timeline");
    let Some(json) = run_gh_optional(
        repo_path,
        &[
            "api",
            &endpoint,
            "--paginate",
            "-H",
            "Accept: application/vnd.github+json",
        ],
    ) else {
        return Ok(Vec::new());
    };
    if json.is_empty() {
        return Ok(Vec::new());
    }
    Ok(map_github_timeline(&parse_paginated_array(&json)))
}

pub fn check_gh_status() -> GhStatus {
    if !gh_available() {
        return GhStatus::NotInstalled;
    }

    let mut cmd = crate::execution::host_command("gh");
    cmd.args(["auth", "status"]);
    // `gh` stores its token in the user's secret-service keyring on
    // Linux desktops (gnome-keyring / kwallet / keepassxc-secret).
    // Reading the token requires DBus session-bus access, so this
    // call must use the keep-dbus variant — the default sanitiser
    // overrides DBUS_SESSION_BUS_ADDRESS=/dev/null and would make
    // every `gh auth status` look NotAuthenticated even after a
    // successful `gh auth login`.
    sanitize_gui_env_std_keep_dbus(&mut cmd);
    let output = cmd.output();

    let Ok(output) = output else {
        return GhStatus::NotAuthenticated;
    };

    if !output.status.success() {
        return GhStatus::NotAuthenticated;
    }

    // "Logged in to github.com account USERNAME (...)" — modern gh
    // (≥2.4x, incl. 2.97) prints auth status to stdout; older releases
    // used stderr. Parsing only stderr made every modern install look
    // like an anonymous viewer: rows still listed (the token was fine)
    // but nothing could be attributed to "you", so the Pull Requests
    // page filed the user's own PRs under Watching and the panel never
    // showed the author's action bar. Check stdout first, then stderr.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let username = parse_auth_status_username(&stdout)
        .or_else(|| parse_auth_status_username(&stderr))
        .unwrap_or_default();

    GhStatus::Authenticated { username }
}

/// Extract USERNAME from a `gh auth status` stream, wherever gh chose
/// to print it. Only lines that describe a login are considered, so a
/// hypothetical "account" in an error message can't produce a viewer.
fn parse_auth_status_username(stream: &str) -> Option<String> {
    stream
        .lines()
        .filter(|line| line.contains("Logged in to"))
        .find_map(|line| {
            line.find("account ").map(|pos| {
                let after = &line[pos + 8..];
                after.split_whitespace().next().unwrap_or("").to_string()
            })
        })
        .filter(|name| !name.is_empty())
}

fn run_gh(repo_path: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = crate::execution::host_command("gh");
    cmd.args(args).current_dir(repo_path);
    // Keep DBus available so the secret-service keyring round-trip
    // works — every gh subcommand pulls the auth token before
    // hitting the API. See `check_gh_status` for the full rationale.
    sanitize_gui_env_std_keep_dbus(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run gh: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "gh {} failed: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

fn run_gh_json(repo_path: &Path, args: &[&str]) -> Result<serde_json::Value, String> {
    let output = run_gh(repo_path, args)?;
    serde_json::from_str(&output).map_err(|e| format!("Failed to parse gh JSON: {e}"))
}

/// Returns None on non-zero exit (e.g. "no PR for this branch") instead of Err.
fn run_gh_optional(repo_path: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = crate::execution::host_command("gh");
    cmd.args(args).current_dir(repo_path);
    // Keep DBus available — see `check_gh_status` rationale.
    sanitize_gui_env_std_keep_dbus(&mut cmd);
    cmd.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
}

pub fn gh_available() -> bool {
    let mut cmd = crate::execution::host_command("which");
    cmd.arg("gh");
    sanitize_gui_env_std(&mut cmd);
    cmd.output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── GitHub Issues ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IssueState {
    Open,
    Closed,
}

impl IssueState {
    fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "CLOSED" => IssueState::Closed,
            _ => IssueState::Open,
        }
    }
}

impl std::fmt::Display for IssueState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IssueState::Open => write!(f, "open"),
            IssueState::Closed => write!(f, "closed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IssueComment {
    pub author: String,
    pub body: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubIssue {
    pub number: u64,
    pub title: String,
    pub state: IssueState,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<String>,
    pub url: String,
    #[serde(default)]
    pub body: Option<String>,
    /// Stage 4 — populated by `get_github_issue` (detail fetch only;
    /// `list_github_issues` leaves it empty). Capped at the first
    /// `MAX_ISSUE_COMMENTS` items.
    #[serde(default)]
    pub comments: Vec<IssueComment>,
    /// Total comment count on the issue. Equal to `comments.len()` when
    /// the issue has fewer than `MAX_ISSUE_COMMENTS` comments; greater
    /// when truncated.
    #[serde(default, rename = "totalComments")]
    pub total_comments: u32,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: Option<String>,
}

/// Cached display data for a workspace's linked issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedIssue {
    pub number: u64,
    pub title: String,
    pub state: IssueState,
    #[serde(default)]
    pub labels: Vec<String>,
}

pub const MAX_ISSUE_BODY_BYTES: usize = 50 * 1024; // 50 KB
const ISSUE_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Stage 4 — cap the comment list shipped to the agent so a long
/// thread can't blow out the prompt. The full count is preserved in
/// `total_comments` so the agent can still see "20 of 250 shown".
pub const MAX_ISSUE_COMMENTS: usize = 20;

/// Run `gh` with a timeout. Returns Err if the process doesn't finish in time.
/// Map a finished `gh` invocation onto the `Result` callers branch on.
///
/// Split out of `run_gh_timed` because the branch-PR pollers key their
/// preserve-vs-clear decision on exactly this mapping: a non-zero exit must
/// surface as `Err`, never as a successful-but-empty `Ok` (which they treat as
/// authoritative and act on by clearing). Being process-free, it is unit
/// testable without a live `gh`.
fn gh_exit_result(
    command: &str,
    success: bool,
    stdout: String,
    stderr: &str,
) -> Result<String, String> {
    if !success {
        return Err(format!("gh {command} failed: {}", stderr.trim()));
    }
    Ok(stdout.trim_end().to_string())
}

fn run_gh_timed(repo_path: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = crate::execution::host_command("gh");
    cmd.args(args).current_dir(repo_path);
    // Keep DBus available — issue list/view both pull the auth
    // token from the user's secret-service keyring on Linux. See
    // `check_gh_status` rationale.
    sanitize_gui_env_std_keep_dbus(&mut cmd);

    let output = crate::git_provider::exec::run_timed(cmd, timeout).map_err(|e| match e {
        crate::git_provider::exec::TimedFailure::Spawn(e) => format!("Failed to run gh: {e}"),
        crate::git_provider::exec::TimedFailure::Wait(e) => format!("Failed to wait for gh: {e}"),
        crate::git_provider::exec::TimedFailure::Timeout => {
            format!("gh command timed out after {}s", timeout.as_secs())
        }
    })?;

    gh_exit_result(
        args.first().unwrap_or(&""),
        output.success,
        output.stdout,
        &output.stderr,
    )
}

pub fn list_github_issues(
    repo_path: &Path,
    search: Option<&str>,
) -> Result<Vec<GitHubIssue>, String> {
    if !gh_available() {
        return Err("gh CLI is not installed".into());
    }
    match check_gh_status() {
        GhStatus::NotInstalled => return Err("gh CLI is not installed".into()),
        GhStatus::NotAuthenticated => return Err("gh CLI is not authenticated. Run: gh auth login".into()),
        GhStatus::Authenticated { .. } => {}
    }

    let json_fields = "number,title,state,labels,assignees,url,updatedAt";

    let output = if let Some(query) = search {
        run_gh_timed(
            repo_path,
            &[
                "issue", "list",
                "--search", query,
                "--state", "all",
                "--limit", "20",
                "--json", json_fields,
            ],
            ISSUE_FETCH_TIMEOUT,
        )?
    } else {
        run_gh_timed(
            repo_path,
            &[
                "issue", "list",
                "--state", "open",
                "--limit", "50",
                "--json", json_fields,
            ],
            ISSUE_FETCH_TIMEOUT,
        )?
    };

    if output.is_empty() {
        return Ok(Vec::new());
    }

    let v: serde_json::Value =
        serde_json::from_str(&output).map_err(|e| format!("Failed to parse issues JSON: {e}"))?;

    let arr = v.as_array().ok_or("Expected JSON array from gh issue list")?;
    Ok(arr.iter().map(parse_issue_json).collect())
}

pub fn get_github_issue(repo_path: &Path, number: u64) -> Result<GitHubIssue, String> {
    if !gh_available() {
        return Err("gh CLI is not installed".into());
    }
    match check_gh_status() {
        GhStatus::NotInstalled => return Err("gh CLI is not installed".into()),
        GhStatus::NotAuthenticated => {
            return Err("gh CLI is not authenticated. Run: gh auth login".into())
        }
        GhStatus::Authenticated { .. } => {}
    }

    let number_str = number.to_string();
    let output = run_gh_timed(
        repo_path,
        &[
            "issue", "view", &number_str,
            "--json", "number,title,state,labels,assignees,url,body,comments,updatedAt",
        ],
        ISSUE_FETCH_TIMEOUT,
    )?;

    let v: serde_json::Value =
        serde_json::from_str(&output).map_err(|e| format!("Failed to parse issue JSON: {e}"))?;

    let mut issue = parse_issue_json(&v);

    // Populate body (truncated to 50KB)
    if let Some(body) = v["body"].as_str() {
        let truncated = if body.len() > MAX_ISSUE_BODY_BYTES {
            let mut end = MAX_ISSUE_BODY_BYTES;
            while end > 0 && !body.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…\n\n[Body truncated at 50KB]", &body[..end])
        } else {
            body.to_string()
        };
        issue.body = Some(truncated);
    }

    // Populate comments. `gh issue view --json comments` emits the
    // full thread; we cap at MAX_ISSUE_COMMENTS in the prompt-bound
    // payload but preserve the true total in `total_comments` so the
    // agent can see "showing 20 of 132".
    let (comments, total) = parse_issue_comments(&v["comments"]);
    issue.comments = comments;
    issue.total_comments = total;

    Ok(issue)
}

/// Extract `IssueComment` entries from `gh`'s `--json comments` output.
/// Returns (truncated comments slice, total count). Pure function so
/// tests can hit it directly.
fn parse_issue_comments(v: &serde_json::Value) -> (Vec<IssueComment>, u32) {
    let Some(arr) = v.as_array() else {
        return (Vec::new(), 0);
    };
    let total = arr.len() as u32;
    let comments = arr
        .iter()
        .take(MAX_ISSUE_COMMENTS)
        .map(|c| IssueComment {
            author: c["author"]["login"].as_str().unwrap_or("").to_string(),
            body: c["body"].as_str().unwrap_or("").to_string(),
            created_at: c["createdAt"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    (comments, total)
}

fn parse_issue_json(v: &serde_json::Value) -> GitHubIssue {
    let labels = v["labels"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let assignees = v["assignees"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a["login"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    GitHubIssue {
        number: v["number"].as_u64().unwrap_or(0),
        title: v["title"].as_str().unwrap_or("").to_string(),
        state: IssueState::from_str(v["state"].as_str().unwrap_or("OPEN")),
        labels,
        assignees,
        url: v["url"].as_str().unwrap_or("").to_string(),
        body: None,
        comments: Vec::new(),
        total_comments: 0,
        updated_at: v["updatedAt"].as_str().map(|s| s.to_string()),
    }
}

/// Generate a branch name suggestion from an issue.
/// Format: `{number}-{kebab-title}` (max ~60 chars for the title portion).
pub fn suggest_branch_name(number: u64, title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse multiple hyphens and trim
    let mut collapsed = String::new();
    let mut prev_hyphen = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen && !collapsed.is_empty() {
                collapsed.push('-');
            }
            prev_hyphen = true;
        } else {
            collapsed.push(c);
            prev_hyphen = false;
        }
    }

    // Trim trailing hyphens
    let trimmed = collapsed.trim_end_matches('-');

    // Truncate title portion to ~60 chars, break at word boundary
    let max_title_len = 60;
    let title_slug = if trimmed.len() > max_title_len {
        // Back off to a char boundary so a multibyte slug (CJK, accented,
        // Cyrillic — all kept by is_alphanumeric above) never panics on the
        // byte-index slice. Mirrors the body-truncation sites elsewhere here.
        let mut end = max_title_len;
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        let truncated = &trimmed[..end];
        // Find last hyphen to break at word boundary
        if let Some(pos) = truncated.rfind('-') {
            &truncated[..pos]
        } else {
            truncated
        }
    } else {
        trimmed
    };

    format!("feature/{number}-{title_slug}")
}

/// Fetch a single PR by number with body + first 20 conversation
/// comments — Stage 5 detail path. Mirrors `get_github_issue`'s
/// shape so the chat composer can reuse the same loading-then-resolve
/// chip lifecycle. Body truncates at 50 KB on a char boundary.
pub fn get_pull_request(repo_path: &Path, number: u32) -> Result<PullRequestInfo, String> {
    if !gh_available() {
        return Err("gh CLI is not installed".into());
    }
    match check_gh_status() {
        GhStatus::NotInstalled => return Err("gh CLI is not installed".into()),
        GhStatus::NotAuthenticated => {
            return Err("gh CLI is not authenticated. Run: gh auth login".into())
        }
        GhStatus::Authenticated { .. } => {}
    }

    let number_str = number.to_string();
    let output = run_gh_timed(
        repo_path,
        &[
            "pr", "view", &number_str,
            "--json", "number,url,state,title,headRefName,baseRefName,isDraft,mergeable,additions,deletions,reviewDecision,updatedAt,createdAt,author,body,comments,changedFiles,mergeStateStatus,mergedBy,mergedAt,reviewRequests,latestReviews",
        ],
        ISSUE_FETCH_TIMEOUT,
    )?;

    let v: serde_json::Value = serde_json::from_str(&output)
        .map_err(|e| format!("Failed to parse PR JSON: {e}"))?;

    let mut pr = parse_pr_json(&v);

    if let Some(body) = v["body"].as_str() {
        let truncated = if body.len() > MAX_ISSUE_BODY_BYTES {
            let mut end = MAX_ISSUE_BODY_BYTES;
            while end > 0 && !body.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…\n\n[Body truncated at 50KB]", &body[..end])
        } else {
            body.to_string()
        };
        pr.body = Some(truncated);
    }

    let (comments, total) = parse_issue_comments(&v["comments"]);
    pr.comments = comments;
    pr.total_comments = total;

    Ok(pr)
}

/// Cap on the diff size we fetch in full mode. Anything bigger gets
/// truncated to a head-anchored prefix with a "[diff truncated at
/// 100KB]" trailer; the agent can still read the full diff via the
/// suggested gh command. Chosen large enough that small/medium PRs
/// always come through whole, small enough that a 5MB monorepo
/// migration doesn't blow out the prompt.
pub const MAX_PR_DIFF_BYTES: usize = 100 * 1024;

/// Fetch the diff for a PR — `--name-only` by default (cheap, fits
/// in a single chip preview), full unified diff when `full=true`.
/// Truncates the full-diff variant at `MAX_PR_DIFF_BYTES` on a char
/// boundary so the prompt stays bounded.
pub fn get_pr_diff(repo_path: &Path, number: u32, full: bool) -> Result<String, String> {
    get_pr_diff_capped(repo_path, number, full, MAX_PR_DIFF_BYTES)
}

/// Cap for the diff the Code tab renders.
///
/// A prompt has to stay small; a review surface has to be *complete*.
/// Truncating here would mean a reviewer scrolls to the end of what
/// looks like the diff and never learns there was more — and every note
/// they write below the cut would anchor against lines the host doesn't
/// agree exist. 4MB clears any pull request a person is going to read
/// line by line, and the per-file size threshold in the UI is what keeps
/// the rendering cheap.
pub const MAX_PR_REVIEW_DIFF_BYTES: usize = 4 * 1024 * 1024;

/// The whole patch, for reviewing rather than summarising.
pub fn get_pr_review_diff(repo_path: &Path, number: u32) -> Result<String, String> {
    get_pr_diff_capped(repo_path, number, true, MAX_PR_REVIEW_DIFF_BYTES)
}

fn get_pr_diff_capped(
    repo_path: &Path,
    number: u32,
    full: bool,
    max_bytes: usize,
) -> Result<String, String> {
    if !gh_available() {
        return Err("gh CLI is not installed".into());
    }
    match check_gh_status() {
        GhStatus::NotInstalled => return Err("gh CLI is not installed".into()),
        GhStatus::NotAuthenticated => {
            return Err("gh CLI is not authenticated. Run: gh auth login".into())
        }
        GhStatus::Authenticated { .. } => {}
    }

    let number_str = number.to_string();
    let mut args: Vec<&str> = vec!["pr", "diff", &number_str];
    if !full {
        args.push("--name-only");
    }
    let output = run_gh_timed(repo_path, &args, ISSUE_FETCH_TIMEOUT)?;

    if !full || output.len() <= max_bytes {
        return Ok(output);
    }

    // Full diff exceeded the cap — truncate at a char boundary and
    // signpost what was cut so the agent doesn't silently miss
    // hunks. The trailing pointer mirrors `get_github_issue`'s
    // truncation marker.
    let mut end = max_bytes;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    Ok(format!(
        "{}\n\n[Diff truncated at {}KB — use `gh pr diff {}` for the full patch]",
        &output[..end],
        max_bytes / 1024,
        number,
    ))
}

const BRANCH_PR_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

fn run_git_optional(repo_path: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = crate::execution::host_command("git");
    cmd.args(args).current_dir(repo_path);
    sanitize_gui_env_std(&mut cmd);
    cmd.output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|output| !output.is_empty())
}

/// Parse a GitHub remote into `(owner, repository)`. The normalized identity
/// is used only to tell an origin branch from a fork branch; all other hosts
/// deliberately fall back to the unqualified branch selector.
fn parse_github_remote(remote_url: &str) -> Option<(String, String)> {
    let remote_url = remote_url.trim();
    let path = if let Some(path) = remote_url.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = remote_url.strip_prefix("github.com:") {
        path
    } else if let Some(path) = remote_url.strip_prefix("github.com/") {
        path
    } else {
        let (_, authority_and_path) = remote_url.split_once("://")?;
        let (authority, path) = authority_and_path.split_once('/')?;
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    }
    .trim_matches('/');
    let mut segments = path.split('/');
    let owner = segments.next()?.trim();
    let repository = segments.next()?.trim().trim_end_matches(".git");
    if owner.is_empty() || repository.is_empty() || segments.next().is_some() {
        return None;
    }
    Some((owner.to_ascii_lowercase(), repository.to_ascii_lowercase()))
}

/// Owner login that a branch's PR head repository must match, or `None`
/// when any owner is acceptable.
///
/// `gh pr list --head` matches the *bare* branch name only. The
/// `owner:branch` form that `gh pr create` accepts is taken without
/// complaint here but matches nothing — verified against gh 2.96.0, where
/// `--head owner:branch` returns `[]` with exit 0 for a branch that
/// `--head branch` resolves fine. Since a successful empty list is
/// authoritative for callers, querying that way would clear a
/// fork-tracking workspace's badge on every poll tick. So the query always
/// uses the bare branch name and cross-repository ambiguity is resolved
/// here instead: when the branch tracks a fork, only PRs whose head
/// repository belongs to that fork's owner may match.
fn branch_head_owner_filter(
    tracking_remote_url: Option<&str>,
    origin_remote_url: Option<&str>,
) -> Option<String> {
    let (tracking_owner, tracking_repo) = tracking_remote_url.and_then(parse_github_remote)?;
    let (origin_owner, origin_repo) = origin_remote_url.and_then(parse_github_remote)?;

    if tracking_owner == origin_owner && tracking_repo == origin_repo {
        None
    } else {
        Some(tracking_owner)
    }
}

/// Does this PR's head repository belong to the owner the branch expects?
/// With no expected owner every PR qualifies. With one, a PR that failed to
/// report its head repository owner cannot be confirmed as the fork's and
/// is rejected — the alternative would be attaching an upstream PR to a
/// fork workspace that merely shares a branch name.
fn pr_matches_expected_owner(pr: &PullRequestInfo, expected_owner: Option<&str>) -> bool {
    let Some(expected) = expected_owner else {
        return true;
    };
    pr.head_repository_owner
        .as_deref()
        .is_some_and(|owner| owner.eq_ignore_ascii_case(expected))
}

fn resolve_branch_head_owner(repo_path: &Path, branch: &str) -> Option<String> {
    let branch_remote_key = format!("branch.{branch}.remote");
    let tracking_remote = run_git_optional(repo_path, &["config", "--get", &branch_remote_key]);
    let tracking_remote_url = tracking_remote.as_deref().and_then(|remote| {
        if remote == "." {
            return None;
        }
        let remote_url_key = format!("remote.{remote}.url");
        run_git_optional(repo_path, &["config", "--get", &remote_url_key])
    });
    let origin_remote_url = run_git_optional(repo_path, &["config", "--get", "remote.origin.url"]);

    branch_head_owner_filter(
        tracking_remote_url.as_deref(),
        origin_remote_url.as_deref(),
    )
}

fn is_newer_pr(candidate: &PullRequestInfo, current: &PullRequestInfo) -> bool {
    candidate
        .updated_at
        .cmp(&current.updated_at)
        .then_with(|| candidate.number.cmp(&current.number))
        .is_gt()
}

/// Choose the PR that represents a branch today. An open/draft PR always wins
/// over historical work when a branch name has been reused; otherwise the most
/// recently updated PR wins. Historical PRs are suppressed only on the
/// repository default branch, where they usually describe reverse-merge
/// history rather than the workspace's work item.
///
/// `expected_owner` scopes the match to one head-repository owner for
/// fork-tracking branches — see `branch_head_owner_filter`.
///
/// Candidates are ordered newest-first (highest PR number) before selection so
/// the outcome never depends on the order gh happened to return, and so a
/// heavily-reused branch name whose history was truncated by `--limit` still
/// resolves deterministically to the newest PR it did see.
fn select_branch_pr(
    pull_requests: impl IntoIterator<Item = PullRequestInfo>,
    branch: &str,
    expected_owner: Option<&str>,
    is_default_branch: bool,
) -> Option<PullRequestInfo> {
    let mut latest_open: Option<PullRequestInfo> = None;
    let mut latest_historical: Option<PullRequestInfo> = None;

    let mut candidates: Vec<PullRequestInfo> = pull_requests
        .into_iter()
        .filter(|pr| pr.head_branch.as_deref() == Some(branch))
        .filter(|pr| pr_matches_expected_owner(pr, expected_owner))
        .collect();
    candidates.sort_by(|a, b| b.number.cmp(&a.number));

    for pr in candidates {
        if is_historical_pr_state(&pr.state) {
            if is_default_branch {
                continue;
            }
            if latest_historical
                .as_ref()
                .is_none_or(|current| is_newer_pr(&pr, current))
            {
                latest_historical = Some(pr);
            }
        } else if latest_open
            .as_ref()
            .is_none_or(|current| is_newer_pr(&pr, current))
        {
            latest_open = Some(pr);
        }
    }

    latest_open.or(latest_historical)
}

/// Resolve the current branch's PR by explicit branch/repository identity.
///
/// `gh pr view` infers from local git state and applies an exact-SHA gate that
/// drops a merged PR whenever review commits made the remote head newer than
/// the still-open local worktree. Listing every state for the branch instead
/// keeps merged/closed history visible: branch identity owns the association,
/// while SHA is metadata. A failed GitHub request remains an `Err`, distinct
/// from a successful empty list, so callers can preserve the last known PR
/// through transient failures.
///
/// A detached HEAD (mid-rebase, mid-bisect, `gh pr checkout` of a SHA) is an
/// `Err` for the same reason: there is no branch to answer *about*, so the
/// question is unanswerable rather than answered "no PR". Returning `Ok(None)`
/// there would let a routine rebase clear an OPEN badge.
pub fn get_branch_pr(repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
    Ok(resolve_branch_pr(repo_path)?.pr)
}

/// The `--json` field set every branch-PR query asks for. Shared by the
/// per-branch lookup and the repo-wide fallback list so `parse_pr_json` can
/// never be handed a row that is missing a field one caller relies on.
const BRANCH_PR_JSON_FIELDS: &str = "number,url,state,title,headRefName,baseRefName,isDraft,mergeable,additions,deletions,reviewDecision,updatedAt,createdAt,headRefOid,headRepositoryOwner,author,body,changedFiles,mergeStateStatus,mergedBy,mergedAt,reviewRequests,latestReviews";

/// The current branch's PR plus the branch context that produced it.
///
/// The context is not decoration: the side-branch fallback needs to know
/// which branch already answered "no PR" (to exclude it from the candidate
/// list) and whether that branch is the repository default (where no fallback
/// may run at all), and re-deriving either would mean re-running local git
/// commands the primary lookup already ran.
struct BranchPrLookup {
    branch: String,
    default_branch: Option<String>,
    pr: Option<PullRequestInfo>,
}

impl BranchPrLookup {
    fn is_default_branch(&self) -> bool {
        self.default_branch.as_deref() == Some(self.branch.as_str())
    }
}

fn resolve_branch_pr(repo_path: &Path) -> Result<BranchPrLookup, String> {
    let Some(branch) = run_git_optional(repo_path, &["branch", "--show-current"]) else {
        return Err("detached HEAD: no branch to resolve a PR for".to_string());
    };
    let expected_owner = resolve_branch_head_owner(repo_path, &branch);
    let output = run_gh_timed(
        repo_path,
        &[
            "pr",
            "list",
            // Bare branch name on purpose — gh does not match the
            // `owner:branch` form here. Fork disambiguation happens
            // client-side via `expected_owner`.
            "--head",
            &branch,
            "--state",
            "all",
            // 100 is gh's per-page maximum. Generous because the newest PR
            // being paged out of a reused branch name's history is the one
            // failure mode a smaller cap can produce.
            "--limit",
            "100",
            "--json",
            BRANCH_PR_JSON_FIELDS,
        ],
        BRANCH_PR_LOOKUP_TIMEOUT,
    )?;
    let value: serde_json::Value =
        serde_json::from_str(&output).map_err(|e| format!("Failed to parse PR JSON: {e}"))?;
    let rows = value
        .as_array()
        .ok_or_else(|| "Expected JSON array from gh pr list".to_string())?;
    let default_branch = crate::git::find_default_branch(repo_path);
    let is_default_branch = default_branch.as_deref() == Some(branch.as_str());

    let pr = select_branch_pr(
        rows.iter().map(parse_pr_json),
        &branch,
        expected_owner.as_deref(),
        is_default_branch,
    );
    Ok(BranchPrLookup {
        branch,
        default_branch,
        pr,
    })
}

/// How many HEAD reflog entries the side-branch fallback reads.
///
/// The window is bounded by entry count rather than wall-clock age on
/// purpose: reflog timestamps are rewritten by `git gc` and absent from a
/// fresh clone, while "the last handful of things this worktree checked out"
/// is exactly the question the fallback wants answered.
const REFLOG_SCAN_ENTRIES: usize = 50;

/// How many distinct recently-checked-out branches the fallback considers.
const REFLOG_CANDIDATE_LIMIT: usize = 5;

/// Repo-wide PR page size for the fallback. Smaller than the per-branch
/// query's 100 because the fallback only ever asks about branches checked
/// out in the recent past, whose PRs are correspondingly recent.
const FALLBACK_PR_LIST_LIMIT: &str = "50";

/// How long one fallback result is reused. Sized to the 60s PR poller so
/// the 5s active-workspace sweep can't multiply it.
///
/// It gates **both** fallback caches, and between them the whole fallback
/// costs nothing on a repeat call: several no-PR workspaces sharing a repo
/// cost one `gh` call per minute between them, and each workspace runs its
/// local git probes (`reflog show`, plus at most a couple of `config`
/// reads) at most once a minute — not once per 5s sweep.
const FALLBACK_PR_LIST_TTL: Duration = Duration::from_secs(60);

type FallbackPrListCache = Mutex<HashMap<String, (Instant, Arc<Vec<serde_json::Value>>)>>;
static FALLBACK_PR_LIST_CACHE: OnceLock<FallbackPrListCache> = OnceLock::new();

/// Memoized fallback outcome, keyed by `(worktree path, current branch)`.
///
/// The reflog is per-worktree and the answer depends on which branch is
/// checked out, so neither part of the key can be dropped. This is the
/// cache that keeps the *local* side of the fallback off the 5s sweep: the
/// `gh` list alone being memoized still left a `git reflog` and a `git
/// config` per tick.
///
/// A branch switch is a new key and is answered immediately; only a PR
/// opened on an already-scanned branch waits out the TTL, which is the same
/// latency the memoized `gh` list already imposes.
type SideBranchPrCache = Mutex<HashMap<(PathBuf, String), (Instant, Option<PullRequestInfo>)>>;
static SIDE_BRANCH_PR_CACHE: OnceLock<SideBranchPrCache> = OnceLock::new();

/// TTL memoization over a `Mutex<HashMap>`: `compute` runs only when no
/// entry younger than `ttl` exists for `key`.
///
/// Errors are never cached — a dropped network or an expired `gh` token
/// must be retried on the next call, not pinned for a minute. Expired
/// entries are swept on write, so the map is bounded by the repos actually
/// polled in one TTL window.
///
/// The lock is released while `compute` runs, so two racing callers may
/// both compute; the outcome is identical either way and holding a global
/// mutex across a subprocess would be worse.
fn memoize_ok<K, V, E>(
    cache: &Mutex<HashMap<K, (Instant, V)>>,
    key: K,
    ttl: Duration,
    compute: impl FnOnce() -> Result<V, E>,
) -> Result<V, E>
where
    K: Eq + std::hash::Hash,
    V: Clone,
{
    if let Some((fetched_at, value)) = cache.lock().expect("PR fallback cache poisoned").get(&key) {
        if fetched_at.elapsed() < ttl {
            return Ok(value.clone());
        }
    }
    let value = compute()?;
    let mut guard = cache.lock().expect("PR fallback cache poisoned");
    guard.retain(|_, (fetched_at, _)| fetched_at.elapsed() < ttl);
    guard.insert(key, (Instant::now(), value.clone()));
    Ok(value)
}

/// True for a name that is really a commit id — what `git checkout <sha>`
/// and `git bisect` write into the reflog. A branch named entirely of hex
/// digits is theoretically possible and is deliberately sacrificed: reading
/// a detached-HEAD SHA as a branch name would send a `gh` query after
/// something that can never match.
fn looks_like_commit_id(name: &str) -> bool {
    (7..=40).contains(&name.len()) && name.chars().all(|c| c.is_ascii_hexdigit())
}

/// Branch names this worktree checked out recently, newest first.
///
/// Input is raw `git reflog` output for HEAD, which in a linked worktree is
/// that worktree's own reflog — so the candidates describe where *this*
/// checkout has been, not the repo at large.
///
/// Only `checkout: moving from X to Y` records carry branch identity, and
/// both sides of one are used: `Y` is the state the worktree entered and `X`
/// the one it left, so reading `Y` then `X` per record, newest record first,
/// yields true recency order. Taking only `Y` would miss a branch whose
/// arrival scrolled off the end of the window while its departure did not.
///
/// The current branch is excluded (it already answered "no PR"), as is the
/// repository default branch (its PR history is reverse-merge noise, the same
/// reason `select_branch_pr` suppresses historical PRs there).
///
/// A branch name containing " to " is ambiguous in this format and is split
/// at the first occurrence; git writes no delimiter that would resolve it,
/// and the failure mode is a candidate that matches no PR.
fn parse_recent_checkout_branches(
    reflog: &str,
    current_branch: &str,
    default_branch: Option<&str>,
    limit: usize,
) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    for line in reflog.lines() {
        let Some((_, moved)) = line.split_once("checkout: moving from ") else {
            continue;
        };
        let Some((from, to)) = moved.split_once(" to ") else {
            continue;
        };
        for name in [to.trim(), from.trim()] {
            if name.is_empty()
                || name == current_branch
                || default_branch == Some(name)
                || looks_like_commit_id(name)
                || candidates.iter().any(|existing| existing == name)
            {
                continue;
            }
            candidates.push(name.to_string());
            if candidates.len() >= limit {
                return candidates;
            }
        }
    }
    candidates
}

/// Cache key for the repo-wide fallback list: the origin URL, so sibling
/// worktrees of one repo share a single entry (they share the PR list too).
/// Repos without an origin fall back to their own path, which is still
/// correct, just unshared.
fn fallback_pr_list_key(repo_path: &Path) -> String {
    run_git_optional(repo_path, &["config", "--get", "remote.origin.url"])
        .unwrap_or_else(|| repo_path.to_string_lossy().into_owned())
}

/// One repo-wide `gh pr list` of **open** PRs, memoized for
/// [`FALLBACK_PR_LIST_TTL`].
///
/// Listing the repo once and matching every candidate branch against it
/// client-side keeps the fallback at a flat one extra `gh` call, instead of
/// one per candidate.
///
/// `--state open` (which includes drafts) rather than `--state all`: the
/// fallback only ever badges an open PR — see [`select_first_candidate_pr`]
/// — so fetching history would be paid-for rows that the selector discards.
/// The selector re-checks the state anyway, so the two cannot drift.
fn fallback_pr_list(repo_path: &Path) -> Result<Arc<Vec<serde_json::Value>>, String> {
    let key = fallback_pr_list_key(repo_path);
    let cache = FALLBACK_PR_LIST_CACHE.get_or_init(FallbackPrListCache::default);

    memoize_ok(cache, key, FALLBACK_PR_LIST_TTL, || {
        let output = run_gh_timed(
            repo_path,
            &[
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                FALLBACK_PR_LIST_LIMIT,
                "--json",
                BRANCH_PR_JSON_FIELDS,
            ],
            BRANCH_PR_LOOKUP_TIMEOUT,
        )?;
        let value: serde_json::Value =
            serde_json::from_str(&output).map_err(|e| format!("Failed to parse PR JSON: {e}"))?;
        Ok(Arc::new(
            value
                .as_array()
                .ok_or_else(|| "Expected JSON array from gh pr list".to_string())?
                .clone(),
        ))
    })
}

/// Badge-only fallback: the open PR of a branch this worktree checked out
/// recently, memoized per `(worktree, branch)` for
/// [`FALLBACK_PR_LIST_TTL`].
///
/// Answers the "agent opened the PR from a side branch, then checked the
/// worktree back" case, where the workspace has visibly produced a pull
/// request that strict current-branch association cannot see. Candidates are
/// tried newest-first and the first branch with an open PR wins; fork owners
/// are still scoped exactly as on the current-branch path.
///
/// The memoization is what makes this affordable on the 5s active-workspace
/// sweep — see [`SIDE_BRANCH_PR_CACHE`]. Without it every tick of every
/// no-PR workspace re-ran `git reflog` and a `git config` or three.
///
/// The result is deliberately weaker than a current-branch association: it
/// carries the PR's own `head_branch`, and auto-settlement refuses any
/// association whose head branch is not the checked-out one.
fn get_side_branch_pr(
    repo_path: &Path,
    lookup: &BranchPrLookup,
) -> Result<Option<PullRequestInfo>, String> {
    let cache = SIDE_BRANCH_PR_CACHE.get_or_init(SideBranchPrCache::default);
    let key = (repo_path.to_path_buf(), lookup.branch.clone());
    memoize_ok(cache, key, FALLBACK_PR_LIST_TTL, || {
        resolve_side_branch_pr(repo_path, lookup)
    })
}

/// The uncached body of [`get_side_branch_pr`] — every local git probe the
/// fallback makes lives here, so the memo above is the only thing standing
/// between it and the 5s sweep.
fn resolve_side_branch_pr(
    repo_path: &Path,
    lookup: &BranchPrLookup,
) -> Result<Option<PullRequestInfo>, String> {
    let Some(reflog) = run_git_optional(
        repo_path,
        &[
            "reflog",
            "show",
            "--max-count",
            &REFLOG_SCAN_ENTRIES.to_string(),
            "HEAD",
        ],
    ) else {
        return Ok(None);
    };
    let candidates = parse_recent_checkout_branches(
        &reflog,
        &lookup.branch,
        lookup.default_branch.as_deref(),
        REFLOG_CANDIDATE_LIMIT,
    );
    if candidates.is_empty() {
        return Ok(None);
    }

    let rows = fallback_pr_list(repo_path)?;
    let prs: Vec<PullRequestInfo> = rows.iter().map(parse_pr_json).collect();
    Ok(select_first_candidate_pr(&prs, &candidates, &|candidate| {
        resolve_branch_head_owner(repo_path, candidate)
    }))
}

/// Newest-first candidate scan: the first recently-checked-out branch with
/// an **open or draft** PR wins outright.
///
/// Merged and closed PRs are excluded here, unlike the current-branch path.
/// The motivating case is narrow — "an agent just pushed a PR from a side
/// branch" — and it is always an open one. Admitting history instead lets
/// any branch that passed through this worktree in the last
/// [`REFLOG_SCAN_ENTRIES`] checkouts donate its badge: `gh pr checkout <n>`
/// on somebody else's merged PR and a switch back would badge the workspace
/// with that PR for as long as the checkout stayed in the reflog window.
/// The current branch's own association keeps showing merged/closed state,
/// because there branch identity — not recency — owns the badge.
///
/// Recency decides between candidates, not PR freshness: a branch checked
/// out more recently is the better description of what this workspace was
/// just doing.
///
/// `expected_owner` is injected rather than resolved inline because it costs
/// a `git config` read per candidate — the caller owns that, the selection
/// rule stays pure. It is consulted **lazily**, only for a candidate that
/// already has at least one open PR row to disambiguate, so the common
/// "recently visited branches, none of them with a PR" case spawns no git
/// processes at all.
fn select_first_candidate_pr(
    prs: &[PullRequestInfo],
    candidates: &[String],
    expected_owner: &dyn Fn(&str) -> Option<String>,
) -> Option<PullRequestInfo> {
    let open: Vec<&PullRequestInfo> = prs
        .iter()
        .filter(|pr| !is_historical_pr_state(&pr.state))
        .collect();
    candidates.iter().find_map(|candidate| {
        let matching: Vec<PullRequestInfo> = open
            .iter()
            .filter(|pr| pr.head_branch.as_deref() == Some(candidate.as_str()))
            .map(|pr| (*pr).clone())
            .collect();
        if matching.is_empty() {
            // Nothing to disambiguate, so `expected_owner` — and the
            // `git config` reads behind it — is never reached.
            return None;
        }
        // `is_default_branch` is irrelevant now that only open/draft rows
        // reach here — it exists to suppress historical PRs — so the
        // cheaper `false` is passed rather than re-deriving the default.
        select_branch_pr(
            matching,
            candidate,
            expected_owner(candidate).as_deref(),
            false,
        )
    })
}

/// The PR a workspace should show a badge for.
///
/// The current branch owns the association whenever it has one, in any state
/// — branch identity, not freshness, decides there. Only when the current
/// branch authoritatively has none does the recently-checked-out fallback
/// run, and never on the repository default branch, where "recently checked
/// out" describes ordinary branch hopping rather than this checkout's own
/// work. The fallback badges **open PRs only**, so a `gh pr checkout` of
/// somebody's merged PR cannot leave its badge behind.
///
/// The three-way [`BranchPrOutcome`] contract is preserved exactly: an
/// unanswerable *current-branch* lookup is still `Err` (preserve the badge),
/// while a failure inside the fallback collapses to `Ok(None)` — the current
/// branch already answered "no PR" authoritatively, and a missing bonus badge
/// must not be upgraded into "keep the old one".
pub fn get_workspace_pr(repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
    let lookup = resolve_branch_pr(repo_path)?;
    if lookup.pr.is_some() {
        return Ok(lookup.pr);
    }
    if lookup.is_default_branch() {
        return Ok(None);
    }
    Ok(get_side_branch_pr(repo_path, &lookup).unwrap_or(None))
}

/// What a branch-PR lookup should do to a workspace's stored association.
///
/// The three arms are load-bearing and deliberately not collapsible: an
/// unanswerable lookup must not be mistaken for an authoritative "no PR", or a
/// dropped network / expired `gh` token would wipe every badge in the sidebar.
#[derive(Debug, Clone)]
pub enum BranchPrOutcome {
    /// A PR is associated with the branch — store it.
    Write(PullRequestInfo),
    /// The lookup succeeded and the branch genuinely has no PR — clear any
    /// stored association, including a merged/closed one.
    Clear,
    /// The lookup could not answer (gh failed or timed out, detached HEAD) —
    /// keep whatever is already stored.
    Preserve,
}

/// Single decision point shared by `refresh_workspace_pr` and both background
/// pollers, so the match/empty/error matrix can't drift between them.
pub fn branch_pr_outcome(lookup: Result<Option<PullRequestInfo>, String>) -> BranchPrOutcome {
    match lookup {
        Ok(Some(pr)) => BranchPrOutcome::Write(pr),
        Ok(None) => BranchPrOutcome::Clear,
        Err(_) => BranchPrOutcome::Preserve,
    }
}

pub fn create_pull_request(
    repo_path: &Path,
    title: &str,
    body: &str,
    base: Option<&str>,
    draft: bool,
) -> Result<PullRequestInfo, String> {
    let mut args = vec!["pr", "create", "--title", title, "--body", body];
    if let Some(b) = base {
        args.push("--base");
        args.push(b);
    }
    if draft {
        args.push("--draft");
    }
    args.extend_from_slice(&["--json", "number,url,state,title,headRefName,baseRefName,isDraft"]);

    let v = run_gh_json(repo_path, &args)?;
    Ok(parse_pr_json(&v))
}

pub fn list_pull_requests(
    repo_path: &Path,
    state: &str,
) -> Result<Vec<PullRequestInfo>, String> {
    if !gh_available() {
        return Err("gh CLI is not installed".into());
    }
    match check_gh_status() {
        GhStatus::NotInstalled => return Err("gh CLI is not installed".into()),
        GhStatus::NotAuthenticated => {
            return Err("gh CLI is not authenticated. Run: gh auth login".into())
        }
        GhStatus::Authenticated { .. } => {}
    }

    let v = run_gh_json(
        repo_path,
        &[
            "pr", "list",
            "--state", state,
            "--limit", "50",
            "--json", "number,url,state,title,headRefName,baseRefName,isDraft,updatedAt,author",
        ],
    )?;

    let arr = v.as_array().ok_or("Expected JSON array from gh pr list")?;
    Ok(arr.iter().map(parse_pr_json).collect())
}

/// Timeout for the incoming-PRs list. On a repo with thousands of PRs
/// `gh pr list` can take several seconds (or hang behind a rate-limit);
/// without a deadline the synchronous shell-out will sit on the IPC
/// runtime forever and freeze the UI. 15s is well past the p99 for
/// healthy repos and short enough that a stuck call surfaces as a
/// visible error instead of a frozen tab.
const INCOMING_PRS_TIMEOUT: Duration = Duration::from_secs(15);

pub fn list_incoming_prs(
    repo_path: &Path,
    base_branch: &str,
) -> Result<Vec<IncomingPrItem>, String> {
    // `statusCheckRollup` is intentionally omitted: it's by far the
    // most expensive field on `gh pr list` because GitHub has to
    // compute the aggregate CI state per PR server-side, and on
    // projects with lots of open PRs that single field can balloon
    // the call from ~200ms to 10s+. The incoming-list rows render a
    // small CI dot from `checks_status`; for the overview we accept
    // null status (no dot) and let the per-PR detail view fetch
    // checks separately when the user actually opens a PR.
    let output = run_gh_timed(
        repo_path,
        &[
            "pr", "list",
            "--base", base_branch,
            "--state", "open",
            "--limit", "50",
            "--json", "number,title,author,headRefName,isDraft,updatedAt,additions,deletions,reviewDecision,url",
        ],
        INCOMING_PRS_TIMEOUT,
    )?;

    let v: serde_json::Value =
        serde_json::from_str(&output).map_err(|e| format!("Failed to parse gh JSON: {e}"))?;
    let arr = v.as_array().ok_or("Expected JSON array from gh pr list")?;
    Ok(arr.iter().map(parse_incoming_pr_json).collect())
}

/// The fields the listing asks for. Named rather than inlined so the
/// split is one assertable string instead of a habit — a test can hold
/// the expensive fields out of it, which is the only thing standing
/// between this and someone adding "just one more field" back.
pub const OVERVIEW_FAST_FIELDS: &str =
    "number,title,author,headRefName,isDraft,updatedAt,reviewDecision,url,reviewRequests";

/// The fields the stats call asks for — everything the listing refused.
pub const OVERVIEW_STATS_FIELDS: &str = "number,statusCheckRollup,additions,deletions";

/// Every open pull request in this repository — the fast half.
///
/// The fields here are the ones GitHub can serve straight off the pull
/// request record: who, what, which branch, and who it is waiting on.
/// Grouping rides along deliberately (`reviewRequests` + `author`), so
/// "Needs your review" is correct in the first paint rather than after a
/// second round trip — triage is the entire reason the page exists.
///
/// What is *not* here is `statusCheckRollup`, `additions` and
/// `deletions`. Each of those is per-row work the host does on demand —
/// an aggregate CI state computed across every check run, and a diff
/// stat computed against the merge base — and together they dominate the
/// call: measured across a dozen repositories the split listing returned
/// in ~4.0s against ~7.1s for the combined one. The same reasoning
/// already applied to `list_incoming_prs` above; this is that trade
/// made for the page, with the difference that here the missing half is
/// fetched immediately afterwards by `list_prs_overview_stats` and
/// merged in, rather than dropped.
pub fn list_prs_overview(repo_path: &Path) -> Result<Vec<PrOverviewItem>, String> {
    let output = run_gh_timed(
        repo_path,
        &[
            "pr", "list",
            "--state", "open",
            "--limit", "50",
            "--json", OVERVIEW_FAST_FIELDS,
        ],
        INCOMING_PRS_TIMEOUT,
    )?;

    let v: serde_json::Value =
        serde_json::from_str(&output).map_err(|e| format!("Failed to parse gh JSON: {e}"))?;
    let arr = v.as_array().ok_or("Expected JSON array from gh pr list")?;
    Ok(arr.iter().map(parse_overview_pr_json).collect())
}

/// The slow half: CI rollup and line counts, keyed by pull request
/// number so the page can merge it into rows that are already on screen.
///
/// Deliberately a second `gh pr list` rather than a per-pull-request
/// call: the expensive fields are expensive per row either way, and one
/// request that takes four seconds beats fifty that take one each.
///
/// Returning only what it was asked for — no titles, no authors — is
/// also what makes the merge safe to write as "fill in the blanks": this
/// payload has nothing in it that could overwrite something the user is
/// already reading.
pub fn list_prs_overview_stats(repo_path: &Path) -> Result<Vec<PrOverviewStats>, String> {
    let output = run_gh_timed(
        repo_path,
        &[
            "pr", "list",
            "--state", "open",
            "--limit", "50",
            "--json", OVERVIEW_STATS_FIELDS,
        ],
        INCOMING_PRS_TIMEOUT,
    )?;

    let v: serde_json::Value =
        serde_json::from_str(&output).map_err(|e| format!("Failed to parse gh JSON: {e}"))?;
    let arr = v.as_array().ok_or("Expected JSON array from gh pr list")?;
    Ok(arr.iter().map(parse_overview_stats_json).collect())
}

/// `passing` / `failing` / `pending` / `none` — the four states a row
/// can be drawn in, decided here so both products answer in the same
/// vocabulary and the frontend never sees a raw check array.
pub fn rollup_state(v: &serde_json::Value) -> String {
    match summarize_checks_status(v).as_deref() {
        Some("failure") => "failing".to_string(),
        Some("pending") => "pending".to_string(),
        Some("success") => "passing".to_string(),
        _ => "none".to_string(),
    }
}

fn parse_overview_pr_json(v: &serde_json::Value) -> PrOverviewItem {
    PrOverviewItem {
        number: v["number"].as_u64().unwrap_or(0) as u32,
        title: v["title"].as_str().unwrap_or("").to_string(),
        author: v["author"]["login"].as_str().unwrap_or("").to_string(),
        head_branch: v["headRefName"].as_str().map(|s| s.to_string()),
        is_draft: v["isDraft"].as_bool().unwrap_or(false),
        // The stats call fills these; the fast listing never asks.
        additions: None,
        deletions: None,
        review_decision: v["reviewDecision"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        checks: None,
        review_requested_from: parse_review_requests(&v["reviewRequests"]),
        updated_at: v["updatedAt"].as_str().map(|s| s.to_string()),
        url: v["url"].as_str().unwrap_or("").to_string(),
    }
}

fn parse_overview_stats_json(v: &serde_json::Value) -> PrOverviewStats {
    PrOverviewStats {
        number: v["number"].as_u64().unwrap_or(0) as u32,
        checks: rollup_state(&v["statusCheckRollup"]),
        additions: v["additions"].as_u64().map(|n| n as u32),
        deletions: v["deletions"].as_u64().map(|n| n as u32),
    }
}

/// Merge a PR. Both the Review panel's merge sheet and the Changes
/// panel toolbar go through here.
///
/// `delete_branch` used to be unconditional, which made the one
/// irreversible action in the app also silently destroy the branch the
/// user might still be standing in. The merge sheet asks now, so the
/// answer has to be able to be "no".
///
/// `commit_title` / `commit_body` map to `--subject` / `--body`. gh
/// rejects both on `--rebase` (there is no merge commit to title), so
/// they are only passed for squash and merge commits.
pub fn merge_pull_request(
    repo_path: &Path,
    pr_number: u32,
    method: &str,
    delete_branch: bool,
    commit_title: Option<&str>,
    commit_body: Option<&str>,
) -> Result<(), String> {
    let owned = merge_args(pr_number, method, delete_branch, commit_title, commit_body);
    let args: Vec<&str> = owned.iter().map(String::as_str).collect();
    run_gh(repo_path, &args)?;
    Ok(())
}

/// The flag assembly on its own, so the rebase rule can be tested
/// without a repository or a `gh` on PATH.
fn merge_args(
    pr_number: u32,
    method: &str,
    delete_branch: bool,
    commit_title: Option<&str>,
    commit_body: Option<&str>,
) -> Vec<String> {
    let method_flag = match method {
        "squash" => "--squash",
        "rebase" => "--rebase",
        _ => "--merge",
    };
    let mut args: Vec<String> = vec![
        "pr".into(),
        "merge".into(),
        pr_number.to_string(),
        method_flag.into(),
    ];
    if delete_branch {
        args.push("--delete-branch".into());
    }
    let is_rebase = method == "rebase";
    let title = commit_title.map(str::trim).filter(|s| !s.is_empty());
    let body = commit_body.map(str::trim).filter(|s| !s.is_empty());
    if !is_rebase {
        if let Some(title) = title {
            args.push("--subject".into());
            args.push(title.into());
        }
        if let Some(body) = body {
            args.push("--body".into());
            args.push(body.into());
        }
    }
    args
}

pub fn close_pull_request(repo_path: &Path, pr_number: u32) -> Result<(), String> {
    run_gh(repo_path, &["pr", "close", &pr_number.to_string()])?;
    Ok(())
}

pub fn reopen_pull_request(repo_path: &Path, pr_number: u32) -> Result<(), String> {
    run_gh(repo_path, &["pr", "reopen", &pr_number.to_string()])?;
    Ok(())
}

/// Flip draft ↔ ready. `gh pr ready --undo` is the documented way back
/// to draft; there is no `gh pr draft`.
pub fn set_pull_request_ready(
    repo_path: &Path,
    pr_number: u32,
    ready: bool,
) -> Result<(), String> {
    let number_str = pr_number.to_string();
    let mut args: Vec<&str> = vec!["pr", "ready", &number_str];
    if !ready {
        args.push("--undo");
    }
    run_gh(repo_path, &args)?;
    Ok(())
}

/// Edit title and/or body. A `None` field is left untouched — passing
/// an empty `--body` would erase a description the user never opened.
pub fn update_pull_request(
    repo_path: &Path,
    pr_number: u32,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<(), String> {
    let number_str = pr_number.to_string();
    let mut args: Vec<&str> = vec!["pr", "edit", &number_str];
    if let Some(title) = title {
        args.push("--title");
        args.push(title);
    }
    if let Some(body) = body {
        args.push("--body");
        args.push(body);
    }
    if args.len() == 3 {
        // Nothing to change; `gh pr edit` with no field flags opens an
        // interactive prompt, which would hang the blocking pool.
        return Ok(());
    }
    run_gh(repo_path, &args)?;
    Ok(())
}

pub fn request_pull_request_review(
    repo_path: &Path,
    pr_number: u32,
    reviewer: &str,
) -> Result<(), String> {
    let reviewer = reviewer.trim();
    if reviewer.is_empty() {
        return Err("A reviewer name is required.".to_string());
    }
    run_gh(
        repo_path,
        &[
            "pr",
            "edit",
            &pr_number.to_string(),
            "--add-reviewer",
            reviewer,
        ],
    )?;
    Ok(())
}

/// How many trailing log lines a failing check's excerpt keeps.
const CHECK_LOG_EXCERPT_LINES: usize = 40;

/// Best-effort tail of a failing check's log.
///
/// Deliberately forgiving: the excerpt is a nicety on the failing-check
/// card, so every way this can come up empty (no matching run, a check
/// that isn't a GitHub Actions job, `gh run view` refusing) returns
/// `Ok("")` and the card renders without it. Only a hard CLI failure is
/// an `Err`, and even that the UI treats as "no excerpt".
pub fn get_check_log_excerpt(
    repo_path: &Path,
    pr_number: u32,
    check_name: &str,
) -> Result<String, String> {
    let number_str = pr_number.to_string();
    // Resolve the PR's head sha, then the failing workflow run on it.
    // `gh run list` is branch/commit scoped, so this avoids fetching a
    // run from an unrelated branch that happens to share a job name.
    let Some(sha) = run_gh_optional(
        repo_path,
        &[
            "pr", "view", &number_str, "--json", "headRefOid", "--jq", ".headRefOid",
        ],
    ) else {
        return Ok(String::new());
    };
    let sha = sha.trim();
    if sha.is_empty() {
        return Ok(String::new());
    }

    let Some(runs) = run_gh_optional(
        repo_path,
        &[
            "run",
            "list",
            "--commit",
            sha,
            "--limit",
            "20",
            "--json",
            "databaseId,conclusion,name",
            "--jq",
            ".[] | select(.conclusion == \"failure\") | \"\\(.databaseId)\\t\\(.name)\"",
        ],
    ) else {
        return Ok(String::new());
    };

    // A commit can fail several workflows at once, and the excerpt is
    // shown under *one* named check. Handing back the first failing run
    // regardless of name puts one workflow's log under another's card,
    // which reads as a fact and is not one.
    let Some(run_id) = pick_failing_run(&runs, check_name) else {
        return Ok(String::new());
    };

    let Some(log) = run_gh_optional(repo_path, &["run", "view", &run_id, "--log-failed"]) else {
        return Ok(String::new());
    };

    Ok(tail_check_log(&log, check_name))
}

/// Pick the failing run whose workflow name goes with `check_name`.
///
/// Rows arrive newest first as `databaseId\tname`. The names are related
/// but not equal — a check is a job inside a workflow, so GitHub renders
/// them as `CI / test (ubuntu)` against a workflow called `CI` — so the
/// same prefix convention [`tail_check_log`] uses decides it, in either
/// direction. No match is `None`: an unrelated run's log is worse than
/// no log.
fn pick_failing_run(rows: &str, check_name: &str) -> Option<String> {
    let needle = check_name.trim();
    if needle.is_empty() {
        return None;
    }
    rows.lines().find_map(|line| {
        let (id, name) = line.split_once('\t')?;
        let (id, name) = (id.trim(), name.trim());
        if id.is_empty() || name.is_empty() {
            return None;
        }
        names_match(name, needle).then(|| id.to_string())
    })
}

/// Whether a workflow name and a check name name the same thing.
fn names_match(run_name: &str, check_name: &str) -> bool {
    let run = run_name.to_ascii_lowercase();
    let check = check_name.to_ascii_lowercase();
    check.starts_with(&run) || run.starts_with(&check)
}

/// Trim a `gh run view --log-failed` dump to the interesting tail.
///
/// The dump prefixes every line with `job\tstep\ttimestamp `. Lines for
/// the named job are preferred; when the name doesn't match anything
/// (job names and check names diverge on matrix builds) the whole log's
/// tail is used rather than returning nothing.
fn tail_check_log(log: &str, check_name: &str) -> String {
    let needle = check_name.trim();
    let matching: Vec<&str> = if needle.is_empty() {
        Vec::new()
    } else {
        log.lines().filter(|l| l.starts_with(needle)).collect()
    };
    let lines: Vec<&str> = if matching.is_empty() {
        log.lines().collect()
    } else {
        matching
    };
    let start = lines.len().saturating_sub(CHECK_LOG_EXCERPT_LINES);
    lines[start..]
        .iter()
        .map(|l| strip_log_prefix(l))
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drop the `job\tstep\t2026-01-01T00:00:00.000Z ` prefix gh puts on
/// every log line, leaving the message the user actually wants to read.
fn strip_log_prefix(line: &str) -> &str {
    let rest = match line.rsplit_once('\t') {
        Some((_, rest)) => rest,
        None => line,
    };
    // The timestamp is the first whitespace-delimited token of what's
    // left; it is only a prefix when it parses as one.
    match rest.split_once(' ') {
        Some((first, tail)) if first.len() >= 20 && first.contains('T') && first.ends_with('Z') => {
            tail
        }
        _ => rest,
    }
}

/// Checks for a pull request. `number = None` means "whatever PR the
/// checked-out branch has", which is the panel's case; the Pull Requests
/// page passes a number because the PR being read is very often not the
/// one this checkout is standing on.
pub fn get_pr_checks(repo_path: &Path, number: Option<u32>) -> Result<Vec<CheckInfo>, String> {
    // `gh pr checks --json` only accepts a specific field set:
    // bucket / completedAt / description / event / link / name /
    // startedAt / state / workflow. The previous version asked for
    // `conclusion,elapsedTime,detailUrl` — none of which exist on
    // gh's side, so gh wrote "Unknown JSON field" to stderr and
    // stdout was empty for every PR. That's why the Checks section
    // showed "No checks reported." even when CI was actively running.
    //
    // Mapping to our CheckInfo struct:
    //   gh `bucket` ("pass" / "fail" / "pending" / "skipping" /
    //                "cancel") → conclusion (matches the strings the
    //                frontend's CheckIcon already understands).
    //   gh `link`            → detail_url
    //   gh `state`           → status (raw "IN_PROGRESS" / "COMPLETED"
    //                                   / "QUEUED" — fallback when
    //                                   bucket is empty).
    //   elapsed_time         → None for now (gh doesn't expose it on
    //                                  this command; could be derived
    //                                  later from started/completed).
    //
    // Also: `gh pr checks` exits non-zero when checks are pending
    // (exit 1) or any have failed (exit 8) but still writes valid
    // JSON to stdout. Bypass `run_gh_optional` (which discards stdout
    // on non-zero exit) and capture stdout regardless.
    let number_str = number.map(|n| n.to_string());
    let mut args: Vec<&str> = vec!["pr", "checks"];
    if let Some(number) = &number_str {
        args.push(number);
    }
    args.extend_from_slice(&["--json", "name,state,bucket,link,startedAt,completedAt"]);

    let output = crate::execution::host_command("gh")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to run gh: {e}"))?;

    let json_str = String::from_utf8_lossy(&output.stdout).trim_end().to_string();
    if json_str.is_empty() {
        return Ok(Vec::new());
    }

    let v: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse checks JSON: {e}"))?;

    let arr = v.as_array().ok_or("Expected JSON array from gh pr checks")?;
    Ok(arr
        .iter()
        .map(|c| CheckInfo {
            name: c["name"].as_str().unwrap_or("").to_string(),
            status: c["state"].as_str().unwrap_or("pending").to_string(),
            conclusion: c["bucket"].as_str().map(|s| s.to_string()),
            elapsed_time: None,
            detail_url: c["link"].as_str().map(|s| s.to_string()),
            started_at: c["startedAt"].as_str().map(|s| s.to_string()),
            completed_at: c["completedAt"].as_str().map(|s| s.to_string()),
        })
        .collect())
}

/// Conversation-level reviews. `number = None` reads the current
/// branch's PR; the Pull Requests page passes the selected one.
pub fn get_pr_review_comments(
    repo_path: &Path,
    number: Option<u32>,
) -> Result<Vec<ReviewComment>, String> {
    let number_str = number.map(|n| n.to_string());
    let mut args: Vec<&str> = vec!["pr", "view"];
    if let Some(number) = &number_str {
        args.push(number);
    }
    args.extend_from_slice(&["--json", "reviews"]);
    let output = run_gh_optional(repo_path, &args);
    let Some(json_str) = output else {
        return Ok(Vec::new());
    };
    if json_str.is_empty() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| format!("Failed to parse reviews JSON: {e}"))?;
    let arr = v["reviews"]
        .as_array()
        .ok_or("Expected reviews array")?;
    Ok(arr
        .iter()
        .map(|r| ReviewComment {
            id: r["id"].as_u64().unwrap_or(0),
            author: r["author"]["login"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            body: r["body"].as_str().unwrap_or("").to_string(),
            state: r["state"]
                .as_str()
                .unwrap_or("COMMENTED")
                .to_string(),
            created_at: r["submittedAt"]
                .as_str()
                .unwrap_or("")
                .to_string(),
        })
        .filter(|r| !r.body.is_empty())
        .collect())
}

/// Get "owner/repo" string for API calls.
fn get_repo_nwo(repo_path: &Path) -> Result<String, String> {
    run_gh(repo_path, &["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"])
}

pub fn get_pr_inline_comments(
    repo_path: &Path,
    pr_number: u32,
) -> Result<Vec<InlineReviewComment>, String> {
    let nwo = get_repo_nwo(repo_path)?;
    let endpoint = format!("repos/{}/pulls/{}/comments", nwo, pr_number);
    let output = run_gh_optional(repo_path, &["api", &endpoint, "--paginate"]);

    let Some(json_str) = output else {
        return Ok(Vec::new());
    };
    if json_str.is_empty() {
        return Ok(Vec::new());
    }

    let v: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| format!("Failed to parse inline comments JSON: {e}"))?;
    let arr = v.as_array().ok_or("Expected JSON array from inline comments API")?;

    Ok(arr
        .iter()
        .map(|c| InlineReviewComment {
            id: c["id"].as_u64().unwrap_or(0),
            author: c["user"]["login"].as_str().unwrap_or("").to_string(),
            body: c["body"].as_str().unwrap_or("").to_string(),
            path: c["path"].as_str().unwrap_or("").to_string(),
            line: c["line"].as_u64().map(|n| n as u32),
            created_at: c["created_at"].as_str().unwrap_or("").to_string(),
            in_reply_to_id: c["in_reply_to_id"].as_u64(),
            pull_request_review_id: c["pull_request_review_id"].as_u64(),
        })
        .filter(|c| !c.body.is_empty())
        .collect())
}

// ── Review threads ──
//
// Three calls, and the first of them is the reason the other two exist.
//
// `GET /pulls/{n}/comments` — what `get_pr_inline_comments` above reads —
// returns comments, not conversations: no thread id, no resolution state,
// no "this no longer matches a line". Grouping it by `pull_request_review_id`
// (which is what this surface used to do) reconstructs *who said it in one
// sitting*, not *what is still open*, and those are different questions.
// Only GraphQL's `reviewThreads` answers the second, so that is where the
// thread list comes from.

/// The comment selection both thread queries read, named once so the
/// follow-up page and the first page cannot drift apart.
const THREAD_COMMENT_NODES: &str = "nodes{id databaseId author{login} body createdAt}";

/// How many long threads will be re-fetched in full. A pull request with
/// more than this many hundred-comment threads is not a review surface
/// any more, and the cap keeps a pathological one from firing a request
/// storm at the host.
const MAX_THREAD_COMMENT_REFETCHES: usize = 20;

/// The thread query, written for `gh api graphql --paginate`.
///
/// `--paginate` requires exactly two things of a query: an `$endCursor`
/// variable it can fill, and a `pageInfo` on the connection it should
/// follow. Both are here, so a pull request with more than one page of
/// threads yields one JSON document per page rather than a truncated
/// first page.
///
/// `--paginate` can only follow *one* connection, and it follows the
/// thread list. A thread's own comments therefore come back capped, and
/// the `pageInfo` on them is how [`get_pr_review_threads`] learns which
/// threads it has to ask about again.
fn review_threads_query() -> String {
    format!(
        "\
query($owner:String!,$name:String!,$number:Int!,$endCursor:String){{\
repository(owner:$owner,name:$name){{\
pullRequest(number:$number){{\
reviewThreads(first:50,after:$endCursor){{\
pageInfo{{hasNextPage endCursor}}\
nodes{{id isResolved isOutdated path line \
comments(first:100){{pageInfo{{hasNextPage endCursor}}{THREAD_COMMENT_NODES}}}}}}}}}}}}}"
    )
}

/// Every comment on one thread, for the threads the first query could
/// not finish. `--paginate` follows this connection because it is the
/// only one in the document.
fn thread_comments_query() -> String {
    format!(
        "\
query($threadId:ID!,$endCursor:String){{\
node(id:$threadId){{... on PullRequestReviewThread{{\
comments(first:100,after:$endCursor){{\
pageInfo{{hasNextPage endCursor}}{THREAD_COMMENT_NODES}}}}}}}}}"
    )
}

/// Conversation threads on a pull request's diff, with their resolution
/// state.
pub fn get_pr_review_threads(
    repo_path: &Path,
    pr_number: u32,
) -> Result<Vec<PrReviewThread>, String> {
    let nwo = get_repo_nwo(repo_path)?;
    let (owner, name) = nwo
        .split_once('/')
        .ok_or_else(|| format!("Unexpected repository name: {nwo}"))?;

    // Deliberately `run_gh`, not `run_gh_optional`: a failed thread fetch
    // has to reach the caller as an error so the surface keeps the
    // threads it already has and says how old they are. An empty list
    // means "no threads", and it may only ever mean that.
    let json = run_gh(
        repo_path,
        &[
            "api",
            "graphql",
            "--paginate",
            "-f",
            &format!("owner={owner}"),
            "-f",
            &format!("name={name}"),
            // Typed, so `number` arrives as the Int! the query declares.
            "-F",
            &format!("number={pr_number}"),
            "-f",
            &format!("query={}", review_threads_query()),
        ],
    )?;

    let mut threads = parse_review_threads(&json);

    // A thread longer than one page came back cut off. Best effort from
    // here: a thread that keeps only its first page is still readable,
    // so a failed follow-up costs that thread's tail, not the tab.
    for thread_id in truncated_thread_ids(&json)
        .into_iter()
        .take(MAX_THREAD_COMMENT_REFETCHES)
    {
        let Some(thread) = threads.iter_mut().find(|t| t.id == thread_id) else {
            continue;
        };
        let Ok(page) = run_gh(
            repo_path,
            &[
                "api",
                "graphql",
                "--paginate",
                "-f",
                &format!("threadId={thread_id}"),
                "-f",
                &format!("query={}", thread_comments_query()),
            ],
        ) else {
            continue;
        };
        let comments = parse_thread_comments(&page);
        // Only ever a superset. A follow-up that came back shorter than
        // what is already on screen is a bad answer, not a shorter
        // thread.
        if comments.len() > thread.comments.len() {
            thread.comments = comments;
        }
    }

    Ok(threads)
}

/// Ids of the threads whose comment connection reported another page.
pub(crate) fn truncated_thread_ids(json: &str) -> Vec<String> {
    let mut out = Vec::new();
    for page in parse_paginated_array(json) {
        let Some(nodes) =
            page["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"].as_array()
        else {
            continue;
        };
        for node in nodes {
            if node["comments"]["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                continue;
            }
            let id = node["id"].as_str().unwrap_or_default();
            if !id.is_empty() {
                out.push(id.to_string());
            }
        }
    }
    out
}

/// One comment node, mapped. Shared so the first page and the follow-up
/// pages cannot disagree about what a comment is.
fn parse_thread_comment(c: &serde_json::Value) -> PrThreadComment {
    PrThreadComment {
        id: c["id"].as_str().unwrap_or_default().to_string(),
        database_id: c["databaseId"].as_u64(),
        author: c["author"]["login"].as_str().unwrap_or("").to_string(),
        body: c["body"].as_str().unwrap_or("").to_string(),
        created_at: c["createdAt"].as_str().unwrap_or("").to_string(),
    }
}

/// Every comment in a `thread_comments_query` response, pagination and
/// all.
pub(crate) fn parse_thread_comments(json: &str) -> Vec<PrThreadComment> {
    let mut out = Vec::new();
    for page in parse_paginated_array(json) {
        let Some(nodes) = page["data"]["node"]["comments"]["nodes"].as_array() else {
            continue;
        };
        out.extend(
            nodes
                .iter()
                .map(parse_thread_comment)
                .filter(|c: &PrThreadComment| !c.body.is_empty()),
        );
    }
    out
}

/// Map the GraphQL payload, pagination and all.
///
/// Split from the subprocess call so the shape can be tested against a
/// recorded two-page payload: the parsing is the part that breaks, and
/// the part a schema change would break silently.
pub(crate) fn parse_review_threads(json: &str) -> Vec<PrReviewThread> {
    let mut out = Vec::new();

    for page in parse_paginated_array(json) {
        let Some(nodes) =
            page["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"].as_array()
        else {
            continue;
        };

        for node in nodes {
            let id = node["id"].as_str().unwrap_or_default().to_string();
            if id.is_empty() {
                continue;
            }

            let comments: Vec<PrThreadComment> = node["comments"]["nodes"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .map(parse_thread_comment)
                        .filter(|c: &PrThreadComment| !c.body.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            // A thread with nothing readable in it is not a thread. It
            // would render as an empty card with a Resolve button, which
            // is the worst of both.
            if comments.is_empty() {
                continue;
            }

            out.push(PrReviewThread {
                id,
                is_resolved: node["isResolved"].as_bool().unwrap_or(false),
                is_outdated: node["isOutdated"].as_bool().unwrap_or(false),
                // Every GitHub review thread can be resolved.
                is_resolvable: true,
                path: node["path"].as_str().map(|s| s.to_string()),
                line: node["line"].as_u64().map(|n| n as u32),
                comments,
            });
        }
    }

    out
}

/// Post a reply into an existing thread.
///
/// REST, not the `addPullRequestReviewThreadReply` mutation, for one
/// reason: this endpoint's semantics are unambiguous — the reply is
/// created, published, and attributed to the caller, full stop. The
/// mutation's `pullRequestReviewId` input is optional in the schema but
/// its behaviour when omitted (does the reply publish, or does it sit in
/// a pending review only the author can see?) is not something this build
/// can verify against a live host, and a reply that silently lands in a
/// draft review is exactly the failure this surface is supposed to make
/// impossible. The thread payload already carries the comment's REST id,
/// so the certain route costs nothing.
pub fn reply_to_pr_thread(
    repo_path: &Path,
    pr_number: u32,
    root_comment_id: u64,
    body: &str,
) -> Result<(), String> {
    if body.trim().is_empty() {
        return Err("A reply cannot be empty.".to_string());
    }
    let nwo = get_repo_nwo(repo_path)?;
    let endpoint = format!("repos/{nwo}/pulls/{pr_number}/comments/{root_comment_id}/replies");
    let payload = serde_json::json!({ "body": body });
    let json = serde_json::to_string(&payload).map_err(|e| e.to_string())?;

    run_gh_stdin(
        repo_path,
        &["api", "--method", "POST", &endpoint, "--input", "-"],
        &json,
    )?;
    Ok(())
}

/// Resolve or unresolve a thread.
///
/// GraphQL-only: resolution has no REST representation at all, in either
/// direction.
pub fn set_pr_thread_resolved(
    repo_path: &Path,
    thread_id: &str,
    resolved: bool,
) -> Result<(), String> {
    let mutation = if resolved {
        "mutation($threadId:ID!){resolveReviewThread(input:{threadId:$threadId}){thread{id isResolved}}}"
    } else {
        "mutation($threadId:ID!){unresolveReviewThread(input:{threadId:$threadId}){thread{id isResolved}}}"
    };

    run_gh(
        repo_path,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={mutation}"),
            "-f",
            &format!("threadId={thread_id}"),
        ],
    )?;
    Ok(())
}

// NOTE: As of 2026-04-26 the Review tab UI no longer exposes review
// submission — the composer was removed in the visual-match PR
// (`feature/review-tab-visual-match`) in favour of a quieter resting
// layout. This command is retained intact in case the UI is restored
// later (e.g. command palette action, modal, or context menu). Don't
// delete without confirming with the maintainer.
pub fn submit_pr_review(
    repo_path: &Path,
    pr_number: u32,
    event: &str,
    body: &str,
) -> Result<(), String> {
    let number_str = pr_number.to_string();
    let event_flag = match event {
        "approve" => "--approve",
        "request-changes" => "--request-changes",
        _ => "--comment",
    };
    let mut args = vec!["pr", "review", &number_str, event_flag];
    if !body.is_empty() {
        args.push("--body");
        args.push(body);
    }
    run_gh(repo_path, &args)?;
    Ok(())
}

/// One pending line note, in the coordinates GitHub's modern review API
/// speaks: a file, a side, and a line number on that side.
///
/// Not the legacy `position` (an offset into the patch text): position
/// is only meaningful against one exact diff, so a comment written a
/// second before a push lands somewhere arbitrary. `line` + `side` are
/// content coordinates and the host re-resolves them itself.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PrDraftComment {
    /// Path of the file the note is on.
    pub file: String,
    pub body: String,
    /// `LEFT` (a deleted line) or `RIGHT` (added or context).
    pub side: String,
    /// Last line of the selection.
    pub line: u32,
    /// First line of a multi-line selection; absent for one line.
    #[serde(default)]
    pub start_line: Option<u32>,
}

impl PrDraftComment {
    fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "path": self.file,
            "body": self.body,
            "side": self.side,
            "line": self.line,
        });
        if let Some(start) = self.start_line {
            v["start_line"] = serde_json::json!(start);
            // A range can't straddle sides, so the start side is always
            // the end side; sending it makes that explicit to the API.
            v["start_side"] = serde_json::json!(self.side);
        }
        v
    }
}

/// Run `gh` with a JSON body on stdin.
///
/// `gh api -f k=v` can only build flat string fields, and a review
/// carries an array of comment objects. `--input -` is the only way to
/// post a real JSON document in one command.
fn run_gh_stdin(repo_path: &Path, args: &[&str], stdin: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;

    let mut cmd = crate::execution::host_command("gh");
    cmd.args(args)
        .current_dir(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Keep DBus available — see `check_gh_status` rationale.
    sanitize_gui_env_std_keep_dbus(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| format!("Failed to run gh: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("gh stdin unavailable")?
        .write_all(stdin.as_bytes())
        .map_err(|e| format!("Failed to write to gh: {e}"))?;
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for gh: {e}"))?;

    gh_exit_result(
        args.first().unwrap_or(&""),
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// Post one inline comment immediately, outside any review.
///
/// `commit_id` must be the head the diff on screen was rendered from. A
/// stale one does not fail: GitHub accepts it and pins the comment to
/// the superseded commit, where it shows as outdated and nobody reads
/// it. The caller re-anchors before calling this, and that is the only
/// reason it is safe.
pub fn add_pr_inline_comment(
    repo_path: &Path,
    pr_number: u32,
    comment: &PrDraftComment,
    commit_id: &str,
) -> Result<(), String> {
    let nwo = get_repo_nwo(repo_path)?;
    let endpoint = format!("repos/{nwo}/pulls/{pr_number}/comments");
    let mut payload = comment.to_json();
    payload["commit_id"] = serde_json::json!(commit_id);
    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;

    run_gh_stdin(
        repo_path,
        &["api", "--method", "POST", &endpoint, "--input", "-"],
        &body,
    )?;
    Ok(())
}

fn review_event(event: &str) -> &'static str {
    match event {
        "approve" => "APPROVE",
        "request-changes" => "REQUEST_CHANGES",
        _ => "COMMENT",
    }
}

/// Submit a review and all of its line notes as one request.
///
/// One request because the alternative — a review plus N comment posts
/// — can half-succeed, and a half-sent review is visible to the author
/// while the reviewer still thinks it's a draft. GitHub makes this
/// all-or-nothing: a single unresolvable line 422s the whole call and
/// nothing is created. The caller validates every anchor locally first
/// so that 422 is a bug, not a workflow.
pub fn submit_pr_review_with_comments(
    repo_path: &Path,
    pr_number: u32,
    event: &str,
    body: &str,
    comments: &[PrDraftComment],
    commit_id: &str,
) -> Result<(), String> {
    if comments.is_empty() {
        // No line notes: the plain `gh pr review` path stays exactly as
        // it was, including its handling of an empty body.
        return submit_pr_review(repo_path, pr_number, event, body);
    }

    let nwo = get_repo_nwo(repo_path)?;
    let endpoint = format!("repos/{nwo}/pulls/{pr_number}/reviews");
    let payload = serde_json::json!({
        "commit_id": commit_id,
        "body": body,
        "event": review_event(event),
        "comments": comments.iter().map(|c| c.to_json()).collect::<Vec<_>>(),
    });
    let json = serde_json::to_string(&payload).map_err(|e| e.to_string())?;

    run_gh_stdin(
        repo_path,
        &["api", "--method", "POST", &endpoint, "--input", "-"],
        &json,
    )
    .map_err(|e| {
        if is_stale_anchor_error(&e) {
            "One of your notes no longer matches a line on this branch — re-anchor and try again.".to_string()
        } else {
            e
        }
    })?;
    Ok(())
}

/// Whether a review 422 means "an anchor went stale between the local
/// check and the send".
///
/// GitHub does not have one sentence for this. The modern
/// `line`/`start_line` API rejects a line that has moved with
/// `pull_request_review_thread.line must be part of the diff` and a range
/// that no longer sits in one hunk with `… must be part of the same
/// hunk`; the legacy `position` API said `Line could not be resolved`.
/// All three are the same event to the reviewer, and the message they
/// need is the same: re-anchor.
fn is_stale_anchor_error(error: &str) -> bool {
    error.contains("Line could not be resolved")
        || error.contains("must be part of the diff")
        || error.contains("must be part of the same hunk")
}

pub fn get_pr_deployments(
    repo_path: &Path,
    pr_number: u32,
) -> Result<Vec<DeploymentInfo>, String> {
    let nwo = get_repo_nwo(repo_path)?;

    // Get the PR head SHA to filter deployments
    let pr_json = run_gh_optional(
        repo_path,
        &["pr", "view", &pr_number.to_string(), "--json", "headRefOid", "--jq", ".headRefOid"],
    );

    let endpoint = if let Some(sha) = &pr_json {
        format!("repos/{}/deployments?per_page=5&sha={}", nwo, sha)
    } else {
        format!("repos/{}/deployments?per_page=5", nwo)
    };

    let output = run_gh_optional(repo_path, &["api", &endpoint]);
    let Some(json_str) = output else {
        return Ok(Vec::new());
    };
    if json_str.is_empty() {
        return Ok(Vec::new());
    }

    let v: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| format!("Failed to parse deployments JSON: {e}"))?;
    let arr = v.as_array().ok_or("Expected JSON array from deployments API")?;

    let mut deployments = Vec::new();
    for d in arr {
        let dep_id = d["id"].as_u64().unwrap_or(0);
        let environment = d["environment"].as_str().unwrap_or("").to_string();
        let created_at = d["created_at"].as_str().unwrap_or("").to_string();

        // Fetch the latest status to get target_url
        let status_endpoint = format!("repos/{}/deployments/{}/statuses?per_page=1", nwo, dep_id);
        let status_output = run_gh_optional(repo_path, &["api", &status_endpoint]);

        let (state, url) = if let Some(status_json) = status_output {
            if let Ok(sv) = serde_json::from_str::<serde_json::Value>(&status_json) {
                if let Some(first) = sv.as_array().and_then(|a| a.first()) {
                    let st = first["state"].as_str().unwrap_or("unknown").to_string();
                    let u = first["target_url"]
                        .as_str()
                        .or_else(|| first["environment_url"].as_str())
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty());
                    (st, u)
                } else {
                    ("unknown".to_string(), None)
                }
            } else {
                ("unknown".to_string(), None)
            }
        } else {
            ("unknown".to_string(), None)
        };

        deployments.push(DeploymentInfo {
            id: dep_id,
            environment,
            state,
            url,
            created_at,
        });
    }

    Ok(deployments)
}

fn parse_pr_json(v: &serde_json::Value) -> PullRequestInfo {
    // `author` may live at top level (`gh pr view --json author`) as
    // `{login: …}` or be omitted entirely on list rows that didn't
    // ask for it. Treat both as None-yielding.
    let author = v["author"]["login"]
        .as_str()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    PullRequestInfo {
        number: v["number"].as_u64().unwrap_or(0) as u32,
        url: v["url"].as_str().unwrap_or("").to_string(),
        state: v["state"].as_str().unwrap_or("OPEN").to_string(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        head_branch: v["headRefName"].as_str().map(|s| s.to_string()),
        base_branch: v["baseRefName"].as_str().map(|s| s.to_string()),
        is_draft: v["isDraft"].as_bool().unwrap_or(false),
        mergeable: v["mergeable"].as_str().map(|s| s.to_string()),
        additions: v["additions"].as_u64().map(|n| n as u32),
        deletions: v["deletions"].as_u64().map(|n| n as u32),
        review_decision: v["reviewDecision"].as_str().map(|s| s.to_string()),
        checks_passing: None, // populated separately via get_pr_checks
        updated_at: v["updatedAt"].as_str().map(|s| s.to_string()),
        head_ref_oid: v["headRefOid"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        // gh nests this as `{"id": …, "login": …}`; only the login is useful.
        head_repository_owner: v["headRepositoryOwner"]["login"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        // Detail-only fields. List paths leave these None / empty so
        // the cheap query stays cheap.
        body: v["body"]
            .as_str()
            .map(truncate_pr_body)
            .filter(|s| !s.is_empty()),
        comments: Vec::new(),
        total_comments: 0,
        author,
        created_at: v["createdAt"].as_str().map(|s| s.to_string()),
        merge_state_status: v["mergeStateStatus"].as_str().map(|s| s.to_string()),
        changed_files: v["changedFiles"].as_u64().map(|n| n as u32),
        merged_by: v["mergedBy"]["login"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty()),
        merged_at: v["mergedAt"].as_str().map(|s| s.to_string()),
        review_requests: parse_review_requests(&v["reviewRequests"]),
        latest_reviews: parse_latest_reviews(&v["latestReviews"]),
    }
}

/// Truncate a PR body at [`MAX_ISSUE_BODY_BYTES`] on a char boundary.
fn truncate_pr_body(body: &str) -> String {
    if body.len() <= MAX_ISSUE_BODY_BYTES {
        return body.to_string();
    }
    let mut end = MAX_ISSUE_BODY_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[Description truncated]", &body[..end])
}

/// `reviewRequests` is a list of requested reviewers; a team request has
/// `name` where a user request has `login`. Both are worth naming.
fn parse_review_requests(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    r["login"]
                        .as_str()
                        .or_else(|| r["name"].as_str())
                        .or_else(|| r["slug"].as_str())
                })
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_latest_reviews(v: &serde_json::Value) -> Vec<PrReviewSummary> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let author = r["author"]["login"].as_str().filter(|s| !s.is_empty())?;
                    Some(PrReviewSummary {
                        author: author.to_string(),
                        state: r["state"].as_str().unwrap_or("COMMENTED").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn summarize_checks_status(v: &serde_json::Value) -> Option<String> {
    let checks = v.as_array()?;
    if checks.is_empty() {
        return None;
    }
    let mut has_pending = false;
    for check in checks {
        let state = check["state"].as_str().unwrap_or("");
        let conclusion = check["conclusion"].as_str().unwrap_or("");
        let effective = if conclusion.is_empty() { state } else { conclusion };
        match effective.to_uppercase().as_str() {
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STALE" => {
                return Some("failure".to_string());
            }
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
            _ => has_pending = true,
        }
    }
    Some(if has_pending { "pending".to_string() } else { "success".to_string() })
}

fn parse_incoming_pr_json(v: &serde_json::Value) -> IncomingPrItem {
    let author = v["author"]["login"]
        .as_str()
        .unwrap_or("")
        .to_string();

    IncomingPrItem {
        number: v["number"].as_u64().unwrap_or(0) as u32,
        title: v["title"].as_str().unwrap_or("").to_string(),
        author,
        head_branch: v["headRefName"].as_str().map(|s| s.to_string()),
        is_draft: v["isDraft"].as_bool().unwrap_or(false),
        additions: v["additions"].as_u64().map(|n| n as u32),
        deletions: v["deletions"].as_u64().map(|n| n as u32),
        review_decision: v["reviewDecision"].as_str().map(|s| s.to_string()),
        checks_status: summarize_checks_status(&v["statusCheckRollup"]),
        updated_at: v["updatedAt"].as_str().map(|s| s.to_string()),
        url: v["url"].as_str().unwrap_or("").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pr_info_from_gh_json() {
        let json = r#"{
            "number": 42,
            "url": "https://github.com/user/repo/pull/42",
            "state": "OPEN",
            "title": "Add authentication",
            "headRefName": "feature/auth",
            "baseRefName": "main",
            "mergeable": "MERGEABLE",
            "additions": 150,
            "deletions": 23,
            "reviewDecision": "APPROVED",
            "statusCheckRollup": []
        }"#;
        let pr: PullRequestInfo = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, "OPEN");
        assert_eq!(pr.head_branch.as_deref(), Some("feature/auth"));
        assert_eq!(pr.additions, Some(150));
        assert_eq!(pr.review_decision.as_deref(), Some("APPROVED"));
    }

    #[test]
    fn test_parse_pr_info_captures_head_ref_oid() {
        // Keep the remote SHA available as metadata without using it as the
        // identity of a branch's PR.
        let json = serde_json::json!({
            "number": 1,
            "url": "https://example",
            "state": "MERGED",
            "title": "T",
            "headRefOid": "abc123def456",
        });
        let pr = parse_pr_json(&json);
        assert_eq!(pr.head_ref_oid.as_deref(), Some("abc123def456"));
    }

    #[test]
    fn test_parse_pr_info_treats_missing_head_ref_oid_as_none() {
        // Older gh output and callers that request a smaller JSON shape may
        // omit the SHA. Branch association must not depend on its presence.
        let json = serde_json::json!({
            "number": 1,
            "url": "https://example",
            "state": "OPEN",
            "title": "T",
        });
        let pr = parse_pr_json(&json);
        assert!(pr.head_ref_oid.is_none());
    }

    fn branch_pr(
        number: u32,
        state: &str,
        branch: &str,
        updated_at: &str,
        head_ref_oid: Option<&str>,
    ) -> PullRequestInfo {
        PullRequestInfo {
            number,
            url: format!("https://example/pull/{number}"),
            state: state.into(),
            title: format!("PR {number}"),
            head_branch: Some(branch.into()),
            base_branch: Some("main".into()),
            is_draft: false,
            mergeable: None,
            additions: None,
            deletions: None,
            review_decision: None,
            checks_passing: None,
            updated_at: Some(updated_at.into()),
            created_at: None,
            head_ref_oid: head_ref_oid.map(|s| s.to_string()),
            head_repository_owner: None,
            body: None,
            comments: Vec::new(),
            total_comments: 0,
            author: None,
            merge_state_status: None,
            changed_files: None,
            merged_by: None,
            merged_at: None,
            review_requests: Vec::new(),
            latest_reviews: Vec::new(),
        }
    }

    /// `branch_pr` plus a head-repository owner, for the fork-disambiguation
    /// cases. `None` models a row where gh reported no owner at all.
    fn branch_pr_owned_by(
        number: u32,
        state: &str,
        branch: &str,
        updated_at: &str,
        owner: Option<&str>,
    ) -> PullRequestInfo {
        PullRequestInfo {
            head_repository_owner: owner.map(|s| s.to_string()),
            ..branch_pr(number, state, branch, updated_at, None)
        }
    }

    #[test]
    fn test_parse_github_remote_supports_ssh_and_https() {
        assert_eq!(
            parse_github_remote("git@github.com:Zeus-Deus/codemux.git"),
            Some(("zeus-deus".into(), "codemux".into()))
        );
        assert_eq!(
            parse_github_remote("https://github.com/Zeus-Deus/codemux.git"),
            Some(("zeus-deus".into(), "codemux".into()))
        );
        assert_eq!(
            parse_github_remote("ssh://git@github.com/Zeus-Deus/codemux.git"),
            Some(("zeus-deus".into(), "codemux".into()))
        );
    }

    #[test]
    fn test_parse_github_remote_rejects_other_hosts_and_malformed_paths() {
        assert_eq!(
            parse_github_remote("https://notgithub.com/owner/repo.git"),
            None
        );
        assert_eq!(parse_github_remote("git@gitlab.com:owner/repo.git"), None);
        assert_eq!(parse_github_remote("https://github.com/owner"), None);
    }

    #[test]
    fn test_branch_head_owner_filter_is_unset_inside_the_origin_repo() {
        // Same repo (case-insensitively) — every PR gh returns for the branch
        // is ours, so no owner filter is needed.
        assert_eq!(
            branch_head_owner_filter(
                Some("git@github.com:owner/repo.git"),
                Some("https://github.com/OWNER/repo.git"),
            ),
            None
        );
    }

    #[test]
    fn test_branch_head_owner_filter_names_the_fork_owner() {
        assert_eq!(
            branch_head_owner_filter(
                Some("git@github.com:contributor/repo.git"),
                Some("https://github.com/upstream/repo.git"),
            ),
            Some("contributor".to_string())
        );
    }

    #[test]
    fn test_branch_head_owner_filter_is_unset_for_unparseable_remotes() {
        // A non-GitHub or missing remote yields no filter rather than a
        // filter that would reject everything.
        assert_eq!(
            branch_head_owner_filter(
                Some("git@gitlab.com:contributor/repo.git"),
                Some("https://github.com/upstream/repo.git"),
            ),
            None
        );
        assert_eq!(
            branch_head_owner_filter(None, Some("https://github.com/upstream/repo.git")),
            None
        );
    }

    #[test]
    fn test_select_branch_pr_keeps_only_the_fork_owners_pr() {
        // Both PRs share the branch name; only the fork's is this
        // workspace's. gh cannot express this server-side (`--head
        // owner:branch` matches nothing), so the filter runs here.
        let upstream = branch_pr_owned_by(
            10,
            "OPEN",
            "feature/popup",
            "2026-08-02T10:00:00Z",
            Some("upstream"),
        );
        let fork = branch_pr_owned_by(
            9,
            "OPEN",
            "feature/popup",
            "2026-08-01T10:00:00Z",
            Some("Contributor"),
        );
        // Owner comparison is case-insensitive: git remotes are normalised to
        // lowercase, gh reports GitHub's canonical casing.
        assert_eq!(
            select_branch_pr(
                [upstream, fork],
                "feature/popup",
                Some("contributor"),
                false
            )
            .map(|pr| pr.number),
            Some(9)
        );
    }

    #[test]
    fn test_select_branch_pr_rejects_prs_with_no_owner_when_a_fork_is_expected() {
        // An unconfirmable row must not be attached to a fork workspace.
        let unknown = branch_pr_owned_by(11, "OPEN", "feature/popup", "2026-08-02T10:00:00Z", None);
        assert!(select_branch_pr([unknown], "feature/popup", Some("contributor"), false).is_none());
    }

    #[test]
    fn test_select_branch_pr_ignores_owner_when_no_fork_is_expected() {
        // Same-repo branches must keep matching even though the list query
        // now carries owner data.
        let pr = branch_pr_owned_by(12, "OPEN", "feature/popup", "2026-08-02T10:00:00Z", None);
        assert_eq!(
            select_branch_pr([pr], "feature/popup", None, false).map(|pr| pr.number),
            Some(12)
        );
    }

    #[test]
    fn test_select_branch_pr_keeps_merged_pr_when_remote_sha_advanced() {
        // Regression: review commits can advance GitHub's PR head beyond the
        // still-open local worktree. Selection is intentionally independent
        // of the local SHA, so the MERGED state still reaches the sidebar.
        let merged = branch_pr(
            231,
            "MERGED",
            "feature/popup",
            "2026-08-01T10:00:00Z",
            Some("remote-sha-after-review"),
        );
        let selected = select_branch_pr([merged], "feature/popup", None, false).unwrap();
        assert_eq!(selected.number, 231);
        assert_eq!(selected.state, "MERGED");
    }

    #[test]
    fn test_select_branch_pr_hides_historical_prs_on_default_branch() {
        let merged = branch_pr(23, "MERGED", "main", "2026-08-01T10:00:00Z", None);
        let closed = branch_pr(24, "CLOSED", "main", "2026-08-02T10:00:00Z", None);
        assert!(select_branch_pr([merged, closed], "main", None, true).is_none());
    }

    #[test]
    fn test_select_branch_pr_allows_open_pr_on_default_branch() {
        let open = branch_pr(25, "OPEN", "main", "2026-08-01T10:00:00Z", None);
        assert_eq!(
            select_branch_pr([open], "main", None, true).map(|pr| pr.number),
            Some(25)
        );
    }

    #[test]
    fn test_select_branch_pr_prefers_open_over_newer_merged_pr() {
        let merged = branch_pr(45, "MERGED", "feature/reused", "2026-08-02T10:00:00Z", None);
        let open = branch_pr(46, "OPEN", "feature/reused", "2026-08-01T10:00:00Z", None);
        assert_eq!(
            select_branch_pr([merged, open], "feature/reused", None, false).map(|pr| pr.number),
            Some(46)
        );
    }

    #[test]
    fn test_select_branch_pr_uses_newest_historical_pr_and_ignores_other_branches() {
        let old = branch_pr(30, "MERGED", "feature/reused", "2026-08-01T10:00:00Z", None);
        let latest = branch_pr(31, "CLOSED", "feature/reused", "2026-08-02T10:00:00Z", None);
        let other = branch_pr(99, "OPEN", "feature/other", "2026-08-03T10:00:00Z", None);
        assert_eq!(
            select_branch_pr([old, latest, other], "feature/reused", None, false)
                .map(|pr| pr.number),
            Some(31)
        );
    }

    #[test]
    fn test_select_branch_pr_is_independent_of_input_order() {
        // Candidates are sorted newest-first before selection, so whatever
        // order gh returns cannot change the answer — including when two
        // historical PRs share an `updatedAt` and only the number separates
        // them.
        let a = branch_pr(60, "MERGED", "feature/reused", "2026-08-01T10:00:00Z", None);
        let b = branch_pr(61, "MERGED", "feature/reused", "2026-08-01T10:00:00Z", None);
        let c = branch_pr(59, "MERGED", "feature/reused", "2026-08-01T10:00:00Z", None);
        let ascending = select_branch_pr(
            [c.clone(), a.clone(), b.clone()],
            "feature/reused",
            None,
            false,
        )
        .map(|pr| pr.number);
        let descending = select_branch_pr([b, a, c], "feature/reused", None, false).map(|pr| pr.number);
        assert_eq!(ascending, Some(61));
        assert_eq!(descending, Some(61));
    }

    // ── Side-branch badge fallback ──────────────────────────────
    //
    // The case these cover: an agent working in a workspace branches off,
    // commits, pushes, opens a PR, then checks the worktree back. Strict
    // current-branch association sees no PR, but the workspace visibly
    // produced one. The fallback finds it from the worktree's own HEAD
    // reflog — as a *badge* only. Whether that association may also settle
    // the workspace is decided downstream, from the PR's head branch.

    /// Real `git reflog` HEAD output shape for exactly that sequence.
    const SIDE_BRANCH_REFLOG: &str = "\
7c1a2b3 HEAD@{0}: checkout: moving from appimage-child-env-hygiene to fix-ui-borders
9d4e5f6 HEAD@{1}: commit: chore: sanitize child env
1a2b3c4 HEAD@{2}: checkout: moving from fix-ui-borders to appimage-child-env-hygiene
5e6f7a8 HEAD@{3}: commit: wip: borders
2b3c4d5 HEAD@{4}: checkout: moving from main to fix-ui-borders";

    #[test]
    fn test_reflog_candidates_find_the_side_branch_the_worktree_returned_from() {
        let candidates =
            parse_recent_checkout_branches(SIDE_BRANCH_REFLOG, "fix-ui-borders", Some("main"), 5);
        assert_eq!(candidates, vec!["appimage-child-env-hygiene".to_string()]);
    }

    #[test]
    fn test_reflog_candidates_exclude_the_current_and_default_branches() {
        // The current branch already answered "no PR" authoritatively, and
        // the default branch's PR history is reverse-merge noise rather than
        // this checkout's work — the same reason `select_branch_pr`
        // suppresses historical PRs there.
        let candidates =
            parse_recent_checkout_branches(SIDE_BRANCH_REFLOG, "fix-ui-borders", Some("main"), 5);
        assert!(!candidates.iter().any(|b| b == "fix-ui-borders"));
        assert!(!candidates.iter().any(|b| b == "main"));
    }

    #[test]
    fn test_reflog_candidates_are_newest_first_and_deduped() {
        let reflog = "\
aaa1111 HEAD@{0}: checkout: moving from third to current
bbb2222 HEAD@{1}: checkout: moving from second to third
ccc3333 HEAD@{2}: checkout: moving from third to second
ddd4444 HEAD@{3}: checkout: moving from first to third
eee5555 HEAD@{4}: checkout: moving from current to first";
        let candidates = parse_recent_checkout_branches(reflog, "current", Some("main"), 5);
        assert_eq!(
            candidates,
            vec![
                "third".to_string(),
                "second".to_string(),
                "first".to_string()
            ],
            "each branch appears once, at its most recent position"
        );
    }

    #[test]
    fn test_reflog_candidates_skip_detached_head_shas() {
        // `git checkout <sha>`, `git bisect`, and `gh pr checkout` of a
        // commit all write a raw object id here. Querying gh for a branch by
        // that name can only ever miss.
        let reflog = "\
aaa1111 HEAD@{0}: checkout: moving from 9d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e to current
bbb2222 HEAD@{1}: checkout: moving from side-branch to 9d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e";
        let candidates = parse_recent_checkout_branches(reflog, "current", Some("main"), 5);
        assert_eq!(candidates, vec!["side-branch".to_string()]);
    }

    #[test]
    fn test_reflog_candidates_ignore_non_checkout_records_and_respect_the_cap() {
        // Commits, resets, rebases and merges all live in the same reflog
        // and carry no branch identity. Only `checkout: moving from X to Y`
        // is parsed, and the candidate list stays small so a long reflog
        // can't turn into a long scan.
        let reflog = "\
aaa1111 HEAD@{0}: commit: some work
bbb2222 HEAD@{1}: reset: moving to HEAD~1
ccc3333 HEAD@{2}: rebase (finish): returning to refs/heads/current
ddd4444 HEAD@{3}: checkout: moving from f to current
eee5555 HEAD@{4}: checkout: moving from e to f
fff6666 HEAD@{5}: checkout: moving from d to e
aaa7777 HEAD@{6}: checkout: moving from c to d
bbb8888 HEAD@{7}: checkout: moving from b to c
ccc9999 HEAD@{8}: checkout: moving from a to b";
        let candidates = parse_recent_checkout_branches(reflog, "current", Some("main"), 3);
        assert_eq!(
            candidates,
            vec!["f".to_string(), "e".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn test_reflog_candidates_are_empty_without_checkout_history() {
        // A worktree created and never switched away from has nothing to
        // fall back to, so the fallback never reaches `gh` at all.
        let reflog = "aaa1111 HEAD@{0}: commit (initial): first";
        assert!(parse_recent_checkout_branches(reflog, "current", Some("main"), 5).is_empty());
    }

    #[test]
    fn test_fallback_takes_the_most_recently_checked_out_branch_with_an_open_pr() {
        // Recency decides between candidates that both qualify.
        let prs = vec![
            branch_pr(70, "OPEN", "older", "2026-08-05T10:00:00Z", None),
            branch_pr(71, "OPEN", "newer", "2026-08-04T10:00:00Z", None),
        ];
        let candidates = vec!["newer".to_string(), "older".to_string()];
        let selected = select_first_candidate_pr(&prs, &candidates, &|_| None).unwrap();
        assert_eq!(selected.number, 71);
    }

    #[test]
    fn test_fallback_ignores_merged_and_closed_candidate_prs() {
        // The fallback answers "an agent just pushed a PR from a side
        // branch", which is always an open PR. Admitting history would let
        // `gh pr checkout <n>` on somebody else's finished PR badge this
        // workspace with it for the whole reflog window — so `newer`'s
        // merged PR is skipped and `older`'s open one wins despite being
        // less recent, and a candidate with only history contributes
        // nothing at all.
        let prs = vec![
            branch_pr(70, "OPEN", "older", "2026-08-05T10:00:00Z", None),
            branch_pr(71, "MERGED", "newer", "2026-08-04T10:00:00Z", None),
            branch_pr(72, "CLOSED", "abandoned", "2026-08-06T10:00:00Z", None),
        ];
        let selected = select_first_candidate_pr(
            &prs,
            &[
                "abandoned".to_string(),
                "newer".to_string(),
                "older".to_string(),
            ],
            &|_| None,
        )
        .unwrap();
        assert_eq!(selected.number, 70);

        assert!(
            select_first_candidate_pr(
                &prs,
                &["newer".to_string(), "abandoned".to_string()],
                &|_| None
            )
            .is_none(),
            "a workspace whose recent branches only have finished PRs gets no badge"
        );
    }

    #[test]
    fn test_fallback_resolves_the_owner_only_for_candidates_that_have_a_pr() {
        // Owner resolution costs `git config` reads, and the common case is
        // a handful of recently-visited branches with no open PR at all.
        // Resolving eagerly inside the scan spent those subprocesses on
        // every 5s active-workspace tick for nothing.
        let prs = vec![branch_pr(73, "OPEN", "has-pr", "2026-08-05T10:00:00Z", None)];
        let asked = std::cell::RefCell::new(Vec::new());
        let selected = select_first_candidate_pr(
            &prs,
            &["no-pr-a".to_string(), "no-pr-b".to_string(), "has-pr".to_string()],
            &|branch| {
                asked.borrow_mut().push(branch.to_string());
                None
            },
        )
        .unwrap();
        assert_eq!(selected.number, 73);
        assert_eq!(
            asked.into_inner(),
            vec!["has-pr".to_string()],
            "only the candidate with a matching open PR costs a git config read"
        );
    }

    #[test]
    fn test_memoize_ok_reuses_a_fresh_entry_and_never_caches_an_error() {
        // The seam that keeps the fallback off the 5s sweep: within the TTL
        // a repeat lookup must not re-run the closure (which is where every
        // `git`/`gh` subprocess lives).
        let cache: Mutex<HashMap<String, (Instant, u32)>> = Mutex::new(HashMap::new());
        let calls = std::cell::Cell::new(0u32);
        let ttl = Duration::from_secs(60);
        let mut compute = || -> Result<u32, String> {
            calls.set(calls.get() + 1);
            Ok(calls.get())
        };

        assert_eq!(memoize_ok(&cache, "k".to_string(), ttl, &mut compute), Ok(1));
        assert_eq!(memoize_ok(&cache, "k".to_string(), ttl, &mut compute), Ok(1));
        assert_eq!(memoize_ok(&cache, "k".to_string(), ttl, &mut compute), Ok(1));
        assert_eq!(calls.get(), 1, "one compute per key per TTL window");

        // A different key is a different question — here, another worktree
        // or the same worktree on a new branch — and is answered at once.
        assert_eq!(memoize_ok(&cache, "other".to_string(), ttl, &mut compute), Ok(2));

        // A zero TTL expires immediately, so the entry is not reused.
        assert_eq!(
            memoize_ok(&cache, "k".to_string(), Duration::ZERO, &mut compute),
            Ok(3)
        );

        // Errors are transient (dropped network, expired token) and must be
        // retried rather than pinned for a minute.
        let failing = |_: ()| -> Result<u32, String> { Err("gh failed".to_string()) };
        assert!(memoize_ok(&cache, "err".to_string(), ttl, || failing(())).is_err());
        assert_eq!(memoize_ok(&cache, "err".to_string(), ttl, &mut compute), Ok(4));
    }

    #[test]
    fn test_fallback_skips_candidates_without_a_pr() {
        let prs = vec![branch_pr(72, "OPEN", "has-pr", "2026-08-05T10:00:00Z", None)];
        let candidates = vec!["no-pr".to_string(), "has-pr".to_string()];
        let selected = select_first_candidate_pr(&prs, &candidates, &|_| None).unwrap();
        assert_eq!(selected.number, 72);
        assert_eq!(selected.head_branch.as_deref(), Some("has-pr"));
    }

    #[test]
    fn test_fallback_applies_the_open_over_history_policy_within_one_candidate() {
        // A reused branch name resolves to its open PR, not the merged one
        // that happens to be more recently updated.
        let prs = vec![
            branch_pr(80, "MERGED", "reused", "2026-08-05T10:00:00Z", None),
            branch_pr(81, "OPEN", "reused", "2026-08-01T10:00:00Z", None),
        ];
        let selected =
            select_first_candidate_pr(&prs, &["reused".to_string()], &|_| None).unwrap();
        assert_eq!(selected.number, 81);
    }

    #[test]
    fn test_fallback_honours_fork_owner_scoping() {
        // Same client-side fork disambiguation as the current-branch path:
        // an upstream PR that merely shares a branch name must not attach to
        // a fork-tracking candidate.
        let prs = vec![branch_pr_owned_by(
            90,
            "OPEN",
            "shared-name",
            "2026-08-05T10:00:00Z",
            Some("upstream"),
        )];
        let candidates = vec!["shared-name".to_string()];
        assert!(
            select_first_candidate_pr(&prs, &candidates, &|_| Some("contributor".to_string()))
                .is_none()
        );
        assert!(
            select_first_candidate_pr(&prs, &candidates, &|_| Some("upstream".to_string()))
                .is_some()
        );
    }

    #[test]
    fn test_default_branch_checkout_is_recognised_so_the_fallback_can_refuse_it() {
        // A repo-root workspace sitting on the default branch checks out
        // every branch in the repo over time; "recently checked out" there
        // means ordinary branch hopping, not this checkout's own work.
        let on_default = BranchPrLookup {
            branch: "main".to_string(),
            default_branch: Some("main".to_string()),
            pr: None,
        };
        assert!(on_default.is_default_branch());

        let on_feature = BranchPrLookup {
            branch: "fix-ui-borders".to_string(),
            default_branch: Some("main".to_string()),
            pr: None,
        };
        assert!(!on_feature.is_default_branch());
    }

    #[test]
    fn test_a_fallback_association_carries_its_own_head_branch() {
        // This is what lets settlement tell the two association kinds
        // apart: the badge is stored with the PR's head branch, which for a
        // side-branch association is not the workspace's checked-out branch.
        let prs = vec![branch_pr(
            250,
            "OPEN",
            "appimage-child-env-hygiene",
            "2026-08-05T10:00:00Z",
            None,
        )];
        let candidates =
            parse_recent_checkout_branches(SIDE_BRANCH_REFLOG, "fix-ui-borders", Some("main"), 5);
        let selected = select_first_candidate_pr(&prs, &candidates, &|_| None).unwrap();
        assert_eq!(selected.number, 250);
        assert_ne!(selected.head_branch.as_deref(), Some("fix-ui-borders"));
    }

    #[test]
    fn test_gh_exit_result_maps_a_non_zero_exit_to_err() {
        // Load-bearing: the pollers preserve a stored PR badge on `Err` and
        // clear it on a successful empty list. A failed `gh` must never look
        // like an empty success.
        let failed = gh_exit_result(
            "pr",
            false,
            "[]".to_string(),
            "gh: To use GitHub CLI in a GitHub Actions workflow, set the GH_TOKEN environment variable.\n",
        );
        assert!(failed.is_err());
        assert!(failed.unwrap_err().starts_with("gh pr failed:"));

        let succeeded = gh_exit_result("pr", true, "[]\n".to_string(), "");
        assert_eq!(succeeded, Ok("[]".to_string()));
    }

    #[test]
    fn test_branch_pr_outcome_matrix() {
        // match → write, successful empty → clear, error → preserve.
        let pr = branch_pr(70, "MERGED", "feature/popup", "2026-08-01T10:00:00Z", None);
        assert!(matches!(
            branch_pr_outcome(Ok(Some(pr))),
            BranchPrOutcome::Write(written) if written.number == 70
        ));
        assert!(matches!(
            branch_pr_outcome(Ok(None)),
            BranchPrOutcome::Clear
        ));
        assert!(matches!(
            branch_pr_outcome(Err("gh pr failed: network unreachable".to_string())),
            BranchPrOutcome::Preserve
        ));
    }

    #[test]
    fn test_parse_pr_info_carries_author_when_present() {
        // Stage 5 — `gh pr view --json author` emits `author.login`;
        // the parser must thread it through so the chip header /
        // tooltip can show "by alice".
        let json = serde_json::json!({
            "number": 7,
            "url": "https://github.com/u/r/pull/7",
            "state": "OPEN",
            "title": "Tweak",
            "author": {"login": "alice"},
        });
        let pr = parse_pr_json(&json);
        assert_eq!(pr.author.as_deref(), Some("alice"));
    }

    #[test]
    fn test_parse_pr_info_treats_empty_author_login_as_none() {
        // Edge: gh JSON sometimes emits `{"author": {"login": ""}}`
        // for ghost users / deleted accounts. Empty string → None
        // so the chip doesn't render "by " with an empty trailer.
        let json = serde_json::json!({
            "number": 1,
            "url": "https://example",
            "state": "OPEN",
            "title": "T",
            "author": {"login": ""},
        });
        let pr = parse_pr_json(&json);
        assert!(pr.author.is_none());
    }

    #[test]
    fn test_pr_detail_serialization_round_trip() {
        // The Stage 5 detail fields must serialize with the
        // camelCase shape the TS layer expects (`totalComments`).
        let pr = PullRequestInfo {
            number: 42,
            url: "https://github.com/u/r/pull/42".into(),
            state: "OPEN".into(),
            title: "Add dark mode".into(),
            head_branch: Some("feat/dark".into()),
            base_branch: Some("main".into()),
            is_draft: false,
            mergeable: Some("MERGEABLE".into()),
            additions: Some(100),
            deletions: Some(5),
            review_decision: Some("APPROVED".into()),
            checks_passing: None,
            updated_at: Some("2026-04-27T00:00:00Z".into()),
            created_at: Some("2026-04-26T00:00:00Z".into()),
            head_ref_oid: Some("deadbeef".into()),
            head_repository_owner: Some("zeus".into()),
            body: Some("PR body here".into()),
            comments: vec![IssueComment {
                author: "alice".into(),
                body: "ship it".into(),
                created_at: "2026-04-27T01:00:00Z".into(),
            }],
            total_comments: 1,
            author: Some("zeus".into()),
            merge_state_status: Some("CLEAN".into()),
            changed_files: Some(3),
            merged_by: None,
            merged_at: None,
            review_requests: vec!["juliusm".into()],
            latest_reviews: vec![PrReviewSummary {
                author: "alice".into(),
                state: "APPROVED".into(),
            }],
        };
        let json = serde_json::to_string(&pr).unwrap();
        assert!(json.contains("\"totalComments\":1"));
        assert!(json.contains("\"createdAt\":"));
        let back: PullRequestInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.body.as_deref(), Some("PR body here"));
        assert_eq!(back.comments.len(), 1);
        assert_eq!(back.total_comments, 1);
        assert_eq!(back.author.as_deref(), Some("zeus"));
    }

    #[test]
    fn test_parse_pr_info_minimal_fields() {
        let json = r#"{
            "number": 10,
            "url": "https://github.com/user/repo/pull/10",
            "state": "OPEN",
            "title": "Fix bug"
        }"#;
        let pr: PullRequestInfo = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 10);
        assert!(pr.head_branch.is_none());
        assert!(pr.additions.is_none());
    }

    #[test]
    fn test_parse_pr_list() {
        let json = r#"[
            {"number": 1, "url": "https://github.com/u/r/pull/1", "state": "OPEN", "title": "Feature A"},
            {"number": 2, "url": "https://github.com/u/r/pull/2", "state": "MERGED", "title": "Feature B"}
        ]"#;
        let prs: Vec<PullRequestInfo> = serde_json::from_str(json).unwrap();
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].state, "OPEN");
        assert_eq!(prs[1].state, "MERGED");
    }

    #[test]
    fn test_parse_checks() {
        let json = r#"[
            {"name": "build", "state": "SUCCESS", "conclusion": "SUCCESS", "elapsedTime": "2m30s", "detailUrl": "https://github.com/u/r/actions/1", "startedAt": "2026-01-01T00:00:00Z", "completedAt": "2026-01-01T00:02:30Z"},
            {"name": "lint", "state": "FAILURE", "conclusion": "FAILURE"},
            {"name": "deploy", "state": "PENDING", "conclusion": null}
        ]"#;
        let checks: Vec<CheckInfo> = serde_json::from_str(json).unwrap();
        assert_eq!(checks.len(), 3);
        assert_eq!(checks[0].name, "build");
        assert_eq!(checks[0].conclusion.as_deref(), Some("SUCCESS"));
        assert_eq!(checks[0].elapsed_time.as_deref(), Some("2m30s"));
        assert_eq!(checks[0].detail_url.as_deref(), Some("https://github.com/u/r/actions/1"));
        assert!(checks[1].elapsed_time.is_none());
        assert!(checks[2].conclusion.is_none());
    }

    #[test]
    fn test_parse_inline_review_comment() {
        let json = r#"[
            {
                "id": 100,
                "user": {"login": "reviewer1"},
                "body": "This looks wrong",
                "path": "src/main.rs",
                "line": 42,
                "created_at": "2026-01-15T10:00:00Z",
                "in_reply_to_id": null,
                "pull_request_review_id": 200
            }
        ]"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let arr = v.as_array().unwrap();
        let c = &arr[0];
        let comment = InlineReviewComment {
            id: c["id"].as_u64().unwrap_or(0),
            author: c["user"]["login"].as_str().unwrap_or("").to_string(),
            body: c["body"].as_str().unwrap_or("").to_string(),
            path: c["path"].as_str().unwrap_or("").to_string(),
            line: c["line"].as_u64().map(|n| n as u32),
            created_at: c["created_at"].as_str().unwrap_or("").to_string(),
            in_reply_to_id: c["in_reply_to_id"].as_u64(),
            pull_request_review_id: c["pull_request_review_id"].as_u64(),
        };
        assert_eq!(comment.author, "reviewer1");
        assert_eq!(comment.path, "src/main.rs");
        assert_eq!(comment.line, Some(42));
        assert_eq!(comment.pull_request_review_id, Some(200));
    }

    #[test]
    fn test_parse_pr_merged_state() {
        let json = r#"{
            "number": 5,
            "url": "https://github.com/u/r/pull/5",
            "state": "MERGED",
            "title": "Done"
        }"#;
        let pr: PullRequestInfo = serde_json::from_str(json).unwrap();
        assert_eq!(pr.state, "MERGED");
    }

    #[test]
    fn test_parse_pr_with_checks_rollup() {
        let json = r#"{
            "number": 7,
            "url": "https://github.com/u/r/pull/7",
            "state": "OPEN",
            "title": "Test",
            "statusCheckRollup": [
                {"name": "CI", "state": "SUCCESS", "conclusion": "SUCCESS"}
            ]
        }"#;
        let pr: PullRequestInfo = serde_json::from_str(json).unwrap();
        assert_eq!(pr.number, 7);
    }

    // ── Issue tests ──

    #[test]
    fn test_parse_issue_json() {
        let json = r#"{
            "number": 92,
            "title": "Backend endpoints voor prospectielijst",
            "state": "OPEN",
            "url": "https://github.com/user/repo/issues/92",
            "labels": [{"name": "enhancement"}, {"name": "backend"}],
            "assignees": [{"login": "zeus"}]
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let issue = parse_issue_json(&v);
        assert_eq!(issue.number, 92);
        assert_eq!(issue.title, "Backend endpoints voor prospectielijst");
        assert_eq!(issue.state, IssueState::Open);
        assert_eq!(issue.labels, vec!["enhancement", "backend"]);
        assert_eq!(issue.assignees, vec!["zeus"]);
        assert!(issue.body.is_none());
    }

    #[test]
    fn test_parse_issue_closed() {
        let json = r#"{
            "number": 10,
            "title": "Fix login",
            "state": "CLOSED",
            "url": "https://github.com/u/r/issues/10",
            "labels": [],
            "assignees": []
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let issue = parse_issue_json(&v);
        assert_eq!(issue.state, IssueState::Closed);
    }

    #[test]
    fn test_parse_issue_minimal() {
        let json = r#"{
            "number": 1,
            "title": "Bug",
            "state": "OPEN",
            "url": "https://github.com/u/r/issues/1"
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let issue = parse_issue_json(&v);
        assert_eq!(issue.number, 1);
        assert!(issue.labels.is_empty());
        assert!(issue.assignees.is_empty());
    }

    #[test]
    fn test_parse_issue_list() {
        let json = r#"[
            {"number": 1, "url": "https://github.com/u/r/issues/1", "state": "OPEN", "title": "A", "labels": [], "assignees": []},
            {"number": 2, "url": "https://github.com/u/r/issues/2", "state": "CLOSED", "title": "B", "labels": [{"name": "bug"}], "assignees": []}
        ]"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let issues: Vec<GitHubIssue> = v.as_array().unwrap().iter().map(parse_issue_json).collect();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].state, IssueState::Open);
        assert_eq!(issues[1].state, IssueState::Closed);
        assert_eq!(issues[1].labels, vec!["bug"]);
    }

    #[test]
    fn test_issue_serialization_roundtrip() {
        let issue = GitHubIssue {
            number: 42,
            title: "Test issue".into(),
            state: IssueState::Open,
            labels: vec!["bug".into()],
            assignees: vec!["user1".into()],
            url: "https://github.com/u/r/issues/42".into(),
            body: Some("Issue body".into()),
            comments: Vec::new(),
            total_comments: 0,
            updated_at: None,
        };
        let json = serde_json::to_string(&issue).unwrap();
        let deserialized: GitHubIssue = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.number, 42);
        assert_eq!(deserialized.state, IssueState::Open);
        assert_eq!(deserialized.body.as_deref(), Some("Issue body"));
    }

    #[test]
    fn test_linked_issue_serialization_roundtrip() {
        let linked = LinkedIssue {
            number: 99,
            title: "Feature request".into(),
            state: IssueState::Closed,
            labels: vec!["feature".into(), "ui".into()],
        };
        let json = serde_json::to_string(&linked).unwrap();
        let deserialized: LinkedIssue = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.number, 99);
        assert_eq!(deserialized.state, IssueState::Closed);
        assert_eq!(deserialized.labels, vec!["feature", "ui"]);
    }

    #[test]
    fn test_suggest_branch_name_basic() {
        assert_eq!(
            suggest_branch_name(92, "Backend endpoints voor prospectielijst"),
            "feature/92-backend-endpoints-voor-prospectielijst"
        );
    }

    #[test]
    fn test_suggest_branch_name_special_chars() {
        assert_eq!(
            suggest_branch_name(5, "Fix: login page (500 error) & redirect"),
            "feature/5-fix-login-page-500-error-redirect"
        );
    }

    #[test]
    fn test_suggest_branch_name_unicode() {
        // Unicode alphanumeric chars are preserved by is_alphanumeric()
        assert_eq!(
            suggest_branch_name(10, "Über die Straße gehen"),
            "feature/10-über-die-straße-gehen"
        );
    }

    #[test]
    fn test_suggest_branch_name_long_multibyte_title_does_not_panic() {
        // 'a' (1 byte) + 20×'技' (3 bytes) = 61-byte slug with no hyphens, so
        // the truncation runs and byte 60 lands mid-character. Slicing there
        // by raw byte index would panic; the cut must back off to a boundary.
        let title = format!("a{}", "技".repeat(20));
        let name = suggest_branch_name(1, &title);
        assert!(name.starts_with("feature/1-a"), "got: {name}");
        assert!(name.contains('技'));
    }

    #[test]
    fn test_suggest_branch_name_long_title() {
        let long_title = "This is a very long issue title that should be truncated to keep the branch name reasonable and not exceed filesystem limits";
        let result = suggest_branch_name(123, long_title);
        assert!(result.starts_with("feature/123-"));
        // Title portion should be at most ~60 chars
        let title_part = result.strip_prefix("feature/123-").unwrap();
        assert!(title_part.len() <= 60, "Title portion too long: {}", title_part);
        // Should break at word boundary
        assert!(!result.ends_with('-'));
    }

    #[test]
    fn test_suggest_branch_name_consecutive_special_chars() {
        assert_eq!(
            suggest_branch_name(1, "fix---multiple   spaces...and!!!dots"),
            "feature/1-fix-multiple-spaces-and-dots"
        );
    }

    #[test]
    fn test_issue_state_display() {
        assert_eq!(IssueState::Open.to_string(), "open");
        assert_eq!(IssueState::Closed.to_string(), "closed");
    }

    #[test]
    fn test_issue_state_from_str() {
        assert_eq!(IssueState::from_str("OPEN"), IssueState::Open);
        assert_eq!(IssueState::from_str("open"), IssueState::Open);
        assert_eq!(IssueState::from_str("CLOSED"), IssueState::Closed);
        assert_eq!(IssueState::from_str("closed"), IssueState::Closed);
        assert_eq!(IssueState::from_str("unknown"), IssueState::Open);
    }

    #[test]
    fn test_issue_body_truncation_respects_char_boundaries() {
        // Build a body that exceeds the limit with a multi-byte char at the boundary
        let body_content = "a".repeat(50 * 1024) + "é"; // 'é' is 2 bytes, pushes past 50KB
        assert!(body_content.len() > MAX_ISSUE_BODY_BYTES);

        let json = serde_json::json!({
            "number": 1,
            "title": "Test",
            "state": "OPEN",
            "url": "https://github.com/u/r/issues/1",
            "body": body_content,
            "labels": [],
            "assignees": []
        });

        // Simulate what get_github_issue does for truncation
        let body = json["body"].as_str().unwrap();
        let truncated = if body.len() > MAX_ISSUE_BODY_BYTES {
            let mut end = MAX_ISSUE_BODY_BYTES;
            while end > 0 && !body.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…\n\n[Body truncated at 50KB]", &body[..end])
        } else {
            body.to_string()
        };
        // Must not panic and must be valid UTF-8
        assert!(truncated.len() > 0);
        assert!(truncated.ends_with("[Body truncated at 50KB]"));
    }

    #[test]
    fn test_issue_state_serde_json_format() {
        // Verify that IssueState serializes as simple strings matching the TypeScript type
        let open = serde_json::to_string(&IssueState::Open).unwrap();
        assert_eq!(open, "\"Open\"");
        let closed = serde_json::to_string(&IssueState::Closed).unwrap();
        assert_eq!(closed, "\"Closed\"");
        // And deserializes back
        let parsed: IssueState = serde_json::from_str("\"Open\"").unwrap();
        assert_eq!(parsed, IssueState::Open);
    }

    #[test]
    fn test_linked_issue_defaults_for_missing_fields() {
        // Simulate deserializing persisted data that has no linked_issue field
        let json = r#"{"number": 1, "title": "T", "state": "Open"}"#;
        let linked: LinkedIssue = serde_json::from_str(json).unwrap();
        assert_eq!(linked.number, 1);
        assert!(linked.labels.is_empty()); // default empty vec
    }

    #[test]
    fn test_parse_issue_comments_basic() {
        let v: serde_json::Value = serde_json::from_str(r#"[
            {"author": {"login": "alice"}, "body": "first", "createdAt": "2026-01-01T00:00:00Z"},
            {"author": {"login": "bob"}, "body": "second", "createdAt": "2026-01-02T00:00:00Z"}
        ]"#).unwrap();
        let (comments, total) = parse_issue_comments(&v);
        assert_eq!(total, 2);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].author, "alice");
        assert_eq!(comments[0].body, "first");
        assert_eq!(comments[0].created_at, "2026-01-01T00:00:00Z");
        assert_eq!(comments[1].author, "bob");
    }

    #[test]
    fn test_parse_issue_comments_truncates_at_max() {
        // Build 25 comments — total should be 25, returned slice 20.
        let mut entries: Vec<serde_json::Value> = Vec::new();
        for i in 0..25 {
            entries.push(serde_json::json!({
                "author": {"login": format!("u{i}")},
                "body": format!("c{i}"),
                "createdAt": "2026-01-01T00:00:00Z"
            }));
        }
        let v = serde_json::Value::Array(entries);
        let (comments, total) = parse_issue_comments(&v);
        assert_eq!(total, 25);
        assert_eq!(comments.len(), MAX_ISSUE_COMMENTS);
        assert_eq!(comments[0].author, "u0");
        assert_eq!(comments[19].author, "u19");
    }

    #[test]
    fn test_parse_issue_comments_empty_or_missing() {
        let v = serde_json::Value::Null;
        let (comments, total) = parse_issue_comments(&v);
        assert!(comments.is_empty());
        assert_eq!(total, 0);

        let v = serde_json::json!([]);
        let (comments, total) = parse_issue_comments(&v);
        assert!(comments.is_empty());
        assert_eq!(total, 0);
    }

    #[test]
    fn test_parse_issue_json_with_updated_at() {
        let json = r#"{
            "number": 5,
            "title": "Test",
            "state": "OPEN",
            "url": "https://github.com/u/r/issues/5",
            "labels": [],
            "assignees": [],
            "updatedAt": "2026-04-01T12:00:00Z"
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let issue = parse_issue_json(&v);
        assert_eq!(issue.updated_at.as_deref(), Some("2026-04-01T12:00:00Z"));
        assert!(issue.comments.is_empty());
        assert_eq!(issue.total_comments, 0);
    }

    #[test]
    fn test_issue_comment_serializes_with_camelcase_created_at() {
        let comment = IssueComment {
            author: "alice".into(),
            body: "hello".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&comment).unwrap();
        // Must be camelCase to match the TS type.
        assert!(json.contains("\"createdAt\""));
        assert!(!json.contains("\"created_at\""));
        // And round-trips back.
        let parsed: IssueComment = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, comment);
    }

    #[test]
    fn test_parse_incoming_pr_json_full_fields() {
        let json: serde_json::Value = serde_json::from_str(r#"{
            "number": 27,
            "title": "Add user settings page",
            "author": {"login": "alice"},
            "headRefName": "feat/settings",
            "isDraft": false,
            "updatedAt": "2026-04-08T12:00:00Z",
            "additions": 110,
            "deletions": 26,
            "reviewDecision": "APPROVED",
            "statusCheckRollup": [
                {"name": "CI", "state": "SUCCESS", "conclusion": "SUCCESS"}
            ],
            "url": "https://github.com/test/repo/pull/27"
        }"#).unwrap();

        let item = parse_incoming_pr_json(&json);
        assert_eq!(item.number, 27);
        assert_eq!(item.title, "Add user settings page");
        assert_eq!(item.author, "alice");
        assert_eq!(item.head_branch.as_deref(), Some("feat/settings"));
        assert!(!item.is_draft);
        assert_eq!(item.additions, Some(110));
        assert_eq!(item.deletions, Some(26));
        assert_eq!(item.review_decision.as_deref(), Some("APPROVED"));
        assert_eq!(item.checks_status.as_deref(), Some("success"));
        assert_eq!(item.updated_at.as_deref(), Some("2026-04-08T12:00:00Z"));
        assert_eq!(item.url, "https://github.com/test/repo/pull/27");
    }

    #[test]
    fn test_parse_incoming_pr_json_minimal_fields() {
        let json: serde_json::Value = serde_json::from_str(r#"{
            "number": 1,
            "title": "Fix bug",
            "author": {"login": "bob"},
            "url": ""
        }"#).unwrap();

        let item = parse_incoming_pr_json(&json);
        assert_eq!(item.number, 1);
        assert_eq!(item.author, "bob");
        assert_eq!(item.head_branch, None);
        assert!(!item.is_draft);
        assert_eq!(item.additions, None);
        assert_eq!(item.deletions, None);
        assert_eq!(item.review_decision, None);
        assert_eq!(item.checks_status, None);
    }

    #[test]
    fn test_summarize_checks_status_all_success() {
        let v: serde_json::Value = serde_json::from_str(r#"[
            {"state": "SUCCESS", "conclusion": "SUCCESS"},
            {"state": "COMPLETED", "conclusion": "NEUTRAL"}
        ]"#).unwrap();
        assert_eq!(summarize_checks_status(&v).as_deref(), Some("success"));
    }

    #[test]
    fn test_summarize_checks_status_failure() {
        let v: serde_json::Value = serde_json::from_str(r#"[
            {"state": "SUCCESS", "conclusion": "SUCCESS"},
            {"state": "COMPLETED", "conclusion": "FAILURE"}
        ]"#).unwrap();
        assert_eq!(summarize_checks_status(&v).as_deref(), Some("failure"));
    }

    #[test]
    fn test_summarize_checks_status_pending() {
        let v: serde_json::Value = serde_json::from_str(r#"[
            {"state": "SUCCESS", "conclusion": "SUCCESS"},
            {"state": "PENDING", "conclusion": ""}
        ]"#).unwrap();
        assert_eq!(summarize_checks_status(&v).as_deref(), Some("pending"));
    }

    #[test]
    fn test_summarize_checks_status_empty() {
        let v: serde_json::Value = serde_json::from_str("[]").unwrap();
        assert_eq!(summarize_checks_status(&v), None);
    }

    #[test]
    fn test_summarize_checks_status_null() {
        let v = serde_json::Value::Null;
        assert_eq!(summarize_checks_status(&v), None);
    }

    // ── Timeline mapping ──

    /// A recorded slice of `GET /issues/{n}/timeline`, including an event
    /// type this build has never heard of.
    fn timeline_fixture() -> Vec<serde_json::Value> {
        serde_json::from_str(
            r#"[
              {"event":"commented","id":1001,"user":{"login":"juliusm"},
               "created_at":"2026-08-16T09:00:00Z","body":"Worth a line here saying so."},
              {"event":"reviewed","id":1002,"user":{"login":"juliusm"},"state":"changes_requested",
               "submitted_at":"2026-08-16T09:05:00Z","body":"The discoverability leg is a follow-up."},
              {"event":"committed","sha":"a1f9c2e5d","author":{"name":"Zeus-Deus","date":"2026-08-16T09:30:00Z"},
               "message":"fix: address review\n\nlonger body that must not appear"},
              {"event":"head_ref_force_pushed","id":1004,"actor":{"login":"Zeus-Deus"},
               "created_at":"2026-08-16T09:45:00Z","commit_id":"bb31d70"},
              {"event":"review_requested","id":1005,"actor":{"login":"Zeus-Deus"},
               "created_at":"2026-08-16T09:50:00Z","requested_reviewer":{"login":"octocat"}},
              {"event":"renamed","id":1006,"actor":{"login":"Zeus-Deus"},"created_at":"2026-08-16T09:55:00Z",
               "rename":{"from":"wip: thing","to":"feat: thing"}},
              {"event":"merged","id":1007,"actor":{"login":"Zeus-Deus"},"created_at":"2026-08-16T10:00:00Z",
               "commit_id":"ff00aa1"},
              {"event":"automatic_base_change_succeeded","id":1008,"actor":{"login":"Zeus-Deus"},
               "created_at":"2026-08-16T10:05:00Z"},
              {"event":"commented","id":1009,"user":{"login":"ghost"},
               "created_at":"2026-08-16T10:06:00Z","body":"   "}
            ]"#,
        )
        .expect("fixture must parse")
    }

    #[test]
    fn every_event_maps_to_a_renderable_variant() {
        let events = map_github_timeline(&timeline_fixture());
        let kinds: Vec<&PrTimelineEventKind> = events.iter().map(|e| &e.kind).collect();

        assert!(matches!(kinds[0], PrTimelineEventKind::Commented { body } if body == "Worth a line here saying so."));
        assert!(
            matches!(kinds[1], PrTimelineEventKind::Reviewed { verdict, .. } if verdict == "CHANGES_REQUESTED"),
            "the verdict is upper-cased so the UI has one vocabulary"
        );
        // Only the subject line: a commit body in a rail entry pushes
        // everything after it off the screen.
        assert!(
            matches!(kinds[2], PrTimelineEventKind::Committed { sha, message }
                if sha == "a1f9c2e5d" && message == "fix: address review"),
        );
        assert!(matches!(kinds[3], PrTimelineEventKind::HeadRefForcePushed { sha } if sha.as_deref() == Some("bb31d70")));
        assert!(matches!(kinds[4], PrTimelineEventKind::ReviewRequested { reviewer } if reviewer.as_deref() == Some("octocat")));
        assert!(matches!(kinds[5], PrTimelineEventKind::Renamed { to, .. } if to == "feat: thing"));
        assert!(matches!(kinds[6], PrTimelineEventKind::Merged { .. }));
    }

    /// The rule that keeps the tab honest as GitHub grows event types:
    /// never dropped, never a parse failure, always a readable label.
    #[test]
    fn an_unknown_event_survives_as_a_labelled_one_liner() {
        let events = map_github_timeline(&timeline_fixture());
        let unknown = events
            .iter()
            .find(|e| matches!(e.kind, PrTimelineEventKind::Other { .. }))
            .expect("the unknown event must not be dropped");
        let PrTimelineEventKind::Other { label } = &unknown.kind else {
            unreachable!()
        };
        assert_eq!(label, "automatic base change succeeded");
        assert_eq!(unknown.actor.as_deref(), Some("Zeus-Deus"));
    }

    /// The force-push row is what ties the timeline to re-anchoring, so
    /// it gets its own assertion rather than riding on the sweep above.
    #[test]
    fn the_force_push_event_is_present_and_carries_its_sha() {
        let events = map_github_timeline(&timeline_fixture());
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.kind, PrTimelineEventKind::HeadRefForcePushed { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn an_emptied_comment_is_not_history() {
        let events = map_github_timeline(&timeline_fixture());
        assert!(
            !events.iter().any(|e| e.actor.as_deref() == Some("ghost")),
            "a whitespace-only comment is a deletion artefact"
        );
    }

    #[test]
    fn event_ids_are_unique_even_when_the_host_reuses_one() {
        // Two rows sharing an id would collapse into one React key and
        // one of them would silently stop rendering.
        let rows: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"event":"closed","id":7},{"event":"reopened","id":7}]"#,
        )
        .unwrap();
        let events = map_github_timeline(&rows);
        assert_eq!(events.len(), 2);
        assert_ne!(events[0].id, events[1].id);
    }

    /// `--paginate` concatenates one array per page; a naive parse would
    /// keep page one and drop the rest.
    #[test]
    fn paginated_pages_are_flattened_rather_than_truncated() {
        let joined = r#"[{"event":"closed","id":1}][{"event":"reopened","id":2}]"#;
        assert_eq!(parse_paginated_array(joined).len(), 2);
    }

    // gh moved `auth status` output from stderr to stdout across major
    // versions; the viewer login must parse from either stream, and a
    // missing login must yield None rather than an empty-string viewer
    // (an empty viewer silently files every PR under Watching).
    #[test]
    fn auth_status_username_parses_modern_stdout_shape() {
        let out = "github.com\n  \u{2713} Logged in to github.com account Zeus-Deus (keyring)\n  - Active account: true\n";
        assert_eq!(
            parse_auth_status_username(out).as_deref(),
            Some("Zeus-Deus")
        );
    }

    #[test]
    fn auth_status_username_parses_legacy_stderr_shape() {
        let err = "Logged in to github.com account octocat (oauth_token)\n";
        assert_eq!(parse_auth_status_username(err).as_deref(), Some("octocat"));
    }

    #[test]
    fn auth_status_username_ignores_unrelated_account_mentions() {
        assert_eq!(
            parse_auth_status_username("error: account suspended, contact support\n"),
            None
        );
        assert_eq!(parse_auth_status_username(""), None);
    }

    // ── The fast/slow split ──────────────────────────────────────────
    //
    // The listing's whole value is what it does *not* ask for, and that
    // is invisible at the call site — it looks like a comma-separated
    // string either way. These tests are the thing that notices when a
    // field quietly moves back across the line.

    #[test]
    fn overview_listing_omits_the_expensive_fields() {
        assert!(
            !OVERVIEW_FAST_FIELDS.contains("statusCheckRollup"),
            "the rollup is per-row work the host does on demand; it belongs in the stats call"
        );
        assert!(!OVERVIEW_FAST_FIELDS.contains("additions"));
        assert!(!OVERVIEW_FAST_FIELDS.contains("deletions"));
    }

    #[test]
    fn overview_listing_keeps_the_fields_grouping_needs() {
        // Triage is the reason the page exists, so "Needs your review"
        // has to be right in the first paint — which means the author
        // and the requested reviewers ride the fast call, not the slow
        // one.
        for field in ["number", "title", "author", "reviewRequests", "url"] {
            assert!(
                OVERVIEW_FAST_FIELDS.contains(field),
                "the listing must carry {field}"
            );
        }
    }

    #[test]
    fn overview_stats_asks_for_exactly_what_the_listing_refused() {
        assert!(OVERVIEW_STATS_FIELDS.contains("statusCheckRollup"));
        assert!(OVERVIEW_STATS_FIELDS.contains("additions"));
        assert!(OVERVIEW_STATS_FIELDS.contains("deletions"));
        // Keyed by number so the page can merge it into rows it has
        // already drawn.
        assert!(OVERVIEW_STATS_FIELDS.contains("number"));
    }

    #[test]
    fn overview_item_leaves_the_unasked_fields_unanswered() {
        let json = serde_json::json!({
            "number": 285,
            "title": "Fix the installer",
            "author": {"login": "juliusm"},
            "headRefName": "fix/installer",
            "isDraft": false,
            "updatedAt": "2026-08-16T10:00:00Z",
            "reviewDecision": "APPROVED",
            "url": "https://github.com/u/r/pull/285",
            "reviewRequests": [{"login": "mock-dev"}]
        });
        let item = parse_overview_pr_json(&json);

        assert_eq!(item.number, 285);
        assert_eq!(item.author, "juliusm");
        assert_eq!(item.review_requested_from, vec!["mock-dev".to_string()]);
        // None, not "none": nobody has asked yet, which is a different
        // claim from "this pull request runs no checks".
        assert_eq!(item.checks, None);
        assert_eq!(item.additions, None);
        assert_eq!(item.deletions, None);
    }

    #[test]
    fn overview_stats_parses_the_rollup_into_one_word() {
        let json = serde_json::json!([
            {
                "number": 1,
                "additions": 12,
                "deletions": 3,
                "statusCheckRollup": [
                    {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "FAILURE"}
                ]
            },
            {
                "number": 2,
                "additions": 0,
                "deletions": 0,
                "statusCheckRollup": [
                    {"__typename": "CheckRun", "status": "IN_PROGRESS", "conclusion": null}
                ]
            },
            {
                "number": 3,
                "additions": 4,
                "deletions": 4,
                "statusCheckRollup": [
                    {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"}
                ]
            },
            {"number": 4, "additions": null, "deletions": null, "statusCheckRollup": []}
        ]);
        let stats: Vec<PrOverviewStats> = json
            .as_array()
            .unwrap()
            .iter()
            .map(parse_overview_stats_json)
            .collect();

        assert_eq!(stats[0].checks, "failing");
        assert_eq!(stats[0].additions, Some(12));
        assert_eq!(stats[0].deletions, Some(3));
        assert_eq!(stats[1].checks, "pending");
        assert_eq!(stats[2].checks, "passing");
        // An empty rollup is a host that answered: no checks configured.
        assert_eq!(stats[3].checks, "none");
        assert_eq!(stats[3].additions, None);
    }

    #[test]
    fn overview_stats_survives_a_row_with_nothing_in_it() {
        let stats = parse_overview_stats_json(&serde_json::json!({}));
        assert_eq!(stats.number, 0);
        assert_eq!(stats.checks, "none");
        assert_eq!(stats.additions, None);
    }

    // ── Review threads ──

    /// One page of the GraphQL payload, shaped exactly as
    /// `gh api graphql` returns it (a `data` envelope per page).
    fn thread_page(cursor_threads: &str) -> String {
        format!(
            r#"{{"data":{{"repository":{{"pullRequest":{{"reviewThreads":{{"pageInfo":{{"hasNextPage":false,"endCursor":null}},"nodes":[{cursor_threads}]}}}}}}}}}}"#
        )
    }

    const UNRESOLVED_THREAD: &str = r#"{
        "id":"PRRT_kwDOA","isResolved":false,"isOutdated":false,
        "path":"src/stores/draft-store.ts","line":84,
        "comments":{"nodes":[
          {"id":"PRRC_1","databaseId":8001,"author":{"login":"juliusm"},
           "body":"Worth a comment on why this survives a reload.","createdAt":"2026-08-16T09:00:00Z"},
          {"id":"PRRC_2","databaseId":8002,"author":{"login":"mock-dev"},
           "body":"Added one.","createdAt":"2026-08-16T09:10:00Z"}
        ]}}"#;

    const RESOLVED_OUTDATED_THREAD: &str = r#"{
        "id":"PRRT_kwDOB","isResolved":true,"isOutdated":true,
        "path":"src/pty/mod.rs","line":null,
        "comments":{"nodes":[
          {"id":"PRRC_3","databaseId":8003,"author":{"login":"juliusm"},
           "body":"This close is on the wrong path.","createdAt":"2026-08-16T08:00:00Z"}
        ]}}"#;

    #[test]
    fn threads_carry_resolution_state_and_their_comments() {
        let threads = parse_review_threads(&thread_page(UNRESOLVED_THREAD));
        assert_eq!(threads.len(), 1);
        let thread = &threads[0];
        assert_eq!(thread.id, "PRRT_kwDOA");
        assert!(!thread.is_resolved);
        assert!(!thread.is_outdated);
        // Every GitHub thread can be resolved, so the button is never
        // drawn against a host that would refuse it.
        assert!(thread.is_resolvable);
        assert_eq!(thread.path.as_deref(), Some("src/stores/draft-store.ts"));
        assert_eq!(thread.line, Some(84));
        assert_eq!(thread.comments.len(), 2);
        // The REST id rides along: it is what the reply endpoint
        // addresses and what the UI dedupes the flat list against.
        assert_eq!(thread.comments[0].database_id, Some(8001));
        assert_eq!(thread.comments[0].author, "juliusm");
    }

    #[test]
    fn an_outdated_resolved_thread_keeps_both_facts() {
        let threads = parse_review_threads(&thread_page(RESOLVED_OUTDATED_THREAD));
        assert_eq!(threads.len(), 1);
        assert!(threads[0].is_resolved);
        assert!(threads[0].is_outdated);
        // A thread whose lines left the diff has no line, and that is
        // not an error — it is the definition of outdated.
        assert_eq!(threads[0].line, None);
    }

    /// `--paginate` concatenates one *object* per page here (the timeline
    /// case concatenates arrays). Reading only the first would silently
    /// hide every thread past the fiftieth.
    #[test]
    fn every_page_of_threads_is_read() {
        let joined = format!(
            "{}\n{}",
            thread_page(UNRESOLVED_THREAD),
            thread_page(RESOLVED_OUTDATED_THREAD)
        );
        let threads = parse_review_threads(&joined);
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].id, "PRRT_kwDOA");
        assert_eq!(threads[1].id, "PRRT_kwDOB");
    }

    /// A thread with nothing readable in it would render as an empty
    /// card with a Resolve button — worse than not rendering.
    #[test]
    fn a_thread_with_no_readable_comment_is_dropped() {
        let empty = r#"{"id":"PRRT_kwDOC","isResolved":false,"isOutdated":false,
            "path":null,"line":null,"comments":{"nodes":[
            {"id":"PRRC_9","databaseId":9,"author":{"login":"ghost"},"body":"","createdAt":""}]}}"#;
        assert!(parse_review_threads(&thread_page(empty)).is_empty());
    }

    /// A payload that is not the shape we expect yields no threads
    /// rather than a panic: the surface then keeps its last good data.
    #[test]
    fn an_unexpected_payload_yields_no_threads() {
        assert!(parse_review_threads("{}").is_empty());
        assert!(parse_review_threads("not json at all").is_empty());
    }

    /// `--paginate` follows the thread list, not each thread's comments,
    /// so a thread past the comment page size comes back cut off. Which
    /// threads those are has to be readable from the payload, or the
    /// missing comments are missing silently.
    #[test]
    fn a_thread_that_reports_more_comments_is_marked_for_a_follow_up() {
        let truncated = r#"{"id":"PRRT_long","isResolved":false,"isOutdated":false,
            "path":"src/a.rs","line":1,"comments":{
              "pageInfo":{"hasNextPage":true,"endCursor":"Y3Vyc29y"},
              "nodes":[{"id":"PRRC_1","databaseId":1,"author":{"login":"a"},
                        "body":"first","createdAt":"2026-08-16T09:00:00Z"}]}}"#;
        let complete = r#"{"id":"PRRT_short","isResolved":false,"isOutdated":false,
            "path":"src/b.rs","line":2,"comments":{
              "pageInfo":{"hasNextPage":false,"endCursor":null},
              "nodes":[{"id":"PRRC_2","databaseId":2,"author":{"login":"b"},
                        "body":"only","createdAt":"2026-08-16T09:00:00Z"}]}}"#;

        let json = thread_page(&format!("{truncated},{complete}"));
        assert_eq!(truncated_thread_ids(&json), vec!["PRRT_long".to_string()]);

        // A payload without the connection's pageInfo at all (an older
        // recorded response) claims nothing rather than everything.
        assert!(truncated_thread_ids(&thread_page(UNRESOLVED_THREAD)).is_empty());
    }

    /// The follow-up query's own pages, which is where the comments past
    /// the first hundred actually live.
    #[test]
    fn the_follow_up_reads_every_page_of_one_threads_comments() {
        let page = |id: &str, body: &str| {
            format!(
                r#"{{"data":{{"node":{{"comments":{{"pageInfo":{{"hasNextPage":false,"endCursor":null}},
                "nodes":[{{"id":"{id}","databaseId":7,"author":{{"login":"juliusm"}},
                "body":"{body}","createdAt":"2026-08-16T09:00:00Z"}}]}}}}}}}}"#
            )
        };
        let joined = format!("{}\n{}", page("PRRC_100", "hundredth"), page("PRRC_101", "hundred-and-first"));
        let comments = parse_thread_comments(&joined);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[1].id, "PRRC_101");
        assert_eq!(comments[1].author, "juliusm");

        // Same forgiveness as the thread parser: an unusable payload is
        // no comments, never a panic.
        assert!(parse_thread_comments("{}").is_empty());
        assert!(parse_thread_comments("not json at all").is_empty());
    }

    // ── Failing-check log excerpt ──

    /// A commit can fail several workflows at once. The excerpt is shown
    /// under one named check, so the run has to be the one that check
    /// came from — another workflow's log reads as a fact about this
    /// check and is not one.
    #[test]
    fn the_failing_run_is_matched_to_the_check_that_asked() {
        let rows = "111\tDeploy\n222\tCI\n333\tNightly";
        // Check names are `workflow / job`, so the workflow name is a
        // prefix of the check name.
        assert_eq!(
            pick_failing_run(rows, "CI / test (ubuntu-latest)").as_deref(),
            Some("222")
        );
        // And the other direction, for hosts that report the bare name.
        assert_eq!(pick_failing_run(rows, "Deploy").as_deref(), Some("111"));
        // Case is not a distinction anybody makes here.
        assert_eq!(pick_failing_run(rows, "nightly").as_deref(), Some("333"));
    }

    #[test]
    fn an_unrelated_failure_is_not_offered_as_this_checks_log() {
        let rows = "111\tDeploy\n222\tCI";
        assert_eq!(pick_failing_run(rows, "Lint"), None);
        // Nothing to match against is also no match — not "the first
        // one".
        assert_eq!(pick_failing_run(rows, "   "), None);
        assert_eq!(pick_failing_run("", "CI"), None);
        // A malformed row is skipped, not treated as an id.
        assert_eq!(pick_failing_run("no-tab-here\n222\tCI", "CI").as_deref(), Some("222"));
    }

    #[test]
    fn the_excerpt_keeps_the_named_jobs_tail_without_its_prefixes() {
        let log = "\
build\tcompile\t2026-08-16T09:00:00.000Z warming up
test\trun\t2026-08-16T09:00:01.000Z running 3 tests
test\trun\t2026-08-16T09:00:02.000Z assertion failed: left == right
build\tcompile\t2026-08-16T09:00:03.000Z done";
        let excerpt = tail_check_log(log, "test");
        assert_eq!(excerpt, "running 3 tests\nassertion failed: left == right");
    }

    /// Job names and check names diverge on matrix builds, and an empty
    /// card is worse than a slightly wider one.
    #[test]
    fn a_check_name_that_matches_no_job_falls_back_to_the_whole_log() {
        let log = "build\tcompile\t2026-08-16T09:00:00.000Z warming up\n\
                   build\tcompile\t2026-08-16T09:00:01.000Z boom";
        assert_eq!(tail_check_log(log, "nothing-like-this"), "warming up\nboom");
        assert_eq!(tail_check_log(log, ""), "warming up\nboom");
    }

    #[test]
    fn the_excerpt_is_capped_at_its_tail() {
        let log: String = (0..CHECK_LOG_EXCERPT_LINES + 10)
            .map(|i| format!("job\tstep\t2026-08-16T09:00:00.000Z line {i}\n"))
            .collect();
        let excerpt = tail_check_log(&log, "job");
        assert_eq!(excerpt.lines().count(), CHECK_LOG_EXCERPT_LINES);
        assert!(excerpt.starts_with("line 10"), "{excerpt}");
    }

    #[test]
    fn only_a_real_timestamp_prefix_is_stripped() {
        assert_eq!(
            strip_log_prefix("job\tstep\t2026-08-16T09:00:00.000Z hello world"),
            "hello world"
        );
        // A line whose last tab-separated field is not a timestamp keeps
        // every word of itself.
        assert_eq!(strip_log_prefix("job\tstep\tnot a timestamp"), "not a timestamp");
        assert_eq!(strip_log_prefix("no tabs at all"), "no tabs at all");
    }

    // ── Merge arguments ──

    #[test]
    fn a_merge_commits_subject_and_body_are_passed_through() {
        assert_eq!(
            merge_args(7, "merge", true, Some("Ship it"), Some("Because.")),
            vec![
                "pr",
                "merge",
                "7",
                "--merge",
                "--delete-branch",
                "--subject",
                "Ship it",
                "--body",
                "Because."
            ]
        );
        // Squash takes the same two flags; only the strategy changes.
        let squash = merge_args(7, "squash", false, Some("Ship it"), None);
        assert_eq!(squash[3], "--squash");
        assert!(!squash.contains(&"--delete-branch".to_string()));
        assert_eq!(squash[squash.len() - 2..], ["--subject", "Ship it"]);
    }

    /// `gh pr merge --rebase` rejects `--subject` and `--body`: a rebase
    /// produces no merge commit for them to title.
    #[test]
    fn a_rebase_carries_no_commit_message_flags() {
        let args = merge_args(7, "rebase", true, Some("Ship it"), Some("Because."));
        assert_eq!(args, vec!["pr", "merge", "7", "--rebase", "--delete-branch"]);
    }

    /// A blank field is the sheet's default, not an instruction to title
    /// the commit with whitespace.
    #[test]
    fn blank_message_fields_are_left_off_entirely() {
        assert_eq!(
            merge_args(7, "merge", false, Some("   "), Some("")),
            vec!["pr", "merge", "7", "--merge"]
        );
        assert_eq!(
            merge_args(7, "merge", false, None, None),
            vec!["pr", "merge", "7", "--merge"]
        );
        // An unrecognised strategy is a merge commit, never a rebase.
        assert_eq!(merge_args(7, "wat", false, None, None)[3], "--merge");
    }

    // ── Draft comment payloads ──

    fn draft(start_line: Option<u32>) -> PrDraftComment {
        PrDraftComment {
            file: "src/lib.rs".to_string(),
            body: "This allocates twice.".to_string(),
            side: "RIGHT".to_string(),
            line: 42,
            start_line,
        }
    }

    #[test]
    fn a_single_line_note_sends_content_coordinates_only() {
        let payload = draft(None).to_json();
        assert_eq!(payload["path"], "src/lib.rs");
        assert_eq!(payload["body"], "This allocates twice.");
        assert_eq!(payload["side"], "RIGHT");
        assert_eq!(payload["line"], 42);
        // No range, so no range keys — GitHub rejects a `start_line`
        // that equals `line`.
        assert!(payload.get("start_line").is_none());
        assert!(payload.get("start_side").is_none());
        // The legacy patch offset is never sent: it is only meaningful
        // against one exact diff.
        assert!(payload.get("position").is_none());
    }

    /// A range cannot straddle sides, so the start side is the end side
    /// — and saying so explicitly is what keeps GitHub from guessing.
    #[test]
    fn a_multi_line_note_sends_both_ends_on_one_side() {
        let payload = draft(Some(38)).to_json();
        assert_eq!(payload["start_line"], 38);
        assert_eq!(payload["start_side"], "RIGHT");
        assert_eq!(payload["line"], 42);
        assert_eq!(payload["side"], "RIGHT");
    }

    /// The 422 a reviewer can act on, in every wording GitHub has used
    /// for it — the modern `line`/`start_line` API does not say "Line
    /// could not be resolved" at all.
    #[test]
    fn a_stale_anchor_is_recognised_in_every_wording() {
        assert!(is_stale_anchor_error(
            "HTTP 422: pull_request_review_thread.line must be part of the diff"
        ));
        assert!(is_stale_anchor_error(
            "HTTP 422: pull_request_review_thread.start_line must be part of the same hunk"
        ));
        assert!(is_stale_anchor_error("Line could not be resolved"));
        // An unrelated failure keeps its own words; replacing them would
        // send the reviewer re-anchoring notes that are fine.
        assert!(!is_stale_anchor_error("HTTP 403: Resource not accessible"));
    }
}
