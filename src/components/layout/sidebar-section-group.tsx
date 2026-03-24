import {
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarMenuSub,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";
import { ChevronRight } from "lucide-react";
import { toggleSectionCollapsed } from "@/tauri/commands";
import type { WorkspaceSectionSnapshot, WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  section: WorkspaceSectionSnapshot;
  workspaces: WorkspaceSnapshot[];
  activeWorkspaceId: string;
  onWorkspaceDragStart?: (workspaceId: string, sectionId: string | null) => (e: React.DragEvent) => void;
  onSectionDragStart?: (e: React.DragEvent) => void;
  dragStateId?: string | null;
}

export function SidebarSectionGroup({
  section,
  workspaces,
  activeWorkspaceId,
  onWorkspaceDragStart,
  onSectionDragStart,
  dragStateId,
}: Props) {
  const handleToggle = () => {
    toggleSectionCollapsed(section.section_id).catch(console.error);
  };

  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        size="sm"
        onClick={handleToggle}
        className="gap-1.5"
        draggable={!!onSectionDragStart}
        onDragStart={onSectionDragStart}
        data-section-header-id={section.section_id}
      >
        <span
          className="inline-block h-1.5 w-1.5 shrink-0 rounded-full"
          style={{ backgroundColor: section.color }}
        />
        <ChevronRight
          className={`h-3 w-3 shrink-0 transition-transform ${
            !section.collapsed ? "rotate-90" : ""
          }`}
        />
        <span className="truncate text-[10px] font-normal uppercase tracking-widest text-muted-foreground/60">
          {section.name}
        </span>
        <span className="ml-auto text-[10px] tabular-nums text-muted-foreground/40">
          {workspaces.length}
        </span>
      </SidebarMenuButton>
      {!section.collapsed && workspaces.length > 0 && (
        <SidebarMenuSub>
          {workspaces.map((ws, idx) => (
            <SidebarMenuSubItem key={ws.workspace_id}>
              <div
                data-ws-id={ws.workspace_id}
                data-ws-index={idx}
                draggable={!!onWorkspaceDragStart}
                onDragStart={onWorkspaceDragStart?.(ws.workspace_id, section.section_id)}
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
