# Codemux Project

## What Codemux Is

`codemux` is a Linux-first desktop workspace for AI-heavy software development.

The core idea is to build something in the spirit of `cmux`, but designed first for Linux and with a deliberate path to macOS and Windows later. It should feel like a serious daily-driver for developers who work with terminal-based coding agents such as OpenCode, Claude Code, Codex, Aider, Gemini CLI, and similar tools.

Codemux is not just a terminal emulator.

It is meant to be:

- a terminal workspace
- a browser-assisted coding environment
- a control plane for coding agents
- a future host for `OpenFlow`, a multi-agent autonomous workflow engine

## Core Product Goal

The user should be able to stay inside one app while building, testing, debugging, and reviewing software with agents.

That means Codemux eventually needs:

- multiple workspaces
- multiple panes per workspace
- terminal panes
- browser panes
- notifications/attention signals when an agent needs input
- external automation through CLI/socket APIs
- internal orchestration through `OpenFlow`

## Inspiration

The direct inspiration is `cmux`.

The important ideas copied from that product direction are:

- vertical workspace/task organization
- split panes
- embedded browser next to terminals
- notifications for agent attention
- scriptable workflows
- support for parallel coding-agent sessions

But Codemux has a different target path:

- Linux first
- Omarchy/Hyprland friendliness early
- cross-platform architecture from the beginning
- deeper built-in orchestration via `OpenFlow`

## OpenFlow Vision

`OpenFlow` is the long-term differentiator.

OpenFlow is the idea that a user should be able to say one thing like:

"build me a calendar booking site for a barbershop"

and the system should take it from there.

Instead of one agent doing everything in one terminal, Codemux should be able to run a swarm of cooperating agents with specialized roles.

Examples of roles:

- orchestrator
- planner
- builder
- reviewer
- tester
- debugger
- researcher

The intended loop is:

1. understand the goal
2. break it into phases
3. assign subagents
4. implement work
5. run tests/builds
6. use the embedded browser where relevant
7. review results
8. re-plan if needed
9. continue until done or blocked

This is not meant to be a toy demo.

It should become a serious autonomous development loop with:

- visibility
- control
- budgets
- permission boundaries
- pause/resume/cancel
- human approval points

## OpenFlow UX Direction

OpenFlow should not replace the normal Codemux workspace UI.

Best direction:

- keep normal Codemux workspaces for day-to-day terminal/browser work
- add `OpenFlow` as a special workspace mode or workspace type inside the same app
- let an OpenFlow run create and manage regular terminal/browser panes underneath
- add a visual orchestration layer on top that shows:
  - active agents
  - phase/plan state
  - messages between roles
  - current blockers
  - approvals needed
  - browser/test evidence

That means the user gets both:

- the normal Codemux pane experience
- an optional higher-level orchestration view

Recommended model:

- sidebar can show an `OpenFlow Runs` section
- selecting a run opens an OpenFlow workspace view
- that view can toggle between:
  - `Flow View` for visual orchestration
  - `Workspace View` for the underlying panes/sessions
- the handoff/memory workflow should guide future tools/sessions to read structured memory instead of replaying raw chat logs

This is better than making OpenFlow a completely separate app mode because:

- it keeps the mental model unified
- it preserves direct pane takeover by the user
- it makes debugging easier
- it avoids splitting the product into two disconnected experiences

So the design decision is:

- OpenFlow is a first-class workspace/run type inside Codemux, not a separate product and not a replacement for normal workspaces

## OpenFlow As A Modular Core

OpenFlow should be designed so it can live in two forms:

- inside Codemux as the flagship integrated experience
- as an embeddable orchestration core that other tools can adopt later

That does not mean splitting it into a separate product right now.

It means the architecture should separate:

- OpenFlow core runtime
- OpenFlow storage/state model
- OpenFlow APIs
- Codemux-specific UI and workspace integration

Recommended direction:

- keep the runtime and orchestration engine in Rust
- define clean contracts for workers, runs, artifacts, approvals, and memory
- let Codemux be one host of OpenFlow
- allow future hosts such as other CLIs, apps, or services to use the same engine

This is good because:

- it increases adoption potential
- it avoids locking OpenFlow only to the Codemux UI
- it keeps the orchestration engine testable and reusable
- it makes future integrations easier without rewriting the core

So the product decision should be:

- Codemux is the primary UX
- OpenFlow is architected like an embeddable engine
- do not over-optimize for external packaging yet, but keep the boundaries clean from the start

## Why Linux First

Linux is the best first target because:

- it matches the intended user environment today
- it works well with terminal-centric agent workflows
- it fits Hyprland/Omarchy usage patterns
- it lowers the complexity of the first shipping target

The first version should feel native enough on Linux to be worth daily-driving before large effort goes into cross-platform parity.

## Cross-Platform Strategy

Codemux is not Linux-only in ambition.

The architecture should leave a clean path for later support on:

- macOS
- Windows

The rule is:

- core domain state and orchestration live in Rust
- UI is replaceable enough to survive platform differences
- platform-specific process/browser/notification behavior should be abstracted behind modules or traits

The important thing is to avoid painting the project into a Linux-only corner.

## Technical Direction

Current stack direction:

- Rust backend
- Tauri 2 desktop shell
- Svelte frontend
- xterm.js terminal rendering
- portable-pty terminal backend

Why this stack is being used now:

- fastest path from current prototype to Linux MVP
- keeps browser UI iteration relatively easy
- preserves a path to other operating systems later
- avoids a risky full restart into another GUI stack too early

This may evolve later, but for now the priority is to ship a strong Linux MVP, not to endlessly restart architecture.

## Codebase Indexing

Codemux should eventually support local codebase indexing.

Why this matters:

- large repos are expensive to repeatedly re-scan with raw token context
- terminal agents often rely on ad hoc file search each run
- a local index can improve speed, reduce repeated work, and reduce token spend

Important clarification:

- MCP is not required to make indexing work
- MCP can expose an index as a tool boundary later, but indexing itself can be fully local
- the index should live inside Codemux or in a closely related local service

What indexing should eventually provide:

- fast file discovery
- symbol and definition lookup
- semantic-ish search over files/chunks
- cached repository metadata
- cheap retrieval for planners/reviewers/builders

Suggested approach:

- start with local lexical + metadata indexing
- then add embeddings for semantic retrieval
- keep everything local-first and incremental
- support hot updates using file watching
- store the first index in a simple project-local file before moving to heavier infrastructure

High-level architecture idea:

- repository watcher
- parser/chunker
- metadata index
- optional embedding pipeline
- local storage for chunks, symbols, and vectors
- retrieval API exposed to Codemux and later to OpenFlow

Likely implementation path:

- SQLite for metadata/chunk storage
- Tantivy or similar for fast text search
- optional local embeddings model or pluggable embedding providers later
- Rust-owned indexing core

The goal is not to replace agent reasoning. The goal is to make repo retrieval faster, cheaper, and more structured.

## Portable Agent Memory

Codemux should also support portable agent memory and handoff context.

The problem:

- one tool knows what the user is building
- another tool starts cold
- switching agents often means rewriting a huge prompt with project intent, status, decisions, and next steps

Codemux can solve this by acting as the shared memory layer around the workspace.

This should eventually include:

- project brief memory
- current goal memory
- recent decisions and constraints
- active task summaries
- workspace/session history
- structured handoff summaries for any external agent

Important clarification:

- this does not require MCP to exist
- MCP can be one transport later
- the core value is a local shared memory store plus retrieval API

Suggested direction:

- keep memory local-first
- store both human-authored and system-generated summaries
- let Codemux generate compact handoff packets for any tool
- let OpenFlow consume the same memory layer later
- do not store full raw chats by default; store compressed structured memory instead

Recommended first implementation:

- store project memory inside the project at `.codemux/project-memory.json`
- keep it human-readable and fast
- generate handoff packets from curated memory fields instead of replaying whole conversations

The goal is that a user can switch from one coding agent to another without re-explaining the whole project every time.

## Performance Expectations

Codemux should feel fast.

That means:

- quick startup
- low memory usage relative to typical Electron-style tooling
- smooth pane switching
- stable browser/terminal interaction
- responsiveness under multiple concurrent agent sessions

Performance matters because this app is supposed to become a central workspace, not a novelty tool.

## Product Principles

- Linux-first, not Linux-only
- keyboard-first workflow
- backend state is the source of truth
- automation is a first-class feature
- browser and terminal belong in the same workspace
- observability matters from early on
- safety matters for autonomous features
- ship the MVP before chasing full parity with everything

## What the MVP Must Become

The first real Codemux MVP should support:

- multiple workspaces
- multiple terminal sessions
- split panes
- sidebar navigation
- agent attention signals
- one stable embedded browser pane
- CLI/socket automation

At that point, a developer should be able to use it daily with terminal-based coding agents.

## What OpenFlow MVP Must Become

OpenFlow MVP should come after the workspace/browser foundations are stable.

OpenFlow MVP should support:

- one-shot high-level task input
- decomposition into phases/subtasks
- spawning multiple agent workers
- iterative build/test/review loops
- browser-based validation where relevant
- user monitoring and intervention

## Current Development Rule

Follow `PLAN.md` phase by phase.

When continuing this project in future chats, treat these two files as the shared project memory:

- `PROJECT.md` for product intent, architecture direction, and vision
- `PLAN.md` for execution order and implementation progress

## Current State

At the time of writing this file, the project has:

- a Rust app-state model
- a multi-session terminal foundation
- session creation/activation/restart/close behavior
- a prototype sidebar/workspace shell in progress

It does not yet have:

- full workspace management
- real split-pane rendering UI
- browser panes
- notifications system
- CLI/socket automation layer
- OpenFlow runtime

## Short Version

Codemux is meant to become the Linux-first AI coding workspace that combines:

- terminal multiplexing
- embedded browser testing
- agent orchestration
- future autonomous software-building workflows

The user should be able to tell it what they want once, then watch a structured swarm of agents work inside a workspace that was built for that exact kind of loop.
