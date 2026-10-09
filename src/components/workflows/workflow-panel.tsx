import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronLeft, Code2, Loader2, Pause, Play, Plus, RotateCw, Square, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Eyebrow } from "@/components/ui/eyebrow";
import { cn } from "@/lib/utils";
import { useWorkflowUIStore, type WorkflowDraft } from "@/stores/workflow-ui-store";
import { DEFAULT_WORKFLOW_LIMITS, workflowArtifactPreview, workflowError, workflowIntegrateArtifact, type WorkflowRunSnapshot, type WorkflowTaskSnapshot, type WorkflowTaskStatus } from "@/tauri/workflows";
import { useWorkflowRuntime } from "./use-workflow-runtime";
import { DEFAULT_WORKFLOW_INSPECTOR, useWorkflowInspectorStore } from "@/stores/workflow-inspector-store";

import { WorkflowSetup } from "./workflow-setup";
import { WORKFLOW_STARTER } from "./workflow-starter";
import { checkpointGroups, currentTaskAttempt, isSnapshotArtifact, taskRouteLabel, taskStatusLabel, runStatusLabel, taskVerification, workflowCapacity } from "./workflow-presentation";

export { WORKFLOW_STARTER } from "./workflow-starter";

const SELECT = "h-8 min-w-0 rounded-md border border-border bg-background px-2 text-body-sm text-foreground focus-visible:outline-2 focus-visible:outline-ring";
const TASK_PAGE = 100;

function initialDraft(): WorkflowDraft {
  return {
    title: "Workspace review",
    goal: "Review this workspace and identify concrete improvements.",
    source: WORKFLOW_STARTER,
    allowWrites: false,
    routes: [{ id: "claude", provider: "claude" }, { id: "codex", provider: "codex" }],
    limits: { ...DEFAULT_WORKFLOW_LIMITS },
  };
}

function statusClass(status: string) {
  if (status === "succeeded" || status === "completed") return "text-status-open";
  if (status === "failed" || status === "unknown" || status === "blocked") return "text-status-attention";
  if (status === "running" || status === "waiting" || status === "stopping") return "text-status-working";
  return "text-muted-foreground";
}

export function WorkflowPanel({ workspaceId }: { workspaceId: string }) {
  const runtime = useWorkflowRuntime(workspaceId);
  const saved = useWorkflowUIStore((state) => state.drafts[workspaceId]);
  const setDraft = useWorkflowUIStore((state) => state.setDraft);
  const draft = useMemo(() => saved ?? initialDraft(), [saved]);
  const update = (patch: Partial<WorkflowDraft>) => setDraft(workspaceId, { ...draft, ...patch });
  const run = runtime.snapshot.data;
  const launch = (mode: "dry_run" | "live") => runtime.launch.mutate({
    source: draft.source,
    spec: { workspace_id: workspaceId, title: draft.title.trim(), goal: draft.goal.trim(), mode, allow_writes: draft.allowWrites ?? false, routes: draft.routes.map((route) => ({ ...route, model: route.model?.trim() || null, effort: route.effort?.trim() || null })), limits: draft.limits, tasks: [] },
  });
  const errors = [runtime.launch.error, runtime.control.error, runtime.save.error, runtime.runs.error, runtime.snapshot.error, ...(!runtime.selectedId ? [runtime.capabilities.error, runtime.scripts.error] : [])].filter(Boolean);

  return (
    <section className="flex h-full min-h-0 min-w-0 flex-col bg-background" data-testid="workflow-panel" aria-label="Dynamic workflows">
      <div className="flex shrink-0 items-center gap-2 border-b border-border/60 px-3 py-2">
        <select aria-label="Workflow run" data-testid="workflow-run-picker" className={cn(SELECT, "flex-1")} value={runtime.selectedId ?? ""} onChange={(event) => runtime.selectRun(event.target.value || null)}>
          <option value="">New workflow</option>
          {(runtime.runs.data ?? []).map((item) => <option key={item.id} value={item.id}>{item.spec.title} · {runStatusLabel(item.status)}</option>)}
        </select>
        <Button variant="ghost" size="icon-sm" aria-label="New workflow" data-testid="workflow-new" onClick={() => runtime.selectRun(null)}><Plus /></Button>
      </div>
      {errors.length > 0 && <div role="alert" className="shrink-0 border-b border-status-attention/30 bg-status-attention/8 px-3 py-2 break-words text-body-sm text-status-attention">{workflowError(errors[0])}</div>}
      <div className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden">
        {runtime.selectedId ? (
          run ? <RunInspector key={run.id} run={run} busy={runtime.control.isPending} onEditScript={() => { update({ title: run.spec.title, goal: run.spec.goal, source: run.spec.script?.source ?? draft.source, routes: run.spec.routes, limits: run.spec.limits, allowWrites: run.spec.allow_writes }); runtime.selectRun(null); }} onControl={(action, taskId, text) => runtime.control.mutate({ action, runId: run.id, taskId, text })} />
            : <p role="status" className="p-4 text-body-sm text-muted-foreground">{runtime.snapshot.isLoading ? "Loading workflow…" : "This run could not be loaded. Choose another run or create a new workflow."}</p>
        ) : (
          <WorkflowSetup
            draft={draft}
            capabilities={runtime.capabilities.data ?? []}
            scripts={runtime.scripts.data ?? []}
            pending={runtime.launch.isPending}
            saving={runtime.save.isPending}
            saved={runtime.save.isSuccess}
            onChange={update}
            onLaunch={launch}
            onSave={() => runtime.save.mutate({ name: draft.title.trim(), source: draft.source })}
          />
        )}
      </div>
    </section>
  );
}

type ControlAction = "pause" | "resume" | "cancel" | "retry" | "retire" | "message" | "cancel_task" | "reconcile";
function RunInspector({ run, busy, onControl, onEditScript }: { onEditScript: () => void; run: WorkflowRunSnapshot; busy: boolean; onControl: (action: ControlAction, taskId?: string, text?: string) => void }) {
  const context = useWorkflowInspectorStore((state) => state.runs[run.id] ?? DEFAULT_WORKFLOW_INSPECTOR);
  const updateContext = useWorkflowInspectorStore((state) => state.update);
  const { selectedTask, settledOpen, visible } = context;
  const setSelectedTask = (id: string | null) => updateContext(run.id, { selectedTask: id });
  const setSettledOpen = (open: boolean) => updateContext(run.id, { settledOpen: open });
  const setVisible = (update: (counts: Record<string, number>) => Record<string, number>) => updateContext(run.id, { visible: update(visible) });
  const [wide, setWide] = useState(false);
  const [scriptResultOpen, setScriptResultOpen] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const focusedTask = useRef<string | null>(selectedTask);
  const restoreFocus = useRef(false);
  const focusWithin = useRef(false);
  const previousSelection = useRef<string | null>(null);
  const previousWide = useRef(false);
  const groups = useMemo(() => checkpointGroups(run), [run]);
  const capacity = useMemo(() => workflowCapacity(run), [run]);
  const task = run.tasks.find((item) => item.spec.id === selectedTask);
  const active = ["running", "paused", "unknown", "stopping"].includes(run.status);
  const runCancelled = run.cancel_requested || run.status === "cancelled";
  const received = run.tasks.filter((item) => taskVerification(run, item).resultAccepted).length;
  const schemaAccepted = run.tasks.filter((item) => taskVerification(run, item).schemaAccepted).length;
  const prefix = useId();

  useLayoutEffect(() => {
    const element = panel.current;
    if (!element) return;
    // Retained selection can render on a remounted pane. Choose its layout
    // before paint so an expanded inspector never flashes a single column.
    setWide(element.getBoundingClientRect().width >= 720);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => setWide(entry.contentRect.width >= 720));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const trackFocus = (event: Event) => { focusWithin.current = event.target instanceof Node && !!panel.current?.contains(event.target); };
    const trackPointer = (event: Event) => { if (!(event.target instanceof Node) || !panel.current?.contains(event.target)) focusWithin.current = false; };
    const leaveWindow = () => { focusWithin.current = false; };
    document.addEventListener("focusin", trackFocus, true);
    document.addEventListener("pointerdown", trackPointer, true);
    window.addEventListener("blur", leaveWindow);
    return () => {
      document.removeEventListener("focusin", trackFocus, true);
      document.removeEventListener("pointerdown", trackPointer, true);
      window.removeEventListener("blur", leaveWindow);
    };
  }, []);

  useLayoutEffect(() => {
    const element = panel.current;
    if (!element) return;
    const selectedChanged = previousSelection.current !== selectedTask;
    const enteredSingle = previousWide.current && !wide;
    previousSelection.current = selectedTask;
    previousWide.current = wide;
    if (task && (selectedChanged || enteredSingle)) {
      const title = element.querySelector<HTMLElement>("[data-workflow-detail-title]");
      if (selectedChanged || focusWithin.current) title?.focus({ preventScroll: true });
      const detail = element.querySelector<HTMLElement>('[data-testid="workflow-task-detail"]');
      if (detail) detail.scrollTop = 0;
      detail?.scrollIntoView?.({ block: wide ? "nearest" : "start" });
    } else if ((wide || !task) && (restoreFocus.current || (focusWithin.current && focusedTask.current && document.activeElement === document.body))) {
      if (!settledOpen && groups.settled.some((item) => item.spec.id === focusedTask.current)) {
        restoreFocus.current = true;
        setSettledOpen(true);
        return;
      }
      const focusedGroup = ([
        ["needs-you", groups.needsYou], ["ready-review", groups.readyToReview], ["in-flight", groups.inFlight], ["settled", groups.settled],
      ] as const).find(([, items]) => items.some((item) => item.spec.id === focusedTask.current));
      if (focusedGroup) {
        const [group, items] = focusedGroup;
        const index = items.findIndex((item) => item.spec.id === focusedTask.current);
        if (index >= (visible[group] ?? TASK_PAGE)) {
          restoreFocus.current = true;
          setVisible((counts) => ({ ...counts, [group]: Math.ceil((index + 1) / TASK_PAGE) * TASK_PAGE }));
          return;
        }
      }
      const row = [...element.querySelectorAll<HTMLButtonElement>("[data-workflow-task-id]")].find((item) => item.dataset.workflowTaskId === focusedTask.current);
      row?.focus({ preventScroll: true });
      row?.scrollIntoView?.({ block: "nearest" });
    }
    restoreFocus.current = false;
  }, [run.revision, selectedTask, settledOpen, visible, wide]);

  const selectTask = (id: string) => {
    focusedTask.current = id;
    setSelectedTask(id);
  };
  const back = () => {
    if (groups.settled.some((item) => item.spec.id === selectedTask)) setSettledOpen(true);
    restoreFocus.current = true;
    setSelectedTask(null);
  };
  const taskRows = (items: WorkflowTaskSnapshot[], group: string) => <ul className="-mx-1.5">
    {items.slice(0, visible[group] ?? TASK_PAGE).map((item) => <li key={item.spec.id}>
      <button
        type="button"
        data-testid={`workflow-task-${item.spec.id}`}
        data-workflow-task-id={item.spec.id}
        aria-pressed={selectedTask === item.spec.id}
        onFocus={() => { focusedTask.current = item.spec.id; }}
        onClick={() => selectTask(item.spec.id)}
        className={cn("grid w-full min-w-0 grid-cols-[14px_minmax(0,1fr)_auto] items-start gap-x-2 rounded-md px-1.5 py-2 text-left hover:bg-surface-2 focus-visible:outline-2 focus-visible:outline-ring", selectedTask === item.spec.id && "bg-surface-2")}
      >
        <TaskIcon status={item.status} />
        <span className="min-w-0 break-words text-body-sm font-medium">{item.spec.title}</span>
        <span className={cn("pt-0.5 text-label", statusClass(item.status))}>{taskStatusLabel(item)}</span>
        <span className="col-span-2 col-start-2 mt-0.5 min-w-0 truncate text-label text-muted-foreground">{taskRouteLabel(run, item)}{item.status === "waiting" ? " · waiting for results, slot released" : ""}</span>
      </button>
    </li>)}
    {items.length > (visible[group] ?? TASK_PAGE) && <li><Button variant="ghost" size="xs" className="mt-1" data-testid={`workflow-more-${group}`} onClick={() => {
      focusedTask.current = items[visible[group] ?? TASK_PAGE]?.spec.id ?? null;
      restoreFocus.current = true;
      setVisible((counts) => ({ ...counts, [group]: (counts[group] ?? TASK_PAGE) + TASK_PAGE }));
    }}>Show {Math.min(TASK_PAGE, items.length - (visible[group] ?? TASK_PAGE))} more tasks</Button></li>}
  </ul>;
  const checkpointSection = (title: string, key: string, items: WorkflowTaskSnapshot[]) => items.length > 0 && <section aria-labelledby={`${prefix}-${key}`} data-testid={`workflow-${key}`} className="border-b border-border/50 px-3.5 py-3">
    <div className="mb-1.5 flex items-center gap-2"><h3 id={`${prefix}-${key}`} className="text-label font-medium text-muted-foreground">{title}</h3><span className="font-mono text-caption tabular-nums text-muted-foreground">{items.length}</span></div>
    {taskRows(items, key)}
  </section>;

  return <div ref={panel} className="min-w-0" data-testid="workflow-run-inspector" data-layout={wide && task ? "split" : "single"} onKeyDown={(event) => {
    if (event.key === "Escape" && task && !["INPUT", "TEXTAREA", "SELECT"].includes((event.target as HTMLElement).tagName)) {
      event.stopPropagation();
      back();
    }
  }}>
    <div className="space-y-2.5 border-b border-border/60 px-3.5 py-3">
      <div className="flex items-start gap-3"><h2 className="min-w-0 flex-1 break-words text-body-lg font-semibold">{run.spec.title}</h2><span data-testid="workflow-run-status" data-status={run.status} className={cn("shrink-0 pt-0.5 text-label", statusClass(run.status))}>{runStatusLabel(run.status)}</span></div>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-label text-muted-foreground"><span>{run.spec.mode === "dry_run" ? "Dry run · no tokens" : "Live agents"}</span>{run.script?.phase && <span className="min-w-0 break-words"><span aria-hidden="true">· </span>{run.script.phase}</span>}</div>
      <div className="flex flex-wrap items-center gap-1.5">
        {run.status === "running" && <Button size="xs" variant="ghost" data-testid="workflow-pause" disabled={busy} onClick={() => onControl("pause")}><Pause />Pause</Button>}
        {run.status === "paused" && <Button size="xs" variant="ghost" data-testid="workflow-resume" disabled={busy} onClick={() => onControl("resume")}><Play />Resume</Button>}
        {active && <Button size="xs" variant="ghost" data-testid="workflow-cancel" disabled={busy || run.status === "stopping"} onClick={() => onControl("cancel")}><Square />{run.status === "stopping" ? "Stopping…" : "Cancel run"}</Button>}
      </div>
      {run.error && <p role="alert" className="break-words text-body-sm text-status-attention">{run.error}</p>}
      {run.script?.error && <p role="alert" className="break-words text-body-sm text-status-attention">{run.script.error}</p>}
      {run.status === "paused" && <p className="text-label text-muted-foreground">New work is paused. Already running workers can finish.</p>}
      {run.status === "unknown" && <p className="text-label text-status-attention">A worker’s outcome needs confirmation. Its slot stays held until its stop is verified.</p>}
      {runCancelled && <p className="text-label text-muted-foreground">This run was cancelled. Results remain available; retry and application are disabled.</p>}
    </div>
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border/60 px-3.5 py-2 text-label" data-testid="workflow-capacity" aria-label={capacity.unconfirmed ? `${capacity.held} confirmed occupied worker slots, ${capacity.unconfirmed} workers awaiting admission accounting` : `${capacity.held} of ${capacity.ceiling} worker slots occupied${capacity.unknown ? `, ${capacity.unknown} awaiting confirmation` : ""}`}>
      <span className="text-muted-foreground">Workers <span className="ml-1 font-mono tabular-nums text-foreground">{capacity.unconfirmed ? "—" : capacity.held} / {capacity.ceiling}</span></span>
      <span className="flex items-center gap-1" aria-hidden="true">{capacity.markers.map((state, index) => <span key={index} className={cn("h-1.5 w-3 rounded-[2px]", state === "active" ? "bg-status-working" : state === "unknown" || state === "unconfirmed" ? "bg-status-attention" : "bg-border")} />)}{capacity.overflow > 0 && <span className="ml-0.5 font-mono text-caption text-muted-foreground">+{capacity.overflow} slots</span>}</span>
      {capacity.unknown > 0 && <span className="text-status-attention">{capacity.unknown} awaiting confirmation</span>}{capacity.unconfirmed > 0 && <span className="text-muted-foreground">Admission accounting incomplete</span>}
    </div>
    <div className={cn(wide && task && "grid grid-cols-[minmax(260px,0.9fr)_minmax(320px,1.1fr)]")}>
      {(wide || !task) && <div className={cn("min-w-0", wide && task && "border-r border-border/60")} data-testid="workflow-checkpoints">
        {checkpointSection("Needs you", "needs-you", groups.needsYou)}
        {checkpointSection("Ready to review", "ready-review", groups.readyToReview)}
        {checkpointSection("In flight", "in-flight", groups.inFlight)}
        {run.tasks.length === 0 && <p role="status" className="px-3.5 py-4 text-body-sm text-muted-foreground">{active ? "The script is preparing its first tasks…" : "This run has no tasks."}</p>}
        {groups.needsYou.length === 0 && run.tasks.length > 0 && <p className="px-3.5 py-2.5 text-label text-muted-foreground">{run.status === "completed" ? "The run finished." : "Nothing needs your attention."}{groups.readyToReview.length > 0 ? " File changes are waiting for your review." : ""}</p>}
        <details data-testid="workflow-settled" open={settledOpen} onToggle={(event) => setSettledOpen(event.currentTarget.open)} className="border-t border-border/50 px-3.5 py-3">
          <summary className="cursor-pointer text-label font-medium text-muted-foreground">Settled <span className="ml-1 font-mono text-caption tabular-nums">{groups.settled.length}</span></summary>
          {settledOpen && <div className="mt-2">{groups.settled.length > 0 ? taskRows(groups.settled, "settled") : <p className="text-label text-muted-foreground">No settled tasks yet.</p>}</div>}
        </details>
        <details data-testid="workflow-verification" className="border-t border-border/50 px-3.5 py-3">
          <summary className="cursor-pointer text-label font-medium text-muted-foreground">What is verified <span className="ml-1 font-normal">· {received} {received === 1 ? "result" : "results"} received</span></summary>
          <div className="mt-2 space-y-2 text-label leading-relaxed text-muted-foreground">
            <p>{run.spec.mode === "live" && schemaAccepted > 0 ? `${schemaAccepted} task ${schemaAccepted === 1 ? "result passed its" : "results passed their"} requested output schema. ` : ""}A completed task records a worker result; it does not prove that project tests passed.</p>
            <p>Retained file changes stay isolated. Review before applying; the host checks the source baseline again during application.</p>
            <p>{run.spec.mode === "dry_run" ? "This dry run exercises coordination without invoking providers." : "Managed workers use captured workflow tools. They cannot run shell commands, builds, or project tests."}</p>
          </div>
        </details>
        <details data-testid="workflow-run-settings" className="border-t border-border/50 px-3.5 py-3">
          <summary className="cursor-pointer text-label font-medium text-muted-foreground">Run settings & usage</summary>
          <div className="mt-3 space-y-3 text-label text-muted-foreground">
            <p className="break-words">{run.spec.goal}</p>
            <dl className="space-y-1"><div className="flex justify-between gap-3"><dt>Observed tokens</dt><dd className="font-mono tabular-nums">{run.usage.total_tokens.toLocaleString()}</dd></div><div className="flex justify-between gap-3"><dt>Reserved for admitted work</dt><dd className="font-mono tabular-nums">{run.usage.reserved_tokens.toLocaleString()}</dd></div></dl>
            {((run.usage.estimated_tokens ?? 0) > 0 || (run.usage.tokens_unknown ?? true)) && <p className="leading-relaxed" data-testid="workflow-usage-estimate">{(run.usage.estimated_tokens ?? 0) > 0 ? `${run.usage.estimated_tokens.toLocaleString()} estimated tokens count toward the budget. ` : ""}{(run.usage.tokens_unknown ?? true) ? "Provider accounting is incomplete; observed usage may be partial." : ""}</p>}
            <details><summary className="cursor-pointer font-medium">Provider routes & limits</summary><Output value={{ limits: run.resolved_limits, routes: run.spec.routes }} /></details>
            <div className="flex flex-wrap items-center gap-2"><Button size="xs" variant="ghost" data-testid="workflow-edit-script" onClick={onEditScript}><Code2 />Edit as a new workflow</Button><span className="ml-auto font-mono text-caption">rev {run.revision}</span></div>
          </div>
        </details>
        {run.script?.result != null && <details onToggle={(event) => setScriptResultOpen(event.currentTarget.open)} className="border-t border-border/50 px-3.5 py-3"><summary className="cursor-pointer text-label font-medium text-muted-foreground">Script result</summary>{scriptResultOpen && <Output value={run.script.result} />}</details>}
      </div>}
      {task && <TaskDetail key={task.spec.id} run={run} task={task} busy={busy} wide={wide} onBack={back} onControl={onControl} />}
    </div>
  </div>;
}

function TaskDetail({ run, task, busy, wide, onBack, onControl }: { wide: boolean; run: WorkflowRunSnapshot; task: WorkflowTaskSnapshot; busy: boolean; onBack: () => void; onControl: (action: ControlAction, taskId?: string, text?: string) => void }) {
  const [message, setMessage] = useState("");
  const [earlierOpen, setEarlierOpen] = useState(false);
  const [coordinationOpen, setCoordinationOpen] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [reportsOpen, setReportsOpen] = useState(false);
  const [resultDetailsOpen, setResultDetailsOpen] = useState(false);
  const cancelled = run.cancel_requested || run.status === "cancelled";
  const currentAttempt = currentTaskAttempt(task);
  const artifacts = task.attempts.flatMap((attempt) => attempt.artifacts.map((artifact, index) => ({ attempt, artifact, index })));
  const isCurrentArtifact = ({ attempt }: (typeof artifacts)[number]) => !task.retired && task.status === "succeeded" && currentAttempt?.status === "succeeded" && attempt.id === currentAttempt.id;
  const currentArtifacts = artifacts.filter(isCurrentArtifact);
  const earlierArtifacts = artifacts.filter((entry) => !isCurrentArtifact(entry));
  const fileArtifacts = currentArtifacts.filter(({ artifact }) => isSnapshotArtifact(artifact));
  const reportArtifacts = currentArtifacts.filter(({ artifact }) => !isSnapshotArtifact(artifact));
  const resultSummary = typeof task.result === "string" ? task.result
    : task.result && typeof task.result === "object" && "summary" in task.result && typeof task.result.summary === "string" ? task.result.summary : null;
  const dependencyNames = (ids: string[]) => ids.map((id) => run.tasks.find((item) => item.spec.id === id)?.spec.title ?? id).join(", ");
  const renderArtifact = ({ attempt, artifact, index }: (typeof artifacts)[number]) => <Artifact key={`${attempt.id}-${index}`} runId={run.id} taskId={task.spec.id} attemptId={attempt.id} current={task.status === "succeeded" && currentAttempt?.status === "succeeded" && attempt.id === currentAttempt.id} canApply={run.spec.mode === "live" && run.spec.allow_writes && task.spec.access === "write" && !task.retired && !cancelled} artifact={artifact} />;
  return <div className={cn("min-w-0 space-y-4 px-3.5 py-3", wide && "sticky top-0 max-h-[80dvh] self-start overflow-y-auto")} data-testid="workflow-task-detail">
    <Button size="xs" variant="ghost" data-testid="workflow-back-checkpoints" onClick={onBack}>{wide ? <X /> : <ChevronLeft />}{wide ? "Close details" : "Back to checkpoints"}</Button>
    <div><h3 data-workflow-detail-title tabIndex={-1} className="break-words text-body font-semibold focus-visible:outline-2 focus-visible:outline-ring">{task.spec.title}</h3><div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-label"><span className={statusClass(task.status)}>{taskStatusLabel(task)}</span><span className="min-w-0 break-words text-muted-foreground">{taskRouteLabel(run, task)}</span></div><p className="mt-1 text-label text-muted-foreground">{task.spec.access === "write" ? "File changes stay isolated for review" : "Read-only task"}</p></div>
    <div className="flex flex-wrap gap-2">{task.status === "unknown" && <Button size="xs" variant="outline" data-testid="workflow-reconcile" disabled={busy} onClick={() => onControl("reconcile", task.spec.id)}><RotateCw />Confirm stopped</Button>}{["running", "waiting", "unknown"].includes(task.status) && <Button size="xs" variant="ghost" data-testid="workflow-cancel-task" disabled={busy} onClick={() => onControl("cancel_task", task.spec.id)}><Square />Cancel task</Button>}{task.status === "stopping" && <span className="text-label text-muted-foreground">Waiting for cancellation to settle…</span>}{!cancelled && !task.retired && ["failed", "cancelled", "blocked"].includes(task.status) && <Button size="xs" variant="outline" data-testid="workflow-retry" disabled={busy} onClick={() => onControl("retry", task.spec.id)}><RotateCw />Retry task</Button>}{!cancelled && !task.retired && ["queued", "blocked"].includes(task.status) && <Button size="xs" variant="ghost" data-testid="workflow-retire" disabled={busy} onClick={() => onControl("retire", task.spec.id)}><X />Remove from run</Button>}</div>
    {task.status === "unknown" && <p className="text-label leading-relaxed text-muted-foreground">The host must confirm this worker has stopped before its slot can be released or the task retried.</p>}
    {task.error && <p role="alert" className="break-words text-body-sm text-status-attention">{task.error}</p>}
    {task.waiting_for.length > 0 && <p className="text-label leading-relaxed text-muted-foreground">Waiting for {dependencyNames(task.waiting_for)}. Its worker slot is released while it waits.</p>}
    {task.result != null && <div data-testid="workflow-task-result"><Eyebrow>Result</Eyebrow>{resultSummary !== null ? <><p className="mt-2 whitespace-pre-wrap break-words text-body-sm leading-relaxed">{resultSummary.slice(0, 65536)}{resultSummary.length > 65536 && "\n…summary truncated in this view"}</p>{typeof task.result !== "string" && <details className="mt-2" onToggle={(event) => setResultDetailsOpen(event.currentTarget.open)}><summary className="cursor-pointer text-label text-muted-foreground">Structured result</summary>{resultDetailsOpen && <Output value={task.result} />}</details>}</> : <Output value={task.result} />}</div>}
    {fileArtifacts.length > 0 && <div><Eyebrow>File changes</Eyebrow>{fileArtifacts.map(renderArtifact)}</div>}
    {reportArtifacts.length > 0 && <details data-testid="workflow-task-reports" onToggle={(event) => setReportsOpen(event.currentTarget.open)}><summary className="cursor-pointer text-body-sm font-medium">Reports <span className="ml-1 text-label text-muted-foreground">{reportArtifacts.length}</span></summary>{reportsOpen && reportArtifacts.map(renderArtifact)}</details>}
    {earlierArtifacts.length > 0 && <details data-testid="workflow-earlier-artifacts" onToggle={(event) => setEarlierOpen(event.currentTarget.open)}><summary className="cursor-pointer text-body-sm font-medium">Earlier reports &amp; changes <span className="ml-1 text-label text-muted-foreground">{earlierArtifacts.length}</span></summary>{earlierOpen && earlierArtifacts.map(renderArtifact)}</details>}
    {!cancelled && !task.retired && ["queued", "running", "waiting"].includes(task.status) && <details><summary className="cursor-pointer text-body-sm font-medium">Send guidance</summary><form className="mt-2 space-y-2" onSubmit={(event) => { event.preventDefault(); if (message.trim()) { onControl("message", task.spec.id, message.trim()); setMessage(""); } }}><label className="block space-y-1"><Eyebrow>Task message</Eyebrow><Textarea aria-label="Message workflow task" value={message} onChange={(event) => setMessage(event.target.value)} maxLength={8192} className="min-h-16" /></label><Button type="submit" variant="outline" size="xs" disabled={busy || !message.trim()}>Send message</Button><p className="text-label text-muted-foreground">Queued for the next admission or continuation.</p></form></details>}
    <details data-testid="workflow-task-coordination" onToggle={(event) => setCoordinationOpen(event.currentTarget.open)}><summary className="cursor-pointer text-body-sm font-medium">Coordination details</summary>{coordinationOpen && <div className="mt-3 space-y-3 text-label text-muted-foreground">
      {task.spec.dependencies.length > 0 && <p>Depends on {dependencyNames(task.spec.dependencies)}.</p>}
      <p>{task.parent_task_id ? `Requested by ${dependencyNames([task.parent_task_id])}.` : "Root task."} Depth {task.depth} · generation {task.generation} · {task.attempts.length} {task.attempts.length === 1 ? "attempt" : "attempts"}.</p>
      <details><summary className="cursor-pointer font-medium">Task prompt</summary><p className="mt-2 whitespace-pre-wrap break-words leading-relaxed">{task.spec.prompt}</p></details>
      {task.messages.length > 0 && <div><Eyebrow>Messages</Eyebrow><ul className="mt-2 space-y-2">{task.messages.map((text, index) => <li key={index} className="break-words">{text}</li>)}</ul></div>}
      <details data-testid="workflow-attempt-history" onToggle={(event) => setHistoryOpen(event.currentTarget.open)}><summary className="cursor-pointer font-medium">Attempt history</summary>{historyOpen && <><Output value={task.attempts.slice(-20)} />{task.attempts.length > 20 && <p>Showing the most recent 20 attempts.</p>}</>}</details>
    </div>}</details>
  </div>;
}

function TaskIcon({ status }: { status: WorkflowTaskStatus }) {
  return <span className={cn("mt-0.5 flex size-3.5 shrink-0 items-center justify-center", statusClass(status))}>{status === "succeeded" ? <Check className="size-3.5" /> : status === "running" || status === "stopping" ? <Loader2 className="size-3.5 animate-spin motion-reduce:animate-none" /> : status === "failed" || status === "unknown" ? <X className="size-3.5" /> : <span className="size-1.5 rounded-full border border-current" />}</span>;
}
function Output({ value }: { value: unknown }) {
  const text = typeof value === "string" ? value : JSON.stringify(value, null, 2) ?? "";
  return <pre className="mt-2 max-h-80 overflow-auto whitespace-pre-wrap break-words rounded-md bg-surface-2 p-2 font-mono text-label leading-relaxed text-muted-foreground">{text.slice(0, 65536)}{text.length > 65536 ? "\n…output truncated in this view" : ""}</pre>;
}

export function WorkflowsPaneActions({ workspaceId }: { workspaceId: string }) {
  const selectRun = useWorkflowUIStore((state) => state.selectRun);
  return <Button variant="ghost" size="icon-xs" title="New workflow" aria-label="New workflow" onClick={() => selectRun(workspaceId, null)}><Plus className="size-3" /></Button>;
}

function Artifact({ runId, taskId, attemptId, current, canApply, artifact }: { runId: string; taskId: string; attemptId: string; current: boolean; canApply: boolean; artifact: unknown }) {
  const apply = useMutation({ mutationFn: (digest: string) => workflowIntegrateArtifact(runId, taskId, attemptId, digest) });
  const snapshot = isSnapshotArtifact(artifact) ? artifact : null;
  const [review, setReview] = useState(false);
  const [selectedPath, setSelectedPath] = useState("");
  const [visibleFiles, setVisibleFiles] = useState(100);
  const [previewedPaths, setPreviewedPaths] = useState<Set<string>>(() => new Set());
  const path = selectedPath || snapshot?.changed_paths[0] || "";
  const preview = useQuery({
    queryKey: ["workflow-artifact-preview", runId, taskId, attemptId, snapshot?.digest, path],
    queryFn: () => workflowArtifactPreview(runId, taskId, attemptId, snapshot!.digest, path),
    enabled: review && !!path && !!snapshot && snapshot.run_id === runId && snapshot.attempt_id === attemptId,
    staleTime: Infinity,
    retry: false,
  });
  useEffect(() => {
    if (preview.isSuccess && preview.data?.path === path) {
      setPreviewedPaths((paths) => paths.has(path) ? paths : new Set([...paths, path]));
    }
  }, [path, preview.isSuccess, preview.data?.path]);
  return <div className="mt-2">
    {snapshot ? <div className="rounded-md border border-border/60 p-2">
      <p className="text-body-sm font-medium">Retained file changes</p><p className="mt-1 text-label text-muted-foreground" data-testid="workflow-preview-coverage">Manifest: {snapshot.changed_paths.length} files · Previewed: {previewedPaths.size}</p>
      <ul className="mt-1 max-h-48 overflow-y-auto font-mono text-label text-muted-foreground">{snapshot.changed_paths.length ? snapshot.changed_paths.slice(0, 100).map((path) => <li className="break-words" key={path}>{path}</li>) : <li>No changed files</li>}</ul>
      {snapshot.changed_paths.length > 100 && <p className="mt-1 text-label text-muted-foreground">{snapshot.changed_paths.length - 100} additional changed paths retained in the manifest.</p>}
      {snapshot.changed_paths.length > 0 && <Button variant="ghost" size="xs" className="mt-2" data-testid="workflow-review-artifact" aria-expanded={review} onClick={() => setReview((value) => !value)}>{review ? "Hide file review" : "Review file changes"}</Button>}
      {review && <div className="mt-3 space-y-3" data-testid="workflow-artifact-review">
        <label className="block space-y-1"><Eyebrow>Changed file</Eyebrow><select aria-label="Review changed file" data-testid="workflow-artifact-file" className={cn(SELECT, "w-full font-mono text-label")} value={path} onChange={(event) => setSelectedPath(event.target.value)}>{snapshot.changed_paths.slice(0, visibleFiles).map((file) => <option key={file} value={file}>{file}</option>)}</select></label>
        {snapshot.changed_paths.length > visibleFiles && <Button variant="ghost" size="xs" onClick={() => setVisibleFiles((count) => count + 100)}>Show more files</Button>}
        {preview.isFetching && <p role="status" className="text-label text-muted-foreground">Loading retained file…</p>}
        {preview.error && <div role="alert"><p className="break-words text-body-sm text-status-attention">{workflowError(preview.error)}</p><Button variant="ghost" size="xs" onClick={() => { void preview.refetch(); }}>Retry file review</Button></div>}
        {preview.data && <>
          {preview.data.binary ? <p className="text-label text-muted-foreground">This file contains binary data. Text preview is unavailable.</p> : <>
            <div><Eyebrow>Before · workspace baseline</Eyebrow>{preview.data.before === null ? <p className="mt-1 text-label text-muted-foreground">File did not exist.</p> : <Output value={preview.data.before} />}</div>
            <div><Eyebrow>After · retained changes</Eyebrow>{preview.data.after === null ? <p className="mt-1 text-label text-muted-foreground">File deleted.</p> : <Output value={preview.data.after} />}</div>
          </>}
          {preview.data.truncated && <p className="text-label text-status-attention">Preview is limited to 64 KiB per side; the complete file remains in the retained artifact.</p>}
          {(preview.data.before_sha256 || preview.data.after_sha256) && <details><summary className="cursor-pointer text-label font-medium">File hashes</summary><Output value={{ before_sha256: preview.data.before_sha256, after_sha256: preview.data.after_sha256 }} /></details>}
        </>}
      </div>}
    </div> : <Output value={artifact} />}
    {snapshot && !current && <p className="mt-2 text-label text-muted-foreground">Earlier attempt · review only. These changes cannot be applied.</p>}
    {snapshot && canApply && current && <><p className="mt-2 text-label text-muted-foreground">Apply includes every file in the manifest, including files you haven’t previewed. Application stops if the source has changed.</p><Button variant="outline" size="xs" className="mt-2" data-testid="workflow-apply-artifact" disabled={snapshot.attempt_id !== attemptId || snapshot.run_id !== runId || snapshot.changed_paths.length === 0 || !review || !preview.isSuccess || apply.isPending || apply.isSuccess} onClick={() => apply.mutate(snapshot.digest)}>{apply.isPending ? "Applying…" : apply.isSuccess ? "Applied" : `Apply ${snapshot.changed_paths.length} file ${snapshot.changed_paths.length === 1 ? "change" : "changes"}`}</Button></>}
    {apply.error && <p role="alert" className="mt-2 break-words text-body-sm text-status-attention">{workflowError(apply.error)}</p>}
    {apply.isSuccess && <p role="status" className="mt-2 text-label text-status-open">Applied {apply.data.changed_paths.length} file {apply.data.changed_paths.length === 1 ? "change" : "changes"}.</p>}
  </div>;
}
