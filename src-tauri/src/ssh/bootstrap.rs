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

    // Step 1: ensure the remote install dir exists. mkdir -p is
    // idempotent so this is safe to re-run.
    let mkdir = run_with_timeout(
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(opts.ssh_target)
            .arg(format!(
                "mkdir -p \"$(dirname {})\"",
                opts.remote_install_path
            )),
        opts.timeout,
    )
    .await;
    if let Err(reason) = mkdir {
        return BootstrapResult::UploadFailed {
            reason: format!("mkdir failed: {reason}"),
        };
    }

    // Step 2: scp the binary.
    let scp = run_with_timeout(
        Command::new("scp")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(&local_binary)
            .arg(format!("{}:{}", opts.ssh_target, opts.remote_install_path)),
        opts.timeout,
    )
    .await;
    if let Err(reason) = scp {
        return BootstrapResult::UploadFailed {
            reason: format!("scp failed: {reason}"),
        };
    }

    // Step 3: chmod +x.
    let chmod = run_with_timeout(
        Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg(opts.ssh_target)
            .arg(format!("chmod +x {}", opts.remote_install_path)),
        opts.timeout,
    )
    .await;
    if let Err(reason) = chmod {
        return BootstrapResult::UploadFailed {
            reason: format!("chmod failed: {reason}"),
        };
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
}
