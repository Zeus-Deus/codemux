//! Workspaces sync — pull/push the user's workspace registry across
//! their devices. Modeled 1:1 on `hosts_sync.rs` and `automations_sync.rs`.
//!
//! What syncs (the metadata that lets another device discover a
//! workspace exists and reason about it):
//! - `title`
//! - `host_server_id` — which host the workspace currently lives on
//! - `project_path` (informational; only meaningful on the originating device)
//! - `project_remote` — git remote URL so other devices know where to clone
//! - `git_branch`
//! - `created_at` / `updated_at` / `deleted_at`
//!
//! What does NOT sync (per-device runtime state):
//! - `cwd`, `worktree_path`, `pane_statuses`, terminal sessions, surface tree
//! - Git deltas (ahead/behind/changed_files) — read from the local git tree
//! - `notification_count`, `notifications_muted` — per-device user state
//! - `pinned_at` — sidebar-inbox visibility override; where a card sits in
//!   *this* device's sidebar is not a property of the workspace itself
//!
//! Wire model:
//! - `pull(token)` → GET `/api/workspaces` → upsert each server row
//!   into the local `workspaces_sync` table. Server rows are
//!   authoritative for any workspace whose `server_id` matches a
//!   local row.
//! - `push(token)` → for each local row with `dirty=1`:
//!     - if `deleted_at IS NOT NULL && server_id IS NOT NULL`:
//!       DELETE `/api/workspaces/:server_id`, then
//!       `mark_workspace_sync_synced` so the next
//!       `purge_acknowledged_workspace_sync_deletes` removes the
//!       tombstone.
//!     - elif `server_id IS NULL`: POST `/api/workspaces` → server
//!       returns the assigned `id`; we
//!       `mark_workspace_sync_synced(id, Some(server_id))`.
//!     - elif `server_id IS NOT NULL`: PATCH `/api/workspaces/:server_id`
//!       with the updated fields → `mark_workspace_sync_synced(id, None)`.
//! - `try_sync_with_app(app)` is the public entrypoint: pull then
//!   push, swallowing single-call failures so a flaky network doesn't
//!   strand the user. Anything still dirty after a failed push stays
//!   dirty for the next sync to retry.
//!
//! Auth/security: the desktop's Bearer token (decrypted from the local
//! encrypted store) is attached to every request. The server enforces
//! per-user isolation via `authenticateBearer(c)` + a `WHERE user_id =
//! :authed` clause baked into every query. A 401 is treated as
//! "log in again"; we log and bail.
//!
//! Failure mode policy: a failed push leaves the row dirty and logs
//! once. We do not surface the error to the user via toast — they
//! already see the workspace in the UI, and the sync indicator (TBD)
//! tells them when it last completed.

use crate::auth::{api_base_url, is_token_expired, load_token};
use crate::database::DatabaseStore;
use crate::state::AppStateSnapshot;
use crate::sync_guard::{order_by_last_attempt, record_attempt, sync_client, Attempts, SyncGuard, SYNC_BUDGET};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Manager;

/// Wire shape returned by `GET /api/workspaces` and `POST /api/workspaces`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerWorkspace {
    pub id: String,
    pub title: String,
    #[serde(rename = "hostServerId")]
    pub host_server_id: Option<String>,
    #[serde(rename = "projectPath")]
    pub project_path: Option<String>,
    #[serde(rename = "projectRemote")]
    pub project_remote: Option<String>,
    #[serde(rename = "gitBranch")]
    pub git_branch: Option<String>,
    #[serde(rename = "gitHeadSha", default)]
    pub git_head_sha: Option<String>,
    #[serde(rename = "projectUid", default)]
    pub project_uid: Option<String>,
    #[serde(rename = "workspaceKind", default)]
    pub workspace_kind: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "deletedAt")]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListWorkspacesResponse {
    workspaces: Vec<ServerWorkspace>,
}

#[derive(Debug, Deserialize)]
struct OneWorkspaceResponse {
    workspace: ServerWorkspace,
}

#[derive(Debug, Serialize)]
struct WorkspaceUpsertBody<'a> {
    title: &'a str,
    #[serde(rename = "hostServerId", skip_serializing_if = "Option::is_none")]
    host_server_id: Option<&'a str>,
    #[serde(rename = "projectPath", skip_serializing_if = "Option::is_none")]
    project_path: Option<&'a str>,
    #[serde(rename = "projectRemote", skip_serializing_if = "Option::is_none")]
    project_remote: Option<&'a str>,
    #[serde(rename = "gitBranch", skip_serializing_if = "Option::is_none")]
    git_branch: Option<&'a str>,
    #[serde(rename = "gitHeadSha", skip_serializing_if = "Option::is_none")]
    git_head_sha: Option<&'a str>,
    #[serde(rename = "projectUid", skip_serializing_if = "Option::is_none")]
    project_uid: Option<&'a str>,
    #[serde(rename = "workspaceKind", skip_serializing_if = "Option::is_none")]
    workspace_kind: Option<&'a str>,
}

/// Guard against concurrent sync attempts. Foreground sync + the
/// fire-and-forget sync each workspace mutation triggers could
/// otherwise overlap and double-push the same row. Skipping when one
/// is already in flight is correct: the in-flight one already sees
/// the latest dirty rows.
static SYNC_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Convenience wrapper used by Tauri commands and the post-mutation
/// triggers. Resolves the database + token from the app state and
/// calls pull-then-push. Returns Ok(()) if the user isn't signed in
/// (sync isn't an error in that case).
/// One sync pass at a time. Without it a second pass — a background tick
/// and "Sync now", or two edits each scheduling a sync — could settle a
/// deletion whose row the first pass is still uploading, before that
/// upload returns the server id the DELETE needs.
static SYNC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// When each row was last sent (see [`order_by_last_attempt`]).
static ATTEMPTS: Attempts = std::sync::LazyLock::new(Default::default);

pub async fn try_sync_with_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    // Queue before reading the session: a pass that waited out a sign-out
    // or account switch must sync whoever is signed in now, not upload the
    // new account's rows with the old account's token.
    let _one_at_a_time = SYNC_LOCK.lock().await;
    let db = app.state::<DatabaseStore>();
    // No auth state means no session to sync for.
    let Some(auth) = app.try_state::<crate::auth::AuthState>() else {
        return Ok(());
    };
    let (generation, token) = auth.session_snapshot(|| valid_token(&db));
    let Some(token) = token else {
        return Ok(());
    };
    sync_locked(&token, &db, SyncGuard::session(&auth, generation)).await
}

/// One pull-then-push pass, queued behind any other.
pub async fn sync_once(token: &str, db: &DatabaseStore) -> Result<(), String> {
    let _one_at_a_time = SYNC_LOCK.lock().await;
    sync_locked(token, db, SyncGuard::unchecked()).await
}

/// Pull, then push. Called with [`SYNC_LOCK`] held. The pass stops
/// starting requests once [`SYNC_BUDGET`] is spent, but never abandons one
/// it has started: each request ends by its own timeout, so the pass always
/// learns whether an upload took — or keeps the row to try again.
async fn sync_locked(token: &str, db: &DatabaseStore, guard: SyncGuard<'_>) -> Result<(), String> {
    sync_with(token, db, &guard.within(SYNC_BUDGET), &sync_client()).await
}

async fn sync_with(
    token: &str,
    db: &DatabaseStore,
    guard: &SyncGuard<'_>,
    client: &reqwest::Client,
) -> Result<(), String> {
    let pull_err = pull_guarded(token, db, guard, client).await.err();
    let push_err = push_guarded(token, db, guard, client).await.err();
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

/// Pull server state into the local `workspaces_sync` table.
/// Idempotent. Rows that exist on the server but not locally are
/// inserted; rows that exist locally AND on the server (matched by
/// `server_id`) are updated in place; purely-local rows (no
/// `server_id` yet) are untouched.
///
/// We never delete a local row purely because the server lacks it —
/// that is the symmetry of the design: local creates wait for the
/// next push to learn their `server_id`. A row whose `server_id` is
/// non-null but missing from the server response is treated as a
/// server-side deletion the local hasn't observed yet.
pub async fn pull(token: &str, db: &DatabaseStore) -> Result<(), String> {
    pull_guarded(token, db, &SyncGuard::unchecked(), &sync_client()).await
}

async fn pull_guarded(
    token: &str,
    db: &DatabaseStore,
    guard: &SyncGuard<'_>,
    client: &reqwest::Client,
) -> Result<(), String> {
    let base = api_base_url();
    guard.ensure_current()?;
    let resp = client
        .get(format!("{base}/api/workspaces"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        // 404 = endpoint not deployed on this server. Harmless skip
        // so dev/prod skew doesn't break the desktop.
        if resp.status().as_u16() == 404 {
            return Ok(());
        }
        return Err(format!("API error: {}", resp.status()));
    }
    let body: ListWorkspacesResponse =
        resp.json().await.map_err(|e| format!("Parse: {e}"))?;
    // Written only while the session that asked is still current.
    guard.commit(|| apply_pulled(db, &body))?
}

fn apply_pulled(db: &DatabaseStore, body: &ListWorkspacesResponse) -> Result<(), String> {
    // Index local rows by server_id so we can detect server-side
    // deletions (server stopped returning a row we know about).
    let local = db.list_workspaces_sync_for_sync();
    let server_ids: std::collections::HashSet<String> =
        body.workspaces.iter().map(|w| w.id.clone()).collect();
    for w in &body.workspaces {
        db.upsert_workspace_sync_from_server(
            &w.id,
            &w.title,
            w.host_server_id.as_deref(),
            w.project_path.as_deref(),
            w.project_remote.as_deref(),
            w.git_branch.as_deref(),
            w.git_head_sha.as_deref(),
            &w.created_at,
            &w.updated_at,
            w.deleted_at.as_deref(),
            w.project_uid.as_deref(),
            w.workspace_kind.as_deref(),
        )?;
    }

    // Server-side deletion sweep: if a local row has a server_id the
    // server no longer returns, it was deleted elsewhere. Tombstone
    // it locally — dirty stays 0 (server-sourced changes are always
    // clean). A row with unpushed changes is left to its push, which
    // settles a row the server dropped (see `push_update`).
    for local_row in &local {
        if let Some(sid) = &local_row.server_id {
            if !server_ids.contains(sid) && local_row.deleted_at.is_none() && !local_row.dirty {
                eprintln!(
                    "[workspaces-sync] server no longer has {sid}; tombstoning locally"
                );
                let now = chrono::Utc::now()
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string();
                db.upsert_workspace_sync_from_server(
                    sid,
                    &local_row.title,
                    local_row.host_server_id.as_deref(),
                    local_row.project_path.as_deref(),
                    local_row.project_remote.as_deref(),
                    local_row.git_branch.as_deref(),
                    local_row.git_head_sha.as_deref(),
                    &local_row.created_at,
                    &now,
                    Some(&now),
                    local_row.project_uid.as_deref(),
                    local_row.workspace_kind.as_deref(),
                )?;
            }
        }
    }

    // Collapse cross-device duplicate sibling rows (see `dedupe_sibling_rows`).
    dedupe_sibling_rows(db);

    Ok(())
}

/// Collapse duplicate **un-adopted sibling** rows that describe the same
/// physical remote workspace.
///
/// When Device A and Device B both poll the same host before either's push
/// lands, each POSTs its own cloud row for one workspace; after a pull both
/// devices then see two cards. The server now prevents this at the source
/// (issue #66): `POST /api/workspaces` upserts (`ON CONFLICT`) against a
/// partial unique index on the workspace's stable identity —
/// `(user_id, host_server_id, project_uid, git_branch, workspace_kind)`
/// `WHERE deleted_at IS NULL AND project_uid IS NOT NULL AND host_server_id`
/// `IS NOT NULL` (lives in the `codemux-api` repo) — so a racing second
/// insert updates the existing row instead of creating a twin. The server
/// can only enforce the `project_uid` fallback identity because `origin_uid`
/// is local-only and never synced.
///
/// This client-side pass is kept as defense-in-depth: it still collapses any
/// duplicates created before the index shipped or by an older server, and it
/// converges on the same keeper the server's backfill does. We group
/// un-adopted siblings (`workspace_id IS NULL`) by `(host, origin_uid)` —
/// or, since `origin_uid` is local-only and absent on cloud-pulled rows, by
/// `(host, project_uid, branch, kind)` — keep the canonical row (one with a
/// `server_id`, lowest id), and tombstone the rest so the next push removes
/// the cloud duplicate. Adopted rows (a real local workspace) are never
/// touched.
fn dedupe_sibling_rows(db: &DatabaseStore) {
    use std::collections::HashMap;
    let rows = db.list_workspaces_sync();
    let mut groups: HashMap<String, Vec<&crate::database::WorkspaceSyncRecord>> =
        HashMap::new();
    for r in &rows {
        // Only un-adopted siblings are dedup candidates; an adopted row is a
        // real local workspace, not a duplicate card.
        if r.workspace_id.is_some() {
            continue;
        }
        let Some(host) = r.host_server_id.as_deref() else {
            continue;
        };
        let key = if let Some(uid) = r.origin_uid.as_deref() {
            format!("u|{host}|{uid}")
        } else if let Some(puid) = r.project_uid.as_deref() {
            format!(
                "p|{host}|{puid}|{}|{}",
                r.git_branch.as_deref().unwrap_or(""),
                r.workspace_kind.as_deref().unwrap_or(""),
            )
        } else {
            // No stable identity to group on — leave it alone.
            continue;
        };
        groups.entry(key).or_default().push(r);
    }
    for (_key, mut group) in groups {
        if group.len() < 2 {
            continue;
        }
        // Keeper = a row WITH a server_id first (already known to the cloud),
        // then lowest server_id, then lowest local id. Index 0 survives.
        group.sort_by(|a, b| {
            b.server_id
                .is_some()
                .cmp(&a.server_id.is_some())
                .then_with(|| a.server_id.cmp(&b.server_id))
                .then_with(|| a.id.cmp(&b.id))
        });
        let keep = group[0].id;
        for dup in &group[1..] {
            eprintln!(
                "[workspaces-sync] collapsing duplicate sibling row id={} \
                 (keeping id={keep})",
                dup.id
            );
            let _ = db.soft_delete_remote_discovered_workspace_sync_by_id(dup.id);
        }
    }
}

/// Push every dirty local row to the server. Each row is handled
/// independently so a single failed PATCH doesn't strand other dirty
/// rows; failures are logged and the row stays dirty for the next
/// sync to retry.
pub async fn push(token: &str, db: &DatabaseStore) -> Result<(), String> {
    push_guarded(token, db, &SyncGuard::unchecked(), &sync_client()).await
}

async fn push_guarded(
    token: &str,
    db: &DatabaseStore,
    guard: &SyncGuard<'_>,
    client: &reqwest::Client,
) -> Result<(), String> {
    let base = api_base_url();
    let mut dirty = guard.commit(|| db.list_dirty_workspaces_sync())?;
    order_by_last_attempt(&ATTEMPTS, &mut dirty, |row| row.id);
    let queued_server_ids: HashSet<&str> = dirty
        .iter()
        .filter_map(|row| row.server_id.as_deref())
        .collect();
    let mut first_error = None;
    // The account's list, fetched at most once a pass, and only if a row
    // turns out to be gone.
    let mut listed: Option<Option<std::collections::HashSet<String>>> = None;

    for queued in &dirty {
        // Out of time: the rest goes first next pass.
        if guard.expired() {
            first_error.get_or_insert_with(|| "workspace sync ran out of time".to_owned());
            break;
        }
        let Some(row) = guard.commit(|| db.dirty_workspace_by_id(queued.id))?? else {
            continue;
        };
        record_attempt(&ATTEMPTS, row.id);
        let receipt = if row.server_id.is_none() {
            guard.commit(|| db.workspace_create_receipt(row.id))??
        } else {
            None
        };
        let result = if let Some((sent, sid)) = receipt {
            guard.commit(|| {
                db.mark_workspace_sync_pushed(
                    &sent,
                    Some((&sid, queued_server_ids.contains(sid.as_str()))),
                )
            })?
        } else if row.deleted_at.is_some() {
            push_delete(client, &base, token, &row, db, guard).await
        } else if row.server_id.is_none() {
            push_insert(client, &base, token, &row, db, guard, &queued_server_ids).await
        } else {
            push_update(client, &base, token, &row, db, &mut listed, guard).await
        };
        if let Err(error) = result {
            eprintln!(
                "[workspaces-sync] push failed for local id {}: {error}",
                row.id
            );
            first_error.get_or_insert(error);
        }
    }

    guard.commit(|| db.purge_acknowledged_workspace_sync_deletes())??;

    first_error.map_or(Ok(()), Err)
}

async fn push_insert(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::WorkspaceSyncRecord,
    db: &DatabaseStore,
    guard: &SyncGuard<'_>,
    queued_server_ids: &HashSet<&str>,
) -> Result<(), String> {
    let body = WorkspaceUpsertBody {
        title: &row.title,
        host_server_id: row.host_server_id.as_deref(),
        project_path: row.project_path.as_deref(),
        project_remote: row.project_remote.as_deref(),
        git_branch: row.git_branch.as_deref(),
        git_head_sha: row.git_head_sha.as_deref(),
        project_uid: row.project_uid.as_deref(),
        workspace_kind: row.workspace_kind.as_deref(),
    };
    let request = client
        .post(format!("{base}/api/workspaces"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body);
    let Some(resp) = crate::sync_guard::send_create(
        crate::database::SyncRegistry::Workspaces,
        db,
        row.id,
        guard,
        request,
    )
    .await?
    else {
        return Ok(());
    };
    let parsed: OneWorkspaceResponse = resp.json().await.map_err(|e| format!("Parse: {e}"))?;
    let sid = parsed.workspace.id.as_str();
    guard.commit(|| {
        db.mark_workspace_sync_pushed(row, Some((sid, queued_server_ids.contains(sid))))
    })??;
    Ok(())
}

async fn push_update(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::WorkspaceSyncRecord,
    db: &DatabaseStore,
    listed: &mut Option<Option<std::collections::HashSet<String>>>,
    guard: &SyncGuard<'_>,
) -> Result<(), String> {
    let server_id = row
        .server_id
        .as_ref()
        .ok_or_else(|| "push_update called without server_id".to_string())?;
    let body = WorkspaceUpsertBody {
        title: &row.title,
        host_server_id: row.host_server_id.as_deref(),
        project_path: row.project_path.as_deref(),
        project_remote: row.project_remote.as_deref(),
        git_branch: row.git_branch.as_deref(),
        git_head_sha: row.git_head_sha.as_deref(),
        project_uid: row.project_uid.as_deref(),
        workspace_kind: row.workspace_kind.as_deref(),
    };
    guard.ensure_current()?;
    let resp = client
        .patch(format!("{base}/api/workspaces/{server_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if resp.status().as_u16() == 404 && server_lacks_workspace(client, base, token, server_id, listed, guard).await {
        // Deleted on another device before this edit reached the account.
        // The pull leaves dirty rows alone, so settle it here: the
        // removal wins, as it does for a clean row.
        eprintln!("[workspaces-sync] server no longer has {server_id}; dropping the local edit");
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        return guard.commit(|| {
        db.mark_workspace_sync_synced(row.id, None)?;
        db.upsert_workspace_sync_from_server(
            server_id,
            &row.title,
            row.host_server_id.as_deref(),
            row.project_path.as_deref(),
            row.project_remote.as_deref(),
            row.git_branch.as_deref(),
            row.git_head_sha.as_deref(),
            &row.created_at,
            &now,
            Some(&now),
            row.project_uid.as_deref(),
            row.workspace_kind.as_deref(),
        )
        })?;
    }
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    guard.commit(|| db.mark_workspace_sync_pushed(row, None))??;
    Ok(())
}

/// Whether the account's workspace list is readable and has no row
/// `server_id`. A server without the workspaces API answers 404 to
/// everything, so a 404 alone does not say the row is gone. The list is
/// fetched once per push pass, into `listed`; if it cannot be read, no row
/// is taken to be gone.
async fn server_lacks_workspace(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    server_id: &str,
    listed: &mut Option<Option<std::collections::HashSet<String>>>,
    guard: &SyncGuard<'_>,
) -> bool {
    if listed.is_none() {
        let ids = async {
            guard.ensure_current().ok()?;
            let resp = client
                .get(format!("{base}/api/workspaces"))
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await
                .ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let body = resp.json::<ListWorkspacesResponse>().await.ok()?;
            Some(body.workspaces.into_iter().map(|w| w.id).collect())
        }
        .await;
        *listed = Some(ids);
    }
    matches!(listed, Some(Some(ids)) if !ids.contains(server_id))
}

async fn push_delete(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::WorkspaceSyncRecord,
    db: &DatabaseStore,
    guard: &SyncGuard<'_>,
) -> Result<(), String> {
    // Only a create that never reached the account can be settled locally.
    let Some(server_id) = &row.server_id else {
        return guard.commit(|| {
            crate::database::SyncRegistry::Workspaces.settle_unsent_delete(db, row.id)
        })?;
    };
    guard.ensure_current()?;
    let resp = client
        .delete(format!("{base}/api/workspaces/{server_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    // 404 = already gone server-side, treat as success.
    if !resp.status().is_success() && resp.status().as_u16() != 404 {
        return Err(format!("API error: {}", resp.status()));
    }
    guard.commit(|| db.mark_workspace_sync_pushed(row, None))??;
    Ok(())
}

/// Read the workspace's git HEAD sha by shelling out to `git
/// rev-parse HEAD` in the given directory. Returns None if git
/// isn't installed, the directory isn't a git repo, or the repo
/// has no commits yet. Cheap (single git process per workspace per
/// reconcile, ~10ms each) so we don't cache.
fn read_git_head_sha(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let output = crate::git::git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() || sha.len() > 200 {
        None
    } else {
        Some(sha)
    }
}

/// Reconcile the local `workspaces_sync` table against the current
/// `AppStateSnapshot.workspaces`. This is the bridge between the
/// runtime-only app state (which has live workspace data) and the
/// sync mirror (which is persisted + sync-bound).
///
/// For each live workspace in the snapshot:
/// - If a `workspaces_sync` row exists with the same local
///   `workspace_id` AND any synced field differs, UPDATE (marks dirty).
/// - If no such sync row exists, INSERT (marks dirty).
///
/// For each `workspaces_sync` row whose `workspace_id` is no longer
/// in the snapshot, soft-delete (marks dirty). The next push DELETEs
/// the server row.
///
/// `workspaces_sync` rows that have `workspace_id IS NULL` (pulled
/// from sibling devices, not yet adopted here) are NEVER touched by
/// reconcile — they have no app_state counterpart by design.
///
/// Compatibility-only removed workspace kinds and the Home singleton are
/// excluded because neither represents a live project to publish.
///
/// Returns Ok even on partial failure: individual row failures are
/// logged so a corrupt row doesn't strand the whole tick.
pub fn reconcile_from_snapshot(
    db: &DatabaseStore,
    snapshot: &AppStateSnapshot,
) -> Result<(), String> {
    // Build local host_id → server_id map for translating
    // WorkspaceSnapshot.host_id (i64) into host_server_id (TEXT).
    let host_id_to_server: HashMap<i64, String> = db
        .list_hosts_for_sync()
        .into_iter()
        .filter_map(|h| h.server_id.map(|sid| (h.id, sid)))
        .collect();

    // Index existing sync rows by their local workspace_id so we can
    // tell INSERT from UPDATE in one pass without a per-row SELECT.
    let sync_rows = db.list_workspaces_sync();
    let sync_by_wid: HashMap<&str, &crate::database::WorkspaceSyncRecord> =
        sync_rows
            .iter()
            .filter_map(|r| r.workspace_id.as_deref().map(|wid| (wid, r)))
            .collect();

    // app_state ids we'll keep — anything else with a workspace_id
    // gets soft-deleted at the end.
    let mut live_wids: HashSet<String> = HashSet::new();

    for ws in &snapshot.workspaces {
        // Compatibility-only workspace kinds never represent live projects.
        if matches!(ws.workspace_type, crate::state::WorkspaceType::Removed) {
            continue;
        }
        // Home workspaces don't represent a project to sync — skip.
        if matches!(ws.workspace_type, crate::state::WorkspaceType::Home) {
            continue;
        }
        // Attach-in-place ("open on host") workspaces are a *local view*
        // onto a workspace that already lives — and is already published —
        // on its host (the inventory poller owns that row). Mirroring it
        // here would create a duplicate cloud row that phantoms on every
        // other device, and closing the local view (a detach) would then
        // soft-delete it. Skip: the host row is the source of truth.
        if ws.attach_only {
            continue;
        }

        let wid = ws.workspace_id.0.clone();
        live_wids.insert(wid.clone());

        let host_server_id = ws
            .host_id
            .and_then(|hid| host_id_to_server.get(&hid).cloned());
        let title = ws.title.as_str();
        let project_path = ws.project_root.as_deref();
        // project_remote isn't on WorkspaceSnapshot today (see the
        // workspace creation doc) — leave as None for v1. When we
        // wire it in later, this is the only call site that needs
        // updating.
        let project_remote: Option<&str> = None;
        let git_branch = ws.git_branch.as_deref();
        // First-class project identity, stamped on the snapshot at
        // create time (`set_workspace_project_root`). Local-only sync
        // columns for now (cloud doesn't carry them yet); they let the
        // overview group by project_uid and the pull-conflict guard
        // match exactly.
        let project_uid = ws.project_uid.as_deref();
        let workspace_kind = ws.workspace_kind.as_deref();

        // Read the workspace's git HEAD sha so Phase-4 divergence
        // detection can compare across devices. Best-effort: if
        // git isn't installed, the workspace isn't a git repo, or
        // it has no commits, we leave it as None. Use the
        // worktree_path when set, else fall back to cwd.
        let head_path = ws
            .worktree_path
            .as_deref()
            .or(Some(ws.cwd.as_str()))
            .unwrap_or("");
        let git_head_sha: Option<String> = read_git_head_sha(head_path);
        let git_head_sha_ref = git_head_sha.as_deref();

        if let Some(existing) = sync_by_wid.get(wid.as_str()) {
            let changed = existing.title != title
                || existing.host_server_id.as_deref() != host_server_id.as_deref()
                || existing.project_path.as_deref() != project_path
                || existing.project_remote.as_deref() != project_remote
                || existing.git_branch.as_deref() != git_branch
                || existing.git_head_sha.as_deref() != git_head_sha_ref
                || existing.project_uid.as_deref() != project_uid
                || existing.workspace_kind.as_deref() != workspace_kind;
            if changed {
                if let Err(error) = db.update_workspace_sync_by_workspace_id(
                    &wid,
                    title,
                    host_server_id.as_deref(),
                    project_path,
                    project_remote,
                    git_branch,
                    git_head_sha_ref,
                    project_uid,
                    workspace_kind,
                ) {
                    eprintln!(
                        "[workspaces-sync] update failed for {wid}: {error}"
                    );
                }
            }
        } else if let Err(error) = db.insert_workspace_sync(
            &wid,
            title,
            host_server_id.as_deref(),
            project_path,
            project_remote,
            git_branch,
            git_head_sha_ref,
            project_uid,
            workspace_kind,
        ) {
            // UNIQUE(workspace_id) collision shouldn't happen given
            // the index pass above — log and continue.
            eprintln!("[workspaces-sync] insert failed for {wid}: {error}");
        }
    }

    // Soft-delete sync rows whose workspace_id is no longer in
    // app_state. Rows with no workspace_id (pulled-only) are skipped
    // by construction (the filter_map above excludes them).
    for row in &sync_rows {
        if let Some(wid) = &row.workspace_id {
            if !live_wids.contains(wid) {
                if let Err(error) =
                    db.soft_delete_workspace_sync_by_workspace_id(wid)
                {
                    eprintln!(
                        "[workspaces-sync] soft-delete failed for {wid}: {error}"
                    );
                }
            }
        }
    }

    Ok(())
}

/// Foreground sync: pull then push, with the SYNC_IN_PROGRESS guard.
/// Used by the auth-check path that runs once at startup and any
/// future "Sync now" UI button.
#[allow(dead_code)]
pub async fn sync_workspaces(token: &str, db: &DatabaseStore) -> Result<(), String> {
    if SYNC_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return Ok(()); // another sync is in flight; skip
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
    use crate::database::DatabaseStore;
    use crate::sync_guard::SyncGuard;
    use serial_test::serial;

    fn fresh_db() -> DatabaseStore {
        DatabaseStore::new_in_memory()
    }

    #[test]
    #[serial]
    fn insert_and_list_round_trips() {
        let db = fresh_db();
        let row = db
            .insert_workspace_sync(
                "workspace-1",
                "alpha",
                None,
                Some("/home/zeus/projects/alpha"),
                Some("git@github.com:alpha/alpha.git"),
                Some("main"),
                Some("abc123"),
                Some("uid-alpha"),
                Some("main"),
            )
            .expect("insert");
        assert!(row.dirty);
        assert!(row.server_id.is_none());
        assert_eq!(row.workspace_id.as_deref(), Some("workspace-1"));
        assert_eq!(row.project_uid.as_deref(), Some("uid-alpha"));
        assert_eq!(row.workspace_kind.as_deref(), Some("main"));
        assert_eq!(row.title, "alpha");

        let list = db.list_workspaces_sync();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "alpha");
    }

    #[test]
    #[serial]
    fn update_marks_row_dirty_again() {
        let db = fresh_db();
        let row = db
            .insert_workspace_sync("workspace-2", "before", None, None, None, None, None, None, None)
            .unwrap();
        db.mark_workspace_sync_synced(row.id, Some("42")).unwrap();

        // Confirm the synced state.
        let listed = db.list_workspaces_sync();
        assert!(!listed[0].dirty);
        assert_eq!(listed[0].server_id.as_deref(), Some("42"));

        // Mutate.
        db.update_workspace_sync_by_workspace_id(
            "workspace-2",
            "after",
            Some("99"),
            None,
            None,
            Some("feature-x"),
            Some("def456"),
            None,
            None,
        )
        .unwrap();
        let dirty = db.list_dirty_workspaces_sync();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].title, "after");
        assert_eq!(dirty[0].host_server_id.as_deref(), Some("99"));
        assert_eq!(dirty[0].git_branch.as_deref(), Some("feature-x"));
    }

    #[test]
    #[serial]
    fn soft_delete_then_purge() {
        let db = fresh_db();
        let row = db
            .insert_workspace_sync("workspace-3", "doomed", None, None, None, None, None, None, None)
            .unwrap();
        db.mark_workspace_sync_synced(row.id, Some("100")).unwrap();

        db.soft_delete_workspace_sync_by_workspace_id("workspace-3")
            .unwrap();

        // list() hides the tombstone, list_for_sync() shows it.
        assert_eq!(db.list_workspaces_sync().len(), 0);
        let all = db.list_workspaces_sync_for_sync();
        assert_eq!(all.len(), 1);
        assert!(all[0].deleted_at.is_some());
        assert!(all[0].dirty);

        // Until the push acknowledges, dirty stays — purge is a no-op.
        db.purge_acknowledged_workspace_sync_deletes().unwrap();
        assert_eq!(db.list_workspaces_sync_for_sync().len(), 1);

        // Mark synced (simulating the post-DELETE callback) then purge:
        // the row is gone for good.
        db.mark_workspace_sync_synced(all[0].id, None).unwrap();
        db.purge_acknowledged_workspace_sync_deletes().unwrap();
        assert_eq!(db.list_workspaces_sync_for_sync().len(), 0);
    }

    #[tokio::test]
    #[serial]
    async fn deletion_after_an_unreadable_create_reply_survives_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sync.db");
        let db = DatabaseStore::test_file(&path);
        db.insert_workspace_sync("lost", "new", None, None, None, None, None, None, None)
            .unwrap();
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        // The account accepts the create, but its success body is truncated
        // before the client can read the assigned ID.
        let post = server
            .mock("POST", "/api/workspaces")
            .with_status(201)
            .with_body("{")
            .expect(1)
            .create_async()
            .await;
        assert!(super::push("token", &db).await.is_err());
        db.soft_delete_workspace_sync_by_workspace_id("lost")
            .unwrap();
        drop(db);

        let reopened = DatabaseStore::test_file(&path);
        let list = server
            .mock("GET", "/api/workspaces")
            .with_body(format!(
                r#"{{"workspaces":[{}]}}"#,
                workspace_json("srv-lost")
            ))
            .expect(1)
            .create_async()
            .await;
        let delete = server
            .mock("DELETE", "/api/workspaces/srv-lost")
            .expect(0)
            .create_async()
            .await;
        assert!(super::sync_once("token", &reopened).await.is_err());
        list.assert_async().await;
        delete.assert_async().await;
        post.assert_async().await;
        let pending = reopened.list_dirty_workspaces_sync();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].deleted_at.is_some() && pending[0].server_id.is_none());
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn a_close_made_while_its_row_was_being_uploaded_is_still_pushed() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        let db = fresh_db();
        let row = db
            .insert_workspace_sync("ws-inflight", "old", None, None, None, Some("main"), None, None, None)
            .unwrap();
        db.mark_workspace_sync_synced(row.id, Some("srv-inflight")).unwrap();
        db.update_workspace_sync_by_workspace_id("ws-inflight", "renamed", None, None, None, Some("main"), None, None, None)
            .unwrap();

        // The PATCH carrying the rename is in flight when the workspace is
        // closed; then its success is acknowledged.
        let in_flight = db.list_dirty_workspaces_sync().pop().unwrap();
        db.soft_delete_workspace_sync_by_workspace_id("ws-inflight").unwrap();
        db.mark_workspace_sync_pushed(&in_flight, None).unwrap();

        let delete = server
            .mock("DELETE", "/api/workspaces/srv-inflight")
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        super::push("token", &db).await.unwrap();
        delete.assert_async().await;
        std::env::remove_var("CODEMUX_API_URL");
    }

    type Handler = std::sync::Arc<
        dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = (&'static str, String)> + Send>>
            + Send
            + Sync,
    >;

    /// A tiny account API: each connection is served on its own task, so a
    /// slow answer holds up nothing else. `handler` gets the request line.
    async fn test_api(handler: Handler) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_string();
                    let (status, body) = handler(line).await;
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        url
    }

    fn workspace_json(id: &str) -> String {
        format!(
            r#"{{"id":"{id}","title":"new","hostServerId":null,"projectPath":null,"projectRemote":null,"gitBranch":"main","createdAt":"2026-01-01 00:00:00","updatedAt":"2026-01-01 00:00:00","deletedAt":null}}"#
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn a_close_during_a_first_upload_is_sent_by_the_next_sync_with_the_new_id() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let posted = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let deletes = std::sync::Arc::new(AtomicUsize::new(0));
        let (p, r, d) = (posted.clone(), release.clone(), deletes.clone());
        let handler: Handler = std::sync::Arc::new(move |line| {
            let (p, r, d) = (p.clone(), r.clone(), d.clone());
            Box::pin(async move {
                if line.starts_with("GET /api/workspaces ") {
                    ("200 OK", r#"{"workspaces":[]}"#.to_string())
                } else if line.starts_with("POST /api/workspaces ") {
                    // Held until the test has closed the workspace and
                    // started a second sync.
                    p.notify_one();
                    r.notified().await;
                    ("200 OK", format!(r#"{{"workspace":{}}}"#, workspace_json("srv-race")))
                } else if line.starts_with("DELETE /api/workspaces/srv-race ") {
                    d.fetch_add(1, Ordering::SeqCst);
                    ("204 No Content", String::new())
                } else {
                    ("404 Not Found", String::new())
                }
            })
        });
        std::env::set_var("CODEMUX_API_URL", test_api(handler).await);
        let db = fresh_db();
        db.insert_workspace_sync("ws-race", "new", None, None, None, Some("main"), None, None, None)
            .unwrap();

        let first = super::sync_once("token", &db);
        let second = async {
            posted.notified().await;
            db.soft_delete_workspace_sync_by_workspace_id("ws-race").unwrap();
            // The second sync runs alongside while the upload is still held;
            // given a moment to get going (or to queue), the upload is
            // answered. With the queue the outcome never depends on that
            // moment; without it, the second sync would settle the close
            // before the server id arrives.
            let (second, ()) = tokio::join!(super::sync_once("token", &db), async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                release.notify_one();
            });
            second
        };
        let (first, second) = tokio::join!(first, second);
        std::env::remove_var("CODEMUX_API_URL");
        first.unwrap();
        second.unwrap();
        assert_eq!(deletes.load(Ordering::SeqCst), 1, "the close goes up as a DELETE for the new id");
    }

    #[tokio::test]
    #[serial]
    async fn a_pass_queued_across_an_account_switch_sends_nothing() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        let list = server.mock("GET", "/api/workspaces").expect(0).create_async().await;
        let upload = server.mock("POST", "/api/workspaces").expect(0).create_async().await;
        let db = fresh_db();
        db.insert_workspace_sync("ws-switch", "new", None, None, None, None, None, None, None).unwrap();

        // The pass resolved account A's session, then waited while the user
        // switched to account B.
        let auth = crate::auth::AuthState::default();
        let (generation, ()) = auth.session_snapshot(|| ());
        auth.replace_session(|| ());
        // Each half fails on the stale session before sending anything.
        let _ = super::sync_locked("token", &db, SyncGuard::session(&auth, generation)).await;
        list.assert_async().await;
        upload.assert_async().await;
        assert!(db.list_dirty_workspaces_sync()[0].server_id.is_none(), "nothing was uploaded");
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn an_answer_that_arrives_after_an_account_switch_is_not_written() {
        let asked = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let (a, r) = (asked.clone(), release.clone());
        let handler: Handler = std::sync::Arc::new(move |_line| {
            let (a, r) = (a.clone(), r.clone());
            Box::pin(async move {
                a.notify_one();
                r.notified().await;
                ("200 OK", format!(r#"{{"workspaces":[{}]}}"#, workspace_json("srv-account-a")))
            })
        });
        std::env::set_var("CODEMUX_API_URL", test_api(handler).await);
        let db = fresh_db();
        let auth = crate::auth::AuthState::default();
        let (generation, ()) = auth.session_snapshot(|| ());
        let guard = SyncGuard::session(&auth, generation);
        let client = crate::sync_guard::sync_client();

        let pulling = super::pull_guarded("token", &db, &guard, &client);
        let switch = async {
            asked.notified().await;
            auth.replace_session(|| ());
            release.notify_one();
        };
        let (pulled, ()) = tokio::join!(pulling, switch);
        std::env::remove_var("CODEMUX_API_URL");

        assert!(pulled.is_err());
        assert!(db.list_workspaces_sync_for_sync().is_empty(), "account A's answer is not written for B");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn a_stalled_upload_ends_by_its_own_timeout_and_frees_the_queue() {
        let handler: Handler = std::sync::Arc::new(move |line| {
            Box::pin(async move {
                if line.starts_with("GET /api/workspaces ") {
                    ("200 OK", r#"{"workspaces":[]}"#.to_string())
                } else if line.starts_with("POST /api/workspaces ") {
                    // Never answered.
                    std::future::pending::<()>().await;
                    unreachable!()
                } else {
                    ("404 Not Found", String::new())
                }
            })
        });
        std::env::set_var("CODEMUX_API_URL", test_api(handler).await);
        let db = fresh_db();
        db.insert_workspace_sync("ws-stall", "new", None, None, None, Some("main"), None, None, None)
            .unwrap();

        // A request timeout short enough for a test; in the app it is 30s.
        let client = crate::sync_guard::sync_client_with(std::time::Duration::from_millis(300));
        let started = std::time::Instant::now();
        let stalled = super::sync_with("token", &db, &SyncGuard::unchecked(), &client).await;
        assert!(stalled.is_err(), "the upload's own timeout ends the pass");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));

        // The queue is free for the next pass, and the row is kept to try
        // again rather than dropped.
        std::env::remove_var("CODEMUX_API_URL");
        let row = db.list_dirty_workspaces_sync().pop().unwrap();
        assert_eq!(row.workspace_id.as_deref(), Some("ws-stall"));
        assert!(row.server_id.is_none() && row.dirty);
        let queue_free = tokio::time::timeout(std::time::Duration::from_secs(1), super::SYNC_LOCK.lock()).await;
        assert!(queue_free.is_ok(), "nothing holds the queue");
    }

    #[tokio::test]
    #[serial]
    async fn a_healthy_row_behind_stalling_ones_is_sent_by_a_later_pass() {
        let budget = std::time::Duration::from_secs(3600);
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = requests.clone();
        let handler: Handler = std::sync::Arc::new(move |line| {
            let observed = observed.clone();
            Box::pin(async move {
                let path = line.split_whitespace().nth(1).unwrap();
                observed.lock().unwrap().push(path.to_string());
                match path {
                    "/api/workspaces/srv-stall" => {
                        // Spend the budget only after this PATCH has reached
                        // the server. Other requests use the ordinary clock.
                        tokio::time::pause();
                        tokio::time::advance(budget).await;
                        tokio::time::resume();
                        ("503 Service Unavailable", String::new())
                    }
                    "/api/workspaces/srv-ok" => ("200 OK", "{}".to_string()),
                    _ => ("404 Not Found", String::new()),
                }
            })
        });
        std::env::set_var("CODEMUX_API_URL", test_api(handler).await);
        // Row ids repeat across test databases; start from a clean record.
        super::ATTEMPTS.lock().unwrap().clear();
        let db = fresh_db();
        // The stalling row sorts first by id.
        for (wid, sid) in [("ws-stall-first", "srv-stall"), ("ws-ok-second", "srv-ok")] {
            let row = db.insert_workspace_sync(wid, "old", None, None, None, None, None, None, None).unwrap();
            db.mark_workspace_sync_synced(row.id, Some(sid)).unwrap();
            db.update_workspace_sync_by_workspace_id(wid, "edited", None, None, None, None, None, None, None)
                .unwrap();
        }
        let client = reqwest::Client::new();
        let stall = "/api/workspaces/srv-stall";
        let healthy = "/api/workspaces/srv-ok";
        for expected in [vec![stall], vec![healthy, stall]] {
            let guard = SyncGuard::unchecked().within(budget);
            assert!(super::push_guarded("token", &db, &guard, &client)
                .await
                .is_err());
            assert_eq!(std::mem::take(&mut *requests.lock().unwrap()), expected);
        }
        std::env::remove_var("CODEMUX_API_URL");
        assert_eq!(db.list_dirty_workspaces_sync().len(), 1);
        assert_eq!(
            db.list_dirty_workspaces_sync()[0].server_id.as_deref(),
            Some("srv-stall")
        );
    }

    #[tokio::test]
    #[serial]
    async fn identity_upsert_merges_only_edited_fields_in_either_push_order() {
        for (layout_title, inventory_title, merged_title) in [
            ("old", "inventory rename", Some("inventory rename")),
            ("layout rename", "old", Some("layout rename")),
            ("same rename", "same rename", Some("same rename")),
            ("layout rename", "inventory rename", None),
        ] {
            check_identity_upsert(layout_title, inventory_title, merged_title, CreateRetry::None)
                .await;
        }
    }

    #[tokio::test]
    #[serial]
    async fn identity_upsert_recovers_acknowledged_edits_after_lost_reply_and_pull() {
        check_identity_upsert("layout rename", "old", Some("layout rename"), CreateRetry::AfterPull).await;
        check_identity_upsert("layout rename", "inventory rename", None, CreateRetry::AfterPull).await;
    }

    #[tokio::test]
    #[serial]
    async fn identity_upsert_preserves_a_path_edited_back_after_pull() {
        // Changing /other to /layout is an independent layout edit and must conflict.
        for (layout_start, merged_title) in [("/layout", Some("layout rename")), ("/other", None)] {
            check_identity_upsert(
                "layout rename",
                "old",
                merged_title,
                CreateRetry::AfterPathRevert { layout_start },
            )
            .await;
        }
    }

    #[derive(Clone, Copy)]
    enum CreateRetry {
        None,
        AfterPull,
        AfterPathRevert { layout_start: &'static str },
    }

    async fn check_identity_upsert(
        layout_title: &'static str,
        inventory_title: &str,
        merged_title: Option<&str>,
        retry: CreateRetry,
    ) {
        let lose_first_reply = !matches!(retry, CreateRetry::None);
        let revert_path = matches!(retry, CreateRetry::AfterPathRevert { .. });
        let layout_start = match retry {
            CreateRetry::AfterPathRevert { layout_start } => layout_start,
            _ => "/layout",
        };
        let expected_path = if revert_path { "/inventory" } else { "/new" };
        for inventory_first in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sync.db");
            let db = std::sync::Arc::new(DatabaseStore::test_file(&path));
            let mut server = mockito::Server::new_async().await;
            std::env::set_var("CODEMUX_API_URL", server.url());
            super::ATTEMPTS.lock().unwrap().clear();
            let host = Some("host");
            let project = Some("project");
            let branch = Some("main");
            let kind = Some("main");
            let insert_layout = || {
                db.insert_workspace_sync(
                    "layout",
                    "old",
                    host,
                    Some(layout_start),
                    None,
                    branch,
                    None,
                    project,
                    kind,
                )
                .unwrap();
                db.update_workspace_sync_by_workspace_id(
                    "layout",
                    layout_title,
                    host,
                    Some("/layout"),
                    None,
                    branch,
                    None,
                    project,
                    kind,
                )
                .unwrap();
            };
            if !inventory_first {
                insert_layout();
            }
            let holder = db
                .insert_remote_discovered_workspace_sync(
                    "host",
                    "daemon-id",
                    "old",
                    Some("/inventory"),
                    None,
                    branch,
                    project,
                    kind,
                    Some("/srv/repo"),
                    branch,
                )
                .unwrap();
            db.mark_workspace_sync_synced(holder.id, Some("canonical")).unwrap();
            db.update_remote_discovered_workspace_sync(
                holder.id,
                inventory_title,
                Some("/new"),
                None,
                branch,
                project,
                kind,
                Some("/srv/repo"),
                branch,
            )
            .unwrap();
            if inventory_first {
                insert_layout();
            }

            let identity = serde_json::json!({
                "hostServerId":"host", "projectUid":"project", "workspaceKind":"main", "gitBranch":"main"
            });
            let mut inventory_body = identity.clone();
            inventory_body["title"] = inventory_title.into();
            inventory_body["projectPath"] = "/new".into();
            let committed = std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null));
            let patched = committed.clone();
            let inventory_patch = server
                .mock("PATCH", "/api/workspaces/canonical")
                .match_body(mockito::Matcher::Json(inventory_body.clone()))
                .with_body_from_request(move |request| {
                    *patched.lock().unwrap() = serde_json::from_slice(request.body().unwrap()).unwrap();
                    Vec::new()
                })
                .expect(usize::from(inventory_first || lose_first_reply))
                .create_async()
                .await;

            let mut layout_body = identity.clone();
            layout_body["title"] = layout_title.into();
            layout_body["projectPath"] = "/layout".into();
            let edit_in_flight = layout_title == "old";
            let mut response: serde_json::Value =
                serde_json::from_str(&workspace_json("canonical")).unwrap();
            response.as_object_mut().unwrap().extend(layout_body.as_object().unwrap().clone());
            // The PATCH may also finish in an earlier pass. Its holder
            // stays clean and available to pull while the POST is pending.
            let separate_patch = revert_path
                || (!lose_first_reply && inventory_first && layout_title == "layout rename" && inventory_title == "old");
            if separate_patch {
                let pending = db.dirty_workspace_by_id(holder.id).unwrap().unwrap();
                super::push_update(
                    &super::sync_client(), &server.url(), "token", &pending, &db,
                    &mut None, &SyncGuard::unchecked(),
                ).await.unwrap();
                assert!(!db.list_workspaces_sync().iter().find(|w| w.id == holder.id).unwrap().dirty);
            }
            let db = if lose_first_reply {
                let posted = committed.clone();
                let lost = server
                    .mock("POST", "/api/workspaces")
                    .match_body(mockito::Matcher::Json(layout_body.clone()))
                    .with_body_from_request(move |request| {
                        *posted.lock().unwrap() = serde_json::from_slice(request.body().unwrap()).unwrap();
                        b"{".to_vec()
                    })
                    .expect(1)
                    .create_async()
                    .await;
                assert!(super::push("token", &db).await.unwrap_err().contains("Parse:"));
                lost.assert_async().await;
                inventory_patch.assert_async().await;
                let after_push = db.list_workspaces_sync_for_sync();
                assert!(!after_push.iter().find(|w| w.id == holder.id).unwrap().dirty);
                assert!(after_push.iter().find(|w| w.workspace_id.is_some()).unwrap().dirty);

                // Return whichever write actually reached the server last.
                let last = committed.lock().unwrap().clone();
                let layout_wrote_last = inventory_first || separate_patch;
                assert_eq!(&last, if layout_wrote_last { &layout_body } else { &inventory_body });
                let mut listed = response.clone();
                listed.as_object_mut().unwrap().extend(last.as_object().unwrap().clone());
                let list = server
                    .mock("GET", "/api/workspaces")
                    .with_body(serde_json::json!({"workspaces": [listed]}).to_string())
                    .expect(1)
                    .create_async()
                    .await;
                super::pull("token", &db).await.unwrap();
                list.assert_async().await;
                let pulled = db.list_workspaces_sync().into_iter().find(|w| w.id == holder.id).unwrap();
                assert_eq!(pulled.project_path.as_deref(), last["projectPath"].as_str());
                if revert_path {
                    assert_eq!(pulled.project_path.as_deref(), Some("/layout"));
                    db.update_remote_discovered_workspace_sync(
                        holder.id, &pulled.title, Some("/inventory"), None, branch, project, kind,
                        Some("/srv/repo"), branch,
                    ).unwrap();
                }
                drop(db);
                super::ATTEMPTS.lock().unwrap().clear();
                std::sync::Arc::new(DatabaseStore::test_file(&path))
            } else {
                db
            };
            let edit = std::sync::Arc::downgrade(&db);
            let post = server
                .mock("POST", "/api/workspaces")
                .match_body(mockito::Matcher::Json(layout_body.clone()))
                .with_body_from_request(move |_| {
                    if edit_in_flight {
                        edit.upgrade()
                            .unwrap()
                            .update_workspace_sync_by_workspace_id(
                                "layout",
                                layout_title,
                                host,
                                Some("/layout"),
                                None,
                                branch,
                                Some("new-head"),
                                project,
                                kind,
                            )
                            .unwrap();
                    }
                    serde_json::to_vec(&serde_json::json!({"workspace":response})).unwrap()
                })
                .expect(1)
                .create_async()
                .await;

            let mut merged_body = layout_body;
            merged_body["title"] = merged_title.unwrap_or(layout_title).into();
            merged_body["projectPath"] = expected_path.into();
            if edit_in_flight {
                merged_body["gitHeadSha"] = "new-head".into();
            }
            let patch = server
                .mock("PATCH", "/api/workspaces/canonical")
                .match_body(mockito::Matcher::Json(merged_body))
                // If the holder retries first, it also PATCHes the revert before the POST.
                .expect(usize::from(merged_title.is_some()) + usize::from(inventory_first && revert_path))
                .create_async()
                .await;

            let first = super::push("token", &db).await;
            drop(db);
            // Reopen between passes: a prior PATCH or a saved receipt
            // must not turn a conflicting field into an acknowledged one.
            let reopened = DatabaseStore::test_file(&path);
            let second = super::push("token", &reopened).await;
            let rows = reopened.list_workspaces_sync_for_sync();
            if let Some(title) = merged_title {
                first.unwrap();
                second.unwrap();
                assert_eq!(rows.len(), 1);
                let row = &rows[0];
                assert_eq!(row.id, holder.id);
                assert_eq!(row.title, title);
                assert_eq!(row.project_path.as_deref(), Some(expected_path));
                assert_eq!(row.git_head_sha.as_deref(), edit_in_flight.then_some("new-head"));
                assert_eq!(row.workspace_id.as_deref(), Some("layout"));
                assert_eq!(row.server_id.as_deref(), Some("canonical"));
                assert_eq!(row.origin_uid.as_deref(), Some("daemon-id"));
                assert_eq!(row.origin_path.as_deref(), Some("/srv/repo"));
                assert_eq!(row.default_branch.as_deref(), branch);
                assert!(!row.dirty);
            } else {
                for result in [first, second] {
                    assert!(result.unwrap_err().contains("conflicts with local row"));
                }
                assert_eq!(rows.len(), 2);
                let layout = rows.iter().find(|r| r.workspace_id.is_some()).unwrap();
                assert_eq!(layout.title, layout_title);
                assert_eq!(layout.project_path.as_deref(), Some("/layout"));
                let inventory = rows.iter().find(|r| r.id == holder.id).unwrap();
                assert_eq!(inventory.title, inventory_title);
                assert_eq!(inventory.project_path.as_deref(), Some(expected_path));
                assert!(inventory.dirty && layout.dirty);
            }
            inventory_patch.assert_async().await;
            post.assert_async().await;
            patch.assert_async().await;
            std::env::remove_var("CODEMUX_API_URL");
        }
    }

    #[test]
    #[serial]
    fn a_retry_answered_with_an_already_mirrored_id_leaves_one_linked_row() {
        let db = fresh_db();
        // The local workspace's first upload lost its answer, and a pull
        // then mirrored the server's copy as an unadopted row.
        let original = db
            .insert_workspace_sync("ws-retried", "title", None, None, None, Some("main"), None, None, None)
            .unwrap();
        db.upsert_workspace_sync_from_server(
            "srv-same", "title", None, None, None, Some("main"), None,
            "2026-01-01 00:00:00", "2026-01-01 00:00:00", None, None, None,
        )
        .unwrap();

        // The retry's answer names that same server row.
        db.mark_workspace_sync_pushed(&original, Some(("srv-same", false)))
            .unwrap();

        let rows = db.list_workspaces_sync_for_sync();
        assert_eq!(rows.len(), 1, "one row: {rows:?}");
        assert_eq!(rows[0].server_id.as_deref(), Some("srv-same"));
        assert_eq!(rows[0].workspace_id.as_deref(), Some("ws-retried"), "the local workspace stays linked");
        assert!(!rows[0].dirty);
    }

    #[test]
    #[serial]
    fn a_host_inventory_holder_clean_or_with_pending_work_is_never_dropped() {
        for (sid, edited) in [("srv-inv-clean", false), ("srv-inv-dirty", true)] {
            let db = fresh_db();
            let holder = db
                .insert_remote_discovered_workspace_sync(
                    "host-1", "origin-1", "t", Some("/srv/p"), None, Some("main"), None, None, Some("/srv/p"), None,
                )
                .unwrap();
            db.mark_workspace_sync_synced(holder.id, Some(sid)).unwrap();
            if edited {
                db.update_remote_discovered_workspace_sync(
                    holder.id, "renamed", Some("/srv/p"), None, Some("main"), None, None, Some("/srv/p"), None,
                )
                .unwrap();
            }
            let retried = db
                .insert_workspace_sync("ws-retry", "t", None, None, None, Some("main"), None, None, None)
                .unwrap();

            assert!(
                db.mark_workspace_sync_pushed(&retried, Some((sid, false)))
                    .is_err(),
                "{sid}"
            );
            let kept = db
                .list_workspaces_sync_for_sync()
                .into_iter()
                .find(|r| r.id == holder.id)
                .unwrap();
            assert_eq!(
                kept.origin_uid.as_deref(),
                Some("origin-1"),
                "{sid}: its host identity is kept"
            );
            assert_eq!(kept.dirty, edited, "{sid}: and its pending change");
        }
    }

    #[tokio::test]
    #[serial]
    async fn conflicting_owners_keep_the_post_receipt_across_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sync.db");
        let db = DatabaseStore::test_file(&path);
        let adopted = db
            .insert_workspace_sync("ws-adopted", "a", None, None, None, None, None, None, None)
            .unwrap();
        db.mark_workspace_sync_synced(adopted.id, Some("srv-held"))
            .unwrap();
        db.insert_workspace_sync("ws-other", "b", None, None, None, None, None, None, None)
            .unwrap();
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        let post = server
            .mock("POST", "/api/workspaces")
            .with_body(format!(r#"{{"workspace":{}}}"#, workspace_json("srv-held")))
            .expect(1)
            .create_async()
            .await;
        assert!(super::push("token", &db)
            .await
            .unwrap_err()
            .contains("conflicts with local row"));
        drop(db);
        let reopened = DatabaseStore::test_file(&path);
        assert!(super::push("token", &reopened)
            .await
            .unwrap_err()
            .contains("conflicts with local row"));
        post.assert_async().await;
        assert_eq!(reopened.list_workspaces_sync_for_sync().len(), 2);
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn a_pass_past_its_deadline_starts_no_request() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        let list = server.mock("GET", "/api/workspaces").expect(0).create_async().await;
        let upload = server.mock("POST", "/api/workspaces").expect(0).create_async().await;
        let db = fresh_db();
        db.insert_workspace_sync("ws-late-pass", "new", None, None, None, None, None, None, None).unwrap();

        let guard = SyncGuard::unchecked().within(std::time::Duration::ZERO);
        let client = crate::sync_guard::sync_client();
        assert!(super::sync_with("token", &db, &guard, &client).await.is_err());
        list.assert_async().await;
        upload.assert_async().await;
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn the_account_list_is_fetched_once_per_push_however_many_rows_are_gone() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        let db = fresh_db();
        for (wid, sid) in [("ws-gone-a", "srv-gone-a"), ("ws-gone-b", "srv-gone-b")] {
            let row = db.insert_workspace_sync(wid, "old", None, None, None, None, None, None, None).unwrap();
            db.mark_workspace_sync_synced(row.id, Some(sid)).unwrap();
            db.update_workspace_sync_by_workspace_id(wid, "edited", None, None, None, None, None, None, None)
                .unwrap();
        }
        let _patch = server
            .mock("PATCH", mockito::Matcher::Regex("^/api/workspaces/srv-gone-".into()))
            .with_status(404)
            .create_async()
            .await;
        let list = server
            .mock("GET", "/api/workspaces")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"workspaces":[]}"#)
            .expect(1)
            .create_async()
            .await;

        super::push("token", &db).await.unwrap();
        list.assert_async().await;
        assert!(db.list_workspaces_sync().is_empty(), "both removals are settled");
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[test]
    #[serial]
    fn upsert_from_server_keeps_unpushed_edits_and_deletes() {
        let db = fresh_db();
        let server_copy = |id: &str, title: &str| {
            db.upsert_workspace_sync_from_server(
                id, title, None, None, None, Some("main"), None,
                "2026-01-01 00:00:00", "2026-01-01 00:00:00", None, None, None,
            )
            .unwrap();
        };
        for (workspace, sid) in [("workspace-1", "1"), ("workspace-2", "2"), ("workspace-3", "3")] {
            let row = db
                .insert_workspace_sync(workspace, "old", None, None, None, Some("main"), None, None, None)
                .unwrap();
            db.mark_workspace_sync_synced(row.id, Some(sid)).unwrap();
        }
        db.update_workspace_sync_by_workspace_id(
            "workspace-1", "renamed", None, None, None, Some("feature"), None, None, None,
        )
        .unwrap();
        db.soft_delete_workspace_sync_by_workspace_id("workspace-2").unwrap();

        // The server still has the old copy of each.
        for sid in ["1", "2", "3"] {
            server_copy(sid, "server title");
        }

        let rows = db.list_workspaces_sync_for_sync();
        assert_eq!(rows.len(), 3, "no duplicate rows");
        let row = |sid: &str| rows.iter().find(|r| r.server_id.as_deref() == Some(sid)).unwrap();
        assert_eq!(row("1").title, "renamed", "a rename is not reverted");
        assert_eq!(row("1").git_branch.as_deref(), Some("feature"));
        assert!(row("1").dirty, "and still goes up with the push");
        assert!(
            row("2").deleted_at.is_some() && row("2").dirty,
            "a closed workspace is not resurrected"
        );
        assert_eq!(row("3").title, "server title", "a clean row still takes the server copy");
        assert!(!row("3").dirty);
    }

    #[test]
    #[serial]
    fn upsert_from_server_is_idempotent_and_clean() {
        let db = fresh_db();
        // First call — inserts.
        db.upsert_workspace_sync_from_server(
            "200",
            "from-server",
            Some("7"),
            Some("/srv/path"),
            Some("git@x:y.git"),
            Some("main"),
            Some("aaa111"),
            "2026-01-01 00:00:00",
            "2026-01-01 00:00:00",
            None,
            Some("uid-200"),
            Some("main"),
        )
        .unwrap();
        let listed = db.list_workspaces_sync();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].server_id.as_deref(), Some("200"));
        assert_eq!(listed[0].title, "from-server");
        assert_eq!(listed[0].git_head_sha.as_deref(), Some("aaa111"));
        assert_eq!(listed[0].project_uid.as_deref(), Some("uid-200"));
        assert_eq!(listed[0].workspace_kind.as_deref(), Some("main"));
        assert!(!listed[0].dirty, "server-sourced rows must be clean");

        // Second call — same server_id but a new updated_at and new
        // title. Should UPDATE in place (no duplicate row).
        db.upsert_workspace_sync_from_server(
            "200",
            "renamed-server-side",
            Some("7"),
            Some("/srv/path"),
            Some("git@x:y.git"),
            Some("dev"),
            Some("bbb222"),
            "2026-01-01 00:00:00",
            "2026-01-02 00:00:00",
            None,
            Some("uid-200"),
            Some("worktree"),
        )
        .unwrap();
        let listed = db.list_workspaces_sync();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "renamed-server-side");
        assert_eq!(listed[0].git_branch.as_deref(), Some("dev"));
        assert_eq!(
            listed[0].workspace_kind.as_deref(),
            Some("worktree"),
            "server pull updates workspace_kind in place"
        );
    }

    #[test]
    #[serial]
    fn dedupe_collapses_cross_device_duplicate_siblings() {
        let db = fresh_db();
        // The cross-device race: two cloud rows for ONE physical workspace,
        // both un-adopted siblings, same host + project_uid + branch + kind,
        // origin_uid null (it's local-only, absent on cloud-pulled rows).
        db.upsert_workspace_sync_from_server(
            "S1", "proj", Some("host-1"), Some("/srv/proj"), None,
            Some("main"), None, "t", "t", None, Some("puid-1"), Some("main"),
        )
        .unwrap();
        db.upsert_workspace_sync_from_server(
            "S2", "proj", Some("host-1"), Some("/srv/proj"), None,
            Some("main"), None, "t", "t", None, Some("puid-1"), Some("main"),
        )
        .unwrap();
        assert_eq!(db.list_workspaces_sync().len(), 2, "two dup cards before");

        super::dedupe_sibling_rows(&db);

        // Exactly one survives — the lowest server_id (S1); S2 is tombstoned.
        let live = db.list_workspaces_sync();
        assert_eq!(live.len(), 1, "duplicate collapsed to one");
        assert_eq!(live[0].server_id.as_deref(), Some("S1"));

        // An ADOPTED row sharing the same identity is NOT a dedup candidate.
        db.link_workspace_sync_to_local("S1", "ws-local").unwrap();
        db.upsert_workspace_sync_from_server(
            "S3", "proj", Some("host-1"), Some("/srv/proj"), None,
            Some("main"), None, "t", "t", None, Some("puid-1"), Some("main"),
        )
        .unwrap();
        super::dedupe_sibling_rows(&db);
        // S1 (adopted) + S3 (sibling) both remain — adopted rows are spared.
        assert_eq!(db.list_workspaces_sync().len(), 2);
    }

    #[test]
    #[serial]
    fn link_local_id_after_adoption() {
        let db = fresh_db();
        // Server gave us a row without a local id (we discovered it
        // via pull — another device created it).
        db.upsert_workspace_sync_from_server(
            "300",
            "discovered",
            Some("12"),
            None,
            None,
            None,
            None,
            "2026-01-01 00:00:00",
            "2026-01-01 00:00:00",
            None,
            None,
            None,
        )
        .unwrap();
        let listed = db.list_workspaces_sync();
        assert!(listed[0].workspace_id.is_none());

        // Adopt: stamp the local workspace id.
        db.link_workspace_sync_to_local("300", "workspace-42").unwrap();
        let listed = db.list_workspaces_sync();
        assert_eq!(listed[0].workspace_id.as_deref(), Some("workspace-42"));
    }

    #[test]
    #[serial]
    fn unlink_reverts_adopted_row_to_remote_only() {
        // Adoption-failure rollback: after linking a row to a local
        // shell, if the pull fails we must REVERT the row to a sibling
        // (workspace_id NULL) so reconcile doesn't tombstone it and the
        // overview keeps offering "Pull to this device".
        let db = fresh_db();
        db.upsert_workspace_sync_from_server(
            "300",
            "discovered",
            Some("12"),
            None,
            None,
            None,
            None,
            "2026-01-01 00:00:00",
            "2026-01-01 00:00:00",
            None,
            None,
            None,
        )
        .unwrap();
        db.link_workspace_sync_to_local("300", "workspace-42").unwrap();
        assert_eq!(
            db.list_workspaces_sync()[0].workspace_id.as_deref(),
            Some("workspace-42")
        );

        db.unlink_workspace_sync_from_local("300").unwrap();
        let listed = db.list_workspaces_sync();
        assert!(
            listed[0].workspace_id.is_none(),
            "unlink must revert the row to a remote-only sibling"
        );
    }

    #[test]
    #[serial]
    fn find_by_host_and_origin_uid_excludes_soft_deleted() {
        // Contract: the live-row lookup must not return a tombstone — a
        // future production caller expecting "live" semantics would
        // otherwise reintroduce the resurrection class of bug. The
        // dedicated `find_remote_discovered_tombstone` is the only path
        // that may see soft-deleted rows.
        let db = fresh_db();
        let row = db
            .insert_remote_discovered_workspace_sync(
                "host-1", "uuid-abc", "ws", None, None, Some("main"), None, None,
                None, None,
            )
            .unwrap();
        assert!(db
            .find_workspace_sync_by_host_and_origin_uid("host-1", "uuid-abc")
            .is_some());

        db.soft_delete_remote_discovered_workspace_sync_by_id(row.id)
            .unwrap();
        assert!(
            db.find_workspace_sync_by_host_and_origin_uid("host-1", "uuid-abc")
                .is_none(),
            "live lookup must not surface a soft-deleted row"
        );
        assert!(
            db.find_remote_discovered_tombstone("host-1", "uuid-abc")
                .is_some(),
            "the tombstone lookup still finds it (for undelete-on-reappear)"
        );
    }

    // ── reconcile_from_snapshot regression guards ───────────────
    //
    // These exercise the full reconcile bridge, not just the DB
    // mutations underneath. The motivating regression is the close
    // path: when a workspace is closed locally, the `workspaces_sync`
    // row MUST be soft-deleted in the same tick so the overview
    // doesn't briefly render the closed workspace as "lives on
    // another device" (see use-overview-items.ts step 2).

    fn make_ws(id: &str, title: &str) -> crate::state::WorkspaceSnapshot {
        use crate::state::*;
        WorkspaceSnapshot {
            workspace_id: WorkspaceId(id.to_string()),
            is_git: true,
            title: title.to_string(),
            workspace_type: WorkspaceType::Standard,
            cwd: "/tmp".into(),
            git_branch: None,
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            notification_count: 0,
            latest_agent_state: None,
            worktree_path: None,
            project_root: None,
            project_uid: None,
            workspace_kind: None,
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            pr_head_branch: None,
            prs: Vec::new(),
            base_branch: None,
            provider_kind: None,
            linked_issue: None,
            notifications_muted: false,
            pinned_at: None,
            tabs: Vec::new(),
            active_tab_id: String::new(),
            active_surface_id: SurfaceId(String::new()),
            surfaces: Vec::new(),
            host_id: None,
            remote_cwd: None,
            remote_root: None,
            attach_only: false,
            imported_snapshot_only: Some(false),
            last_active_at: None,
            last_visited_at: None,
            parent_workspace_id: None,
        }
    }

    fn make_snapshot(
        workspaces: Vec<crate::state::WorkspaceSnapshot>,
    ) -> crate::state::AppStateSnapshot {
        use crate::state::*;
        let active = workspaces
            .first()
            .map(|w| w.workspace_id.clone())
            .unwrap_or_else(|| WorkspaceId(String::new()));
        AppStateSnapshot {
            schema_version: 1,
            snapshot_revision: 0,
            snapshot_instance: String::new(),
            active_workspace_id: active,
            workspaces,
            terminal_sessions: Vec::new(),
            browser_sessions: Vec::new(),
            agent_browser_sessions: Vec::new(),
            notifications: Vec::new(),
            detected_ports: Vec::new(),
            pane_statuses: std::collections::HashMap::new(),
            manual_monitors: std::collections::HashMap::new(),
            archived_workspaces: Vec::new(),
            persistence: PersistenceSchema {
                schema_version: 1,
                stores_layout_metadata: true,
                stores_terminal_metadata: true,
                stores_live_process_state: false,
            },
            config: CodemuxConfigSnapshot {
                config_version: 1,
                default_shell: None,
                theme_source: "system".into(),
                linux_first: false,
                notification_sound_enabled: false,
                ai_commit_message_enabled: false,
                ai_commit_message_cli: None,
                ai_commit_message_model: None,
                ai_resolver_enabled: false,
                ai_resolver_cli: None,
                ai_resolver_model: None,
                ai_resolver_strategy: "smart_merge".into(),
            },
        }
    }

    #[test]
    #[serial]
    fn reconcile_inserts_row_for_new_workspace() {
        let db = fresh_db();
        let snapshot = make_snapshot(vec![make_ws("workspace-A", "alpha")]);

        super::reconcile_from_snapshot(&db, &snapshot).unwrap();

        let list = db.list_workspaces_sync();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].workspace_id.as_deref(), Some("workspace-A"));
        assert_eq!(list[0].title, "alpha");
        assert!(list[0].dirty, "freshly inserted row must be dirty");
    }

    #[test]
    #[serial]
    fn reconcile_skips_attach_in_place_workspaces() {
        // An "open on host" workspace is a local VIEW onto a host workspace
        // the inventory poller already publishes. Mirroring it here would
        // create a duplicate cloud row that phantoms on every other device.
        // The reconcile must skip it entirely (like Home).
        let db = fresh_db();
        let mut attach = make_ws("workspace-onhost", "remote-svc");
        attach.attach_only = true;
        attach.host_id = Some(3);
        attach.remote_cwd = Some("/srv/remote-svc".into());
        let snapshot = make_snapshot(vec![attach, make_ws("workspace-local", "local")]);

        super::reconcile_from_snapshot(&db, &snapshot).unwrap();

        let list = db.list_workspaces_sync();
        assert_eq!(list.len(), 1, "only the real local workspace is mirrored");
        assert_eq!(list[0].workspace_id.as_deref(), Some("workspace-local"));
    }

    #[test]
    #[serial]
    fn reconcile_threads_project_identity_from_snapshot() {
        // A local workspace stamped with project_uid + workspace_kind
        // (by `set_workspace_project_root` at create) must carry those
        // onto its sync row, so the overview groups by project and the
        // pull-conflict guard can match exactly.
        let db = fresh_db();
        let mut ws = make_ws("workspace-P", "passpage");
        ws.project_root = Some("/home/agent/projects/passpage".into());
        ws.project_uid = Some("uid-passpage".into());
        ws.workspace_kind = Some("main".into());
        let snapshot = make_snapshot(vec![ws]);

        super::reconcile_from_snapshot(&db, &snapshot).unwrap();

        let row = &db.list_workspaces_sync()[0];
        assert_eq!(row.project_uid.as_deref(), Some("uid-passpage"));
        assert_eq!(row.workspace_kind.as_deref(), Some("main"));

        // Idempotent: an identical reconcile must not re-dirty the row
        // (otherwise every tick would burn a cloud PATCH).
        db.mark_workspace_sync_synced(row.id, Some("cloud-1")).unwrap();
        let mut ws2 = make_ws("workspace-P", "passpage");
        ws2.project_root = Some("/home/agent/projects/passpage".into());
        ws2.project_uid = Some("uid-passpage".into());
        ws2.workspace_kind = Some("main".into());
        super::reconcile_from_snapshot(&db, &make_snapshot(vec![ws2])).unwrap();
        assert!(
            !db.list_workspaces_sync()[0].dirty,
            "unchanged identity must not re-mark the row dirty"
        );
    }

    #[test]
    #[serial]
    fn reconcile_soft_deletes_row_when_workspace_closed() {
        // The regression guard for the "closed workspace briefly appears
        // as 'lives on another device'" bug. When a local workspace is
        // closed, the close path now calls reconcile_from_snapshot
        // immediately, which must soft-delete the orphan sync row so
        // the overview no longer treats it as a sibling-device row.
        let db = fresh_db();

        // First reconcile: workspace exists, row gets inserted.
        let with_ws =
            make_snapshot(vec![make_ws("workspace-doomed", "to-be-closed")]);
        super::reconcile_from_snapshot(&db, &with_ws).unwrap();
        assert_eq!(db.list_workspaces_sync().len(), 1);

        // Simulate close: snapshot no longer contains the workspace.
        let empty = make_snapshot(Vec::new());
        super::reconcile_from_snapshot(&db, &empty).unwrap();

        // The sync row must be soft-deleted: invisible to list(),
        // visible (with deleted_at + dirty) to list_for_sync() so the
        // next push can DELETE it on the server.
        assert_eq!(
            db.list_workspaces_sync().len(),
            0,
            "list_workspaces_sync must hide the soft-deleted row so \
             useOverviewItems can't classify it as a sibling-device row"
        );
        let all = db.list_workspaces_sync_for_sync();
        assert_eq!(all.len(), 1);
        assert!(all[0].deleted_at.is_some());
        assert!(all[0].dirty, "soft-delete must mark the row dirty for push");
    }

    #[test]
    #[serial]
    fn reconcile_preserves_pulled_only_rows() {
        // Sibling-device rows arrive via pull and have `workspace_id IS
        // NULL`. Reconcile must NEVER touch them — they have no
        // app_state counterpart by design. Otherwise closing a local
        // workspace could accidentally soft-delete a sibling row.
        let db = fresh_db();
        db.upsert_workspace_sync_from_server(
            "srv-99",
            "from-sibling",
            Some("host-7"),
            Some("/sibling/path"),
            None,
            Some("main"),
            None,
            "2026-01-01 00:00:00",
            "2026-01-01 00:00:00",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(db.list_workspaces_sync().len(), 1);

        let empty = make_snapshot(Vec::new());
        super::reconcile_from_snapshot(&db, &empty).unwrap();

        let after = db.list_workspaces_sync();
        assert_eq!(after.len(), 1, "pulled-only sibling row must survive");
        assert!(after[0].workspace_id.is_none());
        assert_eq!(after[0].title, "from-sibling");
    }

    // ── Host-inventory auto-publish helpers ─────────────────────
    //
    // These guard the contract that the inventory reconcile relies
    // on: `(host_server_id, origin_uid)` is a stable identity that
    // dedupes across repeated polls; the rest of the fields can
    // change in place without losing the cloud `server_id` once it's
    // been assigned by the first push.

    #[test]
    #[serial]
    fn insert_remote_discovered_row_is_dirty_and_carries_origin_uid() {
        let db = fresh_db();
        let row = db
            .insert_remote_discovered_workspace_sync(
                "host-1",
                "uuid-abc",
                "discovered-on-host",
                Some("/srv/discovered"),
                Some("git@github.com:user/repo.git"),
                Some("main"),
                Some("proj-uid-1"),
                Some("worktree"),
                Some("/srv/discovered/wt"),
                Some("trunk"),
            )
            .expect("insert remote-discovered");
        assert!(row.workspace_id.is_none(), "remote-discovered rows must have NULL workspace_id");
        assert!(row.server_id.is_none(), "no cloud server_id until first push");
        assert!(row.dirty, "row must be dirty so push uploads it");
        assert_eq!(row.host_server_id.as_deref(), Some("host-1"));
        assert_eq!(row.origin_uid.as_deref(), Some("uuid-abc"));
        assert_eq!(row.title, "discovered-on-host");
        assert_eq!(row.project_uid.as_deref(), Some("proj-uid-1"));
        assert_eq!(row.workspace_kind.as_deref(), Some("worktree"));
        assert_eq!(
            row.default_branch.as_deref(),
            Some("trunk"),
            "daemon-reported default_branch must round-trip through the sync row"
        );

        // The lookup function returns this row when queried by
        // (host, uid) and rejects mismatching pairs.
        let found = db.find_workspace_sync_by_host_and_origin_uid("host-1", "uuid-abc");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, row.id);

        assert!(
            db.find_workspace_sync_by_host_and_origin_uid("host-1", "uuid-other").is_none(),
            "wrong uid on the same host must not match"
        );
        assert!(
            db.find_workspace_sync_by_host_and_origin_uid("host-other", "uuid-abc").is_none(),
            "same uid on a different host must not match — UUIDs are only \
             unique within a single host's registry"
        );
    }

    #[test]
    #[serial]
    fn update_remote_discovered_keeps_server_id_and_marks_dirty() {
        // The poll reconcile must update mutable fields in place
        // without dropping the cloud `server_id` the first push
        // assigned — otherwise other devices would see a brand-new
        // workspace appear every poll cycle instead of seeing the
        // existing one update.
        let db = fresh_db();
        let row = db
            .insert_remote_discovered_workspace_sync(
                "host-7",
                "uuid-zeta",
                "first-title",
                None,
                None,
                Some("main"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        // Simulate the first push assigning a cloud server_id.
        db.mark_workspace_sync_synced(row.id, Some("9001")).unwrap();
        let after_push = db
            .find_workspace_sync_by_host_and_origin_uid("host-7", "uuid-zeta")
            .unwrap();
        assert_eq!(after_push.server_id.as_deref(), Some("9001"));
        assert!(!after_push.dirty);

        db.update_remote_discovered_workspace_sync(
            row.id,
            "renamed-on-host",
            Some("/srv/new"),
            Some("git@github.com:user/repo.git"),
            Some("feature/x"),
            Some("proj-uid-z"),
            Some("main"),
            Some("/srv/new/wt"),
            Some("main"),
        )
        .unwrap();

        let after_update = db
            .find_workspace_sync_by_host_and_origin_uid("host-7", "uuid-zeta")
            .unwrap();
        assert_eq!(after_update.title, "renamed-on-host");
        assert_eq!(after_update.git_branch.as_deref(), Some("feature/x"));
        assert_eq!(
            after_update.server_id.as_deref(),
            Some("9001"),
            "cloud server_id must survive a remote-discovered update"
        );
        assert!(
            after_update.dirty,
            "update must mark the row dirty so push propagates the change"
        );
        assert_eq!(
            after_update.host_server_id.as_deref(),
            Some("host-7"),
            "host_server_id is identity — must not move on update"
        );
        assert_eq!(
            after_update.origin_uid.as_deref(),
            Some("uuid-zeta"),
            "origin_uid is identity — must not move on update"
        );
    }

    #[test]
    #[serial]
    fn list_remote_discovered_for_host_is_scoped() {
        let db = fresh_db();
        db.insert_remote_discovered_workspace_sync("host-A", "uid-1", "a1", None, None, None, None, None, None, None)
            .unwrap();
        db.insert_remote_discovered_workspace_sync("host-A", "uid-2", "a2", None, None, None, None, None, None, None)
            .unwrap();
        db.insert_remote_discovered_workspace_sync("host-B", "uid-3", "b1", None, None, None, None, None, None, None)
            .unwrap();
        // A local-on-this-device row with no origin_uid must not
        // appear in the host scope, even if it carries the same
        // host_server_id (e.g. a workspace pushed from this device).
        db.insert_workspace_sync(
            "workspace-local",
            "local-pushed",
            Some("host-A"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let a = db.list_remote_discovered_for_host("host-A");
        assert_eq!(a.len(), 2, "scoped to host A and only origin_uid IS NOT NULL");
        let titles: Vec<&str> = a.iter().map(|r| r.title.as_str()).collect();
        assert!(titles.contains(&"a1") && titles.contains(&"a2"));

        let b = db.list_remote_discovered_for_host("host-B");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].title, "b1");

        let empty = db.list_remote_discovered_for_host("host-nonexistent");
        assert!(empty.is_empty());
    }

    #[test]
    #[serial]
    fn soft_delete_remote_discovered_marks_dirty_and_hides_from_overview() {
        let db = fresh_db();
        let row = db
            .insert_remote_discovered_workspace_sync(
                "host-Z",
                "uid-doomed",
                "to-be-deleted",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        db.mark_workspace_sync_synced(row.id, Some("5555")).unwrap();
        assert_eq!(db.list_remote_discovered_for_host("host-Z").len(), 1);
        assert_eq!(db.list_workspaces_sync().len(), 1);

        db.soft_delete_remote_discovered_workspace_sync_by_id(row.id)
            .unwrap();

        assert_eq!(
            db.list_remote_discovered_for_host("host-Z").len(),
            0,
            "soft-deleted rows must drop out of the per-host live list"
        );
        assert_eq!(
            db.list_workspaces_sync().len(),
            0,
            "overview must hide soft-deleted remote-discovered rows immediately"
        );
        let tombstone = db
            .list_workspaces_sync_for_sync()
            .into_iter()
            .find(|r| r.id == row.id)
            .expect("tombstone row still present in *_for_sync");
        assert!(tombstone.deleted_at.is_some());
        assert!(tombstone.dirty, "tombstone must be dirty so push DELETEs the cloud row");
        assert_eq!(
            tombstone.server_id.as_deref(),
            Some("5555"),
            "cloud server_id must survive the soft-delete so push knows the target"
        );
    }

    #[test]
    #[serial]
    fn delete_a_row_that_never_synced_is_a_local_no_op() {
        // A row created and deleted before any push happens has no
        // server_id. push_delete should clear dirty without an HTTP
        // call so the next purge removes it locally.
        let db = fresh_db();
        let row = db
            .insert_workspace_sync(
                "workspace-99",
                "ephemeral",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        db.soft_delete_workspace_sync_by_workspace_id("workspace-99")
            .unwrap();
        // Simulate what push_delete does for never-synced rows:
        db.mark_workspace_sync_synced(row.id, None).unwrap();
        db.purge_acknowledged_workspace_sync_deletes().unwrap();
        assert_eq!(db.list_workspaces_sync_for_sync().len(), 0);
    }
}
