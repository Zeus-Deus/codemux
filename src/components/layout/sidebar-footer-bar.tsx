import { SidebarSeparator } from "@/components/ui/sidebar";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Settings, HelpCircle } from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { SidebarPortsPopover } from "./sidebar-ports-popover";

export function SidebarFooterBar() {
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const toggleCommandPalette = useUIStore((s) => s.toggleCommandPalette);

  return (
    <>
      <SidebarSeparator />
      <div className="flex items-center justify-between gap-1 px-2 py-1.5">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Settings"
              className="h-7 w-7 text-muted-foreground hover:text-foreground hover:bg-sidebar-accent"
              onClick={() => setShowSettings(true)}
            >
              <Settings className="size-[18px]" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="top" sideOffset={4} className="text-xs">
            Settings
          </TooltipContent>
        </Tooltip>

        <div className="flex items-center gap-1">
          <SidebarPortsPopover />
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Help"
                className="h-7 w-7 text-muted-foreground hover:text-foreground hover:bg-sidebar-accent"
                onClick={() => toggleCommandPalette()}
              >
                <HelpCircle className="size-[18px]" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="top" sideOffset={4} className="text-xs">
              Command palette
            </TooltipContent>
          </Tooltip>
        </div>
      </div>
    </>
  );
}
