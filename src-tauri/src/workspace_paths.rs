//! Cross-platform path helpers for the workspaces feature.
//!
//! Lives outside `src/ssh/` (which is `#[cfg(unix)]` because the SSH
//! transport modules only compile on Unix) so Windows builds can use
//! these pure path-sanitisation functions without dragging in the SSH
//! machinery. The function bodies are identical to the legacy
//! `src/ssh/push.rs` definitions — `src::ssh::push` re-exports from
//! here to keep existing Unix call sites working unchanged.

use std::path::{Path, PathBuf};

/// Sanitise a single path segment to ASCII-alphanumeric + `_ - .`; anything
/// else becomes `-`, then trim leading/trailing `-`. Shared by every path
/// helper here so the layout is identical across push, pull, and adopt.
fn sanitize_segment(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// The project's worktree-tree directory component.
///
/// When a deterministic `project_uid` is known, this is
/// `<basename>-<short-uid>` — collision-safe (two *different* repos that
/// share a basename, e.g. `acme/api` and `widgets/api`, get distinct
/// directories because their uids differ) while staying human-readable
/// (you can still see the repo name when you `cd` in). Without a uid it
/// degrades to the bare sanitised basename — the legacy layout — so existing
/// pushed workspaces keep resolving.
///
/// The short-uid is the first 8 hex chars of the uid (uids are UUIDs); 32
/// bits is far more than enough to disambiguate the handful of projects on
/// one host, and keeps the path short.
pub fn project_dir_component(project_uid: Option<&str>, project_name: &str) -> String {
    let base = {
        let b = sanitize_segment(project_name);
        if b.is_empty() {
            "workspace".to_string()
        } else {
            b
        }
    };
    match project_uid.map(sanitize_segment).filter(|u| !u.is_empty()) {
        Some(uid) => {
            let short: String = uid.chars().filter(|c| *c != '-').take(8).collect();
            if short.is_empty() {
                base
            } else {
                format!("{base}-{short}")
            }
        }
        None => base,
    }
}

/// The canonical worktree path layout used by push, pull, and
/// adoption across every platform:
///
/// ```text
/// ~/.codemux/worktrees/<project>/<branch>
/// ```
///
/// Sanitises both segments to ASCII-alphanumeric + `_`, `-`, `.` —
/// anything else becomes `-`. Empty inputs default to `workspace`
/// and `main` respectively so callers never end up with a path that
/// has a literally-empty directory component.
///
/// Returns a path with a literal `~/` prefix; callers expand
/// (`dirs::home_dir().join(rest)`) when they need an absolute path
/// for filesystem ops. The `~/` form is kept for the SSH-side
/// rsync target which lets the remote shell do the expansion.
pub fn conventional_remote_path(project_name: &str, branch: &str) -> PathBuf {
    conventional_remote_path_keyed(None, project_name, branch)
}

/// Like [`conventional_remote_path`] but keyed on the deterministic
/// `project_uid` when known (`<basename>-<short-uid>`), so two different
/// repos sharing a basename land in distinct worktree directories instead of
/// clobbering each other. `project_uid = None` reproduces the legacy
/// basename-only path exactly, so already-pushed workspaces still resolve.
pub fn conventional_remote_path_keyed(
    project_uid: Option<&str>,
    project_name: &str,
    branch: &str,
) -> PathBuf {
    let p = project_dir_component(project_uid, project_name);
    let b = {
        let b = sanitize_segment(branch);
        if b.is_empty() {
            "main".to_string()
        } else {
            b
        }
    };
    PathBuf::from(format!("~/.codemux/worktrees/{p}/{b}"))
}

/// Expand a `~/` prefix using the OS-detected home dir. Returns the
/// The canonical layout for a repo ROOT (default-branch) checkout:
///
/// ```text
/// ~/.codemux/projects/<project>
/// ```
///
/// Distinct from [`conventional_remote_path`] (the `worktrees/` tree,
/// for per-branch worktrees). A repo root must live OUTSIDE the
/// worktrees tree so it isn't mistaken for a disposable worktree (the
/// divergent-copy bug). Same sanitisation + `~/`-prefix contract as
/// [`conventional_remote_path`].
pub fn conventional_remote_root_path(project_name: &str) -> PathBuf {
    conventional_remote_root_path_keyed(None, project_name)
}

/// Like [`conventional_remote_root_path`] but keyed on `project_uid` when
/// known (`<basename>-<short-uid>`). `None` reproduces the legacy
/// basename-only root path exactly.
pub fn conventional_remote_root_path_keyed(
    project_uid: Option<&str>,
    project_name: &str,
) -> PathBuf {
    let p = project_dir_component(project_uid, project_name);
    PathBuf::from(format!("~/.codemux/projects/{p}"))
}

/// Expand a `~/` prefix using the OS-detected home dir. Returns the
/// input unchanged when no `~/` prefix is present. Falls back to
/// the raw path when home_dir is unavailable.
pub fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_slash_and_dot_in_branch() {
        let p = conventional_remote_path("my-proj", "feature/login-bug");
        assert_eq!(
            p.to_string_lossy(),
            "~/.codemux/worktrees/my-proj/feature-login-bug",
        );
    }

    #[test]
    fn defaults_empty_inputs() {
        let p = conventional_remote_path("", "");
        assert_eq!(
            p.to_string_lossy(),
            "~/.codemux/worktrees/workspace/main",
        );
    }

    #[test]
    fn preserves_alphanumerics_and_underscore() {
        let p = conventional_remote_path("a_b.c-d", "x_y.z-1");
        assert_eq!(
            p.to_string_lossy(),
            "~/.codemux/worktrees/a_b.c-d/x_y.z-1",
        );
    }

    #[test]
    fn uid_keyed_path_disambiguates_same_basename() {
        // Two DIFFERENT repos sharing a basename get distinct directories —
        // the collision the re-key fixes.
        let a = conventional_remote_path_keyed(
            Some("11111111-2222-3333-4444-555555555555"),
            "api",
            "main",
        );
        let b = conventional_remote_path_keyed(
            Some("99999999-8888-7777-6666-555555555555"),
            "api",
            "main",
        );
        assert_ne!(a, b, "same basename + different uid must not collide");
        assert_eq!(a.to_string_lossy(), "~/.codemux/worktrees/api-11111111/main");
        assert_eq!(
            conventional_remote_root_path_keyed(
                Some("11111111-2222-3333-4444-555555555555"),
                "api",
            )
            .to_string_lossy(),
            "~/.codemux/projects/api-11111111",
        );
    }

    #[test]
    fn none_uid_reproduces_legacy_basename_layout() {
        // The migration's safety net: with no uid, paths are byte-identical
        // to the pre-re-key layout, so already-pushed workspaces still resolve.
        assert_eq!(
            conventional_remote_path_keyed(None, "my-proj", "feature/x").to_string_lossy(),
            conventional_remote_path("my-proj", "feature/x").to_string_lossy(),
        );
        assert_eq!(
            conventional_remote_path_keyed(None, "my-proj", "feature/x").to_string_lossy(),
            "~/.codemux/worktrees/my-proj/feature-x",
        );
    }

    #[test]
    fn root_path_lives_under_projects_not_worktrees() {
        // A repo root lands under projects/, never in the worktrees tree,
        // so it can't be mistaken for a disposable worktree.
        assert_eq!(
            conventional_remote_root_path("passpage").to_string_lossy(),
            "~/.codemux/projects/passpage",
        );
        // Sanitisation + empty-default match conventional_remote_path.
        assert_eq!(
            conventional_remote_root_path("my/proj").to_string_lossy(),
            "~/.codemux/projects/my-proj",
        );
        assert_eq!(
            conventional_remote_root_path("").to_string_lossy(),
            "~/.codemux/projects/workspace",
        );
    }
}
