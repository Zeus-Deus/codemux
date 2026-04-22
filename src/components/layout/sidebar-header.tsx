import { SidebarHeader as ShadcnSidebarHeader, SidebarSeparator } from "@/components/ui/sidebar";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { openHomeChat } from "@/lib/home-chat";
import { Plus } from "lucide-react";

export function SidebarHeader() {
  const setShowDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);

  const handlePlusClick = async (e: React.MouseEvent) => {
    if (e.shiftKey || !enableAgentChat) {
      setShowDialog(true);
      return;
    }

    try {
      await openHomeChat();
    } catch (err) {
      console.error("[sidebar-header] failed to open home chat:", err);
      setShowDialog(true);
    }
  };

  return (
    <ShadcnSidebarHeader className="gap-0 p-0">
      {/* + New Workspace row */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            aria-label="New workspace"
            className="w-full justify-start pl-3 pr-2 py-3 h-auto text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
            onClick={handlePlusClick}
          >
            <div className="size-6 flex items-center justify-center shrink-0 mr-2.5">
              <Plus className="h-3.5 w-3.5" />
            </div>
            <span>New Workspace</span>
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4} className="text-xs">
          {enableAgentChat
            ? "New chat at home · Shift+click for new workspace"
            : "New workspace"}
        </TooltipContent>
      </Tooltip>
      <SidebarSeparator />
    </ShadcnSidebarHeader>
  );
}
