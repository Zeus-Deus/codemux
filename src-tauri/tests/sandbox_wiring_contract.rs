//! Contract canary tests for Phase 2 display-isolation wiring.
//!
//! These tests do not drive real processes; they guard specific *code
//! contracts* in the source tree that would otherwise be silently broken
//! by a refactor. They're ugly-looking but catch the exact class of
//! regression — "someone removed the `release()` call" — that integration
//! tests would miss without a full Tauri app-handle fixture.
//!
//! Tracked in `docs/plans/sandboxing.md` (Phase 2.5 hardening, task #33).
//!
//! If you find yourself editing these assertions because a refactor legit-
//! imately renamed the functions or moved the calls, that's fine — just
//! update the canary and confirm the new wiring is equivalent. The point
//! is that every change to these files gets a code-review prompt via the
//! test failure.

use std::fs;
use std::path::PathBuf;

fn source_root() -> PathBuf {
    // Cargo runs integration tests with CWD = package root (src-tauri/).
    std::env::current_dir().expect("cwd")
}

fn read(rel: &str) -> String {
    let path = source_root().join(rel);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// If this fails: someone removed (or renamed) the virtual-display
/// `release()` call in `close_workspace` — leaked Xvfb on close.
#[test]
fn close_workspace_releases_virtual_display() {
    let src = read("src/commands/workspace.rs");

    // Look for the function header and the release call appearing together.
    // We scope to the function by splitting on the signature.
    let (_, after_header) = src
        .split_once("pub fn close_workspace(")
        .expect("close_workspace fn must exist");

    // Find where the next top-level pub fn starts, so we only look inside
    // close_workspace. `cycle_workspace` is the next one in this file.
    let body = after_header
        .split_once("#[tauri::command]")
        .map(|(body, _)| body)
        .unwrap_or(after_header);

    assert!(
        body.contains("VirtualDisplayManager"),
        "close_workspace must reference VirtualDisplayManager so the \
         per-workspace Xvfb gets released on close"
    );
    assert!(
        body.contains(".release(&workspace_id)")
            || body.contains(".release(&workspace_id.0)")
            || body.contains(".release("),
        "close_workspace must call vd_manager.release(...) to tear down \
         the virtual display when the workspace is closed"
    );
}

/// Same contract for the worktree-variant close path.
#[test]
fn close_workspace_with_worktree_releases_virtual_display() {
    let src = read("src/commands/workspace.rs");
    let (_, after_header) = src
        .split_once("pub fn close_workspace_with_worktree(")
        .expect("close_workspace_with_worktree fn must exist");
    let body = after_header
        .split_once("#[tauri::command]")
        .map(|(body, _)| body)
        .unwrap_or(after_header);

    assert!(
        body.contains("VirtualDisplayManager"),
        "close_workspace_with_worktree must reference VirtualDisplayManager"
    );
    assert!(
        body.contains(".release("),
        "close_workspace_with_worktree must call vd_manager.release(...) \
         to tear down the virtual display on worktree close"
    );
}

/// If this fails: someone removed the virtual-display registration from
/// the Tauri builder in `lib.rs` — the manager would be unavailable as
/// State, causing `spawn_pty_for_agent` to panic on `app.state()`.
#[test]
fn lib_registers_virtual_display_manager_as_tauri_state() {
    let src = read("src/lib.rs");
    assert!(
        src.contains("VirtualDisplayManager::new()"),
        "lib.rs must register VirtualDisplayManager in the Tauri builder \
         so spawn_pty_for_agent can look it up via State<VirtualDisplayManager>"
    );
    assert!(
        src.contains(".manage(execution::virtual_display::"),
        "VirtualDisplayManager must be passed to Tauri's `.manage(...)` — \
         if it isn't managed, app.state() will panic at runtime"
    );
}

/// If this fails: someone removed the `get_workspace_virtual_display`
/// command from the invoke_handler list — the frontend would get a
/// "command not found" error when trying to discover the VNC port.
#[test]
fn lib_registers_get_workspace_virtual_display_command() {
    let src = read("src/lib.rs");
    // Match both the handler-list registration form and the module path.
    assert!(
        src.contains("get_workspace_virtual_display"),
        "lib.rs must register the get_workspace_virtual_display command \
         in the tauri::generate_handler![] macro so the frontend can \
         discover a workspace's VNC port and password"
    );
}

/// If this fails: someone inverted the env-strip order in
/// spawn_pty_for_agent, letting `extra_env` silently re-set DISPLAY. This
/// is a specific regression class we explicitly closed in Phase 1 — guard
/// against it coming back.
#[test]
fn spawn_pty_for_agent_applies_env_unset_after_extra_env() {
    let src = read("src/terminal/mod.rs");

    // Find the spawn_pty_for_agent function body.
    let (_, agent_body) = src
        .split_once("pub fn spawn_pty_for_agent(")
        .expect("spawn_pty_for_agent fn must exist");
    let body = agent_body
        .split_once("pub fn ")
        .map(|(body, _)| body)
        .unwrap_or(agent_body);

    let extra_env_idx = body
        .find("for (key, val) in &extra_env")
        .expect("extra_env merge loop must exist in spawn_pty_for_agent");
    let env_unset_idx = body
        .find("for key in &prepared.env_unset")
        .expect("env_unset apply loop must exist in spawn_pty_for_agent");
    assert!(
        env_unset_idx > extra_env_idx,
        "env_unset MUST be applied after the extra_env merge — otherwise \
         an adapter that puts DISPLAY in extra_env would silently un-do \
         the Phase 1 strip. See Phase 1 gotcha in docs/plans/sandboxing.md."
    );
}

/// If this fails: someone is constructing `ExecutionPolicy` by hand
/// somewhere without setting the new `virtual_display` field, meaning
/// they broke back-compat. We can't catch this at the type level (it's
/// an inherent struct, not a pub-fn-only API), so we contract-test it.
#[test]
fn execution_policy_constructors_set_virtual_display() {
    let src = read("src/execution/mod.rs");
    assert!(
        src.contains("virtual_display"),
        "ExecutionPolicy should expose a `virtual_display` field"
    );
    // Both named constructors must set it — rg for the field in both fns.
    let (_, after_openflow) = src
        .split_once("pub fn openflow_agent_default()")
        .expect("openflow_agent_default must exist");
    let openflow_body = after_openflow
        .split_once("pub fn ")
        .map(|(body, _)| body)
        .unwrap_or(after_openflow);
    assert!(
        openflow_body.contains("virtual_display"),
        "openflow_agent_default must set virtual_display explicitly"
    );

    let (_, after_worktree) = src
        .split_once("pub fn worktree_session_default()")
        .expect("worktree_session_default must exist");
    let worktree_body = after_worktree
        .split_once("pub fn ")
        .map(|(body, _)| body)
        .unwrap_or(after_worktree);
    assert!(
        worktree_body.contains("virtual_display"),
        "worktree_session_default must set virtual_display explicitly"
    );
}
