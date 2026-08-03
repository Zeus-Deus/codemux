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
Beta opt-in) and a real, non-OpenFlow workspace is active; every other case
keeps the legacy chrome unchanged.

## Current Model

`TitleBar` (`src/components/layout/title-bar.tsx`) branches on a computed
`guiChrome` flag: `enableAgentChat && !lazyDraftActive && activeWorkspaceId !=
null && workspace_type != null && workspace_type !== "open_flow"`. This
predicate lives in the shared `useGuiChrome()` hook
(`src/hooks/use-gui-chrome.ts`) so other GUI-mode-only surfaces gate on the
identical rule — currently the background-browser inline chip, context-bar
indicator, and peek overlay (`docs/features/browser.md` § "Background
browser in GUI mode"). A live lazy-creation draft renders the same floating
`h-10` overlay with **draft slots** instead, gated on the sibling
`useDraftGuiChrome()` predicate (`enableAgentChat && lazyDraftActive` —
mutually exclusive with `guiChrome`; see "Draft titlebar variant" below).
When neither predicate holds it returns the byte-identical legacy `h-9`
bar in normal document flow. Workspace GUI chrome is absolutely positioned
over the full app shell and composes four independent control clusters on
desktop (the native-controls cluster is absent on a web remote client):

`[sidebar toggle]    [tabs | + launcher | pinned preset tiles] …drag region… [RunButton split | ResourceMonitor | IdeLauncher compact | right-panel toggle]    [WindowControls]`

The far-left cluster contains **only the sidebar toggle**. The workspace band
starts just after the live sidebar width (`useSidebarGapWidth()` in
`src/hooks/use-sidebar-gap-width.ts` — measures the sidebar's
`[data-slot="sidebar-gap"]` box via ResizeObserver, since the titlebar
renders outside `SidebarProvider`). Its right edge moves left when the right
panel opens, using the persisted panel width, so the action island remains
anchored immediately before that panel. Native window controls remain a
separate cluster at the physical top-right corner. Unoccupied overlay space is
still a Tauri drag region; each interactive cluster opts back into pointer
events.

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
  The expanded sidebar
  reserves the same local clearance only above its search row, and the
  collapsed rail reserves it above its first action. On desktop, the right
  panel keeps its background full-height but gives its tabs `mt-10`, keeping
  them clear of native window controls without clipping narrow tab strips.
  The workspace-tab and action islands stay transparent and frameless at
  rest. Each mounted chat viewport reports whether it has actually scrolled
  beneath the overlay, while `TitleBar` measures whether either island
  physically intersects that viewport's centered 792px reading column. Only
  when both conditions are true do those two 32px islands gain an opaque,
  borderless raised surface. There is no full-width header or fading scrim;
  the empty drag region stays transparent, and the composer is not affected.
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
  right-panel surfaces for a real, non-OpenFlow chat workspace with the
  GUI flag on (default); legacy `h-9` chrome is byte-identical with the flag
  off.
- Pill tabs with status dots, active-tab close, and a chat-tab chevron opening
  the shared session-history dropdown ("+ New Chat" and a "Restore checkpoint"
  item at the top when a run-start checkpoint exists, then the grouped sessions
  with active-session dot and delete-on-hover).
- `+` launcher covering GUI chat presets, CLI agents (with Shift-split),
  Terminal/Browser panes, and Manage presets.
- Inline ember chat favorite; rehomed RunButton plus a right-panel toggle in
  an action cluster that tracks the right-panel edge. (The
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
  chip, context-bar indicator, right-cluster controls) stay off because the
  backend's active workspace is not what's on screen.
- **OpenFlow workspaces are untouched** — they keep their dedicated chrome.
- **A sole-root chat pane loses its per-pane split/close buttons** (the
  suppressed `AgentChatPaneHeader`); splitting is done from the launcher
  (Shift on a CLI agent) instead, and split layouts restore the per-pane
  header.

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
  source of truth, extracted from `title-bar.tsx`) plus the sibling
  `useDraftGuiChrome()` draft predicate.
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
- `src/components/layout/sidebar-action-row.tsx` — local top clearance for the
  expanded sidebar and collapsed rail.
- `src/components/layout/right-panel.tsx` — full-height panel surface with
  desktop-only tab-row clearance below native window controls.
- `src/components/layout/PaneNode.tsx` / `pane-container.tsx` — `isSurfaceRoot`
  header suppression.
- Tests: `title-bar.test.tsx`, `title-bar-tabs.test.tsx`,
  `agent-launcher.test.tsx`, `workspace-main.test.tsx`,
  `right-panel.test.tsx`.

## Notes

- Keep this file about current truth, not future plans.
- The legacy/GUI split is gated purely on the `enable_agent_chat` flag —
  default on, flipped from Settings → Interface (the retired Beta toggle,
  promoted to a regular setting).
