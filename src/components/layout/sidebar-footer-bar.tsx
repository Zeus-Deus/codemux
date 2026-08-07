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
  LayoutGrid,
  LogOut,
  ExternalLink,
} from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useAuthStore } from "@/stores/auth-store";
import { useUpdateStatusStore } from "@/stores/update-status-store";
import { cn } from "@/lib/utils";
import {
  MENU_ROW,
  MENU_ROW_META,
  MENU_SEPARATOR,
  MenuKeycap,
} from "@/components/ui/menu-chrome";
import { SidebarPortsPopover } from "./sidebar-ports-popover";

/**
 * The menu's bottom strip: which build is running, and whether it is the
 * current one. It replaced a disabled "Codemux v0.0.0" menu item — a version
 * string is a fact about the app, not an action, and rendering it as a dead
 * row meant the one row users tried to click was the one that could never do
 * anything. Flush to the container's bottom corners so it reads as chrome.
 */
function AppMenuFooter({ version }: { version: string | null }) {
  const state = useUpdateStatusStore((s) => s.state);
  const updateVersion = useUpdateStatusStore((s) => s.updateVersion);
  const startDownload = useUpdateStatusStore((s) => s.startDownload);
  const installAndRestart = useUpdateStatusStore((s) => s.installAndRestart);

  // Copy stays short enough to sit on one line beside the version: the strip
  // is a status readout, not the update flow — the toast owns that.
  let label = "Up to date";
  let tone = "text-status-open";
  let action: (() => void) | null = null;
  if (state === "checking") {
    label = "Checking…";
    tone = "text-muted-foreground";
  } else if (state === "update-available") {
    label = "Update available";
    tone = "text-status-working";
    action = startDownload;
  } else if (state === "downloading") {
    label = "Downloading…";
    tone = "text-status-working";
  } else if (state === "ready") {
    label = "Restart to update";
    tone = "text-status-working";
    action = installAndRestart;
  } else if (state === "error") {
    label = "Update failed";
    tone = "text-status-attention";
  }

  const status = (
    <>
      <span className={cn("size-[5px] shrink-0 rounded-full bg-current")} />
      {label}
    </>
  );

  return (
    <div className="-mx-1.5 mt-1.5 flex h-8 items-center gap-2 rounded-b-[12px] border-t border-border/70 bg-background/50 px-3.5">
      <span className="font-mono text-[10px] text-muted-foreground/70">
        Codemux {version ? `v${version}` : ""}
      </span>
      <span className="flex-1" />
      {action ? (
        <button
          type="button"
          onClick={action}
          title={updateVersion ? `Version ${updateVersion}` : undefined}
          className={cn(
            "flex items-center gap-1.5 rounded-[5px] px-1 text-[10.5px] transition-colors hover:brightness-125",
            tone,
          )}
        >
          {status}
        </button>
      ) : (
        <span className={cn("flex items-center gap-1.5 text-[10.5px]", tone)}>
          {status}
        </span>
      )}
    </div>
  );
}

function AppMenu({
  tooltipSide = "top",
}: {
  tooltipSide?: "top" | "right";
}) {
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const toggleCommandPalette = useUIStore((s) => s.toggleCommandPalette);
  const accountName = useAuthStore((s) => s.user?.name ?? s.user?.email ?? null);
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
      {/* Bottom padding is zero so the version/update strip can sit flush in
          the container's bottom corners. */}
      <DropdownMenuContent
        side="top"
        align="start"
        className="w-[252px] pb-0"
      >
        <DropdownMenuItem className={MENU_ROW} onClick={() => setShowSettings(true)}>
          <Settings />
          <span className="flex-1">Settings</span>
          <MenuKeycap actionId="openSettings" />
        </DropdownMenuItem>
        <DropdownMenuItem className={MENU_ROW} onClick={() => toggleCommandPalette()}>
          <Search />
          <span className="flex-1">Command palette</span>
          <MenuKeycap actionId="commandPalette" />
        </DropdownMenuItem>
        <DropdownMenuItem
          className={MENU_ROW}
          onClick={() => setShowSettings(true, "shortcuts")}
        >
          <Keyboard />
          <span className="flex-1">Keyboard shortcuts</span>
          <MenuKeycap actionId="showShortcuts" />
        </DropdownMenuItem>
        <DropdownMenuSeparator className={MENU_SEPARATOR} />
        <DropdownMenuItem
          className={MENU_ROW}
          onClick={() => openUrl("https://docs.codemux.org/installation")}
        >
          <BookOpen />
          <span className="flex-1">Documentation</span>
          <ExternalLink className="ml-auto size-[11px] shrink-0 text-muted-foreground/60" />
        </DropdownMenuItem>
        <DropdownMenuItem
          className={MENU_ROW}
          onClick={() =>
            openUrl("https://github.com/Zeus-Deus/codemux/issues/new")
          }
        >
          <Bug />
          <span className="flex-1">Report issue</span>
          <ExternalLink className="ml-auto size-[11px] shrink-0 text-muted-foreground/60" />
        </DropdownMenuItem>
        <DropdownMenuSeparator className={MENU_SEPARATOR} />
        <DropdownMenuItem
          className={MENU_ROW}
          onClick={() => useAuthStore.getState().signOut()}
        >
          <LogOut />
          <span className="flex-1 text-muted-foreground">Sign out</span>
          {/* Only shown when the frontend actually knows the account — a
              signed-out or offline install gets no invented name. */}
          {accountName && (
            <span className={cn(MENU_ROW_META, "max-w-[96px] truncate")}>
              {accountName}
            </span>
          )}
        </DropdownMenuItem>
        <AppMenuFooter version={version} />
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
    // The sidebar keeps its compact destination footer after the workspace
    // context bar's removal; it no longer dictates any work-surface height.
    // 28px controls + 7px vertical padding.
    <div className="flex h-[42px] items-center gap-0.5 border-t border-sidebar-border px-2">
      <button
        type="button"
        aria-label="Automations"
        onClick={() => setShowAutomations(true)}
        className="flex h-7 flex-1 items-center justify-center gap-1.5 rounded-[7px] bg-transparent text-muted-foreground transition-colors hover:bg-foreground/[0.04] hover:text-foreground"
      >
        <CalendarClock className="size-[13px]" />
        <span className="text-[12px] font-medium">Automations</span>
      </button>
      <button
        type="button"
        aria-label="Workspaces"
        onClick={() => setShowWorkspacesOverview(true)}
        className="flex h-7 flex-1 items-center justify-center gap-1.5 rounded-[7px] bg-transparent text-muted-foreground transition-colors hover:bg-foreground/[0.04] hover:text-foreground"
      >
        <LayoutGrid className="size-[13px]" />
        <span className="text-[12px] font-medium">Workspaces</span>
      </button>
      <SidebarPortsPopover />
      <AppMenu />
    </div>
  );
}
