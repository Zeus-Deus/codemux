# OpenFlow Work Plan

- Purpose: Track active OpenFlow implementation work and near-term priorities.
- Audience: Anyone actively changing orchestration behavior or OpenFlow UX.
- Authority: Active OpenFlow work plan, not current truth.
- Update when: Priorities, unresolved questions, or likely touch points change.
- Read next: `docs/features/openflow.md`, `docs/core/STATUS.md`

## Active Priorities

1. Orchestrator response behavior (IN PROGRESS):
   - FIXED: communication-panel sends now trigger an immediate orchestrator cycle instead of waiting for the next polling tick
   - FIXED: completed runs now re-enter replanning when a new user message arrives
   - FIXED: orchestrator cycles now read the full comm log state instead of only incremental deltas, so DONE/BLOCKED/ASSIGN state is not forgotten between cycles
   - FIXED: the orchestrator session is respawned when possible if a follow-up message arrives after its PTY died
   - FIXED: injection handling now waits for an explicit orchestrator response before marking the message handled
   - FIXED: only literal `ASSIGN ...` lines count as OpenFlow assignments; prose about assigning work no longer advances the run incorrectly
   - FIXED: stuck runs now nudge the orchestrator to emit real ASSIGN/STATUS/BLOCKED output instead of silently idling forever
   - FIXED: invalid internal delegation patterns like `General Agent` tasking are detected and corrected automatically
   - simple user questions should be answered directly
   - modification requests should trigger a fresh planning and build loop
2. User intervention during any phase:
   - injected user messages should affect runs before `awaiting_approval`
3. Browser integration in the OpenFlow workspace:
    - replace the placeholder browser view with the real Codemux browser surface or a clearly shared equivalent
    - keep browser/live-preview hints optional so OpenFlow remains generic across non-web projects
4. Reliability and execution hardening:
    - FIXED: preserve the original session working directory during dead-session rescue
    - FIXED: preserve `GOAL`, `APP_URL`, and `AGENTS` headers when rotating large communication logs
    - FIXED: stop hard-coding localhost:1420 for new runs; use a run-scoped app URL instead
    - FIXED: non-orchestrator agents now stay idle until they receive an `ASSIGN ...` command
    - FIXED: `awaiting_approval` no longer tears down the run immediately, so pause keeps the workspace alive
    - FIXED: orchestrator blocked/stalled states now stay explicit instead of collapsing into an idle-looking run
    - FIXED: planning-phase stuck detection now waits longer before auto-rescue to avoid false early stuck states
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
