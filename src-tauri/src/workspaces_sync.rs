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
pub async fn try_sync_with_app(app: &tauri::AppHandle) -> Result<(), String> {
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
                )?;
            }
        }
    }

    Ok(())
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
                || existing.git_head_sha.as_deref() != git_head_sha_ref;
            if changed {
                if let Err(error) = db.update_workspace_sync_by_workspace_id(
                    &wid,
                    title,
                    host_server_id.as_deref(),
                    project_path,
                    project_remote,
                    git_branch,
                    git_head_sha_ref,
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
            )
            .expect("insert");
        assert!(row.dirty);
        assert!(row.server_id.is_none());
        assert_eq!(row.workspace_id.as_deref(), Some("workspace-1"));
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
            .insert_workspace_sync("workspace-2", "before", None, None, None, None, None)
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
            .insert_workspace_sync("workspace-3", "doomed", None, None, None, None, None)
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
        )
        .unwrap();
        let listed = db.list_workspaces_sync();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].server_id.as_deref(), Some("200"));
        assert_eq!(listed[0].title, "from-server");
        assert_eq!(listed[0].git_head_sha.as_deref(), Some("aaa111"));
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
        )
        .unwrap();
        let listed = db.list_workspaces_sync();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "renamed-server-side");
        assert_eq!(listed[0].git_branch.as_deref(), Some("dev"));
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
