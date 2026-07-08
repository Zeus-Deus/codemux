# GUI-Mode Chrome

- Purpose: Describe the collapsed single-titlebar chrome that renders for a
  real workspace when the Agent Chat Beta is on.
- Audience: Anyone working on the title bar, tab strip, preset launch UX, or
  the chat pane header.
- Authority: Canonical feature-level reality doc for GUI chrome.
- Update when: The titlebar composition, the launcher, or the legacy/GUI gate
  changes.
- Read next: `docs/features/agent-chat.md`, `docs/features/presets.md`,
  `docs/features/sidebar.md`

## What This Feature Is

GUI chrome collapses the four stacked chrome rows of a chat workspace —
`TitleBar` → `TabBar` → `PresetBar` → `AgentChatPaneHeader` — into a **single
`h-10` title bar**. It renders only when the `enable_agent_chat` Beta flag is
on and a real, non-OpenFlow workspace is active; every other case keeps the
legacy chrome unchanged.

## Current Model

`TitleBar` (`src/components/layout/title-bar.tsx`) branches on a computed
`guiChrome` flag: `enableAgentChat && !lazyDraftActive && activeWorkspaceId !=
null && workspace_type != null && workspace_type !== "open_flow"`. When false
it returns the byte-identical legacy `h-9` bar; when true it composes discrete
slots left-to-right:

`[sidebar toggle] [tabs] [+ launcher] [chat favorite] …drag spacer… [right-panel toggle] [RunButton] [ResourceMonitor] [IdeLauncher] [sep] [WindowControls]`

- **`TitleBarTabs`** (`src/components/layout/title-bar-tabs.tsx`) — the
  workspace's backend-owned tabs as compact pills (h-7, rounded-lg) with a
  per-tab status dot (highest-priority `pane_statuses` across the tab's panes),
  a chat-bubble / terminal / kind icon, and a hover/active-revealed close `X`.
  The **active chat tab** grows a chevron opening the session-history dropdown
  and renders the live "N subagents running" pill inline beside it.
  Activation/close route through the existing `activateTab` / `closeTab`
  commands.
- **`AgentLauncher`** (`src/components/layout/agent-launcher.tsx`) — the `+`
  popover, a cmdk `Command` with sections **GUI** (`chat_agent` presets,
  `PINNED` tag → `agentChatCreatePane`), **CLI agents** (pinned first, `↗
  terminal` tag → `applyPreset`; Shift = split pane), **Panes** (Terminal →
  `createTab`, Browser → `createBrowserPane`), and a **Manage presets…** footer
  (`setShowSettings(true, "presets")`). Preset data comes from the live
  `usePresetStore()` snapshot (`src/hooks/use-preset-store.ts`).
- **`PinnedChatFavorite`** — one ember-tinted (`accent-ember` tokens) inline
  button per `chat_agent` preset; click launches a new chat tab.
- **Rehomed controls** — the right-panel toggle (`FileDiff`, drives
  `rightPanelTabs`) and `RunButton` move from `TabBar`/`PresetBar` into the
  titlebar right cluster.
- **Row suppression** — `WorkspaceMain` (`workspace-main.tsx`) drops `TabBar`
  and `PresetBar` when the flag is on; `PaneNode` (`isSurfaceRoot` +
  `enableAgentChat`) suppresses `AgentChatPaneHeader` for a **sole-root**
  `agent_chat` pane. Split panes keep their per-pane header.

Session-switch / new-chat orchestration, checkpoint restore, and the
session-history list are extracted so the legacy per-pane header and the
titlebar tab share one implementation (see "Important Touch Points").

## What Works Today

- Single `h-10` titlebar for a real, non-OpenFlow chat workspace with the Beta
  flag on; legacy `h-9` chrome is byte-identical with the flag off.
- Pill tabs with status dots, active-tab close, and a chat-tab chevron opening
  the shared session-history dropdown (grouped sessions, active-session dot,
  delete-on-hover, "+ New Chat", and a "Restore checkpoint" footer item when a
  run-start checkpoint exists).
- `+` launcher covering GUI chat presets, CLI agents (with Shift-split),
  Terminal/Browser panes, and Manage presets.
- Inline ember chat favorite; rehomed right-panel toggle + RunButton; the "N
  subagents running" status pill kept alive next to the active chat tab.
- The empty titlebar center stays an OS drag region.

## Current Constraints

- **Draft (lazy-creation) workspaces keep legacy chrome.** A live chat draft
  keeps the `TabBar`/`PresetBar` rows and the `h-9` bar so the draft surface's
  own PresetBar stays coherent; GUI chrome is gated off while a draft is
  active.
- **OpenFlow workspaces are untouched** — they keep their dedicated chrome.
- **No tab drag-reorder in GUI chrome.** Reusing `TabBar`'s HTML5-drag logic
  inside a `data-tauri-drag-region` parent is not clean; reorder stays a
  legacy-only affordance (flag off) and is a follow-up.
- **A sole-root chat pane loses its per-pane split/close buttons** (the
  suppressed `AgentChatPaneHeader`); splitting is done from the launcher
  (Shift on a CLI agent) instead, and split layouts restore the per-pane
  header.

## Design intent (not shipped)

The titlebar is deliberately slot-composed and the `RightPanel` + resizer +
`rightPanelTabs` infra is preserved (hence rehoming the toggle) so a planned
inline **workflow-orchestration status pill** and an **Orchestration** right
side panel can land without restructuring the chrome. Neither is built.

## Important Touch Points

- `src/components/layout/title-bar.tsx` — the `guiChrome` gate + slot
  composition, `RightPanelToggle`, `PinnedChatFavorite`,
  `TitleBarWorkspaceSlots`.
- `src/components/layout/title-bar-tabs.tsx` — pill tab strip + active chat tab
  (chevron dropdown, subagents pill, checkpoint dialog).
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
- `src/components/layout/workspace-main.tsx` — TabBar/PresetBar suppression.
- `src/components/layout/PaneNode.tsx` / `pane-container.tsx` — `isSurfaceRoot`
  header suppression.
- Tests: `title-bar.test.tsx`, `title-bar-tabs.test.tsx`,
  `agent-launcher.test.tsx`, `workspace-main.test.tsx` (~14 Vitest cases).

## Notes

- Keep this file about current truth, not future plans.
- The legacy/GUI split is gated purely on the `enable_agent_chat` Beta flag —
  no new setting.
