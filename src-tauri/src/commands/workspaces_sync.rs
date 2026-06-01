//! Tauri commands for the cross-device workspace sync surface.
//!
//! The frontend reads `workspaces_sync_list` to render the
//! Workspaces overview — including workspaces that live on *other*
//! devices of the same account (rows with `workspace_id: null`).
//! Sibling-device rows show up as "lives on another device" cards
//! and can be adopted into this device via `workspaces_sync_adopt`.

use serde::Serialize;
use tauri::{Manager, State};

use crate::database::{DatabaseStore, WorkspaceSyncRecord};

/// Wire shape sent to the frontend. Matches the field naming the
/// existing UI expects (snake_case from Rust, camelCase from
/// serde-rename on JS-facing structs would be inconsistent with the
/// rest of the codebase — keep snake_case throughout).
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSyncView {
    /// Local sync-table row id. Stable across pulls within a device.
    pub id: i64,
    /// Server-assigned id. None until the row's first successful push.
    pub server_id: Option<String>,
    /// Local workspace_id this row corresponds to. None for rows
    /// pulled from sibling devices that haven't been adopted here.
    pub workspace_id: Option<String>,
    pub title: String,
    /// Server-side host id (matches `HostView.server_id`). None means
    /// the workspace is local to the device that authored it.
    pub host_server_id: Option<String>,
    pub project_path: Option<String>,
    pub project_remote: Option<String>,
    pub git_branch: Option<String>,
    /// Phase-4 divergence detection: HEAD sha at last reconcile.
    pub git_head_sha: Option<String>,
    /// Deterministic project identity (`UUIDv5`); see
    /// `crate::project_identity`. The overview groups workspaces that
    /// share this so a project's main checkout and its worktrees read
    /// as one project. May be null on rows that haven't been stamped
    /// (e.g. pulled from a sibling device pre-Phase-2).
    pub project_uid: Option<String>,
    /// `"main"` | `"worktree"` — the overview renders a kind badge.
    pub workspace_kind: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// True when the row has unpushed changes. The UI shows a
    /// "Pending sync" pill for these.
    pub dirty: bool,
}

impl From<WorkspaceSyncRecord> for WorkspaceSyncView {
    fn from(r: WorkspaceSyncRecord) -> Self {
        WorkspaceSyncView {
            id: r.id,
            server_id: r.server_id,
            workspace_id: r.workspace_id,
            title: r.title,
            host_server_id: r.host_server_id,
            project_path: r.project_path,
            project_remote: r.project_remote,
            git_branch: r.git_branch,
            git_head_sha: r.git_head_sha,
            project_uid: r.project_uid,
            workspace_kind: r.workspace_kind,
            created_at: r.created_at,
            updated_at: r.updated_at,
            dirty: r.dirty,
        }
    }
}

/// List every workspace this account knows about (across all devices)
/// that this device has either authored or pulled from the server.
/// Tombstones are excluded — only live rows are returned.
///
/// Rendering rules for the UI:
/// - `workspace_id` set → the workspace also exists in this device's
///   app_state; cross-reference to render the rich local card.
/// - `workspace_id` null → lives on another device; render a
///   minimal "remote" card with title + host + branch and offer the
///   "Pull to this device" affordance (when implemented).
#[tauri::command]
pub fn workspaces_sync_list(
    db: State<'_, DatabaseStore>,
) -> Vec<WorkspaceSyncView> {
    db.list_workspaces_sync()
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Trigger an immediate sync pass (pull + push). Returns when both
/// halves have finished or one has errored. Use sparingly — the
/// background loop already runs every 30 seconds.
///
/// Returns Ok(()) when the user is not signed in (sync is a no-op,
/// not an error in that case — matches the hosts and automations
/// commands).
#[tauri::command]
pub async fn workspaces_sync_now(
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Snapshot + reconcile in a sync block so we don't carry the
    // State<'_> guard (which is not Send) across the await below.
    {
        let db_state: tauri::State<'_, DatabaseStore> = app.state();
        let app_state: tauri::State<'_, crate::state::AppStateStore> =
            app.state();
        let snapshot = app_state.snapshot();
        crate::workspaces_sync::reconcile_from_snapshot(
            db_state.inner(),
            &snapshot,
        )?;
    }
    crate::workspaces_sync::try_sync_with_app(&app).await
}

// ─── Cross-device adoption ──────────────────────────────────────
//
// "Adoption" is the act of taking a workspace that exists on another
// device of the same account and materialising a local copy of it
// on this device. The synced row in `workspaces_sync` already carries
// the metadata (title, host_server_id, project_path, branch); the
// adoption flow's job is to:
//
//   1. Validate the user actually CAN adopt (host configured,
//      target path free, etc.).
//   2. Create a local workspace shell.
//   3. Link the sync row to the new local workspace_id.
//   4. Drive the existing pull-back rsync to populate the files.
//
// Two adoption paths exist:
//   - **Host-backed** (`host_server_id` is set on the sync row): the
//     workspace lives on a host this device knows about; we rsync
//     from that host. Implemented in `workspaces_adopt_synced` below.
//   - **Clone fallback** (`host_server_id` is null, no shared host):
//     the workspace lives on a sibling device only; we'd need to
//     `git clone` the project_remote into a fresh worktree. Tracked
//     for Phase 3, not yet implemented.

/// Snapshot of "can we adopt this synced workspace, and how?". The
/// frontend calls this when the Pull dialog opens so it can decide
/// which variant to render (host-backed vs clone fallback) and
/// surface the suggested target path without a second round-trip.
///
/// Field semantics:
/// - `can_host_adopt`: true when the sync row's `host_server_id`
///   resolves to a configured local host. The headline pull path.
/// - `can_clone_adopt`: true when the sync row has a `project_remote`
///   set. Reserved for Phase 3; this command surfaces the flag so
///   the dialog can hint at the option even before it ships.
/// - `host_configured`: same as `can_host_adopt` but separately
///   named so the dialog can show "Configure your host first" copy
///   when the row HAS a host_server_id but this device doesn't know
///   it yet.
/// - `host_label`: friendly name to render in the dialog summary
///   (`"From devbox"`). Null when no host applies.
/// - `project_already_cloned_at`: Phase-3 hint. Always None today.
/// - `suggested_path`: the canonical local worktree path the rsync
///   will land at. Pre-computed so the dialog can show it.
/// - `is_path_in_use`: when the suggested path already belongs to a
///   different local workspace. Bare-flag for now; the eventual
///   dialog UX offers `pickFolderDialog()` to choose an alternate.
/// - `already_adopted_workspace_id`: short-circuit — if the user
///   clicks Pull on a row they've already adopted, the dialog can
///   skip itself and just open the existing workspace.
/// - `same_branch_project_exists_at`: stronger conflict guard than
///   `is_path_in_use`. Set to the local `workspace_id` of any other
///   workspace this device has open whose `(basename(project_path),
///   git_branch)` matches the row being previewed. That tuple is the
///   cross-device identity for "same branch of the same project,"
///   even when the two devices store the repo at different absolute
///   paths. Pulling on top of such a row would silently create a
///   parallel copy of work the user is already doing — so the dialog
///   disables Pull and points at the existing workspace instead. Null
///   when no conflict is detected.
#[derive(Debug, Clone, Serialize)]
pub struct AdoptionPreview {
    pub can_host_adopt: bool,
    pub can_clone_adopt: bool,
    pub host_configured: bool,
    pub host_label: Option<String>,
    pub project_already_cloned_at: Option<String>,
    pub suggested_path: String,
    pub is_path_in_use: bool,
    pub already_adopted_workspace_id: Option<String>,
    pub same_branch_project_exists_at: Option<String>,
}

/// Successful adoption result. The frontend uses `workspace_id` to
/// navigate to the newly-adopted workspace (or to surface a "Open
/// workspace" CTA in the success toast).
#[derive(Debug, Clone, Serialize)]
pub struct AdoptOutcome {
    pub workspace_id: String,
    pub worktree_path: String,
    pub message: String,
}

#[tauri::command]
pub fn workspaces_adoption_preview(
    app: tauri::AppHandle,
    server_id: String,
) -> Result<AdoptionPreview, String> {
    let db: tauri::State<'_, DatabaseStore> = app.state();
    let app_state: tauri::State<'_, crate::state::AppStateStore> =
        app.state();

    // Locate the synced row by its server-assigned id.
    let row = db
        .list_workspaces_sync_for_sync()
        .into_iter()
        .find(|r| r.server_id.as_deref() == Some(server_id.as_str()))
        .ok_or_else(|| format!("Synced workspace not found: {server_id}"))?;

    // Host resolution: synced row carries `host_server_id`; we map
    // it to our local hosts row via the matching `hosts.server_id`.
    let (host_configured, host_label, local_host_id) = match row
        .host_server_id
        .as_deref()
    {
        Some(hsid) => {
            let local_host = db
                .list_hosts()
                .into_iter()
                .find(|h| h.server_id.as_deref() == Some(hsid));
            match local_host {
                Some(h) => (true, Some(h.name.clone()), Some(h.id)),
                None => (false, None, None),
            }
        }
        None => (false, None, None),
    };

    // Phase-3 placeholder: clone adoption is unimplemented for now.
    let can_clone_adopt = row.project_remote.is_some();

    // Compute the canonical local worktree path. Reuses the same
    // helper that pushes use, so a workspace pushed from this device
    // and adopted on another lands at structurally identical paths
    // on both sides.
    // Prefer the project_path basename; fall back to the workspace
    // title when the originating host never recorded a project root
    // (e.g. a root/main checkout created via the MCP `workspace_create`
    // tool). The title is always present, so a synced workspace never
    // collapses to the generic `~/.codemux/worktrees/workspace/<branch>`
    // landing path. `conventional_remote_path` still guards the empty
    // case as a last resort.
    let project_name = row
        .project_path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(row.title.as_str());
    let branch = row.git_branch.as_deref().unwrap_or("main");
    let conv = crate::workspace_paths::conventional_remote_path(project_name, branch);
    let conv_str = conv.to_string_lossy().to_string();
    // The conventional path uses `~/` — expand for the local check.
    let suggested_path = if let Some(rest) = conv_str.strip_prefix("~/") {
        dirs::home_dir()
            .map(|h| h.join(rest).to_string_lossy().to_string())
            .unwrap_or(conv_str.clone())
    } else {
        conv_str.clone()
    };

    // Detect path conflict against other live workspaces.
    let snapshot = app_state.snapshot();
    let is_path_in_use = snapshot.workspaces.iter().any(|w| {
        let p = w.worktree_path.as_deref().unwrap_or(w.cwd.as_str());
        p == suggested_path.as_str()
            && row
                .workspace_id
                .as_deref()
                .map_or(true, |adopted| w.workspace_id.0 != adopted)
    });

    // The set of workspace ids actually live in app_state right now.
    // The same-branch conflict guard below intersects against this so
    // a sync row still linked to a CLOSED workspace (its soft-delete
    // hasn't purged yet, or was resurrected by a pull) is not mistaken
    // for "you already have this branch open." Mirrors the liveness
    // check the overview (`use-overview-items`) and the adopt
    // idempotency path already apply.
    let live_workspace_ids: std::collections::HashSet<String> = snapshot
        .workspaces
        .iter()
        .map(|w| w.workspace_id.0.clone())
        .collect();

    let already_adopted_workspace_id = row.workspace_id.clone();

    // Cross-machine same-branch-same-project guard. The dialog uses
    // this to disable Pull and point the user at the existing local
    // workspace instead of silently creating a parallel copy of work
    // they're already doing on this device.
    //
    // We identify "same project" by the project_path basename (the
    // closest cross-machine identity we have without git remote URL
    // round-tripping — both devices may store the repo at different
    // absolute paths). "Same branch" is straightforward via
    // git_branch.
    //
    // Skip when the previewed row is itself the local one (already
    // adopted) — that's the `already_adopted_workspace_id` short-
    // circuit's job, not this guard's.
    let same_branch_project_exists_at = detect_same_branch_project_conflict(
        &db,
        &row,
        &live_workspace_ids,
    );

    Ok(AdoptionPreview {
        can_host_adopt: host_configured && local_host_id.is_some(),
        can_clone_adopt,
        host_configured,
        host_label,
        project_already_cloned_at: None,
        suggested_path,
        is_path_in_use,
        already_adopted_workspace_id,
        same_branch_project_exists_at,
    })
}

/// Walk every local sync row this device has a `workspace_id` for
/// and find one whose `(basename(project_path), git_branch)` matches
/// the previewed remote row. Returns the matching local workspace_id,
/// or None.
///
/// The basename match is deliberate — across two devices the same git
/// repo will almost always be checked out at paths that share a final
/// segment (`~/projects/foo` here, `/home/deus/projects/foo` there).
/// Comparing the full path would miss this almost every time. False
/// positives are bounded: the user would have to maintain two
/// repositories on this device with the same basename and the same
/// branch, both with `project_path` recorded — extremely rare in
/// practice.
///
/// `live_workspace_ids` is the set of workspace ids currently present
/// in app_state. A sync row's `workspace_id` is only treated as "this
/// device already has it" when that id is still live. Without this
/// guard a row left linked to a CLOSED workspace — its soft-delete not
/// yet purged, or resurrected by a `pull` from the server — would
/// surface a phantom "you already have this branch open" conflict and
/// block a legitimate re-pull. This matches the liveness filter the
/// overview (`use-overview-items`) and the adopt idempotency path
/// already apply, so the three surfaces agree on what counts as
/// locally present.
fn detect_same_branch_project_conflict(
    db: &DatabaseStore,
    previewed: &WorkspaceSyncRecord,
    live_workspace_ids: &std::collections::HashSet<String>,
) -> Option<String> {
    let previewed_branch = previewed.git_branch.as_deref()?;
    // Prefer the stable, deterministic project_uid when the previewed
    // row carries one (Phase 1 of the project-identity work). It's an
    // exact identity — no false positives from two unrelated repos that
    // happen to share a basename, and no false negatives when the same
    // repo lives at different paths on two devices. Fall back to the
    // legacy `basename(project_path)` heuristic for rows that predate
    // project_uid.
    let previewed_basename = previewed
        .project_path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|n| n.to_str());
    if previewed.project_uid.is_none() && previewed_basename.is_none() {
        return None;
    }

    for r in db.list_workspaces_sync() {
        // Only compare against rows that DO correspond to a local
        // workspace (workspace_id IS NOT NULL). Sibling-device rows
        // are not "this device has it already" — skip them.
        let Some(local_wid) = r.workspace_id.as_deref() else {
            continue;
        };
        // Skip rows whose linked workspace is no longer live in
        // app_state. A non-null `workspace_id` alone doesn't mean the
        // workspace still exists — it may have been closed/deleted with
        // a stale sync row left behind. Treating such an orphan as a
        // conflict is exactly the phantom "you already have this branch
        // open" bug this guard must avoid.
        if !live_workspace_ids.contains(local_wid) {
            continue;
        }
        // Don't flag the previewed row's own local copy.
        if previewed.workspace_id.as_deref() == Some(local_wid) {
            continue;
        }
        // Branch must match in all cases.
        if r.git_branch.as_deref() != Some(previewed_branch) {
            continue;
        }
        // Same project: exact project_uid when both sides have one,
        // otherwise the basename fallback.
        let same_project = match (previewed.project_uid.as_deref(), r.project_uid.as_deref()) {
            (Some(a), Some(b)) => a == b,
            _ => {
                let local_basename = r
                    .project_path
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .and_then(|n| n.to_str());
                previewed_basename.is_some() && local_basename == previewed_basename
            }
        };
        if !same_project {
            continue;
        }
        return Some(local_wid.to_string());
    }
    None
}

/// Adopt a sibling-device workspace into this device via the
/// host-backed rsync path.
///
/// Flow:
///   1. Look up the sync row by `server_id`.
///   2. Idempotency: if the row already has a local `workspace_id`,
///      return early with that id (no work to do; the user clicked
///      Pull on something already adopted).
///   3. Resolve `host_server_id` → local `hosts.id`. Fail with a
///      structured error if the host isn't on this device.
///   4. Compute the canonical local worktree path.
///   5. Bail if the path conflicts with a different workspace.
///   6. Create the local workspace shell with host_id pre-set
///      (`create_synced_workspace_shell`).
///   7. Link the sync row to the new local id
///      (`link_workspace_sync_to_local`).
///   8. Drive `workspace_pull_back_impl` — the exact same machinery
///      a manual pull-back uses. This rsyncs the remote worktree to
///      the local path, clears `host_id` on success, tears down the
///      SSH tunnel, respawns any PTY sessions locally.
///   9. Trigger a reconcile + sync push so the server learns the
///      workspace is now also on this device (and `host_server_id`
///      goes back to null because pull-back cleared it).
///
/// On pull-back failure the local shell is left in place with
/// `host_id` still set. The overview will render it under the host
/// bucket with the spinner and the user can retry via the regular
/// "Pull back to this device" action — the recovery path is the
/// same as a failed manual pull.
#[tauri::command]
pub async fn workspaces_adopt_synced(
    app: tauri::AppHandle,
    server_id: String,
) -> Result<AdoptOutcome, String> {
    // A `worktree` row can't be pulled as a standalone directory: its
    // `.git` is a gitfile pointing at a parent repo that doesn't exist
    // on this device, so a bare rsync of the worktree dir lands with
    // broken git. Adopt the PARENT repo (rsync incl. `.git`) and
    // recreate the linked worktree locally instead. `main`/standard
    // rows fall through to the single-dir pull below.
    let is_worktree = {
        let db: tauri::State<'_, DatabaseStore> = app.state();
        db.list_workspaces_sync_for_sync()
            .into_iter()
            .find(|r| r.server_id.as_deref() == Some(server_id.as_str()))
            .and_then(|r| r.workspace_kind)
            .as_deref()
            == Some("worktree")
    };
    if is_worktree {
        return adopt_worktree_via_repo_rsync(app, server_id).await;
    }

    // Step 1+2+3+4+5: do all the sync DB lookups in a scope so the
    // State<'_> guard is dropped before we await anything (Tauri's
    // State is not Send across awaits).
    let (workspace_id, worktree_path) = {
        let db: tauri::State<'_, DatabaseStore> = app.state();
        let app_state: tauri::State<'_, crate::state::AppStateStore> =
            app.state();

        let row = db
            .list_workspaces_sync_for_sync()
            .into_iter()
            .find(|r| r.server_id.as_deref() == Some(server_id.as_str()))
            .ok_or_else(|| format!("Synced workspace not found: {server_id}"))?;

        // Idempotency: if we've already adopted this row, return
        // the existing local workspace id rather than creating a
        // duplicate shell. The caller can then `activate_workspace`
        // on the returned id.
        if let Some(existing) = row.workspace_id.as_deref() {
            // Surface whether the workspace still exists in app_state
            // (it may have been closed locally even though the sync
            // row remembers the link). If gone, we fall through to
            // adoption again.
            let snapshot = app_state.snapshot();
            if snapshot
                .workspaces
                .iter()
                .any(|w| w.workspace_id.0 == existing)
            {
                let worktree = snapshot
                    .workspaces
                    .iter()
                    .find(|w| w.workspace_id.0 == existing)
                    .and_then(|w| w.worktree_path.clone().or_else(|| Some(w.cwd.clone())))
                    .unwrap_or_default();
                return Ok(AdoptOutcome {
                    workspace_id: existing.to_string(),
                    worktree_path: worktree,
                    message: "Workspace already adopted on this device.".into(),
                });
            }
        }

        // Resolve host_server_id → local hosts.id.
        let host_server_id = row.host_server_id.as_deref().ok_or_else(|| {
            "This workspace has no host; clone-based adoption is not \
             implemented yet (Phase 3 of the workspaces-sync rollout)."
                .to_string()
        })?;
        let local_host = db
            .list_hosts()
            .into_iter()
            .find(|h| h.server_id.as_deref() == Some(host_server_id))
            .ok_or_else(|| {
                format!(
                    "host_not_configured: The host this workspace lives on \
                     (host_server_id={host_server_id}) is not configured on \
                     this device yet. Add it in Settings → Devices."
                )
            })?;

        // Compute canonical local path. Mirrors the suggested_path
        // logic in `workspaces_adoption_preview` exactly — including the
        // title fallback — so the path the dialog previews is the path
        // adoption actually lands at.
        let project_name = row
            .project_path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .unwrap_or(row.title.as_str());
        let branch = row.git_branch.as_deref().unwrap_or("main");
        let conv = crate::workspace_paths::conventional_remote_path(project_name, branch);
        let conv_str = conv.to_string_lossy().to_string();
        let worktree_path = if let Some(rest) = conv_str.strip_prefix("~/") {
            dirs::home_dir()
                .map(|h| h.join(rest).to_string_lossy().to_string())
                .unwrap_or(conv_str.clone())
        } else {
            conv_str.clone()
        };

        // Path collision check — refuse to clobber a workspace that
        // already lives here.
        let snapshot = app_state.snapshot();
        let collision = snapshot.workspaces.iter().any(|w| {
            let p = w.worktree_path.as_deref().unwrap_or(w.cwd.as_str());
            p == worktree_path.as_str()
        });
        if collision {
            return Err(format!(
                "path_in_use: A different workspace is already using \
                 {worktree_path}. Close it first or choose another path."
            ));
        }

        // Create the local shell. host_id is set so the upcoming
        // pull-back call routes to the right host.
        let workspace_id = app_state.create_synced_workspace_shell(
            row.title.clone(),
            local_host.id,
            row.project_path.clone(),
            worktree_path.clone(),
            row.git_branch.clone(),
        );
        let workspace_id_str = workspace_id.0.clone();

        // Link the sync row to the new local id so subsequent
        // reconciles keep them paired and the row renders as a
        // local entry instead of "lives on another device".
        db.link_workspace_sync_to_local(&server_id, &workspace_id_str)?;

        (workspace_id_str, worktree_path)
    };

    // Step 8: drive the existing pull-back machinery. `_impl` takes
    // only `AppHandle` and resolves DB/state internally, so we can
    // await freely without juggling `tauri::State<'_>` guards
    // (which are not Send across awaits).
    let pull_outcome = crate::commands::hosts::workspace_pull_back_impl(
        app.clone(),
        workspace_id.clone(),
    )
    .await?;

    // Step 9: nudge the reconcile so the server learns the workspace
    // is now also on this device. Failures here are non-fatal — the
    // background loop will catch up within 30s.
    if let Err(e) = crate::workspaces_sync::try_sync_with_app(&app).await {
        eprintln!(
            "[workspaces-sync] post-adopt sync failed (will retry on next \
             background tick): {e}"
        );
    }

    Ok(AdoptOutcome {
        workspace_id,
        worktree_path,
        message: pull_outcome.message,
    })
}

/// Adopt a `worktree` row by materialising its PARENT repo locally and
/// recreating the linked worktree — the "whole-repo + recreate" model.
///
/// Why not just rsync the worktree dir: a worktree's `.git` is a gitfile
/// pointing at `<parent>/.git/worktrees/<name>`, a path that only exists
/// on the originating host. Copied alone it's a broken repo. And the
/// parent may be a **local-only** repo (no remote), so we can't clone it
/// — rsync is the only way to get the objects + branch refs.
///
/// Flow (mirrors `workspaces_adopt_via_clone`, but the source is an SSH
/// host path instead of a git remote URL):
///   1. Resolve the row + host; idempotent short-circuit if already adopted.
///   2. rsync the parent repo (incl. `.git`) from `project_path` on the
///      host into `~/.codemux/projects/<repo>` — skipped if a local repo
///      is already there (adopting a second worktree of the same project).
///   3. `git worktree prune` (drops the host's stale worktree entries),
///      then `git worktree add` for the branch into the canonical
///      `~/.codemux/worktrees/<repo>/<branch>` path.
///   4. Register a local workspace pointing at the recreated worktree and
///      link the sync row.
async fn adopt_worktree_via_repo_rsync(
    app: tauri::AppHandle,
    server_id: String,
) -> Result<AdoptOutcome, String> {
    // ── resolve row + host + paths (no awaits) ──
    let (host, remote_repo_path, local_project_path, branch, title) = {
        let db: tauri::State<'_, DatabaseStore> = app.state();
        let app_state: tauri::State<'_, crate::state::AppStateStore> = app.state();

        let row = db
            .list_workspaces_sync_for_sync()
            .into_iter()
            .find(|r| r.server_id.as_deref() == Some(server_id.as_str()))
            .ok_or_else(|| format!("Synced workspace not found: {server_id}"))?;

        // Idempotency: already adopted and still live → return it.
        if let Some(existing) = row.workspace_id.as_deref() {
            let snapshot = app_state.snapshot();
            if let Some(w) = snapshot
                .workspaces
                .iter()
                .find(|w| w.workspace_id.0 == existing)
            {
                let worktree = w
                    .worktree_path
                    .clone()
                    .unwrap_or_else(|| w.cwd.clone());
                return Ok(AdoptOutcome {
                    workspace_id: existing.to_string(),
                    worktree_path: worktree,
                    message: "Workspace already adopted on this device.".into(),
                });
            }
        }

        let host_server_id = row.host_server_id.as_deref().ok_or_else(|| {
            "This workspace has no host; clone-based adoption is not \
             implemented yet (Phase 3 of the workspaces-sync rollout)."
                .to_string()
        })?;
        let local_host = db
            .list_hosts()
            .into_iter()
            .find(|h| h.server_id.as_deref() == Some(host_server_id))
            .ok_or_else(|| {
                format!(
                    "host_not_configured: The host this workspace lives on \
                     (host_server_id={host_server_id}) is not configured on \
                     this device yet. Add it in Settings → Devices."
                )
            })?;

        // For a worktree row, `project_path` is the PARENT repo's path on
        // the host (the reconcile maps it from the daemon's project_root).
        // That is the rsync source — NOT `origin_path` (the worktree dir).
        let remote_repo_path = row.project_path.clone().ok_or_else(|| {
            "no_parent_repo: this worktree has no recorded parent repo path \
             on the host, so it can't be reconstructed. Re-poll the host or \
             adopt its main checkout first."
                .to_string()
        })?;
        let project_name = std::path::Path::new(&remote_repo_path)
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| row.title.clone());
        let branch = row.git_branch.clone().unwrap_or_else(|| "main".to_string());
        let home = dirs::home_dir().ok_or_else(|| "home directory unavailable".to_string())?;
        let local_project_path = home.join(".codemux").join("projects").join(&project_name);

        (
            local_host,
            remote_repo_path,
            local_project_path,
            branch,
            row.title.clone(),
        )
    };

    #[cfg(unix)]
    {
        // ── 2. rsync the parent repo (incl .git) unless already present ──
        let already_local = local_project_path.join(".git").exists();
        if !already_local {
            if let Err(e) = std::fs::create_dir_all(&local_project_path) {
                return Err(format!(
                    "could not create local project dir {}: {e}",
                    local_project_path.display()
                ));
            }
            let opts = crate::ssh::PullOptions::new(
                &host.ssh_target,
                &remote_repo_path,
                &local_project_path,
            );
            match crate::ssh::pull_workspace_back(opts).await {
                crate::ssh::PullResult::Pulled { .. } => {}
                crate::ssh::PullResult::RemoteNotFound { path } => {
                    let _ = std::fs::remove_dir_all(&local_project_path);
                    return Err(format!(
                        "Remote repo not found at {path}. The host may have been \
                         wiped or the project moved."
                    ));
                }
                crate::ssh::PullResult::RsyncFailed { reason } => {
                    return Err(format!("rsync failed: {reason}"));
                }
                crate::ssh::PullResult::HostUnreachable { reason } => {
                    return Err(format!("Host unreachable: {reason}"));
                }
            }
        }

        // ── 3. prune stale host worktrees + recreate the branch locally ──
        let repo_for_blocking = local_project_path.clone();
        let branch_for_blocking = branch.clone();
        let worktree_actual_path = tokio::task::spawn_blocking(move || {
            crate::git::git_recreate_worktree_for_adopted_repo(
                &repo_for_blocking,
                &branch_for_blocking,
            )
        })
        .await
        .map_err(|e| format!("worktree recreate join failed: {e}"))??;

        // ── 4. register a local workspace + link the sync row ──
        let app_state: tauri::State<'_, crate::state::AppStateStore> = app.state();
        let db: tauri::State<'_, DatabaseStore> = app.state();
        let local_project_str = local_project_path.to_string_lossy().to_string();
        let workspace_id = app_state.create_synced_workspace_shell(
            title,
            host.id,
            Some(local_project_str.clone()),
            worktree_actual_path.clone(),
            Some(branch.clone()),
        );
        // It's local right away (the files are already on disk) — clear
        // host_id so panes spawn against the local pty-daemon.
        app_state.set_workspace_host_id(&workspace_id.0, None)?;
        db.link_workspace_sync_to_local(&server_id, &workspace_id.0)?;
        drop(app_state);
        drop(db);

        if let Err(e) = crate::workspaces_sync::try_sync_with_app(&app).await {
            eprintln!(
                "[workspaces-sync] post-adopt (worktree) sync failed (will retry): {e}"
            );
        }
        crate::state::emit_app_state(&app);

        Ok(AdoptOutcome {
            workspace_id: workspace_id.0,
            worktree_path: worktree_actual_path,
            message: format!(
                "Adopted worktree: synced repo to {local_project_str} and \
                 recreated the '{branch}' worktree."
            ),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (host, remote_repo_path, local_project_path, branch, title);
        Err("SSH transport is Unix-only for now.".into())
    }
}

/// Adopt a sibling-device workspace into this device via the
/// clone-from-git fallback path. Used when the sync row has no
/// `host_server_id` (the workspace lives only on another device of
/// the user, never pushed to a shared host).
///
/// Semantics differ from `workspaces_adopt_synced` (Phase 2) in one
/// important way: this creates a NEW local workspace with its own
/// fresh `server_id` (the reconcile push assigns one). The
/// sibling-device's existing sync row is left alone. Result: both
/// devices independently own a workspace at the same git remote +
/// branch. They share a git remote but live as two registry
/// entries.
///
/// This semantics tradeoff is the right answer because rsync is not
/// available (no shared host), so the two devices genuinely have
/// independent copies. Phase-4's divergence detection (TBD) handles
/// the "now what?" question when their HEADs diverge.
///
/// Flow:
///   1. Look up the sync row by `server_id`, validate it has
///      `project_remote` set (clone is impossible without a URL).
///   2. Refuse if the sync row already has `workspace_id` set on
///      this device — that's the host-backed adoption's idempotent
///      path, not ours.
///   3. Compute target paths: project at `~/.codemux/projects/<basename>`,
///      worktree at `~/.codemux/worktrees/<basename>/<branch>`.
///   4. Bail if either path is in use (caller already saw the
///      preview's `is_path_in_use` flag, so this is defence-in-
///      depth).
///   5. `git clone --no-checkout` into the project path.
///   6. `git worktree add` for the branch into the worktree path —
///      reuse `git_create_worktree` so PR-ref + branch-fetch logic
///      is unchanged.
///   7. Create a brand-new local workspace (NOT linked to the
///      sync row). The next reconcile push will assign it a fresh
///      `server_id`.
///   8. Return the new local id.
///
/// Failure mode: any git failure leaves the target dir cleaned up
/// (see `git_clone`'s rollback). No partial workspace shell is
/// created until both clone + worktree-add succeed.
#[tauri::command]
pub async fn workspaces_adopt_via_clone(
    app: tauri::AppHandle,
    server_id: String,
) -> Result<AdoptOutcome, String> {
    // Validate + compute paths in a sync scope (no awaits).
    let (project_remote, project_path, worktree_path, branch, title) = {
        let db: tauri::State<'_, DatabaseStore> = app.state();
        let app_state: tauri::State<'_, crate::state::AppStateStore> =
            app.state();

        let row = db
            .list_workspaces_sync_for_sync()
            .into_iter()
            .find(|r| r.server_id.as_deref() == Some(server_id.as_str()))
            .ok_or_else(|| format!("Synced workspace not found: {server_id}"))?;

        // Idempotency / safety: refuse if the row is already linked
        // to a local workspace on this device. That row should be
        // adopted via the host-backed path, not cloned again.
        if row.workspace_id.is_some() {
            return Err(format!(
                "This workspace is already adopted on this device as {}",
                row.workspace_id.unwrap()
            ));
        }

        let project_remote = row.project_remote.as_deref().ok_or_else(|| {
            "no_project_remote: this workspace has no git remote URL \
             recorded, so it can't be cloned. Open the device it \
             lives on and push it to a shared device first."
                .to_string()
        })?;

        let project_name = row
            .project_path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .or_else(|| extract_project_name_from_remote(project_remote))
            // Final fallback if we can't infer a name from anywhere.
            .unwrap_or_else(|| "workspace".to_string());

        let branch = row
            .git_branch
            .clone()
            .unwrap_or_else(|| "main".to_string());

        // Project goes under ~/.codemux/projects/<basename>. We
        // deliberately avoid colliding with user-managed paths like
        // ~/projects/<basename>.
        let home = dirs::home_dir()
            .ok_or_else(|| "home directory unavailable".to_string())?;
        let project_path = home
            .join(".codemux")
            .join("projects")
            .join(&project_name);
        // Worktree path matches the canonical layout push/pull use,
        // so a future push to a shared host lands at the same
        // remote path another device would.
        let conv = crate::workspace_paths::conventional_remote_path(&project_name, &branch);
        let conv_str = conv.to_string_lossy().to_string();
        let worktree_path = if let Some(rest) = conv_str.strip_prefix("~/") {
            home.join(rest)
        } else {
            std::path::PathBuf::from(&conv_str)
        };

        // Path collision check against live workspaces. Defence-in-
        // depth — the preview already showed this; bailing here
        // catches races.
        let snapshot = app_state.snapshot();
        let collision = snapshot.workspaces.iter().any(|w| {
            let p = w.worktree_path.as_deref().unwrap_or(w.cwd.as_str());
            p == worktree_path.to_string_lossy().as_ref()
        });
        if collision {
            return Err(format!(
                "path_in_use: A different workspace is already using \
                 {}.",
                worktree_path.display()
            ));
        }

        (
            project_remote.to_string(),
            project_path,
            worktree_path,
            branch,
            row.title.clone(),
        )
    };

    // Step 5+6: clone + worktree-add, both off the main async
    // thread via spawn_blocking — git operations can take seconds.
    let project_path_str = project_path.to_string_lossy().to_string();
    let worktree_path_str = worktree_path.to_string_lossy().to_string();
    let branch_clone = branch.clone();
    let project_path_clone = project_path.clone();
    let clone_result = tokio::task::spawn_blocking(move || {
        // Phase 1: clone the bare-ish project (no checkout —
        // worktree-add picks the right branch).
        crate::git::git_clone(&project_remote, &project_path_clone)?;
        // Phase 2: add the worktree at the requested branch.
        crate::git::git_create_worktree(
            project_path_clone.as_path(),
            &branch_clone,
            // `new_branch=false` — we want the existing branch from
            // the remote, not a new one.
            false,
            None,
            None,
        )
    })
    .await
    .map_err(|e| format!("git_clone join failed: {e}"))?;

    let worktree_actual_path = clone_result.map_err(|e| {
        // Roll back the cloned project dir on failure so a retry
        // starts clean.
        let _ = std::fs::remove_dir_all(&project_path);
        e
    })?;

    // Step 7: register a brand-new local workspace pointing at the
    // freshly-cloned worktree. Crucially we do NOT link to the
    // existing sync row — the next reconcile push will create a
    // fresh server-side entry for this device's copy.
    let app_state: tauri::State<'_, crate::state::AppStateStore> = app.state();
    let workspace_id = app_state.create_synced_workspace_shell(
        title,
        // host_id = -1 sentinel doesn't exist; we use a different
        // helper that creates a local-only workspace. Since
        // create_synced_workspace_shell expects host_id and we
        // don't want that here, fall back to manually setting it
        // via the closest existing helper.
        0,
        Some(project_path_str.clone()),
        worktree_actual_path.clone(),
        Some(branch.clone()),
    );
    // Clear host_id since this is a local clone (not a remote pull).
    app_state.set_workspace_host_id(&workspace_id.0, None)?;

    // Nudge sync so the server learns about the new workspace.
    if let Err(e) = crate::workspaces_sync::try_sync_with_app(&app).await {
        eprintln!(
            "[workspaces-sync] post-clone-adopt sync failed (will retry): {e}"
        );
    }

    Ok(AdoptOutcome {
        workspace_id: workspace_id.0,
        worktree_path: worktree_actual_path,
        message: format!(
            "Cloned to {} and added worktree at {}",
            project_path_str, worktree_path_str
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DatabaseStore;
    use serial_test::serial;

    // ── detect_same_branch_project_conflict ────────────────────
    //
    // Guards the cross-machine "Pull would clobber work I'm already
    // doing locally on the same branch" check. The detection is
    // deliberately basename-based because two devices almost never
    // store the same repo at the same absolute path.

    /// Build the live-workspace-id set the guard intersects against.
    fn live_set(ids: &[&str]) -> std::collections::HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn insert_local(
        db: &DatabaseStore,
        workspace_id: &str,
        project_path: Option<&str>,
        branch: Option<&str>,
    ) {
        db.insert_workspace_sync(
            workspace_id,
            "local-side",
            None,
            project_path,
            None,
            branch,
            None,
            None,
            None,
        )
        .unwrap();
    }

    fn make_remote_row(
        project_path: Option<&str>,
        branch: Option<&str>,
        adopted_as: Option<&str>,
    ) -> WorkspaceSyncRecord {
        WorkspaceSyncRecord {
            id: 0,
            server_id: Some("srv-x".into()),
            workspace_id: adopted_as.map(|s| s.into()),
            title: "remote-side".into(),
            host_server_id: Some("pandora".into()),
            project_path: project_path.map(|s| s.into()),
            project_remote: None,
            git_branch: branch.map(|s| s.into()),
            git_head_sha: None,
            origin_uid: Some("uuid-1".into()),
            project_uid: None,
            workspace_kind: None,
            origin_path: project_path.map(|s| s.into()),
            created_at: "2026-01-01 00:00:00".into(),
            updated_at: "2026-01-01 00:00:00".into(),
            deleted_at: None,
            dirty: false,
        }
    }

    #[test]
    #[serial]
    fn conflict_detection_matches_on_basename_and_branch() {
        let db = DatabaseStore::new_in_memory();
        // Same project name as the remote, same branch, different
        // absolute path (the realistic cross-device case).
        insert_local(
            &db,
            "workspace-local",
            Some("/home/zeus/projects/my-repo"),
            Some("feature/x"),
        );

        let remote = make_remote_row(
            Some("/home/deus/projects/my-repo"),
            Some("feature/x"),
            None,
        );
        let hit = detect_same_branch_project_conflict(
            &db,
            &remote,
            &live_set(&["workspace-local"]),
        );
        assert_eq!(
            hit.as_deref(),
            Some("workspace-local"),
            "matching basename + matching branch must surface the local workspace id"
        );
    }

    #[test]
    #[serial]
    fn conflict_detection_ignores_orphan_closed_workspace() {
        // Regression: the local row still references a workspace that
        // was closed/deleted, so its id is NOT in app_state. Pulling
        // the same branch back from a host must NOT be blocked by this
        // stale link — the overview already hides such orphans, and
        // this guard must agree instead of showing a phantom "you
        // already have this branch open" message.
        let db = DatabaseStore::new_in_memory();
        insert_local(
            &db,
            "workspace-closed",
            Some("/home/zeus/projects/my-repo"),
            Some("main"),
        );
        let remote = make_remote_row(
            Some("/home/deus/projects/my-repo"),
            Some("main"),
            None,
        );
        // Empty live set → the workspace is gone from app_state.
        assert!(
            detect_same_branch_project_conflict(&db, &remote, &live_set(&[]))
                .is_none(),
            "an orphaned sync row for a closed workspace must not flag as a conflict"
        );
    }

    #[test]
    #[serial]
    fn conflict_detection_ignores_mismatched_branch() {
        let db = DatabaseStore::new_in_memory();
        insert_local(
            &db,
            "workspace-local",
            Some("/home/zeus/projects/my-repo"),
            Some("main"),
        );
        // Same project, different branch → not a conflict; two
        // branches can legitimately exist as separate worktrees.
        let remote = make_remote_row(
            Some("/home/deus/projects/my-repo"),
            Some("feature/x"),
            None,
        );
        assert!(detect_same_branch_project_conflict(
            &db,
            &remote,
            &live_set(&["workspace-local"]),
        )
        .is_none());
    }

    #[test]
    #[serial]
    fn conflict_detection_ignores_mismatched_basename() {
        let db = DatabaseStore::new_in_memory();
        insert_local(
            &db,
            "workspace-local",
            Some("/home/zeus/projects/different-repo"),
            Some("feature/x"),
        );
        let remote = make_remote_row(
            Some("/home/deus/projects/my-repo"),
            Some("feature/x"),
            None,
        );
        assert!(detect_same_branch_project_conflict(
            &db,
            &remote,
            &live_set(&["workspace-local"]),
        )
        .is_none());
    }

    #[test]
    #[serial]
    fn conflict_detection_ignores_the_rows_own_local_copy() {
        // If the remote row was already adopted on this device,
        // there's a local row that matches (basename, branch).
        // That match is not a conflict — the `already_adopted_*`
        // short-circuit is the right surface for it.
        let db = DatabaseStore::new_in_memory();
        insert_local(
            &db,
            "workspace-local",
            Some("/home/zeus/projects/my-repo"),
            Some("feature/x"),
        );
        let remote = make_remote_row(
            Some("/home/deus/projects/my-repo"),
            Some("feature/x"),
            Some("workspace-local"),
        );
        assert!(
            detect_same_branch_project_conflict(
                &db,
                &remote,
                &live_set(&["workspace-local"]),
            )
            .is_none(),
            "the row's own adopted copy must not flag as a conflict"
        );
    }

    #[test]
    #[serial]
    fn conflict_detection_ignores_sibling_only_rows() {
        // Another sibling-device row with the same basename + branch
        // is NOT a "this device already has it" conflict — it lives
        // somewhere else. Skip rows without a local workspace_id.
        let db = DatabaseStore::new_in_memory();
        // A sibling-only row pulled from the cloud.
        db.upsert_workspace_sync_from_server(
            "srv-other-sibling",
            "another-sibling",
            Some("other-host"),
            Some("/home/eve/projects/my-repo"),
            None,
            Some("feature/x"),
            None,
            "2026-01-01 00:00:00",
            "2026-01-01 00:00:00",
            None,
            None,
            None,
        )
        .unwrap();
        let remote = make_remote_row(
            Some("/home/deus/projects/my-repo"),
            Some("feature/x"),
            None,
        );
        // Sibling row has no local workspace_id, so liveness is moot —
        // pass it anyway to mirror the production call shape.
        assert!(detect_same_branch_project_conflict(
            &db,
            &remote,
            &live_set(&["workspace-local"]),
        )
        .is_none());
    }

    #[test]
    #[serial]
    fn conflict_detection_returns_none_when_previewed_row_lacks_branch_or_path() {
        let db = DatabaseStore::new_in_memory();
        insert_local(
            &db,
            "workspace-local",
            Some("/home/zeus/projects/my-repo"),
            Some("feature/x"),
        );
        // Missing branch.
        let no_branch =
            make_remote_row(Some("/home/deus/projects/my-repo"), None, None);
        assert!(detect_same_branch_project_conflict(
            &db,
            &no_branch,
            &live_set(&["workspace-local"]),
        )
        .is_none());
        // Missing path.
        let no_path = make_remote_row(None, Some("feature/x"), None);
        assert!(detect_same_branch_project_conflict(
            &db,
            &no_path,
            &live_set(&["workspace-local"]),
        )
        .is_none());
    }
}

/// Best-effort: extract a project name from a git remote URL when
/// the synced row doesn't carry an explicit `project_path`. Handles
/// the common cases:
///   - https://host/owner/name.git → "name"
///   - git@host:owner/name.git    → "name"
///   - git@host:owner/name        → "name"
/// Returns None if no recognisable basename is present.
fn extract_project_name_from_remote(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let after_slash_or_colon = trimmed
        .rsplit(|c: char| c == '/' || c == ':')
        .next()
        .unwrap_or(trimmed);
    let stripped = after_slash_or_colon
        .strip_suffix(".git")
        .unwrap_or(after_slash_or_colon);
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}
