import { cn } from "@/lib/utils";
import type { ThemeDefinition } from "@/lib/themes";

/**
 * A miniature of the app wearing `theme` — sidebar rail, canvas, one raised
 * surface and three palette dots. Shared by the Theme Studio's live preview
 * and anywhere else a theme has to be shown without applying it.
 *
 * Every color here is read off the theme object rather than a token, because
 * the whole point is to render a palette that is *not* the active one.
 */
export function ThemeSwatches({
  theme,
  compact = false,
}: {
  theme: ThemeDefinition;
  compact?: boolean;
}) {
  return (
    <div
      className={cn(
        "relative overflow-hidden rounded-lg border shadow-inner",
        compact ? "h-12" : "h-16",
      )}
      style={{
        background: theme.roles.background,
        borderColor: theme.roles.border,
      }}
      aria-hidden="true"
    >
      <div
        className="absolute inset-x-2 top-2 bottom-2 rounded-md border"
        style={{ background: theme.roles.card, borderColor: theme.roles.border }}
      />
      <div
        className="absolute top-0 bottom-0 left-0 w-[27%] border-r"
        style={{ background: theme.roles.sidebar, borderColor: theme.roles.sidebarBorder }}
      />
      <div className="absolute right-2 bottom-2 flex gap-1.5">
        {[theme.roles.brandAccent, theme.ansi.green, theme.ansi.yellow].map((color) => (
          <span
            key={color}
            className="h-3.5 w-3.5 rounded-full border border-background/40 shadow-sm"
            style={{ background: color }}
          />
        ))}
      </div>
    </div>
  );
}

/**
 * The theme picker's identity mark: the theme's raised surface and its brand
 * accent as two overlapping discs. Small enough for a 40px palette row, and
 * legible at settings-row scale.
 */
export function ThemeCoins({ theme, size }: { theme: ThemeDefinition; size: number }) {
  const disc = { width: size, height: size };
  return (
    <span className="flex flex-none items-center" aria-hidden="true">
      <span
        className="rounded-full border border-white/15"
        style={{ ...disc, background: theme.roles.card }}
      />
      <span
        className="rounded-full border border-black/40"
        style={{ ...disc, background: theme.roles.brandAccent, marginLeft: -Math.round(size * 0.36) }}
      />
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
