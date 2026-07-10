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

/// Minimum size (in bytes) a candidate `codemux-remote` binary must be
/// before we'll treat it as a real, uploadable binary. The production
/// binary is ~16 MB; the dev-build placeholder sidecars checked into
/// `src-tauri/binaries/` are 0 bytes. 1 MB is comfortably above any
/// plausible placeholder yet far below the real binary, so it cleanly
/// separates "actual binary" from "empty/truncated placeholder".
///
/// Issue #133: without this gate, a 0-byte placeholder passed the old
/// `.exists()` check and the background upgrade poller atomically
/// installed the empty file over a working production binary on the
/// host — silent corruption. See `plausible_remote_binary`.
pub(crate) const MIN_PLAUSIBLE_REMOTE_BINARY_BYTES: u64 = 1024 * 1024;

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
///
/// **Size gate (issue #133):** every candidate is validated by
/// `plausible_remote_binary` — it must exist, be a regular file, AND
/// be at least `MIN_PLAUSIBLE_REMOTE_BINARY_BYTES`. A bare
/// `.exists()` check let a 0-byte dev placeholder (the empty sidecar
/// files checked into `src-tauri/binaries/`) qualify as "bundled";
/// the background upgrade poller then atomically installed that empty
/// file over a working host binary, bricking it. Gating on size means
/// an empty/tiny candidate is treated as *not bundled* → the caller
/// returns `BinaryNotBundled { wanted_target }` and a dev build fails
/// loudly instead of silently shipping a placeholder to a host.
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
            if plausible_remote_binary(&candidate) {
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
        if plausible_remote_binary(&c) {
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
                if plausible_remote_binary(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

/// True only when `path` points at a regular file that is at least
/// `MIN_PLAUSIBLE_REMOTE_BINARY_BYTES` in size — i.e. a candidate that
/// could actually be a real `codemux-remote`.
///
/// Returns false for a missing path, a directory, or an empty/truncated
/// file. This is the guard that stops a 0-byte dev placeholder (or a
/// half-written file) from ever being uploaded over a working host
/// binary (issue #133). A `metadata` error (permission denied, broken
/// symlink) also yields false — if we can't confirm the size, we don't
/// trust the candidate.
fn plausible_remote_binary(path: &std::path::Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.len() >= MIN_PLAUSIBLE_REMOTE_BINARY_BYTES,
        Err(_) => false,
    }
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

    // `enable + restart` (not `enable --now`) is intentional: it makes
    // this function correct for both first install AND upgrade. On
    // first install, restart of an inactive unit is identical to
    // start. On upgrade, the running daemon is killed and respawned
    // from the new binary on disk — without this, an upgraded binary
    // wouldn't take effect until the next host reboot or manual
    // `systemctl --user restart`.
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
                 systemctl --user enable {unit} && \
                 systemctl --user restart {unit}",
                unit = service::SERVE_SYSTEMD_UNIT_NAME
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
    // The `.mcp.json` "command" must be an ABSOLUTE path. Agent CLIs
    // (Claude Code, Codex, …) spawn this command directly — they do NOT
    // run it through a shell (so `~` is not expanded) and they certainly
    // don't expand the systemd `%h` specifier. The previous `%h`
    // rewrite produced a literal `%h/.local/bin/codemux-remote` that no
    // agent could exec, silently breaking remote MCP auto-discovery.
    // Resolve the remote $HOME and emit an absolute path, matching the
    // user-level config `codemux-remote serve` writes via mcp_register
    // (which uses the daemon's absolute `current_exe()`).
    let exec_path = if let Some(rest) = remote_install_path.strip_prefix("~/") {
        let home = resolve_remote_home(ssh_target, deadline).await?;
        format!("{}/{}", home.trim_end_matches('/'), rest)
    } else {
        remote_install_path.to_string()
    };
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

/// Build the remote shell one-liner that receives the streamed binary
/// on stdin, verifies its byte count, and atomically swaps it into
/// place. Extracted so tests can assert the exact pipeline without an
/// SSH round-trip.
///
/// `remote_path` may contain `~/` — the tilde-aware `shell_escape`
/// keeps the leading `~/` unquoted (so the remote shell expands it to
/// $HOME) while single-quoting the rest, neutralising any shell
/// metacharacters in a future caller's path (today's callers pass a
/// hardcoded path, so this is defense-in-depth).
///
/// `expected_len` is the exact source byte count. The remote reads back
/// `wc -c < {p}.tmp` and refuses to `mv` unless it matches — the fix
/// for issue #133, where a partially-streamed binary (e.g. a local
/// `write_all` that failed mid-flight) sent EOF to `cat`, which exited
/// 0, letting the old code `chmod`+`mv` a truncated file over a working
/// binary. Atomic ≠ verified.
///
/// Pipeline shape:
/// `umask 077 && mkdir -p … && cat > tmp && [ wc -c == len ] && chmod +x tmp && mv -f tmp p || { warn; rm -f tmp; exit 1; }`
///
/// In POSIX sh, `a && b && … && e || f` runs `f` when the `&&` chain
/// fails at ANY step. So a failure of mkdir, cat, the size check,
/// chmod, or mv all funnel into the `|| { … }` block: it removes the
/// tmp file, prints a clear stderr reason (which surfaces in
/// `BootstrapResult::UploadFailed`), and exits non-zero. Crucially the
/// previously-working binary at `{p}` is never touched, because it's
/// only replaced by the final `mv` which only runs on full success.
fn upload_script(remote_path: &str, expected_len: u64) -> String {
    // `umask 077` lands the tmpfile 0600 the moment it's created;
    // `chmod +x` then bumps it to 0700 before the rename. `mv -f`
    // overwrites whatever was at the destination — important when a
    // stale older binary from a previous Codemux install is there.
    // Note the `{{`/`}}` escaping: they emit literal `{`/`}` for the
    // shell's `|| { … }` group command.
    let p = crate::ssh::push::shell_escape(remote_path);
    format!(
        "umask 077 && mkdir -p \"$(dirname {p})\" && \
         cat > {p}.tmp && \
         [ \"$(wc -c < {p}.tmp)\" -eq {expected_len} ] && \
         chmod +x {p}.tmp && \
         mv -f {p}.tmp {p} || \
         {{ echo \"upload integrity check failed: expected {expected_len} bytes\" >&2; rm -f {p}.tmp; exit 1; }}"
    )
}

/// Upload a binary to the remote host, verifying its byte count before
/// atomically replacing whatever is currently at `remote_path` and
/// marking it `chmod +x`.
///
/// Streams the binary via stdin into a remote shell one-liner (see
/// `upload_script`), all in **one** SSH session:
///
/// 1. The remote login shell expands `~/`, sidestepping the OpenSSH
///    9.0+ SFTP-default scp bug where `~` is taken literally and the
///    upload fails with `dest open ".local/bin/foo": Failure`.
/// 2. The remote reads back `wc -c` on the received tmpfile and refuses
///    to promote it unless the byte count matches the source exactly
///    (issue #133). A stream that failed partway — or any other reason
///    the tmpfile ends up short — is rejected, the tmpfile removed, and
///    the previously-working binary at the destination left untouched.
///    Atomicity alone doesn't help here: a truncated file `mv`'d into
///    place is atomically wrong.
/// 3. Only after the size check pass does `mv -f` swap the new binary
///    into place — atomic, so anything exec'ing the old binary keeps
///    running on the old inode until it exits.
/// 4. Single SSH round-trip (was three: mkdir, scp, chmod). Cheaper
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

    // Refuse an empty source outright. An empty file would trivially
    // satisfy the remote `[ … -eq 0 ]` size check, defeating the whole
    // guard — never even attempt the upload. Belt-and-braces on top of
    // the ≥1 MB plausibility gate in `bundled_binary_path` (issue #133).
    if bytes.is_empty() {
        return Err(format!(
            "refusing to upload empty binary {} (0 bytes)",
            local_binary.display()
        ));
    }

    let script = upload_script(remote_path, bytes.len() as u64);

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
        if let Err(e) = stdin.write_all(&bytes).await {
            // The stream failed partway. The remote `wc -c` check is
            // the real backstop against a truncated install, but don't
            // leave the ssh child (and its remote shell) running —
            // kill it so we don't dangle a half-written tmpfile.
            let _ = child.kill().await;
            return Err(format!("failed to stream binary: {e}"));
        }
        // `stdin` drops here, closing the pipe so `cat` sees EOF and
        // exits 0, letting the chained size check + `chmod` + `mv` run.
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

/// Resolve the remote login user's absolute `$HOME` over SSH. Used to
/// turn a `~/…`-relative install path into the absolute path agent CLIs
/// need in `.mcp.json` (they spawn the command directly, with no shell
/// or systemd token expansion). `printf %s` avoids a trailing newline.
async fn resolve_remote_home(
    ssh_target: &str,
    deadline: Duration,
) -> Result<String, String> {
    let out = timeout(
        deadline,
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(ssh_target)
            .arg("printf %s \"$HOME\"")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| "resolving remote $HOME timed out".to_string())?
    .map_err(|e| format!("ssh spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "could not resolve remote $HOME: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let home = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if home.is_empty() {
        return Err("remote $HOME resolved to empty".to_string());
    }
    Ok(home)
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
        .arg({
            // Tilde-aware shell quoting (see ssh_upload_executable):
            // keeps `~/` expandable, quotes the body, neutralizes any
            // metacharacters in a future caller's path.
            let p = crate::ssh::push::shell_escape(remote_path);
            format!("umask 077 && mkdir -p \"$(dirname {p})\" && cat > {p}")
        })
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

        // Issue #133: an empty source must be rejected locally with Err
        // and must NOT create or replace the destination. The dest here
        // already holds `payload2`; after a rejected empty upload it must
        // still hold `payload2` untouched.
        let empty_src = tmp.path().join("empty-payload.bin");
        std::fs::write(&empty_src, b"").unwrap();
        let empty_result = ssh_upload_executable(
            "localhost",
            &dst_str,
            &empty_src,
            Duration::from_secs(15),
        )
        .await;
        assert!(
            empty_result.is_err(),
            "empty source must be rejected, got: {empty_result:?}"
        );
        let on_disk3 = std::fs::read(&dst).unwrap();
        assert_eq!(
            on_disk3, payload2,
            "rejected empty upload must not touch the destination"
        );

        // And a brand-new destination path must never be created by an
        // empty upload either.
        let fresh_dst = dst_dir.join("never-created");
        let fresh_result = ssh_upload_executable(
            "localhost",
            &fresh_dst.to_string_lossy(),
            &empty_src,
            Duration::from_secs(15),
        )
        .await;
        assert!(fresh_result.is_err(), "empty source must be rejected");
        assert!(
            !fresh_dst.exists(),
            "rejected empty upload must not create the destination"
        );
    }

    #[test]
    fn plausible_remote_binary_gates_on_size_and_file_type() {
        // Issue #133: only a real, sufficiently-large regular file may
        // ever be uploaded. Exercise every reject reason plus the pass.
        let tmp = tempfile::TempDir::new().unwrap();

        // Missing path → false (nothing to upload).
        assert!(!plausible_remote_binary(&tmp.path().join("does-not-exist")));

        // 0-byte file → false. This is the exact dev-placeholder shape
        // that bricked hosts before the fix.
        let empty = tmp.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        assert!(!plausible_remote_binary(&empty));

        // Small (4 KB) file → false. A truncated / half-written binary
        // is still far below the 1 MB floor.
        let small = tmp.path().join("small");
        std::fs::write(&small, vec![0u8; 4 * 1024]).unwrap();
        assert!(!plausible_remote_binary(&small));

        // A directory → false (must be a regular file).
        let dir = tmp.path().join("a-dir");
        std::fs::create_dir(&dir).unwrap();
        assert!(!plausible_remote_binary(&dir));

        // File at exactly the 1 MB floor → true (real binary is ~16 MB,
        // this stands in for it without writing 16 MB to disk).
        let big = tmp.path().join("big");
        std::fs::write(&big, vec![0u8; MIN_PLAUSIBLE_REMOTE_BINARY_BYTES as usize]).unwrap();
        assert!(plausible_remote_binary(&big));
    }

    #[test]
    fn upload_script_verifies_byte_count_and_cleans_up() {
        // Lock in the integrity-check pipeline shape (issue #133).
        let script = upload_script("~/.local/bin/codemux-remote", 16_777_216);

        // The size check reads the received tmpfile back with `wc -c`
        // and compares against the exact expected byte count.
        assert!(script.contains("wc -c"), "must read back byte count");
        assert!(
            script.contains("-eq 16777216"),
            "must compare against the exact expected length: {script}"
        );

        // On any failure the tmpfile is removed and we exit non-zero so
        // the prior binary is left untouched and the error propagates.
        assert!(script.contains("rm -f"), "must clean up the tmpfile on failure");
        assert!(script.contains("exit 1"), "must exit non-zero on failure");

        // The core atomic-install chain is preserved.
        assert!(script.contains("umask 077"), "tmpfile must land 0600");
        assert!(script.contains("chmod +x"), "must mark executable");
        assert!(script.contains("mv -f"), "must atomically replace");

        // Tilde-aware escaping: a `~/`-prefixed path keeps the leading
        // `~/` unquoted (so the remote shell expands it) with the body
        // single-quoted — exactly what `shell_escape` produces.
        assert!(
            script.contains("~/'.local/bin/codemux-remote'"),
            "must preserve tilde-aware escaping: {script}"
        );
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
