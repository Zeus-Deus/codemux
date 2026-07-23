import { useState, useEffect } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { SidebarSeparator, useSidebar } from "@/components/ui/sidebar";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Settings,
  Search,
  Keyboard,
  BookOpen,
  Bug,
  CalendarClock,
  Info,
  LayoutGrid,
  LogOut,
  ExternalLink,
} from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { SidebarPortsPopover } from "./sidebar-ports-popover";

function ShortcutHint({ actionId }: { actionId: string }) {
  const { getKeysForAction } = useResolvedKeybinds();
  const keys = getKeysForAction(actionId);
  if (!keys) return null;
  return <kbd className="ml-auto text-[10px] text-muted-foreground">{keys}</kbd>;
}

function AppMenu({
  tooltipSide = "top",
}: {
  tooltipSide?: "top" | "right";
}) {
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const toggleCommandPalette = useUIStore((s) => s.toggleCommandPalette);
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    getVersion().then(setVersion);
  }, []);

  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Menu"
              className="h-7 w-7 rounded-[7px] text-muted-foreground hover:text-foreground hover:bg-foreground/[0.04]"
            >
              <Settings className="size-[18px]" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side={tooltipSide} sideOffset={4} className="text-xs">
          Menu
        </TooltipContent>
      </Tooltip>
      <DropdownMenuContent side="top" align="start" className="w-52">
        <DropdownMenuItem onClick={() => setShowSettings(true)}>
          <Settings className="h-4 w-4" />
          <span>Settings</span>
          <ShortcutHint actionId="openSettings" />
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => toggleCommandPalette()}>
          <Search className="h-4 w-4" />
          <span>Command palette</span>
          <ShortcutHint actionId="commandPalette" />
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => setShowSettings(true, "shortcuts")}>
          <Keyboard className="h-4 w-4" />
          <span>Keyboard shortcuts</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onClick={() => openUrl("https://docs.codemux.org/installation")}
        >
          <BookOpen className="h-4 w-4" />
          <span>Documentation</span>
          <ExternalLink className="ml-auto h-3 w-3 text-muted-foreground" />
        </DropdownMenuItem>
        <DropdownMenuItem
          onClick={() =>
            openUrl("https://github.com/Zeus-Deus/codemux/issues/new")
          }
        >
          <Bug className="h-4 w-4" />
          <span>Report issue</span>
          <ExternalLink className="ml-auto h-3 w-3 text-muted-foreground" />
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem disabled>
          <Info className="h-4 w-4" />
          <span>Codemux {version ? `v${version}` : ""}</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onClick={() => {
            import("@/stores/auth-store").then(({ useAuthStore }) =>
              useAuthStore.getState().signOut(),
            );
          }}
        >
          <LogOut className="h-4 w-4" />
          <span>Sign out</span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function SidebarFooterBar() {
  const { state } = useSidebar();
  const setShowAutomations = useUIStore((s) => s.setShowAutomations);
  const setShowWorkspacesOverview = useUIStore(
    (s) => s.setShowWorkspacesOverview,
  );

  if (state === "collapsed") {
    // Same destinations as the expanded row, restacked vertically as an
    // icon rail. Order matches the expanded row left-to-right so the two
    // layouts stay muscle-memory compatible.
    return (
      <>
        <SidebarSeparator />
        <div className="flex flex-col items-center gap-0.5 px-1 py-1.5">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Automations"
                onClick={() => setShowAutomations(true)}
                className="size-7 rounded-[7px] text-muted-foreground hover:text-foreground hover:bg-foreground/[0.04]"
              >
                <CalendarClock className="size-[18px]" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right" sideOffset={4} className="text-xs">
              Automations
            </TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Workspaces"
                onClick={() => setShowWorkspacesOverview(true)}
                className="size-7 rounded-[7px] text-muted-foreground hover:text-foreground hover:bg-foreground/[0.04]"
              >
                <LayoutGrid className="size-[18px]" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right" sideOffset={4} className="text-xs">
              Workspaces
            </TooltipContent>
          </Tooltip>
          <SidebarPortsPopover />
          <AppMenu tooltipSide="right" />
        </div>
      </>
    );
  }

  return (
    // Fixed 42px with a border-top (instead of SidebarSeparator + padding)
    // so its top edge lands on the exact same pixel row as the workspace
    // context bar's border-top — the two read as one continuous line
    // across the bottom of the app. 28px controls + 7px vertical padding.
    <div className="flex h-[42px] items-center gap-0.5 border-t border-sidebar-border px-2">
      <button
        type="button"
        aria-label="Automations"
        onClick={() => setShowAutomations(true)}
        className="flex h-7 flex-1 items-center justify-center gap-1.5 rounded-[7px] bg-transparent text-muted-foreground transition-colors hover:bg-foreground/[0.04] hover:text-foreground"
      >
        <CalendarClock className="size-[13px]" />
        <span className="text-[11.5px] font-medium">Automations</span>
      </button>
      <button
        type="button"
        aria-label="Workspaces"
        onClick={() => setShowWorkspacesOverview(true)}
        className="flex h-7 flex-1 items-center justify-center gap-1.5 rounded-[7px] bg-transparent text-muted-foreground transition-colors hover:bg-foreground/[0.04] hover:text-foreground"
      >
        <LayoutGrid className="size-[13px]" />
        <span className="text-[11.5px] font-medium">Workspaces</span>
      </button>
      <SidebarPortsPopover />
      <AppMenu />
    </div>
  );
}
