# Settings Panel

- Purpose: Describe the current capability and constraints of the settings panel.
- Audience: Anyone working on settings, preferences, or configuration UI.
- Authority: Canonical feature-level reality doc.
- Update when: Settings sections, persistence behavior, or configuration scope changes.
- Read next: `docs/features/settings-sync.md`, `docs/reference/SHORTCUTS.md`

## What This Feature Is

The settings panel is a full-screen overlay providing centralized configuration for all Codemux subsystems. It opens with `Ctrl+,` or from the command palette.

## Current Model

Settings are organized into two nav groups (`buildNavGroups` in `settings-view.tsx`): **Personal** (Account, Appearance, Interface, Notifications, Shortcuts) and **Editor & Workflow** (Editor, Terminal, Presets, Projects, Archive, Git, Source Control, Agent, Browser, Devices, Remote Access, Session Restore — plus Permissions / Skills / MCP Servers, which only appear when the Agent Chat GUI is on). The former top-level **Beta Features** group was retired when the Agent Chat GUI was promoted to the default interface; its master toggle now lives in **Personal → Interface**. Most user-facing preferences are persisted via the synced settings store (server-synced with offline cache); appearance/sidebar preferences live in the machine-local `settings-store.ts`. UI-only state like the active section is managed in the UI store.

## What Works Today

### Personal Settings

- **Account**: sign-in email, display name, dev mode badge, sign out
- **Appearance**: theme preset, font family, border radius, **color palette** (cool/warm — a `.theme-warm` root class), **spacing density** (comfortable/compact — the `data-density` root attr), a **Sidebar** subsection — **Show git stats** (`sidebar.show_git_stats`, default on; off hides the ↑ahead and +/− numbers on inbox cards, branch stays) and **Auto-settle idle work** (`sidebar.auto_settle_days`, Off/1/3/7/14 days, default 3; merged/closed-PR cards auto-settle once idle regardless) — and an **Agents** subsection — a single **Match the orb to the activity** toggle (`agents.orb_match_activity`, default on) with a live two-row preview. On, the agent orb's animation follows what the agent is doing (searching / composing / working / solving / connecting / weaving / listening / breathing); off pins every orb in the app to the neutral `working` state. The orb itself is always monochrome and has no color setting. This replaced the **working-indicator glyph picker** (`sidebar.working_indicator`) and **indicator color swatches** (`sidebar.working_indicator_color`), both **deleted outright** — saved values are dropped on read rather than migrated, which is safe because the machine-local settings table is a free-form `Record<string, string>` with no Rust-side schema, so an old row is simply never read and never written back. See `docs/features/sidebar.md` and `docs/features/agent-chat.md` for the surfaces. The old `sidebar.workspace_detail` Clean/Branch/Detailed control and the `sidebar.live_agents` Stay-in-project/Gather-on-top grouping were both removed earlier. A **Scrolling** subsection carries **Smooth scrolling** (`appearance.smooth_scrolling`, default off) — it renders only on the Linux desktop app, where WebKit's animated wheel scrolling makes fast flicks travel *less* than slow ones; the value is machine-local (not synced) and hidden on the web remote client, since it acts on the desktop host's webview.
- **Interface**: the Agent Chat GUI master toggle (`InterfaceSection`, `interface-section.tsx`). Default **on** — flips `enable_agent_chat` + `enable_lazy_workspace_creation` atomically via the `set_agent_chat_enabled` Tauri command; turning it off returns to the classic terminal-first (CLI) interface. Either flip quits the app (reopen to apply). Existing installs are upgraded to on once by the `agent_chat_promoted` migration in `observability.rs` (marker persisted as a standalone sentinel file so a downgrade → upgrade cycle can't re-run it); an opt-out made after that is respected.
- **Notifications**: notification sounds (agent completion/attention), desktop notification toggle (D-Bus)
- **Shortcuts**: keyboard keybind customization with conflict detection, search, and reset to defaults

### Editor & Workflow Settings

- **Editor**: select default external editor for opening files, detected editor display
- **Terminal**: cursor style (bar/block/underline), font size (10-22px), color theme (app/system)
- **Presets**: terminal command presets, pin/unpin to quick-launch bar, edit/delete custom presets, toggle preset bar visibility
- **Projects**: workspace lifecycle scripts (setup/teardown/run), worktree include patterns, `.codemuxinclude` file support, environment variable references
- **Archive**: restore or permanently delete archived workspaces, grouped by project with relative times and stale hints (see `docs/features/workspace-archive.md`)
- **Git**: default base branch, AI commit message generation (model override), AI merge conflict resolver (strategy/CLI/model selection)
- **Source Control** (`source-control-section.tsx`): the hosting half of the Git subject. Two subsections — **Providers**, a diagnostics row per product (GitHub, GitLab, Bitbucket, Azure DevOps) from the `discover_source_control` probe showing ready/not-signed-in/CLI-missing state, the CLI version, a click-to-reveal masked account, what that adapter serves, and the one command or install URL that fixes a non-ready row, with a **Rescan** button; and **Self-hosted servers**, the `source_control.custom_hosts` editor that tells detection which product a server runs when its hostname doesn't say. Products with no adapter are listed dimmed rather than omitted. Codemux stores no hosting credentials — each product is driven through its own CLI (`gh`, `glab`). See `docs/features/source-control-providers.md`
- **Agent**: auto-configure MCP for workspaces toggle, run-checkpoint toggle, desktop-size background-browser peek toggle
- **Usage** (Agent Chat GUI only): provider-wide token history for Claude Code, Codex, and OpenCode over today / 7 / 30 / 90 days. It reads each provider's durable local history regardless of launcher, counts and flags subagent work, and reports one clearly labelled API/list-price-equivalent estimate rather than guessing what was billed or subscription-covered. Live provider quota remains separate. Includes CSV export (see `docs/features/usage-dashboard.md`).
- **Permissions** (Agent Chat GUI only): view and manage agent tool-permission rules (allow / deny / ask) flattened across the user / project-shared / project-local scopes (see `docs/features/permissions.md`)
- **Skills** (Agent Chat GUI only): skills sync management (see `docs/features/skills-sync.md`)
- **MCP Servers** (Agent Chat GUI only): MCP host/server configuration (see `docs/features/mcp-server.md`)
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
