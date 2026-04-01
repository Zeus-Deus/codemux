# OpenFlow Work Plan

- Purpose: Track active OpenFlow implementation work and near-term priorities.
- Audience: Anyone actively changing orchestration behavior or OpenFlow UX.
- Authority: Active OpenFlow work plan, not current truth.
- Update when: Priorities, unresolved questions, or likely touch points change.
- Read next: `docs/features/openflow.md`, `docs/core/STATUS.md`

## Active Priorities

1. Orchestrator prompt improvement:
   - improve orchestrator prompt for consistent parallel assignment across all models
   - simple user questions should be answered directly
   - modification requests should trigger a fresh planning and build loop
2. Browser integration in the OpenFlow workspace:
    - replace the placeholder browser view with the real Codemux browser surface or a clearly shared equivalent
    - keep browser/live-preview hints optional so OpenFlow remains generic across non-web projects
3. Reliability and execution hardening:
    - complete single-instance and execution-isolation hardening
    - prevent stray GUI launches from agent sessions
4. Scale and log handling:
   - validate 15-20 agent runs under both dev and normal workflows
5. End-to-end validation:
   - validate end-to-end with Claude CLI on real multi-agent runs
   - validate with OpenCode adapter on real multi-agent runs

## Recently Completed

- Backend-driven orchestration loop (5s intervals, Notify-based wakeup, stuck probes, phase recovery)
- Claude CLI adapter with dedicated wrapper script, session continuation, auto-translator
- Wrapper script hardening with eval and error recovery
- Stuck detection tuning (50-90s thresholds, probe re-arming on progress)
- WaitingApproval stays until explicit user approval
- Implicit completion removed; only explicit DONE: markers count

## Likely Touch Points

- `src-tauri/src/openflow/orchestrator.rs`
- `src-tauri/src/openflow/prompts.rs`
- `src-tauri/src/openflow/adapters/` (claude.rs, opencode.rs)
- `src-tauri/src/commands/openflow.rs`
- `src-tauri/src/terminal/mod.rs`
- `src/components/openflow/`
- `src/stores/openflow-store.ts`
- `docs/features/openflow.md`

## Longer-Term Milestones

- richer browser-backed verification inside runs
- clearer approval, budget, and safety controls
- a cleaner extractable OpenFlow runtime boundary
