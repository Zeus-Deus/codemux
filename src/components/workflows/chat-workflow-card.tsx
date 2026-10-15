import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, ChevronDown, Circle, Copy, CornerDownLeft, PanelRight, Square, Workflow as WorkflowIcon, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { ChatMarkdown } from "@/components/chat/ChatMarkdown";
import { modelLabel } from "@/lib/agent-chat/capability-defaults";
import { formatElapsed } from "@/lib/agent-chat/subagents";
import { chatWorkflowPhase, type ChatWorkflowPhase } from "@/lib/workflows/chat-workflow";
import { WORKFLOW_PROVIDER_LABELS, isWorkflowProvider } from "@/lib/workflows/workflow-settings";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useUIStore } from "@/stores/ui-store";
import { useWorkflowUIStore } from "@/stores/workflow-ui-store";
import { workflowCancel, workflowError, workflowGet, type WorkflowRunSnapshot, type WorkflowTaskSnapshot } from "@/tauri/workflows";
import { pollingInterval } from "./use-workflow-runtime";
import { currentFileChanges, currentTaskAttempt } from "./workflow-presentation";

const PHASES: { key: ChatWorkflowPhase; title: string }[] = [
  { key: "plan", title: "Plan" },
  { key: "work", title: "Work" },
  { key: "report", title: "Report" },
];
const ACTIVE = new Set(["running", "paused", "stopping", "unknown"]);
const LIVE_TASK = new Set(["running", "waiting", "stopping"]);

/** Live clock for elapsed labels while something is running. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

function routeName(run: WorkflowRunSnapshot, task: WorkflowTaskSnapshot): string {
  const routeId = currentTaskAttempt(task)?.route_id ?? task.spec.route_id ?? run.spec.routes[0]?.id;
  const route = run.spec.routes.find((entry) => entry.id === routeId);
  if (!route) return "";
  if (!route.model) return isWorkflowProvider(route.provider) ? WORKFLOW_PROVIDER_LABELS[route.provider] : route.provider;
  return modelLabel(route.provider, route.model);
}

function taskDuration(task: WorkflowTaskSnapshot, now: number): string | null {
  const attempts = task.attempts.filter((attempt) => attempt.generation === task.generation);
  const first = attempts[0];
  if (!first) return null;
  const last = attempts[attempts.length - 1];
  return formatElapsed((last.finished_at_ms ?? now) - first.started_at_ms);
}

export function reportText(run: WorkflowRunSnapshot): string | null {
  const result = run.script?.result;
  if (result && typeof result === "object" && "summary" in result && typeof result.summary === "string" && result.summary.trim()) return result.summary;
  const report = run.tasks.find((task) => task.spec.id === "report")?.result;
  if (report && typeof report === "object" && "summary" in report && typeof report.summary === "string" && report.summary.trim()) return report.summary;
  return null;
}

function headline(run: WorkflowRunSnapshot): string {
  const work = run.tasks.filter((task) => chatWorkflowPhase(task.spec.id) === "work");
  const done = work.filter((task) => task.status === "succeeded").length;
  if (run.status === "completed") return work.length ? `${work.length} ${work.length === 1 ? "agent" : "agents"} · report ready` : "Report ready";
  if (run.status === "failed") return "Stopped with an error";
  if (run.status === "cancelled") return "Stopped";
  if (run.status === "paused") return "Paused";
  if (run.status === "unknown") return "A worker needs confirmation";
  const report = run.tasks.find((task) => task.spec.id === "report");
  if (report) return "Writing the report";
  if (work.length) return `${done} of ${work.length} ${work.length === 1 ? "agent" : "agents"} done`;
  return "Planning the work";
}

function StatusPill({ run, now }: { run: WorkflowRunSnapshot; now: number }) {
  const elapsed = formatElapsed((ACTIVE.has(run.status) ? now : run.updated_at_ms) - run.created_at_ms);
  const tone = run.status === "completed" ? "border-status-open/35 bg-status-open/8 text-status-open"
    : run.status === "failed" || run.status === "unknown" ? "border-status-attention/35 bg-status-attention/8 text-status-attention"
      : run.status === "cancelled" ? "border-hairline-strong text-muted-foreground"
        : "border-status-working/35 bg-status-working/8 text-status-working";
  const label = { running: "Running", paused: "Paused", stopping: "Stopping", unknown: "Needs you", completed: "Done", failed: "Failed", cancelled: "Stopped" }[run.status];
  return <span data-testid="chat-workflow-status" data-status={run.status} className={cn("inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2 py-0.5 text-label", tone)}>
    {run.status === "running" || run.status === "stopping" ? <Spinner className="size-3" aria-hidden /> : run.status === "completed" ? <Check className="size-3" aria-hidden /> : null}
    {label}<span className="font-mono text-caption tabular-nums opacity-80">{elapsed}</span>
  </span>;
}

function TaskIcon({ task }: { task: WorkflowTaskSnapshot }) {
  if (task.status === "succeeded") return <Check className="size-3.5 text-status-open" aria-label="Done" />;
  if (LIVE_TASK.has(task.status)) return <Spinner className="size-3.5 text-status-working" label="Running" />;
  if (task.status === "failed" || task.status === "unknown") return <X className="size-3.5 text-status-attention" aria-label="Failed" />;
  if (task.status === "cancelled" || task.status === "blocked") return <X className="size-3.5 text-muted-foreground" aria-label="Stopped" />;
  return <Circle className="size-2.5 text-muted-foreground/70" aria-label="Queued" />;
}

/**
 * Transcript card for a `/workflow` run: the plan, the parallel agents and
 * the final report, live. Details and file review stay in the Workflows pane.
 */
export function ChatWorkflowCard({ runId, workspaceId, threadId }: { runId: string; workspaceId: string; threadId: string }) {
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ["workflows", workspaceId, "run", runId],
    queryFn: () => workflowGet(runId),
    structuralSharing: (previous, incoming) => {
      const old = previous as WorkflowRunSnapshot | undefined;
      const next = incoming as WorkflowRunSnapshot;
      return old && old.revision >= next.revision ? old : next;
    },
    refetchInterval: (state) => pollingInterval(state.state.data),
    retry: false,
  });
  const run = query.data;
  const active = !!run && ACTIVE.has(run.status);
  const now = useNow(active);
  const [expanded, setExpanded] = useState(false);
  const cancel = useMutation({
    mutationFn: () => workflowCancel(runId),
    onSuccess: (next) => client.setQueryData(["workflows", workspaceId, "run", runId], next),
    onError: (error) => toast.failure("Couldn't stop the workflow", error),
  });
  const phases = useMemo(() => {
    const grouped: Record<ChatWorkflowPhase, WorkflowTaskSnapshot[]> = { plan: [], work: [], report: [] };
    for (const task of run?.tasks ?? []) {
      const phase = chatWorkflowPhase(task.spec.id);
      if (phase) grouped[phase].push(task);
    }
    return grouped;
  }, [run]);

  const openDetails = () => {
    useWorkflowUIStore.getState().selectRun(workspaceId, runId);
    useUIStore.getState().setRightPanelTab(workspaceId, "workflows");
  };

  if (!run) {
    return <div data-testid="chat-workflow-card" className="flex items-center gap-2.5 rounded-lg border border-hairline-strong bg-surface-1 px-3.5 py-3 text-body-sm text-muted-foreground">
      <WorkflowIcon className="size-4 shrink-0" aria-hidden />
      {query.isLoading ? <span className="flex items-center gap-2"><Spinner className="size-3.5" />Loading workflow…</span> : "This workflow is no longer available on this device."}
    </div>;
  }

  const report = reportText(run);
  const changes = currentFileChanges(run);
  const changedFiles = changes.reduce((count, change) => count + change.artifact.changed_paths.length, 0);
  const error = run.error ?? run.script?.error ?? null;
  const planDone = phases.plan.some((task) => task.status === "succeeded");

  const continueInChat = () => {
    if (!report) return;
    useAgentChatStore.getState().setInputDraft(threadId, `Here is the report from the workflow "${run.spec.title}":\n\n${report}\n\n`);
  };
  const copyReport = () => {
    if (!report) return;
    void navigator.clipboard.writeText(report).then(() => toast.success("Report copied"), (err: unknown) => toast.failure("Couldn't copy the report", err));
  };

  return <section data-testid="chat-workflow-card" aria-label={`Workflow: ${run.spec.title}`} className="overflow-hidden rounded-lg border border-hairline-strong bg-surface-1">
    <header className="flex items-center gap-2.5 border-b border-hairline px-3.5 py-3">
      <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-accent-ember/15 text-accent-ember"><WorkflowIcon className="size-3.5" aria-hidden /></span>
      <div className="min-w-0 flex-1">
        <h3 className="truncate text-body font-semibold text-foreground" title={run.spec.goal}>{run.spec.title}</h3>
        <p className="truncate text-label text-muted-foreground">Workflow · {headline(run)}</p>
      </div>
      <StatusPill run={run} now={now} />
    </header>

    {PHASES.map(({ key, title }, index) => {
      const tasks = phases[key];
      const waiting = tasks.length === 0;
      if (key === "report" && report) return null;
      const caption = key === "plan" ? (planDone ? "split into parallel subtasks" : "reading the workspace")
        : key === "work" ? (waiting ? (planDone ? "starting agents" : "after the plan") : `${tasks.length} in parallel`)
          : waiting ? "after the agents finish" : "merging findings";
      const done = tasks.length > 0 && tasks.every((task) => task.status === "succeeded");
      return <div key={key} className="border-b border-hairline px-3.5 py-2.5 last:border-b-0" data-testid={`chat-workflow-phase-${key}`}>
        <div className="flex items-center gap-2 text-body-sm">
          <span className="w-4 font-mono text-caption text-muted-foreground/70">{String(index + 1).padStart(2, "0")}</span>
          <span className={cn("font-semibold", waiting ? "text-muted-foreground" : "text-foreground")}>{title}</span>
          <span className="text-label text-muted-foreground">{caption}</span>
          {key === "work" && tasks.length > 0 && <span className="ml-auto font-mono text-caption tabular-nums text-muted-foreground">{tasks.filter((task) => task.status === "succeeded").length}/{tasks.length}</span>}
          {key !== "work" && tasks[0] && (done || !["queued"].includes(tasks[0].status)) && <span className="ml-auto flex"><TaskIcon task={tasks[0]} /></span>}
        </div>
        {key === "work" && tasks.length > 0 && <ul className="mt-1.5 space-y-0.5">
          {tasks.map((task) => <li key={task.spec.id} className={cn("grid grid-cols-[16px_minmax(0,1fr)_auto_auto] items-center gap-2 rounded-md py-1 pl-6 pr-1 text-body-sm", LIVE_TASK.has(task.status) && "bg-surface-2")}>
            <span className="flex justify-center"><TaskIcon task={task} /></span>
            <span className="truncate text-foreground/90" title={task.error ?? task.spec.title}>{task.spec.title}</span>
            <span className="truncate text-label text-muted-foreground">{routeName(run, task)}</span>
            <span className={cn("min-w-14 text-right font-mono text-caption tabular-nums", task.status === "succeeded" ? "text-status-open" : LIVE_TASK.has(task.status) ? "text-status-working" : "text-muted-foreground")}>
              {task.status === "failed" ? "failed" : taskDuration(task, now) ?? "queued"}
            </span>
          </li>)}
        </ul>}
      </div>;
    })}

    {report && <div className="border-b border-hairline px-3.5 py-3" data-testid="chat-workflow-report">
      <div className="mb-1.5 flex items-center gap-2 text-body-sm"><span className="w-4 font-mono text-caption text-muted-foreground/70">03</span><span className="font-semibold">Report</span></div>
      <div className={cn("pl-6 text-body-sm", !expanded && "max-h-56 overflow-hidden [mask-image:linear-gradient(to_bottom,black_70%,transparent)]")}>
        <ChatMarkdown workspaceId={workspaceId}>{report}</ChatMarkdown>
      </div>
      <Button variant="ghost" size="xs" className="mt-1 ml-5 text-muted-foreground" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
        <ChevronDown className={cn("transition-transform duration-150", expanded && "rotate-180")} />{expanded ? "Show less" : "Show full report"}
      </Button>
    </div>}

    {error && run.status !== "completed" && <p role="alert" className="border-b border-hairline px-3.5 py-2.5 text-body-sm break-words text-status-attention">{workflowError(error)}</p>}

    <footer className="flex flex-wrap items-center gap-2 bg-surface-1 px-3.5 py-2.5">
      {report && <Button size="sm" onClick={continueInChat} title="Put the report in the message box so this chat's agent can act on it"><CornerDownLeft />Continue in chat</Button>}
      <Button size="sm" variant="outline" onClick={openDetails}><PanelRight />{changedFiles > 0 ? `Review ${changedFiles} file ${changedFiles === 1 ? "change" : "changes"}` : "Details"}</Button>
      {report && <Button size="sm" variant="ghost" onClick={copyReport}><Copy />Copy</Button>}
      {active && run.status !== "stopping" && <Button size="sm" variant="ghost" disabled={cancel.isPending} onClick={() => cancel.mutate()}><Square />Stop</Button>}
      <span className="ml-auto truncate text-label text-muted-foreground">
        {run.spec.allow_writes ? "Edits isolated for review" : "Read-only"}
        {run.usage.total_tokens > 0 && <span className="ml-2 font-mono text-caption tabular-nums">{Math.round(run.usage.total_tokens / 1000)}k tok</span>}
      </span>
    </footer>
  </section>;
}
