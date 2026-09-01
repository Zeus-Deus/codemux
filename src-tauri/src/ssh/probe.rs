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
        /// True when the remote emitted a `CMR:` line that is neither
        /// `NOT_INSTALLED` nor a parseable `version` JSON payload —
        /// i.e. *something* is present at the install path / on PATH
        /// but it doesn't produce valid version output. Two corruption
        /// modes from issue #133, two mechanisms:
        ///
        /// - A 0-byte file executes as an empty shell script: exit 0,
        ///   no output → the probe emits a bare `CMR: ` line.
        /// - A truncated ELF fails to exec (exit 126, "Exec format
        ///   error"); the probe command's `2>/dev/null || printf
        ///   'BROKEN\n'` fallback swallows the noise and emits
        ///   `CMR: BROKEN` (keeping ssh's exit status 0 so we still
        ///   parse stdout instead of reporting Unreachable).
        ///
        /// Both are unparseable as version JSON → classified broken,
        /// not merely missing — the UI can then say "installed but
        /// corrupted — Install to repair" instead of "isn't installed
        /// yet".
        ///
        /// `#[serde(default)]` so payloads serialized before this field
        /// existed still deserialize (to `false`).
        #[serde(default)]
        binary_present_but_broken: bool,
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
        //
        // The `$HOME/.local/bin/codemux-remote` fallback exists because
        // bootstrap installs there, but a non-interactive SSH shell
        // (which is what `ssh user@host 'cmd'` runs) typically does NOT
        // source `~/.profile` — so `~/.local/bin` is missing from PATH
        // on most distros (Arch, Ubuntu, Debian, Fedora). Without the
        // fallback, `command -v` returns nothing immediately after a
        // successful install and the UI re-shows the install button on
        // every subsequent "Test connection" press. Mirrors the same
        // fix applied in `commands::hosts::ensure_remote_binary_current`
        // and the supervisor's tunnel-spawn command.
        //
        // Both version invocations carry `2>/dev/null || printf 'BROKEN\n'`
        // (issue #133): a truncated ELF fails to exec with exit 126
        // ("cannot execute binary file: Exec format error"), and because
        // the version invocation is the LAST statement of its `if`
        // branch, that 126 would become the whole remote command's exit
        // status — ssh exits 126 and `probe_host` takes the
        // `!status.success()` path, reporting `Unreachable` with raw
        // exec noise instead of ever reaching `parse_probe_stdout`. The
        // `|| printf` fallback turns exec failure into a `CMR: BROKEN`
        // line and printf's exit 0, so the probe stays Reachable and
        // the parser classifies present-but-broken (→ "Install to
        // repair"). `2>/dev/null` keeps the shell's exec-error noise
        // out of the stream. The genuinely-absent branch
        // (`NOT_INSTALLED`) is untouched.
        "printf 'UNAME: ' ; uname -sm ; \
         if command -v codemux-remote >/dev/null 2>&1 ; then \
           printf 'CMR: ' ; codemux-remote version 2>/dev/null || printf 'BROKEN\\n' ; \
         elif [ -x \"$HOME/.local/bin/codemux-remote\" ] ; then \
           printf 'CMR: ' ; \"$HOME/.local/bin/codemux-remote\" version 2>/dev/null || printf 'BROKEN\\n' ; \
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
    // Killed on drop so a probe that outlives the backstop timeout
    // below does not leave an ssh process hanging around until its
    // own ConnectTimeout fires.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

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
    let mut binary_present_but_broken = false;
    let codemux_remote_version = match cmr_line.as_deref() {
        // No `CMR:` line at all, or an explicit `NOT_INSTALLED`: the
        // binary is genuinely absent. Not broken.
        None | Some("NOT_INSTALLED") => None,
        Some(json_line) => {
            // The `version` subcommand emits {"name":"codemux-remote","version":"x.y.z",...}
            let parsed = serde_json::from_str::<serde_json::Value>(json_line)
                .ok()
                .and_then(|v| v["version"].as_str().map(|s| s.to_string()));
            if parsed.is_none() {
                // Something answered on the `CMR:` line but it's not a
                // parseable version (incl. an empty string from a 0-byte
                // binary running as an empty script, or exec-format noise
                // from a truncated ELF). The binary is present but broken
                // — classify it so the UI offers "Install to repair"
                // rather than "isn't installed yet" (issue #133).
                binary_present_but_broken = true;
            }
            parsed
        }
    };
    ProbeOutcome::Reachable {
        codemux_remote_version,
        uname,
        binary_present_but_broken,
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
    fn build_probe_argv_falls_back_to_home_local_bin() {
        // Bootstrap installs to ~/.local/bin/codemux-remote, but a
        // non-interactive ssh shell typically doesn't have that dir on
        // PATH (it's added by ~/.profile, which only login shells
        // source). Without an explicit fallback, `command -v` returns
        // nothing immediately after a successful install and the UI
        // re-shows the install button on every "Test" press.
        //
        // Lock in BOTH the PATH lookup AND the absolute-path fallback
        // so a future refactor doesn't silently drop either branch.
        let argv = build_probe_argv("zeus@10.0.0.5", 8);
        let cmd = argv.last().unwrap();
        assert!(
            cmd.contains("command -v codemux-remote"),
            "PATH lookup must remain (fast path when binary is on PATH)"
        );
        assert!(
            cmd.contains("$HOME/.local/bin/codemux-remote"),
            "absolute-path fallback must remain (covers the common case \
             where ~/.local/bin isn't on the non-interactive PATH)"
        );
        // The fallback must run the binary, not just stat it — otherwise
        // we'd report "installed" for a 0-byte file or a half-uploaded
        // binary that doesn't actually execute.
        assert!(
            cmd.contains("\"$HOME/.local/bin/codemux-remote\" version"),
            "fallback must invoke the binary's version subcommand"
        );
        // BOTH version invocations must carry the `|| printf 'BROKEN`
        // failure fallback (issue #133). Without it, a truncated ELF
        // exec-fails with exit 126 as the LAST statement of its branch,
        // that 126 becomes the whole remote command's (and ssh's) exit
        // status, and probe_host reports `Unreachable` with raw exec
        // noise — parse_probe_stdout is never reached, so the
        // present-but-broken classification (and the Install-to-repair
        // path) can never fire.
        assert!(
            cmd.contains("codemux-remote version 2>/dev/null || printf 'BROKEN"),
            "PATH-lookup branch must tolerate exec failure: {cmd}"
        );
        assert!(
            cmd.contains(
                "\"$HOME/.local/bin/codemux-remote\" version 2>/dev/null || printf 'BROKEN"
            ),
            "absolute-path fallback branch must tolerate exec failure: {cmd}"
        );
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
                binary_present_but_broken,
            } => {
                assert_eq!(codemux_remote_version.as_deref(), Some("0.3.1"));
                assert_eq!(uname.as_deref(), Some("Linux x86_64"));
                // A cleanly-installed binary is not "broken".
                assert!(!binary_present_but_broken);
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
                binary_present_but_broken,
            } => {
                assert!(codemux_remote_version.is_none());
                assert_eq!(uname.as_deref(), Some("Darwin arm64"));
                // Explicit NOT_INSTALLED means genuinely absent, not
                // broken — the UI says "install", not "repair".
                assert!(!binary_present_but_broken);
            }
            other => panic!("expected Reachable, got {other:?}"),
        }
    }

    #[test]
    fn parse_probe_stdout_unparseable_version_treats_as_missing() {
        // If a malformed remote emits garbage where we expect JSON, we
        // no longer pretend the binary isn't installed. Version is still
        // None (we have nothing usable), but we now classify it as
        // present-but-broken so the UI can offer "Install to repair"
        // instead of "isn't installed yet" (issue #133).
        let payload = "UNAME: Linux x86_64\nCMR: not-json-at-all\n";
        match parse_probe_stdout(payload) {
            ProbeOutcome::Reachable {
                codemux_remote_version,
                binary_present_but_broken,
                ..
            } => {
                assert!(codemux_remote_version.is_none());
                assert!(binary_present_but_broken);
            }
            other => panic!("expected Reachable, got {other:?}"),
        }
    }

    #[test]
    fn parse_probe_stdout_bare_cmr_from_zero_byte_binary_is_broken() {
        // A 0-byte codemux-remote executes as an empty shell script:
        // exits 0, prints nothing, so the probe's `printf 'CMR: '`
        // yields a bare `CMR: ` line with no version JSON (and here no
        // trailing newline). This is the exact field-observed corruption
        // from issue #133 — must be classified broken, not missing.
        let payload = "UNAME: Linux x86_64\nCMR: ";
        match parse_probe_stdout(payload) {
            ProbeOutcome::Reachable {
                codemux_remote_version,
                uname,
                binary_present_but_broken,
            } => {
                assert!(codemux_remote_version.is_none());
                assert_eq!(uname.as_deref(), Some("Linux x86_64"));
                assert!(binary_present_but_broken);
            }
            other => panic!("expected Reachable, got {other:?}"),
        }
    }

    #[test]
    fn parse_probe_stdout_broken_marker_from_exec_failure_is_broken() {
        // A truncated ELF can't exec: the remote command's `2>/dev/null
        // || printf 'BROKEN\n'` fallback emits this exact marker (and
        // keeps ssh's exit status 0 so we parse stdout at all instead
        // of reporting Unreachable). `BROKEN` is not parseable version
        // JSON → classified present-but-broken (issue #133).
        let payload = "UNAME: Linux x86_64\nCMR: BROKEN\n";
        match parse_probe_stdout(payload) {
            ProbeOutcome::Reachable {
                codemux_remote_version,
                uname,
                binary_present_but_broken,
            } => {
                assert!(codemux_remote_version.is_none());
                assert_eq!(uname.as_deref(), Some("Linux x86_64"));
                assert!(binary_present_but_broken);
            }
            other => panic!("expected Reachable, got {other:?}"),
        }
    }

    #[test]
    fn parse_probe_stdout_handles_missing_lines() {
        // Empty payload means nothing got back. Still parse as
        // Reachable (the ssh process succeeded) with both fields
        // None — the UI will treat this as "weird, retry." No `CMR:`
        // line at all means not-broken (nothing claimed to be present).
        let outcome = parse_probe_stdout("");
        match outcome {
            ProbeOutcome::Reachable {
                codemux_remote_version,
                uname,
                binary_present_but_broken,
            } => {
                assert!(codemux_remote_version.is_none());
                assert!(uname.is_none());
                assert!(!binary_present_but_broken);
            }
            other => panic!("expected Reachable, got {other:?}"),
        }
    }
}
