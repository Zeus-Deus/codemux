import { useMemo } from "react";
import {
  SidebarGroup,
  SidebarGroupLabel,
  SidebarGroupContent,
  SidebarMenu,
  SidebarMenuItem,
  SidebarMenuButton,
} from "@/components/ui/sidebar";
import { useAppStore } from "@/stores/app-store";
import { Bell } from "lucide-react";

export function SidebarAlertsSection() {
  const appState = useAppStore((s) => s.appState);
  const notifications = useMemo(
    () => appState?.notifications ?? [],
    [appState],
  );
  const unreadCount = useMemo(
    () => notifications.filter((n) => !n.read).length,
    [notifications],
  );

  return (
    <SidebarGroup>
      <SidebarGroupLabel>
        Alerts
        {unreadCount > 0 && (
          <span className="ml-auto text-[10px] font-semibold tabular-nums text-yellow-500">
            {unreadCount}
          </span>
        )}
      </SidebarGroupLabel>
      <SidebarGroupContent>
        <SidebarMenu>
          {notifications.length === 0 ? (
            <SidebarMenuItem>
              <SidebarMenuButton size="sm" className="text-muted-foreground">
                <Bell className="h-4 w-4" />
                <span>No alerts</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ) : (
            notifications.slice(0, 5).map((n) => (
              <SidebarMenuItem key={n.notification_id}>
                <SidebarMenuButton size="sm">
                  <Bell className={`h-4 w-4 ${!n.read ? "text-yellow-500" : ""}`} />
                  <span className="truncate">{n.message}</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
            ))
          )}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}
