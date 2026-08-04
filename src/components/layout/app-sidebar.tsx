import { Sidebar, SidebarContent, SidebarFooter, SidebarRail } from "@/components/ui/sidebar";
import { SidebarActionRow } from "./sidebar-action-row";
import { SidebarWorkspaceList } from "./sidebar-workspace-list";
import { SidebarSetupBanner } from "./sidebar-setup-banner";
import { SidebarFooterBar } from "./sidebar-footer-bar";
import { CloneDialog } from "@/components/overlays/clone-dialog";
import { NewWorkspaceDialog } from "@/components/overlays/new-workspace-dialog";
import { useUIStore } from "@/stores/ui-store";

export function AppSidebar() {
  // Mounted here (not inside SidebarWorkspaceList) so it stays mounted in
  // both expanded and collapsed-rail states — otherwise clicking "New
  // agent"/"New workspace" while collapsed sets the open flag but the dialog
  // only appears once the sidebar is expanded and that subtree mounts.
  const showNewWorkspaceDialog = useUIStore((s) => s.showNewWorkspaceDialog);
  const setShowNewWorkspaceDialog = useUIStore(
    (s) => s.setShowNewWorkspaceDialog,
  );

  return (
    <Sidebar side="left" variant="sidebar" collapsible="icon">
      <SidebarActionRow />
      {/* Override the primitive's icon-mode `overflow-hidden` so the project
          rail can still scroll vertically when there are many projects. */}
      <SidebarContent className="group-data-[collapsible=icon]:overflow-auto">
        <SidebarWorkspaceList />
      </SidebarContent>
      <SidebarSetupBanner />
      <SidebarFooter className="gap-0 p-0">
        <SidebarFooterBar />
      </SidebarFooter>
      <SidebarRail />
      <NewWorkspaceDialog
        open={showNewWorkspaceDialog}
        onOpenChange={setShowNewWorkspaceDialog}
      />
      <CloneDialog />
    </Sidebar>
  );
}
