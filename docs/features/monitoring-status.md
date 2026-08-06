# Monitoring status (background watch loops)

- Purpose: Describe the calm `Monitoring` agent status — what puts a workspace into it, what takes it out, and where it renders.
- Audience: Anyone working on pane status, the sidebar inbox, the agent chat pane, or the control/CLI surface.
- Authority: Canonical feature-level reality doc for `PaneStatus::Monitoring`.
- Update when: The detection rules, the combination choke point, the stop semantics, or the surfaces that render it change.
- Read next: `docs/features/agent-chat.md` (§ Sidebar status indicators), `docs/features/sidebar.md`, `docs/reference/CONTROL.md`

## What This Feature Is

An agent can finish its main deliverable and still be babysitting something:
watching CI checks, tailing a background process, polling a PR. Before this
existed, Codemux had no vocabulary for that. Claude's background tasks sat in
the subagent tracker and pinned the sidebar at **Working** forever, and every
other provider showed nothing at all — the workspace looked finished while a
watch loop was still running.

`Monitoring` is a first-class fifth pane status for exactly that state. It is
deliberately *calm*: a steady cyan dot, no pulse, no spinner, no promotion to
the top of the inbox. A workspace babysitting CI overnight should be something
the eye can rest on, not something that keeps asking for attention.

## Current Model

### The status itself

`PaneStatus::Monitoring` (`src-tauri/src/state/state_impl.rs`, serialized
`"monitoring"`) sits in one priority ladder shared verbatim with
`src/lib/pane-status.ts`:

```
permission > working > monitoring > review > idle
```

It outranks `Review` because a workspace with a live watch loop is still
*doing* something; it never outranks `Working`/`Permission` because nobody has
to look at it, and a badge that hides a permission prompt would be a bug with a
UI.

It is **not persisted**. `retain_persistable_pane_statuses` keeps `Review`
only, so `Monitoring` is dropped on save exactly like `Working`/`Permission` —
a background watch loop with no process behind it is not monitoring anything,
it is a lie the sidebar would tell forever.

Being non-idle, it stamps `last_active_at` (`stamp_workspace_activity`), which
is correct: a babysat workspace is not stale.

### Two ways in

**1. Automatic (Claude): watch-loop task classification.**

Codemux's Claude sidecar forwards the SDK's `system.task_started /
task_progress / task_updated / task_notification` messages, which
`agent_provider/claude/translate.rs` turns into `SubagentUpdated` events. A
`task_started` message may carry a `task_type` string; it is read
opportunistically (`opt_str_field(msg, "task_type")`) and classified by
`classify_task_kind` against a closed set of watch-loop types:

```
"monitor" | "monitor_mcp" | "local_bash" | "shell"
```

Everything else — including a missing or unknown `task_type` — classifies as
`SubagentTaskKind::Agent`. **If the installed SDK does not send `task_type`,
everything is agent work and behavior degrades gracefully to what it was before
this feature existed.** That is the intended failure mode, not an oversight:
mis-labelling a monitor as an agent shows a spinner that is a bit too eager,
while the reverse would stop telling the user that real work is in flight.

Only `task_started` carries the field, so `SubagentDemux` remembers
`subagent_id → kind` and `stamp_task_kind` re-stamps it onto every later
snapshot for that id — the same discipline as `stamp_workflow_attribution`.
Without it the first `task_progress` tick would ship a snapshot claiming the
task is ordinary agent work and flip a monitoring pane back to `Working`.

`ThreadSubagentState` (`commands/agent_chat.rs`) splits its live set in two —
`running` (agent tasks) and `monitors` (watch loops) — and
`ThreadSubagentState::settled_status()` holds **the decision rule** in one
place, applied whenever the parent turn is not running:

| live agent tasks | live monitors | published status |
| --- | --- | --- |
| ≥ 1 | any | `Working` (the pre-existing deferred-review behavior, unchanged) |
| 0 | ≥ 1 | `Monitoring` — **replaces** the owed `Review` while monitors live |
| 0 | 0 | `Review` (consuming the owed transition) |

`review_pending` deliberately survives the `Monitoring` state: when the last
watch loop goes terminal, the owed `Review` is finally published.

Mid-turn nothing changes — the parent owns `Working`, and `Monitoring` only
ever appears once the turn has settled. A new user turn, a
`SessionStateChanged::Running` reset, and session `Closed`/`Error` all clear
**both** sets (`ThreadSubagentState::reset`). There is no TTL: monitoring ends
only via terminal task rows, a new turn, session end, or an explicit stop.

A watch-loop row keeps its transcript card — the user should be able to see
*what* is being watched — but is excluded from the docked
`SubagentActivityBar`'s roster (`runningSubagentEntries` filters on
`isMonitorTask`). It is not a subagent doing work, and counting it there is
what used to keep the amber progress bar up for a thread whose only remaining
activity was a CI poll.

**2. Manual (any provider): `codemux monitor start`.**

Runtime-only per-pane flags live in `AppStateSnapshot.manual_monitors`
(`pane_id → Option<reason>`). Any agent with a shell can set one, whatever it
is running under — a terminal agent, a Codex or OpenCode chat agent, something
Codemux has no event stream for at all.

The flags are emitted to the frontend (the chat bar needs the *reason*) but
cleared by `persistable_snapshot` before `layout.json` is written, alongside
`detected_ports`.

### The combination choke point

The two halves meet in exactly one function: `apply_manual_monitors`
(`state_impl.rs`), called by all three snapshot accessors — `snapshot()`,
`stamped_snapshot()`, `snapshot_at_current_revision()`. It folds the flags into
`pane_statuses` on the way *out*:

- `Working` / `Permission` underneath → the flag is ignored. An agent that is
  mid-run or blocked on the user is not "quietly watching".
- anything else (absent / `Idle` / `Review` / `Monitoring`) → `Monitoring`.
- a flag whose pane has closed is dropped rather than trusted — pane ids are
  the only thing tying a flag to anything real, and a badge on a pane that no
  longer exists cannot be turned off from the UI.

Combining on **read** rather than at write time is the load-bearing choice: the
stored status stays the raw thing the provider published, so `monitor stop`
reveals whatever the pane was really doing underneath instead of having to
remember it separately.

One place repeats the combination, because it bypasses the snapshot path:
`clear_transient_pane_status_by_session_delta` ships a single `PaneStatus`
delta rather than a whole snapshot, so it re-checks the flag itself. Otherwise
a flagged terminal agent that went quiet would have its badge cleared by the
stuck-status sweep instead of settling to `Monitoring`.

The Claude tracker path writes `Monitoring` straight into `pane_statuses` and
needs no flag at all.

### Stopping

`agent_chat_stop_monitoring(provider, thread_id)` (the docked bar's **Stop**)
does three things, independently of each other's success:

1. clears the thread's monitor set from `SubagentTracker::stop_monitoring` and
   computes the resulting status (`Working` if real agent work remains, else
   the owed `Review`, else `Idle`). Agent tasks are deliberately left alone —
   Stop is scoped to the monitoring bar;
2. clears any manual flag on the thread's pane;
3. best-effort interrupts the provider session.

**Known limitation — the interrupt is a *turn* interrupt.** With a turn in
flight it cancels the run and the watch loops die with it. With the turn
already settled — the common case, since `Monitoring` only appears *after* a
turn completes — Claude's SDK exposes no "interrupt the idle session" verb, so
the call is a no-op and a genuinely detached background task can survive it.
Codemux still clears its own state, because the alternative (refusing to turn
the badge off) leaves the user with a dot they cannot dismiss. Killing a
detached task outright needs a provider-side capability that does not exist
yet. `codemux monitor stop` has no such caveat: it only ever owned a flag.

The resulting `Review` gets the same active-workspace → `Idle` downgrade the
event path applies. `Monitoring` itself is **never** downgraded that way: the
review dot is a nag about output you can already see, while the monitoring dot
is a live fact about a process that is still running — and suppressing it in
the active workspace would hide the one surface that owns the Stop button.

`run_finished()` also returns `false` for `Monitoring`, so a monitoring agent
keeps its background browser session: it may well be driving it as part of the
very thing it is watching.

## What Works Today

- Sidebar inbox card, rail dot, hover card, tab dot, command palette and the
  Workspaces overview all render **Monitoring** in a dedicated cyan token with
  **no animation**.
- Monitoring cards keep the background-recede treatment (like quietly-working
  ones), never raise the needs-you strip, and never resurface a settled or
  snoozed workspace.
- Settle and Snooze are **allowed** on a monitoring workspace. A watch loop can
  run for hours and "I know, wake me later" is the whole reason a user would
  park it; the guardrail is only against live and blocked agents.
- A docked `MonitoringBar` between transcript and composer, in the same slot
  and on the same chat column rails as `SubagentActivityBar`, with the reason
  when one was given and a **Stop** button whose "Stopping…" state resolves
  when the status actually leaves `monitoring`.
- `codemux monitor start [--reason <text>] [--pane-id <id>]`,
  `codemux monitor stop`, `codemux monitor status` — reading
  `CODEMUX_PANE_ID` / `CODEMUX_WORKSPACE_ID` from the agent's own injected
  environment, so the common call takes no arguments.
- Socket commands `monitor_start` / `monitor_stop` / `monitor_status`.

## Current Constraints

- **Automatic detection is Claude-only, and only when the SDK reports
  `task_type`.** Codex and OpenCode never set `task_kind`, so their agents
  reach `Monitoring` through the manual CLI path alone.
- **Stop cannot kill a detached background task** once the parent turn has
  settled (see "Stopping" above). It clears Codemux's state honestly and
  interrupts what it can.
- A manual flag set by an agent that then **crashes** survives until
  `monitor stop`, the pane closes, or the app restarts. Nothing polls the
  claiming process for liveness.
- Manual flags are **per pane, not per agent**. Two agents sharing one pane
  share one flag, and either one's `monitor stop` clears it.
- Pane resolution for the socket commands is deliberately dumb: explicit
  `pane_id` → the workspace's active surface's active pane → its first pane in
  tree order. It does not try to find "the agent pane".

## Important Touch Points

- `src-tauri/src/state/state_impl.rs` — `PaneStatus::Monitoring`,
  `manual_monitors`, `apply_manual_monitors`, `start_manual_monitor` /
  `stop_manual_monitor` / `resolve_monitor_pane` /
  `clear_manual_monitors_for_panes`, `retain_persistable_pane_statuses`
- `src-tauri/src/agent_provider/events.rs` — `SubagentTaskKind`,
  `WATCH_LOOP_TASK_TYPES`, `classify_task_kind`, `SubagentSnapshot.task_kind`
- `src-tauri/src/agent_provider/claude/translate.rs` — `SubagentDemux`
  task-kind memory, `stamp_task_kind`, `translate_task_started`
- `src-tauri/src/commands/agent_chat.rs` — `ThreadSubagentState`
  (`track` / `untrack` / `reset` / `settled_status`),
  `SubagentTracker::stop_monitoring`, `agent_chat_stop_monitoring`,
  `map_event_to_pane_status`, `run_finished`
- `src-tauri/src/control.rs` — `monitor_start` / `monitor_stop` /
  `monitor_status`
- `src-tauri/src/cli.rs` — `MonitorCommand`
- `src/lib/pane-status.ts` — priority, label, tone and dot classes
- `src/globals.css` — `--status-monitoring` + `--color-status-monitoring`
- `src/components/chat/MonitoringBar.tsx`, mounted in `AgentChatPane.tsx`
- `src/lib/agent-chat/subagents.ts` — `isMonitorTask`,
  `runningSubagentEntries`
- `src/components/layout/sidebar-inbox-card.tsx`,
  `sidebar-rail-workspaces.tsx`, `src/components/ui/status-indicator.tsx`,
  `src/components/workspaces-overview/workspace-overview-row.tsx`
- `scripts/e2e/monitoring-status-e2e.sh` — headless end-to-end coverage of the
  provider-agnostic path
- `src/dev/mock-fixtures.ts` — the `monitoring-demo` workspace
  (`MOCK_MONITORING_THREAD_ID`)

## Notes

- The dev mock seeds a `monitoring-demo` workspace (project `codemux`, branch
  `demo/monitoring`, PR #482) whose chat thread carries a finished turn plus a
  live `task_kind: "monitor"` watch loop, plus a `manual_monitors` reason
  ("CI checks on PR #482"). `npm run dev` shows both the sidebar badge and the
  docked bar without needing a live provider; the mock's
  `agent_chat_stop_monitoring` clears them.
- Nothing that carries `--status-monitoring` animates. If a future surface adds
  a monitoring affordance, keep it steady — the pulse belongs to `permission`
  alone.
