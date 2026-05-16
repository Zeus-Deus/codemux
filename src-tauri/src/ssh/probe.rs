//! SSH probe — "is this host reachable, and does it have
//! `codemux-remote` installed?"
//!
//! Three observable outcomes:
//! - `Reachable { codemux_remote_version: Some(...) }` — green light:
//!   we can use the host immediately.
//! - `Reachable { codemux_remote_version: None }` — host is up, but
//!   the binary isn't installed yet. Triggers the bootstrap-install
//!   consent modal in the UI.
//! - `Unreachable { reason }` — SSH itself failed. Reason is the
//!   stderr from the `ssh` invocation so the user can see whether
//!   it's a DNS issue, a permission denied, etc.

use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// Outcome of a single probe attempt. Serializable so it can cross
/// the Tauri IPC boundary for the "Test connection" button result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProbeOutcome {
    /// SSH connected and the remote ran our probe command.
    Reachable {
        /// Version reported by `codemux-remote version`, or `None`
        /// when the binary isn't installed. The serialized JSON the
        /// binary prints is parsed on the laptop side; failure to
        /// parse maps to `None`.
        codemux_remote_version: Option<String>,
        /// Combined kernel + arch as reported by `uname -sm`. Used
        /// by the bootstrap step to pick the right binary to scp.
        /// Example: `"Linux x86_64"`, `"Darwin arm64"`.
        uname: Option<String>,
    },
    /// SSH did not connect (DNS failure, refused, timeout, key not
    /// authorized). The user-visible message comes from `reason`.
    Unreachable { reason: String },
}

/// Probe configuration. Mostly hardcoded sensible defaults; the
/// caller only supplies the SSH target.
pub struct ProbeOptions<'a> {
    pub ssh_target: &'a str,
    pub timeout: Duration,
}

impl<'a> ProbeOptions<'a> {
    pub fn new(ssh_target: &'a str) -> Self {
        Self {
            ssh_target,
            timeout: Duration::from_secs(8),
        }
    }
}

/// Build the `ssh` argv we use for probing. Extracted so tests can
/// assert the exact flags without spawning a real ssh process —
/// catching e.g. an accidental drop of `BatchMode=yes` (which would
/// cause the probe to hang on a password prompt and look like a
/// timeout to the user).
pub fn build_probe_argv(ssh_target: &str, timeout_secs: u64) -> Vec<String> {
    vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={timeout_secs}"),
        // StrictHostKeyChecking=accept-new lets a first-time probe
        // succeed without an interactive y/n prompt. The host gets
        // added to known_hosts as usual. Subsequent probes against
        // a changed key still fail closed, which is the right
        // security default.
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        ssh_target.into(),
        // Combined probe: print `uname -sm` then call
        // `codemux-remote version` if available. The remote-side
        // `printf` separates the two with a sentinel so the laptop
        // can split.
        "printf 'UNAME: ' ; uname -sm ; \
         if command -v codemux-remote >/dev/null 2>&1 ; then \
           printf 'CMR: ' ; codemux-remote version ; \
         else \
           printf 'CMR: NOT_INSTALLED\\n' ; \
         fi"
            .into(),
    ]
}

/// Run the probe. Returns one of the three outcomes; never panics,
/// never hangs (the outer `timeout` is a backstop above SSH's own
/// `ConnectTimeout`).
pub async fn probe_host(opts: ProbeOptions<'_>) -> ProbeOutcome {
    let argv = build_probe_argv(opts.ssh_target, opts.timeout.as_secs());
    let mut cmd = Command::new("ssh");
    for arg in &argv {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let result = timeout(opts.timeout + Duration::from_secs(2), async {
        cmd.output().await
    })
    .await;

    let output = match result {
        Ok(Ok(o)) => o,
        Ok(Err(error)) => {
            return ProbeOutcome::Unreachable {
                reason: format!("ssh: {error}"),
            };
        }
        Err(_elapsed) => {
            return ProbeOutcome::Unreachable {
                reason: "ssh probe timed out".to_string(),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return ProbeOutcome::Unreachable {
            reason: if stderr.is_empty() {
                format!("ssh exited with status {}", output.status)
            } else {
                stderr
            },
        };
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_probe_stdout(&stdout)
}

/// Parse the combined `UNAME: ...\nCMR: ...` payload the probe shell
/// command emits. Extracted so we can unit-test parsing without
/// spawning ssh.
pub fn parse_probe_stdout(stdout: &str) -> ProbeOutcome {
    let mut uname: Option<String> = None;
    let mut cmr_line: Option<String> = None;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("UNAME: ") {
            uname = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("CMR: ") {
            cmr_line = Some(rest.trim().to_string());
        }
    }
    let codemux_remote_version = match cmr_line.as_deref() {
        None | Some("NOT_INSTALLED") => None,
        Some(json_line) => {
            // The `version` subcommand emits {"name":"codemux-remote","version":"x.y.z",...}
            // Parse it; on any error treat as "not installed" so the
            // UI offers the bootstrap path (better than claiming an
            // unparseable version is fine).
            serde_json::from_str::<serde_json::Value>(json_line)
                .ok()
                .and_then(|v| v["version"].as_str().map(|s| s.to_string()))
        }
    };
    ProbeOutcome::Reachable {
        codemux_remote_version,
        uname,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_probe_argv_locks_in_batch_mode_and_timeout() {
        let argv = build_probe_argv("zeus@10.0.0.5", 8);
        // Critical flags — losing any of these breaks the user
        // experience (hangs on prompts, or interactive y/n on first
        // probe).
        assert!(argv.iter().any(|a| a == "BatchMode=yes"));
        assert!(argv.iter().any(|a| a == "ConnectTimeout=8"));
        assert!(argv.iter().any(|a| a == "StrictHostKeyChecking=accept-new"));
        assert!(argv.iter().any(|a| a == "zeus@10.0.0.5"));
        // The remote command must be the LAST positional arg.
        assert!(argv.last().unwrap().contains("uname -sm"));
        assert!(argv.last().unwrap().contains("codemux-remote"));
    }

    #[test]
    fn parse_probe_stdout_reachable_with_installed_binary() {
        let payload = r#"UNAME: Linux x86_64
CMR: {"name":"codemux-remote","version":"0.3.1","protocol_version":1}
"#;
        match parse_probe_stdout(payload) {
            ProbeOutcome::Reachable {
                codemux_remote_version,
                uname,
            } => {
                assert_eq!(codemux_remote_version.as_deref(), Some("0.3.1"));
                assert_eq!(uname.as_deref(), Some("Linux x86_64"));
            }
            other => panic!("expected Reachable, got {other:?}"),
        }
    }

    #[test]
    fn parse_probe_stdout_reachable_without_binary() {
        let payload = "UNAME: Darwin arm64\nCMR: NOT_INSTALLED\n";
        match parse_probe_stdout(payload) {
            ProbeOutcome::Reachable {
                codemux_remote_version,
                uname,
            } => {
                assert!(codemux_remote_version.is_none());
                assert_eq!(uname.as_deref(), Some("Darwin arm64"));
            }
            other => panic!("expected Reachable, got {other:?}"),
        }
    }

    #[test]
    fn parse_probe_stdout_unparseable_version_treats_as_missing() {
        // If a malformed remote emits garbage where we expect JSON,
        // we degrade gracefully — pretend the binary isn't installed
        // so the user gets offered the bootstrap path. This is safer
        // than reporting a phantom version.
        let payload = "UNAME: Linux x86_64\nCMR: not-json-at-all\n";
        match parse_probe_stdout(payload) {
            ProbeOutcome::Reachable {
                codemux_remote_version,
                ..
            } => {
                assert!(codemux_remote_version.is_none());
            }
            other => panic!("expected Reachable, got {other:?}"),
        }
    }

    #[test]
    fn parse_probe_stdout_handles_missing_lines() {
        // Empty payload means nothing got back. Still parse as
        // Reachable (the ssh process succeeded) with both fields
        // None — the UI will treat this as "weird, retry."
        let outcome = parse_probe_stdout("");
        match outcome {
            ProbeOutcome::Reachable {
                codemux_remote_version,
                uname,
            } => {
                assert!(codemux_remote_version.is_none());
                assert!(uname.is_none());
            }
            other => panic!("expected Reachable, got {other:?}"),
        }
    }
}
