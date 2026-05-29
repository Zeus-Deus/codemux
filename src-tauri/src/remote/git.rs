//! Minimal git worktree helper for the headless daemon.
//!
//! The desktop has a much richer `crate::git` (PR fetches, remote-ref
//! resolution, dirty/unpushed guards). The headless daemon only needs
//! the one operation the `worktree_create` MCP tool requires: take a
//! repo + branch and produce a checked-out worktree under the same
//! `~/.codemux/worktrees/<repo>/<branch>` convention the desktop uses,
//! so an agent on a remote host gets the SAME atomic "fork a branch and
//! work in it" affordance it has on the desktop.
//!
//! Deliberately narrow: no PR-number fetch, no upstream tracking. A
//! remote host's projects are frequently local-only (no git remote), so
//! the happy path is "create a new branch off `base` and check it out."

use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of a successful worktree creation.
#[derive(Debug, Clone)]
pub struct CreatedWorktree {
    /// Absolute path to the new worktree on disk.
    pub worktree_path: PathBuf,
    /// Absolute path to the repo root the worktree belongs to.
    pub repo_root: PathBuf,
    /// The branch the worktree is checked out on.
    pub branch: String,
}

/// Run `git -C <dir> <args>`, returning trimmed stdout on success or a
/// human-readable error (including git's stderr) on failure.
fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn git: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Resolve the canonical git root of `repo_path` (follows into a
/// worktree's parent). `None` when the path is not a git repository.
pub fn git_root(repo_path: &Path) -> Option<PathBuf> {
    run_git(repo_path, &["rev-parse", "--show-toplevel"])
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// Sanitise a branch name into a single safe path segment. Mirrors the
/// desktop's worktree-path sanitisation: ASCII-alphanumeric + `_ - .`
/// survive, everything else (including `/`) becomes `-`.
pub fn sanitize_branch_for_path(branch: &str) -> String {
    let s: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "branch".to_string()
    } else {
        s
    }
}

/// The conventional worktree path for the headless daemon, matching the
/// desktop: `<home>/.codemux/worktrees/<repo>/<branch>`. `home` is
/// injected so this is unit-testable without touching the real HOME.
pub fn conventional_worktree_path(home: &Path, repo_name: &str, branch: &str) -> PathBuf {
    home.join(".codemux")
        .join("worktrees")
        .join(repo_name)
        .join(sanitize_branch_for_path(branch))
}

/// Returns true iff `git worktree list --porcelain` reports an entry at
/// `target` attached to `refs/heads/<branch>` — i.e. the worktree we'd
/// create already exists for the same branch, so we can reuse it instead
/// of letting `git worktree add` fail with "already exists".
fn existing_worktree_matches(repo_root: &Path, target: &Path, branch: &str) -> bool {
    let Ok(output) = run_git(repo_root, &["worktree", "list", "--porcelain"]) else {
        return false;
    };
    let expected = format!("refs/heads/{branch}");
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    let mut matched = false;
    let mut eval = |p: &Option<PathBuf>, b: &Option<String>| {
        if p.as_deref() == Some(target) && b.as_deref() == Some(expected.as_str()) {
            matched = true;
        }
    };
    for raw in output.lines() {
        let line = raw.trim();
        if line.is_empty() {
            eval(&cur_path, &cur_branch);
            cur_path = None;
            cur_branch = None;
        } else if let Some(rest) = line.strip_prefix("worktree ") {
            cur_path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.to_string());
        }
    }
    eval(&cur_path, &cur_branch);
    matched
}

/// Create (or reuse) a git worktree for `branch` off `repo_path`.
///
/// - `new_branch = true`: create a new branch `branch` based on `base`
///   (defaults to the repo's current HEAD when `base` is None) and check
///   it out in a fresh worktree.
/// - `new_branch = false`: attach a worktree to an existing local
///   `branch`.
///
/// The worktree lands at `<home>/.codemux/worktrees/<repo>/<branch>`.
/// Idempotent: if that exact path is already a registered worktree for
/// the same branch, it's returned as-is rather than erroring.
pub fn create_worktree(
    home: &Path,
    repo_path: &Path,
    branch: &str,
    new_branch: bool,
    base: Option<&str>,
) -> Result<CreatedWorktree, String> {
    if branch.trim().is_empty() {
        return Err("branch is required".into());
    }
    let repo_root = git_root(repo_path)
        .ok_or_else(|| format!("not a git repository: {}", repo_path.display()))?;
    let repo_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());

    let worktree_path = conventional_worktree_path(home, &repo_name, branch);
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create worktree parent dir: {e}"))?;
    }
    let path_str = worktree_path.to_string_lossy().to_string();

    // Reuse an already-registered worktree at the same path+branch.
    if worktree_path.exists() && existing_worktree_matches(&repo_root, &worktree_path, branch) {
        return Ok(CreatedWorktree {
            worktree_path,
            repo_root,
            branch: branch.to_string(),
        });
    }

    // Prune stale worktree registrations before adding. Without this, a
    // branch still "claimed" by a registration whose directory was
    // removed (manually, or by a half-finished teardown) makes
    // `git worktree add` fail with "branch is already used by worktree at
    // <missing-path>". This mirrors Superset's host-service, which prunes
    // before every add for exactly this reason. Permissive: a clean repo
    // with no stale entries is a no-op.
    let _ = run_git(&repo_root, &["worktree", "prune"]);

    if new_branch {
        let mut args = vec!["worktree", "add", "-b", branch, path_str.as_str()];
        if let Some(b) = base {
            if !b.trim().is_empty() {
                args.push(b);
            }
        }
        run_git(&repo_root, &args)?;
    } else {
        run_git(&repo_root, &["worktree", "add", path_str.as_str(), branch])?;
    }

    Ok(CreatedWorktree {
        worktree_path,
        repo_root,
        branch: branch.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Initialise a real git repo with one commit on `main` so worktree
    /// operations have a base to branch from.
    fn init_repo(dir: &Path) {
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git spawn");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);
    }

    #[test]
    fn sanitize_branch_replaces_slashes_and_trims() {
        assert_eq!(sanitize_branch_for_path("feat/login-bug"), "feat-login-bug");
        assert_eq!(sanitize_branch_for_path("--weird--"), "weird");
        assert_eq!(sanitize_branch_for_path(""), "branch");
        assert_eq!(sanitize_branch_for_path("a_b.c-1"), "a_b.c-1");
    }

    #[test]
    fn conventional_path_matches_desktop_layout() {
        let home = Path::new("/home/x");
        assert_eq!(
            conventional_worktree_path(home, "passpage", "ui/polish"),
            Path::new("/home/x/.codemux/worktrees/passpage/ui-polish")
        );
    }

    #[test]
    fn git_root_none_for_non_repo() {
        let dir = TempDir::new().unwrap();
        assert!(git_root(dir.path()).is_none());
    }

    #[test]
    fn create_new_branch_worktree_and_reuse_is_idempotent() {
        let repo = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        init_repo(repo.path());

        let created = create_worktree(home.path(), repo.path(), "feature-x", true, Some("main"))
            .expect("create worktree");
        assert!(created.worktree_path.exists());
        assert!(created.worktree_path.join("README.md").exists(), "worktree is populated");
        assert!(created.worktree_path.join(".git").is_file(), "linked worktree gitfile");
        assert_eq!(created.branch, "feature-x");
        assert_eq!(created.repo_root, git_root(repo.path()).unwrap());

        // Re-asking for the same branch reuses the path instead of erroring.
        let again = create_worktree(home.path(), repo.path(), "feature-x", true, Some("main"))
            .expect("reuse worktree");
        assert_eq!(again.worktree_path, created.worktree_path);
    }

    #[test]
    fn create_worktree_rejects_non_repo() {
        let not_repo = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let err = create_worktree(home.path(), not_repo.path(), "x", true, None).unwrap_err();
        assert!(err.contains("not a git repository"), "got: {err}");
    }

    #[test]
    fn create_worktree_requires_branch() {
        let repo = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        init_repo(repo.path());
        let err = create_worktree(home.path(), repo.path(), "  ", true, None).unwrap_err();
        assert!(err.contains("branch is required"), "got: {err}");
    }
}
