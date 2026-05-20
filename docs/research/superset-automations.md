# Research: Superset Automations

- Purpose: Capture how `superset-sh/superset` implements its "Automations" feature, as input to `docs/plans/automations.md`.
- Audience: Anyone implementing Codemux Automations.
- Authority: Supporting research only. The plan is `docs/plans/automations.md`.
- Update when: Re-investigating Superset, or correcting a finding below.
- Read next: `docs/plans/automations.md`

## What Superset's Automations are

A scheduled prompt-to-agent run. An automation row holds `{ name, prompt, agent, rrule, timezone, targetHostId?, projectId|workspaceId?, dtstart, enabled, mcpScope[] }`, scoped to one owner user + org, stored in Superset's cloud Postgres (Neon).

## Scheduling and dispatch

- Recurrence on the wire is **RRule (RFC 5545)** + IANA timezone, not cron. Cron is CLI-side sugar only.
- A **cloud cron heartbeat** (Upstash QStash) ticks Superset's API server. Each tick selects rows where `nextRunAt <= now()` and enqueues one dispatch message per automation. `nextRunAt` advances regardless of outcome.
- Dispatch mints a short-lived (300s) scoped user JWT, calls the host-service over Superset's relay to create a workspace + worktree, then makes a second call to start the agent in it.
- The host-service runs on the **user's own machine** — Superset does not host execution infrastructure. Workspaces land at the same path layout as a manually created workspace.

## Sidebar behavior (verified from code)

Automation-created workspaces do **not** appear in the main project sidebar by default. The sidebar inner-joins the Electric-synced `v2Workspaces` collection against a **per-device, localStorage-only** `v2WorkspaceLocalState` collection. The dispatcher never writes a local-state row, so the workspace exists as synced data but stays invisible until the user explicitly opens it from the Automations page. There is no `automationId`/`source` column on the workspace table — the link runs the other way: `automation_runs.workspaceId`.

## Gaps in Superset worth beating

- **No completion tracking.** Run statuses are only `dispatching / dispatched / skipped_offline / dispatch_failed`. "Agent finished" is never detected, because dispatch is stateless cloud cron.
- **No overlap protection.** A long agent run plus a tight schedule stacks duplicate workspaces. A reconciler for stuck `dispatching` rows was specced but never shipped.
- **No retention / cleanup.** `new_per_run` mode creates a fresh workspace every fire and never deletes it.
- **No user notifications** on completion/failure — only internal Sentry.
- Fallback host selection orders by `updatedAt` ascending — picks the *least* recently updated host; likely a bug.

## Patterns worth copying

- Idempotency via a unique index on `(automationId, scheduledFor)` with `scheduledFor` floored to the minute, plus a matching dispatch dedup id.
- Per-run branch name `slugify(name, 30)-YYYY-MM-DD-HH-MM-SS`, branched from the project default branch. No auto-commit, no PR — git output is whatever the agent does.
- Un-pausing recomputes `nextRunAt` from now, so a paused automation never fires a backlog.
- RRule exhaustion auto-disables the automation.
- Prompt stored separately from schedule metadata (`get_prompt`/`set_prompt`), so `list` stays cheap and prompts round-trip byte-exact.
- Lightweight prompt version history: one row per `(automation, author, 10-min window)`, tagged `human` / `agent` / `restore`.
- MCP surface: 11 tools — `create / list / get / update / delete / pause / resume / run / logs / get_prompt / set_prompt`. Each is a thin wrapper over the same backend the UI uses.
- `mcpScope` field exists to scope which MCP servers a run may use (currently inert in Superset — forward-compat placeholder).

## Key difference for Codemux

Superset schedules in the cloud and dispatches to a host. Codemux will **schedule on the host itself** (the `codemux-remote` binary). That host-side, stateful scheduler is what lets Codemux fix Superset's biggest gaps: it owns the agent session, so it can record real `succeeded`/`failed` status, do a true in-flight overlap check, reconcile stuck runs, and enforce retention locally.
