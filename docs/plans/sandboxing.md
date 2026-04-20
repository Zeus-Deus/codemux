# Display Isolation Work Plan

- Purpose: Track work to stop agent-spawned GUI windows from popping up on the user's real desktop, and to later enable "computer use" where agents drive real GUI apps inside a hidden virtual display.
- Audience: Anyone changing execution policy, terminal spawn, workspace config, OpenFlow agent plumbing, or adding computer-use features.
- Authority: Active work plan. Current behavior truth lives in `docs/features/execution.md`.
- Update when: Priorities, open questions, or likely touch points change.
- Read next: `docs/features/execution.md`, `docs/features/openflow.md`, `docs/plans/windows-support.md`

## Goal

Stop agents running inside Codemux from pushing GUI windows onto the user's host desktop when they run `npm run dev`, Playwright-headed, Electron apps, etc. Do this in a way that **later becomes the same infrastructure** for agent "computer use" — launching real desktop apps inside a hidden per-workspace virtual display, clicking and typing like a human, and optionally surfacing that display as a pane when the user wants to watch. Must work on Linux and Windows; easy to extend to macOS.

## Status at a Glance

**Original user ask — "stop agent apps popping up on my screen"** — ✅ **SOLVED.** Phase 1 env-strip fires on every agent session spawn in Codemux across Linux/macOS/Windows. Phase 2 virtual display (Linux) is the opt-in upgrade for when you want agents to actually *run* headed apps invisibly.

This is a **Codemux-level feature**, not OpenFlow-specific. OpenFlow (Codemux's built-in multi-agent orchestrator) happens to be the loudest consumer because it runs several agents in parallel, but the same `ExecutionPolicy` + `VirtualDisplayManager` infrastructure applies to any spawn path — future agent modes, user-opted worktree terminals, anything that goes through `prepare_agent_command`.

| Phase | What | Status |
|-------|------|--------|
| 1 | Env-strip on spawn (cross-platform) | ✅ shipped, 16 tests — hardened (April 2026): default flipped to safe-by-default; neutralizer overrides added; direct `Command::new` sites migrated |
| 2 | Per-workspace Xvfb virtual display (Linux) | ✅ shipped, 18 tests (10 lib + 8 integration) |
| 2.5 | xauth cookie + x11vnc + per-workspace config + XDG_RUNTIME_DIR + PID-reuse hardening + crash detect + retry | ✅ shipped, +8 hardening tests + 6 wiring canaries |
| 3 | "Watch the agent" noVNC pane inside Codemux | ⏸ deferred — Tauri command ready, needs React component |
| 3 | Windows WSL2 integration | ⏸ deferred — graceful degrade in place |
| 3 | macOS Lima VM integration | ⏸ deferred — graceful degrade in place |
| 4 | Agent computer-use MCP tools (click/type/screenshot) | ⏸ deferred — needs `xdotool` + MCP server wiring |
| 4 | Real OS sandbox (seccomp/Landlock) | ⏸ deferred — not required for popup problem |

**Test totals (April 2026):** 663 Rust + 303 TypeScript tests passing. `cargo check` + `tsc --noEmit` both clean. Every platform-gated function has a `#[cfg(not(target_os = "linux"))]` stub returning `Error::Unsupported`, so Windows/macOS compile and degrade to Phase 1 env-strip without crashing.

**Verified April 2026** against omarchy-kb (Omarchy inherits Arch's `/tmp` systemd-tmpfiles 10-day ageing → xauth cookies moved to `$XDG_RUNTIME_DIR` with `/tmp` fallback).

## Framing Correction (Important)

Earlier drafts of this plan were titled "sandboxing." That was the wrong frame. **The popup problem is a display-isolation problem, not a security problem.** Full OS sandboxes (bwrap, seccomp, Job Objects) solve a different issue — "can this agent wreck my system" — and only block popups as a side effect. Most 2026 ADEs ship no local sandbox and still don't have popup problems, because their agents almost never launch GUIs in the first place. Codemux's problem is worse because OpenFlow runs multiple agents in parallel — occasional popups become constant flashing.

**The right fix is small: strip `DISPLAY` / `WAYLAND_DISPLAY` and friends from the child environment.** When the agent's `npm run dev` or `chromium` can't find a display, it either errors out cleanly or falls back to headless mode (which is usually what we want anyway). Later, when we want agents to drive real apps, we add an Xvfb / Xwfb virtual display per workspace and point `DISPLAY` at *that* instead. Same primitive, two phases.

Real sandboxing remains a separate, future, deprioritized concern.

## Phased Plan

### Phase 1 — Env-strip (this PR)

Extend `PreparedExecutionCommand` with an `env_unset: Vec<String>` field. On every non-bwrap path (HostPassthrough, MacOsSandbox-stub, WindowsRestricted-stub, and the bwrap-missing fallback), when `policy.allow_desktop_gui == false`, populate `env_unset` with the GUI-sensitive key list. Both `spawn_pty_for_session` and `spawn_pty_for_agent` consume `env_unset` by calling `cmd.env_remove(k)` after all `cmd.env()` calls, so it always wins over inherited env and over adapter-provided `extra_env`.

Also:
- Fix the existing gotcha in `spawn_pty_for_agent` where `extra_env` is merged *after* `prepare_agent_command` (an adapter could silently re-set `DISPLAY`).
- Apply the policy to `spawn_pty_for_session` (today it has no policy at all, so regular worktree shells inherit `DISPLAY` raw).
- Cross-platform safety: the GUI key list (`DISPLAY`, `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`, `DESKTOP_STARTUP_ID`, `XAUTHORITY`, `XDG_SESSION_TYPE`, `GDK_BACKEND`, `QT_QPA_PLATFORM`) is a no-op on Windows (those vars aren't set) and a partial benefit on macOS (XQuartz/GTK apps respect them; native Cocoa apps don't).

**Default posture (persona-based, April 2026 — superseded "always strip" approach):** policy follows the principal driving the PTY. The `TerminalPreset` struct carries a `persona: Persona` (`Human` | `Agent`); sessions carry a matching field so the split survives startup restore.

- `ExecutionPolicy::openflow_agent_default()` — `allow_desktop_gui: false` (unchanged; historical name applies to any agent consumer).
- `ExecutionPolicy::worktree_session_default_for_persona(Persona::Agent)` — `allow_desktop_gui: false`. Used for built-in Claude / Codex / OpenCode / Gemini / Pi presets and any custom preset flagged `persona: agent`.
- `ExecutionPolicy::worktree_session_default_for_persona(Persona::Human)` (aliased as the bare `worktree_session_default()`) — `allow_desktop_gui: true`. Used for plain Terminal tabs, the built-in Shell preset, and any custom preset without the agent flag.

The earlier "both constructors default to `false`" design was too aggressive: users couldn't run `npm run tauri dev` from their own terminal. The persona split restores that while keeping the anti-popup guarantee for agent-driven panes.

Global `CODEMUX_ALLOW_DESKTOP_GUI` is now three-state: `1`/`true`/`yes` force-allows regardless of persona (trust-my-agents); `0`/`false`/`no` force-denies regardless (kiosk/CI); unset or any other token falls through to the persona default.

This is all Phase 1 ships. It's ~150 lines and zero new dependencies.

### Phase 2 — Virtual display (future, same infrastructure)

Spawn one headless display per workspace — `Xvfb` on Linux (widely available, well-understood) with `wlheadless-run`/`Xwfb` as an opt-in upgrade. Track lifecycle against the workspace. When a workspace has `virtual_display: true`, Codemux sets `DISPLAY=:<n>` for its sessions instead of unsetting it, so GUI apps launch — but into the hidden display.

**This is what enables computer-use later.** Once the display exists, agents can:
- Run real Electron/Chromium/desktop apps fully headed
- Send mouse/keyboard events via `xdotool`, `ydotool`, Playwright, or Anthropic's Computer Use model
- Screenshot the display for perception
- All invisible to the user — and optionally viewable via a wayvnc/noVNC "watch the agent" pane

Windows equivalent: WSL2 with WSLg already provides a per-distro Wayland+X display. Phase 2 on Windows is "run the Linux stack inside WSL2" — same code path, different host. Pure-Win32 alternative (Windows Sandbox CLI shipped in 24H2) is opt-in for paranoid users.

macOS equivalent: Lima `vz` headless Ubuntu VM per workspace. Stubbed out in this plan; the Linux path is reusable inside the VM.

### Phase 3 — Real sandbox (deprioritized)

Bwrap profile hardening, seccomp-bpf, Landlock, Job Object UI limits. Only when we want a real security story — "your agents can't read `~/.ssh/id_rsa`." Not required to solve the popup problem and not what the user is asking for today.

## Active Priorities

1. **Ship Phase 1 this PR.** Env-strip for agent sessions on all platforms, wired into both spawn paths. Tests included.
2. **Per-workspace config surface** for `allow_desktop_gui` toggle — back-compat default preserves current behavior, agent sessions keep `false`, regular shells default `true`.
3. **Phase 2 design doc** for the virtual-display path once Phase 1 is baked.
4. **Phase 4 ("watch the agent" VNC pane)** remains the product wedge — no local ADE ships this in 2026.

## Already Landed

- `ExecutionPolicy` struct with `allow_network` / `allow_browser_automation` / `allow_desktop_gui` flags (`src-tauri/src/execution/mod.rs`)
- `ExecutionBackendKind` with four variants + `effective_backend()` graceful fallback
- `ExecutionPolicy::openflow_agent_default()` with `allow_desktop_gui: false`
- `prepare_agent_command` entry point, wraps `bwrap` on Linux when available
- `build_linux_bwrap_args` clears GUI env vars via `--unsetenv` and tmpfs-shadows `/tmp/.X11-unix` and `$XDG_RUNTIME_DIR` (preserving Codemux control socket)
- OpenFlow adapters both construct `AgentSpawnSpec` with `openflow_agent_default()`
- Backend label written to observability logs
- **Phase 1 (display isolation) shipped — hardened April 2026: default flipped to safe-by-default; neutralizer overrides added; direct `Command::new` sites migrated:**
  - `PreparedExecutionCommand.env_unset: Vec<String>` and `env_set: Vec<(String, String)>` fields; `terminal/mod.rs` applies both after all other env setting
  - `gui_env_keys()` extended from 8 to 18+ keys: original X11 set (`DISPLAY`, `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`, `DESKTOP_STARTUP_ID`, `XAUTHORITY`, `XDG_SESSION_TYPE`, `GDK_BACKEND`, `QT_QPA_PLATFORM`) plus compositor sockets (`HYPRLAND_INSTANCE_SIGNATURE`, `SWAYSOCK`), DE detection (`XDG_CURRENT_DESKTOP`, `XDG_SESSION_DESKTOP`, `DESKTOP_SESSION`, `GNOME_DESKTOP_SESSION_ID`), and toolkit-forcing knobs (`NIXOS_OZONE_WL`, `MOZ_ENABLE_WAYLAND`, `MOZ_X11_EGL`)
  - New `gui_env_overrides()` helper returns the neutralizer pairs applied alongside the unset list: `BROWSER=true` (defangs `xdg-open`), `MOZ_NO_REMOTE=1` (breaks Firefox single-instance handoff), `DBUS_SESSION_BUS_ADDRESS=disabled:` (blocks DBus auto-discovery fallback), `XDG_CURRENT_DESKTOP=X-Generic` (forces `xdg-open` onto the generic mimeapps path)
  - `build_linux_bwrap_args` now `--unsetenv`s the full key list and `--setenv`s the neutralizers
  - `HostPassthrough` / macOS stub / Windows stub / bwrap-missing fallback all populate `env_unset` + `env_set` when `allow_desktop_gui=false`
  - `ExecutionPolicy::worktree_session_default()` default for `allow_desktop_gui` flipped from `true` to `false`; both constructors honor `CODEMUX_ALLOW_DESKTOP_GUI=1`/`true`/`yes` as the opt-in escape hatch
  - `spawn_pty_for_session` routed through `prepare_agent_command` with the new `worktree_session_default()` policy
  - `spawn_pty_for_agent` applies `env_unset` AFTER every `cmd.env(...)` call including `extra_env`, and filters `extra_env` to drop keys in the unset list (fixes the silent-override gotcha)
  - New `sanitize_gui_env_std()` / `sanitize_gui_env_tokio()` helpers migrated ~20 direct `Command::new()` spawn sites (agent_browser, git, mcp_server, ai, session_adapters, commands/*, scripts) so they no longer leak host display env
  - 10 unit tests in `src-tauri/src/execution/mod.rs` + 6 integration tests in `src-tauri/tests/execution_env.rs` — all green
- **Phase 2 (per-workspace virtual display, Linux) shipped:**
  - New module `src-tauri/src/execution/virtual_display.rs` with `VirtualDisplayManager` (thread-safe, held as Tauri-managed state)
  - Canonical Xvfb spawn: `-screen 0 1920x1080x24 -dpi 96 -noreset -nolisten tcp +extension GLX +extension RANDR`; display range starts at `:1000` to avoid host/CI collisions
  - Graceful lifecycle: `SIGTERM` → 5s grace → `SIGKILL`; cleans up `/tmp/.X<n>-lock` and `/tmp/.X11-unix/X<n>` socket on release
  - Orphan sweep on `VirtualDisplayManager::new()` — unlinks stale lock files whose PID is dead (handles Codemux crashes)
  - `Drop` impl calls `shutdown_all()` for belt-and-braces cleanup on app exit
  - `ExecutionPolicy.virtual_display: bool` field with `#[serde(default)]` for back-compat
  - `openflow_agent_default()` (the historical name for the agent-session constructor) reads `CODEMUX_VIRTUAL_DISPLAY=1` env var to auto-enable virtual display for every agent it constructs — that's currently OpenFlow adapters, but any future consumer of the same constructor gets the same behavior
  - `spawn_pty_for_agent` acquires via the manager AFTER env-strip and injects `DISPLAY=:N`; graceful degrade with log line if Xvfb isn't installed
  - Both workspace close paths (`close_workspace`, `close_workspace_with_worktree`) call `release(workspace_id)` — idempotent
  - 5 unit tests + 7 integration tests in `src-tauri/tests/virtual_display.rs` (real-Xvfb tests skip cleanly when Xvfb isn't installed)
  - `is_supported()` returns `false` on macOS/Windows for graceful future extension
- **Phase 2.5 (per-workspace config + xauth + x11vnc) shipped:**
  - `WorkspaceConfig.sandbox: SandboxConfig` with three `Option<bool>` fields (`allow_desktop_gui`, `virtual_display`, `watch_vnc`), all `#[serde(default)]`
  - `terminal::apply_workspace_sandbox_overrides` reads `.codemux/config.json` at spawn time and surfaces both a merged `ExecutionPolicy` and a `WorkspaceSandboxExtras { watch_vnc }` tuple
  - `VirtualDisplayEnv` extended with `xauthority_path: Option<String>` and `vnc_port: Option<u16>`; `env_pairs()` now emits `XAUTHORITY` when the cookie file is present
  - `create_xauth_cookie` generates 128-bit random MIT-MAGIC-COOKIE-1 into `/tmp/codemux-vd-<N>.Xauthority` (`0600`), added to Xvfb via `-auth`
  - `AcquireOptions { watch_vnc }` + `acquire_with_options` for callers that want VNC
  - `spawn_x11vnc` spawns `x11vnc -localhost -forever -quiet -noxdamage -nopw -auth <cookie>` with probe-based port allocation (5910-5999)
  - Graceful degrade: missing `xauth` or `x11vnc` is logged but doesn't fail acquire; Xvfb still comes up
  - `env_for_workspace` read-only lookup for the new Tauri command
  - `graceful_kill` extracted so both Xvfb and x11vnc use the same SIGTERM/grace/SIGKILL dance; VNC killed before Xvfb to avoid "connection lost" spam
  - New `commands/virtual_display.rs` + `get_workspace_virtual_display` Tauri command returning `{ display, vnc_port, supported }` for a future "watch the agent" pane
  - 3 new sandbox config tests + 2 new virtual_display unit tests + 3 new virtual_display integration tests (xauth file perms `0600`, x11vnc port binding, env lookup) + 1 command serde test
- **Phase 2.5 hardening + verification (latest turn):**
  - `apply_workspace_sandbox_overrides` split into a pure `apply_sandbox_config_at` helper for testability; 8 new unit tests covering no-config, empty-sandbox, each field override, all-fields-together, malformed JSON, and legacy-config back-compat
  - `orphan_sweep` refactored to `orphan_sweep_in(lock_dir, socket_dir, pid_check)` with 3 new tests: stale locks unlinked, live locks kept, malformed/out-of-range ignored
  - New `pid_is_live_xvfb_for_tests` covered by 2 tests (current PID correctly rejected because its comm isn't "Xvfb"; impossible PIDs rejected)
  - `ensure_secrets_dir` now falls back to `/tmp/codemux-vd-<N>/` when `$XDG_RUNTIME_DIR` is set but unwritable — tested
  - `xvfb_is_alive` + `spawn_xvfb_with_retry` handle mid-session Xvfb crashes and display-number probe races (code review; real exercise needs live Xvfb + `kill -9`)
  - New integration test file `sandbox_wiring_contract.rs` — 6 source-level canary tests that fail loudly if anyone removes: the `release()` call in either close path, the `.manage(VirtualDisplayManager)` registration, the `get_workspace_virtual_display` command, the `env_unset`-after-`extra_env` ordering, or the `virtual_display` field in either policy constructor
- **Final verification:** 663 Rust tests pass, 303 TS tests pass, `cargo check` clean, `tsc --noEmit` clean.
- **Cross-platform safety audited:** every Xvfb/xauth/x11vnc function has a `#[cfg(not(target_os = "linux"))]` stub returning `Error::Unsupported`. Windows and macOS still compile; `is_supported()` returns `false` and spawn code degrades to Phase 1 env-strip.
- **Phase 2.6 (persona split, April 2026 second pass — supersedes the original "always strip" default):**
  - New `presets::Persona { Human, Agent }` enum with `#[serde(default)]` → `Human`. Added as a field to `TerminalPreset` (with `sync_builtins` force-refreshing builtins from the template so upgrades don't silently convert agent builtins back to Human) and to `TerminalSessionSnapshot` (so the split survives startup restore).
  - New `ExecutionPolicy::worktree_session_default_for_persona(Persona)` — Human → `allow_desktop_gui: true`; Agent → `false` + optional `virtual_display` via `CODEMUX_VIRTUAL_DISPLAY`. Bare `worktree_session_default()` kept as a Human-default wrapper.
  - `CODEMUX_ALLOW_DESKTOP_GUI` is now three-state via `parse_gui_override_env`: allow tokens force-allow regardless of persona, deny tokens force-deny regardless, unrecognized/unset falls through to the persona default.
  - `spawn_pty_for_session` reads the session's persona from state, builds the right policy, applies `apply_workspace_sandbox_overrides`, and acquires virtual display if the final policy asks for it. Mirrors the existing `spawn_pty_for_agent` virtual-display block so both paths behave identically.
  - `apply_preset` calls `update_terminal_session_persona(&session_id, preset.persona)` BEFORE `spawn_pty_for_session` for both the split-pane and new-tab branches.
  - `add_agent_terminal_to_workspace` (OpenFlow agent creation) seeds `persona: Agent`.
  - Built-in presets: Claude / Codex / OpenCode / Gemini / Pi → `Persona::Agent`; Shell → `Persona::Human`. Custom presets default `Human`. Persona of builtins is force-refreshed from the template on every `load_presets` call to defeat drift from old-schema saves.
  - `scripts.rs` no longer calls `sanitize_gui_env_std` — setup/teardown scripts and worktree-includes `git ls-files` are user-initiated and must inherit full env (the Run button was broken by the earlier overreach).
  - Frontend: `Persona` type + `persona` field on `TerminalPreset`; `create_preset` / `update_preset` Tauri commands accept optional persona; `PresetEditorSheet` exposes a Human/Agent selector (disabled for builtins with a hint about their fixed identity).
  - Tests: 13 unit tests in `presets.rs` (persona round-trip, sync_builtins refresh, legacy JSON deserialize), 9 new unit tests in `execution/mod.rs` (persona dispatch, three-state env override, token matrix), 13 new integration tests in `tests/persona_execution.rs` (end-to-end: spawn `env` as child, assert DISPLAY inherited for Human, stripped for Agent, neutralizers set, force-allow/force-deny overrides work). Two existing integration tests updated (`tests/execution_env.rs` and `tests/gui_leak_prevention.rs`) to assert the new Human-default. `sandbox_wiring_contract.rs` canary updated to check `worktree_session_default_for_persona` for the `virtual_display` field.
  - Cross-platform: the persona split is pure env-plumbing, no new Linux-only code paths. Frontend types are string-enum `"human"|"agent"` so the UI works identically on macOS / Windows / Linux.

## Open Questions

- ~~**Should regular worktree shells default to `allow_desktop_gui: false`?**~~ **Re-resolved April 2026 (second pass) with the persona split:** no, plain shells go back to `true` because users clicking "+" in their own terminal expect it to behave like any terminal emulator (kitty, alacritty, …). The lockdown moved to the preset layer: agent presets (Claude / Codex / OpenCode / Gemini / Pi) carry `persona: Agent` and their sessions spawn with `allow_desktop_gui: false`. Custom presets default to `Human` but the user can flip them to `Agent` in the preset editor. This is the final design — the "always strip" default broke `npm run tauri dev` for users who typed it themselves, which was the regression that motivated the second pass.
- **Per-workspace config shape.** `.codemux/config.json` `sandbox.allow_desktop_gui` boolean is simplest. Later add `sandbox.virtual_display: bool` for Phase 2.
- ~~**When to flip defaults.**~~ **Done April 2026.** `spawn_pty_for_session` default flipped via `worktree_session_default()` → `allow_desktop_gui: false`; opt-in via `CODEMUX_ALLOW_DESKTOP_GUI` or per-workspace config.
- **Reuse `anthropic-experimental/sandbox-runtime`?** Only if/when Phase 3 (real sandbox) becomes a priority. For display isolation, rolling our own is smaller.

## Edge Cases Handled

- **Bwrap missing on Linux** — falls through to `HostPassthrough` with env-strip populated. Still effective.
- **User's `~/.bashrc` exports `DISPLAY`** — the strip happens at spawn time; if `bashrc` re-exports it, the shell will see it but X socket at `/tmp/.X11-unix` is still accessible on the non-bwrap path. Acceptable for Phase 1 — most apps won't try to reconnect. Phase 2 (virtual display) makes this moot.
- **OpenFlow adapter sets `DISPLAY` in `extra_env`** — filter it out; `env_unset` applied last wins anyway.
- **Windows** — the GUI key list is a no-op (those vars don't exist in a standard Windows env). Infrastructure is in place. Real Windows GUI blocking comes in Phase 2 via WSL2 or Job Objects.
- **macOS Cocoa apps** — not affected by env-strip (they use native WindowServer connections). X11-under-XQuartz is affected. Full macOS coverage comes in Phase 2 via Lima.
- **Session restore** — restored sessions go through the same spawn helpers; env-strip applies on restore automatically.
- **Concurrent spawn race** — Phase 1 touches only the env-setting portion of the spawn, not the atomic reservation. Existing `try_reserve_session_spawn_*` tests still cover that path.

## Testing Strategy

### Unit tests (Phase 1)

Location: `src-tauri/src/execution/mod.rs` — new `#[cfg(test)]` module.

1. `prepare_agent_command` with `HostPassthrough` + `allow_desktop_gui=false` → `env_unset` contains all expected keys.
2. `prepare_agent_command` with `HostPassthrough` + `allow_desktop_gui=true` → `env_unset` is empty.
3. `prepare_agent_command` on Linux with bwrap available + `allow_desktop_gui=false` → returns bwrap backend, `env_unset` empty (bwrap handles via `--unsetenv`).
4. `prepare_agent_command` on Linux with bwrap missing → falls back to `HostPassthrough` with `env_unset` populated.
5. `build_linux_bwrap_args` with `allow_desktop_gui=false` → output contains `--unsetenv DISPLAY` etc. and `--tmpfs /tmp/.X11-unix`.
6. `ExecutionPolicy` serde round-trip.
7. `effective_backend()` on each simulated host OS.

### Integration tests (Phase 1)

Location: new file `src-tauri/tests/execution_env.rs`.

1. Set `DISPLAY=fake:99` in the test process env, run `prepare_agent_command` + spawn a small binary (`printenv DISPLAY` via `/usr/bin/env`) in a portable-pty, capture stdout, assert empty.
2. Same but `allow_desktop_gui=true` — assert `DISPLAY=fake:99` is present.
3. Platform-gate heavy tests with `#[cfg(target_os = "linux")]` / `#[cfg(target_os = "windows")]` as needed.

### Manual E2E (Phase 1)

Document in the PR:
1. Create OpenFlow workspace, open agent pane, run `echo $DISPLAY` → expect empty.
2. Create regular worktree workspace, open shell pane, run `echo $DISPLAY` → expect non-empty (current behavior preserved).
3. Toggle workspace `sandbox.allow_desktop_gui: false` (via editing `.codemux/config.json`), restart pane → expect empty.
4. In the non-GUI pane, attempt `xeyes` or `firefox` → expect "cannot open display" error rather than window opening.

### Regression

- Full `npm run verify` passes.
- Existing OpenFlow claude/opencode spawn tests still pass.
- `try_reserve_session_spawn_*` concurrency tests still pass — we don't touch the reservation path.
- Session restore still works — the new env-strip happens after restore-env is set, so any `DISPLAY` from restore is also stripped (intentional: restored OpenFlow agents should still have GUI hidden).

## Likely Touch Points

- `src-tauri/src/execution/mod.rs` — `env_unset` field, `gui_env_keys()` helper, populate in non-bwrap branches, unit tests
- `src-tauri/src/terminal/mod.rs` — wire `prepare_agent_command` into `spawn_pty_for_session`; fix `extra_env` override in `spawn_pty_for_agent`; apply `env_unset` after all env setting
- `src-tauri/tests/execution_env.rs` — **new** integration test file
- `src-tauri/src/config/workspace_config.rs` — **future PR** for the workspace `sandbox` section
- `src-tauri/src/openflow/adapters/claude.rs`, `opencode.rs` — already passing policy; Phase 2 will allow per-workspace override
- `docs/features/execution.md` — update when Phase 1 lands
- `src-tauri/src/execution/virtual_display.rs` — **Phase 2, new** (Xvfb/Xwfb lifecycle)

## Sources / Prior Art

- Anthropic Computer Use reference (Docker + Xvfb + VNC): https://github.com/anthropics/anthropic-quickstarts/tree/main/computer-use-demo
- Warp Computer Use (Xvfb inside their cloud sandbox): https://docs.warp.dev/agent-platform/warps-agent/capabilities-overview/computer-use
- Red Hat xwayland-run / wlheadless-run (2026 virtual-display recommendation): https://www.phoronix.com/news/xwayland-run
- Cursor 2026 sandboxing blog (shows they don't solve popups either): https://cursor.com/blog/agent-sandboxing
- Superset repo audit (no OS-level sandbox): https://github.com/superset-sh/superset
- portable-pty `CommandBuilder` env handling reviewed; `env_remove` works cross-platform on inherited env

## Notes

- Keep this file about active work. Behavior truth lives in `docs/features/execution.md`; update both in the same commit when Phase 1 lands.
- The "watch the agent" pane (Phase 4) is the wedge worth shipping eventually — no local ADE has this in April 2026. Prioritize once Phase 2 proves out.
- Do NOT rename `ExecutionPolicy` to `DisplayPolicy` or similar — the struct still makes sense for Phase 3's real sandboxing and we want to avoid churn.
