//! End-to-end integration test for the host-inventory auto-publish
//! flow.
//!
//! Exercises the real CLI → real parser → real reconciler pipeline,
//! without needing SSH: the desktop's poller, at runtime, runs
//! `codemux-remote workspace list` over SSH and feeds the stdout into
//! `parse_inventory_json` + `reconcile_host_inventory`. We invoke the
//! same binary locally and drive the same parser + reconciler, so a
//! regression in any of those three layers fails this test.
//!
//! What this covers that the per-layer unit tests don't:
//!
//! - The exact JSON shape `codemux-remote workspace list` writes is
//!   what `parse_inventory_json` accepts. (Both are stable contracts
//!   but they're owned by different files; this test guards the
//!   handoff.)
//! - The reconcile pass, fed the real CLI output, produces exactly
//!   the sync rows we expect — `dirty=1`, `workspace_id=NULL`,
//!   `host_server_id` + `origin_uid` set, no extras.
//! - Re-running the CLI + reconcile after a no-op change is
//!   idempotent: no spurious dirty flips, no duplicate rows.
//!
//! Unix-only — `codemux-remote` itself is Unix-only.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

use codemux_lib::database::DatabaseStore;
use codemux_lib::hosts_inventory::{parse_inventory_json, reconcile_host_inventory};
use codemux_lib::remote::{config, workspace::WorkspaceStore};
use tempfile::TempDir;

fn binary_path() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codemux-remote") {
        return PathBuf::from(path);
    }
    PathBuf::from("target/debug/codemux-remote")
}

/// Helper: invoke `codemux-remote workspace list --state-dir <dir>`
/// and return its stdout, asserting the exit was clean.
fn run_workspace_list(state_dir: &std::path::Path) -> String {
    let bin = binary_path();
    let output = Command::new(&bin)
        .arg("workspace")
        .arg("list")
        .arg("--state-dir")
        .arg(state_dir)
        .output()
        .expect("spawn codemux-remote workspace list");
    assert!(
        output.status.success(),
        "workspace list exited {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 stdout")
}

#[test]
fn cli_to_reconcile_round_trip_publishes_remote_workspaces() {
    let bin = binary_path();
    if !bin.exists() {
        eprintln!(
            "[test] codemux-remote binary at {:?} not built — \
             run `cargo build --bin codemux-remote` first",
            bin
        );
        return;
    }

    // Seed the host-side registry with two workspaces, matching the
    // shape the `workspace_create` MCP tool would produce on a real
    // host.
    let state_tmp = TempDir::new().unwrap();
    let state_dir = state_tmp.path();
    let (alpha_uid, beta_uid) = {
        let store = WorkspaceStore::open(
            &config::database_path(state_dir),
            "test-pandora".into(),
            config::workspaces_root(state_dir),
        )
        .expect("open WorkspaceStore");
        let alpha = store
            .create(
                Some("alpha".into()),
                "/srv/alpha".into(),
                Some("main".into()),
                Some("/srv/alpha-origin".into()),
            )
            .expect("create alpha");
        let beta = store
            .create(
                Some("beta".into()),
                "/srv/beta".into(),
                Some("dev".into()),
                Some("/srv/beta-origin".into()),
            )
            .expect("create beta");
        (alpha.id, beta.id)
    };

    // Drive the real CLI against that state dir.
    let stdout = run_workspace_list(state_dir);
    let parsed = parse_inventory_json(&stdout)
        .expect("workspace list stdout must parse via parse_inventory_json — \
                 a mismatch between the CLI's printed shape and the parser \
                 means every poll tick on every host will silently fail");
    assert_eq!(parsed.workspaces.len(), 2);
    let parsed_uids: Vec<&str> =
        parsed.workspaces.iter().map(|w| w.id.as_str()).collect();
    assert!(parsed_uids.contains(&alpha_uid.as_str()));
    assert!(parsed_uids.contains(&beta_uid.as_str()));

    // Reconcile into a fresh desktop DB. Mimics what the
    // hosts_inventory poller does after fetching the host's
    // inventory over SSH.
    let desktop_db = DatabaseStore::new_in_memory();
    let stats = reconcile_host_inventory(&desktop_db, "host-pandora", &parsed);
    assert_eq!(stats.inserted, 2, "every remote workspace must insert once");
    assert_eq!(stats.updated, 0);
    assert_eq!(stats.soft_deleted, 0);

    // Inspect the rows the reconcile created — they must look
    // exactly like sibling-only sync rows: no local workspace_id,
    // no cloud server_id yet, dirty=1 so the next push uploads.
    let rows = desktop_db.list_remote_discovered_for_host("host-pandora");
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert!(
            r.workspace_id.is_none(),
            "sibling-only rows must not stamp a local workspace_id"
        );
        assert!(
            r.server_id.is_none(),
            "cloud server_id is assigned by push, not by reconcile"
        );
        assert!(r.dirty, "fresh row must be dirty so push uploads it");
        assert_eq!(r.host_server_id.as_deref(), Some("host-pandora"));
        assert!(
            r.origin_uid.is_some(),
            "remote-discovered rows must carry an origin_uid for dedupe"
        );
    }

    // Same titles + branches surfaced through the entire pipeline.
    let alpha_row = rows
        .iter()
        .find(|r| r.origin_uid.as_deref() == Some(&alpha_uid))
        .expect("alpha must be present in the desktop DB");
    assert_eq!(alpha_row.title, "alpha");
    assert_eq!(alpha_row.git_branch.as_deref(), Some("main"));
    let beta_row = rows
        .iter()
        .find(|r| r.origin_uid.as_deref() == Some(&beta_uid))
        .expect("beta must be present in the desktop DB");
    assert_eq!(beta_row.title, "beta");
    assert_eq!(beta_row.git_branch.as_deref(), Some("dev"));
}

#[test]
fn cli_to_reconcile_propagates_project_identity() {
    // The full project-identity chain through the REAL binary: a repo
    // with a git origin → daemon stamps deterministic project_uid +
    // canonical repo_remote + kind at create → `workspace list` emits
    // them → parser reads them → reconcile lands them on the sync row.
    // This is the desktop overview's source of "group by project +
    // label main/worktree".
    let bin = binary_path();
    if !bin.exists() {
        return;
    }

    // Real git repo with an origin remote so the daemon can canonicalise
    // a remote and derive a deterministic uid from it.
    let repo_tmp = TempDir::new().unwrap();
    let repo = repo_tmp.path().join("passpage");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .expect("spawn git")
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q"]);
    git(&["remote", "add", "origin", "git@github.com:acme/passpage.git"]);

    let state_tmp = TempDir::new().unwrap();
    let state_dir = state_tmp.path();
    let expected_uid = {
        let store = WorkspaceStore::open(
            &config::database_path(state_dir),
            "test-host".into(),
            config::workspaces_root(state_dir),
        )
        .unwrap();
        // No project_root passed — the daemon derives identity from the
        // repo path itself (the agent-created-a-root-project case).
        let ws = store
            .create(
                Some("passpage".into()),
                repo.display().to_string(),
                Some("main".into()),
                None,
            )
            .unwrap();
        assert_eq!(ws.kind.as_deref(), Some("main"));
        assert_eq!(
            ws.repo_remote.as_deref(),
            Some("github.com/acme/passpage"),
            "daemon must canonicalise the git origin remote"
        );
        ws.project_uid.expect("daemon must stamp a project_uid")
    };

    // Through the real `workspace list` binary.
    let stdout = run_workspace_list(state_dir);
    let parsed = parse_inventory_json(&stdout).unwrap();
    assert_eq!(parsed.workspaces.len(), 1);
    let w = &parsed.workspaces[0];
    assert_eq!(w.project_uid.as_deref(), Some(expected_uid.as_str()));
    assert_eq!(w.kind.as_deref(), Some("main"));
    assert_eq!(w.repo_remote.as_deref(), Some("github.com/acme/passpage"));

    // Into the desktop sync mirror.
    let desktop_db = DatabaseStore::new_in_memory();
    reconcile_host_inventory(&desktop_db, "host-pandora", &parsed);
    let rows = desktop_db.list_remote_discovered_for_host("host-pandora");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].project_uid.as_deref(), Some(expected_uid.as_str()));
    assert_eq!(rows[0].workspace_kind.as_deref(), Some("main"));
    assert_eq!(
        rows[0].project_remote.as_deref(),
        Some("github.com/acme/passpage"),
        "project_remote is now populated for remote-discovered rows"
    );
}

#[test]
fn cli_to_reconcile_is_idempotent_across_polls() {
    // Steady-state property: if the host's registry didn't change,
    // a second reconcile must produce zero inserts/updates/soft-
    // deletes and must NOT re-mark already-synced rows as dirty.
    // Otherwise the cloud sync layer would PATCH every workspace on
    // every poll for the rest of time, which is both wasteful and
    // would defeat the "dirty == there's something to push" signal
    // the rest of the system depends on.
    let bin = binary_path();
    if !bin.exists() {
        return;
    }

    let state_tmp = TempDir::new().unwrap();
    let state_dir = state_tmp.path();
    let uid = {
        let store = WorkspaceStore::open(
            &config::database_path(state_dir),
            "test-host".into(),
            config::workspaces_root(state_dir),
        )
        .unwrap();
        store
            .create(
                Some("only-one".into()),
                "/srv/only".into(),
                Some("main".into()),
                None,
            )
            .unwrap()
            .id
    };

    let desktop_db = DatabaseStore::new_in_memory();

    // First poll cycle.
    let stdout1 = run_workspace_list(state_dir);
    let parsed1 = parse_inventory_json(&stdout1).unwrap();
    let stats1 = reconcile_host_inventory(&desktop_db, "host-1", &parsed1);
    assert_eq!(stats1.inserted, 1);

    // Simulate the first push assigning a cloud server_id and
    // clearing dirty (the steady state of any successfully-synced
    // row).
    let row = desktop_db
        .find_workspace_sync_by_host_and_origin_uid("host-1", &uid)
        .unwrap();
    desktop_db
        .mark_workspace_sync_synced(row.id, Some("cloud-7"))
        .unwrap();
    assert!(
        !desktop_db
            .find_workspace_sync_by_host_and_origin_uid("host-1", &uid)
            .unwrap()
            .dirty
    );

    // Second poll cycle — identical inventory. Must be a no-op all
    // the way through.
    let stdout2 = run_workspace_list(state_dir);
    let parsed2 = parse_inventory_json(&stdout2).unwrap();
    let stats2 = reconcile_host_inventory(&desktop_db, "host-1", &parsed2);
    assert_eq!(stats2.inserted, 0, "no inserts on identical re-poll");
    assert_eq!(stats2.updated, 0, "no updates on identical re-poll");
    assert_eq!(stats2.soft_deleted, 0);
    assert!(
        !desktop_db
            .find_workspace_sync_by_host_and_origin_uid("host-1", &uid)
            .unwrap()
            .dirty,
        "identical re-poll MUST NOT mark the row dirty — otherwise every \
         poll burns a cloud PATCH for no reason"
    );
}

#[test]
fn cli_to_reconcile_propagates_host_side_renames_and_closes() {
    // Real-world flow: agent renames a workspace via the daemon's
    // `workspace_update` tool, and closes another via
    // `workspace_close`. The desktop must learn both on the next
    // poll cycle — UPDATE in place + soft-delete respectively.
    let bin = binary_path();
    if !bin.exists() {
        return;
    }

    let state_tmp = TempDir::new().unwrap();
    let state_dir = state_tmp.path();

    let (rename_uid, doomed_uid) = {
        let store = WorkspaceStore::open(
            &config::database_path(state_dir),
            "test-host".into(),
            config::workspaces_root(state_dir),
        )
        .unwrap();
        let rn = store
            .create(Some("original".into()), "/srv/rn".into(), Some("main".into()), None)
            .unwrap();
        let dm = store
            .create(Some("doomed".into()), "/srv/dm".into(), Some("main".into()), None)
            .unwrap();
        (rn.id, dm.id)
    };

    let desktop_db = DatabaseStore::new_in_memory();
    let stats = reconcile_host_inventory(
        &desktop_db,
        "host-1",
        &parse_inventory_json(&run_workspace_list(state_dir)).unwrap(),
    );
    assert_eq!(stats.inserted, 2);
    // Pretend both got pushed and assigned cloud ids.
    for uid in [&rename_uid, &doomed_uid] {
        let r = desktop_db
            .find_workspace_sync_by_host_and_origin_uid("host-1", uid)
            .unwrap();
        desktop_db
            .mark_workspace_sync_synced(r.id, Some(&format!("cloud-{uid}")))
            .unwrap();
    }

    // Mutate the host side: rename one, close the other.
    {
        let store = WorkspaceStore::open(
            &config::database_path(state_dir),
            "test-host".into(),
            config::workspaces_root(state_dir),
        )
        .unwrap();
        store
            .update(&rename_uid, Some("renamed-on-host".into()), None, None)
            .unwrap();
        store.close(&doomed_uid).unwrap();
    }

    let stats2 = reconcile_host_inventory(
        &desktop_db,
        "host-1",
        &parse_inventory_json(&run_workspace_list(state_dir)).unwrap(),
    );
    assert_eq!(stats2.inserted, 0);
    assert_eq!(stats2.updated, 1, "renamed workspace must UPDATE in place");
    assert_eq!(
        stats2.soft_deleted, 1,
        "closed workspace must be soft-deleted so push DELETEs the cloud row"
    );

    // The renamed row carries the new title AND keeps its
    // cloud server_id — losing the server_id here would cause the
    // next push to POST a duplicate and orphan the original row.
    let after_rename = desktop_db
        .find_workspace_sync_by_host_and_origin_uid("host-1", &rename_uid)
        .unwrap();
    assert_eq!(after_rename.title, "renamed-on-host");
    assert_eq!(
        after_rename.server_id.as_deref(),
        Some(format!("cloud-{rename_uid}").as_str()),
        "in-place update must preserve the cloud server_id"
    );
    assert!(
        after_rename.dirty,
        "a real field change must mark the row dirty so push propagates it"
    );

    // The doomed row is a tombstone visible only to the sync-loop
    // list, with deleted_at + dirty=1 + the cloud server_id intact.
    let tombstone = desktop_db
        .list_workspaces_sync_for_sync()
        .into_iter()
        .find(|r| r.origin_uid.as_deref() == Some(&doomed_uid))
        .expect("tombstone must still be present pre-push");
    assert!(tombstone.deleted_at.is_some());
    assert!(tombstone.dirty);
    assert_eq!(
        tombstone.server_id.as_deref(),
        Some(format!("cloud-{doomed_uid}").as_str())
    );
}
