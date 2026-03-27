import { useState, useEffect } from "react";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";
import { ChevronRight, Plus } from "lucide-react";
import { cn } from "@/lib/utils";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { useUIStore } from "@/stores/ui-store";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  projectName: string;
  projectPath: string;
  workspaces: WorkspaceSnapshot[];
  activeWorkspaceId: string;
  onWorkspaceDragStart?: (workspaceId: string, projectPath: string | null) => (e: React.DragEvent) => void;
  onProjectDragStart?: (e: React.DragEvent) => void;
  dragStateId?: string | null;
}

export function SidebarProjectGroup({
  projectName,
  projectPath,
  workspaces,
  activeWorkspaceId,
  onWorkspaceDragStart,
  onProjectDragStart,
  dragStateId,
}: Props) {
  const [collapsed, setCollapsed] = useState(false);
  const setShowNewWorkspaceDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);

  useEffect(() => {
    dbGetUiState(`collapsed:project:${projectPath}`).then((val) => {
      if (val === "true") setCollapsed(true);
    }).catch(() => {});
  }, [projectPath]);

  const handleToggle = () => {
    const next = !collapsed;
    setCollapsed(next);
    dbSetUiState(`collapsed:project:${projectPath}`, String(next)).catch(console.error);
  };

  return (
    <div className="border-b border-border/30 last:border-b-0">
      {/* Project header */}
      <div
        className="flex items-center w-full pl-3 pr-2 py-1.5 text-sm font-medium hover:bg-muted/50 transition-colors"
        draggable={!!onProjectDragStart}
        onDragStart={onProjectDragStart}
        data-project-header-path={projectPath}
      >
        <button
          type="button"
          onClick={handleToggle}
          className="flex items-center gap-2 flex-1 min-w-0 py-0.5 text-left cursor-pointer"
        >
          <ChevronRight
            className={cn(
              "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform duration-150",
              !collapsed && "rotate-90",
            )}
          />
          <span className="truncate text-foreground">{projectName}</span>
          <span className="text-xs text-muted-foreground tabular-nums font-normal">
            ({workspaces.length})
          </span>
        </button>

        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); setShowNewWorkspaceDialog(true); }}
          className="p-1 rounded hover:bg-muted transition-colors shrink-0 ml-1"
          title="New workspace"
        >
          <Plus className="h-4 w-4 text-muted-foreground" />
        </button>
      </div>

      {/* Workspace rows — flat, no nesting */}
      {!collapsed && workspaces.map((ws, idx) => (
        <div
          key={ws.workspace_id}
          data-ws-id={ws.workspace_id}
          data-ws-index={idx}
          draggable={!!onWorkspaceDragStart}
          onDragStart={onWorkspaceDragStart?.(ws.workspace_id, projectPath)}
          className={dragStateId === ws.workspace_id ? "opacity-40" : ""}
        >
          <SidebarWorkspaceRow
            workspace={ws}
            isActive={ws.workspace_id === activeWorkspaceId}
          />
        </div>
      ))}
    </div>
  );
}
