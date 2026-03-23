# Codemux Testing

- Purpose: Canonical verification strategy and testing policy.
- Audience: Anyone implementing or validating changes.
- Authority: Testing layers, commands, and manual-validation guidance.
- Update when: Verification commands, tooling, or testing philosophy change.
- Read next: `docs/core/STATUS.md`, relevant feature docs

## Default Commands

- `npm run verify`: full default verification pass
- `cargo check --manifest-path src-tauri/Cargo.toml`: Rust compile check
- `cargo test --manifest-path src-tauri/Cargo.toml`: Rust backend and state tests
- `npm run check`: TypeScript type checks (`tsc --noEmit`)
- `npm run test`: frontend tests

Default to `npm run verify` after meaningful work. Use the narrower commands when iterating on one layer.

## Testing Layers

- Rust domain tests for workspaces, pane trees, terminal lifecycle, persistence, notifications, memory, indexing, and OpenFlow runtime logic
- frontend interaction tests for important workspace, pane, and browser flows
- focused end-to-end coverage later for a few critical workflows rather than every UI detail

## Manual Validation Rules

- Implemented is not the same as verified.
- A roadmap checkbox is not release proof.
- Browser and OpenFlow changes need especially careful manual validation because they are still prototype-heavy areas.

## High-Value Manual Workflows Right Now

- app startup and fallback launch behavior
- workspace creation, switching, closing, and pane operations
- mixed terminal and browser layouts
- browser toolbar and automation command flows
- memory, handoff, and indexing workflows
- OpenFlow run creation, monitoring, control actions, retry, cancel, and stability
