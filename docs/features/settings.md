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
- **Appearance**: unchanged except for the theme control. **Picking** a theme is no longer on this page — it lives in the command palette (⌘K → "theme"), where the whole app repaints behind the list as you arrow through it; the swatch grid is replaced by a single row stating the applied theme (its surface + accent as two discs, its name, "Shell, terminal, code and editor. Synced to your account.") with two doors: **Change** reopens the palette on the theme query, **Customize** opens the Theme Studio on the current theme (or starts a new one when a built-in is applied — built-ins are managed and not editable in place). The studio is a **modal over Settings**, not a rail: Settings stays mounted behind it, so Esc returns you to Appearance. Export and delete for a saved custom theme live in its footer. Everything else on the page stayed put: a truthful fixed typography row, live radius readout, resource monitor, **spacing density** (comfortable/compact — the `data-density` root attr), wrap-code-in-chat, the Linux-only smooth-scrolling toggle, and the **Sidebar** and **Agents** subsections. Selected theme and custom themes sync; the old local cool/warm control is retired and an explicit saved warm value migrates once. The same theme drives product chrome, terminal ANSI, file-editor syntax, and chat syntax. See `docs/features/theming.md`.
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
- **Permissions** (Agent Chat GUI only): view and manage agent tool-permission rules (allow / deny / ask) flattened across the user / project-shared / project-local scopes (see `docs/features/permissions.md`)
- **Skills** (Agent Chat GUI only): skills sync management (see `docs/features/skills-sync.md`)
- **MCP Servers** (Agent Chat GUI only): MCP host/server configuration (see `docs/features/mcp-server.md`)
- **Browser**: **Default viewport** for agent browser sessions (a synced `browser.default_viewport` setting; the panel offers four canned sizes — Default 1280×800 / 1920×1080 / 2560×1440 / 3840×2160 — and surfaces any other value set from another device or by hand as an extra option), browser profile storage size display, clear cookies/site data, clear all browser data
- **Devices**: remote hosts / cloud-push targets — test connection, reinstall agent, upgrade status (see `docs/features/remote-hosts.md`)
- **Remote Access**: expose this desktop to a browser on another device — default-off master toggle, bind scope, pairing (see `docs/features/web-remote-access.md`)
- **Session Restore**: save/restore terminal scrollback on restart, scrollback line limit (1000-50000), max disk usage (10-500 MB)

## Current Constraints

- There is no whole-settings import/export; application themes have their own import/export flow.
- Notification sound toggle exists in UI, but actual audio playback is not implemented
- Custom keybinds stored in `~/.claude/keybindings.json`

## Important Touch Points

- `src/components/settings/settings-view.tsx` — main settings panel component
- `src/components/settings/keybind-editor.tsx` — keyboard shortcuts editor
- `src/stores/synced-settings-store.ts` — persisted user settings (server-synced)
- `src/components/settings/theme-settings.tsx` — the Appearance theme row
- `src/components/settings/theme-studio.tsx` — the app-level Generate / Import modal
- `src/components/settings/theme-swatches.tsx` — shared theme miniature, coins, ANSI dots
- `src/components/overlays/command-palette.tsx` — the theme picker itself
- `src/lib/themes.ts` — application-theme engine
- `src/stores/settings-store.ts` — machine-local appearance state (density, terminal source, scrolling)
- `src/stores/ui-store.ts` — settings panel open/section state
- `.codemux/config.json` — workspace-level project config
