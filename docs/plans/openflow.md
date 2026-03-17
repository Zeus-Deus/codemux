# OpenFlow Work Plan

- Purpose: Track active OpenFlow implementation work and near-term priorities.
- Audience: Anyone actively changing orchestration behavior or OpenFlow UX.
- Authority: Active OpenFlow work plan, not current truth.
- Update when: Priorities, unresolved questions, or likely touch points change.
- Read next: `docs/features/openflow.md`, `docs/core/STATUS.md`

## Active Priorities

1. Orchestrator response behavior:
   - simple user questions should be answered directly
   - modification requests should trigger a fresh planning and build loop
2. User intervention during any phase:
   - injected user messages should affect runs before `awaiting_approval`
3. Browser integration in the OpenFlow workspace:
   - replace the placeholder browser view with the real Codemux browser surface or a clearly shared equivalent
4. Reliability and execution hardening:
   - verify working directory handling
   - complete single-instance and execution-isolation hardening
   - prevent stray GUI launches from agent sessions
5. Scale and log handling:
   - harden communication log locking, rotation, and buffering
   - validate 15-20 agent runs under both dev and normal workflows

## Likely Touch Points

- `src-tauri/src/openflow/orchestrator.rs`
- `src-tauri/src/openflow/prompts.rs`
- `src-tauri/src/commands.rs`
- `src-tauri/src/terminal/mod.rs`
- `src/components/openflow/`
- `docs/features/openflow.md`

## Longer-Term Milestones

- richer browser-backed verification inside runs
- clearer approval, budget, and safety controls
- a cleaner extractable OpenFlow runtime boundary
