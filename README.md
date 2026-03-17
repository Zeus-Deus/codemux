# Codemux

Codemux is a Linux-first AI coding workspace built around terminal agents, browser-assisted testing, and local automation.

It combines:

- multi-session terminals
- browser panes for in-workspace testing
- split layouts and workspace navigation
- attention and notification signaling for agents
- local CLI and socket control
- local project memory and indexing

Long term, it is also meant to host `OpenFlow`, a multi-agent orchestration system built into the workspace.

## Current Status

Codemux is useful as a serious prototype workspace, but it is not ship-ready yet.

Solid enough to treat as real product surface today:

- workspace shell and sidebar
- multi-session terminals
- pane splits, resizing, close, swap, and restore
- notifications and attention badges
- local project memory and lexical indexing
- local CLI and socket control basics

Still prototype-level or in active hardening:

- browser pane interaction fidelity and validation
- browser automation polish around the visible pane
- OpenFlow reliability, browser integration, and intervention flow
- Linux release-readiness validation and polish

For the current source of truth, read `docs/core/STATUS.md`.

## Run

Install dependencies:

```bash
npm install
```

Run the standard verification pass:

```bash
npm run verify
```

Run the app:

```bash
npm run tauri:dev
```

X11 fallback when native Wayland is not working well:

```bash
npm run tauri:dev:x11
```

## Browser And Agent Workflow

When working inside Codemux, use explicit browser subcommands such as:

```bash
codemux browser create
codemux browser open http://localhost:3000
codemux browser snapshot
```

See `AGENTS.md` for agent behavior rules and `docs/reference/CONTROL.md` for protocol details.

## Docs

- `WORKFLOW.md`: first file for a fresh coding session
- `docs/INDEX.md`: canonical internal docs hub
- `docs/core/PROJECT.md`: durable product direction
- `docs/core/STATUS.md`: current repo reality
- `docs/core/PLAN.md`: roadmap and build order
- `docs/core/TESTING.md`: verification policy
