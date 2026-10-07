import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

import { CHAT_COLUMN } from "./chat-column";

/**
 * Stand-in transcript while an existing thread's history loads. Without it
 * the empty slice reads as a brand-new chat ("What should we do today?") for
 * the length of the hydrate round-trip, as if the conversation were gone.
 * The shapes echo a prompt bubble and an answer on the same column rails.
 */
export function TranscriptSkeleton() {
  return (
    <div
      role="status"
      aria-label="Loading conversation"
      data-testid="transcript-skeleton"
      className="min-h-0 w-full flex-1 overflow-hidden"
    >
      <div className={cn(CHAT_COLUMN, "flex flex-col gap-2.5 pt-16")}>
        <Skeleton className="ml-auto h-9 w-2/5 bg-surface-2" />
        <Skeleton className="mt-4 h-3 w-11/12 bg-surface-2" />
        <Skeleton className="h-3 w-4/5 bg-surface-2" />
        <Skeleton className="h-3 w-3/5 bg-surface-2" />
      </div>
    </div>
  );
}
