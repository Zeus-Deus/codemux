import { useEffect } from "react";
import {
  SidebarGroup,
  SidebarGroupContent,
  useSidebar,
} from "@/components/ui/sidebar";
import { SidebarRailWorkspaces } from "./sidebar-rail-workspaces";
import { SidebarInbox } from "./sidebar-inbox";
import { SidebarProjectsView } from "./sidebar-projects-view";
import { useSidebarViewStore } from "./sidebar-view-store";

/** The sidebar's workspace area.
 *
 *  - Expanded: one of two views, chosen by the header's view switch
 *    (persisted, default Activity):
 *    - Activity: the flat workspace inbox (`SidebarInbox`) — project filter,
 *      one card per active workspace, Wrapping up / Snoozed / Settled.
 *    - Projects: chats, then one foldable group per project with one-line
 *      rows and follow-up children nested under their parent
 *      (`SidebarProjectsView`).
 *    Only the chosen view is mounted, so the other costs nothing.
 *  - Collapsed: the icon rail — one avatar button per active workspace with a
 *    per-workspace status dot (`SidebarRailWorkspaces`), mirroring the inbox
 *    order.
 */
export function SidebarWorkspaceList() {
  const { state } = useSidebar();
  const view = useSidebarViewStore((s) => s.view);
  const viewLoaded = useSidebarViewStore((s) => s.loaded);
  const loadView = useSidebarViewStore((s) => s.load);

  useEffect(() => {
    void loadView();
  }, [loadView]);

  if (state === "collapsed") {
    return <SidebarRailWorkspaces />;
  }

  return (
    <SidebarGroup className="flex-1 p-0">
      <SidebarGroupContent className="flex flex-1 flex-col">
        {/* Wait for the stored choice so a sidebar left on Projects never
            mounts the inbox for a frame first. The read is one local IPC. */}
        {!viewLoaded ? null : view === "projects" ? (
          <SidebarProjectsView />
        ) : (
          <SidebarInbox />
        )}
      </SidebarGroupContent>
    </SidebarGroup>
  );
}
