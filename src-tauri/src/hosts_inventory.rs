//! Background host-inventory poller.
//!
//! Sister task to `hosts_upgrade.rs`. Where the upgrade poller keeps
//! every host's `codemux-remote` binary at the same version as the
//! desktop app, this poller keeps every host's *workspace inventory*
//! visible to the user's account.
//!
//! ## Why this exists
//!
//! Before this poller, a workspace only showed up in the cross-device
//! overview if some device explicitly pushed it to the host. That works
//! for the "I'm closing my laptop, continue on my server" flow, but it
//! misses the "I asked my agent on pandora to start a project last
//! night" flow: the agent created the workspace on the host via the
//! `workspace_create` MCP tool, the host's daemon recorded it, but no
//! desktop ever knew about it — so no desktop ever published it to the
//! cloud, so no other device ever saw it.
//!
//! This poller closes that gap. On a 60-second cadence, for every
//! configured host:
//!
//! 1. Probe the host is reachable AND has the right `codemux-remote`
//!    binary installed (re-using `ssh::probe::probe_host`).
//! 2. SSH and run `codemux-remote workspace list` — a thin CLI we
//!    added that reads the daemon's SQLite registry and prints
//!    `{"host_id":"…","workspaces":[…]}` on stdout. The same
//!    `~/.local/bin/codemux-remote` PATH fallback the probe uses
//!    applies here, because non-interactive SSH on Arch/Ubuntu/etc.
//!    doesn't source `~/.profile`.
//! 3. Reconcile the result into `workspaces_sync`:
//!    - Each remote workspace gets a sibling-only row keyed by
//!      `(host_server_id, origin_uid=remote_workspace.id)`.
//!    - Repeated polls UPDATE in place (the row's cloud `server_id`
//!      survives).
//!    - Disappeared rows (origin_uid no longer in the inventory) are
//!      soft-deleted so the next push DELETEs the cloud row.
//! 4. The existing `workspaces_sync::push` tick (every 30s) uploads
//!    every dirty row, so other devices of the same account see the
//!    workspace within ~30-90 seconds of it appearing on the host.
//!
//! ## Failure model
//!
//! Best-effort. Per-host budget caps SSH stalls. Any of:
//!
//! - host offline / SSH refused → log, continue
//! - host reachable but binary missing → log, continue (we don't try
//!   to install it; that's the user's explicit consent in Settings
//!   → Hosts)
//! - host has no `server_id` yet (host record hasn't synced to the
//!   account) → skip silently — we'd have no stable identity to tag
//!   the rows with, and the host_sync loop is the one in charge of
//!   fixing that
//! - JSON parse failure → log the host + the first 200 chars of
//!   stdout, continue
//!
//! The task never fails the app.
//!
//! ## Cross-device dedupe caveat (v1)
//!
//! `origin_uid` is local-only. The cloud schema doesn't carry it. So
//! if two of the user's devices both poll the same host before either
//! has pulled the other's cloud row, both will publish a sync row for
//! the same physical workspace and the user will briefly see two
//! entries in the overview. They converge on the next pull tick
//! because the cloud row Device A published shows up on Device B with
//! the same `host_server_id`, `project_remote`, and `git_branch`.
//! Single-device users (the common case) never hit this.
//!
//! Tracked as a known limitation in `docs/features/workspaces-sync.md`.

#![cfg(unix)]

use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tokio::process::Command;
use tokio::time::timeout;

use crate::database::DatabaseStore;
use crate::ssh::probe::{probe_host, ProbeOptions, ProbeOutcome};

/// How often the poller runs after the first 5-second warm-up.
/// 60 seconds is the floor: combined with the 30-second push loop,
/// the worst-case "agent created a workspace on a host → seen on
/// another device" latency is ~90 seconds. Bumping this lower
/// burns SSH connections (one per host per tick) without a
/// proportional UX win.
const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Hard ceiling on how long any single host's poll can take —
/// probe + workspace-list combined. Hosts that exceed this are
/// logged and skipped for the cycle.
const PER_HOST_BUDGET: Duration = Duration::from_secs(20);

/// Spawn the background inventory poller. Must be called once during
/// app setup. Like `hosts_upgrade`, it delays a few seconds so the
/// app's initial paint isn't competing with us for resources.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Warm-up: let the UI settle, and give `hosts_upgrade::spawn`
        // (which fires at +5s) a head start so any host whose binary
        // is mid-upgrade has already become consistent before we
        // start polling its registry. Otherwise we'd race against
        // the upgrade and might briefly see an empty/half-migrated
        // inventory.
        tokio::time::sleep(Duration::from_secs(12)).await;
        loop {
            run_once(&app).await;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

/// One pass over every configured host. Public so tests / debug
/// surfaces can drive a single cycle without the loop.
pub async fn run_once(app: &AppHandle) {
    let hosts = match app.try_state::<DatabaseStore>() {
        Some(state) => state.list_hosts(),
        None => {
            eprintln!(
                "[hosts_inventory] database state unavailable; skipping inventory poll"
            );
            return;
        }
    };
    if hosts.is_empty() {
        return;
    }

    let db = app.state::<DatabaseStore>();
    for host in hosts {
        // Skip hosts that haven't synced their identity yet. Without a
        // `server_id` we can't tag the inventory rows with a stable
        // cross-device host identity, and any rows we created would
        // never match up against `WorkspaceSnapshot.host_id` on this
        // device or any other.
        let host_server_id = match &host.server_id {
            Some(sid) => sid.clone(),
            None => continue,
        };

        let outcome = timeout(
            PER_HOST_BUDGET,
            poll_one_host(&host.ssh_target, &host_server_id, &db),
        )
        .await;
        match outcome {
            Ok(Ok(stats)) => {
                if stats.changed() {
                    eprintln!(
                        "[hosts_inventory] {} synced ({} discovered, {} updated, {} disappeared)",
                        host.name, stats.inserted, stats.updated, stats.soft_deleted
                    );
                }
            }
            Ok(Err(error)) => {
                eprintln!("[hosts_inventory] {} skipped: {error}", host.name);
            }
            Err(_) => {
                eprintln!(
                    "[hosts_inventory] {} timed out (>{}s) — host slow or offline",
                    host.name,
                    PER_HOST_BUDGET.as_secs()
                );
            }
        }
    }
}

/// Per-host counters, returned by `poll_one_host` so the run-loop
/// can decide whether the host did any meaningful work this tick
/// (we only log when something changed, to keep steady-state logs
/// quiet).
#[derive(Default, Debug, Clone, Copy)]
pub struct PollStats {
    pub inserted: usize,
    pub updated: usize,
    pub soft_deleted: usize,
}

impl PollStats {
    fn changed(&self) -> bool {
        self.inserted > 0 || self.updated > 0 || self.soft_deleted > 0
    }
}

async fn poll_one_host(
    ssh_target: &str,
    host_server_id: &str,
    db: &DatabaseStore,
) -> Result<PollStats, String> {
    // Step 1: probe so we don't spend the inventory budget on a
    // host that's offline or doesn't have the binary. Re-uses the
    // same fallback-aware command the test-connection flow uses.
    match probe_host(ProbeOptions::new(ssh_target)).await {
        ProbeOutcome::Reachable {
            codemux_remote_version: Some(_),
            ..
        } => {}
        ProbeOutcome::Reachable {
            codemux_remote_version: None,
            ..
        } => {
            return Err(
                "codemux-remote not installed (use Settings → Hosts → Install)".into()
            );
        }
        ProbeOutcome::Unreachable { reason } => {
            return Err(format!("unreachable: {reason}"));
        }
    }

    // Step 2: fetch the inventory.
    let inventory = fetch_inventory(ssh_target).await?;
    let parsed = parse_inventory_json(&inventory)
        .map_err(|e| format!("parse inventory: {e}"))?;

    // Step 3: reconcile.
    Ok(reconcile_host_inventory(db, host_server_id, &parsed))
}

/// SSH into the host and capture `codemux-remote workspace list`
/// stdout. Same SSH flags as the probe (BatchMode, ConnectTimeout,
/// StrictHostKeyChecking) but a different remote command.
async fn fetch_inventory(ssh_target: &str) -> Result<String, String> {
    let argv = build_inventory_argv(ssh_target, PER_HOST_BUDGET.as_secs());
    let mut cmd = Command::new("ssh");
    for arg in &argv {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let result = timeout(PER_HOST_BUDGET, async { cmd.output().await }).await;
    let output = match result {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("ssh: {e}")),
        Err(_) => return Err("ssh inventory fetch timed out".into()),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("ssh exited with status {}", output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Build the argv for the inventory SSH call. Extracted so unit
/// tests can lock in the exact flags + remote command (especially
/// the `$HOME/.local/bin/codemux-remote` fallback — losing that on
/// Arch/Ubuntu/etc. would silently break the poller for every user
/// who installed via Settings → Hosts → Install, because
/// non-interactive SSH shells don't put `~/.local/bin` on PATH).
pub fn build_inventory_argv(ssh_target: &str, timeout_secs: u64) -> Vec<String> {
    vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={timeout_secs}"),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        ssh_target.into(),
        // Same PATH-fallback story as the probe (see ssh/probe.rs):
        // bootstrap installs to ~/.local/bin, but non-interactive
        // SSH typically doesn't have that dir on PATH. Without the
        // absolute-path fallback the poller silently degrades to
        // "no inventory" the moment a user installs via the desktop's
        // Install button.
        "if command -v codemux-remote >/dev/null 2>&1 ; then \
           codemux-remote workspace list ; \
         elif [ -x \"$HOME/.local/bin/codemux-remote\" ] ; then \
           \"$HOME/.local/bin/codemux-remote\" workspace list ; \
         else \
           echo 'CMR_MISSING' >&2 ; exit 1 ; \
         fi"
        .into(),
    ]
}

/// Wire shape produced by `codemux-remote workspace list` on the
/// host. Matches `bin/codemux_remote.rs::run_workspace_list` —
/// any change there must be mirrored here.
#[derive(Debug, Clone, Deserialize)]
pub struct InventoryEnvelope {
    #[allow(dead_code)] // recorded for debug logs only
    pub host_id: String,
    pub workspaces: Vec<RemoteWorkspace>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteWorkspace {
    pub id: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub project_root: Option<String>,
    // `owner_id`, `origin_host_id`, `notes`, `created_at`,
    // `updated_at` are accepted by serde but not used by the
    // reconcile — they live on the remote daemon and have no
    // mapping on this side today. Leaving them off the struct
    // (with serde's default of ignoring unknown fields) keeps
    // the parser robust to future remote-side schema additions.
}

/// Parse the daemon's `workspace list` stdout. Public for tests.
pub fn parse_inventory_json(stdout: &str) -> Result<InventoryEnvelope, String> {
    serde_json::from_str(stdout.trim()).map_err(|e| {
        let preview: String = stdout.chars().take(200).collect();
        format!("not valid inventory JSON: {e}; first 200 chars: {preview:?}")
    })
}

/// Apply a freshly-fetched inventory snapshot to the local
/// `workspaces_sync` table for one host. Returns counts the run
/// loop uses to decide whether to log this tick.
///
/// Algorithm:
///
/// 1. Index local "remote-discovered for this host" rows by
///    `origin_uid` so we can tell INSERT from UPDATE in one pass.
/// 2. For each inventory row, INSERT (if no match) or UPDATE in
///    place (if matched — `server_id` survives, `dirty` is set).
/// 3. Any local rows whose `origin_uid` is no longer in the
///    inventory get soft-deleted so the next push DELETEs the
///    cloud row.
///
/// Public so tests can drive it directly without an SSH stub.
pub fn reconcile_host_inventory(
    db: &DatabaseStore,
    host_server_id: &str,
    inventory: &InventoryEnvelope,
) -> PollStats {
    let mut stats = PollStats::default();

    // Index local rows by origin_uid for O(1) lookup during the walk.
    let local_rows = db.list_remote_discovered_for_host(host_server_id);
    let local_by_uid: std::collections::HashMap<String, &crate::database::WorkspaceSyncRecord> =
        local_rows
            .iter()
            .filter_map(|r| r.origin_uid.clone().map(|uid| (uid, r)))
            .collect();

    let mut seen_uids: HashSet<String> = HashSet::new();
    for ws in &inventory.workspaces {
        seen_uids.insert(ws.id.clone());

        // We use `project_root` (the originating repo root on the
        // host) as a `project_path` analog — it's the closest the
        // remote daemon's schema has to "where this workspace
        // came from." Better than nothing for the UI's "open in
        // file manager" affordance.
        //
        // Fallback: a host workspace created via the `workspace_create`
        // MCP tool only records `project_root` when it's a worktree of
        // an origin repo. A plain root/main checkout leaves it null,
        // which used to leave the overview row with an empty project
        // name and the pull dialog with a generic
        // `~/.codemux/worktrees/workspace/main` landing path. For a
        // root checkout the workspace's own working dir (`path`) IS the
        // project root, so its basename is the project name — fall back
        // to it so every remote-discovered workspace surfaces a
        // meaningful project.
        let project_path = ws
            .project_root
            .as_deref()
            .or(Some(ws.path.as_str()));
        let project_remote: Option<&str> = None; // not in the remote schema today
        let branch = ws.branch.as_deref();

        if let Some(existing) = local_by_uid.get(&ws.id) {
            // Only push an UPDATE when something actually changed —
            // otherwise we'd flip `dirty=1` on every tick and waste
            // a cloud PATCH per workspace per minute.
            let changed = existing.title != ws.name
                || existing.project_path.as_deref() != project_path
                || existing.project_remote.as_deref() != project_remote
                || existing.git_branch.as_deref() != branch;
            if changed {
                if let Err(e) = db.update_remote_discovered_workspace_sync(
                    existing.id,
                    &ws.name,
                    project_path,
                    project_remote,
                    branch,
                ) {
                    eprintln!(
                        "[hosts_inventory] update failed for {host_server_id}/{}: {e}",
                        ws.id
                    );
                    continue;
                }
                stats.updated += 1;
            }
        } else if let Err(e) = db.insert_remote_discovered_workspace_sync(
            host_server_id,
            &ws.id,
            &ws.name,
            project_path,
            project_remote,
            branch,
        ) {
            eprintln!(
                "[hosts_inventory] insert failed for {host_server_id}/{}: {e}",
                ws.id
            );
            continue;
        } else {
            stats.inserted += 1;
        }
    }

    // Soft-delete any local row whose origin_uid is no longer in
    // the inventory. The next push DELETEs the cloud row so other
    // devices learn of the disappearance.
    for local in &local_rows {
        if let Some(uid) = &local.origin_uid {
            if !seen_uids.contains(uid) {
                if let Err(e) =
                    db.soft_delete_remote_discovered_workspace_sync_by_id(local.id)
                {
                    eprintln!(
                        "[hosts_inventory] soft-delete failed for id={}: {e}",
                        local.id
                    );
                    continue;
                }
                stats.soft_deleted += 1;
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DatabaseStore;
    use serial_test::serial;

    fn fresh_db() -> DatabaseStore {
        DatabaseStore::new_in_memory()
    }

    fn make_remote(id: &str, name: &str, branch: Option<&str>) -> RemoteWorkspace {
        RemoteWorkspace {
            id: id.into(),
            name: name.into(),
            path: format!("/srv/{name}"),
            branch: branch.map(|s| s.into()),
            project_root: Some("/srv/origin".into()),
        }
    }

    fn make_envelope(workspaces: Vec<RemoteWorkspace>) -> InventoryEnvelope {
        InventoryEnvelope {
            host_id: "test-host".into(),
            workspaces,
        }
    }

    // ── argv lock-in ────────────────────────────────────────────

    #[test]
    fn build_inventory_argv_has_path_fallback_and_batch_mode() {
        // The PATH + ~/.local/bin/codemux-remote fallback is the
        // entire reason this poller works on a freshly-installed
        // host — losing it would silently break the feature for
        // every user who installed via Settings → Hosts → Install.
        let argv = build_inventory_argv("user@10.0.0.7", 15);
        assert!(argv.iter().any(|a| a == "BatchMode=yes"));
        assert!(argv.iter().any(|a| a == "ConnectTimeout=15"));
        assert!(argv.iter().any(|a| a == "StrictHostKeyChecking=accept-new"));
        assert!(argv.iter().any(|a| a == "user@10.0.0.7"));
        let cmd = argv.last().unwrap();
        assert!(
            cmd.contains("command -v codemux-remote"),
            "fast-path PATH lookup must remain"
        );
        assert!(
            cmd.contains("$HOME/.local/bin/codemux-remote"),
            "absolute-path fallback must remain (Arch/Ubuntu non-interactive SSH \
             doesn't have ~/.local/bin on PATH)"
        );
        assert!(
            cmd.contains("workspace list"),
            "remote command must be the workspace list subcommand"
        );
    }

    // ── parse ──────────────────────────────────────────────────

    #[test]
    fn parse_inventory_json_round_trip() {
        let stdout = r#"{
          "host_id": "pandora",
          "workspaces": [
            {
              "id": "11111111-2222-3333-4444-555555555555",
              "name": "alpha",
              "path": "/srv/alpha",
              "branch": "main",
              "project_root": "/srv/alpha-origin",
              "created_at": "2026-05-01T00:00:00Z",
              "updated_at": "2026-05-01T00:00:00Z",
              "owner_id": null,
              "origin_host_id": "pandora",
              "notes": null
            }
          ]
        }"#;
        let parsed = parse_inventory_json(stdout).unwrap();
        assert_eq!(parsed.host_id, "pandora");
        assert_eq!(parsed.workspaces.len(), 1);
        let w = &parsed.workspaces[0];
        assert_eq!(w.id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(w.name, "alpha");
        assert_eq!(w.branch.as_deref(), Some("main"));
        assert_eq!(w.project_root.as_deref(), Some("/srv/alpha-origin"));
    }

    #[test]
    fn parse_inventory_json_rejects_garbage_with_useful_message() {
        let result = parse_inventory_json("not even close to JSON");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("first 200 chars"),
            "error must include a stdout preview so we can debug what the host \
             actually returned: got {err}"
        );
    }

    // ── reconcile ───────────────────────────────────────────────

    #[test]
    #[serial]
    fn reconcile_inserts_new_remote_workspaces() {
        let db = fresh_db();
        let envelope = make_envelope(vec![
            make_remote("uid-1", "alpha", Some("main")),
            make_remote("uid-2", "beta", Some("dev")),
        ]);
        let stats = reconcile_host_inventory(&db, "host-99", &envelope);
        assert_eq!(stats.inserted, 2);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.soft_deleted, 0);

        let rows = db.list_remote_discovered_for_host("host-99");
        assert_eq!(rows.len(), 2);
        for r in &rows {
            assert!(r.workspace_id.is_none(), "sibling-only rows");
            assert!(r.server_id.is_none(), "no cloud id until first push");
            assert!(r.dirty);
            assert_eq!(r.host_server_id.as_deref(), Some("host-99"));
        }
    }

    #[test]
    #[serial]
    fn reconcile_falls_back_to_path_when_project_root_is_missing() {
        // A root/main checkout created via the `workspace_create` MCP
        // tool has no `project_root` (that field is only stamped for
        // worktrees). Before the fallback, its sync row got a null
        // project_path, which surfaced as an empty project name and a
        // generic `~/.codemux/worktrees/workspace/<branch>` landing path
        // in the overview + pull dialog. The reconcile now uses the
        // workspace's own `path` as the project_path so a meaningful
        // project name survives.
        let db = fresh_db();
        let mut ws = make_remote("uid-root", "passpage", Some("main"));
        ws.project_root = None;
        ws.path = "/home/agent/projects/passpage".into();
        let envelope = make_envelope(vec![ws]);

        reconcile_host_inventory(&db, "host-1", &envelope);

        let rows = db.list_remote_discovered_for_host("host-1");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].project_path.as_deref(),
            Some("/home/agent/projects/passpage"),
            "missing project_root must fall back to the workspace path so the \
             overview can derive the 'passpage' project name"
        );
    }

    #[test]
    #[serial]
    fn reconcile_prefers_project_root_over_path_for_worktrees() {
        // When the host DID record a project_root (the worktree case)
        // it stays authoritative — a worktree's own path basename is
        // the BRANCH, not the project, so falling back to it would
        // mislabel the project. Locks in the precedence.
        let db = fresh_db();
        let mut ws =
            make_remote("uid-wt", "passpage-ui-polish", Some("ui-polish-v1"));
        ws.project_root = Some("/home/agent/projects/passpage".into());
        ws.path = "/home/agent/.codemux/worktrees/passpage/ui-polish-v1".into();
        let envelope = make_envelope(vec![ws]);

        reconcile_host_inventory(&db, "host-2", &envelope);

        let rows = db.list_remote_discovered_for_host("host-2");
        assert_eq!(
            rows[0].project_path.as_deref(),
            Some("/home/agent/projects/passpage"),
            "project_root must win over the worktree path basename"
        );
    }

    #[test]
    #[serial]
    fn reconcile_is_idempotent_when_nothing_changed() {
        // Two identical polls back-to-back must NOT mark every row
        // dirty again — otherwise steady-state syncing would burn
        // one PATCH per workspace per poll for no reason.
        let db = fresh_db();
        let envelope = make_envelope(vec![make_remote("uid-1", "alpha", Some("main"))]);
        let first = reconcile_host_inventory(&db, "host-99", &envelope);
        assert_eq!(first.inserted, 1);
        // Simulate the first push clearing dirty.
        let row = db
            .find_workspace_sync_by_host_and_origin_uid("host-99", "uid-1")
            .unwrap();
        db.mark_workspace_sync_synced(row.id, Some("cloud-1")).unwrap();
        assert!(!db
            .find_workspace_sync_by_host_and_origin_uid("host-99", "uid-1")
            .unwrap()
            .dirty);

        // Re-poll with identical inventory.
        let second = reconcile_host_inventory(&db, "host-99", &envelope);
        assert_eq!(second.inserted, 0, "no inserts on identical re-poll");
        assert_eq!(second.updated, 0, "no updates on identical re-poll");
        assert!(
            !db.find_workspace_sync_by_host_and_origin_uid("host-99", "uid-1")
                .unwrap()
                .dirty,
            "identical re-poll must not mark the row dirty again"
        );
    }

    #[test]
    #[serial]
    fn reconcile_updates_in_place_when_remote_renames() {
        let db = fresh_db();
        let initial = make_envelope(vec![make_remote("uid-1", "original-name", Some("main"))]);
        reconcile_host_inventory(&db, "host-99", &initial);
        // Pretend the first push assigned a server_id.
        let row = db
            .find_workspace_sync_by_host_and_origin_uid("host-99", "uid-1")
            .unwrap();
        db.mark_workspace_sync_synced(row.id, Some("cloud-7")).unwrap();

        // The host renamed the workspace (e.g. via the `workspace_update`
        // MCP tool). Same UUID, new title.
        let renamed =
            make_envelope(vec![make_remote("uid-1", "renamed-on-host", Some("main"))]);
        let stats = reconcile_host_inventory(&db, "host-99", &renamed);
        assert_eq!(stats.inserted, 0);
        assert_eq!(stats.updated, 1);
        assert_eq!(stats.soft_deleted, 0);

        let after = db
            .find_workspace_sync_by_host_and_origin_uid("host-99", "uid-1")
            .unwrap();
        assert_eq!(after.title, "renamed-on-host");
        assert_eq!(
            after.server_id.as_deref(),
            Some("cloud-7"),
            "in-place update must preserve the cloud server_id"
        );
        assert!(after.dirty, "title change must mark the row dirty so push propagates");
    }

    #[test]
    #[serial]
    fn reconcile_soft_deletes_when_remote_workspace_disappears() {
        let db = fresh_db();
        let initial = make_envelope(vec![
            make_remote("uid-1", "kept", Some("main")),
            make_remote("uid-2", "doomed", Some("dev")),
        ]);
        reconcile_host_inventory(&db, "host-99", &initial);
        assert_eq!(db.list_remote_discovered_for_host("host-99").len(), 2);

        // Re-poll with uid-2 missing — e.g. the host's daemon
        // received `workspace_close uid-2`.
        let after = make_envelope(vec![make_remote("uid-1", "kept", Some("main"))]);
        let stats = reconcile_host_inventory(&db, "host-99", &after);
        assert_eq!(stats.inserted, 0);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.soft_deleted, 1);

        // The remaining live row is uid-1.
        let live = db.list_remote_discovered_for_host("host-99");
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].origin_uid.as_deref(), Some("uid-1"));

        // The doomed row is a tombstone visible to the sync-loop list.
        let tombstone = db
            .list_workspaces_sync_for_sync()
            .into_iter()
            .find(|r| r.origin_uid.as_deref() == Some("uid-2"))
            .expect("tombstone must remain in *_for_sync until push acknowledges");
        assert!(tombstone.deleted_at.is_some());
        assert!(
            tombstone.dirty,
            "tombstone must be dirty so push DELETEs the cloud row"
        );
    }

    #[test]
    #[serial]
    fn reconcile_scope_is_per_host() {
        // Two hosts can legitimately have the same UUID in their
        // registries (UUIDs are unique within a registry, never
        // assumed unique across hosts). The reconcile must never
        // confuse Host A's workspace with Host B's because they
        // happen to share an `id`.
        let db = fresh_db();
        let env_a = make_envelope(vec![make_remote("uid-shared", "from-A", Some("main"))]);
        let env_b = make_envelope(vec![make_remote("uid-shared", "from-B", Some("main"))]);

        reconcile_host_inventory(&db, "host-A", &env_a);
        reconcile_host_inventory(&db, "host-B", &env_b);

        let row_a = db
            .find_workspace_sync_by_host_and_origin_uid("host-A", "uid-shared")
            .expect("host-A row");
        let row_b = db
            .find_workspace_sync_by_host_and_origin_uid("host-B", "uid-shared")
            .expect("host-B row");
        assert_ne!(row_a.id, row_b.id, "rows must be distinct per host");
        assert_eq!(row_a.title, "from-A");
        assert_eq!(row_b.title, "from-B");

        // And a poll for host-A with the workspace gone must NOT
        // soft-delete host-B's row.
        let empty = make_envelope(Vec::new());
        let stats = reconcile_host_inventory(&db, "host-A", &empty);
        assert_eq!(stats.soft_deleted, 1);
        assert_eq!(db.list_remote_discovered_for_host("host-A").len(), 0);
        assert_eq!(
            db.list_remote_discovered_for_host("host-B").len(),
            1,
            "polling host-A must not touch host-B's rows"
        );
    }
}
