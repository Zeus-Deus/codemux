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
| `code` | VS Code | `Microsoft VS Code\Code.exe`, `Microsoft VS Code Insiders\Code - Insiders.exe` |
| `cursor` | Cursor | `cursor\Cursor.exe` |
| `windsurf` | Windsurf | `Windsurf\Windsurf.exe` |
| `trae` | Trae | `Trae\Trae.exe` |
| `codium` | VSCodium | `VSCodium\VSCodium.exe` |
| `zed` | Zed | `Zed\Zed.exe` |
| `fleet` | Fleet | (PATH only — JetBrains Toolbox shim) |
| `lapce` | Lapce | `Lapce\lapce.exe` |
| `idea` | IntelliJ IDEA | (PATH only — JetBrains Toolbox shim) |
| `pycharm` | PyCharm | (PATH only — JetBrains Toolbox shim) |
| `phpstorm` | PhpStorm | (PATH only — JetBrains Toolbox shim) |
| `webstorm` | WebStorm | (PATH only — JetBrains Toolbox shim) |
| `goland` | GoLand | (PATH only — JetBrains Toolbox shim) |
| `rubymine` | RubyMine | (PATH only — JetBrains Toolbox shim) |
| `clion` | CLion | (PATH only — JetBrains Toolbox shim) |
| `rider` | Rider | (PATH only — JetBrains Toolbox shim) |
| `datagrip` | DataGrip | (PATH only — JetBrains Toolbox shim) |
| `studio` | Android Studio | (PATH only — JetBrains Toolbox shim) |
| `sublime_text` | Sublime Text | (PATH only) |

The `EditorInfo.command` field stores the **full resolved path**, not just the command name, so `open_in_editor` can spawn the exact `.exe` we detected — important on Windows where PATH alone is unreliable for per-user installs.

Editors without a dedicated icon asset (Windsurf, Trae, Fleet, Lapce, JetBrains family) render with a placeholder in the UI: JetBrains-family entries use the IntelliJ icon as a stand-in (consistent JetBrains visual language), other unmapped editors fall back to the generic Code2 lucide glyph. Adding per-product SVGs in `src/assets/editor-icons/` and wiring them into `src/components/icons/editor-icon.tsx` is a drop-in upgrade.

### Opening

`open_in_editor(editor_id, path)` spawns `<editor_command> <path>` as a detached subprocess. The user's default editor preference is stored in synced settings under `editor.default_ide`.

## What Works Today

### UI Entry Points

- **Title bar** — IdeLauncher button showing default editor icon and name. Dropdown arrow reveals all detected editors. Opens workspace `cwd`.
- **Workspace context menu** — Right-click workspace in sidebar shows "Open in {editor}" (single editor) or submenu with all editors (multiple detected).
- **File tree** — Files opened in built-in editor by default, but the external editor button is accessible from the title bar.

The title-bar launcher and context-menu submenu partition entries into labelled sections — **VS Code family** (VS Code, Cursor, VSCodium), **Modern editors** (Zed, Lapce — Windsurf and Trae group under VS Code as forks; Fleet groups under JetBrains), **JetBrains** (IntelliJ IDEA, PyCharm, PhpStorm, WebStorm, GoLand, RubyMine, CLion, Rider, DataGrip, Android Studio), and **Other** (Sublime Text). Section headers only render when more than one family is detected (no lonely headers). Within each section the backend's canonical detection order is preserved. The grouping function lives in `src/lib/editor-groups.ts` (with `src/lib/editor-groups.test.ts` covering the partitioning).

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
- `src/lib/editor-groups.ts` + `src/lib/editor-groups.test.ts` — family partitioning (VS Code family / Modern / JetBrains / Other)
- `src/components/icons/editor-icon.tsx` — per-editor SVG + placeholder fallback (JetBrains-family entries use the IntelliJ glyph as a stand-in)
- `src/components/layout/title-bar.tsx` — IdeLauncher component
- `src/components/layout/sidebar-workspace-row.tsx` — context menu integration
- `src/stores/synced-settings-store.ts` — `editor.default_ide` preference
