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

fn configured_installation(db: &DatabaseStore) -> PathBuf {
    db.get_setting("hermes.installation")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("hermes"))
}

/// A profile arrives from the caller (desktop or web remote), so the
/// executable it names is untrusted. Only the configured Hermes may run.
pub(crate) fn require_configured_installation(
    db: &DatabaseStore,
    profile: &Profile,
) -> Result<(), String> {
    if profile.installation != profile::resolve_installation(&configured_installation(db))? {
        return Err("repair_required: this Hermes profile does not use the configured Hermes executable; refresh profiles in Settings".into());
    }
    Ok(())
}

#[tauri::command]
pub fn hermes_profiles(db: State<'_, DatabaseStore>) -> Result<Vec<Profile>, String> {
    let binary = configured_installation(&db);
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
    db: State<'_, DatabaseStore>,
    provider: State<'_, Arc<HermesProvider>>,
    profile: Profile,
) -> Result<Value, String> {
    require_configured_installation(&db, &profile)?;
    let response = provider.catalog(profile).await.map_err(|e| e.to_string())?;
    let compatibility =
        crate::agent_provider::hermes::durable_catalog(&response).map_err(|e| e.to_string());
    Ok(
        json!({"state":if compatibility.is_ok(){"ready"}else{"unsupported"},"message":compatibility.err(),"session":response}),
    )
}
#[tauri::command]
pub async fn hermes_disconnect(
    db: State<'_, DatabaseStore>,
    provider: State<'_, Arc<HermesProvider>>,
    profile: Profile,
) -> Result<(), String> {
    require_configured_installation(&db, &profile)?;
    provider
        .disconnect(profile)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hermes_profile_must_use_the_configured_executable() {
        let root = tempfile::tempdir().unwrap();
        let db = crate::database::init_test_database();
        let configured = std::env::current_exe().unwrap();
        db.set_setting("hermes.installation", configured.to_str().unwrap())
            .unwrap();
        let mut profile = profile::resolve(&configured, root.path(), "default").unwrap();
        require_configured_installation(&db, &profile).unwrap();
        profile.installation = PathBuf::from("/bin/sh");
        assert!(require_configured_installation(&db, &profile)
            .unwrap_err()
            .contains("configured Hermes executable"));
    }
}
