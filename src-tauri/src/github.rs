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

    let output = Command::new("gh")
        .args(["auth", "status"])
        .output();

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
    let output = Command::new("gh")
        .args(args)
        .current_dir(repo_path)
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
    Command::new("gh")
        .args(args)
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
}

pub fn gh_available() -> bool {
    Command::new("which")
        .arg("gh")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn is_github_repo(repo_path: &Path) -> bool {
    run_gh_optional(repo_path, &["repo", "view", "--json", "url"]).is_some()
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

/// Run `gh` with a timeout. Returns Err if the process doesn't finish in time.
fn run_gh_timed(repo_path: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new("gh")
        .args(args)
        .current_dir(repo_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
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

    let json_fields = "number,title,state,labels,assignees,url";

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

    let number_str = number.to_string();
    let output = run_gh_timed(
        repo_path,
        &[
            "issue", "view", &number_str,
            "--json", "number,title,state,labels,assignees,url,body",
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

    Ok(issue)
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
        let truncated = &trimmed[..max_title_len];
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

pub fn get_branch_pr(repo_path: &Path) -> Result<Option<PullRequestInfo>, String> {
    let output = run_gh_optional(
        repo_path,
        &[
            "pr", "view",
            "--json", "number,url,state,title,headRefName,baseRefName,isDraft,mergeable,additions,deletions,reviewDecision,updatedAt",
        ],
    );

    let Some(json_str) = output else {
        return Ok(None);
    };

    let v: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse PR JSON: {e}"))?;

    Ok(Some(parse_pr_json(&v)))
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
    let v = run_gh_json(
        repo_path,
        &[
            "pr", "list",
            "--state", state,
            "--limit", "30",
            "--json", "number,url,state,title,headRefName,isDraft",
        ],
    )?;

    let arr = v.as_array().ok_or("Expected JSON array from gh pr list")?;
    Ok(arr.iter().map(parse_pr_json).collect())
}

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
    let output = run_gh_optional(
        repo_path,
        &["pr", "checks", "--json", "name,state,conclusion,elapsedTime,detailUrl,startedAt,completedAt"],
    );

    let Some(json_str) = output else {
        return Ok(Vec::new());
    };

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
            conclusion: c["conclusion"].as_str().map(|s| s.to_string()),
            elapsed_time: c["elapsedTime"].as_str().map(|s| s.to_string()),
            detail_url: c["detailUrl"].as_str().map(|s| s.to_string()),
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
    fn test_parse_deployment_info() {
        let json = r#"{
            "id": 500,
            "environment": "preview",
            "state": "success",
            "url": "https://preview.example.com",
            "created_at": "2026-01-20T12:00:00Z"
        }"#;
        let dep: DeploymentInfo = serde_json::from_str(json).unwrap();
        assert_eq!(dep.id, 500);
        assert_eq!(dep.environment, "preview");
        assert_eq!(dep.url.as_deref(), Some("https://preview.example.com"));
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
}
