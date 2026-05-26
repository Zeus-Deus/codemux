//! Bootstrap install — scp `codemux-remote` to a fresh host.
//!
//! Called after the probe says "reachable but binary missing" AND
//! the user has clicked "Install" in the consent modal. We:
//!
//! 1. Pick the right binary based on `uname -sm` reported by the
//!    probe (`Linux x86_64` → `codemux-remote-linux-x86_64`, etc.).
//! 2. `scp` it to `~/.local/bin/codemux-remote` on the remote.
//! 3. `ssh ... chmod +x` it.
//! 4. Re-probe to confirm the binary now reports its version.
//!
//! The bundled binaries live under `src-tauri/binaries/` and are
//! produced by the release CI (one per target). In dev builds the
//! bundling step may not have run, so the bootstrap reports a clear
//! "binary not bundled" error rather than silently failing.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BootstrapResult {
    Installed { reported_version: String },
    BinaryNotBundled { wanted_target: String },
    UploadFailed { reason: String },
    PostInstallProbeFailed { reason: String },
}

/// Map a `uname -sm` string to the Rust target triple our release
/// CI cross-compiles for. Returns `None` for unsupported combos.
///
/// Extracted so tests can lock in the exact mapping — getting this
/// wrong means we'd scp a Linux binary to a Mac and the chmod would
/// succeed but the binary would never run.
pub fn target_for_uname(uname: &str) -> Option<&'static str> {
    let normalized = uname.trim();
    match normalized {
        "Linux x86_64" | "Linux amd64" => Some("x86_64-unknown-linux-gnu"),
        "Linux aarch64" | "Linux arm64" => Some("aarch64-unknown-linux-gnu"),
        "Darwin x86_64" => Some("x86_64-apple-darwin"),
        "Darwin arm64" | "Darwin aarch64" => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

/// Return the on-disk path of the `codemux-remote` binary matching
/// the given target triple. Searched locations, in order:
///
/// 1. **Tauri resource dir** (`app.path().resource_dir() /
///    binaries/codemux-remote-<target>`) — what an INSTALLED Codemux
///    sees. The release CI builds codemux-remote, places it under
///    `src-tauri/binaries/`, and tauri.conf.json's
///    `bundle.resources = ["binaries/codemux-remote-*"]` packages
///    it into the app bundle. At runtime it lives under the OS's
///    standard resource location (e.g. `/usr/lib/codemux/resources/`
///    for a `.deb` install). Requires an AppHandle, hence the
///    `Option<&AppHandle>` parameter.
///
/// 2. **Source-tree relative paths** (`binaries/...`,
///    `src-tauri/binaries/...`, `../binaries/...`) — for dev mode
///    where `cargo run` puts cwd at the repo root or `src-tauri/`.
///
/// 3. **Dev sibling next to the running codemux executable** —
///    `current_exe().parent()/codemux-remote[.exe]`. Cargo produces
///    this when you `cargo build --bin codemux-remote`, sitting at
///    `src-tauri/target/debug/codemux-remote` next to `codemux`.
///    Only used when the target triple matches the build's host
///    triple (you can't push a linux binary to a mac, even in dev).
///
/// 4. **Returns `None`** — caller treats as `BinaryNotBundled` and
///    the UI surfaces the "your build doesn't include this" error
///    with the wanted target triple so the user knows what's
///    missing.
///
/// The Tauri resource_dir path is REQUIRED for installed-mode use;
/// without it, an installed Codemux can't find its bundled binary
/// and push-to-host dies on first attempt. The dev fallbacks are
/// what let `cargo build && npm run tauri:dev` work for push to a
/// SAME-ARCH remote without running the full release pipeline.
pub fn bundled_binary_path(
    app: Option<&tauri::AppHandle>,
    target: &str,
) -> Option<PathBuf> {
    // Tauri resource dir — the installed-mode path. Skipped in
    // tests / non-Tauri contexts where `app` is None.
    if let Some(app) = app {
        use tauri::Manager;
        if let Ok(resource_dir) = app.path().resource_dir() {
            let candidate = resource_dir
                .join("binaries")
                .join(format!("codemux-remote-{target}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    // Source-tree relative — dev mode with `cargo run`.
    let candidates = [
        PathBuf::from(format!("binaries/codemux-remote-{target}")),
        PathBuf::from(format!("src-tauri/binaries/codemux-remote-{target}")),
        PathBuf::from(format!("../binaries/codemux-remote-{target}")),
    ];
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }

    // Dev fallback: look for `codemux-remote` sitting next to the
    // running codemux executable, matching ONLY when the requested
    // target is the same triple we built for. The host build target
    // is whatever cargo gave us at compile time — `TARGET` isn't a
    // standard env var rust exposes, so we synthesize it from the
    // platform cfgs that are stable across rustc versions.
    if target_matches_build_host(target) {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let sibling_name = if cfg!(windows) {
                    "codemux-remote.exe"
                } else {
                    "codemux-remote"
                };
                let candidate = parent.join(sibling_name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

/// True when `target` equals the rust target triple this codemux
/// binary was compiled for. Used to gate the dev-sibling fallback:
/// a linux-built codemux must NOT scp its own codemux-remote to a
/// macOS host even when one happens to be sitting next to it.
fn target_matches_build_host(target: &str) -> bool {
    // Cover the four release targets. cfg-gated rather than reading
    // a build-time TARGET env var because rust doesn't set one
    // reliably and target_arch/target_os are the source of truth at
    // compile time.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return target == "x86_64-unknown-linux-gnu";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return target == "aarch64-unknown-linux-gnu";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return target == "x86_64-apple-darwin";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return target == "aarch64-apple-darwin";
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )))]
    {
        let _ = target;
        false
    }
}

pub struct BootstrapOptions<'a> {
    pub ssh_target: &'a str,
    pub uname: &'a str,
    /// Remote install path. Defaults to `~/.local/bin/codemux-remote`
    /// which works for both Linux and macOS without sudo and is on
    /// PATH for most modern shells.
    pub remote_install_path: &'a str,
    pub timeout: Duration,
    /// Optional Tauri AppHandle. When Some, `bundled_binary_path`
    /// can locate the binary in the app's resource_dir (installed
    /// mode). When None (tests, CLI paths), only the source-tree +
    /// sibling fallbacks are tried.
    pub app: Option<&'a tauri::AppHandle>,
}

impl<'a> BootstrapOptions<'a> {
    pub fn new(ssh_target: &'a str, uname: &'a str) -> Self {
        Self {
            ssh_target,
            uname,
            remote_install_path: "~/.local/bin/codemux-remote",
            timeout: Duration::from_secs(90),
            app: None,
        }
    }

    pub fn with_app(mut self, app: &'a tauri::AppHandle) -> Self {
        self.app = Some(app);
        self
    }
}

pub async fn bootstrap_remote(opts: BootstrapOptions<'_>) -> BootstrapResult {
    let target = match target_for_uname(opts.uname) {
        Some(t) => t,
        None => {
            return BootstrapResult::BinaryNotBundled {
                wanted_target: format!("(unknown uname: {})", opts.uname),
            };
        }
    };
    let local_binary = match bundled_binary_path(opts.app, target) {
        Some(p) => p,
        None => {
            return BootstrapResult::BinaryNotBundled {
                wanted_target: target.to_string(),
            };
        }
    };

    // Steps 1–3: mkdir + upload + chmod, all in one `ssh … 'cat > path'`
    // pipeline. This used to be three round-trips (ssh mkdir, scp, ssh
    // chmod) but `scp` on OpenSSH 9.0+ defaults to the SFTP transport,
    // which does **not** expand `~` in destination paths — the server
    // tries to open a literal `~/.local/bin/codemux-remote` and fails
    // with `dest open ".local/bin/codemux-remote": Failure`. The fix
    // is to drive the upload through the remote login shell (which
    // expands `~` correctly) and to bundle mkdir/chmod into the same
    // shell session, halving SSH connection overhead.
    if let Err(reason) =
        ssh_upload_executable(opts.ssh_target, opts.remote_install_path, &local_binary, opts.timeout)
            .await
    {
        return BootstrapResult::UploadFailed { reason };
    }

    // Step 4: verify by re-probing the version subcommand.
    let verify = run_capture_with_timeout(
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg(opts.ssh_target)
            .arg(format!("{} version", opts.remote_install_path)),
        opts.timeout,
    )
    .await;
    let stdout = match verify {
        Ok(s) => s,
        Err(reason) => {
            return BootstrapResult::PostInstallProbeFailed { reason };
        }
    };
    let version = serde_json::from_str::<serde_json::Value>(stdout.trim())
        .ok()
        .and_then(|v| v["version"].as_str().map(|s| s.to_string()));
    match version {
        Some(v) => BootstrapResult::Installed { reported_version: v },
        None => BootstrapResult::PostInstallProbeFailed {
            reason: format!(
                "freshly-installed binary did not emit a parseable version line: {}",
                stdout.trim()
            ),
        },
    }
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

/// Provision the automation scheduler on a freshly-bootstrapped host.
///
/// Writes the scheduler auth token + this host's server id, installs a
/// systemd **user** service running `codemux-remote scheduler`, and
/// enables it with lingering so it survives logout and reboots.
///
/// Best-effort by contract: the caller treats a failure as non-fatal —
/// a host's push-workspace capability does not depend on the scheduler,
/// and not every host runs systemd.
pub async fn provision_scheduler(
    ssh_target: &str,
    remote_install_path: &str,
    token: &str,
    host_server_id: &str,
    deadline: Duration,
) -> Result<(), String> {
    use crate::automations::service;

    // Token + host-identity files. `umask 077` lands them 0600 — the
    // token is account-bearing.
    ssh_write_file(
        ssh_target,
        "~/.local/share/codemux/scheduler-token",
        token,
        deadline,
    )
    .await
    .map_err(|e| format!("writing scheduler token: {e}"))?;
    ssh_write_file(
        ssh_target,
        "~/.local/share/codemux/scheduler-host",
        host_server_id,
        deadline,
    )
    .await
    .map_err(|e| format!("writing host identity: {e}"))?;

    // systemd user unit. `~` in the install path becomes `%h` so the
    // unit's ExecStart is an absolute path systemd will accept.
    let exec_path = remote_install_path.replacen('~', "%h", 1);
    let unit = service::systemd_unit(&exec_path);
    ssh_write_file(
        ssh_target,
        &format!("~/.config/systemd/user/{}", service::SYSTEMD_UNIT_NAME),
        &unit,
        deadline,
    )
    .await
    .map_err(|e| format!("writing systemd unit: {e}"))?;

    // Enable lingering (so the service runs without an active login)
    // and start the unit.
    run_with_timeout(
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(ssh_target)
            .arg(format!(
                "loginctl enable-linger \"$USER\" >/dev/null 2>&1; \
                 systemctl --user daemon-reload && \
                 systemctl --user enable --now {}",
                service::SYSTEMD_UNIT_NAME
            )),
        deadline,
    )
    .await
    .map_err(|e| format!("enabling the scheduler service: {e}"))
}

/// Provision the headless `codemux-remote serve` daemon on a host.
///
/// Installs a systemd **user** unit that runs `codemux-remote serve`,
/// enables lingering so it survives logout and reboot, and starts it.
/// After this returns successfully an MCP-capable agent on the host
/// (configured with `codemux-remote mcp`) can drive Codemux locally
/// without any further setup — that's the auto-magic UX target.
///
/// Best-effort by contract: callers treat a failure as non-fatal —
/// a host's push-workspace capability does not depend on serve, and
/// not every host runs systemd.
pub async fn provision_serve(
    ssh_target: &str,
    remote_install_path: &str,
    deadline: Duration,
) -> Result<(), String> {
    use crate::automations::service;

    // `~` in the install path becomes `%h` so systemd expands it.
    // Unit's ExecStart needs an absolute path systemd will accept.
    let exec_path = remote_install_path.replacen('~', "%h", 1);
    let unit = service::serve_systemd_unit(&exec_path);
    ssh_write_file(
        ssh_target,
        &format!(
            "~/.config/systemd/user/{}",
            service::SERVE_SYSTEMD_UNIT_NAME
        ),
        &unit,
        deadline,
    )
    .await
    .map_err(|e| format!("writing serve systemd unit: {e}"))?;

    run_with_timeout(
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(ssh_target)
            .arg(format!(
                "loginctl enable-linger \"$USER\" >/dev/null 2>&1; \
                 systemctl --user daemon-reload && \
                 systemctl --user enable --now {}",
                service::SERVE_SYSTEMD_UNIT_NAME
            )),
        deadline,
    )
    .await
    .map_err(|e| format!("enabling the serve service: {e}"))
}

/// Register a workspace in the remote daemon's registry via SSH.
///
/// Runs `codemux-remote workspace register --path <p> --name <n>
/// --branch <b>` on the remote host. That binary, in turn, talks to
/// its own local `serve` daemon over loopback HTTP (no SSH-L tunnel
/// from the desktop). This is much simpler than maintaining an
/// HTTP-over-SSH pipe from the laptop, and the same code path can
/// be called by any future "import this dir as a workspace" UX.
///
/// Best-effort: if the daemon isn't running on the remote (systemd
/// unit not installed, host hates systemd, …), this returns Err and
/// the caller logs but doesn't fail the push. The workspace is still
/// on disk; the daemon just won't have it in its registry.
pub async fn register_workspace_on_remote(
    ssh_target: &str,
    remote_install_path: &str,
    remote_workspace_path: &str,
    name: Option<&str>,
    branch: Option<&str>,
    project_root: Option<&str>,
    deadline: Duration,
) -> Result<String, String> {
    let mut script = format!(
        "{bin} workspace register --path {path}",
        bin = shell_arg(remote_install_path),
        path = shell_arg(remote_workspace_path),
    );
    if let Some(n) = name {
        script.push_str(&format!(" --name {}", shell_arg(n)));
    }
    if let Some(b) = branch {
        script.push_str(&format!(" --branch {}", shell_arg(b)));
    }
    if let Some(p) = project_root {
        script.push_str(&format!(" --project-root {}", shell_arg(p)));
    }

    let out = run_capture_with_timeout(
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(ssh_target)
            .arg(&script),
        deadline,
    )
    .await
    .map_err(|e| format!("ssh workspace register: {e}"))?;

    // The remote prints the workspace JSON (with id, name, branch, …)
    // on stdout. Return it verbatim so the caller can parse if it
    // wants — for now the desktop just records "registered" and the
    // user can list via MCP.
    Ok(out.trim().to_string())
}

/// Single-quote a string for safe inclusion in a remote shell command.
/// Embedded single quotes are escaped via the standard `'\''` dance.
fn shell_arg(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Write a `.mcp.json` in the pushed workspace's directory on the
/// remote so an agent CLI (Claude Code, etc.) launched in that
/// directory auto-discovers Codemux via `codemux-remote mcp`. Mirrors
/// the desktop's per-workspace `.mcp.json` pattern but with the remote
/// command.
///
/// `workspace_remote_path` is the absolute path on the host where the
/// pushed workspace's worktree lives. `remote_install_path` is the
/// absolute path to the installed `codemux-remote` binary (so the
/// MCP config points at a stable binary path regardless of `$PATH`).
///
/// Best-effort: a failure to write `.mcp.json` doesn't fail the push
/// — the agent can still be configured manually, and the daemon
/// itself is unaffected.
pub async fn provision_workspace_mcp_config(
    ssh_target: &str,
    workspace_remote_path: &str,
    remote_install_path: &str,
    deadline: Duration,
) -> Result<(), String> {
    // Build the .mcp.json content. Same shape Claude Code, Codex, and
    // friends consume: a top-level "mcpServers" map keyed by server
    // name. The "codemux" key here matches the desktop's per-workspace
    // entry so users get a consistent tool name whether they're on
    // their laptop or on a pushed host.
    let exec_path = remote_install_path.replacen('~', "%h", 1);
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "codemux": {
                "command": exec_path,
                "args": ["mcp"]
            }
        }
    }))
    .map_err(|e| format!("serialise .mcp.json: {e}"))?;

    // The file lives inside the workspace tree, which has already
    // been rsync'd by the push step. We overwrite if it exists so a
    // stale entry from a previous Codemux version is refreshed.
    let target_path = format!("{}/.mcp.json", workspace_remote_path.trim_end_matches('/'));
    ssh_write_file(ssh_target, &target_path, &content, deadline)
        .await
        .map_err(|e| format!("writing .mcp.json: {e}"))
}

/// Upload a binary to the remote host, atomically replacing whatever
/// is currently at `remote_path` and marking it `chmod +x`.
///
/// Uses `ssh … 'cat > <path>.tmp && chmod +x <path>.tmp && mv <path>.tmp <path>'`
/// with the binary streamed via stdin, all in **one** SSH session:
///
/// 1. The remote login shell expands `~/`, sidestepping the OpenSSH
///    9.0+ SFTP-default scp bug where `~` is taken literally and the
///    upload fails with `dest open ".local/bin/foo": Failure`.
/// 2. Writing to a sibling `.tmp` path and then `mv`-ing into place
///    makes the swap atomic — if anything is currently exec'ing the
///    old binary, the new one becomes visible at the path the moment
///    rename completes. Old fd-holders keep running on the old inode
///    until they exit.
/// 3. Single SSH round-trip (was three: mkdir, scp, chmod). Cheaper
///    on flaky networks.
///
/// `remote_path` may contain `~/` — the remote shell expands it.
async fn ssh_upload_executable(
    ssh_target: &str,
    remote_path: &str,
    local_binary: &std::path::Path,
    deadline: Duration,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    // Stream the source into ssh stdin. `tokio::fs::read` slurps the
    // whole binary into memory — for a ~16 MB codemux-remote that's
    // fine; the alternative (copy in chunks) doesn't measurably help.
    let bytes = tokio::fs::read(local_binary)
        .await
        .map_err(|e| format!("read {}: {e}", local_binary.display()))?;

    // One-liner the remote shell runs. Use `umask 077` so the tmpfile
    // is 0600 from the moment it lands; `chmod +x` then bumps it to
    // 0700 before the rename. `mv -f` overwrites whatever was at the
    // destination — important when a stale older binary from a
    // previous Codemux install is sitting there.
    let script = format!(
        "umask 077 && mkdir -p \"$(dirname {p})\" && \
         cat > {p}.tmp && \
         chmod +x {p}.tmp && \
         mv -f {p}.tmp {p}",
        p = remote_path
    );

    let mut child = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(ssh_target)
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ssh spawn failed: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&bytes)
            .await
            .map_err(|e| format!("failed to stream binary: {e}"))?;
        // `stdin` drops here, closing the pipe so `cat` sees EOF and
        // exits 0, letting the chained `chmod` + `mv` run.
    }

    let out = timeout(deadline, child.wait_with_output())
        .await
        .map_err(|_| "operation timed out".to_string())?
        .map_err(|e| format!("ssh failed: {e}"))?;
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

/// `ssh <target> 'umask 077; mkdir -p <dir>; cat > <path>'` with
/// `content` streamed over stdin so a secret never appears in an
/// argv the host's process list could expose.
async fn ssh_write_file(
    ssh_target: &str,
    remote_path: &str,
    content: &str,
    deadline: Duration,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    let mut child = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(ssh_target)
        .arg(format!(
            "umask 077 && mkdir -p \"$(dirname {p})\" && cat > {p}",
            p = remote_path
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ssh spawn failed: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(content.as_bytes())
            .await
            .map_err(|e| format!("failed to stream content: {e}"))?;
        // `stdin` drops here, closing the pipe so `cat` sees EOF.
    }

    let out = timeout(deadline, child.wait_with_output())
        .await
        .map_err(|_| "operation timed out".to_string())?
        .map_err(|e| format!("ssh failed: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_for_uname_covers_all_four_release_targets() {
        assert_eq!(
            target_for_uname("Linux x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            target_for_uname("Linux aarch64"),
            Some("aarch64-unknown-linux-gnu")
        );
        assert_eq!(
            target_for_uname("Linux arm64"),
            Some("aarch64-unknown-linux-gnu")
        );
        assert_eq!(
            target_for_uname("Darwin x86_64"),
            Some("x86_64-apple-darwin")
        );
        assert_eq!(
            target_for_uname("Darwin arm64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            target_for_uname("Darwin aarch64"),
            Some("aarch64-apple-darwin")
        );
    }

    #[test]
    fn target_for_uname_returns_none_for_unsupported() {
        assert!(target_for_uname("FreeBSD x86_64").is_none());
        assert!(target_for_uname("Windows x86_64").is_none());
        assert!(target_for_uname("garbage").is_none());
        assert!(target_for_uname("").is_none());
    }

    #[test]
    fn target_for_uname_trims_whitespace() {
        assert_eq!(
            target_for_uname("  Linux x86_64  "),
            Some("x86_64-unknown-linux-gnu")
        );
    }

    #[test]
    fn shell_arg_wraps_in_single_quotes() {
        assert_eq!(shell_arg("hello"), "'hello'");
        assert_eq!(shell_arg("/tmp/path with space"), "'/tmp/path with space'");
    }

    #[test]
    fn shell_arg_escapes_embedded_single_quotes() {
        // Standard sh quoting dance for embedded single quotes.
        // Output looks ugly but is safe in any POSIX shell.
        assert_eq!(
            shell_arg("it's"),
            "'it'\\''s'",
            "must escape embedded quotes so the shell can't break out"
        );
    }

    /// End-to-end test that `ssh_upload_executable` works against a
    /// real SSH listener — using `ssh localhost` if the user has it
    /// keyed up. Skipped when SSH-to-self isn't reachable (BatchMode +
    /// no key + no agent → fail fast). This is the *real* regression
    /// guard for the OpenSSH 9.0+ SFTP tilde bug: on systems that
    /// reproduced the original "dest open '.local/bin/...': Failure"
    /// failure, the old code path (scp + ~/) would fail here too.
    /// The new code path goes through the remote shell and works.
    #[tokio::test]
    async fn ssh_upload_executable_works_against_localhost() {
        // BatchMode test: are we allowed to ssh to ourselves without
        // a password prompt? If not, skip.
        let probe = std::process::Command::new("ssh")
            .arg("-o").arg("BatchMode=yes")
            .arg("-o").arg("ConnectTimeout=3")
            .arg("-o").arg("StrictHostKeyChecking=accept-new")
            .arg("localhost")
            .arg("true")
            .output();
        let probe_ok = matches!(probe, Ok(out) if out.status.success());
        if !probe_ok {
            eprintln!("[test] skipping: ssh BatchMode=yes localhost not reachable");
            return;
        }

        // Source binary: any file Cargo built will do. We just need
        // *some* bytes to upload + roundtrip.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("payload.bin");
        // Write a unique payload so we can verify byte-equality on
        // the round-trip read.
        let payload: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        std::fs::write(&src, &payload).unwrap();

        // Destination: an absolute path under /tmp on this same
        // machine so the test cleans up after itself.
        let dst_dir = tmp.path().join("dst");
        let dst = dst_dir.join("uploaded-binary");
        // The function we're testing expects the tilde to be handled
        // by the remote shell — but for a localhost test we just use
        // an absolute path. Either path shape exercises the same
        // shell-pipeline code path.
        let dst_str = dst.to_string_lossy().to_string();

        let result = ssh_upload_executable(
            "localhost",
            &dst_str,
            &src,
            Duration::from_secs(15),
        )
        .await;
        assert!(result.is_ok(), "upload failed: {:?}", result);

        // Round-trip the bytes back.
        let on_disk = std::fs::read(&dst).expect("dest read");
        assert_eq!(on_disk, payload, "uploaded bytes differ from source");

        // Executable bit is set.
        let perms = std::fs::metadata(&dst).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        let mode = perms.mode() & 0o111;
        assert_ne!(mode, 0, "executable bit must be set on uploaded binary");

        // Atomic-replace: re-upload a different payload, dest should
        // now have the new bytes.
        let payload2: Vec<u8> = vec![0xab; 1024];
        std::fs::write(&src, &payload2).unwrap();
        let result = ssh_upload_executable(
            "localhost",
            &dst_str,
            &src,
            Duration::from_secs(15),
        )
        .await;
        assert!(result.is_ok(), "second upload failed: {:?}", result);
        let on_disk2 = std::fs::read(&dst).unwrap();
        assert_eq!(on_disk2, payload2, "second upload didn't replace");
    }

    #[test]
    fn shell_arg_handles_shell_metachars() {
        // A pathological remote path including $(, `, &, ;, etc. The
        // single-quoted body must contain them as literal bytes.
        let nasty = "$(rm -rf /); echo pwned; `evil`";
        let quoted = shell_arg(nasty);
        assert!(quoted.starts_with('\''));
        assert!(quoted.ends_with('\''));
        // Inner content is unchanged (no embedded ' to escape).
        assert!(quoted.contains(nasty));
    }
}
