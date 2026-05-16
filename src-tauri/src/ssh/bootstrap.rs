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

/// Return the on-disk path of the bundled `codemux-remote` binary
/// matching the given target triple. The release CI puts these at
/// `src-tauri/binaries/codemux-remote-<target>`; in dev builds the
/// path may not exist if the cross-compile step was skipped — the
/// caller handles `None` by returning `BinaryNotBundled`.
pub fn bundled_binary_path(target: &str) -> Option<PathBuf> {
    // The runtime resolution is "binary lives next to the laptop
    // codemux executable's resources dir." In Tauri that's the
    // app's bundle resource dir at runtime; in dev/tests we fall
    // back to looking under the workspace `src-tauri/binaries/`.
    //
    // We try both so dev builds also work without hard-coding a
    // Tauri-only API into this module.
    let candidates = [
        PathBuf::from(format!("binaries/codemux-remote-{target}")),
        PathBuf::from(format!("src-tauri/binaries/codemux-remote-{target}")),
        PathBuf::from(format!(
            "../binaries/codemux-remote-{target}"
        )),
    ];
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    None
}

pub struct BootstrapOptions<'a> {
    pub ssh_target: &'a str,
    pub uname: &'a str,
    /// Remote install path. Defaults to `~/.local/bin/codemux-remote`
    /// which works for both Linux and macOS without sudo and is on
    /// PATH for most modern shells.
    pub remote_install_path: &'a str,
    pub timeout: Duration,
}

impl<'a> BootstrapOptions<'a> {
    pub fn new(ssh_target: &'a str, uname: &'a str) -> Self {
        Self {
            ssh_target,
            uname,
            remote_install_path: "~/.local/bin/codemux-remote",
            timeout: Duration::from_secs(90),
        }
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
    let local_binary = match bundled_binary_path(target) {
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
