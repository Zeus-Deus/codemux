---
title: File Editor
description: Built-in CodeMirror 6 editor for quick file edits with syntax highlighting, dirty tracking, and markdown preview.
---

# File Editor

Codemux includes a built-in file editor for quick edits and reviews without leaving the app. It's not a full IDE — it's for fast changes, reading code, and reviewing files alongside your terminal and diff views.

## Opening Files

Files open in editor tabs from several places:

- **File tree panel** — Click any file in the sidebar file tree
- **File search** — `Ctrl+P` to fuzzy-search filenames, Enter to open
- **Content search** — `Ctrl+Shift+F` to search file contents, click a result to open at the matching line
- **[Diff viewer](diff-viewer.md)** — Click the pencil icon in the diff toolbar to edit the current file

If the file is already open in another tab, Codemux activates that tab instead of opening a duplicate.

## Language Support

Syntax highlighting is provided via CodeMirror 6 language extensions. Supported languages:

| Language | Extensions |
|----------|------------|
| JavaScript | `.js`, `.mjs`, `.cjs` |
| TypeScript | `.ts` |
| JSX | `.jsx` |
| TSX | `.tsx` |
| Rust | `.rs` |
| Python | `.py` |
| Go | `.go` |
| HTML | `.html`, `.htm`, `.svelte`, `.vue` |
| CSS | `.css`, `.scss`, `.less` |
| JSON | `.json`, `.jsonc` |
| Markdown | `.md`, `.mdx` |
| YAML | `.yaml`, `.yml` |

Languages are lazy-loaded — only the extension for the current file is fetched. Files with unrecognized extensions open without highlighting.

Binary files (images, fonts, archives, compiled binaries, media) are detected and blocked with a "Binary file — cannot edit" message.

## Editing and Saving

The editor includes line numbers, bracket matching, active line highlighting, search (`Ctrl+F`), multi-cursor support, and line wrapping.

**Saving**: Press `Ctrl+S` to write changes to disk.

**Dirty tracking**: When you have unsaved changes, two indicators appear:

- A dot next to the filename in the editor toolbar, with a "Ctrl+S to save" hint
- A dot on the tab in the tab bar

Both clear when you save. The dirty state is tracked by comparing the current content against the last-saved baseline.

## Markdown Preview

Markdown files (`.md`, `.mdx`) open in **rendered mode** by default — a styled HTML preview with GitHub Flavored Markdown support (tables, checkboxes, strikethrough).

Toggle between views using the **Rendered** and **Raw** buttons in the toolbar. Raw mode shows the standard CodeMirror editor with markdown syntax highlighting.

Changes made in raw mode are reflected when you switch back to rendered view.

## Theming

The editor theme syncs with the rest of Codemux. Syntax highlighting colors are derived from the system ANSI palette, so the editor matches your terminal. Font is JetBrains Mono at 13px.

## Limitations

- Not a replacement for a full editor — no extensions, no LSP, no integrated terminal
- Maximum file size: 2 MB
- No multi-file editing features (find-and-replace across files, etc.)
- Scrollback and cursor position are not persisted across restarts
