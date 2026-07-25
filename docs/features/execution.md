# Execution Backends

- Purpose: Describe Codemux's sandbox-execution abstraction that gates how agent sessions (and any other spawn path that opts in) launch child processes.
- Audience: Anyone working on agent sandboxing, cross-platform spawn behavior, display isolation, or permission policies. OpenFlow (Codemux's built-in multi-agent orchestrator) is the largest current consumer but not the only one.
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
    pub virtual_display: bool,
}
```

`ExecutionPolicy::openflow_agent_default()` (historical name — today it's the default for every agent session, not strictly OpenFlow-only) picks the backend based on the compile-time host OS (`cfg!(target_os = ...)`) and sets sensible defaults (network on, browser automation on, desktop GUI off).

`prepare_agent_command(executable, args, cwd, policy)` is the spawn-time entry point. It reads `policy.effective_backend()` and returns a `PreparedExecutionCommand` with the real executable, args, and the backend that was actually applied. On Linux with Bubblewrap, it wraps the command in `bwrap --bind / / --unshare-pid --unshare-ipc --die-with-parent --new-session --proc /proc --dev-bind /dev --tmpfs /tmp/.X11-unix ...` (see `build_linux_bwrap_args`), plus an `XDG_RUNTIME_DIR` tmpfs mask. `--unshare-net` is **conditional** — emitted only when `policy.allow_network` is false, which the default agent policy is not, so the default agent spawn does not unshare the network namespace. On every other branch today, it returns the original command unwrapped — the macOS and Windows backends are placeholders.

`effective_backend()` degrades gracefully: a preference of `LinuxBubblewrap` on non-Linux falls back to `HostPassthrough`, and if `bwrap` isn't on `PATH` on Linux, the spawn also falls back to `HostPassthrough` with a diagnostics breadcrumb.

## What Works Today

- Linux Bubblewrap sandbox with PID / IPC namespace unsharing and
  `--die-with-parent` cleanup (network unsharing only when `allow_network`
  is false — see above)
- Graceful `HostPassthrough` fallback when `bwrap` is not installed (logged but not fatal)
- Capability flags threaded through the policy struct and serialized for UI / settings
- Agent sessions spawn via this path — consumers (OpenFlow today, potentially others later) do NOT touch `Command::new()` directly
- Backend label string (`"linux_bubblewrap"`, `"host_passthrough"`, etc.) surfaced in observability logs for every agent spawn
- Cross-platform compile (the `LinuxBubblewrap` arm is `#[cfg(target_os = "linux")]` gated inside `prepare_agent_command`)
- **Cross-platform GUI env handling (Phase 1 display isolation).** `PreparedExecutionCommand` carries **two** lists:
  - `env_unset` — GUI variables removed via `CommandBuilder::env_remove` by both `spawn_pty_for_agent` and `spawn_pty_for_session` after all env setting. Empty on the bwrap branch (bwrap handles it via `--unsetenv`); populated from `gui_env_keys()` on every fallback path (macOS stub, Windows stub, bwrap-missing Linux, `HostPassthrough`) whenever `allow_desktop_gui=false`.
  - `env_set` — GUI *neutralizers* from `gui_env_overrides()`: `BROWSER=true`, `MOZ_NO_REMOTE=1`, `DBUS_SESSION_BUS_ADDRESS=disabled:`, `XDG_CURRENT_DESKTOP=X-Generic`. Unsetting alone isn't enough; these actively steer well-behaved tools away from the host session.

  Helpers `sanitize_gui_env_std` / `sanitize_gui_env_tokio` (and their `*_keep_dbus` variants) apply the same treatment to non-PTY `Command` spawns.
- **Adapter `extra_env` is filtered** against `env_unset` in `spawn_pty_for_agent` so an adapter propagating `DISPLAY` from the host env cannot silently un-do the strip.
- **`ExecutionPolicy::worktree_session_default()`** governs regular (non-agent) worktree shells: `HostPassthrough` backend, no wrapping, and `allow_desktop_gui` read from the `CODEMUX_ALLOW_DESKTOP_GUI` env var — **defaulting to `false`** (safe-by-default since April 2026). Set `CODEMUX_ALLOW_DESKTOP_GUI=1|true|yes` before launching Codemux to restore host GUI inheritance for these shells.
- **Policy `virtual_display: bool` field** with `#[serde(default)]` for backward compatibility. `openflow_agent_default()` reads `CODEMUX_VIRTUAL_DISPLAY=1|true|yes` at construction time and flips the flag.
- **Workspace close releases the virtual display.** Both `close_workspace` and `close_workspace_with_worktree` in `src-tauri/src/commands/workspace.rs` call `VirtualDisplayManager::release(workspace_id)` after PTY teardown. Idempotent — no-op if the workspace never acquired one.

## Built But Not Wired (virtual display, Phase 2 / 2.5)

The Xvfb virtual-display stack in `src-tauri/src/execution/virtual_display.rs` is
implemented and unit-tested, and `VirtualDisplayManager` is `.manage()`d in
`lib.rs` — but **no spawn path calls `acquire()`**. The only non-definition call
site in the repo is inside the module's own `#[cfg(test)]` block. Everything in
this section is therefore unreachable at runtime today; the release-on-close
wiring above exists and would work, but nothing ever acquires a display for it
to release.

What the module implements, for whoever wires it up:

- Per-workspace Xvfb: idempotent acquire, `SIGTERM` → 5s grace → `SIGKILL` on release, orphan sweep of `/tmp/.X<n>-lock` on startup, shutdown-all on exit via `Drop`. Flags: `-screen 0 1920x1080x24 -dpi 96 -noreset -nolisten tcp +extension GLX +extension RANDR -auth <cookie>`. Displays allocate from `:1000` to dodge host sessions (`:0`–`:2`) and CI (`:99`).
- Unsupported on macOS/Windows: `is_supported()` returns `false`, `acquire()` returns `Error::Unsupported` / `Error::XvfbNotFound`.
- MIT-MAGIC-COOKIE-1 xauth: a 128-bit cookie at `/tmp/codemux-vd-<N>.Xauthority` (mode `0600`) via the `xauth` CLI; `XAUTHORITY` propagates through `VirtualDisplayEnv::env_pairs()`. Degrades without `xauth`.
- `x11vnc` companion ("watch the agent"), gated on the internal `AcquireOptions.watch_vnc` field: `-passwdfile read:<file>` with a random per-display token (deliberately **not** `-nopw`, which left the same-UID case open), bound to `127.0.0.1`, port probe range `5910-5999`. Killed before Xvfb on release. Missing `x11vnc` is non-fatal.
- Tauri command `get_workspace_virtual_display` returns `VirtualDisplayInfo { display, vnc_port, vnc_password, supported }`. It is registered in `lib.rs` but has **no wrapper in `src/tauri/commands.ts` and no caller in `src/`** — the intended hook for a future "watch the agent" pane.

There is **no `.codemux/config.json` `sandbox` section.** `WorkspaceConfig`
(`src-tauri/src/config/workspace_config.rs`) has exactly four fields — `setup`,
`teardown`, `run`, `worktree_includes` — and no `SandboxConfig` type exists.
A previous version of this doc described a `sandbox` block with
`virtual_display` / `watch_vnc` / `allow_desktop_gui` keys and a
`terminal::apply_workspace_sandbox_overrides` reader; neither was ever
implemented. Per-workspace opt-in remains unbuilt; the env vars above are the
only switches.

## Current Constraints

- **macOS backend is a placeholder**: `MacOsSandbox` falls through to `HostPassthrough`. A real implementation would use `sandbox-exec` with a profile (`/usr/bin/sandbox-exec -f <profile> ...`).
- **Windows backend is a placeholder**: `WindowsRestricted` also falls through to `HostPassthrough`. A real implementation would use Job Objects (`CreateJobObject` + `SetInformationJobObject` for process limits) or AppContainer (`CreateProcessAsUser` with a restricted token).
- **The Bubblewrap policy is hardcoded** — `build_linux_bwrap_args` has a single profile tuned for agent-session use. There is no per-workspace policy at all yet — neither config-file nor UI (see "Built But Not Wired" above).
- **`allow_desktop_gui: false` is now enforced cross-platform via env-strip** on the `HostPassthrough` branch and the bwrap-missing fallback, in addition to the `--unsetenv` flags on the real bwrap branch. The strip is not a security control — a determined child can probe sockets directly — but it stops ~95% of accidental GUI popups from `npm run dev`, `electron .`, `playwright --headed`, etc. Real containment would be the virtual display (built, unwired) or a full sandbox (not started).
- **No syscall filtering** — seccomp-bpf isn't wired into the Bubblewrap profile. An agent can still `unlink()` the user's files inside its own filesystem view because `bwrap --bind / /` exposes the host root read-write by default.

## Important Touch Points

- `src-tauri/src/execution/mod.rs` — `ExecutionBackendKind`, `ExecutionPolicy`, `PreparedExecutionCommand`, `prepare_agent_command`, `build_linux_bwrap_args`, `gui_env_keys`, `gui_env_overrides`, `sanitize_gui_env_*`
- `src-tauri/src/execution/virtual_display.rs` — the built-but-unwired Xvfb/x11vnc manager
- `src-tauri/src/openflow/mod.rs` / `src-tauri/src/commands/openflow.rs` — OpenFlow agents read an `ExecutionPolicy` from their `AgentConfig` and pass it to `spawn_pty_for_agent`
- `src-tauri/src/terminal/mod.rs` — `spawn_pty_for_agent` calls `prepare_agent_command` before touching portable-pty
- `docs/plans/windows-support.md` — tracks the Windows backend work and notes that OpenFlow is currently disabled on Windows because the wrappers (separate concern from execution backend) are not ported yet
- `docs/features/openflow.md` — describes the agent orchestration that sits on top of this execution layer
