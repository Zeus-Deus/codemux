//! Cursor's official SDK is an opt-in managed harness. Ordinary ACP sessions
//! retain their CLI login and configuration; no CLI credentials are copied.

use crate::agent_provider::{
    managed::ManagedSession,
    managed_bridge::{self, ManagedBridgeSession},
    ProviderError, ProviderKind, ProviderRuntimeEvent, StartSessionInput,
};
use crate::json_rpc_child::SpawnConfig;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

pub async fn spawn(
    input: StartSessionInput,
    context: Arc<ManagedSession>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
) -> Result<Arc<ManagedBridgeSession>, ProviderError> {
    if input.context_window.is_some() || input.fast_mode {
        return Err(ProviderError::ValidationError { message: "managed-start-rejected: Cursor SDK workflows do not support CLI context or fast mode overrides".into() });
    }
    let config = SpawnConfig {
        program: managed_bridge::sidecar_path(ProviderKind::Cursor)?,
        args: vec![],
        env: input.env.clone().unwrap_or_default(),
        cwd: Some(input.cwd.clone()),
        default_timeout: Duration::from_secs(45),
    };
    ManagedBridgeSession::spawn(
        input,
        ProviderKind::Cursor,
        "cursor-sdk-1.0.37",
        context,
        config,
        serde_json::json!({}),
        event_tx,
    )
    .await
}
