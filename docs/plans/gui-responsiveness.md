# GUI Responsiveness Plan

- Purpose: Track the implementation of the 2026-07-31 responsiveness audit — bound renderer work, resume chat by cursor, paint selection optimistically, and schedule background work away from interaction.
- Audience: Anyone working on workspace switching, Agent Chat rendering/hydration, terminal lifecycle, or backend refresh loops.
- Authority: Active work plan only, not current truth.
- Update when: A phase lands, an exit gate is measured, or evidence changes the priority order.
- Read next: `docs/features/agent-chat.md`, `docs/features/terminal.md`, `docs/features/sidebar.md`, `docs/core/STATUS.md`
- Status: MOSTLY LANDED (manual verification matrix + measured exit gates left)

## Goal

Make Codemux feel immediate at real profile scale (audited profile: 79 live workspaces, 161 chat threads, 74k persisted events, 165 MB chat payload, 4,317-event largest thread) without a rewrite: keep Tauri + Rust + SQLite, keep raw chat history, keep terminal recovery and mirror/web behavior. The audit verdict: the backend is not the frame — the long poles are unbounded renderer work, replay-from-origin chat hydration, synchronous terminal lifecycle work, and periodic jobs colliding with interaction.

## Active Priorities

All seven phases are implemented and unit-covered.

**First measured A/B evidence exists (2026-08-01, dev-mock renderer):** identical
browser-driven scripts against this tree vs the branch-point baseline
(`5ce9397a`), both serving the `xl` stress fixture (80 workspaces / 5,000-event
thread) through the dev mock in Chromium-CDP (`agent-browser`), dev-mode React:

| Metric (renderer pipeline) | baseline | this tree | budget |
|---|---|---|---|
| workspace switch click → tab paint, p50 (n=30) | 671.6 ms | **123.1 ms** | ≤ 100 ms |
| workspace switch, p95 | 893.0 ms | **183.1 ms** | max < 200 ms |
| composer keystroke → paint in 5,000-event thread, p50 (n=40) | 42.0 ms | **19.8 ms** (double-rAF floor) | ≤ 32 ms |
| keystroke p95 / max | 127.2 / 128.0 ms | **21.1 / 21.7 ms** | no task > 50 ms |
| warm chat revisit click → chat interactive, p50 (n=8) | 126.4 ms | **55.5 ms** | ≤ 200 ms |
| warm chat revisit max | 287.1 ms | **86.3 ms** | — |

Caveats that keep priority 1 open: this measures the **renderer-side pipeline
only** (the dev mock's in-page IPC is nearly free, so backend wins — emit
gating, IPC-thread unblocking, deferred PTY spawn — are invisible here); it is
Chromium, not the WebKitGTK the budgets target; and dev-mode React inflates
absolute numbers on both sides (ratios are the signal: ~5.5× switch p50,
~6× keystroke p95, ~2.3× warm revisit p50).

1. **Run the stress-fixture measurement matrix on a real desktop session.** On
   Linux WebKitGTK **and one non-Linux platform**, with `codemux:perf-trace` on
   and the `xl` fixture loaded: ≥30 workspace switches, plus the draft-input,
   warm-revisit and 100-deltas/s cases. Report p50/p95/max from
   `summarizeTraces()` against § "Product Budgets" and attach
   `exportDiagnostics()` for any outlier. The dev-mock A/B above proves the
   renderer-side wins; this step covers the backend + WebKitGTK half.
2. **Decide React Compiler from those traces, not before.** The trial protocol
   and its four required measurements are specified under § "Renderer/compiler
   follow-up" below. It needs the step-1 numbers as its "before".
3. **Close or consciously keep the deferred items** listed below.

### Deferred, with reasons

- **Persisted cold-start projection cache (Phase 3's third bullet) — not
  built, deliberately.** The byte-weighted warm-slice LRU landed; the durable
  cache did not, because the reducer is **not a pure function of the rows**:
  it stamps wall-clock times, carries a module-global id counter, branches on
  `runLive` / `provider`, applies trim caps, and runs tail-dependent post-passes.
  A projection keyed on a row range would therefore not be verifiable as
  equivalent to a replay, and a wrong cached transcript is a far worse failure
  than a slow one. Revisit only behind a reducer-determinism fix plus a real
  reducer version — not as a performance patch.
- **`WorkflowPhaseList` still takes a coarse `now` prop.** The Orchestration
  panel's header moved to `TickingText` (1 Hz, zero commits); the per-phase
  labels did not, because the value crosses a component boundary. They run on a
  5 s tick instead — a few seconds of precision on a secondary readout for a
  12× drop in panel re-renders. Converting them means moving the elapsed
  computation into the row.
- **Two latent lazy-tool-result notes** (confirmed in code, harmless *today*):
  the stub's doc comments in `lazy-tool-result.ts` and `commands/agent_chat.rs`
  describe fetching the body "on expand **or copy**", but no copy-result path
  exists in the chat UI; and `grepHitCount` (`activity-steps.ts`) reads
  `result_content` to label a Grep step "N hits", so an oversized Grep result
  sees the stub and falls back to "done". Both turn into real bugs the moment a
  copy-result, transcript-search, or export surface lands — any new consumer of
  a tool body must go through `agent_chat_get_tool_result`.

## Audit Findings (baseline v0.15.6 / `ba9061f6`)

Six bottlenecks, first four on confirmed user-action paths:

1. **P0 — selection has no optimistic commit.** The sidebar wraps an async invoke in `startTransition` with no synchronous state update inside it; visible selection waits for the backend snapshot, JSON IPC, the 16 ms coalescer (`ec336a87`), and the next pane mount.
2. **P0 — hydration starts at event zero.** A remounted chat pane fetches every stored event, may reduce the history twice (a ~33 ms warm preflight replay on top of a 90–117 ms first replay for a 2,425-event thread), and uses rendered message count where a durable DB cursor is needed.
3. **P0 — CSS containment is not virtualization.** `content-visibility` skips some layout/paint but every transcript slot stays a mounted React element and DOM node (1,271 message items for that thread). True Virtuoso virtualization existed in June (`c4fb241a`) and was removed in July's redesign (`0339af8a`). **Superseded on HEAD:** commit `6a298021` (2026-07-31, same day as the audit) re-virtualized the transcript with `@legendapp/list` 3.2.0 — stable slot keys, dynamic measurement, `maintainScrollAtEnd`, `alwaysRender` opt-outs. Phase 2 is therefore mostly landed; what remains is the exit-gate verification plus the memoization gaps around the list (unmemoized `ChatTranscript`/`MessageList`/`MessageTrail`, inline `ListHeaderComponent` element, per-render `keyExtractor`/`itemsAreEqual` closures).
4. **P0 — typing and stream tokens enter the whole chat pane.** The pane subscribes to the whole thread slice; draft keystrokes and each provider delta re-enter the broad tree; streamed tails clone/reconcile settled history too frequently.
5. **P1 — terminal teardown can precede the new paint.** Switching away from recently active output may synchronously serialize xterm scrollback before disposal (intermittent — the idle optimization hides it).
6. **P1 — periodic work is unscheduled contention.** Git sweep, ports, resource metrics, PR/fetch, and CWD housekeeping run on independent timers; broad git refresh considers every workspace; phases can align with a click.

## Phases (each with a measurable exit gate)

### Phase 0 — Make latency attributable
- One interaction ID marks click → invoke → snapshot receipt → active-ID commit → pane mount → double-rAF paint.
- Time backend snapshot/emit, chat rows/bytes/query, terminal cleanup, every periodic job.
- Feature-detected Long Task / Long Animation Frame observation; privacy-safe diagnostic export.
- Stress fixtures: 1–80 workspaces, 50–5,000 events, 1–15 MB payloads, 20–100 deltas/s, active/idle terminals. ≥30 switches, report p50/p95/max.
- **Exit:** every slow switch attributable to backend, delivery, hydration, React commit, terminal cleanup, or background contention.

### Phase 1 — Paint selection now; isolate chat updates
- Synchronous pending active-workspace ID; invoke Rust after; rollback on error; reconcile by revision.
- Remove ineffective async `startTransition` wrappers (selection stays urgent).
- Split timeline / composer / header / activity / request state into focused selector boundaries; memoize transcript rows with stable callbacks.
- Batch content deltas at display cadence; flush terminal events immediately; separate streaming tail.
- **Exit:** selection paints before IPC returns; a keystroke renders no timeline rows; 100 deltas/s → bounded commits with exact final text.

### Phase 2 — True transcript virtualization (mostly landed via `6a298021`)
- The LegendList window already exists (`MessageList.tsx:408-455`: `keyExtractor` by slot key, `getItemType`, `itemsAreEqual` identity, `estimatedItemSize 112`, `drawDistance 800`, `maintainScrollAtEnd`, `maintainVisibleContentPosition`, `alwaysRender` keys). Do **not** reintroduce react-virtuoso.
- Remaining work: `memo()` `ChatTranscript`, `MessageList`, `MessageTrail`/`TrailRail`; hoist `keyExtractor`/`itemsAreEqual` to module scope; memoize the `ListHeaderComponent` element (the footer already is); memoize `shouldShowThinkingIndicator`.
- MessageTrail already driven from `onFirstVisibleItemChanged`. Preserve all behaviors (Markdown/tool expansion, request/subagent cards, images, free scroll, bottom anchoring, jump-to-latest, copy, keyboard focus).
- **Exit:** 50 and 5,000-event histories both mount only ~20–40 visible/overscan rows; full behavior matrix passes.

### Phase 3 — Resume chat by cursor; stop shipping collapsed megabytes
- Durable DB row IDs + `after_id` tail query; `lastPersistedEventId` stored with the materialized thread.
- Remove the preflight replay/count comparison; one ordered tail reduction.
- Byte-weighted LRU/TTL of warm recent/running projections; reducer-versioned cold-start projection cache (invalidate, don't duplicate the reducer in Rust).
- Keep raw events; hydrate large collapsed tool results as metadata, fetch bodies on expand/copy.
- Test the attach/head-cursor/live-drain race for duplicates and gaps.
- **Exit:** warm unchanged revisit does no full-history work; a cold 13 MB thread doesn't transfer 13 MB of collapsed output; catch-up is exact.

### Phase 4 — Move terminal teardown after the target paint
- Detach the old terminal immediately; park dirty active terminals in a tiny weighted LRU or defer serialize/dispose to post-paint idle, under a memory cap.
- Keep backend history recovery and deterministic memory eviction.
- Test: key latency, active output, malformed reattach, dimensions, theme, WebGL/context loss, 100 switch cycles — the prior cache rollback cases (`14735bf` → rolled back `2baa42f`; disabled code retained at `src/components/terminal/terminal-cache.ts`).
- **Exit:** old-terminal serialization contributes no >50 ms task before new selection paint; memory bounded.

### Phase 5 — Replace timer collisions with a scheduler
- Replace the ~5 s all-workspace git sweep with activation/visibility/settlement triggers; dedupe per repo root; concurrency 1–2.
- Emit only on metadata change. Jitter/stagger; pause/degrade nonessential work while hidden or during a measured interaction.
- Cheap cached resource summary when the monitor is closed; detailed process/`smaps_rollup` work only when open or slowly.
- CWD pruning triggered by session-set changes, not scans of every saved layout.
- **Exit:** no overlapping job, no unchanged full-state refresh, no periodic task causing an interaction task >50 ms.

### Phase 6 — Split app-state domains; then harden the renderer (landed)
- The revisioned full snapshot stays the boot / resync / activation vehicle. Three high-frequency domains moved to `app-state-delta` (`{ revision, delta }`): `WorkspaceGit { workspace_id, git }` from the git sweep's per-workspace change path, `DetectedPorts { ports }` from the 3 s port poll, and `PaneStatus { pane_id, status }` from the two mutation sites whose entire operation is that one clear (`hooks.rs` agent-exit monitor, `clear_agent_status`).
- **What deliberately stayed on full snapshot emits**, because the same emit carries other domains and a partial delta would leave the renderer's copy wrong: PR pill / linked-issue refresh (git sweep tail); `publish_pane_status` (also stamps `workspace.last_active_at` and may release a detached agent browser); `set_pane_status_by_session` (stamps `last_active_at`); `agent_chat_close_pane` (also removes the pane); `record_workspace_switch` and `activate_tab` (also move selection / focus / notification state). Correctness of the ordered stream beats coverage — a delta domain can be widened later, an inconsistent renderer cannot be undone.
- Deltas and snapshots share ONE `AtomicU64`, stamped under the state lock (`AppStateStore::stamped_snapshot` / `mutate_with_delta`), so a delta at revision N reflects state at N and a snapshot at N supersedes every delta ≤ N. `get_app_state` reports the revision it already reflects and consumes none — a read must not punch a hole in the sequence.
- The renderer applies deltas immediately (no 16 ms window), flushing any debounced snapshot first so the debounce can never reorder the stream. A non-contiguous revision is treated as a dropped message and opens a single-flight full resync; deltas arriving during it are dropped, since the snapshot on its way supersedes them. No delta variant carries `active_workspace_id`, so background refreshes cannot clobber the optimistic selection.
- A 60 s jittered `app-state-revision { revision }` heartbeat (counter only, no snapshot build) restores eventual consistency for any emit lost in delivery — cheap insurance for the 5 s full-snapshot resync that Phase 5's change-gating removed.
- Sidebar narrowed to `workspaces` / `pane_statuses` / a `hasAppState` boolean; `SidebarInboxCard`, `SettledRow` and `SnoozeRow` memoized with `useCallback` handlers over a latched ref and interned `InboxRepo` objects. `useCoarseClock` now holds its timestamp in state instead of returning a per-render `Date.now()`, which had been defeating every memo boundary beneath it.
- `exportDiagnostics()` gained a `renderer` section: UA, WebKit/WebKitGTK version, devicePixelRatio, and the terminal WebGL verdict + reason read from the probe's cache (never triggering a probe).
- React Compiler was **not** trialled — see the follow-up note under Open Questions for what was prepared and what a trial must measure.
- **Exit:** metadata updates touch only domain subscribers (asserted in `sidebar-inbox-delta.test.tsx`); gap recovery is exact; remaining outliers carry renderer-health evidence.

## Product Budgets (test on Linux WebKitGTK + one non-Linux platform)

- ≤ 100 ms p95 workspace click → selected shell/header paint; max < 200 ms
- ≤ 200 ms p95 workspace click → warm chat or terminal usable
- ≤ 32 ms p95 draft input → next paint in a 2,500-event thread; no draft task > 50 ms
- 20–40 mounted transcript rows across 50–5,000 stored events
- 0 full replays revisiting a warm unchanged thread
- 0 overlap per background job; unchanged results emit no state

## Suggested PR slices

1. harness + immediate selection; 2. chat boundaries + stream batching; 3. virtual timeline; 4. cursor hydration + lazy payloads; 5. terminal critical path; 6. background scheduler; 7. domain state protocol; 8. renderer/compiler follow-up.

## Exact Touch Points (from code research, 2026-08-01)

**Selection (Phase 1):**
- Click handlers: `sidebar-inbox-card.tsx:131-136`, `sidebar-inbox.tsx:293-299`, `sidebar-rail-workspaces.tsx:46-51`, `sidebar-needs-you-strip.tsx:57-62`, `sidebar-project-group.tsx:183`, `use-keyboard-shortcuts.ts:170-190`, `command-palette.tsx:96`, `sidebar-ports-popover.tsx:115`, `ResourceMonitor.tsx:118`, `workspace-overview-row.tsx:225` — all fire-and-forget `activateWorkspace()`; the `startTransition` wrappers are inert (no sync state update inside).
- `src/stores/app-store.ts:36-59` — no local active id; `setAppState` + `shareStructural`. Optimistic pending-ID + revision reconcile goes here.
- `src/hooks/use-app-state.ts:38-46` — trailing-edge 16 ms `setTimeout` debounce that can be starved by streaming emits; the activate snapshot must bypass or flush it.
- Backend: `src-tauri/src/commands/workspace.rs:1583-1684` (`activate_workspace_impl` — note the full snapshot clone at `:1622` just to read a cwd, the sync SQLite write at `:1663`), `state_impl.rs:751-796` (`record_workspace_switch`), `state_impl.rs:4055-4064` (`emit_app_state`, full deep clone + serde), `:4089-4140` (16 ms `EmitDebouncer`). No revision counter exists anywhere — `schema_version` is a compile-time constant.
- `cycle_workspace` (`workspace.rs:1866-1884`) diverges: sync PTY spawn on IPC thread, no git refresh, no persistence.
- Pane switch: `pane-container.tsx:13-30` (only active surface mounted, unkeyed `PaneNode`), `workspace-main.tsx:85,222`.

**Chat rendering (Phases 1–2):**
- `AgentChatPane.tsx:456-462` — the whole-slice subscription (draft + messages + streaming in one). Split into field selectors; `useShallow` pattern proven at `use-workspace-workflow.ts:57-80`.
- `agent-chat-store.ts:308-317` (`updateSlice` clones the threads map per keystroke/delta), `:406-411` (`setInputDraft`), `:329-363` (`applyEvent`).
- Delta path has zero batching: `use-agent-chat-events.ts:33-69` → `AgentChatPane.tsx:791-794` → store; one `set()` + one render per token. Add rAF-cadence coalescing for `content_delta` only (terminal events flush immediately).
- `MessageList.tsx` unmemoized shell (`:125`), inline `ListHeaderComponent` (`:450-454`), per-render `keyExtractor`/`itemsAreEqual` (`:411-413`); `ChatTranscript.tsx:54,71`; `MessageTrail.tsx:54,75`.
- Slot identity machinery to preserve: `transcript-slots.ts:172-194` (`reuseTranscriptSlots`), `SlotRowMemo` (`MessageList.tsx:882`).

**Hydration (Phase 3):**
- DB: `agent_chat_messages` has an autoincrement `id` + `idx_agent_chat_messages_thread(thread_id, id ASC)` — cursor-ready, no migration needed. `database.rs:292-303`, `:2965-2978` (`list_agent_chat_messages` — add `(id, payload)` + `after_id` variant).
- Command: `agent_chat.rs:2534-2540` (`agent_chat_list_messages`), wrapper `commands.ts:1665`.
- Double-reduce hydrate: `AgentChatPane.tsx:824-877` (preflight replay at `:835` discarded except `messages.length`; count-guard at `:841` is not a durable cursor), `agent-chat-store.ts:469-486` (`hydrateThread` replays again). `replayPayloads` (`hydrate.ts:85-200`) + `lastTurnUnsettled` (`hydrate.ts:211-233`) parse every payload up to 4×.
- Attach/hydrate race: events between the DB read (`:832`) and `hydrateThread` (`:855`) are clobbered; events during the attach in-flight window are dropped after persistence (`agent_chat.rs:2734-2748`). Track `lastPersistedEventId` in the slice; tail-fetch `after_id` on reattach.
- Lazy tool bodies: `ToolResult.content` ships verbatim (`events.rs:201-205`); collapsed cards render ≤12 lines (`ToolCallBlock.tsx:6,27-31`) but hold full bodies. Note `ToolCallCard.tsx:95` needs image-shape metadata to auto-expand.
- Projection-cache invalidation hazards: replay is not a pure function of rows — `runLive`/`provider` options, wall-clock stamping (`reducer.ts:441,464,476,503`), module-global `idCounter` (`reducer.ts:56-60`), trim caps, tail-dependent post-passes.

**Terminal (Phase 4):**
- `TerminalPane.tsx:964-1054` — synchronous unmount cleanup; the blocking piece is `buildFreshOrCached("unmount")` (`:1029-1035`) → sync serialize when the idle cache is dirty (`:536-545`). Alt-screen skip already exists (`:582-587`); idle serializer (`scrollback-idle-serializer.ts`) already avoids most cost.
- Prior regressions to respect: `14735bf3` → `4d90f9f9` (N× keystroke cost, WebGL 16-context cap) → `acd15664` → rollback `2baa42f6`; also `83a50a2d` (write pump), `d6562ffd` (idle serializer). Disabled cache at `terminal-cache.ts` + inert `use-terminal-cache-gc.ts`/`use-terminal-theme-sync.ts` (mounted `App.tsx:77-78`).
- Backend recovery guarantees: `terminal/mod.rs:2325-2382` (`attach_pty_output` replay, generation-scoped detach), 256 MiB `pending_output` cap (`:176`).

**Background jobs (Phase 5):**
- `lib.rs:1016-1141` — 5 s serial all-workspace git sweep, **emits every tick unconditionally** (`:1134-1136`); includes active-workspace PR refresh.
- `lib.rs:1317-1364` — 60 s `git fetch` sweep, **unbounded `tokio::spawn` per workspace** (`:1342`).
- `lib.rs:1373-1541` — 60 s PR poll, sequential, change-gated emit but unconditional stderr tick log (`:1537-1539`).
- `lib.rs:1548-1573` — 3 s port poll (change-gated ✅). `lib.rs:1599-1631` — 30 s workspaces-sync.
- Frontend: `use-terminal-cwd-poll.ts:74-82` (`pruneCwds` walks every pane at 2 Hz), `DiffPane.tsx:43` + `changes-panel.tsx:338` (unconditional setState per tick), 1 s chat "now" clocks (`SubagentActivityBar.tsx:328`, `WorkflowRunCard.tsx:427`, `SubagentsCard.tsx:296`, `StreamingMarker.tsx:32`).
- Resource metrics: frontend-driven React Query 2 s open / 15 s closed (`ResourceMonitor.tsx:78-85`); each call walks the full process table (`resource_metrics.rs:365-366`) + smaps_rollup.

**State protocol (Phase 6):**
- `state_impl.rs:616-638` (`AppStateSnapshot` — ships `config`/`persistence`/`archived_workspaces` every tick), `:4055-4064`, `:4089-4140`; `structural-share.ts` (the frontend's O(n) defense); `sidebar-inbox.tsx:739` (whole-appState subscription), `SidebarInboxCard` unmemoized (`sidebar-inbox-card.tsx:81`).

## Non-Goals

- No renderer/stack rewrite; no Electron migration. Electron is not the explanation — bounded and resumable state is.
- No chat history deletion or lossy migration of stored events.
- No duplicating the TS event reducer in Rust.
- No removal of mirror/web-remote fan-out behavior or terminal backend recovery.

## Open Questions

- Where the byte-weighted warm projection cache lives (frontend module vs Rust) — decided during Phase 3 from measurements.
- Whether React Compiler adoption (Phase 6 tail) is worth it on WebKitGTK — decide from traces only after selector boundaries are clean.

### Renderer/compiler follow-up (deferred out of Phase 6)

Phase 6 prepared the ground for a compiler trial but deliberately did not run one: the decision is a *measurement*, and the measurement only means anything on a real desktop session (WebKitGTK, a real profile, real interaction traces). A headless verdict would be a guess wearing a number.

**What is now in place:**

- Selector boundaries are narrow. `sidebar-inbox.tsx` subscribes to `workspaces`, `pane_statuses` and a `hasAppState` boolean instead of the whole snapshot; the two guard-only effects key off the boolean.
- Reference stability holds through both delivery paths — structural sharing on full snapshots, targeted replacement on deltas — so an unchanged workspace keeps its identity across a tick.
- Manual memoization is retained on purpose: `SidebarInboxCard`, `SettledRow` and `SnoozeRow` are `memo()`d and their callback props are `useCallback`'d over a latched ref. Do **not** strip these as part of a compiler trial — the trial's whole question is whether the compiler can *replace* them, which is only answerable by removing them in a branch and comparing.
- `useCoarseClock` no longer returns a per-render `Date.now()` (it held state), which was silently defeating every memo boundary below it.
- `exportDiagnostics()` now carries a `renderer` section (UA, WebKit/WebKitGTK version, devicePixelRatio, terminal WebGL verdict + reason), so an outlier trace arrives with engine evidence attached.

**What a trial must measure**, on the Phase 0 fixtures at real profile scale (79 workspaces), before/after, on Linux WebKitGTK and one non-Linux platform:

1. Workspace click → paint p50/p95/max, against the ≤100 ms p95 budget.
2. Commit counts per backend tick — specifically, whether the compiler's inferred memoization holds the "one delta re-renders one card" property that `sidebar-inbox-delta.test.tsx` asserts today.
3. Semantics, not just timing: the latched-ref handlers, the store-mutation-during-render patterns, and every `useSyncExternalStore` boundary Zustand installs are exactly the shapes a compiler can change behavior on.
4. Build cost and bundle delta.

Adopt only if (1) improves materially **and** (2) is preserved with the manual memos removed. Consider another renderer only if the same stress matrix stays materially slower after that.

## Already Landed

All seven phases plus a review-fix round, all 2026-08-01, all unreleased. Current
behavior for each now lives in the feature doc named on its line — keep it there,
not here.

- **Phase 0 — interaction-trace harness.** `src/lib/perf/interaction-trace.ts` (7-phase trace under one id, double-rAF paint stamp, 100-trace ring, feature-detected `longtask`/LoAF observers, p50/p95/max, `exportDiagnostics()` with a `renderer` section), `[codemux::perf::emit]` / `[codemux::perf::job]` backend section timings, and `src/dev/stress-fixture.ts`. Gated on `localStorage["codemux:perf-trace"]` or a dev build; no behavior changed. → `docs/features/observability.md`, `docs/features/dev-mock-runtime.md`.
- **Phase 1 — optimistic selection.** `pendingActiveWorkspaceId` / `selectActiveWorkspaceId` paint the click in its own task; all ~15 activation sites route through `activateWorkspaceInteraction` (`instrumented-activate.ts`) with id-scoped rollback and a 5 s backstop; the confirming snapshot bypasses the 16 ms coalescer; `cycle_workspace` reached parity via the shared `run_activation_side_effects` (async PTY spawn, git refresh, persistence). → `docs/features/sidebar.md` § "Selection and update cost".
- **Phase 1b — chat isolation.** Field-level pane subscriptions (draft / timeline / settings / attachments), rAF-batched `content_delta` with synchronous flush for every other event kind (`event-batcher.ts`), and `TickingText` for 1 Hz elapsed labels. → `docs/features/agent-chat.md`.
- **Phase 2 — transcript memoization.** The LegendList window from `6a298021` kept; `ChatTranscript` / `MessageList` / `MessageTrail` memoized, `keyExtractor`/`itemsAreEqual` hoisted to module scope, header element memoized. Covered by `MessageList.memoization.test.tsx` + `transcript-keystroke-isolation.test.tsx`. → `docs/features/agent-chat.md` § "Transcript scroller".
- **Phase 3 — cursor hydration + lazy payloads.** `agent_chat_list_messages_after` + `lastPersistedEventId`, one ordered reduction (`cursor-hydrate.ts`), hold/attach/read/filtered-release ordering (`attach-registry.ts`), cold-empty-read wipe guard, persisted-id and `client_nonce` dedup, byte-weighted warm-slice LRU, and 32 KiB tool-result stubs fetched on expand (`lazy-tool-result.ts`). The durable cold-start projection cache was **not** built — see § "Deferred, with reasons". → `docs/features/agent-chat.md` § "Hydration by cursor".
- **Phase 4 — deferred terminal teardown.** `deferred-teardown.ts` parks serialize/dispose past the new selection's paint, ≤2 jobs, same-session flush before remount, WebGL contexts bounded. → `docs/features/terminal.md`.
- **Phase 5 — background scheduler.** `jobs.rs` (active-every-tick + 6-tick stride sweep, per-cwd dedupe, blocking-pool gathers, startup jitter), `git fetch` capped at 2 concurrent with `kill_on_drop`, hidden-window gating, log gating, `get_resource_metrics(detail)` cheap summary, `poll-equality.ts` on the frontend polls. Every loop is change-gated, so an idle fleet emits nothing — which is what removed the implicit 5 s full-snapshot resync that Phase 6's heartbeat replaces.
- **Phase 6 — domain deltas + renderer hardening.** `app-state-delta` (`workspace_git` / `detected_ports` / `pane_status`) and a jittered 60–70 s `app-state-revision` heartbeat share one lock-stamped `AtomicU64`; the frontend does gap detection with a 100 ms reorder buffer, buffer-during-resync and contiguous replay; the sidebar subscribes narrowly and memoizes its rows (`sidebar-inbox-delta.test.tsx`). React Compiler documented as a follow-up, not adopted. → `docs/features/sidebar.md`, `docs/features/web-remote-access.md` § "Event hub".
- **Review-fix round.** `snapshot_revision` zeroed in `persistable_snapshot()` (a persisted stamp seeded a restored store above the live counter — the web-remote freeze) and the web-remote seed switched to `snapshot_at_current_revision()`; terminal-exit pane-status clear now emits a `PaneStatus` delta and a previously-missing emit was added for session-status changes; ~10 mutate-without-emit sites became change-gated emitters (workspaces adopt/clone flows, `control.rs` browser automation, adapter captures, hooks); trace finalize semantics fixed.

## Notes

- Reference patterns audited from a comparable agent-desktop codebase (2026-07-31): LegendList-style virtual timeline, `afterSequence` warm resume, granular state atoms, local-routing-first navigation, React Compiler. Adopt the patterns, not the stack — Codemux keeps its own transcript implementation on Rust + SQLite.
- Prior art in-repo: `74bf5e4c` (mount-time IPC roundtrips), `ec336a87` (16 ms emit coalescing), `5828f538` (markdown re-render scoping), terminal write pump, disabled terminal cache.
- Attach a before/after p50/p95/max from the Phase 0 fixtures to every optimization PR; stop at each exit gate.
