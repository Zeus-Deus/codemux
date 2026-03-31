# File Editor

- Purpose: Describe the built-in file editor with CodeMirror and markdown preview.
- Audience: Anyone working on the editor, file tree, or tab system.
- Authority: Canonical file editor feature doc.
- Update when: Editor capabilities, supported languages, or file handling change.
- Read next: `docs/reference/FEATURES.md`, `docs/core/STATUS.md`

## What This Feature Is

A built-in code editor using CodeMirror 6 that opens files directly inside Codemux as editor tabs. Supports syntax highlighting for common languages and a rendered markdown preview mode. Files can be opened from the file tree panel or search results.

## Current Model

### Architecture

- **Editor engine**: CodeMirror 6 with language-specific extensions
- **Tab type**: Editor tabs sit alongside terminal and browser tabs in the tab bar
- **File I/O**: Read and write go through Tauri commands (`read_file`, `write_file`)
- **State**: `editor-store` (zustand, localStorage-persisted) tracks open files, dirty state, and baseline content
- **Theme**: Custom CodeMirror theme that reads from Codemux CSS variables for consistent dark mode

### Language Support

Language detection is path-based. Supported languages include:
JavaScript, TypeScript, JSX, TSX, HTML, CSS, JSON, Markdown, Python, Rust, Go, C, C++, Java, PHP, XML, YAML, TOML, SQL, Shell/Bash, and more.

### Markdown Preview

Markdown files get a rendered preview mode alongside the editor. The `MarkdownRendered` component renders markdown content with proper styling.

### File Size Limits

- Maximum file size: 2 MB
- Binary file detection via null byte check (binary files are rejected)
- UTF-8 only

### File Tree Integration

The file tree panel lists directories with `.gitignore` awareness. Clicking a file opens it in an editor tab. Common directories are auto-skipped: `node_modules`, `target`, `dist`, `build`, `__pycache__`, `.next`, `.nuxt`, `.output`, `vendor`.

## What Works Today

- Open files from file tree or search results as editor tabs
- Syntax highlighting for 20+ languages via CodeMirror
- Markdown rendered preview
- Dirty state tracking (modified indicator on tab)
- Save files back to disk via write command
- Custom dark theme matching Codemux shell
- File tree with gitignore filtering and common directory exclusion
- Search in files (via `rg` with `grep` fallback) and file name search (via `fd` with `find` fallback)
- Reveal file in system file manager

## Current Constraints

- No multi-cursor or advanced refactoring features
- No LSP integration (no autocomplete, go-to-definition, or diagnostics)
- 2 MB file size limit
- Binary files cannot be opened
- No file rename or delete from within the editor
- Editor tabs cleared from persistence on app reload (files must be re-opened)

## Important Touch Points

- `src/components/editor/EditorPane.tsx` — Main CodeMirror editor component
- `src/components/editor/MarkdownRendered.tsx` — Markdown preview renderer
- `src/lib/codemirror-theme.ts` — Custom CodeMirror theme from CSS variables
- `src/lib/editor-languages.ts` — Language detection and extension loading
- `src/lib/open-editor-tab.ts` — Helper to open files as editor tabs
- `src/stores/editor-store.ts` — Editor tab state (open files, dirty tracking)
- `src-tauri/src/commands/files.rs` — `read_file`, `write_file`, `list_directory`, `search_in_files`, `search_file_names`
- `src/components/workspace/file-tree-panel.tsx` — File tree with gitignore filtering
