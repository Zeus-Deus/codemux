import type { OpenFlowRunRecord, WorkspaceSnapshot } from "@/tauri/types";
import {
  phaseLabel,
  getPhaseBadgeClass,
  getHealthInfo,
  getStatusDotClass,
} from "@/lib/openflow-utils";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  stopOpenflowRun,
  injectOrchestratorMessage,
  triggerOrchestratorCycle,
} from "@/tauri/commands";
import { useOpenFlowStore } from "@/stores/openflow-store";
import {
  Square,
  CheckCircle,
  XCircle,
  RotateCw,
  Plus,
  Users,
} from "lucide-react";

interface OrchestrationHeaderProps {
  workspace: WorkspaceSnapshot;
  run: OpenFlowRunRecord;
}

export function OrchestrationHeader({
  workspace,
  run,
}: OrchestrationHeaderProps) {
  const syncRuntime = useOpenFlowStore((s) => s.syncRuntime);
  const setNewRunDialogOpen = useOpenFlowStore((s) => s.setNewRunDialogOpen);
  const health = getHealthInfo(run.orchestration_state);
  const isAwaitingApproval = run.status === "awaiting_approval";
  const isStalled =
    run.orchestration_state === "stalled" ||
    run.orchestration_state === "error";

  const handleStop = async () => {
    try {
      await stopOpenflowRun(run.run_id, "User cancelled");
      syncRuntime();
    } catch (err) {
      console.error("Failed to stop run:", err);
    }
  };

  const handleApprove = async () => {
    try {
      await injectOrchestratorMessage(run.run_id, "APPROVED. Proceed.");
      syncRuntime();
    } catch (err) {
      console.error("Failed to approve:", err);
    }
  };

  const handleReject = async () => {
    try {
      await stopOpenflowRun(run.run_id, "User rejected");
      syncRuntime();
    } catch (err) {
      console.error("Failed to reject:", err);
    }
  };

  const handleReprime = async () => {
    try {
      await triggerOrchestratorCycle(run.run_id);
      syncRuntime();
    } catch (err) {
      console.error("Failed to re-prime:", err);
    }
  };

  return (
    <div className="flex items-center gap-2 border-b border-border/50 px-3 py-1.5 bg-card/50">
      {/* Title */}
      <span className="text-xs font-semibold text-foreground truncate max-w-[200px]">
        {workspace.title}
      </span>

      {/* Phase badge */}
      <Badge
        variant="outline"
        className={cn(
          "h-5 px-1.5 text-[10px] font-semibold",
          getPhaseBadgeClass(run.current_phase),
        )}
      >
        {phaseLabel(run.current_phase)}
      </Badge>

      {/* Health indicator */}
      <div className="flex items-center gap-1">
        <span
          className={cn(
            "h-1.5 w-1.5 rounded-full",
            getStatusDotClass(health.tone === "success" ? "active" : health.tone === "danger" ? "failed" : "idle"),
          )}
        />
        <span className="text-[10px] text-muted-foreground">{health.label}</span>
      </div>

      {/* Agent count */}
      <div className="flex items-center gap-1 text-muted-foreground">
        <Users className="h-3 w-3" />
        <span className="text-[10px] tabular-nums">{run.workers.length}</span>
      </div>

      {/* Replan count */}
      {run.replan_count > 0 && (
        <span className="text-[10px] text-muted-foreground">
          R{run.replan_count}
        </span>
      )}

      <div className="flex-1" />

      {/* Controls */}
      <div className="flex items-center gap-1">
        {isAwaitingApproval && (
          <>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 px-2 text-[10px] text-emerald-400 hover:text-emerald-300"
              onClick={handleApprove}
            >
              <CheckCircle className="h-3 w-3 mr-1" />
              Approve
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 px-2 text-[10px] text-red-400 hover:text-red-300"
              onClick={handleReject}
            >
              <XCircle className="h-3 w-3 mr-1" />
              Reject
            </Button>
          </>
        )}

        {isStalled && (
          <Button
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-[10px]"
            onClick={handleReprime}
          >
            <RotateCw className="h-3 w-3 mr-1" />
            Re-prime
          </Button>
        )}

        <Button
          variant="ghost"
          size="sm"
          className="h-6 px-2 text-[10px]"
          onClick={() => setNewRunDialogOpen(true)}
        >
          <Plus className="h-3 w-3 mr-1" />
          New
        </Button>

        <Button
          variant="ghost"
          size="sm"
          className="h-6 px-2 text-[10px] text-red-400 hover:text-red-300"
          onClick={handleStop}
        >
          <Square className="h-3 w-3 mr-1" />
          Stop
        </Button>
      </div>
    </div>
  );
}
