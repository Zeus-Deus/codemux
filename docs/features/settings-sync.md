# Settings Sync

- Purpose: Describe the per-user synced settings system, what syncs, and current constraints.
- Audience: Anyone working on settings, preferences, or the settings UI.
- Authority: Canonical settings sync feature doc.
- Update when: Synced settings fields, sync protocol, cache behavior, or UI sections change.
- Read next: `docs/features/auth.md`, `docs/core/STATUS.md`

## What This Feature Is

Per-user settings that sync to the server via the auth API. A single `synced-settings-store` is the source of truth for all cloud-replicated preferences. Machine-local settings (sidebar state, window layout, workspace presets) stay in SQLite and are not synced.

## Current Model

### Architecture

- **Source of truth**: Server when reachable, offline cache when not
- **Server endpoints**: `api.codemux.org/api/settings` — GET, PUT, PATCH, DELETE
- **Auth**: Bearer token from the auth system (all endpoints require it)
- **Offline cache**: `~/.local/share/codemux/settings-cache.json` with a separate dirty flag file
- **Frontend store**: Zustand `synced-settings-store` with optimistic updates
- **Event bridge**: Backend emits `settings-synced` Tauri event after any mutation

### Synced Settings Schema

```
UserSettings
  appearance
    theme: string              (default: "system")
    shell_font: string | null  (default: null)
    terminal_font_size: number (default: 13.0)
  editor
    default_ide: string | null (default: null)
  terminal
    scrollback_limit: number   (default: 10000)
    cursor_style: string       (default: "bar")
  git
    default_base_branch: string (default: "main")
  keyboard
    shortcuts: Record<string, string> (default: {}, not yet UI-editable)
  notifications
    sound_enabled: boolean     (default: true)
    desktop_enabled: boolean   (default: true)
```

### Offline Cache

- **Cache path**: `~/.local/share/codemux/settings-cache.json`
- **Dirty flag path**: `~/.local/share/codemux/settings-dirty` (presence = dirty)
- Cache is always updated on any successful API response
- When offline: mutations save to cache and set dirty flag
- Dirty flag cleared on successful fetch, push, patch, or delete
- `clear_cache()` removes both files (called on sign-out)

### Key Design Decision: Login Fetch Order

**`flush_dirty` must NOT run before `fetch_settings` on login.** The server is the source of truth when reachable. A stale local cache (e.g., defaults written during sign-out by auto-detect effects) would overwrite the user's real cloud settings if flushed first. The login flow is:

1. Token validation succeeds
2. `fetch_settings(token)` pulls server state (authoritative)
3. Cache updated from server response
4. Dirty flag cleared

### Local-Only Settings (Not Synced)

These stay in the local SQLite database via `useSettingsStore`:

- AI agent settings (commit message model, resolver config, strategy)
- Terminal presets (workspace-local)
- Project scripts (workspace-local setup/teardown/run)
- Sidebar collapse state, window layout

### Adding a New Synced Setting

Three places to update:

1. **Rust struct**: Add field with `#[serde(default)]` to the appropriate section in `src-tauri/src/settings_sync.rs`
2. **TypeScript type**: Add field to matching interface in `src/tauri/types.ts`
3. **UI control**: Add input/toggle/select in the appropriate section of `src/components/settings/settings-view.tsx`, wired via `updateSyncedSetting(section, key, value)`

Serde default annotations ensure backward compatibility — missing fields deserialize to defaults.

## Settings UI

Opened via the settings button or command palette. Sections:

| Section | Category | What It Controls | Synced |
|---------|----------|-----------------|--------|
| Account | Personal | Email, name (read-only), sign out | No (auth state) |
| Appearance | Personal | Theme, font, radius (display-only) | Partial |
| Notifications | Personal | Sound toggle, desktop toggle | Yes |
| Shortcuts | Personal | Shortcut reference (read-only display) | No |
| Editor | Editor & Workflow | Default IDE dropdown | Yes |
| Terminal | Editor & Workflow | Scrollback limit, cursor style | Yes |
| Presets | Editor & Workflow | Terminal preset management | No (local) |
| Projects | Editor & Workflow | Setup/teardown/run scripts | No (local) |
| Git | Editor & Workflow | Default base branch | Yes |
| Agent | Editor & Workflow | AI commit/resolver config | No (local) |

## What Works Today

- Full CRUD for synced settings via API (GET/PUT/PATCH/DELETE)
- Offline cache with dirty flag for network resilience
- Optimistic UI updates (settings apply locally before server confirms)
- Partial JSON deserialization (missing fields fill in defaults)
- `settings-synced` Tauri event keeps frontend and backend aligned
- Memoized selectors for efficient re-renders
- Cache cleared on sign-out to prevent cross-user data leakage
- Reset to defaults via DELETE endpoint
- Auto-detect default editor on first settings load

## Current Constraints

- `flush_dirty()` exists but is not called on any automatic trigger (no online-transition flush)
- Keyboard shortcuts struct syncs but has no UI editor
- Patch operations do not have offline fallback (will fail if network unavailable)
- No conflict resolution between devices — last write wins
- No settings versioning or migration strategy yet

## Important Touch Points

- `src-tauri/src/settings_sync.rs` — Sync logic, cache, dirty flag, API calls
- `src-tauri/src/commands/settings_sync.rs` — Tauri commands: get, update, patch, reset
- `src-tauri/src/commands/auth.rs` — Login flow triggers `fetch_settings()` (not flush)
- `src/stores/synced-settings-store.ts` — Zustand store, selectors, optimistic updates
- `src/components/settings/settings-view.tsx` — Settings UI with all sections
- `src/tauri/types.ts` — `UserSettings` and nested interfaces
