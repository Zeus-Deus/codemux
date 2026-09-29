//! Exercise the real GitHub read paths without contacting GitHub.
//!
//! One test in its own integration binary keeps PATH setup isolated from
//! other tests. Worker threads start only after that setup is complete.
#![cfg(target_os = "linux")]

use codemux_lib::{github, github_budget};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

struct RestorePath(Option<OsString>);

impl Drop for RestorePath {
    fn drop(&mut self) {
        match &self.0 {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
}

struct FakeGh {
    root: tempfile::TempDir,
    log: PathBuf,
    _restore_path: RestorePath,
}

impl FakeGh {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let log = root.path().join("calls");
        fs::write(&log, "").unwrap();
        let script = r#"#!/bin/sh
printf '%s\t%s\n' "$PWD" "$*" >> '@LOG@'
mode=healthy
if [ -f .fake-gh-mode ]; then mode=$(cat .fake-gh-mode); fi
case "$1 $2" in
  'auth token') printf 'fixture-token'; exit 0 ;;
  'auth status') printf 'Logged in to github.com account mock-user\n'; exit 0 ;;
  'config get') printf 'mock-user'; exit 0 ;;
  'api user') printf '{"login":"mock-user"}'; exit 0 ;;
  'api rate_limit')
    core=4999
    graphql=4999
    if [ "$mode" = reserve-core ]; then core=499; fi
    if [ "$mode" = reserve-graphql ]; then graphql=499; fi
    reset=$(( $(date +%s) + 3600 ))
    printf '{"resources":{"core":{"limit":5000,"remaining":%s,"reset":%s},"graphql":{"limit":5000,"remaining":%s,"reset":%s}}}' "$core" "$reset" "$graphql" "$reset"
    exit 0 ;;
  'pr close') exit 0 ;;
esac
if [ "$mode" = refused ]; then
  printf 'API rate limit exceeded: secondary rate limit; retry after 120 seconds\n' >&2
  exit 1
fi
if [ "$mode" = failed ]; then
  printf 'temporary network failure\n' >&2
  exit 1
fi
case "$1 $2" in
  'pr list')
    # Keep a cold request in flight long enough for concurrent callers to meet.
    sleep 0.05
    title=before
    if [ -f .fake-gh-title ]; then title=$(cat .fake-gh-title); fi
    printf '[{"number":42,"url":"https://github.com/mock/repo/pull/42","state":"OPEN","title":"%s","headRefName":"feature","baseRefName":"main","isDraft":false,"author":{"login":"mock-user"},"updatedAt":"2026-09-29T00:00:00Z"}]' "$title"
    exit 0 ;;
  'pr checks')
    printf '[{"name":"build","state":"IN_PROGRESS","bucket":"pending","link":"https://example.test/check"}]'
    exit 8 ;;
  'repo view') printf 'mock/repo'; exit 0 ;;
  'issue view') printf '{"number":1,"title":"Issue","state":"OPEN","url":"https://example.test/issue/1"}'; exit 0 ;;
  'api graphql')
    if [ "$mode" = slow-pages ]; then
      case "$*" in *endCursor=next-page*) exec sleep 30 ;; esac
      sleep 0.1
      printf '{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[],"pageInfo":{"hasNextPage":true,"endCursor":"next-page"}}}}}}'
    elif [ "$mode" = pagination-reserve ]; then
      reset=$(date -u -d '+1 hour' '+%Y-%m-%dT%H:%M:%SZ')
      printf '{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[],"pageInfo":{"hasNextPage":true,"endCursor":"next-page"}}}},"rateLimit":{"cost":1,"limit":5000,"remaining":690,"resetAt":"%s"}}}' "$reset"
    else
      printf '{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[],"pageInfo":{"hasNextPage":false}}}}}}'
    fi
    exit 0 ;;
  'api repos/mock/repo/pulls/42/comments?per_page=100&page=1')
    printf '['
    i=1
    while [ "$i" -le 100 ]; do
      if [ "$i" -gt 1 ]; then printf ','; fi
      printf '{"id":%s,"body":"comment","user":{"login":"mock-user"}}' "$i"
      i=$((i + 1))
    done
    printf ']'
    exit 0 ;;
  'api repos/mock/repo/pulls/42/comments?per_page=100&page=2')
    printf '[{"id":101,"body":"last","user":{"login":"mock-user"}}]'
    exit 0 ;;
esac
printf 'Unexpected fake gh command: %s\n' "$*" >&2
exit 99
"#
            .replace("@LOG@", log.to_str().unwrap());
        let gh = bin.join("gh");
        fs::write(&gh, script).unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let previous = std::env::var_os("PATH");
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(
            previous.as_deref().unwrap_or_default(),
        ));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        Self {
            root,
            log,
            _restore_path: RestorePath(previous),
        }
    }

    fn repo(&self, name: &str, host: &str, mode: &str) -> PathBuf {
        let repo = self.root.path().join(name);
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.test"]);
        git(&repo, &["commit", "--allow-empty", "-m", "fixture"]);
        git(&repo, &["switch", "-c", "feature"]);
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                &format!("https://{host}/mock/repo.git"),
            ],
        );
        git(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        fs::write(repo.join(".fake-gh-mode"), mode).unwrap();
        repo
    }

    fn count(&self, repo: &Path, command: &str) -> usize {
        let prefix = format!("{}\t{command}", repo.display());
        fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with(&prefix))
            .count()
    }

    fn read_count(&self, repo: &Path) -> usize {
        let prefix = format!("{}\t", repo.display());
        fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .filter(|line| {
                line.strip_prefix(&prefix).is_some_and(|args| {
                    args.starts_with("pr list ")
                        || args.starts_with("pr checks ")
                        || args.starts_with("repo view ")
                        || args.starts_with("api graphql ")
                        || args.starts_with("issue view ")
                })
            })
            .count()
    }

    fn command_count(&self, command: &str) -> usize {
        fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .filter(|line| {
                line.split_once('\t')
                    .is_some_and(|(_, args)| args.starts_with(command))
            })
            .count()
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn github_polling_budget_coalesces_reads_and_pauses_every_surface() {
    let fake = FakeGh::new();

    // Authentication polling must remain local. Repeated probes reuse a
    // token/config read rather than validating remotely with auth status.
    for _ in 0..2 {
        match github::check_gh_status() {
            github::GhStatus::Authenticated { username } => assert_eq!(username, "mock-user"),
            status => panic!("unexpected fake authentication status: {status:?}"),
        }
    }
    assert_eq!(fake.command_count("auth token "), 1);
    assert_eq!(fake.command_count("config get "), 1);

    // Duplicate sidebar/workspace readers must share one real CLI request,
    // including when their first calls arrive concurrently.
    let shared = fake.repo("shared", "github-shared.example.test", "healthy");
    let barrier = Arc::new(Barrier::new(8));
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let repo = &shared;
                scope.spawn(move || {
                    barrier.wait();
                    assert_eq!(github::get_branch_pr(repo).unwrap().unwrap().number, 42);
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
    for _ in 0..3 {
        assert_eq!(
            github::get_workspace_pr(&shared).unwrap().unwrap().number,
            42
        );
        assert_eq!(github::get_workspace_prs(&shared).unwrap()[0].pr.number, 42);
    }
    assert_eq!(
        fake.count(&shared, "pr list "),
        1,
        "all branch association callers must share a cached response"
    );
    assert_eq!(
        fake.count(&shared, "api rate_limit"),
        1,
        "concurrent cold callers must share a quota probe"
    );

    // A successful mutation discards the read cache immediately.
    fs::write(shared.join(".fake-gh-title"), "after").unwrap();
    github::close_pull_request(&shared, 42).unwrap();
    assert_eq!(
        github::get_branch_pr(&shared).unwrap().unwrap().title,
        "after"
    );
    assert_eq!(fake.count(&shared, "pr close "), 1);
    assert_eq!(
        fake.count(&shared, "pr list "),
        2,
        "writes must invalidate branch reads"
    );

    // Reserve both API resources before the first PR request, so another
    // application using the same account retains the final 10% of quota.
    for (name, mode) in [
        ("reserve-core", "reserve-core"),
        ("reserve-graphql", "reserve-graphql"),
    ] {
        let repo = fake.repo(name, &format!("github-{name}.example.test"), mode);
        assert!(github::get_branch_pr(&repo).is_err());
        assert!(github::list_prs_overview(&repo).is_err());
        assert_eq!(fake.count(&repo, "api rate_limit"), 1);
        assert_eq!(
            fake.count(&repo, "pr list "),
            0,
            "reserve must stop the request before spawning gh pr list"
        );
    }

    // A refusal in the backend must stop other backend read paths, even
    // though those surfaces have separate frontend query observers.
    let refused = fake.repo("refused", "github-paused.example.test", "refused");
    let sibling = fake.repo("paused-sibling", "github-paused.example.test", "healthy");
    assert!(github::list_prs_overview(&refused).is_err());
    assert_eq!(fake.count(&refused, "pr list "), 1);
    let before = fake.read_count(&refused);
    assert!(github::get_pr_checks(&refused, Some(42)).is_err());
    assert!(github::get_pr_review_threads(&refused, 42).is_err());
    assert!(github::get_github_issue(&refused, 1).is_err());
    assert!(github::list_prs_overview_stats(&refused).is_err());
    assert!(
        github::get_branch_pr(&sibling).is_err(),
        "cooldown must cover other repositories on the same host"
    );
    assert_eq!(
        fake.read_count(&refused),
        before,
        "checks, threads, issues and stats must not spawn during cooldown"
    );
    assert_eq!(fake.read_count(&sibling), 0);

    github_budget::invalidate_read_cache(&refused).unwrap();
    assert!(github::list_prs_overview(&refused).is_err());
    assert_eq!(
        fake.read_count(&refused),
        before,
        "manual refresh must preserve host cooldown"
    );

    // Explicit writes may succeed while passive reads remain paused.
    github::close_pull_request(&refused, 42).unwrap();
    assert!(github::list_prs_overview(&refused).is_err());
    assert_eq!(
        fake.read_count(&refused),
        before,
        "successful writes must not clear a host cooldown"
    );

    // The cooldown is scoped to its host; pending checks on another host
    // still return useful JSON even when gh exits nonzero for their state.
    let independent = fake.repo("independent", "github-independent.example.test", "healthy");
    assert_eq!(github::list_prs_overview(&independent).unwrap().len(), 1);
    let checks = github::get_pr_checks(&independent, Some(42)).unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].status, "IN_PROGRESS");
    assert_eq!(checks[0].conclusion.as_deref(), Some("pending"));
    assert_eq!(fake.count(&independent, "pr checks "), 1);

    // Manual refresh invalidates one repository, including cached data
    // shared with another checkout, without evicting its neighbor's rows.
    let refresh = fake.repo("refresh", "github-refresh.example.test", "healthy");
    let same_repo = fake.repo("refresh-sibling", "github-refresh.example.test", "healthy");
    let other_repo = fake.repo("refresh-other", "github-refresh.example.test", "healthy");
    git(
        &other_repo,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github-refresh.example.test/mock/other.git",
        ],
    );
    assert_eq!(
        github::get_branch_pr(&refresh).unwrap().unwrap().title,
        "before"
    );
    assert_eq!(
        github::get_branch_pr(&same_repo).unwrap().unwrap().title,
        "before"
    );
    assert_eq!(
        fake.count(&same_repo, "pr list "),
        0,
        "sibling checkouts must share explicit branch reads"
    );
    assert_eq!(
        github::get_branch_pr(&other_repo).unwrap().unwrap().title,
        "before"
    );
    fs::write(refresh.join(".fake-gh-title"), "refreshed").unwrap();
    github_budget::invalidate_read_cache(&same_repo).unwrap();
    assert_eq!(
        github::get_branch_pr(&refresh).unwrap().unwrap().title,
        "refreshed"
    );
    assert_eq!(
        github::get_branch_pr(&other_repo).unwrap().unwrap().title,
        "before"
    );
    assert_eq!(fake.count(&refresh, "pr list "), 2);
    assert_eq!(
        fake.count(&other_repo, "pr list "),
        1,
        "refresh must preserve other repository caches"
    );

    let failed = fake.repo("failed", "github-failed.example.test", "failed");
    assert!(github::get_branch_pr(&failed).is_err());
    github_budget::invalidate_read_cache(&failed).unwrap();
    assert!(github::get_branch_pr(&failed).is_err());
    assert_eq!(
        fake.count(&failed, "pr list "),
        1,
        "manual refresh must preserve retry backoff for failed reads"
    );

    // A complete REST pagination combines bounded pages, and every page
    // remains cached when another observer requests the same comments.
    let paginated = fake.repo("paginated", "github-pages.example.test", "healthy");
    let comments = github::get_pr_inline_comments(&paginated, 42).unwrap();
    assert_eq!(comments.len(), 101);
    assert_eq!(comments.last().unwrap().id, 101);
    assert_eq!(fake.count(&paginated, "api repos/"), 2);
    assert_eq!(
        github::get_pr_inline_comments(&paginated, 42)
            .unwrap()
            .len(),
        101
    );
    assert_eq!(
        fake.count(&paginated, "api repos/"),
        2,
        "each REST page must use the read cache"
    );

    // GraphQL pagination must re-admit the next page against actual quota
    // from the preceding response. Return an error rather than an incomplete
    // thread set when completing the read would cross the reserve.
    let pagination_reserve = fake.repo(
        "pagination-reserve",
        "github-page-reserve.example.test",
        "pagination-reserve",
    );
    let error = github::get_pr_review_threads(&pagination_reserve, 42).unwrap_err();
    assert!(
        error.contains("rate limit"),
        "unexpected pagination failure: {error}"
    );
    assert_eq!(
        fake.count(&pagination_reserve, "api graphql "),
        1,
        "next GraphQL page must be stopped before spawning gh"
    );

    // Page deadlines share one operation budget. The second fake process
    // replaces itself with sleep so killing gh also closes its pipes.
    let slow_pages = fake.repo("slow-pages", "github-slow-pages.example.test", "healthy");
    github::list_prs_overview(&slow_pages).unwrap(); // warm local auth and quota
    fs::write(slow_pages.join(".fake-gh-mode"), "slow-pages").unwrap();
    let started = Instant::now();
    let error = github_budget::run(
        &slow_pages,
        &[
            "api", "graphql", "--paginate", "-f", "owner=mock", "-f", "name=repo", "-f",
            "query=query($endCursor:String){repository(owner:\"mock\",name:\"repo\"){pullRequest(number:42){reviewThreads(first:50,after:$endCursor){pageInfo{hasNextPage endCursor}}}}}",
        ],
        Duration::from_millis(350),
    ).unwrap_err();
    assert!(
        error.contains("timed out"),
        "unexpected deadline failure: {error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "pagination exceeded its aggregate deadline: {:?}",
        started.elapsed()
    );
    assert_eq!(fake.count(&slow_pages, "api graphql "), 2);

    assert_eq!(
        fake.command_count("auth status"),
        0,
        "authentication polling must never validate through GitHub"
    );
    assert_eq!(
        fake.command_count("api user"),
        0,
        "local authentication must not fetch the remote user"
    );
}
