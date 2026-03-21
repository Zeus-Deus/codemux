use serde::{Deserialize, Serialize};
use std::path::Path;
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

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run git and return stdout even on non-zero exit (for commands where failure is expected).
fn run_git_permissive(repo_path: &Path, args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn git_status(repo_path: &Path) -> Result<Vec<GitFileStatus>, String> {
    let output = run_git(repo_path, &["status", "--porcelain=v1"])?;
    Ok(parse_porcelain_status(&output))
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
    run_git(repo_path, &args)?;
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

        results.push(GitFileStatus { path, status });
    }
    results
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

        assert_eq!(results[0].path, "untracked.txt");
        assert_eq!(results[0].status, FileStatus::Untracked);

        assert_eq!(results[1].path, "staged-new.txt");
        assert_eq!(results[1].status, FileStatus::Added);

        assert_eq!(results[2].path, "modified-staged.txt");
        assert_eq!(results[2].status, FileStatus::Modified);

        assert_eq!(results[3].path, "modified-unstaged.txt");
        assert_eq!(results[3].status, FileStatus::Modified);

        assert_eq!(results[4].path, "modified-both.txt");
        assert_eq!(results[4].status, FileStatus::Modified);

        assert_eq!(results[5].path, "deleted.txt");
        assert_eq!(results[5].status, FileStatus::Deleted);

        assert_eq!(results[6].path, "deleted-unstaged.txt");
        assert_eq!(results[6].status, FileStatus::Deleted);

        assert_eq!(results[7].path, "new-name.txt");
        assert_eq!(results[7].status, FileStatus::Renamed);

        assert_eq!(results[8].path, "copy.txt");
        assert_eq!(results[8].status, FileStatus::Copied);
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
}
