# GUI-Mode Chrome

- Purpose: Describe the floating, edge-to-edge chrome that renders for a
  real workspace when the Agent Chat GUI is on (the default interface).
- Audience: Anyone working on the title bar, tab strip, preset launch UX, or
  the chat pane header.
- Authority: Canonical feature-level reality doc for GUI chrome.
- Update when: The titlebar composition, the launcher, or the legacy/GUI gate
  changes.
- Read next: `docs/features/agent-chat.md`, `docs/features/presets.md`,
  `docs/features/sidebar.md`, `docs/features/workflow-orchestration.md`

## What This Feature Is

GUI chrome collapses the four stacked chrome rows of a chat workspace —
`TitleBar` → `TabBar` → `PresetBar` → `AgentChatPaneHeader` — into a **floating
`h-10` overlay with compact control clusters**. There is no full-width titlebar
surface or divider: the sidebar, workspace, and optional right panel all reach
the physical top edge, the cluster wrappers are transparent and frameless, and
only controls that need an affordance carry their own chrome. It
renders when the `enable_agent_chat` flag is on
(the default — the flag is now a regular Settings → Interface toggle, not a
Beta opt-in) and a real workspace is active; every other case
keeps the in-flow legacy `h-9` bar.

**Chrome-mode gate for adjacent surfaces.** Every surface that reaches the
physical top edge in overlay mode adapts to it, gated on
`useTitlebarOverlay()` (`src/hooks/use-gui-chrome.ts` — `useGuiChrome() ||
useDraftGuiChrome()`, i.e. "the titlebar is floating"). Gating on anything
weaker is a bug: with the flag off, the legacy bar already occupies that
space in normal flow, so unconditional clearance renders as a dead band above
the sidebar search row and the right-panel tabs.

There are two ways to adapt, and which one is right depends on whether the
surface has chrome of its own to put there. The sidebar and the workspace
content box **reserve** a 40px collision zone. The right panel **participates
instead**: its own tab row grows to `h-10`, sits flush with the window's top
edge, and becomes the panel's slice of the band. It used to reserve like the
others (`mt-10`), which drew an empty 40px header across the top of the panel
— see "Band geometry" below.

## Current Model

`TitleBar` (`src/components/layout/title-bar.tsx`) branches on a computed
`guiChrome` flag: `enableAgentChat && !lazyDraftActive && activeWorkspaceId !=
null`. This
predicate lives in the shared `useGuiChrome()` hook
(`src/hooks/use-gui-chrome.ts`) so other GUI-mode-only surfaces gate on the
identical rule — currently the background-browser inline chip, the
terminal-header indicator, and the peek overlay (`docs/features/browser.md` § "Background
browser in GUI mode"). A live lazy-creation draft renders the same floating
`h-10` overlay with **draft slots** instead, gated on the sibling
`useDraftGuiChrome()` predicate (`enableAgentChat && lazyDraftActive` —
mutually exclusive with `guiChrome`; see "Draft titlebar variant" below).
When neither predicate holds it returns the byte-identical legacy `h-9`
bar in normal document flow. Workspace GUI chrome is absolutely positioned
over the full app shell and composes four independent control clusters on
desktop (the native-controls cluster is absent on a web remote client):

`[sidebar toggle]    [tabs | + launcher | pinned preset tiles] …drag region… [RunButton split | IdeLauncher compact | ResourceMonitor]  ‖  [right-panel tab row] …drag gap… [pane actions]    [⤢ | right-panel toggle]  [WindowControls]`

The `‖` is the right panel's left edge. Everything left of it belongs to the
workspace column; everything right of it is the panel's own tab row, which
**renders inside this band** rather than below it.

The far-left cluster contains **only the sidebar toggle**. The workspace band
starts just after the live sidebar width (`useSidebarGapWidth()` in
`src/hooks/use-sidebar-gap-width.ts` — measures the sidebar's
`[data-slot="sidebar-gap"]` box via ResizeObserver, since the titlebar
renders outside `SidebarProvider`). Its right edge moves left when the right
panel opens, using the persisted panel width, **for the workspace branch
only** — during a draft the backend's active workspace is not what's on
screen, so its panel state is ignored and the band spans full width.

### Band geometry: the fixed top-right corner

`src/lib/titlebar-geometry.ts` is the single place the numbers live, because
two React trees have to agree on them: the overlay (`title-bar.tsx`, rendered
outside the layout) and the right panel's tab row (`right-panel/
pane-tab-strip.tsx`, rendered inside it).

The rule is that **the top-right cluster never moves**. `⤢` (full expand) and
the right-panel toggle sit at `right: 104px` on desktop (`6px` on the web
client, which draws no window buttons) whether the panel is open, closed,
narrow, wide or fully expanded. Everything else is derived from that corner:

- the workspace band stops at `topRightReserve()` = `104 + 56 + 6` = **166px**
  when the panel is closed, and at the panel's left edge (`panelWidth + 8`)
  when it is open;
- the panel's tab row runs to the physical window edge and pads its right
  side by that same `166px` (68px on the web client) to clear the cluster and
  the window buttons drawn above it.

This replaced the previous geometry, where the action island's right edge
tracked `rightPanelWidth + 8` in *all* states and the panel reserved a blank
`mt-10` strip for the overlay. That produced the two defects this rework
targets: a ~40px empty band across the top of the panel (the panel read as a
pane inside a pane with a blank header), and a right-panel toggle that
teleported from the window's top-right corner to the panel's left edge every
time the panel opened. A draft keeps the old `104px` reserve — it renders no
right panel, so it has no cluster to clear.

Unoccupied overlay space is a Tauri drag region on **desktop only**, and the
overlay's drag layer now **stops at the panel's left edge** (`inset-y-0
left-0` with a computed `right`) instead of spanning the window. It has to:
the panel's tabs, `+` and pane actions now live under that layer, and a
full-width one would swallow every click on them. The panel supplies its own
drag surface in the flex gap after its tabs
(`[data-testid="right-panel-drag-gap"]`), so the whole band is still
draggable end to end. The gap between the workspace band's two islands stays
a drag region as before. On the web remote client every one of these layers
drops `pointer-events` / the attribute: `data-tauri-drag-region` does nothing
in a browser, so there they are pure pointer sinks. Each interactive cluster
opts back into pointer events in both modes.

While the panel is **fully expanded** the workspace band is `hidden`
outright — the panel owns the whole content row, so workspace tabs would be
chrome for a zero-width column drawn on top of the panel's own row. The
sidebar toggle, the panel cluster and the window buttons stay. Restoring
brings the band straight back.

- **`TitleBarTabs`** (`src/components/layout/title-bar-tabs.tsx`) — the
  workspace's backend-owned tabs as compact pills (h-7, rounded-lg) with a
  per-tab status dot (highest-priority `pane_statuses` across the tab's panes),
  a chat-bubble / terminal / kind icon, and a hover/active-revealed close `X`.
  Inactive tabs use the full `text-muted-foreground` token so their labels
  remain legible over the narrow-window glass surface. The **active chat tab**
  grows a chevron opening the session-history dropdown.
  (The inline "N subagents running" pill that used to ride beside it was
  removed in favor of the docked `SubagentActivityBar` above the composer —
  see `docs/features/agent-chat.md` "Docked live activity bar".)
  Activation/close route through the existing `activateTab` / `closeTab`
  commands. Pills cap at `max-w-[130px]` and the strip scrolls
  horizontally (wheel → horizontal translation, hidden scrollbar) instead
  of shrinking when tabs overflow.
- **`AgentLauncher`** (`src/components/layout/agent-launcher.tsx`) — the `+`
  popover, a cmdk `Command` with sections **GUI** (`chat_agent` presets,
  `PINNED` tag → `agentChatCreatePane`), **CLI agents** (pinned first, `↗
  terminal` tag → `applyPreset`; Shift = split pane), **Panes** (Terminal →
  `createTab`, Browser → `createBrowserPane`), and a **Manage presets…** footer
  (`setShowSettings(true, "presets")`). Preset data comes from the live
  `usePresetStore()` snapshot (`src/hooks/use-preset-store.ts`).
- **`PinnedPresetTiles`** — 27px glyph tiles right of the `+` launcher
  (after a 1px divider), **opt-in and empty by default**: only presets the
  user pinned to the title bar render (ember-tinted `accent-ember` tiles
  for `chat_agent`, neutral for `cli`), each with a hover Tooltip naming
  the preset. The pin set lives in `src/stores/titlebar-pins-store.ts`
  (zustand persist, separate from the legacy `preset.pinned` flag that
  drives the flag-off PresetBar) and is toggled from a hover pin button on
  each launcher row. Click semantics mirror the launcher (chat = new chat
  tab; CLI = new tab, Shift-click = split via `applyPreset`).
- **Rehomed controls** — the right-panel toggle (`PanelRight`, mirroring the
  left sidebar's `PanelLeft` glyph, drives
  `rightPanelTabs`) and `RunButton` move from `TabBar`/`PresetBar` into the
  floating action cluster. The panel toggle follows the content controls
  without an extra divider; the separate native-controls cluster remains at
  the window edge. The controls are split into two visual groups: `RunButton`
  plus the compact IDE launcher are bordered primary actions; the bordered
  toolbar-style `ResourceMonitor` plus frameless right-panel toggle are
  utilities, separated from the action chips by a wider gap. The monitor uses
  the same `border-border` + `bg-secondary/50` treatment as the adjacent chips
  so its square silhouette does not read smaller. The legacy titlebar keeps
  the resource monitor's default ghost treatment. In GUI chrome the
  `RunButton` renders its
  `variant="split"` form (main segment = green play + Run/Set Run, caret
  segment = configure; no standalone gear) and `IdeLauncher` renders
  `compact` (icon square + caret, combined tooltip). Both components keep
  their legacy default rendering for the flag-off `PresetBar`, preserving
  the byte-identical contract.
- **Row suppression** — `WorkspaceMain` (`workspace-main.tsx`) drops `TabBar`
  and `PresetBar` when the flag is on (both on the real-workspace branch and
  the draft branch); `PaneNode` (`isSurfaceRoot` + `enableAgentChat`)
  suppresses `AgentChatPaneHeader` for a **sole-root** `agent_chat` pane.
  Split panes keep their per-pane header. `DraftChatSurface` likewise
  suppresses its placeholder `DraftSurfaceHeader` band in GUI mode. The sole
  root chat therefore reclaims the top edge. Terminal panes retain a 28px
  local drag/action row, but it is transparent: a sole-root terminal omits its
  redundant title, while split children render title/CWD and actions as compact
  pane-local islands instead of a second full-width header. Other non-chat
  workspace surfaces and onboarding reserve a local `pt-10` collision zone.
  The expanded sidebar reserves the same local clearance only above its
  search row, and the collapsed rail reserves it above its first action —
  both gated on `useTitlebarOverlay()`, so the legacy insets (`pt-3` /
  `pt-2`) come back with the in-flow bar. The right panel keeps its
  background full-height and gives its tabs `mt-10` on **desktop in overlay
  mode only**, clearing the native window controls without clipping narrow
  tab strips; the web client needs no allowance because it renders neither
  those controls nor the overlay drag layer.
  The workspace-tab and action islands stay transparent and frameless at
  rest. Each mounted chat viewport reports whether it has actually scrolled
  beneath the overlay, while `TitleBar` measures whether either island
  physically intersects that viewport's centered 792px reading column. Only
  when both conditions are true do those two 32px islands gain an opaque,
  borderless raised surface. There is no full-width header or fading scrim;
  the empty drag region stays transparent, and the composer is not affected.
  Because `PaneContainer` renders only the active surface, every tab or
  workspace switch mounts a brand-new transcript node — so each viewport
  registers itself in `titlebar-content-under.ts`'s element registry, whose
  version counter re-keys the titlebar's measurement effect. Snapshotting
  `document.querySelectorAll` once is not sufficient: the titlebar would
  keep observing a detached node and the raised treatment would stop firing
  after the first navigation.
- **Draft titlebar variant** — while a lazy-creation draft is the active
  surface (`useDraftGuiChrome()`), the `h-10` bar renders
  `TitleBarDraftSlots` in place of the workspace slots: a single static
  "Agent Chat" pill in the active-tab style plus `DraftAgentLauncher`
  (`agent-launcher.tsx`) — a `+` popover whose GUI/CLI rows materialise the
  draft via the shared `launchDraftWithPreset` helper
  (`src/lib/agent-chat/draft-preset-launch.ts`, also used by the legacy
  draft `PresetBar`). The launcher hides on a Home-target draft and disables
  while a materialise is in flight, mirroring the legacy draft PresetBar's
  rules. Workspace-scoped right-cluster controls (right-panel toggle,
  `RunButton`, `IdeLauncher`) are suppressed — the backend's "active
  workspace" during a draft is whatever was focused before the draft opened,
  not what's on screen. This keeps the titlebar silhouette identical across
  the draft → materialised transition, so pressing "+" no longer flashes the
  legacy `h-9` bar + `PresetBar` rows.

Session-switch / new-chat orchestration, checkpoint restore, and the
session-history list are extracted so the legacy per-pane header and the
titlebar tab share one implementation (see "Important Touch Points").

## What Works Today

- Frameless floating control clusters over full-height sidebar, workspace, and
  right-panel surfaces for a real chat workspace with the
  GUI flag on (default). With the flag off the legacy `h-9` bar and every
  layout inset around it are unchanged — but the flag-off surfaces are *not*
  literally byte-identical any more: the legacy `TabBar`'s panel toggle uses
  the `PanelRight` glyph (mirroring the sidebar's `PanelLeft`, matching the
  GUI toggle) rather than `FileDiff`, and the transparent terminal pane
  chrome (`docs/features/terminal.md`) is deliberately flag-independent so a
  terminal looks the same in both modes. Both are intended.
- Pill tabs with status dots, active-tab close, and a chat-tab chevron opening
  the shared session-history dropdown ("+ New Chat" and a "Restore checkpoint"
  item at the top when a run-start checkpoint exists, then the grouped sessions
  with active-session dot and delete-on-hover).
- `+` launcher covering GUI chat presets, CLI agents (with Shift-split),
  Terminal/Browser panes, and Manage presets.
- Inline ember chat favorite; rehomed RunButton in an action cluster that
  tracks the right-panel edge. The panel's own controls (`⤢` full expand and
  the right-panel toggle) are **not** in that cluster — they sit in a fixed
  top-right cluster that never moves. (The
  "N subagents running" status pill that originally rehomed next to the
  active chat tab was later removed — subagent status now lives in the
  docked `SubagentActivityBar` above the composer; see
  `docs/features/agent-chat.md` "Docked live activity bar".)
- Tab drag-reorder via a pointer-based drag (pointerdown on a pill body +
  ~5px movement threshold → drag mode, drop index computed from tab
  midpoints, same `reorder_tabs` backend command and insertion-indicator
  styling as the legacy `TabBar`). Pointer events avoid the titlebar's
  `data-tauri-drag-region`, unlike HTML5 DnD; a completed drag suppresses
  the trailing click so it never activates a tab or pops the chat-tab
  history dropdown.
- Empty space along the top overlay stays an OS drag region.

## Current Constraints

- **Draft chrome is a reduced floating overlay, not full workspace chrome.** A
  live chat draft renders the GUI-styled `h-10` draft variant (static pill +
  draft launcher), but `useGuiChrome()` itself still resolves `false` during
  a draft — workspace-scoped GUI surfaces (titlebar tabs, background-browser
  chip, terminal-header indicator, right-cluster controls) stay off because the
  backend's active workspace is not what's on screen.
- **A sole-root chat pane loses its per-pane split/close buttons** (the
  suppressed `AgentChatPaneHeader`); splitting is done from the launcher
  (Shift on a CLI agent) instead, and split layouts restore the per-pane
  header.
- **On desktop, the top 40px of a sole-root chat transcript is a window-drag
  strip, not scrollable content.** This is the deliberate price of letting
  the transcript reclaim the top edge (no `pt-10`) while keeping a frameless
  window draggable — the standard frameless-app tradeoff. The reader can
  still scroll anywhere below it, and the raised-island treatment keeps the
  controls legible over whatever passes underneath. The web client has no
  window to drag, so it drops those layers entirely and the full transcript
  height stays interactive there.

## Workflow orchestration integration

The titlebar is deliberately slot-composed and the `RightPanel` + resizer +
`rightPanelTabs` infra is preserved (hence rehoming the toggle), which let
the **Orchestration** right-panel tab for Claude workflow runs land without
restructuring this chrome — `RightPanelTab` gained an `"orchestration"`
variant that `right-panel.tsx` renders conditionally, only when the active
workspace has a workflow run. See `docs/features/workflow-orchestration.md`
for the full pipeline (Claude-only `Workflow` tool tap, the in-thread
`WorkflowRunCard`, and the panel body).

## Important Touch Points

- `src/hooks/use-gui-chrome.ts` — the shared `guiChrome` predicate (single
  source of truth, extracted from `title-bar.tsx`), the sibling
  `useDraftGuiChrome()` draft predicate, and `useTitlebarOverlay()` (their
  union) — the one gate every top-edge collision clearance must use.
- `src/lib/titlebar-geometry.ts` — the band's height and the fixed
  top-right reserve, shared by the overlay and the right panel's tab row.
  Change a number here, not in a component.
- `src/components/layout/title-bar.tsx` — consumes `useGuiChrome()` +
  `useDraftGuiChrome()` for the slot-composition branch, `RightPanelToggle`,
  `PinnedPresetTiles`, `TitleBarWorkspaceSlots`, `TitleBarDraftSlots`, and the
  floating-island placement.
- `src/components/layout/app-shell.tsx` — provides the relative, clipped shell
  that contains the absolute GUI overlay while leaving legacy chrome in flow.
- `src/lib/agent-chat/draft-preset-launch.ts` — shared
  materialise-with-preset action behind both the legacy draft `PresetBar`
  and `DraftAgentLauncher`.
- `src/components/layout/title-bar-tabs.tsx` — pill tab strip + active chat tab
  (chevron dropdown, checkpoint dialog). The inline subagents pill that used to
  live here was removed by PR #143 — see `src/components/chat/SubagentActivityBar.tsx`.
- `src/components/layout/agent-launcher.tsx` — the `+` popover.
- `src/hooks/use-preset-store.ts` — live preset snapshot for the launcher +
  favorite.
- `src/components/chat/session-history-menu.tsx` — shared `SessionHistoryList`
  + `useSessionHistory`, consumed by both `SessionSelector` and the titlebar
  chat tab.
- `src/hooks/use-agent-chat-session-actions.ts` — shared resume/new-chat
  orchestration extracted from `AgentChatPaneHeader`.
- `src/hooks/use-agent-chat-checkpoint-restore.ts` +
  `src/components/chat/restore-checkpoint-dialog.tsx` — shared checkpoint
  restore state + confirm dialog.
- `src/components/layout/workspace-main.tsx` — TabBar/PresetBar suppression,
  full-height panel composition, and per-surface top collision clearance.
- `src/components/layout/sidebar-action-row.tsx` — overlay-gated top
  clearance for the expanded sidebar and collapsed rail.
- `src/components/layout/right-panel.tsx` — full-height panel surface with
  overlay-mode, desktop-only tab-row clearance below native window controls.
- `src/lib/titlebar-content-under.ts` — the per-workspace "transcript has
  scrolled under the overlay" aggregate **and** the live transcript-element
  registry + version counter that keeps the overlap measurement off detached
  nodes; published by `src/components/chat/MessageList.tsx`.
- `src/components/layout/PaneNode.tsx` / `pane-container.tsx` — `isSurfaceRoot`
  header suppression.
- Tests: `title-bar.test.tsx`, `title-bar-tabs.test.tsx`,
  `agent-launcher.test.tsx`, `workspace-main.test.tsx`,
  `right-panel.test.tsx`, `sidebar-action-row.test.tsx`,
  `titlebar-content-under.test.ts`, `MessageList.test.tsx`.

## Notes

- Keep this file about current truth, not future plans.
- The legacy/GUI split is gated purely on the `enable_agent_chat` flag —
  default on, flipped from Settings → Interface (the retired Beta toggle,
  promoted to a regular setting).
