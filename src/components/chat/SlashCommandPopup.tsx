import { Command as CommandPrimitive } from "cmdk";
import { useEffect, useRef } from "react";

import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  groupSlashItems,
  type SlashCommandItem,
} from "@/lib/agent-chat/slash-commands";

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
  /** Reports user-driven highlight changes (mouse hover). */
  onHighlightChange: (id: string) => void;
  /** Activated when the user clicks an item. The parent also calls
   *  `item.onSelect` directly when the textarea Enter handler resolves
   *  the highlighted item — this prop is for mouse-driven selection. */
  onSelect: (item: SlashCommandItem) => void;
  /** Hide / show. */
  open: boolean;
  /** Optional muted/error annotation rendered after the items list. */
  footerNote?: SlashCommandFooterNote | null;
}

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
 * mouse-hover changes back through `onValueChange`.
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

  // cmdk's built-in scrollIntoView only fires when cmdk itself handles
  // the keyboard event. Because the textarea keeps focus and the parent
  // drives highlight via the controlled `value` prop, cmdk never sees
  // ArrowUp/Down — so we replicate the scroll-into-view ourselves
  // whenever the highlighted item changes.
  useEffect(() => {
    if (!open || !highlightedId) return;
    const list = listRef.current;
    if (!list) return;
    const target = list.querySelector<HTMLElement>(
      `[data-testid="slash-item-${CSS.escape(highlightedId)}"]`,
    );
    target?.scrollIntoView({ block: "nearest" });
  }, [highlightedId, open]);

  if (!open) return null;

  const groups = groupSlashItems(items);

  return (
    <div
      data-testid="slash-command-popup"
      className={cn(
        "absolute bottom-full left-0 right-0 mb-2 z-50",
        "rounded-lg border border-border/60 bg-popover shadow-md",
        "overflow-hidden",
      )}
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
        onValueChange={(id) => {
          if (id) onHighlightChange(id);
        }}
        className="text-popover-foreground"
      >
        {/*
          Stage 4 polish — wrap the cmdk list in shadcn `ScrollArea`
          so the popup gets a visible (Radix-styled) scrollbar when
          content overflows the 320 px cap. Previously the list had
          `overflow-y-auto no-scrollbar`, which scrolled silently and
          gave users no affordance that more rows existed below the
          fold (notably: the new "MCP Servers…" attach row was
          frequently below the fold). The ScrollArea owns the
          scrolling ancestor; the cmdk List is a plain listbox inside
          it. `scrollIntoView` from the keyboard-nav effect walks up
          to the ScrollArea viewport unchanged.
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
          {items.length === 0 ? (
            <div className="px-3 py-4 text-center text-xs text-muted-foreground">
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
                  "**:[[cmdk-group-heading]]:text-[10px]",
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
                        "flex items-center gap-2 rounded px-2 py-1.5 text-sm",
                        item.stacked && "gap-2.5 py-2",
                        "cursor-pointer outline-none select-none",
                        "data-[selected=true]:bg-muted",
                        "data-[selected=true]:text-foreground",
                        disabled && "opacity-50 cursor-not-allowed",
                      )}
                    >
                      {Icon && (
                        <span
                          className={cn(
                            item.stacked &&
                              "flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-border/40 bg-muted/35",
                          )}
                        >
                        <Icon
                          className={cn(
                            "h-3.5 w-3.5 shrink-0",
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
                            <span className="mt-0.5 block truncate text-[11px] leading-4 text-muted-foreground">
                              {item.description}
                            </span>
                          )}
                        </span>
                      ) : (
                        <>
                      <span className="text-foreground">{item.label}</span>
                      {item.description && (
                        <span className="truncate text-xs text-muted-foreground">
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
                          onClick={(e) => e.stopPropagation()}
                          onMouseDown={(e) => e.stopPropagation()}
                          onPointerDown={(e) => e.stopPropagation()}
                        >
                          {item.rightAdornment}
                        </span>
                      ) : (
                        <span className="ml-auto font-mono text-[11px] text-muted-foreground/70">
                          {item.command}
                        </span>
                      )}
                    </CommandPrimitive.Item>
                  );
                })}
              </CommandPrimitive.Group>
            ))
          )}
          {footerNote && (
            <div
              data-testid="slash-popup-footer"
              data-tone={footerNote.tone}
              className={cn(
                "px-3 py-2 text-[11px] border-t border-border/40",
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
      </CommandPrimitive>
    </div>
  );
}
