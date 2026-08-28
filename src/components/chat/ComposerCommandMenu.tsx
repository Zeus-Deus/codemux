import { Command as CommandPrimitive } from "cmdk";
import { Search } from "lucide-react";
import { useEffect, useRef } from "react";

import { cn } from "@/lib/utils";
import {
  groupSlashItems,
  type CommandTone,
  type SlashCommandItem,
} from "@/lib/agent-chat/slash-commands";

import type { SlashCommandFooterNote } from "./SlashCommandPopup";

/**
 * Tone → token-utility map for the 24px icon chip. Chip background is
 * the tone at 15%; the icon is the tone at full strength. Every value
 * routes through an app colour token (no hex / oklch literals) so the
 * menu re-skins with the theme. Sky reuses the `status-remote` token,
 * amber the `warning` token, and so on — the design's palette expressed
 * in Codemux's own vocabulary.
 */
const TONE_CLASSES: Record<CommandTone, { chip: string; icon: string }> = {
  sky: { chip: "bg-status-remote/15", icon: "text-status-remote" },
  amber: { chip: "bg-warning/15", icon: "text-warning" },
  violet: { chip: "bg-accent-violet/15", icon: "text-accent-violet" },
  green: { chip: "bg-success/15", icon: "text-success" },
  red: { chip: "bg-danger/15", icon: "text-danger" },
  muted: { chip: "bg-muted-foreground/15", icon: "text-muted-foreground" },
  ember: { chip: "bg-accent-ember/15", icon: "text-accent-ember" },
};

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
}

/**
 * Redesigned composer command menu (the `+` button's popup). Anchored
 * bottom-left to the trigger, content-width (360px), with a search
 * header, tone-tinted icon-chip rows, and mono tag chips.
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
}: Props) {
  const inputRef = useRef<HTMLInputElement>(null);

  // Reclaim focus on open and after every submode pivot. Clicking a
  // drill-in row blurs the input; re-focusing keeps cmdk's keyboard nav
  // live for the freshly-swapped list.
  useEffect(() => {
    if (!open) return;
    const raf = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(raf);
  }, [open, submode]);

  if (!open) return null;

  const groups = groupSlashItems(items);

  return (
    <div
      data-testid="composer-command-menu"
      className={cn(
        // Anchored bottom-left to the `+` trigger, 8px above the
        // composer card, content-width — never stretched across it.
        "absolute bottom-full left-2 z-[60] mb-2 w-[360px]",
        "overflow-hidden rounded-[13px] border border-border",
        "bg-popover text-popover-foreground shadow-2xl",
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
        className="flex flex-col"
      >
        <div className="flex items-center gap-2.5 border-b border-border/60 px-3 py-2.5">
          <Search
            className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
            aria-hidden
          />
          <CommandPrimitive.Input
            ref={inputRef}
            value={query}
            onValueChange={onQueryChange}
            placeholder={placeholder}
            data-testid="composer-command-search"
            className={cn(
              "flex-1 bg-transparent text-[13px] text-foreground",
              "outline-none placeholder:text-muted-foreground",
            )}
          />
        </div>
        <CommandPrimitive.List className="max-h-[340px] overflow-x-hidden overflow-y-auto p-1.5 outline-none">
          {items.length === 0 ? (
            <div className="px-3 py-6 text-center text-xs text-muted-foreground">
              No matches
            </div>
          ) : (
            groups.map(({ group, items: groupItems }) => (
              <CommandPrimitive.Group
                key={group}
                heading={group}
                className={cn(
                  "**:[[cmdk-group-heading]]:px-2",
                  "**:[[cmdk-group-heading]]:pt-2 **:[[cmdk-group-heading]]:pb-1",
                  "**:[[cmdk-group-heading]]:font-mono",
                  "**:[[cmdk-group-heading]]:text-[10px]",
                  "**:[[cmdk-group-heading]]:font-semibold",
                  "**:[[cmdk-group-heading]]:uppercase",
                  "**:[[cmdk-group-heading]]:tracking-[0.09em]",
                  "**:[[cmdk-group-heading]]:text-muted-foreground",
                )}
              >
                {groupItems.map((item) => {
                  const Icon = item.icon;
                  const disabled = item.disabled === true;
                  const tone = TONE_CLASSES[item.tone ?? "muted"];
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
                      className={cn(
                        "flex items-center gap-2.5 rounded-md px-2 py-1.5",
                        "cursor-pointer outline-none select-none",
                        "data-[selected=true]:bg-muted/70",
                        // Disabled rows stay VISIBLE (dimmed) with their
                        // reason in the description slot.
                        disabled && "cursor-not-allowed opacity-[0.42]",
                      )}
                    >
                      <span
                        className={cn(
                          "flex h-6 w-6 shrink-0 items-center justify-center rounded-sm",
                          tone.chip,
                        )}
                      >
                        {Icon && (
                          <Icon
                            className={cn(
                              "h-3.5 w-3.5",
                              // Per-item override (e.g. state-coloured
                              // issue icons) wins; else the row's tone.
                              item.iconClassName ?? tone.icon,
                            )}
                            aria-hidden
                          />
                        )}
                      </span>
                      <span className="flex min-w-0 flex-1 flex-col gap-px">
                        <span className="truncate text-[13px] font-semibold leading-tight text-foreground">
                          {item.label}
                        </span>
                        {item.description && (
                          <span className="truncate text-[11px] leading-tight text-muted-foreground">
                            {item.description}
                          </span>
                        )}
                      </span>
                      {item.rightAdornment ? (
                        <span
                          className="ml-auto shrink-0"
                          // Keep the trailing control's clicks from
                          // bubbling into row selection (inline Switch).
                          onClick={(e) => e.stopPropagation()}
                          onMouseDown={(e) => e.stopPropagation()}
                          onPointerDown={(e) => e.stopPropagation()}
                        >
                          {item.rightAdornment}
                        </span>
                      ) : item.command ? (
                        <span className="ml-auto shrink-0 rounded-[5px] bg-foreground/[0.06] px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground">
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
                "border-t border-border/40 px-3 py-2 text-[11px]",
                footerNote.tone === "error"
                  ? "text-destructive"
                  : "text-muted-foreground/80",
              )}
            >
              {footerNote.message}
            </div>
          )}
        </CommandPrimitive.List>
      </CommandPrimitive>
    </div>
  );
}
