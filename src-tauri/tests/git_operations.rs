use codemux_lib::git::*;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

// ── Helpers ──

fn run_git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .expect("failed to run git");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("git {:?} failed: {}", args, stderr.trim());
    }
    String::from_utf8_lossy(&output.stdout).trim_end().to_string()
}

fn create_test_repo() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("create temp dir");
    let path = dir.path().to_path_buf();
    run_git(&path, &["init"]);
    run_git(&path, &[
        "-c", "user.name=Test",
        "-c", "user.email=test@test.com",
        "commit", "--allow-empty", "-m", "initial",
    ]);
    (dir, path)
}

fn create_test_repo_with_remote() -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new().expect("create temp dir");
    let remote = dir.path().join("remote.git");
    let local = dir.path().join("local");

    run_git(dir.path(), &["init", "--bare", remote.to_str().unwrap()]);
    run_git(dir.path(), &["clone", remote.to_str().unwrap(), local.to_str().unwrap()]);

    std::fs::write(local.join("init.txt"), "initial").expect("write");
    run_git(&local, &["add", "init.txt"]);
    commit(&local, "initial");
    run_git(&local, &["push"]);

    (dir, local, remote)
}

fn commit(path: &Path, msg: &str) {
    run_git(path, &[
        "-c", "user.name=Test",
        "-c", "user.email=test@test.com",
        "commit", "-m", msg,
    ]);
}

fn write_file(repo: &Path, name: &str, content: &str) {
    std::fs::write(repo.join(name), content).expect("write file");
}

fn add_and_commit(repo: &Path, name: &str, content: &str, msg: &str) {
    write_file(repo, name, content);
    run_git(repo, &["add", name]);
    commit(repo, msg);
}

// ═══════════════════════════════════════
// git_status tests
// ═══════════════════════════════════════

#[test]
fn status_clean_repo_returns_empty() {
    let (_dir, repo) = create_test_repo();
    let status = git_status(&repo).expect("status");
    assert!(status.is_empty(), "clean repo should have no changes");
}

#[test]
fn status_modified_file_shows_modified() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "file.txt", "original", "add file");
    write_file(&repo, "file.txt", "modified");

    let status = git_status(&repo).expect("status");
    let f = status.iter().find(|s| s.path == "file.txt").expect("file in status");
    assert_eq!(f.status, FileStatus::Modified);
    assert!(f.is_unstaged);
    assert!(!f.is_staged);
}

#[test]
fn status_new_file_shows_untracked() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "new.txt", "content");

    let status = git_status(&repo).expect("status");
    let f = status.iter().find(|s| s.path == "new.txt").expect("file in status");
    assert_eq!(f.status, FileStatus::Untracked);
    assert!(f.is_unstaged);
    assert!(!f.is_staged);
}

#[test]
fn status_deleted_file_shows_deleted() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "gone.txt", "content", "add file");
    std::fs::remove_file(repo.join("gone.txt")).expect("delete");

    let status = git_status(&repo).expect("status");
    let f = status.iter().find(|s| s.path == "gone.txt").expect("file in status");
    assert_eq!(f.status, FileStatus::Deleted);
    assert!(f.is_unstaged);
}

#[test]
fn status_staged_file_in_staged_section() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "staged.txt", "content");
    run_git(&repo, &["add", "staged.txt"]);

    let status = git_status(&repo).expect("status");
    let f = status.iter().find(|s| s.path == "staged.txt").expect("file in status");
    assert!(f.is_staged, "should be staged");
    assert!(!f.is_unstaged, "should not be unstaged");
}

#[test]
fn status_mixed_staged_and_unstaged() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "both.txt", "v1", "add");

    // Stage a modification
    write_file(&repo, "both.txt", "v2");
    run_git(&repo, &["add", "both.txt"]);

    // Modify again without staging
    write_file(&repo, "both.txt", "v3");

    let status = git_status(&repo).expect("status");
    let f = status.iter().find(|s| s.path == "both.txt").expect("file in status");
    assert!(f.is_staged, "should be staged");
    assert!(f.is_unstaged, "should also be unstaged");
}

#[test]
fn status_includes_additions_deletions() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "counted.txt", "line1\nline2\nline3\n", "add");

    // Modify: remove 2 lines, add 1
    write_file(&repo, "counted.txt", "line1\nnew-line\n");

    let status = git_status(&repo).expect("status");
    let f = status.iter().find(|s| s.path == "counted.txt").expect("file in status");
    // Should have some additions and deletions
    assert!(f.additions > 0 || f.deletions > 0, "should have diff stats, got +{} -{}", f.additions, f.deletions);
}

// ═══════════════════════════════════════
// git_stage / git_unstage tests
// ═══════════════════════════════════════

#[test]
fn stage_modified_file() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "mod.txt", "original", "add");
    write_file(&repo, "mod.txt", "changed");

    git_stage(&repo, &["mod.txt".to_string()]).expect("stage");

    let status = git_status(&repo).expect("status");
    let f = status.iter().find(|s| s.path == "mod.txt").expect("file");
    assert!(f.is_staged);
}

#[test]
fn stage_untracked_file() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "new.txt", "content");

    git_stage(&repo, &["new.txt".to_string()]).expect("stage");

    let status = git_status(&repo).expect("status");
    let f = status.iter().find(|s| s.path == "new.txt").expect("file");
    assert!(f.is_staged);
    assert_eq!(f.status, FileStatus::Added);
}

#[test]
fn stage_already_staged_is_idempotent() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "idem.txt", "content");
    git_stage(&repo, &["idem.txt".to_string()]).expect("stage 1");
    git_stage(&repo, &["idem.txt".to_string()]).expect("stage 2 should not error");

    let status = git_status(&repo).expect("status");
    let count = status.iter().filter(|s| s.path == "idem.txt").count();
    assert_eq!(count, 1, "should appear exactly once");
}

#[test]
fn unstage_moves_back() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "back.txt", "content");
    git_stage(&repo, &["back.txt".to_string()]).expect("stage");
    git_unstage(&repo, &["back.txt".to_string()]).expect("unstage");

    let status = git_status(&repo).expect("status");
    let f = status.iter().find(|s| s.path == "back.txt").expect("file");
    assert!(!f.is_staged);
    assert!(f.is_unstaged);
}

#[test]
fn unstage_unstaged_is_idempotent() {
    let (_dir, repo) = create_test_repo();
    // Stage a file first, then unstage it twice
    write_file(&repo, "notstaged.txt", "content");
    git_stage(&repo, &["notstaged.txt".to_string()]).expect("stage");
    git_unstage(&repo, &["notstaged.txt".to_string()]).expect("unstage 1");

    // Second unstage on a tracked-but-unstaged file should not error
    // Note: for truly untracked files, git restore --staged fails, so we test
    // the idempotent case with a file that was previously staged
    add_and_commit(&repo, "tracked.txt", "content", "add tracked");
    write_file(&repo, "tracked.txt", "modified");
    // Don't stage — just try to unstage an unstaged modification
    let result = git_unstage(&repo, &["tracked.txt".to_string()]);
    assert!(result.is_ok(), "unstaging an unstaged tracked file should not error");
}

#[test]
fn stage_multiple_files() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "a.txt", "a");
    write_file(&repo, "b.txt", "b");

    git_stage(&repo, &["a.txt".to_string(), "b.txt".to_string()]).expect("stage");

    let status = git_status(&repo).expect("status");
    assert!(status.iter().any(|s| s.path == "a.txt" && s.is_staged));
    assert!(status.iter().any(|s| s.path == "b.txt" && s.is_staged));
}

#[test]
fn unstage_multiple_files() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "x.txt", "x");
    write_file(&repo, "y.txt", "y");
    git_stage(&repo, &["x.txt".to_string(), "y.txt".to_string()]).expect("stage");
    git_unstage(&repo, &["x.txt".to_string(), "y.txt".to_string()]).expect("unstage");

    let status = git_status(&repo).expect("status");
    for f in &status {
        if f.path == "x.txt" || f.path == "y.txt" {
            assert!(!f.is_staged, "{} should not be staged", f.path);
        }
    }
}

// ═══════════════════════════════════════
// git_discard_file tests
// ═══════════════════════════════════════

#[test]
fn discard_tracked_restores_content() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "tracked.txt", "original", "add");
    write_file(&repo, "tracked.txt", "modified");

    git_discard_file(&repo, "tracked.txt").expect("discard");

    let content = std::fs::read_to_string(repo.join("tracked.txt")).expect("read");
    assert_eq!(content, "original");
}

#[test]
fn discard_untracked_removes_file() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "untracked.txt", "temp");
    assert!(repo.join("untracked.txt").exists());

    git_discard_file(&repo, "untracked.txt").expect("discard");
    assert!(!repo.join("untracked.txt").exists());
}

// ═══════════════════════════════════════
// git_commit tests
// ═══════════════════════════════════════

#[test]
fn commit_creates_entry_with_message() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "committed.txt", "content");
    git_stage(&repo, &["committed.txt".to_string()]).expect("stage");
    git_commit(&repo, "test commit message").expect("commit");

    let log = run_git(&repo, &["log", "--oneline", "-1"]);
    assert!(log.contains("test commit message"));
}

#[test]
fn commit_clears_staged_files() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "cleared.txt", "content");
    git_stage(&repo, &["cleared.txt".to_string()]).expect("stage");
    git_commit(&repo, "clear test").expect("commit");

    let status = git_status(&repo).expect("status");
    assert!(status.is_empty(), "should be clean after commit");
}

#[test]
fn commit_with_nothing_staged_fails() {
    let (_dir, repo) = create_test_repo();
    let result = git_commit(&repo, "nothing to commit");
    assert!(result.is_err(), "commit with nothing staged should fail");
}

#[test]
fn commit_with_empty_message_fails() {
    let (_dir, repo) = create_test_repo();
    write_file(&repo, "empty-msg.txt", "content");
    git_stage(&repo, &["empty-msg.txt".to_string()]).expect("stage");

    let result = git_commit(&repo, "");
    assert!(result.is_err(), "commit with empty message should fail");
}

// ═══════════════════════════════════════
// git_diff tests
// ═══════════════════════════════════════

#[test]
fn diff_shows_modifications() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "diff.txt", "line1\nline2\n", "add");
    write_file(&repo, "diff.txt", "line1\nchanged\nline3\n");

    let diff = git_diff(&repo, "diff.txt", false).expect("diff");
    assert!(diff.contains("+changed") || diff.contains("+line3"), "diff should show additions: {}", diff);
    assert!(diff.contains("-line2"), "diff should show deletions: {}", diff);
}

#[test]
fn diff_staged_vs_unstaged() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "sv.txt", "original\n", "add");

    // Stage a change
    write_file(&repo, "sv.txt", "staged-change\n");
    run_git(&repo, &["add", "sv.txt"]);

    // Make another unstaged change
    write_file(&repo, "sv.txt", "unstaged-change\n");

    let staged_diff = git_diff(&repo, "sv.txt", true).expect("staged diff");
    let unstaged_diff = git_diff(&repo, "sv.txt", false).expect("unstaged diff");

    assert!(staged_diff.contains("staged-change"), "staged diff: {}", staged_diff);
    assert!(unstaged_diff.contains("unstaged-change"), "unstaged diff: {}", unstaged_diff);
    assert_ne!(staged_diff, unstaged_diff, "staged and unstaged diffs should differ");
}

#[test]
fn diff_unchanged_file_returns_empty() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "clean.txt", "content", "add");

    let diff = git_diff(&repo, "clean.txt", false).expect("diff");
    assert!(diff.is_empty(), "unchanged file should have empty diff, got: {}", diff);
}

// ═══════════════════════════════════════
// git_diff_stat tests
// ═══════════════════════════════════════

#[test]
fn diff_stat_counts_staged_and_unstaged() {
    let (_dir, repo) = create_test_repo();
    // Create two tracked files
    add_and_commit(&repo, "stat.txt", "line1\nline2\nline3\n", "add stat");
    add_and_commit(&repo, "stat2.txt", "old\n", "add stat2");

    // Stage a change to stat.txt: replace 3 lines with 2
    write_file(&repo, "stat.txt", "new1\nnew2\n");
    run_git(&repo, &["add", "stat.txt"]);

    // Make an unstaged change to stat2.txt
    write_file(&repo, "stat2.txt", "new\nextra\n");

    let stat = git_diff_stat(&repo).expect("diff stat");
    assert!(stat.staged_additions > 0, "staged additions: {}", stat.staged_additions);
    assert!(stat.staged_deletions > 0, "staged deletions: {}", stat.staged_deletions);
    assert!(stat.unstaged_additions > 0, "unstaged additions: {}", stat.unstaged_additions);
}

#[test]
fn diff_stat_clean_repo_all_zeros() {
    let (_dir, repo) = create_test_repo();
    let stat = git_diff_stat(&repo).expect("diff stat");
    assert_eq!(stat.staged_additions, 0);
    assert_eq!(stat.staged_deletions, 0);
    assert_eq!(stat.unstaged_additions, 0);
    assert_eq!(stat.unstaged_deletions, 0);
}

// ═══════════════════════════════════════
// git_push tests
// ═══════════════════════════════════════

#[test]
fn push_to_remote_succeeds() {
    let (_dir, local, remote) = create_test_repo_with_remote();

    add_and_commit(&local, "pushed.txt", "push me", "push test");
    git_push(&local, false).expect("push");

    // Verify by cloning remote
    let verify = _dir.path().join("verify");
    run_git(_dir.path(), &["clone", remote.to_str().unwrap(), verify.to_str().unwrap()]);
    let log = run_git(&verify, &["log", "--oneline"]);
    assert!(log.contains("push test"));
}

#[test]
fn push_set_upstream_publishes_branch() {
    let (_dir, local, _remote) = create_test_repo_with_remote();

    run_git(&local, &["checkout", "-b", "new-branch"]);
    add_and_commit(&local, "feat.txt", "feature", "feature commit");

    let info = git_branch_info(&local).expect("info before push");
    assert!(!info.has_upstream);

    git_push(&local, true).expect("push with upstream");

    let info = git_branch_info(&local).expect("info after push");
    assert!(info.has_upstream);
}

#[test]
fn push_with_nothing_is_idempotent() {
    let (_dir, local, _remote) = create_test_repo_with_remote();
    // Already pushed everything in setup — push again
    let result = git_push(&local, false);
    assert!(result.is_ok(), "push with nothing should succeed: {:?}", result.err());
}

// ═══════════════════════════════════════
// git_pull tests
// ═══════════════════════════════════════

#[test]
fn pull_fetches_new_commits() {
    let (_dir, local, remote) = create_test_repo_with_remote();

    // Push from a second clone
    let clone2 = _dir.path().join("clone2");
    run_git(_dir.path(), &["clone", remote.to_str().unwrap(), clone2.to_str().unwrap()]);
    add_and_commit(&clone2, "from-clone2.txt", "clone2", "clone2 commit");
    run_git(&clone2, &["push"]);

    git_pull(&local).expect("pull");

    assert!(local.join("from-clone2.txt").exists());
    let log = run_git(&local, &["log", "--oneline"]);
    assert!(log.contains("clone2 commit"));
}

#[test]
fn pull_with_no_changes_is_idempotent() {
    let (_dir, local, _remote) = create_test_repo_with_remote();
    let result = git_pull(&local);
    assert!(result.is_ok(), "pull with nothing should succeed: {:?}", result.err());
}

// ═══════════════════════════════════════
// git_log tests
// ═══════════════════════════════════════

#[test]
fn log_returns_correct_count() {
    let (_dir, repo) = create_test_repo();
    for i in 1..=3 {
        add_and_commit(&repo, &format!("f{i}.txt"), &format!("c{i}"), &format!("commit {i}"));
    }

    let entries = git_log(&repo, 10).expect("log");
    assert_eq!(entries.len(), 4, "initial + 3 commits");
}

#[test]
fn log_entries_have_metadata() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "meta.txt", "content", "metadata test");

    let entries = git_log(&repo, 5).expect("log");
    let first = &entries[0];
    assert!(!first.hash.is_empty(), "hash should not be empty");
    assert!(!first.short_hash.is_empty(), "short_hash should not be empty");
    assert_eq!(first.message, "metadata test");
    assert_eq!(first.author, "Test");
    assert!(!first.time_ago.is_empty(), "time_ago should not be empty");
}

#[test]
fn log_most_recent_first() {
    let (_dir, repo) = create_test_repo();
    add_and_commit(&repo, "a.txt", "a", "first");
    add_and_commit(&repo, "b.txt", "b", "second");

    let entries = git_log(&repo, 10).expect("log");
    assert_eq!(entries[0].message, "second");
    assert_eq!(entries[1].message, "first");
}

#[test]
fn log_respects_limit() {
    let (_dir, repo) = create_test_repo();
    for i in 1..=5 {
        add_and_commit(&repo, &format!("lim{i}.txt"), "c", &format!("c{i}"));
    }

    let entries = git_log(&repo, 2).expect("log with limit");
    assert_eq!(entries.len(), 2, "should respect limit=2");
    assert_eq!(entries[0].message, "c5");
    assert_eq!(entries[1].message, "c4");
}

// ═══════════════════════════════════════
// git_branch_info tests
// ═══════════════════════════════════════

#[test]
fn branch_info_with_upstream_ahead_behind() {
    let (_dir, local, _remote) = create_test_repo_with_remote();
    add_and_commit(&local, "ahead.txt", "ahead", "unpushed");

    let info = git_branch_info(&local).expect("info");
    assert!(info.has_upstream);
    assert_eq!(info.ahead, 1);
    assert_eq!(info.behind, 0);
}

#[test]
fn branch_info_no_upstream() {
    let (_dir, repo) = create_test_repo();
    let info = git_branch_info(&repo).expect("info");
    assert!(!info.has_upstream);
    assert_eq!(info.ahead, 0);
    assert_eq!(info.behind, 0);
}

#[test]
fn branch_info_has_branch_name() {
    let (_dir, repo) = create_test_repo();
    let info = git_branch_info(&repo).expect("info");
    assert!(info.branch.is_some(), "should have a branch name");
}

// ═══════════════════════════════════════
// git_list_branches tests
// ═══════════════════════════════════════

#[test]
fn list_branches_includes_created() {
    let (_dir, repo) = create_test_repo();
    run_git(&repo, &["branch", "feature-a"]);
    run_git(&repo, &["branch", "feature-b"]);

    let branches = git_list_branches(&repo, false).expect("list");
    assert!(branches.contains(&"feature-a".to_string()));
    assert!(branches.contains(&"feature-b".to_string()));
}

// ═══════════════════════════════════════
// Worktree independence tests
// ═══════════════════════════════════════

#[test]
fn worktree_operations_independent() {
    let (_dir, repo) = create_test_repo();
    let wt_path_str = git_create_worktree(&repo, "wt-indep", true, None).expect("create wt");
    let wt_path = PathBuf::from(&wt_path_str);

    // Commit in worktree
    write_file(&wt_path, "wt-only.txt", "worktree content");
    run_git(&wt_path, &["add", "wt-only.txt"]);
    run_git(&wt_path, &[
        "-c", "user.name=Test", "-c", "user.email=test@test.com",
        "commit", "-m", "wt commit",
    ]);

    // Main repo status should still be clean
    let main_status = git_status(&repo).expect("main status");
    assert!(main_status.is_empty(), "main repo should be clean after wt commit");

    git_remove_worktree(Path::new(&wt_path_str), Some("wt-indep")).expect("cleanup");
}

#[test]
fn worktree_status_isolated() {
    let (_dir, repo) = create_test_repo();
    let wt_path_str = git_create_worktree(&repo, "wt-iso", true, None).expect("create wt");
    let wt_path = PathBuf::from(&wt_path_str);

    // Create untracked file in worktree
    write_file(&wt_path, "wt-file.txt", "wt content");
    // Create untracked file in main
    write_file(&repo, "main-file.txt", "main content");

    let wt_status = git_status(&wt_path).expect("wt status");
    let main_status = git_status(&repo).expect("main status");

    // Worktree should see wt-file but not main-file
    assert!(wt_status.iter().any(|s| s.path == "wt-file.txt"), "wt should see wt-file");
    assert!(!wt_status.iter().any(|s| s.path == "main-file.txt"), "wt should not see main-file");

    // Main should see main-file but not wt-file
    assert!(main_status.iter().any(|s| s.path == "main-file.txt"), "main should see main-file");
    assert!(!main_status.iter().any(|s| s.path == "wt-file.txt"), "main should not see wt-file");

    git_remove_worktree(Path::new(&wt_path_str), Some("wt-iso")).expect("cleanup");
}

#[test]
fn worktree_commit_not_in_other_log() {
    let (_dir, repo) = create_test_repo();
    let wt_path_str = git_create_worktree(&repo, "wt-log", true, None).expect("create wt");
    let wt_path = PathBuf::from(&wt_path_str);

    // Commit in worktree
    write_file(&wt_path, "wt-log.txt", "content");
    run_git(&wt_path, &["add", "wt-log.txt"]);
    run_git(&wt_path, &[
        "-c", "user.name=Test", "-c", "user.email=test@test.com",
        "commit", "-m", "wt-only-commit",
    ]);

    // Main repo log should NOT contain the worktree commit (different branch)
    let main_log = git_log(&repo, 10).expect("main log");
    assert!(
        !main_log.iter().any(|e| e.message == "wt-only-commit"),
        "main log should not contain worktree's commit"
    );

    // Worktree log should contain it
    let wt_log = git_log(&wt_path, 10).expect("wt log");
    assert!(
        wt_log.iter().any(|e| e.message == "wt-only-commit"),
        "worktree log should contain its own commit"
    );

    git_remove_worktree(Path::new(&wt_path_str), Some("wt-log")).expect("cleanup");
}
