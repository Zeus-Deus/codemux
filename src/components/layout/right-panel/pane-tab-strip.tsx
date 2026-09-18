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
 * while its pane is active) — nothing in the strip blinks; app-wide
 * "working" is an orb.
 *
 * **Overflow.** The deck grows: every file opened from the tree is another
 * tab, and a busy thread adds tasks, orchestration and subagents on top of
 * the three defaults. Tabs never shrink. Each one keeps its icon and a label
 * that is capped and truncated, and the row scrolls sideways (a plain
 * vertical wheel pans it) with a soft fade on whichever edge is clipped.
 * Tabs reorder by drag, same gesture as the titlebar tabs.
 *
 * **Narrow panels.** In the titlebar band the row shares its width with the
 * fixed top-right cluster and the native window buttons, which together take
 * ~174px. At the panel's default width that left too little room for even
 * one labelled tab. So when the tabs can't fit beside the band's fixed
 * content, the row stacks: the band keeps the drag surface and the active
 * pane's actions, and the tabs get their own full-width row directly below it.
  */
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  ChevronsLeftRight,
  PanelRight,
  Plus,
  Search,
  X,
  type LucideIcon,
} from "lucide-react";

import { isRemoteClient } from "@/components/remote/is-remote-client";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useTabReorder, type PillReorderHandlers } from "@/lib/tab-reorder";
import { topRightReserve } from "@/lib/titlebar-geometry";
import { cn } from "@/lib/utils";
import { useHorizontalWheelScroll } from "@/lib/wheel";
import type { RightPanelTab } from "@/stores/ui-store";

import { TabDropIndicator } from "../tab-drop-indicator";
import { PaneActionButton } from "./pane-actions";
import { PANE_REGISTRY, type PaneMeta } from "./pane-registry";
import type { SurfaceAction } from "./surface-actions";

export interface DeckTab {
  id: RightPanelTab;
  label: string;
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
  onBackground,
  reorderProps,
  onSelect,
  onClose,
}: {
  tab: DeckTab;
  active: boolean;
  dragging: boolean;
  /** The row is painted on the panel's `bg-background` rather than
   *  `bg-card`, so the close affordance's mask can match it. See its
   *  `bg-*` below. */
  onBackground: boolean;
  reorderProps: PillReorderHandlers;
  onSelect: () => void;
  onClose: () => void;
}) {
  const Icon = tab.icon;
  const ref = useRef<HTMLDivElement>(null);

  // Once the tabs and the pane actions share one row, a deck of four or
  // five panes overflows at the panel's default width — and the tab you
  // just selected is exactly the one that must not be the clipped one.
  useEffect(() => {
    if (!active) return;
    ref.current?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [active]);

  const badge = tab.badge != null && (
    <span
      className={cn(
        "font-mono text-caption tabular-nums",
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

  return (
    <div
      ref={ref}
      {...reorderProps}
      data-testid={tab.testId}
      data-state={active ? "active" : "inactive"}
      // Middle-click closes, as in every browser and editor tab strip. The
      // mousedown is cancelled too: on an overflowing strip a middle
      // button-press would otherwise also engage the webview's autoscroll
      // on its way to the close.
      onMouseDown={(event) => {
        if (event.button === 1) event.preventDefault();
      }}
      onAuxClick={(event) => {
        if (event.button !== 1) return;
        event.preventDefault();
        onClose();
      }}
      className={cn(
        // No border, no shadow, no ring — the fill is the whole signal.
        "group/tab relative flex h-[26px] shrink-0 items-center rounded-md",
        "transition-colors duration-[120ms]",
        active
          ? "bg-foreground/7 font-semibold text-foreground"
          : "font-medium text-foreground/42 hover:bg-foreground/5 hover:text-foreground/70",
        dragging && "opacity-40",
      )}
    >
      <button
        type="button"
        onClick={onSelect}
        aria-pressed={active}
        // The full name on hover, for labels the cap below truncates.
        title={tab.label}
        className={cn(
          "flex h-full min-w-0 items-center gap-[7px] whitespace-nowrap pl-[9px] text-body-sm",
          // Room for the close affordance only where it is always shown.
          // On inactive tabs it appears on hover, over the label's tail.
          active ? "pr-[20px]" : "pr-[9px]",
        )}
      >
        <Icon className="size-[13px] shrink-0" strokeWidth={1.6} />
        <span className="max-w-[140px] truncate">{tab.label}</span>
        {badge}
      </button>
      <button
          type="button"
          data-no-drag
          aria-label={`Close ${tab.label}`}
          onClick={(event) => {
            event.stopPropagation();
            onClose();
          }}
          className={cn(
            // Overlays the tab's right edge rather than taking a layout
            // slot, so revealing it on hover doesn't shove the row around.
            "absolute right-[3px] top-1/2 flex size-[15px] -translate-y-1/2 items-center justify-center rounded-sm transition-opacity",
            "hover:bg-foreground/15 focus-visible:opacity-100",
            active
              ? "opacity-50 hover:opacity-100"
              : // Opaque on purpose: it masks the label it sits on top of,
                // so it has to match whatever the row is painted on.
                cn(
                  "opacity-0 group-hover/tab:opacity-70",
                  onBackground ? "bg-background" : "bg-card",
                ),
          )}
        >
          <X className="size-[10px]" strokeWidth={1.8} />
        </button>
    </div>
  );
}

/** Width of the edge fade that marks clipped tabs. */
const EDGE_FADE_PX = 16;
/** The drag gap's `min-w-4`: the part of it that can never be lent back to
 *  the tabs. */
const DRAG_GAP_MIN_PX = 16;
/** The row's `px-[7px]`. */
const ROW_PADDING_PX = 7;
/** The row's `gap-[2px]`. */
const ROW_GAP_PX = 2;
/** The `+` button: `size-[24px]` plus its `ml-[3px]`. */
const ADD_BUTTON_PX = 27;

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

/**
 * Whether the titlebar row has to stack its tabs below the band.
 *
 * Every input is independent of the layout it chooses: the tabs never
 * shrink, so their natural width is the same in either layout, and so are
 * the header's width and the actions' width. The answer can't flip-flop
 * as a result of applying it.
 */
function useStackedTabs(
  enabled: boolean,
  reserve: number,
  headerRef: React.RefObject<HTMLDivElement | null>,
  contentRef: React.RefObject<HTMLDivElement | null>,
  actionsRef: React.RefObject<HTMLDivElement | null>,
  layoutKey: string,
): boolean {
  const [stacked, setStacked] = useState(false);

  const measure = useCallback(() => {
    const header = headerRef.current;
    const content = contentRef.current;
    // A header with no width hasn't been laid out (a hidden panel), so
    // there is nothing to decide yet.
    if (!enabled || !header || !content || header.clientWidth === 0) {
      setStacked(false);
      return;
    }
    const actions = actionsRef.current?.offsetWidth ?? 0;
    const fixed =
      ROW_PADDING_PX +
      reserve +
      ADD_BUTTON_PX +
      DRAG_GAP_MIN_PX +
      actions +
      ROW_GAP_PX * (actions > 0 ? 3 : 2);
    const room = header.clientWidth - fixed;
    setStacked(content.scrollWidth > room);
  }, [actionsRef, contentRef, enabled, headerRef, reserve]);

  useLayoutEffect(() => {
    measure();
  }, [measure, layoutKey]);

  useEffect(() => {
    if (!enabled || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    for (const node of [
      headerRef.current,
      contentRef.current,
      actionsRef.current,
    ]) {
      if (node) observer.observe(node);
    }
    return () => observer.disconnect();
    // The actions node remounts when the layout flips; re-observe it.
  }, [actionsRef, contentRef, enabled, headerRef, measure, stacked, layoutKey]);

  return stacked;
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
  const reserve = inTitlebar ? topRightReserve(remoteClient, true) : 0;

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
  const headerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const actionsRef = useRef<HTMLDivElement>(null);
  const layoutKey = `${tabIds.join("|")}#${activeTab ?? ""}`;
  const stacked = useStackedTabs(
    inTitlebar,
    reserve,
    headerRef,
    contentRef,
    actionsRef,
    layoutKey,
  );
  const edges = useEdgeFade(containerRef, contentRef);
  const mask = edgeMask(edges);

  // Switching layouts remounts nothing in the scroller, but its width
  // changes, so the active tab may have been pushed out of view.
  useEffect(() => {
    containerRef.current
      ?.querySelector<HTMLElement>('[data-tab-id][data-state="active"]')
      ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [stacked, containerRef]);

  const tabRun = (
    <>
      {/* `no-scrollbar` is referenced elsewhere in the tree but never
          defined, so the hiding is spelled out here rather than trusted.
          `scroll-px-4` keeps a tab scrolled into view clear of the fade. */}
      <div
        ref={setScrollerNode}
        data-testid="right-panel-tabs-scroll"
        className="relative flex min-w-0 scroll-px-4 items-center overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        style={mask ? { maskImage: mask, WebkitMaskImage: mask } : undefined}
      >
        <div
          ref={contentRef}
          data-testid="right-panel-tabs-content"
          className="flex w-max shrink-0 items-center gap-[2px]"
        >
          {tabs.map((tab) => (
            <DeckTabChip
              key={tab.id}
              tab={tab}
              active={tab.id === activeTab}
              dragging={dragTabId === tab.id}
              onBackground={inTitlebar}
              reorderProps={getPillProps(tab.id)}
              onSelect={() => onSelect(tab.id)}
              onClose={() => onClose(tab.id)}
            />
          ))}
        </div>
        {dragTabId && dropIndicatorLeft !== null && (
          <TabDropIndicator left={dropIndicatorLeft} />
        )}
      </div>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label="Open pane"
            data-testid="right-panel-add-pane"
            className="ml-[3px] flex size-[24px] shrink-0 items-center justify-center rounded-md text-foreground/42 transition-colors duration-[120ms] hover:bg-foreground/8 hover:text-foreground data-[state=open]:bg-foreground/12 data-[state=open]:text-foreground"
          >
            <Plus className="size-[13px]" strokeWidth={1.7} />
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
          {surfaces.map((surface) => (
            <DropdownMenuItem
              key={surface.id}
              className="h-[30px] rounded-md px-[9px] text-body font-medium"
              onClick={surface.onOpen}
            >
              <surface.icon className="size-[14px]" strokeWidth={1.5} />
              {surface.label}
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator className="mx-1 my-[5px]" />
          <DropdownMenuItem
            className="h-[30px] rounded-md px-[9px] text-body font-medium"
            onClick={onOpenFile}
          >
            <Search className="size-[14px]" strokeWidth={1.5} />
            Open file…
            {openFileKeys && (
              <DropdownMenuShortcut className="font-mono text-caption">
                {openFileKeys}
              </DropdownMenuShortcut>
            )}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );

  // The gap between the tabs and the pane actions. While it sits in the
  // window band it is also the panel's drag surface: the titlebar's own
  // drag layer stops at the panel's left edge so it can't swallow these
  // controls. Desktop only: `data-tauri-drag-region` does nothing in a
  // browser, and the bare spacer has no children to shadow.
  const dragGap = (inBand: boolean) => (
    <div
      data-testid="right-panel-drag-gap"
      data-tauri-drag-region={inBand && !remoteClient ? true : undefined}
      className="min-w-4 flex-1 self-stretch"
    />
  );

  // The active pane's controls. The panel-level controls (expand, close)
  // live in the fixed top-right cluster in GUI chrome, which the band's
  // right padding clears.
  const paneActions = actions != null && (
    <div
      ref={actionsRef}
      data-testid="right-panel-pane-actions"
      className="flex shrink-0 items-center gap-[2px]"
    >
      {actions}
    </div>
  );

  const bandStyle = inTitlebar ? { paddingRight: `${reserve}px` } : undefined;

  if (stacked) {
    return (
      <div
        ref={headerRef}
        data-testid="right-panel-tabs-header"
        data-in-titlebar="true"
        data-stacked="true"
        className={cn("flex shrink-0 flex-col", className)}
      >
        <div
          className="flex h-10 items-center gap-[2px] px-[7px]"
          style={bandStyle}
        >
          {dragGap(true)}
          {paneActions}
        </div>
        <div className="flex h-9 items-center gap-[2px] border-b border-border/60 px-[7px]">
          {tabRun}
          {dragGap(false)}
        </div>
      </div>
    );
  }

  return (
    <div
      ref={headerRef}
      data-testid="right-panel-tabs-header"
      data-in-titlebar={inTitlebar ? "true" : undefined}
      className={cn(
        // One hairline under this row and nothing else between it and the
        // pane body. The body starts flush.
        "flex shrink-0 items-center gap-[2px] border-b border-border/60 px-[7px]",
        // 40px when this row *is* the window band, so its seam lands exactly
        // on the band's bottom edge and its controls sit on the same
        // baseline as the sidebar toggle and the window buttons.
        //
        // Transparent, not `bg-card`: the titlebar is frameless, so a filled
        // row here would draw a lighter slab across the panel's half of the
        // band. Under the legacy in-flow bar the row is ordinary panel
        // chrome below a real titlebar surface, so it keeps its card fill.
        inTitlebar ? "h-10 bg-transparent" : "h-9 bg-card",
        className,
      )}
      style={bandStyle}
    >
      {tabRun}
      {dragGap(inTitlebar)}
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
    </div>
  );
});

/** Registry order, for the `+` menu's pane list. */
export function orderPanes(ids: readonly string[]): PaneMeta[] {
  return PANE_REGISTRY.filter((meta) => ids.includes(meta.id));
}
