//! Live round trip of the GitLab adapter against a real instance.
//!
//! `#[ignore]`d: it needs a reachable GitLab, an authenticated `glab`,
//! and it creates a project. Run it explicitly:
//!
//! ```text
//! CODEMUX_GITLAB_TEST_TOKEN=<personal access token> \
//!   cargo test --manifest-path src-tauri/Cargo.toml \
//!   --test gitlab_live -- --ignored --nocapture
//! ```
//!
//! `CODEMUX_GITLAB_TEST_HOST` overrides the default `localhost:8929`.
//! The token needs `api` scope and is used for the git push only — every
//! adapter call authenticates through `glab`'s own stored credentials,
//! which is the code path that matters.
//!
//! Everything the test creates is left behind; the expectation is a
//! disposable instance.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use codemux_lib::git_provider::{self, ProviderKind};
use codemux_lib::github::{GhStatus, IssueState};
use codemux_lib::settings_sync::{self, UserSettings};

// ── Environment ─────────────────────────────────────────────────

fn host() -> String {
    std::env::var("CODEMUX_GITLAB_TEST_HOST").unwrap_or_else(|_| "localhost:8929".to_string())
}

fn token() -> String {
    std::env::var("CODEMUX_GITLAB_TEST_TOKEN").expect(
        "CODEMUX_GITLAB_TEST_TOKEN must name a personal access token with `api` scope \
         on the test instance",
    )
}

fn base_url() -> String {
    format!("http://{}", host())
}

fn bare_host() -> String {
    let host = host();
    host.split(':').next().unwrap_or(&host).to_string()
}

/// `glab`, run the way the adapter runs it: from inside the checkout, so
/// the host, the token and the project all resolve from the remote.
fn glab(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("glab")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("glab must be installed and on PATH");
    assert!(
        output.status.success(),
        "glab {args:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn glab_json(dir: &Path, args: &[&str]) -> serde_json::Value {
    let out = glab(dir, args);
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("glab {args:?} emitted non-JSON: {e}\n{out}"))
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Settings-cache injection ────────────────────────────────────

/// Maps the test instance's hostname to GitLab in the synced settings,
/// which is the only way detection can classify a host whose name gives
/// nothing away (`localhost`). Restores whatever was there on drop, so
/// running the test does not disturb a real install's settings.
struct CustomHostGuard {
    previous: Option<UserSettings>,
}

impl CustomHostGuard {
    fn install() -> Self {
        let previous = settings_sync::load_cache();
        let mut settings = previous.clone().unwrap_or_default();
        settings
            .source_control
            .custom_hosts
            .insert(bare_host(), "gitlab".to_string());
        settings_sync::save_cache(&settings).expect("write settings cache");
        // Detection memoises for 60s and the cache is keyed by path, not
        // by settings, so a stale entry would outlive this change.
        git_provider::invalidate_detection_cache(None);
        Self { previous }
    }
}

impl Drop for CustomHostGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(settings) => {
                let _ = settings_sync::save_cache(settings);
            }
            None => settings_sync::clear_cache(),
        }
        git_provider::invalidate_detection_cache(None);
    }
}

// ── Fixture ─────────────────────────────────────────────────────

struct Fixture {
    dir: PathBuf,
    project_path: String,
    _custom_host: CustomHostGuard,
    // Kept alive so the temp dir is not reaped mid-test.
    _temp: tempfile::TempDir,
}

/// Create a project on the instance, push a repository with a feature
/// branch to it, and point detection at it.
fn set_up() -> Fixture {
    assert!(
        Command::new("which")
            .arg("glab")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        "glab CLI is required for this test"
    );

    let custom_host = CustomHostGuard::install();
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().to_path_buf();

    // Unique per run: the instance is long-lived even when the projects
    // on it are disposable.
    let name = format!(
        "codemux-live-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "live@test.invalid"]);
    git(&dir, &["config", "user.name", "Live Test"]);

    // The remote is set before the project exists so every subsequent
    // `glab` call resolves the instance from it, exactly as the adapter
    // does inside a user's checkout.
    let remote = format!(
        "http://root:{}@{}/root/{name}.git",
        token(),
        host()
    );
    git(&dir, &["remote", "add", "origin", &remote]);

    let created = glab_json(
        &dir,
        &[
            "api",
            "--method",
            "POST",
            "projects",
            "-f",
            &format!("name={name}"),
            "-f",
            &format!("path={name}"),
            "-f",
            "visibility=private",
        ],
    );
    let project_path = created["path_with_namespace"]
        .as_str()
        .expect("created project path")
        .to_string();

    std::fs::write(dir.join("a.txt"), "line1\nline2\nline3\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "initial"]);
    git(&dir, &["push", "-q", "origin", "main"]);

    git(&dir, &["checkout", "-qb", "feature/live"]);
    std::fs::write(dir.join("a.txt"), "line1\nline2 changed\nline3\nline4\n").unwrap();
    std::fs::write(dir.join("b.txt"), "new file\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "feature commit"]);
    git(&dir, &["push", "-q", "origin", "feature/live"]);

    Fixture {
        dir,
        project_path,
        _custom_host: custom_host,
        _temp: temp,
    }
}

// ── The round trip ──────────────────────────────────────────────

#[test]
#[ignore]
fn gitlab_adapter_round_trips_against_a_live_instance() {
    let fixture = set_up();
    let dir = fixture.dir.as_path();

    // ── Detection resolves the checkout to the GitLab adapter ──

    let detected = git_provider::detect_provider(dir);
    assert_eq!(
        detected.kind,
        ProviderKind::GitLab,
        "custom-host mapping should classify {} as GitLab: {detected:?}",
        bare_host()
    );
    assert_eq!(detected.host.as_deref(), Some(bare_host().as_str()));
    assert_eq!(detected.base_url.as_deref(), Some(base_url().as_str()));

    let provider = git_provider::provider_for_path(dir);
    assert_eq!(provider.kind(), ProviderKind::GitLab);
    assert!(provider.is_implemented());
    assert!(provider.cli_available());

    // ── Auth probe reads the instance's block, not gitlab.com's ──

    match provider.auth_status() {
        GhStatus::Authenticated { username } => {
            assert!(!username.is_empty(), "should have parsed a username");
            println!("authenticated as {username}");
        }
        other => panic!("expected an authenticated glab, got {other:?}"),
    }

    // ── Create ──

    let created = provider
        .create_pull_request(dir, "Live round trip", "Body of the merge request.", Some("main"), false)
        .expect("create merge request");
    assert!(created.number > 0);
    assert_eq!(created.state, "OPEN");
    assert!(!created.is_draft);
    assert_eq!(created.head_branch.as_deref(), Some("feature/live"));
    assert_eq!(created.base_branch.as_deref(), Some("main"));
    assert_eq!(
        created.url,
        format!(
            "{}/{}/-/merge_requests/{}",
            base_url().trim_end_matches('/'),
            fixture.project_path,
            created.number
        ),
        "the URL the panel opens must be the instance's own merge-request page"
    );
    let number = created.number;

    // ── Branch lookup ──

    let looked_up = provider
        .branch_pull_request(dir)
        .expect("branch lookup")
        .expect("the checked-out branch has a merge request");
    assert_eq!(looked_up.number, number);
    assert_eq!(looked_up.state, "OPEN");
    assert_eq!(looked_up.display_state(), "OPEN");
    assert_eq!(
        provider
            .workspace_pull_request(dir)
            .expect("workspace lookup")
            .map(|pr| pr.number),
        Some(number)
    );

    // ── Detail ──

    let detail = provider.get_pull_request(dir, number).expect("get");
    assert_eq!(detail.number, number);
    assert_eq!(detail.title, "Live round trip");
    assert_eq!(detail.body.as_deref(), Some("Body of the merge request."));
    assert_eq!(detail.author.as_deref(), Some("root"));
    // Mergeability is computed asynchronously by the instance, so a
    // freshly created merge request legitimately reports `checking`.
    // What matters here is that whatever it reports lands in the
    // three-word vocabulary the attachment block prints.
    assert!(
        matches!(
            detail.mergeable.as_deref(),
            Some("MERGEABLE") | Some("CONFLICTING") | Some("UNKNOWN")
        ),
        "unexpected mergeable value: {:?}",
        detail.mergeable
    );
    // +3 / -1 across the two changed files.
    assert_eq!(detail.additions, Some(3), "diff stats from GraphQL");
    assert_eq!(detail.deletions, Some(1));
    assert_eq!(detail.review_decision.as_deref(), Some("REVIEW_REQUIRED"));

    // ── Lists ──

    let open = provider.list_pull_requests(dir, "open").expect("list open");
    assert!(open.iter().any(|pr| pr.number == number), "{open:?}");
    let merged_only = provider
        .list_pull_requests(dir, "merged")
        .expect("list merged");
    assert!(
        !merged_only.iter().any(|pr| pr.number == number),
        "an open merge request must not show up under merged"
    );

    let incoming = provider
        .list_incoming_pull_requests(dir, "main")
        .expect("incoming");
    let row = incoming
        .iter()
        .find(|item| item.number == number)
        .expect("the merge request targets main");
    assert_eq!(row.author, "root");
    assert_eq!(row.head_branch.as_deref(), Some("feature/live"));

    // ── Diff ──

    let names = provider.pull_request_diff(dir, number, false).expect("name-only diff");
    let listed: Vec<&str> = names.lines().collect();
    assert!(listed.contains(&"a.txt"), "{names}");
    assert!(listed.contains(&"b.txt"), "{names}");

    let full = provider.pull_request_diff(dir, number, true).expect("full diff");
    assert!(full.contains("diff --git"), "{full}");
    assert!(full.contains("+line2 changed"), "{full}");

    // ── Discussions → threads + inline comments ──

    seed_discussions(dir, number);

    let threads = provider
        .pull_request_review_comments(dir, None)
        .expect("review threads");
    let thread = threads
        .iter()
        .find(|t| t.body == "Overall looks good")
        .expect("the conversation note is a thread, not an inline comment");
    assert_eq!(thread.author, "root");
    assert_eq!(thread.state, "COMMENTED");
    assert!(
        !threads.iter().any(|t| t.body == "Inline nit on line 2"),
        "a diff-anchored note must not leak into the conversation surface"
    );

    let inline = provider
        .pull_request_inline_comments(dir, number)
        .expect("inline comments");
    let anchored = inline
        .iter()
        .find(|c| c.body == "Inline nit on line 2")
        .expect("the diff-anchored note is an inline comment");
    assert_eq!(anchored.path, "a.txt");
    assert_eq!(anchored.line, Some(2));
    assert_eq!(anchored.in_reply_to_id, None);
    assert_eq!(anchored.pull_request_review_id, Some(anchored.id));
    assert!(
        !inline.iter().any(|c| c.body == "Overall looks good"),
        "a conversation note must not leak into the inline surface"
    );

    // ── Checks ──

    let sha = String::from_utf8_lossy(
        &Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();
    for (name, state) in [("build", "running"), ("lint", "success"), ("test", "failed")] {
        glab(
            dir,
            &[
                "api",
                "--method",
                "POST",
                &format!("projects/:id/statuses/{sha}"),
                "-f",
                &format!("state={state}"),
                "-f",
                &format!("name={name}"),
                "-f",
                &format!("target_url=http://ci.invalid/{name}"),
                "-f",
                "ref=feature/live",
            ],
        );
    }

    let checks = provider.pull_request_checks(dir, None).expect("checks");
    let by_name: HashMap<&str, &codemux_lib::github::CheckInfo> =
        checks.iter().map(|c| (c.name.as_str(), c)).collect();
    assert_eq!(
        by_name["build"].conclusion.as_deref(),
        Some("pending"),
        "{checks:?}"
    );
    assert_eq!(by_name["lint"].conclusion.as_deref(), Some("pass"));
    assert_eq!(by_name["test"].conclusion.as_deref(), Some("fail"));
    assert_eq!(
        by_name["lint"].detail_url.as_deref(),
        Some("http://ci.invalid/lint")
    );

    // ── Issues ──

    glab(
        dir,
        &[
            "api",
            "--method",
            "POST",
            "projects/:id/issues",
            "-f",
            "title=Live issue",
            "-f",
            "description=Issue body",
            "-f",
            "labels=bug,ui",
        ],
    );
    glab(
        dir,
        &[
            "api",
            "--method",
            "POST",
            "projects/:id/issues/1/notes",
            "-f",
            "body=A comment on the issue",
        ],
    );

    let issues = provider.list_issues(dir, None).expect("issue list");
    let listed = issues
        .iter()
        .find(|i| i.title == "Live issue")
        .expect("the open issue is listed");
    assert_eq!(listed.number, 1);
    assert_eq!(listed.state, IssueState::Open);
    assert_eq!(listed.labels, vec!["bug".to_string(), "ui".to_string()]);

    let issue = provider.get_issue_fresh(dir, 1).expect("issue detail");
    assert_eq!(issue.body.as_deref(), Some("Issue body"));
    assert_eq!(issue.total_comments, 1);
    assert_eq!(issue.comments[0].body, "A comment on the issue");
    assert_eq!(issue.comments[0].author, "root");

    let searched = provider
        .list_issues(dir, Some("Live issue"))
        .expect("issue search");
    assert!(searched.iter().any(|i| i.number == 1), "{searched:?}");

    // ── Draft merge requests ──

    git(dir, &["checkout", "-qb", "feature/draft"]);
    std::fs::write(dir.join("c.txt"), "draft\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "draft commit"]);
    git(dir, &["push", "-q", "origin", "feature/draft"]);

    let draft = provider
        .create_pull_request(dir, "Work in progress", "", Some("main"), true)
        .expect("create draft");
    assert!(draft.is_draft, "{draft:?}");
    assert_eq!(draft.state, "OPEN");
    // The sidebar reads `display_state`, which folds the draft flag into
    // the state string.
    assert_eq!(draft.display_state(), "DRAFT");
    assert_eq!(
        provider
            .branch_pull_request(dir)
            .expect("draft branch lookup")
            .map(|pr| pr.display_state()),
        Some("DRAFT".to_string())
    );

    // ── Fork-merge-request fetch refspec ──

    git(dir, &["checkout", "-q", "main"]);
    let refspec = provider
        .fork_pr_fetch_refspec(number, "live-mr-head")
        .expect("GitLab exposes a merge-request head ref");
    assert_eq!(refspec, format!("refs/merge-requests/{number}/head:live-mr-head"));
    // The whole point of the refspec is that this fetch succeeds.
    git(dir, &["fetch", "origin", &refspec]);
    git(dir, &["rev-parse", "--verify", "live-mr-head"]);

    // ── Merge, and the state round trip ──

    provider
        .merge_pull_request(dir, number, "merge", true, None, None)
        .expect("merge");

    let merged = provider.get_pull_request(dir, number).expect("get merged");
    assert_eq!(
        merged.state, "MERGED",
        "merging must be observable through the adapter, not just on the instance"
    );
    assert_eq!(merged.display_state(), "MERGED");
    assert!(codemux_lib::github::is_historical_pr_state(&merged.state));

    // And the branch lookup agrees, so the sidebar pill settles.
    git(dir, &["checkout", "-q", "feature/live"]);
    let after = provider
        .branch_pull_request(dir)
        .expect("post-merge branch lookup")
        .expect("the merge request still resolves from its source branch");
    assert_eq!(after.number, number);
    assert_eq!(after.state, "MERGED");

    println!("live round trip complete against project {}", fixture.project_path);
}

/// One conversation note and one diff-anchored note.
///
/// The diff-anchored one needs a nested `position` object, which `glab
/// api -f` cannot express (it JSON-encodes each `-f` as a flat string
/// key), so it goes through `--input` with an explicit content type.
fn seed_discussions(dir: &Path, number: u32) {
    glab(
        dir,
        &[
            "api",
            "--method",
            "POST",
            &format!("projects/:id/merge_requests/{number}/discussions"),
            "-f",
            "body=Overall looks good",
        ],
    );

    let mr = glab_json(dir, &[
        "api",
        &format!("projects/:id/merge_requests/{number}"),
    ]);
    let refs = &mr["diff_refs"];
    let payload = serde_json::json!({
        "body": "Inline nit on line 2",
        "position": {
            "base_sha": refs["base_sha"],
            "start_sha": refs["start_sha"],
            "head_sha": refs["head_sha"],
            "position_type": "text",
            "new_path": "a.txt",
            "old_path": "a.txt",
            "new_line": 2,
        }
    });
    let body_file = dir.join(".discussion.json");
    std::fs::write(&body_file, serde_json::to_string(&payload).unwrap()).unwrap();
    glab(
        dir,
        &[
            "api",
            "--method",
            "POST",
            &format!("projects/:id/merge_requests/{number}/discussions"),
            "--input",
            body_file.to_str().unwrap(),
            "-H",
            "Content-Type: application/json",
        ],
    );
    std::fs::remove_file(&body_file).ok();
}
