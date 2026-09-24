//! Profile-scoped Hermes IPC. Only layout metadata crosses this boundary, never credentials.
use crate::{
    agent_provider::hermes::{
        binding::Binding,
        profile::{self, Profile},
        HermesProvider,
    },
    database::DatabaseStore,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tauri::State;

#[tauri::command]
pub fn hermes_profiles(db: State<'_, DatabaseStore>) -> Result<Vec<Profile>, String> {
    let binary = db
        .get_setting("hermes.installation")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("hermes"));
    let root = db
        .get_setting("hermes.root")
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(profile::default_root);
    profile::discover(&binary, &root)
}
#[tauri::command]
pub fn hermes_binding(
    db: State<'_, DatabaseStore>,
    thread_id: String,
) -> Result<Option<Binding>, String> {
    db.hermes_binding(&thread_id)
}
#[tauri::command]
pub async fn hermes_catalog(
    provider: State<'_, Arc<HermesProvider>>,
    profile: Profile,
) -> Result<Value, String> {
    let response = provider.catalog(profile).await.map_err(|e| e.to_string())?;
    let compatibility =
        crate::agent_provider::hermes::durable_catalog(&response).map_err(|e| e.to_string());
    Ok(
        json!({"state":if compatibility.is_ok(){"ready"}else{"unsupported"},"message":compatibility.err(),"session":response}),
    )
}
#[tauri::command]
pub async fn hermes_disconnect(
    provider: State<'_, Arc<HermesProvider>>,
    profile: Profile,
) -> Result<(), String> {
    provider
        .disconnect(profile)
        .await
        .map_err(|e| e.to_string())
}
