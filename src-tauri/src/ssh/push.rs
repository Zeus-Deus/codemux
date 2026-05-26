//! Push / pull workspace to a remote host.
//!
//! `push_workspace` rsyncs the local worktree to the remote, mirroring the
//! `~/.codemux/worktrees/<repo>/<branch>` layout exactly so agents see an
//! identical filesystem on either side. The local sessions are torn down;
//! the user reopens them on the remote (adapter-aware agents like
//! Claude Code auto-resume via `--continue` / `--resume`).
//!
//! `pull_workspace_back` does the reverse: rsync back any work done on the
//! remote, shut down the remote daemon, close the tunnel, clear host_id.
//!
//! Why rsync and not a fancier sync layer:
//! - Already on every Unix-y system, no extra binary to install
//! - Smart about deltas (only transfers changed files)
//! - Easy to reason about (one process, one direction)
//! - The user can run the exact command by hand to debug
//!
//! What's intentionally NOT here:
//! - Live PTY migration across the network. Agents are interrupted on
//!   push; they resume cleanly via the existing adapter system. This is
//!   the same "stop-sync-restart" model the persistent-agent doc
//!   describes for the local case.

#![cfg(unix)]

use serde::Serialize;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// Outcome of a push attempt. Serializable so it crosses the Tauri IPC
/// boundary for the workspace push button.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PushResult {
    /// Worktree transferred and the remote workspace path is ready.
    /// `remote_path` is what the daemon should `cd` into.
    Pushed {
        remote_path: String,
        rsync_summary: String,
    },
    /// Rsync failed. `reason` is captured stderr so the user can debug.
    RsyncFailed { reason: String },
    /// SSH could not reach the host, or the prepare step (mkdir) failed.
    /// Wraps the underlying error verbatim.
    HostUnreachable { reason: String },
    /// The local worktree path doesn't exist (corrupted state, deleted
    /// directory). Doesn't try to push.
    LocalNotFound { path: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PullResult {
    Pulled {
        local_path: String,
        rsync_summary: String,
    },
    RsyncFailed { reason: String },
    HostUnreachable { reason: String },
    RemoteNotFound { path: String },
}

pub struct PushOptions<'a> {
    pub ssh_target: &'a str,
    /// Absolute local path of the worktree to push.
    pub local_worktree: &'a Path,
    /// The remote-side path the worktree should land at. The desktop
    /// computes this from the same convention codemux uses locally
    /// (`~/.codemux/worktrees/<project>/<branch>`) so the agents inside
    /// see identical paths on either side.
    pub remote_path: &'a str,
    /// Per-step timeout. The mkdir is fast; the rsync can be slow for
    /// large worktrees. We use the same timeout for both for simplicity;
    /// 10 minutes covers nearly any realistic worktree.
    pub step_timeout: Duration,
}

impl<'a> PushOptions<'a> {
    pub fn new(
        ssh_target: &'a str,
        local_worktree: &'a Path,
        remote_path: &'a str,
    ) -> Self {
        Self {
            ssh_target,
            local_worktree,
            remote_path,
            step_timeout: Duration::from_secs(600),
        }
    }
}

pub struct PullOptions<'a> {
    pub ssh_target: &'a str,
    pub remote_path: &'a str,
    pub local_worktree: &'a Path,
    pub step_timeout: Duration,
}

impl<'a> PullOptions<'a> {
    pub fn new(
        ssh_target: &'a str,
        remote_path: &'a str,
        local_worktree: &'a Path,
    ) -> Self {
        Self {
            ssh_target,
            remote_path,
            local_worktree,
            step_timeout: Duration::from_secs(600),
        }
    }
}

/// Build the rsync argv for `push`. Extracted for unit testing — getting
/// the trailing-slash semantics wrong is the kind of bug that silently
/// nests directories one level deep on the remote.
pub fn build_push_rsync_argv(opts: &PushOptions<'_>) -> Vec<String> {
    let mut local = opts.local_worktree.to_string_lossy().to_string();
    // Trailing slash makes rsync copy CONTENTS into the target, not the
    // directory itself. Without this `homelab:/path/branch/` we'd end up
    // with `homelab:/path/branch/branch/`.
    if !local.ends_with('/') {
        local.push('/');
    }
    let remote_spec = format!("{}:{}/", opts.ssh_target, opts.remote_path);
    vec![
        // -a = archive (recursive + preserve perms/times/owner/group)
        // -z = compress in transit (worth it for source code)
        // --partial = resume interrupted transfers on retry
        // --human-readable = friendlier --stats output
        "-az".into(),
        "--partial".into(),
        "--human-readable".into(),
        // --delete makes the remote MIRROR the local — files removed
        // locally also disappear remotely. Without this, a stale build
        // artifact removed locally would haunt the remote forever.
        "--delete".into(),
        // Exclude the few things we never want to ship: git's lock
        // files (transient, source of races), and the codemux scrollback
        // cache (~/.local/share/codemux is symlinked from the workspace
        // in some setups; never the right thing to copy).
        "--exclude=.git/index.lock".into(),
        "--exclude=.git/COMMIT_EDITMSG.swp".into(),
        // Skip the noisy stuff every modern project has. The user can
        // override with a `.codemuxignore` (matched by rsync's
        // `--filter`) — TODO when someone asks.
        "--exclude=node_modules/".into(),
        "--exclude=target/".into(),
        "--exclude=dist/".into(),
        "--exclude=.next/".into(),
        // SSH transport: reuse the user's config + agent, with the same
        // BatchMode guard as the probe so we never hang on a prompt.
        "-e".into(),
        "ssh -o BatchMode=yes -o ConnectTimeout=10".into(),
        local,
        remote_spec,
    ]
}

/// Mirror of `build_push_rsync_argv` for the reverse direction.
pub fn build_pull_rsync_argv(opts: &PullOptions<'_>) -> Vec<String> {
    let remote_spec = format!("{}:{}/", opts.ssh_target, opts.remote_path);
    let mut local = opts.local_worktree.to_string_lossy().to_string();
    if !local.ends_with('/') {
        local.push('/');
    }
    vec![
        "-az".into(),
        "--partial".into(),
        "--human-readable".into(),
        "--delete".into(),
        "--exclude=.git/index.lock".into(),
        "--exclude=.git/COMMIT_EDITMSG.swp".into(),
        "--exclude=node_modules/".into(),
        "--exclude=target/".into(),
        "--exclude=dist/".into(),
        "--exclude=.next/".into(),
        "-e".into(),
        "ssh -o BatchMode=yes -o ConnectTimeout=10".into(),
        remote_spec,
        local,
    ]
}

/// Push the worktree to the remote host.
pub async fn push_workspace(opts: PushOptions<'_>) -> PushResult {
    if !opts.local_worktree.exists() {
        return PushResult::LocalNotFound {
            path: opts.local_worktree.display().to_string(),
        };
    }

    // Pre-create the remote directory so rsync's first transfer doesn't
    // race against a missing parent. mkdir -p is idempotent.
    let mkdir = run_with_timeout(
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(opts.ssh_target)
            .arg(format!("mkdir -p {}", shell_escape(opts.remote_path))),
        opts.step_timeout,
    )
    .await;
    if let Err(reason) = mkdir {
        return PushResult::HostUnreachable {
            reason: format!("mkdir failed: {reason}"),
        };
    }

    // Actual rsync.
    let argv = build_push_rsync_argv(&opts);
    let mut cmd = Command::new("rsync");
    for arg in &argv {
        cmd.arg(arg);
    }
    let result = run_capture_with_timeout(&mut cmd, opts.step_timeout).await;
    let pushed = match result {
        Ok(stdout) => PushResult::Pushed {
            remote_path: opts.remote_path.to_string(),
            rsync_summary: trim_rsync_output(&stdout),
        },
        Err(reason) => return PushResult::RsyncFailed { reason },
    };

    // Best-effort: drop a `.mcp.json` into the pushed workspace dir so
    // an agent CLI (Claude Code, Codex, Gemini, …) launched on the
    // remote inside this workspace auto-discovers Codemux via
    // `codemux-remote mcp`. This mirrors the desktop's per-workspace
    // `.mcp.json` pattern (`mcp_server::upsert_mcp_config`).
    //
    // A failure here does NOT roll back the push — the user's files
    // are already on the remote, and the daemon itself doesn't need
    // .mcp.json. The agent on the remote can still be configured by
    // hand. We log so the host pane can show a soft warning later.
    if let Err(error) = crate::ssh::bootstrap::provision_workspace_mcp_config(
        opts.ssh_target,
        opts.remote_path,
        "~/.local/bin/codemux-remote",
        opts.step_timeout,
    )
    .await
    {
        eprintln!(
            "[codemux::ssh::push] .mcp.json provisioning failed for {}: {error}",
            opts.remote_path
        );
    }

    // Best-effort: register the pushed workspace in the remote
    // daemon's registry so it shows up in `workspace_list` from any
    // MCP-aware agent on the host. If the daemon isn't running yet
    // (host hates systemd, bootstrap was skipped, …) this fails
    // silently — the user can still register manually via the agent.
    //
    // Branch + project_root aren't known at this layer; the push UI
    // can pass them later by extending PushOptions. v1 just registers
    // by path so the workspace at least exists in the registry.
    let workspace_name = std::path::Path::new(opts.remote_path)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    if let Err(error) = crate::ssh::bootstrap::register_workspace_on_remote(
        opts.ssh_target,
        "~/.local/bin/codemux-remote",
        opts.remote_path,
        workspace_name.as_deref(),
        None,
        None,
        opts.step_timeout,
    )
    .await
    {
        eprintln!(
            "[codemux::ssh::push] workspace register failed for {}: {error}",
            opts.remote_path
        );
    }

    pushed
}

/// Pull the worktree back from the remote host.
pub async fn pull_workspace_back(opts: PullOptions<'_>) -> PullResult {
    if !opts.local_worktree.exists() {
        // The local target dir must exist for rsync to write into. Try
        // to create it; if that fails (permissions, disk full), surface
        // a useful error rather than letting rsync produce a cryptic
        // one.
        if let Err(error) = std::fs::create_dir_all(opts.local_worktree) {
            return PullResult::RsyncFailed {
                reason: format!(
                    "could not create local target {}: {error}",
                    opts.local_worktree.display()
                ),
            };
        }
    }

    // Verify the remote path actually exists. Without this, an empty
    // mirror would happily delete every local file (because of
    // --delete).
    let remote_check = run_with_timeout(
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(opts.ssh_target)
            .arg(format!(
                "test -d {} || exit 7",
                shell_escape(opts.remote_path)
            )),
        opts.step_timeout,
    )
    .await;
    if let Err(reason) = remote_check {
        // exit 7 means our explicit "not a directory" signal; anything
        // else means SSH itself failed.
        if reason.contains("exit status 7") {
            return PullResult::RemoteNotFound {
                path: opts.remote_path.to_string(),
            };
        }
        return PullResult::HostUnreachable {
            reason: format!("remote_check failed: {reason}"),
        };
    }

    let argv = build_pull_rsync_argv(&opts);
    let mut cmd = Command::new("rsync");
    for arg in &argv {
        cmd.arg(arg);
    }
    let result = run_capture_with_timeout(&mut cmd, opts.step_timeout).await;
    match result {
        Ok(stdout) => PullResult::Pulled {
            local_path: opts.local_worktree.display().to_string(),
            rsync_summary: trim_rsync_output(&stdout),
        },
        Err(reason) => PullResult::RsyncFailed { reason },
    }
}

/// Quote a path for safe inclusion in a shell command, while
/// preserving leading `~/` and `~user/` so the remote shell still
/// expands them to the appropriate home directory.
///
/// Defensive against pathological host paths like `/tmp/a b 'c'`
/// — single quotes around the body block every shell metachar
/// inside.
///
/// The tilde-preservation matters because our remote paths use
/// the conventional `~/.codemux/worktrees/<project>/<branch>`
/// layout. A naive `'~/foo'` would tell the shell "create a
/// literal `~` dir," not "create `foo` inside your home." We hit
/// this in production with the push flow: mkdir succeeded
/// creating `~/...` in cwd, then rsync failed because the
/// expected `$HOME/...` parent didn't exist.
fn shell_escape(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("~/{}", shell_escape_body(rest));
    }
    // `~user/...` — less common but legitimate for paths into
    // another user's home. Tilde + user must stay unquoted for
    // the shell to expand it.
    if path.starts_with('~') {
        if let Some(slash_off) = path[1..].find('/') {
            let split = 1 + slash_off + 1;
            let (tilde_user_slash, rest) = path.split_at(split);
            return format!("{}{}", tilde_user_slash, shell_escape_body(rest));
        }
        // Bare `~` or `~user` with nothing after — no body to
        // quote, the tilde IS the whole path.
        return path.to_string();
    }
    shell_escape_body(path)
}

fn shell_escape_body(s: &str) -> String {
    // POSIX-safe single-quote escape: replace any inner `'` with
    // `'\''` (close-quote, escaped quote, open-quote).
    let escaped = s.replace('\'', r"'\''");
    format!("'{escaped}'")
}

/// Trim rsync's noisy progress output to the last few summary lines.
/// The full output is in the captured stdout but rendering 200 lines of
/// per-file progress in the success toast is bad UX.
fn trim_rsync_output(stdout: &str) -> String {
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    if lines.len() <= 8 {
        return lines.join("\n");
    }
    let tail: Vec<&str> = lines.iter().rev().take(6).copied().collect();
    let mut tail = tail;
    tail.reverse();
    tail.join("\n")
}

async fn run_with_timeout(
    cmd: &mut Command,
    deadline: Duration,
) -> Result<(), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = timeout(deadline, async { cmd.output().await })
        .await
        .map_err(|_| "operation timed out".to_string())?
        .map_err(|e| format!("spawn failed: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("exit status {}", out.status)
        } else {
            stderr
        });
    }
    Ok(())
}

async fn run_capture_with_timeout(
    cmd: &mut Command,
    deadline: Duration,
) -> Result<String, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = timeout(deadline, async { cmd.output().await })
        .await
        .map_err(|_| "operation timed out".to_string())?
        .map_err(|e| format!("spawn failed: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("exit status {}", out.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Compute the conventional remote workspace path for a given branch
/// and project. Mirrors the local layout
/// (`~/.codemux/worktrees/<project>/<branch>`) so agents see identical
/// paths on either side.
///
/// Returns `~/.codemux/worktrees/<sanitized-project>/<sanitized-branch>`
/// with leading-slash + non-`[A-Za-z0-9_.-]` collapsed to `-`.
/// Encode an absolute path the way Claude Code does for its
/// per-project session-history directory. Claude stores each
/// project's conversation JSONLs at
/// `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`, where
/// the encoding replaces both `/` AND `.` with `-`.
///
/// Example: `/home/zeus/.codemux/worktrees/proj/main` →
/// `-home-zeus--codemux-worktrees-proj-main`. The double dash comes
/// from `/.codemux`: the `/` becomes `-` AND the `.` becomes `-`,
/// adjacent. (Confirmed empirically: replacing only `/` produces
/// `-home-zeus-.codemux-...` which Claude doesn't recognize — Claude
/// uses `-home-zeus--codemux-...` with the dot ALSO mapped to `-`.)
///
/// Used by the push flow to figure out where on the remote host to
/// rsync the laptop's Claude session JSONLs so `claude --resume <uuid>`
/// finds them.
pub fn claude_project_dir_name(absolute_path: &std::path::Path) -> String {
    absolute_path
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// Cross-platform — see `crate::workspace_paths::conventional_remote_path`.
/// Kept as a Unix-side re-export so existing call sites compile
/// unchanged. Windows builds reach the underlying function through
/// `crate::workspace_paths` directly.
pub use crate::workspace_paths::conventional_remote_path;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn claude_project_dir_name_matches_observed_encoding() {
        // Pinned against a real directory listing on the author's
        // machine — Claude Code stores per-project session JSONLs at
        // `~/.claude/projects/<encoded>/` where the encoding is just
        // `/` → `-`. The double-dash for `/.codemux` is incidental
        // (leading `/` of `.codemux` becomes `-`, adjacent to the
        // preceding `-`).
        assert_eq!(
            claude_project_dir_name(std::path::Path::new(
                "/home/zeus/.codemux/worktrees/codemux-step1-test/final-smoke"
            )),
            "-home-zeus--codemux-worktrees-codemux-step1-test-final-smoke"
        );
    }

    #[test]
    fn claude_project_dir_name_handles_simple_path() {
        assert_eq!(
            claude_project_dir_name(std::path::Path::new("/home/user")),
            "-home-user"
        );
    }

    #[test]
    fn claude_project_dir_name_handles_no_leading_slash() {
        // Relative paths shouldn't really be passed here, but make
        // sure we don't panic if they are.
        assert_eq!(
            claude_project_dir_name(std::path::Path::new("foo/bar")),
            "foo-bar"
        );
    }

    #[test]
    fn push_rsync_argv_has_trailing_slash_on_source() {
        // Trailing slash on source means "copy contents". Without it
        // we'd nest the worktree dir one level deep on the remote,
        // which would break path-aware agents.
        let local = PathBuf::from("/tmp/foo");
        let opts = PushOptions {
            ssh_target: "u@h",
            local_worktree: &local,
            remote_path: "~/.codemux/worktrees/proj/branch",
            step_timeout: Duration::from_secs(60),
        };
        let argv = build_push_rsync_argv(&opts);
        let src = argv.iter().find(|a| a.starts_with("/tmp/foo")).unwrap();
        assert!(src.ends_with('/'), "source must have trailing slash, got {src}");
    }

    #[test]
    fn push_rsync_argv_uses_ssh_with_batchmode() {
        let local = PathBuf::from("/tmp/foo");
        let opts = PushOptions {
            ssh_target: "u@h",
            local_worktree: &local,
            remote_path: "~/.codemux/worktrees/proj/branch",
            step_timeout: Duration::from_secs(60),
        };
        let argv = build_push_rsync_argv(&opts);
        let e_idx = argv.iter().position(|a| a == "-e").expect("has -e");
        let spec = &argv[e_idx + 1];
        assert!(spec.contains("BatchMode=yes"), "spec={spec}");
        assert!(spec.contains("ConnectTimeout"), "spec={spec}");
    }

    #[test]
    fn push_rsync_argv_uses_delete_for_mirror_semantics() {
        let local = PathBuf::from("/tmp/foo");
        let opts = PushOptions {
            ssh_target: "u@h",
            local_worktree: &local,
            remote_path: "~/.codemux/worktrees/proj/branch",
            step_timeout: Duration::from_secs(60),
        };
        let argv = build_push_rsync_argv(&opts);
        // --delete is load-bearing: without it, files removed locally
        // would persist on the remote forever. Catch a regression
        // here loudly.
        assert!(argv.iter().any(|a| a == "--delete"));
    }

    #[test]
    fn push_rsync_argv_excludes_node_modules_and_target() {
        let local = PathBuf::from("/tmp/foo");
        let opts = PushOptions {
            ssh_target: "u@h",
            local_worktree: &local,
            remote_path: "~/.codemux/worktrees/proj/branch",
            step_timeout: Duration::from_secs(60),
        };
        let argv = build_push_rsync_argv(&opts);
        assert!(argv.iter().any(|a| a == "--exclude=node_modules/"));
        assert!(argv.iter().any(|a| a == "--exclude=target/"));
    }

    #[test]
    fn pull_rsync_argv_inverts_source_and_destination() {
        let local = PathBuf::from("/tmp/foo");
        let opts = PullOptions {
            ssh_target: "u@h",
            remote_path: "~/.codemux/worktrees/proj/branch",
            local_worktree: &local,
            step_timeout: Duration::from_secs(60),
        };
        let argv = build_pull_rsync_argv(&opts);
        // The last two positional args must be remote-first, local-
        // second for pull (rsync convention: src then dst).
        let remote_pos = argv.iter().position(|a| a.contains("u@h:")).unwrap();
        let local_pos = argv.iter().position(|a| a.starts_with("/tmp/foo")).unwrap();
        assert!(
            remote_pos < local_pos,
            "pull must have remote BEFORE local in argv"
        );
    }

    #[test]
    fn conventional_remote_path_sanitizes_branch_names() {
        // Branch names can contain slashes (`feature/foo`) which would
        // create unintended subdirs on the remote. The convention
        // collapses non-safe chars to `-` to match what the local
        // codemux does.
        let p = conventional_remote_path("my-proj", "feature/login-bug");
        assert_eq!(
            p,
            PathBuf::from("~/.codemux/worktrees/my-proj/feature-login-bug")
        );
    }

    #[test]
    fn conventional_remote_path_handles_empty_inputs() {
        let p = conventional_remote_path("", "");
        assert_eq!(p, PathBuf::from("~/.codemux/worktrees/workspace/main"));
    }

    #[test]
    fn shell_escape_handles_embedded_quotes() {
        assert_eq!(shell_escape("simple"), "'simple'");
        assert_eq!(shell_escape("with space"), "'with space'");
        assert_eq!(shell_escape("/path/with'quote"), r"'/path/with'\''quote'");
    }

    #[test]
    fn shell_escape_preserves_tilde_for_remote_home_expansion() {
        // Regression guard: a naive quote like `'~/foo'` tells the
        // shell to use a LITERAL `~` directory instead of the
        // user's home. Real-world failure: push to a remote where
        // the username doesn't match the local one would silently
        // create `cwd/~/.codemux/...` and rsync would fail with a
        // confusing "No such file or directory" because the
        // expected `$HOME/.codemux/...` parent never existed.
        assert_eq!(shell_escape("~/.codemux/worktrees/proj/branch"),
                   "~/'.codemux/worktrees/proj/branch'");
    }

    #[test]
    fn shell_escape_preserves_tilde_user_form() {
        // `~user/...` is the rarer "into another user's home"
        // form. Same hazard, same fix.
        assert_eq!(shell_escape("~alice/code/x"), "~alice/'code/x'");
    }

    #[test]
    fn shell_escape_bare_tilde_unchanged() {
        // `~` alone is just the home dir reference; nothing to
        // quote.
        assert_eq!(shell_escape("~"), "~");
        assert_eq!(shell_escape("~alice"), "~alice");
    }

    #[test]
    fn shell_escape_tilde_with_embedded_quote_in_body() {
        // The tilde-preserving variant must still escape inner
        // quotes in the post-tilde body.
        assert_eq!(
            shell_escape("~/path/with'quote/file"),
            r"~/'path/with'\''quote/file'"
        );
    }

    #[test]
    fn trim_rsync_output_returns_short_input_verbatim() {
        let input = "sending\nincremental\ndone";
        assert_eq!(trim_rsync_output(input), input);
    }

    #[test]
    fn trim_rsync_output_keeps_only_tail_for_long_input() {
        let mut lines = Vec::new();
        for i in 0..50 {
            lines.push(format!("file-{i}"));
        }
        let input = lines.join("\n");
        let trimmed = trim_rsync_output(&input);
        assert!(
            trimmed.split('\n').count() <= 6,
            "trimmed should have at most 6 lines, got {} lines: {trimmed}",
            trimmed.split('\n').count()
        );
        // The tail should preserve the last meaningful lines.
        assert!(trimmed.contains("file-49"));
    }
}
