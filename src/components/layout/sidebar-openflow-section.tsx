import { useState, useMemo } from "react";
import { useAppStore } from "@/stores/app-store";
import { useOpenFlowStore } from "@/stores/openflow-store";
import { activateWorkspace } from "@/tauri/commands";
import {
  getStatusDotClass,
  phaseLabel,
  getPhaseBadgeClass,
} from "@/lib/openflow-utils";
import { cn } from "@/lib/utils";
import { Workflow, ChevronRight, Plus } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { NewRunDialog } from "@/components/openflow/new-run-dialog";

export function SidebarOpenflowSection() {
  const appState = useAppStore((s) => s.appState);
  const runtimeSnapshot = useOpenFlowStore((s) => s.runtimeSnapshot);
  const setNewRunDialogOpen = useOpenFlowStore((s) => s.setNewRunDialogOpen);
  const [expanded, setExpanded] = useState(false);

  const openflowWorkspaces = useMemo(
    () =>
      appState?.workspaces.filter((w) => w.workspace_type === "open_flow") ??
      [],
    [appState],
  );

  return (
    <div className="shrink-0 border-t border-sidebar-border">
      <div
        role="button"
        tabIndex={0}
        className="flex w-full items-center gap-1.5 px-3 py-1.5 text-left text-[11px] font-semibold uppercase tracking-wider text-sidebar-foreground/70 hover:text-sidebar-foreground transition-colors cursor-pointer select-none"
        onClick={() => setExpanded(!expanded)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setExpanded(!expanded);
          }
        }}
      >
        <ChevronRight
          className={`h-3 w-3 shrink-0 transition-transform ${expanded ? "rotate-90" : ""}`}
        />
        <span>OpenFlow</span>
        <span className="flex-1" />
        {openflowWorkspaces.length > 0 && (
          <span className="text-[10px] font-semibold tabular-nums text-muted-foreground">
            {openflowWorkspaces.length}
          </span>
        )}
        <button
          className="rounded p-0.5 hover:bg-accent/30 transition-colors"
          onClick={(e) => {
            e.stopPropagation();
            setNewRunDialogOpen(true);
          }}
        >
          <Plus className="h-3 w-3" />
        </button>
      </div>

      {expanded && (
        <div className="px-3 pb-2">
          {openflowWorkspaces.length === 0 ? (
            <div className="flex items-center gap-1.5 py-1 text-muted-foreground">
              <Workflow className="h-3.5 w-3.5" />
              <span className="text-xs">No active runs</span>
            </div>
          ) : (
            <div className="space-y-0.5">
              {openflowWorkspaces.map((ws) => {
                // Try to find a matching run for this workspace
                const run = runtimeSnapshot?.active_runs.find(
                  (r) => r.title === ws.title,
                );

                return (
                  <button
                    key={ws.workspace_id}
                    className="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs hover:bg-accent/20 transition-colors"
                    onClick={() =>
                      activateWorkspace(ws.workspace_id).catch(console.error)
                    }
                  >
                    {/* Status dot */}
                    <span
                      className={cn(
                        "h-2 w-2 shrink-0 rounded-full",
                        run
                          ? getStatusDotClass(run.status)
                          : "bg-muted-foreground/30",
                      )}
                    />
                    <span className="truncate text-foreground flex-1">
                      {ws.title}
                    </span>
                    {run && (
                      <Badge
                        variant="outline"
                        className={cn(
                          "h-4 px-1 text-[9px] font-medium shrink-0",
                          getPhaseBadgeClass(run.current_phase),
                        )}
                      >
                        {phaseLabel(run.current_phase)}
                      </Badge>
                    )}
                    {run && (
                      <span className="text-[9px] text-muted-foreground tabular-nums shrink-0">
                        {run.workers.length}
                      </span>
                    )}
                  </button>
                );
              })}
            </div>
          )}
        </div>
      )}
      <NewRunDialog />
    </div>
  );
}
