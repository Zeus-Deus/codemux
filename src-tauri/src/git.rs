use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Copied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileStatus {
    pub path: String,
    pub status: FileStatus,
    pub is_staged: bool,
    pub is_unstaged: bool,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiffStat {
    pub staged_additions: u32,
    pub staged_deletions: u32,
    pub unstaged_additions: u32,
    pub unstaged_deletions: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitBranchInfo {
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLogEntry {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub author: String,
    pub time_ago: String,
}

fn run_git(repo_path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

/// Run git and return stdout even on non-zero exit (for commands where failure is expected).
fn run_git_permissive(repo_path: &Path, args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
        .unwrap_or_default()
}

pub fn git_status(repo_path: &Path) -> Result<Vec<GitFileStatus>, String> {
    let output = run_git(repo_path, &["status", "--porcelain=v1"])?;
    let mut files = parse_porcelain_status(&output);

    // Merge per-file diff stats
    let unstaged_stats = run_git_permissive(repo_path, &["diff", "--numstat"]);
    let staged_stats = run_git_permissive(repo_path, &["diff", "--cached", "--numstat"]);
    let per_file = parse_numstat_per_file(&unstaged_stats, &staged_stats);
    for file in &mut files {
        if let Some(&(a, d)) = per_file.get(&file.path) {
            file.additions = a;
            file.deletions = d;
        }
    }
    Ok(files)
}

pub fn git_diff(repo_path: &Path, file_path: &str, staged: bool) -> Result<String, String> {
    if staged {
        run_git(repo_path, &["diff", "--cached", "--", file_path])
    } else {
        run_git(repo_path, &["diff", "--", file_path])
    }
}

pub fn git_diff_stat(repo_path: &Path) -> Result<GitDiffStat, String> {
    let unstaged = run_git(repo_path, &["diff", "--numstat"])?;
    let staged = run_git(repo_path, &["diff", "--cached", "--numstat"])?;
    let (unstaged_additions, unstaged_deletions) = parse_numstat(&unstaged);
    let (staged_additions, staged_deletions) = parse_numstat(&staged);
    Ok(GitDiffStat {
        staged_additions,
        staged_deletions,
        unstaged_additions,
        unstaged_deletions,
    })
}

pub fn git_stage(repo_path: &Path, files: &[String]) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let mut args: Vec<&str> = vec!["add", "--"];
    args.extend(files.iter().map(|f| f.as_str()));
    run_git(repo_path, &args)?;
    Ok(())
}

pub fn git_unstage(repo_path: &Path, files: &[String]) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let mut args: Vec<&str> = vec!["restore", "--staged", "--"];
    args.extend(files.iter().map(|f| f.as_str()));
    if run_git(repo_path, &args).is_err() {
        // Fallback for newly added files on older git versions
        let mut rm_args: Vec<&str> = vec!["rm", "--cached", "--"];
        rm_args.extend(files.iter().map(|f| f.as_str()));
        run_git(repo_path, &rm_args)?;
    }
    Ok(())
}

pub fn git_commit(repo_path: &Path, message: &str) -> Result<(), String> {
    run_git(repo_path, &["commit", "-m", message])?;
    Ok(())
}

pub fn git_push(repo_path: &Path) -> Result<(), String> {
    run_git(repo_path, &["push"])?;
    Ok(())
}

pub fn git_pull(repo_path: &Path) -> Result<(), String> {
    run_git(repo_path, &["pull", "--rebase"])?;
    Ok(())
}

pub fn git_discard_file(repo_path: &Path, file: &str) -> Result<(), String> {
    // Try git restore first (works for tracked files)
    let restore = run_git(repo_path, &["restore", "--", file]);
    if restore.is_ok() {
        return Ok(());
    }
    // For untracked files, try clean
    run_git(repo_path, &["clean", "-f", "--", file])?;
    Ok(())
}

pub fn git_log(repo_path: &Path, count: usize) -> Result<Vec<GitLogEntry>, String> {
    let count_str = count.to_string();
    let output = run_git(
        repo_path,
        &["log", "--format=%H%n%h%n%s%n%an%n%ar", "-n", &count_str],
    )?;
    let lines: Vec<&str> = output.lines().collect();
    let mut entries = Vec::new();
    for chunk in lines.chunks(5) {
        if chunk.len() < 5 {
            break;
        }
        entries.push(GitLogEntry {
            hash: chunk[0].to_string(),
            short_hash: chunk[1].to_string(),
            message: chunk[2].to_string(),
            author: chunk[3].to_string(),
            time_ago: chunk[4].to_string(),
        });
    }
    Ok(entries)
}

pub fn git_branch_info(repo_path: &Path) -> Result<GitBranchInfo, String> {
    let branch_name = run_git_permissive(repo_path, &["branch", "--show-current"]);
    let branch = if branch_name.is_empty() {
        None
    } else {
        Some(branch_name)
    };

    let rev_list = run_git_permissive(
        repo_path,
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    );
    let (ahead, behind) = parse_ahead_behind(&rev_list);

    Ok(GitBranchInfo {
        branch,
        ahead,
        behind,
    })
}

// ---- Parsing helpers ----

fn parse_porcelain_status(output: &str) -> Vec<GitFileStatus> {
    let mut results = Vec::new();
    for line in output.lines() {
        if line.len() < 4 {
            continue;
        }
        let index_status = line.as_bytes()[0];
        let worktree_status = line.as_bytes()[1];
        let path_part = &line[3..];

        // For renames/copies, path is "old -> new"; use the new path
        let path = if let Some(arrow_pos) = path_part.find(" -> ") {
            path_part[arrow_pos + 4..].to_string()
        } else {
            path_part.to_string()
        };

        let status = match (index_status, worktree_status) {
            (b'?', b'?') => FileStatus::Untracked,
            (b'A', _) => FileStatus::Added,
            (b'R', _) => FileStatus::Renamed,
            (b'C', _) => FileStatus::Copied,
            (b'D', _) | (_, b'D') => FileStatus::Deleted,
            (b'M', _) | (_, b'M') => FileStatus::Modified,
            _ => FileStatus::Modified,
        };

        // X column: staged status (anything except ' ' and '?' means staged)
        let is_staged = index_status != b' ' && index_status != b'?';
        // Y column: unstaged status (anything except ' ' means unstaged; '?' = untracked = unstaged)
        let is_unstaged = worktree_status != b' ';

        results.push(GitFileStatus { path, status, is_staged, is_unstaged, additions: 0, deletions: 0 });
    }
    results
}

fn parse_numstat_per_file(unstaged: &str, staged: &str) -> std::collections::HashMap<String, (u32, u32)> {
    let mut map = std::collections::HashMap::new();
    for line in unstaged.lines().chain(staged.lines()) {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let adds = parts[0].parse::<u32>().unwrap_or(0);
            let dels = parts[1].parse::<u32>().unwrap_or(0);
            let path = parts[2].to_string();
            let entry = map.entry(path).or_insert((0, 0));
            entry.0 += adds;
            entry.1 += dels;
        }
    }
    map
}

fn parse_numstat(output: &str) -> (u32, u32) {
    let mut total_add = 0u32;
    let mut total_del = 0u32;
    for line in output.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            // Binary files show "-" for additions/deletions
            total_add += parts[0].parse::<u32>().unwrap_or(0);
            total_del += parts[1].parse::<u32>().unwrap_or(0);
        }
    }
    (total_add, total_del)
}

// ---- Worktree operations ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    pub is_bare: bool,
}

pub fn git_list_branches(repo_path: &Path, remote: bool) -> Result<Vec<String>, String> {
    let output = if remote {
        run_git(repo_path, &["branch", "-r", "--format=%(refname:short)"])?
    } else {
        run_git(repo_path, &["branch", "--format=%(refname:short)"])?
    };
    let branches: Vec<String> = output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.contains("HEAD"))
        .map(|l| {
            if remote {
                l.strip_prefix("origin/").unwrap_or(l).to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    Ok(branches)
}

pub fn git_create_worktree(
    repo_path: &Path,
    branch: &str,
    new_branch: bool,
    base: Option<&str>,
) -> Result<String, String> {
    let repo_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());
    let sanitized_branch = branch.replace('/', "-");
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let worktree_path = PathBuf::from(&home)
        .join(".codemux")
        .join("worktrees")
        .join(&repo_name)
        .join(&sanitized_branch);

    // Ensure parent directory exists
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create worktree directory: {e}"))?;
    }

    let path_str = worktree_path.to_string_lossy().to_string();

    if new_branch {
        let mut args = vec!["worktree", "add", "-b", branch, &path_str];
        if let Some(b) = base {
            args.push(b);
        }
        run_git(repo_path, &args)?;
    } else {
        run_git(repo_path, &["worktree", "add", &path_str, branch])?;
    }

    Ok(path_str)
}

pub fn git_remove_worktree(worktree_path: &Path, branch: Option<&str>) -> Result<(), String> {
    // Find the main repo by reading .git file in worktree
    let git_file = worktree_path.join(".git");
    let repo_path = if git_file.is_file() {
        let content = std::fs::read_to_string(&git_file)
            .map_err(|e| format!("Failed to read .git file: {e}"))?;
        // Content is "gitdir: /path/to/main/.git/worktrees/<name>"
        let gitdir = content
            .strip_prefix("gitdir: ")
            .unwrap_or(&content)
            .trim();
        // Go up from .git/worktrees/<name> to the repo root
        PathBuf::from(gitdir)
            .parent() // worktrees/
            .and_then(|p| p.parent()) // .git/
            .and_then(|p| p.parent()) // repo root
            .unwrap_or(worktree_path)
            .to_path_buf()
    } else {
        worktree_path.to_path_buf()
    };

    run_git(
        &repo_path,
        &["worktree", "remove", &worktree_path.to_string_lossy(), "--force"],
    )?;

    // Delete the branch if requested (skip main/master and the repo's current branch)
    if let Some(branch_name) = branch {
        let protected = ["main", "master"];
        if !protected.contains(&branch_name) {
            let current = run_git_permissive(&repo_path, &["branch", "--show-current"]);
            if current != branch_name {
                // Best-effort: don't fail the whole operation if branch deletion fails
                let _ = run_git(&repo_path, &["branch", "-D", branch_name]);
            }
        }
    }

    Ok(())
}

pub fn git_list_worktrees(repo_path: &Path) -> Result<Vec<WorktreeInfo>, String> {
    let output = run_git(repo_path, &["worktree", "list", "--porcelain"])?;
    let mut worktrees = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            // Save previous entry
            if let Some(p) = current_path.take() {
                worktrees.push(WorktreeInfo {
                    path: p,
                    branch: current_branch.take(),
                    is_bare,
                });
            }
            current_path = Some(path.to_string());
            current_branch = None;
            is_bare = false;
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(branch.to_string());
        } else if line == "bare" {
            is_bare = true;
        }
    }
    // Save last entry
    if let Some(p) = current_path {
        worktrees.push(WorktreeInfo {
            path: p,
            branch: current_branch,
            is_bare,
        });
    }

    Ok(worktrees)
}

fn parse_ahead_behind(output: &str) -> (u32, u32) {
    let parts: Vec<&str> = output.split_whitespace().collect();
    if parts.len() == 2 {
        let ahead = parts[0].parse::<u32>().unwrap_or(0);
        let behind = parts[1].parse::<u32>().unwrap_or(0);
        (ahead, behind)
    } else {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_porcelain_status_handles_common_statuses() {
        let input = "\
?? untracked.txt
A  staged-new.txt
M  modified-staged.txt
 M modified-unstaged.txt
MM modified-both.txt
D  deleted.txt
 D deleted-unstaged.txt
R  old-name.txt -> new-name.txt
C  source.txt -> copy.txt";

        let results = parse_porcelain_status(input);
        assert_eq!(results.len(), 9);

        // ?? untracked.txt → unstaged only
        assert_eq!(results[0].path, "untracked.txt");
        assert_eq!(results[0].status, FileStatus::Untracked);
        assert!(!results[0].is_staged);
        assert!(results[0].is_unstaged);

        // A  staged-new.txt → staged only
        assert_eq!(results[1].path, "staged-new.txt");
        assert_eq!(results[1].status, FileStatus::Added);
        assert!(results[1].is_staged);
        assert!(!results[1].is_unstaged);

        // M  modified-staged.txt → staged only
        assert_eq!(results[2].path, "modified-staged.txt");
        assert_eq!(results[2].status, FileStatus::Modified);
        assert!(results[2].is_staged);
        assert!(!results[2].is_unstaged);

        // ' M' modified-unstaged.txt → unstaged only
        assert_eq!(results[3].path, "modified-unstaged.txt");
        assert_eq!(results[3].status, FileStatus::Modified);
        assert!(!results[3].is_staged);
        assert!(results[3].is_unstaged);

        // MM modified-both.txt → both staged and unstaged
        assert_eq!(results[4].path, "modified-both.txt");
        assert_eq!(results[4].status, FileStatus::Modified);
        assert!(results[4].is_staged);
        assert!(results[4].is_unstaged);

        // D  deleted.txt → staged only
        assert_eq!(results[5].path, "deleted.txt");
        assert_eq!(results[5].status, FileStatus::Deleted);
        assert!(results[5].is_staged);
        assert!(!results[5].is_unstaged);

        // ' D' deleted-unstaged.txt → unstaged only
        assert_eq!(results[6].path, "deleted-unstaged.txt");
        assert_eq!(results[6].status, FileStatus::Deleted);
        assert!(!results[6].is_staged);
        assert!(results[6].is_unstaged);

        // R  old-name.txt -> new-name.txt → staged only
        assert_eq!(results[7].path, "new-name.txt");
        assert_eq!(results[7].status, FileStatus::Renamed);
        assert!(results[7].is_staged);
        assert!(!results[7].is_unstaged);

        // C  source.txt -> copy.txt → staged only
        assert_eq!(results[8].path, "copy.txt");
        assert_eq!(results[8].status, FileStatus::Copied);
        assert!(results[8].is_staged);
        assert!(!results[8].is_unstaged);
    }

    #[test]
    fn parse_porcelain_status_handles_empty_output() {
        let results = parse_porcelain_status("");
        assert!(results.is_empty());
    }

    #[test]
    fn parse_numstat_sums_additions_and_deletions() {
        let input = "\
10\t5\tsrc/main.rs
3\t0\tsrc/lib.rs
-\t-\tbinary-file.png
0\t20\told-file.rs";

        let (adds, dels) = parse_numstat(input);
        assert_eq!(adds, 13);
        assert_eq!(dels, 25);
    }

    #[test]
    fn parse_numstat_handles_empty_output() {
        let (adds, dels) = parse_numstat("");
        assert_eq!(adds, 0);
        assert_eq!(dels, 0);
    }

    #[test]
    fn parse_ahead_behind_extracts_counts() {
        assert_eq!(parse_ahead_behind("3\t2"), (3, 2));
        assert_eq!(parse_ahead_behind("0\t5"), (0, 5));
        assert_eq!(parse_ahead_behind("12\t0"), (12, 0));
    }

    #[test]
    fn parse_ahead_behind_defaults_on_bad_input() {
        assert_eq!(parse_ahead_behind(""), (0, 0));
        assert_eq!(parse_ahead_behind("error"), (0, 0));
    }

    #[test]
    fn parse_porcelain_status_handles_paths_with_spaces() {
        let input = "M  path with spaces/file name.txt\n?? another file.txt";
        let results = parse_porcelain_status(input);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].path, "path with spaces/file name.txt");
        assert_eq!(results[1].path, "another file.txt");
    }

    // ---- Integration tests (real git repos) ----

    use tempfile::TempDir;

    fn setup_test_repo() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("create temp dir");
        let path = dir.path().to_path_buf();
        run_git(&path, &["init"]).expect("git init");
        run_git(&path, &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "--allow-empty", "-m", "initial"]).expect("initial commit");
        (dir, path)
    }

    #[test]
    fn test_create_and_remove_worktree() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "feature-test", true, None).expect("create worktree");
        assert!(PathBuf::from(&wt_path).exists(), "worktree dir should exist");

        let branches = git_list_branches(&repo, false).expect("list branches");
        assert!(branches.contains(&"feature-test".to_string()), "branch should exist");

        git_remove_worktree(Path::new(&wt_path), Some("feature-test")).expect("remove worktree");
        assert!(!PathBuf::from(&wt_path).exists(), "worktree dir should be gone");

        let branches_after = git_list_branches(&repo, false).expect("list branches after");
        assert!(!branches_after.contains(&"feature-test".to_string()), "branch should be deleted");
    }

    #[test]
    fn test_create_worktree_existing_branch() {
        let (_dir, repo) = setup_test_repo();
        run_git(&repo, &["branch", "existing-branch"]).expect("create branch");

        let wt_path = git_create_worktree(&repo, "existing-branch", false, None).expect("create worktree");
        assert!(PathBuf::from(&wt_path).exists());

        let info = git_branch_info(Path::new(&wt_path)).expect("branch info");
        assert_eq!(info.branch.as_deref(), Some("existing-branch"));

        git_remove_worktree(Path::new(&wt_path), Some("existing-branch")).expect("cleanup");
    }

    #[test]
    fn test_create_worktree_with_base_branch() {
        let (_dir, repo) = setup_test_repo();
        // Create develop branch with an extra commit
        run_git(&repo, &["checkout", "-b", "develop"]).expect("create develop");
        std::fs::write(repo.join("dev.txt"), "dev content").expect("write file");
        run_git(&repo, &["add", "dev.txt"]).expect("stage");
        run_git(&repo, &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "dev commit"]).expect("commit");
        let _ = run_git(&repo, &["checkout", "master"])
            .or_else(|_| run_git(&repo, &["checkout", "main"]));

        // Create worktree based on develop
        let wt_path = git_create_worktree(&repo, "feature-from-dev", true, Some("develop")).expect("create worktree");
        // The worktree should have dev.txt (inherited from develop)
        assert!(PathBuf::from(&wt_path).join("dev.txt").exists(), "should have develop's file");

        git_remove_worktree(Path::new(&wt_path), Some("feature-from-dev")).expect("cleanup");
    }

    #[test]
    fn test_list_worktrees() {
        let (_dir, repo) = setup_test_repo();
        let wt1 = git_create_worktree(&repo, "wt-one", true, None).expect("create wt1");
        let wt2 = git_create_worktree(&repo, "wt-two", true, None).expect("create wt2");

        let worktrees = git_list_worktrees(&repo).expect("list worktrees");
        assert!(worktrees.len() >= 3, "should have main + 2 worktrees, got {}", worktrees.len());

        let branches: Vec<Option<&str>> = worktrees.iter().map(|w| w.branch.as_deref()).collect();
        assert!(branches.iter().any(|b| *b == Some("wt-one")), "should include wt-one");
        assert!(branches.iter().any(|b| *b == Some("wt-two")), "should include wt-two");

        git_remove_worktree(Path::new(&wt1), Some("wt-one")).expect("cleanup wt1");
        git_remove_worktree(Path::new(&wt2), Some("wt-two")).expect("cleanup wt2");
    }

    #[test]
    fn test_list_branches_local() {
        let (_dir, repo) = setup_test_repo();
        run_git(&repo, &["branch", "alpha"]).expect("create alpha");
        run_git(&repo, &["branch", "beta"]).expect("create beta");

        let branches = git_list_branches(&repo, false).expect("list local");
        assert!(branches.contains(&"alpha".to_string()));
        assert!(branches.contains(&"beta".to_string()));
    }

    #[test]
    fn test_git_status_staged_vs_unstaged() {
        let (_dir, repo) = setup_test_repo();
        // Create a tracked file first
        std::fs::write(repo.join("file.txt"), "original").expect("write");
        run_git(&repo, &["add", "file.txt"]).expect("add");
        run_git(&repo, &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "add file"]).expect("commit");

        // Modify (unstaged)
        std::fs::write(repo.join("file.txt"), "modified").expect("modify");
        let status = git_status(&repo).expect("status");
        let f = status.iter().find(|s| s.path == "file.txt").expect("file in status");
        assert!(!f.is_staged, "should not be staged");
        assert!(f.is_unstaged, "should be unstaged");

        // Stage it
        git_stage(&repo, &["file.txt".to_string()]).expect("stage");
        let status = git_status(&repo).expect("status");
        let f = status.iter().find(|s| s.path == "file.txt").expect("file in status");
        assert!(f.is_staged, "should be staged");
        assert!(!f.is_unstaged, "should not be unstaged");

        // Modify again (both staged and unstaged)
        std::fs::write(repo.join("file.txt"), "modified again").expect("modify again");
        let status = git_status(&repo).expect("status");
        let f = status.iter().find(|s| s.path == "file.txt").expect("file in status");
        assert!(f.is_staged, "should be staged");
        assert!(f.is_unstaged, "should also be unstaged");
    }

    #[test]
    fn test_git_stage_unstage_commit() {
        let (_dir, repo) = setup_test_repo();
        std::fs::write(repo.join("new.txt"), "content").expect("write");
        git_stage(&repo, &["new.txt".to_string()]).expect("stage");

        let status = git_status(&repo).expect("status");
        assert!(status.iter().any(|s| s.path == "new.txt" && s.is_staged));

        git_commit(&repo, "add new file").expect("commit");
        let status = git_status(&repo).expect("status after commit");
        assert!(status.is_empty(), "should be clean after commit");

        // Verify commit exists in log
        let log = run_git(&repo, &["log", "--oneline", "-1"]).expect("log");
        assert!(log.contains("add new file"));
    }

    #[test]
    fn test_remove_worktree_preserves_main() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "temp-branch", true, None).expect("create worktree");
        git_remove_worktree(Path::new(&wt_path), Some("main")).expect("remove with main as branch arg");

        // main should still exist (it was the branch arg but is protected)
        let branches = git_list_branches(&repo, false).expect("list branches");
        // The default branch (main or master) should still be there
        assert!(
            branches.iter().any(|b| b == "main" || b == "master"),
            "main/master should not be deleted"
        );
        // But temp-branch was not requested for deletion, and the worktree is gone
        // The branch temp-branch still exists because we passed "main" as the branch to delete
        assert!(branches.contains(&"temp-branch".to_string()), "temp-branch should remain since we tried to delete 'main' not 'temp-branch'");
    }

    #[test]
    fn staged_vs_unstaged_parsing() {
        let input = " M unstaged-only.rs\nM  staged-only.rs\nMM both-staged-and-unstaged.rs\n?? untracked-file.txt\nA  staged-added.rs";

        let results = parse_porcelain_status(input);
        assert_eq!(results.len(), 5);

        // " M" = unstaged only
        assert_eq!(results[0].path, "unstaged-only.rs");
        assert!(!results[0].is_staged, "' M' should NOT be staged");
        assert!(results[0].is_unstaged, "' M' should be unstaged");

        // "M " = staged only
        assert_eq!(results[1].path, "staged-only.rs");
        assert!(results[1].is_staged, "'M ' should be staged");
        assert!(!results[1].is_unstaged, "'M ' should NOT be unstaged");

        // "MM" = both
        assert_eq!(results[2].path, "both-staged-and-unstaged.rs");
        assert!(results[2].is_staged, "'MM' should be staged");
        assert!(results[2].is_unstaged, "'MM' should be unstaged");

        // "??" = untracked (unstaged only)
        assert_eq!(results[3].path, "untracked-file.txt");
        assert!(!results[3].is_staged, "'??' should NOT be staged");
        assert!(results[3].is_unstaged, "'??' should be unstaged");

        // "A " = staged added
        assert_eq!(results[4].path, "staged-added.rs");
        assert!(results[4].is_staged, "'A ' should be staged");
        assert!(!results[4].is_unstaged, "'A ' should NOT be unstaged");
    }
}
