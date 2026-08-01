# Observability

- Purpose: Describe the structured logging, metrics, feature flag, and safety config subsystem.
- Audience: Anyone working on OpenFlow safety gates, feature flags, debug logging, or the replay-record UI.
- Authority: Canonical feature doc for `ObservabilityStore` and its persisted snapshot.
- Update when: A new metric, feature flag, permission policy, or safety config field is added.
- Read next: `docs/features/openflow.md`, `docs/features/settings.md`

## What This Feature Is

`ObservabilityStore` is a small persistent JSON store that holds structured app-level telemetry: counters, structured log entries, experimental feature flags, permission policy for risky actions, replay records for past sessions, and the safety config that governs OpenFlow runs. It's not a full observability pipeline (no metrics exporter, no trace collector) — it's the single durable source of truth for "what is Codemux's runtime safety posture right now?".

The data flows both directions: the Rust backend writes metrics and logs; the React frontend reads the snapshot via `get_observability_snapshot` and writes feature flags / permission policy / safety config back via dedicated Tauri commands.

## Current Model

All types live in `src-tauri/src/observability.rs`:

- **`StructuredLogEntry`** — `{ entry_id, source, level, message, metadata: Vec<(String, String)>, created_at_ms }`. Levels are `Info`, `Warning`, `Error`. Backend subsystems call `store.log(source, level, message, metadata)` to push an entry. The store keeps the most recent 300 entries (older entries are trimmed).
- **`MetricsSnapshot`** — four `u64` counters: `startup_count`, `pane_count`, `browser_operation_count`, `openflow_run_count`. Incremented via `store.increment_metric(key)`.
- **`FeatureFlags`** — `unstable_openflow`, `unstable_browser_automation`, `unstable_indexing`. Gates experimental paths; the UI reads these to decide whether to show the matching surfaces at all.
- **`PermissionPolicy`** — `require_risky_action_approval`, `allow_destructive_actions`. Consumed by OpenFlow's action review code to gate writes, deletes, and similar high-blast-radius operations behind an explicit user click.
- **`ReplayRecord`** — `{ replay_id, title, summary, created_at_ms }`. The UI keeps the most recent 50 records so the user can see past OpenFlow runs without opening the full run.
- **`SafetyConfig`** — `{ model_budget_usd, max_concurrency, auto_apply, approval_required_for_completion }`. Read by OpenFlow runtime to decide whether to pause a run that's about to exceed budget or start too many concurrent agents.

All six pieces are bundled in `ObservabilitySnapshot` and persisted as one JSON file to `dirs::data_dir() / APP_DIR_NAME / observability.json` — `codemux` for release builds, `codemux-dev` for debug builds (exact path via `snapshot_path()`). The per-build scoping matters: feature flags (including the Agent Chat GUI toggle) live in this file, and an earlier machine-shared location (`~/.codemux/observability.json`) made the toggle flip in both the installed release and a locally-running dev build at once. `load_observability_store()` reads the file on app startup — falling back once to the legacy `~/.codemux/observability.json` and copying it into the per-build location when the new file doesn't exist yet (the legacy file is left in place for older versions) — and every mutation writes the snapshot back synchronously via `save_snapshot(&snapshot)`.

## What Works Today

- One-shot `snapshot()` read returning all six pieces in a single consistent frame
- Auto-trimming of logs (300) and replay records (50) so the JSON file stays bounded
- Structured metadata on log entries (`Vec<(String, String)>`) so the frontend can render key-value tables
- Feature-flag-gated UI surfaces for OpenFlow, browser automation, and indexing
- Permission policy consumed by OpenFlow's risky-action review
- Safety config consumed by the OpenFlow runtime for budget + concurrency gating
- `add_replay_record(title, summary)` ergonomics for capturing completed run summaries
- The full `ObservabilitySnapshot` is **backend-only** — `get_observability_snapshot` has no caller in `src/`. The frontend reads only the feature-flag slice, through the narrow `get_feature_flags` command into `src/stores/feature-flags.ts`, and flips the Agent Chat GUI flag (default on) via `set_agent_chat_enabled`. Only `FeatureFlags` is mirrored into `src/tauri/types.ts`. The one-time `agent_chat_promoted` migration marker (see `promote_agent_chat_default` in `observability.rs`) that upgraded existing installs when the GUI became the default interface lives in a standalone sentinel file, `<data root>/agent_chat_promoted`, next to `observability.json` — a snapshot-only key would be erased by an older binary rewriting the file without fields it doesn't know, re-running the promotion and reverting an opt-out. The snapshot still carries a read-only `agent_chat_promoted` field for back-compat with installs promoted before the sentinel existed.

## Current Constraints

- **Single JSON file, serial writes** — every mutation reacquires the mutex, clones the snapshot, and rewrites the whole file. Fine for current scale; would need a proper write-ahead if event volume grows by 10x.
- **No multi-process safety** — the store assumes one Codemux process per user. Two processes writing simultaneously would race on the file.
- **Metrics are monotonic counters only** — no gauges, no histograms, no quantiles. The four counters are tracked for debugging, not SLO reporting.
- **No log rotation beyond the 300-entry cap** — errors older than ~300 log entries are lost forever. There's no archival to a separate file.
- **No external exporter** — the data never leaves the local machine. No OTLP, no Prometheus, no structured log shipping. That's deliberate for v1 (local-first principle), but means remote debugging requires shipping the JSON file manually.
- **Feature flags are boolean only** — no percentages, no ramps, no user targeting. A flag is either on or off for the current user.

## Native Log File (tauri-plugin-log)

Separate from `ObservabilityStore`, the desktop app installs a real
`log`-crate logger via tauri-plugin-log (registered in `lib.rs`),
writing warn-and-above to stderr **and** to a rotating file in the
platform app-log dir (`~/.local/share/com.codemux.app/logs/codemux.log`
on Linux, 2 MB cap, one rotation kept). This exists because
dependencies report real failures through the `log` crate — rfd's
"Failed to pick folder" when no dialog backend exists (issue #95) was
invisible before a logger was installed.

Support surface:

- `codemux logs [--tail <n>]` — print recent log lines, no running app
  required (`src-tauri/src/app_logs.rs`).
- `codemux doctor` — environment diagnostics including the file-dialog
  backend preflight (`src-tauri/src/doctor.rs`,
  `src-tauri/src/dialog_preflight.rs`); it reports the portal/zenity
  state and prints the same cause-specific remediation
  (`no_backend_remediation`) the in-app toast uses when no backend can
  open a dialog. When the portal is unusable but `zenity` exists,
  Codemux drives zenity itself (`src-tauri/src/dialog_fallback.rs`,
  sanitized env + timeout) rather than relying on rfd's portal path.

## Cloud-Push Diagnostic Tracing (`CODEMUX_TRACE_CLOUD_PUSH`)

The push-to-host spawn path (SSH tunnel → remote PTY daemon → agent
relaunch) is wired with detailed per-step tracing that was essential to
finding the original cross-machine bug stack. In normal operation those
lines are pure noise — a single 4-pane push would otherwise emit dozens
of `[trace:session-id] …`, `[client_for_workspace:…]`, and
`[tunnel-supervisor] …` lines — so they are gated behind an environment
variable instead of printed unconditionally.

- **Default (unset):** a normal 4-pane push emits fewer than ten lines of
  cloud-push logs, and the ones that remain are actionable — errors
  (`tunnel did not come up`, `daemon list failed`), agent-not-installed
  preflight, the connection landmarks (`[tunnel-supervisor] start` /
  `published Connected`, `daemon reached: pid=… version=…`), and the
  per-pane relaunch landmark (`remote relaunch for X: claude --resume Y`).
- **Re-enabling:** set `CODEMUX_TRACE_CLOUD_PUSH` to any value (e.g.
  `CODEMUX_TRACE_CLOUD_PUSH=1`) in the environment of the process whose
  stderr you want to inspect — the desktop app for the laptop-side trace,
  or the `codemux-remote` daemon for the host-side `[daemon::spawn]`
  trace. No recompile and no code change required; the full diagnostic
  trace returns.

Implementation: `src-tauri/src/trace.rs` defines `cloud_push_enabled()`
(a process-wide cached env check) and the `trace_cloud_push!` macro — an
`eprintln!`-compatible wrapper that emits only when the gate is on. Gated
call sites live in `terminal/daemon_backed.rs`, `ssh/registry.rs`,
`ssh/tunnel_supervisor.rs`, and `pty_daemon/server.rs`.

## Important Touch Points

- `src-tauri/src/observability.rs`:
  - Types: `LogLevel`, `StructuredLogEntry`, `MetricsSnapshot`, `FeatureFlags`, `PermissionPolicy`, `ReplayRecord`, `SafetyConfig`, `ObservabilitySnapshot`
  - Store: `ObservabilityStore::default`, `snapshot`, `log`, `increment_metric`, `set_feature_flags`, `set_permission_policy`, `set_safety_config`, `add_replay_record`
  - Persistence: `load_observability_store`, `save_snapshot`, `snapshot_path`, `trim_logs`, `trim_replays`, `default_snapshot`
- `src-tauri/src/commands/mod.rs` — Tauri commands: `get_observability_snapshot`, `add_structured_log`, `update_feature_flags`, `update_permission_policy`, `update_safety_config`, `add_replay_record`, plus the two the frontend actually calls: `get_feature_flags`, `set_agent_chat_enabled`
- `src-tauri/src/trace.rs` — `cloud_push_enabled()` + the `trace_cloud_push!` macro (`CODEMUX_TRACE_CLOUD_PUSH` gate for cloud-push diagnostics)
- `src/stores/feature-flags.ts` — the only frontend consumer; zustand store over `get_feature_flags` / `set_agent_chat_enabled`
- `src/components/settings/interface-section.tsx` — Settings → Interface toggle (plain-quit on toggle flip via `quit_app`)
- `src-tauri/src/openflow/orchestrator.rs` — reads `PermissionPolicy` and `SafetyConfig` to gate risky actions and enforce run budgets
- Persisted file: `dirs::data_dir() / APP_DIR_NAME / observability.json`, where `APP_DIR_NAME` is `codemux` for release builds and `codemux-dev` for debug builds (platform-specific: `~/.local/share/codemux/observability.json` on Linux, `%APPDATA%\codemux\observability.json` on Windows, `~/Library/Application Support/codemux/observability.json` on macOS). The legacy machine-shared `~/.codemux/observability.json` is read once and migrated forward when the per-build file doesn't exist yet.
