# Settings Panel

- Purpose: Describe the current capability and constraints of the settings panel.
- Audience: Anyone working on settings, preferences, or configuration UI.
- Authority: Canonical feature-level reality doc.
- Update when: Settings sections, persistence behavior, or configuration scope changes.
- Read next: `docs/features/settings-sync.md`, `docs/reference/SHORTCUTS.md`

## What This Feature Is

The settings panel is a full-screen overlay providing centralized configuration for all Codemux subsystems. It opens with `Ctrl+,` or from the command palette.

## Current Model

Settings are organized into three nav groups (`buildNavGroups` in `settings-view.tsx`): **Beta Features** (Agent Chat), **Personal** (Account, Appearance, Notifications, Shortcuts), and **Editor & Workflow** (Editor, Terminal, Presets, Projects, Archive, Git, Agent, Browser, Devices, Remote Access, Session Restore — plus Permissions / Skills / MCP Servers, which only appear when the Agent Chat Beta is on). Most user-facing preferences are persisted via the synced settings store (server-synced with offline cache); appearance/sidebar preferences live in the machine-local `settings-store.ts`. UI-only state like the active section is managed in the UI store.

## What Works Today

### Personal Settings

- **Account**: sign-in email, display name, dev mode badge, sign out
- **Appearance**: theme preset, font family, border radius, **color palette** (cool/warm — a `.theme-warm` root class), **spacing density** (comfortable/compact — the `data-density` root attr), a **Sidebar** subsection — **Show git stats** (`sidebar.show_git_stats`, default on; off hides the ↑ahead and +/− numbers on inbox cards, branch stays) and **Auto-settle idle work** (`sidebar.auto_settle_days`, Off/1/3/7/14 days, default 3; merged/closed-PR cards auto-settle once idle regardless) — and an **Agents** subsection — the working-indicator glyph (braille / ring / blink / sweep / typing, `sidebar.working_indicator`) + color (amber / white / ember / green / sky / violet, no red, `sidebar.working_indicator_color`) with a live preview. These drive the sidebar inbox (see `docs/features/sidebar.md`); the old `sidebar.workspace_detail` Clean/Branch/Detailed control and the `sidebar.live_agents` Stay-in-project/Gather-on-top grouping were both removed. A **Scrolling** subsection carries **Smooth scrolling** (`appearance.smooth_scrolling`, default off) — it renders only on the Linux desktop app, where WebKit's animated wheel scrolling makes fast flicks travel *less* than slow ones; the value is machine-local (not synced) and hidden on the web remote client, since it acts on the desktop host's webview.
- **Notifications**: notification sounds (agent completion/attention), desktop notification toggle (D-Bus)
- **Shortcuts**: keyboard keybind customization with conflict detection, search, and reset to defaults

### Editor & Workflow Settings

- **Editor**: select default external editor for opening files, detected editor display
- **Terminal**: cursor style (bar/block/underline), font size (10-22px), color theme (app/system)
- **Presets**: terminal command presets, pin/unpin to quick-launch bar, edit/delete custom presets, toggle preset bar visibility
- **Projects**: workspace lifecycle scripts (setup/teardown/run), worktree include patterns, `.codemuxinclude` file support, environment variable references
- **Archive**: restore or permanently delete archived workspaces, grouped by project with relative times and stale hints (see `docs/features/workspace-archive.md`)
- **Git**: default base branch, AI commit message generation (model override), AI merge conflict resolver (strategy/CLI/model selection)
- **Agent**: auto-configure MCP for workspaces toggle, run-checkpoint toggle, desktop-size background-browser peek toggle
- **Permissions** (Agent Chat Beta only): view and manage agent tool-permission rules (allow / deny / ask) flattened across the user / project-shared / project-local scopes (see `docs/features/permissions.md`)
- **Skills** (Agent Chat Beta only): skills sync management (see `docs/features/skills-sync.md`)
- **MCP Servers** (Agent Chat Beta only): MCP host/server configuration (see `docs/features/mcp-server.md`)
- **Browser**: **Default viewport** for agent browser sessions (a synced `browser.default_viewport` setting; the panel offers four canned sizes — Default 1280×800 / 1920×1080 / 2560×1440 / 3840×2160 — and surfaces any other value set from another device or by hand as an extra option), browser profile storage size display, clear cookies/site data, clear all browser data
- **Devices**: remote hosts / cloud-push targets — test connection, reinstall agent, upgrade status (see `docs/features/remote-hosts.md`)
- **Remote Access**: expose this desktop to a browser on another device — default-off master toggle, bind scope, pairing (see `docs/features/web-remote-access.md`)
- **Session Restore**: save/restore terminal scrollback on restart, scrollback line limit (1000-50000), max disk usage (10-500 MB)

## Current Constraints

- No import/export of settings
- Notification sound toggle exists in UI, but actual audio playback is not implemented
- Custom keybinds stored in `~/.claude/keybindings.json`

## Important Touch Points

- `src/components/settings/settings-view.tsx` — main settings panel component
- `src/components/settings/keybind-editor.tsx` — keyboard shortcuts editor
- `src/stores/synced-settings-store.ts` — persisted user settings (server-synced)
- `src/stores/settings-store.ts` — local theme/appearance state
- `src/stores/ui-store.ts` — settings panel open/section state
- `.codemux/config.json` — workspace-level project config
