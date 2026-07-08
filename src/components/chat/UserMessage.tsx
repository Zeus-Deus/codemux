import { memo } from "react";
import { X } from "lucide-react";

import type { UserMessageItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

/**
 * Right-aligned user bubble (design D3). Card fill, soft border, the
 * asymmetric `14px 14px 5px 14px` radius that tucks the bottom-right
 * corner toward the composer, and pre-wrapped text so pasted snippets
 * keep their line breaks. Width is capped so long turns don't span the
 * whole 760px column.
 *
 * Follow-up queueing: while `item.queued` is set the bubble renders
 * greyed-out (reduced opacity + muted foreground) with a small "Queued"
 * pill and, on hover, an X to cancel — cancelling restores the text into
 * the composer (handled by the parent). All colors are theme tokens.
 */
export const UserMessage = memo(function UserMessage({
  item,
  onCancelQueued,
}: {
  item: UserMessageItem;
  onCancelQueued?: (queuedId: string, text: string) => void;
}) {
  const queued = item.queued;

  return (
    <div className="group flex justify-end">
      <div className="flex max-w-[82%] flex-col items-end gap-1">
        <div
          className={cn(
            "whitespace-pre-wrap break-words rounded-[14px_14px_5px_14px] border px-[15px] py-[11px] text-[13.5px] leading-[1.55]",
            queued
              ? "border-dashed border-border/50 bg-muted/40 text-muted-foreground opacity-70"
              : "border-border/60 bg-card text-foreground",
          )}
        >
          {item.text}
        </div>
        {queued ? (
          <div className="flex items-center gap-1.5 pr-0.5">
            <span className="rounded-full bg-muted px-2 py-[1px] text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
              Queued
            </span>
            {onCancelQueued ? (
              <button
                type="button"
                aria-label="Cancel queued message"
                onClick={() => onCancelQueued(queued.queuedId, item.text)}
                className="flex items-center gap-0.5 rounded text-[10px] text-muted-foreground opacity-0 transition-opacity hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
              >
                <X className="h-3 w-3" aria-hidden />
                Cancel
              </button>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
});
