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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitFileEntry {
    pub path: String,
    pub status: String,
}

/// Sanitize a branch name so it can be used as a filesystem directory
/// component on every supported OS.
///
/// Replaces the path-separator characters (`/` and `\`) plus all
/// Windows-forbidden filename characters (`< > : " | ? *`) with `-`.
/// Linux would accept most of these (only `/` is forbidden), but worktrees
/// created on a Linux machine may eventually be synced, checked out, or
/// listed by a Windows user (e.g. through `codemux workspace export`), so
/// we apply the Windows-strict rule cross-platform and accept that it's
/// slightly noisier than strictly necessary on Linux.
///
/// Not cfg-gated — same behavior on every platform. The test for this
/// function runs on every CI matrix leg and catches regressions in the
/// character set if someone edits it later.
///
/// Reference: https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file
/// (see "Naming Conventions" — forbidden chars in NTFS/FAT filenames).
pub(crate) fn sanitize_branch_for_worktree_path(branch: &str) -> String {
    branch
        .chars()
        .map(|c| match c {
            '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*' => '-',
            _ => c,
        })
        .collect()
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

/// Clone a remote repository into `target_dir`. Used by cross-device
/// workspace adoption when a sibling-device workspace has no shared
/// host — we clone the git remote and create a fresh worktree at the
/// branch.
///
/// `target_dir` must NOT already exist (git clone refuses if the
/// target dir is non-empty). Caller is expected to compute a unique
/// target like `~/.codemux/projects/<basename>` and bail with a
/// structured error if it collides.
///
/// Returns the absolute path of the cloned repo on success.
///
/// SSH credentials and any git auth (token, key) live in the user's
/// `~/.gitconfig` / `~/.ssh` / `~/.netrc` — this helper inherits
/// them implicitly via the spawned git process. We never marshal
/// credentials through this layer.
pub fn git_clone(remote_url: &str, target_dir: &Path) -> Result<String, String> {
    if remote_url.trim().is_empty() {
        return Err("git_clone: remote_url cannot be empty".into());
    }
    if target_dir.exists() {
        // Don't clobber — caller should have picked a fresh path.
        return Err(format!(
            "git_clone: target directory already exists: {}",
            target_dir.display()
        ));
    }
    #[cfg(test)]
    {
        // Test hook — short-circuit BEFORE spawning git so unit
        // tests don't need network or git installed. Real callers
        // never hit this because they pass non-test markers.
        if remote_url.starts_with("test://") {
            return Ok(target_dir.to_string_lossy().to_string());
        }
    }
    let parent = target_dir.parent().ok_or_else(|| {
        format!(
            "git_clone: target path has no parent directory: {}",
            target_dir.display()
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|e| {
        format!("git_clone: failed to create parent {}: {e}", parent.display())
    })?;
    let target_str = target_dir.to_string_lossy().to_string();
    // Use `--no-checkout` so the worktree-add we'll do next chooses
    // the right branch — clone's default checkout of the remote's
    // HEAD branch is wasted work for our flow.
    let output = Command::new("git")
        .args(["clone", "--no-checkout", remote_url, &target_str])
        .current_dir(parent)
        .output()
        .map_err(|e| format!("git_clone: failed to spawn git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Best-effort cleanup so a half-cloned dir doesn't poison
        // the next attempt.
        let _ = std::fs::remove_dir_all(target_dir);
        return Err(format!(
            "git clone failed: {}",
            stderr.trim()
        ));
    }
    Ok(target_str)
}

#[cfg(test)]
mod git_clone_input_validation {
    use super::git_clone;
    use std::path::PathBuf;

    #[test]
    fn rejects_empty_remote_url() {
        let dir = std::env::temp_dir().join("codemux-git-clone-test-empty");
        let _ = std::fs::remove_dir_all(&dir);
        let err = git_clone("", &dir).unwrap_err();
        assert!(err.contains("cannot be empty"), "got: {err}");
    }

    #[test]
    fn rejects_whitespace_only_remote_url() {
        let dir = std::env::temp_dir().join("codemux-git-clone-test-ws");
        let _ = std::fs::remove_dir_all(&dir);
        let err = git_clone("   \t  ", &dir).unwrap_err();
        assert!(err.contains("cannot be empty"), "got: {err}");
    }

    #[test]
    fn rejects_when_target_dir_already_exists() {
        let dir = std::env::temp_dir().join("codemux-git-clone-test-exists");
        // Create the directory so the precondition fires.
        std::fs::create_dir_all(&dir).expect("set up test dir");
        let err = git_clone("test://foo", &dir).unwrap_err();
        assert!(err.contains("already exists"), "got: {err}");
        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accepts_well_formed_input_via_test_hook() {
        // Uses the `test://` short-circuit so we don't need git or
        // network on the CI runner. The hook returns success
        // BEFORE doing any disk I/O — that's the point: the test
        // only exercises the precondition checks above it (empty
        // url, existing target), then yields a deterministic
        // success that proves both checks let valid input through.
        let dir = std::env::temp_dir().join("codemux-git-clone-test-ok-fresh");
        let _ = std::fs::remove_dir_all(&dir);
        let result = git_clone("test://foo/bar.git", &dir).unwrap();
        assert_eq!(PathBuf::from(result), dir);
    }
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

/// Resolve a usable upstream ref for the current branch.
///
/// Returns the ref name (e.g. `origin/main`) we should diff against to
/// compute ahead/behind and "is this commit pushed?" — or `None` if the
/// branch genuinely hasn't been published yet.
///
/// Priority:
/// 1. Formal upstream tracking via `branch.<name>.{remote,merge}`. This is
///    what `@{upstream}` resolves to. Set automatically by clone and by
///    `git push -u`.
/// 2. Fallback: a same-named remote-tracking ref (`refs/remotes/<remote>/<branch>`).
///    This catches branches that were pushed without `-u` — for example
///    via `push.autoSetupRemote=true` or `git push origin <branch>`. Git
///    accepts those pushes but doesn't write the tracking config, so
///    `@{upstream}` stays unset even though the branch clearly exists on
///    the remote. Before this fallback, the UI mistakenly offered to
///    "Publish Branch" for such branches.
///
/// `origin` is preferred when multiple remotes match, since it's the
/// conventional default and what `git push` targets when no remote is
/// specified.
fn resolve_upstream_ref(repo_path: &Path) -> Option<String> {
    // Preferred path: branch.<name>.{remote,merge} is configured.
    let upstream = run_git_permissive(
        repo_path,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    );
    if !upstream.is_empty() {
        return Some(upstream);
    }

    // Fallback path: look for a same-named remote-tracking ref. Empty branch
    // name means detached HEAD — nothing to resolve.
    let branch = run_git_permissive(repo_path, &["branch", "--show-current"]);
    if branch.is_empty() {
        return None;
    }

    let remotes_raw = run_git_permissive(repo_path, &["remote"]);
    let mut remotes: Vec<&str> = remotes_raw.lines().filter(|r| !r.is_empty()).collect();
    // Probe origin first; it's the conventional default and avoids
    // surprising the user when multiple remotes have the same branch.
    remotes.sort_by_key(|r| if *r == "origin" { 0 } else { 1 });

    for remote in remotes {
        let ref_name = format!("refs/remotes/{remote}/{branch}");
        let resolved = run_git_permissive(
            repo_path,
            &["rev-parse", "--verify", "--quiet", &ref_name],
        );
        if !resolved.is_empty() {
            return Some(format!("{remote}/{branch}"));
        }
    }
    None
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

pub fn git_fetch(repo_path: &Path) -> Result<(), String> {
    run_git(repo_path, &["fetch"])?;
    Ok(())
}

pub fn git_amend_commit(repo_path: &Path, message: Option<&str>) -> Result<(), String> {
    if let Some(msg) = message {
        run_git(repo_path, &["commit", "--amend", "-m", msg])?;
    } else {
        run_git(repo_path, &["commit", "--amend", "--no-edit"])?;
    }
    Ok(())
}

pub fn git_undo_last_commit(repo_path: &Path) -> Result<(), String> {
    run_git(repo_path, &["reset", "--soft", "HEAD~1"])?;
    Ok(())
}

pub fn git_stash_push(repo_path: &Path, include_untracked: bool) -> Result<(), String> {
    if include_untracked {
        run_git(repo_path, &["stash", "push", "--include-untracked"])?;
    } else {
        run_git(repo_path, &["stash", "push"])?;
    }
    Ok(())
}

pub fn git_stash_pop(repo_path: &Path) -> Result<(), String> {
    run_git(repo_path, &["stash", "pop"])?;
    Ok(())
}

// ── Run checkpoints (issue #80) ─────────────────────────────────────
//
// Non-destructive working-tree snapshots taken in the background when
// an agent-chat run starts, so the user can roll back everything the
// run changed. Three invariants drive the implementation:
//
//   1. Creating a checkpoint must NOT disturb the user's index,
//      worktree, or stash list. `git stash create` would qualify but
//      ignores untracked files, so we instead build a snapshot commit
//      through a temporary index (`GIT_INDEX_FILE`), which git treats
//      as a fully independent staging area.
//   2. The snapshot must survive `git gc` — it is anchored on a
//      dedicated shadow ref under `refs/codemux/`.
//   3. No hooks may fire (the user's `pre-commit` must not run on a
//      background snapshot) — `git commit-tree` is plumbing and runs
//      none.

/// Ref namespace for run-start checkpoints.
pub const CHECKPOINT_REF_PREFIX: &str = "refs/codemux/checkpoints";
/// Ref namespace for the safety snapshots taken right before a restore.
pub const PRE_RESTORE_REF_PREFIX: &str = "refs/codemux/pre-restore";
/// How many refs to keep per namespace when pruning.
pub const CHECKPOINT_KEEP_PER_NAMESPACE: usize = 20;

/// A created working-tree snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCheckpoint {
    /// Fully-qualified ref anchoring the snapshot (gc protection).
    pub ref_name: String,
    /// Commit object whose tree is the full worktree content
    /// (tracked changes + untracked non-ignored files).
    pub snapshot_commit: String,
    /// `HEAD` at snapshot time. Restore moves the branch back here,
    /// undoing any commits the run made.
    pub head_commit: String,
    /// Checked-out branch at snapshot time (`None` when detached).
    pub branch: Option<String>,
}

/// Sanitize an arbitrary id (e.g. an agent-chat thread id) into a
/// single safe ref component. Conservative: anything outside
/// `[A-Za-z0-9_-]` becomes `-`, which sidesteps every
/// `git check-ref-format` rule (`..`, `@{`, leading `.`, `*.lock`, …).
pub fn sanitize_ref_component(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    out.truncate(100);
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "checkpoint".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Full checkpoint ref for a thread id.
pub fn checkpoint_ref_name(thread_id: &str) -> String {
    format!("{CHECKPOINT_REF_PREFIX}/{}", sanitize_ref_component(thread_id))
}

/// Full pre-restore safety ref for a thread id.
pub fn pre_restore_ref_name(thread_id: &str) -> String {
    format!("{PRE_RESTORE_REF_PREFIX}/{}", sanitize_ref_component(thread_id))
}

/// Run git with a private index file so staging operations never touch
/// the user's real index.
fn run_git_with_index(
    repo_path: &Path,
    index_file: &Path,
    args: &[&str],
) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .env("GIT_INDEX_FILE", index_file)
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

/// Snapshot the full working-tree state (staged + unstaged + untracked,
/// `.gitignore` respected) into a commit anchored at `full_ref`,
/// without touching the user's index, worktree, or stash list.
///
/// Returns `Ok(None)` when there is nothing snapshottable — the path
/// is not inside a git repo, or the repo has an unborn HEAD (no
/// commits yet). Callers treat `None` as "checkpoint skipped".
pub fn git_checkpoint_create(
    repo_path: &Path,
    full_ref: &str,
    message: &str,
) -> Result<Option<GitCheckpoint>, String> {
    // Resolve the worktree root; failure means "not a git repo" and
    // also covers bare repos (no toplevel → nothing to snapshot).
    let Ok(toplevel) = run_git(repo_path, &["rev-parse", "--show-toplevel"]) else {
        return Ok(None);
    };
    let repo = Path::new(&toplevel);
    // Unborn HEAD (fresh `git init`, zero commits): there is no parent
    // to anchor the snapshot to and no baseline to roll back to.
    let Ok(head) = run_git(repo, &["rev-parse", "HEAD"]) else {
        return Ok(None);
    };
    let branch = {
        let b = run_git_permissive(repo, &["branch", "--show-current"]);
        if b.is_empty() {
            None
        } else {
            Some(b)
        }
    };

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Per-call monotonic suffix so two concurrent checkpoint operations
    // in the same process never share a temp-index path (and thus never
    // collide on its `.lock`). `nanos` alone is insufficient under a
    // coarse/contended clock — two threads can read the same value.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_index = std::env::temp_dir().join(format!(
        "codemux-checkpoint-index-{}-{nanos}-{seq}",
        std::process::id()
    ));

    let result = (|| -> Result<String, String> {
        // Seed the temp index from the real one when possible: the
        // copied stat cache means the following `add -A` only rehashes
        // files that actually changed, instead of the whole repo.
        // Index writes are atomic (write + rename), so a concurrent
        // git process can't hand us a torn file.
        let index_rel = run_git(repo, &["rev-parse", "--git-path", "index"])?;
        let real_index = {
            let p = PathBuf::from(&index_rel);
            if p.is_absolute() {
                p
            } else {
                repo.join(p)
            }
        };
        if real_index.exists() {
            std::fs::copy(&real_index, &tmp_index)
                .map_err(|e| format!("Failed to copy index for checkpoint: {e}"))?;
        } else {
            run_git_with_index(repo, &tmp_index, &["read-tree", "HEAD"])?;
        }
        // Stage everything (tracked modifications, deletions, and
        // untracked files) into the PRIVATE index only.
        run_git_with_index(repo, &tmp_index, &["add", "-A"])?;
        let tree = run_git_with_index(repo, &tmp_index, &["write-tree"])?;
        // Plumbing commit: no hooks, forced identity so a machine
        // without user.name/email configured still checkpoints.
        let snapshot = run_git(
            repo,
            &[
                "-c",
                "user.name=Codemux",
                "-c",
                "user.email=checkpoint@codemux.invalid",
                "commit-tree",
                &tree,
                "-p",
                &head,
                "-m",
                message,
            ],
        )?;
        run_git(repo, &["update-ref", full_ref, &snapshot])?;
        Ok(snapshot)
    })();
    // Always clean up the temp index, success or failure.
    let _ = std::fs::remove_file(&tmp_index);
    let snapshot_commit = result?;

    Ok(Some(GitCheckpoint {
        ref_name: full_ref.to_string(),
        snapshot_commit,
        head_commit: head,
        branch,
    }))
}

/// Roll the working tree back to a checkpoint created by
/// [`git_checkpoint_create`].
///
/// Effect: worktree content == snapshot, run-created files deleted
/// (ignored files spared), run-made commits undone (branch reset to
/// `head_commit`), and the restored pre-run dirty state shown as
/// unstaged changes / untracked files. The pre-run staged-vs-unstaged
/// split is flattened to unstaged — the snapshot records one tree,
/// not the index.
///
/// Before mutating anything, the CURRENT state is snapshotted to
/// `safety_ref` (parented on the run's last commit), so a restore is
/// itself recoverable.
pub fn git_checkpoint_restore(
    repo_path: &Path,
    snapshot_commit: &str,
    head_commit: &str,
    branch: Option<&str>,
    safety_ref: &str,
) -> Result<(), String> {
    let toplevel = run_git(repo_path, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "Cannot restore: not a git repository".to_string())?;
    let repo = Path::new(&toplevel);

    // Both commits must still exist (a pruned checkpoint after `git gc`
    // would otherwise fail halfway through the restore).
    for (label, commit) in [("snapshot", snapshot_commit), ("base", head_commit)] {
        run_git(repo, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).map_err(|_| {
            format!(
                "Cannot restore: the checkpoint {label} commit no longer exists (it may have been pruned)"
            )
        })?;
    }

    // Same-branch guard: restoring a snapshot from branch A while B is
    // checked out would silently reset B to A-era content.
    let current = run_git_permissive(repo, &["branch", "--show-current"]);
    let expected = branch.unwrap_or("");
    if current != expected {
        let cur_label = if current.is_empty() { "(detached)" } else { &current };
        let exp_label = if expected.is_empty() { "(detached)" } else { expected };
        return Err(format!(
            "Cannot restore: the workspace is now on branch '{cur_label}' but the checkpoint was taken on '{exp_label}'. Switch back to that branch first."
        ));
    }

    // Safety net: snapshot the CURRENT state (including any commits the
    // run made — they stay reachable through this ref's parent chain)
    // before we start rewriting the worktree. Best-effort.
    if let Err(error) = git_checkpoint_create(repo, safety_ref, "codemux pre-restore safety snapshot")
    {
        eprintln!("[codemux::git] pre-restore safety snapshot failed: {error}");
    }

    // 1. Worktree + index → snapshot tree. `read-tree --reset -u` is
    //    the plumbing behind `reset --hard`: it overwrites local
    //    modifications and deletes files tracked in the old index that
    //    are absent from the snapshot.
    run_git(repo, &["read-tree", "--reset", "-u", snapshot_commit])?;
    // 2. Delete run-created files. After step 1 the index == snapshot
    //    tree, so anything untracked now did not exist at checkpoint
    //    time. No `-x`: ignored files (node_modules, build dirs, …)
    //    are spared.
    run_git(repo, &["clean", "-fd"])?;
    // 3. Move the branch back to the pre-run commit and reset the
    //    index to it, leaving the snapshot content in the worktree as
    //    unstaged changes. Formerly-untracked files fall out of the
    //    index here and show as untracked again.
    run_git(repo, &["reset", "--mixed", head_commit])?;
    Ok(())
}

/// Delete all but the newest `keep` refs under `namespace` (ordered by
/// committer date, newest first). Returns the deleted ref names so the
/// caller can drop matching bookkeeping rows.
pub fn git_checkpoint_prune(
    repo_path: &Path,
    namespace: &str,
    keep: usize,
) -> Result<Vec<String>, String> {
    let out = run_git(
        repo_path,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)",
            namespace,
        ],
    )?;
    let mut pruned = Vec::new();
    for ref_name in out.lines().filter(|l| !l.is_empty()).skip(keep) {
        run_git(repo_path, &["update-ref", "-d", ref_name])?;
        pruned.push(ref_name.to_string());
    }
    Ok(pruned)
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

    // Resolve upstream (falls back to remote-tracking ref if @{upstream}
    // isn't configured — see resolve_upstream_ref).
    let upstream_ref = resolve_upstream_ref(repo_path);
    let has_upstream = upstream_ref.is_some();
    let unpushed_output = if let Some(ref u) = upstream_ref {
        run_git_permissive(repo_path, &["rev-list", &format!("{u}..HEAD")])
    } else {
        String::new()
    };
    let unpushed: HashSet<&str> = unpushed_output.lines().collect();

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

pub fn get_commit_files(repo_path: &Path, hash: &str) -> Result<Vec<CommitFileEntry>, String> {
    let output = run_git(
        repo_path,
        &["diff-tree", "--no-commit-id", "--name-status", "-r", hash],
    )?;
    let mut files = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Format: "M\tpath" or "A\tpath" or "D\tpath" or "R100\told\tnew"
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let status_code = parts[0].chars().next().unwrap_or('M');
        let status = match status_code {
            'A' => "added",
            'M' => "modified",
            'D' => "deleted",
            'R' => "renamed",
            'C' => "copied",
            _ => "modified",
        };
        // For renames/copies, parts[1] is "old\tnew" — use the new path
        let path = if status_code == 'R' || status_code == 'C' {
            parts[1].split('\t').last().unwrap_or(parts[1])
        } else {
            parts[1]
        };
        files.push(CommitFileEntry {
            path: path.to_string(),
            status: status.to_string(),
        });
    }
    Ok(files)
}

pub fn git_branch_info(repo_path: &Path) -> Result<GitBranchInfo, String> {
    let branch_name = run_git_permissive(repo_path, &["branch", "--show-current"]);
    let branch = if branch_name.is_empty() {
        None
    } else {
        Some(branch_name)
    };

    let upstream_ref = resolve_upstream_ref(repo_path);
    let has_upstream = upstream_ref.is_some();

    let (ahead, behind) = if let Some(ref u) = upstream_ref {
        let range = format!("HEAD...{u}");
        let rev_list = run_git_permissive(
            repo_path,
            &["rev-list", "--left-right", "--count", &range],
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

/// Extract the destination path from a `git diff --numstat` path field,
/// handling git's compact rename forms so the key matches what
/// `git diff --name-status` reports (and stats attach correctly):
///   "old => new"        -> "new"
///   "dir/{old => new}"  -> "dir/new"
///   "{old => new}/file" -> "new/file"
///   "a/{old => new}/b"  -> "a/new/b"
/// Non-rename paths are returned unchanged.
fn numstat_new_path(raw: &str) -> String {
    if !raw.contains(" => ") {
        return raw.to_string();
    }
    if let (Some(open), Some(close)) = (raw.find('{'), raw.find('}')) {
        if open < close {
            let prefix = &raw[..open];
            let inside = &raw[open + 1..close];
            let suffix = &raw[close + 1..];
            let new_part = inside.split(" => ").nth(1).unwrap_or(inside);
            return format!("{prefix}{new_part}{suffix}");
        }
    }
    // No braces: the whole field is "old => new".
    raw.split(" => ").nth(1).unwrap_or(raw).to_string()
}

fn parse_numstat_per_file(unstaged: &str, staged: &str) -> std::collections::HashMap<String, (u32, u32)> {
    let mut map = std::collections::HashMap::new();
    for line in unstaged.lines().chain(staged.lines()) {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let adds = parts[0].parse::<u32>().unwrap_or(0);
            let dels = parts[1].parse::<u32>().unwrap_or(0);
            let path = numstat_new_path(parts[2]);
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
            // Handle renames: numstat shows compact forms like
            // "src/lib/{old => new}" — reconstruct the full destination path.
            let path = numstat_new_path(parts[2]);
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

/// Merge a source branch into the current branch.
/// Returns: "merged" (new commits), "up_to_date" (nothing to merge), or "conflicts".
pub fn merge_branch(repo_path: &Path, source_branch: &str) -> Result<String, String> {
    // Safety: refuse to run on dirty working tree
    let status = git_status(repo_path)?;
    if !status.is_empty() {
        return Err("Cannot merge: working tree has uncommitted changes. Commit or stash your changes first.".to_string());
    }

    let head_before = run_git(repo_path, &["rev-parse", "HEAD"])?;

    let (_stdout, _stderr, success) = run_git_full(
        repo_path,
        &["merge", "--no-ff", source_branch],
    )?;

    if success {
        let head_after = run_git(repo_path, &["rev-parse", "HEAD"])?;
        if head_before.trim() == head_after.trim() {
            return Ok("up_to_date".to_string());
        }
        return Ok("merged".to_string());
    }

    // Check if the failure was due to conflicts (merge in progress)
    let merge_state = get_merge_state(repo_path)?;
    if merge_state.is_merging && !merge_state.conflicted_files.is_empty() {
        return Ok("conflicts".to_string());
    }

    // Some other failure (e.g. branch doesn't exist)
    Err(format!("Merge failed: {}", _stderr.trim()))
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

/// Strip a previous resolver-branch prefix and trailing `-into-<target>-<ts>`
/// from a branch name so a SECOND resolver invocation does not produce
/// `bot/resolve-bot-resolve-...` recursion. Idempotent.
///
/// Recognizes BOTH prefix forms:
///   - `bot/resolve-` (the original, what create_resolver_branch produces)
///   - `bot-resolve-` (the dash form left over inside an already-nested
///     branch name, because `replace('/', '-')` was applied to the prior
///     branch when it was used as `safe_source`)
///
/// Examples:
///   `bot/resolve-feature-into-main-1776365511` -> `feature`
///   `bot/resolve-bot-resolve-foo-into-main-123-into-main-456` -> `foo`
///   `feature/foo` -> `feature/foo` (untouched)
pub(crate) fn strip_resolver_prefix(branch: &str) -> String {
    let mut current = branch.to_string();
    // Repeatedly peel `<prefix>...-into-<anything>-<digits>` layers,
    // where <prefix> is either `bot/resolve-` or `bot-resolve-`.
    loop {
        let body = match current
            .strip_prefix("bot/resolve-")
            .or_else(|| current.strip_prefix("bot-resolve-"))
        {
            Some(rest) => rest,
            None => return current,
        };
        // Find the last `-into-` that is followed by `<segment>-<digits>`.
        let Some(into_idx) = body.rfind("-into-") else {
            return current;
        };
        let after_into = &body[into_idx + "-into-".len()..];
        // Trailing token must be all digits (timestamp).
        let Some(last_dash) = after_into.rfind('-') else {
            return current;
        };
        let trailing = &after_into[last_dash + 1..];
        if trailing.is_empty() || !trailing.chars().all(|c| c.is_ascii_digit()) {
            return current;
        }
        // Peel: keep just the source portion before `-into-`.
        let stripped = body[..into_idx].to_string();
        if stripped.is_empty() {
            return current;
        }
        current = stripped;
    }
}

/// Returns true if `text` contains any git conflict marker line.
/// Looks for `<<<<<<< `, `>>>>>>> `, and `=======` AT THE START OF A LINE,
/// matching how `git merge` writes them. The trailing space (or end-of-line
/// for `=======`) prevents false positives on Markdown rule lines or
/// `<<<<` patterns inside string literals/comments mid-line.
pub(crate) fn has_conflict_markers(text: &str) -> bool {
    for line in text.lines() {
        if line.starts_with("<<<<<<< ")
            || line.starts_with(">>>>>>> ")
            || line == "======="
        {
            return true;
        }
    }
    false
}

/// Scan the listed files (relative to repo_path) for unresolved conflict
/// markers. Returns the paths that still contain markers. Files that don't
/// exist or can't be read are silently skipped — git status check covers
/// "file deleted" cases separately.
pub fn scan_files_for_conflict_markers(
    repo_path: &Path,
    files: &[String],
) -> Vec<String> {
    let mut hits = Vec::new();
    for rel in files {
        let abs = repo_path.join(rel);
        if let Ok(content) = std::fs::read_to_string(&abs) {
            if has_conflict_markers(&content) {
                hits.push(rel.clone());
            }
        }
    }
    hits
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

    // Generate temp branch name. Strip any prior resolver prefix so that a
    // retry from a stale `bot/resolve-*` branch doesn't produce nested names.
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let canonical_source = strip_resolver_prefix(&original_branch);
    let safe_source = canonical_source.replace('/', "-");
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
    // Verify all conflicts are resolved by git's index.
    let status = git_status(repo_path)?;
    let unresolved: Vec<_> = status.iter().filter(|f| f.status == FileStatus::Conflicted).collect();
    if !unresolved.is_empty() {
        return Err(format!(
            "Cannot apply resolution: {} unresolved conflict(s) — agent did not finish",
            unresolved.len()
        ));
    }

    // Defense in depth: even if `git add` cleared the U flag, the file may
    // still contain conflict markers in its content. Scan EVERY tracked file
    // touched by this merge — both staged files and any path git knows about
    // in the merge state.
    let staged_paths: Vec<String> = status
        .iter()
        .filter(|f| f.is_staged)
        .map(|f| f.path.clone())
        .collect();
    let dirty = scan_files_for_conflict_markers(repo_path, &staged_paths);
    if !dirty.is_empty() {
        return Err(format!(
            "Cannot apply resolution: {} file(s) still contain conflict markers ({})",
            dirty.len(),
            dirty.join(", ")
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

// ---- Merge into base (safe resolver-branch pattern) ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeIntoBaseResult {
    pub status: String,                  // "merged", "conflicts", "already_up_to_date"
    pub temp_branch: Option<String>,
    pub source_branch: String,
    pub conflicted_files: Vec<ConflictFile>,
}

/// After `update-ref`, the worktree where `base_branch` is checked out still has
/// stale files on disk.  Find that worktree via `git worktree list --porcelain`
/// and run `reset --hard HEAD` to sync its working tree.
/// Failures are silently ignored — the ref update already succeeded and stale
/// files are annoying but not data-loss.
fn refresh_base_worktree(repo_path: &Path, base_branch: &str) {
    let output = match run_git(repo_path, &["worktree", "list", "--porcelain"]) {
        Ok(o) => o,
        Err(_) => return,
    };

    let target_ref = format!("refs/heads/{}", base_branch);
    let mut current_path: Option<&str> = None;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(path);
        } else if let Some(branch) = line.strip_prefix("branch ") {
            if branch == target_ref {
                if let Some(path) = current_path {
                    let _ = run_git(Path::new(path), &["reset", "--hard", "HEAD"]);
                    return;
                }
            }
        } else if line.is_empty() {
            current_path = None;
        }
    }
}

/// Safely merge the current branch into a base branch using a temporary resolver branch.
/// Main is NEVER modified until the merge is proven clean.
pub fn merge_into_base(repo_path: &Path, base_branch: &str, delete_source_branch: bool) -> Result<MergeIntoBaseResult, String> {
    // Safety: refuse to run on dirty working tree
    let status = git_status(repo_path)?;
    if !status.is_empty() {
        return Err("Cannot merge: working tree has uncommitted changes. Commit or stash your changes first.".to_string());
    }

    let source_branch = run_git(repo_path, &["branch", "--show-current"])?;
    if source_branch.is_empty() {
        return Err("Cannot merge: not on a named branch (detached HEAD)".to_string());
    }
    if source_branch == base_branch {
        return Err(format!("Already on {base_branch}. Switch to a feature branch first."));
    }

    // Check if there's anything to merge
    let merge_base = run_git(repo_path, &["merge-base", &source_branch, base_branch])?;
    let base_head = run_git(repo_path, &["rev-parse", base_branch])?;
    let source_head = run_git(repo_path, &["rev-parse", &source_branch])?;

    // If the source branch head IS the merge base, there's nothing new to merge into base
    if source_head.trim() == merge_base.trim() {
        return Ok(MergeIntoBaseResult {
            status: "already_up_to_date".to_string(),
            temp_branch: None,
            source_branch,
            conflicted_files: Vec::new(),
        });
    }

    // If the base already contains the source, also up to date
    let (_, _, is_ancestor) = run_git_full(
        repo_path,
        &["merge-base", "--is-ancestor", &source_head, &base_head],
    )?;
    if is_ancestor {
        return Ok(MergeIntoBaseResult {
            status: "already_up_to_date".to_string(),
            temp_branch: None,
            source_branch,
            conflicted_files: Vec::new(),
        });
    }

    // Generate temp branch name
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let safe_source = source_branch.replace('/', "-");
    let safe_base = base_branch.replace('/', "-");
    let temp_branch = format!("merge/{}-into-{}-{}", safe_source, safe_base, timestamp);

    // Create temp branch from base
    run_git(repo_path, &["checkout", "-b", &temp_branch, base_branch])?;

    // Merge source into temp branch
    let (_stdout, _stderr, success) = run_git_full(
        repo_path,
        &["merge", "--no-ff", "-m", &format!("Merge {} into {}", source_branch, base_branch), &source_branch],
    )?;

    if success {
        // Clean merge — update base ref directly (no checkout, works in worktrees)
        // Verify fast-forward is safe: base must be ancestor of temp
        let (_, _, is_ff) = run_git_full(
            repo_path,
            &["merge-base", "--is-ancestor", base_branch, &temp_branch],
        )?;
        if !is_ff {
            // Base diverged while we were merging — abort safely
            run_git(repo_path, &["checkout", &source_branch])?;
            let _ = run_git(repo_path, &["branch", "-D", &temp_branch]);
            return Err("Cannot fast-forward base branch — history has diverged".to_string());
        }
        let temp_commit = run_git(repo_path, &["rev-parse", &temp_branch])?;
        run_git(repo_path, &["update-ref", &format!("refs/heads/{}", base_branch), temp_commit.trim()])?;
        refresh_base_worktree(repo_path, base_branch);

        if delete_source_branch {
            // Delete source while still on temp (can't delete a checked-out branch)
            let _ = run_git(repo_path, &["branch", "-d", &source_branch]);
            // Land on base if possible, otherwise detach (worktree scenario)
            if run_git(repo_path, &["checkout", base_branch]).is_err() {
                run_git(repo_path, &["checkout", "--detach"])?;
            }
        } else {
            run_git(repo_path, &["checkout", &source_branch])?;
        }
        // Force-delete temp: -d would fail because temp isn't merged into HEAD (source),
        // but its content is safely in base via branch -f above
        let _ = run_git(repo_path, &["branch", "-D", &temp_branch]);

        return Ok(MergeIntoBaseResult {
            status: "merged".to_string(),
            temp_branch: None,
            source_branch,
            conflicted_files: Vec::new(),
        });
    }

    // Conflicts — stay on temp branch for resolution
    let conflict_status = git_status(repo_path).unwrap_or_default();
    let conflicted_files: Vec<ConflictFile> = conflict_status
        .iter()
        .filter(|f| f.status == FileStatus::Conflicted)
        .map(|f| ConflictFile {
            path: f.path.clone(),
            conflict_type: f.conflict_type.clone().unwrap_or_else(|| "both_modified".to_string()),
        })
        .collect();

    if conflicted_files.is_empty() {
        // Merge failed but no conflicts — something else went wrong
        let _ = run_git(repo_path, &["merge", "--abort"]);
        run_git(repo_path, &["checkout", &source_branch])?;
        let _ = run_git(repo_path, &["branch", "-D", &temp_branch]);
        return Err(format!("Merge failed: {}", _stderr.trim()));
    }

    Ok(MergeIntoBaseResult {
        status: "conflicts".to_string(),
        temp_branch: Some(temp_branch),
        source_branch,
        conflicted_files,
    })
}

/// Complete the merge-into-base flow after conflict resolution.
pub fn complete_merge_into_base(
    repo_path: &Path,
    base_branch: &str,
    temp_branch: &str,
    source_branch: &str,
    delete_source_branch: bool,
) -> Result<(), String> {
    // Verify all conflicts are resolved
    let status = git_status(repo_path)?;
    let unresolved: Vec<_> = status.iter().filter(|f| f.status == FileStatus::Conflicted).collect();
    if !unresolved.is_empty() {
        return Err(format!("{} unresolved conflict(s) remain", unresolved.len()));
    }

    // Commit the merge on temp branch
    let merge_state = get_merge_state(repo_path)?;
    if merge_state.is_merging {
        run_git(repo_path, &["commit", "--no-edit"])?;
    }

    // Update base ref directly (no checkout, works in worktrees)
    // Verify fast-forward is safe: base must be ancestor of temp
    let (_, _, is_ff) = run_git_full(
        repo_path,
        &["merge-base", "--is-ancestor", base_branch, temp_branch],
    )?;
    if !is_ff {
        return Err("Cannot fast-forward base branch — history has diverged".to_string());
    }
    let temp_commit = run_git(repo_path, &["rev-parse", temp_branch])?;
    run_git(repo_path, &["update-ref", &format!("refs/heads/{}", base_branch), temp_commit.trim()])?;
    refresh_base_worktree(repo_path, base_branch);

    if delete_source_branch {
        // Delete source while still on temp (can't delete a checked-out branch)
        let _ = run_git(repo_path, &["branch", "-d", source_branch]);
        // Land on base if possible, otherwise detach (worktree scenario)
        if run_git(repo_path, &["checkout", base_branch]).is_err() {
            run_git(repo_path, &["checkout", "--detach"])?;
        }
    } else {
        run_git(repo_path, &["checkout", source_branch])?;
    }

    // Force-delete temp: -d would fail because temp isn't merged into HEAD (source),
    // but its content is safely in base via branch -f above
    let _ = run_git(repo_path, &["branch", "-D", temp_branch]);

    Ok(())
}

/// Abort the merge-into-base flow, restoring everything to pre-merge state.
pub fn abort_merge_into_base(
    repo_path: &Path,
    source_branch: &str,
    temp_branch: &str,
) -> Result<(), String> {
    // Abort any in-progress merge on temp branch
    let _ = run_git(repo_path, &["merge", "--abort"]);

    // Switch back to source branch
    run_git(repo_path, &["checkout", source_branch])?;

    // Force-delete the temp branch
    let _ = run_git(repo_path, &["branch", "-D", temp_branch]);

    Ok(())
}

// ---- Worktree operations ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchDetail {
    pub name: String,
    pub last_commit_unix: i64,
    pub is_local: bool,
    pub is_remote: bool,
}

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

/// Bare `git init` — creates the repository and stops. It never runs
/// `git add` or `git commit`, so no user files are staged and no commit
/// is created (HEAD stays unborn until the user makes their own first
/// commit).
///
/// This is the split-out counterpart of `git_init_repo` above, and the
/// two are intentionally different:
///
/// - `git_init_no_commit` backs the user-facing "Initialize Git"
///   affordance (`use-initialize-git.ts`), which turns an *existing*
///   folder full of the user's own files into a repo on an explicit
///   click. That flow must NEVER stage or commit anything — the folder
///   may hold secrets, `node_modules`, or anything else the user has
///   not vetted, and there is no `.gitignore` yet. Silently committing
///   all of it would be a data-safety footgun.
/// - `git_init_repo` stays commit-ful because its only caller is
///   `create_empty_repo`'s "new empty project" flow, where the folder
///   was just created by us and an initial (empty-ish) commit is the
///   desired starting point.
pub fn git_init_no_commit(path: &Path) -> Result<String, String> {
    run_git(path, &["init"])?;
    Ok("Repository initialized".to_string())
}

pub fn git_list_branches(repo_path: &Path, remote: bool) -> Result<Vec<String>, String> {
    // We use `for-each-ref` with the FULL refname rather than `git branch
    // --format=%(refname:short)`, because `:short` collapses
    // `refs/remotes/<remote>/HEAD` to just `<remote>` (e.g. "origin",
    // "upstream"). That ambiguous short form slips past any "contains HEAD"
    // filter and surfaces in the picker as a phantom branch named after the
    // remote — which then becomes the worktree directory name if selected.
    // Filtering on the full refname's `/HEAD` suffix is unambiguous and
    // works for every remote.
    let pattern = if remote { "refs/remotes/" } else { "refs/heads/" };
    let output = run_git(repo_path, &["for-each-ref", "--format=%(refname)", pattern])?;
    let branches: Vec<String> = output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.ends_with("/HEAD"))
        .map(|l| {
            if remote {
                // Strip `refs/remotes/origin/` for origin (preserves prior
                // behavior of bare names), and `refs/remotes/` for any
                // other remote (so e.g. `upstream/master` stays qualified
                // and doesn't collide with origin's branches).
                if let Some(rest) = l.strip_prefix("refs/remotes/origin/") {
                    rest.to_string()
                } else if let Some(rest) = l.strip_prefix("refs/remotes/") {
                    rest.to_string()
                } else {
                    l.to_string()
                }
            } else {
                l.strip_prefix("refs/heads/").unwrap_or(l).to_string()
            }
        })
        .collect();
    Ok(branches)
}

pub fn git_list_branches_detailed(repo_path: &Path) -> Result<Vec<BranchDetail>, String> {
    use std::collections::HashMap;

    // Use full refnames so we can reliably detect and skip the
    // `refs/remotes/origin/HEAD` symref. `%(refname:short)` collapses it to
    // just `"origin"`, which silently leaks into the branch list and ends
    // up as a worktree directory name if the user selects it.
    let output = run_git(
        repo_path,
        &[
            "for-each-ref",
            "--format=%(refname)\t%(committerdate:unix)",
            "refs/heads/",
            "refs/remotes/origin/",
        ],
    )?;

    let mut map: HashMap<String, BranchDetail> = HashMap::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let (refname, timestamp_str) = match line.split_once('\t') {
            Some(pair) => pair,
            None => continue,
        };

        // Skip any `<remote>/HEAD` symref — they alias another branch and
        // would otherwise dedupe-collide on an empty/short name.
        if refname.ends_with("/HEAD") {
            continue;
        }

        // Classify as local or remote and strip the namespace prefix.
        let (name, is_remote) = if let Some(rest) = refname.strip_prefix("refs/heads/") {
            (rest, false)
        } else if let Some(rest) = refname.strip_prefix("refs/remotes/origin/") {
            (rest, true)
        } else {
            // Anything outside refs/heads/ and refs/remotes/origin/ — the
            // for-each-ref patterns shouldn't return these, but be defensive.
            continue;
        };

        let timestamp: i64 = timestamp_str.trim().parse().unwrap_or(0);

        match map.get_mut(name) {
            Some(existing) => {
                // Merge: mark both flags, keep the newer timestamp
                if is_remote {
                    existing.is_remote = true;
                } else {
                    existing.is_local = true;
                }
                if timestamp > existing.last_commit_unix {
                    existing.last_commit_unix = timestamp;
                }
            }
            None => {
                map.insert(
                    name.to_string(),
                    BranchDetail {
                        name: name.to_string(),
                        last_commit_unix: timestamp,
                        is_local: !is_remote,
                        is_remote,
                    },
                );
            }
        }
    }

    let mut branches: Vec<BranchDetail> = map.into_values().collect();
    branches.sort_by(|a, b| b.last_commit_unix.cmp(&a.last_commit_unix));

    Ok(branches)
}

/// Find the remote tracking ref for a branch (e.g. `origin/main` for `main`).
/// Find the default branch for the repo (main, master, etc.).
/// `pub` so the cross-device daemon (`remote::workspace`) and the protected-root
/// classifier can reuse the same resolution logic. Does NOT supersede
/// `git_default_branch` — that one is the frontend-facing command with a
/// slightly different fallback (`Ok("main")` instead of `None`). Consolidating
/// the two is a separate follow-up.
///
/// IMPORTANT: this resolver is intentionally *conservative* — `origin/HEAD`,
/// then `main`, then `master`, else `None`. It deliberately does NOT fall back
/// to "whatever branch is currently checked out", because two callers rely on
/// that strictness: `is_protected_repo_root` (a current-branch fallback would
/// protect a root on any branch) and `git_create_worktree` (it checks out the
/// default to *free* a branch — a current-branch fallback would return the very
/// branch it's trying to free). For the looser "best effort, never None for a
/// real repo" form used to stamp the informational `default_branch` column, use
/// [`resolve_default_branch`].
pub fn find_default_branch(repo_path: &Path) -> Option<String> {
    // Try origin/HEAD first
    if let Ok(output) = run_git(repo_path, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        if let Some(branch) = output.trim().strip_prefix("refs/remotes/origin/") {
            return Some(branch.to_string());
        }
    }
    // Fallback: try main, then master
    for candidate in &["main", "master"] {
        if run_git(repo_path, &["rev-parse", "--verify", *candidate]).is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Best-effort default-branch resolution for *recording* a repo's default
/// branch (the cross-device `default_branch` hint), as opposed to the strict
/// [`find_default_branch`] used for protection/worktree decisions.
///
/// Order: `origin/HEAD` → `main`/`master` → the currently checked-out branch.
/// The last step is what makes this work for **local-only repos whose default
/// branch is neither `main` nor `master`** (no remote to read `origin/HEAD`
/// from, e.g. a repo defaulting to `trunk`/`develop`). Returns `None` only for
/// a detached/unborn HEAD on such a repo.
///
/// This value is advisory metadata. The authoritative protection decision is
/// always recomputed locally via [`is_protected_repo_root`] against the
/// materialized copy, so a slightly-off hint can never cause a wrong protected
/// stamp.
pub fn resolve_default_branch(repo_path: &Path) -> Option<String> {
    if let Some(b) = find_default_branch(repo_path) {
        return Some(b);
    }
    let current = run_git_permissive(repo_path, &["branch", "--show-current"]);
    if current.is_empty() {
        None
    } else {
        Some(current)
    }
}

/// Populate `refs/remotes/origin/HEAD` so [`find_default_branch`]'s
/// `origin/HEAD` probe succeeds for freshly-cloned repos. Runs
/// `git remote set-head origin --auto`.
///
/// **No-op by design when there is no `origin` remote** (e.g. local-only repos
/// adopted via rsync) — calling `set-head` there errors with
/// "No such remote 'origin'". Any failure of the underlying set-head (offline,
/// transient) is swallowed: this only ever *improves* default-branch
/// resolution, never blocks adoption.
pub fn ensure_origin_head(repo_path: &Path) -> Result<(), String> {
    let remotes = run_git_permissive(repo_path, &["remote"]);
    if !remotes.lines().any(|r| r.trim() == "origin") {
        return Ok(());
    }
    let _ = run_git(repo_path, &["remote", "set-head", "origin", "--auto"]);
    Ok(())
}

/// Switch the repo to its default branch (main / master / whatever
/// `origin/HEAD` points at). Returns:
/// - `Ok(Some(branch_name))` on success, or if the repo is already on the
///   default branch (no checkout actually runs in the already-on-default case).
/// - `Ok(None)` if no default branch could be determined (unborn HEAD, no
///   main/master, no `origin/HEAD` symref).
/// - `Err(stderr)` if git refused the checkout — dirty working tree with
///   conflicts, rebase/merge in progress, `index.lock` contention, etc. The
///   error string is passed through verbatim so callers can surface git's
///   own message. Never destructive: git's native refusal protects work.
pub fn checkout_default_branch(repo_path: &Path) -> Result<Option<String>, String> {
    let default = match find_default_branch(repo_path) {
        Some(b) => b,
        None => return Ok(None),
    };
    let current = run_git_permissive(repo_path, &["branch", "--show-current"]);
    if current == default {
        return Ok(Some(default));
    }
    run_git(repo_path, &["checkout", &default])?;
    Ok(Some(default))
}

/// Returns `None` if no remote ref exists. Only checks `origin` remote
/// (consistent with the rest of the codebase).
pub fn find_remote_ref(repo_path: &Path, branch: &str) -> Option<String> {
    // Already a remote or absolute ref — return as-is
    if branch.starts_with("origin/") || branch.starts_with("refs/") {
        return Some(branch.to_string());
    }
    let remote_ref = format!("origin/{branch}");
    if run_git(
        repo_path,
        &["rev-parse", "--verify", &format!("refs/remotes/{remote_ref}")],
    )
    .is_ok()
    {
        Some(remote_ref)
    } else {
        None
    }
}

/// Wall-clock cap for the best-effort base fetch in
/// [`fetch_origin_branch`]. Matches the 10s per-fetch timeout used by the
/// background sidebar-divergence fetcher: long enough for a healthy remote,
/// short enough that an unreachable one doesn't stall worktree creation.
const BASE_FETCH_TIMEOUT_SECS: u64 = 10;

/// Best-effort, scoped `git fetch origin <branch>` so
/// `refs/remotes/origin/<branch>` reflects the true remote tip before a
/// caller resolves it (e.g. as the base of a new workspace branch).
/// Without this, `find_remote_ref` resolves to whatever the last fetch
/// left behind — a potentially stale snapshot of the remote.
///
/// Returns `true` iff the fetch ran and succeeded. Designed to never
/// block or fail worktree creation:
/// - Scoped to a single branch (not every ref) to keep latency low.
/// - Skipped entirely when there is no `origin` remote (local-only repos)
///   or when `branch` is an absolute `refs/...` path.
/// - `origin/<b>` inputs are normalised to `<b>` so callers can pass a
///   base in either form.
/// - `GIT_TERMINAL_PROMPT=0` so git can never hang on a credential prompt.
/// - Killed after [`BASE_FETCH_TIMEOUT_SECS`] if the remote is slow or
///   unreachable (a plain `.output()` would block indefinitely on a
///   half-open connection).
/// - All failures (offline, auth, missing branch) are swallowed — callers
///   fall back to the existing local `origin/<b>` / `<b>` refs.
pub fn fetch_origin_branch(repo_path: &Path, branch: &str) -> bool {
    let branch = match branch.strip_prefix("origin/") {
        Some(rest) => rest,
        None if branch.starts_with("refs/") => return false,
        None => branch,
    };
    if branch.is_empty() {
        return false;
    }
    // Cheap local check first: `git fetch origin` on a repo with no
    // `origin` remote errors immediately, but skipping it avoids spawning
    // a doomed subprocess for the common local-only-repo case.
    let remotes = run_git_permissive(repo_path, &["remote"]);
    if !remotes.lines().any(|r| r.trim() == "origin") {
        return false;
    }
    let Ok(mut child) = Command::new("git")
        .args(["fetch", "--quiet", "--no-tags", "origin", branch])
        .current_dir(repo_path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(BASE_FETCH_TIMEOUT_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => {
                // Can't observe the child any more; don't leak it.
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Returns true iff `git worktree list --porcelain` for `repo_path` contains
/// an entry whose `worktree` line matches `target_path` AND whose `branch`
/// line matches `refs/heads/<branch>` (the form git uses for porcelain
/// output). Used by `git_create_worktree` to short-circuit when the brain
/// re-asks for a worktree that's already on disk and attached to the
/// requested branch.
///
/// The porcelain format groups records by blank lines. Each record looks
/// like:
///   worktree /home/.../<repo>/<branch>
///   HEAD <sha>
///   branch refs/heads/<branch>
///
/// Detached worktrees print `detached` instead of `branch …`; we treat
/// those as non-match because callers always pass a branch name and a
/// detached worktree at the path is not what they asked for.
fn existing_worktree_matches(repo_path: &Path, target_path: &str, branch: &str) -> bool {
    let Ok(output) = run_git(repo_path, &["worktree", "list", "--porcelain"]) else {
        return false;
    };
    let expected_branch_ref = format!("refs/heads/{branch}");
    // Compare paths as `Path`, not `&str`: on Windows `git worktree list`
    // emits forward-slash paths (`C:/Users/...`) while our `target_path`
    // comes from a `PathBuf` and uses backslashes (`C:\Users\...`). A raw
    // string compare never matches there, so the reuse short-circuit in
    // `git_create_worktree` is skipped and git re-runs `worktree add`,
    // failing with `fatal: '<path>' already exists`. `Path` equality
    // treats `/` and `\` as equivalent separators on Windows.
    let target = Path::new(target_path);
    let path_matches = |p: &Option<String>| p.as_deref().map(Path::new) == Some(target);
    let mut current_path: Option<String> = None;
    let mut current_branch: Option<String> = None;
    for raw in output.lines() {
        let line = raw.trim();
        if line.is_empty() {
            // End of a record — evaluate before resetting.
            if path_matches(&current_path)
                && current_branch.as_deref() == Some(&expected_branch_ref)
            {
                return true;
            }
            current_path = None;
            current_branch = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            current_path = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            current_branch = Some(rest.to_string());
        }
    }
    // Last record may not be terminated by a blank line.
    path_matches(&current_path) && current_branch.as_deref() == Some(&expected_branch_ref)
}

/// Recreate a worktree for an EXISTING branch inside a repo that was
/// copied from another machine (e.g. rsynced from a `codemux-remote`
/// host during worktree adoption).
///
/// Such a repo carries the host's worktree admin entries under
/// `.git/worktrees/<name>/`, each pointing at an absolute path that
/// doesn't exist on this machine. Left in place, those stale entries
/// make git think the branch is "already checked out" and refuse a
/// fresh `git worktree add`. We `git worktree prune` first (drops every
/// registration whose path is gone — which is all of them, since they
/// reference the source machine), then add a clean local worktree for
/// the branch via `git_create_worktree`.
///
/// Returns the local worktree path. The branch must already exist in
/// the repo (it came over with the rsync); we attach it, never create.
/// Identity of the git repository a path belongs to, resolved via a
/// single `git rev-parse`. Distinguishes the genuine repo root from a
/// linked worktree, and surfaces the *canonical* root (the main working
/// tree) so the sync layer can treat a repo + its worktrees as one
/// connected unit instead of independent folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRoot {
    /// Working-tree root (`git rev-parse --show-toplevel`). `None` for a
    /// bare repository (no working tree).
    pub toplevel: Option<PathBuf>,
    /// The shared git dir (`--git-common-dir`), absolute. For a linked
    /// worktree this points at the parent repo's `.git`; for the main
    /// checkout it is this repo's own `.git`.
    pub common_dir: PathBuf,
    /// True when this path is a linked worktree — its per-worktree
    /// `--git-dir` differs from the shared `--git-common-dir`.
    pub is_worktree: bool,
    /// True for a bare repository.
    pub is_bare: bool,
}

impl RepoRoot {
    /// The canonical repo root: the main working tree that owns the
    /// shared object store. For a linked worktree this is the *parent*
    /// repo (not the worktree dir); for the main checkout it is the
    /// checkout itself. Derived from `common_dir` by stripping the
    /// trailing `.git` leaf, falling back to `toplevel` for unusual /
    /// bare layouts.
    pub fn canonical_root_path(&self) -> PathBuf {
        if self.common_dir.file_name().and_then(|n| n.to_str()) == Some(".git") {
            self.common_dir
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.common_dir.clone())
        } else {
            // Bare repo or non-standard layout: the common dir IS the repo.
            self.toplevel
                .clone()
                .unwrap_or_else(|| self.common_dir.clone())
        }
    }
}

/// Resolve the canonical git identity of `path`. Returns `None` when the
/// path is not inside a git repository. Uses one combined `rev-parse`
/// for `--git-dir`/`--git-common-dir` (both valid in bare repos and
/// linked worktrees), then a permissive `--show-toplevel` (which is empty
/// for bare repos) and a permissive `--is-bare-repository`.
pub fn git_canonical_root(path: &Path) -> Option<RepoRoot> {
    let combined = run_git(
        path,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ],
    )
    .ok()?;
    let mut lines = combined.lines();
    let resolve = |raw: &str| -> Option<PathBuf> {
        let s = raw.trim();
        if s.is_empty() {
            return None;
        }
        let p = PathBuf::from(s);
        Some(if p.is_absolute() { p } else { path.join(p) })
    };
    let git_dir = resolve(lines.next()?)?;
    let common_dir = resolve(lines.next()?)?;

    let is_bare = run_git_permissive(path, &["rev-parse", "--is-bare-repository"]) == "true";
    let toplevel = {
        let raw = run_git_permissive(path, &["rev-parse", "--path-format=absolute", "--show-toplevel"]);
        let t = raw.trim();
        if t.is_empty() {
            None
        } else {
            Some(PathBuf::from(t))
        }
    };

    Some(RepoRoot {
        toplevel,
        is_worktree: git_dir != common_dir,
        common_dir,
        is_bare,
    })
}

/// True when `path` looks like a *divergent full copy* of a repo rather
/// than a genuine linked worktree — the failure mode the repo-unit sync
/// fix targets. A real linked worktree always has a `.git` **file**
/// (a gitdir pointer); a divergent copy has its own `.git` **directory**
/// (its own object store) yet sits under `~/.codemux/worktrees/`, where
/// only linked worktrees are ever supposed to live. So a `.git` directory
/// in that tree is the unmistakable signature of a whole-dir copy (the
/// old default-branch rsync fallback), not a legitimate checkout.
pub fn is_divergent_copy(path: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    is_divergent_copy_under(path, &home.join(".codemux").join("worktrees"))
}

/// Testable core of [`is_divergent_copy`] with the worktrees root injected.
fn is_divergent_copy_under(path: &Path, worktrees_root: &Path) -> bool {
    path.join(".git").is_dir() && path.starts_with(worktrees_root)
}

/// Why a divergent copy can't be "reconciled away" (its workspace card
/// detached) yet, or `None` if it's safe. Blocks on a dirty working tree or
/// committed-but-unpushed work so a card never silently vanishes while the
/// user has changes in flight. Reconcile never deletes files, so this is a
/// guard against *confusion*, not data loss.
pub fn reconcile_copy_blocker(path: &Path) -> Option<String> {
    if let Ok(files) = git_status(path) {
        if !files.is_empty() {
            return Some(format!(
                "{} uncommitted change(s) — commit or stash first",
                files.len()
            ));
        }
    }
    if let Some(upstream) = resolve_upstream_ref(path) {
        let out = run_git_permissive(
            path,
            &["rev-list", "--count", &format!("{upstream}..HEAD")],
        );
        if let Ok(n) = out.trim().parse::<u64>() {
            if n > 0 {
                return Some(format!("{n} unpushed commit(s) — push first"));
            }
        }
    }
    None
}

/// True when `checkout` is the repo's protected default-branch root —
/// the entry the overview must not let you delete like a disposable
/// worktree. It qualifies only when ALL hold:
/// - it's a genuine main checkout (not a linked worktree, not bare),
/// - it is NOT a divergent full copy living in the worktrees tree
///   (those are reconciled, not protected — see [`is_divergent_copy`]),
/// - and it is checked out on the repo's default branch.
///
/// Returns `false` (fail-open to "deletable") for non-repos, detached
/// HEADs, or when the default branch can't be resolved — we only ever
/// add protection when we're sure, never block deletion on a guess.
pub fn is_protected_repo_root(checkout: &Path, git_branch: Option<&str>) -> bool {
    let Some(info) = git_canonical_root(checkout) else {
        return false;
    };
    if info.is_worktree || info.is_bare {
        return false;
    }
    if is_divergent_copy(checkout) {
        return false;
    }
    match (git_branch, find_default_branch(checkout)) {
        (Some(branch), Some(default)) => branch == default,
        _ => false,
    }
}

#[cfg(test)]
mod repo_root_tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &Path) {
        run(dir, &["init", "--initial-branch=main"]);
        run(dir, &["config", "user.email", "test@example.com"]);
        run(dir, &["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-m", "init"]);
    }

    /// Compare two paths by resolved identity rather than raw string —
    /// git emits forward-slash, non-verbatim paths while Rust's
    /// `canonicalize` yields `\\?\` extended-length paths on Windows and
    /// `/private/var` on macOS. Canonicalising both sides makes the
    /// equality assertions platform-agnostic; we feed git the
    /// non-canonicalised temp path so it never sees a `\\?\` prefix
    /// (which `git worktree add` rejects on Windows).
    fn same_path(a: &Path, b: &Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => a == b,
        }
    }

    #[test]
    fn canonical_root_for_main_checkout_is_itself() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        let info = git_canonical_root(&repo).expect("repo root");
        assert!(!info.is_worktree, "main checkout is not a worktree");
        assert!(!info.is_bare);
        assert!(same_path(info.toplevel.as_deref().unwrap(), &repo));
        assert!(same_path(&info.common_dir, &repo.join(".git")));
        assert!(same_path(&info.canonical_root_path(), &repo));
    }

    #[test]
    fn canonical_root_for_linked_worktree_points_at_parent() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        let wt = tmp.path().join("wt");
        run(&repo, &["worktree", "add", "-b", "feature", wt.to_str().unwrap()]);

        // A real linked worktree has a `.git` FILE.
        assert!(wt.join(".git").is_file(), "linked worktree gitfile");

        let info = git_canonical_root(&wt).expect("worktree root");
        assert!(info.is_worktree, "linked worktree detected");
        assert!(same_path(&info.common_dir, &repo.join(".git")));
        // The canonical root is the PARENT repo, not the worktree dir.
        assert!(same_path(&info.canonical_root_path(), &repo));
    }

    #[test]
    fn canonical_root_none_for_non_repo() {
        let tmp = TempDir::new().unwrap();
        assert!(git_canonical_root(tmp.path()).is_none());
    }

    #[test]
    fn divergent_copy_detected_only_for_git_dir_under_worktrees() {
        let tmp = TempDir::new().unwrap();
        let worktrees = tmp.path().join(".codemux").join("worktrees");

        // A whole-dir COPY: `.git` is a directory, living under the
        // worktrees tree -> divergent copy (the bug signature).
        let copy = worktrees.join("passpage").join("main");
        std::fs::create_dir_all(copy.join(".git")).unwrap();
        assert!(is_divergent_copy_under(&copy, &worktrees));

        // A genuine linked worktree: `.git` is a FILE -> not a copy.
        let real_wt = worktrees.join("passpage").join("feature");
        std::fs::create_dir_all(&real_wt).unwrap();
        std::fs::write(real_wt.join(".git"), "gitdir: /repo/.git/worktrees/feature\n").unwrap();
        assert!(!is_divergent_copy_under(&real_wt, &worktrees));

        // A real repo root OUTSIDE the worktrees tree -> not a copy even
        // though `.git` is a directory.
        let real_root = tmp.path().join("projects").join("passpage");
        std::fs::create_dir_all(real_root.join(".git")).unwrap();
        assert!(!is_divergent_copy_under(&real_root, &worktrees));
    }

    #[test]
    fn protected_for_main_checkout_on_default_branch() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        assert!(is_protected_repo_root(&repo, Some("main")));
    }

    #[test]
    fn not_protected_for_worktree_or_wrong_branch_or_non_repo() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        // A linked worktree is never the protected root.
        let wt = root.join("wt");
        run(&repo, &["worktree", "add", "-b", "feature", wt.to_str().unwrap()]);
        assert!(!is_protected_repo_root(&wt, Some("feature")));

        // The root checked out on a NON-default branch is not protected
        // (only the default-branch checkout is).
        run(&repo, &["checkout", "-b", "side"]);
        assert!(!is_protected_repo_root(&repo, Some("side")));

        // Not a repo at all.
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(!is_protected_repo_root(&plain, Some("main")));
    }

    #[test]
    fn resolve_default_branch_falls_back_to_current_for_nonstandard_local_repo() {
        // A local-only repo whose default branch is neither `main` nor
        // `master` and which has no remote — exactly the `passpage`-shaped
        // case where `origin/HEAD` can't be read.
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "--initial-branch=trunk"]);
        run(&repo, &["config", "user.email", "t@example.com"]);
        run(&repo, &["config", "user.name", "T"]);
        std::fs::write(repo.join("f"), "x").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-m", "init"]);

        // The strict resolver gives up (no origin/HEAD, no main, no master)…
        assert_eq!(find_default_branch(&repo), None);
        // …but the record-stamping resolver falls back to the current branch,
        // so the daemon still records a usable default for a local-only repo.
        assert_eq!(resolve_default_branch(&repo).as_deref(), Some("trunk"));
    }

    #[test]
    fn reconcile_copy_blocker_flags_dirty_allows_clean() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        // Clean, no upstream → safe to reconcile (nothing blocks).
        assert!(reconcile_copy_blocker(&repo).is_none());

        // Uncommitted change → blocked with guidance.
        std::fs::write(repo.join("README.md"), "changed").unwrap();
        let blocker = reconcile_copy_blocker(&repo).expect("dirty tree blocks");
        assert!(blocker.contains("uncommitted"), "got: {blocker}");
    }

    #[test]
    fn ensure_origin_head_is_noop_without_origin_remote() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        // No `origin` remote — must succeed (not error on "No such remote")
        // and leave default-branch resolution working via the main fallback.
        assert!(ensure_origin_head(&repo).is_ok());
        assert_eq!(find_default_branch(&repo).as_deref(), Some("main"));
    }
}

pub fn git_recreate_worktree_for_adopted_repo(
    repo_path: &Path,
    branch: &str,
) -> Result<String, String> {
    // Prune stale worktree registrations inherited from the source
    // machine. Permissive: a fresh clone with no worktrees is a no-op,
    // and any failure here shouldn't block the add (which will surface
    // its own clear error if the state is genuinely broken).
    let _ = run_git_permissive(repo_path, &["worktree", "prune"]);
    git_create_worktree(repo_path, branch, false, None, None)
}

/// True when `git worktree list` reports a worktree registered at
/// `target_path` for ANY branch. Used to refuse reclaiming a directory
/// git still tracks — removing it would corrupt that worktree.
fn path_is_registered_worktree(repo_path: &Path, target_path: &Path) -> bool {
    let Ok(output) = run_git(repo_path, &["worktree", "list", "--porcelain"]) else {
        return false;
    };
    output.lines().any(|line| {
        line.trim()
            .strip_prefix("worktree ")
            .map(|p| Path::new(p) == target_path)
            .unwrap_or(false)
    })
}

/// Decide whether the directory already sitting at a would-be worktree
/// path is a safe-to-remove leftover: an orphaned Codemux worktree dir
/// whose git registration is already gone, holding nothing but Codemux's
/// own metadata. Returns true ONLY when deleting it cannot lose user data
/// or corrupt a live worktree.
///
/// This unblocks the common case where a previous worktree for the same
/// (often issue-derived) branch was pruned from git's registry but its
/// directory was left on disk: without reclaiming, `git worktree add`
/// dies with `fatal: '<path>' already exists`.
fn is_reclaimable_orphan_worktree(repo_path: &Path, path: &Path) -> bool {
    // Never touch a path git still tracks as a worktree (for ANY branch):
    // removing it would corrupt that worktree. A path collision against a
    // different branch must keep failing loudly.
    if path_is_registered_worktree(repo_path, path) {
        return false;
    }
    // A `.git` entry marks a real worktree checkout (possibly mid-teardown,
    // or pruned-but-not-cleaned). Treat as user data — don't delete.
    if path.join(".git").exists() {
        return false;
    }
    // Every top-level entry must be Codemux-owned metadata; anything else
    // (source, configs, the user's edits) makes removal unsafe. An empty
    // directory is trivially reclaimable.
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    const CODEMUX_METADATA: &[&str] = &[".codemux"];
    for entry in entries {
        let Ok(entry) = entry else { return false };
        match entry.file_name().to_str() {
            Some(name) if CODEMUX_METADATA.contains(&name) => continue,
            _ => return false,
        }
    }
    true
}

pub fn git_create_worktree(
    repo_path: &Path,
    branch: &str,
    new_branch: bool,
    base: Option<&str>,
    pr_number: Option<u32>,
) -> Result<String, String> {
    let git_root = crate::config::workspace_config::find_git_root(repo_path)
        .ok_or_else(|| format!("Not a git repository: {}", repo_path.display()))?;
    let repo_name = git_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());
    let sanitized_branch = sanitize_branch_for_worktree_path(branch);
    let home = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string());
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

    // Reuse path: if the worktree directory already exists, was previously
    // registered with git for the SAME branch, and is still attached
    // (not stale / pruned), short-circuit by returning that path. Without
    // this, calling worktree_create with a previously-used branch errors
    // with `fatal: '<path>' already exists`, the brain falls back to
    // `workspace_create`, the workspace gets a generic "Workspace N"
    // title, and any `initial_prompt` gets dropped because the fallback
    // doesn't run the preset-launch / prompt-injection block in
    // `create_worktree_workspace_impl`. Detect-and-reuse keeps the
    // happy path intact for the Telegram "spin up an agent in this
    // branch" flow even when the worktree is already on disk.
    //
    // Safety: we only reuse when `git worktree list` confirms the
    // existing entry maps to OUR `path_str` AND our `branch`. A path
    // collision against a different branch still errors loudly so the
    // brain doesn't silently attach an agent to the wrong branch.
    if worktree_path.exists() {
        if existing_worktree_matches(repo_path, &path_str, branch) {
            return Ok(path_str);
        }
        // Path exists but git doesn't register it as OUR worktree. If it's
        // a safe-to-remove orphan — a leftover Codemux dir (e.g. from a
        // previously-worked issue branch) whose registration is already
        // gone — reclaim it so creation proceeds instead of dying with
        // `fatal: '<path>' already exists`. Anything resembling user data,
        // or a path git still tracks (a real collision against a different
        // branch), is left untouched so git fails loudly as before.
        if is_reclaimable_orphan_worktree(repo_path, &worktree_path) {
            std::fs::remove_dir_all(&worktree_path).map_err(|e| {
                format!("Failed to reclaim leftover worktree directory '{path_str}': {e}")
            })?;
            // A registration may still claim this branch from a now-missing
            // path; prune so `worktree add` starts clean. Permissive: a
            // repo with no stale entries is a no-op.
            let _ = run_git_permissive(repo_path, &["worktree", "prune"]);
        }
    }

    if new_branch {
        // Resolve base: prefer origin/<base> so new branches start from the
        // latest remote commit, not a potentially stale local ref. Freshen
        // `refs/remotes/origin/<base>` first — without the fetch it only
        // reflects the remote as of the LAST fetch, so "latest remote
        // commit" was previously aspirational. Best-effort: offline or
        // no-remote repos fall through to the stale/local refs below.
        // Callers run this whole function on a blocking pool
        // (`spawn_blocking`), so the fetch never stalls the UI thread.
        if let Some(b) = base {
            fetch_origin_branch(repo_path, b);
        }
        let resolved_base = base.map(|b| find_remote_ref(repo_path, b).unwrap_or_else(|| b.to_string()));
        let mut args = vec!["worktree", "add", "-b", branch, &path_str];
        if let Some(ref b) = resolved_base {
            args.push(b.as_str());
        }
        run_git(repo_path, &args)?;
    } else {
        // If the branch is currently checked out in the main repo, switch the
        // main repo to the default branch first so the worktree can claim it.
        let current = run_git_permissive(repo_path, &["branch", "--show-current"]);
        if current == branch {
            let default = find_default_branch(repo_path).ok_or_else(|| {
                format!("Branch '{branch}' is already checked out. Could not find a default branch to switch to.")
            })?;
            run_git(repo_path, &["checkout", &default]).map_err(|e| {
                format!("Cannot switch repo to '{default}' before creating worktree: {e}")
            })?;
        }

        // Open existing branch: if the branch has a remote counterpart, use -B
        // to reset the local branch to the remote tip, avoiding stale checkouts.
        // If not found locally, fetch it from origin first (common for PR branches
        // that were never pulled, or fork PRs that need pull/<n>/head).
        if find_remote_ref(repo_path, branch).is_none() {
            // Try direct branch fetch (same-repo PRs / remote branches)
            if run_git(repo_path, &["fetch", "origin", &format!("{branch}:{branch}")]).is_err() {
                // Fork PRs: branch lives under pull/<number>/head on origin
                if let Some(pr_num) = pr_number {
                    let refspec = format!("pull/{pr_num}/head:{branch}");
                    let _ = run_git(repo_path, &["fetch", "origin", &refspec]);
                }
            }
        }
        if let Some(remote_ref) = find_remote_ref(repo_path, branch) {
            run_git(
                repo_path,
                &["worktree", "add", "-B", branch, &path_str, &remote_ref],
            )?;
        } else {
            // Branch exists locally after fetch (or was already local)
            run_git(repo_path, &["worktree", "add", &path_str, branch])?;
        }
    }

    Ok(path_str)
}

/// Dirty/unpushed guard shared by `git_remove_worktree` (its
/// force=false path) and the close-workspace pre-flight in
/// `commands::workspace::close_workspace_with_worktree_impl`. The
/// pre-flight exists so a refused removal returns BEFORE any workspace
/// teardown — the workspace (and its delete dialog) stay alive for the
/// force escalation.
///
/// CONTRACT: the frontend detects a guard refusal by regex-matching
/// /use force/i on the error message, so the "Use force to override."
/// suffix in both errors is load-bearing. Do not reword it.
pub(crate) fn ensure_worktree_removable(worktree_path: &Path) -> Result<(), String> {
    // Check for uncommitted changes
    let status = run_git_permissive(worktree_path, &["status", "--porcelain"]);
    let dirty_count = status.lines().filter(|l| !l.is_empty()).count();
    if dirty_count > 0 {
        return Err(format!(
            "Worktree has {dirty_count} uncommitted change(s). Use force to override."
        ));
    }

    // Check for unpushed commits (only if upstream exists)
    let upstream = run_git_permissive(
        worktree_path,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    );
    if !upstream.is_empty() && !upstream.contains("fatal") {
        let unpushed = run_git_permissive(worktree_path, &["log", "@{upstream}..HEAD", "--oneline"]);
        let unpushed_count = unpushed.lines().filter(|l| !l.is_empty()).count();
        if unpushed_count > 0 {
            return Err(format!(
                "Worktree has {unpushed_count} unpushed commit(s). Use force to override."
            ));
        }
    }

    Ok(())
}

/// True when `branch` (a shortname like "feature/x") exists as a local
/// branch OR on ANY remote. `git show-ref --quiet -- <shortname>`
/// matches refs by path-component suffix — refs/heads/<b> and
/// refs/remotes/<any-remote>/<b> both hit — and reports via exit status
/// (0 = at least one match, 1 = none). This replaces scanning
/// `git_list_branches` output, which returned remote branches qualified
/// ("upstream/feature/x") and therefore never equality-matched a
/// shortname for non-origin remotes.
pub fn git_branch_exists(repo_path: &Path, branch: &str) -> bool {
    Command::new("git")
        .args(["show-ref", "--quiet", "--", branch])
        .current_dir(repo_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn git_remove_worktree(worktree_path: &Path, branch: Option<&str>, force: bool) -> Result<(), String> {
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

    if !force {
        ensure_worktree_removable(worktree_path)?;
    }

    if force {
        run_git(
            &repo_path,
            &["worktree", "remove", &worktree_path.to_string_lossy(), "--force"],
        )?;
    } else {
        run_git(
            &repo_path,
            &["worktree", "remove", &worktree_path.to_string_lossy()],
        )?;
    }

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

/// Add `entry` to `.git/info/exclude` so git ignores it without modifying `.gitignore`.
///
/// Works for both normal repositories (`.git` is a directory) and worktrees
/// (`.git` is a file containing `gitdir: ...`) by resolving to the main
/// repo's `.git/info/exclude`.
///
/// Uses a process-wide cache to avoid repeated filesystem checks after the
/// entry has been written once for a given (workspace, entry) pair.
///
/// Silently returns on any failure (not a git repo, parse error, I/O error).
pub fn ensure_git_exclude(workspace_dir: &Path, entry: &str) {
    use std::collections::HashSet;
    use std::sync::Mutex;

    static DONE: std::sync::LazyLock<Mutex<HashSet<(PathBuf, String)>>> =
        std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

    let key = (workspace_dir.to_path_buf(), entry.to_string());
    {
        let set = DONE.lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(&key) {
            return;
        }
    }

    let git_path = workspace_dir.join(".git");

    let git_dir = if git_path.is_dir() {
        git_path
    } else if git_path.is_file() {
        // Worktree: .git file contains "gitdir: /path/to/main/.git/worktrees/<name>"
        let content = match std::fs::read_to_string(&git_path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let gitdir = match content.strip_prefix("gitdir: ") {
            Some(s) => s.trim(),
            None => return,
        };
        // Navigate up from .git/worktrees/<name> to .git/
        match PathBuf::from(gitdir).parent().and_then(|p| p.parent()) {
            Some(dot_git) => dot_git.to_path_buf(),
            None => return,
        }
    } else {
        return;
    };

    let info_dir = git_dir.join("info");
    let _ = std::fs::create_dir_all(&info_dir);
    let exclude_path = info_dir.join("exclude");

    let already_present = if let Ok(content) = std::fs::read_to_string(&exclude_path) {
        if content.lines().any(|line| line.trim() == entry) {
            true
        } else {
            let prefix = if content.ends_with('\n') { "" } else { "\n" };
            let _ = std::fs::write(&exclude_path, format!("{content}{prefix}{entry}\n"));
            false
        }
    } else {
        let _ = std::fs::write(&exclude_path, format!("{entry}\n"));
        false
    };

    // Cache the result so concurrent/repeated calls skip filesystem I/O.
    let _ = already_present;
    let mut set = DONE.lock().unwrap_or_else(|e| e.into_inner());
    set.insert(key);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Branch name sanitization (cross-platform) ─────────────────────
    //
    // These tests are deliberately NOT cfg-gated. `sanitize_branch_for_worktree_path`
    // applies the Windows-strict forbidden-character rule on every
    // platform, so every CI matrix leg runs the same assertions. If the
    // rule ever diverges per-OS, this test will fail on one leg and
    // flag the drift.

    #[test]
    fn test_sanitize_branch_replaces_forward_slash() {
        // The classic case: `feat/my-feature` → `feat-my-feature`.
        assert_eq!(
            sanitize_branch_for_worktree_path("feat/my-feature"),
            "feat-my-feature"
        );
    }

    #[test]
    fn test_sanitize_branch_replaces_windows_forbidden_chars() {
        // Each Windows-forbidden filename character in isolation.
        // Reference: https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file
        assert_eq!(sanitize_branch_for_worktree_path("a<b"), "a-b");
        assert_eq!(sanitize_branch_for_worktree_path("a>b"), "a-b");
        assert_eq!(sanitize_branch_for_worktree_path("a:b"), "a-b");
        assert_eq!(sanitize_branch_for_worktree_path("a\"b"), "a-b");
        assert_eq!(sanitize_branch_for_worktree_path("a|b"), "a-b");
        assert_eq!(sanitize_branch_for_worktree_path("a?b"), "a-b");
        assert_eq!(sanitize_branch_for_worktree_path("a*b"), "a-b");
        assert_eq!(sanitize_branch_for_worktree_path("a\\b"), "a-b");
    }

    #[test]
    fn test_sanitize_branch_replaces_multiple_forbidden_chars() {
        // Realistic pathological case: every forbidden char at once.
        // Every occurrence becomes a dash; everything else is preserved.
        // Char-by-char trace:
        //   f e a t / s u b : n a m e < v 1 > " i d " | ? * \ e n d
        //   f e a t - s u b - n a m e - v 1 - - i d - - - - - e n d
        let input = r#"feat/sub:name<v1>"id"|?*\end"#;
        let output = sanitize_branch_for_worktree_path(input);
        assert_eq!(output, "feat-sub-name-v1--id-----end");
        // Sanity: no forbidden char survives.
        for ch in ['/', '\\', '<', '>', ':', '"', '|', '?', '*'] {
            assert!(
                !output.contains(ch),
                "forbidden char {ch:?} survived sanitization: {output:?}"
            );
        }
    }

    #[test]
    fn test_sanitize_branch_preserves_safe_chars() {
        // Everything outside the forbidden set passes through unchanged:
        // alphanumerics, dashes, underscores, dots, and non-ASCII Unicode.
        let cases = [
            "main",
            "release-2024.09.01",
            "feat_underscore_branch",
            "bugfix-issue-1234",
            "v1.0.0-rc.1",
            "branch.with.dots",
        ];
        for case in cases {
            assert_eq!(
                sanitize_branch_for_worktree_path(case),
                case,
                "safe input {case:?} should pass through unchanged"
            );
        }
    }

    #[test]
    fn test_sanitize_branch_empty_and_edge_cases() {
        assert_eq!(sanitize_branch_for_worktree_path(""), "");
        assert_eq!(sanitize_branch_for_worktree_path("/"), "-");
        assert_eq!(sanitize_branch_for_worktree_path("///"), "---");
        // Only forbidden chars → only dashes.
        assert_eq!(sanitize_branch_for_worktree_path("<>:\""), "----");
    }

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
    fn numstat_new_path_handles_rename_forms() {
        // Non-rename path is unchanged.
        assert_eq!(numstat_new_path("src/a.txt"), "src/a.txt");
        // Whole-path rename (no shared component).
        assert_eq!(numstat_new_path("old.txt => new.txt"), "new.txt");
        assert_eq!(
            numstat_new_path("src/a.txt => dest/b.txt"),
            "dest/b.txt"
        );
        // Shared prefix / suffix / both, in git's brace form.
        assert_eq!(
            numstat_new_path("src/lib/{old.txt => new.txt}"),
            "src/lib/new.txt"
        );
        assert_eq!(
            numstat_new_path("{src => dest}/file.txt"),
            "dest/file.txt"
        );
        assert_eq!(
            numstat_new_path("src/{a => b}/file.txt"),
            "src/b/file.txt"
        );
    }

    #[test]
    fn parse_single_numstat_attaches_stats_to_renamed_path() {
        // The key must be the full destination path so it matches
        // `git diff --name-status` (`src/lib/new.txt`) and stats attach.
        let map = parse_single_numstat("5\t3\tsrc/lib/{old.txt => new.txt}\n");
        assert_eq!(map.get("src/lib/new.txt"), Some(&(5, 3)));
        assert_eq!(map.get("new.txt"), None);
    }

    #[test]
    fn parse_numstat_per_file_attaches_stats_to_renamed_path() {
        let map = parse_numstat_per_file("4\t1\t{src => dest}/file.txt\n", "");
        assert_eq!(map.get("dest/file.txt"), Some(&(4, 1)));
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
        let wt_path = git_create_worktree(&repo, "feature-test", true, None, None).expect("create worktree");
        assert!(PathBuf::from(&wt_path).exists(), "worktree dir should exist");

        let branches = git_list_branches(&repo, false).expect("list branches");
        assert!(branches.contains(&"feature-test".to_string()), "branch should exist");

        git_remove_worktree(Path::new(&wt_path), Some("feature-test"), true).expect("remove worktree");
        assert!(!PathBuf::from(&wt_path).exists(), "worktree dir should be gone");

        let branches_after = git_list_branches(&repo, false).expect("list branches after");
        assert!(!branches_after.contains(&"feature-test".to_string()), "branch should be deleted");
    }

    #[test]
    fn test_recreate_worktree_for_adopted_repo_prunes_stale_host_entry() {
        // Simulate a repo rsynced from a codemux-remote host during
        // worktree adoption: it carries a worktree admin entry for the
        // branch, but that entry points at the HOST's path, which does
        // not exist on this machine. A naive `git worktree add` for the
        // branch then fails ("already used by worktree"). The recreate
        // helper must prune the stale entry and add a clean local one.
        let (_dir, repo) = setup_test_repo();
        // A branch that "came over with the rsync".
        run_git(&repo, &["branch", "adopted-feature"]).expect("create branch");
        // A worktree at a path we then delete → stale, prunable, and it
        // holds the branch "checked out" until pruned.
        let ghost = _dir.path().join("ghost-host-worktree");
        run_git(
            &repo,
            &["worktree", "add", &ghost.to_string_lossy(), "adopted-feature"],
        )
        .expect("add ghost worktree");
        std::fs::remove_dir_all(&ghost).expect("delete ghost worktree dir");

        // Recreate: prunes the ghost, adds a clean local worktree.
        let wt_path = git_recreate_worktree_for_adopted_repo(&repo, "adopted-feature")
            .expect("recreate worktree for adopted repo");
        let wt = PathBuf::from(&wt_path);
        assert!(wt.exists(), "recreated worktree dir should exist");
        assert!(wt.join(".git").is_file(), "should be a linked worktree (.git file)");

        // It's attached to the right branch.
        let current = run_git_permissive(&wt, &["branch", "--show-current"]);
        assert_eq!(current.trim(), "adopted-feature");

        // Cleanup so we don't leave a worktree under the real ~/.codemux.
        let _ = git_remove_worktree(&wt, Some("adopted-feature"), true);
    }

    #[test]
    fn test_create_worktree_existing_branch() {
        let (_dir, repo) = setup_test_repo();
        run_git(&repo, &["branch", "existing-branch"]).expect("create branch");

        let wt_path = git_create_worktree(&repo, "existing-branch", false, None, None).expect("create worktree");
        assert!(PathBuf::from(&wt_path).exists());

        let info = git_branch_info(Path::new(&wt_path)).expect("branch info");
        assert_eq!(info.branch.as_deref(), Some("existing-branch"));

        git_remove_worktree(Path::new(&wt_path), Some("existing-branch"), true).expect("cleanup");
    }

    // ── Worktree reuse (worktree_create resilience) ─────────────────
    //
    // Regression coverage for the bug where calling `worktree_create`
    // with `new_branch=false` against a branch whose Codemux worktree
    // directory already existed would error with `fatal: '<path>'
    // already exists`. The brain's natural fallback (calling
    // `workspace_create` with the same path) produced a workspace with
    // generic "Workspace N" title and silently dropped `initial_prompt`
    // because the fallback skips the preset-launch + prompt-injection
    // block in `create_worktree_workspace_impl`.
    //
    // Fix: `git_create_worktree` now detects when the requested worktree
    // path is already registered against the requested branch and
    // returns Ok early. The downstream impl then runs the same title-
    // setting + prompt-injection code path as a fresh creation.

    /// A unique random branch name per test run so parallel cargo test
    /// processes don't collide on `~/.codemux/worktrees/<repo>/<branch>`
    /// (the path `git_create_worktree` derives from the branch).
    fn unique_branch(label: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("worktree-reuse-{label}-{}-{nanos}", std::process::id())
    }

    #[test]
    fn worktree_create_reuses_existing_path_for_same_branch() {
        let (_dir, repo) = setup_test_repo();
        let branch = unique_branch("same");
        run_git(&repo, &["branch", &branch]).expect("create branch");

        // First call materializes the worktree on disk.
        let first = git_create_worktree(&repo, &branch, false, None, None)
            .expect("first worktree create");
        assert!(PathBuf::from(&first).exists(), "worktree dir should exist after first call");

        // Second call MUST NOT error with `already exists`. It must
        // return the same path so the downstream impl can run its
        // preset-launch and prompt-injection block on the workspace
        // it creates for this reused worktree.
        let second = git_create_worktree(&repo, &branch, false, None, None)
            .expect("second call must reuse, not error");
        assert_eq!(
            first, second,
            "reused worktree should return the same path"
        );

        git_remove_worktree(Path::new(&first), Some(&branch), true).expect("cleanup");
    }

    #[test]
    fn worktree_create_does_not_reuse_path_for_different_branch() {
        // Safety net: if a path collision somehow happened against the
        // WRONG branch (e.g. stale dir from a half-deleted worktree),
        // git_create_worktree must still error rather than silently
        // attaching an agent to the wrong branch.
        let (_dir, repo) = setup_test_repo();
        let branch_a = unique_branch("wrong-a");
        let branch_b = unique_branch("wrong-b");
        run_git(&repo, &["branch", &branch_a]).expect("create branch a");
        run_git(&repo, &["branch", &branch_b]).expect("create branch b");

        let wt_a = git_create_worktree(&repo, &branch_a, false, None, None)
            .expect("worktree a");

        // Path for branch_b would be different (different name), so this
        // doesn't directly test path collision against wrong-branch.
        // Instead, simulate the collision: create an empty dir at
        // branch_a's path AFTER removing the worktree, then ask for
        // branch_b at that exact path. Use the helper directly to
        // verify the porcelain check refuses to match.
        assert!(
            !existing_worktree_matches(&repo, &wt_a, &branch_b),
            "porcelain check must refuse to match a path that's registered against a different branch"
        );
        assert!(
            existing_worktree_matches(&repo, &wt_a, &branch_a),
            "porcelain check must match a path that's registered against the requested branch"
        );

        git_remove_worktree(Path::new(&wt_a), Some(&branch_a), true).expect("cleanup");
    }

    #[test]
    fn existing_worktree_matches_returns_false_when_no_worktrees() {
        let (_dir, repo) = setup_test_repo();
        assert!(
            !existing_worktree_matches(&repo, "/tmp/nonexistent-path", "main"),
            "no registered worktrees => no match"
        );
    }

    #[test]
    fn worktree_create_reclaims_orphan_leftover_dir() {
        // Regression: a previous worktree for this branch was pruned from
        // git's registry but its directory was left on disk (a stray
        // `.codemux/` and nothing else). Re-creating the worktree must
        // reclaim that orphan instead of dying with
        // `fatal: '<path>' already exists` — the exact failure seen when
        // spinning up a workspace from an already-worked (often closed)
        // issue branch.
        let (_dir, repo) = setup_test_repo();
        let branch = unique_branch("orphan");
        run_git(&repo, &["branch", &branch]).expect("create branch");

        // Materialize the worktree, then drop ONLY its registration + dir
        // (the branch survives) — this is the exact path git will derive.
        let wt_path =
            git_create_worktree(&repo, &branch, false, None, None).expect("first create");
        run_git(&repo, &["worktree", "remove", "--force", &wt_path])
            .expect("remove worktree, keep branch");
        assert!(!PathBuf::from(&wt_path).exists(), "dir gone after remove");

        // Recreate the orphan leftover: dir back on disk holding only
        // Codemux metadata, with no git registration behind it.
        let orphan = PathBuf::from(&wt_path);
        std::fs::create_dir_all(orphan.join(".codemux")).expect("mk .codemux");
        std::fs::write(orphan.join(".codemux").join("index.json"), "{}").expect("write metadata");

        // Create must now reclaim the orphan and produce a real worktree.
        let again = git_create_worktree(&repo, &branch, false, None, None)
            .expect("must reclaim orphan, not error with 'already exists'");
        assert_eq!(again, wt_path, "reclaimed path matches the original");
        assert!(
            PathBuf::from(&again).join(".git").is_file(),
            "reclaimed dir is now a real linked worktree"
        );

        git_remove_worktree(Path::new(&again), Some(&branch), true).expect("cleanup");
    }

    #[test]
    fn is_reclaimable_orphan_worktree_distinguishes_safe_from_unsafe() {
        let (_dir, repo) = setup_test_repo();
        let tmp = TempDir::new().expect("tmp");

        // Empty dir → reclaimable.
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(is_reclaimable_orphan_worktree(&repo, &empty), "empty dir is reclaimable");

        // Only Codemux metadata → reclaimable.
        let meta_only = tmp.path().join("meta");
        std::fs::create_dir_all(meta_only.join(".codemux")).unwrap();
        std::fs::write(meta_only.join(".codemux").join("index.json"), "{}").unwrap();
        assert!(
            is_reclaimable_orphan_worktree(&repo, &meta_only),
            "Codemux-metadata-only dir is reclaimable"
        );

        // Contains user data → NOT reclaimable.
        let with_user = tmp.path().join("userdata");
        std::fs::create_dir_all(&with_user).unwrap();
        std::fs::write(with_user.join("main.rs"), "fn main() {}").unwrap();
        assert!(
            !is_reclaimable_orphan_worktree(&repo, &with_user),
            "a dir with user files must never be reclaimed"
        );

        // Contains a `.git` → NOT reclaimable (looks like a real checkout).
        let with_git = tmp.path().join("hasgit");
        std::fs::create_dir_all(&with_git).unwrap();
        std::fs::write(with_git.join(".git"), "gitdir: /somewhere").unwrap();
        assert!(
            !is_reclaimable_orphan_worktree(&repo, &with_git),
            "a dir with a .git entry must never be reclaimed"
        );

        // A live registered worktree → NOT reclaimable even though its
        // only non-tracked content might look disposable.
        let branch = unique_branch("registered");
        run_git(&repo, &["branch", &branch]).unwrap();
        let wt = git_create_worktree(&repo, &branch, false, None, None).unwrap();
        assert!(
            !is_reclaimable_orphan_worktree(&repo, Path::new(&wt)),
            "a registered worktree must never be reclaimed"
        );
        git_remove_worktree(Path::new(&wt), Some(&branch), true).unwrap();
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
        let wt_path = git_create_worktree(&repo, "feature-from-dev", true, Some("develop"), None).expect("create worktree");
        // The worktree should have dev.txt (inherited from develop)
        assert!(PathBuf::from(&wt_path).join("dev.txt").exists(), "should have develop's file");

        git_remove_worktree(Path::new(&wt_path), Some("feature-from-dev"), true).expect("cleanup");
    }

    /// Build the stale-clone scenario behind issue #76: a bare "origin",
    /// a `local-<tag>` clone (the repo Codemux operates on), and a
    /// publisher clone that pushes one extra commit which `local` never
    /// fetches. Returns `(tempdir, local_clone_path, true_remote_tip)`,
    /// with `local`'s `refs/remotes/origin/main` guaranteed stale.
    fn setup_stale_clone_with_remote(tag: &str) -> (TempDir, PathBuf, String) {
        let dir = TempDir::new().expect("create temp dir");
        let root = dir.path().to_path_buf();

        // Seed repo with one commit on main.
        let seed = root.join("seed");
        std::fs::create_dir_all(&seed).expect("mkdir seed");
        run_git(&seed, &["init", "--initial-branch=main"]).expect("init seed");
        run_git(&seed, &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "--allow-empty", "-m", "initial"]).expect("seed commit");

        // Bare "origin" plus two clones: `local` (under test) and
        // `publisher` (a teammate pushing while `local` doesn't fetch).
        let bare = root.join("origin.git");
        run_git(&root, &["clone", "--bare", seed.to_str().unwrap(), bare.to_str().unwrap()]).expect("clone bare");
        let local = root.join(format!("local-{tag}"));
        run_git(&root, &["clone", bare.to_str().unwrap(), local.to_str().unwrap()]).expect("clone local");
        let publisher = root.join("publisher");
        run_git(&root, &["clone", bare.to_str().unwrap(), publisher.to_str().unwrap()]).expect("clone publisher");
        run_git(&publisher, &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "--allow-empty", "-m", "remote-only"]).expect("publisher commit");
        run_git(&publisher, &["push", "origin", "main"]).expect("publisher push");

        let remote_tip = run_git(&bare, &["rev-parse", "main"]).expect("bare tip");
        let stale = run_git(&local, &["rev-parse", "refs/remotes/origin/main"]).expect("local origin/main");
        assert_ne!(stale, remote_tip, "precondition: local origin/main must be stale");
        (dir, local, remote_tip)
    }

    // Issue #76: a new workspace branch must start from the LATEST remote
    // commit of its base, not the stale `origin/<base>` snapshot left by
    // the last fetch. `git_create_worktree` now runs a scoped
    // `git fetch origin <base>` before resolving the base ref.
    #[test]
    fn test_new_branch_worktree_starts_from_freshly_fetched_origin_base() {
        let (_dir, local, remote_tip) = setup_stale_clone_with_remote("fresh");

        let wt_path = git_create_worktree(&local, "fresh-base-test", true, Some("main"), None)
            .expect("create worktree");
        let head = run_git(Path::new(&wt_path), &["rev-parse", "HEAD"]).expect("worktree HEAD");
        assert_eq!(
            head, remote_tip,
            "new branch should start at the freshly-fetched remote tip, not the stale origin/main"
        );

        git_remove_worktree(Path::new(&wt_path), Some("fresh-base-test"), true).expect("cleanup");
    }

    // Issue #76 fallback: when the remote is unreachable (offline), the
    // best-effort fetch must NOT hard-fail worktree creation — the new
    // branch falls back to the existing (stale) `origin/<base>` ref.
    #[test]
    fn test_new_branch_worktree_offline_falls_back_to_stale_origin_ref() {
        let (_dir, local, remote_tip) = setup_stale_clone_with_remote("offline");
        let stale = run_git(&local, &["rev-parse", "refs/remotes/origin/main"]).expect("stale ref");

        // Sever the remote: origin now points at a path that doesn't exist.
        run_git(&local, &["remote", "set-url", "origin", "/nonexistent/codemux-test-origin.git"])
            .expect("set-url");

        let wt_path = git_create_worktree(&local, "offline-base-test", true, Some("main"), None)
            .expect("worktree creation must not hard-fail when the remote is unreachable");
        let head = run_git(Path::new(&wt_path), &["rev-parse", "HEAD"]).expect("worktree HEAD");
        assert_eq!(head, stale, "offline fallback should use the existing (stale) origin/main");
        assert_ne!(head, remote_tip, "remote tip is unreachable offline");

        git_remove_worktree(Path::new(&wt_path), Some("offline-base-test"), true).expect("cleanup");
    }

    #[test]
    fn fetch_origin_branch_updates_remote_tracking_ref() {
        let (_dir, local, remote_tip) = setup_stale_clone_with_remote("fetch");
        // `origin/<b>` input is normalised to `<b>` before fetching.
        assert!(fetch_origin_branch(&local, "origin/main"), "fetch should succeed");
        let updated = run_git(&local, &["rev-parse", "refs/remotes/origin/main"]).expect("ref");
        assert_eq!(updated, remote_tip, "origin/main should now be at the true remote tip");
    }

    #[test]
    fn fetch_origin_branch_is_noop_without_origin_remote() {
        let (_dir, repo) = setup_test_repo();
        assert!(!fetch_origin_branch(&repo, "main"), "no origin remote → no fetch");
    }

    #[test]
    fn fetch_origin_branch_skips_absolute_and_empty_refs() {
        let (_dir, repo) = setup_test_repo();
        assert!(!fetch_origin_branch(&repo, "refs/heads/main"));
        assert!(!fetch_origin_branch(&repo, ""));
        assert!(!fetch_origin_branch(&repo, "origin/"));
    }

    #[test]
    fn test_list_worktrees() {
        let (_dir, repo) = setup_test_repo();
        let wt1 = git_create_worktree(&repo, "wt-one", true, None, None).expect("create wt1");
        let wt2 = git_create_worktree(&repo, "wt-two", true, None, None).expect("create wt2");

        let worktrees = git_list_worktrees(&repo).expect("list worktrees");
        assert!(worktrees.len() >= 3, "should have main + 2 worktrees, got {}", worktrees.len());

        let branches: Vec<Option<&str>> = worktrees.iter().map(|w| w.branch.as_deref()).collect();
        assert!(branches.iter().any(|b| *b == Some("wt-one")), "should include wt-one");
        assert!(branches.iter().any(|b| *b == Some("wt-two")), "should include wt-two");

        git_remove_worktree(Path::new(&wt1), Some("wt-one"), true).expect("cleanup wt1");
        git_remove_worktree(Path::new(&wt2), Some("wt-two"), true).expect("cleanup wt2");
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
    fn test_list_branches_detailed_basic() {
        let (_dir, repo) = setup_test_repo();
        run_git(&repo, &["branch", "feature-a"]).expect("create feature-a");
        run_git(&repo, &["branch", "feature-b"]).expect("create feature-b");

        let branches = git_list_branches_detailed(&repo).expect("list detailed");
        assert!(branches.len() >= 3, "should have main/master + 2 features, got {}", branches.len());

        let feature_a = branches.iter().find(|b| b.name == "feature-a").expect("feature-a present");
        assert!(feature_a.is_local, "feature-a should be local");
        assert!(!feature_a.is_remote, "feature-a should not be remote");
        assert!(feature_a.last_commit_unix > 0, "timestamp should be positive");
    }

    #[test]
    fn test_list_branches_detailed_single_branch() {
        let (_dir, repo) = setup_test_repo();

        let branches = git_list_branches_detailed(&repo).expect("list detailed");
        assert_eq!(branches.len(), 1, "should have only the default branch");

        let default = &branches[0];
        assert!(default.is_local);
        assert!(!default.is_remote);
        assert!(default.last_commit_unix > 0);
    }

    #[test]
    fn test_list_branches_detailed_dedup_local_and_remote() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();

        // Create a local branch, push it so it exists both locally and as remote tracking
        run_git(&local, &["checkout", "-b", "shared-branch"]).expect("create branch");
        std::fs::write(local.join("shared.txt"), "content").expect("write");
        run_git(&local, &["add", "shared.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "shared commit"],
        ).expect("commit");
        run_git(&local, &["push", "-u", "origin", "shared-branch"]).expect("push");

        let branches = git_list_branches_detailed(&local).expect("list detailed");
        let shared: Vec<&BranchDetail> = branches.iter().filter(|b| b.name == "shared-branch").collect();
        assert_eq!(shared.len(), 1, "should be deduplicated to one entry, got {}", shared.len());
        assert!(shared[0].is_local, "should be marked local");
        assert!(shared[0].is_remote, "should be marked remote");
    }

    #[test]
    fn test_list_branches_detailed_sorted_by_recency() {
        let (_dir, repo) = setup_test_repo();

        // Create branches with commits at different times
        run_git(&repo, &["checkout", "-b", "old-branch"]).expect("create old");
        std::fs::write(repo.join("old.txt"), "old").expect("write");
        run_git(&repo, &["add", "old.txt"]).expect("add");
        run_git(
            &repo,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "old"],
        ).expect("commit old");

        // Sleep briefly so timestamps differ
        std::thread::sleep(std::time::Duration::from_secs(1));

        run_git(&repo, &["checkout", "-b", "new-branch"]).expect("create new");
        std::fs::write(repo.join("new.txt"), "new").expect("write");
        run_git(&repo, &["add", "new.txt"]).expect("add");
        run_git(
            &repo,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "new"],
        ).expect("commit new");

        let branches = git_list_branches_detailed(&repo).expect("list detailed");
        let new_idx = branches.iter().position(|b| b.name == "new-branch").expect("new-branch present");
        let old_idx = branches.iter().position(|b| b.name == "old-branch").expect("old-branch present");
        assert!(new_idx < old_idx, "new-branch should come before old-branch (sorted by recency)");
    }

    #[test]
    fn test_list_branches_detailed_remote_only() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();

        // Create a branch on remote only (push from local, then delete local branch)
        run_git(&local, &["checkout", "-b", "remote-only"]).expect("create branch");
        std::fs::write(local.join("remote.txt"), "content").expect("write");
        run_git(&local, &["add", "remote.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "remote commit"],
        ).expect("commit");
        run_git(&local, &["push", "-u", "origin", "remote-only"]).expect("push");

        // Switch back and delete local branch
        let _ = run_git(&local, &["checkout", "master"])
            .or_else(|_| run_git(&local, &["checkout", "main"]));
        run_git(&local, &["branch", "-D", "remote-only"]).expect("delete local");

        let branches = git_list_branches_detailed(&local).expect("list detailed");
        let remote_only = branches.iter().find(|b| b.name == "remote-only").expect("remote-only present");
        assert!(!remote_only.is_local, "should not be local");
        assert!(remote_only.is_remote, "should be remote");
    }

    /// Regression: `git for-each-ref --format=%(refname:short)` and
    /// `git branch -r --format=%(refname:short)` both collapse
    /// `refs/remotes/origin/HEAD` to literally `"origin"` (the unambiguous
    /// short form of the symref's target's namespace). The old
    /// `!name.contains("HEAD")` filter let that through, so the New
    /// Workspace branch picker showed a phantom "origin" branch — and
    /// selecting it created a worktree directory named `origin/`. Make sure
    /// the new full-refname filter on `/HEAD` suffix excludes it from both
    /// branch listing functions.
    #[test]
    fn test_list_branches_skips_remote_head_symref() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();

        // Establish origin/HEAD as a symref (mirrors what `git clone` would
        // do for a real remote — our test scaffolding doesn't set it
        // automatically because it inits a bare repo first).
        run_git(&local, &["remote", "set-head", "origin", "--auto"])
            .expect("set origin/HEAD");

        // Sanity: the symref should now exist.
        let head = run_git(&local, &["symbolic-ref", "refs/remotes/origin/HEAD"])
            .expect("origin/HEAD exists");
        assert!(
            head.trim().starts_with("refs/remotes/origin/"),
            "origin/HEAD should be a symref, got {head:?}"
        );

        // git_list_branches (remote=true) must not include "origin".
        let remote_branches =
            git_list_branches(&local, true).expect("list remote branches");
        assert!(
            !remote_branches.iter().any(|b| b == "origin"),
            "remote branch list should not include phantom 'origin' entry, got {remote_branches:?}"
        );

        // git_list_branches_detailed must not include a branch named "origin".
        let detailed =
            git_list_branches_detailed(&local).expect("list detailed");
        assert!(
            !detailed.iter().any(|b| b.name == "origin"),
            "detailed branch list should not include phantom 'origin' entry, got {:?}",
            detailed.iter().map(|b| &b.name).collect::<Vec<_>>()
        );
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
        let wt_path = git_create_worktree(&repo, "temp-branch", true, None, None).expect("create worktree");
        git_remove_worktree(Path::new(&wt_path), Some("main"), true).expect("remove with main as branch arg");

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

    /// Regression: branches that exist on the remote but lack
    /// `branch.<name>.{remote,merge}` config (e.g. pushed via
    /// `push.autoSetupRemote=true` or `git push origin <branch>` without
    /// `-u`) should still report `has_upstream=true`. Before the
    /// `resolve_upstream_ref` fallback was added, the UI mistakenly offered
    /// "Publish Branch" for these branches even though the user could push
    /// fine from the terminal.
    #[test]
    fn test_git_branch_info_remote_ref_without_tracking_config() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();

        // Sanity: cloned branch starts with proper upstream tracking.
        let info = git_branch_info(&local).expect("branch info before unset");
        assert!(info.has_upstream, "cloned branch should have upstream initially");

        let branch = run_git_permissive(&local, &["branch", "--show-current"]);
        assert!(!branch.is_empty(), "test setup: should have a current branch");

        // Remove tracking config. Leaves refs/remotes/origin/<branch> intact —
        // this is exactly the state a no-`-u` push leaves behind.
        let _ = run_git(
            &local,
            &["config", "--unset", &format!("branch.{branch}.remote")],
        );
        let _ = run_git(
            &local,
            &["config", "--unset", &format!("branch.{branch}.merge")],
        );

        // `@{upstream}` should now fail, confirming the test fixture.
        let raw = run_git_permissive(
            &local,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        );
        assert!(
            raw.is_empty(),
            "test setup: @{{upstream}} should be unset after stripping tracking config",
        );

        // The fallback should still surface the remote-tracking ref.
        let info = git_branch_info(&local).expect("branch info after unset");
        assert!(
            info.has_upstream,
            "branch should be considered published via remote-tracking ref fallback"
        );
        assert_eq!(info.ahead, 0, "no new commits → 0 ahead");
        assert_eq!(info.behind, 0, "remote unchanged → 0 behind");

        // Adding a local commit should bump ahead, proving the fallback
        // actually computes ahead/behind against the remote ref, not just
        // flips the boolean.
        std::fs::write(local.join("after-unset.txt"), "after").expect("write");
        run_git(&local, &["add", "after-unset.txt"]).expect("add");
        run_git(
            &local,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@test.com",
                "commit",
                "-m",
                "after unset",
            ],
        )
        .expect("commit");

        let info = git_branch_info(&local).expect("branch info after commit");
        assert!(info.has_upstream, "still published via remote-tracking ref");
        assert_eq!(info.ahead, 1, "should count the new local commit as ahead");
        assert_eq!(info.behind, 0);
    }

    /// Regression: `git_log` powers the sidebar's pushed/unpushed badges on
    /// each commit. It used to share the same `@{upstream}`-only check as
    /// `git_branch_info`, so removing tracking config flipped every
    /// already-pushed commit to "unpushed" in the UI.
    #[test]
    fn test_git_log_is_pushed_without_tracking_config() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();

        // Initial commit was pushed during setup → should show as pushed.
        let log_before = git_log(&local, 10).expect("log before unset");
        let initial_before = log_before
            .iter()
            .find(|e| e.message == "initial")
            .expect("initial commit in log");
        assert!(
            initial_before.is_pushed,
            "initial commit should be pushed before we touch config"
        );

        // Remove formal upstream tracking.
        let branch = run_git_permissive(&local, &["branch", "--show-current"]);
        let _ = run_git(
            &local,
            &["config", "--unset", &format!("branch.{branch}.remote")],
        );
        let _ = run_git(
            &local,
            &["config", "--unset", &format!("branch.{branch}.merge")],
        );

        // is_pushed should still resolve correctly via the remote-tracking
        // ref fallback.
        let log_after = git_log(&local, 10).expect("log after unset");
        let initial_after = log_after
            .iter()
            .find(|e| e.message == "initial")
            .expect("initial commit still in log");
        assert!(
            initial_after.is_pushed,
            "commit present on remote should still report is_pushed=true after upstream tracking removed"
        );

        // Make a local commit — it should report as unpushed, while the
        // initial commit stays pushed.
        std::fs::write(local.join("local-only.txt"), "x").expect("write");
        run_git(&local, &["add", "local-only.txt"]).expect("add");
        run_git(
            &local,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@test.com",
                "commit",
                "-m",
                "local only",
            ],
        )
        .expect("commit");

        let log = git_log(&local, 10).expect("log after local commit");
        let local_only = log
            .iter()
            .find(|e| e.message == "local only")
            .expect("local-only commit in log");
        assert!(
            !local_only.is_pushed,
            "new local commit should be reported as unpushed"
        );
        let initial = log
            .iter()
            .find(|e| e.message == "initial")
            .expect("initial commit still in log");
        assert!(
            initial.is_pushed,
            "initial commit must remain pushed alongside the new unpushed commit"
        );
    }

    #[test]
    fn test_git_operations_in_worktree() {
        let (_dir, repo) = setup_test_repo();
        let wt_path_str = git_create_worktree(&repo, "wt-ops-test", true, None, None).expect("create worktree");
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
        git_remove_worktree(Path::new(&wt_path_str), Some("wt-ops-test"), true).expect("cleanup worktree");
    }

    #[test]
    fn test_worktree_name_uses_main_repo_not_worktree_dir() {
        // When creating a new worktree from INSIDE an existing worktree,
        // the worktree directory name should use the main repo's name,
        // not the worktree's directory name.
        let (_dir, repo) = setup_test_repo();

        // Create first worktree
        let wt1_path = git_create_worktree(&repo, "wt-first", true, None, None)
            .expect("create first worktree");
        let wt1 = PathBuf::from(&wt1_path);
        assert!(wt1.exists(), "first worktree should exist");

        // Now create a second worktree from INSIDE the first worktree
        // This simulates: user's active workspace is a worktree, they create another
        let wt2_path = git_create_worktree(&wt1, "wt-second", true, None, None)
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
        git_remove_worktree(Path::new(&wt2_path), Some("wt-second"), true).expect("cleanup wt2");
        git_remove_worktree(Path::new(&wt1_path), Some("wt-first"), true).expect("cleanup wt1");
    }

    #[test]
    fn test_worktree_branch_with_slashes_sanitized() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "feature/deep/nested", true, None, None)
            .expect("create worktree for branch with slashes");

        // Slashes in branch name should be replaced with hyphens in directory name
        let dir_name = PathBuf::from(&wt_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(dir_name, "feature-deep-nested", "slashes should be replaced with hyphens");
        assert!(PathBuf::from(&wt_path).exists(), "worktree should exist");

        git_remove_worktree(Path::new(&wt_path), Some("feature/deep/nested"), true).expect("cleanup");
    }

    #[test]
    fn test_worktree_empty_repo_does_not_crash() {
        let dir = TempDir::new().expect("create temp dir");
        let path = dir.path().to_path_buf();
        run_git(&path, &["init"]).expect("git init");
        // No commits — behavior may vary by git version but should not crash

        let result = git_create_worktree(&path, "new-branch", true, None, None);
        // Either succeeds (modern git) or returns Err (older git) — just don't crash
        if let Ok(wt_path) = &result {
            let _ = std::fs::remove_dir_all(wt_path);
        }
    }

    #[test]
    fn test_delete_workspace_removes_branch() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "test-delete-me", true, None, None)
            .expect("create worktree");

        let branches_before = git_list_branches(&repo, false).expect("list before");
        assert!(branches_before.contains(&"test-delete-me".to_string()), "branch should exist before delete");

        // Remove worktree WITH branch deletion (simulates delete with checkbox checked)
        git_remove_worktree(Path::new(&wt_path), Some("test-delete-me"), true).expect("remove with branch");

        let branches_after = git_list_branches(&repo, false).expect("list after");
        assert!(!branches_after.contains(&"test-delete-me".to_string()), "branch should be gone after delete");
    }

    #[test]
    fn test_close_workspace_keeps_branch() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "test-keep-me", true, None, None)
            .expect("create worktree");

        let branches_before = git_list_branches(&repo, false).expect("list before");
        assert!(branches_before.contains(&"test-keep-me".to_string()), "branch should exist before close");

        // Remove worktree WITHOUT branch deletion (simulates close without checkbox)
        git_remove_worktree(Path::new(&wt_path), None, true).expect("remove without branch");

        let branches_after = git_list_branches(&repo, false).expect("list after");
        assert!(branches_after.contains(&"test-keep-me".to_string()), "branch should still exist after close");

        // Clean up the branch manually
        let _ = run_git(&repo, &["branch", "-D", "test-keep-me"]);
    }

    // ── worktree deletion safety tests ──

    #[test]
    fn test_remove_worktree_clean_no_force() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "clean-wt", true, None, None).expect("create worktree");

        // Clean worktree should be removable without force
        git_remove_worktree(Path::new(&wt_path), Some("clean-wt"), false).expect("should succeed on clean worktree");
        assert!(!PathBuf::from(&wt_path).exists(), "worktree dir should be gone");
    }

    #[test]
    fn test_remove_worktree_blocks_dirty() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "dirty-wt", true, None, None).expect("create worktree");

        // Create uncommitted changes in the worktree
        std::fs::write(PathBuf::from(&wt_path).join("dirty.txt"), "unsaved work").expect("write");

        let result = git_remove_worktree(Path::new(&wt_path), None, false);
        assert!(result.is_err(), "should block removal of dirty worktree");
        let err = result.unwrap_err();
        assert!(err.contains("uncommitted change"), "error should mention uncommitted changes: {err}");
        assert!(PathBuf::from(&wt_path).exists(), "worktree should still exist");

        // Cleanup with force
        git_remove_worktree(Path::new(&wt_path), Some("dirty-wt"), true).expect("force cleanup");
    }

    #[test]
    fn test_remove_worktree_force_overrides_dirty() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "force-dirty-wt", true, None, None).expect("create worktree");

        // Create uncommitted changes
        std::fs::write(PathBuf::from(&wt_path).join("dirty.txt"), "unsaved work").expect("write");

        // Force should override dirty check
        git_remove_worktree(Path::new(&wt_path), Some("force-dirty-wt"), true).expect("force should succeed");
        assert!(!PathBuf::from(&wt_path).exists(), "worktree dir should be gone");
    }

    #[test]
    fn test_remove_worktree_no_upstream_allowed() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "no-upstream-wt", true, None, None).expect("create worktree");

        // Commit something in the worktree (no upstream configured)
        run_git(Path::new(&wt_path), &["-c", "user.name=Test", "-c", "user.email=test@test.com",
            "commit", "--allow-empty", "-m", "local commit"]).expect("commit");

        // Should succeed since there's no upstream — lenient policy
        git_remove_worktree(Path::new(&wt_path), Some("no-upstream-wt"), false).expect("should succeed without upstream");
        assert!(!PathBuf::from(&wt_path).exists(), "worktree dir should be gone");
    }

    #[test]
    fn test_remove_worktree_no_upstream_clean_allows_delete() {
        let (_dir, repo) = setup_test_repo();
        let wt_path = git_create_worktree(&repo, "local-only-wt", true, None, None).expect("create worktree");

        // Clean working tree, no upstream — force=false should succeed (lenient policy)
        git_remove_worktree(Path::new(&wt_path), Some("local-only-wt"), false).expect("clean local-only worktree should be deletable without force");
        assert!(!PathBuf::from(&wt_path).exists(), "worktree dir should be gone");
    }

    // ── merge_branch comprehensive integration tests ──
    //
    // Every test creates real git repos, makes real commits, and verifies
    // real file contents and git state. No mocks.

    fn git_config(repo: &Path) {
        let _ = run_git(repo, &["config", "user.name", "Test"]);
        let _ = run_git(repo, &["config", "user.email", "test@test.com"]);
    }

    /// Resolve main branch name (git init defaults to "master" on some systems, "main" on others).
    fn main_branch(repo: &Path) -> &'static str {
        if run_git(repo, &["rev-parse", "--verify", "main"]).is_ok() { "main" } else { "master" }
    }

    /// Switch to main, optionally creating divergence.
    fn checkout_main(repo: &Path) -> &'static str {
        let name = main_branch(repo);
        run_git(repo, &["checkout", name]).expect("checkout main");
        name
    }

    fn head_hash(repo: &Path) -> String {
        run_git(repo, &["rev-parse", "HEAD"]).expect("rev-parse HEAD").trim().to_string()
    }

    fn log_oneline(repo: &Path, count: usize) -> String {
        run_git(repo, &["log", "--oneline", &format!("-{}", count)]).expect("git log")
    }

    // ── 1. Full clean merge workflow ──

    #[test]
    fn test_full_merge_workflow_clean() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Feature branch adds feature.txt
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("feature.txt"), "feature work").unwrap();
        run_git(&repo, &["add", "feature.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add feature.txt"]).unwrap();

        // Main adds main.txt (different file — no conflict)
        let main = checkout_main(&repo);
        std::fs::write(repo.join("main.txt"), "main work").unwrap();
        run_git(&repo, &["add", "main.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add main.txt"]).unwrap();

        // Merge main into feature
        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_branch(&repo, main).expect("merge should succeed");
        assert_eq!(result, "merged", "clean merge should report 'merged'");

        // Both files exist in working tree
        assert!(repo.join("feature.txt").exists(), "feature.txt should exist");
        assert!(repo.join("main.txt").exists(), "main.txt should exist");

        // Git log shows a merge commit
        let log = log_oneline(&repo, 1);
        assert!(log.contains("Merge"), "top commit should be a merge commit, got: {}", log);

        // Git status is clean
        let status = git_status(&repo).unwrap();
        assert!(status.is_empty(), "working tree should be clean after merge");
    }

    // ── 2. Conflict → resolve ours ──

    #[test]
    fn test_full_merge_workflow_with_conflict_resolve_ours() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Shared file on main
        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared.txt"]).unwrap();

        // Feature changes it
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature version").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "feature edit"]).unwrap();

        // Main changes it differently
        let main = checkout_main(&repo);
        std::fs::write(repo.join("shared.txt"), "main version").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "main edit"]).unwrap();

        // Merge → conflict
        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_branch(&repo, main).unwrap();
        assert_eq!(result, "conflicts");

        // Verify conflict appears in merge state
        let state = get_merge_state(&repo).unwrap();
        assert!(state.is_merging);
        assert!(state.conflicted_files.iter().any(|f| f.path == "shared.txt"),
            "shared.txt should be in conflicted files");

        // Resolve with ours
        resolve_conflict_ours(&repo, "shared.txt").unwrap();
        continue_merge(&repo, "Merge: keep ours").unwrap();

        // Ours wins
        let content = std::fs::read_to_string(repo.join("shared.txt")).unwrap();
        assert_eq!(content, "feature version", "resolve_ours should keep feature content");

        // Clean
        assert!(git_status(&repo).unwrap().is_empty());

        // Log contains the merge message
        let log = log_oneline(&repo, 1);
        assert!(log.contains("Merge: keep ours"), "merge commit message mismatch: {}", log);
    }

    // ── 3. Conflict → resolve theirs ──

    #[test]
    fn test_full_merge_workflow_with_conflict_resolve_theirs() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared.txt"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature version").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "feature edit"]).unwrap();

        let main = checkout_main(&repo);
        std::fs::write(repo.join("shared.txt"), "main version").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "main edit"]).unwrap();

        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_branch(&repo, main).unwrap();
        assert_eq!(result, "conflicts");

        resolve_conflict_theirs(&repo, "shared.txt").unwrap();
        continue_merge(&repo, "Merge: keep theirs").unwrap();

        let content = std::fs::read_to_string(repo.join("shared.txt")).unwrap();
        assert_eq!(content, "main version", "resolve_theirs should keep main content");
        assert!(git_status(&repo).unwrap().is_empty());
    }

    // ── 4. Abort restores state exactly ──

    #[test]
    fn test_full_merge_workflow_abort_restores_state() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared.txt"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature version").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "feature edit"]).unwrap();

        let main = checkout_main(&repo);
        std::fs::write(repo.join("shared.txt"), "main version").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "main edit"]).unwrap();

        run_git(&repo, &["checkout", "feature"]).unwrap();
        let pre_merge_head = head_hash(&repo);

        let result = merge_branch(&repo, main).unwrap();
        assert_eq!(result, "conflicts");

        // Abort
        abort_merge(&repo).unwrap();

        // HEAD restored
        assert_eq!(head_hash(&repo), pre_merge_head, "HEAD should revert to pre-merge commit");

        // File restored
        let content = std::fs::read_to_string(repo.join("shared.txt")).unwrap();
        assert_eq!(content, "feature version", "file should revert to pre-merge content");

        // Clean status
        assert!(git_status(&repo).unwrap().is_empty(), "status should be clean after abort");

        // No active merge
        let state = get_merge_state(&repo).unwrap();
        assert!(!state.is_merging, "should not be merging after abort");
        assert!(!state.is_rebasing);
    }

    // ── 5. Dirty tree refused, uncommitted work preserved ──

    #[test]
    fn test_merge_refuses_dirty_working_tree() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Create a tracked file so we can modify it
        std::fs::write(repo.join("tracked.txt"), "original").unwrap();
        run_git(&repo, &["add", "tracked.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add tracked"]).unwrap();

        // Dirty the tree with a staged change
        std::fs::write(repo.join("tracked.txt"), "dirty").unwrap();
        run_git(&repo, &["add", "tracked.txt"]).unwrap();

        let main = main_branch(&repo);
        let result = merge_branch(&repo, main);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("uncommitted changes"));

        // Uncommitted change still intact
        let content = std::fs::read_to_string(repo.join("tracked.txt")).unwrap();
        assert_eq!(content, "dirty", "uncommitted change should not be lost");

        // No merge in progress
        let state = get_merge_state(&repo).unwrap();
        assert!(!state.is_merging);
    }

    // ── 6. Multiple conflicted files, mixed resolution strategies ──

    #[test]
    fn test_merge_multiple_conflicted_files() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Create 3 files
        for name in &["a.txt", "b.txt", "c.txt"] {
            std::fs::write(repo.join(name), "original").unwrap();
        }
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add three files"]).unwrap();

        // Feature modifies all 3
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("a.txt"), "feature-a").unwrap();
        std::fs::write(repo.join("b.txt"), "feature-b").unwrap();
        std::fs::write(repo.join("c.txt"), "feature-c").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature edits all"]).unwrap();

        // Main modifies all 3 differently
        let main = checkout_main(&repo);
        std::fs::write(repo.join("a.txt"), "main-a").unwrap();
        std::fs::write(repo.join("b.txt"), "main-b").unwrap();
        std::fs::write(repo.join("c.txt"), "main-c").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main edits all"]).unwrap();

        // Merge → 3 conflicts
        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_branch(&repo, main).unwrap();
        assert_eq!(result, "conflicts");

        let state = get_merge_state(&repo).unwrap();
        assert_eq!(state.conflicted_files.len(), 3, "all 3 files should conflict");

        // Resolve a.txt with ours
        resolve_conflict_ours(&repo, "a.txt").unwrap();
        // Resolve b.txt with theirs
        resolve_conflict_theirs(&repo, "b.txt").unwrap();
        // Resolve c.txt manually then mark resolved
        std::fs::write(repo.join("c.txt"), "manually merged").unwrap();
        mark_conflict_resolved(&repo, "c.txt").unwrap();

        continue_merge(&repo, "Merge with mixed resolutions").unwrap();

        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "feature-a");
        assert_eq!(std::fs::read_to_string(repo.join("b.txt")).unwrap(), "main-b");
        assert_eq!(std::fs::read_to_string(repo.join("c.txt")).unwrap(), "manually merged");

        assert!(git_status(&repo).unwrap().is_empty());
    }

    // ── 7. Already up to date (no-op merge) ──

    #[test]
    fn test_merge_when_already_up_to_date() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Feature branches from latest main, main has no new commits
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("feature.txt"), "work").unwrap();
        run_git(&repo, &["add", "feature.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "feature work"]).unwrap();

        let pre_merge_head = head_hash(&repo);
        let log_before = log_oneline(&repo, 5);

        let main = main_branch(&repo);
        let result = merge_branch(&repo, main).unwrap();
        assert_eq!(result, "up_to_date");

        // HEAD should not change (already up to date, no merge commit needed)
        assert_eq!(head_hash(&repo), pre_merge_head, "no new commit for already-up-to-date merge");

        // Log should be identical
        let log_after = log_oneline(&repo, 5);
        assert_eq!(log_before, log_after, "git log should not change");
    }

    // ── 8. File additions and deletions merge correctly ──

    #[test]
    fn test_merge_with_file_additions_and_deletions() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Create initial files
        std::fs::write(repo.join("keep.txt"), "keep me").unwrap();
        std::fs::write(repo.join("delete_on_main.txt"), "will be deleted on main").unwrap();
        std::fs::write(repo.join("delete_on_feature.txt"), "will be deleted on feature").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "initial files"]).unwrap();

        // Feature: delete one, add one
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::remove_file(repo.join("delete_on_feature.txt")).unwrap();
        std::fs::write(repo.join("new_feature.txt"), "new from feature").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature: delete and add"]).unwrap();

        // Main: delete a different one, add a different one
        let main = checkout_main(&repo);
        std::fs::remove_file(repo.join("delete_on_main.txt")).unwrap();
        std::fs::write(repo.join("new_main.txt"), "new from main").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main: delete and add"]).unwrap();

        // Merge
        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_branch(&repo, main).unwrap();
        assert_eq!(result, "merged", "non-overlapping adds/deletes should merge clean");

        // Verify final state
        assert!(repo.join("keep.txt").exists(), "keep.txt should survive");
        assert!(repo.join("new_feature.txt").exists(), "new_feature.txt should exist");
        assert!(repo.join("new_main.txt").exists(), "new_main.txt should exist from merge");
        assert!(!repo.join("delete_on_main.txt").exists(), "delete_on_main.txt should be gone");
        assert!(!repo.join("delete_on_feature.txt").exists(), "delete_on_feature.txt should be gone");

        assert!(git_status(&repo).unwrap().is_empty());
    }

    // ── 9. Merge does not touch main branch ──

    #[test]
    fn test_merge_does_not_touch_main_branch() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Diverge main and feature
        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "initial"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("feature.txt"), "feature").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature commit"]).unwrap();

        let main = checkout_main(&repo);
        std::fs::write(repo.join("main.txt"), "main").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main commit"]).unwrap();
        let main_head_before = head_hash(&repo);
        let main_log_before = log_oneline(&repo, 10);

        // Merge main into feature
        run_git(&repo, &["checkout", "feature"]).unwrap();
        merge_branch(&repo, main).unwrap();

        // Switch back to main — verify untouched
        run_git(&repo, &["checkout", main]).unwrap();
        assert_eq!(head_hash(&repo), main_head_before, "main HEAD should be unchanged");
        assert_eq!(log_oneline(&repo, 10), main_log_before, "main history should be unchanged");
        assert!(!repo.join("feature.txt").exists(), "feature.txt should NOT exist on main");
    }

    // ── 10. Second merge after conflict resolution is clean ──

    #[test]
    fn test_conflict_resolution_then_second_merge_is_clean() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Set up first conflict
        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "initial"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature v1").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature v1"]).unwrap();

        let main = checkout_main(&repo);
        std::fs::write(repo.join("shared.txt"), "main v1").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main v1"]).unwrap();

        // First merge: conflicts
        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_branch(&repo, main).unwrap();
        assert_eq!(result, "conflicts", "first merge should conflict");
        resolve_conflict_ours(&repo, "shared.txt").unwrap();
        continue_merge(&repo, "First merge resolved").unwrap();

        // Main adds a NEW file (no conflict with feature)
        run_git(&repo, &["checkout", main]).unwrap();
        std::fs::write(repo.join("new_main.txt"), "new on main").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add new_main.txt"]).unwrap();

        // Second merge: should be clean
        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_branch(&repo, main).unwrap();
        assert_eq!(result, "merged", "second merge should be clean");

        assert!(repo.join("new_main.txt").exists(), "new file from main should appear");
        assert!(git_status(&repo).unwrap().is_empty());
    }

    // ── 11. Dry-run (check_merge_conflicts) does not modify repo ──

    #[test]
    fn test_check_merge_conflicts_dry_run_does_not_modify_repo() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "initial"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature edit"]).unwrap();

        let main = checkout_main(&repo);
        std::fs::write(repo.join("shared.txt"), "main").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main edit"]).unwrap();

        // Record state before dry run
        run_git(&repo, &["checkout", "feature"]).unwrap();
        let head_before = head_hash(&repo);
        let content_before = std::fs::read_to_string(repo.join("shared.txt")).unwrap();

        // Dry-run conflict check
        let result = check_merge_conflicts(&repo, main).unwrap();
        assert!(result.has_conflicts, "should detect conflicts");
        assert!(!result.conflicting_files.is_empty());

        // State unchanged
        assert_eq!(head_hash(&repo), head_before, "HEAD should not change after dry run");
        let content_after = std::fs::read_to_string(repo.join("shared.txt")).unwrap();
        assert_eq!(content_before, content_after, "file content should not change after dry run");
        assert!(git_status(&repo).unwrap().is_empty(), "working tree should be clean after dry run");

        let state = get_merge_state(&repo).unwrap();
        assert!(!state.is_merging, "no merge should be in progress after dry run");
    }

    // ── merge_into_base comprehensive integration tests ──

    #[test]
    fn test_merge_into_base_clean() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        let main = main_branch(&repo);

        // Feature branch adds a file
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("feature.txt"), "feature work").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature commit"]).unwrap();

        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "merged");
        assert!(result.temp_branch.is_none(), "temp branch should be cleaned up");
        assert_eq!(result.source_branch, "feature");

        // Should stay on source branch (not switch to main)
        let current = run_git(&repo, &["branch", "--show-current"]).unwrap();
        assert_eq!(current, "feature");

        // Main ref should have the merge commit
        let main_log = run_git(&repo, &["log", "--oneline", "-1", main]).unwrap();
        assert!(main_log.contains("Merge feature into"), "should have merge commit: {}", main_log);

        assert!(git_status(&repo).unwrap().is_empty());
    }

    #[test]
    fn test_merge_into_base_clean_deletes_source() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("f.txt"), "work").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature"]).unwrap();

        let result = merge_into_base(&repo, main, true).unwrap();
        assert_eq!(result.status, "merged");

        let branches = git_list_branches(&repo, false).unwrap();
        assert!(!branches.contains(&"feature".to_string()), "feature branch should be deleted");
    }

    #[test]
    fn test_merge_into_base_already_on_base_error() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        let result = merge_into_base(&repo, main, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Already on"), "should say already on base: {}", err);
        assert!(err.contains("Switch to a feature branch"), "should suggest switching: {}", err);
    }

    #[test]
    fn test_merge_into_base_with_conflicts() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        // Shared file
        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared"]).unwrap();

        let main_head_before = head_hash(&repo);

        // Feature changes it
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature version").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature edit"]).unwrap();

        // Main changes it differently
        run_git(&repo, &["checkout", main]).unwrap();
        std::fs::write(repo.join("shared.txt"), "main version").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main edit"]).unwrap();
        let main_head_after_diverge = head_hash(&repo);

        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "conflicts");
        assert!(result.temp_branch.is_some());
        assert!(!result.conflicted_files.is_empty());

        // Should be on temp branch (not main, not feature)
        let current = run_git(&repo, &["branch", "--show-current"]).unwrap();
        assert!(current.starts_with("merge/"), "should be on temp branch: {}", current);

        // Main MUST be unchanged
        let main_head_now = run_git(&repo, &["rev-parse", main]).unwrap();
        assert_eq!(main_head_now.trim(), main_head_after_diverge.trim(), "main HEAD must not change");

        // Cleanup
        abort_merge_into_base(&repo, "feature", &result.temp_branch.unwrap()).unwrap();
    }

    #[test]
    fn test_merge_into_base_complete_after_resolve() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature version").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature edit"]).unwrap();

        run_git(&repo, &["checkout", main]).unwrap();
        std::fs::write(repo.join("shared.txt"), "main version").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main edit"]).unwrap();

        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "conflicts");
        let temp = result.temp_branch.unwrap();

        // Resolve
        resolve_conflict_theirs(&repo, "shared.txt").unwrap();

        // Complete
        complete_merge_into_base(&repo, main, &temp, "feature", false).unwrap();

        // Should be back on source branch (not main)
        let current = run_git(&repo, &["branch", "--show-current"]).unwrap();
        assert_eq!(current, "feature");

        // Main ref has the resolved content
        run_git(&repo, &["checkout", main]).unwrap();
        let content = std::fs::read_to_string(repo.join("shared.txt")).unwrap();
        assert_eq!(content, "feature version", "theirs = feature on temp branch");

        // Temp branch gone
        let branches = git_list_branches(&repo, false).unwrap();
        assert!(!branches.iter().any(|b| b.starts_with("merge/")), "temp branch should be deleted");

        // Feature branch still exists (delete_source=false)
        assert!(branches.contains(&"feature".to_string()));
    }

    #[test]
    fn test_merge_into_base_complete_with_branch_deletion() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("feature.txt"), "work").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature"]).unwrap();

        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "merged");

        // Now re-create feature and do a conflict merge to test delete_source
        run_git(&repo, &["checkout", "-b", "feature2"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "f2").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "f2"]).unwrap();

        run_git(&repo, &["checkout", main]).unwrap();
        std::fs::write(repo.join("shared.txt"), "main2").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main2"]).unwrap();

        run_git(&repo, &["checkout", "feature2"]).unwrap();
        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "conflicts");
        let temp = result.temp_branch.unwrap();

        resolve_conflict_ours(&repo, "shared.txt").unwrap();
        complete_merge_into_base(&repo, main, &temp, "feature2", true).unwrap();

        let branches = git_list_branches(&repo, false).unwrap();
        assert!(!branches.contains(&"feature2".to_string()), "feature2 should be deleted");
    }

    #[test]
    fn test_merge_into_base_abort_restores_everything() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared"]).unwrap();
        let main_head = head_hash(&repo);

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature"]).unwrap();
        let feature_head = head_hash(&repo);

        run_git(&repo, &["checkout", main]).unwrap();
        std::fs::write(repo.join("shared.txt"), "main").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main"]).unwrap();
        let main_head_diverged = head_hash(&repo);

        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "conflicts");
        let temp = result.temp_branch.unwrap();

        // Abort
        abort_merge_into_base(&repo, "feature", &temp).unwrap();

        // Back on feature
        let current = run_git(&repo, &["branch", "--show-current"]).unwrap();
        assert_eq!(current, "feature");
        assert_eq!(head_hash(&repo), feature_head);

        // Main untouched
        let main_now = run_git(&repo, &["rev-parse", main]).unwrap();
        assert_eq!(main_now.trim(), main_head_diverged.trim());

        // Temp branch gone
        let branches = git_list_branches(&repo, false).unwrap();
        assert!(!branches.iter().any(|b| b.starts_with("merge/")));

        // Feature content unchanged
        let content = std::fs::read_to_string(repo.join("shared.txt")).unwrap();
        assert_eq!(content, "feature");

        // No merge in progress
        assert!(!get_merge_state(&repo).unwrap().is_merging);
    }

    #[test]
    fn test_merge_into_base_main_never_dirty() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature"]).unwrap();

        run_git(&repo, &["checkout", main]).unwrap();
        std::fs::write(repo.join("shared.txt"), "main").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main"]).unwrap();
        let main_head = head_hash(&repo);
        let main_content = std::fs::read_to_string(repo.join("shared.txt")).unwrap();

        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "conflicts");
        let temp = result.temp_branch.clone().unwrap();

        // Check main is pristine via rev-parse (no checkout needed)
        let main_head_now = run_git(&repo, &["rev-parse", main]).unwrap();
        assert_eq!(main_head_now.trim(), main_head.trim(), "main HEAD must be pristine during conflicts");

        // Resolve conflict on temp branch
        resolve_conflict_ours(&repo, "shared.txt").unwrap();

        // Main still pristine before complete
        let main_head_still = run_git(&repo, &["rev-parse", main]).unwrap();
        assert_eq!(main_head_still.trim(), main_head.trim(), "main still pristine after resolve");

        complete_merge_into_base(&repo, main, &temp, "feature", false).unwrap();

        // NOW main has changes (check via rev-parse since we stay on feature)
        let main_head_after = run_git(&repo, &["rev-parse", main]).unwrap();
        assert_ne!(main_head_after.trim(), main_head.trim(), "main should have new commits after complete");
    }

    #[test]
    fn test_merge_into_base_refuses_dirty_tree() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("dirty.txt"), "uncommitted").unwrap();
        run_git(&repo, &["add", "."]).unwrap();

        let result = merge_into_base(&repo, main, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("uncommitted changes"));

        // No temp branch created
        let branches = git_list_branches(&repo, false).unwrap();
        assert!(!branches.iter().any(|b| b.starts_with("merge/")));
    }

    #[test]
    fn test_merge_into_base_already_up_to_date() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        // Feature with no new commits vs main
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();

        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "already_up_to_date");
        assert!(result.temp_branch.is_none());
    }

    #[test]
    fn test_merge_into_base_diverged_clean() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        // Feature adds file A
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("a.txt"), "from feature").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add a"]).unwrap();

        // Main adds file B (no conflict)
        run_git(&repo, &["checkout", main]).unwrap();
        std::fs::write(repo.join("b.txt"), "from main").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add b"]).unwrap();

        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "merged");

        // Verify main has both files via git ls-tree (we stay on feature)
        let tree = run_git(&repo, &["ls-tree", "--name-only", main]).unwrap();
        assert!(tree.contains("a.txt"), "main should have a.txt");
        assert!(tree.contains("b.txt"), "main should have b.txt");
    }

    #[test]
    fn test_merge_into_base_multiple_conflicts() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        for name in &["a.txt", "b.txt", "c.txt"] {
            std::fs::write(repo.join(name), "original").unwrap();
        }
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add files"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        for name in &["a.txt", "b.txt", "c.txt"] {
            std::fs::write(repo.join(name), format!("feature-{}", name)).unwrap();
        }
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature edits"]).unwrap();

        run_git(&repo, &["checkout", main]).unwrap();
        for name in &["a.txt", "b.txt", "c.txt"] {
            std::fs::write(repo.join(name), format!("main-{}", name)).unwrap();
        }
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main edits"]).unwrap();

        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "conflicts");
        assert_eq!(result.conflicted_files.len(), 3);
        let temp = result.temp_branch.unwrap();

        // Resolve each differently
        resolve_conflict_ours(&repo, "a.txt").unwrap();
        resolve_conflict_theirs(&repo, "b.txt").unwrap();
        std::fs::write(repo.join("c.txt"), "manually merged").unwrap();
        mark_conflict_resolved(&repo, "c.txt").unwrap();

        complete_merge_into_base(&repo, main, &temp, "feature", false).unwrap();

        // Verify content on main (checkout to inspect working tree)
        run_git(&repo, &["checkout", main]).unwrap();
        // Note: on the temp branch, "ours" = main, "theirs" = feature
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), format!("main-a.txt"));
        assert_eq!(std::fs::read_to_string(repo.join("b.txt")).unwrap(), format!("feature-b.txt"));
        assert_eq!(std::fs::read_to_string(repo.join("c.txt")).unwrap(), "manually merged");
    }

    #[test]
    fn test_orphaned_temp_branch_cleanup() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "feature").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "feature"]).unwrap();

        run_git(&repo, &["checkout", main]).unwrap();
        std::fs::write(repo.join("shared.txt"), "main").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main"]).unwrap();

        run_git(&repo, &["checkout", "feature"]).unwrap();
        let result = merge_into_base(&repo, main, false).unwrap();
        let temp = result.temp_branch.unwrap();

        // Simulate app crash — don't complete or abort
        // Later, call abort with known branch names
        abort_merge_into_base(&repo, "feature", &temp).unwrap();

        // Cleaned up
        let current = run_git(&repo, &["branch", "--show-current"]).unwrap();
        assert_eq!(current, "feature");
        let branches = git_list_branches(&repo, false).unwrap();
        assert!(!branches.iter().any(|b| b.starts_with("merge/")));
    }

    #[test]
    fn test_merge_into_base_temp_branch_naming() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let main = main_branch(&repo);

        run_git(&repo, &["checkout", "-b", "feat/my-feature"]).unwrap();
        std::fs::write(repo.join("f.txt"), "work").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "work"]).unwrap();

        let result = merge_into_base(&repo, main, false).unwrap();
        assert_eq!(result.status, "merged");

        // Verify merge commit message on main references the branch
        let log = run_git(&repo, &["log", "--oneline", "-1", main]).unwrap();
        assert!(log.contains("feat/my-feature"), "merge commit should name the branch: {}", log);
    }

    // ── merge_into_base worktree tests ──

    /// Helper: create a repo with main checked out, then a worktree on `feature`.
    /// Returns (_dir, main_repo_path, worktree_path).
    fn setup_worktree_merge_scenario() -> (TempDir, PathBuf, PathBuf) {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let wt_path_str = git_create_worktree(&repo, "feature", true, None, None)
            .expect("create worktree");
        git_config(&PathBuf::from(&wt_path_str));
        (_dir, repo, PathBuf::from(wt_path_str))
    }

    #[test]
    fn test_merge_into_base_worktree_clean() {
        let (_dir, repo, wt) = setup_worktree_merge_scenario();
        let main = main_branch(&repo);

        // Add a file in the worktree (feature branch)
        std::fs::write(wt.join("feature.txt"), "worktree work").unwrap();
        run_git(&wt, &["add", "."]).unwrap();
        run_git(&wt, &["commit", "-m", "feature commit"]).unwrap();

        // Merge from the worktree — main is checked out in parent repo
        let result = merge_into_base(&wt, main, false).unwrap();
        assert_eq!(result.status, "merged");
        assert!(result.temp_branch.is_none(), "temp branch should be cleaned up");

        // User stays on feature in the worktree
        let current = run_git(&wt, &["branch", "--show-current"]).unwrap();
        assert_eq!(current, "feature");

        // Main ref was updated
        let main_log = run_git(&wt, &["log", "--oneline", "-1", main]).unwrap();
        assert!(main_log.contains("Merge feature into"), "main should have merge commit: {}", main_log);

        // Main in the parent repo also reflects the update
        let parent_log = run_git(&repo, &["log", "--oneline", "-1"]).unwrap();
        assert!(parent_log.contains("Merge feature into"), "parent should see merge: {}", parent_log);

        // Main worktree files on disk must reflect the merged content (not stale)
        let on_disk = std::fs::read_to_string(repo.join("feature.txt")).unwrap();
        assert_eq!(on_disk, "worktree work", "main worktree file must match merged content");
    }

    #[test]
    fn test_merge_into_base_worktree_main_files_updated() {
        let (_dir, repo, wt) = setup_worktree_merge_scenario();
        let main = main_branch(&repo);

        // Create a file on main first so we can verify it changes
        std::fs::write(repo.join("shared.txt"), "original from main").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared"]).unwrap();

        // Pull into worktree, modify, and add a new file
        run_git(&wt, &["merge", main]).unwrap();
        std::fs::write(wt.join("shared.txt"), "updated by feature").unwrap();
        std::fs::write(wt.join("new-feature.txt"), "brand new file").unwrap();
        run_git(&wt, &["add", "."]).unwrap();
        run_git(&wt, &["commit", "-m", "feature changes"]).unwrap();

        // Verify main worktree still has old content before merge
        assert_eq!(
            std::fs::read_to_string(repo.join("shared.txt")).unwrap(),
            "original from main",
        );
        assert!(!repo.join("new-feature.txt").exists());

        let result = merge_into_base(&wt, main, false).unwrap();
        assert_eq!(result.status, "merged");

        // After merge, main worktree files on disk must reflect merged content
        assert_eq!(
            std::fs::read_to_string(repo.join("shared.txt")).unwrap(),
            "updated by feature",
            "shared.txt in main worktree must have merged content",
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("new-feature.txt")).unwrap(),
            "brand new file",
            "new-feature.txt must appear in main worktree after merge",
        );

        // Git status in main worktree must be clean (no fake unstaged changes)
        let main_status = git_status(&repo).unwrap();
        assert!(main_status.is_empty(), "main worktree must be clean after merge, got: {:?}", main_status);
    }

    #[test]
    fn test_merge_into_base_worktree_conflicts() {
        let (_dir, repo, wt) = setup_worktree_merge_scenario();
        let main = main_branch(&repo);

        // Create shared file on main
        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared"]).unwrap();
        let main_head = run_git(&repo, &["rev-parse", main]).unwrap();

        // Pull the shared file into worktree, then modify
        run_git(&wt, &["merge", main]).unwrap();
        std::fs::write(wt.join("shared.txt"), "feature version").unwrap();
        run_git(&wt, &["add", "."]).unwrap();
        run_git(&wt, &["commit", "-m", "feature edit"]).unwrap();

        // Diverge main
        std::fs::write(repo.join("shared.txt"), "main version").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main edit"]).unwrap();
        let main_head_diverged = run_git(&repo, &["rev-parse", main]).unwrap();

        let result = merge_into_base(&wt, main, false).unwrap();
        assert_eq!(result.status, "conflicts");
        assert!(result.temp_branch.is_some());
        assert!(!result.conflicted_files.is_empty());

        // On temp branch in worktree
        let current = run_git(&wt, &["branch", "--show-current"]).unwrap();
        assert!(current.starts_with("merge/"), "should be on temp branch: {}", current);

        // Main must be unchanged
        let main_now = run_git(&wt, &["rev-parse", main]).unwrap();
        assert_eq!(main_now.trim(), main_head_diverged.trim(), "main must not change during conflicts");

        // Cleanup
        abort_merge_into_base(&wt, "feature", &result.temp_branch.unwrap()).unwrap();
    }

    #[test]
    fn test_merge_into_base_worktree_complete_after_resolve() {
        let (_dir, repo, wt) = setup_worktree_merge_scenario();
        let main = main_branch(&repo);

        // Create shared file on main
        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared"]).unwrap();

        // Pull into worktree, then modify
        run_git(&wt, &["merge", main]).unwrap();
        std::fs::write(wt.join("shared.txt"), "feature version").unwrap();
        run_git(&wt, &["add", "."]).unwrap();
        run_git(&wt, &["commit", "-m", "feature edit"]).unwrap();

        // Diverge main
        std::fs::write(repo.join("shared.txt"), "main version").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main edit"]).unwrap();

        let result = merge_into_base(&wt, main, false).unwrap();
        assert_eq!(result.status, "conflicts");
        let temp = result.temp_branch.unwrap();

        // Resolve
        resolve_conflict_theirs(&wt, "shared.txt").unwrap();
        complete_merge_into_base(&wt, main, &temp, "feature", false).unwrap();

        // Back on feature in worktree
        let current = run_git(&wt, &["branch", "--show-current"]).unwrap();
        assert_eq!(current, "feature");

        // Main ref updated (check from parent repo too)
        let content = run_git(&repo, &["show", &format!("{}:shared.txt", main)]).unwrap();
        assert_eq!(content, "feature version", "main should have resolved content");

        // Temp branch cleaned up
        let branches = git_list_branches(&wt, false).unwrap();
        assert!(!branches.iter().any(|b| b.starts_with("merge/")), "temp branch should be deleted");
    }

    #[test]
    fn test_merge_into_base_worktree_abort() {
        let (_dir, repo, wt) = setup_worktree_merge_scenario();
        let main = main_branch(&repo);

        std::fs::write(repo.join("shared.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared"]).unwrap();

        run_git(&wt, &["merge", main]).unwrap();
        std::fs::write(wt.join("shared.txt"), "feature version").unwrap();
        run_git(&wt, &["add", "."]).unwrap();
        run_git(&wt, &["commit", "-m", "feature edit"]).unwrap();
        let feature_head = run_git(&wt, &["rev-parse", "HEAD"]).unwrap();

        std::fs::write(repo.join("shared.txt"), "main version").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main edit"]).unwrap();
        let main_head = run_git(&repo, &["rev-parse", main]).unwrap();

        let result = merge_into_base(&wt, main, false).unwrap();
        assert_eq!(result.status, "conflicts");
        let temp = result.temp_branch.unwrap();

        abort_merge_into_base(&wt, "feature", &temp).unwrap();

        // Back on feature
        let current = run_git(&wt, &["branch", "--show-current"]).unwrap();
        assert_eq!(current, "feature");
        let head = run_git(&wt, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(head.trim(), feature_head.trim());

        // Main unchanged
        let main_now = run_git(&wt, &["rev-parse", main]).unwrap();
        assert_eq!(main_now.trim(), main_head.trim());

        // Temp branch gone
        let branches = git_list_branches(&wt, false).unwrap();
        assert!(!branches.iter().any(|b| b.starts_with("merge/")));
    }

    #[test]
    fn test_merge_into_base_worktree_diverged_during_merge() {
        let (_dir, repo, wt) = setup_worktree_merge_scenario();
        let main = main_branch(&repo);

        // Feature adds a file
        std::fs::write(wt.join("feature.txt"), "work").unwrap();
        run_git(&wt, &["add", "."]).unwrap();
        run_git(&wt, &["commit", "-m", "feature"]).unwrap();

        // Start the merge (will create temp branch and merge cleanly)
        // But first, simulate: someone pushes to main AFTER the temp branch is created
        // We do this by making merge_into_base succeed at the merge step,
        // then manually advancing main before the branch -f check.
        //
        // Since we can't intercept mid-function, test the is-ancestor check directly:
        // Create a situation where base is NOT an ancestor of temp.

        // Make main diverge: add a commit to main that's not in feature
        std::fs::write(repo.join("main-only.txt"), "main diverge").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "main diverge"]).unwrap();

        // Create temp branch from an OLD main (before the diverge)
        // Simulate: temp was created from old main, then main advanced
        let old_main = run_git(&wt, &["rev-parse", "HEAD"]).unwrap(); // feature head
        let temp = "merge/test-diverge";
        run_git(&wt, &["branch", temp, &old_main.trim()]).unwrap();

        // Now verify: main is NOT an ancestor of temp (main has diverged)
        let (_, _, is_ancestor) = run_git_full(
            &wt,
            &["merge-base", "--is-ancestor", main, temp],
        ).unwrap();
        assert!(!is_ancestor, "main should NOT be ancestor of temp (diverged)");

        // Cleanup
        let _ = run_git(&wt, &["branch", "-D", temp]);
    }

    #[test]
    fn test_merge_into_base_worktree_delete_source_branch() {
        let (_dir, repo, wt) = setup_worktree_merge_scenario();
        let main = main_branch(&repo);

        std::fs::write(wt.join("feature.txt"), "work").unwrap();
        run_git(&wt, &["add", "."]).unwrap();
        run_git(&wt, &["commit", "-m", "feature"]).unwrap();

        let result = merge_into_base(&wt, main, true).unwrap();
        assert_eq!(result.status, "merged");

        // Source branch deleted
        let branches = git_list_branches(&wt, false).unwrap();
        assert!(!branches.contains(&"feature".to_string()), "feature should be deleted");

        // In worktree scenario, can't checkout main (already checked out),
        // so we end up on detached HEAD
        let current = run_git(&wt, &["branch", "--show-current"]).unwrap();
        assert!(current.is_empty(), "should be on detached HEAD in worktree after source deletion");
    }

    #[test]
    fn test_merge_into_base_worktree_user_stays_on_source() {
        let (_dir, repo, wt) = setup_worktree_merge_scenario();
        let main = main_branch(&repo);

        std::fs::write(wt.join("feature.txt"), "work").unwrap();
        run_git(&wt, &["add", "."]).unwrap();
        run_git(&wt, &["commit", "-m", "feature"]).unwrap();

        let result = merge_into_base(&wt, main, false).unwrap();
        assert_eq!(result.status, "merged");

        // Verify HEAD in worktree is feature, not main
        let current = run_git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(current, "feature", "user must stay on source branch in worktree");

        // Verify main was updated
        let main_tree = run_git(&wt, &["ls-tree", "--name-only", main]).unwrap();
        assert!(main_tree.contains("feature.txt"), "main should have feature.txt");
    }

    // ── git_fetch tests ──

    #[test]
    fn test_git_fetch_pulls_remote_refs() {
        let (_dir, local, remote) = setup_test_repo_with_remote();

        // Push a new commit from a second clone so remote is ahead
        let clone2 = _dir.path().join("clone2");
        run_git(
            _dir.path(),
            &["clone", remote.to_str().unwrap(), clone2.to_str().unwrap()],
        )
        .expect("clone2");
        std::fs::write(clone2.join("fetched.txt"), "fetch me").expect("write in clone2");
        run_git(&clone2, &["add", "fetched.txt"]).expect("add");
        run_git(
            &clone2,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "fetch target"],
        )
        .expect("commit in clone2");
        run_git(&clone2, &["push"]).expect("push from clone2");

        // Fetch (not pull) in local — working tree should NOT have the file yet
        git_fetch(&local).expect("fetch");
        assert!(
            !local.join("fetched.txt").exists(),
            "fetch should not update working tree"
        );

        // But the remote ref should be updated — branch info should show behind
        let info = git_branch_info(&local).expect("branch info after fetch");
        assert!(info.behind > 0, "should be behind after fetch: behind={}", info.behind);
    }

    #[test]
    fn test_git_fetch_no_remote_is_noop() {
        let (_dir, repo) = setup_test_repo();
        // Repo has no remote configured — git fetch exits 0 (no-op)
        let result = git_fetch(&repo);
        assert!(result.is_ok(), "fetch on repo with no remote should succeed as no-op");
    }

    // ── git_stash tests ──

    #[test]
    fn test_git_stash_push_stashes_changes() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Create and commit a file
        std::fs::write(repo.join("file.txt"), "original").unwrap();
        run_git(&repo, &["add", "file.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add file"]).unwrap();

        // Modify it (unstaged change)
        std::fs::write(repo.join("file.txt"), "modified").unwrap();
        let status = git_status(&repo).unwrap();
        assert!(!status.is_empty(), "should have changes before stash");

        // Stash
        git_stash_push(&repo, false).expect("stash push");

        // Working tree should be clean
        let status = git_status(&repo).unwrap();
        assert!(status.is_empty(), "working tree should be clean after stash");

        // File should be back to original
        let content = std::fs::read_to_string(repo.join("file.txt")).unwrap();
        assert_eq!(content, "original", "file should be reverted after stash");
    }

    #[test]
    fn test_git_stash_push_include_untracked() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Create an untracked file
        std::fs::write(repo.join("untracked.txt"), "new file").unwrap();

        // Stash without include_untracked — untracked file should remain
        // (nothing tracked to stash, so this would error — create a tracked change too)
        std::fs::write(repo.join("init.txt"), "tracked").unwrap();
        run_git(&repo, &["add", "init.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add init"]).unwrap();
        std::fs::write(repo.join("init.txt"), "changed").unwrap();

        git_stash_push(&repo, false).expect("stash without untracked");
        assert!(
            repo.join("untracked.txt").exists(),
            "untracked file should still exist after stash without --include-untracked"
        );

        // Now create another tracked change + untracked file and stash with include_untracked
        std::fs::write(repo.join("init.txt"), "changed again").unwrap();
        std::fs::write(repo.join("new_untracked.txt"), "also new").unwrap();

        git_stash_push(&repo, true).expect("stash with untracked");
        assert!(
            !repo.join("new_untracked.txt").exists(),
            "untracked file should be gone after stash --include-untracked"
        );
    }

    #[test]
    fn test_git_stash_push_nothing_to_stash_is_noop() {
        let (_dir, repo) = setup_test_repo();
        // Clean working tree — git stash push exits 0 with "No local changes to save"
        let result = git_stash_push(&repo, false);
        assert!(result.is_ok(), "stash with nothing to stash should succeed as no-op");
    }

    #[test]
    fn test_git_stash_pop_restores_changes() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Create, commit, modify
        std::fs::write(repo.join("file.txt"), "original").unwrap();
        run_git(&repo, &["add", "file.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add file"]).unwrap();
        std::fs::write(repo.join("file.txt"), "stashed content").unwrap();

        // Stash then pop
        git_stash_push(&repo, false).expect("stash");
        assert_eq!(
            std::fs::read_to_string(repo.join("file.txt")).unwrap(),
            "original",
            "should be clean after stash"
        );

        git_stash_pop(&repo).expect("stash pop");
        let content = std::fs::read_to_string(repo.join("file.txt")).unwrap();
        assert_eq!(content, "stashed content", "pop should restore stashed changes");
    }

    #[test]
    fn test_git_stash_pop_empty_stash_returns_error() {
        let (_dir, repo) = setup_test_repo();
        // No stash entries
        let result = git_stash_pop(&repo);
        assert!(result.is_err(), "pop with empty stash should error");
    }

    // ── git_checkpoint tests (issue #80) ──

    /// Raw `git status --porcelain` so staged-vs-unstaged is visible.
    fn porcelain(repo: &Path) -> String {
        run_git_permissive(repo, &["status", "--porcelain"])
    }

    #[test]
    fn checkpoint_sanitize_ref_component() {
        assert_eq!(sanitize_ref_component("chat-pane-3-17000"), "chat-pane-3-17000");
        assert_eq!(sanitize_ref_component("a b@{c..d}.lock"), "a-b--c--d--lock");
        assert_eq!(sanitize_ref_component("???"), "checkpoint");
        assert_eq!(sanitize_ref_component(""), "checkpoint");
        let long = "x".repeat(300);
        assert_eq!(sanitize_ref_component(&long).len(), 100);
    }

    #[test]
    fn checkpoint_create_skips_non_repo_and_unborn_head() {
        let dir = TempDir::new().unwrap();
        // Plain directory — not a repo.
        let result =
            git_checkpoint_create(dir.path(), "refs/codemux/checkpoints/t", "msg").unwrap();
        assert!(result.is_none(), "non-repo should skip");

        // Fresh init, zero commits — unborn HEAD.
        let unborn = TempDir::new().unwrap();
        run_git(unborn.path(), &["init"]).unwrap();
        let result =
            git_checkpoint_create(unborn.path(), "refs/codemux/checkpoints/t", "msg").unwrap();
        assert!(result.is_none(), "unborn HEAD should skip");
    }

    #[test]
    fn checkpoint_create_does_not_disturb_index_worktree_or_stash() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Build a mixed state: one committed+staged file, one
        // committed+unstaged file, one untracked file.
        std::fs::write(repo.join("staged.txt"), "v1").unwrap();
        std::fs::write(repo.join("unstaged.txt"), "v1").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "base"]).unwrap();
        std::fs::write(repo.join("staged.txt"), "v2-staged").unwrap();
        run_git(&repo, &["add", "staged.txt"]).unwrap();
        std::fs::write(repo.join("unstaged.txt"), "v2-unstaged").unwrap();
        std::fs::write(repo.join("untracked.txt"), "new").unwrap();

        let before = porcelain(&repo);
        assert!(before.contains("M  staged.txt"), "precondition: staged change");
        assert!(before.contains(" M unstaged.txt"), "precondition: unstaged change");
        assert!(before.contains("?? untracked.txt"), "precondition: untracked file");

        let ref_name = "refs/codemux/checkpoints/thread-1";
        let cp = git_checkpoint_create(&repo, ref_name, "test checkpoint")
            .unwrap()
            .expect("checkpoint should be created");

        // The user's state is byte-identical afterwards.
        assert_eq!(porcelain(&repo), before, "status must be undisturbed");
        let stash = run_git_permissive(&repo, &["stash", "list"]);
        assert!(stash.is_empty(), "stash list must stay empty, got: {stash}");

        // The snapshot captured all three flavors of change.
        for (file, expect) in [
            ("staged.txt", "v2-staged"),
            ("unstaged.txt", "v2-unstaged"),
            ("untracked.txt", "new"),
        ] {
            let content = run_git(
                &repo,
                &["show", &format!("{}:{file}", cp.snapshot_commit)],
            )
            .unwrap();
            assert_eq!(content, expect, "snapshot content for {file}");
        }

        // Anchored on the shadow ref, parented on HEAD.
        let resolved = run_git(&repo, &["rev-parse", ref_name]).unwrap();
        assert_eq!(resolved, cp.snapshot_commit);
        let parent = run_git(&repo, &["rev-parse", &format!("{}^", cp.snapshot_commit)]).unwrap();
        assert_eq!(parent, cp.head_commit);
        // Branch name depends on the machine's init.defaultBranch —
        // assert against whatever the repo actually reports.
        let current_branch = run_git(&repo, &["branch", "--show-current"]).unwrap();
        assert_eq!(cp.branch.as_deref(), Some(current_branch.as_str()), "branch recorded");
    }

    #[test]
    fn checkpoint_restore_round_trip_undoes_a_simulated_run() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        // Default-branch name differs across git versions; pin it.
        run_git(&repo, &["checkout", "-B", "main"]).unwrap();

        // Pre-run state: tracked file with an unstaged edit, an
        // untracked file, and an ignored file.
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(repo.join("code.txt"), "original").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "base"]).unwrap();
        std::fs::write(repo.join("code.txt"), "user-edit").unwrap();
        std::fs::write(repo.join("notes.txt"), "user-notes").unwrap();
        std::fs::write(repo.join("ignored.txt"), "local-junk").unwrap();

        let cp = git_checkpoint_create(
            &repo,
            "refs/codemux/checkpoints/run-1",
            "run checkpoint",
        )
        .unwrap()
        .expect("checkpoint created");

        // Simulated agent run: edits, deletions, new files, a commit.
        std::fs::write(repo.join("code.txt"), "agent-rewrite").unwrap();
        std::fs::remove_file(repo.join("notes.txt")).unwrap();
        std::fs::write(repo.join("agent-new.txt"), "agent artifact").unwrap();
        run_git(&repo, &["add", "-A"]).unwrap();
        run_git(&repo, &["commit", "-m", "agent commit"]).unwrap();
        let agent_head = run_git(&repo, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(repo.join("agent-new2.txt"), "uncommitted artifact").unwrap();

        git_checkpoint_restore(
            &repo,
            &cp.snapshot_commit,
            &cp.head_commit,
            cp.branch.as_deref(),
            "refs/codemux/pre-restore/run-1",
        )
        .expect("restore should succeed");

        // Branch is back on the pre-run commit.
        assert_eq!(run_git(&repo, &["rev-parse", "HEAD"]).unwrap(), cp.head_commit);
        assert_eq!(
            run_git(&repo, &["branch", "--show-current"]).unwrap(),
            "main"
        );
        // File contents are back to the pre-run state.
        assert_eq!(
            std::fs::read_to_string(repo.join("code.txt")).unwrap(),
            "user-edit"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("notes.txt")).unwrap(),
            "user-notes"
        );
        // Run artifacts are gone; ignored local files survive.
        assert!(!repo.join("agent-new.txt").exists(), "committed artifact removed");
        assert!(!repo.join("agent-new2.txt").exists(), "uncommitted artifact removed");
        assert_eq!(
            std::fs::read_to_string(repo.join("ignored.txt")).unwrap(),
            "local-junk",
            "ignored file spared by clean"
        );
        // Restored dirty state shows as unstaged + untracked.
        let status = porcelain(&repo);
        assert!(status.contains(" M code.txt"), "edit unstaged, got: {status}");
        assert!(status.contains("?? notes.txt"), "untracked again, got: {status}");
        // The safety ref exists and keeps the agent's commit reachable.
        let safety = run_git(&repo, &["rev-parse", "refs/codemux/pre-restore/run-1"]).unwrap();
        let safety_parent = run_git(&repo, &["rev-parse", &format!("{safety}^")]).unwrap();
        assert_eq!(safety_parent, agent_head, "safety snapshot parents the run's last commit");
    }

    #[test]
    fn checkpoint_restore_refuses_branch_mismatch() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        run_git(&repo, &["checkout", "-B", "main"]).unwrap();
        std::fs::write(repo.join("f.txt"), "x").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "base"]).unwrap();

        let cp = git_checkpoint_create(&repo, "refs/codemux/checkpoints/t", "cp")
            .unwrap()
            .unwrap();

        run_git(&repo, &["checkout", "-b", "other"]).unwrap();
        let err = git_checkpoint_restore(
            &repo,
            &cp.snapshot_commit,
            &cp.head_commit,
            cp.branch.as_deref(),
            "refs/codemux/pre-restore/t",
        )
        .expect_err("restore on the wrong branch must refuse");
        assert!(err.contains("on branch 'other'"), "got: {err}");
        assert!(err.contains("'main'"), "got: {err}");
    }

    #[test]
    fn checkpoint_restore_refuses_missing_snapshot() {
        let (_dir, repo) = setup_test_repo();
        let head = run_git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let bogus = "0123456789abcdef0123456789abcdef01234567";
        let err = git_checkpoint_restore(
            &repo,
            bogus,
            &head,
            None,
            "refs/codemux/pre-restore/t",
        )
        .expect_err("missing snapshot must refuse");
        assert!(err.contains("no longer exists"), "got: {err}");
    }

    #[test]
    fn checkpoint_round_trip_works_in_a_linked_worktree() {
        // Codemux workspaces are usually linked worktrees, not the
        // main checkout: refs are SHARED with the main repo while the
        // index and HEAD are per-worktree. The snapshot must read the
        // worktree-specific index (`rev-parse --git-path index`) and
        // the restore must only rewrite the worktree's own state.
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        std::fs::write(repo.join("base.txt"), "base").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "base"]).unwrap();

        let wt_path = git_create_worktree(&repo, "cp-branch", true, None, None)
            .expect("create worktree");
        let wt = PathBuf::from(&wt_path);

        // Dirty the WORKTREE only.
        std::fs::write(wt.join("base.txt"), "wt-edit").unwrap();
        std::fs::write(wt.join("wt-untracked.txt"), "wt-new").unwrap();

        let cp = git_checkpoint_create(&wt, "refs/codemux/checkpoints/wt-1", "wt cp")
            .unwrap()
            .expect("worktree is snapshottable");
        assert_eq!(cp.branch.as_deref(), Some("cp-branch"));
        // Ref is visible from the main repo too (shared ref store).
        assert_eq!(
            run_git(&repo, &["rev-parse", "refs/codemux/checkpoints/wt-1"]).unwrap(),
            cp.snapshot_commit
        );

        // Simulated run inside the worktree.
        std::fs::write(wt.join("base.txt"), "agent").unwrap();
        std::fs::write(wt.join("agent.txt"), "artifact").unwrap();

        git_checkpoint_restore(
            &wt,
            &cp.snapshot_commit,
            &cp.head_commit,
            cp.branch.as_deref(),
            "refs/codemux/pre-restore/wt-1",
        )
        .expect("restore in worktree");

        assert_eq!(
            std::fs::read_to_string(wt.join("base.txt")).unwrap(),
            "wt-edit"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("wt-untracked.txt")).unwrap(),
            "wt-new"
        );
        assert!(!wt.join("agent.txt").exists());
        // The MAIN checkout was never touched.
        assert_eq!(
            std::fs::read_to_string(repo.join("base.txt")).unwrap(),
            "base"
        );
        let main_status = run_git_permissive(&repo, &["status", "--porcelain"]);
        assert!(main_status.is_empty(), "main checkout stays clean: {main_status}");

        // Cleanup the worktree so TempDir drop works on all platforms.
        let _ = git_remove_worktree(&wt, Some("cp-branch"), true);
    }

    #[test]
    fn checkpoint_prune_keeps_newest() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        // Three checkpoints with strictly increasing committer dates so
        // the `-committerdate` sort is deterministic.
        for (i, date) in [(1, "2024-01-01T00:00:00"), (2, "2024-01-02T00:00:00"), (3, "2024-01-03T00:00:00")] {
            std::fs::write(repo.join(format!("f{i}.txt")), format!("v{i}")).unwrap();
            let tree_msg = format!("cp {i}");
            // Force the committer date through the env-free `-c` route:
            // commit-tree honors GIT_COMMITTER_DATE, so wrap create
            // manually here via update-ref on a dated commit.
            let head = run_git(&repo, &["rev-parse", "HEAD"]).unwrap();
            let tmp_index = std::env::temp_dir().join(format!(
                "codemux-prune-test-index-{}-{i}",
                std::process::id()
            ));
            run_git_with_index(&repo, &tmp_index, &["read-tree", "HEAD"]).unwrap();
            run_git_with_index(&repo, &tmp_index, &["add", "-A"]).unwrap();
            let tree = run_git_with_index(&repo, &tmp_index, &["write-tree"]).unwrap();
            let _ = std::fs::remove_file(&tmp_index);
            let output = Command::new("git")
                .args([
                    "-c", "user.name=Test", "-c", "user.email=t@t.t",
                    "commit-tree", &tree, "-p", &head, "-m", &tree_msg,
                ])
                .env("GIT_COMMITTER_DATE", date)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(output.status.success());
            let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
            run_git(
                &repo,
                &["update-ref", &format!("refs/codemux/checkpoints/t{i}"), &commit],
            )
            .unwrap();
        }

        let pruned = git_checkpoint_prune(&repo, CHECKPOINT_REF_PREFIX, 2).unwrap();
        assert_eq!(pruned, vec!["refs/codemux/checkpoints/t1".to_string()]);
        let remaining = run_git(
            &repo,
            &["for-each-ref", "--format=%(refname)", CHECKPOINT_REF_PREFIX],
        )
        .unwrap();
        assert!(remaining.contains("t2") && remaining.contains("t3"));
        assert!(!remaining.contains("t1"));

        // Pruning when under the cap is a no-op.
        let pruned = git_checkpoint_prune(&repo, CHECKPOINT_REF_PREFIX, 20).unwrap();
        assert!(pruned.is_empty());
    }

    // ── get_commit_files tests ──

    #[test]
    fn test_get_commit_files_added() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        std::fs::write(repo.join("new.txt"), "hello").unwrap();
        std::fs::write(repo.join("other.txt"), "world").unwrap();
        run_git(&repo, &["add", "."]).unwrap();
        run_git(&repo, &["commit", "-m", "add files"]).unwrap();

        let hash = run_git(&repo, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        let files = get_commit_files(&repo, &hash).unwrap();

        assert_eq!(files.len(), 2, "should have 2 files");
        let new_file = files.iter().find(|f| f.path == "new.txt").expect("new.txt");
        assert_eq!(new_file.status, "added");
        let other_file = files.iter().find(|f| f.path == "other.txt").expect("other.txt");
        assert_eq!(other_file.status, "added");
    }

    #[test]
    fn test_get_commit_files_modified() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        std::fs::write(repo.join("file.txt"), "original").unwrap();
        run_git(&repo, &["add", "file.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add file"]).unwrap();

        std::fs::write(repo.join("file.txt"), "modified").unwrap();
        run_git(&repo, &["add", "file.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "modify file"]).unwrap();

        let hash = run_git(&repo, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        let files = get_commit_files(&repo, &hash).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "file.txt");
        assert_eq!(files[0].status, "modified");
    }

    #[test]
    fn test_get_commit_files_deleted() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        std::fs::write(repo.join("doomed.txt"), "bye").unwrap();
        run_git(&repo, &["add", "doomed.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add file"]).unwrap();

        run_git(&repo, &["rm", "doomed.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "delete file"]).unwrap();

        let hash = run_git(&repo, &["rev-parse", "HEAD"]).unwrap().trim().to_string();
        let files = get_commit_files(&repo, &hash).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "doomed.txt");
        assert_eq!(files[0].status, "deleted");
    }

    #[test]
    fn test_get_commit_files_invalid_hash() {
        let (_dir, repo) = setup_test_repo();
        let result = get_commit_files(&repo, "deadbeefdeadbeef");
        assert!(result.is_err(), "invalid hash should return error");
    }

    // -----------------------------------------------------------------------
    // ensure_git_exclude tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_ensure_git_exclude_creates_entry() {
        let dir = tempfile::tempdir().unwrap();
        let git_info = dir.path().join(".git").join("info");
        std::fs::create_dir_all(&git_info).unwrap();
        std::fs::write(git_info.join("exclude"), "*.log\n").unwrap();

        ensure_git_exclude(dir.path(), ".codemux");

        let content = std::fs::read_to_string(git_info.join("exclude")).unwrap();
        assert!(content.contains("*.log"));
        assert!(content.contains(".codemux"));
    }

    #[test]
    fn test_ensure_git_exclude_no_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let git_info = dir.path().join(".git").join("info");
        std::fs::create_dir_all(&git_info).unwrap();
        std::fs::write(git_info.join("exclude"), ".codemux\n").unwrap();

        ensure_git_exclude(dir.path(), ".codemux");
        ensure_git_exclude(dir.path(), ".codemux");

        let content = std::fs::read_to_string(git_info.join("exclude")).unwrap();
        assert_eq!(content.matches(".codemux").count(), 1);
    }

    #[test]
    fn test_ensure_git_exclude_no_git_noop() {
        let dir = tempfile::tempdir().unwrap();
        // No .git at all — should not crash or create anything
        ensure_git_exclude(dir.path(), ".codemux");
        assert!(!dir.path().join(".git").exists());
    }

    #[test]
    fn test_ensure_git_exclude_creates_info_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        // .git/ exists but no info/ subdir

        ensure_git_exclude(dir.path(), ".codemux");

        let content =
            std::fs::read_to_string(dir.path().join(".git/info/exclude")).unwrap();
        assert!(content.contains(".codemux"));
    }

    #[test]
    fn test_ensure_git_exclude_worktree() {
        let main_repo = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        // Set up main repo's .git directory structure
        let main_git = main_repo.path().join(".git");
        let wt_gitdir = main_git.join("worktrees").join("feat");
        std::fs::create_dir_all(&wt_gitdir).unwrap();
        std::fs::create_dir_all(main_git.join("info")).unwrap();

        // Worktree has a .git file pointing to the main repo
        std::fs::write(
            worktree.path().join(".git"),
            format!("gitdir: {}", wt_gitdir.display()),
        )
        .unwrap();

        ensure_git_exclude(worktree.path(), ".codemux");

        // Exclude entry should land in the main repo's .git/info/exclude
        let content =
            std::fs::read_to_string(main_git.join("info/exclude")).unwrap();
        assert!(content.contains(".codemux"));
    }

    // ---- find_remote_ref / worktree ref resolution tests ----

    #[test]
    fn test_find_remote_ref_returns_none_for_local_only() {
        let (_dir, repo) = setup_test_repo();
        run_git(&repo, &["branch", "local-only"]).expect("create branch");
        assert!(find_remote_ref(&repo, "local-only").is_none());
    }

    #[test]
    fn test_find_remote_ref_returns_origin_for_remote_branch() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();
        // Push a branch to origin
        run_git(&local, &["checkout", "-b", "feat-remote"]).expect("create branch");
        std::fs::write(local.join("feat.txt"), "content").expect("write");
        run_git(&local, &["add", "feat.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "feat"],
        ).expect("commit");
        run_git(&local, &["push", "-u", "origin", "feat-remote"]).expect("push");

        let result = find_remote_ref(&local, "feat-remote");
        assert_eq!(result, Some("origin/feat-remote".to_string()));
    }

    #[test]
    fn test_find_remote_ref_passthrough_already_prefixed() {
        let (_dir, repo) = setup_test_repo();
        // Already has origin/ prefix — return as-is (even if ref doesn't exist)
        let result = find_remote_ref(&repo, "origin/some-branch");
        assert_eq!(result, Some("origin/some-branch".to_string()));
    }

    #[test]
    fn test_find_remote_ref_branch_with_slashes() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();
        run_git(&local, &["checkout", "-b", "feature/auth/oauth"]).expect("create branch");
        std::fs::write(local.join("oauth.txt"), "content").expect("write");
        run_git(&local, &["add", "oauth.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "oauth"],
        ).expect("commit");
        run_git(&local, &["push", "-u", "origin", "feature/auth/oauth"]).expect("push");

        let result = find_remote_ref(&local, "feature/auth/oauth");
        assert_eq!(result, Some("origin/feature/auth/oauth".to_string()));
    }

    #[test]
    fn test_worktree_new_branch_uses_remote_base() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();

        // Advance origin/master by pushing from a second clone-like setup:
        // just push a new commit on master directly
        run_git(&local, &["checkout", "-b", "dev-base"]).expect("create base");
        std::fs::write(local.join("base.txt"), "base content").expect("write");
        run_git(&local, &["add", "base.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "base"],
        ).expect("commit");
        run_git(&local, &["push", "-u", "origin", "dev-base"]).expect("push");

        // Go back to default branch
        let _ = run_git(&local, &["checkout", "master"])
            .or_else(|_| run_git(&local, &["checkout", "main"]));

        // Create worktree with dev-base as base — should resolve to origin/dev-base
        let wt = git_create_worktree(&local, "from-dev", true, Some("dev-base"), None)
            .expect("create worktree");
        // The worktree should have base.txt (from origin/dev-base)
        assert!(PathBuf::from(&wt).join("base.txt").exists(), "should inherit from remote base");
        git_remove_worktree(Path::new(&wt), Some("from-dev"), true).expect("cleanup");
    }

    #[test]
    fn test_worktree_new_branch_local_only_base_works() {
        let (_dir, repo) = setup_test_repo();
        // Create a local-only base branch
        run_git(&repo, &["checkout", "-b", "local-base"]).expect("create base");
        std::fs::write(repo.join("local.txt"), "local").expect("write");
        run_git(&repo, &["add", "local.txt"]).expect("add");
        run_git(
            &repo,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "local"],
        ).expect("commit");
        let _ = run_git(&repo, &["checkout", "master"])
            .or_else(|_| run_git(&repo, &["checkout", "main"]));

        let wt = git_create_worktree(&repo, "from-local", true, Some("local-base"), None)
            .expect("create worktree");
        assert!(PathBuf::from(&wt).join("local.txt").exists(), "should inherit from local base");
        git_remove_worktree(Path::new(&wt), Some("from-local"), true).expect("cleanup");
    }

    #[test]
    fn test_worktree_open_existing_prefers_remote_ref() {
        let (_dir, local, remote) = setup_test_repo_with_remote();

        // Create branch and push it
        run_git(&local, &["checkout", "-b", "stale-branch"]).expect("create branch");
        std::fs::write(local.join("v1.txt"), "v1").expect("write v1");
        run_git(&local, &["add", "v1.txt"]).expect("add");
        run_git(
            &local,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "v1"],
        ).expect("commit v1");
        run_git(&local, &["push", "-u", "origin", "stale-branch"]).expect("push");

        // Simulate remote advancing: push a new commit from a separate clone
        let tmp = TempDir::new().expect("tmp");
        let clone2 = tmp.path().join("clone2");
        run_git(
            tmp.path(),
            &["clone", remote.to_str().unwrap(), clone2.to_str().unwrap()],
        ).expect("clone2");
        run_git(&clone2, &["checkout", "stale-branch"]).expect("checkout");
        std::fs::write(clone2.join("v2.txt"), "v2").expect("write v2");
        run_git(&clone2, &["add", "v2.txt"]).expect("add");
        run_git(
            &clone2,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "v2"],
        ).expect("commit v2");
        run_git(&clone2, &["push"]).expect("push v2");

        // Fetch in original repo so origin/stale-branch is updated but local is behind
        run_git(&local, &["fetch"]).expect("fetch");
        let _ = run_git(&local, &["checkout", "master"])
            .or_else(|_| run_git(&local, &["checkout", "main"]));

        // Open existing: should get v2.txt from origin/stale-branch
        let wt = git_create_worktree(&local, "stale-branch", false, None, None)
            .expect("create worktree");
        assert!(
            PathBuf::from(&wt).join("v2.txt").exists(),
            "worktree should have remote's v2.txt, not stale local"
        );
        git_remove_worktree(Path::new(&wt), Some("stale-branch"), true).expect("cleanup");
    }

    #[test]
    fn test_worktree_open_existing_local_only_works() {
        let (_dir, repo) = setup_test_repo();
        run_git(&repo, &["branch", "local-feat"]).expect("create branch");

        let wt = git_create_worktree(&repo, "local-feat", false, None, None)
            .expect("create worktree");
        let info = git_branch_info(Path::new(&wt)).expect("branch info");
        assert_eq!(info.branch.as_deref(), Some("local-feat"));
        git_remove_worktree(Path::new(&wt), Some("local-feat"), true).expect("cleanup");
    }

    #[test]
    fn test_worktree_pr_number_fetches_fork_branch() {
        // Simulate a fork PR: a branch that only exists under refs/pull/<n>/head
        // on the remote, not as a normal branch.
        let (_dir, local, remote) = setup_test_repo_with_remote();

        // Create a commit on the remote under refs/pull/42/head (simulating GitHub)
        let staging = _dir.path().join("staging");
        run_git(_dir.path(), &["clone", remote.to_str().unwrap(), staging.to_str().unwrap()])
            .expect("clone staging");
        std::fs::write(staging.join("pr-file.txt"), "from fork").expect("write");
        run_git(&staging, &["add", "pr-file.txt"]).expect("add");
        run_git(
            &staging,
            &["-c", "user.name=Test", "-c", "user.email=test@test.com", "commit", "-m", "fork PR"],
        ).expect("commit");
        // Push to a PR ref on the bare remote
        run_git(&staging, &["push", "origin", "HEAD:refs/pull/42/head"]).expect("push PR ref");

        // Verify: local has no branch "fork-branch" and no origin/fork-branch
        assert!(find_remote_ref(&local, "fork-branch").is_none());

        // Without pr_number, this should fail
        let _ = run_git(&local, &["checkout", "master"])
            .or_else(|_| run_git(&local, &["checkout", "main"]));
        let result = git_create_worktree(&local, "fork-branch", false, None, None);
        assert!(result.is_err(), "should fail without pr_number for unfetched branch");

        // With pr_number, it should fetch from pull/42/head and succeed
        let wt = git_create_worktree(&local, "fork-branch", false, None, Some(42))
            .expect("create worktree with pr_number");
        assert!(
            PathBuf::from(&wt).join("pr-file.txt").exists(),
            "worktree should have the fork PR's file"
        );
        git_remove_worktree(Path::new(&wt), Some("fork-branch"), true).expect("cleanup");
    }

    #[test]
    fn test_worktree_pr_number_not_needed_for_same_repo_branch() {
        // Branch exists on origin — should work with or without pr_number
        let (_dir, local, _remote) = setup_test_repo_with_remote();

        // Create and push a branch on origin
        run_git(&local, &["branch", "same-repo-feat"]).expect("create branch");
        run_git(&local, &["push", "origin", "same-repo-feat"]).expect("push branch");
        let _ = run_git(&local, &["checkout", "master"])
            .or_else(|_| run_git(&local, &["checkout", "main"]));

        let wt = git_create_worktree(&local, "same-repo-feat", false, None, None)
            .expect("should work without pr_number for origin branch");
        git_remove_worktree(Path::new(&wt), Some("same-repo-feat"), true).expect("cleanup");
    }

    // ── Merge-resolver fixes ──────────────────────────────────────────
    //
    // Regression tests for the bugs reported in the merge-resolver deep
    // dive: (1) `bot/resolve-` prefix recursion on retry, (2) conflict
    // markers slipping past `apply_resolution` because `git add` clears
    // the U flag from porcelain status. These are pure-function and
    // file-system tests — no agent CLI is invoked.

    #[test]
    fn strip_resolver_prefix_passthrough_for_normal_branches() {
        assert_eq!(strip_resolver_prefix("main"), "main");
        assert_eq!(strip_resolver_prefix("feature/foo"), "feature/foo");
        assert_eq!(strip_resolver_prefix("fix-stt-deadlock"), "fix-stt-deadlock");
        assert_eq!(strip_resolver_prefix(""), "");
    }

    #[test]
    fn strip_resolver_prefix_peels_one_layer() {
        // The exact pattern produced by create_resolver_branch.
        assert_eq!(
            strip_resolver_prefix("bot/resolve-feature-into-main-1776365511"),
            "feature"
        );
        assert_eq!(
            strip_resolver_prefix("bot/resolve-fix-stt-into-main-1776365850"),
            "fix-stt"
        );
    }

    #[test]
    fn strip_resolver_prefix_peels_nested_layers() {
        // The exact branch reported in the user's incident:
        //   bot/resolve-bot-resolve-fix-gpu-transcribe-deadlock-into-main-1776365511-into-main-1776365850
        // strip_resolver_prefix MUST recurse all the way down to the
        // original source branch `fix-gpu-transcribe-deadlock`. It
        // recognizes both `bot/resolve-` (slash, original prefix) and
        // `bot-resolve-` (dash form left by replace('/', '-')) so it can
        // peel the inner layer too.
        let nested =
            "bot/resolve-bot-resolve-fix-gpu-transcribe-deadlock-into-main-1776365511-into-main-1776365850";
        assert_eq!(
            strip_resolver_prefix(nested),
            "fix-gpu-transcribe-deadlock"
        );
    }

    #[test]
    fn strip_resolver_prefix_handles_dash_form_directly() {
        // After the outer slash form is peeled once, the body lives in
        // dash form. Verify we still peel that.
        assert_eq!(
            strip_resolver_prefix("bot-resolve-feature-into-main-1776365511"),
            "feature"
        );
    }

    #[test]
    fn strip_resolver_prefix_idempotent_when_recursing() {
        // Calling strip on its own output must converge — no infinite loops,
        // no further changes once peeled.
        let once = strip_resolver_prefix("bot/resolve-feature-into-main-1776365511");
        let twice = strip_resolver_prefix(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn strip_resolver_prefix_rejects_malformed_suffix() {
        // Looks like a resolver branch but trailing token is not numeric:
        // strip should NOT peel — return as-is.
        assert_eq!(
            strip_resolver_prefix("bot/resolve-foo-into-main-notanumber"),
            "bot/resolve-foo-into-main-notanumber"
        );
        // Missing -into- separator: not a resolver pattern, return as-is.
        assert_eq!(
            strip_resolver_prefix("bot/resolve-foo-1776365511"),
            "bot/resolve-foo-1776365511"
        );
    }

    #[test]
    fn has_conflict_markers_clean_file() {
        assert!(!has_conflict_markers("hello world\n"));
        assert!(!has_conflict_markers(""));
        assert!(!has_conflict_markers("fn main() {\n    println!(\"hi\");\n}\n"));
    }

    #[test]
    fn has_conflict_markers_detects_real_merge_output() {
        // The exact shape `git merge` writes.
        let conflicted = "\
fn foo() {
<<<<<<< HEAD
    println!(\"ours\");
=======
    println!(\"theirs\");
>>>>>>> branch
}
";
        assert!(has_conflict_markers(conflicted));
    }

    #[test]
    fn has_conflict_markers_ignores_in_string_literals_midline() {
        // The pattern only matches at start-of-line with the trailing space
        // (or end-of-line for `=======`). String literals or comments that
        // contain `<<<<<<<` mid-line should NOT trip it.
        let safe = r#"
let banner = "<<<<<<< banner inside string";
// also <<<<<<< not at start of line
println!("{}", banner);
"#;
        assert!(!has_conflict_markers(safe));
    }

    #[test]
    fn has_conflict_markers_ignores_markdown_rules() {
        // `=======` is also a Markdown setext heading underline. Our match
        // requires the line to be EXACTLY `=======` so a longer rule like
        // `========` (8 equals) for a heading doesn't trip — but a 7-equals
        // line in a real Markdown file is rare and would be a false
        // positive. We accept that as the trade-off.
        let md = "Title\n========\nBody text\n";
        assert!(!has_conflict_markers(md));
    }

    #[test]
    fn has_conflict_markers_partial_resolution() {
        // Agent removed half the markers but left a stray `>>>>>>>`.
        // This is the failure mode we explicitly want to catch.
        let half = "\
fn foo() {
    println!(\"resolved\");
>>>>>>> branch
}
";
        assert!(has_conflict_markers(half));
    }

    #[test]
    fn scan_files_for_conflict_markers_finds_dirty_files() {
        let dir = TempDir::new().expect("tmp");
        let repo = dir.path();
        std::fs::write(repo.join("clean.txt"), "all good\n").unwrap();
        std::fs::write(
            repo.join("dirty.txt"),
            "<<<<<<< HEAD\nA\n=======\nB\n>>>>>>> theirs\n",
        )
        .unwrap();
        std::fs::write(repo.join("missing.txt"), "stub").unwrap(); // exists, clean
        let hits = scan_files_for_conflict_markers(
            repo,
            &[
                "clean.txt".into(),
                "dirty.txt".into(),
                "nonexistent.txt".into(),
                "missing.txt".into(),
            ],
        );
        assert_eq!(hits, vec!["dirty.txt"]);
    }

    /// Build a real two-branch merge-conflict scenario and return
    /// (TempDir, repo_path, original_branch_name, target_branch_name).
    /// The repo has a conflict in `shared.txt`. Cwd is the original branch.
    fn setup_conflict_repo() -> (TempDir, PathBuf, String, String) {
        let (dir, repo) = setup_test_repo();
        run_git(&repo, &["-c", "user.name=Test", "-c", "user.email=t@t",
            "commit", "--allow-empty", "-m", "base"]).unwrap();
        // Pick whichever default branch exists.
        let default_branch = run_git(&repo, &["branch", "--show-current"]).unwrap();
        // Create target branch (main-side) with one edit.
        run_git(&repo, &["checkout", "-b", "target"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "TARGET version\n").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["-c", "user.name=Test", "-c", "user.email=t@t",
            "commit", "-m", "target edit"]).unwrap();
        // Source branch (feature-side) with a conflicting edit.
        run_git(&repo, &["checkout", &default_branch]).unwrap();
        run_git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "FEATURE version\n").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["-c", "user.name=Test", "-c", "user.email=t@t",
            "commit", "-m", "feature edit"]).unwrap();
        (dir, repo, "feature".to_string(), "target".to_string())
    }

    #[test]
    fn create_resolver_branch_strips_prior_prefix_on_retry() {
        let (_dir, repo, source, target) = setup_conflict_repo();
        // First invocation: source is `feature` → temp branch
        // `bot/resolve-feature-into-target-<ts1>`.
        let info1 = create_resolver_branch(&repo, &target).expect("first resolver");
        assert!(info1.temp_branch.starts_with("bot/resolve-feature-into-target-"));
        assert_eq!(info1.original_branch, source);
        // Abort the merge so we can simulate "Try Again" while still on
        // the temp branch.
        let _ = run_git(&repo, &["merge", "--abort"]);

        // Sleep 1s to guarantee the next timestamp differs (otherwise
        // git refuses to create a duplicate branch).
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Second invocation: cwd is now the temp branch. Without the
        // strip_resolver_prefix fix, this would produce
        // `bot/resolve-bot-resolve-feature-into-target-<ts1>-into-target-<ts2>`.
        let info2 = create_resolver_branch(&repo, &target).expect("retry resolver");
        assert!(
            info2.temp_branch.starts_with("bot/resolve-feature-into-target-"),
            "retry should peel prior resolver prefix, got {}",
            info2.temp_branch
        );
        assert!(
            !info2.temp_branch.contains("bot-resolve-"),
            "retry must not nest the resolver prefix, got {}",
            info2.temp_branch
        );
    }

    #[test]
    fn apply_resolution_rejects_unresolved_porcelain_status() {
        let (_dir, repo, _source, target) = setup_conflict_repo();
        let info = create_resolver_branch(&repo, &target).expect("resolver branch");
        // Don't touch the file — conflicts are still UU in the index.
        let err = apply_resolution(&repo, &info.temp_branch, &info.original_branch, "msg")
            .expect_err("should reject unresolved");
        assert!(
            err.contains("unresolved conflict") || err.contains("unmerged"),
            "expected unresolved-conflict error, got: {err}"
        );
    }

    #[test]
    fn apply_resolution_rejects_markers_even_after_git_add() {
        let (_dir, repo, _source, target) = setup_conflict_repo();
        let info = create_resolver_branch(&repo, &target).expect("resolver branch");
        // Simulate the bug: agent writes a "resolution" that still has
        // markers, then runs `git add` — clearing the U flag from
        // porcelain status. The file-content scan must still catch it.
        let bad = "\
<<<<<<< HEAD
FEATURE version
=======
TARGET version
>>>>>>> target
";
        std::fs::write(repo.join("shared.txt"), bad).unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();

        let err = apply_resolution(&repo, &info.temp_branch, &info.original_branch, "msg")
            .expect_err("should reject conflict markers in staged content");
        assert!(
            err.contains("conflict markers") || err.contains("unmerged") || err.contains("unresolved"),
            "expected marker-content error, got: {err}"
        );
    }

    #[test]
    fn apply_resolution_succeeds_when_conflict_truly_resolved() {
        let (_dir, repo, source, target) = setup_conflict_repo();
        let info = create_resolver_branch(&repo, &target).expect("resolver branch");
        // Real resolution: pick one side, add, and let apply commit + merge.
        std::fs::write(repo.join("shared.txt"), "RESOLVED version\n").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();

        // Need git committer identity for the commit step.
        run_git(&repo, &["config", "user.name", "Test"]).unwrap();
        run_git(&repo, &["config", "user.email", "t@t"]).unwrap();

        apply_resolution(&repo, &info.temp_branch, &info.original_branch, "resolved")
            .expect("apply should succeed");

        // After apply we should be back on the original branch with the
        // merge committed.
        let cur = run_git(&repo, &["branch", "--show-current"]).unwrap();
        assert_eq!(cur, source);
        let content = std::fs::read_to_string(repo.join("shared.txt")).unwrap();
        assert_eq!(content, "RESOLVED version\n");
    }

    // ---- checkout_default_branch ----

    /// Clean feature-branch repo → checkout_default_branch switches HEAD
    /// back to the default and returns the branch name.
    #[test]
    fn checkout_default_branch_switches_from_feature_to_default() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let default = main_branch(&repo).to_string();

        run_git(&repo, &["checkout", "-b", "feature/x"]).expect("create feature");
        let current = run_git_permissive(&repo, &["branch", "--show-current"]);
        assert_eq!(current, "feature/x");

        let result = checkout_default_branch(&repo).expect("checkout should succeed");
        assert_eq!(result, Some(default.clone()));

        let after = run_git_permissive(&repo, &["branch", "--show-current"]);
        assert_eq!(after, default, "HEAD should be on default branch");
    }

    /// Already on default → returns Ok(Some(default)) without running a
    /// no-op checkout. Verified by checking the reflog is unchanged.
    #[test]
    fn checkout_default_branch_noop_when_already_on_default() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let default = main_branch(&repo).to_string();
        let current = run_git_permissive(&repo, &["branch", "--show-current"]);
        assert_eq!(current, default, "precondition: should start on default");

        // Snapshot HEAD reflog count before the call.
        let reflog_before = run_git_permissive(&repo, &["reflog", "HEAD"])
            .lines()
            .count();

        let result = checkout_default_branch(&repo).expect("checkout should succeed");
        assert_eq!(result, Some(default));

        let reflog_after = run_git_permissive(&repo, &["reflog", "HEAD"])
            .lines()
            .count();
        assert_eq!(
            reflog_before, reflog_after,
            "no-op path must not add a reflog entry for a redundant checkout"
        );
    }

    /// Repo with no commits / no branches / no remote → find_default_branch
    /// returns None → helper returns Ok(None) instead of Err or panic.
    #[test]
    fn checkout_default_branch_returns_none_when_no_default_exists() {
        let dir = TempDir::new().expect("tmp");
        let repo = dir.path().to_path_buf();
        run_git(&repo, &["init"]).expect("init");
        // No initial commit, no origin, no main/master branch.

        let result = checkout_default_branch(&repo).expect("should not Err on unborn repo");
        assert!(result.is_none(), "expected None, got {:?}", result);
    }

    /// Uncommitted change that conflicts with a tracked file on the default
    /// branch → git refuses → Err(stderr) bubbles up, current branch is
    /// unchanged.
    #[test]
    fn checkout_default_branch_errs_on_dirty_conflict_and_preserves_branch() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let default = main_branch(&repo).to_string();

        // Create tracked file on default.
        std::fs::write(repo.join("shared.txt"), "default content\n").unwrap();
        run_git(&repo, &["add", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "add shared on default"]).unwrap();

        // Feature branch with no shared.txt yet.
        run_git(&repo, &["checkout", "-b", "feature/x"]).unwrap();
        run_git(&repo, &["rm", "shared.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "remove shared on feature"]).unwrap();

        // Now create an *untracked* file at the same path — checkout would
        // have to overwrite it, which git refuses.
        std::fs::write(repo.join("shared.txt"), "local dirty content\n").unwrap();

        let err = checkout_default_branch(&repo).expect_err("should refuse to overwrite");
        assert!(
            err.contains("would be overwritten") || err.contains("Aborting"),
            "expected git's 'would be overwritten' message, got: {err}"
        );

        let after = run_git_permissive(&repo, &["branch", "--show-current"]);
        assert_eq!(after, "feature/x", "branch must be unchanged after failed checkout");
        let _ = default; // silence unused binding
    }

    /// `origin/HEAD` pointing at `develop` → we switch to `develop`, not
    /// main. This is the "custom default name" case.
    #[test]
    fn checkout_default_branch_uses_origin_head_for_non_standard_default() {
        let (_dir, local, _remote) = setup_test_repo_with_remote();
        git_config(&local);

        // Create a `develop` branch, push it, then point origin/HEAD at it.
        run_git(&local, &["checkout", "-b", "develop"]).expect("create develop");
        std::fs::write(local.join("d.txt"), "dev").unwrap();
        run_git(&local, &["add", "d.txt"]).unwrap();
        run_git(&local, &["commit", "-m", "dev commit"]).unwrap();
        run_git(&local, &["push", "-u", "origin", "develop"]).expect("push develop");

        // Force origin/HEAD → refs/remotes/origin/develop.
        run_git(
            &local,
            &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/develop"],
        )
        .expect("set origin/HEAD to develop");

        // Switch to a feature branch so we're not already on develop.
        run_git(&local, &["checkout", "-b", "feature/custom-default"]).expect("feature");

        let result = checkout_default_branch(&local).expect("checkout should succeed");
        assert_eq!(
            result,
            Some("develop".to_string()),
            "should resolve default via origin/HEAD, not hardcoded main/master"
        );

        let after = run_git_permissive(&local, &["branch", "--show-current"]);
        assert_eq!(after, "develop");
    }

    /// Rebase in progress → git refuses checkout with a specific error. Same
    /// Err-and-preserve-branch semantics as the dirty-conflict case; adding
    /// this test guards against a regression where we'd accidentally swallow
    /// this class of error (e.g. if someone tried to force the checkout).
    #[test]
    fn checkout_default_branch_errs_during_rebase() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);
        let default = main_branch(&repo).to_string();

        // Two divergent commits to rebase.
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        run_git(&repo, &["add", "a.txt"]).unwrap();
        run_git(&repo, &["commit", "-m", "a on default"]).unwrap();

        run_git(&repo, &["checkout", "-b", "feature/x"]).unwrap();
        std::fs::write(repo.join("a.txt"), "a-feature\n").unwrap();
        run_git(&repo, &["commit", "-am", "a on feature"]).unwrap();

        run_git(&repo, &["checkout", &default]).unwrap();
        std::fs::write(repo.join("a.txt"), "a-default\n").unwrap();
        run_git(&repo, &["commit", "-am", "a on default v2"]).unwrap();
        run_git(&repo, &["checkout", "feature/x"]).unwrap();

        // Start an interactive-style rebase that will stop on conflicts.
        let (_out, _err, _ok) = run_git_full(&repo, &["rebase", &default]).unwrap();
        // Confirm a rebase is actually in progress (regardless of conflict state).
        let rebase_in_progress = repo.join(".git/rebase-apply").exists()
            || repo.join(".git/rebase-merge").exists();
        assert!(
            rebase_in_progress,
            "test precondition failed: rebase didn't start"
        );

        // checkout should fail; branch stays on the rebase head (feature/x
        // or a detached HEAD, depending on git version — what matters is
        // that it's NOT the default).
        let result = checkout_default_branch(&repo);
        assert!(
            result.is_err(),
            "expected Err while rebase is in progress, got {:?}",
            result
        );

        // Clean up so the TempDir drop doesn't hit a lingering rebase.
        let _ = run_git(&repo, &["rebase", "--abort"]);
    }

    /// No `origin/HEAD` symref, no remote at all, but `main` exists locally
    /// → helper falls through to the hardcoded `main` / `master` check and
    /// switches to `main`. Guards the fallback path that `find_default_branch`
    /// uses when `symbolic-ref refs/remotes/origin/HEAD` returns nothing.
    #[test]
    fn checkout_default_branch_falls_back_to_main_without_origin_head() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // If the test harness landed on `master` instead of `main`, rename
        // so we can assert specifically on the `main` fallback branch.
        let current = run_git_permissive(&repo, &["branch", "--show-current"]);
        if current == "master" {
            run_git(&repo, &["branch", "-m", "master", "main"]).expect("rename to main");
        }

        // Verify precondition: no origin/HEAD, no remote refs.
        let origin_head = run_git(&repo, &["symbolic-ref", "refs/remotes/origin/HEAD"]);
        assert!(origin_head.is_err(), "precondition: origin/HEAD shouldn't exist");

        run_git(&repo, &["checkout", "-b", "feature/y"]).unwrap();
        let result = checkout_default_branch(&repo).expect("should fall back to main");
        assert_eq!(result, Some("main".to_string()), "expected fallback to 'main'");
        let after = run_git_permissive(&repo, &["branch", "--show-current"]);
        assert_eq!(after, "main");
    }

    /// No `origin/HEAD`, no `main`, but `master` exists → falls through to
    /// the second fallback candidate. Paired with the previous test to
    /// ensure both sides of the hardcoded fallback list are covered.
    #[test]
    fn checkout_default_branch_falls_back_to_master_when_no_main() {
        let (_dir, repo) = setup_test_repo();
        git_config(&repo);

        // Normalize default to `master`: rename `main` if that's what init produced.
        let current = run_git_permissive(&repo, &["branch", "--show-current"]);
        if current == "main" {
            run_git(&repo, &["branch", "-m", "main", "master"]).expect("rename to master");
        }
        // Ensure `main` does NOT exist so the first fallback candidate misses.
        let main_exists = run_git(&repo, &["rev-parse", "--verify", "main"]).is_ok();
        assert!(!main_exists, "precondition: 'main' must not exist");

        run_git(&repo, &["checkout", "-b", "feature/z"]).unwrap();
        let result = checkout_default_branch(&repo).expect("should fall back to master");
        assert_eq!(result, Some("master".to_string()), "expected fallback to 'master'");
        let after = run_git_permissive(&repo, &["branch", "--show-current"]);
        assert_eq!(after, "master");
    }
}

#[cfg(test)]
mod worktree_guard_tests {
    //! Contract coverage for `ensure_worktree_removable` — the shared
    //! dirty/unpushed guard behind `git_remove_worktree` AND the
    //! close-workspace pre-flight — plus `git_branch_exists`, the
    //! unarchive pre-flight's local+remote branch check.

    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &Path) {
        run(dir, &["init", "--initial-branch=main"]);
        run(dir, &["config", "user.email", "test@example.com"]);
        run(dir, &["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-m", "init"]);
    }

    /// CONTRACT: the frontend's force-escalation detects a guard
    /// refusal by regex-matching /use force/i on the error message, so
    /// the "Use force to override." suffix is load-bearing. This test
    /// pins the wording; changing it silently breaks the escalation UI.
    #[test]
    fn dirty_worktree_error_carries_use_force_contract_suffix() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        std::fs::write(repo.join("dirty.txt"), "uncommitted").unwrap();

        let err = ensure_worktree_removable(&repo)
            .expect_err("a dirty worktree must refuse removal");
        assert!(
            err.contains("uncommitted change(s)"),
            "unexpected guard message: {err}"
        );
        assert!(
            err.contains("Use force to override."),
            "guard error lost the frontend's /use force/i contract suffix: {err}"
        );
    }

    #[test]
    fn clean_worktree_is_removable() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);

        assert_eq!(ensure_worktree_removable(&repo), Ok(()));
    }

    /// `git_branch_exists` must see local branches, branches on ANY
    /// remote by shortname (the old list-scan false-negatived on
    /// non-origin remotes, whose entries come back qualified like
    /// "upstream/feature/x"), and report absence via exit status.
    #[test]
    fn branch_exists_matches_local_and_any_remote_shortname() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        run(&repo, &["branch", "feature/local-branch"]);
        // Simulate a branch that exists ONLY on a non-origin remote.
        run(
            &repo,
            &["update-ref", "refs/remotes/upstream/feature/remote-only", "HEAD"],
        );

        assert!(git_branch_exists(&repo, "main"));
        assert!(git_branch_exists(&repo, "feature/local-branch"));
        assert!(
            git_branch_exists(&repo, "feature/remote-only"),
            "shortname must match refs/remotes/<any-remote>/<branch>"
        );
        assert!(!git_branch_exists(&repo, "feature/nope"));
    }
}
