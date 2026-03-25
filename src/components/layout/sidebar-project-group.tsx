import { useState, useEffect } from "react";
import {
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarMenuSub,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";
import { ChevronRight, Folder } from "lucide-react";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
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

  // Load collapse state from SQLite
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
    <SidebarMenuItem>
      <SidebarMenuButton
        size="sm"
        onClick={handleToggle}
        className="gap-1.5"
        title={projectPath}
        draggable={!!onProjectDragStart}
        onDragStart={onProjectDragStart}
        data-project-header-path={projectPath}
      >
        <Folder className="h-3 w-3 shrink-0 text-muted-foreground/60" />
        <ChevronRight
          className={`h-3 w-3 shrink-0 transition-transform ${
            !collapsed ? "rotate-90" : ""
          }`}
        />
        <span className="truncate text-[10px] font-normal uppercase tracking-widest text-muted-foreground/60">
          {projectName}
        </span>
        <span className="ml-auto text-[10px] tabular-nums text-muted-foreground/40">
          {workspaces.length}
        </span>
      </SidebarMenuButton>
      {!collapsed && workspaces.length > 0 && (
        <SidebarMenuSub>
          {workspaces.map((ws, idx) => (
            <SidebarMenuSubItem key={ws.workspace_id}>
              <div
                data-ws-id={ws.workspace_id}
                data-ws-index={idx}
                draggable={!!onWorkspaceDragStart}
                onDragStart={onWorkspaceDragStart?.(ws.workspace_id, projectPath)}
                className={dragStateId === ws.workspace_id ? "opacity-40" : ""}
              >
                <SidebarWorkspaceRow
                  workspace={ws}
                  isActive={ws.workspace_id === activeWorkspaceId}
                  nested
                />
              </div>
            </SidebarMenuSubItem>
          ))}
        </SidebarMenuSub>
      )}
    </SidebarMenuItem>
  );
}
