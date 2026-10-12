//! A dedicated native server per workflow attempt, separate from ordinary chat.
use crate::agent_provider::{
    managed::ManagedSession,
    managed_bridge::{self, ManagedBridgeSession},
    ProviderError, ProviderKind, ProviderRuntimeEvent, StartSessionInput,
};
use crate::json_rpc_child::SpawnConfig;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

fn setup(message: &str) -> ProviderError {
    ProviderError::ValidationError {
        message: format!("managed-start-rejected: setup_required: {message}"),
    }
}

fn mise_wrapper(script: &str) -> bool {
    script.starts_with("#!/")
        && script.contains("mise use -g")
        && script.lines().any(|line| {
            let words: Vec<_> = line
                .split_whitespace()
                .map(|word| word.trim_matches('"'))
                .collect();
            words.len() >= 5 && words[..5] == ["exec", "mise", "x", "opencode", "--"]
        })
}

async fn prepared_binary() -> Result<std::path::PathBuf, ProviderError> {
    let configured = std::env::var_os("CODEMUX_OPENCODE_BINARY").filter(|value| !value.is_empty());
    let mut path = configured.map(std::path::PathBuf::from).or_else(|| which::which("opencode").ok())
        .ok_or_else(|| setup("Install OpenCode 1.18.35 or set CODEMUX_OPENCODE_BINARY to its prepared executable"))?;
    // Resolve the existing Omarchy/mise installation without executing its
    // wrapper, which runs `mise use -g` even for a passive version check.
    let wrapper = std::fs::metadata(&path)
        .ok()
        .filter(|meta| meta.len() <= 8192)
        .and_then(|_| std::fs::read_to_string(&path).ok())
        .is_some_and(|script| mise_wrapper(&script));
    let shim = path
        .parent()
        .is_some_and(|parent| parent.ends_with("shims"))
        && path
            .canonicalize()
            .ok()
            .is_some_and(|resolved| resolved.file_stem().is_some_and(|name| name == "mise"));
    if wrapper || shim {
        let mise = which::which("mise")
            .map_err(|_| setup("Cannot resolve the prepared OpenCode mise installation"))?;
        let mut command = crate::execution::host_command_tokio(mise);
        command.args(["which", "opencode"]);
        if wrapper {
            command.args(["--tool", "opencode"]);
        }
        command
            .env("MISE_AUTO_INSTALL", "false")
            .env("MISE_NO_ENV", "1")
            .env("MISE_NO_HOOKS", "1")
            .env("MISE_AUTO_UPDATE", "false")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        if let Some(home) = dirs::home_dir() {
            command.current_dir(home);
        }
        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .map_err(|_| setup("Resolving prepared OpenCode timed out"))?
            .map_err(|_| setup("Cannot resolve prepared OpenCode"))?;
        if !output.status.success() || output.stdout.len() > 8192 {
            return Err(setup(
                "OpenCode is not prepared; CodeMux will not install it",
            ));
        }
        let resolved = std::str::from_utf8(&output.stdout)
            .ok()
            .map(str::trim)
            .filter(|text| !text.is_empty() && !text.contains(['\n', '\r']))
            .ok_or_else(|| setup("OpenCode resolver returned an invalid path"))?;
        path = resolved.into();
    }
    if !path.is_absolute() {
        return Err(setup(
            "OpenCode requires an absolute prepared executable path",
        ));
    }
    path.canonicalize()
        .ok()
        .filter(|value| value.is_file())
        .ok_or_else(|| setup("The prepared OpenCode executable is unavailable"))
}

pub async fn spawn(
    input: StartSessionInput,
    context: Arc<ManagedSession>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
) -> Result<Arc<ManagedBridgeSession>, ProviderError> {
    if input
        .model
        .as_deref()
        .is_none_or(|model| !model.contains('/'))
        || input.context_window.is_some()
        || input.fast_mode
    {
        return Err(ProviderError::ValidationError { message: "managed-start-rejected: OpenCode workflows require an explicit provider/model and do not support context or fast mode overrides".into() });
    }
    let binary = prepared_binary().await?;
    let config = SpawnConfig {
        program: managed_bridge::sidecar_path(ProviderKind::OpenCode)?,
        args: vec![],
        env: input.env.clone().unwrap_or_default(),
        cwd: Some(input.cwd.clone()),
        default_timeout: Duration::from_secs(60),
    };
    ManagedBridgeSession::spawn(
        input,
        ProviderKind::OpenCode,
        "opencode-managed-v1",
        context,
        config,
        serde_json::json!({"nativeBinary": binary}),
        event_tx,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_opencode_wrapper_detection_never_evaluates_shell() {
        assert!(mise_wrapper("#!/bin/bash\nflock lock mise use -g opencode\nexec mise x \"opencode\" -- \"$bin_path\" \"$@\""));
        assert!(!mise_wrapper(
            "#!/bin/bash\nexec mise x \"opencode; arbitrary\" -- opencode"
        ));
        assert!(!mise_wrapper(
            "#!/bin/bash\nmise use -g other\nexec mise x other -- opencode"
        ));
    }
}
