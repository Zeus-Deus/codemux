use codemux_lib::github::*;

// ═══════════════════════════════════════
// Parsing tests (always run, no gh CLI needed)
// ═══════════════════════════════════════

#[test]
fn parse_pr_with_all_fields() {
    let json = r#"{
        "number": 42,
        "url": "https://github.com/owner/repo/pull/42",
        "state": "OPEN",
        "title": "Add authentication",
        "headRefName": "feature/auth",
        "baseRefName": "main",
        "isDraft": false,
        "mergeable": "MERGEABLE",
        "additions": 150,
        "deletions": 23,
        "reviewDecision": "APPROVED",
        "updatedAt": "2026-03-20T10:30:00Z"
    }"#;
    let pr: PullRequestInfo = serde_json::from_str(json).unwrap();
    assert_eq!(pr.number, 42);
    assert_eq!(pr.url, "https://github.com/owner/repo/pull/42");
    assert_eq!(pr.state, "OPEN");
    assert_eq!(pr.title, "Add authentication");
    assert_eq!(pr.head_branch.as_deref(), Some("feature/auth"));
    assert_eq!(pr.base_branch.as_deref(), Some("main"));
    assert!(!pr.is_draft);
    assert_eq!(pr.mergeable.as_deref(), Some("MERGEABLE"));
    assert_eq!(pr.additions, Some(150));
    assert_eq!(pr.deletions, Some(23));
    assert_eq!(pr.review_decision.as_deref(), Some("APPROVED"));
    assert_eq!(pr.updated_at.as_deref(), Some("2026-03-20T10:30:00Z"));
}

#[test]
fn parse_pr_with_null_optionals() {
    let json = r#"{
        "number": 10,
        "url": "https://github.com/u/r/pull/10",
        "state": "OPEN",
        "title": "Fix bug",
        "headRefName": null,
        "baseRefName": null,
        "isDraft": false,
        "mergeable": null,
        "additions": null,
        "deletions": null,
        "reviewDecision": null,
        "updatedAt": null
    }"#;
    let pr: PullRequestInfo = serde_json::from_str(json).unwrap();
    assert_eq!(pr.number, 10);
    assert!(pr.head_branch.is_none());
    assert!(pr.base_branch.is_none());
    assert!(pr.mergeable.is_none());
    assert!(pr.additions.is_none());
    assert!(pr.deletions.is_none());
    assert!(pr.review_decision.is_none());
    assert!(pr.updated_at.is_none());
}

#[test]
fn parse_pr_draft_state() {
    let json = r#"{
        "number": 5,
        "url": "https://github.com/u/r/pull/5",
        "state": "OPEN",
        "title": "WIP: new feature",
        "isDraft": true
    }"#;
    let pr: PullRequestInfo = serde_json::from_str(json).unwrap();
    assert!(pr.is_draft);
    assert_eq!(pr.state, "OPEN");
}

#[test]
fn parse_pr_with_updated_at() {
    let json = r#"{
        "number": 99,
        "url": "https://github.com/u/r/pull/99",
        "state": "MERGED",
        "title": "Done",
        "updatedAt": "2026-03-24T15:00:00Z"
    }"#;
    let pr: PullRequestInfo = serde_json::from_str(json).unwrap();
    assert_eq!(pr.state, "MERGED");
    assert_eq!(pr.updated_at.as_deref(), Some("2026-03-24T15:00:00Z"));
}

#[test]
fn parse_checks_with_timing() {
    let json = r#"[
        {
            "name": "build",
            "state": "SUCCESS",
            "conclusion": "SUCCESS",
            "elapsedTime": "2m30s",
            "detailUrl": "https://github.com/u/r/actions/runs/1",
            "startedAt": "2026-01-01T00:00:00Z",
            "completedAt": "2026-01-01T00:02:30Z"
        },
        {
            "name": "lint",
            "state": "FAILURE",
            "conclusion": "FAILURE"
        }
    ]"#;
    let checks: Vec<CheckInfo> = serde_json::from_str(json).unwrap();
    assert_eq!(checks.len(), 2);

    assert_eq!(checks[0].name, "build");
    assert_eq!(checks[0].elapsed_time.as_deref(), Some("2m30s"));
    assert_eq!(checks[0].detail_url.as_deref(), Some("https://github.com/u/r/actions/runs/1"));
    assert_eq!(checks[0].started_at.as_deref(), Some("2026-01-01T00:00:00Z"));
    assert_eq!(checks[0].completed_at.as_deref(), Some("2026-01-01T00:02:30Z"));

    // Second check has no timing fields
    assert!(checks[1].elapsed_time.is_none());
    assert!(checks[1].detail_url.is_none());
}

#[test]
fn parse_checks_empty_array() {
    let json = r#"[]"#;
    let checks: Vec<CheckInfo> = serde_json::from_str(json).unwrap();
    assert!(checks.is_empty());
}

#[test]
fn parse_review_comments_multiple() {
    let json = r#"{
        "reviews": [
            {
                "id": 1,
                "author": {"login": "alice"},
                "body": "LGTM",
                "state": "APPROVED",
                "submittedAt": "2026-03-20T10:00:00Z"
            },
            {
                "id": 2,
                "author": {"login": "bob"},
                "body": "Please fix the typo",
                "state": "CHANGES_REQUESTED",
                "submittedAt": "2026-03-20T11:00:00Z"
            },
            {
                "id": 3,
                "author": {"login": "charlie"},
                "body": "Interesting approach",
                "state": "COMMENTED",
                "submittedAt": "2026-03-20T12:00:00Z"
            }
        ]
    }"#;
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let arr = v["reviews"].as_array().unwrap();
    let comments: Vec<ReviewComment> = arr
        .iter()
        .map(|r| ReviewComment {
            id: r["id"].as_u64().unwrap_or(0),
            author: r["author"]["login"].as_str().unwrap_or("").to_string(),
            body: r["body"].as_str().unwrap_or("").to_string(),
            state: r["state"].as_str().unwrap_or("COMMENTED").to_string(),
            created_at: r["submittedAt"].as_str().unwrap_or("").to_string(),
        })
        .filter(|r| !r.body.is_empty())
        .collect();

    assert_eq!(comments.len(), 3);
    assert_eq!(comments[0].author, "alice");
    assert_eq!(comments[0].state, "APPROVED");
    assert_eq!(comments[1].author, "bob");
    assert_eq!(comments[1].state, "CHANGES_REQUESTED");
    assert_eq!(comments[2].author, "charlie");
    assert_eq!(comments[2].state, "COMMENTED");
}

#[test]
fn parse_review_comments_empty_body_filtered() {
    let json = r#"{
        "reviews": [
            {
                "id": 1,
                "author": {"login": "alice"},
                "body": "",
                "state": "APPROVED",
                "submittedAt": "2026-03-20T10:00:00Z"
            },
            {
                "id": 2,
                "author": {"login": "bob"},
                "body": "Good work",
                "state": "APPROVED",
                "submittedAt": "2026-03-20T11:00:00Z"
            }
        ]
    }"#;
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let arr = v["reviews"].as_array().unwrap();
    let comments: Vec<ReviewComment> = arr
        .iter()
        .map(|r| ReviewComment {
            id: r["id"].as_u64().unwrap_or(0),
            author: r["author"]["login"].as_str().unwrap_or("").to_string(),
            body: r["body"].as_str().unwrap_or("").to_string(),
            state: r["state"].as_str().unwrap_or("COMMENTED").to_string(),
            created_at: r["submittedAt"].as_str().unwrap_or("").to_string(),
        })
        .filter(|r| !r.body.is_empty())
        .collect();

    assert_eq!(comments.len(), 1, "empty body should be filtered");
    assert_eq!(comments[0].author, "bob");
}

#[test]
fn parse_inline_comment_with_line_ref() {
    let json = r#"[{
        "id": 100,
        "user": {"login": "reviewer"},
        "body": "This variable is unused",
        "path": "src/main.rs",
        "line": 42,
        "created_at": "2026-03-20T10:00:00Z",
        "in_reply_to_id": null,
        "pull_request_review_id": 500
    }]"#;
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let c = &v[0];
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

    assert_eq!(comment.path, "src/main.rs");
    assert_eq!(comment.line, Some(42));
    assert_eq!(comment.pull_request_review_id, Some(500));
    assert!(comment.in_reply_to_id.is_none());
}

#[test]
fn parse_inline_comment_thread_reply() {
    let json = r#"[{
        "id": 101,
        "user": {"login": "author"},
        "body": "Fixed in next commit",
        "path": "src/main.rs",
        "line": 42,
        "created_at": "2026-03-20T11:00:00Z",
        "in_reply_to_id": 100,
        "pull_request_review_id": 500
    }]"#;
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let c = &v[0];
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

    assert_eq!(comment.in_reply_to_id, Some(100));
    assert_eq!(comment.pull_request_review_id, Some(500));
}

#[test]
fn parse_deployment_with_url() {
    let json = r#"{
        "id": 1000,
        "environment": "preview",
        "state": "success",
        "url": "https://preview-42.example.com",
        "created_at": "2026-03-20T12:00:00Z"
    }"#;
    let dep: DeploymentInfo = serde_json::from_str(json).unwrap();
    assert_eq!(dep.id, 1000);
    assert_eq!(dep.environment, "preview");
    assert_eq!(dep.state, "success");
    assert_eq!(dep.url.as_deref(), Some("https://preview-42.example.com"));
}

#[test]
fn parse_deployment_without_url() {
    let json = r#"{
        "id": 2000,
        "environment": "staging",
        "state": "pending",
        "created_at": "2026-03-20T13:00:00Z"
    }"#;
    let dep: DeploymentInfo = serde_json::from_str(json).unwrap();
    assert_eq!(dep.id, 2000);
    assert_eq!(dep.environment, "staging");
    assert!(dep.url.is_none());
}

// ═══════════════════════════════════════
// Integration tests (need real gh CLI)
// These are #[ignore] by default — run with:
//   cargo test --manifest-path src-tauri/Cargo.toml -- --include-ignored
// ═══════════════════════════════════════

#[test]
#[ignore]
fn gh_status_returns_valid_enum() {
    let status = check_gh_status();
    match &status {
        GhStatus::NotInstalled => println!("gh not installed"),
        GhStatus::NotAuthenticated => println!("gh not authenticated"),
        GhStatus::Authenticated { username } => {
            println!("authenticated as {}", username);
            assert!(!username.is_empty());
        }
    }
}

#[test]
#[ignore]
fn gh_available_returns_bool() {
    let available = gh_available();
    // Should not panic, just return true or false
    println!("gh available: {}", available);
}

#[test]
#[ignore]
fn is_github_repo_detects_correctly() {
    // Run against the codemux repo root
    let repo_root = std::env::current_dir().expect("cwd");
    let result = is_github_repo(&repo_root);
    // This test assumes we're in the codemux repo
    println!("is_github_repo: {}", result);
}

#[test]
#[ignore]
fn get_branch_pr_returns_option() {
    let repo_root = std::env::current_dir().expect("cwd");
    let result = get_branch_pr(&repo_root);
    match result {
        Ok(Some(pr)) => {
            println!("PR #{}: {}", pr.number, pr.title);
            assert!(pr.number > 0);
            assert!(!pr.url.is_empty());
        }
        Ok(None) => println!("No PR for current branch"),
        Err(e) => println!("Error (acceptable if not a GH repo): {}", e),
    }
}

#[test]
#[ignore]
fn get_pr_checks_returns_vec() {
    let repo_root = std::env::current_dir().expect("cwd");
    let result = get_pr_checks(&repo_root);
    match result {
        Ok(checks) => {
            println!("{} checks found", checks.len());
            for c in &checks {
                println!("  {} - {}", c.name, c.status);
            }
        }
        Err(e) => println!("Error (acceptable): {}", e),
    }
}

#[test]
#[ignore]
fn get_pr_review_comments_returns_vec() {
    let repo_root = std::env::current_dir().expect("cwd");
    let result = get_pr_review_comments(&repo_root);
    match result {
        Ok(reviews) => {
            println!("{} reviews found", reviews.len());
            for r in &reviews {
                println!("  {} - {} - {}", r.author, r.state, r.body.chars().take(50).collect::<String>());
            }
        }
        Err(e) => println!("Error (acceptable): {}", e),
    }
}

#[test]
#[ignore]
fn get_pr_inline_comments_returns_vec() {
    let repo_root = std::env::current_dir().expect("cwd");
    // Need a PR number — try to get one
    if let Ok(Some(pr)) = get_branch_pr(&repo_root) {
        let result = get_pr_inline_comments(&repo_root, pr.number);
        match result {
            Ok(comments) => {
                println!("{} inline comments found", comments.len());
                for c in &comments {
                    println!("  {}:{:?} by {} - {}", c.path, c.line, c.author, c.body.chars().take(50).collect::<String>());
                }
            }
            Err(e) => println!("Error (acceptable): {}", e),
        }
    } else {
        println!("No PR for current branch, skipping");
    }
}

#[test]
#[ignore]
fn get_pr_deployments_returns_vec() {
    let repo_root = std::env::current_dir().expect("cwd");
    if let Ok(Some(pr)) = get_branch_pr(&repo_root) {
        let result = get_pr_deployments(&repo_root, pr.number);
        match result {
            Ok(deployments) => {
                println!("{} deployments found", deployments.len());
                for d in &deployments {
                    println!("  {} - {} - {:?}", d.environment, d.state, d.url);
                }
            }
            Err(e) => println!("Error (acceptable): {}", e),
        }
    } else {
        println!("No PR for current branch, skipping");
    }
}
