import { cn } from "@/lib/utils";

/**
 * Small inline cursor shown at the tail of a streaming assistant
 * message. Neutral only — never colored, never a spinner.
 */
export function StreamingIndicator({ className }: { className?: string }) {
  return (
    <span
      aria-hidden
      className={cn(
        "inline-block h-[0.9em] w-[2px] translate-y-[0.15em] bg-muted-foreground/70",
        "animate-[pulse_1.2s_ease-in-out_infinite]",
        className,
      )}
    />
  );
}
