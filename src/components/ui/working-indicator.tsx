import { Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { AsciiSpinner } from "@/components/ui/ascii-spinner";
import type {
  WorkingIndicatorVariant,
  WorkingIndicatorColor,
} from "@/stores/settings-store";

// Tailwind can't compose class names at runtime, so every token → utility
// pairing is spelled out statically here (the scanner sees each literal).
const TEXT_COLOR: Record<WorkingIndicatorColor, string> = {
  "status-working": "text-status-working",
  foreground: "text-foreground",
  "accent-ember": "text-accent-ember",
  "status-open": "text-status-open",
  "status-remote": "text-status-remote",
  "accent-violet": "text-accent-violet",
};

const BG_COLOR: Record<WorkingIndicatorColor, string> = {
  "status-working": "bg-status-working",
  foreground: "bg-foreground",
  "accent-ember": "bg-accent-ember",
  "status-open": "bg-status-open",
  "status-remote": "bg-status-remote",
  "accent-violet": "bg-accent-violet",
};

// Track color for the sweep bar — the solid tone at /20 opacity.
const TRACK_COLOR: Record<WorkingIndicatorColor, string> = {
  "status-working": "bg-status-working/20",
  foreground: "bg-foreground/20",
  "accent-ember": "bg-accent-ember/20",
  "status-open": "bg-status-open/20",
  "status-remote": "bg-status-remote/20",
  "accent-violet": "bg-accent-violet/20",
};

interface Props {
  variant?: WorkingIndicatorVariant;
  color?: WorkingIndicatorColor;
  /** Larger canvas sizing for the settings tile previews. */
  preview?: boolean;
  className?: string;
}

/**
 * The glyph shown in place of a workspace's leading icon while its agent is
 * working. Renders the user-selected variant + token color (Settings →
 * Appearance → Agents). All variants reuse motion that already exists in the
 * app: the braille `AsciiSpinner`, `Loader2`'s spin, and the shared
 * `cm-blink` / `cm-sweep` keyframes.
 */
export function WorkingIndicator({
  variant = "braille",
  color = "status-working",
  preview = false,
  className,
}: Props) {
  const text = TEXT_COLOR[color];
  const bg = BG_COLOR[color];
  // Decorative in the settings previews; announced in a live row.
  const label = preview ? undefined : "Agent working";

  switch (variant) {
    case "ring":
      return (
        <Loader2
          className={cn(
            "animate-spin",
            text,
            preview ? "size-[15px]" : "size-3.5",
            className,
          )}
          aria-label={label}
        />
      );

    case "blink":
      return (
        <span
          className={cn(
            "cm-blink rounded-full",
            bg,
            preview ? "size-[9px]" : "size-2",
            className,
          )}
          aria-label={label}
        />
      );

    case "sweep":
      // A mini progress bar: a /20 track clipping a colored inner segment
      // that sweeps left→right via `cm-sweep`. Narrower in the row's 20px
      // icon slot so it fits; wider in the settings preview.
      return (
        <span
          className={cn(
            "relative inline-block h-1 overflow-hidden rounded-full",
            TRACK_COLOR[color],
            preview ? "w-6" : "w-[17px]",
            className,
          )}
          aria-label={label}
        >
          <span
            className={cn(
              // 40% wide, so the `cm-sweep` travel (expressed in the
              // segment's own width) is retuned from the 38% defaults:
              // -38% of the track = -95% of the segment, and 100% of the
              // track = 250% of it.
              "cm-sweep absolute top-0 left-0 h-1 w-[40%] rounded-full [--cm-sweep-from:-95%] [--cm-sweep-to:250%]",
              bg,
            )}
          />
        </span>
      );

    case "typing":
      // Three dots blinking on a staggered 0 / .2s / .4s cadence.
      return (
        <span
          className={cn(
            "inline-flex items-center",
            preview ? "gap-[3px]" : "gap-[2px]",
            className,
          )}
          aria-label={label}
        >
          {[0, 0.2, 0.4].map((delay, i) => (
            <span
              key={i}
              className={cn(
                "cm-blink rounded-full",
                bg,
                preview ? "size-[4.5px]" : "size-1",
              )}
              style={{ animationDelay: `${delay}s` }}
            />
          ))}
        </span>
      );

    case "braille":
    default:
      // AsciiSpinner defaults to text-status-working / text-sm; both are
      // overridden here by the trailing color + size classes (tailwind-merge
      // keeps the last utility in each group).
      return (
        <AsciiSpinner
          className={cn(text, preview && "text-[19px]", className)}
        />
      );
  }
}
