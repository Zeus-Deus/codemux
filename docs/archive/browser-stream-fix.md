# Browser Stream Fix Plan (archived)

- Status: **LANDED — archived.** The unified port keying + PID-tracked daemon lifecycle landed in commit `7e36420` ("fix(browser): unify port keying and harden daemon lifecycle"). Dead `workspace_id` alias lookups removed in commit `fba8697`. Current browser behaviour lives in `docs/features/browser.md`.
- Purpose: Historical record of the cross-platform stream-stability work — PID tracking, single canonical key, atomic teardown, symmetric `TcpListener::bind` probe, reactive `stream_url` reconnect on the frontend.
- Audience: Anyone debugging browser stream regressions or revisiting the reaper / PID-file / startup-adoption ideas (steps 5, 7, 8 from the original plan).
- Read next: `docs/features/browser.md`, `docs/plans/browser.md`, `docs/reference/BROWSER-AGENT-COMMANDS.md`, `AGENTS.md`.

## Goal

Eliminate the silent stream failure that appears after multiple concurrent worktrees use the browser. The pane currently shows the correct URL but no frames because the manager loses track of which port belongs to which workspace, leaks daemons, and the React pane never reconnects when the URL changes. The fix unifies the workspace ↔ port ↔ daemon mapping, tracks daemons by PID instead of port, and makes the frontend reactive to URL changes. Cross-platform, no behavior changes for callers, and no new dependencies.

## Scope

- In scope: `AgentBrowserManager` allocator and lifecycle, `control.rs` routing, `state_impl.rs` session record, `BrowserPane.tsx` connection logic, log/PID file layout under `~/.codemux/run/`.
- Out of scope: switching off the screenshot stream model, replacing `agent-browser`, redesigning the inspector panel, any change to the public CLI surface (`codemux browser ...` commands stay byte-identical).

## Cross-Platform Posture

- Most defects are OS-independent. Two OS-specific touch points to keep symmetric:
  - **Unix**: add the `TcpListener::bind("127.0.0.1", port)` probe inside `allocate_port()` (today it only runs under `#[cfg(windows)]`).
  - **Windows**: replace the no-op `kill_session_tree` path with `taskkill /PID {pid} /T /F` driven by the tracked PID, so ghost-PID port retention stops happening.
- Every step below ships in one cross-platform diff with `#[cfg(...)]` only where the syscall actually differs.

## Active Priorities (ordered, each independently shippable)

1. **Track daemons by PID, not by port.** Capture the spawned `Child`, store `pid` on `StreamSession`, and replace every `kill_process_on_port` call site with `kill(pid)` (Unix) / `taskkill /PID {pid} /T /F` (Windows). Removes the `fuser -k` collateral damage where one workspace's startup wipes another workspace's healthy daemon.
2. **Single canonical key.** Pick `workspace_id` as the only map key. Delete the `ensure_port(cli_session_name, ...)` alias hack in `control.rs`; `cli_session_name` becomes a value on the session, not a second key. Resolves the worktree-collision bug where worktrees sharing a parent cwd alias to the same `cli_session_name` slot.
3. **Atomic teardown in `close()`.** Remove the single `workspace_id` entry, kill the tracked PID, await exit with a short timeout, then verify the port is free. With aliasing gone there is no second entry to leak.
4. **Symmetric bind-test in `allocate_port()`.** Lift the Windows `TcpListener::bind` probe out of its `cfg` and run it on every OS. Cheap, eliminates ghost-port reuse on Linux/macOS.
5. **Real health check, not a 1-second sleep.** After spawn, open a probe WebSocket (or hit CDP `Target.getTargets`) and only set `running = true` once a real signal returns. Track `last_frame_at` and let a background reaper close sessions whose stream has been silent past a configurable threshold.
6. **Reactive stream URL on the frontend.** Stop relying on the one-shot `startBrowserStream(streamSessionId)` return value inside `BrowserPane.tsx`. Read `agentSession?.stream_url` from the store, add it to the `useEffect` dependency array, and force a clean WebSocket reconnect when the URL changes. Update `state_impl.rs` so re-allocation rewrites `stream_url` instead of leaving the original baked-in copy.
7. **Persistent logs and PID files.** Redirect daemon stderr to `~/.codemux/run/agent-browser-{workspace_id}.log` and write `~/.codemux/run/agent-browser-{workspace_id}.pid` next to it. Lets the next investigation take five minutes instead of an hour.
8. **Safe adoption on app start.** Replace the blanket `pkill -f 'agent-browser.*--session'` with a startup pass that reads each PID file, probes the port, adopts healthy daemons, reaps dead ones. Stops the current behavior of nuking everything blindly on launch.

## Migration / Compatibility Notes

- The CLI surface and Tauri command names do not change. Callers see no breaking change.
- One legacy entrypoint to watch: `commands/browser.rs` paths that pass an empty `browser_id` and fall back to `"default"`. Keep that fallback intact during steps 1–4 so existing automation does not regress; remove it (if at all) only after step 6 lands.
- Step 8 must run *after* steps 1–7 ship, otherwise old PID files have no schema to read.

## Verification

- `cargo check --manifest-path src-tauri/Cargo.toml`
- `cargo test --manifest-path src-tauri/Cargo.toml` — add a unit test that allocates 80 ports, closes them, and verifies the next allocator call still works (catches the leak regression).
- `npm run check`
- `npm run test`
- `npm run verify` for the final pass.
- Manual smoke: spin up three worktrees, run `codemux browser open` in each, watch all three panes render frames simultaneously, then close two and confirm the third keeps streaming.
- Manual smoke (Windows): same flow under Windows runner; confirm `taskkill` path only takes the tracked PID, not the whole agent-browser tree on the box.

## Open Questions

- Where should `~/.codemux/run/` live on Windows? `%LOCALAPPDATA%\codemux\run\` is the obvious answer; confirm it matches the existing data-dir convention before step 7.
- Reaper threshold for step 5: 30 s of stream silence is a safe default, but should it be configurable in `Settings > Browser`? Decide before step 5 lands.
- Do we want a one-time migration on first launch after this ships that kills any pre-fix orphaned `agent-browser` processes the user already has running, or do we just let users restart their worktrees? Lean toward the latter — safer.

## Likely Touch Points

- `src-tauri/src/agent_browser.rs`
- `src-tauri/src/commands/browser.rs`
- `src-tauri/src/control.rs`
- `src-tauri/src/state/state_impl.rs`
- `src-tauri/src/lib.rs` (manager construction / cleanup hook)
- `src/components/browser/BrowserPane.tsx`
- `src/stores/app-store.ts` (if `stream_url` becomes reactive)
- `docs/features/browser.md` (update once the manager contract changes)
- `docs/plans/browser.md` (cross-link this plan from the Active Priorities list)

## Already Landed

- Per-workspace stream port allocation across 9223–9299 (commit `82afda6`).
- Stale-daemon kill on `start_stream` and app-exit cleanup (commit `66b5a9d`).
- WebSocket auto-reconnect retry loop in `BrowserPane.tsx` (commit `d5eeeec`).

## Notes

- Keep this file scoped to the fix work. Move sections to `docs/archive/` once each priority lands and no longer drives next steps.
- Once steps 1–6 ship, update `docs/features/browser.md` to describe the new contract (one key, PID-tracked daemons, reactive frontend URL) and shrink this plan to whatever remains.
