import { cn } from "@/lib/utils";
import type { ThemeDefinition, ThemeScheme } from "@/lib/themes";

/**
 * The theme picker's identity mark: the theme's raised surface and its brand
 * accent as two overlapping discs. Small enough for a 40px palette row, and
 * legible at settings-row scale.
 */
export function ThemeCoins({ theme, size }: { theme: ThemeDefinition; size: number }) {
  const disc = { width: size, height: size };
  return (
    // The hairlines are the *applied* theme's ink, not a fixed white and
    // black: a light theme's near-white surface disc needs a dark edge to
    // exist at all on a light page, and a dark one needs a light edge on a
    // dark page. `foreground` flips with the page, so both stay drawn.
    <span className="flex flex-none items-center" aria-hidden="true">
      <span
        className="rounded-full border border-foreground/15"
        style={{ ...disc, background: theme.roles.card }}
      />
      <span
        className="rounded-full border border-foreground/30"
        style={{ ...disc, background: theme.roles.brandAccent, marginLeft: -Math.round(size * 0.36) }}
      />
    </span>
  );
}

/**
 * A theme's scheme, said out loud.
 *
 * Light is a property of a theme rather than a switch beside the picker, so
 * the only place it can be read is on the theme itself — quiet enough to
 * ignore when every row says the same thing, and there when one doesn't.
 *
 * Takes the scheme rather than a theme so the Marketplace list, which only
 * has what an extension's manifest declared, prints the same badge.
 */
export function ThemeSchemeBadge({
  scheme,
  className,
}: {
  scheme: ThemeScheme;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "flex-none rounded-full border border-border/70 px-1.5 py-px font-mono text-micro tracking-[0.06em] text-muted-foreground/70 uppercase",
        className,
      )}
    >
      {scheme}
    </span>
  );
}

/** The four ANSI hues a theme row shows as proof the terminal changes too. */
export function ThemeAnsiDots({ theme }: { theme: ThemeDefinition }) {
  return (
    <span className="flex flex-none gap-[3px]" aria-hidden="true">
      {[theme.ansi.green, theme.ansi.yellow, theme.ansi.cyan, theme.ansi.magenta].map(
        (color, index) => (
          <span
            key={`${color}-${index}`}
            className="size-[9px] rounded-[3px]"
            style={{ background: color }}
          />
        ),
      )}
    </span>
  );
}
