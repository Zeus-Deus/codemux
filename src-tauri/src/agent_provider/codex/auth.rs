//! One-shot probes for whether the `codex` CLI is installed and
//! authenticated.
//!
//! Health uses the same app-server account contract as sessions. A custom
//! provider can run without an OpenAI account, even when `login status`
//! reports that no OAuth login exists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::protocol::{AccountReadResponse, Capabilities, ClientInfo, InitializeParams};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};
use serde_json::json;
use std::time::Duration;

use tokio::process::Command;

use crate::agent_provider::ProviderError;

/// Linux `ETXTBSY` (errno 26): "an attempt was made to execute a file that
/// is currently open for writing." This is a *transient* kernel state, not
/// a permanent failure — the file is busy because some process in our
/// process tree still has a writable FD on the inode.
///
/// We see this in two places:
/// 1. **CI / multi-thread tests:** one test thread is between `write()` and
///    `close()` on a fresh script while another thread `fork()`s. The fork
///    inherits the writer FD; even with `O_CLOEXEC` it remains open until
///    the child's `exec()` runs, and during that microseconds-wide window
///    a sibling thread's `exec()` of the same inode trips ETXTBSY.
/// 2. **End-user machines:** anti-virus, indexers, or backup agents may
///    briefly hold a freshly-installed `codex` binary open for scanning.
///
/// Both cases resolve in milliseconds, so a short bounded retry loop is the
/// correct response — far better than surfacing a confusing
/// `ProcessError { source: "Text file busy (os error 26)" }` to the UI.
const ETXTBSY: i32 = 26;
const ETXTBSY_RETRY_DELAY: Duration = Duration::from_millis(20);
const ETXTBSY_MAX_ATTEMPTS: u32 = 25; // ~500 ms total worst case.

/// Wrap [`Command::output`] with retry-on-ETXTBSY. Any non-ETXTBSY error
/// (and the success case) is returned immediately on the first attempt.
async fn output_retrying_etxtbsy(mut cmd: Command) -> std::io::Result<std::process::Output> {
    for attempt in 0..ETXTBSY_MAX_ATTEMPTS {
        match cmd.output().await {
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && attempt + 1 < ETXTBSY_MAX_ATTEMPTS => {
                tokio::time::sleep(ETXTBSY_RETRY_DELAY).await;
                continue;
            }
            other => return other,
        }
    }
    unreachable!("loop above always returns when attempt+1 == MAX_ATTEMPTS")
}

/// Bound the initialize/account handshake; labels are best-effort.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub enum AuthStatus {
    Authenticated { auth_label: Option<String> },
    Unauthenticated { message: String },
}

/// The CLI home used when no account-specific home was selected.
pub(crate) fn config_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
}

pub(crate) fn read_user_config(home: &Path) -> Result<Option<toml::Value>, String> {
    let raw = match std::fs::read_to_string(home.join("config.toml")) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("could not read Codex provider configuration".into()),
    };
    toml::from_str(&raw)
        .map(Some)
        .map_err(|_| "could not parse Codex provider configuration".into())
}

/// Run `codex --version` and return the parsed version string if the CLI
/// is on PATH. `Ok(None)` means the binary was not runnable; `Err` covers
/// timeouts and other unexpected failures.
pub async fn probe_installed(codex_binary: &Path) -> Result<Option<String>, ProviderError> {
    let mut cmd = Command::new(codex_binary);
    cmd.arg("--version").kill_on_drop(true);
    let result = tokio::time::timeout(PROBE_TIMEOUT, output_retrying_etxtbsy(cmd)).await;
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

/// Probe native CLI routing. The Codemux-owned ChatGPT route is checked by
/// the caller before reaching this function.
pub async fn probe_authenticated(codex_binary: &Path) -> Result<AuthStatus, ProviderError> {
    probe_authenticated_in_home(codex_binary, None).await
}

pub(crate) async fn probe_authenticated_in_home(
    codex_binary: &Path,
    home: Option<&Path>,
) -> Result<AuthStatus, ProviderError> {
    // A malformed config can make account/read look like a fresh signed-out
    // home. Preserve an advisory result instead of telling the user to log in.
    let config = home
        .map(Path::to_path_buf)
        .or_else(config_home)
        .map(|home| read_user_config(&home))
        .transpose()
        .map_err(|message| ProviderError::RpcError { message })?
        .flatten();
    let mut env = HashMap::new();
    if let Some(home) = home {
        env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
    }
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: codex_binary.into(),
        args: vec!["app-server".into()],
        env,
        cwd: dirs::home_dir(),
        default_timeout: PROBE_TIMEOUT,
    })
    .await
    .map_err(|error| ProviderError::ProcessError {
        message: "failed to start Codex authentication probe".into(),
        source: Some(error.to_string()),
    })?;
    let result = tokio::time::timeout(PROBE_TIMEOUT, async {
        let init = serde_json::to_value(InitializeParams {
            client_info: ClientInfo {
                name: "codemux-health".into(),
                title: "Codemux".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: Capabilities {
                experimental_api: true,
            },
        })
        .map_err(|error| ProviderError::RpcError {
            message: error.to_string(),
        })?;
        child.request("initialize", init).await.map_err(rpc_error)?;
        child
            .notify("initialized", json!({}))
            .await
            .map_err(rpc_error)?;
        // Reading cached account data must not refresh or replace credentials.
        let response = child
            .request("account/read", json!({"refreshToken": false}))
            .await
            .map_err(rpc_error)?;
        if response
            .get("requiresOpenaiAuth")
            .and_then(serde_json::Value::as_bool)
            .is_none()
        {
            return Err(ProviderError::RpcError {
                message: "account/read omitted requiresOpenaiAuth".into(),
            });
        }
        let account: AccountReadResponse =
            serde_json::from_value(response).map_err(|error| ProviderError::RpcError {
                message: format!("account/read: {error}"),
            })?;
        Ok(account)
    })
    .await
    .map_err(|_| ProviderError::Timeout {
        operation: "codex app-server account/read".into(),
        elapsed_ms: PROBE_TIMEOUT.as_millis() as u64,
    })
    .and_then(|result| result);
    let result = async {
        let account = result?;
        let name = if account.account.is_none() {
            // A rejected config must not be mistaken for missing login. A
            // Codex build without `config/read` simply names nothing.
            let response = match tokio::time::timeout(
                Duration::from_secs(1),
                child.request("config/read", json!({"includeLayers": false})),
            )
            .await
            .map_err(|_| ProviderError::Timeout {
                operation: "codex app-server config/read".into(),
                elapsed_ms: 1000,
            })? {
                Ok(response) => Some(response),
                Err(crate::json_rpc_child::RpcChildError::RpcError(error))
                    if error.code == METHOD_NOT_FOUND =>
                {
                    None
                }
                Err(error) => return Err(rpc_error(error)),
            };
            response
                .and_then(|response| custom_provider_name(&response["config"]))
                .or_else(|| custom_provider_name(&serde_json::to_value(config.as_ref()?).ok()?))
        } else {
            None
        };
        Ok(classify_account_read(&account, name.as_deref()))
    }
    .await;
    let _ = child.shutdown().await;
    result
}

/// JSON-RPC "method not found".
const METHOD_NOT_FOUND: i64 = -32601;

fn rpc_error(error: crate::json_rpc_child::RpcChildError) -> ProviderError {
    ProviderError::RpcError {
        message: format!("Codex authentication probe: {error}"),
    }
}

fn custom_provider_name(config: &serde_json::Value) -> Option<String> {
    let id = config.get("model_provider")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    Some(
        config
            .get("model_providers")
            .and_then(|providers| providers.get(id))
            .and_then(|provider| provider.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(id)
            .to_string(),
    )
}

pub fn classify_account_read(
    account: &AccountReadResponse,
    custom_name: Option<&str>,
) -> AuthStatus {
    if account.needs_login() {
        return AuthStatus::Unauthenticated {
            message: "Codex CLI is not authenticated. Run `codex login` and try again.".into(),
        };
    }
    let label = match account.account.as_ref() {
        Some(info) => match info.account_type.as_deref() {
            Some("apiKey") => Some("OpenAI API key".into()),
            Some("chatgpt") | Some("chatgptDeviceCode") => Some(
                super::session::codex_plan_label(
                    info.plan_type
                        .as_deref()
                        .filter(|plan| !plan.eq_ignore_ascii_case("unknown")),
                )
                .unwrap_or_else(|| "ChatGPT".into()),
            ),
            _ => None,
        },
        None => Some(
            custom_name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| format!("Custom provider · {name}"))
                .unwrap_or_else(|| "Custom provider".into()),
        ),
    };
    AuthStatus::Authenticated { auth_label: label }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_readiness_account_rules_and_labels() {
        for (account, requires, name, expected) in [
            (
                json!({"type":"chatgpt", "planType":"pro"}),
                true,
                None,
                Some("ChatGPT Pro"),
            ),
            (json!({"type":"chatgpt"}), true, None, Some("ChatGPT")),
            (
                json!({"type":"chatgpt", "planType":"unknown"}),
                false,
                None,
                Some("ChatGPT"),
            ),
            (json!({"type":"apiKey"}), true, None, Some("OpenAI API key")),
            (json!(null), true, None, None),
            (
                json!(null),
                false,
                Some("ai-gateway"),
                Some("Custom provider · ai-gateway"),
            ),
            (json!(null), false, None, Some("Custom provider")),
        ] {
            let response =
                serde_json::from_value(json!({"account":account,"requiresOpenaiAuth":requires}))
                    .unwrap();
            match (classify_account_read(&response, name), expected) {
                (AuthStatus::Authenticated { auth_label }, Some(expected)) => {
                    assert_eq!(auth_label.as_deref(), Some(expected))
                }
                (AuthStatus::Unauthenticated { .. }, None) => {}
                other => panic!("unexpected mapping: {other:?}"),
            }
        }
    }

    #[test]
    fn proxy_readiness_provider_config_fallback_reads_toml() {
        let home = tempfile::tempdir().unwrap();
        assert!(read_user_config(home.path()).unwrap().is_none());
        std::fs::write(
            home.path().join("config.toml"),
            r#"
model_provider = "proxy"
[model_providers.proxy]
name = "My gateway"
"#,
        )
        .unwrap();
        let config = read_user_config(home.path()).unwrap().unwrap();
        assert_eq!(
            custom_provider_name(&serde_json::to_value(config).unwrap()).as_deref(),
            Some("My gateway")
        );
        std::fs::write(home.path().join("config.toml"), "invalid TOML").unwrap();
        assert!(read_user_config(home.path()).is_err());
    }

    #[test]
    fn proxy_readiness_provider_display_name_then_id() {
        assert_eq!(custom_provider_name(&json!({"model_provider":"proxy", "model_providers":{"proxy":{"name":"My gateway"}}})).as_deref(), Some("My gateway"));
        assert_eq!(
            custom_provider_name(&json!({"model_provider":"proxy"})).as_deref(),
            Some("proxy")
        );
        assert_eq!(custom_provider_name(&json!({})), None);
    }
}
