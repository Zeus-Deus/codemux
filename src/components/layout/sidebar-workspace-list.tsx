import {
  SidebarGroup,
  SidebarGroupLabel,
  SidebarGroupAction,
  SidebarGroupContent,
  SidebarMenu,
} from "@/components/ui/sidebar";
import { useSectionedWorkspaces, useAppStore } from "@/stores/app-store";
import { SidebarSectionGroup } from "./sidebar-section-group";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";
import { Plus } from "lucide-react";

export function SidebarWorkspaceList() {
  const { sectionGroups, unsorted } = useSectionedWorkspaces();
  const activeWorkspaceId = useAppStore(
    (s) => s.appState?.active_workspace_id ?? "",
  );

  return (
    <SidebarGroup>
      <SidebarGroupLabel>Workspaces</SidebarGroupLabel>
      <SidebarGroupAction title="New workspace">
        <Plus className="h-3.5 w-3.5" />
      </SidebarGroupAction>
      <SidebarGroupContent>
        <SidebarMenu>
          {unsorted.map((ws) => (
            <SidebarWorkspaceRow
              key={ws.workspace_id}
              workspace={ws}
              isActive={ws.workspace_id === activeWorkspaceId}
            />
          ))}
          {sectionGroups.map((group) => (
            <SidebarSectionGroup
              key={group.section.section_id}
              section={group.section}
              workspaces={group.workspaces}
              activeWorkspaceId={activeWorkspaceId}
            />
          ))}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}
