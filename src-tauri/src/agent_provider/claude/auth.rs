//! Auth / install probes for the Claude provider.
//!
//! Both probes work by spawning a short-lived sidecar, invoking one
//! of its probe RPCs (`probe-installed` / `probe-authenticated`),
//! reading the response, and shutting the sidecar down. The sidecar
//! in turn shells out to the user's local `claude` binary; the Rust
//! side never touches it directly — that's the ToS boundary this
//! whole path exists to protect.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde_json::json;

use crate::agent_provider::{ProviderError, ProviderKind};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

use super::protocol::{
    ProbeAuthenticatedResponse, ProbeInstalledResponse, METHOD_PROBE_AUTHENTICATED,
    METHOD_PROBE_INSTALLED,
};

/// Ceiling for the whole probe round trip (spawn + RPC + shutdown).
const PROBE_DEADLINE: Duration = Duration::from_secs(10);

/// settings.json keys naming commands that fetch credentials. They are
/// commands, not secrets, so helpers pass them through `--settings`.
/// Keep in step with `ROUTING_SETTING_KEYS` in sidecar/claude-agent/src/user-routing.ts.
pub(crate) const CLAUDE_ROUTING_SETTING_KEYS: &[&str] = &[
    "apiKeyHelper",
    "awsCredentialExport",
    "awsAuthRefresh",
    "gcpAuthRefresh",
];

/// Provider routing accepted by Claude Code. Helpers forward only these
/// settings in the child environment so user instructions stay isolated.
/// Keep in step with `ROUTING_ENV_KEYS` in sidecar/claude-agent/src/user-routing.ts.
pub(crate) const CLAUDE_ROUTING_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
    "CLAUDE_CODE_SKIP_VERTEX_AUTH",
    "CLAUDE_CODE_SKIP_FOUNDRY_AUTH",
    "CLAUDE_CODE_SKIP_MANTLE_AUTH",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "ANTHROPIC_FOUNDRY_RESOURCE",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
    "CLOUD_ML_REGION",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_PROFILE",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_SHARED_CREDENTIALS_FILE",
    "AWS_CONFIG_FILE",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "NODE_EXTRA_CA_CERTS",
    "CLAUDE_CODE_CLIENT_CERT",
    "CLAUDE_CODE_CLIENT_KEY",
    "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE",
    "API_TIMEOUT_MS",
];

pub(crate) fn settings_routing_env(settings: Option<&serde_json::Value>) -> Vec<(String, String)> {
    let env = settings.and_then(|settings| settings.get("env"));
    CLAUDE_ROUTING_ENV_KEYS
        .iter()
        .filter_map(|key| {
            let value = routing_env_value(env?.get(*key)?)?;
            Some(((*key).to_string(), value))
        })
        .collect()
}

/// Claude Code stringifies scalar settings env values before spawning.
pub(crate) fn routing_env_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => Some(value.to_string()),
        _ => None,
    }
}

/// Result of a `probe_installed` call.
#[derive(Debug, Clone)]
pub struct ProbeInstalledResult {
    pub installed: bool,
    pub version: Option<String>,
    /// The CLI exists but did not answer `--version` cleanly.
    pub unresponsive: bool,
}

/// Coarse authentication status.
#[derive(Debug, Clone)]
pub enum AuthStatus {
    /// CLI is installed AND the user is logged in.
    Authenticated { auth_label: Option<String> },
    /// CLI is installed but not logged in. `message` describes the
    /// remediation path.
    Unauthenticated { message: String },
    /// Output couldn't be classified — surface the raw text.
    Unknown {
        raw: String,
        auth_label: Option<String>,
    },
}

/// Spawn the sidecar, call `probe-installed`, shut down.
pub async fn probe_installed(
    sidecar_binary: &Path,
    claude_binary: Option<&Path>,
) -> Result<ProbeInstalledResult, ProviderError> {
    let value = tokio::time::timeout(
        PROBE_DEADLINE,
        run_probe(
            sidecar_binary,
            METHOD_PROBE_INSTALLED,
            claude_binary_param(claude_binary),
        ),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        operation: "probe-installed".into(),
        elapsed_ms: PROBE_DEADLINE.as_millis() as u64,
    })??;

    let parsed: ProbeInstalledResponse =
        serde_json::from_value(value).map_err(|e| ProviderError::RpcError {
            message: format!("malformed probe-installed response: {e}"),
        })?;
    Ok(ProbeInstalledResult {
        installed: parsed.installed,
        version: parsed.version,
        unresponsive: parsed.unresponsive,
    })
}

/// Spawn the sidecar, call `probe-authenticated`, shut down.
pub async fn probe_authenticated(
    sidecar_binary: &Path,
    claude_binary: Option<&Path>,
) -> Result<AuthStatus, ProviderError> {
    let value = tokio::time::timeout(
        PROBE_DEADLINE,
        run_probe(
            sidecar_binary,
            METHOD_PROBE_AUTHENTICATED,
            claude_binary_param(claude_binary),
        ),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        operation: "probe-authenticated".into(),
        elapsed_ms: PROBE_DEADLINE.as_millis() as u64,
    })??;

    let parsed: ProbeAuthenticatedResponse =
        serde_json::from_value(value).map_err(|e| ProviderError::RpcError {
            message: format!("malformed probe-authenticated response: {e}"),
        })?;
    Ok(auth_status_from_response(parsed))
}

fn auth_status_from_response(parsed: ProbeAuthenticatedResponse) -> AuthStatus {
    match parsed.status.as_str() {
        "authenticated" => AuthStatus::Authenticated {
            auth_label: parsed.auth_label,
        },
        "unauthenticated" => AuthStatus::Unauthenticated {
            message: parsed.message.unwrap_or_else(|| {
                "Claude CLI is not authenticated. Run `claude login` and retry.".into()
            }),
        },
        _ => AuthStatus::Unknown {
            raw: parsed.message.unwrap_or_default(),
            auth_label: parsed.auth_label,
        },
    }
}

/// User settings may route the CLI through a proxy without OAuth files.
pub(crate) fn read_settings(home: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(home.join("settings.json")).ok()?).ok()
}

pub(crate) fn config_home() -> Option<std::path::PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
}

fn claude_binary_param(claude_binary: Option<&Path>) -> serde_json::Value {
    match claude_binary {
        Some(p) => json!({ "binaryPath": p.to_string_lossy() }),
        None => json!({}),
    }
}

/// Spawn the sidecar, send one request, return the response.
async fn run_probe(
    sidecar_binary: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, ProviderError> {
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: sidecar_binary.to_path_buf(),
        args: vec![],
        env: HashMap::new(),
        // The probe's answer must not depend on where the app was started:
        // an unreadable inherited cwd (e.g. launched from /root) makes the
        // sidecar's `claude` child fail before it runs, which reads as an
        // unresponsive CLI.
        cwd: dirs::home_dir(),
        default_timeout: Duration::from_secs(5),
    })
    .await
    .map_err(|e| {
        // Spawn failures surface as NotInstalled so the UI can nudge
        // the user to (re)install.
        match e {
            crate::json_rpc_child::RpcChildError::SpawnFailed(io)
                if io.kind() == std::io::ErrorKind::NotFound =>
            {
                ProviderError::NotInstalled {
                    provider: ProviderKind::Claude,
                    hint: format!("sidecar binary not found at {}", sidecar_binary.display()),
                }
            }
            other => ProviderError::ProcessError {
                message: "failed to spawn claude-agent sidecar for probe".into(),
                source: Some(other.to_string()),
            },
        }
    })?;
    let resp = child
        .request(method, params)
        .await
        .map_err(|e| ProviderError::RpcError {
            message: format!("{method} RPC failed: {e}"),
        });
    let _ = child.shutdown().await;
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_readiness_claude_label_is_optional_and_passes_through() {
        for label in [None, Some("Custom endpoint · localhost:8080")] {
            let mut response = json!({"status": "authenticated"});
            if let Some(label) = label {
                response["authLabel"] = json!(label);
            }
            let status = auth_status_from_response(serde_json::from_value(response).unwrap());
            match status {
                AuthStatus::Authenticated { auth_label } => {
                    assert_eq!(auth_label.as_deref(), label)
                }
                other => panic!("expected authenticated, got {other:?}"),
            }
        }
        let response = json!({
            "status": "unknown",
            "authLabel": "Custom endpoint · localhost:8080",
            "message": "Custom endpoint has no credentials",
        });
        match auth_status_from_response(serde_json::from_value(response).unwrap()) {
            AuthStatus::Unknown { raw, auth_label } => {
                assert_eq!(raw, "Custom endpoint has no credentials");
                assert_eq!(
                    auth_label.as_deref(),
                    Some("Custom endpoint · localhost:8080")
                );
            }
            other => panic!("expected unverified endpoint, got {other:?}"),
        }
    }
}
