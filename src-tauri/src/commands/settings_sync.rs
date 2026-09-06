use tauri::{Emitter, State};

use crate::auth::{is_token_expired, load_cached_user, load_token, AuthState};
use crate::database::DatabaseStore;
use crate::settings_sync::{self, UserSettings};

enum SettingsSession {
    Verified { token: String, user_id: String },
    PendingIdentity,
    SignedOut,
}

fn get_settings_session(db: &DatabaseStore, auth_state: &AuthState) -> (u64, SettingsSession) {
    auth_state.session_snapshot(|| {
        let Some((token, expires_at)) = load_token(db) else {
            return SettingsSession::SignedOut;
        };
        if is_token_expired(&expires_at) {
            return SettingsSession::SignedOut;
        }
        let Some(user_id) = load_cached_user(db).map(|user| user.id) else {
            return SettingsSession::PendingIdentity;
        };
        SettingsSession::Verified { token, user_id }
    })
}

fn emit_settings_synced<R: tauri::Runtime>(app: &tauri::AppHandle<R>, settings: &UserSettings) {
    let _ = app.emit("settings-synced", settings);
}

#[tauri::command]
pub async fn get_synced_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<UserSettings, String> {
    let (session_generation, session) = get_settings_session(&db, &auth_state);
    let (token, user_id) = match session {
        SettingsSession::Verified { token, user_id } => (token, user_id),
        // No verified identity, so the server cannot be consulted. Both
        // arms answer from the current cache scope — the same thing
        // `bootstrap_session` returns and the same base the local edit
        // commands merge into. Under a pending identity the owner is still
        // `None`, so this only ever surfaces signed-out/pending local edits
        // (or a pre-scoping legacy blob), never another account's cache.
        // Returning bare defaults here would let a refresh silently revert
        // edits the user just made in the UI.
        SettingsSession::PendingIdentity | SettingsSession::SignedOut => {
            return Ok(settings_sync::load_cache().unwrap_or_default())
        }
    };

    // Server is the source of truth when reachable. sync_settings fetches first,
    // then flushes any offline changes — safe ordering prevents stale cache overwrites.
    match settings_sync::sync_settings_for_owner(&token, &user_id).await {
        Ok(s) => auth_state
            .commit_if_session_current(session_generation, || {
                emit_settings_synced(&app, &s);
                s
            })
            .ok_or_else(|| "Authentication changed while settings were loading".into()),
        Err(_) => {
            if auth_state
                .commit_if_session_current(session_generation, || ())
                .is_none()
            {
                return Err("Authentication changed while settings were loading".into());
            }
            Ok(settings_sync::load_cache().unwrap_or_default())
        }
    }
}

#[tauri::command]
pub async fn update_synced_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
    settings: UserSettings,
) -> Result<UserSettings, String> {
    let (session_generation, session) = get_settings_session(&db, &auth_state);
    let (token, user_id) = match session {
        SettingsSession::Verified { token, user_id } => (token, user_id),
        SettingsSession::PendingIdentity | SettingsSession::SignedOut => {
            // No verified identity: signed out, or a valid token whose owner
            // hasn't been verified yet (e.g. sign-in completed while
            // offline). Save under the current cache scope and mark dirty in
            // one atomic step — verification can flip the owner while this
            // command is in flight, and the write-generation guard in the
            // settings_sync module keeps a concurrent adoption fetch from
            // clobbering the committed edit. Once the identity verifies, the
            // sign-in adoption pushes these edits to the server.
            return auth_state
                .commit_if_session_current(session_generation, || {
                    settings_sync::save_cache_dirty(&settings)?;
                    emit_settings_synced(&app, &settings);
                    Ok(settings)
                })
                .unwrap_or_else(|| {
                    Err("Authentication changed while settings were updating".into())
                });
        }
    };

    let result = settings_sync::push_settings_for_owner(&token, &user_id, &settings).await?;
    auth_state
        .commit_if_session_current(session_generation, || {
            emit_settings_synced(&app, &result);
            result
        })
        .ok_or_else(|| "Authentication changed while settings were updating".into())
}

#[tauri::command]
pub async fn update_setting<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
    section: String,
    key: String,
    value: serde_json::Value,
) -> Result<UserSettings, String> {
    let mut partial_section = serde_json::Map::new();
    partial_section.insert(key.clone(), value.clone());
    let mut partial_root = serde_json::Map::new();
    partial_root.insert(section.clone(), serde_json::Value::Object(partial_section));
    let partial = serde_json::Value::Object(partial_root);

    let (session_generation, session) = get_settings_session(&db, &auth_state);
    let (token, user_id) = match session {
        SettingsSession::Verified { token, user_id } => (token, user_id),
        SettingsSession::PendingIdentity | SettingsSession::SignedOut => {
            // No verified identity — merge into the local cache and mark
            // dirty atomically (see update_synced_settings) so the edit
            // syncs after the identity verifies (or after the next sign-in
            // adopts it).
            return auth_state
                .commit_if_session_current(session_generation, || {
                    let current = settings_sync::load_cache().unwrap_or_default();
                    let mut current_val =
                        serde_json::to_value(&current).map_err(|e| e.to_string())?;
                    if let Some(obj) = current_val.as_object_mut() {
                        let section_obj =
                            obj.entry(&section).or_insert_with(|| serde_json::json!({}));
                        if let Some(s) = section_obj.as_object_mut() {
                            s.insert(key, value);
                        }
                    }
                    let merged: UserSettings =
                        serde_json::from_value(current_val).unwrap_or_default();
                    settings_sync::save_cache_dirty(&merged)?;
                    emit_settings_synced(&app, &merged);
                    Ok(merged)
                })
                .unwrap_or_else(|| {
                    Err("Authentication changed while settings were updating".into())
                });
        }
    };

    let result = settings_sync::patch_settings_for_owner(&token, &user_id, partial).await?;
    auth_state
        .commit_if_session_current(session_generation, || {
            emit_settings_synced(&app, &result);
            result
        })
        .ok_or_else(|| "Authentication changed while settings were updating".into())
}

#[tauri::command]
pub async fn reset_synced_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<UserSettings, String> {
    let defaults = UserSettings::default();

    let (session_generation, session) = get_settings_session(&db, &auth_state);
    let (token, user_id) = match session {
        SettingsSession::Verified { token, user_id } => (token, user_id),
        SettingsSession::PendingIdentity => {
            // The owner isn't verified yet, so the reset can't reach the
            // server. Cache the defaults dirty; the sign-in adoption in the
            // settings_sync module pushes the reset once the identity
            // verifies. (Signed-out reset below stays a plain cache clear —
            // there is no pending account to sync to.)
            return auth_state
                .commit_if_session_current(session_generation, || {
                    settings_sync::save_cache_dirty(&defaults)?;
                    emit_settings_synced(&app, &defaults);
                    Ok(defaults)
                })
                .unwrap_or_else(|| {
                    Err("Authentication changed while settings were resetting".into())
                });
        }
        SettingsSession::SignedOut => {
            return auth_state
                .commit_if_session_current(session_generation, || {
                    settings_sync::clear_cache();
                    emit_settings_synced(&app, &defaults);
                    defaults
                })
                .ok_or_else(|| "Authentication changed while settings were resetting".into());
        }
    };

    let result = settings_sync::delete_settings_for_owner(&token, &user_id).await?;
    auth_state
        .commit_if_session_current(session_generation, || {
            emit_settings_synced(&app, &result);
            result
        })
        .ok_or_else(|| "Authentication changed while settings were resetting".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{save_auth, save_token, AuthUser};
    use serial_test::serial;
    use tauri::Manager;

    /// Build a mock app whose managed DB holds a valid token but no cached
    /// user — the pending-identity window (e.g. sign-in completed offline).
    fn pending_identity_app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        app.manage(DatabaseStore::new_in_memory());
        app.manage(AuthState::default());
        save_token(
            &app.handle().state::<DatabaseStore>(),
            "pending-token",
            "2099-01-01T00:00:00Z",
        )
        .unwrap();
        app
    }

    #[test]
    fn valid_token_without_cached_identity_is_not_treated_as_signed_out() {
        let db = DatabaseStore::new_in_memory();
        let auth_state = AuthState::default();
        save_token(&db, "pending-token", "2099-01-01T00:00:00Z").unwrap();

        let (_, session) = get_settings_session(&db, &auth_state);
        assert!(matches!(session, SettingsSession::PendingIdentity));
    }

    #[test]
    fn cached_identity_pairs_the_settings_token_with_its_owner() {
        let db = DatabaseStore::new_in_memory();
        let auth_state = AuthState::default();
        let user = AuthUser {
            id: "user-a".into(),
            email: "a@example.test".into(),
            name: None,
            image: None,
        };
        save_auth(&db, "user-a-token", "2099-01-01T00:00:00Z", Some(&user)).unwrap();

        let (_, session) = get_settings_session(&db, &auth_state);
        assert!(matches!(
            session,
            SettingsSession::Verified { token, user_id }
                if token == "user-a-token" && user_id == "user-a"
        ));
    }

    #[tokio::test]
    #[serial]
    async fn pending_identity_update_saves_locally_and_marks_dirty() {
        settings_sync::set_cache_owner(None);
        settings_sync::clear_cache();
        let app = pending_identity_app();
        let handle = app.handle().clone();

        let mut settings = UserSettings::default();
        settings.appearance.theme = "dark".into();

        let result = update_synced_settings(
            handle.clone(),
            handle.state(),
            handle.state(),
            settings.clone(),
        )
        .await
        .expect("pending-identity edits must save locally, not error");

        assert_eq!(result, settings);
        assert_eq!(settings_sync::load_cache(), Some(settings));
        assert!(
            settings_sync::is_dirty(),
            "dirty flag queues the edit for the post-verification sync"
        );
        settings_sync::clear_cache();
    }

    #[tokio::test]
    #[serial]
    async fn pending_identity_single_setting_merges_into_dirty_cache() {
        settings_sync::set_cache_owner(None);
        settings_sync::clear_cache();
        let app = pending_identity_app();
        let handle = app.handle().clone();

        let result = update_setting(
            handle.clone(),
            handle.state(),
            handle.state(),
            "appearance".into(),
            "theme".into(),
            serde_json::json!("dark"),
        )
        .await
        .expect("pending-identity edits must save locally, not error");

        assert_eq!(result.appearance.theme, "dark");
        assert_eq!(
            settings_sync::load_cache().map(|s| s.appearance.theme),
            Some("dark".into())
        );
        assert!(settings_sync::is_dirty());
        settings_sync::clear_cache();
    }

    #[tokio::test]
    #[serial]
    async fn pending_identity_get_returns_cached_local_edits_not_defaults() {
        settings_sync::set_cache_owner(None);
        settings_sync::clear_cache();
        let mut local = UserSettings::default();
        local.appearance.theme = "pending-local-theme".into();
        settings_sync::save_cache_dirty(&local).unwrap();
        let app = pending_identity_app();
        let handle = app.handle().clone();

        // Same answer as `bootstrap_session` for this session state; a
        // refresh must not revert the edit the user just made.
        let result = get_synced_settings(handle.clone(), handle.state(), handle.state())
            .await
            .expect("pending-identity read must answer from the local cache");
        assert_eq!(result, local);
        settings_sync::clear_cache();
    }

    #[tokio::test]
    #[serial]
    async fn pending_identity_reset_caches_defaults_dirty() {
        settings_sync::set_cache_owner(None);
        settings_sync::clear_cache();
        let mut existing = UserSettings::default();
        existing.appearance.theme = "dark".into();
        settings_sync::save_cache(&existing).unwrap();
        let app = pending_identity_app();
        let handle = app.handle().clone();

        let result = reset_synced_settings(handle.clone(), handle.state(), handle.state())
            .await
            .expect("pending-identity reset must land locally, not error");

        assert_eq!(result, UserSettings::default());
        assert_eq!(settings_sync::load_cache(), Some(UserSettings::default()));
        assert!(
            settings_sync::is_dirty(),
            "the reset must sync once the identity verifies"
        );
        settings_sync::clear_cache();
    }
}
