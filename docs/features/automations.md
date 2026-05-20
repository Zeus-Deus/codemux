# Automations

- Purpose: Describe the current capability and constraints of the Automations feature.
- Audience: Anyone working in or around scheduled agent runs.
- Authority: Canonical feature-level reality doc.
- Update when: Behavior, constraints, or major touch points change.
- Read next: `docs/plans/automations.md`, `docs/research/superset-automations.md`

## What This Feature Is

An automation is a named prompt + agent + recurrence that runs on a host
the user picks. The schedule fires on the chosen host; each fire creates
a fresh workspace and runs the agent in it. Automations are account-bound
so every signed-in device sees the same list.

The feature is being built in layers. This doc describes what is **landed
in the repo today** — the plan in `docs/plans/automations.md` tracks the
layers still to come.

## Current Model

- **Persistence** — `automations` and `automation_runs` tables in the
  local SQLite database (`database.rs`, schema v5). They carry the same
  `server_id` / `deleted_at` / `dirty` columns as `hosts`, so the
  account-sync layer can be added with no migration.
- **Recurrence** — an automation's `schedule` is a complete RFC 5545
  iCalendar block (a `DTSTART` line plus one `RRULE` line). The
  `automations::recurrence` module wraps the `rrule` crate to validate a
  schedule and compute the next occurrence, with IANA-timezone / DST
  correctness.
- **Next-run bookkeeping** — `next_run_at` is derived state. The command
  layer recomputes it from *now* on every create, schedule edit, and
  resume, so a paused automation never fires a backlog when resumed.
- **Scheduler** — `automations::scheduler` holds the decision logic
  (`is_due`, `fire_key`) and the `tick` loop. `fire_key` floors a fire
  to its minute and is the `scheduled_for` dedup key. A background task
  in `lib.rs` runs `tick` once a minute against the local database.
- **Executor** — `automations::executor` takes each fired run, creates
  an isolated git worktree, runs the agent headlessly with the prompt,
  and writes the terminal status back. The desktop scheduler task spawns
  one executor per fire.
- **Sync** — `automations_sync` replicates the registry through the
  Codemux API, the same dirty-flag / tombstone model as `hosts_sync`.
  Only `automations` syncs; `automation_runs` stay per-device.
- **Remote execution** — `codemux-remote scheduler` runs the identical
  reconcile + tick + executor loop on an always-on host; host routing
  keeps each scheduler to its own automations.
- **Run history** — one `automation_runs` row per fire. The
  `UNIQUE(automation_id, scheduled_for)` constraint makes a re-delivered
  tick idempotent. Status flows `scheduled → running → succeeded |
  failed | skipped_offline | skipped_busy`. Run rows are kept
  indefinitely; only host-side worktrees are pruned (per
  `retention_limit`).

## What Works Today

- Full automation CRUD over the local database, with create-time
  validation (name/prompt/agent/schedule/timezone/retention bounds, and
  a real RFC 5545 parse of the schedule).
- Pause / resume, with resume recomputing the next fire time.
- **A live scheduler.** A background task in the desktop app calls
  `automations::scheduler::tick` once a minute: due automations get a
  run row recorded and their `next_run_at` advanced; overlapping fires
  are serialised (`skipped_busy`); the per-minute `fire_key` keeps a
  double tick idempotent. Each fresh run is emitted as an
  `automations://fire` event and surfaced to the user as a toast.
- **A fire runs the agent.** `automations::executor::execute_run`
  creates an isolated git worktree off the automation's project (a
  fresh `automation-<slug>-<timestamp>` branch), spawns the chosen
  agent headlessly with the prompt (`claude --print` / `codex exec`),
  and records the terminal `succeeded` / `failed` status plus the
  workspace path on the run row.
- **Automations view** — a first-class destination opened from the left
  sidebar (under "New agent", above the project list — the same
  placement Codex and Superset use). A sidebar list + detail pane to
  create, edit, pause/resume, and delete automations, with a
  frequency/time/weekday schedule builder (and a raw RFC 5545 escape
  hatch) and a per-automation run-history view.
- Desktop command surface — `automations_list`, `automations_get`,
  `automations_create`, `automations_update`, `automations_set_enabled`,
  `automations_delete`, `automations_runs`.
- Agent / MCP control surface — eight tools (`automation_list`,
  `automation_get`, `automation_create`, `automation_update`,
  `automation_delete`, `automation_pause`, `automation_resume`,
  `automation_runs`), each routed through the control socket to the same
  shared `commands::automations::*_impl` helpers the desktop uses.
- **Stuck-run reconciler** — `reconcile_stale_runs` fails any run left
  `running`/`scheduled` by a crash or quit; it runs at scheduler
  startup (desktop and `codemux-remote`), so a crashed run can never
  pin its automation in `skipped_busy`.
- **Account sync** — `automations_sync` pulls/pushes the automation
  registry through the Codemux API's live `/api/automations` endpoints,
  mirroring `hosts_sync`: a fire-and-forget sync after every mutation, a
  startup pull, and a pull on every scheduler tick. Host targeting
  crosses the wire as the host's `server_id`; the remote scheduler pulls
  host-scoped (`?hostServerId=`).
- **Host routing** — `scheduler::tick` takes a `local_only` switch: the
  desktop runs only automations targeting this machine (`host_id IS
  NULL`); a `codemux-remote scheduler` runs the rest.
- **`codemux-remote scheduler`** — a subcommand on the remote binary
  that runs the same reconcile + pull + tick + execute loop on an
  always-on host. Host bootstrap provisions it: it writes the scheduler
  token + host identity and registers a systemd user service (via
  `automations::service`) with lingering, so it survives reboots.
- Unit coverage — 43 Rust tests (recurrence, scheduler decision + tick +
  host routing, the fire executor, account sync, the reconciler, the
  service-unit generators, database CRUD / dedup / run lifecycle), plus
  16 server tests for the `/api/automations` endpoints.

## Current Constraints

- **Scheduler tokens are full account tokens.** The host bootstrap
  copies the desktop's auth token to the host so its scheduler can call
  the API. A per-host scoped token would limit blast radius if a host
  is compromised — a future hardening.
- **No run-now.** A one-shot `automation_run` tool is still deferred.
- **No workspace lifecycle.** `workspace_sync_from_host` and the
  automation-workspace pull-back guard (`docs/plans/automations-sync.md`
  Phase F) are not built.

## Important Touch Points

- `src-tauri/src/database.rs` — `automations` / `automation_runs` schema,
  records, CRUD (`AutomationRecord`, `AutomationRunRecord`,
  `AutomationInput`).
- `src-tauri/src/automations/recurrence.rs` — schedule validation + next
  occurrence.
- `src-tauri/src/automations/scheduler.rs` — `is_due` / `fire_key` /
  `tick`.
- `src-tauri/src/automations/executor.rs` — worktree creation + agent
  spawn (`run_fire` / `apply_outcome`, Tauri-free; `execute_run` is the
  desktop wrapper).
- `src-tauri/src/automations/service.rs` — systemd / launchd unit
  generation for the remote scheduler.
- `src-tauri/src/automations_sync.rs` — account sync (pull / push).
- `src-tauri/src/commands/automations.rs` — shared `*_impl` helpers,
  Tauri command wrappers, `schedule_automations_sync`.
- `src-tauri/src/control.rs` — `automation_*` control-socket handlers.
- `src-tauri/src/mcp_server.rs` — the eight `automation_*` MCP tools.
- `src-tauri/src/bin/codemux_remote.rs` — the `scheduler` subcommand.
- `src-tauri/src/lib.rs` — the once-a-minute desktop scheduler task.
- `src/components/automations/automations-view.tsx` +
  `automations-section.tsx` — the full-screen Automations view.
- `src/components/layout/sidebar-action-row.tsx` — the sidebar entry.
- `src/hooks/use-automation-fire-toast.ts` — the fire-event toast.

## Notes

- The `rrule` crate is the recurrence engine; it bundles `chrono-tz` for
  the IANA timezone database.
- The deliberate divergence from Superset (cloud-cron dispatch) is that
  Codemux schedules on the host itself — see
  `docs/research/superset-automations.md` for why that enables real
  completion status, overlap prevention, and local retention.
