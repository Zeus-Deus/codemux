//! Live SSH push/pull round-trip against a real host.
//!
//! Exercises the ACTUAL transport code — `ssh::push_workspace` and
//! `ssh::pull_workspace_back` — over a real `ssh`/`rsync` to a real
//! host, rather than asserting on argv construction. Proves:
//!   - push lands the worktree contents (not dir-nested) at the remote
//!     path, honoring the excludes (`node_modules/`, `.git/index.lock`);
//!   - `--delete` mirrors a subsequent push (removed local file vanishes
//!     remotely);
//!   - pull brings it back intact;
//!   - the `test -d` guard returns `RemoteNotFound` for a missing remote
//!     path and does NOT `--delete`-wipe the local target (the dangerous
//!     edge the guard exists to prevent).
//!
//! GATED: skips unless `CODEMUX_E2E_SSH_HOST` names an SSH target the
//! current user can reach passwordlessly (e.g. a `~/.ssh/config` alias).
//! CI without such a host simply no-ops. Unix-only (matches `ssh` mod).

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use codemux_lib::ssh::{
    pull_workspace_back, push_workspace, PullOptions, PullResult, PushOptions,
    PushResult,
};

fn host() -> Option<String> {
    match std::env::var("CODEMUX_E2E_SSH_HOST") {
        Ok(h) if !h.trim().is_empty() => Some(h),
        _ => None,
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

#[test]
fn ssh_push_pull_round_trip_against_real_host() {
    let Some(host) = host() else {
        eprintln!("SKIP: set CODEMUX_E2E_SSH_HOST to run the live SSH round-trip");
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Unique sandbox under /tmp; remote == localhost in the loopback
    // setup, so the "remote" path is inspectable on this filesystem.
    let stamp = std::process::id();
    let base = std::env::temp_dir().join(format!("cmx-ssh-rt-{stamp}"));
    let src = base.join("src_ws");
    let remote = base.join("remote_ws");
    let pulled = base.join("pulled_ws");
    let _ = std::fs::remove_dir_all(&base);

    // A realistic worktree: tracked files, nested dirs, a .git dir, and
    // the two things the excludes must drop.
    write(&src.join("README.md"), "hello\n");
    write(&src.join("src/lib/util.rs"), "pub fn x() {}\n");
    write(&src.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&src.join(".git/index.lock"), "LOCK\n"); // must be excluded
    write(&src.join("node_modules/junk.js"), "// huge\n"); // must be excluded

    let opts = PushOptions {
        ssh_target: &host,
        local_worktree: &src,
        remote_path: remote.to_str().unwrap(),
        step_timeout: Duration::from_secs(60),
    };
    match rt.block_on(push_workspace(opts)) {
        PushResult::Pushed { .. } => {}
        other => panic!("push failed: {other:?}"),
    }

    // Contents landed (not nested under an extra dir level).
    assert!(remote.join("README.md").exists(), "README must land");
    assert!(
        remote.join("src/lib/util.rs").exists(),
        "nested file must land"
    );
    assert!(remote.join(".git/HEAD").exists(), ".git/HEAD must land");
    // Excludes honored.
    assert!(
        !remote.join("node_modules/junk.js").exists(),
        "node_modules must be excluded"
    );
    assert!(
        !remote.join(".git/index.lock").exists(),
        ".git/index.lock must be excluded"
    );

    // R5 regression: the .mcp.json the push provisions must carry an
    // ABSOLUTE command path. Agent CLIs spawn it directly — no shell
    // (so `~` won't expand) and no systemd (`%h` won't expand). The old
    // `%h` rewrite silently broke remote MCP auto-discovery.
    let mcp_json = std::fs::read_to_string(remote.join(".mcp.json"))
        .expect(".mcp.json must be provisioned by push");
    let v: serde_json::Value = serde_json::from_str(&mcp_json).unwrap();
    let cmd = v["mcpServers"]["codemux"]["command"].as_str().unwrap();
    assert!(cmd.starts_with('/'), "mcp command must be absolute, got: {cmd}");
    assert!(
        !cmd.contains('~') && !cmd.contains("%h"),
        "mcp command must not contain a ~ or %h token: {cmd}"
    );
    assert!(
        cmd.ends_with("/.local/bin/codemux-remote"),
        "expected the resolved codemux-remote path, got: {cmd}"
    );

    // --delete mirror: remove a file locally, add another, re-push.
    std::fs::remove_file(src.join("README.md")).unwrap();
    write(&src.join("NEW.txt"), "added\n");
    let opts2 = PushOptions {
        ssh_target: &host,
        local_worktree: &src,
        remote_path: remote.to_str().unwrap(),
        step_timeout: Duration::from_secs(60),
    };
    match rt.block_on(push_workspace(opts2)) {
        PushResult::Pushed { .. } => {}
        other => panic!("second push failed: {other:?}"),
    }
    assert!(
        !remote.join("README.md").exists(),
        "--delete must remove the file that was deleted locally"
    );
    assert!(remote.join("NEW.txt").exists(), "new file must land");

    // Pull back into a fresh dir → contents match the current source.
    let pull = PullOptions {
        ssh_target: &host,
        remote_path: remote.to_str().unwrap(),
        local_worktree: &pulled,
        step_timeout: Duration::from_secs(60),
    };
    match rt.block_on(pull_workspace_back(pull)) {
        PullResult::Pulled { .. } => {}
        other => panic!("pull failed: {other:?}"),
    }
    assert!(pulled.join("NEW.txt").exists(), "pulled NEW.txt");
    assert!(pulled.join("src/lib/util.rs").exists(), "pulled nested file");
    assert!(
        !pulled.join("README.md").exists(),
        "pulled tree must reflect the deletion"
    );

    // RemoteNotFound guard: pulling a missing remote path must NOT wipe
    // the local target (the whole reason for the `test -d` pre-check).
    let guard_local = base.join("guard_local");
    write(&guard_local.join("sentinel.txt"), "do not delete me\n");
    let missing = base.join("does_not_exist_remote");
    let guard = PullOptions {
        ssh_target: &host,
        remote_path: missing.to_str().unwrap(),
        local_worktree: &guard_local,
        step_timeout: Duration::from_secs(60),
    };
    match rt.block_on(pull_workspace_back(guard)) {
        PullResult::RemoteNotFound { .. } => {}
        other => panic!("expected RemoteNotFound for a missing remote, got: {other:?}"),
    }
    assert!(
        guard_local.join("sentinel.txt").exists(),
        "RemoteNotFound must NOT --delete-wipe the local target",
    );

    // R6 regression: a `~/`-relative remote path must be tilde-EXPANDED
    // by the remote shell. push's mkdir AND provision's ssh_write_file
    // both quote via the tilde-aware shell_escape; a naive single-quote
    // would create a literal `~` directory and lose the files.
    let home = std::env::var("HOME").unwrap();
    let tilde_rel = format!("cmx-e2e-tilde-{stamp}");
    let tilde_abs = std::path::PathBuf::from(&home).join(&tilde_rel);
    let _ = std::fs::remove_dir_all(&tilde_abs);
    let tsrc = base.join("tilde_src");
    write(&tsrc.join("marker.txt"), "tilde\n");
    let tpush = PushOptions {
        ssh_target: &host,
        local_worktree: &tsrc,
        remote_path: &format!("~/{tilde_rel}"),
        step_timeout: Duration::from_secs(60),
    };
    match rt.block_on(push_workspace(tpush)) {
        PushResult::Pushed { .. } => {}
        other => panic!("tilde push failed: {other:?}"),
    }
    assert!(
        tilde_abs.join("marker.txt").exists(),
        "~/ remote path must expand to $HOME (not a literal ~ dir)"
    );
    assert!(
        tilde_abs.join(".mcp.json").exists(),
        "tilde-aware ssh_write_file must drop .mcp.json under $HOME"
    );
    let _ = std::fs::remove_dir_all(&tilde_abs);

    let _ = std::fs::remove_dir_all(&base);
}
