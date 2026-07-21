// Tauri commands for the skills sync engine (Step 10).
//
// Two surfaces:
//
//   - `skills_sync_now`: trigger a full pull-then-push cycle.
//     Called from the file watcher's "skills changed" listener and
//     from the "Sync now" button.
//   - `skills_sync_status`: read the engine's current state for
//     UI rendering.
//
// Both are best-effort: when the user isn't signed in the commands
// return a sensible "skipped" rather than failing the call. Callers
// can rely on the frontend's `syncAvailable` boolean to gate the
// trigger but shouldn't have to.

use std::path::PathBuf;

use tauri::State;

use crate::auth::{load_cached_user, load_token};
use crate::database::DatabaseStore;
use crate::skills::paths::enumerate_scan_paths;
use crate::skills_sync::export::{
    export_all_synced_skills, import_exported_skills, recommended_export_filename,
    ExportSummary, ImportSummary,
};
use crate::skills_sync::{SyncEngine, SyncResult, SyncStateSnapshot};

#[tauri::command]
pub async fn skills_sync_now<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    engine: State<'_, SyncEngine>,
) -> Result<SyncResult, String> {
    // Bail cleanly when there's no bearer token. Returning Ok with
    // all-zero counts keeps the frontend's "Sync now" button from
    // surfacing transient errors right after signin (when the auth
    // state is still settling).
    let token = match load_token(&db) {
        Some((t, _expires)) => t,
        None => return Ok(SyncResult::default()),
    };

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

    let result = engine.sync_now(&token, paths).await;

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

/// Pull every synced skill and atomic-write a plaintext JSON file.
/// Frontend opens the OS save-dialog first and passes the
/// user-chosen path here.
///
/// Errors when:
///   - No bearer token (user signed out).
///   - The user has no cached email (rare; means the auth token
///     hasn't been verified at least once).
#[tauri::command]
pub async fn export_skills_to_file(
    db: State<'_, DatabaseStore>,
    file_path: String,
) -> Result<ExportSummary, String> {
    let (token, _expires) =
        load_token(&db).ok_or_else(|| "Not signed in".to_string())?;
    let user = load_cached_user(&db)
        .ok_or_else(|| "No cached user; sign in once before exporting.".to_string())?;

    export_all_synced_skills(&token, &PathBuf::from(file_path), &user.email).await
}

/// Read a previously-exported file and re-push every skill to the
/// server. Use case: restoring skills from a local backup, or
/// seeding a fresh account.
#[tauri::command]
pub async fn import_skills_from_file(
    db: State<'_, DatabaseStore>,
    file_path: String,
) -> Result<ImportSummary, String> {
    let (token, _expires) =
        load_token(&db).ok_or_else(|| "Not signed in".to_string())?;
    let user = load_cached_user(&db)
        .ok_or_else(|| "No cached user; sign in once before importing.".to_string())?;

    import_exported_skills(&token, &PathBuf::from(file_path), &user.email).await
}
