//! Custom ACP settings are local launch authority, not caller-supplied subprocess payloads.
use std::{path::PathBuf, sync::Arc};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use crate::{
    agent_provider::{custom_acp::{config::{AcpAgent, AcpAgentInput, AcpLaunchConfig}, AcpBinding, AcpCatalog, AcpStore, GenericAcpProvider}, ProviderKind, ThreadId},
    database::DatabaseStore,
};

pub struct AppAcpStore<R: Runtime>(pub AppHandle<R>);
impl<R: Runtime> AcpStore for AppAcpStore<R> {
    fn launch_config(&self, id: &str) -> Result<AcpLaunchConfig, String> {
        self.0.state::<DatabaseStore>().acp_launch_config(id)
    }
    fn binding(&self, thread: &str) -> Result<Option<AcpBinding>, String> {
        self.0.state::<DatabaseStore>().acp_binding(thread)
    }
    fn save_binding(&self, binding: &AcpBinding) -> Result<(), String> {
        self.0.state::<DatabaseStore>().save_acp_binding(binding)
    }
    fn update_binding(&self, binding: &AcpBinding) -> Result<(), String> {
        self.0.state::<DatabaseStore>().update_acp_binding(binding)
    }
}

#[tauri::command]
pub fn acp_agents(db: State<'_, DatabaseStore>) -> Result<Vec<AcpAgent>, String> {
    db.acp_agents()
}

#[tauri::command]
pub fn acp_save_agent<R: Runtime>(app: AppHandle<R>, db: State<'_, DatabaseStore>, input: AcpAgentInput) -> Result<AcpAgent, String> {
    let agent = db.save_acp_agent(input)?;
    let _ = app.emit("custom_acp_changed", ());
    Ok(agent)
}

#[tauri::command]
pub async fn acp_delete_agent<R: Runtime>(app: AppHandle<R>, db: State<'_, DatabaseStore>, provider: State<'_, Arc<GenericAcpProvider>>, agent_id: String) -> Result<(), String> {
    // Delete launch authority first; racing starts fail their final binding check.
    db.delete_acp_agent(&agent_id)?;
    let result = provider.disconnect_agent(&agent_id).await.map_err(|e| e.to_string());
    let _ = app.emit("custom_acp_changed", ());
    result
}

#[tauri::command]
pub async fn acp_probe(provider: State<'_, Arc<GenericAcpProvider>>, agent_id: String, cwd: Option<String>) -> Result<AcpCatalog, String> {
    let cwd = match cwd {
        Some(cwd) => PathBuf::from(cwd),
        None => dirs::home_dir().ok_or_else(|| "Cannot resolve a probe directory.".to_string())?,
    };
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err("The probe workspace must be an existing absolute directory.".into());
    }
    provider.probe(&agent_id, cwd).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn acp_binding(db: State<'_, DatabaseStore>, thread_id: String) -> Result<Option<AcpBinding>, String> {
    db.acp_binding(&thread_id)
}

#[tauri::command]
pub async fn acp_catalog(db: State<'_, DatabaseStore>, provider: State<'_, Arc<GenericAcpProvider>>, thread_id: String) -> Result<AcpCatalog, String> {
    Ok(acp_thread_catalog(db, provider, thread_id).await?.catalog)
}

/// Readback authority for a cold renderer; live is process-local, never durable.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AcpThreadCatalog {
    pub catalog: AcpCatalog,
    pub live: bool,
}

#[tauri::command]
pub async fn acp_thread_catalog(db: State<'_, DatabaseStore>, provider: State<'_, Arc<GenericAcpProvider>>, thread_id: String) -> Result<AcpThreadCatalog, String> {
    let binding = db.acp_binding(&thread_id)?.ok_or_else(|| "Select a custom ACP agent for this chat.".to_string())?;
    if let Some(catalog) = provider.live_catalog(&ThreadId(thread_id.clone())).await {
        if catalog.agent_id == binding.agent_id { return Ok(AcpThreadCatalog { catalog, live: true }); }
        return Err("ACP live catalog does not match the durable instance binding.".into());
    }
    let config = db.acp_launch_config(&binding.agent_id)?;
    if config.agent.revision != binding.revision {
        return Err("This agent's launch configuration changed. Restore it or start a new chat; this conversation will not switch configurations.".into());
    }
    Ok(AcpThreadCatalog { catalog: binding.catalog, live: false })
}

pub(super) fn persist_catalog(db: &DatabaseStore, thread_id: &str, _catalog: &AcpCatalog) -> Result<(), String> {
    // A caller snapshot is not durable authority; read inside the transaction.
    db.project_acp_binding(thread_id)
}

#[tauri::command]
pub async fn acp_set_config<R: Runtime>(app: AppHandle<R>, db: State<'_, DatabaseStore>, provider: State<'_, Arc<GenericAcpProvider>>, thread_id: String, config_id: String, value: Value) -> Result<AcpCatalog, String> {
    let thread = ThreadId(thread_id.clone());
    super::agent_chat::ensure_live_session(&app, ProviderKind::Acp, &thread).await?;
    let lock = super::agent_chat::resume_lock_for(&thread_id);
    let _guard = lock.lock().await;
    let catalog = provider.set_config(thread, config_id, value).await.map_err(|e| e.to_string())?;
    // Accepted configuration, not an optimistic UI request, becomes restart intent.
    persist_catalog(&db, &thread_id, &catalog)?;
    let _ = app.emit("custom_acp_catalog_changed", serde_json::json!({"thread_id":thread_id,"catalog":catalog}));
    Ok(catalog)
}
