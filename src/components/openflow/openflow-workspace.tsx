import { useEffect, useMemo } from "react";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { useOpenFlowStore, useActiveRun } from "@/stores/openflow-store";
import { useOpenFlowPolling } from "@/hooks/use-openflow-polling";
import { OrchestrationView } from "./orchestration-view";
import { Button } from "@/components/ui/button";
import { Workflow, Plus } from "lucide-react";

interface OpenFlowWorkspaceProps {
  workspace: WorkspaceSnapshot;
}

export function OpenFlowWorkspace({ workspace }: OpenFlowWorkspaceProps) {
  const runtimeSnapshot = useOpenFlowStore((s) => s.runtimeSnapshot);
  const activeRunId = useOpenFlowStore((s) => s.activeRunId);
  const setActiveRun = useOpenFlowStore((s) => s.setActiveRun);
  const setNewRunDialogOpen = useOpenFlowStore((s) => s.setNewRunDialogOpen);
  const activeRun = useActiveRun();

  // Auto-detect the run for this workspace from the runtime snapshot
  const detectedRunId = useMemo(() => {
    if (!runtimeSnapshot) return null;
    // Find the first active run (runs are workspace-scoped via the backend)
    const runs = runtimeSnapshot.active_runs;
    if (runs.length === 0) return null;
    // If we already have an activeRunId that's still valid, keep it
    if (activeRunId && runs.find((r) => r.run_id === activeRunId)) {
      return activeRunId;
    }
    return runs[0].run_id;
  }, [runtimeSnapshot, activeRunId]);

  // Sync detected run with store
  useEffect(() => {
    if (detectedRunId && detectedRunId !== activeRunId) {
      setActiveRun(detectedRunId);
    }
  }, [detectedRunId, activeRunId, setActiveRun]);

  // Start polling
  useOpenFlowPolling(activeRunId);

  if (activeRun) {
    return <OrchestrationView workspace={workspace} run={activeRun} />;
  }

  // Empty state
  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 text-muted-foreground">
      <Workflow className="h-12 w-12 opacity-30" />
      <div className="text-center">
        <p className="text-sm font-medium text-foreground/70">
          No active OpenFlow run
        </p>
        <p className="text-xs mt-1">
          Start a multi-agent orchestration run to begin.
        </p>
      </div>
      <Button
        variant="outline"
        size="sm"
        onClick={() => setNewRunDialogOpen(true)}
      >
        <Plus className="h-3.5 w-3.5 mr-1.5" />
        Start Run
      </Button>
    </div>
  );
}
