# Testing Strategy

Codemux should have tests, but only where they provide real signal.

## What is worth testing

### 1. Rust domain logic

These tests are high-value and should be expanded steadily.

Good targets:

- workspace creation/switch/close
- pane split/close/resize behavior
- terminal session lifecycle state transitions
- browser session navigation state
- notification state and unread counts
- persistence save/restore behavior

Why these matter:

- they protect the app's source-of-truth logic
- they are fast and deterministic
- they catch regressions without requiring manual clicking

## 2. Frontend interaction tests

These are also worth doing, but only for important user flows.

Good targets:

- creating a workspace
- switching workspaces
- creating/splitting/closing panes
- browser pane toolbar interactions
- notification panel rendering

These should test meaningful user behavior, not trivial rendering details.

## 3. End-to-end tests

These are possible and useful later, but should stay focused.

Best use:

- one or two critical workflows that mimic real usage
- launch app
- open workspace
- split pane
- create browser pane
- navigate browser
- verify persisted layout

Do not try to test every tiny interaction end-to-end.

## What is not worth doing

- shallow snapshot tests with little behavioral value
- tests that assert implementation details instead of user outcomes
- huge brittle UI test suites that fail for cosmetic reasons

## Current Testing Policy

Use three layers:

- Rust unit tests for domain/state logic
- frontend interaction tests for important UI flows
- a small number of future end-to-end tests for whole-app workflows

## Current Commands

Use these as the standard verification entry points:

- `npm run verify` - runs the full default verification pass
- `cargo check --manifest-path src-tauri/Cargo.toml` - Rust compile check
- `cargo test --manifest-path src-tauri/Cargo.toml` - Rust backend/state tests
- `npm run check` - Svelte/type checks
- `npm run test` - Vitest frontend tests

`npm run verify` should be the main command to remember. The others are still useful when iterating on one layer.

## Current Coverage

Rust tests currently cover:

- app-state and pane-tree domain logic
- workspace preset construction
- pane swap invariants across all built-in layouts
- incrementally built terminal layouts
- mixed terminal/browser layouts
- workspace-scoped terminal limit behavior across multiple workspaces

Vitest currently covers:

- pane tree helper behavior
- pane swap invariants across multiple layout shapes in a dynamic all-pairs fashion

The swap regression coverage is dynamic rather than hardcoded to one layout pair. The tests create isolated layouts during execution and do not depend on or mutate the user's currently open workspaces.

## Current tooling direction

- Rust built-in test runner for backend logic
- Vitest + Testing Library for Svelte interaction tests
- Playwright later for high-value desktop/web end-to-end flows if needed

## Immediate goals

- add initial frontend interaction tests
- keep expanding Rust tests around new subsystems
- only add broader end-to-end coverage once browser automation and socket control are more mature
