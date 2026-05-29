//! Stable, deterministic project identity shared across the desktop,
//! the headless daemon, and the cross-device sync layer.
//!
//! Codemux derives a project's identity *deterministically* instead of
//! minting a random UUID and replicating it (the Superset approach,
//! which needs a replicated projects table + a reconcile step). Every
//! checkout of the same repo — on any host or device — computes the
//! same `project_uid` with zero coordination:
//!
//! ```text
//! canonical_key = canonical_remote(repo) ?? absolute(project_root)
//! project_uid   = UUIDv5(CODEMUX_PROJECT_NAMESPACE, canonical_key)
//! ```
//!
//! So two devices that independently clone `github.com/acme/app` group
//! its workspaces together without ever talking to each other, and a
//! workspace an agent creates on a remote host lands under the same
//! project as its siblings. Local-only repos (no git remote) fall back
//! to their absolute path, giving a device-stable identity that can't
//! converge cross-device until a remote is added — the same trade-off
//! Superset makes for `repoCloneUrl = null` projects.

use std::path::Path;
use uuid::Uuid;

/// Fixed namespace for Codemux project UUIDv5s. Never change this — it
/// would re-key every existing project. (Random constant, generated
/// once for this purpose.)
const CODEMUX_PROJECT_NAMESPACE: Uuid = Uuid::from_bytes([
    0xc0, 0xde, 0x34, 0x17, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0x00, 0x00, 0xc0, 0xde, 0xc0,
    0xde,
]);

/// Canonicalise a git remote URL to a stable, comparable key so the
/// same repo addressed via ssh / https / scp-style all collapse to one
/// value. Returns `host/owner/repo` lowercased, with any scheme,
/// `user@`, trailing `.git`, and trailing slash stripped.
///
/// Examples (all → `github.com/acme/app`):
/// - `git@github.com:acme/app.git`
/// - `ssh://git@github.com/acme/app.git`
/// - `https://github.com/acme/app`
/// - `https://user:token@github.com/Acme/App/`
///
/// Returns `None` for an empty / unparseable remote.
pub fn canonical_remote(remote: &str) -> Option<String> {
    let s = remote.trim();
    if s.is_empty() {
        return None;
    }

    // scp-like `git@host:owner/repo` → `host/owner/repo`
    let s = if let Some(rest) = s.strip_prefix("git@") {
        match rest.split_once(':') {
            Some((host, path)) => format!("{host}/{path}"),
            None => rest.to_string(),
        }
    } else {
        s.to_string()
    };

    // strip `scheme://`
    let s = match s.split_once("://") {
        Some((_scheme, rest)) => rest.to_string(),
        None => s,
    };

    // strip a leading `user[:pass]@` authority (only if it's before the
    // first path separator, so an `@` inside the path is left alone)
    let s = match s.split_once('@') {
        Some((authority, rest)) if !authority.contains('/') => rest.to_string(),
        _ => s,
    };

    // strip trailing slash, then `.git`, then any leftover slash
    let s = s.trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let s = s.trim_end_matches('/');

    if s.is_empty() {
        return None;
    }
    Some(s.to_lowercase())
}

/// Compute the deterministic `project_uid` from the canonical key
/// (`canonical_remote(...)` when available, otherwise the absolute
/// project-root path).
pub fn project_uid(canonical_key: &str) -> String {
    Uuid::new_v5(&CODEMUX_PROJECT_NAMESPACE, canonical_key.as_bytes()).to_string()
}

/// Convenience: derive the `project_uid` from a remote (preferred) or a
/// project-root path (fallback), matching the documented rule.
pub fn project_uid_for(remote: Option<&str>, project_root: &str) -> String {
    let key = remote
        .and_then(canonical_remote)
        .unwrap_or_else(|| project_root.trim_end_matches('/').to_lowercase());
    project_uid(&key)
}

/// Best-effort canonical git remote for a directory: runs
/// `git -C <dir> config --get remote.origin.url` and canonicalises the
/// result. Returns `None` if the dir isn't a repo, has no origin, or
/// git is unavailable. Keeps the pure functions above pure — this is
/// the one impure helper, used by create paths to derive identity.
pub fn git_canonical_remote(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout);
    canonical_remote(url.trim())
}

/// `main` (the repo root checkout — `.git` is a directory) vs
/// `worktree` (`.git` is a file pointing at the parent repo). When the
/// path doesn't exist or has no `.git`, default to `main`: a plain
/// project folder is a root, never a worktree.
pub fn derive_kind(path: &Path) -> &'static str {
    let git = path.join(".git");
    if git.is_file() {
        "worktree"
    } else {
        "main"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn canonical_remote_collapses_ssh_https_scp_to_one_key() {
        let expected = Some("github.com/acme/app".to_string());
        assert_eq!(canonical_remote("git@github.com:acme/app.git"), expected);
        assert_eq!(
            canonical_remote("ssh://git@github.com/acme/app.git"),
            expected
        );
        assert_eq!(canonical_remote("https://github.com/acme/app"), expected);
        assert_eq!(canonical_remote("https://github.com/acme/app.git"), expected);
        assert_eq!(canonical_remote("https://github.com/acme/app/"), expected);
        assert_eq!(
            canonical_remote("https://user:token@github.com/Acme/App/"),
            expected,
            "scheme, userinfo, case, trailing slash all normalised"
        );
    }

    #[test]
    fn canonical_remote_handles_non_github_and_empty() {
        assert_eq!(
            canonical_remote("git@gitlab.com:group/sub/proj.git"),
            Some("gitlab.com/group/sub/proj".to_string())
        );
        assert_eq!(canonical_remote(""), None);
        assert_eq!(canonical_remote("   "), None);
    }

    #[test]
    fn project_uid_is_deterministic_and_remote_wins_over_path() {
        // Same remote, different local paths on two devices → same uid.
        let a = project_uid_for(Some("git@github.com:acme/app.git"), "/home/alice/app");
        let b = project_uid_for(
            Some("https://github.com/acme/app"),
            "/Users/bob/projects/app",
        );
        assert_eq!(a, b, "deterministic convergence across devices");
        // It's a valid UUID string.
        assert!(Uuid::parse_str(&a).is_ok());
    }

    #[test]
    fn project_uid_falls_back_to_path_without_remote() {
        let a = project_uid_for(None, "/home/alice/local-only");
        let b = project_uid_for(None, "/home/alice/local-only");
        let c = project_uid_for(None, "/home/alice/other");
        assert_eq!(a, b, "stable for the same path");
        assert_ne!(a, c, "distinct paths → distinct projects");
    }

    #[test]
    fn worktree_and_main_grouping_share_uid_via_remote() {
        // A repo's main checkout and its worktree resolve to the same
        // project_uid because they share the remote.
        let remote = Some("git@github.com:acme/app.git");
        let main_uid = project_uid_for(remote, "/home/alice/app");
        let wt_uid = project_uid_for(remote, "/home/alice/.codemux/worktrees/app/feature");
        assert_eq!(main_uid, wt_uid);
    }

    #[test]
    fn derive_kind_main_for_real_repo_worktree_for_git_file() {
        let dir = TempDir::new().unwrap();

        let main = dir.path().join("repo");
        fs::create_dir_all(main.join(".git")).unwrap();
        assert_eq!(derive_kind(&main), "main");

        let wt = dir.path().join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: /repo/.git/worktrees/wt\n").unwrap();
        assert_eq!(derive_kind(&wt), "worktree");

        // No .git at all → treated as a root project.
        let bare = dir.path().join("plain");
        fs::create_dir_all(&bare).unwrap();
        assert_eq!(derive_kind(&bare), "main");
    }
}
