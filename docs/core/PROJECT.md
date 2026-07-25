# Codemux Project

- Purpose: Durable product direction and architecture intent.
- Audience: Anyone making design or implementation decisions.
- Authority: Canonical product and architecture document.
- Update when: Product goals, core principles, or architecture boundaries change.
- Read next: `docs/core/STATUS.md`, `docs/core/PLAN.md`

## Product Summary

Codemux is a Linux-first AI coding workspace that keeps terminal work, browser testing, local automation, and long-running agent sessions inside one app.

## What Codemux Should Become

- a daily-driver workspace for terminal-based coding agents
- a multi-workspace, multi-pane terminal and browser environment
- a conversational chat surface for CLI coding agents alongside the terminal and browser
- a local control plane via CLI and socket APIs
- reachable from the user's other devices — the same workspace driven from a browser, and the same frontend driven against a remote machine's backend
- the flagship host for `OpenFlow`, a multi-agent orchestration system

## Product Principles

- Linux-first, not Linux-only
- backend state is the source of truth
- terminal, browser, and chat belong in the same workspace
- automation is a first-class feature
- memory and indexing stay local-first
- keep `OpenFlow` modular enough to embed elsewhere later
- avoid over-engineering before a solid Linux MVP exists

## Architecture Direction

- Rust owns the domain: workspaces, panes, sessions, automation, persistence, and the OpenFlow runtime
- React (Tailwind + shadcn) and Tauri own presentation and desktop-shell concerns
- browser support must work inside pane layouts and remain scriptable
- remote access composes as two orthogonal transports over the same code: a **UI transport** (a browser drives one running backend through the `__TAURI_INTERNALS__` seam — `docs/features/web-remote-access.md`) and a **compute transport** (the local frontend drives a remote machine's backend over SSH — `docs/features/remote-hosts.md`). Neither may fork app components; both are default-off
- OpenFlow should feel integrated inside Codemux while keeping a clean runtime boundary

## Linux MVP Shape

The first real Codemux MVP should support:

- multiple workspaces and pane splits
- stable terminal sessions
- one usable browser pane
- a conversational agent chat pane backed by CLI coding agents
- notifications and attention signals
- CLI and socket automation
- enough reliability to daily-drive with coding agents

## OpenFlow Direction

OpenFlow should be a first-class workspace or run type inside Codemux, while its core runtime stays modular enough to be reused outside the app later.
