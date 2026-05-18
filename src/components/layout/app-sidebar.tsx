import { Sidebar, SidebarContent, SidebarFooter, SidebarRail } from "@/components/ui/sidebar";
import { SidebarActionRow } from "./sidebar-action-row";
import { SidebarWorkspaceList } from "./sidebar-workspace-list";
import { SidebarSetupBanner } from "./sidebar-setup-banner";
import { SidebarFooterBar } from "./sidebar-footer-bar";
import { CloneDialog } from "@/components/overlays/clone-dialog";
import { NewRunDialog } from "@/components/openflow/new-run-dialog";

export function AppSidebar() {
  return (
    <Sidebar side="left" variant="sidebar" collapsible="offcanvas">
      <SidebarActionRow />
      <SidebarContent>
        <SidebarWorkspaceList />
      </SidebarContent>
      <SidebarSetupBanner />
      <SidebarFooter className="gap-0 p-0">
        <SidebarFooterBar />
      </SidebarFooter>
      <SidebarRail />
      <CloneDialog />
      {/* OpenFlow's NewRunDialog used to live inside the (now removed)
          sidebar section. Keep it mounted here so the "Start Run"
          button inside an active OpenFlow workspace still opens it. */}
      <NewRunDialog />
    </Sidebar>
  );
}
