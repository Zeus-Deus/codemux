//! Cross-platform path helpers for the workspaces feature.
//!
//! Lives outside `src/ssh/` (which is `#[cfg(unix)]` because the SSH
//! transport modules only compile on Unix) so Windows builds can use
//! these pure path-sanitisation functions without dragging in the SSH
//! machinery. The function bodies are identical to the legacy
//! `src/ssh/push.rs` definitions — `src::ssh::push` re-exports from
//! here to keep existing Unix call sites working unchanged.

use std::path::{Path, PathBuf};

/// The canonical worktree path layout used by push, pull, and
/// adoption across every platform:
///
///     ~/.codemux/worktrees/<project>/<branch>
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
    fn sanitize(s: &str) -> String {
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
    let p = sanitize(project_name);
    let b = sanitize(branch);
    let p = if p.is_empty() {
        "workspace".to_string()
    } else {
        p
    };
    let b = if b.is_empty() {
        "main".to_string()
    } else {
        b
    };
    PathBuf::from(format!("~/.codemux/worktrees/{p}/{b}"))
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
}
