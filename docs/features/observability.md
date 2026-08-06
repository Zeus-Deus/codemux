# Observability

- Purpose: Describe the structured logging, metrics, and feature-flag subsystem, plus the diagnostic surfaces layered beside it (native log file, cloud-push tracing, the interaction-trace perf harness).
- Audience: Anyone working on feature flags, debug logging, diagnostics, or latency measurement.
- Authority: Canonical feature doc for `ObservabilityStore` and its persisted snapshot, and for the app's diagnostic/tracing surfaces.
- Update when: A new metric or feature flag is added, or a diagnostic/trace surface changes.
- Read next: `docs/features/settings.md`, `docs/core/TESTING.md`

## What This Feature Is

`ObservabilityStore` is a small persistent JSON store for structured app-level telemetry, counters, and feature flags. It is not a full observability pipeline: there is no metrics exporter or trace collector.

The Rust backend writes metrics and logs. The frontend reads the narrow feature-flag command surface and updates the Agent Chat interface toggle.

## Current Model

All types live in `src-tauri/src/observability.rs`:

- **`StructuredLogEntry`** — `{ entry_id, source, level, message, metadata: Vec<(String, String)>, created_at_ms }`. Levels are `Info`, `Warning`, `Error`. Backend subsystems call `store.log(source, level, message, metadata)` to push an entry. The store keeps the most recent 300 entries (older entries are trimmed).
- **`MetricsSnapshot`** — three `u64` counters: `startup_count`, `pane_count`, and `browser_operation_count`. Incremented via `store.increment_metric(key)`.
- **`FeatureFlags`** — browser/indexing experimental flags plus the paired `enable_agent_chat` and `enable_lazy_workspace_creation` interface flags.

The snapshot is persisted as one JSON file at `dirs::data_dir() / APP_DIR_NAME / observability.json` — `codemux` for release builds and `codemux-dev` for debug builds. Unknown fields from older snapshots are ignored on read and disappear on the next save. `load_observability_store()` falls back once to the legacy `~/.codemux/observability.json` location and copies it forward when needed.

## What Works Today

- One-shot `snapshot()` read returning logs, counters, and flags in a consistent frame
- Auto-trimming of logs to 300 entries so the JSON file stays bounded
- Structured metadata on log entries (`Vec<(String, String)>`) so the frontend can render key-value tables
- Feature flags for browser automation, indexing, Agent Chat, and lazy workspace creation
- The full `ObservabilitySnapshot` is **backend-only** — `get_observability_snapshot` has no caller in `src/`. The frontend reads only the feature-flag slice, through the narrow `get_feature_flags` command into `src/stores/feature-flags.ts`, and flips the Agent Chat GUI flag (default on) via `set_agent_chat_enabled`. Only `FeatureFlags` is mirrored into `src/tauri/types.ts`. The one-time `agent_chat_promoted` migration marker (see `promote_agent_chat_default` in `observability.rs`) that upgraded existing installs when the GUI became the default interface lives in a standalone sentinel file, `<data root>/agent_chat_promoted`, next to `observability.json` — a snapshot-only key would be erased by an older binary rewriting the file without fields it doesn't know, re-running the promotion and reverting an opt-out. The snapshot still carries a read-only `agent_chat_promoted` field for back-compat with installs promoted before the sentinel existed.

## Current Constraints

- **Single JSON file, serial writes** — every mutation reacquires the mutex, clones the snapshot, and rewrites the whole file. Fine for current scale; would need a proper write-ahead if event volume grows by 10x.
- **No multi-process safety** — the store assumes one Codemux process per user. Two processes writing simultaneously would race on the file.
- **Metrics are monotonic counters only** — no gauges, histograms, or quantiles. The counters are tracked for debugging, not SLO reporting.
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

## Interaction Trace Harness (`codemux:perf-trace`)

Separate again from both stores: a frontend harness that makes a slow
workspace switch *attributable* rather than merely slow. It exists because
"the app feels laggy" was never actionable — the 2026-07-31 responsiveness
audit needed to say which of backend, delivery, hydration, React commit,
terminal cleanup, or background contention owned each millisecond. See
`docs/plans/gui-responsiveness.md`.

- **One id spans the whole interaction.** `src/lib/perf/interaction-trace.ts`
  stamps seven phases under a single interaction id: `click`, `invoke-start`,
  `invoke-returned`, `snapshot-received`, `state-committed`, `pane-mounted`,
  `painted` — the last written from a **double `requestAnimationFrame`** after
  the mount, which is the closest a page can get to "the user saw it". Spans
  are derived from consecutive pairs (`invoke`, `delivery`, `commit`, `mount`,
  `paint`). The trace *closes* at `state-committed` rather than at `painted`,
  because optimistic selection paints before the round trip finishes. Marks are
  mirrored into User Timing as `codemux/<kind>#<id>/<phase>` so a browser
  profile lines up with the log.
- **Wiring.** `instrumented-activate.ts` opens the trace at every activation
  site, `use-app-state.ts` marks delivery and commit (without changing the
  debounce), `pane-container.tsx` marks the mount and arms the paint stamp, and
  `TerminalPane.tsx` records a `terminal-teardown` sub-measure around the
  scrollback serialize.
- **Off by default, and short-circuited when off.** The gate is
  `localStorage["codemux:perf-trace"] === "1"` or a dev build; one boolean
  check exits every entry point otherwise. When on, `window.codemuxPerf`
  exposes `{ getTraces, clearTraces, summarize, renderer, export }`.
- **Bounded and self-cleaning.** A 100-trace ring buffer; a trace with no
  progress for 10 s is marked `abandoned` rather than leaked; long tasks
  arriving up to 1 s late are back-attributed to the trace they belong to.
- **Long-task observation is feature-detected.** A `PerformanceObserver` over
  `longtask` and `long-animation-frame`, filtered through
  `PerformanceObserver.supportedEntryTypes` — WebKitGTK may advertise neither,
  and asking for an unsupported type throws. Observers start on the first open
  trace and stop when the last one closes.
- **`summarizeTraces()`** reports per-kind `count` / `abandoned` plus
  `p50` / `p95` / `max` (nearest-rank) for the total and each span, and long
  tasks per trace — the shape the § "Product Budgets" gates in the plan are
  written against.
- **`exportDiagnostics()`** returns a versioned envelope (`version: 2`) of
  `enabled`, `observedEntryTypes`, `renderer`, `traceCount`, `summaries` and
  the raw traces. The `renderer` section is what makes an outlier arrive with
  engine evidence attached: `userAgent`, `webkitVersion`,
  `webkitReleaseVersion`, `linuxWebKitGtk`, `devicePixelRatio`, and the
  terminal WebGL verdict (`{ use, reason, glRenderer }`) read from the probe's
  **cache** — it never triggers a probe. Mark metadata is typed
  `Record<string, number | boolean>`, so no path, project name, branch, or
  transcript content can reach an export; the only string carried is the
  interaction's `target` (an internal workspace id).

**Backend section timings** ride the normal stderr/log path under
`[codemux::perf::*]` tags, each gated so a healthy run stays quiet:

- `[codemux::perf::emit] snapshot=<ms> serialize=<ms> workspaces=<n>` — emitted
  only when the two together exceed 8 ms, splitting the state deep-clone from
  serde.
- `[codemux::perf::job] git-sweep|pr-poll|port-poll tick=<ms>ms` — only when a
  tick exceeds 50 ms.
- `[codemux::perf::job] git-fetch tick=<s>s repos=<n> (drain outlasted the 60s
  period)` — only when the bounded fetch drain overruns its own period.
- Adjacent, not `perf::`-tagged: `[codemux::workspace] activate_workspace(<id>)
  returned in <ms>ms (mutate=… emit=… persist=…)`, gated above 8 ms.

**Measuring against something real.** The curated dev seed is 19 workspaces,
far below the audited profile, so numbers taken on it flatter everything.
`src/dev/stress-fixture.ts` scales the dev-mock seed to `small` / `medium` /
`large` / `xl` (the audited profile: 80 workspaces, 5,000 persisted events,
~15 MB of tool-result payload) via `?fixture=` or
`localStorage["codemux:fixture"]`, with inline JSON for one-off shapes. See
`docs/features/dev-mock-runtime.md` § "Stress fixtures".

## Important Touch Points

- `src/lib/perf/interaction-trace.ts` — the interaction-trace ring buffer,
  phase/span model, long-task observers, `summarizeTraces`, `exportDiagnostics`
  and the `window.codemuxPerf` handle; `src/lib/perf/instrumented-activate.ts` —
  `activateWorkspaceInteraction`, the single traced activation entry point
  (which also owns the optimistic pending-selection rollback and its 5 s
  timeout)
- `src-tauri/src/observability.rs`:
  - Types: `LogLevel`, `StructuredLogEntry`, `MetricsSnapshot`, `FeatureFlags`, `ObservabilitySnapshot`
  - Store: `ObservabilityStore::default`, `snapshot`, `log`, `increment_metric`, `set_feature_flags`
  - Persistence: `load_observability_store`, `save_snapshot`, `snapshot_path`, `trim_logs`, `default_snapshot`
- `src-tauri/src/commands/mod.rs` — snapshot/log/feature-flag commands, including the two frontend callers: `get_feature_flags` and `set_agent_chat_enabled`
- `src-tauri/src/trace.rs` — `cloud_push_enabled()` + the `trace_cloud_push!` macro (`CODEMUX_TRACE_CLOUD_PUSH` gate for cloud-push diagnostics)
- `src/stores/feature-flags.ts` — the only frontend consumer; zustand store over `get_feature_flags` / `set_agent_chat_enabled`
- `src/components/settings/interface-section.tsx` — Settings → Interface toggle (plain-quit on toggle flip via `quit_app`)
- Persisted file: `dirs::data_dir() / APP_DIR_NAME / observability.json`, where `APP_DIR_NAME` is `codemux` for release builds and `codemux-dev` for debug builds (platform-specific: `~/.local/share/codemux/observability.json` on Linux, `%APPDATA%\codemux\observability.json` on Windows, `~/Library/Application Support/codemux/observability.json` on macOS). The legacy machine-shared `~/.codemux/observability.json` is read once and migrated forward when the per-build file doesn't exist yet.
