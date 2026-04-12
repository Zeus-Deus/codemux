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

All six pieces are bundled in `ObservabilitySnapshot` and persisted as one JSON file to `dirs::data_dir() / codemux / observability.json` (exact path via `snapshot_path()`). `load_observability_store()` reads the file on app startup; every mutation writes the snapshot back synchronously via `save_snapshot(&snapshot)`.

## What Works Today

- One-shot `snapshot()` read returning all six pieces in a single consistent frame
- Auto-trimming of logs (300) and replay records (50) so the JSON file stays bounded
- Structured metadata on log entries (`Vec<(String, String)>`) so the frontend can render key-value tables
- Feature-flag-gated UI surfaces for OpenFlow, browser automation, and indexing
- Permission policy consumed by OpenFlow's risky-action review
- Safety config consumed by the OpenFlow runtime for budget + concurrency gating
- `add_replay_record(title, summary)` ergonomics for capturing completed run summaries
- Frontend types ported 1:1 via `src/tauri/types.ts` and exposed through `src/stores/observability-store.ts`

## Current Constraints

- **Single JSON file, serial writes** — every mutation reacquires the mutex, clones the snapshot, and rewrites the whole file. Fine for current scale; would need a proper write-ahead if event volume grows by 10x.
- **No multi-process safety** — the store assumes one Codemux process per user. Two processes writing simultaneously would race on the file.
- **Metrics are monotonic counters only** — no gauges, no histograms, no quantiles. The four counters are tracked for debugging, not SLO reporting.
- **No log rotation beyond the 300-entry cap** — errors older than ~300 log entries are lost forever. There's no archival to a separate file.
- **No external exporter** — the data never leaves the local machine. No OTLP, no Prometheus, no structured log shipping. That's deliberate for v1 (local-first principle), but means remote debugging requires shipping the JSON file manually.
- **Feature flags are boolean only** — no percentages, no ramps, no user targeting. A flag is either on or off for the current user.

## Important Touch Points

- `src-tauri/src/observability.rs`:
  - Types: `LogLevel`, `StructuredLogEntry`, `MetricsSnapshot`, `FeatureFlags`, `PermissionPolicy`, `ReplayRecord`, `SafetyConfig`, `ObservabilitySnapshot`
  - Store: `ObservabilityStore::default`, `snapshot`, `log`, `increment_metric`, `set_feature_flags`, `set_permission_policy`, `set_safety_config`, `add_replay_record`
  - Persistence: `load_observability_store`, `save_snapshot`, `snapshot_path`, `trim_logs`, `trim_replays`, `default_snapshot`
- `src-tauri/src/commands/mod.rs` — Tauri commands: `get_observability_snapshot`, `add_structured_log`, `update_feature_flags`, `update_permission_policy`, `update_safety_config`, `add_replay_record`
- `src/stores/observability-store.ts` — Zustand store consuming the snapshot
- `src-tauri/src/openflow/orchestrator.rs` — reads `PermissionPolicy` and `SafetyConfig` to gate risky actions and enforce run budgets
- Persisted file: `dirs::data_dir() / codemux / observability.json` (platform-specific: `~/.local/share/codemux/observability.json` on Linux, `%APPDATA%\codemux\observability.json` on Windows, `~/Library/Application Support/codemux/observability.json` on macOS)
