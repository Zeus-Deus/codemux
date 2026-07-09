# Browser Capability

- Purpose: Describe the current browser subsystem, what it can do, and what constraints still matter.
- Audience: Anyone working on browser features, automation, or browser-based validation.
- Authority: Canonical browser capability and constraints document.
- Update when: Browser behavior, user expectations, or known limitations change.
- Read next: `docs/plans/browser.md`, `docs/reference/BROWSER-AGENT-COMMANDS.md`, `AGENTS.md`

## Current Model

The browser pane uses a screenshot-driven Chromium session backed by `agent-browser` v0.24.0 (pure Rust, direct CDP). The visible pane and the explicit CLI browser commands share the same browser session and use the same internal execution helpers.

## What Works Today

- create a browser pane inside a workspace
- navigate with the address bar or `codemux browser open`
- keep browser panes inside split layouts next to terminals
- let agents use `snapshot`, `click`, `fill`, `screenshot`, and `console-logs`
- show user-visible browser updates through repeated screenshot refreshes
- stealth Chromium flags to reduce bot detection fingerprinting
- realistic user-agent string derived from installed Chrome/Chromium version
- per-workspace browser sessions with reconnection on pane recreation
- dynamic stream ports (9223-9299) for concurrent workspace browsers
- browser data management in Settings (clear cookies, clear all data, view data size)
- inspector panel for debugging web content
- viewport presets for mobile / tablet / desktop responsive testing via `codemux browser viewport <preset>` (e.g. `mobile`, `tablet`, `desktop-large`, `reset`) or custom `WxH` dimensions — applies real viewport resize + DPR through CDP, so CSS media queries fire and screenshots capture at the simulated dimensions (replaces the older iframe trick)
- native-feel manual interaction in the pane: drag-to-select text (moves carry the held CDP button; pointer capture finishes drags released outside the pane), hover effects (`:hover` styling — mousemoves are forwarded with a coalescing throttle: ~60/s during drags, ~30/s hover, 10/s in inspector mode), double/triple-click word/paragraph selection (chained `clickCount`), right/middle-click forwarding (web-app context menus work; the host context menu is suppressed), and a live cursor that mirrors the remote page (pointer over links, I-beam over text, resize cursors) via a throttled `elementFromPoint` probe
- host-clipboard bridge: the headless browser's clipboard is invisible to the OS, so Ctrl/Cmd+C and +X mirror the page selection (including input/textarea selections) onto the host clipboard while still performing the in-page copy/cut, and Ctrl/Cmd+V inserts host-clipboard text into the focused editable via chunked `execCommand("insertText")` evals (falls back to forwarding the raw keystroke when focus isn't editable, so in-page paste handlers still run)
- calm reconnect UX: a quiet screencast is not treated as a dead stream (CDP only emits frames on visual change) — when no frame has arrived for 15s the pane probes the daemon's HTTP `/api/status` endpoint and only reconnects when the daemon is actually unresponsive; during a real reconnect the last frame stays visible with a small corner pill instead of a full-screen spinner, and after the fast retry budget (15 × 1.5s) the pane drops to a slow 10s retry loop instead of giving up, so it self-heals whenever the daemon comes back
- stream stability across concurrent worktrees: daemons are PID-tracked (not port-tracked), the workspace-id map is the single canonical key, `close()` is atomic (remove entry → kill PID → await exit → verify port free), `allocate_port()` runs a symmetric `TcpListener::bind` probe on every OS, and `BrowserPane.tsx` reactively reconnects when `stream_url` changes. Replaces the older flow where the manager could lose track of which port belonged to which workspace, leak daemons, and leave panes stuck at the right URL with no frames. See `docs/archive/browser-stream-fix.md` for the full landed plan.
- workspace-correct routing for env-less callers: `browser_automation` requests carry both the caller's `CODEMUX_WORKSPACE_ID` (injected into terminal PTYs and, since the agent-chat fix, into Claude/Codex chat sidecars) and a best-effort `cwd` hint. When `workspace_id` is empty, `resolve_workspace_id_by_cwd` in `src-tauri/src/control.rs` resolves the owning workspace lexically from the cwd (exact or component-safe subdirectory match against each workspace's `cwd`/`worktree_path`, longest root wins for nested worktrees) before falling back to the legacy active-workspace path — so agent-chat sessions (and OpenCode's shared server, which can't take per-session env) open their browser pane in the agent's own workspace, not whichever one the user is viewing
- hang-proof external calls (issue #96): every `agent-browser` CLI / process-control invocation runs through hard-timeout wrappers (`output_capture_with_timeout` / `launch_status_with_timeout`) that kill the child — and, on Unix, its whole process group — on overrun, so a wedged daemon can never block `close()` / `start_stream()` / automation forever. The async callers also push the blocking work onto `tokio::task::spawn_blocking` so a slow CLI can't stall the runtime's worker threads. Default ceilings: ~5s process control, ~10s `close`, ~30s DOM actions, ~60s `open`/`wait`. Replaces the unbounded `std::process::Command::output()` calls that hung `cargo test` to the 30-minute job timeout on the Windows CI image.

## Background browser in GUI mode

When the Agent Chat GUI Beta (`enable_agent_chat`) is on for a real,
non-OpenFlow workspace (the same gate as `docs/features/gui-chrome.md`'s
`useGuiChrome`), an agent-opened browser (`codemux browser open` /
`browser_automation`) no longer splits the chat pane into a browser pane.
The daemon-level session, streaming, and automation handling are
**unchanged** — this is purely a rendering decision. The `AgentBrowserSession`
that would normally get `attach_agent_browser_to_pane`d instead stays
detached (`pane_id: None`), same as it already does after the user manually
closes a browser pane; the only difference is the session is marked
`is_active` immediately (`AppStateStore::mark_agent_browser_active`) instead
of waiting for a pane attach that will never come, and `emit_app_state`
fires right away so the frontend picks it up without delay.

**Backend gate** (`src-tauri/src/control.rs`, `browser_automation` handler):
the existing auto-pane-creation decision (previously inline: `let
should_create = agent_session.pane_id.is_none() && !agent_session
.user_dismissed;`) is now behind a small pure function,
`should_create_browser_pane(pane_attached, user_dismissed,
gui_background_mode)`, unit-tested directly. `gui_background_mode` is
`observability.agent_chat_enabled() && workspace_type !=
Some(WorkspaceType::OpenFlow)`, applied on the workspace-scoped path only.
Flag off, or an OpenFlow workspace, produces byte-identical behavior to
before this feature — a pane is always created.

The **legacy no-`workspace_id` fallback** (empty `workspace_id` and no cwd
resolution) deliberately ignores the GUI flag and always creates the pane
(`should_create_legacy_browser_pane`, also unit-tested): that path has no
resolved workspace, so no `AgentBrowserSession` exists to mark active — the
chip / indicator / peek could never surface the browser, and suppressing the
pane would leave it running completely invisibly. A visible pane is the only
safe behavior there.

**Close wiring**: when a `browser_automation` `close` action completes
successfully on the workspace-scoped path, the handler calls
`AppStateStore::mark_agent_browser_inactive` (mirror of
`mark_agent_browser_active` — flips `is_active` to false, keeps the session's
URL / `cli_session_name` for a later reopen) and emits app state, so the
chip, indicator, and peek stop presenting the session as live the moment the
browser closes. Daemon-death detection (a browser dying without a `close`)
is out of scope — only the explicit close path is wired.

**Run-finished release**: agents rarely call `browser close` themselves, so
the explicit-close path above almost never fires in practice and the chip
would otherwise show `LIVE · agent is navigating…` forever after the turn
ends. `agent_chat.rs`'s `publish_pane_status` (the same function that drives
the sidebar status dot) now also releases the background session: once the
per-thread `SubagentTracker` settles a turn to `PaneStatus::Review` or
`PaneStatus::Idle` — turn complete with no subagents still in flight, or the
session closing/erroring into `Idle` — it resolves the thread's pane
(`agent_chat_pane_id_for_thread`) and workspace (`workspace_id_for_pane`)
and calls `AppStateStore::release_detached_agent_browser`. That method only
flips `is_active` (and only returns `true`) for a session that is currently
active *and* pane-less (`pane_id.is_none()`); a session already attached to
a legacy split-pane is left alone since that flow has its own close
lifecycle. `PaneStatus::Working`/`Permission` never release — the tracker
already defers `Review` while a subagent is still running, so by the time a
terminal status reaches this check the run really is over. As with the
explicit-close path, the session's URL and `cli_session_name` are kept, so
the agent's next browser action simply re-marks it active and the chip
returns. Daemon-death detection remains out of scope either way.

Known accepted edge case: if two chat threads are both driving browser
activity in the *same* workspace, one thread finishing its run releases the
chip even while the other thread is still mid-run. This self-heals — the
still-running thread's next browser action immediately re-marks the shared
session active.

**Frontend surfaces**, all gated on the shared `useGuiChrome()` hook
(`src/hooks/use-gui-chrome.ts`, extracted from `title-bar.tsx`'s inline
predicate — see `docs/features/gui-chrome.md`) or the equivalent per-workspace
flag+type check where a specific (possibly non-active) workspace id is
already in hand:

- **Inline conversation chip** (`src/components/chat/BackgroundBrowserChip.tsx`)
  — `MessageList` cross-references `agent_browser_sessions` for the pane's
  workspace (a session that `is_active` and has no `pane_id`) and appends
  the chip as a derived row after the transcript (not a reducer
  `ChatViewItem`). Globe icon chip with a blinking amber dot, "Browser
  opened in background" + `LIVE` badge while active, the current URL
  (`agent_browser_sessions[].current_url`, already tracked — no new field
  needed), and a "View" affordance. Click opens the peek overlay.
- **Context-bar indicator** (`src/components/layout/workspace-context-bar.tsx`)
  — a sky-tinted pill with a blinking amber dot in the right-aligned
  cluster, shown while the active workspace has a live background session.
  The bar's "nothing to report" early-return now also checks this, so the
  bar renders for the indicator alone even with no git/PR/issue. See
  `docs/features/workspace-context-bar.md`.
- **Peek overlay** (`src/components/browser/BrowserPeekOverlay.tsx`) — a
  440×300 floating panel, absolutely positioned top-right inside the
  already-`position: relative` `SidebarInset` (mounted once in
  `app-shell.tsx`) so it overlays the chat surface without resizing it.
  Escape, click-outside, and switching the active workspace all close it —
  the peek is a transient "look at this now" affordance, so returning to a
  workspace never pops it open unprompted. Header: green status dot, mono
  URL readout, "Open as pane" (promote) and close buttons. Body: a live
  `BrowserPane`. Peek open/closed state is a single `openWorkspaceId` in a
  small zustand store, `src/stores/browser-peek-store.ts` (at most one peek
  can be open app-wide).
- **Promote to pane** — "Open as pane" calls the same `create_browser_pane`
  Tauri command the `+` launcher's Panes → Browser item uses
  (`agent-launcher.tsx`); `create_browser_pane_impl`
  (`src-tauri/src/commands/browser.rs`) already finds and reconnects a
  workspace's detached agent session, so no new backend command was
  needed. The peek closes on promote; the chip/indicator stop showing
  `LIVE`/the blink once the session is pane-attached (no longer
  "background") since the chip/indicator lookups filter on `pane_id ===
  null`.

**`BrowserPane` plumbing note**: a detached/background session has no
`browser_id` (that's only assigned once a pane exists), but `BrowserPane`
resolved its `agentSession` strictly by `browser_id`. It now accepts an
optional `workspaceId` prop; when set, the `agent_browser_sessions` lookup
falls back to matching on `workspace_id` when the `browser_id` match misses.
The peek overlay passes the session's own `cli_session_name` as `browserId`
(so the stream daemon starts against the right session — the pane-attached
path's existing `agent_session_name` fallback wasn't populated in the
detached case) plus `workspaceId`. A new `hideToolbar` prop suppresses
`BrowserPane`'s embedded address-bar toolbar for the peek, which renders its
own compact header instead. Both props are additive and default to today's
pane behavior (`workspaceId` unset falls through to the exact prior
`browser_id`-only lookup; `hideToolbar` defaults to showing the toolbar).

**Desktop-size peek viewport (opt-in)**: by default, `BrowserPane` syncs the
browser's real CDP viewport to its container — inside the 440×300 peek that
means pages reflow at popover dimensions and agents end up re-sending
`viewport` every turn. Settings → Agent → "Desktop-size background browser"
(`UserSettings.agent_chat.background_browser_desktop_viewport`, synced
settings, default OFF) instead pins the peek's viewport to 1280×800 —
matching the `desktop` preset / `RESET_SPEC` in
`src-tauri/src/browser_viewport.rs` — via a new optional `BrowserPane`
`fixedViewport` prop. When set, the WebSocket-open handshake sends the fixed
size instead of container dims, and the `ResizeObserver` keeps syncing the
canvas element to the container but skips the viewport re-send, so the
existing frame-draw letterbox scales the full-size frame down to fit. Input
mapping needs no changes — `mapToViewport` already routes clicks through the
canvas rect + draw rect into viewport coordinates. Applies only to the peek
overlay; normal browser panes (and the peek with the setting off) keep the
container-sync behavior unchanged.

## Expected Operating Model

- agents control the browser programmatically
- users see the browser pane as live evidence of that work
- this is usable now, but it is not the final browser architecture yet

## Current Internal Boundary

- canonical path: `agent_browser` commands plus `AgentBrowserManager` in `src-tauri/src/agent_browser.rs`
- CLI browser commands delegate to the same `agent-browser` execution path
- the legacy Playwright/Node.js path and the unused `BrowserManager` Rust CDP implementation have been removed (v0.24.0 migration)

## Current Constraints

- not a native embedded Tauri webview
- the pane is screenshot-driven, so some fidelity gaps remain: no native context menu, no in-pane file dialogs, cursor updates are probe-driven (~8/s) rather than instant
- every eval POSTed to the stream daemon's `/api/command` must keep the whole HTTP request inside one TCP segment (~1.4KB) — the daemon reads requests with a single peek and sees an empty body otherwise. Measured: bodies ≤ ~870 bytes always survive, ≥ ~970 always fail. This is why the pane's page scripts are minified and paste text is chunked at 120 chars (`PASTE_CHUNK_SIZE` in `src/components/browser/stream-protocol.ts`)
- toolbar behavior, back and forward, reload, and arbitrary text entry still need focused validation
- browser console capture is not yet a full live log stream from the displayed pane

## Important Touch Points

- `src-tauri/src/agent_browser.rs` — `AgentBrowserManager`, stealth flags, stream port allocation (PID-tracked daemons keyed by `workspace_id`, symmetric bind probe on every OS, atomic teardown), viewport argv builder (`resolve_viewport_params`, `format_dpr`), and the external-process timeout layer (`output_capture_with_timeout`, `launch_status_with_timeout`, `wait_with_timeout`, `kill_timed_out_child`, `action_timeout`) that bounds every CLI/kill call (issue #96)
- `src-tauri/src/browser_viewport.rs` — preset table (`PRESETS`, `RESET_SPEC`) and `parse_spec` for preset / `WxH` / `reset` parsing
- `src-tauri/src/commands/browser.rs` — Tauri commands for pane creation, URL navigation, automation
- `src-tauri/src/control.rs` — `browser_automation` control-socket handler: workspace-scoped routing by `workspace_id`, the `resolve_workspace_id_by_cwd` fallback for env-less callers, and the legacy active-workspace path when neither resolves
- `src-tauri/src/cli.rs` — `BrowserCommand::Viewport` and `BrowserCommand::ViewportPresets` CLI subcommands
- `src-tauri/src/mcp_server.rs` — `browser_viewport` and `browser_viewport_presets` MCP tools
- `src/components/browser/BrowserPane.tsx` — screenshot rendering, toolbar, address bar, reactive `stream_url` reconnect on URL change, auto-syncs viewport on pane resize via legacy `width`/`height` payload (which the new socket handler accepts unchanged); pointer-capture input forwarding (drag selection, hover, click-count chaining, right/middle click), cursor probe, host-clipboard bridge, probe-based liveness + corner reconnect pill
- `src/components/browser/stream-protocol.ts` — pure helpers for the stream-input protocol (button mapping, click-count chaining, coordinate mapping, cursor sanitization) and the daemon HTTP endpoints (`/api/status` liveness probe, `/api/command` evals; page scripts minified for the single-segment HTTP constraint). Unit-tested in `stream-protocol.test.ts`
- `src/components/browser/InspectorPanel.tsx` — browser inspector/DevTools panel
- `src/components/browser/BrowserPeekOverlay.tsx` — GUI-mode background-browser floating peek (see "Background browser in GUI mode" above)
- `src/components/chat/BackgroundBrowserChip.tsx` — the inline conversation chip for a background session
- `src/hooks/use-gui-chrome.ts` — the shared `useGuiChrome()` gate the chip/indicator/peek key off
- `src/stores/browser-peek-store.ts` — peek open/closed state (single `openWorkspaceId`; cleared on workspace switch)
- `src-tauri/src/state/state_impl.rs` — `AgentBrowserSession` (`is_active`, `pane_id`, `current_url`), `mark_agent_browser_active` / `mark_agent_browser_inactive` / `release_detached_agent_browser`, `attach_agent_browser_to_pane`, `detach_agent_browser_from_pane`, `find_detached_agent_browser`
- `src-tauri/src/commands/agent_chat.rs` — `publish_pane_status` calls `release_detached_agent_browser` once a run settles to `Review`/`Idle` (see "Run-finished release" above)
- `docs/reference/BROWSER-AGENT-COMMANDS.md` — CLI and socket command reference
- `docs/archive/browser-stream-fix.md` — landed cross-platform stream-stability plan (PID tracking, single canonical key, atomic teardown, symmetric bind probe, reactive frontend reconnect)
