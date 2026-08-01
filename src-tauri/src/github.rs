use crate::execution::{sanitize_gui_env_std, sanitize_gui_env_std_keep_dbus};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

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
    /// Head commit SHA reported by GitHub. Kept as useful PR metadata, but
    /// association is branch/repository based: a local worktree may
    /// legitimately be behind the final remote PR head after review commits
    /// land, so exact SHA equality is not a valid identity check.
    #[serde(alias = "headRefOid", default)]
    pub head_ref_oid: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentInfo {
    pub id: u64,
    pub environment: String,
    pub state: String,
    pub url: Option<String>,
    pub created_at: String,
}

pub fn check_gh_status() -> GhStatus {
    if !gh_available() {
        return GhStatus::NotInstalled;
    }

    let mut cmd = Command::new("gh");
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

    // gh auth status prints to stderr: "Logged in to github.com account USERNAME (...)"
    let stderr = String::from_utf8_lossy(&output.stderr);
    let username = stderr
        .lines()
        .find_map(|line| {
            line.find("account ").map(|pos| {
                let after = &line[pos + 8..];
                after.split_whitespace().next().unwrap_or("").to_string()
            })
        })
        .unwrap_or_default();

    GhStatus::Authenticated { username }
}

fn run_gh(repo_path: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("gh");
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
    let mut cmd = Command::new("gh");
    cmd.args(args).current_dir(repo_path);
    // Keep DBus available — see `check_gh_status` rationale.
    sanitize_gui_env_std_keep_dbus(&mut cmd);
    cmd.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
}

pub fn gh_available() -> bool {
    let mut cmd = Command::new("which");
    cmd.arg("gh");
    sanitize_gui_env_std(&mut cmd);
    cmd.output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn is_github_repo(repo_path: &Path) -> bool {
    // Pure-git check: does any remote URL point at github.com?
    //
    // We deliberately do NOT shell out to `gh repo view` here — that
    // command requires `gh` to be installed AND authenticated, so a
    // user with a perfectly valid GitHub remote but no `gh auth login`
    // would otherwise see the preflight return `false` and the popup
    // surface "Not a GitHub repo" copy. Auth state is a separate
    // signal, surfaced via `check_gh_status()`; the UI disambiguates
    // the two failure modes (not a github repo vs. needs auth).
    let mut cmd = Command::new("git");
    cmd.args(["remote", "-v"]).current_dir(repo_path);
    sanitize_gui_env_std(&mut cmd);
    let Ok(output) = cmd.output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    remote_text_points_at_github(&text)
}

/// Pure helper so we can unit-test the URL-matching logic without
/// spinning up a real git repo on disk. Accepts the raw stdout from
/// `git remote -v` and returns true when any remote URL is hosted on
/// github.com — both SSH (`git@github.com:…`) and HTTPS
/// (`https://github.com/…`) forms count, in either fetch or push rows.
/// We match the bare hostname rather than a full URL prefix so a stray
/// remote-name containing "github.com" can't false-positive (the
/// hostname always sits between protocol and path).
pub(crate) fn remote_text_points_at_github(text: &str) -> bool {
    text.lines().any(|line| {
        // `git remote -v` rows look like:
        //   origin\tgit@github.com:user/repo.git (fetch)
        //   origin\thttps://github.com/user/repo (fetch)
        // The URL is the second whitespace-delimited token.
        let url = match line.split_whitespace().nth(1) {
            Some(u) => u,
            None => return false,
        };
        url.contains("github.com:") || url.contains("github.com/")
    })
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

const MAX_ISSUE_BODY_BYTES: usize = 50 * 1024; // 50 KB
const ISSUE_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Stage 4 — cap the comment list shipped to the agent so a long
/// thread can't blow out the prompt. The full count is preserved in
/// `total_comments` so the agent can still see "20 of 250 shown".
pub const MAX_ISSUE_COMMENTS: usize = 20;

/// Run `gh` with a timeout. Returns Err if the process doesn't finish in time.
fn run_gh_timed(repo_path: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new("gh");
    cmd.args(args)
        .current_dir(repo_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Keep DBus available — issue list/view both pull the auth
    // token from the user's secret-service keyring on Linux. See
    // `check_gh_status` rationale.
    sanitize_gui_env_std_keep_dbus(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to run gh: {e}"))?;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = child.stdout.take().map(|mut s| {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut s, &mut buf).ok();
                    buf
                }).unwrap_or_default();

                if !status.success() {
                    let stderr = child.stderr.take().map(|mut s| {
                        let mut buf = String::new();
                        std::io::Read::read_to_string(&mut s, &mut buf).ok();
                        buf
                    }).unwrap_or_default();
                    return Err(format!(
                        "gh {} failed: {}",
                        args.first().unwrap_or(&""),
                        stderr.trim()
                    ));
                }
                return Ok(stdout.trim_end().to_string());
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("gh command timed out after {}s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("Failed to wait for gh: {e}")),
        }
    }
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
            "--json", "number,url,state,title,headRefName,baseRefName,isDraft,mergeable,additions,deletions,reviewDecision,updatedAt,author,body,comments",
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

    if !full || output.len() <= MAX_PR_DIFF_BYTES {
        return Ok(output);
    }

    // Full diff exceeded the cap — truncate at a char boundary and
    // signpost what was cut so the agent doesn't silently miss
    // hunks. The trailing pointer mirrors `get_github_issue`'s
    // truncation marker.
    let mut end = MAX_PR_DIFF_BYTES;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    Ok(format!(
        "{}\n\n[Diff truncated at {}KB — use `gh pr diff {}` for the full patch]",
        &output[..end],
        MAX_PR_DIFF_BYTES / 1024,
        number,
    ))
}

const BRANCH_PR_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

fn run_git_optional(repo_path: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
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

fn branch_head_selector(
    branch: &str,
    tracking_remote_url: Option<&str>,
    origin_remote_url: Option<&str>,
) -> String {
    let Some((tracking_owner, tracking_repo)) = tracking_remote_url.and_then(parse_github_remote)
    else {
        return branch.to_string();
    };
    let Some((origin_owner, origin_repo)) = origin_remote_url.and_then(parse_github_remote) else {
        return branch.to_string();
    };

    if tracking_owner == origin_owner && tracking_repo == origin_repo {
        branch.to_string()
    } else {
        format!("{tracking_owner}:{branch}")
    }
}

fn resolve_branch_head_selector(repo_path: &Path, branch: &str) -> String {
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

    branch_head_selector(
        branch,
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
fn select_branch_pr(
    pull_requests: impl IntoIterator<Item = PullRequestInfo>,
    branch: &str,
    is_default_branch: bool,
) -> Option<PullRequestInfo> {
    let mut latest_open: Option<PullRequestInfo> = None;
    let mut latest_historical: Option<PullRequestInfo> = None;

    for pr in pull_requests {
        if pr.head_branch.as_deref() != Some(branch) {
            continue;
        }
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
/// `gh pr view` infers from local git state and an exact-SHA gate used to drop
/// a merged PR whenever review commits made the remote head newer than the
/// still-open local worktree. Listing every state for the branch mirrors T3
/// Code's model: branch identity owns the association, while SHA is metadata.
/// A failed GitHub request remains an `Err`, distinct from a successful empty
/// list, so callers can preserve the last known PR through transient failures.
pub fn get_branch_pr(repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
    let Some(branch) = run_git_optional(repo_path, &["branch", "--show-current"]) else {
        return Ok(None);
    };
    let head_selector = resolve_branch_head_selector(repo_path, &branch);
    let output = run_gh_timed(
        repo_path,
        &[
            "pr",
            "list",
            "--head",
            &head_selector,
            "--state",
            "all",
            "--limit",
            "20",
            "--json",
            "number,url,state,title,headRefName,baseRefName,isDraft,mergeable,additions,deletions,reviewDecision,updatedAt,headRefOid",
        ],
        BRANCH_PR_LOOKUP_TIMEOUT,
    )?;
    let value: serde_json::Value =
        serde_json::from_str(&output).map_err(|e| format!("Failed to parse PR JSON: {e}"))?;
    let rows = value
        .as_array()
        .ok_or_else(|| "Expected JSON array from gh pr list".to_string())?;
    let is_default_branch = crate::git::find_default_branch(repo_path).as_deref() == Some(&branch);

    Ok(select_branch_pr(
        rows.iter().map(parse_pr_json),
        &branch,
        is_default_branch,
    ))
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

// NOTE: As of 2026-04-26 the Review tab UI no longer exposes its own
// merge controls — that section was removed in the visual-match PR
// (`feature/review-tab-visual-match`). The Changes panel toolbar's
// split-button merge dropdown still uses this command, so this is not
// dead code; it's the active merge surface for Codemux. Comment kept
// for future archeology in case someone wonders why the Review tab
// doesn't have its own merge UI.
pub fn merge_pull_request(
    repo_path: &Path,
    pr_number: u32,
    method: &str,
) -> Result<(), String> {
    let number_str = pr_number.to_string();
    let method_flag = match method {
        "squash" => "--squash",
        "rebase" => "--rebase",
        _ => "--merge",
    };
    run_gh(
        repo_path,
        &["pr", "merge", &number_str, method_flag, "--delete-branch"],
    )?;
    Ok(())
}

pub fn get_pr_checks(repo_path: &Path) -> Result<Vec<CheckInfo>, String> {
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
    let output = Command::new("gh")
        .args([
            "pr",
            "checks",
            "--json",
            "name,state,bucket,link,startedAt,completedAt",
        ])
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

pub fn get_pr_review_comments(repo_path: &Path) -> Result<Vec<ReviewComment>, String> {
    let output = run_gh_optional(repo_path, &["pr", "view", "--json", "reviews"]);
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

// NOTE: As of 2026-04-26 the Review tab UI no longer exposes review
// submission — the composer was removed in the visual-match PR
// (`feature/review-tab-visual-match`) to mirror Superset's resting
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
        // Detail-only fields. List paths leave these None / empty so
        // the cheap query stays cheap.
        body: None,
        comments: Vec::new(),
        total_comments: 0,
        author,
    }
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
            head_ref_oid: head_ref_oid.map(|s| s.to_string()),
            body: None,
            comments: Vec::new(),
            total_comments: 0,
            author: None,
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
    fn test_branch_head_selector_uses_plain_branch_in_the_origin_repo() {
        assert_eq!(
            branch_head_selector(
                "feature/popup",
                Some("git@github.com:owner/repo.git"),
                Some("https://github.com/OWNER/repo.git"),
            ),
            "feature/popup"
        );
    }

    #[test]
    fn test_branch_head_selector_qualifies_a_fork_branch_by_owner() {
        assert_eq!(
            branch_head_selector(
                "feature/popup",
                Some("git@github.com:contributor/repo.git"),
                Some("https://github.com/upstream/repo.git"),
            ),
            "contributor:feature/popup"
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
        let selected = select_branch_pr([merged], "feature/popup", false).unwrap();
        assert_eq!(selected.number, 231);
        assert_eq!(selected.state, "MERGED");
    }

    #[test]
    fn test_select_branch_pr_hides_historical_prs_on_default_branch() {
        let merged = branch_pr(23, "MERGED", "main", "2026-08-01T10:00:00Z", None);
        let closed = branch_pr(24, "CLOSED", "main", "2026-08-02T10:00:00Z", None);
        assert!(select_branch_pr([merged, closed], "main", true).is_none());
    }

    #[test]
    fn test_select_branch_pr_allows_open_pr_on_default_branch() {
        let open = branch_pr(25, "OPEN", "main", "2026-08-01T10:00:00Z", None);
        assert_eq!(
            select_branch_pr([open], "main", true).map(|pr| pr.number),
            Some(25)
        );
    }

    #[test]
    fn test_select_branch_pr_prefers_open_over_newer_merged_pr() {
        let merged = branch_pr(45, "MERGED", "feature/reused", "2026-08-02T10:00:00Z", None);
        let open = branch_pr(46, "OPEN", "feature/reused", "2026-08-01T10:00:00Z", None);
        assert_eq!(
            select_branch_pr([merged, open], "feature/reused", false).map(|pr| pr.number),
            Some(46)
        );
    }

    #[test]
    fn test_select_branch_pr_uses_newest_historical_pr_and_ignores_other_branches() {
        let old = branch_pr(30, "MERGED", "feature/reused", "2026-08-01T10:00:00Z", None);
        let latest = branch_pr(31, "CLOSED", "feature/reused", "2026-08-02T10:00:00Z", None);
        let other = branch_pr(99, "OPEN", "feature/other", "2026-08-03T10:00:00Z", None);
        assert_eq!(
            select_branch_pr([old, latest, other], "feature/reused", false).map(|pr| pr.number),
            Some(31)
        );
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
            head_ref_oid: Some("deadbeef".into()),
            body: Some("PR body here".into()),
            comments: vec![IssueComment {
                author: "alice".into(),
                body: "ship it".into(),
                created_at: "2026-04-27T01:00:00Z".into(),
            }],
            total_comments: 1,
            author: Some("zeus".into()),
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
    fn remote_text_points_at_github_ssh_form() {
        let stdout = "\
origin\tgit@github.com:user/repo.git (fetch)
origin\tgit@github.com:user/repo.git (push)
";
        assert!(remote_text_points_at_github(stdout));
    }

    #[test]
    fn remote_text_points_at_github_https_form() {
        let stdout = "\
origin\thttps://github.com/user/repo (fetch)
origin\thttps://github.com/user/repo.git (push)
";
        assert!(remote_text_points_at_github(stdout));
    }

    #[test]
    fn remote_text_points_at_github_mixed_remotes() {
        // User has both an upstream (gitlab) and a fork (github). The
        // GitHub remote alone is enough — the function only cares
        // whether ANY remote points at github.com.
        let stdout = "\
upstream\thttps://gitlab.com/orig/repo (fetch)
upstream\thttps://gitlab.com/orig/repo (push)
fork\tgit@github.com:user/repo.git (fetch)
fork\tgit@github.com:user/repo.git (push)
";
        assert!(remote_text_points_at_github(stdout));
    }

    #[test]
    fn remote_text_points_at_github_returns_false_for_non_github_remotes() {
        let stdout = "\
origin\tgit@gitlab.com:user/repo.git (fetch)
origin\thttps://bitbucket.org/user/repo (push)
";
        assert!(!remote_text_points_at_github(stdout));
    }

    #[test]
    fn remote_text_points_at_github_returns_false_for_empty_input() {
        assert!(!remote_text_points_at_github(""));
        assert!(!remote_text_points_at_github("\n\n"));
    }

    #[test]
    fn remote_text_points_at_github_ignores_remote_names_that_contain_github() {
        // Regression: a user could theoretically name a non-GitHub
        // remote `github-old`. The match must look at the URL
        // (second token), not the remote-name (first token).
        let stdout = "\
github-old\tgit@gitlab.com:user/repo.git (fetch)
github-old\tgit@gitlab.com:user/repo.git (push)
";
        assert!(!remote_text_points_at_github(stdout));
    }

    #[test]
    fn remote_text_points_at_github_handles_enterprise_lookalikes() {
        // GHE on a custom hostname (e.g., `github.acme.internal`)
        // looks like a github URL but isn't github.com — it's a
        // separate product. We match the bare `github.com` host so
        // GHE doesn't false-positive. Users on GHE can still attach
        // issues via direct number entry; the popup hint is just
        // informational.
        let stdout = "\
origin\thttps://github.acme.internal/user/repo (fetch)
origin\thttps://github.acme.internal/user/repo (push)
";
        assert!(!remote_text_points_at_github(stdout));
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
}
