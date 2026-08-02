# Automations Work Plan

- Purpose: Track the remaining hardening and polish for scheduled agent runs.
- Audience: Anyone implementing or operating Automations.
- Authority: Active work plan only, not current truth.
- Update when: A remaining priority lands or the execution/security model changes.
- Read next: `docs/features/automations.md`, `docs/archive/automations-sync.md`, `docs/research/superset-automations.md`
- Status: MOSTLY LANDED — local scheduling, account registry sync, remote-host execution, and Git-backed repo transport ship; security and lifecycle polish remain.

## Goal

Finish the operational edges around the shipped Automations system without changing its core model: the registry syncs across the account, scheduling and execution happen on the selected machine, every fire gets an isolated git worktree, and run history records real terminal outcomes.

## Active Priorities

1. **Scope scheduler credentials.** Remote bootstrap currently copies the desktop account token to the host. Replace it with a revocable per-host credential and define rotation/removal behavior.
2. **Enforce retention.** Apply each automation's stored `retention_limit` to completed run worktrees while preserving active runs and anything the user has adopted or synced elsewhere.
3. **Add an explicit run-now surface.** Implement one shared one-shot command/tool (`automation_run`) and route UI/MCP callers through the same execution path as scheduled fires.
4. **Wire completion notifications.** Send success/failure through the existing notification policy, including per-workspace mute and duplicate-suppression behavior.
5. **Close the workspace handoff loop.** Add a direct action to fetch/open a run branch locally, plus a dedicated automation-workspace visual treatment.

## Guardrails

- `automations` is the account-synced registry; `automation_runs` remains per-device/per-host history unless a separate product decision changes that contract.
- The desktop scheduler handles `host_id IS NULL`; `codemux-remote scheduler` handles automations targeted at its registered host.
- The Codemux API stores registry state only. It does not tick schedules or execute agents.
- A remote host obtains the repository from its git remote using that host's own credentials. Codemux does not inject GitHub tokens.
- Same-automation overlap produces `skipped_busy`; distinct automations may run concurrently.
- Resume recomputes `next_run_at` from now rather than replaying a paused backlog.

## Open Questions

- What exact host-credential scope is sufficient for registry pull, run updates, and device identity without granting unrelated account access?
- When a retained worktree has a pushed branch or PR but was never adopted locally, should pruning remove only the checkout or also offer branch cleanup?
- Should run-now use the scheduled fire's minute-based idempotency key or a separate user-request id?

## Likely Touch Points

- `src-tauri/src/automations/{scheduler,executor,service}.rs`
- `src-tauri/src/automations_sync.rs`
- `src-tauri/src/commands/automations.rs`
- `src-tauri/src/bin/codemux_remote.rs`
- `src-tauri/src/mcp_server.rs`
- `src-tauri/src/notifications.rs`
- `src/components/automations/`
- `docs/features/automations.md`

## Already Landed

- Local SQLite registry and per-fire run records with RFC 5545 recurrence, DST-aware next-run calculation, overlap serialization, crash reconciliation, and real `succeeded` / `failed` outcomes.
- Desktop scheduler/executor plus the full Automations view and eight read/CRUD/pause/resume MCP tools.
- Account sync for the automation registry through `/api/automations`; run rows intentionally stay local to the executing machine.
- `codemux-remote scheduler`, persistent host service provisioning, host routing, and Git clone/fetch transport using the host's credentials.
- Per-repository reachability preflight and run-history health indicators.

Historical Phase 2 implementation detail lives in `docs/archive/automations-sync.md`; current behavior and constraints live in `docs/features/automations.md`.
