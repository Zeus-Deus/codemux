# File Tree

- Purpose: Describe the file tree panel in the right sidebar.
- Audience: Anyone working on file browsing or the right panel.
- Authority: Canonical feature-level reality doc.
- Update when: Tree behavior, file actions, or filtering changes.
- Read next: `docs/features/file-editor.md`, `docs/features/search.md`

## What This Feature Is

The file tree panel is a tab in the right sidebar that shows the workspace directory structure. Users can browse, expand directories, and open files in the built-in editor.

## Current Model

The tree uses lazy loading — only the root directory is fetched on mount, and subdirectories load on expand. Directory contents are cached in a `Map<path, FileEntry[]>` and expanded state is tracked in a `Set<path>`. Backend `listDirectory(path, showHidden)` returns entries with gitignore status.

## What Works Today

- Lazy-loaded directory expansion with loading spinners
- Recursive tree rendering with depth-based indentation (8 + depth * 16px)
- File type icons via `FileTypeIcon` component (extension-based)
- File size display (KB or B)
- Gitignored items shown with reduced opacity (not hidden)
- Hidden files toggle (persisted in synced settings `file_tree.show_hidden_files`)
- Refresh button to clear cache and reload tree
- Click file to open in built-in CodeMirror editor tab
- Empty directory placeholder ("(empty)")
- Folder and chevron icons for directories (chevron rotates on expand)
- ScrollArea for overflow handling

## Current Constraints

- No file virtualization (renders all visible nodes, may be slow for very large trees)
- No file actions beyond open (no rename, delete, copy path, or new file)
- No drag-and-drop for file operations
- No custom sort order (uses backend listing order)
- Cannot open files in external editor directly from tree
- No file search or filter within the tree (use Ctrl+P for file search)

## Important Touch Points

- `src/components/workspace/file-tree-panel.tsx` — tree panel component
- `src/components/icons/file-type-icon.tsx` — file extension icon mapping
- `src/lib/open-editor-tab.ts` — `openEditorTab()` helper for opening files
- `src-tauri/src/commands/files.rs` — `list_directory()` Tauri command
- `src/stores/synced-settings-store.ts` — `file_tree.show_hidden_files` setting
