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
}

export function SidebarSectionGroup({ section, workspaces, activeWorkspaceId }: Props) {
  const handleToggle = () => {
    toggleSectionCollapsed(section.section_id).catch(console.error);
  };

  return (
    <SidebarMenuItem>
      <SidebarMenuButton size="sm" onClick={handleToggle} className="gap-1.5">
        <span
          className="inline-block h-2 w-2 shrink-0 rounded-full"
          style={{ backgroundColor: section.color }}
        />
        <ChevronRight
          className={`h-3 w-3 shrink-0 transition-transform ${
            !section.collapsed ? "rotate-90" : ""
          }`}
        />
        <span className="truncate text-xs font-medium uppercase tracking-wider text-muted-foreground">
          {section.name}
        </span>
        <span className="ml-auto text-[10px] tabular-nums text-muted-foreground/50">
          {workspaces.length}
        </span>
      </SidebarMenuButton>
      {!section.collapsed && workspaces.length > 0 && (
        <SidebarMenuSub>
          {workspaces.map((ws) => (
            <SidebarMenuSubItem key={ws.workspace_id}>
              <SidebarWorkspaceRow
                workspace={ws}
                isActive={ws.workspace_id === activeWorkspaceId}
                nested
              />
            </SidebarMenuSubItem>
          ))}
        </SidebarMenuSub>
      )}
    </SidebarMenuItem>
  );
}
