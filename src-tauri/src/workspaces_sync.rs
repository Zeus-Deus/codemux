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
pub async fn try_sync_with_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let db = app.state::<DatabaseStore>();
    let token = match valid_token(&db) {
        Some(t) => t,
        None => return Ok(()),
    };
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
    let base = api_base_url();
    let client = reqwest::Client::new();
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
    // clean).
    for local_row in &local {
        if let Some(sid) = &local_row.server_id {
            if !server_ids.contains(sid) && local_row.deleted_at.is_none() {
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
    let base = api_base_url();
    let client = reqwest::Client::new();
    let dirty = db.list_dirty_workspaces_sync();
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
                "[workspaces-sync] push failed for local id {}: {error}",
                row.id
            );
            any_failed = true;
        }
    }

    db.purge_acknowledged_workspace_sync_deletes()?;

    if any_failed {
        Err("one or more workspace pushes failed; see logs".into())
    } else {
        Ok(())
    }
}

async fn push_insert(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::WorkspaceSyncRecord,
    db: &DatabaseStore,
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
    let resp = client
        .post(format!("{base}/api/workspaces"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    let parsed: OneWorkspaceResponse =
        resp.json().await.map_err(|e| format!("Parse: {e}"))?;
    db.mark_workspace_sync_synced(row.id, Some(&parsed.workspace.id))?;
    Ok(())
}

async fn push_update(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::WorkspaceSyncRecord,
    db: &DatabaseStore,
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
    let resp = client
        .patch(format!("{base}/api/workspaces/{server_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    db.mark_workspace_sync_synced(row.id, None)?;
    Ok(())
}

async fn push_delete(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &crate::database::WorkspaceSyncRecord,
    db: &DatabaseStore,
) -> Result<(), String> {
    // A row that was created and deleted entirely while offline (no
    // server_id) has nothing to push — clear dirty so the next purge
    // pass hard-deletes it locally.
    let server_id = match &row.server_id {
        Some(sid) => sid,
        None => {
            db.mark_workspace_sync_synced(row.id, None)?;
            return Ok(());
        }
    };
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
    db.mark_workspace_sync_synced(row.id, None)?;
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
    let output = std::process::Command::new("git")
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
/// OpenFlow workspaces are excluded — they are ephemeral by design
/// and never persist beyond a single device, so syncing them would
/// just churn the registry. Same exclusion `save_persisted_state`
/// uses.
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
        // Skip OpenFlow workspaces — they're not user-durable.
        if matches!(
            ws.workspace_type,
            crate::state::WorkspaceType::OpenFlow
        ) {
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
            linked_issue: None,
            notifications_muted: false,
            tabs: Vec::new(),
            active_tab_id: String::new(),
            active_surface_id: SurfaceId(String::new()),
            surfaces: Vec::new(),
            host_id: None,
            remote_cwd: None,
            attach_only: false,
            last_active_at: None,
            last_visited_at: None,
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
        // The reconcile must skip it entirely (like OpenFlow/Home).
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
