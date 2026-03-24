use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

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
}
