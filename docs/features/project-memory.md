# Project Memory

- Purpose: Describe the current capability and constraints of the project memory system.
- Audience: Anyone working on memory, handoff, or agent context features.
- Authority: Canonical feature-level reality doc.
- Update when: Memory entry types, storage limits, or handoff behavior changes.
- Read next: `docs/features/session-persistence.md`, `docs/reference/CONTROL.md`

## What This Feature Is

The project memory system persists structured context across coding sessions. It stores project goals, decisions, next steps, and session summaries to support agent handoff and continuity.

## Current Model

Memory is stored as a JSON file at `{project_root}/.codemux/project-memory.json`. Each entry has a kind, source (human or system), content, optional tags, and a timestamp. The system enforces per-type capacity limits with FIFO trimming.

## What Works Today

### Entry Types

- **Pinned Context** (max 24): important code patterns, architecture notes, or reference information
- **Decisions** (max 40): technical choices and rationale
- **Next Steps** (max 24): planned work and TODOs
- **Session Summaries** (max 40): tool/session-scoped summaries for handoff continuity

### Project Metadata

- Project brief, current goal, current focus, constraints list

### CLI Commands

- `codemux memory show` — display entire project memory
- `codemux memory set [--brief TEXT] [--goal TEXT] [--focus TEXT] [--constraint TEXT...]` — update project metadata
- `codemux memory add <kind> <content> [--tool NAME] [--session LABEL] [--tag TAG...]` — add a memory entry

### Handoff

- `codemux handoff` — generates a compact summary (8 recent items per type, 4 session summaries, plus metadata) for AI context handoff between sessions

## Current Constraints

- No frontend drawer/panel UI (backend-only, CLI/socket access)
- Entries are immutable once created (FIFO replacement when limits are reached)
- No search within memory entries
- Auto-excluded from git via `.gitignore`

## Important Touch Points

- `src-tauri/src/memory.rs` — memory store, entry types, handoff packet generation
- `src-tauri/src/cli.rs` — CLI `memory` and `handoff` subcommands
- `.codemux/project-memory.json` — persisted memory file
