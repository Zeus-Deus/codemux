import { useState } from "react";
import { ExternalLink, Plus, X } from "lucide-react";

import { cn } from "@/lib/utils";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { providerRef, resolveProvider } from "@/lib/source-control";
import { matchesPrSearch, parsePrSearch, rowKey, type PrRow } from "@/lib/pr-overview";
import {
  tzBody,
  tzMeta,
  tzMetaNum,
} from "@/components/workspace/review/review-ui";

const DOT_TONE: Record<string, string> = {
  failing: "bg-destructive",
  pending: "bg-status-working",
  passing: "bg-status-open",
  none: "bg-border",
};

export interface PrTabStripProps {
  tabs: PrRow[];
  activeKey: string | null;
  /** Everything loaded, for the `+` picker. */
  candidates: PrRow[];
  onSelect: (row: PrRow) => void;
  onClose: (key: string) => void;
  onOpenInBrowser: () => void;
}

/**
 * The open pull requests, as tabs.
 *
 * Each tab carries its own status dot, so two open pull requests tell
 * you their state without being switched to — which is the whole reason
 * to keep more than one open at a time.
 *
 * Page-local: these are what *this session* is working through, not a
 * setting, so they are deliberately not persisted.
 */
export function PrTabStrip({
  tabs,
  activeKey,
  candidates,
  onSelect,
  onClose,
  onOpenInBrowser,
}: PrTabStripProps) {
  const [pickerOpen, setPickerOpen] = useState(false);
  const [query, setQuery] = useState("");

  const openKeys = new Set(tabs.map(rowKey));
  const search = parsePrSearch(query);
  const picks = candidates
    .filter((row) => !openKeys.has(rowKey(row)) && matchesPrSearch(row, search))
    .slice(0, 30);

  return (
    <div
      className="flex h-[33px] shrink-0 items-center gap-0.5 border-b border-border/40 px-2"
      data-testid="pr-tab-strip"
    >
      {tabs.map((row) => {
        const key = rowKey(row);
        const active = key === activeKey;
        return (
          <span
            key={key}
            data-testid={`pr-tab-${row.number}`}
            data-active={active}
            onMouseDown={(event) => {
              // Middle click closes, the same as everywhere else tabs
              // exist; it must not also select on the way past.
              if (event.button === 1) {
                event.preventDefault();
                onClose(key);
              }
            }}
            className={cn(
              "group flex h-[25px] shrink-0 cursor-default items-center gap-1.5 rounded-[5px] px-2.5",
              active ? "bg-card" : "hover:bg-muted/40",
            )}
            onClick={() => onSelect(row)}
          >
            <span
              aria-hidden
              className={cn(
                "size-[7px] shrink-0 rounded-full",
                DOT_TONE[row.checks] ?? DOT_TONE.none,
              )}
            />
            <span
              className={cn(
                "font-mono",
                tzMetaNum,
                active ? "text-foreground" : "text-muted-foreground",
              )}
            >
              {providerRef(resolveProvider(row.providerKind), row.number)}
            </span>
            <button
              type="button"
              aria-label={`Close ${row.number}`}
              data-testid={`pr-tab-close-${row.number}`}
              onClick={(event) => {
                event.stopPropagation();
                onClose(key);
              }}
              className="hidden size-3 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground group-hover:flex"
            >
              <X className="size-2.5" />
            </button>
          </span>
        );
      })}

      <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            aria-label="Open another pull request"
            data-testid="pr-tab-add"
            className="flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
          >
            <Plus className="size-3" />
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-72 p-0">
          <input
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Open a pull request…"
            data-testid="pr-tab-picker-search"
            className={cn(
              "w-full border-b border-border/40 bg-transparent px-2.5 py-2.5 outline-none placeholder:text-muted-foreground",
              tzBody,
            )}
          />
          <div className="max-h-64 overflow-y-auto py-1">
            {picks.length === 0 ? (
              <p className={cn("px-2.5 py-2 text-muted-foreground", tzBody)}>
                Everything loaded is already open here.
              </p>
            ) : (
              picks.map((row) => (
                <button
                  key={rowKey(row)}
                  type="button"
                  onClick={() => {
                    onSelect(row);
                    setPickerOpen(false);
                    setQuery("");
                  }}
                  className="flex w-full items-center gap-2 px-2.5 py-1.5 text-left hover:bg-accent/50"
                >
                  <span
                    aria-hidden
                    className={cn(
                      "size-[7px] shrink-0 rounded-full",
                      DOT_TONE[row.checks] ?? DOT_TONE.none,
                    )}
                  />
                  <span className={cn("shrink-0 font-mono text-muted-foreground", tzMeta)}>
                    {providerRef(resolveProvider(row.providerKind), row.number)}
                  </span>
                  <span className={cn("min-w-0 flex-1 truncate text-foreground", tzBody)}>
                    {row.title}
                  </span>
                </button>
              ))
            )}
          </div>
        </PopoverContent>
      </Popover>

      <span className="flex-1" />

      <button
        type="button"
        aria-label="Open in browser"
        data-testid="pr-tab-open-browser"
        onClick={onOpenInBrowser}
        className="flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
      >
        <ExternalLink className="size-3" />
      </button>
    </div>
  );
}
