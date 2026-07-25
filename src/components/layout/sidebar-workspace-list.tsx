import {
  SidebarGroup,
  SidebarGroupContent,
  useSidebar,
} from "@/components/ui/sidebar";
import { SidebarRailWorkspaces } from "./sidebar-rail-workspaces";
import { SidebarInbox } from "./sidebar-inbox";

/** The sidebar's workspace area.
 *
 *  - Expanded: the flat workspace inbox (`SidebarInbox`) — repo filter chips
 *    + one card per active workspace + the "Settled" section. This replaced
 *    the nested project tree (project groups, drag-reorder, the pinned
 *    "Needs you" strip and the LIVE "gather on top" section).
 *  - Collapsed: the icon rail — one avatar button per active workspace with a
 *    per-workspace status dot (`SidebarRailWorkspaces`), mirroring the inbox
 *    order.
 */
export function SidebarWorkspaceList() {
  const { state } = useSidebar();

  if (state === "collapsed") {
    return <SidebarRailWorkspaces />;
  }

  return (
    <SidebarGroup className="p-0">
      <SidebarGroupContent>
        <SidebarInbox />
      </SidebarGroupContent>
    </SidebarGroup>
  );
}
