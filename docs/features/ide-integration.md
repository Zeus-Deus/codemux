# IDE Integration

- Purpose: Describe editor detection and the open-in-editor workflow.
- Audience: Anyone working on external editor integration.
- Authority: Canonical feature-level reality doc.
- Update when: Detected editors, detection method, or UI entry points change.
- Read next: `docs/features/file-editor.md`, `docs/reference/SHORTCUTS.md`

## What This Feature Is

Codemux detects installed code editors on the system and provides "Open in Editor" actions throughout the UI. This lets users open workspaces or files in their preferred external IDE.

## Current Model

### Detection

On first call to `detect_editors()`, the backend resolves each candidate via the `which::which()` Rust crate (NOT a shelled-out `which` binary — that wouldn't exist on Windows). On Windows, if `PATH` lookup misses, the resolver falls back to a hardcoded list of well-known per-user install paths under `%LOCALAPPDATA%\Programs`, `%ProgramFiles%`, and `%ProgramFiles(x86)%`. Results are cached in a static `OnceLock<Vec<EditorInfo>>` for the session lifetime.

Detected editors:

| Command | Display Name | Windows Fallback Paths |
|---------|-------------|---------------------|
| `code` | VS Code | `Microsoft VS Code\Code.exe` |
| `cursor` | Cursor | `cursor\Cursor.exe` |
| `codium` | VSCodium | `VSCodium\VSCodium.exe` |
| `zed` | Zed | `Zed\Zed.exe` |
| `idea` | IntelliJ IDEA | (PATH only — JetBrains Toolbox shims live on `PATH`) |
| `goland` | GoLand | (PATH only) |
| `webstorm` | WebStorm | (PATH only) |
| `sublime_text` | Sublime Text | (PATH only) |

The `EditorInfo.command` field stores the **full resolved path**, not just the command name, so `open_in_editor` can spawn the exact `.exe` we detected — important on Windows where PATH alone is unreliable for per-user installs.

### Opening

`open_in_editor(editor_id, path)` spawns `<editor_command> <path>` as a detached subprocess. The user's default editor preference is stored in synced settings under `editor.default_ide`.

## What Works Today

### UI Entry Points

- **Title bar** — IdeLauncher button showing default editor icon and name. Dropdown arrow reveals all detected editors. Opens workspace `cwd`.
- **Workspace context menu** — Right-click workspace in sidebar shows "Open in {editor}" (single editor) or submenu with all editors (multiple detected).
- **File tree** — Files opened in built-in editor by default, but the external editor button is accessible from the title bar.

### Behavior

- Default editor auto-detected on first settings load (first available editor)
- User can change default via title bar dropdown or Settings > Editor
- Preference syncs across devices via settings sync
- Disabled state when no editors detected or no active workspace

## Current Constraints

- No global keyboard shortcut (accessed via title bar or context menu)
- Detection is PATH-based only (no desktop entry or registry scanning)
- Cannot open individual files in external editor from file tree (opens workspace root)
- Editor list is static (hard-coded candidates, not extensible by user)

## Important Touch Points

- `src-tauri/src/commands/workspace.rs` — `EDITOR_CANDIDATES`, `WINDOWS_EDITOR_FALLBACKS`, `resolve_editor_command()`, `windows_install_roots()`, `find_editors()`, `detect_editors()`, `open_in_editor()`, `DETECTED_EDITORS`
- `src/components/layout/title-bar.tsx` — IdeLauncher component
- `src/components/layout/sidebar-workspace-row.tsx` — context menu integration
- `src/stores/synced-settings-store.ts` — `editor.default_ide` preference
