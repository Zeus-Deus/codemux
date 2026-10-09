//! Real discovery + quota coordinator with a tokenless, offline gh fixture.
#![cfg(target_os = "linux")]
use codemux_lib::{github, github_budget};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl Drop for RestoreEnv {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}

fn git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo(root: &Path, name: &str, branch: &str, repository: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    git(
        &path,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    );
    git(&path, &["checkout", "-qb", branch]);
    git(
        &path,
        &[
            "remote",
            "add",
            "origin",
            &format!("https://discovery.example.test/fixture/{repository}.git"),
        ],
    );
    path
}

#[test]
fn siblings_batch_and_share_coordinator_cache_without_live_credentials() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let config = root.join("gh-config");
    fs::create_dir(&config).unwrap();
    let log = root.join("calls.jsonl");
    fs::write(&log, "").unwrap();
    let script = r#"#!/usr/bin/env python3
import json, os, re, sys, time
args = sys.argv[1:]
with open(os.environ['CODEMUX_DISCOVERY_FIXTURE_LOG'], 'a') as log:
    log.write(json.dumps(args) + '\n')
if args[:2] == ['auth', 'token']:
    sys.exit(1) # Force tokenless CLI; no synthetic bearer ever reaches HTTP.
if args[:2] == ['api', 'rate_limit']:
    quota = {'limit': 5000, 'remaining': 4999, 'reset': int(time.time()) + 3600}
    # Cheap flat discovery still fits above the reserve when an unknown
    # 200-point nested query would not. This exercises the real cost hint.
    graphql = dict(quota, remaining=699)
    print(json.dumps({'resources': {'core': quota, 'graphql': graphql}}))
    sys.exit(0)
if args[:2] != ['api', 'graphql']:
    sys.exit(99)
query = next(a[6:] for a in args if a.startswith('query='))
repository = {}
mode_path = os.path.join(os.getcwd(), '.discovery-mode')
mode = open(mode_path).read().strip() if os.path.exists(mode_path) else 'rows'
for alias, quoted in re.findall(r'(b\d+):pullRequests\(headRefName:("(?:[^"\\]|\\.)*")', query):
    branch = json.loads(quoted)
    number = 42 if branch == 'feature/a' else 43
    nodes = [] if mode == 'empty' else [{'number': number, 'url': 'https://discovery.example.test/fixture/app/pull/' + str(number), 'title': 'fixture', 'state': 'MERGED' if branch == 'feature/a' else 'OPEN', 'headRefName': branch, 'baseRefName': 'main', 'isDraft': False, 'updatedAt': '2026-01-01T00:00:00Z', 'headRepositoryOwner': {'login': 'fixture'}}]
    repository[alias] = {'totalCount': len(nodes), 'pageInfo': {'hasNextPage': mode == 'partial' and branch == 'feature/a'}, 'nodes': nodes}
print(json.dumps({'data': {'repository': repository}}))
"#;
    let gh = bin.join("gh");
    fs::write(&gh, script).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    // This integration binary owns its environment and runs one test only.
    let keys = [
        "PATH",
        "HOME",
        "GH_CONFIG_DIR",
        "GH_HOST",
        "GH_REPO",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "CODEMUX_DISCOVERY_FIXTURE_LOG",
        "GIT_CONFIG_NOSYSTEM",
        "GIT_CONFIG_GLOBAL",
    ];
    let _restore = RestoreEnv(
        keys.iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect(),
    );
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&old_path));
    std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
    std::env::set_var("HOME", root);
    std::env::set_var("GH_CONFIG_DIR", config);
    std::env::set_var("CODEMUX_DISCOVERY_FIXTURE_LOG", &log);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GIT_CONFIG_GLOBAL", root.join("absent-git-config"));
    for key in [
        "GH_HOST",
        "GH_REPO",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
    ] {
        std::env::remove_var(key);
    }
    let one = repo(root, "one", "feature/a", "app");
    let two = repo(root, "two", "feature/b", "app");
    let same = repo(root, "same", "feature/a", "app");
    let answers = github::get_workspace_prs_batch(&[one.clone(), two.clone()]);
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0].as_ref().unwrap()[0].pr.state, "MERGED");
    assert_eq!(answers[1].as_ref().unwrap()[0].pr.state, "OPEN");
    let calls = || -> Vec<Vec<String>> {
        fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };
    let graphql_count = || {
        calls()
            .iter()
            .filter(|args| {
                args.first().map(String::as_str) == Some("api")
                    && args.get(1).map(String::as_str) == Some("graphql")
            })
            .count()
    };
    assert_eq!(graphql_count(), 1, "two workspace heads share one request");
    let query_call = calls()
        .into_iter()
        .find(|args| args.get(1).map(String::as_str) == Some("graphql"))
        .unwrap();
    assert!(query_call.iter().any(|arg| arg == "owner=fixture"));
    assert!(query_call.iter().any(|arg| arg == "name=app"));
    assert!(query_call
        .iter()
        .all(|arg| !arg.contains("body") && !arg.contains("latestReviews")));
    // One-head queries from other callers coalesce across different CWDs.
    assert_eq!(github::get_branch_pr(&one).unwrap().unwrap().number, 42);
    assert_eq!(graphql_count(), 2);
    assert_eq!(github::get_branch_pr(&same).unwrap().unwrap().number, 42);
    assert_eq!(
        graphql_count(),
        2,
        "same selected repository and head share a raw cache key"
    );
    github_budget::invalidate_read_cache(&one).unwrap();
    fs::write(one.join(".discovery-mode"), "partial").unwrap();
    let answers = github::get_workspace_prs_batch(&[one.clone(), two.clone()]);
    assert!(matches!(
        github::workspace_prs_outcome(answers[0].clone()),
        github::WorkspacePrsOutcome::Preserve
    ));
    assert!(matches!(
        github::workspace_prs_outcome(answers[1].clone()),
        github::WorkspacePrsOutcome::Write(_)
    ));
    github_budget::invalidate_read_cache(&one).unwrap();
    fs::write(one.join(".discovery-mode"), "empty").unwrap();
    fs::write(two.join(".discovery-mode"), "empty").unwrap();
    let answers = github::get_workspace_prs_batch(&[one, two]);
    for answer in answers {
        assert!(matches!(
            github::workspace_prs_outcome(answer),
            github::WorkspacePrsOutcome::Clear
        ));
    }
    assert_eq!(
        calls()
            .iter()
            .filter(|args| args.get(1).map(String::as_str) == Some("rate_limit"))
            .count(),
        1
    );
}
