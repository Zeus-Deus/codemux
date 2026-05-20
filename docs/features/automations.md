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
- Unit coverage — 35 tests across the recurrence engine, scheduler
  decision logic + tick loop, the fire executor (agent-command mapping,
  branch slugging, worktree creation against a real temp repo), and
  database CRUD / dedup / run lifecycle.

## Current Constraints

- **Host targeting is not yet routed.** The desktop scheduler executes
  every fired automation locally regardless of `host_id`. Honouring a
  remote `host_id` needs account sync + the `codemux-remote scheduler`
  subcommand (below).
- **No stuck-run reconciler.** If the app quits mid-run, that run stays
  `running` and the overlap guard keeps the automation `skipped_busy`
  until the row is cleared. A reconciler is tracked in the plan.
- **No account sync yet.** Automations carry the `dirty` flag but
  `automations_sync` and the `/api/automations` endpoints do not exist,
  so the list is local to one install.
- **Desktop-only scheduler.** The minute tick runs inside the desktop
  app. The `codemux-remote scheduler` subcommand for always-on hosts is
  not built — it is blocked on account sync (a remote host's database
  has no automations until sync delivers them).
- **No run-now.** A one-shot `automation_run` tool is deferred until a
  fire can dispatch real work.
- **`host_id` is unvalidated.** It is stored as a plain integer; nothing
  yet checks the referenced host exists or is reachable.

## Important Touch Points

- `src-tauri/src/database.rs` — `automations` / `automation_runs` schema,
  records, CRUD (`AutomationRecord`, `AutomationRunRecord`,
  `AutomationInput`).
- `src-tauri/src/automations/recurrence.rs` — schedule validation + next
  occurrence.
- `src-tauri/src/automations/scheduler.rs` — `is_due` / `fire_key` /
  `tick`.
- `src-tauri/src/automations/executor.rs` — worktree creation + agent
  spawn for a fired run.
- `src-tauri/src/commands/automations.rs` — shared `*_impl` helpers +
  Tauri command wrappers.
- `src-tauri/src/control.rs` — `automation_*` control-socket handlers.
- `src-tauri/src/mcp_server.rs` — the eight `automation_*` MCP tools.
- `src-tauri/src/lib.rs` — the once-a-minute scheduler background task.
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
