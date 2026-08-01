import { useCallback, useRef, useState } from "react";
import {
  AlertTriangle,
  Ban,
  CheckCircle2,
  ChevronRight,
  LoaderCircle,
  Workflow as WorkflowIcon,
  XCircle,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  buildPermissionUpdate,
} from "@/lib/agent-chat/permission-rules";
import { formatElapsed } from "@/lib/agent-chat/subagents";
import { workflowRunStats } from "@/lib/agent-chat/workflows";
import type { PermissionRequestItem, WorkflowRunItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import type { ApprovalDecision } from "@/tauri/events";
import { useUIStore } from "@/stores/ui-store";

interface Props {
  item: WorkflowRunItem;
  /** Resolved from the thread's messages by matching
   *  `item.approvalRequestId` — same lookup pattern as ToolCallCard's
   *  `approval` prop. `null` once resolved or when no gate applies. */
  approval: PermissionRequestItem | null;
  onDecide: (decision: ApprovalDecision) => void;
  /** Active workspace id, for the "Open panel" affordance
   *  (`useUIStore.getState().setRightPanelTab(workspaceId, "orchestration")`).
   *  `null`/absent makes the open-panel actions inert rather than throw. */
  workspaceId?: string | null;
}

/**
 * Orchestration card for a `Workflow` tool run (design
 * `workflow-orchestration.dc.html`). One of four shapes depending on
 * `item.status`:
 *
 *   pending_approval → ember-tinted approval card (planned phases,
 *                       usage caution, Run once / Always / View script / Deny)
 *   running           → clickable inline progress line (shimmer phase
 *                       label, agents-active count, bottom progress bar)
 *   completed         → green summary row
 *   failed / stopped  → attention / muted summary row
 *
 * Rendered full-width (no avatar gutter) by MessageList, mirroring
 * SubagentsCard's `subagent_run` treatment.
 */
export function WorkflowRunCard({ item, approval, onDecide, workspaceId }: Props) {
  const openPanel = useCallback(() => {
    if (!workspaceId) return;
    useUIStore.getState().setRightPanelTab(workspaceId, "orchestration");
  }, [workspaceId]);

  switch (item.status) {
    case "pending_approval":
      return (
        <WorkflowApprovalCard item={item} approval={approval} onDecide={onDecide} />
      );
    case "running":
      return <WorkflowRunningRow item={item} onOpenPanel={openPanel} />;
    case "completed":
    case "failed":
    case "stopped":
      return <WorkflowSummaryRow item={item} onOpenPanel={openPanel} />;
  }
}

// ---------------------------------------------------------------------------
// Approval card
// ---------------------------------------------------------------------------

function WorkflowApprovalCard({
  item,
  approval,
  onDecide,
}: {
  item: WorkflowRunItem;
  approval: PermissionRequestItem | null;
  onDecide: (decision: ApprovalDecision) => void;
}) {
  const [scriptOpen, setScriptOpen] = useState(false);
  // Same synchronous in-flight guard ToolCallCard's ApprovalFooter uses:
  // the parent only flips `approval.resolution` to `responding` after the
  // IPC round-trips, so a double-click in the same tick could otherwise
  // dispatch twice.
  const dispatchedRef = useRef(false);
  const isResponding = approval?.resolution.state === "responding";

  const decide = (decision: ApprovalDecision) => {
    if (dispatchedRef.current) return;
    dispatchedRef.current = true;
    onDecide(decision);
  };

  const runOnce = () => decide({ decision: "allow" });
  const runAlways = () => {
    const updatedPermissions = buildPermissionUpdate("project", {
      toolName: "Workflow",
    });
    decide({
      decision: "allow",
      ...(updatedPermissions ? { updated_permissions: updatedPermissions } : {}),
    });
  };
  const deny = () =>
    decide({ decision: "deny", message: "User denied the workflow." });

  return (
    <div
      data-testid="workflow-approval-card"
      className="overflow-hidden rounded-[13px] border border-accent-ember/30 bg-accent-ember/5"
    >
      <div className="flex items-center gap-2.5 border-b border-border/60 px-3.5 py-3">
        <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-[7px] bg-accent-ember/20 text-accent-ember">
          <WorkflowIcon className="h-3.5 w-3.5" strokeWidth={1.6} aria-hidden />
        </span>
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-bold text-foreground">
            Run as a workflow?
          </div>
          <div className="text-[12px] text-muted-foreground">
            Claude wrote a script to orchestrate this — you can read it before
            running.
          </div>
        </div>
      </div>

      {item.plannedPhases.length > 0 && (
        <div className="flex flex-col gap-1.5 px-3.5 py-2.5">
          {item.plannedPhases.map((phase, i) => (
            <div
              key={`${phase.title}-${i}`}
              className="flex items-center gap-2.5 text-[12px]"
            >
              <span className="flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-[5px] bg-foreground/[0.08] font-mono text-[10px] font-semibold text-muted-foreground">
                {i + 1}
              </span>
              <span className="flex-1 font-medium text-foreground/90">
                {phase.title}
              </span>
              {phase.detail && (
                <span className="font-mono text-[11px] text-muted-foreground">
                  {phase.detail}
                </span>
              )}
            </div>
          ))}
        </div>
      )}

      <div className="flex items-center gap-2.5 border-t border-border/60 bg-status-working/[0.04] px-3.5 py-2.5">
        <AlertTriangle
          className="h-3.5 w-3.5 shrink-0 text-status-working"
          strokeWidth={1.6}
          aria-hidden
        />
        <span className="flex-1 text-[11px] text-muted-foreground">
          Spawns up to 16 agents in parallel · higher token use than a normal
          turn.
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-2 border-t border-border/60 px-3.5 py-2.5">
        <Button
          type="button"
          size="sm"
          className="h-[33px] px-3.5 text-xs bg-foreground text-background hover:bg-foreground/90"
          onClick={runOnce}
          disabled={isResponding}
        >
          Run once
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-[33px] px-3.5 text-xs"
          onClick={runAlways}
          disabled={isResponding}
        >
          Always for this project
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-[33px] px-3 text-xs bg-transparent"
          onClick={() => setScriptOpen(true)}
        >
          View script
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="ml-auto h-[33px] px-3 text-xs text-muted-foreground hover:text-foreground"
          onClick={deny}
          disabled={isResponding}
        >
          Deny
        </Button>
      </div>

      {isResponding && (
        <div className="border-t border-border/60 px-3.5 py-2 text-[11px] text-muted-foreground">
          Submitting decision…
        </div>
      )}

      <Dialog open={scriptOpen} onOpenChange={setScriptOpen}>
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>{item.name ?? "Workflow script"}</DialogTitle>
            <DialogDescription>
              {item.description ??
                "The script Claude wrote to orchestrate this workflow."}
            </DialogDescription>
          </DialogHeader>
          <pre className="max-h-[60vh] overflow-auto rounded-md bg-muted/40 p-3 font-mono text-[12px] leading-5 text-foreground whitespace-pre-wrap break-words">
            {item.script ?? "No script available."}
          </pre>
        </DialogContent>
      </Dialog>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Running row
// ---------------------------------------------------------------------------

function WorkflowRunningRow({
  item,
  onOpenPanel,
}: {
  item: WorkflowRunItem;
  onOpenPanel: () => void;
}) {
  // No wall-clock tick here: the running row shows phase / agent / progress
  // stats, all derived from `item`. `elapsedMs` is the only `now`-dependent
  // field and this row never renders it, so the old 1 Hz `setNow` re-render
  // changed nothing on screen.
  const stats = workflowRunStats(item);
  const phaseIdx = Math.min(
    stats.currentPhaseIndex,
    Math.max(stats.phasesTotal - 1, 0),
  );
  const currentPhase = item.phases[phaseIdx] ?? null;
  const phaseLabel =
    currentPhase?.title ||
    currentPhase?.detail ||
    item.description ||
    item.name ||
    "Working";
  const activeAgents = countRunningAgents(item);
  const phaseOrdinal = stats.phasesTotal > 0 ? phaseIdx + 1 : 0;
  const progressPct =
    stats.phasesTotal > 0
      ? Math.round((stats.phasesDone / stats.phasesTotal) * 100)
      : 0;

  const statusLine = [
    phaseOrdinal > 0 ? `Phase ${phaseOrdinal} of ${stats.phasesTotal}` : null,
    phaseLabel,
    activeAgents > 0
      ? `${activeAgents} agent${activeAgents === 1 ? "" : "s"} active`
      : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <div
      data-testid="workflow-run-card"
      data-status="running"
      role="button"
      tabIndex={0}
      onClick={onOpenPanel}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpenPanel();
        }
      }}
      className="cursor-pointer overflow-hidden rounded-[12px] border border-border bg-foreground/[0.025] hover:border-muted-foreground/60"
    >
      <div className="flex items-center gap-2.5 px-3.5 py-2.5">
        <LoaderCircle
          className="h-[17px] w-[17px] shrink-0 animate-spin text-status-working"
          strokeWidth={1.9}
          aria-hidden
        />
        <span className="shrink-0 text-[13px] font-bold text-foreground">
          Workflow running
        </span>
        <span className="min-w-0 flex-1 overflow-hidden">
          <span className="shimmer block truncate text-[12px]">
            {statusLine}
          </span>
        </span>
        <span
          data-testid="workflow-open-panel"
          className="flex h-[26px] shrink-0 items-center gap-1.5 rounded-[7px] border border-border bg-background px-2.5 text-[11px] font-semibold text-muted-foreground"
        >
          Open panel
          <ChevronRight className="h-3 w-3" strokeWidth={1.7} aria-hidden />
        </span>
      </div>
      <div className="h-[3px] bg-border/60">
        <div
          className="h-full bg-status-working"
          style={{ width: `${progressPct}%` }}
        />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Summary row (completed / failed / stopped)
// ---------------------------------------------------------------------------

function WorkflowSummaryRow({
  item,
  onOpenPanel,
}: {
  item: WorkflowRunItem;
  onOpenPanel: () => void;
}) {
  const stats = workflowRunStats(item);
  const agents = item.agentCount ?? stats.agents;
  const phases = Math.max(item.plannedPhases.length, item.phases.length);
  const elapsed = formatElapsed(stats.elapsedMs);
  const tokens = formatCompactTokens(stats.tokens);

  const label =
    item.status === "completed"
      ? "Workflow complete"
      : item.status === "failed"
        ? "Workflow failed"
        : "Workflow stopped";
  const Icon =
    item.status === "completed"
      ? CheckCircle2
      : item.status === "failed"
        ? XCircle
        : Ban;
  const iconClass =
    item.status === "completed"
      ? "text-status-open"
      : item.status === "failed"
        ? "text-status-attention"
        : "text-muted-foreground";

  return (
    <div
      data-testid="workflow-run-card"
      data-status={item.status}
      role="button"
      tabIndex={0}
      onClick={onOpenPanel}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpenPanel();
        }
      }}
      title={`${agents} agents · ${phases} phases · ${tokens} tokens · ${elapsed}`}
      className="flex cursor-pointer items-center gap-2.5 rounded-[12px] border border-border/60 bg-muted/30 px-3.5 py-2.5 hover:border-muted-foreground/60"
    >
      <Icon className={cn("h-4 w-4 shrink-0", iconClass)} strokeWidth={1.8} aria-hidden />
      <span className="flex-1 text-[13px] font-semibold text-muted-foreground">
        {label} · {agents} agent{agents === 1 ? "" : "s"} · {phases} phase
        {phases === 1 ? "" : "s"} · {elapsed}
      </span>
      <span className="font-mono text-[11px] text-muted-foreground">
        View run
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Count of currently-running agents across every phase of THIS run —
 *  scoped counterpart to `countRunningWorkflowAgents` (which sums across
 *  every workflow run in a thread). */
function countRunningAgents(item: WorkflowRunItem): number {
  let n = 0;
  for (const phase of item.phases) {
    for (const agent of phase.agents) {
      if (agent.status === "running") n += 1;
    }
  }
  return n;
}

/** "12K" / "2.9M" style compact token count. Values under 1000 render as
 *  plain integers; K/M values drop a trailing ".0". */
export function formatCompactTokens(n: number): string {
  if (n < 1000) return String(Math.max(0, Math.round(n)));
  if (n < 1_000_000) return `${trimTrailingZero(n / 1000)}K`;
  return `${trimTrailingZero(n / 1_000_000)}M`;
}

function trimTrailingZero(v: number): string {
  const rounded = Math.round(v * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

