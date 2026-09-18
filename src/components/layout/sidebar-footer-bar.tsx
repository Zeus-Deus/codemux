import { useState, useEffect, useRef } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  DndContext,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  horizontalListSortingStrategy,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useSidebar } from "@/components/ui/sidebar";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
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
import {
  useFooterPinsStore,
  type FooterPin,
} from "@/stores/footer-pins-store";
import type { FooterActionId } from "@/lib/footer-actions";
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
    <div className="-mx-1.5 mt-1.5 flex h-8 items-center gap-2 rounded-b-lg border-t border-border/70 bg-background/50 px-3.5">
      <span className="font-mono text-caption text-muted-foreground/70">
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
            // `tone` is a fixed status colour, so hover has to *deepen* it on
            // a light rail and *lift* it on a dark one — a single
            // `brightness-125` washes the label out to nothing on white.
            "flex items-center gap-1.5 rounded-sm px-1 text-label transition-colors duration-150 hover:brightness-90 dark:hover:brightness-125",
            tone,
          )}
        >
          {status}
        </button>
      ) : (
        <span className={cn("flex items-center gap-1.5 text-label", tone)}>
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
              size="icon-sm"
              aria-label="Menu"
              className="text-muted-foreground hover:text-foreground hover:bg-surface-2"
            >
              <Settings className="size-[18px]" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side={tooltipSide} sideOffset={4} className="text-label">
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
  const button = (
    <Button
      variant="ghost"
      size={labeled ? "sm" : "icon-sm"}
      aria-label={action.label}
      className={cn(
        "shrink-0 text-muted-foreground hover:bg-surface-2 hover:text-foreground",
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
  );
  // A labelled row already says what it is; only icon-only buttons need the tooltip.
  if (labeled) return button;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{button}</TooltipTrigger>
      <TooltipContent side={tooltipSide} sideOffset={4} className="text-label">
        {action.label}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * A footer icon that can be dragged to a new slot. The wrapper is only the
 * drag handle: the 5px activation distance keeps a plain click reaching the
 * inner button, and dnd-kit swallows the click that ends a real drag.
 * `attributes` is not spread so the inner button stays the only focusable,
 * semantic control.
 */
function SortableFooterDestination({
  pin,
  tooltipSide,
}: {
  pin: FooterPin;
  tooltipSide: "top" | "right";
}) {
  const { listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: pin.id });
  return (
    <div
      ref={setNodeRef}
      data-testid={`footer-pin-${pin.id}`}
      style={{
        transform: CSS.Translate.toString(transform),
        transition,
      }}
      {...listeners}
      className={cn(
        "shrink-0 touch-none",
        isDragging && "relative z-10 opacity-60",
      )}
    >
      <FooterDestination pin={pin} tooltipSide={tooltipSide} />
    </div>
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
  // Every destination is an icon, so the row is one uniform strip. Reserve
  // space for the permanent menu, then the overflow trigger if needed.
  const capacity = collapsed
    ? Math.max(1, Math.floor((height * 0.35) / 30) - 1)
    : Math.max(0, Math.floor((width - 16 - 30) / 30));
  const visibleCount =
    availablePins.length > capacity ? Math.max(0, capacity - 1) : capacity;
  const visible = availablePins.slice(0, visibleCount);
  const overflow = availablePins.slice(visibleCount);
  const tooltipSide = collapsed ? "right" : "top";
  const reorderPin = useFooterPinsStore((s) => s.reorderPin);
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
  );
  const handleDragEnd = ({ active, over }: DragEndEvent) => {
    if (over && active.id !== over.id)
      reorderPin(active.id as FooterActionId, over.id as FooterActionId);
  };

  return (
    // Footer labels are plain text, so they never need to stay open for the
    // pointer to travel into them. Radix's hoverable-content grace area would
    // otherwise keep the previous icon's label up — and ignore the next icon —
    // while the pointer slides sideways along the strip.
    <TooltipProvider disableHoverableContent>
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
        {/* The app menu anchors the start of the row; destinations pack
            beside it so the strip reads as one left-aligned group. */}
        <AppMenu
          tooltipSide={tooltipSide}
          onCustomize={() => setCustomizing(true)}
        />
        {/* Destinations reorder by drag; the menu stays outside the sortable
            list so it always anchors the start. */}
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={handleDragEnd}
        >
          <SortableContext
            items={visible.map((pin) => pin.id)}
            strategy={
              collapsed
                ? verticalListSortingStrategy
                : horizontalListSortingStrategy
            }
          >
            {visible.map((pin) => (
              <SortableFooterDestination
                key={pin.id}
                pin={pin}
                tooltipSide={tooltipSide}
              />
            ))}
          </SortableContext>
        </DndContext>
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
              <p className="px-2 py-1 text-label font-medium text-muted-foreground">
                Footer destinations
              </p>
              <div className="thin-scrollbar [scrollbar-gutter:stable] max-h-[50vh] overflow-y-auto p-1">
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
      </div>
      {customizing && (
        <CustomizeFooterDialog
          open={customizing}
          onOpenChange={setCustomizing}
        />
      )}
    </TooltipProvider>
  );
}
