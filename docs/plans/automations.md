# Plan: Automations

- Purpose: Track the design and build order for the Automations feature — scheduled agent runs on a user-chosen host.
- Audience: Anyone implementing Automations.
- Authority: Active work plan only, not current truth.
- Update when: Priorities, open questions, or likely touch points change.
- Read next: `docs/research/superset-automations.md`, `docs/features/remote-hosts.md`, `docs/core/STATUS.md`

## Goal

Let a user define an Automation — a named prompt + agent + recurrence — that runs on a host they pick from their connected hosts. The schedule lives in the Codemux API (account-bound, so every signed-in device sees the same list), but the schedule *fires* on the chosen host via the `codemux-remote` binary. Each fire creates a fresh workspace + worktree and runs the agent in it. Run history is account-bound; the resulting workspaces stay on the host and never auto-appear on the user's main desktop unless they explicitly sync one in. The model follows Superset (see research doc) but is deliberately host-side scheduled so Codemux can track real completion, prevent overlap, reconcile stuck runs, and enforce retention.

## Architecture (locked decisions)

- **Registry:** `automations` + `automation_runs` tables on the Codemux API server (the existing auth/hosts/settings VPS). Plain CRUD endpoints — no scheduler worker, no cron tick on the VPS. Storage is kilobytes per user.
- **Scheduler:** a new `codemux-remote scheduler` subcommand on the chosen host. Polls the API (~30s, immediately after each fire), ticks via an `rrule` crate, runs as a persistent service so it survives host reboots.
- **Target host:** every automation picks its own host (`automations.target_host_id`) — different automations can run on different hosts. A "host" is any machine connected as a Codemux host and running the `codemux-remote scheduler` service; that includes a user's own always-on desktop/laptop connected as a host. There is no separate "run inside the desktop app" scheduler — one scheduler implementation, and automations fire whether or not the Codemux app is open.
- **Edit propagation:** the remote binary polls the API. Auth is a per-host scoped token provisioned during the existing `hosts_bootstrap_install` flow. Edits made from any device (or via MCP) reach the host without the desktop needing to be online.
- **Fire flow:** the scheduler creates a fresh workspace + worktree itself (deterministic, same code path as `workspace_push_to_host`), branch `slug-<timestamp>` off the project default branch, then spawns the chosen agent inside it with the prompt. The agent does not self-create the workspace via MCP — workspace creation is infrastructure.
- **Offline:** host off → no tick → a `skipped_offline` run row is written on next boot. No cloud worker needed.
- **Sidebar:** automation run-workspaces never auto-appear in the main project sidebar. They render only in the Automations panel. A sidebar entry appears only when the user explicitly syncs one in (mirrors how Superset gates the sidebar on per-device local state).
- **Sync vs pull:** new `workspace_sync_from_host` action — a non-destructive host→local mirror; host stays canonical, `host_id` unchanged, automation keeps running. `workspace_pull_back` is hard-blocked for automation workspaces (UI hidden + command rejects, so an MCP caller cannot kill an automation either).
- **MCP:** full CRUD + run — 11 tools, mirroring Superset's surface.
- **Run model:** fresh workspace per fire.
- **Retention:** keep the last 10 *completed* run-workspaces per automation on the host (plus any currently-running one); auto-prune older worktrees. Never prune a run the user has synced locally. `automation_runs` log rows are kept regardless — history survives worktree pruning.
- **Overlap:** one run at a time per automation. If a fire lands while that automation's previous run is still going, write a `skipped_busy` row and skip it — the next scheduled occurrence is the natural retry. Different automations still run concurrently with each other; only the same automation is serialized. (Superset stacks duplicate workspaces here — deliberately rejected as poor UX.)

## Run lifecycle

`scheduled` → `running` → `succeeded` | `failed` | `skipped_offline` | `skipped_busy`.
The host scheduler owns the agent session, so `succeeded`/`failed` are real terminal states (Superset cannot do this — it only records `dispatched`). A reconciler in the scheduler marks any `running` row whose agent session has died as `failed`.

## Active Priorities

The feature works end-to-end on the local machine, and the Phase 2
client work (sync, host routing, the remote scheduler, the reconciler)
has landed — see *Already Landed*. What remains:

1. **API server endpoints.** `automations` / `automation_runs` tables
   and `/api/automations*` REST endpoints, plus the per-host
   scheduler-token endpoint, on the API server (separate repo). The
   desktop sync client is finished and degrades gracefully (`404` =
   skip) until these deploy.
2. **Host bootstrap wiring.** `hosts_bootstrap_install` writes the
   scheduler token and registers the `codemux-remote scheduler` service
   (`automations::service` generates the units). Needs item 1 for the
   token endpoint.
3. **Workspace lifecycle.** `workspace_sync_from_host` (non-destructive
   mirror), an `automation_id` column on workspaces, and a
   `workspace_pull_back` guard for automation workspaces — Phase F in
   `docs/plans/automations-sync.md`.
4. **Polish.** A one-shot `automation_run` MCP tool; the
   `retention_limit` worktree prune; run completion/failure piped into
   the notification system (`docs/features/notifications.md`); a
   dedicated automation workspace icon.

## Open Questions

No outstanding design questions — the items below were raised and resolved:

- **Retention** is count-based: last 10 completed runs per automation, configurable.
- **Overlap:** one run at a time per automation; an overlapping fire becomes `skipped_busy` with no retry.
- **`mcpScope` is dropped from v1.** A do-nothing forward-compat field is schema noise — add real MCP-server scoping only when there is a concrete need.
- **Scheduler token** reuses the existing host credential lifecycle (issued at host bootstrap, removed when the host is removed) — no separate token-management surface.
- **No desktop-app scheduler.** Automations always target a host; a user's own always-on machine can be connected as a host. One scheduler implementation only.

Implementation-time detail still to settle: token leak/rotation handling within the host credential lifecycle.

## Likely Touch Points

- `src-tauri/src/bin/codemux_remote.rs` — add the `scheduler` subcommand
- `src-tauri/src/ssh/bootstrap.rs` — provision scoped token, register persistent service
- `src-tauri/src/ssh/push.rs` — reuse rsync helpers for `workspace_sync_from_host`
- `src-tauri/src/commands/hosts.rs` — `workspace_push_to_host`, `workspace_pull_back` (add guard), new `workspace_sync_from_host`
- `src-tauri/src/hosts_sync.rs` — pattern to mirror as new `src-tauri/src/automations_sync.rs`
- `src-tauri/src/database.rs` — new `automations` / `automation_runs` tables, `automation_id` on workspaces
- `src-tauri/src/mcp_server.rs` — register the 11 automation tools
- Codemux API server (see `codemux-api-infrastructure` skill) — `/api/automations`, `/api/automations/:id/runs`, token issuance
- `src/` renderer — new Automations panel, automation workspace icon, sync-locally action
- `docs/features/` — add `automations.md` once shipping; update `remote-hosts.md` and `notifications.md`

## Already Landed

Automations feature — local foundation, scheduler, and UI (this branch):

- **Data model** — `automations` + `automation_runs` tables (`database.rs`,
  schema v5) with sync-ready `server_id` / `deleted_at` / `dirty` columns;
  records + full CRUD; idempotent per-minute run inserts; terminal run
  lifecycle.
- **Recurrence engine** — `automations::recurrence` (RFC 5545 via the
  `rrule` crate): validate a schedule, compute the next occurrence,
  DST-correct.
- **Scheduler** — `automations::scheduler`: `is_due` / `fire_key` and the
  `tick` loop (records runs, serialises overlap as `skipped_busy`,
  advances `next_run_at`). A once-a-minute background task in `lib.rs`
  drives it and emits `automations://fire` events.
- **Executor** — `automations::executor`: per fire, creates an isolated
  git worktree off the project, spawns the agent headlessly with the
  prompt (`claude --print` / `codex exec`), and records the terminal
  `succeeded` / `failed` status. The scheduler task spawns one per fire.
- **Desktop command surface** — seven `automations_*` Tauri commands.
- **Agent / MCP control surface** — eight `automation_*` MCP tools routed
  through `control.rs` to shared `*_impl` helpers.
- **Automations view** — a full-screen view opened from the left sidebar
  (under "New agent", above projects): create / edit / pause / resume /
  delete with a schedule builder and run-history view; a fire-event toast.
- **Automations view** + sidebar entry (Phase 1).

Phase 2 — account sync + remote-host execution (this branch):

- **Stuck-run reconciler** — `reconcile_stale_runs`, run at every
  scheduler startup.
- **Account sync** — `automations_sync` (pull / push, host-identity
  translation), wired into every mutation + the scheduler loop.
- **Host routing** — `scheduler::tick`'s `local_only` switch.
- **`codemux-remote scheduler`** — the remote-binary subcommand running
  the reconcile + pull + tick + execute loop.
- **Service units** — `automations::service` (systemd / launchd).
- **Executor refactor** — `run_fire` / `apply_outcome` are Tauri-free so
  the desktop and the remote share one code path.
- **Tests** — 43 unit tests; `cargo check` / `tsc` / `vitest` all green,
  apart from one pre-existing unrelated `agent_browser` env test.

What is still open (see `docs/plans/automations-sync.md`): the
`/api/automations` server endpoints (separate API repo); the host
bootstrap writing a scheduler token + registering the service; and
Phase F (workspace lifecycle).

Pre-existing platform Automations builds on:

- Push workspace to host (`workspace_push_to_host`) and pull-back
  (`workspace_pull_back`) — full rsync + remote PTY respawn.
- `codemux-remote` binary, auto-installed during host bootstrap; survives
  laptop reconnects. Remote PTY daemon keeps agents alive across desktop
  close.
- Account-bound sync for hosts, settings, and skills via the API server —
  the pattern `automations_sync.rs` will copy.

## Notes

- No scheduling logic runs on the Codemux API server — it is a registry only. This keeps server cost negligible and means automations do not depend on the Codemux VPS being up.
- The host-side scheduler is the deliberate divergence from Superset (which schedules in the cloud). It is what enables real completion status, overlap prevention, stuck-run reconciliation, and local retention — all gaps in Superset.
- Supporting research: `docs/research/superset-automations.md`.
