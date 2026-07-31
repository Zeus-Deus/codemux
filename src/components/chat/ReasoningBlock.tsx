import { ChevronRight, Lightbulb } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";

import { cn } from "@/lib/utils";
import type { ReasoningItem } from "@/lib/agent-chat/types";

/**
 * Collapsible "thinking" card (design D5). Driven entirely by the
 * reducer's `ReasoningItem`:
 *
 *  - While `streaming`: header shimmers "Thinking…", the block auto-opens
 *    and the body streams the thinking text live.
 *  - Once sealed: header reads "Thought for Ns" (from `duration_ms`) and
 *    the block auto-collapses once — the user can still toggle it back.
 *
 * The body is rendered as plain italic prose (thinking traces are not
 * trusted markdown and we don't want a shiki/markdown pass on a hot
 * streaming row); inline `code`-fenced spans are left verbatim.
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
    <div className="overflow-hidden rounded-[11px] border border-border/60 bg-foreground/[0.025]">
      <button
        type="button"
        onClick={() => {
          userTouched.current = true;
          setOpen((v) => !v);
        }}
        aria-expanded={open}
        className="flex w-full items-center gap-[9px] px-3 py-[9px] text-left"
      >
        <Lightbulb
          className="h-3.5 w-3.5 shrink-0 text-accent-violet"
          strokeWidth={1.5}
          aria-hidden
        />
        <span
          className={cn(
            "text-[13px] font-semibold text-muted-foreground",
            item.streaming && "shimmer",
          )}
        >
          {label}
        </span>
        <ChevronRight
          className={cn(
            "ml-auto h-3 w-3 shrink-0 text-muted-foreground/70 transition-transform",
            open && "rotate-90",
          )}
          aria-hidden
        />
      </button>
      {open && item.text.length > 0 && (
        <div className="whitespace-pre-wrap break-words border-t border-border/60 py-[11px] pl-[33px] pr-3.5 text-[13px] italic leading-[1.65] text-muted-foreground">
          {item.text}
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
