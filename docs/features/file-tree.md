# File Tree

- Purpose: Describe the file tree panel in the right sidebar.
- Audience: Anyone working on file browsing or the right panel.
- Authority: Canonical feature-level reality doc.
- Update when: Tree behavior, file actions, or filtering changes.
- Read next: `docs/features/file-editor.md`, `docs/features/search.md`

## What This Feature Is

The file tree panel is a pane in the right-panel deck that shows the workspace directory structure. Users can browse, expand directories, and open files in the built-in editor.

It also renders as the optional 196px column inside a doc pane (the deck's file-explorer toggle) — same component, same lazy loading.

## Current Model

The tree uses lazy loading — only the root directory is fetched on mount, and subdirectories load on expand. Directory contents are cached in a `Map<path, FileEntry[]>` and expanded state is tracked in a `Set<path>`. Backend `listDirectory(path, showHidden)` returns entries with gitignore status.

## What Works Today

- Lazy-loaded directory expansion with loading spinners
- Recursive tree rendering with depth-based indentation (6 + depth * 11px)
- 22px rows with a 5px radius, folder names at 62% foreground and file names at 44% so the *shape* of the tree reads before any one name
- File type icons via `FileTypeIcon` component (extension-based)
- An 8% foreground fill on the selected row, driven by the optional `selectedPath` prop (the deck passes the file it last opened into a doc pane; a doc pane's own tree column passes its file)
- Gitignored items shown with reduced opacity (not hidden)
- Hidden files toggle (persisted in synced settings `file_tree.show_hidden_files`) — now a button in the deck's single row of panel chrome, not a header of this panel's own
- Refresh, also in that row; it bumps the panel's `refreshKey` prop, which clears the cache and reloads the tree
- Click file to open it. The default is a main-area CodeMirror editor tab (`openEditorTab`); the deck passes an `onOpenFile` override so a click inside the panel opens a `doc:` pane there instead
- Empty directory placeholder (`empty`); an empty root renders `No files`
- Folder and chevron icons for directories (chevron rotates on expand)
- ScrollArea for overflow handling

## Current Constraints

- **No per-file size column, deliberately.** The right-aligned `290B / 17K / 4K`
  numbers were the noisiest thing on a surface whose whole job is navigation,
  and they answered a question nobody asks of a file list. `FileEntry.size` is
  still returned by the backend and still drives other callers; the tree
  ignores it, and `file-tree-panel.test.tsx` fails if it comes back.
- No file virtualization (renders all visible nodes, may be slow for very large trees)
- No file actions beyond open (no rename, delete, copy path, or new file)
- No drag-and-drop for file operations
- No custom sort order (uses backend listing order)
- Cannot open files in external editor directly from tree
- No file search or filter within the tree (use Ctrl+Shift+P for file search)

## Important Touch Points

- `src/components/workspace/file-tree-panel.tsx` — tree panel component (props: `onOpenFile`, `refreshKey`, `selectedPath`)
- `src/components/layout/right-panel.tsx` — the deck that hosts it and owns its tab-row controls
- `src/components/icons/file-type-icon.tsx` — file extension icon mapping
- `src/lib/open-editor-tab.ts` — `openEditorTab()` helper for opening files
- `src-tauri/src/commands/files.rs` — `list_directory()` Tauri command
- `src/stores/synced-settings-store.ts` — `file_tree.show_hidden_files` setting
