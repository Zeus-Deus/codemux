# Plan: Automations — Account Sync & Remote-Host Execution (Phase 2)

- Purpose: Detailed implementation plan for the second phase of
  Automations — account sync and running automations on remote hosts.
- Audience: Whoever implements this phase.
- Authority: Active work plan only, not current truth.
- Update when: Steps, sequencing, or open questions change.
- Read next: `docs/features/automations.md`, `docs/plans/automations.md`,
  `docs/research/superset-automations.md`

## Goal

Phase 1 shipped a working local Automations feature (data model,
recurrence, scheduler, executor, control surface, UI — all on one
machine). Phase 2 makes it account-bound and multi-host:

- Automations and their run history sync through the Codemux API so
  every signed-in device sees the same list.
- A `codemux-remote scheduler` service runs an automation on a chosen
  remote host (always-on VPS / home server), not just the desktop.
- The desktop and each remote host run only the automations routed to
  them.

The architecture is unchanged from `docs/plans/automations.md`: the API
is a **registry only** (no cloud scheduler); each host runs its own
`scheduler::tick` + `executor`.

## Status

**Phases A–F are done.** The feature works end-to-end. Landed on this
branch and verified:

- Prerequisite refactor, Phase A (reconciler), Phase C
  (`automations_sync`), Phase D (host routing + `codemux-remote
  scheduler`), Phase E (`automations::service` + the
  `provision_scheduler` bootstrap wiring).
- **Phase B is deployed to production** — `/api/automations` GET / POST
  / PATCH / DELETE (+ `?hostServerId=` filter, + `project_remote`) on
  `api.codemux.org`, backed by the `codemux_automations` table.
- **Phase F — the GitHub backbone — is built.** Each automation carries
  the project's git remote URL; the executor's `resolve_repo` uses the
  local path when present and otherwise clones / fetches the remote
  (`~/.codemux/automation-repos/`), branching off `origin/<default>`.
  Runs record their branch + PR URL. "Test connection" probes the
  host's `git` / `gh` (the preflight).
- 1471 Rust unit tests + 263 server tests; verified end-to-end against
  the live API (`project_remote` round-trip). `tsc` / `vitest` green.

One hardening note: the host scheduler currently uses a copy of the
desktop's account token; a per-host scoped token would be a better
blast-radius limit.

## Prerequisite refactor — de-couple the executor from Tauri

`automations::executor::execute_run` currently takes a `tauri::AppHandle`
and reaches the DB through `with_db(handle, …)`. The remote binary has no
Tauri. Before Phase 2 work:

- Split the executor so its core takes `&DatabaseStore` directly:
  `execute_run(db: &DatabaseStore, automation, run)`. The desktop
  scheduler task in `lib.rs` acquires the `State` and calls it; the
  `codemux-remote scheduler` loop passes its own `DatabaseStore`.
- This keeps `automations::executor` Tauri-free, matching `recurrence`
  and `scheduler`.

## Phase A — Stuck-run reconciler (standalone, do first)

Smallest, highest-confidence piece; no API dependency. A run left
`running` by an app/host crash keeps its automation permanently
`skipped_busy` (the overlap guard in `scheduler::has_active_run` counts
`running`).

1. Add `DatabaseStore::reconcile_stale_runs(older_than: &str) -> usize`
   in `database.rs`: `UPDATE automation_runs SET status='failed',
   finished_at=datetime('now'), error='Run did not complete (process or
   app exited)' WHERE status IN ('scheduled','running') AND started_at <
   ?1` (and a `created_at` fallback for rows never started).
2. Call it once at scheduler startup — in the `lib.rs` background task
   before the first tick, and in the `codemux-remote scheduler` startup
   (Phase D) — with a generous ceiling (e.g. now − 6h).
3. Unit-test against an in-memory DB: a `running` row with an old
   `started_at` flips to `failed`; a fresh one is untouched.

## Phase B — API server: schema + endpoints

Server-side, in the API repo (`codemux-api-infrastructure` skill, the
VPS). Mirrors the existing `/api/hosts` convention.

1. **Tables** (Postgres): `automations` and `automation_runs`, columns
   matching the SQLite schema (`database.rs:227-310`), keyed by
   `user_id`. `automations` is the registry; `automation_runs` is
   append-only history.
2. **Endpoints**, all bearer-auth, scoped to the authenticated user
   (single-owner, like Superset — see research §7):
   - `GET /api/automations` → `{ automations: [...] }`
   - `POST /api/automations` → `{ automation }`
   - `PATCH /api/automations/:serverId`
   - `DELETE /api/automations/:serverId`
   - `GET /api/automations/:serverId/runs` → `{ runs: [...] }`
   - `POST /api/automations/:serverId/runs` → `{ run }` (append)
3. **Scheduler-token issuance**: `POST /api/hosts/:serverId/scheduler-token`
   → a short-lived-ish, host-scoped token the remote uses to pull its
   automations and push runs. Scoped to that host only — never the
   user's session token (research §7 confirms this shape).
4. Until these are deployed the client treats `404` as a harmless skip,
   exactly as `hosts_sync` already does (`hosts_sync.rs:133`) — so the
   client half of Phase C can land and lie dormant.

## Phase C — `automations_sync.rs` (client sync)

Model on `hosts_sync.rs` (the `automations` table already carries the
identical `server_id` / `deleted_at` / `dirty` columns + partial index —
`database.rs:227-253` — so **no migration**). Borrow the `api_client`
split from `skills_sync` to keep HTTP isolated.

1. **DB helpers** in `database.rs` (deferred from Phase 1 — add now):
   `list_dirty_automations`, `mark_automation_synced(id, server_id)`,
   `upsert_automation_from_server(...)`, `purge_acknowledged_automation_
   deletes`, and for runs `list_unsynced_runs` / `mark_run_synced` /
   `upsert_run_from_server`.
2. **`src-tauri/src/automations_sync.rs`**: `pull` / `push` /
   `try_sync_with_app(app)`, mirroring `hosts_sync.rs:76-313`. Auth via
   `auth::api_base_url()` + `Bearer` from `load_token(db)`. Pull does the
   server-side deletion sweep (`hosts_sync.rs:163-184`). Runs are
   append-only — push on creation, pull-only, no conflict logic.
3. **Trigger**: `schedule_background_sync(app)` fire-and-forget after
   every mutation in `commands/automations.rs` (the pattern in
   `commands/hosts.rs:847-853`), plus a pull on foreground. Guard
   concurrent runs with a `static AtomicBool` (`hosts_sync.rs:71`).
4. **Optional**: a `SyncEngine`-style state (`skills_sync/mod.rs:72`)
   for a real sync indicator in the Automations view, instead of the
   bare per-row `dirty` dot.

## Phase D — `codemux-remote scheduler` subcommand + host routing

1. **Subcommand**: add `Scheduler { … }` to the clap `enum Command` in
   `bin/codemux_remote.rs:62-76` (Unix-only, like the rest of the file).
2. **The loop**: open the remote's local DB (`database::init_database()`),
   then every `scheduler::TICK_INTERVAL_SECS`:
   a. pull this host's automations from the API into the local DB
      (host-scoped token from Phase E);
   b. `scheduler::tick(&db, now)`;
   c. for each fired run, `executor::execute_run(&db, …)` (the
      Tauri-free core from the prerequisite refactor);
   d. push new/updated run rows back to the API.
   Run the Phase A reconciler once at startup.
3. **Host routing**: the desktop scheduler task should execute only
   `host_id == None` automations; a host's scheduler executes only the
   automations whose `host_id` resolves to itself. Add a host-id filter
   parameter to `scheduler::tick` (or filter its input). The remote
   learns its own identity from the bootstrap-written host id (Phase E).
4. The remote host must have the agent CLIs (`claude` / `codex`)
   installed and authenticated — document this as a host prerequisite;
   a missing binary surfaces as a `failed` run with a clear error
   (already handled by `executor::run_inner`).

## Phase E — Bootstrap: token + persistent service

Extend `ssh/bootstrap.rs` (`bootstrap_remote`, `bootstrap.rs:220-289`).

1. **Token file**: after the `chmod +x` step (`bootstrap.rs:273`), call
   the Phase B token endpoint and write the result to
   `~/.local/share/codemux/scheduler-token` on the host via
   `ssh … 'umask 077; cat > …'`. Write the host's server id alongside it
   so the scheduler knows which automations are its own.
2. **Persistent service** (greenfield — research §5 confirms no existing
   service code): write a systemd **user** unit
   `~/.config/systemd/user/codemux-scheduler.service` invoking the
   absolute `~/.local/bin/codemux-remote scheduler`, then
   `systemctl --user enable --now codemux-scheduler`. Use the same
   absolute-path discipline as the tunnel (`hosts.rs:429-440`). For
   macOS hosts, a `launchd` plist; for hosts without either, fall back
   to a `nohup` + manual note. Gate behind a consent prompt like the
   existing remote-binary install.

## Phase F — Remote-host projects: the GitHub backbone

This phase makes a remote-host automation actually able to run — Phases
D/E deliver the scheduler and service, but the host still has no copy of
the project repo. It also corrects an earlier design dead-end.

### Design correction (supersedes earlier thinking)

An earlier draft proposed seeding the host's repo by **rsyncing or
`git push`-ing the project over the desktop↔host SSH link**, to avoid
needing git credentials on the host. That is dropped. Reasoning:

- The automations worth running — triage PRs, label issues, fix an
  issue, weekly dependency bumps — all need the **agent** to read and
  write GitHub at runtime (`gh issue list`, comment, push a branch,
  `gh pr create`). So the host needs working git + `gh` credentials
  **regardless** of how the repo arrived.
- Given that, "the host clones the repo from its git remote" costs no
  extra credential. The SSH-transport machinery solved a problem that
  does not exist for the real use cases, while adding a bespoke
  two-working-copies sync path. Dropped.
- This also matches Superset: its host-service clones from
  `repoCloneUrl` with the host's own git credentials and never injects
  a token (`host-service/.../resolve-repo.ts:278-315`). Superset's
  cloud GitHub App is for cloud PR-sync only and never reaches the
  host or the agent.

### The model

GitHub (or any git remote) is the backbone. Both directions go through
it; the host and the desktop are independent clones.

1. **Project → remote URL.** A Codemux project is a local repo. At
   automation-create time, resolve its remote with
   `git -C <project> remote get-url origin` and store it on the
   automation (a new `project_remote` column alongside `project_path`).
   A project with no remote can only target **This machine**.
2. **Host clone.** The `codemux-remote scheduler`, the first time it
   needs a project, runs `git clone <project_remote>` into a host-local
   path (e.g. `~/.codemux/automation-repos/<slug>`). Auth is the host's
   own git credentials (SSH key / credential helper / `gh`) — Codemux
   injects no token. Later runs `git fetch` and branch off fresh
   `origin/<default>` (Superset's "locals are often stale" fix).
3. **Host GitHub credentials = the host's own.** The agent inherits the
   host shell's `git` + `gh` auth. Codemux does not build or manage a
   GitHub App for the host side. Host prerequisites to document: `git`
   with `user.name`/`user.email` set, git credentials/SSH for the
   remote, and an authenticated `gh` for any issue/PR work.
4. **Results travel as branches.** The agent commits to
   `automation-<slug>-<timestamp>` and pushes it (and may `gh pr
   create`). To bring a run home, the **desktop** `git fetch`es that
   branch from the remote — the dev machine already has remote access —
   and Codemux materialises it as a workspace in the project. No
   rsync, no SSH mirror, no merge-on-pull: it is just a branch landing
   next to the user's work, integrated on their terms.

### What to build

- **`project_remote` on `automations`** (local schema + the API table +
  the sync wire format). Resolve it in `commands::automations` at
  create/update from the chosen project.
- **Host clone/fetch** in `automations::executor` (or a sibling): given
  `project_remote`, ensure a host-local clone exists, `git fetch`, then
  the existing worktree-create branches off `origin/<default>`. On the
  desktop / "This machine" this step is a no-op — the repo is `project_path`.
- **Preflight check** — the genuine improvement over Superset, which has
  none. When a remote-host automation is created (and as part of
  `hosts_test_connection`), probe the host over SSH for: `git` present +
  identity configured; `gh` present + `gh auth status` OK; the
  `project_remote` reachable. Surface a clear, actionable warning at
  setup time ("This host can't reach `<repo>` — run `gh auth login` on
  it, or add a deploy key"). Inform, do not hard-block: a local-only
  automation can still run. Extends the existing `GhStatus` check.
- **Run → branch/PR linkage** — add `branch` and `pr_url` to
  `automation_runs`; the executor records the branch, and the agent's
  `gh pr create` (if any) is captured. The Automations run-history list
  then reads like a PR list. Superset has neither column — this is ours.
- **"Pull this run home"** action on a run row — desktop-side
  `git fetch` of the run's branch + materialise a workspace.

## Suggested sequencing

A (reconciler) → B (API) → C (client sync) → D (remote scheduler) →
E (bootstrap) → **F (GitHub backbone)**. A is independent. Note that
**D + E without F cannot run a remote-host automation usefully** — the
host has no repo until F — so F is not optional polish; it is the last
load-bearing piece of the remote-host story. "This machine" automations
need none of D/E/F and already work.

## Open questions

- Scheduler-token lifetime and rotation — fixed long-lived per-host, or
  refreshed? Leaning long-lived, revoked when the host is removed.
- `automation_runs` volume — append-only history syncing every run for
  every device could grow. Cap the pull window (e.g. last 100 per
  automation) and/or prune server-side after N days.
- Non-systemd / non-launchd hosts — is a `nohup` fallback acceptable for
  v1, or require systemd/launchd?
- Should the executor append a standard "commit, push the branch, open
  a PR" instruction to the agent prompt, or leave push/PR entirely to
  the user's prompt (as Superset does)? Leaning: leave it to the prompt
  for v1, but always record whatever branch/PR results.
- Private-repo auth on the host — deploy key vs. fine-grained PAT vs.
  the user's `gh` token. Codemux should not store these; the preflight
  check just verifies one of them works.
- A project with no git remote — confine it to "This machine", or also
  allow a no-remote SSH-rsync fallback later? Leaning: This-machine-only
  for now.

## Likely touch points

- `src-tauri/src/automations/executor.rs` — Tauri-free core refactor
- `src-tauri/src/automations/scheduler.rs` — host-id routing filter
- `src-tauri/src/database.rs` — sync helpers, `reconcile_stale_runs`
- `src-tauri/src/automations_sync.rs` — new sync module
- `src-tauri/src/commands/automations.rs` — `schedule_background_sync`
- `src-tauri/src/bin/codemux_remote.rs` — `Scheduler` subcommand
- `src-tauri/src/ssh/bootstrap.rs` — token + service registration
- `src-tauri/src/lib.rs` — startup reconcile, host-id routing
- API repo — `/api/automations*` endpoints + scheduler-token
- Phase F: `automations/executor.rs` — host clone/fetch before worktree;
  `commands/automations.rs` — resolve `project_remote`; `commands/hosts.rs`
  — GitHub-access preflight in `hosts_test_connection`; `database.rs` +
  the API `automations` table — `project_remote`, run `branch` / `pr_url`
