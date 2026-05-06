import { cn } from "@/lib/utils";

/**
 * Transcript-tail affordance shown between a user action and the
 * agent's first visible response (prose delta, tool call, approval
 * request). Three small dots with a staggered opacity wave — neutral
 * color, no spinner, no label — so it reads as "the agent is working"
 * without competing with any prose for attention.
 *
 * Distinct from `StreamingIndicator` (the inline cursor bar that sits
 * at the tail of a streaming assistant message). That one is about
 * "text is flowing"; this one is about "something is being composed,
 * nothing visible yet." Keeping them separate lets each stay tuned to
 * its semantic: a 2px cursor reads fine next to streaming prose but is
 * hard to spot as a standalone tail marker.
 */
export function ThinkingIndicator({ className }: { className?: string }) {
  return (
    <span
      aria-hidden
      className={cn("inline-flex items-center gap-1", className)}
    >
      <span className="inline-block h-1.5 w-1.5 rounded-full bg-muted-foreground animate-[cm-thinking-wave_1.2s_ease-in-out_infinite]" />
      <span className="inline-block h-1.5 w-1.5 rounded-full bg-muted-foreground animate-[cm-thinking-wave_1.2s_ease-in-out_infinite] [animation-delay:160ms]" />
      <span className="inline-block h-1.5 w-1.5 rounded-full bg-muted-foreground animate-[cm-thinking-wave_1.2s_ease-in-out_infinite] [animation-delay:320ms]" />
    </span>
  );
}
