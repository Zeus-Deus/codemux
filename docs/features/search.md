# Search

- Purpose: Describe the current capability and constraints of the search system.
- Audience: Anyone working on search, file navigation, or indexing.
- Authority: Canonical feature-level reality doc.
- Update when: Search UI, backends, or keyboard triggers change.
- Read next: `docs/reference/SHORTCUTS.md`

## What This Feature Is

Codemux provides two search modes: file name search (Ctrl+P) for quickly navigating to files, and content search (Ctrl+Shift+F) for finding text across the project. Both open as overlay dialogs.

## Current Model

File name search uses `fd` via the `searchFileNames` Tauri command. Content search uses `rg` (ripgrep) via the `searchInFiles` Tauri command. Both are debounced in the frontend and return limited result sets. Results can be opened in the built-in file editor or navigated to in the terminal.

## What Works Today

- File name search (Ctrl+P): fuzzy file finder with debounced input, opens results in editor tabs
- Content search (Ctrl+Shift+F): full-text search with regex toggle, case-sensitivity toggle, and match context
- Result limiting (20 files default for content search)
- Search results show file paths with match highlighting
- Content search results show matching lines with surrounding context

## Current Constraints

- No search-and-replace
- No search scoping (always searches from workspace root)
- No saved searches or search history
- Content search depends on `rg` being installed
- File search depends on `fd` being installed

## Important Touch Points

- `src/components/search/file-search-dialog.tsx` — file name search UI
- `src/components/search/content-search-dialog.tsx` — content search UI
- `src/hooks/use-keyboard-shortcuts.ts` — Ctrl+P and Ctrl+Shift+F bindings
- `src/tauri/commands.ts` — `searchFileNames`, `searchInFiles` wrappers
- `src-tauri/src/indexing.rs` — file indexing backend (40-line chunks, file watcher, debounced reindex)
