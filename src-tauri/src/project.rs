use std::env;
use std::path::PathBuf;

pub fn current_project_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    if cwd.file_name().and_then(|name| name.to_str()) == Some("src-tauri") {
        if let Some(parent) = cwd.parent() {
            let parent = parent.to_path_buf();
            if parent.join("package.json").exists() {
                return parent;
            }
        }
    }

    cwd
}

/// Expand a leading `~/` (or a bare `~`) in a path-as-string into the
/// process's `$HOME`. No-op for paths without a leading tilde or when
/// `$HOME` is unset.
///
/// `chdir`, `std::fs`, and `git` do NOT expand `~` — that is a shell-only
/// convention. Two places rely on this helper:
///
///  1. Project creation: the New Project screen lets the user *type* a
///     location (the placeholder even suggests `~/Projects`). Without
///     expansion, `create_empty_repo`/`git clone` build a path with a
///     literal `~` and create a phantom directory named `~` under the
///     process cwd; the workspace then stores `~/...` as its cwd and every
///     terminal fails to `chdir` into it, landing in `$HOME` instead.
///  2. Remote/tunneled PTY spawns pass `~/.codemux/worktrees/<...>` as cwd
///     because the laptop side does not know the remote's `$HOME`; the
///     daemon resolves it against its own `$HOME`.
pub fn expand_tilde(path: &str) -> String {
    expand_tilde_with(path, env::var("HOME").ok().as_deref())
}

/// Pure-function core of [`expand_tilde`], parameterized on `home` so unit
/// tests don't have to mutate the process-wide `HOME` env var (which would
/// pollute other tests in the same binary that read `$HOME`).
pub fn expand_tilde_with(path: &str, home: Option<&str>) -> String {
    if path == "~" {
        return home
            .map(|h| h.to_string())
            .unwrap_or_else(|| path.to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests exercise `expand_tilde_with`, the pure-function core that
    // takes `home` as an argument — NOT `expand_tilde`, which reads `$HOME`
    // globally. We deliberately don't touch `std::env::set_var("HOME", ...)`
    // because that mutation is process-wide and pollutes any other test in
    // the binary that reads `$HOME`.

    #[test]
    fn expand_tilde_slash_uses_home_env() {
        assert_eq!(
            expand_tilde_with("~/.codemux/worktrees/proj/branch", Some("/fake/home")),
            "/fake/home/.codemux/worktrees/proj/branch"
        );
    }

    #[test]
    fn expand_tilde_bare_returns_home() {
        assert_eq!(expand_tilde_with("~", Some("/another/home")), "/another/home");
    }

    #[test]
    fn expand_tilde_absolute_path_unchanged() {
        assert_eq!(
            expand_tilde_with("/usr/local/bin", Some("/whatever")),
            "/usr/local/bin"
        );
    }

    #[test]
    fn expand_tilde_relative_path_unchanged() {
        assert_eq!(
            expand_tilde_with("relative/path", Some("/whatever")),
            "relative/path"
        );
    }

    #[test]
    fn expand_tilde_mid_path_tilde_unchanged() {
        // We only handle a LEADING tilde — `foo/~/bar` is not a
        // tilde-expansion form; treat it as a literal path.
        assert_eq!(expand_tilde_with("foo/~/bar", Some("/whatever")), "foo/~/bar");
    }

    #[test]
    fn expand_tilde_with_no_home_leaves_tilde_alone() {
        // When `$HOME` isn't set the expansion is a no-op. Better to surface
        // the resulting failure than to silently resolve somewhere unexpected.
        assert_eq!(expand_tilde_with("~/foo", None), "~/foo");
        assert_eq!(expand_tilde_with("~", None), "~");
    }
}
