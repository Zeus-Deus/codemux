import {
  SidebarGroup,
  SidebarGroupContent,
  useSidebar,
} from "@/components/ui/sidebar";
import { SidebarRailProjects } from "./sidebar-rail-projects";
import { SidebarInbox } from "./sidebar-inbox";

/** The sidebar's workspace area.
 *
 *  - Expanded: the flat workspace inbox (`SidebarInbox`) — repo filter chips
 *    + one card per active workspace + the "Settled" section. This replaced
 *    the nested project tree (project groups, drag-reorder, the pinned
 *    "Needs you" strip and the LIVE "gather on top" section).
 *  - Collapsed: the icon rail — one project avatar per group with aggregate
 *    status dots and a hover flyout (unchanged).
 */
export function SidebarWorkspaceList() {
  const { state } = useSidebar();

  if (state === "collapsed") {
    return <SidebarRailProjects />;
  }

  return (
    <SidebarGroup className="p-0">
      <SidebarGroupContent>
        <SidebarInbox />
      </SidebarGroupContent>
    </SidebarGroup>
  );
}
