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
    //
    // We signal presence via a STDOUT sentinel rather than the remote
    // command's exit code: the remote `if … then echo … fi` always exits
    // 0, so a genuinely-missing directory is distinguished by the token,
    // not by string-matching `ExitStatus`'s Display (whose exact shape —
    // e.g. `exit status: 7` with a colon — is not contractual and bit us
    // before: a missing path was misreported as `HostUnreachable`). Only
    // a real SSH/transport failure yields a non-zero exit → `Err` here.
    let remote_check = run_capture_with_timeout(
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(opts.ssh_target)
            .arg(format!(
                "if test -d {} ; then echo CMX_REMOTE_DIR_OK ; else echo CMX_REMOTE_DIR_MISSING ; fi",
                shell_escape(opts.remote_path)
            )),
        opts.step_timeout,
    )
    .await;
    match remote_check {
        Ok(stdout) if stdout.contains("CMX_REMOTE_DIR_MISSING") => {
            return PullResult::RemoteNotFound {
                path: opts.remote_path.to_string(),
            };
        }
        Ok(stdout) if stdout.contains("CMX_REMOTE_DIR_OK") => {
            // Remote dir confirmed present — safe to rsync with --delete.
        }
        Ok(stdout) => {
            // SSH connected but didn't return our sentinel (shell init
            // noise, unexpected error). Treat as unreachable rather than
            // risk a --delete against an unconfirmed remote.
            return PullResult::HostUnreachable {
                reason: format!(
                    "remote_check returned unexpected output: {}",
                    stdout.trim()
                ),
            };
        }
        Err(reason) => {
            return PullResult::HostUnreachable {
                reason: format!("remote_check failed: {reason}"),
            };
        }
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
pub(crate) fn shell_escape(path: &str) -> String {
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
/// Length at which the encoded name is truncated and given a hash
/// suffix. Matches the constant the CLI ships with.
const CLAUDE_PROJECT_DIR_MAX_LEN: usize = 200;

/// Encode an absolute path the way Claude Code does for its
/// per-project session-history directory. Claude stores each
/// project's conversation JSONLs at
/// `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`.
///
/// The encoding is: replace EVERY character outside `[A-Za-z0-9]`
/// with `-`. Not just `/` and `.` — `_`, spaces, `+`, `@`, accented
/// letters and CJK all collapse to `-` too. If the result exceeds
/// `CLAUDE_PROJECT_DIR_MAX_LEN`, it is cut to that length and a
/// `-<hash>` suffix is appended, where the hash is taken over the
/// ORIGINAL (unencoded) path.
///
/// Example: `/home/zeus/.codemux/worktrees/proj/main` →
/// `-home-zeus--codemux-worktrees-proj-main`. The double dash comes
/// from `/.codemux`: the `/` becomes `-` AND the `.` becomes `-`,
/// adjacent.
///
/// The character class, the 200-char cap and the base-36 `h * 31 + c`
/// hash are ported from the encoder in the shipped CLI/SDK bundle and
/// are pinned by the tests below, so the ENCODING STEP is byte-exact.
/// Two subtleties there are load-bearing and are why this iterates
/// UTF-16 code units instead of `chars()`:
/// - The CLI's regex has no `u` flag, so it matches per UTF-16 unit.
///   A non-BMP character (one `char`, two units) becomes `--`.
/// - "Alphanumeric" means ASCII only. Rust's `char::is_alphanumeric`
///   is Unicode-aware and would wrongly keep `é` or `日`.
///
/// PARITY IS NOT UNCONDITIONAL, though, and the boundary matters. The
/// CLI never encodes a raw path: every caller first resolves it with
/// `realpath()` (falling back to the raw path when that fails) and
/// then applies Unicode NFC. This function does neither — it is a
/// pure function of its argument. It therefore agrees with the CLI
/// exactly when the path handed to it is ALREADY fully resolved and
/// ALREADY NFC, and not otherwise. Concretely:
/// - Symlinked `$HOME` (`/home` → `/var/home` on ostree-based
///   distros): the CLI writes under `-var-home-user-…` while a raw
///   encode yields `-home-user-…`. For paths on THIS machine, call
///   `claude_project_dir_name_local`, which performs the `realpath()`
///   step. Paths on a REMOTE host must stay raw — resolving those
///   against the local filesystem would be a worse guess than not
///   resolving them at all.
/// - Decomposed (NFD) paths encode differently from their NFC form:
///   `café` written as `e` + U+0301 is one UTF-16 unit longer, so it
///   gains an extra `-`. This gap is NOT closed: no Unicode-
///   normalization crate is in the dependency tree and one is not
///   worth adding for it. ASCII is NFC by definition, so every path
///   CodeMux itself builds is unaffected; a user-chosen directory
///   carrying decomposed accents is the exposed case.
///
/// Used by the push flow to figure out where on the remote host to
/// rsync the laptop's Claude session JSONLs so `claude --resume <uuid>`
/// finds them. Getting this wrong syncs to a directory the remote
/// `claude --resume` will never look in, so it fails silently.
pub fn claude_project_dir_name(absolute_path: &std::path::Path) -> String {
    let path = absolute_path.to_string_lossy();
    let encoded: String = path
        .encode_utf16()
        .map(|unit| match u8::try_from(unit) {
            Ok(byte) if byte.is_ascii_alphanumeric() => char::from(byte),
            _ => '-',
        })
        .collect();

    if encoded.len() <= CLAUDE_PROJECT_DIR_MAX_LEN {
        return encoded;
    }
    // `encoded` is pure ASCII by construction, so slicing by bytes
    // can't split a char boundary.
    format!(
        "{}-{}",
        &encoded[..CLAUDE_PROJECT_DIR_MAX_LEN],
        claude_path_hash(&path)
    )
}

/// `claude_project_dir_name` for a path on THIS machine, including the
/// `realpath()` step the CLI performs before encoding. Resolves
/// symlinks — a symlinked `$HOME` otherwise sends the sync to a
/// directory the CLI never reads — and falls back to the raw path
/// when resolution fails, same as the CLI, whose resolver is wrapped
/// in a try/catch that returns the input unchanged.
///
/// Touches the filesystem, so it is only meaningful for local paths;
/// remote paths keep using the pure encoder.
///
/// Residual gap: the CLI also NFC-normalizes and this does not — see
/// `claude_project_dir_name`.
pub fn claude_project_dir_name_local(
    local_path: &std::path::Path,
) -> String {
    let resolved = std::fs::canonicalize(local_path)
        .unwrap_or_else(|_| local_path.to_path_buf());
    claude_project_dir_name(&resolved)
}

/// The CLI's hash for over-long project paths: the classic
/// `h = h * 31 + code` string hash accumulated in a wrapping 32-bit
/// signed integer over UTF-16 code units, then rendered as unsigned
/// base-36. Deliberately not a cryptographic hash — the point is to
/// reproduce the CLI's directory name byte for byte, not to be
/// collision-resistant, so `sha2` would be the wrong tool here.
fn claude_path_hash(input: &str) -> String {
    let mut hash: i32 = 0;
    for unit in input.encode_utf16() {
        hash = hash
            .wrapping_shl(5)
            .wrapping_sub(hash)
            .wrapping_add(i32::from(unit));
    }
    // Widen before taking the magnitude: `i32::MIN.abs()` overflows,
    // whereas the CLI's arithmetic is on doubles and yields 2^31.
    let mut magnitude = i64::from(hash).unsigned_abs();
    if magnitude == 0 {
        return "0".to_string();
    }

    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while magnitude > 0 {
        out.push(DIGITS[(magnitude % 36) as usize]);
        magnitude /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base-36 digits are ASCII")
}

/// Cross-platform — see `crate::workspace_paths::conventional_remote_path`.
/// Kept as a Unix-side re-export so existing call sites compile
/// unchanged. Windows builds reach the underlying function through
/// `crate::workspace_paths` directly.
pub use crate::workspace_paths::{
    conventional_remote_path, conventional_remote_path_keyed,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn claude_project_dir_name_matches_observed_encoding() {
        // Pinned against a real directory listing on the author's
        // machine — Claude Code stores per-project session JSONLs at
        // `~/.claude/projects/<encoded>/`. The double-dash for
        // `/.codemux` comes from `/` and `.` both mapping to `-`.
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
    fn claude_project_dir_name_replaces_underscores() {
        // Regression: the encoder used to map only `/` and `.`, so
        // `my_project` survived verbatim and the rsync landed in a
        // directory `claude --resume` never reads.
        assert_eq!(
            claude_project_dir_name(std::path::Path::new(
                "/home/zeus/my_project"
            )),
            "-home-zeus-my-project"
        );
    }

    #[test]
    fn claude_project_dir_name_replaces_all_punctuation() {
        // Every non-`[A-Za-z0-9]` byte collapses to `-`, one `-` per
        // character, with no run-squashing.
        assert_eq!(
            claude_project_dir_name(std::path::Path::new(
                "/home/zeus/proj (v2)/a+b@c!/x_y.z"
            )),
            "-home-zeus-proj--v2--a-b-c--x-y-z"
        );
    }

    #[test]
    fn claude_project_dir_name_replaces_non_ascii_per_utf16_unit() {
        // `char::is_alphanumeric` would keep `é`; the CLI does not.
        // Non-BMP characters cost two `-` because the CLI's regex runs
        // over UTF-16 units.
        assert_eq!(
            claude_project_dir_name(std::path::Path::new(
                "/home/zeus/café/日本/x"
            )),
            // `café` → `caf-` (1), `/` → `-`, `日本` → `--` (2 BMP
            // chars, 1 unit each), `/` → `-`.
            "-home-zeus-caf-----x"
        );
    }

    #[test]
    fn claude_project_dir_name_truncates_long_paths_with_hash() {
        // 20 × 25-char segments blows past the 200-char cap.
        let long: String = (0..20)
            .map(|i| format!("/segment_{i}_longish_name"))
            .collect();
        let path = format!("/home/zeus{long}");
        let encoded = claude_project_dir_name(std::path::Path::new(&path));

        let (head, hash) = encoded.rsplit_once('-').expect("has suffix");
        assert_eq!(head.len(), 200, "head must be cut to exactly 200");
        assert!(!hash.is_empty(), "hash suffix must not be empty");
        assert!(
            hash.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "hash must be lowercase base-36, got {hash}"
        );
        // Pinned against the encoder in the shipped CLI bundle.
        assert_eq!(
            encoded,
            "-home-zeus-segment-0-longish-name-segment-1-longish-name-segment-2-longish-name-segment-3-longish-name-segment-4-longish-name-segment-5-longish-name-segment-6-longish-name-segment-7-longish-name-segme-zbggsa"
        );
    }

    #[test]
    fn claude_project_dir_name_leaves_short_paths_unhashed() {
        // Exactly at the cap: no truncation, no suffix.
        let path = format!("/{}", "a".repeat(199));
        let encoded = claude_project_dir_name(std::path::Path::new(&path));
        assert_eq!(encoded.len(), 200);
        assert_eq!(encoded, format!("-{}", "a".repeat(199)));
    }

    #[test]
    fn claude_project_dir_name_is_stable_and_idempotent() {
        // Both sides of a push must agree, so the same input has to
        // give the same output every call — including for the hashed
        // long-path branch, and including across the local/remote
        // encode pair in `sync_claude_projects`.
        for path in [
            "/home/zeus/my_project",
            "/home/zeus/.codemux/worktrees/proj/main",
            "/a/very/long/path/that/repeats/itself/over/and/over/again/until/it/comfortably/exceeds/the/two/hundred/character/cap/imposed/by/the/encoder/and/therefore/needs/a/trailing/hash/suffix/to/stay/unique",
        ] {
            let p = std::path::Path::new(path);
            assert_eq!(claude_project_dir_name(p), claude_project_dir_name(p));
        }

        // Distinct long paths sharing a 200-char prefix must not
        // collide — that is what the hash suffix is for.
        let prefix = "/home/zeus/".to_string() + &"padding/".repeat(30);
        let a = claude_project_dir_name(std::path::Path::new(
            &(prefix.clone() + "alpha"),
        ));
        let b = claude_project_dir_name(std::path::Path::new(
            &(prefix + "beta"),
        ));
        assert_ne!(a, b);
    }

    #[test]
    fn claude_project_dir_name_treats_nfd_and_nfc_as_different_dirs() {
        // Documents the one divergence from the CLI we knowingly keep:
        // it NFC-normalizes before encoding, we do not. Precomposed
        // `é` is one UTF-16 unit; decomposed it is two, so the
        // decomposed form spends an extra `-`.
        let nfc =
            claude_project_dir_name(std::path::Path::new("/home/zeus/café"));
        let nfd = claude_project_dir_name(std::path::Path::new(
            "/home/zeus/cafe\u{301}",
        ));
        assert_eq!(nfc, "-home-zeus-caf-");
        assert_eq!(nfd, "-home-zeus-cafe-");
        assert_ne!(
            nfc, nfd,
            "if these ever match, the NFC gap has been closed and the \
             doc comment needs updating"
        );
    }

    #[test]
    fn claude_project_dir_name_local_resolves_symlinks() {
        // The CLI realpath()s before encoding, so a symlinked $HOME
        // (`/home` → `/var/home`) puts the session JSONLs under the
        // RESOLVED name. Encoding the unresolved path points the sync
        // at a directory the CLI never reads.
        let tmp = tempfile::tempdir().expect("tempdir");
        // The tempdir itself may sit behind a symlink (`/var` →
        // `/private/var`), so start from its resolved form.
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
        let target = root.join("var-home");
        let real = target.join("proj");
        std::fs::create_dir_all(&real).expect("create dirs");
        let link = root.join("home");
        if std::os::unix::fs::symlink(&target, &link).is_err() {
            // No usable symlinks on this platform/filesystem.
            return;
        }

        let via_link = link.join("proj");
        assert_eq!(
            claude_project_dir_name_local(&via_link),
            claude_project_dir_name(&real),
            "local encode must follow the symlink"
        );
        assert_ne!(
            claude_project_dir_name(&via_link),
            claude_project_dir_name(&real),
            "the pure encoder deliberately does not resolve — that is \
             why the _local variant exists"
        );
    }

    #[test]
    fn claude_project_dir_name_local_falls_back_to_the_raw_path() {
        // The CLI's resolver is wrapped in a try/catch that hands back
        // the input, so an unresolvable path must encode raw rather
        // than blow up. Reached whenever the workspace dir has not
        // been created yet.
        let missing =
            std::path::Path::new("/codemux-does-not-exist/proj/main");
        assert_eq!(
            claude_project_dir_name_local(missing),
            claude_project_dir_name(missing)
        );
        assert!(!missing.exists(), "test premise: path must not exist");
    }

    #[test]
    fn claude_project_dir_name_local_matches_pure_encode_when_canonical() {
        // Already-resolved input: the realpath() step is a no-op, so
        // the two entry points must not drift apart.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("create dir");
        assert_eq!(
            claude_project_dir_name_local(&workspace),
            claude_project_dir_name(&workspace)
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
