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
    let binary = super::discovery::prepared_binary()
        .await
        .map_err(|message| setup(&message))?;
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
