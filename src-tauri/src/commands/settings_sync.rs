use tauri::Emitter;

use crate::auth::{is_token_expired, load_token};
use crate::settings_sync::{self, UserSettings};

fn get_valid_token() -> Option<String> {
    let (token, expires_at) = load_token()?;
    if is_token_expired(&expires_at) {
        None
    } else {
        Some(token)
    }
}

fn emit_settings_synced(app: &tauri::AppHandle, settings: &UserSettings) {
    let _ = app.emit("settings-synced", settings);
}

#[tauri::command]
pub async fn get_synced_settings(app: tauri::AppHandle) -> Result<UserSettings, String> {
    let token = match get_valid_token() {
        Some(t) => t,
        None => return Ok(settings_sync::load_cache().unwrap_or_default()),
    };

    // Server is the source of truth when reachable. Don't push dirty cache
    // before fetching — a stale cache (e.g., defaults written during sign-out
    // by auto-detect effects) would overwrite the user's real settings.
    match settings_sync::fetch_settings(&token).await {
        Ok(s) => {
            emit_settings_synced(&app, &s);
            Ok(s)
        }
        Err(_) => Ok(settings_sync::load_cache().unwrap_or_default()),
    }
}

#[tauri::command]
pub async fn update_synced_settings(
    app: tauri::AppHandle,
    settings: UserSettings,
) -> Result<UserSettings, String> {
    let token = match get_valid_token() {
        Some(t) => t,
        None => {
            // Not authenticated — save locally only
            settings_sync::save_cache(&settings)?;
            settings_sync::set_dirty(true);
            emit_settings_synced(&app, &settings);
            return Ok(settings);
        }
    };

    let result = settings_sync::push_settings(&token, &settings).await?;
    emit_settings_synced(&app, &result);
    Ok(result)
}

#[tauri::command]
pub async fn update_setting(
    app: tauri::AppHandle,
    section: String,
    key: String,
    value: serde_json::Value,
) -> Result<UserSettings, String> {
    let mut partial_section = serde_json::Map::new();
    partial_section.insert(key.clone(), value.clone());
    let mut partial_root = serde_json::Map::new();
    partial_root.insert(section.clone(), serde_json::Value::Object(partial_section));
    let partial = serde_json::Value::Object(partial_root);

    let token = match get_valid_token() {
        Some(t) => t,
        None => {
            // Offline: merge into local cache
            let current = settings_sync::load_cache().unwrap_or_default();
            let mut current_val = serde_json::to_value(&current).map_err(|e| e.to_string())?;
            if let Some(obj) = current_val.as_object_mut() {
                let section_obj = obj
                    .entry(&section)
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(s) = section_obj.as_object_mut() {
                    s.insert(key, value);
                }
            }
            let merged: UserSettings =
                serde_json::from_value(current_val).unwrap_or_default();
            settings_sync::save_cache(&merged)?;
            settings_sync::set_dirty(true);
            emit_settings_synced(&app, &merged);
            return Ok(merged);
        }
    };

    let result = settings_sync::patch_settings(&token, partial).await?;
    emit_settings_synced(&app, &result);
    Ok(result)
}

#[tauri::command]
pub async fn reset_synced_settings(app: tauri::AppHandle) -> Result<UserSettings, String> {
    let defaults = UserSettings::default();

    let token = match get_valid_token() {
        Some(t) => t,
        None => {
            settings_sync::clear_cache();
            emit_settings_synced(&app, &defaults);
            return Ok(defaults);
        }
    };

    let result = settings_sync::delete_settings(&token).await?;
    emit_settings_synced(&app, &result);
    Ok(result)
}
