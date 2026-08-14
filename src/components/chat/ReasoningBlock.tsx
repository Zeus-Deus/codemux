import { ChevronDown, Lightbulb } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import { cn } from "@/lib/utils";
import type { ReasoningItem } from "@/lib/agent-chat/types";

/**
 * Collapsible thinking row. Driven entirely by the
 * reducer's `ReasoningItem`:
 *
 *  - While `streaming`: header shimmers "Thinking…", the block auto-opens
 *    and the body streams the thinking text live.
 *  - Once sealed: header reads "Thought for Ns" (from `duration_ms`) and
 *    the block auto-collapses once — the user can still toggle it back.
 *
 * The flat row follows the compact work-log treatment: no surrounding card,
 * a single 20px glyph slot, quiet metadata, and inline detail on a hairline.
 * The body remains plain italic prose (thinking traces are not trusted
 * markdown and we don't want a shiki/markdown pass on a hot streaming row);
 * inline `code`-fenced spans are left verbatim.
 */
export const ReasoningBlock = memo(function ReasoningBlock({
  item,
}: {
  item: ReasoningItem;
}) {
  // Auto-open while streaming; auto-collapse once (when streaming ends)
  // unless the reader has taken manual control.
  const [open, setOpen] = useState(item.streaming);
  const userTouched = useRef(false);
  const prevStreaming = useRef(item.streaming);

  useEffect(() => {
    if (userTouched.current) return;
    if (item.streaming && !open) setOpen(true);
    if (prevStreaming.current && !item.streaming && open) setOpen(false);
    prevStreaming.current = item.streaming;
  }, [item.streaming, open]);

  const label = deriveLabel(item);

  return (
    <div className="-mx-1 select-text px-1 py-0.5">
      <button
        type="button"
        onClick={() => {
          userTouched.current = true;
          setOpen((v) => !v);
        }}
        aria-expanded={open}
        className="flex w-full min-w-0 items-center gap-1.5 rounded-md px-0.5 py-0.5 text-left text-[12px] leading-5 transition-colors hover:bg-foreground/[0.035] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70"
      >
        <span className="flex size-5 shrink-0 items-center justify-center">
          {item.streaming ? (
            <AgentOrb size={20} aria-hidden />
          ) : (
            <Lightbulb
              className="size-3.5 text-accent-violet"
              strokeWidth={1.5}
              aria-hidden
            />
          )}
        </span>
        <span
          className={cn(
            "min-w-0 flex-1 truncate font-medium text-muted-foreground",
            item.streaming && "shimmer",
          )}
        >
          {label}
        </span>
        <ChevronDown
          className={cn(
            "size-3 shrink-0 text-muted-foreground/45 transition-transform",
            open && "rotate-180",
          )}
          aria-hidden
        />
      </button>
      {open && item.text.length > 0 && (
        <div className="ml-[10px] mt-0.5 border-l border-border/60 py-1.5 pl-3">
          <p className="whitespace-pre-wrap break-words text-[13px] italic leading-[1.6] text-muted-foreground">
            {item.text}
          </p>
        </div>
      )}
    </div>
  );
});

function deriveLabel(item: ReasoningItem): string {
  if (item.streaming) return "Thinking…";
  if (item.duration_ms != null) {
    const seconds = Math.max(1, Math.round(item.duration_ms / 1000));
    return `Thought for ${seconds}s`;
  }
  return "Thought";
}
