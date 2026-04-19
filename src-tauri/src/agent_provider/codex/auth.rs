//! One-shot probes for whether the `codex` CLI is installed and
//! authenticated.
//!
//! These helpers are intentionally separate from the long-lived
//! `codex app-server` session managed by [`CodexSession`](super::session::CodexSession):
//! they shell out once, parse stdout, and return. No JSON-RPC involvement.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use crate::agent_provider::{ProviderError, ProviderKind};

/// Patterns that indicate the CLI is installed but the user is not logged
/// in. Matched case-insensitively against both stdout and stderr.
const UNAUTHENTICATED_PATTERNS: &[&str] = &[
    "not logged in",
    "run codex login",
    "not authenticated",
    "please log in",
];

/// Deadline for each subprocess probe. Keeps startup responsive even on
/// flaky CLIs.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of an auth-status probe.
#[derive(Debug, Clone)]
pub enum AuthStatus {
    /// The user is authenticated. Optional hint string (email, org, or
    /// similar) may be surfaced in the UI.
    Authenticated {
        /// Free-form, user-visible hint (e.g. an account label).
        account_hint: Option<String>,
    },
    /// The user is not authenticated. The `message` field suggests next
    /// steps.
    Unauthenticated {
        /// Suggested remediation, suitable for surfacing to the user.
        message: String,
    },
    /// Output could not be parsed. The raw combined stdout+stderr is
    /// surfaced so the UI can show the CLI's own diagnostic verbatim.
    Unknown {
        /// Raw combined output.
        raw_output: String,
    },
}

/// Run `codex --version` and return the parsed version string if the CLI
/// is on PATH. `Ok(None)` means the binary was not runnable; `Err` covers
/// timeouts and other unexpected failures.
pub async fn probe_installed(codex_binary: &Path) -> Result<Option<String>, ProviderError> {
    let result = tokio::time::timeout(
        PROBE_TIMEOUT,
        Command::new(codex_binary).arg("--version").output(),
    )
    .await;
    let output = match result {
        Ok(Ok(out)) => out,
        Ok(Err(err)) => {
            // A spawn failure with NotFound means "binary not on PATH",
            // which is the normal "not installed" signal.
            if err.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(ProviderError::ProcessError {
                message: "failed to run `codex --version`".into(),
                source: Some(err.to_string()),
            });
        }
        Err(_) => {
            return Err(ProviderError::Timeout {
                operation: "codex --version".into(),
                elapsed_ms: PROBE_TIMEOUT.as_millis() as u64,
            })
        }
    };

    if !output.status.success() {
        // Some builds exit non-zero on `--version`. Return None rather
        // than surface a spurious error.
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        Ok(Some("unknown".into()))
    } else {
        // Typical output is "codex 1.2.3" — extract the last whitespace
        // token when available, otherwise return the whole first line.
        let first_line = trimmed.lines().next().unwrap_or(trimmed);
        let token = first_line.split_whitespace().last().unwrap_or(first_line);
        Ok(Some(token.to_string()))
    }
}

/// Run `codex auth status` and classify its output.
pub async fn probe_authenticated(codex_binary: &Path) -> Result<AuthStatus, ProviderError> {
    let result = tokio::time::timeout(
        PROBE_TIMEOUT,
        Command::new(codex_binary)
            .arg("auth")
            .arg("status")
            .output(),
    )
    .await;
    let output = match result {
        Ok(Ok(out)) => out,
        Ok(Err(err)) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                return Err(ProviderError::NotInstalled {
                    provider: ProviderKind::Codex,
                    hint: "`codex` CLI not found on PATH".into(),
                });
            }
            return Err(ProviderError::ProcessError {
                message: "failed to run `codex auth status`".into(),
                source: Some(err.to_string()),
            });
        }
        Err(_) => {
            return Err(ProviderError::Timeout {
                operation: "codex auth status".into(),
                elapsed_ms: PROBE_TIMEOUT.as_millis() as u64,
            })
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    Ok(classify_auth_output(&combined))
}

/// Pure helper: classify a combined stdout+stderr blob into an
/// [`AuthStatus`]. Split out so it is unit-testable without spawning a
/// real process.
pub fn classify_auth_output(output: &str) -> AuthStatus {
    let lower = output.to_lowercase();
    for pat in UNAUTHENTICATED_PATTERNS {
        if lower.contains(pat) {
            return AuthStatus::Unauthenticated {
                message: "Codex CLI is not authenticated. Run `codex login` and try again."
                    .into(),
            };
        }
    }

    // Heuristic: if the output clearly mentions "logged in" or shows an
    // email address, treat as authenticated.
    if lower.contains("logged in") || lower.contains("authenticated") {
        let hint = output
            .lines()
            .find_map(|line| {
                let t = line.trim();
                if t.contains('@') && t.len() < 200 {
                    Some(t.to_string())
                } else {
                    None
                }
            });
        AuthStatus::Authenticated { account_hint: hint }
    } else {
        AuthStatus::Unknown {
            raw_output: output.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_not_logged_in() {
        let out = "You are not logged in. Run `codex login` to continue.";
        match classify_auth_output(out) {
            AuthStatus::Unauthenticated { message } => assert!(message.contains("codex login")),
            other => panic!("expected Unauthenticated, got {other:?}"),
        }
    }

    #[test]
    fn classifies_run_codex_login() {
        let out = "Session missing. Run codex login.";
        assert!(matches!(
            classify_auth_output(out),
            AuthStatus::Unauthenticated { .. }
        ));
    }

    #[test]
    fn classifies_not_authenticated() {
        let out = "Error: Not authenticated.";
        assert!(matches!(
            classify_auth_output(out),
            AuthStatus::Unauthenticated { .. }
        ));
    }

    #[test]
    fn classifies_authenticated_with_email_hint() {
        let out = "Logged in as\n  test@example.com\n";
        match classify_auth_output(out) {
            AuthStatus::Authenticated { account_hint } => {
                assert!(account_hint.as_deref().unwrap().contains("@"))
            }
            other => panic!("expected Authenticated, got {other:?}"),
        }
    }

    #[test]
    fn unknown_output_is_surfaced_raw() {
        let out = "some unrelated output\n";
        match classify_auth_output(out) {
            AuthStatus::Unknown { raw_output } => assert_eq!(raw_output, out),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }
}
