# Codemux Plan

Build `codemux`: a Linux-first, high-performance terminal workspace inspired by `cmux`, with a long-term path to macOS and Windows support, plus `OpenFlow`, a built-in multi-agent orchestration system that can plan, build, test, review, and iterate on software inside the app.

This plan assumes:

- Linux is the first-class target, especially Arch/Hyprland/Omarchy.
- Cross-platform support is designed in from day one, but shipped later.
- The current codebase is a prototype seed, not a finished foundation.
- We optimize for a working Linux MVP first, then grow toward full `cmux` parity and `OpenFlow`.

## Can we continue from the current code?

Yes, but only as a prototype base.

What is worth keeping:

- `Tauri 2 + SvelteKit + Rust` app shell
- `portable-pty` terminal backend idea
- `xterm.js` terminal pane prototype
- Omarchy theme integration concept

What must be refactored before serious feature work:

- single global PTY state -> multi-session architecture
- single terminal view -> workspace/pane tree model
- startup error handling -> robust lifecycle/state system
- Omarchy-only assumptions -> optional Linux theme integration with fallback defaults
- ad hoc commands/events -> typed app domain and event bus

Recommendation: continue from this repo, but treat the next stage as an architectural rebuild inside the same project rather than incremental feature patching.

## Product Vision

`codemux` should become:

- a native-feeling desktop workspace for AI-heavy coding
- a multi-pane terminal + browser environment
- a control plane for coding agents running in terminals
- a scriptable automation tool with CLI/socket APIs
- a host for `OpenFlow`, where one high-level prompt can trigger a managed swarm of specialized agents until a task is complete

## Core Product Pillars

- Fast startup, low memory, responsive panes
- Keyboard-first workflow
- Strong Linux support first, cross-platform abstractions early
- Browser embedded in the same workspace as terminals
- Agent visibility: know which pane/session/agent needs attention
- Automation-first design: everything important should be scriptable
- Safety: human approval points, budget limits, permission boundaries, logs

## Recommended Technical Direction

Keep this stack for now:

- Rust: backend, process/session management, workspace state, automation APIs, OpenFlow runtime
- Tauri 2: desktop shell and cross-platform app packaging
- SvelteKit: UI shell, pane layout UI, sidebar, notifications, browser controls
- xterm.js: terminal rendering for MVP
- platform webview via Tauri: embedded browser surface for MVP
- Tokio: async runtime
- serde/toml/sqlite: config, persistence, state

Do not restart in `iced` right now.

Reason:

- current repo already uses Tauri/Svelte/Rust
- Linux MVP is faster to ship with current stack
- browser embedding and app UI iteration are easier here
- it preserves a cleaner path to Windows/macOS later than a fresh experimental native GUI rewrite

Revisit a deeper native shell only after a successful Linux MVP proves where Tauri is or is not limiting.

## Target Architecture

### 1. Rust backend owns the domain

Rust should own:

- workspaces
- pane tree state
- terminal sessions
- browser session registry
- notifications and unread state
- automation/socket APIs
- persistence
- OpenFlow orchestration runtime

### 2. Svelte frontend owns presentation

Frontend should render:

- sidebar
- pane chrome
- split layout UI
- terminal pane host
- browser pane host
- notifications panel
- OpenFlow monitor UI
- settings and logs

Frontend should not own business truth; it should subscribe to backend state.

### 3. Cross-platform abstraction layer

Create backend traits/modules early for:

- shell spawning
- file watching
- notifications
- keyboard shortcut registration
- browser integration
- OS-specific paths/config discovery

Linux implementation ships first; macOS/Windows implementations are added later without rewriting the app domain.

## Delivery Phases

## Phase 0 - Research, scope, and architecture baseline

- [x] Analyze current prototype and identify what is salvageable
- [x] Compare against real `cmux` feature set and likely failure points
- [x] Decide Linux-first strategy with future macOS/Windows support
- [x] Decide to continue from current repo with internal architectural refactor
- [ ] Write architecture decision records for key choices
- [ ] Define MVP vs post-MVP vs long-term roadmap in the repo docs

Exit criteria:

- clear scope exists
- stack choice is locked for MVP
- repo contains an actionable roadmap and architecture notes

## Phase 1 - Stabilize the current prototype

- [x] Replace `unwrap`/crash-prone startup paths in Rust with proper error handling
- [x] Add visible error states in UI for terminal startup/theme failures
- [x] Add structured logging for startup, PTY lifecycle, and frontend mount
- [x] Buffer PTY output until frontend listeners attach
- [x] Support shell exit detection and status reporting
- [x] Reapply terminal theme on live theme changes
- [x] Make Omarchy theme optional with sane fallback theme
- [ ] Clean generated/build artifacts out of the repo if needed and tighten `.gitignore`
- [x] Replace starter `README.md` with actual project description

Exit criteria:

- app starts reliably
- single terminal works consistently
- failure states are visible and debuggable
- theming no longer breaks core usability

## Phase 2 - Build the real app domain model

- [x] Add backend app state module for `AppState`
- [x] Add `WorkspaceId`, `PaneId`, `SessionId`, `BrowserId` types
- [x] Define pane tree data model for horizontal/vertical splits
- [x] Define workspace model with active pane, active surface, metadata
- [x] Add typed event bus for backend -> frontend state updates
- [x] Add command routing by session/pane ID instead of singleton globals
- [x] Add persistence schema for layout and metadata
- [x] Define versioned config format

Exit criteria:

- backend can represent many workspaces and many panes cleanly
- no singleton PTY assumptions remain in the architecture
- frontend can subscribe to app state snapshots/deltas

## Phase 3 - Multi-terminal foundation

- [x] Refactor PTY backend to manage multiple terminal sessions
- [x] Create terminal session manager with create/close/restart/resize/write APIs
- [x] Track cwd, shell, title, exit code, environment, and state per session
- [x] Support per-pane scrollback policies
- [x] Add session reconnect rules for frontend remounts
- [x] Add resource limits and cleanup for closed sessions
- [x] Add tests for terminal manager and lifecycle behavior

Exit criteria:

- multiple terminal panes can exist independently
- sessions survive UI remounts where appropriate
- session cleanup is reliable

## Phase 4 - Workspace shell and sidebar

- [x] Build app shell UI around backend state
- [x] Add sidebar with workspaces/surfaces list
- [x] Show cwd, branch, notification badge, and latest agent state in sidebar
- [x] Add create/rename/close workspace flows
- [x] Add keyboard navigation across workspaces/surfaces
- [x] Add empty states and loading states

Exit criteria:

- app feels like a workspace manager, not a single terminal tab
- sidebar is functional and state-driven

## Phase 5 - Splits and pane management

- [x] Implement pane tree rendering in Svelte
- [x] Support split right/split down operations
- [x] Support pane focus movement and active pane highlighting
- [x] Support pane close, replace, move, and swap operations
- [x] Keep layout state synchronized with backend
- [x] Add drag handles and keyboard resizing
- [x] Add serialization/restoration of pane layouts

Exit criteria:

- arbitrary terminal split layouts work reliably
- layout persists across relaunches

## Phase 6 - Notifications and attention system

- [x] Define notification model with pane/workspace scope
- [x] Parse terminal notification signals and manual notify commands
- [x] Add pane highlight rings and sidebar badges
- [x] Add notifications panel/history
- [x] Add rules for focused vs unfocused workspace behavior
- [x] Add desktop notification integration on Linux
- [x] Add notification sound policy/configuration

Exit criteria:

- agents can reliably signal for attention
- user can identify which pane/workspace needs input immediately

## Phase 7 - Browser pane MVP

- [x] Add browser pane type to the pane tree
- [x] Implement browser surface embedding using Tauri-compatible webview capabilities
- [x] Add address bar, back/forward/reload, open URL
- [x] Persist URL and basic navigation state
- [x] Support split terminal + browser layouts
- [x] Handle focus, resize, and redraw lifecycle carefully
- [x] Add screenshot capability
- [x] Add browser error/loading states

Exit criteria:

- browser pane is stable inside split layouts
- a user can run a dev server in one pane and test it in another

## Phase 8 - Browser automation API

- [x] Define browser automation command model
- [x] Add open URL API
- [x] Add DOM snapshot / accessibility tree API where supported
- [x] Add click/type/fill/scroll/evaluate APIs
- [x] Add screenshot and console log capture APIs
- [x] Add permission/safety boundaries for automation commands
- [x] Add durable error reporting for failed automation steps

Exit criteria:

- an agent can inspect and act on the embedded browser programmatically
- browser automation is scriptable through backend APIs

## Phase 9 - CLI and socket automation layer

- [x] Add `codemux` CLI binary/subcommands
- [x] Add local socket server for app control
- [x] Support commands for workspace creation, pane splitting, focus, send keys, notify, open URL
- [x] Add machine-readable JSON output mode
- [x] Add auth/permissions model for local control
- [x] Add protocol versioning
- [x] Document example integrations for OpenCode, Claude Code, Codex, Aider

Exit criteria:

- app can be fully driven externally
- coding agents can control codemux without UI clicking

## Phase 9.5 - Codebase indexing

- [x] Define indexing scope: lexical only vs hybrid semantic retrieval
- [x] Define repository/chunk/symbol storage schema
- [x] Build local file watcher and incremental reindex pipeline
- [x] Add fast text retrieval for files, chunks, and symbols
- [x] Evaluate optional embeddings for semantic search
- [x] Expose retrieval API to Codemux and future OpenFlow workers
- [x] Add budget/performance constraints so indexing stays fast and cheap

Exit criteria:

- local repositories can be indexed incrementally
- retrieval is faster and cheaper than repeated broad file scans
- indexing is local-first and does not require MCP

## Phase 9.6 - Portable agent memory

- [x] Define shared memory model for project brief, current goal, decisions, and handoff state
- [x] Add local memory storage for workspace summaries and agent session context
- [x] Build handoff summary generation for switching between tools/agents
- [x] Expose retrieval API for memory lookups and compact context packets
- [x] Allow human-pinned context and system-generated context to coexist
- [x] Define privacy and retention rules for stored memory

Exit criteria:

- switching between tools does not require rewriting the whole project prompt
- shared project memory is local-first and reusable by Codemux and future OpenFlow workers

## Phase 10 - OpenFlow core design

- [x] Define what `OpenFlow` is at the product level
- [x] Define agent roles: orchestrator, planner, builder, reviewer, tester, debugger, researcher
- [x] Define shared task graph and memory model
- [x] Define phase loop: plan -> execute -> verify -> review -> replan
- [x] Define stop conditions, success criteria, timeout/budget limits
- [x] Define human approval checkpoints
- [x] Define artifact model: plans, logs, screenshots, diffs, review notes
- [x] Define security model for agent permissions

Exit criteria:

- OpenFlow is specified as an engine, not just an idea
- roles, state, and loop behavior are documented clearly

## Phase 11 - OpenFlow runtime MVP

- [x] Build backend orchestration engine for multi-agent runs
- [x] Add run state machine with resumable tasks
- [x] Add worker abstraction for terminal-based agents
- [x] Add planner worker contract
- [x] Add builder worker contract
- [x] Add reviewer/tester worker contract
- [x] Add shared context/memory store
- [x] Add run timeline and logs
- [x] Add basic retry, backoff, and failure recovery

Exit criteria:

- one high-level request can spawn multiple coordinated agent tasks
- each run is observable, resumable, and auditable

## Phase 12 - OpenFlow + Codemux integration

- [x] Allow OpenFlow to create workspaces and panes automatically
- [x] Launch one terminal agent per role or per task branch
- [x] Link browser panes to active workspaces/tasks
- [x] Feed browser screenshots/DOM snapshots back into the orchestration loop
- [x] Show run progress in sidebar and workspace UI
- [x] Surface review verdicts, blockers, and pending approvals
- [x] Allow pause/resume/cancel of live flows
- [x] Allow user takeover of any pane at any time

Exit criteria:

- a single user prompt can create a working multi-agent workspace
- user can watch the swarm operate and intervene when needed

## Phase 13 - OpenFlow autonomous dev loop

- [x] Implement iterative phased execution loop
- [x] Require verification after each build phase
- [x] Run browser-based smoke checks where relevant
- [x] Run project tests/builds/lints where relevant
- [x] Have reviewer agent score output and request fixes
- [x] Replan based on failures, regressions, or incomplete requirements
- [x] Stop only on success, budget exhaustion, policy stop, or user approval point

Exit criteria:

- OpenFlow can repeatedly plan/build/test/review until completion or a hard stop
- browser-based validation is part of normal flow

## Phase 14 - Quality, observability, and safety

- [x] Add structured logs across backend, UI, browser automation, and OpenFlow runs
- [x] Add crash reporting hooks for release builds
- [x] Add metrics for startup time, memory, pane counts, browser operations, run durations
- [x] Add feature flags for unstable capabilities
- [x] Add permission prompts for risky actions
- [x] Add config for model budgets, concurrency, auto-apply, and approval behavior
- [x] Add replay/debug tooling for failed OpenFlow runs

Exit criteria:

- the app is debuggable in production-like use
- autonomous runs are observable and bounded

## Phase 15 - Linux polish and release readiness

- [ ] Ship stable Hyprland/Omarchy experience
- [ ] Add package/distribution plan for Arch first
- [ ] Improve fonts, theme compatibility, keybindings, and clipboard behavior
- [ ] Add session restore for layout and metadata
- [ ] Add docs for agent integrations and OpenFlow workflows
- [ ] Add benchmarks and performance budgets
- [ ] Publish Linux alpha/beta releases

Exit criteria:

- Linux users can daily-drive it for agent-heavy work
- core workflows are documented and supportable

## Phase 16 - Cross-platform expansion preparation

- [ ] Audit Linux-specific assumptions in paths, shells, notifications, shortcuts, browser behavior
- [ ] Move platform-specific code behind traits/modules
- [ ] Add platform capability matrix to docs
- [ ] Add CI strategy for Linux/macOS/Windows builds
- [ ] Decide on Windows shell strategy: PowerShell, cmd, WSL, Git Bash support boundaries
- [ ] Decide on macOS browser/notification/focus behavior mapping

Exit criteria:

- codebase is ready for OS-specific implementations without domain rewrites

## Phase 17 - macOS support

- [ ] Implement macOS shell/session integration
- [ ] Implement macOS notifications and shortcut handling
- [ ] Validate browser embedding behavior on macOS
- [ ] Fix focus, resize, and clipboard edge cases
- [ ] Ship limited macOS preview

Exit criteria:

- macOS preview build can run core codemux workflows

## Phase 18 - Windows support

- [ ] Implement Windows process/session integration
- [ ] Support PowerShell and basic shell profiles
- [ ] Decide and document WSL integration story
- [ ] Implement Windows notifications and shortcut handling
- [ ] Validate browser pane behavior and automation on Windows
- [ ] Ship limited Windows preview

Exit criteria:

- Windows preview supports at least core terminal + workspace + browser workflows

## OpenFlow Functional Requirements

OpenFlow should eventually support:

- one-shot task entry from the user
- automatic decomposition into subgoals/phases
- spawning multiple specialized agent workers
- browser-driven verification
- repo-aware planning and code review
- pause/resume/cancel/restart
- human-in-the-loop approvals
- budget controls and concurrency controls
- detailed run history and replay

## Non-Functional Requirements

- startup should feel instant or near-instant for normal sessions
- pane operations should feel smooth under heavy load
- browser pane should stay stable during resize/split operations
- many concurrent terminals should remain responsive
- OpenFlow should never block the whole UI thread
- all long-running actions must be cancellable
- state corruption on crash should be minimized with snapshots/journaling

## Risks

- browser embedding inside complex pane layouts can be tricky across platforms
- automation APIs differ by platform/webview backend
- agent orchestration can become expensive, noisy, and hard to debug without strong observability
- terminal focus/input edge cases are surprisingly deep
- session restore is easy to overpromise and hard to do safely
- cross-platform parity will take much longer than Linux MVP

## What not to do

- Do not chase full macOS/Windows parity before Linux MVP works well
- Do not overbuild OpenFlow before the workspace, browser, and automation foundations exist
- Do not let frontend state become the source of truth for sessions/panes
- Do not keep singleton terminal architecture any longer than needed
- Do not rewrite the whole app into a different GUI stack yet

## Immediate Next Steps

- [x] Replace the current prototype README with project vision and scope
- [x] Refactor terminal backend into a session manager
- [x] Add app domain/state layer in Rust
- [x] Add typed backend -> frontend state sync
- [x] Build a basic sidebar backed by real workspace state
- [ ] Ship a stable Linux MVP with multiple terminals before starting browser automation

## Definition of MVP

The first real Linux MVP is reached when all of the following are true:

- multiple workspaces exist
- multiple terminal panes exist
- split layout works
- sidebar works
- notifications work
- one stable browser pane works
- CLI/socket control works
- user can run coding agents inside it daily

`OpenFlow MVP` comes after that, when:

- one user prompt can trigger a multi-agent run
- the run creates panes/workspaces automatically
- the system can build, test, and review iteratively
- the system can use the embedded browser for verification
- the user can monitor and intervene safely
