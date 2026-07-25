# Search

- Purpose: Describe the current capability and constraints of the search system.
- Audience: Anyone working on search, file navigation, or indexing.
- Authority: Canonical feature-level reality doc.
- Update when: Search UI, backends, or keyboard triggers change.
- Read next: `docs/reference/SHORTCUTS.md`

## What This Feature Is

Codemux provides two search modes: file name search (Ctrl+Shift+P) for quickly navigating to files, and content search (Ctrl+Shift+F) for finding text across the project. Both open as overlay dialogs.

## Current Model

File name search uses `fd` via the `searchFileNames` Tauri command. Content search uses `rg` (ripgrep) via the `searchInFiles` Tauri command. Both are debounced in the frontend and return limited result sets. Results open in the built-in file editor (`openEditorTab`). Neither dialog has any terminal integration.

## What Works Today

- File name search (Ctrl+Shift+P): file-name finder with debounced input, opens results in editor tabs
- Content search (Ctrl+Shift+F): full-text search with regex toggle, case-sensitivity toggle, and match context
- Result limiting — content search passes a 100-result cap (`content-search-dialog.tsx`), file-name search passes 20 (`file-search-dialog.tsx`); the backend defaults when unspecified are 100 and 50 respectively (`files.rs`)
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
- `src/hooks/use-keyboard-shortcuts.ts` — Ctrl+Shift+P and Ctrl+Shift+F bindings
- `src/tauri/commands.ts` — `searchFileNames`, `searchInFiles` wrappers
- `src-tauri/src/indexing.rs` — **not used by this feature**; the search dialogs go through `searchInFiles`/`searchFileNames` (rg/fd) only. See `docs/features/code-indexing.md`, whose commands have no frontend caller.
- (former indexing touch point) — file indexing backend (40-line chunks, file watcher, debounced reindex)
