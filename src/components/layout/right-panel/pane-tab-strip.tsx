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
import { useTabReorder, type PillReorderHandlers } from "@/lib/tab-reorder";
import { topRightReserve } from "@/lib/titlebar-geometry";
import { cn } from "@/lib/utils";
import { useHorizontalWheelScroll } from "@/lib/wheel";
import type { RightPanelTab } from "@/stores/ui-store";

import { TabDropIndicator } from "../tab-drop-indicator";
import { PaneActionButton } from "./pane-actions";
import { isAddonPane, PANE_REGISTRY, type PaneMeta } from "./pane-registry";
import type { SurfaceAction } from "./surface-actions";
import { PanelHeader } from "@/components/ui/panel-header";

export interface DeckTab {
  id: RightPanelTab;
  label: string;
  /** Who contributed the pane, for panes that are not CodeMux's own (an
   *  add-on's name). Named in the tooltip and the accessible name, so an
   *  add-on titled "Changes" never passes for the core pane. */
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
        className="group/close flex size-[18px] shrink-0 items-center justify-center rounded-sm hover:bg-foreground/10"
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
        {badge}
      </button>
    </div>
  );
}

/** Width of the edge fade that marks clipped tabs. */
const EDGE_FADE_PX = 16;

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
  const reserve = inTitlebar ? topRightReserve(remoteClient, true) : 0;
  const coreSurfaces = surfaces.filter((surface) => !isAddonPane(surface.id));
  const addonSurfaces = surfaces.filter((surface) => isAddonPane(surface.id));

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

  const tabRun = (
    <>
      {/* `no-scrollbar` is a real utility (shadcn/tailwind.css, imported by
          globals.css) — an earlier comment here claimed it was undefined,
          which is what spawned three hand-rolled copies of it.
          `scroll-px-4` keeps a tab scrolled into view clear of the fade. */}
      <div
        ref={setScrollerNode}
        data-testid="right-panel-tabs-scroll"
        className="no-scrollbar relative flex min-w-0 scroll-px-4 items-center overflow-x-auto"
        style={mask ? { maskImage: mask, WebkitMaskImage: mask } : undefined}
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

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label="Open pane"
            data-testid="right-panel-add-pane"
            className="ml-[3px] flex size-[24px] shrink-0 items-center justify-center rounded-md text-foreground/42 transition-colors duration-100 hover:bg-surface-2 hover:text-foreground data-[state=open]:bg-surface-3 data-[state=open]:text-foreground"
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
          {/* Add-on panels get their own section, each attributed to its
              add-on, so none can pass for a core pane. Absent without any. */}
          {addonSurfaces.length > 0 && (
            <>
              <DropdownMenuSeparator className="mx-1 my-[5px]" />
              <DropdownMenuLabel className="px-[9px] pb-[5px] pt-1.5 font-mono text-micro tracking-[0.13em] text-muted-foreground">
                ADD-ONS
              </DropdownMenuLabel>
              {addonSurfaces.map((surface) => (
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
  // right padding clears.
  const paneActions = actions != null && (
    <div
      data-testid="right-panel-pane-actions"
      className="flex shrink-0 items-center gap-[2px]"
    >
      {actions}
    </div>
  );

  return (
    <PanelHeader
      variant={inTitlebar ? "floating" : "inline"}
      data-testid="right-panel-tabs-header"
      data-in-titlebar={inTitlebar ? "true" : undefined}
      className={cn(
        // One hairline under this row and nothing else between it and the
        // pane body. The body starts flush. The floating variant carries no
        // rule of its own, so the band row asks for one explicitly: its seam
        // is the bottom edge of the window band.
        "gap-[2px] border-b border-hairline px-[7px]",
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
