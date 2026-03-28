# OpenFlow Work Plan

- Purpose: Track active OpenFlow implementation work and near-term priorities.
- Audience: Anyone actively changing orchestration behavior or OpenFlow UX.
- Authority: Active OpenFlow work plan, not current truth.
- Update when: Priorities, unresolved questions, or likely touch points change.
- Read next: `docs/features/openflow.md`, `docs/core/STATUS.md`

## Active Priorities

1. Orchestrator response behavior (DONE):
   - FIXED: backend-driven orchestration loop replaces frontend polling; runs continuously at 5s intervals
   - FIXED: user injections wake the orchestration loop immediately via `tokio::sync::Notify`
   - FIXED: stuck probes written directly to comm log as SYSTEM PROBE entries (no more shell escaping feedback loop)
   - FIXED: Blocked phase now recoverable via user injection
   - FIXED: WaitingApproval stays until explicit user approval
   - FIXED: implicit completion removed; only explicit DONE: markers count
   - FIXED: stuck detection thresholds raised to realistic values (50-90 seconds)
   - FIXED: probe re-arming fixed to clear on meaningful progress, not just counts
   - FIXED: conflicting `run_autonomous_loop` / `advance_run_phase` auto-advance removed
   - FIXED: wrapper script hardened with `eval` and error recovery
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
5. Claude CLI adapter (DONE):
   - FIXED: `ClaudeAdapter` implemented with dedicated wrapper script (`claude-wrapper.sh`)
   - FIXED: uses `claude -p` with `--system-prompt`, `--resume`, `--output-format json`, `--permission-mode bypassPermissions`
   - FIXED: session ID captured from JSON output for session continuation
   - FIXED: `--system-prompt` passed on every call (doesn't persist across `--resume`)
   - FIXED: auto-translator converts opencode's internal delegation patterns to ASSIGN lines
   - all Claude models work: haiku (cheapest), sonnet (balanced), opus (strongest)
6. Remaining work:
   - validate end-to-end with Claude CLI on real multi-agent runs
   - improve orchestrator prompt for consistent parallel assignment across all models

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
