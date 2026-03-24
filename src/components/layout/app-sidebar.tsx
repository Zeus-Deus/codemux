import { Sidebar, SidebarContent, SidebarRail } from "@/components/ui/sidebar";
import { SidebarHeader } from "./sidebar-header";
import { SidebarWorkspaceList } from "./sidebar-workspace-list";
import { SidebarOpenflowSection } from "./sidebar-openflow-section";
import { SidebarAlertsSection } from "./sidebar-alerts-section";
import { SidebarPortsSection } from "./sidebar-ports-section";
import { SidebarFooter } from "./sidebar-footer";

export function AppSidebar() {
  return (
    <Sidebar side="left" variant="sidebar" collapsible="offcanvas">
      <SidebarHeader />
      <SidebarContent>
        <SidebarWorkspaceList />
      </SidebarContent>
      <SidebarOpenflowSection />
      <SidebarAlertsSection />
      <SidebarPortsSection />
      <SidebarFooter />
      <SidebarRail />
    </Sidebar>
  );
}
