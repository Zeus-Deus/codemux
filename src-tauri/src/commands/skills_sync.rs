// Tauri commands for the skills sync engine (Step 10 — Stage 3).
//
// Two surfaces:
//
//   - `skills_sync_now`: trigger a full pull-then-push cycle.
//     Called from the file watcher's "skills changed" listener and
//     from the (forthcoming) Stage 5 "Sync now" button.
//   - `skills_sync_status`: read the engine's current state for
//     UI rendering.
//
// Both are best-effort: when the user isn't signed in or the
// encryption key isn't loaded, the commands return a sensible
// "skipped" rather than failing the call. Callers can rely on the
// frontend's `syncAvailable` boolean to gate the trigger but
// shouldn't have to.

use std::path::PathBuf;

use tauri::State;

use crate::auth::{
    api_base_url, delete_encryption_key, load_cached_user, load_token, AuthState,
};
use crate::database::DatabaseStore;
use crate::encryption::EncryptionManager;
use crate::skills::paths::enumerate_scan_paths;
use crate::skills_sync::api_client::wipe_skills;
use crate::skills_sync::export::{
    export_all_synced_skills, import_exported_skills, recommended_export_filename,
    ExportSummary, ImportSummary,
};
use crate::skills_sync::{SyncEngine, SyncResult, SyncStateSnapshot};

#[tauri::command]
pub async fn skills_sync_now(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    encryption: State<'_, EncryptionManager>,
    engine: State<'_, SyncEngine>,
) -> Result<SyncResult, String> {
    // Bail cleanly when there's no bearer token or no encryption
    // key loaded. Returning Ok with all-zero counts keeps the
    // frontend's "Sync now" button from surfacing transient
    // errors right after signin (when the auth state is still
    // settling).
    let token = match load_token(&db) {
        Some((t, _expires)) => t,
        None => return Ok(SyncResult::default()),
    };
    if !encryption.is_available() {
        return Ok(SyncResult::default());
    }

    // Emit "we just transitioned to Syncing" so the frontend's
    // status display can spin its icon without waiting for the
    // engine to return. The engine itself flips its internal
    // state machine inside `sync_now`; this just wakes up
    // subscribers that didn't initiate the call (auto-triggered
    // syncs from the file watcher, periodic 5-min timer).
    //
    // We emit a hand-built Syncing snapshot here rather than
    // calling `engine.snapshot()` because `snapshot()` reflects
    // the engine's CURRENT state — which is still Idle/Error
    // until `sync_now` actually starts. The engine doesn't take
    // an `AppHandle`, so wiring the emit through it would couple
    // the pure module to Tauri. Doing it from the wrapper is
    // simpler.
    use tauri::Emitter;
    let started_at_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = app.emit(
        "skills-sync-state-changed",
        &serde_json::json!({
            "state": "syncing",
            "startedAtMillis": started_at_millis,
        }),
    );

    // The engine walks `enumerate_scan_paths().user_paths` for
    // its push pass; project + plugin paths are intentionally
    // excluded (project sync is Step 10.5; plugins are never
    // synced).
    let paths: Vec<std::path::PathBuf> = enumerate_scan_paths(None, false)
        .user_paths
        .into_iter()
        .map(|(p, _)| p)
        .collect();

    let result = engine.sync_now(&token, &encryption, paths).await;

    // Always emit the post-cycle snapshot — Idle on success,
    // Error on failure. Both transitions are interesting to the
    // frontend (success clears any prior error banner; failure
    // turns the icon red).
    let _ = app.emit("skills-sync-state-changed", &engine.snapshot());

    result
}

#[tauri::command]
pub fn skills_sync_status(
    engine: State<'_, SyncEngine>,
) -> SyncStateSnapshot {
    engine.snapshot()
}

// ────────────────────────────────────────────────────────────────
// Stage 4 — local export / import / reset.
// ────────────────────────────────────────────────────────────────

/// Suggested default filename for the OS save dialog. Includes
/// today's date so users can keep multiple backups distinguishable
/// at a glance.
#[tauri::command]
pub fn get_export_recommended_filename() -> String {
    recommended_export_filename()
}

/// Pull every encrypted skill, decrypt with the in-memory key, and
/// atomic-write a plaintext JSON file. Frontend opens the OS
/// save-dialog first and passes the user-chosen path here.
///
/// Errors when:
///   - No bearer token (user signed out).
///   - No encryption key loaded (sync was never set up, or local
///     `sync-key.enc` is missing — the user should run the
///     "Re-enter your password" repair flow first).
///   - The user has no cached email (rare; means the auth token
///     hasn't been verified at least once).
#[tauri::command]
pub async fn export_skills_to_file(
    db: State<'_, DatabaseStore>,
    encryption: State<'_, EncryptionManager>,
    file_path: String,
) -> Result<ExportSummary, String> {
    let (token, _expires) =
        load_token(&db).ok_or_else(|| "Not signed in".to_string())?;
    if !encryption.is_available() {
        return Err("Sync key not loaded — re-enter your password to export.".into());
    }
    let user = load_cached_user(&db)
        .ok_or_else(|| "No cached user; sign in once before exporting.".to_string())?;

    export_all_synced_skills(
        &token,
        &encryption,
        &PathBuf::from(file_path),
        &user.email,
    )
    .await
}

/// Read a previously-exported file and re-push every skill to the
/// server using the CURRENT in-memory encryption key. Use case:
/// after a password reset wiped the server, the user picks their
/// pre-reset backup and gets their skills back.
#[tauri::command]
pub async fn import_skills_from_file(
    db: State<'_, DatabaseStore>,
    encryption: State<'_, EncryptionManager>,
    file_path: String,
) -> Result<ImportSummary, String> {
    let (token, _expires) =
        load_token(&db).ok_or_else(|| "Not signed in".to_string())?;
    if !encryption.is_available() {
        return Err("Sync key not loaded — set up sync first.".into());
    }
    let user = load_cached_user(&db)
        .ok_or_else(|| "No cached user; sign in once before importing.".to_string())?;

    import_exported_skills(
        &token,
        &encryption,
        &PathBuf::from(file_path),
        &user.email,
    )
    .await
}

/// The destructive half of the password-reset flow:
///
///   1. `DELETE /api/skills` — wipes every encrypted skill on the
///      server. Without this, the user's old ciphertext stays
///      around encrypted under the forgotten key, costing storage
///      and offering nothing recoverable.
///   2. `delete_encryption_key()` + `EncryptionManager::clear()` —
///      drops the local key file and zeroes the in-memory copy.
///   3. `auth_state.set_auth_method(None)` — clears the session's
///      provider tag so the UI re-fetches state cleanly on signin.
///   4. `POST /api/auth/desktop/forgot-password` — triggers Better
///      Auth's email-based reset flow. The user clicks the link in
///      their inbox, sets a new password via the existing
///      reset-password page (Stage 1's
///      `api/src/reset-password/`), then comes back here and
///      signs in with the new password. The new password
///      auto-derives a fresh `encryption_key` (Stage 2's
///      `signin_email` path) and the user can then run
///      `import_skills_from_file` to restore their export.
///
/// Sign-out is NOT performed here — the user keeps their bearer
/// session active until the email link sets a new password.
/// That way they can continue using non-sync features of Codemux
/// during the reset window.
///
/// The function is the equivalent of "I have backed up what I
/// need, please nuke the server side." Frontend gates this behind
/// an explicit user confirmation step.
#[tauri::command]
pub async fn wipe_remote_skills_for_reset(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
    encryption: State<'_, EncryptionManager>,
) -> Result<(), String> {
    let (token, _expires) =
        load_token(&db).ok_or_else(|| "Not signed in".to_string())?;
    let user = load_cached_user(&db)
        .ok_or_else(|| "No cached user — sign in again before resetting.".to_string())?;

    // Step 1: wipe server-side ciphertext. Errors here are fatal
    // for the reset flow — we don't want to clear local state if
    // the server still has the user's encrypted blobs.
    wipe_skills(&token).await?;

    // Step 2: tear down local key state.
    encryption.clear();
    let _ = delete_encryption_key();
    auth_state.set_auth_method(None);

    // Step 3: notify the frontend so the SyncSection re-renders
    // without sync-available.
    use tauri::Emitter;
    let _ = app.emit(
        "sync-state-changed",
        &serde_json::json!({
            "syncAvailable": false,
            "authMethod": null,
        }),
    );

    // Step 4: trigger the email-reset. Mirrors the existing
    // `forgot_password` Tauri command but kept inline here so the
    // whole reset is a single user-initiated action.
    let url = format!("{}/api/auth/desktop/forgot-password", api_base_url());
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "email": user.email }))
        .send()
        .await
        .map_err(|e| format!("Failed to send reset email: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "Reset email request failed: HTTP {}",
            resp.status()
        ));
    }

    Ok(())
}
