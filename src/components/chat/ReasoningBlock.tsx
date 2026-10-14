import { Lightbulb } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import type { ReasoningItem } from "@/lib/agent-chat/types";

/**
 * Collapsible thinking line, drawn with shadcn's Trace so it matches the work
 * log. Driven entirely by the reducer's `ReasoningItem`:
 *
 *  - While `streaming`: the title shimmers "Thinking…", the line auto-opens
 *    and the body streams the thinking text live.
 *  - Once sealed: the title reads "Thought for Ns" (from `duration_ms`) and
 *    the line auto-collapses once — the user can still toggle it back.
 *
 * The body remains plain italic prose (thinking traces are not trusted
 * markdown and we don't want a shiki/markdown pass on a hot streaming row);
 * inline `code`-fenced spans are left verbatim.
 *
 * `live` decides whether this row owns the animated orb. It defaults to the
 * item's own `streaming` flag, which is right in the main transcript; the
 * subagent drill-in passes `false` because its live tail already animates the
 * run's single orb (one-orb doctrine — see `lib/agent-chat/orb-activity.ts`).
 */
export const ReasoningBlock = memo(function ReasoningBlock({
  item,
  live = item.streaming,
}: {
  item: ReasoningItem;
  live?: boolean;
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

  return (
    <div className="-mx-1 select-text px-1 py-0.5">
      <Trace>
        <TraceCollapsible
          open={open}
          onOpenChange={(next) => {
            userTouched.current = true;
            setOpen(next);
          }}
        >
          <TraceCollapsibleTrigger>
            <TraceLine status={item.streaming ? "running" : undefined}>
              {live ? (
                <TraceIcon className="size-5 -mx-0.5">
                  <AgentOrb size={20} aria-hidden />
                </TraceIcon>
              ) : (
                <TraceIcon>
                  <Lightbulb />
                </TraceIcon>
              )}
              <TraceTitle>{deriveLabel(item)}</TraceTitle>
            </TraceLine>
          </TraceCollapsibleTrigger>
          {item.text.length > 0 && (
            <TraceCollapsibleContent>
              <p className="whitespace-pre-wrap break-words text-body italic leading-[1.6] text-muted-foreground">
                {item.text}
              </p>
            </TraceCollapsibleContent>
          )}
        </TraceCollapsible>
      </Trace>
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
