//! OpenCode CLI / server discovery.
//!
//! Stage 1 deliberately keeps this surface narrow: detect whether
//! `opencode` is on PATH, ask it for its `--version`, and (when the
//! caller supplies a known server URL) probe whether an HTTP server is
//! reachable at that URL. Stage 1 never spawns `opencode serve` itself
//! — that lifecycle belongs to Stage 2 once the live model harvest
//! needs a running server to talk to.
//!
//! All entry points return `Ok` on the "binary not installed" path
//! (with `installed: false`) instead of bubbling an error. That keeps
//! the eventual UI surface simple — empty state with install hints —
//! and means a fresh machine without OpenCode never crashes the
//! capabilities-store refresh that Stage 2/3 wires up.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::execution::sanitize_gui_env_tokio;

/// Minimum OpenCode CLI version Codemux requires. Mirrors the
/// reference pin captured in
/// `docs/plans/step-12-opencode-research.md` §3 and verified against
/// `/tmp/<reference>/apps/server/src/provider/Layers/OpenCodeProvider.ts:31`.
/// Versions below this are reported as `installed: true` with the
/// detected version still attached so the eventual UI can surface a
/// "please upgrade" hint instead of a hard error.
pub const MINIMUM_OPENCODE_VERSION: &str = "1.14.19";

/// Hard cap on each subprocess / network probe so a flaky CLI or
/// unreachable server never stalls startup. Tuned to match the codex
/// auth-probe budget at `agent_provider/codex/auth.rs`.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Snapshot returned by [`check_opencode_availability`].
///
/// All fields are independently meaningful so the UI can render
/// granular states (installed but server down, server up but version
/// too old, etc.) without re-probing. JSON shape is locked at Stage 1
/// — Stage 2/3 may add fields but must not rename existing ones, since
/// the frontend wrapper in `src/tauri/commands.ts` consumes this
/// verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeAvailability {
    /// True when an `opencode` binary was found on PATH. Independent
    /// of whether the server is reachable.
    pub installed: bool,
    /// Resolved binary path. `None` when `installed` is false; also
    /// `None` if PATH lookup succeeded but the result could not be
    /// canonicalised (rare).
    pub binary_path: Option<PathBuf>,
    /// Version string parsed out of `opencode --version`. Best-effort:
    /// `None` when the binary refused to run, exited non-zero, or
    /// emitted an unparseable banner.
    pub version: Option<String>,
    /// True when an HTTP server was reachable at the URL the caller
    /// supplied. Always `false` when `server_url` is `None` — Stage 1
    /// does not spawn its own server, so a true value here only
    /// happens when discovery was given a URL to probe.
    pub server_running: bool,
    /// URL the caller asked us to probe. Echoed back so the frontend
    /// does not need to keep two copies of the value in sync.
    pub server_url: Option<String>,
}

impl OpenCodeAvailability {
    /// Construct the "binary not installed" empty state.
    ///
    /// Hand-rolled because `Default` would imply `installed = false`
    /// is "valid forever"; this constructor exists so failure paths
    /// inside [`check_opencode_availability`] can short-circuit
    /// without manually filling every field, and so tests can assert
    /// against a single canonical not-installed value.
    pub fn not_installed() -> Self {
        Self {
            installed: false,
            binary_path: None,
            version: None,
            server_running: false,
            server_url: None,
        }
    }
}

/// Probe whether OpenCode is usable on this machine.
///
/// Steps:
///
/// 1. Resolve `opencode` on PATH via the [`which`] crate.
/// 2. Run `opencode --version` (timeout-bound) and pluck the version
///    token out of the first line. A non-zero exit or unparseable
///    banner leaves `version` as `None` but keeps `installed: true`.
/// 3. If `server_url` was supplied, send a `GET` and treat any HTTP
///    response (including 401/403) as "server is up". The caller's
///    auth credentials are not validated here — that's the eventual
///    Stage 2 job.
///
/// `server_url` is **always echoed back** in the response, independent
/// of whether the binary was found — a user could point the integration
/// at a remote OpenCode server they manage themselves, and the eventual
/// settings-panel UI needs to render a "url X is/isn't reachable" hint
/// even when the local binary is absent.
///
/// Spawns are routed through [`sanitize_gui_env_tokio`] (per
/// `CLAUDE.md` Spawning Child Processes) so an OpenCode binary that
/// inadvertently launches a GUI helper does not pop windows on the
/// host display.
pub async fn check_opencode_availability(server_url: Option<String>) -> OpenCodeAvailability {
    let (server_running, server_url) = match server_url {
        Some(url) => {
            let reachable = probe_server(&url).await;
            (reachable, Some(url))
        }
        None => (false, None),
    };

    let binary_path = match which::which("opencode") {
        Ok(path) => path,
        Err(_) => {
            return OpenCodeAvailability {
                installed: false,
                binary_path: None,
                version: None,
                server_running,
                server_url,
            };
        }
    };

    let version = probe_version(&binary_path).await;

    OpenCodeAvailability {
        installed: true,
        binary_path: Some(binary_path),
        version,
        server_running,
        server_url,
    }
}

/// Run `opencode --version` and pull the version token out of the
/// first line. Returns `None` on any failure — the caller treats that
/// as "binary present but version unknown".
async fn probe_version(binary_path: &Path) -> Option<String> {
    let mut cmd = Command::new(binary_path);
    cmd.arg("--version");
    sanitize_gui_env_tokio(&mut cmd);
    let result = tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await;
    let output = match result {
        Ok(Ok(out)) => out,
        Ok(Err(_)) => return None,
        Err(_) => return None,
    };
    if !output.status.success() {
        return None;
    }
    parse_version_output(&String::from_utf8_lossy(&output.stdout))
}

/// Pure helper so version-banner parsing is unit-testable.
///
/// Behaviour:
///
/// * Trim and split on lines; bail on empty input.
/// * Take the first non-empty line.
/// * If that line has more than one whitespace-separated token, the
///   last token is the version (matches typical `opencode 1.14.19`
///   output and is robust against future banners that prepend a
///   binary name).
/// * Otherwise the whole line is the version (some builds print
///   `1.14.19` with no prefix).
pub fn parse_version_output(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first_line = trimmed.lines().find(|l| !l.trim().is_empty())?.trim();
    let tokens: Vec<&str> = first_line.split_whitespace().collect();
    let token = match tokens.as_slice() {
        [] => return None,
        [single] => *single,
        many => *many.last().unwrap(),
    };
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Treat any HTTP response (2xx, 4xx, 5xx) as "server is up" — the
/// goal here is "is something listening", not "is the user
/// authenticated". A network-level failure (connection refused, DNS,
/// timeout) returns false.
async fn probe_server(url: &str) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client.get(url).send().await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_installed_constructor_zeros_every_field() {
        let av = OpenCodeAvailability::not_installed();
        assert!(!av.installed);
        assert_eq!(av.binary_path, None);
        assert_eq!(av.version, None);
        assert!(!av.server_running);
        assert_eq!(av.server_url, None);
    }

    #[test]
    fn parse_version_output_handles_prefixed_banner() {
        assert_eq!(
            parse_version_output("opencode 1.14.19\n"),
            Some("1.14.19".into())
        );
    }

    #[test]
    fn parse_version_output_handles_bare_version() {
        assert_eq!(parse_version_output("1.14.19"), Some("1.14.19".into()));
    }

    #[test]
    fn parse_version_output_skips_leading_blank_lines() {
        // Some CLIs print a blank line before the banner when stdout
        // is line-buffered.
        assert_eq!(
            parse_version_output("\n\nopencode 1.15.0\nthird line\n"),
            Some("1.15.0".into())
        );
    }

    #[test]
    fn parse_version_output_returns_none_for_empty() {
        assert_eq!(parse_version_output(""), None);
        assert_eq!(parse_version_output("   \n  "), None);
    }

    #[test]
    fn parse_version_output_takes_last_token_when_multiple_words() {
        // Future-proofs against `opencode cli 1.14.19` style banners.
        assert_eq!(
            parse_version_output("opencode cli 1.14.19"),
            Some("1.14.19".into())
        );
    }

    #[test]
    fn minimum_version_constant_matches_reference_pin() {
        // Pinned: changing this value silently changes which OpenCode
        // versions Codemux warns about. Keep in sync with
        // `docs/plans/step-12-opencode-research.md` §3.
        assert_eq!(MINIMUM_OPENCODE_VERSION, "1.14.19");
    }

    /// Tests in this module that mutate `PATH` or rely on the live
    /// `opencode` binary are serialised under the `opencode_path`
    /// label (shared with `manager.rs`'s sibling tests). Without
    /// this guard a PATH-mutating sibling test would race the
    /// live-binary tests under parallel cargo runs.
    use serial_test::serial;

    #[tokio::test]
    #[serial(opencode_path)]
    async fn check_when_binary_missing_returns_not_installed() {
        // Force PATH to a guaranteed-empty location so `which` fails.
        let original = std::env::var("PATH").unwrap_or_default();
        let tmp = tempfile::tempdir().expect("tempdir");
        // SAFETY: serial_test serialises this against any other
        // opencode test that touches PATH, so the env mutation is
        // exclusive while it's in effect.
        unsafe {
            std::env::set_var("PATH", tmp.path());
        }

        let av = check_opencode_availability(None).await;

        // Restore before asserting so a panic still leaves PATH sane.
        unsafe {
            std::env::set_var("PATH", original);
        }

        assert_eq!(av, OpenCodeAvailability::not_installed());
    }

    #[tokio::test]
    async fn check_with_unreachable_server_url_marks_server_down() {
        // No need for a binary on PATH; we're probing the server-URL
        // branch independently. Pick an obviously-unroutable URL so
        // reqwest fails fast on connect.
        // (Address chosen from RFC 5737 documentation block — never
        // routable.)
        let url = "http://192.0.2.1:1/".to_string();
        let av = check_opencode_availability(Some(url.clone())).await;

        // Whether `installed` is true or false depends on the test
        // host having opencode on PATH; we only care about the server
        // probe result.
        assert_eq!(av.server_url, Some(url));
        assert!(!av.server_running);
    }

    #[tokio::test]
    #[serial(opencode_path)]
    async fn check_against_live_binary_when_present() {
        // Live smoke: when this dev box has `opencode` on PATH, run
        // the full discovery pipeline and assert the live binary
        // reports a parseable version. Skipped (with a printed
        // breadcrumb) on machines without opencode so the test
        // remains green in CI / fresh worktrees.
        if which::which("opencode").is_err() {
            eprintln!(
                "[opencode::discovery] skipping live-binary smoke — \
                 opencode not on PATH"
            );
            return;
        }

        let av = check_opencode_availability(None).await;
        assert!(av.installed, "expected installed=true on live host");
        assert!(av.binary_path.is_some(), "binary_path must be populated");
        // Version should parse as something containing a dot — both
        // the `1.14.19` and `1.14.19-canary.0` shapes the upstream
        // banner can produce satisfy this.
        let version = av
            .version
            .as_ref()
            .expect("live opencode --version must produce a version");
        assert!(
            version.contains('.'),
            "version banner unexpectedly unparseable: {version:?}"
        );
        // server_url remained None because we passed None — the
        // server-probe branch is fully covered by the previous test.
        assert!(!av.server_running);
        assert_eq!(av.server_url, None);
    }
}
