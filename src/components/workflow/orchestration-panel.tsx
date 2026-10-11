import { useEffect, useState, type ReactNode } from "react";
import { ChevronLeft, Square } from "lucide-react";

import { ScrollArea } from "@/components/ui/scroll-area";
import { Button } from "@/components/ui/button";
import { WORKFLOW_DENIED_MESSAGE } from "@/components/chat/WorkflowRunCard";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { agentChatInterruptTurn, agentChatRespondToRequest } from "@/tauri/commands";
import type { ApprovalDecision } from "@/tauri/events";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { formatCompactTokens } from "@/components/chat/WorkflowRunCard";
import { TickingText } from "@/components/chat/TickingText";
import { formatElapsed } from "@/lib/agent-chat/subagents";
import { workflowRunStats } from "@/lib/agent-chat/workflows";
import type {
  ChatViewItem,
  PermissionRequestItem,
  WorkflowRunItem,
} from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

import { findAgentContext } from "./workflow-phases";
import { workflowRunTone } from "./workflow-tone";
import { WorkflowPhaseList } from "./workflow-phase-list";
import { WorkflowAgentDetail } from "./workflow-agent-detail";
import { Eyebrow } from "@/components/ui/eyebrow";

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

  // A run waiting on approval can be answered here as well as from its
  // card in the thread; both go through the same request.
  const approvalRequestId =
    run.status === "pending_approval" ? run.approvalRequestId : null;
  const [sending, setSending] = useState<"allow" | "deny" | null>(null);
  useEffect(() => setSending(null), [approvalRequestId]);
  // The thread's card can answer the same request. Once either surface
  // has sent a decision the request is no longer pending, and a second
  // send would reach the backend as a stale response that fails the run.
  const requestState = useAgentChatStore((s) =>
    threadId && approvalRequestId
      ? findRequestState(s.threads[threadId]?.messages, approvalRequestId)
      : null,
  );
  const answering =
    sending !== null || (requestState !== null && requestState !== "pending");
  const respond = (choice: "allow" | "deny") => {
    if (!threadId || !approvalRequestId || answering) return;
    const decision: ApprovalDecision =
      choice === "allow"
        ? { decision: "allow" }
        : { decision: "deny", message: WORKFLOW_DENIED_MESSAGE };
    const store = useAgentChatStore.getState();
    setSending(choice);
    store.markRequestResponding(threadId, approvalRequestId, decision);
    agentChatRespondToRequest("claude", threadId, approvalRequestId, decision).catch(
      (err: unknown) => {
        useAgentChatStore.getState().markRequestPending(threadId, approvalRequestId);
        setSending(null);
        toast.error(`Failed to send decision: ${err}`);
      },
    );
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
              className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-surface-2 hover:text-foreground"
            >
              <ChevronLeft className="size-3.5" aria-hidden />
            </button>
          )}
          <span className="min-w-0 flex-1 truncate text-body-lg font-bold text-foreground">
            {level === "agent" ? "Agent detail" : (run.name ?? "Workflow")}
          </span>
          <span
            className={cn(
              "shrink-0 rounded-sm px-2 py-0.5 text-label font-bold uppercase tracking-wide",
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
            <Stat
              label="elapsed"
              value={
                <TickingText
                  active={running}
                  compute={(nowMs) => formatElapsed(workflowRunStats(run, nowMs).elapsedMs)}
                  testId="workflow-elapsed"
                />
              }
            />
            <div className="ml-auto flex gap-1.5">
              {approvalRequestId && threadId ? (
                <>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={answering}
                    data-testid="workflow-deny"
                    onClick={() => respond("deny")}
                    className="text-muted-foreground hover:text-foreground"
                  >
                    {sending === "deny" ? "Denying…" : "Deny"}
                  </Button>
                  <Button
                    size="sm"
                    disabled={answering}
                    data-testid="workflow-approve"
                    onClick={() => respond("allow")}
                    className="bg-foreground text-background hover:bg-foreground/90"
                  >
                    {sending === "allow" ? "Starting…" : "Run once"}
                  </Button>
                </>
              ) : running ? (
                // A finished run has nothing to stop; its status chip
                // already says how it ended.
                <Button
                  variant="outline"
                  size="icon-sm"
                  disabled={!threadId}
                  aria-label="Stop workflow"
                  data-testid="workflow-stop"
                  onClick={handleStop}
                  className="border-status-attention/35 bg-status-attention/10 text-status-attention hover:bg-status-attention/20"
                >
                  <Square fill="currentColor" aria-hidden />
                </Button>
              ) : null}
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

function Stat({ label, value }: { label: string; value: ReactNode }) {
  return (
    <span className="flex flex-col">
      <span className="font-mono text-body font-semibold tabular-nums text-foreground">{value}</span>
      <Eyebrow>{label}</Eyebrow>
    </span>
  );
}

/** Coarse tick while `active`, feeding the per-phase elapsed labels that
 *  take `now` as a prop. The header's own elapsed readout is a
 *  `TickingText` and stays exact at 1 Hz without any React commit; this
 *  interval exists only for the phase list, where the value crosses a
 *  component boundary, so it trades a few seconds of precision on a
 *  secondary readout for a 12× drop in panel re-renders. */
const PHASE_TICK_MS = 5000;

function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => setNow(Date.now()), PHASE_TICK_MS);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}

/** The approval request's resolution state, or `null` when the thread
 *  does not hold it. */
function findRequestState(
  messages: readonly ChatViewItem[] | undefined,
  requestId: string,
): PermissionRequestItem["resolution"]["state"] | null {
  if (!messages) return null;
  for (let i = messages.length - 1; i >= 0; i--) {
    const item = messages[i];
    if (item.kind === "permission_request" && item.request_id === requestId) {
      return item.resolution.state;
    }
  }
  return null;
}
