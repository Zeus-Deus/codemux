import { useState, useEffect, useRef } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useSidebar } from "@/components/ui/sidebar";
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
  SlidersHorizontal,
  RotateCcw,
  MoreHorizontal,
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
import { SidebarPullRequestsButton } from "./sidebar-pr-button";
import { SidebarDevicesButton } from "./sidebar-devices-button";

import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  FOOTER_ICONS,
  getFooterAction,
  isFooterActionAvailable,
} from "@/lib/footer-actions";
import { useFooterPinsStore, type FooterPin } from "@/stores/footer-pins-store";
import { CustomizeFooterDialog } from "./customize-footer-dialog";
import { useFooterAvailability } from "./footer-availability";

/**
 * The menu's bottom strip: which build is running, and whether it is the
 * current one. It replaced a disabled "Codemux v0.0.0" menu item — a version
 * string is a fact about the app, not an action, and rendering it as a dead
 * row meant the one row users tried to click was the one that could never do
 * anything. Flush to the container's bottom corners so it reads as chrome.
 */
function AppMenuFooter({ version }: { version: string | null }) {
  const state = useUpdateStatusStore((s) => s.state);
  const published = useUpdateStatusStore((s) => s.published);
  const updateVersion = useUpdateStatusStore((s) => s.updateVersion);
  const isRemote = useUpdateStatusStore((s) => s.isRemote);
  const startDownload = useUpdateStatusStore((s) => s.startDownload);
  const installAndRestart = useUpdateStatusStore((s) => s.installAndRestart);
  const requestDesktopUpdate = useUpdateStatusStore(
    (s) => s.requestDesktopUpdate,
  );

  // Copy stays short enough to sit on one line beside the version: the strip
  // is a status readout, not the update flow — the toast owns that.
  let label = "Up to date";
  let tone = "text-status-open";
  let action: (() => void) | null = null;
  if (state === "checking") {
    label = "Checking…";
    tone = "text-muted-foreground";
  } else if (state === "update-available") {
    // The remote client has no updater plugin, so `startDownload` there is a
    // no-op; its only route is asking the desktop to update itself, exactly as
    // the toast's "Update & restart desktop" button does.
    label = isRemote ? "Update desktop" : "Update available";
    tone = "text-status-working";
    action = isRemote ? requestDesktopUpdate : startDownload;
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
      {/* No checker has published yet (the first check is still pending, or
          this is a dev build where it never runs). The version alone is still
          a fact; "Up to date" would be a claim nothing has verified. */}
      {!published ? null : action ? (
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
  onCustomize,
}: {
  tooltipSide?: "top" | "right";
  onCustomize: () => void;
}) {
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const toggleCommandPalette = useUIStore((s) => s.toggleCommandPalette);
  const accountName = useAuthStore(
    (s) => s.user?.name ?? s.user?.email ?? null,
  );
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
      <DropdownMenuContent side="top" align="start" className="w-[252px] pb-0">
        <DropdownMenuItem
          className={MENU_ROW}
          onClick={() => setShowSettings(true)}
        >
          <Settings />
          <span className="flex-1">Settings</span>
          <MenuKeycap actionId="openSettings" />
        </DropdownMenuItem>
        <DropdownMenuItem className={MENU_ROW} onClick={onCustomize}>
          <SlidersHorizontal />
          <span className="flex-1">Customize footer</span>
        </DropdownMenuItem>
        <DropdownMenuItem
          className={MENU_ROW}
          onClick={() => useFooterPinsStore.getState().reset()}
        >
          <RotateCcw />
          <span className="flex-1">Restore footer defaults</span>
        </DropdownMenuItem>
        <DropdownMenuItem
          className={MENU_ROW}
          onClick={() => toggleCommandPalette()}
        >
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

function FooterDestination({
  pin,
  labeled = false,
  fullWidth = false,
  tooltipSide,
}: {
  pin: FooterPin;
  labeled?: boolean;
  fullWidth?: boolean;
  tooltipSide: "top" | "right";
}) {
  const setShowAutomations = useUIStore((s) => s.setShowAutomations);
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const action = getFooterAction(pin.id)!;
  const Icon = pin.iconId ? FOOTER_ICONS[pin.iconId] : action.icon;
  if (pin.id === "codemux.devices.open")
    return (
      <SidebarDevicesButton
        icon={Icon}
        labeled={labeled}
        tooltipSide={tooltipSide}
      />
    );
  if (pin.id === "codemux.pull-requests.open")
    return (
      <SidebarPullRequestsButton
        icon={Icon}
        labeled={labeled}
        tooltipSide={tooltipSide}
      />
    );
  if (pin.id === "codemux.ports.open")
    return (
      <SidebarPortsPopover
        icon={Icon}
        labeled={labeled}
        tooltipSide={tooltipSide}
      />
    );
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size={labeled ? "sm" : "icon-xs"}
          aria-label={action.label}
          className={cn(
            "h-7 shrink-0 rounded-[7px] text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground",
            labeled ? "gap-1.5 px-2 text-[12px]" : "w-7",
            fullWidth && "w-full justify-start",
          )}
          onClick={() =>
            action.section
              ? setShowSettings(true, action.section)
              : setShowAutomations(true)
          }
        >
          <Icon className={labeled ? "size-[13px]" : "size-[18px]"} />
          {labeled && action.label}
        </Button>
      </TooltipTrigger>
      <TooltipContent side={tooltipSide} sideOffset={4} className="text-xs">
        {action.label}
      </TooltipContent>
    </Tooltip>
  );
}

export function SidebarFooterBar() {
  const { state } = useSidebar();
  const collapsed = state === "collapsed";
  const pins = useFooterPinsStore((s) => s.pins);
  const { agentChatEnabled, hasDevices } = useFooterAvailability();
  const [customizing, setCustomizing] = useState(false);
  const [overflowOpen, setOverflowOpen] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(300);
  const [height, setHeight] = useState(() => window.innerHeight);
  useEffect(() => {
    const element = container.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => setWidth(element.clientWidth));
    observer.observe(element);
    const resize = () => setHeight(window.innerHeight);
    window.addEventListener("resize", resize);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", resize);
    };
  }, []);
  const availablePins = pins.filter((pin) =>
    isFooterActionAvailable(
      getFooterAction(pin.id)!,
      agentChatEnabled,
      hasDevices,
    ),
  );
  // Keep every destination directly reachable before spending space on a
  // label. Only introduce overflow once the icon-only row also runs out of room.
  const labeledCapacity = Math.floor((width - 16 - 30 - 82) / 30);
  const showAutomationLabel =
    !collapsed &&
    availablePins.length <= labeledCapacity &&
    availablePins.some((pin) => pin.id === "codemux.automations.open");
  // Reserve space for the permanent menu, then the overflow trigger if needed.
  const capacity = collapsed
    ? Math.max(1, Math.floor((height * 0.35) / 30) - 1)
    : Math.max(
        0,
        Math.floor((width - 16 - 30 - (showAutomationLabel ? 82 : 0)) / 30),
      );
  const visibleCount =
    availablePins.length > capacity ? Math.max(0, capacity - 1) : capacity;
  const visible = availablePins.slice(0, visibleCount);
  const overflow = availablePins.slice(visibleCount);
  const tooltipSide = collapsed ? "right" : "top";

  return (
    <>
      <div
        ref={container}
        data-testid="sidebar-footer"
        className={cn(
          "flex shrink-0 gap-0.5 border-t border-sidebar-border",
          collapsed
            ? "flex-col items-center px-1 py-1.5"
            : "h-[42px] items-center px-2",
        )}
      >
        {visible.map((pin) => (
          <FooterDestination
            key={pin.id}
            pin={pin}
            labeled={
              showAutomationLabel && pin.id === "codemux.automations.open"
            }
            tooltipSide={tooltipSide}
          />
        ))}
        {overflow.length > 0 && (
          <Popover open={overflowOpen} onOpenChange={setOverflowOpen}>
            <PopoverTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="More footer destinations"
                title="More footer destinations"
                className="size-7 shrink-0 text-muted-foreground"
              >
                <MoreHorizontal className="size-4" />
              </Button>
            </PopoverTrigger>
            <PopoverContent
              side={collapsed ? "right" : "top"}
              align="start"
              className="w-64 p-2"
            >
              <p className="px-2 py-1 text-xs font-medium text-muted-foreground">
                Footer destinations
              </p>
              <div className="thin-scrollbar max-h-[50vh] overflow-y-auto p-1">
                {overflow.map((pin) => (
                  <div key={pin.id} className="py-0.5">
                    <FooterDestination
                      pin={pin}
                      labeled
                      fullWidth
                      tooltipSide="right"
                    />
                  </div>
                ))}
              </div>
            </PopoverContent>
          </Popover>
        )}
        <div className={collapsed ? "" : "ml-auto"}>
          <AppMenu
            tooltipSide={tooltipSide}
            onCustomize={() => setCustomizing(true)}
          />
        </div>
      </div>
      {customizing && (
        <CustomizeFooterDialog
          open={customizing}
          onOpenChange={setCustomizing}
        />
      )}
    </>
  );
}
