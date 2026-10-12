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
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Result of a successful worktree creation.
#[derive(Debug, Clone)]
pub struct CreatedWorktree {
    /// Absolute path to the new worktree on disk.
    pub worktree_path: PathBuf,
    /// Absolute path to the repo root the worktree belongs to.
    pub repo_root: PathBuf,
    /// The branch the worktree is checked out on.
    pub branch: String,
    /// False when an existing worktree for the same branch was reused.
    pub created: bool,
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

/// Wall-clock cap for the best-effort base fetch in
/// [`fetch_origin_branch`]. Mirrors the desktop's timeout.
const BASE_FETCH_TIMEOUT_SECS: u64 = 10;

/// Best-effort, scoped `git fetch origin <branch>` so
/// `refs/remotes/origin/<branch>` reflects the true remote tip before
/// [`create_worktree`] resolves it as the base of a new branch. Mirrors
/// the desktop's `crate::git::fetch_origin_branch` — same scoping, same
/// graceful degradation:
/// - Skipped when there is no `origin` remote (remote-host projects are
///   frequently local-only) or for absolute `refs/...` bases.
/// - `origin/<b>` is normalised to `<b>`.
/// - `GIT_TERMINAL_PROMPT=0` so git never hangs on a credential prompt.
/// - Killed after [`BASE_FETCH_TIMEOUT_SECS`] so a slow or unreachable
///   remote can't stall worktree creation.
/// - All failures are swallowed; callers fall back to local refs.
fn fetch_origin_branch(repo_root: &Path, branch: &str) -> bool {
    let branch = match branch.strip_prefix("origin/") {
        Some(rest) => rest,
        None if branch.starts_with("refs/") => return false,
        None => branch,
    };
    if branch.is_empty() {
        return false;
    }
    let has_origin = run_git(repo_root, &["remote"])
        .map(|out| out.lines().any(|r| r.trim() == "origin"))
        .unwrap_or(false);
    if !has_origin {
        return false;
    }
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["fetch", "--quiet", "--no-tags", "origin", branch])
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
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Resolve `branch` to its `origin/<branch>` remote-tracking ref when one
/// exists locally. Mirrors the desktop's `find_remote_ref`: inputs that
/// are already remote (`origin/...`) or absolute (`refs/...`) pass
/// through unchanged; otherwise `None` when origin has no such branch.
fn find_remote_ref(repo_root: &Path, branch: &str) -> Option<String> {
    if branch.starts_with("origin/") || branch.starts_with("refs/") {
        return Some(branch.to_string());
    }
    let remote_ref = format!("origin/{branch}");
    run_git(
        repo_root,
        &["rev-parse", "--verify", &format!("refs/remotes/{remote_ref}")],
    )
    .ok()
    .map(|_| remote_ref)
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

/// True when `git worktree list` reports a worktree registered at
/// `target` for ANY branch — i.e. git still tracks this directory, so
/// reclaiming it would corrupt that worktree.
fn path_is_registered_worktree(repo_root: &Path, target: &Path) -> bool {
    let Ok(output) = run_git(repo_root, &["worktree", "list", "--porcelain"]) else {
        return false;
    };
    output.lines().any(|line| {
        line.trim()
            .strip_prefix("worktree ")
            .map(|p| Path::new(p) == target)
            .unwrap_or(false)
    })
}

/// Decide whether the directory already at a would-be worktree path is a
/// safe-to-remove leftover: an orphaned Codemux worktree dir whose git
/// registration is gone, holding nothing but Codemux's own metadata.
/// Returns true ONLY when deleting it can't lose user data or corrupt a
/// live worktree. Mirrors the desktop's `git_create_worktree` policy so a
/// remote-host agent reclaiming a stale issue-branch dir behaves the same.
fn is_reclaimable_orphan_worktree(repo_root: &Path, path: &Path) -> bool {
    if path_is_registered_worktree(repo_root, path) {
        return false;
    }
    if path.join(".git").exists() {
        return false;
    }
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

/// Create (or reuse) a git worktree for `branch` off `repo_path`.
///
/// - `new_branch = true`: create a new branch `branch` based on `base`
///   (defaults to the repo's current HEAD when `base` is None) and check
///   it out in a fresh worktree. When the repo has an `origin` remote,
///   `base` is first fetched from it (best-effort, scoped, time-capped)
///   and resolved to `origin/<base>` so the new branch starts from the
///   latest remote commit; offline / local-only repos fall back to the
///   local ref.
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
            created: false,
        });
    }

    // Path exists but isn't our registered worktree. Reclaim it when it's
    // a safe orphan — a leftover Codemux dir whose registration is gone —
    // so `worktree add` doesn't die with `fatal: '<path>' already exists`.
    // `worktree prune` below only drops registrations whose dir is MISSING,
    // so it can't clear an on-disk orphan; this can.
    if worktree_path.exists() && is_reclaimable_orphan_worktree(&repo_root, &worktree_path) {
        std::fs::remove_dir_all(&worktree_path).map_err(|e| {
            format!("failed to reclaim leftover worktree dir '{}': {e}", worktree_path.display())
        })?;
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
        // Resolve base: prefer origin/<base> so new branches start from
        // the latest remote commit, not a potentially stale local ref —
        // same policy as the desktop's `git_create_worktree`. The scoped
        // fetch freshens `refs/remotes/origin/<base>` first; offline or
        // local-only repos fall back to whatever ref already exists
        // (stale origin/<base>, else local <base>).
        let resolved_base = base
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(|b| {
                fetch_origin_branch(&repo_root, b);
                find_remote_ref(&repo_root, b).unwrap_or_else(|| b.to_string())
            });
        let mut args = vec!["worktree", "add", "-b", branch, path_str.as_str()];
        if let Some(ref b) = resolved_base {
            args.push(b.as_str());
        }
        run_git(&repo_root, &args)?;
    } else {
        run_git(&repo_root, &["worktree", "add", path_str.as_str(), branch])?;
    }

    Ok(CreatedWorktree {
        worktree_path,
        repo_root,
        branch: branch.to_string(),
        created: true,
    })
}

/// True when `branch` exists locally or on `origin`. Freshens
/// `origin/<branch>` first (best-effort, time-capped) so a branch pushed
/// from another device is found, and `git worktree add <path> <branch>`
/// can then create a local branch tracking it.
pub fn branch_exists(repo_root: &Path, branch: &str) -> bool {
    let local = format!("refs/heads/{branch}");
    if run_git(repo_root, &["rev-parse", "--verify", "--quiet", &local]).is_ok() {
        return true;
    }
    fetch_origin_branch(repo_root, branch);
    let remote = format!("refs/remotes/origin/{branch}");
    run_git(repo_root, &["rev-parse", "--verify", "--quiet", &remote]).is_ok()
}

/// `git check-ref-format --branch`: whether git accepts `branch` as a new
/// branch name.
pub fn is_valid_branch_name(repo_root: &Path, branch: &str) -> bool {
    !branch.starts_with('-') && run_git(repo_root, &["check-ref-format", "--branch", branch]).is_ok()
}

/// Wall-clock cap for cloning a project onto this host, https retry
/// included. Generous on purpose: a first clone of a large repo over a
/// slow link takes minutes. The desktop's wait on `project ensure` is
/// sized from it.
pub const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

/// Cap for `git remote set-head origin --auto`, one round trip to origin.
/// `project ensure` runs it after a clone, so the desktop's wait adds it.
pub const ORIGIN_HEAD_TIMEOUT: Duration = Duration::from_secs(20);

/// `git clone -- <url> <target>` for a caller with nobody at the keyboard:
/// credential, passphrase and host-key prompts fail fast instead of
/// hanging, and the clone is killed after `timeout`. Errors carry the tail
/// of git's stderr on one line.
pub fn clone_repo(url: &str, target: &Path, timeout: Duration) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.args(["clone", "--quiet", "--"]).arg(url).arg(target);
    if let Some(parent) = target.parent() {
        cmd.current_dir(parent);
    }
    non_interactive(&mut cmd, target.parent());
    run_capped(cmd, timeout).map(|_| ())
}

/// Point `refs/remotes/origin/HEAD` at the remote's default branch when it
/// isn't set yet. A fresh clone sets it; an rsynced or hand-made checkout
/// may not, and default-branch resolution reads it first. Best-effort.
pub fn ensure_origin_head(repo_root: &Path) {
    if run_git(repo_root, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"]).is_ok() {
        return;
    }
    let has_origin = run_git(repo_root, &["remote"])
        .map(|out| out.lines().any(|r| r.trim() == "origin"))
        .unwrap_or(false);
    if !has_origin {
        return;
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo_root)
        .args(["remote", "set-head", "origin", "--auto"]);
    non_interactive(&mut cmd, Some(repo_root));
    let _ = run_capped(cmd, ORIGIN_HEAD_TIMEOUT);
}

/// Never prompt. Over ssh that means BatchMode, and accepting a forge's
/// host key on first contact (a fresh device has never seen github.com).
/// A user's own ssh command (env, or `core.sshCommand` as seen from
/// `config_dir`) wins, since `GIT_SSH_COMMAND` would override it.
fn non_interactive(cmd: &mut Command, config_dir: Option<&Path>) {
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let user_ssh = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || std::env::var_os("GIT_SSH").is_some()
        || {
            let mut probe = Command::new("git");
            if let Some(dir) = config_dir {
                probe.current_dir(dir);
            }
            probe
                .args(["config", "--get", "core.sshCommand"])
                .output()
                .map(|out| out.status.success() && !out.stdout.trim_ascii().is_empty())
                .unwrap_or(false)
        };
    if !user_ssh {
        cmd.env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new",
        );
    }
}

/// Run `cmd` to completion, or kill it (and anything it spawned, such as
/// ssh) after `timeout`. Returns trimmed stdout, or the tail of stderr.
fn run_capped(mut cmd: Command, timeout: Duration) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;

    // Own process group, so a timeout can take down git's helpers too.
    cmd.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("couldn't run git: {e}"))?;

    // Drain both pipes on threads: a chatty git must not block on a full
    // pipe while we poll for its exit.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                // SAFETY: plain syscall; the group id is the child's pid
                // because of `process_group(0)` above.
                unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) };
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    match status {
        Some(status) if status.success() => Ok(stdout.trim().to_string()),
        Some(status) => Err(stderr_tail(&stderr).unwrap_or_else(|| format!("git exited with {status}"))),
        None => Err(format!("git timed out after {}s", timeout.as_secs())),
    }
}

/// The last few lines of `stderr` on one line, for error messages that
/// must stay single-line. `None` when stderr is blank.
pub fn stderr_tail(stderr: &str) -> Option<String> {
    const MAX_LINES: usize = 3;
    const MAX_CHARS: usize = 400;
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let tail = lines[lines.len().saturating_sub(MAX_LINES)..].join(" ");
    let count = tail.chars().count();
    if count <= MAX_CHARS {
        return Some(tail);
    }
    Some(format!("…{}", tail.chars().skip(count - MAX_CHARS).collect::<String>()))
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
        assert!(created.created, "first call creates the worktree");
        assert!(!again.created, "second call reports the reuse");
    }

    #[test]
    fn create_worktree_reclaims_orphan_leftover_dir() {
        // Daemon parity with the desktop fix: a worktree dir left on disk
        // after its git registration was pruned (holding only a stray
        // `.codemux/`) must be reclaimed, not error with "already exists".
        let repo = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        init_repo(repo.path());

        // The path create_worktree will derive for this branch.
        let repo_name = git_root(repo.path()).unwrap();
        let repo_name = repo_name.file_name().unwrap().to_string_lossy();
        let orphan = conventional_worktree_path(home.path(), &repo_name, "feature-orphan");
        std::fs::create_dir_all(orphan.join(".codemux")).unwrap();
        std::fs::write(orphan.join(".codemux").join("index.json"), "{}").unwrap();
        assert!(orphan.exists() && !orphan.join(".git").exists(), "orphan precondition");

        let created =
            create_worktree(home.path(), repo.path(), "feature-orphan", true, Some("main"))
                .expect("must reclaim orphan, not error");
        assert_eq!(created.worktree_path, orphan);
        assert!(created.worktree_path.join(".git").is_file(), "now a real worktree");
        assert!(created.worktree_path.join("README.md").exists(), "populated checkout");
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

    /// Stale-clone scenario (issue #76, daemon parity): bare "origin", a
    /// `local` clone whose `origin/main` is one commit behind, and the
    /// true remote tip sha pushed by a second clone.
    fn setup_stale_clone(root: &Path) -> (PathBuf, String) {
        let seed = root.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        init_repo(&seed);

        let bare = root.join("origin.git");
        run_git(root, &["clone", "--bare", seed.to_str().unwrap(), bare.to_str().unwrap()])
            .expect("clone bare");
        let local = root.join("local");
        run_git(root, &["clone", bare.to_str().unwrap(), local.to_str().unwrap()])
            .expect("clone local");
        let publisher = root.join("publisher");
        run_git(root, &["clone", bare.to_str().unwrap(), publisher.to_str().unwrap()])
            .expect("clone publisher");
        run_git(
            &publisher,
            &["-c", "user.name=Test", "-c", "user.email=test@example.com",
              "commit", "--allow-empty", "-m", "remote-only"],
        )
        .expect("publisher commit");
        run_git(&publisher, &["push", "origin", "main"]).expect("publisher push");

        let remote_tip = run_git(&bare, &["rev-parse", "main"]).expect("bare tip");
        let stale = run_git(&local, &["rev-parse", "refs/remotes/origin/main"]).expect("stale");
        assert_ne!(stale, remote_tip, "precondition: local origin/main must be stale");
        (local, remote_tip)
    }

    #[test]
    fn new_branch_starts_from_freshly_fetched_origin_base() {
        let dir = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let (local, remote_tip) = setup_stale_clone(dir.path());

        let created = create_worktree(home.path(), &local, "fresh-base", true, Some("main"))
            .expect("create worktree");
        let head = run_git(&created.worktree_path, &["rev-parse", "HEAD"]).expect("HEAD");
        assert_eq!(
            head, remote_tip,
            "daemon-created branch should start at the freshly-fetched remote tip"
        );
    }

    #[test]
    fn new_branch_offline_falls_back_to_stale_origin_ref() {
        let dir = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let (local, remote_tip) = setup_stale_clone(dir.path());
        let stale = run_git(&local, &["rev-parse", "refs/remotes/origin/main"]).expect("stale");

        // Sever the remote so the fetch fails (offline simulation).
        run_git(&local, &["remote", "set-url", "origin", "/nonexistent/origin.git"])
            .expect("set-url");

        let created = create_worktree(home.path(), &local, "offline-base", true, Some("main"))
            .expect("creation must not hard-fail when the remote is unreachable");
        let head = run_git(&created.worktree_path, &["rev-parse", "HEAD"]).expect("HEAD");
        assert_eq!(head, stale, "offline fallback should use the existing (stale) origin/main");
        assert_ne!(head, remote_tip);
    }

    #[test]
    fn new_branch_without_remote_uses_local_base() {
        // Local-only repos (routine on headless hosts) must keep working:
        // no origin remote → fetch is skipped, base resolves to the local
        // branch.
        let repo = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        init_repo(repo.path());
        let local_tip = run_git(repo.path(), &["rev-parse", "main"]).expect("tip");

        let created = create_worktree(home.path(), repo.path(), "no-remote-base", true, Some("main"))
            .expect("create worktree");
        let head = run_git(&created.worktree_path, &["rev-parse", "HEAD"]).expect("HEAD");
        assert_eq!(head, local_tip);
    }
}
