import { Home } from "lucide-react";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { openHomeChat } from "@/lib/home-chat";

export function SidebarHomeRow() {
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const isActive = useAppStore((s) => {
    if (!s.appState) return false;
    const activeId = s.appState.active_workspace_id;
    const active = s.appState.workspaces.find((w) => w.workspace_id === activeId);
    return active?.workspace_type === "home";
  });

  if (!enableAgentChat) return null;

  const handleClick = () => {
    openHomeChat().catch((err) => {
      console.error("[sidebar-home-row] failed to open home chat:", err);
    });
  };

  return (
    <div
      role="button"
      tabIndex={0}
      aria-label="Home"
      onClick={handleClick}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") handleClick();
      }}
      className={cn(
        "flex w-full pl-3 pr-2 py-1.5 text-sm cursor-pointer group relative",
        "hover:bg-muted/50 transition-colors",
        isActive && "bg-muted",
      )}
    >
      {isActive && (
        <div className="absolute left-0 top-0 bottom-0 w-0.5 bg-foreground rounded-r" />
      )}
      <div className="size-6 flex items-center justify-center shrink-0 mr-2.5">
        <Home className="h-4 w-4 shrink-0 text-muted-foreground" />
      </div>
      <div className="flex-1 min-w-0">
        <span
          className={cn(
            "truncate text-[13px] leading-tight font-medium",
            isActive ? "text-foreground" : "text-foreground/80",
          )}
        >
          Home
        </span>
      </div>
    </div>
  );
}
