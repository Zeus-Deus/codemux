//! Who each account is signed in as.
//!
//! Claude records the signed-in account in `.claude.json` beside its
//! config, so a file read answers without spawning anything. Codex is asked
//! through `account/read` on a short-lived app-server, the same channel a
//! session uses. Credential files are only ever checked for existence.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::agent_provider::codex::protocol::{Capabilities, ClientInfo, InitializeParams};
use crate::agent_provider::ProviderKind;
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

const CODEX_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// What is known about one account's sign-in.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AccountIdentity {
    /// `None` when it could not be told.
    pub signed_in: Option<bool>,
    pub email: Option<String>,
    pub organization: Option<String>,
    pub plan: Option<String>,
}

/// Codex answers are cached: they need a process, and the email of a
/// signed-in account does not change under it. Each answer remembers the
/// `auth.json` it was given under, so a sign-in or sign-out in a terminal
/// outdates it.
static CODEX_CACHE: Mutex<Option<HashMap<PathBuf, (AuthFile, AccountIdentity)>>> = Mutex::new(None);

/// Whether `auth.json` exists. Codex keyring storage never writes one, so a
/// keyring answer stays valid until the next refresh asks again.
type AuthFile = bool;

fn auth_file(home: &Path) -> AuthFile {
    home.join("auth.json").is_file()
}

/// Where Claude keeps `.claude.json` for a home. The CLI's own home keeps it
/// in `~` unless `CLAUDE_CONFIG_DIR` moved the config.
pub(super) fn claude_state_file(home: &Path, is_default: bool) -> Option<PathBuf> {
    if is_default && std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).is_none() {
        return dirs::home_dir().map(|h| h.join(".claude.json"));
    }
    Some(home.join(".claude.json"))
}

pub fn claude_identity(home: &Path, is_default: bool) -> AccountIdentity {
    let state = claude_state_file(home, is_default)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    claude_identity_from(state.as_ref(), home.join(".credentials.json").is_file())
}

fn claude_identity_from(state: Option<&Value>, credentials_file: bool) -> AccountIdentity {
    let oauth = state.and_then(|s| s.get("oauthAccount")).filter(|v| v.is_object());
    let text = |key: &str| {
        oauth
            .and_then(|o| o.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    AccountIdentity {
        signed_in: Some(oauth.is_some() || credentials_file),
        email: text("emailAddress"),
        organization: text("organizationName"),
        plan: None,
    }
}

/// The cached Codex answer, or a cheap guess from `auth.json` existing.
pub fn codex_identity_cached(home: &Path) -> AccountIdentity {
    let cached = CODEX_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.as_ref().and_then(|c| c.get(home).cloned()));
    let auth = auth_file(home);
    match cached {
        Some((at, identity)) if at == auth => identity,
        _ => AccountIdentity { signed_in: Some(auth), ..Default::default() },
    }
}

/// Ask Codex who `home` is signed in as, and cache the answer.
pub async fn codex_identity(home: &Path, is_default: bool) -> AccountIdentity {
    let auth = auth_file(home);
    let identity = match tokio::time::timeout(CODEX_PROBE_TIMEOUT, read_codex_account(home, is_default)).await {
        Ok(Ok(identity)) => identity,
        Ok(Err(error)) => {
            eprintln!("[codemux::provider_accounts] codex account/read for {}: {error}", home.display());
            return codex_identity_cached(home);
        }
        Err(_) => return codex_identity_cached(home),
    };
    if let Ok(mut cache) = CODEX_CACHE.lock() {
        cache.get_or_insert_with(HashMap::new).insert(home.to_path_buf(), (auth, identity.clone()));
    }
    identity
}

async fn read_codex_account(home: &Path, is_default: bool) -> Result<AccountIdentity, String> {
    let binary = which::which("codex").map_err(|_| "Codex is not installed".to_string())?;
    let mut env = HashMap::new();
    if !is_default {
        env.insert("CODEX_HOME".to_string(), home.to_string_lossy().into_owned());
    }
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: binary,
        args: vec!["app-server".into()],
        env,
        cwd: dirs::home_dir(),
        default_timeout: CODEX_PROBE_TIMEOUT,
    })
    .await
    .map_err(|e| format!("spawn: {e}"))?;
    let result = async {
        let init = serde_json::to_value(InitializeParams {
            client_info: ClientInfo {
                name: "codemux-accounts".into(),
                title: "Codemux".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            capabilities: Capabilities { experimental_api: true },
        })
        .map_err(|e| e.to_string())?;
        child.request("initialize", init).await.map_err(|e| format!("initialize: {e}"))?;
        child
            .notify("initialized", json!({}))
            .await
            .map_err(|e| format!("initialized: {e}"))?;
        child
            .request("account/read", json!({}))
            .await
            .map_err(|e| format!("account/read: {e}"))
    }
    .await;
    let _ = child.shutdown().await;
    Ok(codex_identity_from(&result?))
}

fn codex_identity_from(response: &Value) -> AccountIdentity {
    let account = response.get("account").filter(|v| v.is_object());
    let text = |key: &str| {
        account
            .and_then(|a| a.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let requires_auth = response
        .get("requiresOpenaiAuth")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    AccountIdentity {
        signed_in: Some(account.is_some() || !requires_auth),
        email: text("email"),
        organization: None,
        plan: crate::agent_provider::codex::session::codex_plan_label(text("planType").as_deref()),
    }
}

/// Identity for any supported provider without spawning anything.
pub fn identity_now(provider: ProviderKind, home: &Path, is_default: bool) -> AccountIdentity {
    match provider {
        ProviderKind::Claude => claude_identity(home, is_default),
        _ => codex_identity_cached(home),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_reads_the_signed_in_account() {
        let state = json!({ "oauthAccount": { "emailAddress": " me@work.dev ", "organizationName": "Work" } });
        let id = claude_identity_from(Some(&state), false);
        assert_eq!(id.signed_in, Some(true));
        assert_eq!(id.email.as_deref(), Some("me@work.dev"));
        assert_eq!(id.organization.as_deref(), Some("Work"));
        assert_eq!(claude_identity_from(None, false).signed_in, Some(false));
        assert_eq!(claude_identity_from(Some(&json!({})), true).signed_in, Some(true));
    }

    #[test]
    fn claude_account_home_keeps_its_own_state_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"side@me.dev"}}"#,
        )
        .unwrap();
        assert_eq!(claude_identity(tmp.path(), false).email.as_deref(), Some("side@me.dev"));
    }

    #[test]
    fn codex_account_read_maps_email_and_plan() {
        let id = codex_identity_from(&json!({
            "account": { "type": "chatgpt", "email": "me@home.dev", "planType": "pro" },
            "requiresOpenaiAuth": true
        }));
        assert_eq!(id.signed_in, Some(true));
        assert_eq!(id.email.as_deref(), Some("me@home.dev"));
        assert!(id.plan.is_some());
        let out = codex_identity_from(&json!({ "account": null, "requiresOpenaiAuth": true }));
        assert_eq!(out.signed_in, Some(false));
    }

    #[test]
    fn a_keyring_sign_in_survives_without_auth_json() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let signed_in = AccountIdentity { signed_in: Some(true), email: Some("k@me.dev".into()), ..Default::default() };
        CODEX_CACHE
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(home.clone(), (auth_file(&home), signed_in.clone()));
        assert_eq!(codex_identity_cached(&home), signed_in);
        // Signing out with file storage removes `auth.json`; signing in adds it.
        std::fs::write(home.join("auth.json"), "{}").unwrap();
        assert_eq!(codex_identity_cached(&home).email, None);
    }
}
