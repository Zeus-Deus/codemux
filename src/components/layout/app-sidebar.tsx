import { Sidebar, SidebarContent, SidebarFooter, SidebarRail } from "@/components/ui/sidebar";
import { SidebarHeader } from "./sidebar-header";
import { SidebarWorkspaceList } from "./sidebar-workspace-list";
import { SidebarOpenflowSection } from "./sidebar-openflow-section";
import { SidebarPortsSection } from "./sidebar-ports-section";
import { SidebarSetupBanner } from "./sidebar-setup-banner";
import { SidebarAddRepo } from "./sidebar-add-repo";
import { CloneDialog } from "@/components/overlays/clone-dialog";

export function AppSidebar() {
  return (
    <Sidebar side="left" variant="sidebar" collapsible="offcanvas">
      <SidebarHeader />
      <SidebarContent>
        <SidebarWorkspaceList />
      </SidebarContent>
      <SidebarSetupBanner />
      <SidebarOpenflowSection />
      <SidebarPortsSection />
      <SidebarFooter className="gap-0 p-0">
        <SidebarAddRepo />
      </SidebarFooter>
      <SidebarRail />
      <CloneDialog />
    </Sidebar>
  );
}
