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
`subagent_id → kind` and re-stamps it onto every later snapshot for that id —
the same discipline as `stamp_workflow_attribution`. Without it the first
`task_progress` tick would ship a snapshot claiming the task is ordinary agent
work and flip a monitoring pane back to `Working`.

#### Two classifications, not two candidates

`SubagentSnapshot` carries a second, independent classification:
`background_task`, from the run-settlement work (see
`docs/features/agent-chat.md` § "Sidebar status indicators"). The two are
complementary, and a task can be both:

| field | derived from | always available? | means |
| --- | --- | --- | --- |
| `background_task` | the demux's top-level-launch registry | yes | not a registered `Agent`/`Task` launch, so it can outlive the turn forever and must never block settlement |
| `task_kind` | the SDK's optional `task_type` | only on a recent SDK | this is a watch loop, so the settled status should read `Monitoring` |

A background bash watch loop (`Bash { run_in_background: true }` tailing CI)
carries both, and that is the interesting case rather than a conflict. Both are
stamped by a single `stamp_task_classification` call in each `translate_task_*`
fn, so they can never drift apart.

The tracker folds them into one `TaskClass` per id, with a fixed precedence:

1. an explicit **monitor** wins over everything — the background bash watch
   loop must show the badge;
2. otherwise **background** wins over any agent claim (including a nested
   subagent's) — a row that is not a registered top-level launch must never be
   what holds the run open;
3. otherwise the provider's explicit word, or — for a snapshot that says
   nothing — whatever the tracker already knows about the id, defaulting to
   agent work for an id it has never seen.

A background row that is not a watch loop is tracked **nowhere**. That is the
whole point of the flag, and it is why `TaskClass` has no `Background` variant:
such an entry could never be drained.

#### The decision table

`ThreadSubagentState` (`commands/agent_chat.rs`) holds one
`BTreeMap<subagent_id, TaskClass>` plus a `turn_settled` flag, and
`settled_status()` is **the decision table**, read straight off those live sets:

| turn settled | force-settled | live agents | live monitors | status |
| --- | --- | --- | --- | --- |
| no | – | – | – | *no change* (the turn owns the dot) |
| yes | yes | – | – | *no change* (the watchdog already published) |
| yes | no | ≥ 1 | – | `Working` (the pre-existing deferred-review behavior, unchanged) |
| yes | no | 0 | ≥ 1 | `Monitoring` |
| yes | no | 0 | 0 | `Review` |

**Monitoring is orthogonal to settlement, not a substitute for it.** A watch
loop never defers anything. The turn settles exactly as it would without one —
the `Review` publishes at `TurnCompleted`, `run_finished()` is true, and the
workspace's detached agent browser is released on that genuine transition. All
a live monitor changes is *which* settled status is shown; when the last one
ends, the pane falls to the `Review`/`Idle` it would otherwise have had.

Two consequences follow from the badge being derived from live sets rather than
from an owed transition:

- A monitor whose **first** `Running` snapshot arrives *after* the turn already
  settled still lights the badge. (An earlier design gated it on a pending
  `Review`, which lost exactly this race.)
- A **monitor-only thread owes no `Review` at all**. `review_pending()` is
  derived as `turn_settled && !forced_settled && has_agents()`, so the 600s
  force-settle watchdog (`select_overdue_review`) cannot see such a thread by
  construction and can never knock it off its badge. For the same reason a
  monitor tick does not re-arm the watchdog's silence clock
  (`refreshes_owed_review`): a chatty watch loop must not shield a subagent
  that has genuinely gone quiet.

Mid-turn nothing changes — the parent owns `Working`, and `Monitoring` only
ever appears once the turn has settled. A new user turn, a
`SessionStateChanged::Running` reset, and session `Closed`/`Error` all clear
the whole tracker (`ThreadSubagentState::reset`). There is no TTL: monitoring
ends only via terminal task rows, a new turn, session end, or an explicit stop.

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

`agent_chat_stop_monitoring(provider, thread_id, pane_id)` (the docked bar's
**Stop**) does up to three things, independently of each other's success:

1. clears the thread's monitor set via `SubagentTracker::stop_monitoring` and
   computes the resulting status (`Working` if real agent work remains, else
   the settled `Review`). Agent tasks are deliberately left alone — Stop is
   scoped to the monitoring bar, and abandoning tracking of real in-flight work
   would strand the spinner;
2. clears any manual flag on the pane;
3. best-effort interrupts the provider session.

**`thread_id` is optional.** The two halves of the status are independent: a
pane can carry a manual flag with no chat thread bound to it at all (a terminal
pane, or a chat pane whose session never started). Steps 1 and 3 are skipped in
that case; step 2 is the part that always applies, and it is exactly where such
a pane's badge came from. Requiring a thread would leave Stop doing nothing on
the one surface that can clear the flag — pressed, spinning, never resolving.

**Stop is durable.** Clearing the monitor set is not enough on its own: the
Claude demux remembers a task's kind for the whole session and re-stamps it
onto every later tick, so a detached watch loop that survives the interrupt
would walk straight back into the monitor set on its next `task_progress` and
drift the pane back to `Monitoring` after the next turn. So the stopped ids are
**blocklisted** (`ThreadSubagentState::stopped_monitors`) and their later
snapshots are no-ops. The blocklist is scoped to the run that recorded it: a
new turn (`SessionStateChanged::Running` or the send-message `clear_thread`) or
session close drops it, so a watch loop the *next* turn starts counts normally
— including one with the same id.

**Known limitation — the interrupt is a *turn* interrupt.** With a turn in
flight it cancels the run and the watch loops die with it. With the turn
already settled — the common case, since `Monitoring` only appears *after* a
turn completes — Claude's SDK exposes no "interrupt the idle session" verb, so
the call is a no-op and a genuinely detached background process can survive it.
Codemux still clears its own state, because the alternative (refusing to turn
the badge off) leaves the user with a dot they cannot dismiss; the blocklist is
what stops that surviving process from re-lighting it. Killing it outright
needs a provider-side capability that does not exist yet. `codemux monitor
stop` has no such caveat: it only ever owned a flag.

The resulting `Review` gets the same active-workspace → `Idle` downgrade the
event path applies. `Monitoring` itself is **never** downgraded that way: the
review dot is a nag about output you can already see, while the monitoring dot
is a live fact about a process that is still running — and suppressing it in
the active workspace would hide the one surface that owns the Stop button.

`run_finished()` returns **true** for `Monitoring`, so the settle releases the
workspace's detached agent browser like any other finished run. This is the
deliberate call: a watch loop is not the run, and letting one hold the session
would put "is the run over?" back at the mercy of a task that may never
terminate — precisely the failure the `background_task` classification exists
to prevent, and it would leave the GUI-mode `LIVE` chip up indefinitely. A
monitor that genuinely needs a browser opens one; it does not squat on the
finished run's.

### Flag lifetime

A manual flag is a claim by a live process about a specific pane, so it ends
when the pane does:

- closing a pane clears its own flag (`clear_manual_monitors_for_panes`, from
  `close_pane_impl`);
- closing **or archiving** a workspace removes every pane in it in one move
  without naming them, so `AppStateStore::close_workspace` — the choke point
  both paths share — runs `prune_manual_monitors`, dropping every flag whose
  pane no longer resolves;
- `persistable_snapshot` clears the map wholesale, so nothing survives a
  restart;
- `apply_manual_monitors` additionally ignores (and drops from the emitted
  clone) a flag whose pane is gone. That is a read-side guard, not the fix —
  the prune above is what keeps the *stored* map bounded.

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
  settled (see "Stopping" above). It clears Codemux's state honestly,
  blocklists the ids so the surviving task cannot re-light the badge, and
  interrupts what it can.
- **The sidebar workspace row has no monitoring label.** A monitoring
  workspace shows the cyan dot but keeps the settled/snoozed one-liner layout,
  with no "Monitoring · 4m" line 2 of its own. That is deliberate for now: the
  row's line-2 treatment is coupled to `isCard` and to a coarse clock that only
  ticks for working / permission / freshly-settled rows, so a label there would
  either expand every babysitting row into a card or show a frozen elapsed
  time. The inbox card, rail, hover card, tab dot, palette and overview all
  carry the full treatment.
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
  `manual_monitors`, `apply_manual_monitors`, `prune_manual_monitors`,
  `start_manual_monitor` / `stop_manual_monitor` / `resolve_monitor_pane` /
  `clear_manual_monitors_for_panes`, `retain_persistable_pane_statuses`
- `src-tauri/src/agent_provider/events.rs` — `SubagentTaskKind`,
  `WATCH_LOOP_TASK_TYPES`, `classify_task_kind`, `SubagentSnapshot.task_kind`
  and `SubagentSnapshot.background_task`
- `src-tauri/src/agent_provider/claude/translate.rs` — `SubagentDemux`
  task-kind memory, `stamp_task_classification` (both fields, one call),
  `translate_task_started`
- `src-tauri/src/commands/agent_chat.rs` — `TaskClass`, `SnapshotClass`,
  `classify_snapshot`, `ThreadSubagentState` (`track` / `untrack` / `reset` /
  `review_pending` / `settled_status` / `stop_monitors`),
  `SubagentTracker::stop_monitoring`, `agent_chat_stop_monitoring`,
  `map_event_to_pane_status`, `refreshes_owed_review`,
  `select_overdue_review`, `run_finished`
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
