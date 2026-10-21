import { Lightbulb } from "lucide-react";
import { memo } from "react";

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
 *  - While `streaming`: the title shimmers "Thinking…". The thought stays
 *    folded, so the transcript holds still while it streams; opening the
 *    line shows it live.
 *  - Once sealed: the title reads "Thought for Ns" (from `duration_ms`).
 *
 * The body remains plain italic prose (thinking traces are not trusted
 * markdown and we don't want a shiki/markdown pass on a hot streaming row);
 * inline `code`-fenced spans are left verbatim.
 */
export const ReasoningBlock = memo(function ReasoningBlock({
  item,
}: {
  item: ReasoningItem;
}) {
  return (
    <div className="-mx-1 select-text px-1 py-0.5">
      <Trace>
        <TraceCollapsible>
          <TraceCollapsibleTrigger>
            <TraceLine status={item.streaming ? "running" : undefined}>
              <TraceIcon>
                <Lightbulb />
              </TraceIcon>
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
