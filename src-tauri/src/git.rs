use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
    Conflicted,
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
    #[serde(default)]
    pub conflict_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseBranchDiff {
    pub files: Vec<GitFileStatus>,
    pub merge_base_commit: String,
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
    pub has_upstream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLogEntry {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub author: String,
    pub time_ago: String,
    pub is_pushed: bool,
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

/// Run git and return (stdout, stderr, success) regardless of exit code.
fn run_git_full(repo_path: &Path, args: &[&str]) -> Result<(String, String, bool), String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim_end().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim_end().to_string();
    Ok((stdout, stderr, output.status.success()))
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

pub fn git_push(repo_path: &Path, set_upstream: bool) -> Result<(), String> {
    if set_upstream {
        let branch_name = run_git_permissive(repo_path, &["branch", "--show-current"]);
        if branch_name.is_empty() {
            return Err("Cannot publish: no branch name".to_string());
        }
        run_git(repo_path, &["push", "-u", "origin", &branch_name])?;
    } else {
        run_git(repo_path, &["push"])?;
    }
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

    // Get unpushed commit hashes (empty if no upstream)
    let unpushed_output = run_git_permissive(repo_path, &["rev-list", "@{upstream}..HEAD"]);
    let unpushed: HashSet<&str> = unpushed_output.lines().collect();
    let has_upstream = !run_git_permissive(
        repo_path,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    )
    .is_empty();

    let lines: Vec<&str> = output.lines().collect();
    let mut entries = Vec::new();
    for chunk in lines.chunks(5) {
        if chunk.len() < 5 {
            break;
        }
        let hash = chunk[0].to_string();
        let is_pushed = has_upstream && !unpushed.contains(hash.as_str());
        entries.push(GitLogEntry {
            hash,
            short_hash: chunk[1].to_string(),
            message: chunk[2].to_string(),
            author: chunk[3].to_string(),
            time_ago: chunk[4].to_string(),
            is_pushed,
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

    let upstream = run_git_permissive(
        repo_path,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    );
    let has_upstream = !upstream.is_empty();

    let (ahead, behind) = if has_upstream {
        let rev_list = run_git_permissive(
            repo_path,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        );
        parse_ahead_behind(&rev_list)
    } else {
        (0, 0)
    };

    Ok(GitBranchInfo {
        branch,
        ahead,
        behind,
        has_upstream,
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

        let (status, conflict_type) = match (index_status, worktree_status) {
            // Conflict codes — must be checked before single-letter matches
            (b'U', b'U') => (FileStatus::Conflicted, Some("both_modified".to_string())),
            (b'A', b'A') => (FileStatus::Conflicted, Some("both_added".to_string())),
            (b'D', b'D') => (FileStatus::Conflicted, Some("both_deleted".to_string())),
            (b'U', b'D') | (b'D', b'U') => (FileStatus::Conflicted, Some("deleted_by_them".to_string())),
            (b'U', b'A') | (b'A', b'U') => (FileStatus::Conflicted, Some("added_by_them".to_string())),
            // Normal status codes
            (b'?', b'?') => (FileStatus::Untracked, None),
            (b'A', _) => (FileStatus::Added, None),
            (b'R', _) => (FileStatus::Renamed, None),
            (b'C', _) => (FileStatus::Copied, None),
            (b'D', _) | (_, b'D') => (FileStatus::Deleted, None),
            (b'M', _) | (_, b'M') => (FileStatus::Modified, None),
            _ => (FileStatus::Modified, None),
        };

        // Conflicted files belong to neither staged nor unstaged — they have their own section
        let (is_staged, is_unstaged) = if status == FileStatus::Conflicted {
            (false, false)
        } else {
            // X column: staged status (anything except ' ' and '?' means staged)
            let staged = index_status != b' ' && index_status != b'?';
            // Y column: unstaged status (anything except ' ' means unstaged; '?' = untracked = unstaged)
            let unstaged = worktree_status != b' ';
            (staged, unstaged)
        };

        results.push(GitFileStatus { path, status, is_staged, is_unstaged, additions: 0, deletions: 0, conflict_type });
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

fn parse_name_status(output: &str) -> Vec<GitFileStatus> {
    let mut files = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let status_code = parts[0].chars().next().unwrap_or('M');
        let path_part = parts[1];
        // Renames show as "R100\told\tnew"
        let path = if let Some(tab_pos) = path_part.find('\t') {
            path_part[tab_pos + 1..].to_string()
        } else {
            path_part.to_string()
        };
        let status = match status_code {
            'A' => FileStatus::Added,
            'M' => FileStatus::Modified,
            'D' => FileStatus::Deleted,
            'R' => FileStatus::Renamed,
            'C' => FileStatus::Copied,
            _ => FileStatus::Modified,
        };
        files.push(GitFileStatus {
            path,
            status,
            is_staged: false,
            is_unstaged: false,
            additions: 0,
            deletions: 0,
            conflict_type: None,
        });
    }
    files
}

fn parse_single_numstat(output: &str) -> std::collections::HashMap<String, (u32, u32)> {
    let mut map = std::collections::HashMap::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let adds = parts[0].parse::<u32>().unwrap_or(0);
            let dels = parts[1].parse::<u32>().unwrap_or(0);
            // Handle renames: numstat shows "old => new" or just the path
            let raw_path = parts[2];
            let path = if let Some(arrow) = raw_path.find(" => ") {
                // e.g. "{src => dest}/file.txt" or "old.txt => new.txt"
                let after = &raw_path[arrow + 4..];
                after.trim_matches('}').to_string()
            } else {
                raw_path.to_string()
            };
            let entry = map.entry(path).or_insert((0, 0));
            entry.0 += adds;
            entry.1 += dels;
        }
    }
    map
}

/// Resolve a base branch ref, trying origin/<branch> first, then local.
fn resolve_base_ref(repo_path: &Path, base_branch: &str) -> Result<String, String> {
    let remote_ref = format!("origin/{}", base_branch);
    let (_, _, ok) = run_git_full(repo_path, &["rev-parse", "--verify", &remote_ref])?;
    if ok {
        return Ok(remote_ref);
    }
    let (_, _, ok) = run_git_full(repo_path, &["rev-parse", "--verify", base_branch])?;
    if ok {
        return Ok(base_branch.to_string());
    }
    Err(format!("Branch '{}' not found locally or on origin", base_branch))
}

pub fn git_diff_base_branch(repo_path: &Path, base_branch: &str) -> Result<BaseBranchDiff, String> {
    let base_ref = resolve_base_ref(repo_path, base_branch)?;
    let merge_base = run_git(repo_path, &["merge-base", "HEAD", &base_ref])?;
    if merge_base.is_empty() {
        return Err("No common ancestor found".to_string());
    }

    let range = format!("{}..HEAD", merge_base);
    let name_status = run_git_permissive(repo_path, &["diff", "--name-status", &range]);
    let mut files = parse_name_status(&name_status);

    let numstat = run_git_permissive(repo_path, &["diff", "--numstat", &range]);
    let stats = parse_single_numstat(&numstat);
    for file in &mut files {
        if let Some(&(a, d)) = stats.get(&file.path) {
            file.additions = a;
            file.deletions = d;
        }
    }

    Ok(BaseBranchDiff {
        files,
        merge_base_commit: merge_base,
    })
}

pub fn git_diff_base_branch_file(repo_path: &Path, base_branch: &str, file_path: &str) -> Result<String, String> {
    let base_ref = resolve_base_ref(repo_path, base_branch)?;
    let merge_base = run_git(repo_path, &["merge-base", "HEAD", &base_ref])?;
    if merge_base.is_empty() {
        return Err("No common ancestor found".to_string());
    }
    let range = format!("{}..HEAD", merge_base);
    run_git(repo_path, &["diff", &range, "--", file_path])
}

pub fn git_default_branch(repo_path: &Path) -> Result<String, String> {
    // Try symbolic-ref first
    let (stdout, _, ok) = run_git_full(repo_path, &["symbolic-ref", "refs/remotes/origin/HEAD"])?;
    if ok && !stdout.is_empty() {
        if let Some(branch) = stdout.strip_prefix("refs/remotes/origin/") {
            let branch = branch.trim();
            if !branch.is_empty() {
                return Ok(branch.to_string());
            }
        }
    }
    // Fallback: check which common branches exist
    let branches = git_list_branches(repo_path, false).unwrap_or_default();
    if branches.iter().any(|b| b == "main") {
        return Ok("main".to_string());
    }
    if branches.iter().any(|b| b == "master") {
        return Ok("master".to_string());
    }
    Ok("main".to_string())
}

// ---- Merge conflict operations ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictFile {
    pub path: String,
    pub conflict_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeState {
    pub is_merging: bool,
    pub is_rebasing: bool,
    pub merge_head: Option<String>,
    pub conflicted_files: Vec<ConflictFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictCheckResult {
    pub has_conflicts: bool,
    pub conflicting_files: Vec<ConflictFile>,
    pub target_branch: String,
}

/// Get the actual git directory (handles worktrees where .git is a file).
fn resolve_git_dir(repo_path: &Path) -> Result<PathBuf, String> {
    let output = run_git(repo_path, &["rev-parse", "--git-dir"])?;
    let git_dir = PathBuf::from(output.trim());
    if git_dir.is_absolute() {
        Ok(git_dir)
    } else {
        Ok(repo_path.join(git_dir))
    }
}

pub fn get_merge_state(repo_path: &Path) -> Result<MergeState, String> {
    let git_dir = resolve_git_dir(repo_path)?;

    let is_merging = git_dir.join("MERGE_HEAD").exists();
    let is_rebasing = git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists();

    let merge_head = if is_merging {
        std::fs::read_to_string(git_dir.join("MERGE_HEAD"))
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        None
    };

    let conflicted_files = if is_merging || is_rebasing {
        git_status(repo_path)?
            .into_iter()
            .filter(|f| f.status == FileStatus::Conflicted)
            .map(|f| ConflictFile {
                path: f.path,
                conflict_type: f.conflict_type.unwrap_or_else(|| "both_modified".to_string()),
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(MergeState { is_merging, is_rebasing, merge_head, conflicted_files })
}

pub fn check_merge_conflicts(
    repo_path: &Path,
    target_branch: &str,
) -> Result<ConflictCheckResult, String> {
    // Safety: refuse to run on dirty working tree
    let status = git_status(repo_path)?;
    if !status.is_empty() {
        return Err("Cannot check for conflicts: working tree has uncommitted changes. Commit or stash your changes first.".to_string());
    }

    // Attempt dry-run merge
    let (_stdout, _stderr, success) = run_git_full(
        repo_path,
        &["merge", "--no-commit", "--no-ff", target_branch],
    )?;

    if success {
        // Clean merge — abort to undo
        let _ = run_git(repo_path, &["merge", "--abort"]);
        return Ok(ConflictCheckResult {
            has_conflicts: false,
            conflicting_files: Vec::new(),
            target_branch: target_branch.to_string(),
        });
    }

    // Merge had conflicts — collect them from status
    let conflict_status = git_status(repo_path).unwrap_or_default();
    let conflicting_files: Vec<ConflictFile> = conflict_status
        .iter()
        .filter(|f| f.status == FileStatus::Conflicted)
        .map(|f| ConflictFile {
            path: f.path.clone(),
            conflict_type: f.conflict_type.clone().unwrap_or_else(|| "both_modified".to_string()),
        })
        .collect();

    // Abort the merge to restore clean state
    let _ = run_git(repo_path, &["merge", "--abort"]);

    Ok(ConflictCheckResult {
        has_conflicts: !conflicting_files.is_empty(),
        conflicting_files,
        target_branch: target_branch.to_string(),
    })
}

pub fn resolve_conflict_ours(repo_path: &Path, file: &str) -> Result<(), String> {
    run_git(repo_path, &["checkout", "--ours", "--", file])?;
    run_git(repo_path, &["add", "--", file])?;
    Ok(())
}

pub fn resolve_conflict_theirs(repo_path: &Path, file: &str) -> Result<(), String> {
    run_git(repo_path, &["checkout", "--theirs", "--", file])?;
    run_git(repo_path, &["add", "--", file])?;
    Ok(())
}

pub fn mark_conflict_resolved(repo_path: &Path, file: &str) -> Result<(), String> {
    run_git(repo_path, &["add", "--", file])?;
    Ok(())
}

pub fn abort_merge(repo_path: &Path) -> Result<(), String> {
    let git_dir = resolve_git_dir(repo_path)?;
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        run_git(repo_path, &["rebase", "--abort"])?;
    } else {
        run_git(repo_path, &["merge", "--abort"])?;
    }
    Ok(())
}

pub fn continue_merge(repo_path: &Path, message: &str) -> Result<(), String> {
    let git_dir = resolve_git_dir(repo_path)?;
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        // For rebase, continue with the current state
        run_git(repo_path, &["rebase", "--continue"])?;
    } else {
        // For merge, commit with the provided message
        run_git(repo_path, &["commit", "-m", message])?;
    }
    Ok(())
}

// ---- Resolver branch operations ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolverBranchInfo {
    pub temp_branch: String,
    pub original_branch: String,
    pub target_branch: String,
    pub conflicting_files: Vec<ConflictFile>,
}

pub fn create_resolver_branch(
    repo_path: &Path,
    target_branch: &str,
) -> Result<ResolverBranchInfo, String> {
    // Get current branch
    let original_branch = run_git(repo_path, &["branch", "--show-current"])?;
    if original_branch.is_empty() {
        return Err("Cannot create resolver branch: not on a named branch".to_string());
    }

    // Generate temp branch name
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let safe_source = original_branch.replace('/', "-");
    let safe_target = target_branch.replace('/', "-");
    let temp_branch = format!("bot/resolve-{}-into-{}-{}", safe_source, safe_target, timestamp);

    // Create and switch to temp branch
    run_git(repo_path, &["checkout", "-b", &temp_branch])?;

    // Start the merge (will fail with conflicts — that's expected)
    let (_stdout, _stderr, _success) = run_git_full(
        repo_path,
        &["merge", "--no-edit", target_branch],
    )?;

    // Parse status for conflicted files
    let status = git_status(repo_path).unwrap_or_default();
    let conflicting_files: Vec<ConflictFile> = status
        .iter()
        .filter(|f| f.status == FileStatus::Conflicted)
        .map(|f| ConflictFile {
            path: f.path.clone(),
            conflict_type: f.conflict_type.clone().unwrap_or_else(|| "both_modified".to_string()),
        })
        .collect();

    Ok(ResolverBranchInfo {
        temp_branch,
        original_branch,
        target_branch: target_branch.to_string(),
        conflicting_files,
    })
}

pub fn apply_resolution(
    repo_path: &Path,
    temp_branch: &str,
    original_branch: &str,
    message: &str,
) -> Result<(), String> {
    // Verify all conflicts are resolved
    let status = git_status(repo_path)?;
    let unresolved: Vec<_> = status.iter().filter(|f| f.status == FileStatus::Conflicted).collect();
    if !unresolved.is_empty() {
        return Err(format!(
            "Cannot apply resolution: {} unresolved conflict(s)",
            unresolved.len()
        ));
    }

    // Commit the merge on temp branch
    run_git(repo_path, &["commit", "-m", message])?;

    // Switch back to original branch
    run_git(repo_path, &["checkout", original_branch])?;

    // Merge the temp branch (should be a fast-forward)
    run_git(repo_path, &["merge", temp_branch])?;

    // Clean up temp branch
    let _ = run_git(repo_path, &["branch", "-d", temp_branch]);

    Ok(())
}

pub fn abort_resolution(
    repo_path: &Path,
    temp_branch: &str,
    original_branch: &str,
) -> Result<(), String> {
    // Abort any in-progress merge
    let _ = run_git(repo_path, &["merge", "--abort"]);

    // Switch back to original branch
    run_git(repo_path, &["checkout", original_branch])?;

    // Force-delete the temp branch
    run_git(repo_path, &["branch", "-D", temp_branch])?;

    Ok(())
}

pub fn get_resolution_diff(repo_path: &Path) -> Result<String, String> {
    run_git(repo_path, &["diff", "--cached"])
}

// ---- Worktree operations ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    pub is_bare: bool,
}

pub fn is_git_repo(path: &Path) -> bool {
    run_git(path, &["rev-parse", "--git-dir"]).is_ok()
}

pub fn git_init_repo(path: &Path) -> Result<String, String> {
    run_git(path, &["init"])?;
    run_git(path, &["add", "."])?;
    run_git(path, &["commit", "--allow-empty", "-m", "Initial commit"])?;
    Ok("Repository initialized".to_string())
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
    let git_root = crate::config::workspace_config::find_git_root(repo_path);
    let repo_name = git_root
        .as_deref()
        .unwrap_or(repo_path)
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

    // ---- Additional integration tests ----

    /// Helper: creates a bare "remote" repo and a cloned "local" with an initial commit pushed.
    fn setup_test_repo_with_remote() -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new().expect("create temp dir");
        let remote = dir.path().join("remote.git");
        let local = dir.path().join("local");

        // Create bare remote
        run_git(dir.path(), &["init", "--bare", remote.to_str().unwrap()]).expect("init bare");

        // Clone it
        run_git(dir.path(), &["clone", remote.to_str().unwrap(), local.to_str().unwrap()])
            .expect("clone");

        // Initial commit + push in local
        std::fs::write(local.join("init.txt"), "initial").expect("write init file");
        run_git(&local, &["add", "init.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "initial"],
        )
        .expect("initial commit");
        run_git(&local, &["push"]).expect("initial push");

        (dir, local, remote)
    }

    #[test]
    fn test_git_unstage() {
        let (_dir, repo) = setup_test_repo();
        std::fs::write(repo.join("unstage-me.txt"), "content").expect("write");
        git_stage(&repo, &["unstage-me.txt".to_string()]).expect("stage");

        // Verify it's staged
        let status = git_status(&repo).expect("status");
        let f = status.iter().find(|s| s.path == "unstage-me.txt").expect("file in status");
        assert!(f.is_staged, "should be staged before unstage");

        // Unstage it
        git_unstage(&repo, &["unstage-me.txt".to_string()]).expect("unstage");

        // Verify it's no longer staged (still untracked)
        let status = git_status(&repo).expect("status after unstage");
        let f = status.iter().find(|s| s.path == "unstage-me.txt").expect("file still in status");
        assert!(!f.is_staged, "should not be staged after unstage");
        assert!(f.is_unstaged, "should be unstaged");
    }

    #[test]
    fn test_git_discard_tracked_file() {
        let (_dir, repo) = setup_test_repo();
        // Create and commit a file
        std::fs::write(repo.join("tracked.txt"), "original content").expect("write");
        run_git(&repo, &["add", "tracked.txt"]).expect("add");
        run_git(
            &repo,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "add tracked"],
        )
        .expect("commit");

        // Modify the file
        std::fs::write(repo.join("tracked.txt"), "modified content").expect("modify");
        let content = std::fs::read_to_string(repo.join("tracked.txt")).expect("read");
        assert_eq!(content, "modified content");

        // Discard changes
        git_discard_file(&repo, "tracked.txt").expect("discard");

        // Verify content restored
        let content = std::fs::read_to_string(repo.join("tracked.txt")).expect("read after discard");
        assert_eq!(content, "original content");
    }

    #[test]
    fn test_git_discard_untracked_file() {
        let (_dir, repo) = setup_test_repo();
        let file_path = repo.join("untracked.txt");
        std::fs::write(&file_path, "should be deleted").expect("write");
        assert!(file_path.exists(), "file should exist before discard");

        git_discard_file(&repo, "untracked.txt").expect("discard untracked");
        assert!(!file_path.exists(), "untracked file should be deleted after discard");
    }

    #[test]
    fn test_git_log_entries() {
        let (_dir, repo) = setup_test_repo();
        // Make 3 more commits (setup_test_repo already has "initial")
        for i in 1..=3 {
            std::fs::write(repo.join(format!("file{i}.txt")), format!("content {i}")).expect("write");
            run_git(&repo, &["add", "."]).expect("add");
            run_git(
                &repo,
                &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", &format!("commit {i}")],
            )
            .expect("commit");
        }

        let entries = git_log(&repo, 10).expect("git log");
        assert_eq!(entries.len(), 4, "should have 4 entries (initial + 3)");

        // Most recent first
        assert_eq!(entries[0].message, "commit 3");
        assert_eq!(entries[1].message, "commit 2");
        assert_eq!(entries[2].message, "commit 1");
        assert_eq!(entries[3].message, "initial");

        // Hashes should be non-empty
        assert!(!entries[0].hash.is_empty());
        assert!(!entries[0].short_hash.is_empty());
        assert_eq!(entries[0].author, "Test");
    }

    #[test]
    fn test_git_push() {
        let (_dir, local, remote) = setup_test_repo_with_remote();

        // Make a new commit locally
        std::fs::write(local.join("pushed.txt"), "push me").expect("write");
        run_git(&local, &["add", "pushed.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "push test"],
        )
        .expect("commit");

        // Push using our function
        git_push(&local, false).expect("push");

        // Verify by cloning the remote again and checking the log
        let verify = _dir.path().join("verify");
        run_git(_dir.path(), &["clone", remote.to_str().unwrap(), verify.to_str().unwrap()])
            .expect("clone for verify");
        let log = run_git(&verify, &["log", "--oneline"]).expect("log");
        assert!(log.contains("push test"), "remote should have the pushed commit");
    }

    #[test]
    fn test_git_push_set_upstream() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();

        // Create a new local branch with no upstream
        run_git(&local, &["checkout", "-b", "new-feature"]).expect("create branch");
        std::fs::write(local.join("feature.txt"), "new feature").expect("write");
        run_git(&local, &["add", "feature.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "feature commit"],
        )
        .expect("commit");

        // Branch should have no upstream
        let info = git_branch_info(&local).expect("branch info before push");
        assert!(!info.has_upstream, "new branch should have no upstream");

        // Push with set_upstream
        git_push(&local, true).expect("push with set upstream");

        // Branch should now have upstream
        let info = git_branch_info(&local).expect("branch info after push");
        assert!(info.has_upstream, "branch should have upstream after publish");

        // Verify remote has the branch
        let remote_branches = run_git(&local, &["ls-remote", "--heads", "origin"]).expect("ls-remote");
        assert!(remote_branches.contains("new-feature"), "remote should have the branch");
    }

    #[test]
    fn test_git_pull() {
        let (_dir, local, remote) = setup_test_repo_with_remote();

        // Create a second clone that pushes a new commit
        let clone2 = _dir.path().join("clone2");
        run_git(_dir.path(), &["clone", remote.to_str().unwrap(), clone2.to_str().unwrap()])
            .expect("clone2");
        std::fs::write(clone2.join("from-clone2.txt"), "clone2 content").expect("write in clone2");
        run_git(&clone2, &["add", "from-clone2.txt"]).expect("add in clone2");
        run_git(
            &clone2,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "clone2 commit"],
        )
        .expect("commit in clone2");
        run_git(&clone2, &["push"]).expect("push from clone2");

        // Pull in original local
        git_pull(&local).expect("pull");

        // Verify local has the new file
        assert!(
            local.join("from-clone2.txt").exists(),
            "pulled file should exist in local"
        );
        let log = run_git(&local, &["log", "--oneline"]).expect("log");
        assert!(log.contains("clone2 commit"), "local should have the pulled commit");
    }

    #[test]
    fn test_git_branch_info_with_upstream() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();

        // Make a local commit (don't push)
        std::fs::write(local.join("ahead.txt"), "unpushed").expect("write");
        run_git(&local, &["add", "ahead.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "unpushed"],
        )
        .expect("commit");

        let info = git_branch_info(&local).expect("branch info");
        assert!(info.has_upstream, "cloned repo should have upstream");
        assert_eq!(info.ahead, 1, "should be 1 commit ahead");
        assert_eq!(info.behind, 0, "should not be behind");
        assert!(info.branch.is_some(), "should have a branch name");
    }

    #[test]
    fn test_git_branch_info_no_upstream() {
        let (_dir, repo) = setup_test_repo();

        let info = git_branch_info(&repo).expect("branch info");
        assert!(!info.has_upstream, "local-only repo should have no upstream");
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
        assert!(info.branch.is_some(), "should have a branch name");
    }

    #[test]
    fn test_git_operations_in_worktree() {
        let (_dir, repo) = setup_test_repo();
        let wt_path_str = git_create_worktree(&repo, "wt-ops-test", true, None).expect("create worktree");
        let wt_path = PathBuf::from(&wt_path_str);

        // Write a file in the worktree
        std::fs::write(wt_path.join("wt-file.txt"), "worktree content").expect("write in worktree");

        // Status should show the new file
        let status = git_status(&wt_path).expect("status in worktree");
        assert!(
            status.iter().any(|s| s.path == "wt-file.txt" && s.is_unstaged),
            "worktree should show untracked file"
        );

        // Stage it
        git_stage(&wt_path, &["wt-file.txt".to_string()]).expect("stage in worktree");
        let status = git_status(&wt_path).expect("status after stage");
        assert!(
            status.iter().any(|s| s.path == "wt-file.txt" && s.is_staged),
            "file should be staged in worktree"
        );

        // Commit it
        run_git(&wt_path, &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "wt commit"])
            .expect("commit in worktree");
        let status = git_status(&wt_path).expect("status after commit");
        assert!(status.is_empty(), "worktree should be clean after commit");

        // Log should show the commit
        let log = git_log(&wt_path, 5).expect("log in worktree");
        assert!(
            log.iter().any(|e| e.message == "wt commit"),
            "worktree log should include our commit"
        );

        // Branch info should work
        let info = git_branch_info(&wt_path).expect("branch info in worktree");
        assert_eq!(info.branch.as_deref(), Some("wt-ops-test"));

        // Cleanup
        git_remove_worktree(Path::new(&wt_path_str), Some("wt-ops-test")).expect("cleanup worktree");
    }

    #[test]
    fn test_worktree_name_uses_main_repo_not_worktree_dir() {
        // When creating a new worktree from INSIDE an existing worktree,
        // the worktree directory name should use the main repo's name,
        // not the worktree's directory name.
        let (_dir, repo) = setup_test_repo();

        // Create first worktree
        let wt1_path = git_create_worktree(&repo, "wt-first", true, None)
            .expect("create first worktree");
        let wt1 = PathBuf::from(&wt1_path);
        assert!(wt1.exists(), "first worktree should exist");

        // Now create a second worktree from INSIDE the first worktree
        // This simulates: user's active workspace is a worktree, they create another
        let wt2_path = git_create_worktree(&wt1, "wt-second", true, None)
            .expect("create second worktree from inside first");
        let wt2 = PathBuf::from(&wt2_path);
        assert!(wt2.exists(), "second worktree should exist");

        // Both worktrees should be under the SAME repo-name directory
        let wt1_parent = PathBuf::from(&wt1_path).parent().unwrap().file_name().unwrap().to_string_lossy().to_string();
        let wt2_parent = PathBuf::from(&wt2_path).parent().unwrap().file_name().unwrap().to_string_lossy().to_string();

        assert_eq!(
            wt1_parent, wt2_parent,
            "both worktrees should be under the same project directory, \
             but wt1 is under '{}' and wt2 is under '{}'. \
             wt1_path={}, wt2_path={}",
            wt1_parent, wt2_parent, wt1_path, wt2_path,
        );

        // The parent dir should be the repo's folder name, not a branch name
        let repo_folder = repo.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(
            wt2_parent, repo_folder,
            "worktree should be under repo name '{}', not '{}'",
            repo_folder, wt2_parent,
        );

        // Cleanup
        git_remove_worktree(Path::new(&wt2_path), Some("wt-second")).expect("cleanup wt2");
        git_remove_worktree(Path::new(&wt1_path), Some("wt-first")).expect("cleanup wt1");
    }

    #[test]
    fn test_worktree_branch_with_slashes_sanitized() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "feature/deep/nested", true, None)
            .expect("create worktree for branch with slashes");

        // Slashes in branch name should be replaced with hyphens in directory name
        let dir_name = PathBuf::from(&wt_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(dir_name, "feature-deep-nested", "slashes should be replaced with hyphens");
        assert!(PathBuf::from(&wt_path).exists(), "worktree should exist");

        git_remove_worktree(Path::new(&wt_path), Some("feature/deep/nested")).expect("cleanup");
    }

    #[test]
    fn test_worktree_empty_repo_does_not_crash() {
        let dir = TempDir::new().expect("create temp dir");
        let path = dir.path().to_path_buf();
        run_git(&path, &["init"]).expect("git init");
        // No commits — behavior may vary by git version but should not crash

        let result = git_create_worktree(&path, "new-branch", true, None);
        // Either succeeds (modern git) or returns Err (older git) — just don't crash
        if let Ok(wt_path) = &result {
            let _ = std::fs::remove_dir_all(wt_path);
        }
    }

    #[test]
    fn test_delete_workspace_removes_branch() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "test-delete-me", true, None)
            .expect("create worktree");

        let branches_before = git_list_branches(&repo, false).expect("list before");
        assert!(branches_before.contains(&"test-delete-me".to_string()), "branch should exist before delete");

        // Remove worktree WITH branch deletion (simulates delete with checkbox checked)
        git_remove_worktree(Path::new(&wt_path), Some("test-delete-me")).expect("remove with branch");

        let branches_after = git_list_branches(&repo, false).expect("list after");
        assert!(!branches_after.contains(&"test-delete-me".to_string()), "branch should be gone after delete");
    }

    #[test]
    fn test_close_workspace_keeps_branch() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "test-keep-me", true, None)
            .expect("create worktree");

        let branches_before = git_list_branches(&repo, false).expect("list before");
        assert!(branches_before.contains(&"test-keep-me".to_string()), "branch should exist before close");

        // Remove worktree WITHOUT branch deletion (simulates close without checkbox)
        git_remove_worktree(Path::new(&wt_path), None).expect("remove without branch");

        let branches_after = git_list_branches(&repo, false).expect("list after");
        assert!(branches_after.contains(&"test-keep-me".to_string()), "branch should still exist after close");

        // Clean up the branch manually
        let _ = run_git(&repo, &["branch", "-D", "test-keep-me"]);
    }
}
