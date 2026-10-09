//! Hosts sync — pull/push the user's host list across their devices.
//!
//! Mirrors the shape of `settings_sync.rs`. SSH credentials never enter
//! this layer; only the identity (name + ssh_target) syncs.
//!
//! Wire model:
//! - `pull(token)` → GET `/api/hosts` → upsert each server row into the
//!   local DB. Server rows are authoritative for any host whose
//!   `server_id` matches a local row, unless that row still has
//!   unpushed changes (`dirty=1`): the pull runs first, so the server
//!   copy is older than the user's edit or delete.
//! - `push(token)` → for each local row with `dirty=1`:
//!     - if `deleted_at IS NOT NULL`: DELETE `/api/hosts/:server_id`
//!       (when it has one), then `mark_host_synced` so the next
//!       `purge_acknowledged_deletes` removes the tombstone.
//!     - elif `server_id IS NULL`: POST `/api/hosts` → server returns
//!       the assigned `id`; we `mark_host_synced(row, Some(server_id))`.
//!     - elif `server_id IS NOT NULL`: PATCH `/api/hosts/:server_id`
//!       with the updated fields → `mark_host_synced(row, None)`. A 404
//!       means another device removed it; the removal wins.
//! - A sync that changes the device list emits `hosts-changed`, so the
//!   UI picks up devices added, renamed or removed elsewhere.
//! - `try_sync` is the public entrypoint: pull then push, swallowing
//!   any single-call failures so a flaky network doesn't strand the
//!   user. Anything still dirty after a failed push stays dirty and
//!   the next `try_sync` retries. It runs after every local edit and
//!   once shortly after launch (`spawn_startup_host_sync`), so a device
//!   that just signed in picks up the devices added elsewhere.
//! - A pulled row whose `sshTarget` would not be a safe single ssh
//!   argument is skipped: targets reach `ssh` argv, and the account is
//!   not trusted to have validated them.
//!
//! Failure mode policy: a failed push leaves the row dirty and logs
//! once. We do not surface the error to the user via toast — they
//! already see the host in the UI, the sync indicator in Settings →
//! Account tells them when it last completed.

use crate::auth::{api_base_url, is_token_expired, load_token};
use crate::database::DatabaseStore;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{Emitter, Manager};

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

/// Queues `try_sync_with_app` runs. Two overlapping runs (the startup
/// pull racing the sync a device edit triggers) would both POST the same
/// new row and leave the account with a duplicate device; queued, the
/// second run finds the row already synced.
static SYNC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Ceiling on one queued sync, so a stalled request can't hold the lock.
const SYNC_BUDGET: Duration = Duration::from_secs(60);

/// Event emitted (no payload) when a sync changed the device list, so
/// the frontend hosts store reloads it. Mirrors `HOSTS_CHANGED_EVENT` in
/// `src/stores/hosts-store.ts`.
pub const HOSTS_CHANGED_EVENT: &str = "hosts-changed";

/// The parts of the device list the UI shows, compared before and after
/// a sync to tell whether it changed anything. The poller's local-only
/// columns are left out so its writes don't count.
fn visible_hosts(db: &DatabaseStore) -> Vec<(i64, Option<String>, String, String, bool)> {
    db.list_hosts()
        .into_iter()
        .map(|h| (h.id, h.server_id, h.name, h.ssh_target, h.dirty))
        .collect()
}

/// Convenience wrapper used by Tauri commands. Resolves the database
/// + token from the app state and calls `try_sync`. Returns Ok(()) if
/// the user isn't signed in (sync isn't an error in that case).
pub async fn try_sync_with_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let db = app.state::<DatabaseStore>();
    let token = match valid_token(&db) {
        Some(t) => t,
        None => return Ok(()),
    };
    let _queued = SYNC_LOCK.lock().await;
    // Clone-by-value the Arc/State so we can drop the State borrow
    // before awaiting (Tauri's State is not Send across awaits).
    let db_ref: &DatabaseStore = &db;
    let before = visible_hosts(db_ref);
    let outcome = tokio::time::timeout(SYNC_BUDGET, async {
        let pull_err = pull(&token, db_ref).await.err();
        let push_err = push(&token, db_ref).await.err();
        (pull_err, push_err)
    })
    .await;
    // Checked even after a timeout: whatever landed before it is real.
    if visible_hosts(db_ref) != before {
        if let Err(e) = app.emit(HOSTS_CHANGED_EVENT, ()) {
            eprintln!("[hosts-sync] emit {HOSTS_CHANGED_EVENT}: {e}");
        }
    }
    let (pull_err, push_err) =
        outcome.map_err(|_| format!("timed out after {}s", SYNC_BUDGET.as_secs()))?;
    match (pull_err, push_err) {
        (None, None) => Ok(()),
        (Some(p), None) => Err(format!("pull failed: {p}")),
        (None, Some(p)) => Err(format!("push failed: {p}")),
        (Some(a), Some(b)) => Err(format!("pull failed: {a}; push failed: {b}")),
    }
}

/// Pull the account's device list once shortly after launch, then wake
/// the status poller so newly pulled devices get a live dot. Call once
/// from app setup. A no-op when signed out.
pub fn spawn_startup_host_sync<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        // Let startup IO settle first.
        tokio::time::sleep(Duration::from_secs(2)).await;
        if let Err(error) = try_sync_with_app(&app).await {
            eprintln!("[hosts-sync] startup sync failed: {error}");
        }
        #[cfg(unix)]
        crate::hosts_inventory::request_poll();
    });
}

/// Whether a pulled row may be stored. Tombstones are always applied —
/// a deleted row is never probed — but a live row with a target `ssh`
/// could read as an option is dropped, leaving any local copy as it was.
fn accept_server_host(host: &ServerHost) -> bool {
    host.deleted_at.is_some()
        || crate::commands::hosts::validate_ssh_target(&host.ssh_target).is_ok()
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
/// purely-local rows (no `server_id` yet) are untouched, and so are
/// rows with unpushed changes (`upsert_host_from_server` skips them).
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
    apply_server_hosts(db, &body.hosts)
}

/// Fold one `GET /api/hosts` response into the local DB. Split from
/// `pull` so the merge rules are testable without a server.
fn apply_server_hosts(db: &DatabaseStore, hosts: &[ServerHost]) -> Result<(), String> {
    // Index local rows by server_id so we know which local rows were
    // covered by the server response. Any local row with a server_id
    // NOT in the response was deleted server-side and should be
    // tombstoned locally.
    let local = db.list_hosts_for_sync();
    let server_ids: std::collections::HashSet<String> =
        hosts.iter().map(|h| h.id.clone()).collect();

    for h in hosts {
        if !accept_server_host(h) {
            eprintln!("[hosts-sync] skipping {}: unsafe SSH target", h.id);
            continue;
        }
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
    // mark it dirty — there's nothing to push. A row with unpushed
    // changes is left to its push, which settles the conflict.
    for local_row in &local {
        if let Some(sid) = &local_row.server_id {
            if !server_ids.contains(sid) && local_row.deleted_at.is_none() && !local_row.dirty {
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
            push_delete(&client, &base, token, row, db).await
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
    db.mark_host_synced(row, Some(&parsed.host.id))?;
    Ok(())
}

/// Whether the account's device list is reachable and lacks `server_id`.
async fn account_lacks_host(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    server_id: &str,
) -> bool {
    let Ok(resp) = client
        .get(format!("{base}/api/hosts"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
    else {
        return false;
    };
    if !resp.status().is_success() {
        return false;
    }
    match resp.json::<ListHostsResponse>().await {
        Ok(list) => !list.hosts.iter().any(|host| host.id == server_id),
        Err(_) => false,
    }
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
    if resp.status().as_u16() == 404 {
        // A server without the hosts API answers 404 too (see `pull`);
        // keep the edit for a later push unless the account's list
        // exists and no longer has this device.
        if !account_lacks_host(client, base, token, server_id).await {
            return Err(format!("API error: {}", resp.status()));
        }
        // Removed on another device before this edit reached the
        // account. The pull leaves dirty rows alone, so settle it here:
        // the removal wins, as it does for a clean row.
        eprintln!("[hosts-sync] server no longer has {server_id}; dropping the local edit");
        db.mark_host_synced(row, None)?;
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        return db.upsert_host_from_server(
            server_id,
            &row.name,
            &row.ssh_target,
            &row.created_at,
            &now,
            Some(&now),
        );
    }
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    db.mark_host_synced(row, None)?;
    Ok(())
}

async fn push_delete(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::HostRecord,
    db: &DatabaseStore,
) -> Result<(), String> {
    // A row that was created and deleted entirely while offline (no
    // server_id) has nothing to push — clear dirty so the purge that
    // follows removes it locally.
    let Some(server_id) = &row.server_id else {
        return db.mark_host_synced(row, None);
    };
    let resp = client
        .delete(format!("{base}/api/hosts/{server_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    // 404 = already gone server-side, treat as success.
    if !resp.status().is_success() && resp.status().as_u16() != 404 {
        return Err(format!("API error: {}", resp.status()));
    }
    // Acknowledged, so the purge that follows can drop the tombstone.
    // The pull leaves dirty rows alone, so nothing else clears it.
    db.mark_host_synced(row, None)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn server_host(ssh_target: &str, deleted_at: Option<&str>) -> ServerHost {
        ServerHost {
            id: "srv-1".into(),
            name: "zeus".into(),
            ssh_target: ssh_target.into(),
            created_at: String::new(),
            updated_at: String::new(),
            deleted_at: deleted_at.map(Into::into),
        }
    }

    #[test]
    fn pull_drops_option_like_targets_but_keeps_tombstones() {
        assert!(accept_server_host(&server_host("deus@zeus", None)));
        assert!(accept_server_host(&server_host("homelab", None)));
        assert!(!accept_server_host(&server_host("-oProxyCommand=touch /tmp/x", None)));
        assert!(!accept_server_host(&server_host("user@host -p 22", None)));
        assert!(!accept_server_host(&server_host("", None)));
        assert!(
            accept_server_host(&server_host("-oProxyCommand=x", Some("2026-01-01"))),
            "a deletion still applies so the local copy goes away"
        );
    }

    fn synced(db: &DatabaseStore, name: &str, target: &str, server_id: &str) -> i64 {
        let host = db.insert_host(name, target).unwrap();
        db.mark_host_synced(&host, Some(server_id)).unwrap();
        host.id
    }

    fn row(db: &DatabaseStore, id: i64) -> crate::database::HostRecord {
        db.list_hosts_for_sync().into_iter().find(|h| h.id == id).unwrap()
    }

    #[test]
    fn pull_keeps_unpushed_edits_and_removals() {
        let db = DatabaseStore::new_in_memory();
        let edited = synced(&db, "box", "u@typo", "srv-1");
        let removed = synced(&db, "old", "u@old", "srv-2");
        let edited_gone = synced(&db, "lab", "u@lab", "srv-3");
        let clean_gone = synced(&db, "spare", "u@spare", "srv-4");
        db.update_host(edited, "box", "u@fixed").unwrap();
        db.delete_host(removed).unwrap();
        db.update_host(edited_gone, "lab-renamed", "u@lab").unwrap();

        // The account still has the old copies of the first two and no
        // longer has the last two.
        let mut first = server_host("u@typo", None);
        first.name = "box".into();
        let mut second = server_host("u@old", None);
        second.id = "srv-2".into();
        second.name = "old".into();
        apply_server_hosts(&db, &[first, second]).unwrap();

        let edited = row(&db, edited);
        assert_eq!(edited.ssh_target, "u@fixed", "the fix is not reverted");
        assert!(edited.dirty, "and still goes up with the push");
        let removed = row(&db, removed);
        assert!(removed.deleted_at.is_some(), "a removed device is not resurrected");
        assert!(removed.dirty, "and its removal still goes up");
        let edited_gone = row(&db, edited_gone);
        assert!(edited_gone.deleted_at.is_none() && edited_gone.dirty, "left to its push");
        assert!(
            row(&db, clean_gone).deleted_at.is_some(),
            "a clean row the account dropped is still tombstoned"
        );
    }

    #[test]
    fn visible_hosts_tracks_list_changes_but_not_poller_stamps() {
        let db = DatabaseStore::new_in_memory();
        let host = db.insert_host("box", "u@box").unwrap();
        let fresh = visible_hosts(&db);
        db.record_host_seen(host.id, "2026-08-27T10:00:00Z", Some(1)).unwrap();
        assert_eq!(visible_hosts(&db), fresh, "a poll tick is not a list change");

        db.mark_host_synced(&host, Some("srv-1")).unwrap();
        let uploaded = visible_hosts(&db);
        assert_ne!(uploaded, fresh, "a first upload sets server_id and clears dirty");

        let mut renamed = server_host("u@box", None);
        renamed.name = "renamed-elsewhere".into();
        apply_server_hosts(&db, &[renamed]).unwrap();
        assert_ne!(visible_hosts(&db), uploaded, "a rename pulled from the account");

        apply_server_hosts(&db, &[]).unwrap();
        assert!(visible_hosts(&db).is_empty(), "a removal pulled from the account");
    }
}
