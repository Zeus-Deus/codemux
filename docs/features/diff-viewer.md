# Diff Viewer

- Purpose: Describe the full-tab diff viewer that opens from the Changes panel, file tree, or commit history.
- Audience: Anyone working on git diff rendering, diff navigation, or the section/focus-mode controls.
- Authority: Canonical feature doc for the diff viewer tab surface.
- Update when: Layout modes, section filters, navigation, or base-branch comparison behavior change.
- Read next: `docs/features/changes-panel.md`, `docs/features/file-editor.md`

## What This Feature Is

The Diff Viewer is a full-pane tab surface (separate from the right-sidebar Changes panel) that renders a git diff with navigation, view modes, and section filtering. It's the surface the Changes panel opens into when you click a file, and it's also how "against base branch" previews and per-commit diffs are shown inline inside a workspace.

## Current Model

Diff tabs are regular workspace tabs with `kind: "diff"`, and each one gets its own persisted UI state (layout, section, file selection) keyed by tab ID. The backend supplies the data via `getGitStatus`, `getGitDiff`, `getBaseBranchDiff`, and `getBaseBranchFileDiff`; the frontend parses the unified-diff output with `parseDiff` and renders either a unified or split view. The diff store is persisted to `localStorage` so the user's layout and section choices survive reloads.

The toolbar carries all of the diff-specific controls: file path, edit-file button (opens the file in the built-in CodeMirror editor), section selector (when focus mode is on), layout toggle (unified / split), hunk navigation (prev/next change), file navigation (prev/next file with a `1/N` indicator), focus-mode toggle, and a close button.

## Sections

A diff tab can filter its file list by section:

| Section | What it shows |
|---------|---------------|
| `all` | Every changed file in the working tree (default) |
| `staged` | Files staged for commit |
| `unstaged` | Files with working-tree changes not yet staged |
| `against_base` | Files that differ between the current branch tip and the workspace's base branch (vs `main`, `master`, or whatever the workspace's `default_base_branch` is) |

`against_base` mode is the "PR preview" view — it fetches `getBaseBranchDiff` and `getBaseBranchFileDiff` so the user can see the full diff that a PR would contain before opening it. When `against_base` is active outside focus mode, the toolbar shows a `vs <base-branch>` indicator.

## Layouts

Two rendering modes:

- **Unified** (default): one-column view with `-` and `+` lines interleaved inside each hunk. Hunk headers are shown as muted separators. Uses a fixed `18px` line height so `scrollToHunk` can compute target offsets by index.
- **Split**: two-column side-by-side view with old lines on the left and new lines on the right.

Both views use a monospace font and share the same `DiffViewHandle` interface so the toolbar's hunk navigation works regardless of layout.

## Focus Mode

Focus mode is a toolbar toggle that adds the section selector strip and isolates the user's attention on a narrower workflow:

- section buttons (`staged` / `unstaged` / `vs <base>` / `all`) appear in the toolbar
- the viewer hides the sidebar file list visual noise and focuses on the currently-selected file
- preferred for quick "am I ready to commit?" or "what does this PR contain?" passes

Focus mode state is per-tab — one diff tab can be in focus mode while another is not.

## Navigation

- **Hunk navigation** (`prev change` / `next change`): scroll the viewer to the next hunk header. The unified view computes target offsets via `line index * 18px`; the split view exposes the same handle.
- **File navigation** (`prev file` / `next file`, `1/N` counter): step through the filtered file list, updating `fileIndex` in the diff store. Disabled when there's only one file.
- **Edit file**: opens the current file in the built-in CodeMirror editor via `openEditorTab`.

## Opening a Diff Tab

Diff tabs are created via `createTab(workspaceId, { kind: "diff" })`. Entry points:

- Clicking a file in the Changes panel (right sidebar)
- Clicking a commit in the commit history to see the full commit diff
- Clicking a file in the "against base" sidebar section

Each tab initializes its state in the diff store on mount if none exists for that `tabId`.

> The standalone "Diff Viewer" new-tab (`+`) menu item and the "Open Diff
> Viewer" command-palette action were removed in `v0.7.5` (`283660a`): both
> opened a diff tab with no file selected, landing on an empty "Select a file
> to view changes" placeholder and duplicating the Changes-panel flow. The
> diff tab kind and all diff infrastructure stay in place — only the redundant
> file-less entry points are gone.

## What Works Today

- Unified and split layouts with per-tab persistence
- Four section filters: `all`, `staged`, `unstaged`, `against_base`
- Base-branch comparison with inline toolbar indicator
- Hunk-level navigation (prev/next change)
- File-level navigation with `1/N` counter
- Focus mode with inline section selector
- Edit-file button that opens the file in the built-in editor
- Auto-refreshing file list (5s polling on top of `app-state-changed`)
- Parse and render ANSI-free unified diff output from `git diff`
- Per-tab state persisted to `localStorage` (survives reloads)

## Current Constraints

- No inline staging (cannot stage/unstage from the diff tab — must go through the Changes panel)
- No inline conflict resolution (goes through the AI merge resolver or manual terminal workflow)
- No hunk-level staging (whole files only)
- No word-level diff highlighting inside changed lines
- Diff tab state survives reloads but not app reinstalls (localStorage scope)
- Unified-view hunk nav assumes a fixed 18px line height — any future line-height change needs to update the constant

## Important Touch Points

- `src/components/diff/DiffPane.tsx` — top-level diff tab container, fetches data, wires up navigation
- `src/components/diff/DiffToolbar.tsx` — toolbar with layout, section, navigation, focus mode, edit, close
- `src/components/diff/DiffUnifiedView.tsx` — unified-view renderer with `scrollToHunk` imperative handle
- `src/components/diff/DiffSplitView.tsx` — split-view renderer
- `src/stores/diff-store.ts` — zustand store with per-tab persisted state
- `src/lib/diff-parser.ts` — `parseDiff()` converts `git diff` output into `DiffLine[]`
- `src-tauri/src/git.rs` — backend diff commands (`get_git_diff`, `get_base_branch_diff`, `get_base_branch_file_diff`)
- `src-tauri/src/commands/git.rs` — Tauri command wrappers
- `src/lib/open-editor-tab.ts` — opens the selected file in the CodeMirror editor
