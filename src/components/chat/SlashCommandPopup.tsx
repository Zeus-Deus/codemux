import { Command as CommandPrimitive } from "cmdk";
import { memo, useCallback, useLayoutEffect, useMemo, useRef, type SyntheticEvent } from "react";

import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { CommandSourceIcon } from "./CommandSourceIcon";
import { usePopupHighlight, type PopupHighlightStore } from "./popup-highlight-store";
import {
  groupSlashItems,
  type SlashCommandItem,
} from "@/lib/agent-chat/slash-commands";

/** Where a highlight change came from — see `onHighlightChange`. */
export type HighlightSource = "pointer" | "list";

/**
 * Optional muted footer row appended below all items. Used by Step 7
 * (skills) to communicate the lazy-load progress and surface errors
 * without blocking modes from being picked. Non-selectable.
 */
export interface SlashCommandFooterNote {
  tone: "muted" | "error";
  message: string;
}

interface Props {
  /** Filtered items to show. Filtering happens in the parent so the
   *  parent can decide which items belong (modes vs. skills, active
   *  mode hidden, etc.). */
  items: SlashCommandItem[];
  /** Current cmdk highlight value (matches `SlashCommandItem.id`).
   *  Controlled so the parent can keep the textarea focused while the
   *  popup advances on Up/Down. */
  highlightedId: string | null;
  /** Reports highlight changes. `source` says where one came from:
   *  `"pointer"` when the cursor moved onto a row (a deliberate user
   *  action, exactly like ArrowUp/ArrowDown), `"list"` when cmdk
   *  re-selected on its own because the item list churned. Parents
   *  that correct the auto-highlight need the difference — treating
   *  churn as intent freezes the auto-pick on whichever row happened
   *  to arrive first. */
  onHighlightChange: (id: string, source: HighlightSource) => void;
  /** Activated when the user clicks an item. The parent also calls
   *  `item.onSelect` directly when the textarea Enter handler resolves
   *  the highlighted item — this prop is for mouse-driven selection. */
  onSelect: (item: SlashCommandItem) => void;
  /** Hide / show. */
  open: boolean;
  /** Optional muted/error annotation rendered after the items list. */
  footerNote?: SlashCommandFooterNote | null;
}

/** Shared handler so the three interactive-adornment listeners stay
 *  identical. */
const stopRowSelection = (event: SyntheticEvent) => event.stopPropagation();

/**
 * Slash-command popup. Anchored above the composer textarea, this
 * component is intentionally generic — it knows about `SlashCommandItem`
 * and nothing else, so Step 7 (skills) can append its own items
 * without touching this file.
 *
 * Filtering and keyboard nav are driven by the parent via the
 * `highlightedId` prop. The parent intercepts ArrowUp/ArrowDown/Enter
 * on the textarea and updates `highlightedId` accordingly. cmdk
 * renders the highlighted state via its `value` prop and forwards
 * mouse-hover changes back through the popup pointer handler.
 *
 * Positioning: rendered as `absolute bottom-full` inside the same
 * `relative` wrapper as the textarea so the popup floats just above
 * the input. Composer sets the wrapper class.
 */
export function SlashCommandPopup({
  items,
  highlightedId,
  onHighlightChange,
  onSelect,
  open,
  footerNote = null,
}: Props) {
  const listRef = useRef<HTMLDivElement | null>(null);
  const pointerHighlightRef = useRef<string | null>(null);
  const pointerPositionRef = useRef<{ x: number; y: number } | null>(null);
  const selectRef = useRef(onSelect);
  selectRef.current = onSelect;
  const selectItem = useCallback((item: SlashCommandItem) => selectRef.current(item), []);

  // Hover must not pull a manually scrolled menu back toward its selection.
  // Keyboard navigation scrolls only this viewport, never the chat or window.
  useLayoutEffect(() => {
    if (!open || !highlightedId) return;
    if (pointerHighlightRef.current === highlightedId) {
      pointerHighlightRef.current = null;
      return;
    }
    pointerHighlightRef.current = null;
    const list = listRef.current;
    const viewport = list?.closest<HTMLElement>("[data-slot=scroll-area-viewport]");
    const target = list?.querySelector<HTMLElement>(
      `[data-testid="slash-item-${CSS.escape(highlightedId)}"]`,
    );
    if (!viewport || !target) return;
    const bounds = viewport.getBoundingClientRect();
    const row = target.getBoundingClientRect();
    if (row.top < bounds.top) viewport.scrollTop += row.top - bounds.top;
    else if (row.bottom > bounds.bottom) viewport.scrollTop += row.bottom - bounds.bottom;
  }, [highlightedId, open]);

  // Keep the scroll area out of highlight moves: cmdk's store updates only the
  // two affected rows, so held arrow keys stay cheap in WebKitGTK.
  const list = useMemo(
    () => (
      <>
        {/*
          Stage 4 polish — wrap the cmdk list in shadcn `ScrollArea`
          so the popup gets a visible (Radix-styled) scrollbar when
          content overflows the 320 px cap. Previously the list had
          `overflow-y-auto no-scrollbar`, which scrolled silently and
          gave users no affordance that more rows existed below the
          fold (notably: the new "MCP Servers…" attach row was
          frequently below the fold). The ScrollArea owns the
          scrolling ancestor; the cmdk List is a plain listbox inside
          it. Keyboard navigation adjusts only this viewport’s scroll position.
        */}
        {/*
          `type="always"` keeps the scrollbar permanently visible
          whenever content overflows the 320 px cap (Radix's default
          is `"hover"`, which fades the scrollbar out — invisible to
          users who haven't moved the cursor over the right edge yet).
          The wider scrollbar (`w-2`) gets explicit sizing via the
          `[&_…scrollbar]` selectors below so it's a solid affordance
          rather than a 1 px line.
        */}
        <ScrollArea
          type="always"
          className={cn(
            "max-h-80 w-full",
            // Viewport: cap height + force the inner div cmdk renders
            // to sit on a single block (cmdk's <Command> spreads
            // multi-children inside CommandList; the viewport's
            // default flex layout otherwise stretches a single child).
            "[&>[data-slot=scroll-area-viewport]]:max-h-80",
            "[&>[data-slot=scroll-area-viewport]]:overscroll-contain",
            // Radix's viewport wraps children in a `display: table;
            // min-width: 100%` div (inline styles). A table box is
            // shrink-to-fit, so any row wider than the popup — a long
            // chat title plus its provider/timestamp adornment —
            // stretches the table past 100% and gets clipped by the
            // wrapper's `overflow-hidden` instead of truncating.
            // Forcing the wrapper back to a plain full-width block
            // gives `truncate` a definite width to work against.
            // `!` is required: these override inline styles.
            "[&>[data-slot=scroll-area-viewport]>div]:!block",
            "[&>[data-slot=scroll-area-viewport]>div]:!w-full",
            "[&>[data-slot=scroll-area-viewport]>div]:!min-w-0",
            // Scrollbar: solid track + visible thumb in the popover's
            // contrast tier so it reads against the dark popup bg.
            "[&_[data-slot=scroll-area-scrollbar]]:w-2",
            "[&_[data-slot=scroll-area-thumb]]:bg-foreground/30",
            "[&_[data-slot=scroll-area-thumb]]:hover:bg-foreground/50",
          )}
        >
          <CommandPrimitive.List
            ref={listRef}
            className="outline-none"
          >
          <CommandRows items={items} onSelect={selectItem} />
          {footerNote && (
            <div
              data-testid="slash-popup-footer"
              data-tone={footerNote.tone}
              className={cn(
                "px-3 py-2 text-label border-t border-border/40",
                footerNote.tone === "error"
                  ? "text-destructive"
                  : "text-muted-foreground/80",
              )}
            >
              {footerNote.message}
            </div>
          )}
          </CommandPrimitive.List>
        </ScrollArea>
      </>
    ),
    [items, footerNote, selectItem],
  );

  if (!open) return null;

  return (
    <div
      data-testid="slash-command-popup"
      className={cn(
        "absolute bottom-full left-0 right-0 mb-2 z-50",
        "rounded-lg border border-border/60 bg-popover shadow-md",
        "overflow-hidden",
      )}
      onPointerLeave={() => {
        pointerPositionRef.current = null;
      }}
      onPointerMove={(event) => {
        if (event.buttons !== 0) return;
        const previous = pointerPositionRef.current;
        const position = { x: event.clientX, y: event.clientY };
        pointerPositionRef.current = position;
        // Scrolling can place another row under a stationary cursor.
        // Only actual pointer movement should change the highlight.
        if (previous?.x === position.x && previous.y === position.y) return;
        const row = (event.target as HTMLElement).closest<HTMLElement>("[cmdk-item]");
        const id = row?.getAttribute("data-value");
        if (!id || row?.getAttribute("aria-disabled") === "true") return;
        pointerHighlightRef.current = id;
        onHighlightChange(id, "pointer");
      }}
      // Pointer events live on the popup itself; the textarea keeps
      // focus, so cmdk never gets keyboard input — the parent drives
      // highlight via the `value` prop.
      //
      // EXCEPTION: when the user clicks anywhere inside the
      // `ScrollArea` scrollbar (track or thumb), we skip
      // `preventDefault` so Radix's drag handling actually works.
      // Without this exception, mousedown on the thumb is cancelled
      // and the scrollbar can't be dragged — clicks on the track
      // also misbehave (they end up jumping to extremes because
      // Radix's offset calc reads stale pointer coords). We still
      // preventDefault for everything else to keep the textarea
      // focused.
      onMouseDown={(e) => {
        const target = e.target as HTMLElement | null;
        if (
          target &&
          (target.closest("[data-slot=scroll-area-scrollbar]") ||
            target.closest("[data-slot=scroll-area-thumb]"))
        ) {
          return;
        }
        e.preventDefault();
      }}
    >
      <CommandPrimitive
        // Manual filtering — parent decides what's visible.
        shouldFilter={false}
        value={highlightedId ?? ""}
        disablePointerSelection
        onValueChange={(id) => {
          if (id) onHighlightChange(id, "list");
        }}
        className="text-popover-foreground"
      >
        {list}
        {items.find((item) => item.id === highlightedId)?.identity && <div data-testid="slash-item-source" className="border-t border-border/40 px-3 py-2 text-label text-muted-foreground">
          {items.find((item) => item.id === highlightedId)?.identity?.label}
        </div>}
      </CommandPrimitive>
    </div>
  );
}

/** Reads the highlight from a store so arrow keys re-render only this popup. */
export function StoreSlashCommandPopup({
  highlight,
  ...props
}: Omit<Props, "highlightedId"> & { highlight: PopupHighlightStore }) {
  return <SlashCommandPopup {...props} highlightedId={usePopupHighlight(highlight)} />;
}

// Keep the catalogue mounted during arrow navigation. cmdk updates the two
// selected rows through its store; recreating every row on each repeat makes
// long skill inventories lag behind the user's key release.
const CommandRows = memo(function CommandRows({ items, onSelect }: Pick<Props, "items" | "onSelect">) {
  const groups = groupSlashItems(items);
  return (
    <>
      {items.length === 0 ? (
            <div className="px-3 py-4 text-center text-label text-muted-foreground">
              No commands match
            </div>
          ) : (
            groups.map(({ group, items: groupItems }) => (
              <CommandPrimitive.Group
                key={group}
                heading={group}
                className={cn(
                  "p-1",
                  "**:[[cmdk-group-heading]]:px-2",
                  "**:[[cmdk-group-heading]]:pt-1.5",
                  "**:[[cmdk-group-heading]]:pb-1",
                  "**:[[cmdk-group-heading]]:text-caption",
                  "**:[[cmdk-group-heading]]:font-semibold",
                  "**:[[cmdk-group-heading]]:uppercase",
                  "**:[[cmdk-group-heading]]:tracking-wider",
                  "**:[[cmdk-group-heading]]:text-muted-foreground/70",
                )}
              >
                {groupItems.map((item) => {
                  const Icon = item.icon;
                  const disabled = item.disabled === true;
                  return (
                    <CommandPrimitive.Item
                      key={item.id}
                      value={item.id}
                      // Step 8 Stage 3 — disabled rows skip onSelect
                      // entirely so accidental clicks on "coming soon"
                      // entries don't activate them. cmdk's built-in
                      // `disabled` prop also prevents selection-state
                      // updates, which keeps the highlight from
                      // landing on un-pickable items via mouse hover.
                      disabled={disabled}
                      onSelect={() => {
                        if (disabled) return;
                        onSelect(item);
                      }}
                      data-testid={`slash-item-${item.id}`}
                      data-disabled={disabled || undefined}
                      className={cn(
                        "flex items-center gap-2 rounded-sm px-2 py-1.5 text-body",
                        item.stacked && "gap-2.5 py-2",
                        "cursor-pointer outline-none select-none",
                        "data-[selected=true]:bg-muted",
                        "data-[selected=true]:text-foreground",
                        disabled && "opacity-50 cursor-not-allowed",
                      )}
                    >
                      {item.identity ? <CommandSourceIcon identity={item.identity} /> : Icon && (
                        <span
                          className={cn(
                            "flex w-3.5 shrink-0 items-center",
                            item.stacked &&
                              "flex size-7 shrink-0 items-center justify-center rounded-md border border-border/40 bg-muted/35",
                          )}
                        >
                        <Icon
                          className={cn(
                            "size-3.5 shrink-0",
                            // Per-item override (e.g. green for open
                            // issues) wins; otherwise default muted.
                            item.iconClassName ?? "text-muted-foreground",
                          )}
                          aria-hidden
                        />
                        </span>
                      )}
                      {item.stacked ? (
                        <span className="min-w-0 flex-1">
                          <span className="block truncate font-medium text-foreground">
                            {item.label}
                          </span>
                          {item.description && (
                            <span className="mt-0.5 block truncate text-label leading-4 text-muted-foreground">
                              {item.description}
                            </span>
                          )}
                        </span>
                      ) : (
                        <>
                      <span className="max-w-[40%] shrink-0 truncate text-foreground" title={item.label}>{item.label}</span>
                      {item.argumentHint &&
                        // The description already falls back to
                        // `/<name> <hint>`; don't repeat it.
                        item.description !==
                          `${item.command} ${item.argumentHint}` && (
                          <span className="truncate font-mono text-label text-muted-foreground/70">
                            {item.argumentHint}
                          </span>
                        )}
                      {item.description && (
                        <span className="min-w-0 flex-1 truncate text-label text-muted-foreground">
                          {item.description}
                        </span>
                      )}
                        </>
                      )}
                      {item.rightAdornment ? (
                        <span
                          className={cn(
                            "ml-auto",
                            item.stacked && "shrink-0 self-stretch",
                          )}
                          // Stop the trailing control's clicks from
                          // bubbling up and triggering row selection
                          // (e.g. inline Switch in the MCP submenu).
                          // Only for adornments that really are
                          // interactive — guarding a decorative one
                          // makes its slice of the row unclickable.
                          {...(item.rightAdornmentInteractive
                            ? {
                                onClick: stopRowSelection,
                                onMouseDown: stopRowSelection,
                                onPointerDown: stopRowSelection,
                              }
                            : null)}
                        >
                          {item.rightAdornment}
                        </span>
                      ) : (
                        <span className="ml-auto max-w-[30%] shrink-0 truncate font-mono text-label text-muted-foreground/70" title={item.command}>
                          {item.command}
                        </span>
                      )}
                    </CommandPrimitive.Item>
                  );
                })}
              </CommandPrimitive.Group>
            ))
          )}
    </>
  );
});
