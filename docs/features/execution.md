# Execution Backends

- Purpose: Describe the sandbox-execution abstraction that gates how OpenFlow agents spawn child processes.
- Audience: Anyone working on OpenFlow, sandboxing, cross-platform spawn behavior, or permission policies.
- Authority: Canonical feature doc for execution policy and backends.
- Update when: A new backend is added, an existing backend's behavior changes, or the default policy changes.
- Read next: `docs/features/openflow.md`, `docs/plans/windows-support.md`

## What This Feature Is

Execution backends are Codemux's cross-platform abstraction for "how should we spawn an agent child process?". Each host OS picks a different backend: Linux uses Bubblewrap for sandboxed namespace isolation, macOS will use `sandbox-exec`, Windows will use Job Objects / AppContainer, and anything else falls through to a host-passthrough backend that just runs the command directly.

The policy also carries capability flags (`allow_network`, `allow_browser_automation`, `allow_desktop_gui`) so callers can express intent like "this agent can do network requests and control the browser, but it should not be allowed to pop GUI windows on the user's display".

## Current Model

The Rust type lives in `src-tauri/src/execution/mod.rs`:

```rust
pub enum ExecutionBackendKind {
    HostPassthrough,
    LinuxBubblewrap,
    MacOsSandbox,
    WindowsRestricted,
}

pub struct ExecutionPolicy {
    pub backend_preference: ExecutionBackendKind,
    pub allow_network: bool,
    pub allow_browser_automation: bool,
    pub allow_desktop_gui: bool,
}
```

`ExecutionPolicy::openflow_agent_default()` picks the backend based on the compile-time host OS (`cfg!(target_os = ...)`) and sets sensible defaults (network on, browser automation on, desktop GUI off).

`prepare_agent_command(executable, args, cwd, policy)` is the spawn-time entry point. It reads `policy.effective_backend()` and returns a `PreparedExecutionCommand` with the real executable, args, and the backend that was actually applied. On Linux with Bubblewrap, it wraps the command in `bwrap --bind / / --unshare-pid --unshare-ipc --unshare-net --die-with-parent ...` (see `build_linux_bwrap_args`). On every other branch today, it returns the original command unwrapped — the macOS and Windows backends are placeholders.

`effective_backend()` degrades gracefully: a preference of `LinuxBubblewrap` on non-Linux falls back to `HostPassthrough`, and if `bwrap` isn't on `PATH` on Linux, the spawn also falls back to `HostPassthrough` with a diagnostics breadcrumb.

## What Works Today

- Linux Bubblewrap sandbox with PID / IPC / network namespace unsharing and `--die-with-parent` cleanup
- Graceful `HostPassthrough` fallback when `bwrap` is not installed (logged but not fatal)
- Capability flags threaded through the policy struct and serialized for UI / settings
- OpenFlow agents spawn via this path — the orchestrator does NOT touch `Command::new()` directly
- Backend label string (`"linux_bubblewrap"`, `"host_passthrough"`, etc.) surfaced in observability logs for every agent spawn
- Cross-platform compile (the `LinuxBubblewrap` arm is `#[cfg(target_os = "linux")]` gated inside `prepare_agent_command`)
- **Cross-platform GUI env-strip (Phase 1 display isolation):** `PreparedExecutionCommand` carries an `env_unset` list that both `spawn_pty_for_agent` and `spawn_pty_for_session` apply via `CommandBuilder::env_remove` after all env setting. On the bwrap branch the list is empty (bwrap handles it via `--unsetenv`); on every fallback path — macOS stub, Windows stub, bwrap-missing Linux, `HostPassthrough` — the list is populated from `gui_env_keys()` whenever `allow_desktop_gui=false`. This is what finally makes the OpenFlow `allow_desktop_gui: false` default actually take effect on macOS and Windows instead of being advisory.
- **Adapter `extra_env` is filtered** against `env_unset` in `spawn_pty_for_agent` so an adapter propagating `DISPLAY` from the host env cannot silently un-do the strip.
- **`ExecutionPolicy::worktree_session_default()`** preserves current behavior for regular (non-OpenFlow) worktree shells: `HostPassthrough` backend, `allow_desktop_gui: true`, no wrapping. Future per-workspace config can flip `allow_desktop_gui: false` to opt a workspace into env-strip without wrapping the shell in bwrap.
- **Phase 2: Per-workspace virtual display (Linux).** `src-tauri/src/execution/virtual_display.rs` manages Xvfb processes per workspace: lazy spawn on first agent acquire, idempotent acquire (same workspace gets same display), graceful `SIGTERM` → 5s grace → `SIGKILL` on release, orphan sweep of `/tmp/.X<n>-lock` on startup, shutdown-all on app exit via `Drop`. Xvfb is launched with the 2026-canonical flags for agent dev workflows: `-screen 0 1920x1080x24 -dpi 96 -noreset -nolisten tcp +extension GLX +extension RANDR -auth <cookie>`. Display numbers are allocated starting at `:1000` to avoid colliding with host session displays (`:0`–`:2`) and CI conventions (`:99`). Unsupported on macOS/Windows today — `is_supported()` returns `false` and `acquire()` returns `Error::Unsupported`/`Error::XvfbNotFound`; spawn code falls back to Phase 1 env-strip without crashing.
- **Policy `virtual_display: bool` field** with `#[serde(default)]` for backward compatibility. `ExecutionPolicy::openflow_agent_default()` reads `CODEMUX_VIRTUAL_DISPLAY=1` / `true` / `yes` at construction time and flips the flag — opt-in for users who want computer-use today without editing JSON config. Per-workspace config (`.codemux/config.json` `sandbox.virtual_display`) also available — see below.
- **Workspace close releases the virtual display.** Both `close_workspace` and `close_workspace_with_worktree` in `src-tauri/src/commands/workspace.rs` call `VirtualDisplayManager::release(workspace_id)` after PTY teardown. Idempotent — no-op if the workspace never opted in.
- **Phase 2.5: Per-workspace `.codemux/config.json` `sandbox` section.** Users opt into display isolation per workspace without setting env vars:
  ```json
  {
    "sandbox": {
      "virtual_display": true,
      "watch_vnc": true,
      "allow_desktop_gui": false
    }
  }
  ```
  All three fields are `Option<bool>` — absent fields fall back to the caller's policy defaults. `terminal::apply_workspace_sandbox_overrides` reads the config at spawn time and overrides the adapter-provided `ExecutionPolicy` before `prepare_agent_command`.
- **Phase 2.5: MIT-MAGIC-COOKIE-1 xauth file per display.** On acquire, a 128-bit random cookie is written to `/tmp/codemux-vd-<N>.Xauthority` (permissions `0600`) using the `xauth` CLI. Xvfb is launched with `-auth <file>`; `XAUTHORITY` is propagated to child processes via `VirtualDisplayEnv::env_pairs()`. Without the cookie, any process on the host that guesses the display number could connect to the hidden display. Best-effort: if `xauth` isn't on PATH, Xvfb still spawns without `-auth` and a log line mentions the degrade.
- **Phase 2.5: `x11vnc` companion process for "watch the agent".** When `sandbox.watch_vnc: true`, the manager spawns `x11vnc -display :N -rfbport <free-port> -localhost -forever -quiet -noxdamage -nopw -auth <cookie>` bound to `127.0.0.1`. Port probe range `5910-5999` (leaves `5900-5909` for real VNC servers). On release, VNC is killed *before* Xvfb so the user doesn't see "connection lost" spam. Missing `x11vnc` is non-fatal: acquire succeeds without VNC, log line notes the degrade.
- **Tauri command `get_workspace_virtual_display`.** Returns `{ display, vnc_port, supported }` so the frontend can discover whether a workspace has an active hidden display and where VNC is listening. This is the hook for a future "watch the agent" pane (Phase 3).

## Current Constraints

- **macOS backend is a placeholder**: `MacOsSandbox` falls through to `HostPassthrough`. A real implementation would use `sandbox-exec` with a profile (`/usr/bin/sandbox-exec -f <profile> ...`).
- **Windows backend is a placeholder**: `WindowsRestricted` also falls through to `HostPassthrough`. A real implementation would use Job Objects (`CreateJobObject` + `SetInformationJobObject` for process limits) or AppContainer (`CreateProcessAsUser` with a restricted token).
- **The Bubblewrap policy is hardcoded** — `build_linux_bwrap_args` has a single profile tuned for OpenFlow agents. There's no UI-configurable per-workspace policy yet.
- **`allow_desktop_gui: false` is now enforced cross-platform via env-strip** on the `HostPassthrough` branch and the bwrap-missing fallback, in addition to the `--unsetenv` flags on the real bwrap branch. The strip is not a security control — a determined child can probe sockets directly — but it stops ~95% of accidental GUI popups from `npm run dev`, `electron .`, `playwright --headed`, etc. Real containment is Phase 2 (virtual display) / Phase 3 (full sandbox).
- **No syscall filtering** — seccomp-bpf isn't wired into the Bubblewrap profile. An agent can still `unlink()` the user's files inside its own filesystem view because `bwrap --bind / /` exposes the host root read-write by default.

## Important Touch Points

- `src-tauri/src/execution/mod.rs` — `ExecutionBackendKind`, `ExecutionPolicy`, `PreparedExecutionCommand`, `prepare_agent_command`, `build_linux_bwrap_args`
- `src-tauri/src/openflow/mod.rs` / `src-tauri/src/commands/openflow.rs` — OpenFlow agents read an `ExecutionPolicy` from their `AgentConfig` and pass it to `spawn_pty_for_agent`
- `src-tauri/src/terminal/mod.rs` — `spawn_pty_for_agent` calls `prepare_agent_command` before touching portable-pty
- `docs/plans/windows-support.md` — tracks the Windows backend work and notes that OpenFlow is currently disabled on Windows because the wrappers (separate concern from execution backend) are not ported yet
- `docs/features/openflow.md` — describes the agent orchestration that sits on top of this execution layer
