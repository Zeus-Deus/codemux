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
//! configured host that has synced its identity (`server_id`):
//!
//! 1. Probe the host is reachable AND has the right `codemux-remote`
//!    binary installed (re-using `ssh::probe::probe_host`).
//! 2. SSH and run `codemux-remote workspace list` — a thin CLI we
//!    added that reads the daemon's SQLite registry and prints
//!    `{"host_id":"…","workspaces":[…]}` on stdout, plus the host facts
//!    the Devices page shows (`disk_bytes`, `remote_control_serving`).
//!    The same `~/.local/bin/codemux-remote` PATH fallback the probe
//!    uses applies here, because non-interactive SSH on Arch/Ubuntu/etc.
//!    doesn't source `~/.profile`.
//! 3. Reconcile the result into `workspaces_sync`:
//!    - Each remote workspace gets a sibling-only row keyed by
//!      `(host_server_id, origin_uid=remote_workspace.id)`.
//!    - Repeated polls UPDATE in place (the row's cloud `server_id`
//!      survives).
//!    - Disappeared rows (origin_uid no longer in the inventory) are
//!      soft-deleted so the next push DELETEs the cloud row.
//! 4. Record what the tick learned about the host itself: the live
//!    bits (reachable or not, why not, whether a Remote Control server
//!    is up) go to `hosts_status::HostStatusStore`; `last_seen_at` and
//!    the disk figure are stamped on the `hosts` row so they survive a
//!    restart. `hosts-status-changed` is emitted when any card would
//!    look different. The disk walk is the expensive part of the
//!    envelope, so it is requested per host only when the last
//!    measurement is older than `DISK_REFRESH_SECS`.
//! 5. The existing `workspaces_sync::push` tick (every 30s) uploads
//!    every dirty row, so other devices of the same account see the
//!    workspace within ~30-90 seconds of it appearing on the host.
//!
//! ## Failure model
//!
//! Best-effort. Per-call budgets cap SSH stalls. Any of:
//!
//! - host offline / SSH refused / probe timed out → recorded as
//!   unreachable with the reason, continue
//! - host reachable but binary missing, inventory failed or timed out
//!   → recorded as seen-with-error (the card stays "online"), continue
//!   (we don't try to install it; that's the user's explicit consent
//!   in Settings → Hosts)
//! - host has no `server_id` yet (host record hasn't synced to the
//!   account) → skipped entirely, not even probed: we'd have no stable
//!   identity to tag the rows with, and the host_sync loop is the one
//!   in charge of fixing that. Its card reads "not checked yet".
//! - JSON parse failure → log the host + the first 200 chars of
//!   stdout, continue
//! - daemon predates the host facts → they are simply absent from the
//!   envelope; the workspace list is still reconciled
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
//! This is a known limitation.

#![cfg(unix)]

use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::process::Command;
use tokio::time::timeout;

use crate::database::{DatabaseStore, HostRecord};
use crate::hosts_status::{HostFacts, HostStatusStore, Observation, HOSTS_STATUS_CHANGED_EVENT};
use crate::remote::host_status::{DISK_WALK_BUDGET, SKIP_DISK_ENV};
use crate::ssh::probe::{probe_host, ProbeOptions, ProbeOutcome};

/// How often the poller runs after the first 5-second warm-up.
/// 60 seconds is the floor: combined with the 30-second push loop,
/// the worst-case "agent created a workspace on a host → seen on
/// another device" latency is ~90 seconds. Bumping this lower
/// burns SSH connections (one per host per tick) without a
/// proportional UX win.
const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Ceiling on the inventory SSH call (connect + `workspace list`) on a
/// tick that skips the disk walk. Ticks that ask for a walk get
/// `DISK_WALK_BUDGET` on top, matching the host-side cap. The probe has
/// its own, shorter budget inside `probe_host`.
const INVENTORY_BUDGET: Duration = Duration::from_secs(20);

/// Re-measure a host's workspace disk footprint when the last
/// measurement is older than this. The number moves slowly, and the
/// walk is the one expensive piece of the envelope.
const DISK_REFRESH_SECS: i64 = 5 * 60;

/// Spawn the background inventory poller. Must be called once during
/// app setup. Like `hosts_upgrade`, it delays a few seconds so the
/// app's initial paint isn't competing with us for resources.
pub fn spawn<R: Runtime>(app: AppHandle<R>) {
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

/// The hosts a tick may touch: only those that have synced their
/// identity. Without a `server_id` we can't tag inventory rows with a
/// stable cross-device host identity, and any rows we created would
/// never match up against `WorkspaceSnapshot.host_id` on this device or
/// any other — so we don't open a connection to such a host at all.
pub fn hosts_to_poll(hosts: &[HostRecord]) -> Vec<&HostRecord> {
    hosts.iter().filter(|h| h.server_id.is_some()).collect()
}

/// Whether this tick should ask the host to walk its workspace
/// directories: never measured (or an unparseable stamp), or measured
/// longer ago than `DISK_REFRESH_SECS`.
pub fn disk_walk_due(disk_measured_at: Option<&str>, now: DateTime<Utc>) -> bool {
    match disk_measured_at.and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
        None => true,
        Some(measured) => {
            now.signed_duration_since(measured.with_timezone(&Utc))
                .num_seconds()
                >= DISK_REFRESH_SECS
        }
    }
}

/// One pass over every configured host. Public so tests / debug
/// surfaces can drive a single cycle without the loop.
pub async fn run_once<R: Runtime>(app: &AppHandle<R>) {
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
    let status = app.state::<HostStatusStore>();
    let mut any_changed = false;
    for host in hosts_to_poll(&hosts) {
        let server_id = host
            .server_id
            .as_deref()
            .expect("hosts_to_poll only yields synced hosts");
        let now = Utc::now();
        let walk_disk = disk_walk_due(host.disk_measured_at.as_deref(), now);
        let observation = match poll_one_host(&host.ssh_target, server_id, &db, walk_disk).await {
            Ok(poll) => {
                if poll.stats.changed() {
                    eprintln!(
                        "[hosts_inventory] {} synced ({} discovered, {} updated, {} disappeared)",
                        host.name, poll.stats.inserted, poll.stats.updated, poll.stats.soft_deleted
                    );
                }
                Observation::Reachable { facts: poll.facts }
            }
            Err(PollError::Unreachable(reason)) => {
                eprintln!("[hosts_inventory] {} unreachable: {reason}", host.name);
                Observation::Unreachable { reason }
            }
            Err(PollError::Degraded(reason)) => {
                eprintln!("[hosts_inventory] {} skipped: {reason}", host.name);
                Observation::Degraded { reason }
            }
        };

        if status.apply(host.id, &observation) {
            any_changed = true;
        }
        // The host answered: stamp last_seen_at (and the disk figure
        // when the envelope carried one) on its row. A new disk number
        // changes the card even though the live bits didn't move.
        let seen_disk = match &observation {
            Observation::Unreachable { .. } => continue,
            Observation::Degraded { .. } => None,
            Observation::Reachable { facts } => facts.disk_bytes,
        };
        if seen_disk.is_some() && seen_disk != host.disk_bytes {
            any_changed = true;
        }
        if let Err(e) = db.record_host_seen(host.id, &now.to_rfc3339(), seen_disk) {
            eprintln!("[hosts_inventory] persist status for {}: {e}", host.name);
        }
    }

    if any_changed {
        // Re-read so the payload carries the columns just stamped.
        let payload = status.views_for(&db.list_hosts());
        if let Err(e) = app.emit(HOSTS_STATUS_CHANGED_EVENT, payload) {
            eprintln!("[hosts_inventory] emit {HOSTS_STATUS_CHANGED_EVENT}: {e}");
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

/// Everything one successful tick learned about a host.
struct HostPoll {
    stats: PollStats,
    facts: HostFacts,
}

/// Why a tick fell short, split by what the Devices card should say.
enum PollError {
    /// SSH never connected.
    Unreachable(String),
    /// SSH connected but the tick could not complete (binary missing,
    /// fetch failed or timed out, output unparseable).
    Degraded(String),
}

async fn poll_one_host(
    ssh_target: &str,
    host_server_id: &str,
    db: &DatabaseStore,
    walk_disk: bool,
) -> Result<HostPoll, PollError> {
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
            return Err(PollError::Degraded(
                "codemux-remote missing on this host (use Settings → Hosts → Install)".into(),
            ));
        }
        ProbeOutcome::Unreachable { reason } => {
            return Err(PollError::Unreachable(reason));
        }
    }

    // Step 2: fetch the envelope (inventory + host facts) in one session.
    let stdout = fetch_inventory(ssh_target, walk_disk)
        .await
        .map_err(PollError::Degraded)?;
    let parsed = parse_inventory_json(&stdout)
        .map_err(|e| PollError::Degraded(format!("parse inventory: {e}")))?;

    // Step 3: reconcile.
    let stats = reconcile_host_inventory(db, host_server_id, &parsed);
    Ok(HostPoll {
        stats,
        facts: parsed.facts(),
    })
}

/// SSH into the host and capture `codemux-remote workspace list`
/// stdout. Same SSH flags as the probe (BatchMode, ConnectTimeout,
/// StrictHostKeyChecking) but a different remote command. The child is
/// killed if the budget elapses so a wedged SSH never outlives the tick.
async fn fetch_inventory(ssh_target: &str, walk_disk: bool) -> Result<String, String> {
    let budget = if walk_disk {
        INVENTORY_BUDGET + DISK_WALK_BUDGET
    } else {
        INVENTORY_BUDGET
    };
    let argv = build_inventory_argv(ssh_target, INVENTORY_BUDGET.as_secs(), walk_disk);
    let mut cmd = Command::new("ssh");
    for arg in &argv {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = match timeout(budget, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("ssh: {e}")),
        Err(_) => {
            return Err(format!(
                "inventory timed out after {}s",
                budget.as_secs()
            ))
        }
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
pub fn build_inventory_argv(ssh_target: &str, timeout_secs: u64, walk_disk: bool) -> Vec<String> {
    // An env prefix rather than a flag: a daemon that predates the
    // disk walk ignores the variable instead of rejecting the command.
    let env_prefix = if walk_disk {
        String::new()
    } else {
        format!("{SKIP_DISK_ENV}=1 ")
    };
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
        format!(
            "if command -v codemux-remote >/dev/null 2>&1 ; then \
               {env_prefix}codemux-remote workspace list ; \
             elif [ -x \"$HOME/.local/bin/codemux-remote\" ] ; then \
               {env_prefix}\"$HOME/.local/bin/codemux-remote\" workspace list ; \
             else \
               echo 'CMR_MISSING' >&2 ; exit 1 ; \
             fi"
        ),
    ]
}

/// Wire shape produced by `codemux-remote workspace list` on the
/// host. Matches `bin/codemux_remote.rs::run_workspace_list` —
/// any change there must be mirrored here.
#[derive(Debug, Clone, Deserialize)]
pub struct InventoryEnvelope {
    /// The host's `gethostname`.
    pub host_id: String,
    pub workspaces: Vec<RemoteWorkspace>,
    /// Host facts for the Devices page (`remote::host_status`). Older
    /// daemons omit both; serde defaults them to `None`, which the
    /// status store treats as "unknown".
    #[serde(default)]
    pub disk_bytes: Option<u64>,
    #[serde(default)]
    pub remote_control_serving: Option<bool>,
}

impl InventoryEnvelope {
    pub fn facts(&self) -> HostFacts {
        HostFacts {
            disk_bytes: self.disk_bytes,
            remote_control_serving: self.remote_control_serving,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteWorkspace {
    pub id: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub project_root: Option<String>,
    /// First-class project identity from the daemon registry (see
    /// `crate::project_identity`). Older daemons won't send these;
    /// serde defaults them to `None`, so the poller degrades to the
    /// path-based fallback below.
    #[serde(default)]
    pub project_uid: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub repo_remote: Option<String>,
    /// Repo default branch from the daemon (`origin/HEAD` → main/master →
    /// current). Older daemons omit it → serde defaults to `None` → the
    /// desktop falls back to resolving it locally at pull time.
    #[serde(default)]
    pub default_branch: Option<String>,
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
        // Guard against a single inventory envelope listing the same id
        // twice: `local_by_uid` is built once before the loop and never
        // updated, so without this a duplicate id would INSERT a second
        // row (it misses the live map on the second pass). `insert`
        // returns false when the id was already present this envelope.
        if !seen_uids.insert(ws.id.clone()) {
            continue;
        }

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
        // The daemon now reports a canonical git remote (Phase 1 of the
        // project-identity work); older daemons send null. This finally
        // populates `project_remote` for remote-discovered rows, which
        // used to be hardcoded null.
        let project_remote = ws.repo_remote.as_deref();
        let project_uid = ws.project_uid.as_deref();
        let workspace_kind = ws.kind.as_deref();
        let branch = ws.branch.as_deref();
        let default_branch = ws.default_branch.as_deref();
        // The workspace's REAL absolute path on the host. Unlike
        // `project_path` (which collapses to the project root for a
        // worktree), this is the authoritative rsync source for pulling
        // the workspace back. Always present — it's the daemon's `path`.
        let origin_path = Some(ws.path.as_str());

        if let Some(existing) = local_by_uid.get(&ws.id) {
            // Only push an UPDATE when something actually changed —
            // otherwise we'd flip `dirty=1` on every tick and waste
            // a cloud PATCH per workspace per minute.
            let changed = existing.title != ws.name
                || existing.project_path.as_deref() != project_path
                || existing.project_remote.as_deref() != project_remote
                || existing.git_branch.as_deref() != branch
                || existing.project_uid.as_deref() != project_uid
                || existing.workspace_kind.as_deref() != workspace_kind
                || existing.origin_path.as_deref() != origin_path
                || existing.default_branch.as_deref() != default_branch;
            if changed {
                if let Err(e) = db.update_remote_discovered_workspace_sync(
                    existing.id,
                    &ws.name,
                    project_path,
                    project_remote,
                    branch,
                    project_uid,
                    workspace_kind,
                    origin_path,
                    default_branch,
                ) {
                    eprintln!(
                        "[hosts_inventory] update failed for {host_server_id}/{}: {e}",
                        ws.id
                    );
                    continue;
                }
                stats.updated += 1;
            }
        } else if let Some(tomb) =
            db.find_remote_discovered_tombstone(host_server_id, &ws.id)
        {
            // Not in the live map, but a soft-deleted tombstone exists for
            // this (host, origin_uid) — left behind when a previously
            // adopted-then-closed workspace's row was tombstoned. Resurrect
            // it (clearing the stale workspace_id link) instead of inserting
            // a duplicate, so the cloud identity survives the close/reopen
            // and other devices don't see a vanish-then-reappear.
            if let Err(e) = db.undelete_remote_discovered_workspace_sync(
                tomb.id,
                &ws.name,
                project_path,
                project_remote,
                branch,
                project_uid,
                workspace_kind,
                origin_path,
                default_branch,
            ) {
                eprintln!(
                    "[hosts_inventory] undelete failed for {host_server_id}/{}: {e}",
                    ws.id
                );
                continue;
            }
            stats.updated += 1;
        } else if let Err(e) = db.insert_remote_discovered_workspace_sync(
            host_server_id,
            &ws.id,
            &ws.name,
            project_path,
            project_remote,
            branch,
            project_uid,
            workspace_kind,
            origin_path,
            default_branch,
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
            project_uid: Some("uid-origin".into()),
            kind: Some("worktree".into()),
            repo_remote: Some("github.com/acme/origin".into()),
            default_branch: Some("main".into()),
        }
    }

    fn make_envelope(workspaces: Vec<RemoteWorkspace>) -> InventoryEnvelope {
        InventoryEnvelope {
            host_id: "test-host".into(),
            workspaces,
            disk_bytes: None,
            remote_control_serving: None,
        }
    }

    fn host(id: i64, server_id: Option<&str>) -> HostRecord {
        HostRecord {
            id,
            server_id: server_id.map(Into::into),
            name: format!("host-{id}"),
            ssh_target: format!("user@host-{id}"),
            created_at: String::new(),
            updated_at: String::new(),
            deleted_at: None,
            dirty: false,
            last_seen_at: None,
            disk_bytes: None,
            disk_measured_at: None,
        }
    }

    // ── host selection / disk cadence ───────────────────────────

    #[test]
    fn hosts_without_server_id_are_never_polled() {
        // The no-network rule: an unsynced host gets no probe and no
        // SSH session. Its card shows "not checked yet" until the host
        // sync loop assigns a server_id.
        let hosts = vec![host(1, None), host(2, Some("srv-2")), host(3, None)];
        let polled: Vec<i64> = hosts_to_poll(&hosts).iter().map(|h| h.id).collect();
        assert_eq!(polled, vec![2]);
    }

    #[test]
    fn disk_walk_is_due_when_never_measured_or_stale() {
        let now = DateTime::parse_from_rfc3339("2026-08-27T10:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(disk_walk_due(None, now), "never measured");
        assert!(disk_walk_due(Some("not a timestamp"), now), "unparseable counts as never");
        assert!(
            !disk_walk_due(Some("2026-08-27T10:08:00Z"), now),
            "two minutes old is fresh"
        );
        assert!(
            !disk_walk_due(Some("2026-08-27T10:05:01Z"), now),
            "just under five minutes is still fresh"
        );
        assert!(disk_walk_due(Some("2026-08-27T10:05:00Z"), now), "exactly five minutes");
        assert!(disk_walk_due(Some("2026-08-27T09:00:00Z"), now), "an hour old");
    }

    // ── argv lock-in ────────────────────────────────────────────

    #[test]
    fn build_inventory_argv_has_path_fallback_and_batch_mode() {
        // The PATH + ~/.local/bin/codemux-remote fallback is the
        // entire reason this poller works on a freshly-installed
        // host — losing it would silently break the feature for
        // every user who installed via Settings → Hosts → Install.
        let argv = build_inventory_argv("user@10.0.0.7", 15, true);
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
        assert!(
            !cmd.contains(SKIP_DISK_ENV),
            "a tick that wants the disk walk must not set the skip variable"
        );
    }

    #[test]
    fn build_inventory_argv_skips_disk_walk_via_env_prefix() {
        let argv = build_inventory_argv("user@10.0.0.7", 15, false);
        let cmd = argv.last().unwrap();
        // Both branches of the PATH fallback carry the prefix, so the
        // walk is skipped regardless of where the binary was found.
        assert!(cmd.contains("CODEMUX_SKIP_DISK=1 codemux-remote workspace list"));
        assert!(cmd.contains("CODEMUX_SKIP_DISK=1 \"$HOME/.local/bin/codemux-remote\" workspace list"));
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
    fn parse_inventory_json_reads_host_facts_when_present() {
        let stdout = r#"{"host_id":"pandora","workspaces":[],"disk_bytes":42,"remote_control_serving":true}"#;
        let facts = parse_inventory_json(stdout).unwrap().facts();
        assert_eq!(facts.disk_bytes, Some(42));
        assert_eq!(facts.remote_control_serving, Some(true));

        // A skipped or over-budget walk reports an explicit null.
        let stdout = r#"{"host_id":"pandora","workspaces":[],"disk_bytes":null,"remote_control_serving":false}"#;
        let facts = parse_inventory_json(stdout).unwrap().facts();
        assert_eq!(facts.disk_bytes, None);
        assert_eq!(facts.remote_control_serving, Some(false));
    }

    #[test]
    fn parse_inventory_json_tolerates_daemons_without_host_facts() {
        let facts = parse_inventory_json(r#"{"host_id":"old","workspaces":[]}"#)
            .unwrap()
            .facts();
        assert_eq!(facts, HostFacts::default());
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
    fn parse_inventory_json_reads_project_identity_fields() {
        // The daemon's `workspace list` now emits project_uid /
        // project_name / kind / repo_remote. The poller's parser must
        // read them (and still tolerate their absence — see the
        // round-trip test above, whose payload omits them).
        let stdout = r#"{
          "host_id": "pandora",
          "workspaces": [
            {
              "id": "uid-1",
              "name": "app",
              "path": "/home/agent/projects/app",
              "branch": "main",
              "project_root": "/home/agent/projects/app",
              "project_uid": "0f9a-deterministic",
              "project_name": "app",
              "kind": "main",
              "repo_remote": "github.com/acme/app"
            }
          ]
        }"#;
        let w = &parse_inventory_json(stdout).unwrap().workspaces[0];
        assert_eq!(w.project_uid.as_deref(), Some("0f9a-deterministic"));
        assert_eq!(w.kind.as_deref(), Some("main"));
        assert_eq!(w.repo_remote.as_deref(), Some("github.com/acme/app"));
    }

    #[test]
    #[serial]
    fn e2e_daemon_to_sync_row_carries_project_identity() {
        // End-to-end across the wire: a workspace the daemon created
        // (with first-class identity) → JSON envelope → poller parse →
        // reconcile → local sync row. The desktop overview then groups
        // by project_uid and labels the kind.
        let daemon_json = r#"{
          "host_id": "pandora",
          "workspaces": [
            {
              "id": "wt-uid",
              "name": "ui-polish",
              "path": "/home/agent/.codemux/worktrees/passpage/ui-polish",
              "branch": "ui-polish-v1",
              "project_root": "/home/agent/projects/passpage",
              "project_uid": "shared-passpage-uid",
              "project_name": "passpage",
              "kind": "worktree",
              "repo_remote": "github.com/acme/passpage"
            },
            {
              "id": "main-uid",
              "name": "passpage",
              "path": "/home/agent/projects/passpage",
              "branch": "main",
              "project_root": "/home/agent/projects/passpage",
              "project_uid": "shared-passpage-uid",
              "project_name": "passpage",
              "kind": "main",
              "repo_remote": "github.com/acme/passpage"
            }
          ]
        }"#;
        let envelope = parse_inventory_json(daemon_json).unwrap();

        let db = fresh_db();
        let stats = reconcile_host_inventory(&db, "pandora", &envelope);
        assert_eq!(stats.inserted, 2);

        let rows = db.list_remote_discovered_for_host("pandora");
        assert_eq!(rows.len(), 2);

        // Both rows landed with the SAME project_uid → the overview
        // clusters them as one project, across main + worktree.
        let uids: std::collections::HashSet<_> =
            rows.iter().filter_map(|r| r.project_uid.clone()).collect();
        assert_eq!(uids.len(), 1, "main + worktree share one project_uid");
        assert!(uids.contains("shared-passpage-uid"));

        // kind is preserved per row; project_remote is now populated
        // (it used to be hardcoded null for remote-discovered rows).
        let kinds: std::collections::HashSet<_> =
            rows.iter().filter_map(|r| r.workspace_kind.clone()).collect();
        assert!(kinds.contains("main") && kinds.contains("worktree"));
        for r in &rows {
            assert_eq!(r.project_remote.as_deref(), Some("github.com/acme/passpage"));
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
    fn reconcile_records_real_origin_path_distinct_from_project_path() {
        // The crux of the empty-pull bug: a worktree's `project_path`
        // collapses to the PARENT repo, but the pull must rsync from the
        // worktree's OWN on-host path. `origin_path` carries that real
        // path verbatim so pull-back never reconstructs a wrong location.
        let db = fresh_db();
        let mut ws = make_remote("uid-wt", "passpage-ui-polish", Some("ui-polish-v1"));
        ws.project_root = Some("/home/deus/projects/passpage".into());
        ws.path = "/home/deus/projects/passpage-ui-polish".into();
        let envelope = make_envelope(vec![ws]);

        reconcile_host_inventory(&db, "host-3", &envelope);

        let rows = db.list_remote_discovered_for_host("host-3");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].origin_path.as_deref(),
            Some("/home/deus/projects/passpage-ui-polish"),
            "origin_path must be the workspace's REAL on-host path (the rsync source)"
        );
        assert_eq!(
            rows[0].project_path.as_deref(),
            Some("/home/deus/projects/passpage"),
            "project_path stays the parent repo — distinct from origin_path"
        );
    }

    #[test]
    #[serial]
    fn reconcile_updates_origin_path_when_host_path_changes() {
        // A daemon that re-homes a workspace (rare, but possible) must
        // update origin_path in place — otherwise pull-back keeps using
        // the stale source.
        let db = fresh_db();
        let mut ws = make_remote("uid-move", "proj", Some("main"));
        ws.path = "/srv/old/proj".into();
        reconcile_host_inventory(&db, "host-4", &make_envelope(vec![ws.clone()]));
        ws.path = "/srv/new/proj".into();
        let stats = reconcile_host_inventory(&db, "host-4", &make_envelope(vec![ws]));
        assert_eq!(stats.updated, 1, "changed origin_path should trigger an update");
        let rows = db.list_remote_discovered_for_host("host-4");
        assert_eq!(rows[0].origin_path.as_deref(), Some("/srv/new/proj"));
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
    fn reconcile_undeletes_tombstone_on_reappear_instead_of_duplicating() {
        // Models the adopt → close → re-poll race that used to leak a
        // duplicate sibling row (and churn the cloud row). A
        // remote-discovered workspace is adopted (workspace_id linked),
        // then the local workspace is CLOSED — the close-path reconcile
        // soft-deletes the row BY workspace_id, leaving a tombstone that
        // still carries the host's origin_uid. The host still has the
        // workspace, so the next poll must RESURRECT the same row (cloud
        // server_id preserved, stale link cleared), not insert a second.
        let db = fresh_db();
        let env = make_envelope(vec![make_remote("uid-1", "proj", Some("main"))]);

        reconcile_host_inventory(&db, "host-99", &env);
        let row = db
            .find_workspace_sync_by_host_and_origin_uid("host-99", "uid-1")
            .expect("first poll inserts the row");
        // Simulate the first cloud push assigning a server_id.
        db.mark_workspace_sync_synced(row.id, Some("srv-1")).unwrap();
        // Simulate ADOPTION (link) then CLOSE (soft-delete by workspace_id).
        db.link_workspace_sync_to_local("srv-1", "workspace-7").unwrap();
        db.soft_delete_workspace_sync_by_workspace_id("workspace-7")
            .unwrap();

        // Now: zero live remote rows, one tombstone still keyed by uid-1.
        assert!(db.list_remote_discovered_for_host("host-99").is_empty());
        assert!(db
            .find_remote_discovered_tombstone("host-99", "uid-1")
            .is_some());

        // Re-poll: the host still reports uid-1.
        let stats = reconcile_host_inventory(&db, "host-99", &env);
        assert_eq!(stats.inserted, 0, "must NOT insert a duplicate row");
        assert_eq!(stats.updated, 1, "must resurrect the tombstone in place");

        let live = db.list_remote_discovered_for_host("host-99");
        assert_eq!(live.len(), 1, "exactly one live row — no duplicate");
        assert_eq!(
            live[0].id, row.id,
            "same row resurrected, so the cloud server_id survives"
        );
        assert_eq!(live[0].server_id.as_deref(), Some("srv-1"));
        assert!(
            live[0].workspace_id.is_none(),
            "stale adoption link must be cleared so it's a clean re-pullable sibling"
        );
        assert!(live[0].deleted_at.is_none());
        assert!(
            live[0].dirty,
            "resurrected row must be dirty so push re-asserts it as a PATCH"
        );
    }

    #[test]
    fn reconcile_dedupes_duplicate_ids_within_one_envelope() {
        // Defensive guard: `local_by_uid` is built once before the loop,
        // so if a daemon ever reported the same id twice in one envelope
        // the second pass would miss the live map and INSERT again.
        let db = fresh_db();
        let env = make_envelope(vec![
            make_remote("dup", "proj", Some("main")),
            make_remote("dup", "proj", Some("main")),
        ]);
        let stats = reconcile_host_inventory(&db, "host-99", &env);
        assert_eq!(stats.inserted, 1, "duplicate id inserts exactly once");
        assert_eq!(db.list_remote_discovered_for_host("host-99").len(), 1);
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
