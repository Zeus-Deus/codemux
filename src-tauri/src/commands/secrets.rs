//! Tauri commands for the private secret store (`crate::secrets`).
//!
//! A value goes in through `secrets_store` and never comes back out: every
//! command returns `SecretMeta` (id, label, creation time) only.

use crate::database::automation_triggers::SecretMeta;
use crate::database::DatabaseStore;
use tauri::State;

#[tauri::command]
pub fn secrets_store(
    db: State<'_, DatabaseStore>,
    label: String,
    value: String,
) -> Result<SecretMeta, String> {
    crate::secrets::store(db.inner(), &label, &value)
}

#[tauri::command]
pub fn secrets_list(db: State<'_, DatabaseStore>) -> Vec<SecretMeta> {
    db.list_secrets()
}

#[tauri::command]
pub fn secrets_delete(db: State<'_, DatabaseStore>, id: String) -> Result<(), String> {
    db.delete_secret(&id)
}
