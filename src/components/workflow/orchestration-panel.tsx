import { useEffect, useState } from "react";
import { ChevronLeft, Pause, Square } from "lucide-react";

import { ScrollArea } from "@/components/ui/scroll-area";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { agentChatInterruptTurn } from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { formatCompactTokens } from "@/components/chat/WorkflowRunCard";
import { formatElapsed } from "@/lib/agent-chat/subagents";
import { workflowRunStats } from "@/lib/agent-chat/workflows";
import type { WorkflowRunItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

import { findAgentContext } from "./workflow-phases";
import { workflowRunTone } from "./workflow-tone";
import { WorkflowPhaseList } from "./workflow-phase-list";
import { WorkflowAgentDetail } from "./workflow-agent-detail";

const STATUS_LABEL: Record<WorkflowRunItem["status"], string> = {
  pending_approval: "Pending approval",
  running: "Running",
  completed: "Complete",
  failed: "Failed",
  stopped: "Stopped",
};

interface Props {
  workspace: WorkspaceSnapshot;
  run: WorkflowRunItem;
  threadId: string | null;
}

/**
 * Orchestration right-panel body (design "run header → phases list →
 * agent detail"). Level (`phases` | `agent`) and the selected agent id
 * are local UI state — reset to `phases` whenever the underlying run
 * changes (a new `Workflow` tool launch), and Escape backs out of an
 * agent detail view.
 */
export function OrchestrationPanel({ workspace, run, threadId }: Props) {
  const [level, setLevel] = useState<"phases" | "agent">("phases");
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);

  // A fresh workflow launch (new `Workflow` tool call) always starts back
  // at the phases level, even if the previous run left the panel drilled
  // into an agent.
  useEffect(() => {
    setLevel("phases");
    setSelectedAgentId(null);
  }, [run.workflowId]);

  useEffect(() => {
    if (level !== "agent") return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setLevel("phases");
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [level]);

  const running = run.status === "running";
  const now = useNow(running);
  const stats = workflowRunStats(run, now);
  const tone = workflowRunTone(run.status);

  const agentContext =
    level === "agent" && selectedAgentId ? findAgentContext(run, selectedAgentId) : null;

  // The selected agent may have scrolled out of the run's phases (e.g. a
  // new workflow launched) between selection and render — fall back to
  // phases rather than rendering a broken detail view.
  useEffect(() => {
    if (level === "agent" && !agentContext) setLevel("phases");
  }, [level, agentContext]);

  const handleStop = () => {
    if (!threadId) return;
    // Same interrupt command the chat Composer's Stop button uses
    // (AgentChatPane.tsx `handleStop`) — aborts the current turn without
    // tearing down the whole session, which is the right scope for a
    // "Stop workflow" control (unlike the Composer's full stop+restart,
    // this shouldn't kill the pane's session for a running chat).
    // Workflow runs are Claude-only (see use-workspace-workflow.ts).
    agentChatInterruptTurn("claude", threadId, null).catch(console.error);
  };

  return (
    <div className="flex h-full min-h-0 flex-col bg-background" data-testid="orchestration-panel">
      <div className="shrink-0 border-b border-border/60 px-3.5 py-3">
        <div className="flex items-center gap-2">
          {level === "agent" && (
            <button
              type="button"
              onClick={() => setLevel("phases")}
              aria-label="Back to phases"
              className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-foreground/[0.08] hover:text-foreground"
            >
              <ChevronLeft className="h-3.5 w-3.5" strokeWidth={1.7} aria-hidden />
            </button>
          )}
          <span className="min-w-0 flex-1 truncate text-[14px] font-bold text-foreground">
            {level === "agent" ? "Agent detail" : (run.name ?? "Workflow")}
          </span>
          <span
            className={cn(
              "shrink-0 rounded-[5px] px-2 py-0.5 text-[11px] font-bold uppercase tracking-wide",
              tone.chipBg,
            )}
          >
            {STATUS_LABEL[run.status]}
          </span>
        </div>

        {level !== "agent" && (
          <div className="mt-2.5 flex items-center gap-3.5">
            <Stat label="agents" value={String(stats.agents)} />
            <Stat label="tokens" value={formatCompactTokens(stats.tokens)} />
            <Stat label="elapsed" value={formatElapsed(stats.elapsedMs)} />
            <div className="ml-auto flex gap-1.5">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="outline"
                    size="icon-sm"
                    disabled
                    aria-label="Pause"
                    data-testid="workflow-pause"
                  >
                    <Pause className="h-3 w-3" strokeWidth={1.8} fill="currentColor" aria-hidden />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom">Pausing isn't supported yet.</TooltipContent>
              </Tooltip>
              <Button
                variant="outline"
                size="icon-sm"
                disabled={!running || !threadId}
                aria-label="Stop workflow"
                data-testid="workflow-stop"
                onClick={handleStop}
                className="border-status-attention/35 bg-status-attention/10 text-status-attention hover:bg-status-attention/20"
              >
                <Square className="h-2.5 w-2.5" strokeWidth={1.8} fill="currentColor" aria-hidden />
              </Button>
            </div>
          </div>
        )}
      </div>

      <ScrollArea className="flex-1 min-h-0">
        {/* Radix's viewport wrapper is `display: table`, which sizes to
            max-content and lets long unbroken strings (result JSON, file
            paths) push past the panel edge. `w-px min-w-full` pins the
            content column to exactly the viewport width. */}
        <div className="w-px min-w-full">
        {level === "agent" && agentContext ? (
          <WorkflowAgentDetail
            agent={agentContext.agent}
            phaseIndex={agentContext.phaseIndex}
            phaseTitle={agentContext.phaseTitle}
            agentIndex={agentContext.agentIndex}
            agentsInPhase={agentContext.agentsInPhase}
            workspace={workspace}
          />
        ) : (
          <WorkflowPhaseList
            run={run}
            now={now}
            onSelectAgent={(agentId) => {
              setSelectedAgentId(agentId);
              setLevel("agent");
            }}
          />
        )}
        </div>
      </ScrollArea>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <span className="flex flex-col">
      <span className="font-mono text-[13px] font-semibold text-foreground">{value}</span>
      <span className="text-[10px] uppercase tracking-wide text-muted-foreground">{label}</span>
    </span>
  );
}

/** 1s tick while `active`, so derived elapsed stats advance without a
 *  fresh provider snapshot. Same discipline as SubagentsCard/
 *  WorkflowRunCard's local `useNow`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}
