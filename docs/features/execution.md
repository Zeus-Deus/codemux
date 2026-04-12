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

## Current Constraints

- **macOS backend is a placeholder**: `MacOsSandbox` falls through to `HostPassthrough`. A real implementation would use `sandbox-exec` with a profile (`/usr/bin/sandbox-exec -f <profile> ...`).
- **Windows backend is a placeholder**: `WindowsRestricted` also falls through to `HostPassthrough`. A real implementation would use Job Objects (`CreateJobObject` + `SetInformationJobObject` for process limits) or AppContainer (`CreateProcessAsUser` with a restricted token).
- **The Bubblewrap policy is hardcoded** — `build_linux_bwrap_args` has a single profile tuned for OpenFlow agents. There's no UI-configurable per-workspace policy yet.
- **`allow_desktop_gui: false` is advisory, not enforced** — on the `HostPassthrough` fallback (which is everything except real Linux + bwrap), nothing stops an agent from opening a GUI window. On the Bubblewrap path, `build_linux_bwrap_args` clears `DISPLAY` / `WAYLAND_DISPLAY` / `DBUS_SESSION_BUS_ADDRESS` to block GUI spawning when `allow_desktop_gui` is false.
- **No syscall filtering** — seccomp-bpf isn't wired into the Bubblewrap profile. An agent can still `unlink()` the user's files inside its own filesystem view because `bwrap --bind / /` exposes the host root read-write by default.

## Important Touch Points

- `src-tauri/src/execution/mod.rs` — `ExecutionBackendKind`, `ExecutionPolicy`, `PreparedExecutionCommand`, `prepare_agent_command`, `build_linux_bwrap_args`
- `src-tauri/src/openflow/mod.rs` / `src-tauri/src/commands/openflow.rs` — OpenFlow agents read an `ExecutionPolicy` from their `AgentConfig` and pass it to `spawn_pty_for_agent`
- `src-tauri/src/terminal/mod.rs` — `spawn_pty_for_agent` calls `prepare_agent_command` before touching portable-pty
- `docs/plans/windows-support.md` — tracks the Windows backend work and notes that OpenFlow is currently disabled on Windows because the wrappers (separate concern from execution backend) are not ported yet
- `docs/features/openflow.md` — describes the agent orchestration that sits on top of this execution layer
