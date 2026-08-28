//! Cross-provider runtime health reporting.
//!
//! Every chat provider is backed by a local CLI/runtime that can be
//! missing, broken, or unauthenticated — and until now the chat UI only
//! learned about that indirectly (a wedged "Working…" spinner, or a
//! stall notice many minutes later). This module gives each provider a
//! uniform, probe-backed health snapshot the frontend can render as a
//! status banner *before* the user burns time on a dead session.
//!
//! The probes themselves already existed per provider (install/auth
//! checks used at session start); this module only maps their outcomes
//! onto one wire shape. Mapping functions are pure and unit-tested;
//! the async entry points just run the probes and delegate.

use serde::{Deserialize, Serialize};

use super::errors::ProviderError;
use super::types::ProviderKind;
use crate::agent_provider::claude;
use crate::agent_provider::codex;
use crate::agent_provider::hermes;
use crate::agent_provider::opencode;
use std::time::Duration;

/// Coarse severity for a provider health snapshot.
///
/// `Warning` renders as an amber advisory (probe inconclusive), `Error`
/// as a red banner (provider cannot run a session as-is).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthStatus {
    Ready,
    Warning,
    Error,
}

/// Probe-backed snapshot of one provider's local runtime.
///
/// Consumed verbatim by the frontend (`src/tauri/types.ts` mirrors the
/// field names) — add fields, do not rename.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealthReport {
    pub provider: ProviderKind,
    pub status: ProviderHealthStatus,
    /// False when the CLI/runtime binary could not be found at all.
    pub installed: bool,
    /// Human-readable explanation when not `Ready`; `None` when `Ready`.
    pub message: Option<String>,
    /// CLI version when a probe surfaced one. Best-effort.
    pub version: Option<String>,
}

impl ProviderHealthReport {
    fn ready(provider: ProviderKind, version: Option<String>) -> Self {
        Self {
            provider,
            status: ProviderHealthStatus::Ready,
            installed: true,
            message: None,
            version,
        }
    }

    fn error(provider: ProviderKind, installed: bool, message: String) -> Self {
        Self {
            provider,
            status: ProviderHealthStatus::Error,
            installed,
            message: Some(message),
            version: None,
        }
    }

    fn warning(provider: ProviderKind, version: Option<String>, message: String) -> Self {
        Self {
            provider,
            status: ProviderHealthStatus::Warning,
            installed: true,
            message: Some(message),
            version,
        }
    }
}

/// Probe the health of `provider`'s local runtime. Never errors — a
/// failed probe IS the answer, folded into the report.
pub async fn check_provider_health(provider: ProviderKind) -> ProviderHealthReport {
    match provider {
        ProviderKind::Claude => check_claude_health().await,
        ProviderKind::Codex => check_codex_health().await,
        ProviderKind::Cursor => check_cursor_health().await,
        ProviderKind::OpenCode => check_opencode_health().await,
        ProviderKind::Hermes => check_hermes_health().await,
    }
}

// ── Claude ──────────────────────────────────────────────────────────

async fn check_claude_health() -> ProviderHealthReport {
    // The Rust side never shells the `claude` binary directly (ToS
    // boundary); both probes go through the bundled sidecar.
    let sidecar = match claude::sidecar_path::resolve_sidecar_path() {
        Ok(path) => path,
        Err(err) => {
            return ProviderHealthReport::error(
                ProviderKind::Claude,
                false,
                format!("Claude integration is unavailable: {err}"),
            )
        }
    };
    let installed = claude::auth::probe_installed(&sidecar, None).await;
    let installed = match installed {
        Ok(result) => result,
        Err(err) => return claude_probe_failure_report(&err),
    };
    if !installed.installed {
        return ProviderHealthReport::error(
            ProviderKind::Claude,
            false,
            "Claude Code CLI (`claude`) is not installed or not on PATH.".into(),
        );
    }
    let auth = claude::auth::probe_authenticated(&sidecar, None).await;
    claude_report_from_auth(auth, installed.version)
}

/// Map a failed install probe (the sidecar itself would not run) onto a
/// report. Pure; unit-tested.
fn claude_probe_failure_report(err: &ProviderError) -> ProviderHealthReport {
    match err {
        ProviderError::NotInstalled { hint, .. } => ProviderHealthReport::error(
            ProviderKind::Claude,
            false,
            format!("Claude integration is unavailable: {hint}"),
        ),
        other => ProviderHealthReport::error(
            ProviderKind::Claude,
            true,
            format!("Claude Agent runtime is installed but failed to run. ({other})"),
        ),
    }
}

/// Map an auth probe outcome onto a report. Pure; unit-tested.
fn claude_report_from_auth(
    auth: Result<claude::auth::AuthStatus, ProviderError>,
    version: Option<String>,
) -> ProviderHealthReport {
    match auth {
        Ok(claude::auth::AuthStatus::Authenticated) => {
            ProviderHealthReport::ready(ProviderKind::Claude, version)
        }
        Ok(claude::auth::AuthStatus::Unauthenticated { message }) => {
            let mut report = ProviderHealthReport::error(ProviderKind::Claude, true, message);
            report.version = version;
            report
        }
        // Inconclusive output / probe failure after a confirmed install:
        // advisory only — sessions may still work (e.g. API-key auth the
        // probe can't see), so don't block with a red banner.
        Ok(claude::auth::AuthStatus::Unknown { .. }) => ProviderHealthReport::warning(
            ProviderKind::Claude,
            version,
            "Could not verify Claude authentication status.".into(),
        ),
        Err(err) => ProviderHealthReport::warning(
            ProviderKind::Claude,
            version,
            format!("Could not verify Claude authentication status. ({err})"),
        ),
    }
}

// ── Codex ───────────────────────────────────────────────────────────

async fn check_codex_health() -> ProviderHealthReport {
    let binary = match which::which("codex") {
        Ok(path) => path,
        Err(_) => {
            return ProviderHealthReport::error(
                ProviderKind::Codex,
                false,
                "Codex CLI (`codex`) is not installed or not on PATH.".into(),
            )
        }
    };
    let version = match codex::auth::probe_installed(&binary).await {
        // On PATH but `--version` exited non-zero. `probe_installed`
        // documents this as benign — some Codex builds simply do that —
        // so it is an advisory, not a dead provider.
        Ok(None) => return codex_version_unavailable_report(),
        Ok(Some(version)) => Some(version),
        Err(err) => {
            return ProviderHealthReport::error(
                ProviderKind::Codex,
                true,
                format!("Codex CLI is installed but failed to run. ({err})"),
            )
        }
    };
    codex_report_from_auth(codex::auth::probe_authenticated(&binary).await, version)
}

/// `codex --version` exited non-zero on a binary that IS on PATH.
///
/// `codex::auth::probe_installed` deliberately returns `Ok(None)` here
/// rather than an error because some builds behave this way and still
/// run sessions fine. Reporting it as `Error` painted a red "cannot
/// run" banner over a working install, so it is a `Warning`: say what
/// the probe could not determine, and don't claim the CLI is broken.
/// Pure; unit-tested.
fn codex_version_unavailable_report() -> ProviderHealthReport {
    ProviderHealthReport::warning(
        ProviderKind::Codex,
        None,
        "Codex CLI did not report a version (`codex --version` failed). Sessions may still work."
            .into(),
    )
}

/// Map a Codex auth probe outcome onto a report. Pure; unit-tested.
fn codex_report_from_auth(
    auth: Result<codex::auth::AuthStatus, ProviderError>,
    version: Option<String>,
) -> ProviderHealthReport {
    match auth {
        Ok(codex::auth::AuthStatus::Authenticated { .. }) => {
            ProviderHealthReport::ready(ProviderKind::Codex, version)
        }
        Ok(codex::auth::AuthStatus::Unauthenticated { message }) => {
            let mut report = ProviderHealthReport::error(ProviderKind::Codex, true, message);
            report.version = version;
            report
        }
        Ok(codex::auth::AuthStatus::Unknown { .. }) => ProviderHealthReport::warning(
            ProviderKind::Codex,
            version,
            "Could not verify Codex authentication status.".into(),
        ),
        Err(err) => ProviderHealthReport::warning(
            ProviderKind::Codex,
            version,
            format!("Could not verify Codex authentication status. ({err})"),
        ),
    }
}

// ── Cursor ──────────────────────────────────────────────────────────

async fn check_cursor_health() -> ProviderHealthReport {
    let binary = match which::which("cursor-agent") {
        Ok(path) => path,
        Err(_) => {
            return ProviderHealthReport::error(
                ProviderKind::Cursor,
                false,
                "Cursor Agent CLI (`cursor-agent`) is not installed or not on PATH.".into(),
            )
        }
    };
    let version = match tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(&binary)
            .arg("--version")
            .output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!text.is_empty()).then_some(text)
        }
        Ok(Ok(_)) => None,
        Ok(Err(error)) => {
            return ProviderHealthReport::error(
                ProviderKind::Cursor,
                true,
                format!("Cursor Agent is installed but failed to run. ({error})"),
            )
        }
        Err(_) => {
            return ProviderHealthReport::warning(
                ProviderKind::Cursor,
                None,
                "Cursor Agent version probe timed out. Sessions may still work.".into(),
            )
        }
    };

    // `models` is an official, read-only command and exercises both the
    // installation and current login without opening an interactive flow.
    match tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(&binary).arg("models").output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => {
            ProviderHealthReport::ready(ProviderKind::Cursor, version)
        }
        Ok(Ok(output)) => {
            let detail = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if crate::agent_provider::cursor::capabilities::looks_unauthenticated(&detail) {
                let mut report = ProviderHealthReport::error(
                    ProviderKind::Cursor,
                    true,
                    "Cursor Agent is not authenticated. Run `cursor-agent login` and try again."
                        .into(),
                );
                report.version = version;
                report
            } else {
                ProviderHealthReport::warning(
                    ProviderKind::Cursor,
                    version,
                    "Cursor Agent model probe failed; sessions may still work.".into(),
                )
            }
        }
        Ok(Err(error)) => ProviderHealthReport::error(
            ProviderKind::Cursor,
            true,
            format!("Cursor Agent is installed but failed to run. ({error})"),
        ),
        Err(_) => ProviderHealthReport::warning(
            ProviderKind::Cursor,
            version,
            "Cursor Agent authentication probe timed out. Sessions may still work.".into(),
        ),
    }
}

// ── Hermes ──────────────────────────────────────────────────────────

async fn check_hermes_health() -> ProviderHealthReport {
    // `hermes acp --check` is the whole probe: it exercises the same ACP
    // entry point a chat session launches and returns in well under a
    // second, so there is nothing cheaper worth splitting out.
    let availability = hermes::check_hermes_availability().await;
    hermes_report_from_availability(&availability)
}

/// Pure mapping from the Hermes probe onto the wire report, so the
/// classification is unit-testable without shelling anything.
fn hermes_report_from_availability(
    availability: &hermes::HermesAvailability,
) -> ProviderHealthReport {
    let Some(message) = availability.message.clone() else {
        return ProviderHealthReport::ready(ProviderKind::Hermes, availability.version.clone());
    };
    if availability.binary.is_none() {
        return ProviderHealthReport::error(ProviderKind::Hermes, false, message);
    }
    // The binary is present but the self-check did not come back clean.
    // Warning, not error: the probe is advisory and a session may still
    // start, so this must not black out a provider the user can use.
    ProviderHealthReport::warning(ProviderKind::Hermes, availability.version.clone(), message)
}

// ── OpenCode ────────────────────────────────────────────────────────

async fn check_opencode_health() -> ProviderHealthReport {
    let availability = opencode::check_opencode_availability(None).await;
    opencode_report_from_availability(&availability)
}

/// Map an OpenCode availability probe onto a report. Pure; unit-tested.
fn opencode_report_from_availability(
    availability: &opencode::OpenCodeAvailability,
) -> ProviderHealthReport {
    if !availability.installed {
        return ProviderHealthReport::error(
            ProviderKind::OpenCode,
            false,
            "OpenCode CLI (`opencode`) is not installed or not on PATH.".into(),
        );
    }
    // Installed but `--version` failed: the server spawn will very
    // likely fail the same way, so surface it now.
    match &availability.version {
        None => ProviderHealthReport::error(
            ProviderKind::OpenCode,
            true,
            "OpenCode CLI is installed but failed to run.".into(),
        ),
        Some(version) => {
            ProviderHealthReport::ready(ProviderKind::OpenCode, Some(version.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::opencode::OpenCodeAvailability;

    #[test]
    fn hermes_missing_binary_maps_to_not_installed_error() {
        let report = hermes_report_from_availability(&hermes::HermesAvailability {
            binary: None,
            acp_ready: false,
            version: None,
            message: Some("Hermes CLI (`hermes`) is not installed or not on PATH.".into()),
        });
        assert_eq!(report.status, ProviderHealthStatus::Error);
        assert!(!report.installed);
    }

    #[test]
    fn hermes_failing_self_check_stays_a_warning() {
        // Installed-but-inconclusive must not black the provider out: the
        // probe is advisory and a session may still start.
        let report = hermes_report_from_availability(&hermes::HermesAvailability {
            binary: Some(std::path::PathBuf::from("/usr/bin/hermes")),
            acp_ready: false,
            version: Some("0.20.6".into()),
            message: Some("Hermes is installed but its ACP self-check failed.".into()),
        });
        assert_eq!(report.status, ProviderHealthStatus::Warning);
        assert!(report.installed);
        assert_eq!(report.version.as_deref(), Some("0.20.6"));
    }

    #[test]
    fn hermes_clean_probe_maps_to_ready_with_version() {
        let report = hermes_report_from_availability(&hermes::HermesAvailability {
            binary: Some(std::path::PathBuf::from("/usr/bin/hermes")),
            acp_ready: true,
            version: Some("0.20.6".into()),
            message: None,
        });
        assert_eq!(report.status, ProviderHealthStatus::Ready);
        assert!(report.message.is_none());
        assert_eq!(report.version.as_deref(), Some("0.20.6"));
    }

    #[test]
    fn claude_sidecar_missing_maps_to_not_installed_error() {
        let report = claude_probe_failure_report(&ProviderError::NotInstalled {
            provider: ProviderKind::Claude,
            hint: "sidecar binary not found".into(),
        });
        assert_eq!(report.status, ProviderHealthStatus::Error);
        assert!(!report.installed);
        assert!(report.message.as_deref().unwrap().contains("unavailable"));
    }

    #[test]
    fn claude_sidecar_broken_maps_to_installed_but_failed() {
        let report = claude_probe_failure_report(&ProviderError::ProcessError {
            message: "failed to spawn claude-agent sidecar for probe".into(),
            source: Some("Permission denied".into()),
        });
        assert_eq!(report.status, ProviderHealthStatus::Error);
        assert!(report.installed);
        assert!(report
            .message
            .as_deref()
            .unwrap()
            .contains("installed but failed to run"));
    }

    #[test]
    fn claude_unauthenticated_maps_to_error_with_probe_message() {
        let report = claude_report_from_auth(
            Ok(claude::auth::AuthStatus::Unauthenticated {
                message: "Run `claude login`.".into(),
            }),
            Some("2.1.0".into()),
        );
        assert_eq!(report.status, ProviderHealthStatus::Error);
        assert!(report.installed);
        assert_eq!(report.message.as_deref(), Some("Run `claude login`."));
        assert_eq!(report.version.as_deref(), Some("2.1.0"));
    }

    #[test]
    fn claude_auth_probe_failure_is_only_a_warning() {
        let report = claude_report_from_auth(
            Err(ProviderError::Timeout {
                operation: "probe-authenticated".into(),
                elapsed_ms: 10_000,
            }),
            None,
        );
        assert_eq!(report.status, ProviderHealthStatus::Warning);
        assert!(report.installed);
    }

    #[test]
    fn claude_authenticated_is_ready_with_no_message() {
        let report =
            claude_report_from_auth(Ok(claude::auth::AuthStatus::Authenticated), Some("2.1.0".into()));
        assert_eq!(report.status, ProviderHealthStatus::Ready);
        assert!(report.message.is_none());
    }

    #[test]
    fn codex_unauthenticated_maps_to_error() {
        let report = codex_report_from_auth(
            Ok(codex::auth::AuthStatus::Unauthenticated {
                message: "Run `codex login`.".into(),
            }),
            Some("1.2.3".into()),
        );
        assert_eq!(report.status, ProviderHealthStatus::Error);
        assert_eq!(report.message.as_deref(), Some("Run `codex login`."));
    }

    #[test]
    fn codex_unreadable_version_is_only_a_warning() {
        // `--version` exiting non-zero is documented-benign for some
        // Codex builds; it must not paint a red "cannot run" banner.
        let report = codex_version_unavailable_report();
        assert_eq!(report.status, ProviderHealthStatus::Warning);
        assert!(report.installed);
        assert!(report.message.as_deref().unwrap().contains("version"));
    }

    #[test]
    fn opencode_missing_binary_maps_to_not_installed() {
        let report = opencode_report_from_availability(&OpenCodeAvailability::not_installed());
        assert_eq!(report.status, ProviderHealthStatus::Error);
        assert!(!report.installed);
    }

    #[test]
    fn opencode_unrunnable_binary_maps_to_installed_but_failed() {
        let availability = OpenCodeAvailability {
            installed: true,
            binary_path: None,
            version: None,
            server_running: false,
            server_url: None,
        };
        let report = opencode_report_from_availability(&availability);
        assert_eq!(report.status, ProviderHealthStatus::Error);
        assert!(report.installed);
    }
}
