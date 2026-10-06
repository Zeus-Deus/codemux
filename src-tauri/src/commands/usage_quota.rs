//! On-demand plan-quota reads for Settings → Usage → Limits.
//!
//! The [`PlanQuotaStore`] otherwise only fills while a session runs: Claude
//! streams `rate_limit_event`s mid-turn and Codex reads its limits at session
//! start. Opening the Usage page right after launch would show no limits at
//! all. This module asks each provider directly, through the same channels a
//! session uses — the Claude CLI's `get_usage` control request (via the
//! sidecar) and the Codex app-server's `account/rateLimits/read` — and records
//! the answers in the store.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;

use crate::agent_provider::codex::protocol::{
    AccountReadResponse, Capabilities, ClientInfo, GetAccountRateLimitsResponse,
    InitializeParams,
};
use crate::agent_provider::{
    PlanAuthMode, PlanUsageWindow, PlanWindowKind, FIVE_HOUR_WINDOW_MINS,
    SEVEN_DAY_WINDOW_MINS,
};
use crate::commands::usage::{PlanQuotaStore, ProviderQuota};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

/// Upper bound on one provider read, spawn included. A probe that hangs must
/// not hold the refresh button forever.
const PROBE_TIMEOUT: Duration = Duration::from_secs(25);

/// What one provider read produced.
#[derive(Debug, Clone, PartialEq)]
struct QuotaReading {
    windows: Vec<PlanUsageWindow>,
    plan_label: Option<String>,
    auth_mode: Option<PlanAuthMode>,
}

/// How one provider's read went, so the page can tell "no limits apply"
/// from "could not read limits" instead of showing nothing for both.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaProbeOutcome {
    /// Windows were read and recorded.
    Ok,
    /// The provider answered, but no plan limits apply (API key, or a CLI
    /// too old to report them).
    Unavailable,
    /// The read itself failed.
    Failed,
    /// The provider's CLI is not on PATH; it has nothing to report.
    NotInstalled,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaProbeStatus {
    pub provider: String,
    pub outcome: QuotaProbeOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaRefreshReport {
    /// Every provider's newest reading after this refresh.
    pub quota: HashMap<String, ProviderQuota>,
    pub statuses: Vec<QuotaProbeStatus>,
    pub refreshed_at_ms: i64,
}

enum ProbeError {
    NotInstalled,
    Unavailable(String),
    Failed(String),
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Serializes refreshes: a second caller waits for the read in flight and
/// reuses its result instead of spawning another pair of CLIs.
static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Read Claude and Codex plan limits now and record them.
#[tauri::command]
pub async fn usage_refresh_quota(
    quota: State<'_, PlanQuotaStore>,
) -> Result<QuotaRefreshReport, String> {
    let guard = match REFRESH_LOCK.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            // Another refresh is running; its answer is as fresh as ours
            // would be.
            let _wait = REFRESH_LOCK.lock().await;
            return Ok(QuotaRefreshReport {
                quota: quota.snapshot(),
                statuses: Vec::new(),
                refreshed_at_ms: now_ms(),
            });
        }
    };

    let (claude, codex) = tokio::join!(
        with_timeout(probe_claude()),
        with_timeout(probe_codex())
    );
    let now = now_ms();
    let statuses = vec![
        settle(&quota, "claude", claude, now),
        settle(&quota, "codex", codex, now),
    ];
    drop(guard);
    Ok(QuotaRefreshReport {
        quota: quota.snapshot(),
        statuses,
        refreshed_at_ms: now,
    })
}

async fn with_timeout(
    probe: impl std::future::Future<Output = Result<QuotaReading, ProbeError>>,
) -> Result<QuotaReading, ProbeError> {
    tokio::time::timeout(PROBE_TIMEOUT, probe)
        .await
        .unwrap_or_else(|_| Err(ProbeError::Failed("timed out".into())))
}

/// Record a reading and describe the outcome.
fn settle(
    store: &PlanQuotaStore,
    provider: &str,
    result: Result<QuotaReading, ProbeError>,
    now: i64,
) -> QuotaProbeStatus {
    let (outcome, message) = match result {
        Ok(reading) => {
            let has_windows = !reading.windows.is_empty();
            store.replace_windows(
                provider,
                reading.windows,
                reading.plan_label,
                reading.auth_mode,
                now,
            );
            if has_windows {
                (QuotaProbeOutcome::Ok, None)
            } else {
                (
                    QuotaProbeOutcome::Unavailable,
                    Some(match reading.auth_mode {
                        Some(PlanAuthMode::ApiKey) => {
                            "Signed in with an API key, so no plan limits apply.".to_string()
                        }
                        _ => "No plan limits reported for this account.".to_string(),
                    }),
                )
            }
        }
        Err(ProbeError::NotInstalled) => (QuotaProbeOutcome::NotInstalled, None),
        Err(ProbeError::Unavailable(message)) => {
            (QuotaProbeOutcome::Unavailable, Some(message))
        }
        Err(ProbeError::Failed(message)) => {
            eprintln!("[codemux::usage_quota] {provider} limits read failed: {message}");
            (QuotaProbeOutcome::Failed, Some(message))
        }
    };
    QuotaProbeStatus {
        provider: provider.to_string(),
        outcome,
        message,
    }
}

// ── Claude ──

async fn probe_claude() -> Result<QuotaReading, ProbeError> {
    let claude_binary = which::which("claude").map_err(|_| ProbeError::NotInstalled)?;
    let sidecar = crate::agent_provider::claude::sidecar_path::resolve_sidecar_path()
        .map_err(|e| ProbeError::Failed(format!("Claude sidecar unavailable: {e:?}")))?;
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: sidecar,
        args: vec![],
        env: HashMap::new(),
        cwd: None,
        default_timeout: PROBE_TIMEOUT,
    })
    .await
    .map_err(|e| ProbeError::Failed(format!("sidecar spawn: {e}")))?;
    let response = child
        .request(
            "get-usage",
            json!({
                "cwd": std::env::temp_dir().to_string_lossy(),
                "pathToClaudeCodeExecutable": claude_binary.to_string_lossy(),
            }),
        )
        .await;
    let _ = child.shutdown().await;
    let response = response.map_err(|e| {
        let message = e.to_string();
        // A CLI that predates `get_usage` refuses the subtype; that is a
        // version gap, not a broken install.
        if message.contains("get_usage") || message.contains("Unsupported") {
            ProbeError::Unavailable("Update Claude Code to read plan limits.".into())
        } else {
            ProbeError::Failed(message)
        }
    })?;
    Ok(claude_reading_from_get_usage(&response))
}

/// Map the CLI's `get_usage` answer to quota windows.
///
/// Unlike the streamed `rate_limit_event` (a 0..1 fraction with an
/// epoch-seconds reset), `get_usage` reports utilization as a 0–100 percent
/// and resets as ISO-8601 strings.
fn claude_reading_from_get_usage(response: &Value) -> QuotaReading {
    let subscription = response
        .get("subscription_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let available = response
        .get("rate_limits_available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let plan_label = subscription.map(claude_plan_label);
    let auth_mode = if subscription.is_some() || available {
        Some(PlanAuthMode::Subscription)
    } else {
        Some(PlanAuthMode::ApiKey)
    };

    let mut windows = Vec::new();
    let Some(limits) = response.get("rate_limits").filter(|v| v.is_object()) else {
        return QuotaReading {
            windows,
            plan_label,
            auth_mode,
        };
    };

    for (key, kind, mins) in [
        ("five_hour", PlanWindowKind::FiveHour, FIVE_HOUR_WINDOW_MINS),
        ("seven_day", PlanWindowKind::SevenDay, SEVEN_DAY_WINDOW_MINS),
        ("seven_day_opus", PlanWindowKind::SevenDayOpus, SEVEN_DAY_WINDOW_MINS),
        ("seven_day_sonnet", PlanWindowKind::SevenDaySonnet, SEVEN_DAY_WINDOW_MINS),
    ] {
        if let Some(window) = percent_window(limits.get(key), kind, Some(key.into()), Some(mins)) {
            windows.push(window);
        }
    }

    // Model-scoped weeklies (newer CLIs) carry their model's display name.
    if let Some(scoped) = limits.get("model_scoped").and_then(Value::as_array) {
        for entry in scoped {
            let Some(name) = entry.get("display_name").and_then(Value::as_str) else {
                continue;
            };
            if let Some(window) = percent_window(
                Some(entry),
                PlanWindowKind::Other,
                Some(format!("Weekly · {name}")),
                Some(SEVEN_DAY_WINDOW_MINS),
            ) {
                windows.push(window);
            }
        }
    }

    // Paid extra usage is a monthly credit pool, not a rolling window.
    if let Some(extra) = limits.get("extra_usage").filter(|v| {
        v.get("is_enabled").and_then(Value::as_bool).unwrap_or(false)
    }) {
        if let Some(window) = percent_window(
            Some(extra),
            PlanWindowKind::Overage,
            Some("Extra usage".into()),
            None,
        ) {
            windows.push(window);
        }
    }

    QuotaReading {
        windows,
        plan_label,
        auth_mode,
    }
}

/// One `{ utilization, resets_at }` entry, or `None` when it carries no
/// figure (a `null` utilization means the window does not apply).
fn percent_window(
    entry: Option<&Value>,
    kind: PlanWindowKind,
    label: Option<String>,
    window_mins: Option<i64>,
) -> Option<PlanUsageWindow> {
    let entry = entry?;
    let used_pct = entry.get("utilization").and_then(Value::as_f64)?;
    if !used_pct.is_finite() {
        return None;
    }
    let resets_at_ms = entry
        .get("resets_at")
        .and_then(Value::as_str)
        .and_then(|iso| chrono::DateTime::parse_from_rfc3339(iso).ok())
        .map(|at| at.timestamp_millis());
    Some(PlanUsageWindow {
        kind,
        used_pct: used_pct.clamp(0.0, 100.0),
        resets_at_ms,
        label,
        window_mins,
    })
}

fn claude_plan_label(subscription: &str) -> String {
    match subscription.to_ascii_lowercase().as_str() {
        "pro" => "Claude Pro".into(),
        "max" => "Claude Max".into(),
        "team" => "Claude Team".into(),
        "enterprise" => "Claude Enterprise".into(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => format!("Claude {}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => "Claude".into(),
            }
        }
    }
}

// ── Codex ──

async fn probe_codex() -> Result<QuotaReading, ProbeError> {
    let binary: PathBuf = which::which("codex").map_err(|_| ProbeError::NotInstalled)?;
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: binary,
        args: vec!["app-server".into()],
        env: HashMap::new(),
        cwd: None,
        default_timeout: PROBE_TIMEOUT,
    })
    .await
    .map_err(|e| ProbeError::Failed(format!("codex app-server spawn: {e}")))?;
    let result = read_codex_limits(&child).await;
    let _ = child.shutdown().await;
    result
}

async fn read_codex_limits(child: &JsonRpcChild) -> Result<QuotaReading, ProbeError> {
    let init = serde_json::to_value(InitializeParams {
        client_info: ClientInfo {
            name: "codemux-usage-limits".into(),
            title: "Codemux".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        capabilities: Capabilities {
            experimental_api: true,
        },
    })
    .map_err(|e| ProbeError::Failed(format!("initialize serialize: {e}")))?;
    child
        .request("initialize", init)
        .await
        .map_err(|e| ProbeError::Failed(format!("initialize: {e}")))?;
    child
        .notify("initialized", json!({}))
        .await
        .map_err(|e| ProbeError::Failed(format!("initialized: {e}")))?;

    let account: AccountReadResponse = child
        .request("account/read", json!({}))
        .await
        .map_err(|e| ProbeError::Failed(format!("account/read: {e}")))
        .and_then(|v| {
            serde_json::from_value(v)
                .map_err(|e| ProbeError::Failed(format!("account/read decode: {e}")))
        })?;
    if account.needs_login() {
        return Err(ProbeError::Unavailable(
            "Run `codex login` to read plan limits.".into(),
        ));
    }
    let info = account.account.as_ref();
    let auth_mode = match info.and_then(|a| a.account_type.as_deref()) {
        Some("apiKey") => Some(PlanAuthMode::ApiKey),
        Some("chatgpt") | Some("chatgptDeviceCode") => Some(PlanAuthMode::Subscription),
        _ => None,
    };
    let plan_label = crate::agent_provider::codex::session::codex_plan_label(
        info.and_then(|a| a.plan_type.as_deref()),
    );
    if auth_mode == Some(PlanAuthMode::ApiKey) {
        return Ok(QuotaReading {
            windows: Vec::new(),
            plan_label,
            auth_mode,
        });
    }

    let limits: GetAccountRateLimitsResponse = child
        .request("account/rateLimits/read", json!({}))
        .await
        .map_err(|e| {
            ProbeError::Unavailable(format!("This Codex build cannot report plan limits ({e})."))
        })
        .and_then(|v| {
            serde_json::from_value(v)
                .map_err(|e| ProbeError::Failed(format!("account/rateLimits/read decode: {e}")))
        })?;
    Ok(QuotaReading {
        windows: crate::agent_provider::codex::translate::plan_windows_from_rate_limits(
            &limits.rate_limits,
        ),
        // The plan type names the subscription; the snapshot's
        // `limit_name` is only a fallback.
        plan_label: plan_label.or(limits.rate_limits.limit_name),
        auth_mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_usage_maps_every_reported_window() {
        let response = json!({
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": {
                "five_hour": { "utilization": 41.0, "resets_at": "2026-10-06T20:00:00Z" },
                "seven_day": { "utilization": 88.4, "resets_at": "2026-10-09T08:00:00+00:00" },
                "seven_day_opus": { "utilization": 72.0, "resets_at": null },
                "seven_day_sonnet": null,
                "model_scoped": [
                    { "display_name": "Fable", "utilization": 12.0, "resets_at": null }
                ],
                "extra_usage": { "is_enabled": false, "utilization": 3.0 }
            }
        });
        let reading = claude_reading_from_get_usage(&response);
        assert_eq!(reading.plan_label.as_deref(), Some("Claude Max"));
        assert_eq!(reading.auth_mode, Some(PlanAuthMode::Subscription));
        let kinds: Vec<_> = reading.windows.iter().map(|w| w.kind).collect();
        assert_eq!(
            kinds,
            vec![
                PlanWindowKind::FiveHour,
                PlanWindowKind::SevenDay,
                PlanWindowKind::SevenDayOpus,
                PlanWindowKind::Other,
            ]
        );
        let five = &reading.windows[0];
        // Already a percent on this path — no ×100.
        assert_eq!(five.used_pct, 41.0);
        assert_eq!(five.resets_at_ms, Some(1_791_316_800_000));
        assert_eq!(five.window_mins, Some(300));
        assert_eq!(reading.windows[3].label.as_deref(), Some("Weekly · Fable"));
    }

    #[test]
    fn get_usage_includes_enabled_extra_usage_as_overage() {
        let response = json!({
            "subscription_type": "pro",
            "rate_limits_available": true,
            "rate_limits": {
                "extra_usage": { "is_enabled": true, "utilization": 140.0 }
            }
        });
        let reading = claude_reading_from_get_usage(&response);
        assert_eq!(reading.windows.len(), 1);
        assert_eq!(reading.windows[0].kind, PlanWindowKind::Overage);
        assert_eq!(reading.windows[0].used_pct, 100.0);
        assert_eq!(reading.windows[0].window_mins, None);
    }

    #[test]
    fn get_usage_without_a_plan_reads_as_api_key() {
        let response = json!({
            "subscription_type": null,
            "rate_limits_available": false,
            "rate_limits": null
        });
        let reading = claude_reading_from_get_usage(&response);
        assert!(reading.windows.is_empty());
        assert_eq!(reading.plan_label, None);
        assert_eq!(reading.auth_mode, Some(PlanAuthMode::ApiKey));
    }

    #[test]
    fn null_utilization_is_skipped_not_zeroed() {
        let response = json!({
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": { "five_hour": { "utilization": null, "resets_at": null } }
        });
        assert!(claude_reading_from_get_usage(&response).windows.is_empty());
    }

    #[test]
    fn unknown_subscription_types_are_humanized() {
        assert_eq!(claude_plan_label("ultra"), "Claude Ultra");
        assert_eq!(claude_plan_label("Team"), "Claude Team");
    }

    #[test]
    fn settle_records_windows_and_reports_api_key_accounts() {
        let store = PlanQuotaStore::default();
        let status = settle(
            &store,
            "claude",
            Ok(QuotaReading {
                windows: vec![PlanUsageWindow {
                    kind: PlanWindowKind::FiveHour,
                    used_pct: 10.0,
                    resets_at_ms: None,
                    label: None,
                    window_mins: Some(300),
                }],
                plan_label: Some("Claude Max".into()),
                auth_mode: Some(PlanAuthMode::Subscription),
            }),
            5,
        );
        assert_eq!(status.outcome, QuotaProbeOutcome::Ok);
        assert_eq!(store.snapshot()["claude"].windows.len(), 1);

        let status = settle(
            &store,
            "codex",
            Ok(QuotaReading {
                windows: Vec::new(),
                plan_label: None,
                auth_mode: Some(PlanAuthMode::ApiKey),
            }),
            5,
        );
        assert_eq!(status.outcome, QuotaProbeOutcome::Unavailable);
        assert!(status.message.unwrap().contains("API key"));
    }
}
