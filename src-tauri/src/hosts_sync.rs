//! Hosts sync — pull/push the user's host list across their devices.
//!
//! Mirrors the shape of `settings_sync.rs`. SSH credentials never enter
//! this layer; only the identity (name + ssh_target) syncs.
//!
//! Wire model:
//! - `pull(token)` → GET `/api/hosts` → upsert each server row into the
//!   local DB. Server rows are authoritative for any host whose
//!   `server_id` matches a local row.
//! - `push(token)` → for each local row with `dirty=1`:
//!     - if `deleted_at IS NOT NULL && server_id IS NOT NULL`:
//!       DELETE `/api/hosts/:server_id`, then `mark_host_synced` so the
//!       next `purge_acknowledged_deletes` removes the tombstone.
//!     - elif `server_id IS NULL`: POST `/api/hosts` → server returns
//!       the assigned `id`; we `mark_host_synced(id, Some(server_id))`.
//!     - elif `server_id IS NOT NULL`: PATCH `/api/hosts/:server_id`
//!       with the updated fields → `mark_host_synced(id, None)`.
//! - `try_sync` is the public entrypoint: pull then push, swallowing
//!   any single-call failures so a flaky network doesn't strand the
//!   user. Anything still dirty after a failed push stays dirty and
//!   the next `try_sync` retries.
//!
//! Failure mode policy: a failed push leaves the row dirty and logs
//! once. We do not surface the error to the user via toast — they
//! already see the host in the UI, the sync indicator in Settings →
//! Account tells them when it last completed.

use crate::auth::{api_base_url, is_token_expired, load_token};
use crate::database::DatabaseStore;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;

/// Wire shape returned by `GET /api/hosts` and `POST /api/hosts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHost {
    pub id: String,
    pub name: String,
    #[serde(rename = "sshTarget")]
    pub ssh_target: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "deletedAt")]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListHostsResponse {
    hosts: Vec<ServerHost>,
}

#[derive(Debug, Deserialize)]
struct OneHostResponse {
    host: ServerHost,
}

#[derive(Debug, Serialize)]
struct HostUpsertBody<'a> {
    name: &'a str,
    #[serde(rename = "sshTarget")]
    ssh_target: &'a str,
}

/// Guard against concurrent sync attempts. Foreground sync + the
/// fire-and-forget sync each Tauri host CRUD command triggers could
/// otherwise overlap and double-push the same row. Skipping when one
/// is already in flight is correct: the in-flight one already sees
/// the latest dirty rows.
static SYNC_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Convenience wrapper used by Tauri commands. Resolves the database
/// + token from the app state and calls `try_sync`. Returns Ok(()) if
/// the user isn't signed in (sync isn't an error in that case).
pub async fn try_sync_with_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let db = app.state::<DatabaseStore>();
    let token = match valid_token(&db) {
        Some(t) => t,
        None => return Ok(()),
    };
    // Clone-by-value the Arc/State so we can drop the State borrow
    // before awaiting (Tauri's State is not Send across awaits).
    let db_ref: &DatabaseStore = &db;
    let pull_err = match pull(&token, db_ref).await {
        Ok(()) => None,
        Err(e) => Some(e),
    };
    let push_err = match push(&token, db_ref).await {
        Ok(()) => None,
        Err(e) => Some(e),
    };
    match (pull_err, push_err) {
        (None, None) => Ok(()),
        (Some(p), None) => Err(format!("pull failed: {p}")),
        (None, Some(p)) => Err(format!("push failed: {p}")),
        (Some(a), Some(b)) => Err(format!("pull failed: {a}; push failed: {b}")),
    }
}

fn valid_token(db: &DatabaseStore) -> Option<String> {
    let (token, expires_at) = load_token(db)?;
    if is_token_expired(&expires_at) {
        None
    } else {
        Some(token)
    }
}

/// Pull server state into the local DB. Idempotent. Rows that exist on
/// the server but not locally are inserted; rows that exist locally
/// AND on the server (matched by `server_id`) are updated in place;
/// purely-local rows (no `server_id` yet) are untouched.
///
/// We never delete a local row purely because the server lacks it —
/// that's the symmetry of the design: local creates wait for the next
/// push to learn their `server_id`. A row whose `server_id` is non-null
/// but missing from the server response is treated as a server-side
/// deletion the local hasn't observed yet.
pub async fn pull(token: &str, db: &DatabaseStore) -> Result<(), String> {
    let base = api_base_url();
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/api/hosts"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        // 404 is "endpoint not deployed yet" — treat as harmless skip
        // so dev/prod skew doesn't break the desktop. Matches the
        // pattern Vexis's voice sync uses.
        if resp.status().as_u16() == 404 {
            return Ok(());
        }
        return Err(format!("API error: {}", resp.status()));
    }
    let body: ListHostsResponse = resp.json().await.map_err(|e| format!("Parse: {e}"))?;

    // Index local rows by server_id so we know which local rows were
    // covered by the server response. Any local row with a server_id
    // NOT in the response was deleted server-side and should be
    // tombstoned locally.
    let local = db.list_hosts_for_sync();
    let server_ids: std::collections::HashSet<String> =
        body.hosts.iter().map(|h| h.id.clone()).collect();

    for h in &body.hosts {
        db.upsert_host_from_server(
            &h.id,
            &h.name,
            &h.ssh_target,
            &h.created_at,
            &h.updated_at,
            h.deleted_at.as_deref(),
        )?;
    }

    // Server-side deletion sweep: if a local row has a server_id that
    // the server no longer returns, it was deleted elsewhere. Mark it
    // tombstoned locally so it disappears from `list_hosts`. We don't
    // mark it dirty — there's nothing to push.
    for local_row in &local {
        if let Some(sid) = &local_row.server_id {
            if !server_ids.contains(sid) && local_row.deleted_at.is_none() {
                eprintln!(
                    "[hosts-sync] server no longer has {sid}; tombstoning locally"
                );
                // Use upsert with deleted_at = now to keep the dirty=0
                // invariant (server-sourced changes are always clean).
                let now = chrono::Utc::now()
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string();
                db.upsert_host_from_server(
                    sid,
                    &local_row.name,
                    &local_row.ssh_target,
                    &local_row.created_at,
                    &now,
                    Some(&now),
                )?;
            }
        }
    }

    Ok(())
}

/// Push dirty local rows to the server. Each row is handled
/// independently so a single failed PATCH doesn't strand other dirty
/// rows; the failure is logged and the row stays dirty for the next
/// sync to retry.
pub async fn push(token: &str, db: &DatabaseStore) -> Result<(), String> {
    let base = api_base_url();
    let client = reqwest::Client::new();
    let dirty = db.list_dirty_hosts();
    let mut any_failed = false;

    for row in &dirty {
        let result = if row.deleted_at.is_some() {
            push_delete(&client, &base, token, row).await
        } else if row.server_id.is_none() {
            push_insert(&client, &base, token, row, db).await
        } else {
            push_update(&client, &base, token, row, db).await
        };
        if let Err(error) = result {
            eprintln!(
                "[hosts-sync] push failed for local id {}: {error}",
                row.id
            );
            any_failed = true;
            // Continue — other rows still deserve a try.
        }
    }

    // Once all in-flight tombstones have been ack'd by the server, the
    // local row can be physically removed.
    db.purge_acknowledged_deletes()?;

    if any_failed {
        Err("one or more host pushes failed; see logs".into())
    } else {
        Ok(())
    }
}

async fn push_insert(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::HostRecord,
    db: &DatabaseStore,
) -> Result<(), String> {
    let body = HostUpsertBody {
        name: &row.name,
        ssh_target: &row.ssh_target,
    };
    let resp = client
        .post(format!("{base}/api/hosts"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    let parsed: OneHostResponse = resp.json().await.map_err(|e| format!("Parse: {e}"))?;
    db.mark_host_synced(row.id, Some(&parsed.host.id))?;
    Ok(())
}

async fn push_update(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::HostRecord,
    db: &DatabaseStore,
) -> Result<(), String> {
    let server_id = row
        .server_id
        .as_ref()
        .ok_or_else(|| "push_update called without server_id".to_string())?;
    let body = HostUpsertBody {
        name: &row.name,
        ssh_target: &row.ssh_target,
    };
    let resp = client
        .patch(format!("{base}/api/hosts/{server_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    db.mark_host_synced(row.id, None)?;
    Ok(())
}

async fn push_delete(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::HostRecord,
) -> Result<(), String> {
    // A row that was created and deleted entirely while offline (no
    // server_id) has nothing to push — just let it be purged locally.
    let server_id = match &row.server_id {
        Some(sid) => sid,
        None => return Ok(()),
    };
    let resp = client
        .delete(format!("{base}/api/hosts/{server_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    Ok(())
}

/// Foreground sync: pull then push, with the SYNC_IN_PROGRESS guard.
/// Used by Settings → Account's "Sync now" button (when we add one)
/// and by the auth-check path that runs once at startup.
#[allow(dead_code)]
pub async fn sync_hosts(token: &str, db: &DatabaseStore) -> Result<(), String> {
    if SYNC_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return Ok(()); // another sync is in flight, skip
    }
    let result = async {
        pull(token, db).await?;
        push(token, db).await?;
        Ok(())
    }
    .await;
    SYNC_IN_PROGRESS.store(false, Ordering::SeqCst);
    result
}
