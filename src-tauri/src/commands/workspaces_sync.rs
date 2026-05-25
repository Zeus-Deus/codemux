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
    let project_name = row
        .project_path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let branch = row.git_branch.as_deref().unwrap_or("main");
    let conv = crate::ssh::conventional_remote_path(project_name, branch);
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

    let already_adopted_workspace_id = row.workspace_id.clone();

    Ok(AdoptionPreview {
        can_host_adopt: host_configured && local_host_id.is_some(),
        can_clone_adopt,
        host_configured,
        host_label,
        project_already_cloned_at: None,
        suggested_path,
        is_path_in_use,
        already_adopted_workspace_id,
    })
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

        // Compute canonical local path.
        let project_name = row
            .project_path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");
        let branch = row.git_branch.as_deref().unwrap_or("main");
        let conv = crate::ssh::conventional_remote_path(project_name, branch);
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
        let conv = crate::ssh::conventional_remote_path(&project_name, &branch);
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
