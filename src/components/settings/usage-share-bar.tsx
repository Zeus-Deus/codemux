import type { CategoryCost } from "@/tauri/commands";
import { Eyebrow } from "@/components/ui/eyebrow";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export interface ShareSegment {
  label: string;
  value: number;
  color: string;
}

/** A neutral step between background and foreground, so a type split never
 *  borrows a provider's series color. */
const ink = (percent: number) =>
  `color-mix(in oklab, var(--foreground) ${percent}%, var(--background))`;

/** Adjacent segments stay visibly distinct in both themes. */
const TYPE_COLORS = {
  input: ink(60),
  cacheRead: ink(30),
  cacheWrite: ink(76),
  output: ink(100),
  other: ink(44),
};

export function costTypeSegments(cost: CategoryCost): ShareSegment[] {
  return [
    { label: "Input", value: cost.input, color: TYPE_COLORS.input },
    { label: "Cache read", value: cost.cache_read, color: TYPE_COLORS.cacheRead },
    { label: "Cache write", value: cost.cache_write, color: TYPE_COLORS.cacheWrite },
    { label: "Output", value: cost.output, color: TYPE_COLORS.output },
    // Provider-reported cost for a model with no rates to split it by.
    // Below a cent it is rounding, not usage.
    {
      label: "Unsplit",
      value: cost.unsplit >= 0.005 ? cost.unsplit : 0,
      color: TYPE_COLORS.other,
    },
  ];
}

export function tokenTypeSegments(tokens: {
  input_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  output_tokens: number;
}): ShareSegment[] {
  return [
    { label: "Input", value: tokens.input_tokens, color: TYPE_COLORS.input },
    { label: "Cache read", value: tokens.cache_read_tokens, color: TYPE_COLORS.cacheRead },
    { label: "Cache write", value: tokens.cache_write_tokens, color: TYPE_COLORS.cacheWrite },
    { label: "Output", value: tokens.output_tokens, color: TYPE_COLORS.output },
  ];
}

function percent(fraction: number): string {
  return `${Math.round(fraction * 100)}%`;
}

/**
 * One part-to-whole bar with its legend. Empty segments are left out, and
 * nothing renders without a total — an empty bar would read as "zero spent
 * on everything" rather than "nothing to split".
 */
export function UsageShareBar({
  label,
  segments,
  format,
}: {
  label: string;
  segments: ShareSegment[];
  format: (value: number) => string;
}) {
  const visible = segments.filter((segment) => segment.value > 0);
  const total = visible.reduce((sum, segment) => sum + segment.value, 0);
  if (total <= 0) return null;

  return (
    <div className="flex min-w-0 flex-col gap-2.5">
      <Eyebrow>{label}</Eyebrow>
      <div
        role="img"
        aria-label={`${label}: ${visible
          .map((segment) => `${segment.label} ${format(segment.value)}`)
          .join(", ")}`}
        className="flex h-2 gap-0.5"
      >
        {visible.map((segment) => (
          <Tooltip key={segment.label}>
            <TooltipTrigger asChild>
              <div
                className="h-full min-w-1 rounded-[2px] first:rounded-l-full last:rounded-r-full"
                style={{ flex: `${segment.value} 1 0`, backgroundColor: segment.color }}
              />
            </TooltipTrigger>
            <TooltipContent>
              {segment.label} · {format(segment.value)} · {percent(segment.value / total)}
            </TooltipContent>
          </Tooltip>
        ))}
      </div>
      <div className="flex flex-wrap gap-x-4 gap-y-1 text-label">
        {visible.map((segment) => (
          <span key={segment.label} className="inline-flex items-center gap-1.5">
            <span
              aria-hidden
              className="size-2 rounded-[2px]"
              style={{ backgroundColor: segment.color }}
            />
            <span className="text-muted-foreground">{segment.label}</span>
            <span className="select-text font-mono tabular-nums text-foreground">
              {format(segment.value)}
            </span>
          </span>
        ))}
      </div>
    </div>
  );
}
