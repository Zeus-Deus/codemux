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
 * the three defaults. Rather than ellipsize every inactive label down to
 * an identical unreadable stub the moment the row gets tight, the strip
 * switches modes: once the full-label row would overflow, inactive tabs
 * collapse to icon + badge (the pinned-tab convention — the label lives in
 * the tooltip) and the active tab alone keeps its label. Whatever still
 * doesn't fit scrolls, with a soft fade on the clipped edge so the
 * overflow is visible without a scrollbar, and a plain vertical wheel pans
 * it. Tabs reorder by drag, same gesture as the titlebar tabs.
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
  testId?: string;
}

function DeckTabChip({
  tab,
  active,
  compact,
  dragging,
  inTitlebar,
  reorderProps,
  onSelect,
  onClose,
}: {
  tab: DeckTab;
  active: boolean;
  /** Icon-only: the strip has overflowed and this (inactive) tab gave up
   *  its label. See the file header. */
  compact: boolean;
  dragging: boolean;
  /** Which surface the row is painted on, so the close affordance's mask
   *  can match it — see its `bg-*` below. */
  inTitlebar: boolean;
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
        "font-mono text-[9.5px] tabular-nums",
        active && tab.accentBadgeWhenActive
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
      data-compact={compact ? "true" : undefined}
      // Middle-click closes, as in every browser and editor tab strip —
      // and it is the only close gesture a compact tab has, since an X
      // overlaid on a 28px chip would be the thing you hit when aiming for
      // the tab.
      onAuxClick={(event) => {
        if (event.button !== 1) return;
        event.preventDefault();
        onClose();
      }}
      className={cn(
        // No border, no shadow, no ring — the fill is the whole signal.
        "group/tab relative flex h-[26px] shrink-0 items-center rounded-[7px]",
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
        aria-label={compact ? tab.label : undefined}
        // Compact tabs spell their name out on hover rather than earning a
        // second row to do it in.
        title={tab.label}
        className={cn(
          "flex h-full min-w-0 items-center whitespace-nowrap text-[12px]",
          compact
            ? "gap-[5px] px-[8px]"
            : cn(
                "gap-[7px] pl-[9px]",
                // Room for the close affordance only where it is actually
                // shown; an always-reserved slot per tab costs a whole
                // label at this width.
                active ? "pr-[20px]" : "pr-[9px]",
              ),
        )}
      >
        <Icon className="size-[13px] shrink-0" strokeWidth={1.6} />
        {!compact && <span className="truncate">{tab.label}</span>}
        {badge}
      </button>
      {!compact && (
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
            "absolute right-[3px] top-1/2 flex size-[15px] -translate-y-1/2 items-center justify-center rounded-[4px] transition-opacity",
            "hover:bg-foreground/15 focus-visible:opacity-100",
            active
              ? "opacity-50 hover:opacity-100"
              : // Opaque on purpose: it masks the label it sits on top of,
                // so it has to match whatever the row is painted on.
                cn(
                  "opacity-0 group-hover/tab:opacity-70",
                  inTitlebar ? "bg-background" : "bg-card",
                ),
          )}
        >
          <X className="size-[10px]" strokeWidth={1.8} />
        </button>
      )}
    </div>
  );
}

/** Width of the edge fade that marks clipped tabs. */
const EDGE_FADE_PX = 16;
/** The drag gap's `min-w-4`: the part of it that can never be lent back to
 *  the tabs. */
const DRAG_GAP_MIN_PX = 16;

/**
 * Decides between the full-label and the icon-only strip, and tracks which
 * edges have tabs hidden behind them.
 *
 * The two layouts have different widths, so "does it overflow?" cannot be
 * asked of the compact strip about the full one: the answer would flip
 * every frame. Instead the full strip's width is measured once whenever it
 * is laid out (`naturalWidth`), and the compact strip only lets go once
 * the room available — its own width plus whatever the drag gap next to it
 * could give back — covers that. Every change to the tab set (or to which
 * tab is active, since only that one keeps its label) re-runs the full
 * layout under `useLayoutEffect`, so the expanded frame is measured but
 * never painted.
 */
function useDeckOverflow(
  scrollerRef: React.RefObject<HTMLDivElement | null>,
  gapRef: React.RefObject<HTMLDivElement | null>,
  layoutKey: string,
) {
  const [compact, setCompact] = useState(false);
  const [edges, setEdges] = useState({ start: false, end: false });
  const naturalWidthRef = useRef(0);

  const measureEdges = useCallback(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const start = el.scrollLeft > 1;
    const end = el.scrollWidth - el.clientWidth - el.scrollLeft > 1;
    setEdges((prev) =>
      prev.start === start && prev.end === end ? prev : { start, end },
    );
  }, [scrollerRef]);

  // A new tab set is measured from the full layout.
  useLayoutEffect(() => {
    setCompact(false);
  }, [layoutKey]);

  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    if (!compact) {
      naturalWidthRef.current = el.scrollWidth;
      if (el.scrollWidth > el.clientWidth + 1) {
        setCompact(true);
        return;
      }
    }
    measureEdges();
  }, [compact, layoutKey, measureEdges, scrollerRef]);

  useEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const onResize = () => {
      const gap = gapRef.current;
      const available =
        el.clientWidth + Math.max(0, (gap?.clientWidth ?? 0) - DRAG_GAP_MIN_PX);
      if (compact && available >= naturalWidthRef.current) setCompact(false);
      else if (!compact && el.scrollWidth > el.clientWidth + 1)
        setCompact(true);
      measureEdges();
    };
    el.addEventListener("scroll", measureEdges, { passive: true });
    let observer: ResizeObserver | null = null;
    if (typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(onResize);
      observer.observe(el);
      if (gapRef.current) observer.observe(gapRef.current);
    }
    return () => {
      el.removeEventListener("scroll", measureEdges);
      observer?.disconnect();
    };
  }, [compact, gapRef, measureEdges, scrollerRef]);

  return { compact, edges };
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

  const tabIds = tabs.map((tab) => tab.id);
  const { containerRef, dragTabId, dropIndicatorLeft, getPillProps } =
    useTabReorder<HTMLDivElement>(tabIds, onReorder as (ids: string[]) => void);

  // A plain vertical wheel pans the strip once it overflows — `overflow-x:
  // auto` only answers to native horizontal input on its own. See
  // `@/lib/wheel`. The scroller is also the reorder hook's measurement
  // container and the overflow hook's, so all three share one ref.
  const attachWheelScroll = useHorizontalWheelScroll<HTMLDivElement>();
  const setScrollerNode = useCallback(
    (node: HTMLDivElement | null) => {
      containerRef.current = node;
      attachWheelScroll(node);
    },
    [attachWheelScroll, containerRef],
  );
  const gapRef = useRef<HTMLDivElement>(null);
  const { compact, edges } = useDeckOverflow(
    containerRef,
    gapRef,
    `${tabIds.join("|")}#${activeTab ?? ""}`,
  );
  const mask = edgeMask(edges);

  // The active chip scrolls itself into view, but that runs against the
  // full-label layout the strip measures and discards (passive effects
  // flush before the collapse re-render), which leaves the compact strip
  // sitting on a stale offset with its first icon under the fade. Re-seat
  // from zero once the layout has settled.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    el.scrollLeft = 0;
    el.querySelector<HTMLElement>(
      '[data-tab-id][data-state="active"]',
    )?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [compact, containerRef]);

  return (
    <div
      data-testid="right-panel-tabs-header"
      data-in-titlebar={inTitlebar ? "true" : undefined}
      className={cn(
        // One hairline under this row and nothing else between it and the
        // pane body — the body starts flush.
        "flex shrink-0 items-center gap-[2px] border-b border-border/60 px-[7px]",
        // 40px when this row *is* the window band, so its seam lands exactly
        // on the band's bottom edge and its controls sit on the same
        // baseline as the sidebar toggle and the window buttons.
        //
        // And transparent, not `bg-card`. The titlebar is frameless — the
        // sidebar, the workspace and the panel all reach the physical top
        // edge and no cluster paints a surface of its own — so a filled row
        // here drew a lighter grey slab across the panel's half of the band
        // with a visible seam where the workspace's half ended. Letting the
        // panel's own `bg-background` show through makes the band read as
        // one continuous window edge. Under the legacy in-flow bar the row
        // is ordinary panel chrome sitting below a real titlebar surface, so
        // it keeps its card fill.
        inTitlebar ? "h-10 bg-transparent" : "h-9 bg-card",
        className,
      )}
      style={
        inTitlebar
          ? // Clear the fixed panel cluster and, on desktop, the native
            // window buttons sitting above this row's right end.
            { paddingRight: `${topRightReserve(remoteClient, true)}px` }
          : undefined
      }
    >
      {/* Tabs are the only part of the row allowed to compress or scroll:
          the `+`, the pane actions and the panel controls stay reachable at
          any panel width. */}
      {/* `no-scrollbar` is referenced elsewhere in the tree but never
          defined, so the hiding is spelled out here rather than trusted. */}
      <div
        ref={setScrollerNode}
        data-testid="right-panel-tabs-scroll"
        data-compact={compact ? "true" : undefined}
        className="relative flex min-w-0 items-center gap-[2px] overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        style={mask ? { maskImage: mask, WebkitMaskImage: mask } : undefined}
      >
        {dragTabId && dropIndicatorLeft !== null && (
          <TabDropIndicator left={dropIndicatorLeft} />
        )}
        {tabs.map((tab) => (
          <DeckTabChip
            key={tab.id}
            tab={tab}
            active={tab.id === activeTab}
            compact={compact && tab.id !== activeTab}
            dragging={dragTabId === tab.id}
            inTitlebar={inTitlebar}
            reorderProps={getPillProps(tab.id)}
            onSelect={() => onSelect(tab.id)}
            onClose={() => onClose(tab.id)}
          />
        ))}
      </div>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label="Open pane"
            data-testid="right-panel-add-pane"
            className="ml-[3px] flex size-[24px] shrink-0 items-center justify-center rounded-[7px] text-foreground/42 transition-colors duration-[120ms] hover:bg-foreground/8 hover:text-foreground data-[state=open]:bg-foreground/12 data-[state=open]:text-foreground"
          >
            <Plus className="size-[13px]" strokeWidth={1.7} />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="start"
          className="w-[206px] rounded-[11px] p-[5px] [&_[role=menuitem]]:whitespace-nowrap"
        >
          <DropdownMenuLabel className="px-[9px] pb-[5px] pt-1.5 font-mono text-[9px] tracking-[0.13em] text-muted-foreground">
            OPEN PANE
          </DropdownMenuLabel>
          {/* Same `surfaces` array the empty-panel picker renders as cards —
              including Terminal, which is a *workspace* pane and routes to
              the action the main tab strip's "+" uses. */}
          {surfaces.map((surface) => (
            <DropdownMenuItem
              key={surface.id}
              className="h-[30px] rounded-[7px] px-[9px] text-[12.5px] font-medium"
              onClick={surface.onOpen}
            >
              <surface.icon className="size-[14px]" strokeWidth={1.5} />
              {surface.label}
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator className="mx-1 my-[5px]" />
          <DropdownMenuItem
            className="h-[30px] rounded-[7px] px-[9px] text-[12.5px] font-medium"
            onClick={onOpenFile}
          >
            <Search className="size-[14px]" strokeWidth={1.5} />
            Open file…
            {openFileKeys && (
              <DropdownMenuShortcut className="font-mono text-[9.5px]">
                {openFileKeys}
              </DropdownMenuShortcut>
            )}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      {/* The gap between the tabs and the pane actions. While this row is
          the window band it is also the panel's drag surface — the
          titlebar's own full-width drag layer stops at the panel's left
          edge so it can't swallow these controls, so without this the
          panel's whole top edge would stop dragging the window. Desktop
          only: `data-tauri-drag-region` does nothing in a browser, and the
          bare spacer has no children to shadow. */}
      <div
        ref={gapRef}
        data-testid="right-panel-drag-gap"
        data-tauri-drag-region={inTitlebar && !remoteClient ? true : undefined}
        className="min-w-4 flex-1 self-stretch"
      />

      {/* Right slot: the active pane's controls. The panel-level controls
          (expand, close) used to follow them behind a divider; in GUI
          chrome they live in the fixed top-right cluster this row's padding
          reserves, so they no longer move with the panel's edge. */}
      {actions != null && (
        <div
          data-testid="right-panel-pane-actions"
          className="flex shrink-0 items-center gap-[2px]"
        >
          {actions}
        </div>
      )}
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
