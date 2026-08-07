# Browser Capability

- Purpose: Describe the current browser subsystem, what it can do, and what constraints still matter.
- Audience: Anyone working on browser features, automation, or browser-based validation.
- Authority: Canonical browser capability and constraints document.
- Update when: Browser behavior, user expectations, or known limitations change.
- Read next: `docs/plans/browser.md`, `docs/reference/BROWSER-AGENT-COMMANDS.md`, `AGENTS.md`

## Current Model

The browser pane uses a screenshot-driven Chromium session backed by `agent-browser` v0.24.0 (pure Rust, direct CDP). The visible pane and the explicit CLI browser commands share the same browser session and use the same internal execution helpers.

## What Works Today

- open the browser inside the right panel as a real closable deck pane (the same session the agent drives — see "The browser in the right-panel deck")
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

## The browser in the right-panel deck

The browser's home is the **right-panel deck's `browser` pane** — not a
split in the workspace pane tree. Opening it from the panel's `+` menu
does not create a browser; it **docks** the workspace's existing
`AgentBrowserSession` into the panel.

**Why this is safe.** `BrowserPane` is position-independent: a `<canvas>`
fed by a WebSocket screencast, not a native webview welded to a slot in
the pane tree. Nothing about the daemon, the port, the cookies or the
automation path depends on where the component is mounted.
`BrowserPeekOverlay` already mounted one outside the pane tree; the deck
is the same trick with a permanent home.

**One session, one surface.** The backend's notion of "the browser" was
never the pane — it is the per-workspace `AgentBrowserSession`, whose
`cli_session_name` (derived from the workspace cwd) is the key
`allocate_port` and every `codemux browser` / MCP call resolves to. The
deck pane mounts `BrowserPane` against that name, so the browser the user
drives and the Chromium the agent drives are the same daemon on the same
port showing the same page.

`AgentBrowserSession` therefore carries **two** hosts, and
`is_surfaced()` is the union:

| Field | Meaning |
| --- | --- |
| `pane_id: Option<PaneId>` | attached to a workspace pane-tree node |
| `right_panel_docked: bool` | hosted by the right-panel deck |

They are mutually exclusive by construction — `dock_agent_browser_in_right_panel`
clears `pane_id`, `attach_agent_browser_to_pane` clears
`right_panel_docked` — so the same Chromium can never be mirrored by two
surfaces. Everything that used to ask "is a pane attached?" now asks
`is_surfaced()`:

- `should_create_browser_pane` in `control.rs` — a docked browser
  suppresses auto-creating a main-area pane, so an agent's next
  `browser open` drives the panel instead of splitting the workspace.
- `release_detached_agent_browser` — a docked session is one the user is
  looking at, so the end of an agent run no longer marks it inactive.
- the background chip / terminal-header indicator / peek overlay, which
  all resolve through the single `selectBackgroundBrowserSession`
  predicate in `background-browser-indicator.tsx`.

**Commands.** `dock_browser_in_right_panel(workspace_id)` mirrors the
`browser_automation` handler's sequence — resolve-or-create the session,
allocate its port under `cli_session_name`, write the port back, mark it
docked — and returns the session so the frontend gets `cli_session_name`
and `stream_url` without re-deriving either. It **adopts** rather than
duplicates: if the session is currently attached to a pane-tree node,
that node is closed as part of docking. `undock_browser_from_right_panel(workspace_id, dismissed)`
mirrors closing a browser pane — the daemon keeps running, and
`dismissed: true` (the tab's ×) stops the agent re-surfacing it on its
next command. Collapsing the whole panel undocks with `dismissed: false`:
a collapsed panel is not a surface, so leaving the session docked would
hide the browser from both the agent's pane gate and the background chip.
The tab stays in the deck, so re-opening the panel re-docks it.

That rule lives in **one place**: `collapseRightPanel(workspaceId)` on the
UI store. The titlebar cluster, the panel's own close button, the
`toggleRightPanel` keybind (Ctrl+Shift+B) and the legacy tab-bar toggle all
route through it, and `setRightPanelTab(ws, null)` delegates to it, so a new
collapse path cannot bypass the undock. Opening is symmetrical everywhere:
the panel comes back on `RIGHT_PANEL_EMPTY`, the picker sentinel, rather
than force-opening Files over a pane the user had closed.

Docking is a round trip (it allocates a port), so the deck re-checks itself
when the promise resolves: if the tab was closed or the panel collapsed
while it was in flight, the user's own undock no-opped on the not-yet-docked
session, and the reply would otherwise mark it surfaced with nothing hosting
it. The deck undocks again in that case, carrying the intent that raced it
(`dismissed: true` for a closed tab, `false` for a collapsed panel).

**Entry points, and what each one does now:**

| Entry point | Behavior |
| --- | --- |
| Right panel `+` ▸ Browser | Docks the workspace session into the deck. If a main-area browser pane already holds that session, it is adopted (closed) rather than mirrored. |
| Peek overlay ▸ "Open in side panel" | Docks the same session into the deck. Promote no longer splits the main area — the deck is the browser's one persistent home, so the peek stays a transient look that graduates into it. |
| Agent `codemux browser …` with the pane docked | Drives the docked browser. No second pane is created (`is_surfaced()`). |
| Agent `codemux browser …` with nothing surfaced | Unchanged: a main-area pane in legacy mode, a detached background session under the Agent Chat GUI. |
| Main tab strip `+` ▸ Browser, ports popover, command palette | Unchanged — still `createBrowserPane`, still a main-area pane. If that re-attaches the docked session, the deck yields its tab rather than mirroring it. |

**Pane chrome.** The deck has one 36px row of chrome shared with the tabs,
so the browser contributes only back / forward / reload as 24px icon
buttons in its action slot, routed through `runBrowserNav` in
`src/components/browser/browser-nav.ts` — the same helper (and the same
agent-browser command channel) the main-area pane's `BrowserToolbar` uses,
so a click in either place is indistinguishable to the daemon.
`BrowserPane` is mounted with `hideToolbar`, because that slot is the
toolbar here. **The address is not in the row** — a URL cannot sit honestly
in 36px it shares with the tab strip, so it renders in the deck's status
foot instead (scheme stripped, `· agent session` appended while the agent
is driving). There is no URL entry field either; typing an address is still
the main-area pane's affordance, and in-page navigation, the agent, and
back/forward all work normally in the panel.

## Background browser in GUI mode

When the Agent Chat GUI (`enable_agent_chat`) is on for a real workspace
(the same gate as `docs/features/gui-chrome.md`'s
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
`observability.agent_chat_enabled()` on the workspace-scoped path. With the
flag off, a pane is created as before.

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

Because the release is gated on that settled status, anything that stopped
the turn from settling also stopped the chip from clearing. Two changes
close that off:

- **A background shell task can no longer block the release.** A
  `Bash { run_in_background: true }` used to be tracked as a subagent, and
  a dev server never exits — so the tracker deferred `Review` forever and
  the chip sat on `LIVE · agent is navigating…` indefinitely. Those rows
  now carry `SubagentSnapshot.background_task` and are excluded from the
  running-set, so the turn settles and the release fires normally. See
  `docs/features/agent-chat.md` (§ Sidebar status indicators).
- **A watchdog backstop that stops short of this release.** If a `Review`
  is still owed 10 minutes after the turn completed with no real run
  activity, the stall-watchdog sweep force-settles the *pane status*
  through `apply_pane_status`. It passes `SettleOrigin::ForcedBackstop`,
  which deliberately withholds the browser release: the trigger is
  silence, and a subagent sitting inside one long quiet tool call is
  silent too. Publishing a settled dot early is repainted by the next
  real event; closing a browser session the agent is still driving is
  not. So the sidebar unsticks on the next sweep, while the chip clears
  only on a real terminal transition (or when the pane/session closes).
  See `docs/features/agent-chat.md` (§ Sidebar status indicators) for the
  full forced-settle semantics.

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
- **Terminal-header indicator**
  (`src/components/browser/background-browser-indicator.tsx`, mounted by
  `PaneNode.tsx`) — the active terminal shows a compact sky-tinted Browser
  control with a blinking amber dot while its workspace has a live detached
  session. Click opens the same peek overlay. This replaced the full-width
  workspace context bar, so browser activity stays discoverable without
  reserving 42px below every terminal. See
  `docs/features/workspace-context-bar.md`.
- **Peek overlay** (`src/components/browser/BrowserPeekOverlay.tsx`) — a
  440×300 floating panel, absolutely positioned top-right inside the
  already-`position: relative` `SidebarInset` (mounted once in
  `app-shell.tsx`) so it overlays the chat surface without resizing it.
  Escape, click-outside, and switching the active workspace all close it —
  the peek is a transient "look at this now" affordance, so returning to a
  workspace never pops it open unprompted. Header: green status dot, mono
  URL readout, "Open in side panel" (promote) and close buttons. Body: a live
  `BrowserPane`. Peek open/closed state is a single `openWorkspaceId` in a
  small zustand store, `src/stores/browser-peek-store.ts` (at most one peek
  can be open app-wide).
- **Promote to the side panel** — "Open in side panel" calls
  `dock_browser_in_right_panel` and activates the deck's `browser` tab,
  the same thing the right panel's `+` ▸ Browser does. It used to call
  `create_browser_pane` and split the chat in half; the deck is now the
  browser's one persistent home, so the peek graduates into it rather
  than creating a second place a browser can permanently live. The peek
  closes on promote, and the chip/indicator stop offering to reveal the
  session because their shared predicate
  (`selectBackgroundBrowserSession`) filters on `pane_id === null && !right_panel_docked`
  — a browser the user is already looking at is not a background one.

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

**Desktop-size peek viewport (default ON)**: with the setting off,
`BrowserPane` syncs the browser's real CDP viewport to its container — inside
the 440×300 peek that means pages reflow at popover dimensions and agents end
up re-sending `viewport` every turn. Settings → Agent → "Desktop-size
background browser" (`UserSettings.agent_chat.background_browser_desktop_viewport`,
synced settings, default ON since the field flipped post-`v0.13.2`; an
explicitly saved `false` is preserved) pins the peek's viewport to 1280×800 —
matching the `desktop` preset / `RESET_SPEC` in
`src-tauri/src/browser_viewport.rs` — via a new optional `BrowserPane`
`fixedViewport` prop. When set, the WebSocket-open handshake sends the fixed
size instead of container dims, and the `ResizeObserver` keeps syncing the
canvas element to the container but skips the viewport re-send, so the
existing frame-draw letterbox scales the full-size frame down to fit. Input
mapping needs no changes — `mapToViewport` already routes clicks through the
canvas rect + draw rect into viewport coordinates. Applies only to the peek
overlay; normal browser panes (and the peek with the setting toggled off) keep
the container-sync behavior unchanged. When a user default viewport is
configured (next section), the peek pins to that instead of 1280×800 —
`DESKTOP_PEEK_VIEWPORT` is only the fallback.

## User default viewport (`browser.default_viewport`)

Settings → Browser → "Default viewport" (`UserSettings.browser
.default_viewport`, synced settings, default unset). Motivation: the built-in
1280×800 baseline reads as "zoomed in" to users on larger monitors — agent
screenshots for PRs/issues show the narrow-desktop responsive layout instead
of what the user sees in their own browser. Setting e.g. QHD (2560×1440) makes
agent screenshots match the user's screen proportions. Cosmetic cost only:
frames/screenshots scale ~linearly with pixel count, and model-side vision
downscales past ~1.15 MP anyway, so agent token cost is unchanged.

The value is a `browser_viewport::parse_spec` string — preset name or `WxH`.
The **Settings panel only offers four canned `WxH` sizes** (Default 1280×800,
1920×1080, 2560×1440, 3840×2160); preset names and other dimensions can only
arrive from another device or a hand-edited value, and such a value is
surfaced back as an extra option in the select. Note that preset names also
don't parse in the peek overlay (`parseViewportString` is `WxH`-only), so a
preset-name value is best avoided in practice.
Consumption points (all lenient — an invalid synced value degrades to the
1280×800 baseline, never errors):

- **Fresh daemon launch** (`AgentBrowserManager::run_command`): when the
  session's `running` flag flips false→true, the configured viewport is
  applied via a `set viewport` command BEFORE the requested action, so even a
  first-command screenshot renders at the user's size. Fires once per daemon
  lifetime and only when the setting is present; skipped for `viewport`
  (explicit size wins) and `close` (don't boot a daemon to resize it). An
  agent's own later `viewport` calls are never stomped. A `close` action
  resets the session's `running`/`pid` state (the agent-facing `close`
  routes through `run_command`, not `AgentBrowserManager::close`, so the
  session-map entry outlives the daemon) — this keeps the once-per-daemon
  guarantee honest across an agent's open → close → open sequence: the
  reopened daemon gets the default re-applied.
- **`viewport reset`** (CLI `codemux browser viewport reset` + MCP
  `browser_viewport {preset: "reset"}`): resolves through
  `browser_viewport::parse_spec_configured`, which lands on the configured
  default (`configured_default_spec`) instead of the hard-coded `RESET_SPEC`.
  Both `viewport-presets` listings report the actual reset target. The CLI is
  the same binary as the app, so it reads the same on-disk settings cache.
- **Peek overlay pin** (`BrowserPeekOverlay.tsx`): with "Desktop-size
  background browser" ON, the `fixedViewport` prop uses
  `parseViewportString(browser.default_viewport)` (WxH strings only on the
  frontend) with `DESKTOP_PEEK_VIEWPORT` (1280×800) as fallback, keeping the
  peek consistent with what the agent's screenshots show.

Visible browser panes still sync the viewport to their container size —
the default only governs headless/background sessions and the reset target.

## Session reaping during app lifetime (issue #126)

Before this fix, agent-browser daemons (the headless Chromium processes
behind each `AgentBrowserSession`) only got cleaned up on app restart
(`kill_stream_daemons`/PID-file reconciliation in `AgentBrowserManager::new_*`).
Closing a workspace mid-session never tore down its daemon, so long-running
app sessions accumulated one orphaned Chromium process per closed workspace
that had ever opened a browser pane or background session.

**Reap-on-close (primary mechanism).** `AppStateStore::close_workspace`
collects the `cli_session_name`s of the agent-browser sessions removed
alongside the workspace — under the same lock acquisition that removes the
workspace, so there's no TOCTOU race with a concurrent session creation for
that workspace — and returns them on `CloseWorkspaceResult
.removed_agent_browser_sessions`. Every close path reaps them via the
shared `pub(crate) reap_agent_browser_sessions` helper in
`src-tauri/src/commands/workspace.rs`, which fire-and-forgets a single
background task calling `AgentBrowserManager::close()` per name (already
timeout-bounded internally, so this never blocks the close command):

- `close_workspace` (plain workspace close, sidebar/palette).
- `close_workspace_with_worktree_impl` (worktree close + the MCP
  `workspace_close` tool). This path previously had **no** agent-browser
  teardown at all — it was the main leak the issue reported.
- The workspaces-sync paths in `commands/workspaces_sync.rs`:
  `workspaces_reconcile_copy` (detaching a standalone-copy card) and the
  two adopt-rollback sites that remove a just-created shell.

**Shared-cwd safety (post-close live-set check).** Two workspaces opened
at the same cwd get separate `AgentBrowserSession` records but the SAME
`cli_session_name` (a pure hash of the cwd via
`stable_browser_session_name`), so they share one daemon. Before closing
each name, the reap task re-computes
`AppStateStore::live_agent_browser_session_names()` — after the close's
state mutation completed, so it's the post-close live set — and skips any
name a surviving workspace still maps to. This gives refcount-like
semantics: the daemon dies only when the LAST workspace sharing the name
closes. Relatedly, `resolve_agent_browser_session` no longer persists a
session record when the target workspace doesn't exist (an in-flight
`browser_automation` racing a close) — a persisted record for a dead
workspace could never be closed and would permanently poison the sweep's
live set; the transient session is returned un-persisted, and any daemon
spawned under it is orphan-swept later precisely because its name never
enters the live set.

**Periodic orphan sweep (backstop).** A background loop in `lib.rs`'s
`.setup()` closure ticks every 5 minutes (skipping the immediate first tick
so it never runs at t=0, before startup reconcile settles) and closes any
tracked `ws-*` session whose owning workspace no longer exists — catching
anything reap-on-close misses (e.g. a future workspace-removal path that
forgets to call the reap helper). Each tick:

1. `AgentBrowserManager::session_keys()` snapshots the tracked session keys
   FIRST.
2. `AppStateStore::live_agent_browser_session_names()` then computes the
   live set (the stable cwd-derived name for every workspace, plus every
   tracked `agent_browser_sessions[].cli_session_name`).
3. `agent_browser::orphaned_ws_sessions(&tracked, &live)` — a pure,
   unit-tested helper — returns tracked keys that start with `ws-` and
   aren't in the live set; each gets closed via `manager.close()`.

Safety properties: snapshotting `tracked` before `live` means a session
created mid-sweep is always in the live set and can never be reaped: the
ordering guarantees the workspace still exists in state at the moment
`live` is read. The sweep only ever inspects `AgentBrowserManager`'s
in-memory map (no `agent-browser session list` shell-out), so it can never
touch a daemon owned by another codemux instance. The `ws-*` filter means
user-initiated browser-pane sessions (`browser-NNN`) and the `default`
session are never swept, only workspace-scoped ones.

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
- `src-tauri/src/commands/browser.rs` — Tauri commands for pane creation, URL navigation, automation, and right-panel hosting (`dock_browser_in_right_panel`, `undock_browser_from_right_panel`)
- `src/components/layout/right-panel/browser-pane.tsx` — the deck's browser pane (body + tab-row nav actions); dock/undock lifecycle lives in `right-panel.tsx`
- `src/components/browser/browser-nav.ts` — shared `back`/`forward`/`reload`, URL normalization, and the short display form of a URL, used by both the main-area toolbar and the deck's status foot
- `src-tauri/src/control.rs` — `browser_automation` control-socket handler: workspace-scoped routing by `workspace_id`, the `resolve_workspace_id_by_cwd` fallback for env-less callers, and the legacy active-workspace path when neither resolves
- `src-tauri/src/cli.rs` — `BrowserCommand::Viewport` and `BrowserCommand::ViewportPresets` CLI subcommands
- `src-tauri/src/mcp_server.rs` — `browser_viewport` and `browser_viewport_presets` MCP tools
- `src/components/browser/BrowserPane.tsx` — screenshot rendering, toolbar, address bar, reactive `stream_url` reconnect on URL change, auto-syncs viewport on pane resize via legacy `width`/`height` payload (which the new socket handler accepts unchanged); pointer-capture input forwarding (drag selection, hover, click-count chaining, right/middle click), cursor probe, host-clipboard bridge, probe-based liveness + corner reconnect pill
- `src/components/browser/stream-protocol.ts` — pure helpers for the stream-input protocol (button mapping, click-count chaining, coordinate mapping, cursor sanitization) and the daemon HTTP endpoints (`/api/status` liveness probe, `/api/command` evals; page scripts minified for the single-segment HTTP constraint). Unit-tested in `stream-protocol.test.ts`
- `src/components/browser/InspectorPanel.tsx` — browser inspector/DevTools panel
- `src/components/browser/BrowserPeekOverlay.tsx` — GUI-mode background-browser floating peek (see "Background browser in GUI mode" above)
- `src/components/browser/background-browser-indicator.tsx` — shared detached-session lookup plus Agent Chat and terminal-header controls
- `src/components/chat/BackgroundBrowserChip.tsx` — the inline conversation chip for a background session
- `src/hooks/use-gui-chrome.ts` — the shared `useGuiChrome()` gate the chip/indicator/peek key off
- `src/stores/browser-peek-store.ts` — peek open/closed state (single `openWorkspaceId`; cleared on workspace switch)
- `src-tauri/src/state/state_impl.rs` — `AgentBrowserSession` (`is_active`, `pane_id`, `current_url`), `mark_agent_browser_active` / `mark_agent_browser_inactive` / `release_detached_agent_browser`, `attach_agent_browser_to_pane`, `detach_agent_browser_from_pane`, `find_detached_agent_browser`, `CloseWorkspaceResult.removed_agent_browser_sessions`, `live_agent_browser_session_names` (issue #126)
- `src-tauri/src/commands/agent_chat.rs` — `publish_pane_status` calls `release_detached_agent_browser` once a run settles to `Review`/`Idle` (see "Run-finished release" above)
- `src-tauri/src/commands/workspace.rs` — `reap_agent_browser_sessions` shared helper, called from both `close_workspace` and `close_workspace_with_worktree_impl` (issue #126)
- `src-tauri/src/agent_browser.rs` — `AgentBrowserManager::session_keys`, `orphaned_ws_sessions` pure helper backing the periodic orphan sweep in `lib.rs` (issue #126)
- `docs/reference/BROWSER-AGENT-COMMANDS.md` — CLI and socket command reference
- `docs/archive/browser-stream-fix.md` — landed cross-platform stream-stability plan (PID tracking, single canonical key, atomic teardown, symmetric bind probe, reactive frontend reconnect)
