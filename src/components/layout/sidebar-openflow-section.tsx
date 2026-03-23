import {
  SidebarGroup,
  SidebarGroupLabel,
  SidebarGroupContent,
  SidebarMenu,
  SidebarMenuItem,
  SidebarMenuButton,
} from "@/components/ui/sidebar";
import { useAppStore } from "@/stores/app-store";
import { Workflow } from "lucide-react";

export function SidebarOpenflowSection() {
  const openflowWorkspaces = useAppStore(
    (s) => s.appState?.workspaces.filter((w) => w.workspace_type === "open_flow") ?? [],
  );

  return (
    <SidebarGroup>
      <SidebarGroupLabel>OpenFlow</SidebarGroupLabel>
      <SidebarGroupContent>
        <SidebarMenu>
          {openflowWorkspaces.length === 0 ? (
            <SidebarMenuItem>
              <SidebarMenuButton size="sm" className="text-muted-foreground">
                <Workflow className="h-4 w-4" />
                <span>No active runs</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ) : (
            openflowWorkspaces.map((ws) => (
              <SidebarMenuItem key={ws.workspace_id}>
                <SidebarMenuButton size="sm">
                  <Workflow className="h-4 w-4" />
                  <span className="truncate">{ws.title}</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
            ))
          )}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}
