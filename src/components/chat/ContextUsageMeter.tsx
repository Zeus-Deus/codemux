import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  deriveContextUsageDisplay,
  formatContextPercentage,
  formatContextTokens,
} from "@/lib/agent-chat/context-usage";
import { cn } from "@/lib/utils";
import type { ContextUsageSnapshot } from "@/tauri/events";

/** Percentage past which the ring and bar turn to the danger token —
 *  the point where a turn is plausibly one long tool result away from
 *  hitting the wall. */
const WARN_PERCENT = 90;

/** Donut geometry. `r` and the stroke width are tuned so a `size-5`
 *  ring sits optically level with the send button's icon weight. */
const RING_RADIUS = 9.75;
const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;

interface Props {
  /** Latest snapshot for the thread. `null` renders nothing at all —
   *  a thread with no session (or no usage report yet) must not make
   *  the composer reserve space for a meter that may never appear. */
  usage: ContextUsageSnapshot | null;
  /** Capability-registry window size, used only until the provider
   *  reports its own. */
  seedMaxTokens?: number | null;
  /** Display name of the agent for the auto-compaction note. Falls
   *  back to a generic phrase when the provider is unknown. */
  providerLabel?: string | null;
}

/**
 * Context-window occupancy indicator for the composer.
 *
 * A donut ring sized to the send button, click-opening a popover with
 * the exact numbers. The ring is deliberately quiet — this is ambient
 * telemetry, not an alert — until usage crosses {@link WARN_PERCENT},
 * where it recolours to the danger token.
 *
 * Everything degrades around an unknown window: no percentage, no
 * bar, ring parked at empty, and the readout falls back to a bare
 * token count. We never divide by a guessed denominator.
 */
export function ContextUsageMeter({
  usage,
  seedMaxTokens = null,
  providerLabel = null,
}: Props) {
  const {
    usedTokens,
    maxTokens,
    usedPercentage,
    totalProcessedTokens,
    compactsAutomatically,
  } = deriveContextUsageDisplay(usage, seedMaxTokens);

  if (!usage) return null;

  const percentText = formatContextPercentage(usedPercentage);
  const usedText = formatContextTokens(usedTokens);
  const maxText = maxTokens !== null ? formatContextTokens(maxTokens) : null;
  const warning = usedPercentage !== null && usedPercentage > WARN_PERCENT;

  // No window ⇒ nothing to fill. An empty ring reads as "unknown"
  // far better than a full one would.
  const fillRatio = usedPercentage !== null ? usedPercentage / 100 : 0;
  const dashOffset = RING_CIRCUMFERENCE * (1 - fillRatio);

  const label =
    percentText !== null
      ? `Context window ${percentText} used`
      : `Context window ${usedText} tokens used`;
  const readout =
    percentText !== null && maxText !== null
      ? `${percentText} · ${usedText}/${maxText}`
      : usedText;

  return (
    <TooltipProvider delayDuration={250}>
      <Popover>
        <Tooltip>
          <TooltipTrigger asChild>
            <PopoverTrigger asChild>
              <button
                type="button"
                data-testid="context-usage-trigger"
                aria-label={label}
                className={cn(
                  // Same circle diameter as the attach / send buttons
                  // so the footer's right cluster stays on one optical
                  // baseline.
                  "inline-flex h-8 w-8 items-center justify-center rounded-full",
                  "text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground",
                  "outline-none focus-visible:ring-1 focus-visible:ring-ring",
                  warning && "text-danger hover:text-danger",
                )}
              >
                <svg
                  viewBox="0 0 24 24"
                  className="size-5 -rotate-90"
                  aria-hidden="true"
                  focusable="false"
                >
                  <circle
                    cx="12"
                    cy="12"
                    r={RING_RADIUS}
                    fill="none"
                    strokeWidth="3"
                    className="stroke-current text-muted-foreground/25"
                  />
                  <circle
                    cx="12"
                    cy="12"
                    r={RING_RADIUS}
                    fill="none"
                    strokeWidth="3"
                    strokeLinecap="round"
                    stroke="currentColor"
                    strokeDasharray={RING_CIRCUMFERENCE}
                    strokeDashoffset={dashOffset}
                    className="transition-[stroke-dashoffset] duration-500 ease-out motion-reduce:transition-none"
                  />
                </svg>
              </button>
            </PopoverTrigger>
          </TooltipTrigger>
          <TooltipContent side="top" sideOffset={6}>
            {percentText !== null
              ? `Context window · ${percentText} used`
              : `Context window · ${usedText} tokens used`}
          </TooltipContent>
        </Tooltip>

        <PopoverContent side="top" align="end" className="w-64 p-3">
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-[11px] font-medium text-muted-foreground">
              Context Window
            </span>
            <span
              className="font-mono text-[11px] tabular-nums text-foreground"
              data-testid="context-usage-readout"
            >
              {readout}
            </span>
          </div>

          {usedPercentage !== null && (
            <div
              className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-muted/60"
              role="progressbar"
              aria-label="Context window used"
              aria-valuenow={Math.round(usedPercentage)}
              aria-valuemin={0}
              aria-valuemax={100}
            >
              <div
                className={cn(
                  "h-full rounded-full transition-[width] duration-300",
                  warning ? "bg-danger/80" : "bg-foreground/40",
                )}
                style={{ width: `${usedPercentage}%` }}
              />
            </div>
          )}

          {totalProcessedTokens !== null && (
            <div
              className="mt-2.5 flex items-baseline justify-between gap-2"
              data-testid="context-usage-total-processed"
            >
              <span className="text-[11px] text-muted-foreground">
                Total processed
              </span>
              <span className="font-mono text-[11px] tabular-nums text-foreground">
                {formatContextTokens(totalProcessedTokens)}
              </span>
            </div>
          )}

          {compactsAutomatically && (
            <p className="mt-2.5 text-[11px] leading-snug text-muted-foreground">
              {providerLabel ?? "The agent"} automatically compacts its context
              when needed.
            </p>
          )}
        </PopoverContent>
      </Popover>
    </TooltipProvider>
  );
}
