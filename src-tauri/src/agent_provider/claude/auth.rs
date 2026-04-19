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

/// Result of a `probe_installed` call.
#[derive(Debug, Clone)]
pub struct ProbeInstalledResult {
    pub installed: bool,
    pub version: Option<String>,
}

/// Coarse authentication status.
#[derive(Debug, Clone)]
pub enum AuthStatus {
    /// CLI is installed AND the user is logged in.
    Authenticated,
    /// CLI is installed but not logged in. `message` describes the
    /// remediation path.
    Unauthenticated { message: String },
    /// Output couldn't be classified — surface the raw text.
    Unknown { raw: String },
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
    Ok(match parsed.status.as_str() {
        "authenticated" => AuthStatus::Authenticated,
        "unauthenticated" => AuthStatus::Unauthenticated {
            message: parsed.message.unwrap_or_else(|| {
                "Claude CLI is not authenticated. Run `claude login` and retry.".into()
            }),
        },
        _ => AuthStatus::Unknown {
            raw: parsed.message.unwrap_or_default(),
        },
    })
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
        cwd: None,
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
