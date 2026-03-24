---
title: Diff Viewer
description: Split and unified diff views with hunk navigation, focus mode, and conflict marker highlighting.
---

# Diff Viewer

The diff viewer displays file changes in a dedicated tab. Open it by clicking a file in the Changes panel.

## View Modes

Toggle between two layouts using the toolbar buttons:

- **Unified view** — Traditional single-column diff with `+`/`-` prefixes and line numbers on both sides
- **Split view** — Side-by-side comparison with synchronized scrolling. Deletions on the left, additions on the right.

The selected layout persists per tab.

## File Navigation

When viewing diffs, navigate between changed files:

- **Previous / Next file** buttons in the toolbar
- File position indicator showing `X/Y` (e.g., "3/7")
- Files can be filtered by section: Staged, Unstaged, or All

## Hunk Navigation

Jump between change hunks (the `@@` markers in a diff):

- **Previous hunk** (ChevronUp) — Jump to the previous change block
- **Next hunk** (ChevronDown) — Jump to the next change block
- Smooth scrolling between hunks

## Focus Mode

Toggle focus mode to review one section at a time. When enabled, section selector buttons appear: **Staged**, **Unstaged**, **All**. This filters the file list to the selected section.

## Conflict Marker Highlighting

When viewing files with merge conflicts, the diff viewer highlights conflict markers with distinct styling:

| Marker | Style | Label |
|--------|-------|-------|
| `<<<<<<< HEAD` | Blue background + left border | **OURS** |
| `=======` | Gray background + left border | — |
| `>>>>>>> branch` | Purple background + left border | **THEIRS** |

This works in both unified and split views.

## Opening the Diff Viewer

- **Click** a file in the Changes panel to open it in the diff tab
- **Alt+Click** to expand the inline diff preview instead
- The diff viewer is a full tab — it persists when switching between files
