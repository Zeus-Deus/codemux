import { Check, ChevronRight } from "lucide-react";
import { memo, useState } from "react";

import { cn } from "@/lib/utils";
import type { ToolCallItem } from "@/lib/agent-chat/types";

import { describeToolCall } from "./ToolCallStatus";
import { categoryIcon, categoryTint, type ToolCategory } from "./tool-visuals";

/**
 * Tool group card (design D6): a contiguous run of ≥2 completed tool
 * calls folded into one collapsible card. Header shows a category icon
 * chip, a derived title, a "N commands" meta and a green check; the body
 * lists one mono row per call. Stays a single scroller row that grows on
 * expand. Running / pending-approval / error / TodoWrite calls are never
 * grouped (they render as their own cards) — the run builder guarantees
 * every item here is a completed, ungated tool call.
 */
export const ToolGroupCard = memo(function ToolGroupCard({
  items,
}: {
  items: ToolCallItem[];
}) {
  const [open, setOpen] = useState(false);
  const title = deriveGroupTitle(items);
  const category = groupCategory(items);
  const Icon = categoryIcon(category);

  return (
    <div className="overflow-hidden rounded-[11px] border border-border/60 bg-muted/40">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex w-full items-center gap-2.5 px-3 py-2.5 text-left"
      >
        <span
          className={cn(
            "flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-md",
            categoryTint(category),
          )}
        >
          <Icon className="h-3.5 w-3.5" strokeWidth={1.6} aria-hidden />
        </span>
        <span className="text-[12.5px] font-semibold text-foreground">
          {title}
        </span>
        <span className="font-mono text-[11px] text-muted-foreground">
          {items.length} commands
        </span>
        <span className="ml-auto flex items-center gap-2.5">
          <Check
            className="h-3.5 w-3.5 text-status-open"
            strokeWidth={1.8}
            aria-hidden
          />
          <ChevronRight
            className={cn(
              "h-3 w-3 text-muted-foreground/70 transition-transform",
              open && "rotate-90",
            )}
            aria-hidden
          />
        </span>
      </button>
      {open && (
        <div className="flex flex-col gap-0.5 border-t border-border/60 px-3 py-2">
          {items.map((item) => {
            const { target, argument } = describeToolCall(item);
            const cmd = [target, argument].filter(Boolean).join(" ");
            return (
              <div
                key={item.id}
                className="flex min-w-0 items-center gap-2.5 py-1 font-mono text-[11.5px] text-muted-foreground"
              >
                <span className="shrink-0 text-muted-foreground/60">
                  {item.tool_name.toLowerCase()}
                </span>
                <span className="min-w-0 flex-1 truncate">{cmd}</span>
                <Check
                  className="h-3.5 w-3.5 shrink-0 text-status-open"
                  strokeWidth={1.8}
                  aria-hidden
                />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
});

const SEARCH_LIKE = new Set(["Grep", "Glob", "Read", "WebSearch", "WebFetch"]);

/** Derive the group's title from its members: an all search/read run reads
 *  as "Searched the codebase"; an all-Bash run as "Ran commands"; anything
 *  mixed as "Ran N tools". */
export function deriveGroupTitle(items: ToolCallItem[]): string {
  const names = items.map((i) => i.tool_name);
  if (names.every((n) => SEARCH_LIKE.has(n))) return "Searched the codebase";
  if (names.every((n) => n === "Bash")) return "Ran commands";
  return `Ran ${items.length} tools`;
}

function groupCategory(items: ToolCallItem[]): ToolCategory {
  const names = items.map((i) => i.tool_name);
  if (names.every((n) => SEARCH_LIKE.has(n))) return "search";
  if (names.every((n) => n === "Bash")) return "terminal";
  return "other";
}
