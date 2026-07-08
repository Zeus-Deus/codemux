# Workflow Orchestration (Claude-only)

- Purpose: Describe the current capability and constraints of the
  workflow-orchestration GUI — the in-thread card, right-panel drill-in,
  and slash command for a Claude `Workflow` tool-use run.
- Audience: Anyone working on the Claude translate layer, the agent-chat
  reducer, the right panel, or the `/workflow` command.
- Authority: Canonical feature-level reality doc.
- Update when: The `WorkflowUpdated` event shape, phase/approval linkage,
  the orchestration panel, or the Claude-only gate changes.
- Read next: `docs/features/agent-chat.md` (§ Subagent view (cross-provider)),
  `docs/features/gui-chrome.md`

## What This Feature Is

When a Claude Agent Chat session runs a top-level `Workflow` tool (a
script that spawns and coordinates many subagents across named phases),
Codemux surfaces it as a first-class run instead of a raw tool-use block:
an in-thread approval/progress/summary card, a conditional **Orchestration**
right-panel tab with per-phase agent drill-in, and a `/workflow` slash
command that inserts the trigger prompt. Design spec:
`.design/workflow-orchestration.dc.html`.

This is **Claude-only**. Codex and OpenCode never emit workflow events —
the `Workflow` tool tap lives solely in the Claude translate layer, and
the `/workflow` command shows as visible-but-disabled ("Only available
with Claude models") for the other two providers.

## Current Model

**Backend (Rust).** `src-tauri/src/agent_provider/events.rs` adds
`ProviderRuntimeEvent::WorkflowUpdated { thread_id, workflow: WorkflowSnapshot }`.
`WorkflowSnapshot` carries `workflow_id`, `status`
(`"pending_approval"|"running"|"completed"|"failed"|"stopped"`), `name`,
`description`, `script`, `phases: Option<Vec<WorkflowPhaseSnapshot>>`
(`{title, detail}`), `result_text`, `total_tokens`, `agent_count`,
`duration_ms` — every field but `workflow_id`/`status` is optional and
`#[serde(default)]`, mirroring `SubagentSnapshot`'s additive-serde
discipline. `SubagentSnapshot` itself gained two additive optional
fields, `workflow_id` and `phase`, for attributing a subagent to the
active workflow run.

`src-tauri/src/agent_provider/claude/translate.rs` taps a top-level
`Workflow` tool_use the same way it demuxes `Agent`/`Task` subagents:

- On launch, it parses the script's `export const meta = {...}` with a
  tolerant regex scanner (`parse_workflow_meta`) to recover `name`,
  `description`, and `phases`; emits `WorkflowUpdated` with status
  `"running"`; marks the tool_use id as the active workflow in per-session
  demux state; and **suppresses the generic `ToolUse` item** (the card
  replaces it).
- While a workflow is active, every top-level subagent snapshot gets its
  `workflow_id` stamped and a best-effort `phase:X` hint parsed into
  `.phase` (`stamp_workflow_attribution`) — outside a workflow run this
  is a no-op, so ordinary subagent behavior is unchanged.
- On the matching `tool_result`, it emits `WorkflowUpdated` with status
  `"completed"` or `"failed"` (from `is_error`), truncates `result_text`
  to 4000 chars, clears the active-workflow marker, and **suppresses the
  raw `ToolResult`**.

The TypeScript mirror lives in `src/tauri/events.ts` (`WorkflowSnapshot`,
`WorkflowPhaseSnapshot`, the `SubagentSnapshot.workflow_id`/`phase`
fields, and the `WorkflowUpdated` event variant).

**Frontend model.** `src/lib/agent-chat/types.ts` adds `WorkflowRunStatus`
(mirrors the Rust status union), `WorkflowPhaseView`
(`{title, detail, agents: SubagentView[]}`), and `WorkflowRunItem`
(`kind: "workflow_run"`, joined into `ChatViewItem`) carrying the run's
id, status, name/description/script, planned phases, live phases (with
routed agents), result text, token/agent counts, timestamps, and the
linked `approvalRequestId`.

`src/lib/agent-chat/reducer.ts` locates-or-creates the workflow item on
`WorkflowUpdated` (`findWorkflow` / `newWorkflowRunItem` /
`mergeWorkflowSnapshot`), and routes `workflow_id`-tagged subagent
snapshots into the matching phase (falling back to the generic
subagent card when a snapshot lacks `workflow_id`). Approval linkage:
`request_opened` matches the event's `tool_use_id` to the workflow item
and sets `approvalRequestId` + status `pending_approval`;
`request_resolved` looks the workflow up **by that same
`approvalRequestId`** and flips it to `running` (allow) or `stopped`
(deny) — the linkage is kept after resolution so `buildTranscriptSlots`
keeps suppressing the standalone permission block for that request.
`stopRunningWorkflows` flips any still-`running`/`pending_approval`
workflow to `stopped` when the turn errors out.

Pure helpers in `src/lib/agent-chat/workflows.ts` cover status merging,
per-phase and per-run stats (agent/token/elapsed rollups, current-phase
index), phase status derivation, and per-subagent finding badges parsed
from JSON-ish `result_text` (`findings`/`issues` arrays).

**UI.** `src/components/chat/WorkflowRunCard.tsx` renders full-width from
`MessageList` (like the Subagents card), branching on `item.status`:

- `pending_approval` — planned-phases list, a token-use caution strip,
  **Run once** / **Always for this project** / **View script** (dialog)
  / **Deny**. "Always for this project" calls
  `buildPermissionUpdate("project", {toolName: "Workflow"})`, which the
  SDK persists to `.claude/settings.local.json`.
- `running` — a clickable progress line ("phase i of n", running-agent
  count, progress bar) that opens the Orchestration panel.
- `completed` / `failed` / `stopped` — a clickable summary row that
  opens the panel.

The generic permission block for the workflow's own approval gate is
suppressed in the transcript — `WorkflowRunCard` owns it.

**Right panel.** `RightPanelTab` (`src/stores/ui-store.ts`) gained
`"orchestration"`. `src/components/layout/right-panel.tsx` renders that
tab **conditionally** — only when `useWorkspaceWorkflow(workspace)`
(`src/components/workflow/use-workspace-workflow.ts`, which scans every
`agent_chat` pane's thread across the workspace's surfaces for an active
or latest workflow run) finds one — with a pulsing status-working dot
while running. This is the first conditionally-rendered right-panel tab.
`workspace-main.tsx` has a stale-tab guard that coerces a persisted
`"orchestration"` selection back to `"files"` when no run exists.

The panel body (`src/components/workflow/orchestration-panel.tsx` +
`workflow-phase-list.tsx`, `workflow-agent-detail.tsx`,
`workflow-tone.ts`, `workflow-phases.ts`) shows: a run header (name,
status pill, agents/tokens/elapsed ticking stats, a disabled Pause with
a "not supported yet" tooltip, a Stop button wired to
`agentChatInterruptTurn("claude", threadId, null)`); phase cards
(expand/collapse, per-phase agent/token/elapsed rollups, status filter
chips all/running/issues); agent rows with finding badges; and an
agent drill-in (prompt, recent tool calls, result, a disabled-stub
Restart, and an "Open {file}" action via `openEditorTab` when the
agent's label looks like a repo path). Esc backs out of the drill-in.

**Command menu.** `buildWorkflowCommand` in
`src/lib/agent-chat/slash-commands.ts` is spliced into both the typed
`/` menu and the `+` menu in `Composer.tsx` under a `WORKFLOWS` group.
For the Claude provider it's enabled ("Orchestrate this task with many
subagents") and selecting it inserts `/workflow ` into the draft — no
auto-send, no frontend-side orchestration logic. For Codex/OpenCode it's
visible but `disabled` with reason "Only available with Claude models".

**Dev mock.** `src/dev/` seeds three workspaces —
`workflow-approval` / `workflow-running` / `workflow-complete` — that
replay real `WorkflowUpdated` event sequences, guarded by
`src/dev/workflow-fixture.test.ts`.

## What Works Today

- End-to-end Claude-only pipeline: `Workflow` tool_use → `WorkflowUpdated`
  events → in-thread card → conditional Orchestration right-panel tab.
- Approval flow with "Run once" / "Always for this project" (persists a
  `Workflow`-tool permission rule) / "View script" / "Deny", replacing
  the generic permission block for that request.
- Running progress card (phase i of n, running-agent count, progress
  bar) and completed/failed/stopped summary rows, both clickable into
  the panel.
- Phase-grouped agent list with status filters, per-phase and per-run
  token/agent/elapsed rollups, and per-agent finding badges parsed from
  result text.
- Agent drill-in showing prompt, recent tool calls, result text, and an
  "Open {file}" action for path-shaped activity labels.
- Stop wired to the existing `agentChatInterruptTurn` interrupt path.
- `/workflow` slash command in both the `/` and `+` composer menus,
  Claude-enabled / other-providers-disabled.
- Three seeded dev-mock workspaces replaying real event sequences for
  visual QA.

## Current Constraints

- **Claude-only.** Codex and OpenCode adapters never emit `WorkflowUpdated`
  — the tap lives solely in `claude/translate.rs`. The pill/tab never
  appear for non-Claude runs.
- **Pause is stubbed.** The panel's Pause button is always `disabled`
  with a "not supported yet" tooltip — no runtime support exists.
- **Per-agent Restart is a disabled stub** in the agent drill-in.
- **Stop interrupts the whole turn**, not just the workflow — there is
  no narrower "stop this workflow only" primitive yet.
- **Phase attribution is heuristic.** Every top-level subagent snapshot
  is attributed to the active workflow while one is running (there is no
  hard spawn-to-workflow link from the SDK), and phase assignment relies
  on a best-effort `phase:X` label hint parsed out of activity text; a
  snapshot with no `workflow_id` falls back to rendering as a normal
  generic subagent card.
- **Token/agent rollups are approximate.** Per-run totals come from the
  workflow snapshot plus each subagent's own task usage, not a single
  authoritative source.
- **`result_text` is truncated to 4000 chars** on the Rust side before
  it reaches the frontend.

## Important Touch Points

- `src-tauri/src/agent_provider/events.rs` — `ProviderRuntimeEvent::WorkflowUpdated`,
  `WorkflowSnapshot`, `WorkflowPhaseSnapshot`, the `SubagentSnapshot.workflow_id`/`phase` fields.
- `src-tauri/src/agent_provider/claude/translate.rs` — the `Workflow`
  tool tap: `parse_workflow_meta`, `stamp_workflow_attribution`,
  ToolUse/ToolResult suppression, workflow-inline unit tests
  (`#[cfg(test)]` at the bottom of the file, e.g.
  `workflow_launch_parses_meta_and_emits_running_snapshot_suppressing_tool_use`,
  `workflow_tool_result_completes_workflow_and_suppresses_tool_result`,
  `subagent_task_progress_during_active_workflow_gets_workflow_id_and_phase`).
- `src/tauri/events.ts` — TS mirror of the Rust event/snapshot types.
- `src/lib/agent-chat/types.ts` — `WorkflowRunItem`, `WorkflowPhaseView`,
  `WorkflowRunStatus`.
- `src/lib/agent-chat/reducer.ts` — `applyWorkflowUpdated`,
  `applyWorkflowSubagentUpdated`, `findWorkflow`,
  `findWorkflowByApprovalRequestId`, `stopRunningWorkflows`.
- `src/lib/agent-chat/workflows.ts` — pure stats/status/finding-badge
  helpers (`workflowRunStats`, `workflowPhaseStats`,
  `workflowPhaseStatus`, `subagentFindingBadge`, `activeWorkflowRun`,
  `latestWorkflowRun`).
- `src/lib/agent-chat/slash-commands.ts` — `buildWorkflowCommand`.
- `src/components/chat/Composer.tsx` — splices the workflow command into
  the typed-`/` menu and the `+` menu under `WORKFLOWS`.
- `src/components/chat/WorkflowRunCard.tsx` — the in-thread card.
- `src/stores/ui-store.ts` — `RightPanelTab` incl. `"orchestration"`.
- `src/components/layout/right-panel.tsx` — conditional Orchestration
  tab rendering.
- `src/components/layout/workspace-main.tsx` — stale-tab guard.
- `src/components/workflow/use-workspace-workflow.ts` — finds the
  workspace's active/latest workflow run.
- `src/components/workflow/orchestration-panel.tsx`,
  `workflow-phase-list.tsx`, `workflow-agent-detail.tsx`,
  `workflow-tone.ts`, `workflow-phases.ts` — the panel body.
- `src/dev/tauri-mock.ts`, `src/dev/mock-fixtures.ts` — the three seeded
  workflow mock workspaces/threads.
- `src/dev/workflow-fixture.test.ts` — fixture invariants guard.
- Tests: `src/components/workflow/orchestration-panel.test.tsx`,
  `src/components/chat/WorkflowRunCard.test.tsx`,
  `src/components/layout/right-panel.test.tsx`,
  `src/components/layout/workspace-main.test.tsx`,
  `src/lib/agent-chat/workflows.test.ts`,
  `src/lib/agent-chat/reducer.test.ts`,
  `src/lib/agent-chat/slash-commands.test.ts`,
  `src/dev/workflow-fixture.test.ts`, plus Composer/MessageList/settings
  test files touched by the command-menu splice.
- `.design/workflow-orchestration.dc.html` — the design handoff.

## Screenshots

Desktop-view captures from the dev-mock runtime (the three seeded
`workflow-*` workspaces), in `assets/docs/workflow-orchestration/`:

- `01-approval.png` — approval card (Run once / Always for this project /
  View script / Deny).
- `02-running-panel.png` — inline running pill + Orchestration panel
  (phases, agent rows, finding badges, status filters).
- `03-agent-detail.png` — agent drill-in (prompt, tool calls, result,
  restart stub, jump-to-file).
- `04-complete.png` — completion summary row + assistant report.
- `05-command-menu.png` — `/workflow` in the typed-`/` command menu
  (Claude-enabled).

## Notes

- Keep this file about current truth, not future plans.
- This feature completes the "design intent, not shipped" note previously
  in `docs/features/gui-chrome.md` about a workflow status pill and
  Orchestration side panel; see that file's current text.
