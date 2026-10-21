import { Command as CommandPrimitive } from "cmdk";
import { Search } from "lucide-react";
import { useEffect, useRef, type SyntheticEvent } from "react";

import { KeyHint } from "@/components/ui/key-hint";
import { ScrollEdgeFade, useScrollEdges } from "@/components/ui/scroll-edge-fade";
import { cn } from "@/lib/utils";
import {
  groupSlashItems,
  type SlashCommandItem,
} from "@/lib/agent-chat/slash-commands";

import {
  COMMAND_MENU_GROUP_HEADING,
  COMMAND_MENU_ICON,
  COMMAND_MENU_ROW,
  COMMAND_MENU_SURFACE,
} from "./command-menu-styles";
import { useRoomAbove } from "./command-menu-room";
import type { SlashCommandFooterNote } from "./SlashCommandPopup";

/** Shared handler so the three interactive-adornment listeners stay
 *  identical. */
const stopRowSelection = (event: SyntheticEvent) => event.stopPropagation();

interface Props {
  /** Hide / show. */
  open: boolean;
  /** Rows to render — already submode-appropriate AND already filtered
   *  by the parent (so keyboard nav + the visible list stay in sync).
   *  Grouped here by each item's `group`. */
  items: SlashCommandItem[];
  /** Search box value + change reporter. Controlled by the composer so
   *  the filtered `items` prop reflects it. */
  query: string;
  onQueryChange: (next: string) => void;
  /** Activated on click OR Enter (cmdk owns keyboard nav off the
   *  focused search input). Disabled rows never fire this. */
  onSelect: (item: SlashCommandItem) => void;
  /** Escape handler. The composer decides whether Escape walks a
   *  submode back to `main` or closes the menu. */
  onEscape: () => void;
  /** Search-box placeholder. Defaults to the main-view hint. */
  placeholder?: string;
  /** Optional muted / error annotation rendered after the items list
   *  (submode loading + "Esc to go back" hints). */
  footerNote?: SlashCommandFooterNote | null;
  /** Current submode key. Only used to re-focus the search input after
   *  a drill-in swaps the list under us. */
  submode: string;
  /** Session-config controls that no longer fit the composer's controls
   *  row at narrow widths (effort + access), shown under the search box. */
  headerSlot?: React.ReactNode;
}

/**
 * Composer command menu (the `+` button's popup). Anchored bottom-left
 * to the trigger, content-width (380px), on the composer's own surface:
 * a search header, one-line rows with plain icons, and a keyboard-hint
 * footer. Only the highlighted row uses the accent colour.
 *
 * Unlike {@link SlashCommandPopup} (slash / mention surfaces, which
 * keep the textarea focused and are driven from the parent), this menu
 * owns a real focused `cmdk` search input — so cmdk handles filtering-
 * agnostic keyboard nav (arrows / Enter) and, crucially, skips disabled
 * rows for free. Filtering itself happens in the parent so the disabled
 * "coming soon / not available" rows stay visible with their reason.
 */
export function ComposerCommandMenu({
  open,
  items,
  query,
  onQueryChange,
  onSelect,
  onEscape,
  placeholder = "Search or type /",
  footerNote = null,
  submode,
  headerSlot = null,
}: Props) {
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const edges = useScrollEdges(listRef, open);
  const rootRef = useRef<HTMLDivElement>(null);
  const room = useRoomAbove(rootRef, open);

  // Reclaim focus on open and after every submode pivot. Clicking a
  // drill-in row blurs the input; re-focusing keeps cmdk's keyboard nav
  // live for the freshly-swapped list. Not on touch screens: there the
  // focus would raise the on-screen keyboard over the menu just opened.
  useEffect(() => {
    if (!open || window.matchMedia?.("(pointer: coarse)").matches) return;
    const raf = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(raf);
  }, [open, submode]);

  if (!open) return null;

  const groups = groupSlashItems(items);

  return (
    <div
      ref={rootRef}
      data-testid="composer-command-menu"
      // Shrinks to the room above the card (a phone's landing page has
      // little), scrolling its list rather than sliding under the header.
      style={room === undefined ? undefined : { maxHeight: room }}
      className={cn(
        // Anchored bottom-left to the `+` trigger, 8px above the
        // composer card, content-width — never stretched across it, nor
        // past its far edge on a narrow card.
        "absolute bottom-full left-2 z-[60] mb-2 flex w-[380px] max-w-[calc(100%-1rem)] flex-col overflow-hidden",
        COMMAND_MENU_SURFACE,
        "rise-in",
      )}
      // Escape lives on the wrapper (outside cmdk's root) so it never
      // collides with cmdk's own arrow / Enter handling on the root:
      // keydown bubbles input → cmdk root (handles nav) → here (Escape).
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          onEscape();
        }
      }}
    >
      <CommandPrimitive
        // Parent decides which rows are visible (so disabled-with-reason
        // rows survive the filter); cmdk just renders + navigates them.
        shouldFilter={false}
        loop
        className="flex min-h-0 flex-col"
      >
        <div className="flex shrink-0 items-center gap-2.5 px-3.5 pt-3 pb-2">
          <Search
            className="size-3.5 shrink-0 text-muted-foreground"
            aria-hidden
          />
          <CommandPrimitive.Input
            ref={inputRef}
            value={query}
            onValueChange={onQueryChange}
            placeholder={placeholder}
            data-testid="composer-command-search"
            className={cn(
              "flex-1 bg-transparent text-body text-foreground caret-accent-ember",
              "outline-none placeholder:text-muted-foreground/70",
            )}
          />
        </div>
        {headerSlot && (
          <div
            data-testid="composer-command-menu-config"
            // Settings have their own buttons and portalled command menus.
            // Keep their keys out of the outer cmdk root, which otherwise
            // handles Enter by selecting the highlighted composer command.
            onKeyDown={(e) => {
              if (e.key !== "Escape" || e.defaultPrevented) e.stopPropagation();
            }}
            className="flex shrink-0 items-center gap-0.5 border-y border-hairline px-1.5 py-1"
          >
            {headerSlot}
          </div>
        )}
        <div className="relative flex min-h-0 flex-col">
          <CommandPrimitive.List ref={listRef} className="thin-scrollbar max-h-[340px] min-h-0 scroll-py-5 overflow-x-hidden overflow-y-auto px-1.5 pb-1.5 outline-none">
            {items.length === 0 ? (
              <div className="px-3 py-6 text-center text-label text-muted-foreground">
                No matches
              </div>
            ) : (
              groups.map(({ group, items: groupItems }) => (
                <CommandPrimitive.Group
                  key={group}
                  heading={group}
                  className={COMMAND_MENU_GROUP_HEADING}
                >
                  {groupItems.map((item) => {
                    const Icon = item.icon;
                    const disabled = item.disabled === true;
                    return (
                      <CommandPrimitive.Item
                        key={item.id}
                        value={item.id}
                        // cmdk skips disabled rows in keyboard nav and
                        // won't fire onSelect for them — but we double-
                        // guard so a stray click can't activate a "not a
                        // GitHub repo" / "coming soon" row either.
                        disabled={disabled}
                        onSelect={() => {
                          if (disabled) return;
                          onSelect(item);
                        }}
                        data-testid={`slash-item-${item.id}`}
                        data-disabled={disabled || undefined}
                        // Truncated descriptions (and disabled rows'
                        // reasons) stay readable on hover.
                        title={item.description}
                        className={cn(
                          COMMAND_MENU_ROW,
                          "min-h-8 py-1.5",
                          // Disabled rows stay VISIBLE (dimmed) with their
                          // reason in the description slot.
                          disabled && "cursor-not-allowed opacity-[0.42]",
                        )}
                      >
                        {Icon && (
                          <Icon
                            className={cn(
                              "size-3.5 shrink-0",
                              // Per-item override (e.g. state-coloured
                              // issue icons) wins over the default.
                              item.iconClassName ?? COMMAND_MENU_ICON,
                            )}
                            aria-hidden
                          />
                        )}
                        {item.stacked ? (
                          // Entity rows (conversations) keep name and
                          // preview on separate lines so each truncates
                          // on its own.
                          <span className="flex min-w-0 flex-1 flex-col gap-px">
                            <span className="truncate text-body font-medium leading-tight text-foreground">
                              {item.label}
                            </span>
                            {item.description && (
                              <span className="truncate text-label leading-tight text-muted-foreground">
                                {item.description}
                              </span>
                            )}
                          </span>
                        ) : (
                          <span className="flex min-w-0 flex-1 items-baseline gap-2">
                            <span
                              className={cn(
                                "truncate text-body font-medium text-foreground",
                                item.description
                                  ? "max-w-[55%] shrink-0"
                                  : "min-w-0 flex-1",
                              )}
                            >
                              {item.label}
                            </span>
                            {item.description && (
                              <span
                                className={cn(
                                  "min-w-0 flex-1 truncate text-label text-muted-foreground/70",
                                  // Highlighting only recolours: a row that
                                  // grew to show its whole description
                                  // would shove every row below it as the
                                  // pointer passes. The title tooltip has
                                  // the full text.
                                  "group-data-[selected=true]/row:text-muted-foreground",
                                )}
                              >
                                {item.description}
                              </span>
                            )}
                          </span>
                        )}
                        {item.rightAdornment ? (
                          <span
                            className="ml-auto shrink-0"
                            // Keep the trailing control's clicks from
                            // bubbling into row selection (inline
                            // Switch). A decorative adornment — a chat
                            // row's provider + timestamp — has to stay
                            // click-through, or its slice of the row
                            // becomes a dead zone.
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
                        ) : item.command ? (
                          <span className="ml-auto shrink-0 font-mono text-label text-muted-foreground/60 group-data-[selected=true]/row:text-foreground">
                            {item.command}
                          </span>
                        ) : null}
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
                  "border-t border-hairline px-3 py-2 text-label",
                  footerNote.tone === "error"
                    ? "text-destructive"
                    : "text-muted-foreground/80",
                )}
              >
                {footerNote.message}
              </div>
            )}
          </CommandPrimitive.List>
          <ScrollEdgeFade edge="top" visible={edges.top} surface="color-mix(in oklab, var(--muted) 40%, var(--background))" />
          <ScrollEdgeFade edge="bottom" visible={edges.bottom} surface="color-mix(in oklab, var(--muted) 40%, var(--background))" />
        </div>
        {/* Keyboard hints are noise on a touch screen. */}
        <div
          aria-hidden
          className="flex shrink-0 items-center gap-4 border-t border-hairline px-3.5 py-2 pointer-coarse:hidden"
        >
          <KeyHint keys="↑↓" label="move" />
          <KeyHint keys="↵" label="select" />
          <KeyHint keys="esc" label={submode === "main" ? "close" : "back"} />
        </div>
      </CommandPrimitive>
    </div>
  );
}
