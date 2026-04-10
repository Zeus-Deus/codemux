# Code Indexing

- Purpose: Describe the current capability and constraints of the local code indexing system.
- Audience: Anyone working on search, indexing, or code intelligence features.
- Authority: Canonical feature-level reality doc.
- Update when: Indexing behavior, supported formats, or search capabilities change.
- Read next: `docs/features/search.md`, `docs/reference/CONTROL.md`

## What This Feature Is

The indexing system creates a searchable local index of project source files. It enables full-text keyword search across the workspace codebase via CLI, socket API, or the search UI.

## Current Model

The indexer scans the project root, chunks source files into 40-line blocks, extracts symbol definitions (functions, structs, classes, etc.), and writes a JSON index to `.codemux/index.json`. A file watcher auto-rebuilds the index on a 2-second debounce when files change.

## What Works Today

- Full-text lexical search with match-count ranking and filename weighting
- Symbol extraction for function, struct, class, enum, trait, type, interface, and const definitions
- Automatic file watching with debounced rebuild
- Exclusion of common non-source directories (.git, node_modules, target, dist, build, .svelte-kit, .codemux)
- CLI commands: `codemux index build`, `codemux index status`, `codemux index search <query>`
- Search results include file path, line range, code snippet, matched symbols, and relevance score

## Current Constraints

- Individual files limited to 512 KB
- Total index size capped at 50 MB (stops scanning when reached)
- Chunk size fixed at 40 lines
- Default search limit: 12 results (configurable via `--limit`)
- Lexical search only (no semantic/embedding-based search)
- Supported extensions: rs, ts, tsx, js, jsx, svelte, md, json, toml, yaml, yml, py, go, java, c, cpp, h, hpp, css, html, txt, sh

## Important Touch Points

- `src-tauri/src/indexing.rs` — index builder, file watcher, search engine
- `src-tauri/src/cli.rs` — CLI `index` subcommands (build, status, search)
- `.codemux/index.json` — persisted index (auto-excluded from git)
