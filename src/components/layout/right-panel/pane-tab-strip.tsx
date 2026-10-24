/**
 * The deck's single row of panel chrome (design "Right panel · pane deck").
 *
 * One band carries everything: closable icon tabs and the `+` menu on the
 * left, then the *active pane's* own actions on the right. There used to be
 * a second 32px breadcrumb row underneath holding those pane actions — it
 * repeated the workspace name (already in the sidebar and the composer)
 * and the pane name (already the tab label), so above the first line of
 * real content the panel rendered three stacked bands. It renders one now.
 *
 * **In GUI chrome that row *is* the titlebar band** (`inTitlebar`): 40px
 * tall, flush with the window's top edge, sharing the band with the fixed
 * top-right cluster that carries the panel's own expand/close controls (see
 * `src/lib/titlebar-geometry.ts`). It used to reserve a blank `mt-10` strip
 * for the floating titlebar and start below it, which read as a pane inside
 * a pane with an empty header. With legacy chrome the in-flow `h-9` bar
 * still occupies that space, so there the row stays 36px, starts at the top
 * of the panel, and keeps the panel controls itself.
 *
 * Tabs are deliberately light: the active tab is a 7% foreground fill with
 * no border and no shadow, the inactive ones are transparent text. The
 * bordered active chip was the single heaviest thing in the panel.
 *
 * Live activity is carried by the badge itself (a mono count, accent-tinted
 * while its pane is active) — nothing in the strip blinks; live work is
 * shown inside the thread.
 *
 * **Overflow.** The deck grows: every file opened from the tree is another
 * tab, and a busy thread adds tasks, orchestration and subagents on top of
 * the three defaults. Tabs never shrink. Each one keeps its icon and a label
 * that is capped and truncated, and the row scrolls sideways (a plain
 * vertical wheel pans it) with a soft fade on whichever edge is clipped.
 * Panning alone left the clipped tabs hard to find at the default width,
 * where the row shows about one and a half, so while any tab is out of
 * view the row also lists them all: in its "…" once the pane actions have
 * folded there, or behind an "All tabs" chevron beside the tabs while they
 * sit inline. The active tab is kept in view when it is picked and when
 * the panel narrows. Tabs reorder by drag, same gesture as the titlebar tabs.
 *
 * **Narrow panels.** In the titlebar band the row shares its width with the
 * fixed top-right cluster and the native window buttons (~174px). The row
 * never wraps: the tabs keep their single line and simply scroll. It used to
 * stack the tabs into a second row whenever they didn't all fit, which made
 * the whole header jump by a row mid-drag and left a near-empty band above.
 *
 * **Close affordance.** A tab's leading icon doubles as its close button and
 * turns into an `×` on hover, so closing never covers the label's tail.
 * Right-click offers the bulk closes the main tab bar has.
 */
import {
  Children,
  isValidElement,
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  ChevronDown,
  ChevronsLeftRight,
  Ellipsis,
  PanelRight,
  Plus,
  Search,
  X,
  type LucideIcon,
} from "lucide-react";

import { isRemoteClient } from "@/components/remote/is-remote-client";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useTabReorder, type PillReorderHandlers } from "@/lib/tab-reorder";
import { topRightReserve } from "@/lib/titlebar-geometry";
import { useTitlebarActionsWidth } from "@/contrib/react/titlebar-actions";
import { cn } from "@/lib/utils";
import { useHorizontalWheelScroll } from "@/lib/wheel";
import type { RightPanelTab } from "@/stores/ui-store";

import { TabDropIndicator } from "../tab-drop-indicator";
import { middleClickCloseProps } from "../tab-strip-actions";
import { PaneActionButton } from "./pane-actions";
import { isPluginPane, PANE_REGISTRY, type PaneMeta } from "./pane-registry";
import type { SurfaceAction } from "./surface-actions";
import { PanelHeader } from "@/components/ui/panel-header";
import { useRootFontPx } from "@/hooks/use-root-font-px";
import {
  selectShowResourceMonitor,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";

export interface DeckTab {
  id: RightPanelTab;
  label: string;
  /** Who contributed the pane, for panes that are not CodeMux's own (a
   *  plugin's name). Named in the tooltip and the accessible name, so a
   *  plugin pane titled "Changes" never passes for the core pane. */
  attribution?: string;
  icon: LucideIcon;
  /** Small mono badge — a count today ("12", "3/4"). */
  badge?: ReactNode;
  /** Tint the badge with the theme accent while this pane is active. */
  accentBadgeWhenActive?: boolean;
  /** Paint the badge in the attention token regardless of active state —
   *  a count the user is being asked to deal with (a failed subagent) has
   *  to read the same whether or not its pane happens to be in front. */
  badgeTone?: "attention";
  testId?: string;
}

function DeckTabChip({
  tab,
  active,
  dragging,
  reorderProps,
  onSelect,
  onClose,
}: {
  tab: DeckTab;
  active: boolean;
  dragging: boolean;
  reorderProps: PillReorderHandlers;
  onSelect: () => void;
  onClose: () => void;
}) {
  const Icon = tab.icon;
  return (
    <div
      {...reorderProps}
      data-testid={tab.testId}
      data-state={active ? "active" : "inactive"}
      {...middleClickCloseProps(onClose)}
      className={cn(
        // No border, no shadow, no ring — the fill is the whole signal.
        "group/tab relative flex h-[26px] shrink-0 items-center rounded-md pl-[5px]",
        "transition-colors duration-100",
        active
          ? "bg-surface-3 font-semibold text-foreground"
          : "font-medium text-foreground/42 hover:bg-surface-2 hover:text-foreground/70",
        dragging && "opacity-40",
      )}
    >
      <button
        type="button"
        data-no-drag
        aria-label={`Close ${tab.label}`}
        onClick={(event) => {
          event.stopPropagation();
          onClose();
        }}
        className="group/close flex size-[18px] shrink-0 items-center justify-center rounded-sm hover:bg-surface-3"
      >
        <Icon className="size-[13px] group-hover/tab:hidden group-focus-visible/close:hidden" />
        <X className="hidden size-[12px] group-hover/tab:block group-focus-visible/close:block" />
      </button>
      <button
        type="button"
        onClick={onSelect}
        aria-pressed={active}
        aria-label={
          tab.attribution ? `${tab.label} — ${tab.attribution}` : undefined
        }
        // The full name on hover, for labels the cap below truncates.
        title={tab.attribution ? `${tab.label} — ${tab.attribution}` : tab.label}
        className="flex h-full min-w-0 items-center gap-[7px] whitespace-nowrap pl-[5px] pr-[9px] text-body-sm"
      >
        <span className="max-w-[140px] truncate">{tab.label}</span>
        <TabBadge tab={tab} active={active} />
      </button>
    </div>
  );
}

function TabBadge({ tab, active }: { tab: DeckTab; active: boolean }) {
  if (tab.badge == null) return null;
  return (
    <span
      className={cn(
        // A row, not a run of inline text: a badge can be a count plus a
        // status icon (Review), which must sit beside the count, not under it.
        "inline-flex items-center gap-1 font-mono text-caption tabular-nums",
        tab.badgeTone === "attention"
          ? "text-status-attention"
          : active && tab.accentBadgeWhenActive
            ? "text-accent-ember"
            : "text-foreground/38",
      )}
    >
      {tab.badge}
    </span>
  );
}

/**
 * Every open tab as a vertical list, for a row too narrow to show them all.
 * The strip itself still scrolls; this is the way to reach a tab that sits
 * behind the fade without panning for it. Arrow keys move between rows.
 */
function OverflowTabList({
  tabs,
  activeTab,
  onSelect,
}: {
  tabs: DeckTab[];
  activeTab: RightPanelTab | null;
  onSelect: (id: RightPanelTab) => void;
}) {
  return (
    <div
      role="group"
      aria-label="Open tabs"
      data-testid="right-panel-tab-list"
      className="flex min-w-[180px] flex-col gap-px"
    >
      {tabs.map((tab) => {
        const Icon = tab.icon;
        const active = tab.id === activeTab;
        return (
          <button
            key={tab.id}
            type="button"
            data-overflow-tab={tab.id}
            aria-current={active ? "true" : undefined}
            aria-label={
              tab.attribution ? `${tab.label} — ${tab.attribution}` : undefined
            }
            title={tab.attribution ? `${tab.label} — ${tab.attribution}` : tab.label}
            onClick={() => onSelect(tab.id)}
            className={cn(
              "flex h-7 items-center gap-2 rounded-md px-2 text-left text-body-sm outline-hidden",
              "transition-colors duration-100 focus-visible:bg-surface-2",
              active
                ? "bg-surface-3 font-semibold text-foreground"
                : "font-medium text-foreground/70 hover:bg-surface-2 hover:text-foreground",
            )}
          >
            <Icon className="size-[13px] shrink-0" />
            <span className="min-w-0 max-w-[220px] flex-1 truncate">
              {tab.label}
            </span>
            <TabBadge tab={tab} active={active} />
          </button>
        );
      })}
    </div>
  );
}

/** Up/Down/Home/End between the rows of an {@link OverflowTabList} inside
 *  `container`. Keys a focused control already handled (an action's own
 *  menu trigger opens on ArrowDown) are left alone. */
function moveTabListFocus(
  event: React.KeyboardEvent<HTMLElement>,
  container: HTMLElement | null,
) {
  if (event.defaultPrevented || !container) return;
  const rows = [
    ...container.querySelectorAll<HTMLButtonElement>("[data-overflow-tab]"),
  ];
  if (rows.length === 0) return;
  const at = rows.indexOf(document.activeElement as HTMLButtonElement);
  const next =
    event.key === "ArrowDown"
      ? at < 0
        ? 0
        : Math.min(at + 1, rows.length - 1)
      : event.key === "ArrowUp"
        ? at < 0
          ? rows.length - 1
          : Math.max(at - 1, 0)
        : event.key === "Home"
          ? 0
          : event.key === "End"
            ? rows.length - 1
            : null;
  if (next === null) return;
  event.preventDefault();
  rows[next].focus();
}

/** Width of the edge fade that marks clipped tabs. */
const EDGE_FADE_PX = 16;

/** What the tab scroller is promised before the pane actions get any room:
 *  enough for the active tab's label and badge. */
const TAB_FLOOR_PX = 140;
/**
 * What the row spends besides the tab scroller and the pane actions, inline
 * and folded. The "+", its margin, the divider and the row gaps are px; the
 * drag gap's floor (`min-w-4`), the "…" and legacy chrome's expand/close
 * buttons (`size-6`) are rem, so they grow with the interface-size setting.
 */
function rowBudgets(remPx: number, legacy: boolean) {
  const dragGap = remPx;
  // Divider with its margins, two buttons, three row gaps.
  const legacyControls = legacy ? 11 + 3 * remPx + 6 : 0;
  return {
    // The "+" (24px plus its 3px margin), the drag gap, three row gaps.
    inline: 27 + dragGap + 6 + legacyControls,
    // The drag gap, the "…" and two row gaps; the "+" is in the popover.
    folded: dragGap + 1.5 * remPx + 4 + legacyControls,
    // What an inline row adds for the "All tabs" button and its row gap
    // while the tabs overflow. A folded row lists them in its "…" instead.
    tabList: 1.5 * remPx + 2,
  };
}

/**
 * A cheap fingerprint of the actions' shape: element types, labels and text.
 * Actions can change in place without a pane switch (the Diff layout toggle
 * swaps its buttons), and a folded row has nothing inline to measure, so a
 * changed fingerprint is what tells it to measure them again.
 */
function actionsShape(node: ReactNode): string {
  const parts: string[] = [];
  const walk = (child: ReactNode) => {
    Children.forEach(child, (item) => {
      if (typeof item === "string" || typeof item === "number") {
        parts.push(String(item));
        return;
      }
      if (!isValidElement<Record<string, unknown>>(item)) return;
      const { type, props } = item;
      parts.push(
        typeof type === "string"
          ? type
          : typeof type === "function"
            ? type.name
            : "",
      );
      for (const key of ["label", "aria-label", "testId"]) {
        if (typeof props[key] === "string") parts.push(props[key]);
      }
      walk(props.children as ReactNode);
    });
  };
  walk(node);
  return parts.join("|");
}
/** Slack a collapsed row needs before it expands again, so a row sitting
 *  right on the line doesn't flip between the two layouts. */
const COLLAPSE_HYSTERESIS_PX = 4;

/**
 * Whether the active pane's actions (and the "+") have to fold into the "…"
 * popover to leave the tab scroller its floor. Action sets differ per pane
 * (two icons for Files, three for Browser, a stat plus two for Changes), so
 * this measures instead of using a fixed breakpoint. The actions' natural
 * width is read whenever they are inline and kept while they are folded, so
 * a collapsed row can tell when they would fit again. Each time a pane
 * becomes active, or its actions change shape in place, they are shown
 * inline once to be measured afresh, so a stale width never decides; both
 * passes run in layout effects, before paint. A shape change while the
 * popover is open waits for it to close rather than close it under the
 * cursor.
 *
 * They stay inline only if that leaves the tabs at least what folding would
 * give them (capped at `TAB_FLOOR_PX`), with a few px of hysteresis.
 *
 * It also says whether the tabs overflow beside inline actions, in which
 * case the inline row carries an "All tabs" button too and budgets for it.
 * That comes from the measured widths, not from the scroller's current
 * overflow: the button narrows the scroller, so reading the overflow back
 * would let the button decide its own presence.
 */
function useCollapsedActions({
  headerRef,
  actionsRef,
  tabsRef,
  actionsKey,
  actionsShape,
  hasActions,
  budgets,
}: {
  headerRef: React.RefObject<HTMLElement | null>;
  actionsRef: React.RefObject<HTMLDivElement | null>;
  /** The tab run's content, whose width is the tabs' natural width. */
  tabsRef: React.RefObject<HTMLDivElement | null>;
  actionsKey: string;
  actionsShape: string;
  hasActions: boolean;
  budgets: { inline: number; folded: number; tabList: number };
}) {
  const [layout, setLayout] = useState({ collapsed: false, tabsOverflow: false });
  const natural = useRef<{ key: string; shape: string; width: number } | null>(
    null,
  );
  const popoverOpen = useRef(false);

  const measure = useCallback(() => {
    const commit = (collapsed: (prev: boolean) => boolean, tabsOverflow?: boolean) =>
      setLayout((prev) => {
        const next = {
          collapsed: collapsed(prev.collapsed),
          tabsOverflow: tabsOverflow ?? prev.tabsOverflow,
        };
        return next.collapsed === prev.collapsed &&
          next.tabsOverflow === prev.tabsOverflow
          ? prev
          : next;
      });
    const header = headerRef.current;
    // No layout to measure (jsdom): keep everything inline.
    if (!header || header.clientWidth === 0) {
      commit(() => false, false);
      return;
    }
    const style = getComputedStyle(header);
    const row =
      header.clientWidth -
      (parseFloat(style.paddingLeft) || 0) -
      (parseFloat(style.paddingRight) || 0);
    const tabs = tabsRef.current?.scrollWidth ?? 0;
    if (!hasActions) {
      commit(() => false, tabs > row - budgets.inline);
      return;
    }
    const inline = actionsRef.current;
    if (inline) {
      natural.current = {
        key: actionsKey,
        shape: actionsShape,
        width: inline.scrollWidth,
      };
    }
    const known = natural.current;
    if (
      known?.key !== actionsKey ||
      (known.shape !== actionsShape && !popoverOpen.current)
    ) {
      commit(() => false);
      return;
    }
    const width = known.width;
    const tabsOverflow = tabs > row - budgets.inline - width;
    const floor = Math.min(TAB_FLOOR_PX, row - budgets.folded);
    const room =
      row - budgets.inline - (tabsOverflow ? budgets.tabList : 0) - floor;
    commit(
      (prev) => (prev ? width > room - COLLAPSE_HYSTERESIS_PX : width > room),
      tabsOverflow,
    );
  }, [
    actionsKey,
    actionsRef,
    actionsShape,
    budgets,
    hasActions,
    headerRef,
    tabsRef,
  ]);

  // Every commit: a pane switch, or actions that changed width in place.
  // `setLayout` bails out when nothing changed.
  useLayoutEffect(measure);

  const { collapsed } = layout;
  useLayoutEffect(() => {
    const header = headerRef.current;
    if (!header || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(header);
    if (actionsRef.current) observer.observe(actionsRef.current);
    if (tabsRef.current) observer.observe(tabsRef.current);
    return () => observer.disconnect();
  }, [actionsRef, collapsed, headerRef, measure, tabsRef]);

  // An inline row has no popover; a pane switch can unmount an open one.
  useLayoutEffect(() => {
    if (!collapsed) popoverOpen.current = false;
  }, [collapsed]);

  const onPopoverOpenChange = useCallback(
    (open: boolean) => {
      popoverOpen.current = open;
      // Catch up on a shape change made from inside the popover.
      if (!open) measure();
    },
    [measure],
  );

  return { collapsed, tabsOverflow: layout.tabsOverflow, onPopoverOpenChange };
}

/**
 * What didn't fit in the row, behind one button. Folded (`folded` given),
 * it is the "…" holding the "+" and the pane actions — the very same nodes,
 * so each keeps its own behavior, label, tooltip and menu — plus every open
 * tab while some are scrolled out of view. With the actions inline and only
 * the tabs overflowing, it is a plain "All tabs" list.
 */
function RowOverflowMenu({
  folded,
  tabList,
  onOpenChange,
}: {
  folded?: { openPane: ReactNode; actions: ReactNode };
  tabList?: {
    tabs: DeckTab[];
    activeTab: RightPanelTab | null;
    onSelect: (id: RightPanelTab) => void;
  };
  onOpenChange?: (open: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const [tooltipOpen, setTooltipOpen] = useState(false);
  // A pointer open lands focus on the popover itself, not its first action,
  // whose tooltip would otherwise pop up under the cursor. A keyboard open
  // goes straight to the first action, or to the active tab in a bare list.
  const openedByPointer = useRef(false);
  const contentRef = useRef<HTMLDivElement>(null);
  const setOpenState = (next: boolean) => {
    setOpen(next);
    onOpenChange?.(next);
  };
  const label = folded ? "Pane actions" : "All tabs";
  const Icon = folded ? Ellipsis : ChevronDown;
  return (
    <Popover open={open} onOpenChange={setOpenState}>
      {/* Popover outside the tooltip, so the button keeps the popover's
          `data-state`, and no tooltip while the popover is up. */}
      <Tooltip open={tooltipOpen && !open} onOpenChange={setTooltipOpen}>
        <PopoverTrigger asChild>
          <TooltipTrigger asChild>
            <button
              type="button"
              aria-label={label}
              data-testid={
                folded
                  ? "right-panel-pane-actions-more"
                  : "right-panel-tabs-overflow"
              }
              onPointerDown={() => {
                openedByPointer.current = true;
              }}
              onKeyDown={() => {
                openedByPointer.current = false;
              }}
              className={cn(
                "flex size-6 shrink-0 items-center justify-center rounded-md",
                "transition-colors duration-100",
                "text-foreground/42 hover:bg-surface-2 hover:text-foreground/80",
                "data-[state=open]:bg-surface-3 data-[state=open]:text-foreground",
              )}
            >
              <Icon className="size-[13px]" />
            </button>
          </TooltipTrigger>
        </PopoverTrigger>
        <TooltipContent side="bottom">
          {folded ? (tabList ? "More tabs and actions" : "More actions") : label}
        </TooltipContent>
      </Tooltip>
      <PopoverContent
        align="end"
        collisionPadding={8}
        aria-label={label}
        data-testid={
          folded
            ? "right-panel-pane-actions-popover"
            : "right-panel-tabs-overflow-popover"
        }
        ref={contentRef}
        tabIndex={-1}
        onOpenAutoFocus={(event) => {
          if (openedByPointer.current) {
            event.preventDefault();
            contentRef.current?.focus();
          } else if (!folded) {
            event.preventDefault();
            const content = contentRef.current;
            (
              content?.querySelector<HTMLElement>('[aria-current="true"]') ??
              content?.querySelector<HTMLElement>("[data-overflow-tab]")
            )?.focus();
          }
        }}
        onKeyDown={(event) => moveTabListFocus(event, contentRef.current)}
        className="thin-scrollbar flex max-h-(--radix-popover-content-available-height) w-auto max-w-(--radix-popover-content-available-width) flex-col gap-1 overflow-y-auto rounded-lg p-1"
      >
        {folded && (
          <div className="flex items-center gap-[2px] px-0.5">
            {folded.openPane}
            <div
              className="mx-[3px] h-[15px] w-px shrink-0 bg-border/60"
              aria-hidden
            />
            {folded.actions}
          </div>
        )}
        {folded && tabList && (
          <div className="mx-1 h-px bg-border/60" aria-hidden />
        )}
        {tabList && (
          <OverflowTabList
            tabs={tabList.tabs}
            activeTab={tabList.activeTab}
            onSelect={(id) => {
              tabList.onSelect(id);
              setOpenState(false);
            }}
          />
        )}
      </PopoverContent>
    </Popover>
  );
}

/** Tracks which edges of the tab scroller have tabs hidden behind them. */
function useEdgeFade(
  scrollerRef: React.RefObject<HTMLDivElement | null>,
  contentRef: React.RefObject<HTMLDivElement | null>,
) {
  const [edges, setEdges] = useState({ start: false, end: false });

  const measure = useCallback(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const start = el.scrollLeft > 1;
    const end = el.scrollWidth - el.clientWidth - el.scrollLeft > 1;
    setEdges((prev) =>
      prev.start === start && prev.end === end ? prev : { start, end },
    );
  }, [scrollerRef]);

  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    measure();
    el.addEventListener("scroll", measure, { passive: true });
    let observer: ResizeObserver | null = null;
    if (typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(measure);
      observer.observe(el);
      if (contentRef.current) observer.observe(contentRef.current);
    }
    return () => {
      el.removeEventListener("scroll", measure);
      observer?.disconnect();
    };
  }, [contentRef, measure, scrollerRef]);

  return edges;
}

function edgeMask(edges: { start: boolean; end: boolean }): string | undefined {
  if (!edges.start && !edges.end) return undefined;
  const from = edges.start ? `transparent, black ${EDGE_FADE_PX}px` : "black";
  const to = edges.end
    ? `black calc(100% - ${EDGE_FADE_PX}px), transparent`
    : "black";
  return `linear-gradient(to right, ${from}, ${to})`;
}

export interface PaneTabStripProps {
  tabs: DeckTab[];
  activeTab: RightPanelTab | null;
  onSelect: (id: RightPanelTab) => void;
  onClose: (id: RightPanelTab) => void;
  /** Close several tabs at once (the tab context menu). `focus` is the tab
   *  to land on if the active one is among them; `null` for the picker. */
  onCloseMany: (ids: RightPanelTab[], focus: RightPanelTab | null) => void;
  /** Drag-to-reorder landed: the strip's full new order. */
  onReorder: (ids: RightPanelTab[]) => void;
  /**
   * The active pane's own controls, rendered in this row's right-hand slot
   * and swapped in place when the active tab changes. Built by
   * `right-panel.tsx`, which holds the per-pane view state they drive.
   */
  actions?: ReactNode;
  /**
   * Everything the panel can open, in registry order — the *same* array
   * that drives the empty-state picker (`pane-picker.tsx`). One action set,
   * two renderers: a menu when there is a deck to add to, a card grid when
   * there isn't.
   */
  surfaces: SurfaceAction[];
  onOpenFile: () => void;
  /** Resolved binding for the file-search action, shown next to "Open file…". */
  openFileKeys: string;
  /**
   * The row is the window's titlebar band (GUI chrome): 40px, flush with
   * the top edge, right-padded to clear the fixed panel cluster and the
   * native window buttons, and it hands the panel-level controls to that
   * cluster instead of drawing them itself.
   */
  inTitlebar: boolean;
  /** Panel-level controls, rendered here only in legacy chrome. */
  onToggleExpand: () => void;
  expanded: boolean;
  onCollapsePanel: () => void;
  className?: string;
}

export const PaneTabStrip = memo(function PaneTabStrip({
  tabs,
  activeTab,
  onSelect,
  onClose,
  onCloseMany,
  onReorder,
  actions,
  surfaces,
  onOpenFile,
  openFileKeys,
  inTitlebar,
  onToggleExpand,
  expanded,
  onCollapsePanel,
  className,
}: PaneTabStripProps) {
  const remoteClient = isRemoteClient();
  const remPx = useRootFontPx();
  const monitor = useSyncedSettingsStore(selectShowResourceMonitor);
  const pluginWidth = useTitlebarActionsWidth();
  const reserve = inTitlebar
    ? topRightReserve(remoteClient, { panelOpen: true, monitor, remPx }) + pluginWidth
    : 0;
  const coreSurfaces = surfaces.filter((surface) => !isPluginPane(surface.id));
  const pluginSurfaces = surfaces.filter((surface) => isPluginPane(surface.id));

  const tabIds = tabs.map((tab) => tab.id);
  const { containerRef, dragTabId, dropIndicatorLeft, getPillProps } =
    useTabReorder<HTMLDivElement>(tabIds, onReorder as (ids: string[]) => void);

  // A plain vertical wheel pans the strip once it overflows. `overflow-x:
  // auto` only answers to native horizontal input on its own. See
  // `@/lib/wheel`. The scroller is also the reorder hook's measurement
  // container, so both share one ref.
  const attachWheelScroll = useHorizontalWheelScroll<HTMLDivElement>();
  const setScrollerNode = useCallback(
    (node: HTMLDivElement | null) => {
      containerRef.current = node;
      attachWheelScroll(node);
    },
    [attachWheelScroll, containerRef],
  );
  const contentRef = useRef<HTMLDivElement>(null);
  const edges = useEdgeFade(containerRef, contentRef);
  const mask = edgeMask(edges);

  const headerRef = useRef<HTMLElement>(null);
  const actionsRef = useRef<HTMLDivElement>(null);
  const budgets = useMemo(
    () => rowBudgets(remPx, !inTitlebar),
    [inTitlebar, remPx],
  );
  const {
    collapsed: actionsCollapsed,
    tabsOverflow,
    onPopoverOpenChange,
  } = useCollapsedActions({
    headerRef,
    actionsRef,
    tabsRef: contentRef,
    actionsKey: activeTab ?? "",
    actionsShape: actionsShape(actions),
    hasActions: actions != null,
    budgets,
  });
  // An inline row grows an "All tabs" button once the tabs overflow beside
  // it; a folded row lists them in its "…" whenever any are out of view.
  const inlineTabList = !actionsCollapsed && tabsOverflow;
  // The scroller's floor: `TAB_FLOOR_PX`, or whatever the rest of the row
  // leaves once the reserve has taken its share, so the row never runs into
  // the reserve.
  const scrollerMinWidth = `min(${TAB_FLOOR_PX}px, calc(100% - ${
    actionsCollapsed
      ? budgets.folded
      : budgets.inline + (inlineTabList ? budgets.tabList : 0)
  }px))`;

  // The active tab must never be the clipped one: not when it is picked
  // (from the strip or the tab list, where it may already be active but
  // scrolled away), and not when the panel narrows under it.
  const revealTab = useCallback(
    (id: RightPanelTab | null) => {
      if (id == null) return;
      containerRef.current
        ?.querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(id)}"]`)
        ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    },
    [containerRef],
  );
  useEffect(() => revealTab(activeTab), [activeTab, revealTab]);
  useLayoutEffect(() => {
    const scroller = containerRef.current;
    if (!scroller || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => revealTab(activeTab));
    observer.observe(scroller);
    return () => observer.disconnect();
  }, [activeTab, containerRef, revealTab]);
  const tabList = {
    tabs,
    activeTab,
    onSelect: (id: RightPanelTab) => {
      onSelect(id);
      revealTab(id);
    },
  };

  // The "+": inline after the tabs, or first in the "…" popover once the
  // row has folded.
  const openPaneMenu = (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Open pane"
          data-testid="right-panel-add-pane"
          className={cn(
            "flex size-[24px] shrink-0 items-center justify-center rounded-md text-foreground/42 transition-colors duration-100 hover:bg-surface-2 hover:text-foreground data-[state=open]:bg-surface-3 data-[state=open]:text-foreground",
            !actionsCollapsed && "ml-[3px]",
          )}
        >
          <Plus className="size-[13px]" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="w-[206px] rounded-lg p-[5px] [&_[role=menuitem]]:whitespace-nowrap"
      >
        <DropdownMenuLabel className="px-[9px] pb-[5px] pt-1.5 font-mono text-micro tracking-[0.13em] text-muted-foreground">
          OPEN PANE
        </DropdownMenuLabel>
        {/* Same `surfaces` array the empty-panel picker renders as cards,
            including Terminal, which is a *workspace* pane and routes to
            the action the main tab strip's "+" uses. */}
        {coreSurfaces.map((surface) => (
          <DropdownMenuItem
            key={surface.id}
            className="h-[30px] rounded-md px-[9px] text-body font-medium"
            onClick={surface.onOpen}
          >
            <surface.icon className="size-[14px]" />
            {surface.label}
          </DropdownMenuItem>
        ))}
        {/* Plugin panes get their own section, each attributed to its plugin,
            so none can pass for a core pane. Absent without any. */}
        {pluginSurfaces.length > 0 && (
          <>
            <DropdownMenuSeparator className="mx-1 my-[5px]" />
            <DropdownMenuLabel className="px-[9px] pb-[5px] pt-1.5 font-mono text-micro tracking-[0.13em] text-muted-foreground">
              PLUGINS
            </DropdownMenuLabel>
            {pluginSurfaces.map((surface) => (
              <DropdownMenuItem
                key={surface.id}
                aria-label={`${surface.label} — ${surface.description}`}
                className="h-[30px] rounded-md px-[9px] text-body font-medium"
                onClick={surface.onOpen}
              >
                <surface.icon className="size-[14px]" />
                <span className="min-w-0 truncate">{surface.label}</span>
                <span className="ml-auto min-w-0 truncate pl-2 text-label font-normal text-muted-foreground">
                  {surface.description}
                </span>
              </DropdownMenuItem>
            ))}
          </>
        )}
        <DropdownMenuSeparator className="mx-1 my-[5px]" />
        <DropdownMenuItem
          className="h-[30px] rounded-md px-[9px] text-body font-medium"
          onClick={onOpenFile}
        >
          <Search className="size-[14px]" />
          Open file…
          {openFileKeys && (
            <DropdownMenuShortcut className="font-mono text-caption">
              {openFileKeys}
            </DropdownMenuShortcut>
          )}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );

  const tabRun = (
    <>
      {/* `no-scrollbar` is a real utility (shadcn/tailwind.css, imported by
          globals.css) — an earlier comment here claimed it was undefined,
          which is what spawned three hand-rolled copies of it.
          `scroll-px-4` keeps a tab scrolled into view clear of the fade.
          The min width keeps the active tab readable once the top-right
          reserve leaves the row ~160px; past it, the pane actions fold into
          "…" rather than squeeze the tabs. */}
      <div
        ref={setScrollerNode}
        data-testid="right-panel-tabs-scroll"
        className="no-scrollbar relative flex min-w-0 scroll-px-4 items-center overflow-x-auto"
        style={{
          minWidth: scrollerMinWidth,
          ...(mask ? { maskImage: mask, WebkitMaskImage: mask } : null),
        }}
      >
        <div
          ref={contentRef}
          data-testid="right-panel-tabs-content"
          className="flex w-max shrink-0 items-center gap-[2px]"
        >
          {tabs.map((tab, index) => (
            <ContextMenu key={tab.id}>
              {/* A wrapper, so the chip's own pointer handlers (reorder)
                  don't have to be merged with the menu trigger's. */}
              <ContextMenuTrigger asChild>
                <div className="flex shrink-0">
                  <DeckTabChip
                    tab={tab}
                    active={tab.id === activeTab}
                    dragging={dragTabId === tab.id}
                    reorderProps={getPillProps(tab.id)}
                    onSelect={() => onSelect(tab.id)}
                    onClose={() => onClose(tab.id)}
                  />
                </div>
              </ContextMenuTrigger>
              <ContextMenuContent>
                <ContextMenuItem onClick={() => onClose(tab.id)}>
                  Close tab
                </ContextMenuItem>
                <ContextMenuItem
                  disabled={tabs.length <= 1}
                  onClick={() =>
                    onCloseMany(
                      tabIds.filter((id) => id !== tab.id),
                      tab.id,
                    )
                  }
                >
                  Close other tabs
                </ContextMenuItem>
                <ContextMenuItem
                  disabled={index >= tabs.length - 1}
                  onClick={() => onCloseMany(tabIds.slice(index + 1), tab.id)}
                >
                  Close tabs to the right
                </ContextMenuItem>
                <ContextMenuSeparator />
                <ContextMenuItem onClick={() => onCloseMany(tabIds, null)}>
                  Close all tabs
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          ))}
        </div>
        {dragTabId && dropIndicatorLeft !== null && (
          <TabDropIndicator left={dropIndicatorLeft} />
        )}
      </div>

      {inlineTabList && <RowOverflowMenu tabList={tabList} />}
      {!actionsCollapsed && openPaneMenu}
    </>
  );

  // The gap between the tabs and the pane actions. While it sits in the
  // window band it is also the panel's drag surface: the titlebar's own
  // drag layer stops at the panel's left edge so it can't swallow these
  // controls. Desktop only: `data-tauri-drag-region` does nothing in a
  // browser, and the bare spacer has no children to shadow.
  const dragGap = (
    <div
      data-testid="right-panel-drag-gap"
      data-tauri-drag-region={inTitlebar && !remoteClient ? true : undefined}
      className="min-w-4 flex-1 self-stretch"
    />
  );

  // The active pane's controls. The panel-level controls (expand, close)
  // live in the fixed top-right cluster in GUI chrome, which the band's
  // right padding clears. Never clipped: too narrow to sit inline beside
  // the tabs' floor, they fold into "…" together with the "+" (and the
  // tab list, while tabs are out of view).
  const paneActions =
    actions == null ? null : actionsCollapsed ? (
      <RowOverflowMenu
        folded={{ openPane: openPaneMenu, actions }}
        tabList={edges.start || edges.end ? tabList : undefined}
        onOpenChange={onPopoverOpenChange}
      />
    ) : (
      <div
        ref={actionsRef}
        data-testid="right-panel-pane-actions"
        className="flex shrink-0 items-center gap-[2px]"
      >
        {actions}
      </div>
    );

  return (
    <PanelHeader
      ref={headerRef}
      variant={inTitlebar ? "floating" : "inline"}
      data-testid="right-panel-tabs-header"
      data-in-titlebar={inTitlebar ? "true" : undefined}
      className={cn(
        // One hairline under this row and nothing else between it and the
        // pane body. The body starts flush. The floating variant carries no
        // rule of its own, so the band row asks for one explicitly: its seam
        // is the bottom edge of the window band.
        // A named container so pane actions can drop secondary text (the
        // Changes diff stat) when the row is narrow.
        "@container/pane-band gap-[2px] border-b border-hairline px-[7px]",
        // 40px when this row *is* the window band, so its seam lands exactly
        // on the band's bottom edge and its controls sit on the same
        // baseline as the sidebar toggle and the window buttons.
        //
        // Transparent, not `bg-card`: the titlebar is frameless, so a filled
        // row here would draw a lighter slab across the panel's half of the
        // band. Under the legacy in-flow bar the row is ordinary panel
        // chrome below a real titlebar surface, so it keeps its card fill.
        inTitlebar ? "bg-transparent" : "bg-card",
        className,
      )}
      style={inTitlebar ? { paddingRight: `${reserve}px` } : undefined}
    >
      {tabRun}
      {dragGap}
      {paneActions}
      {!inTitlebar && (
        <>
          <div
            className="mx-[5px] h-[15px] w-px shrink-0 bg-border/60"
            aria-hidden
          />
          <PaneActionButton
            label={expanded ? "Restore panel width" : "Expand panel"}
            icon={ChevronsLeftRight}
            onClick={onToggleExpand}
            active={expanded}
          />
          <PaneActionButton
            label="Close panel"
            icon={PanelRight}
            onClick={onCollapsePanel}
            active
          />
        </>
      )}
    </PanelHeader>
  );
});

/** Registry order, for the `+` menu's pane list. */
export function orderPanes(ids: readonly string[]): PaneMeta[] {
  return PANE_REGISTRY.filter((meta) => ids.includes(meta.id));
}
