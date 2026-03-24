import { useState, useMemo } from "react";
import { useAppStore } from "@/stores/app-store";
import { Bell, ChevronRight } from "lucide-react";

export function SidebarAlertsSection() {
  const appState = useAppStore((s) => s.appState);
  const [expanded, setExpanded] = useState(false);

  const notifications = useMemo(
    () => appState?.notifications ?? [],
    [appState],
  );
  const unreadCount = useMemo(
    () => notifications.filter((n) => !n.read).length,
    [notifications],
  );

  return (
    <div className="shrink-0 border-t border-sidebar-border">
      <button
        className="flex w-full items-center gap-1.5 px-3 py-1.5 text-left text-[11px] font-semibold uppercase tracking-wider text-sidebar-foreground/70 hover:text-sidebar-foreground transition-colors"
        onClick={() => setExpanded(!expanded)}
      >
        <ChevronRight
          className={`h-3 w-3 shrink-0 transition-transform ${expanded ? "rotate-90" : ""}`}
        />
        <span>Alerts</span>
        {unreadCount > 0 && (
          <span className="ml-auto text-[10px] font-semibold tabular-nums text-warning">
            {unreadCount}
          </span>
        )}
      </button>

      {expanded && (
        <div className="px-3 pb-2">
          {notifications.length === 0 ? (
            <div className="flex items-center gap-1.5 py-1 text-muted-foreground">
              <Bell className="h-3.5 w-3.5" />
              <span className="text-xs">No alerts</span>
            </div>
          ) : (
            <div className="space-y-0.5">
              {notifications.slice(0, 10).map((n) => (
                <div
                  key={n.notification_id}
                  className="flex items-start gap-1.5 rounded-md px-2 py-1 text-xs"
                >
                  <Bell
                    className={`h-3.5 w-3.5 shrink-0 mt-0.5 ${!n.read ? "text-warning" : "text-muted-foreground"}`}
                  />
                  <span className="truncate text-foreground">{n.message}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
