/** The chat composer's surface, shared by every box that should read as the
 *  same composer (the main thread's and the `/btw` side chat's). */

/** Composer pill / card: 22px radius in both states (a 44px pill is a
 *  stadium; the expanded card keeps the same corner), a hairline border,
 *  and a deep low-opacity shadow so the card lifts off the pane background.
 *  The border stays identical at rest and on focus; focus comes from
 *  surface and elevation. The surface is the muted tint pre-composited over
 *  the pane background: opaque, so the strip's seam never shows through.
 *  Geometry and colour split so the tint stays a themeable utility. Only
 *  the properties that change transition, so a re-render costs nothing. */
export const COMPOSER_CARD =
  "rounded-[22px] border border-border/80 bg-[color-mix(in_oklab,var(--muted)_40%,var(--background))] " +
  "shadow-[0_12px_28px_-18px] shadow-black/40 " +
  "transition-[box-shadow,border-color,background-color] duration-150 " +
  "focus-within:bg-[color-mix(in_oklab,var(--muted)_60%,var(--background))] focus-within:shadow-[0_16px_38px_-14px] focus-within:shadow-black/60";

/** The strips docked above and below the card: one tonal step under the
 *  card, pre-composited over the pane background like the card itself. It
 *  stays opaque where nothing else backs it, so whatever sits behind the
 *  composer (the landing's backdrop art) never shows through the labels. */
export const COMPOSER_STRIP_SURFACE =
  "bg-[color-mix(in_oklab,var(--muted)_20%,var(--background))]";

/** 34px circle shared by attach / send / stop so the row's two ends sit on
 *  one optical baseline, concentric with the 44px pill (5px inset). */
export const COMPOSER_ROUND_CONTROL =
  "inline-flex h-[34px] w-[34px] shrink-0 items-center justify-center rounded-full";

/** The primary send circle's fill, motion and disabled state. */
export const COMPOSER_SEND =
  "bg-primary/90 text-primary-foreground shadow-xs shadow-primary/25 " +
  "transition-[transform,background-color,color] duration-150 hover:scale-105 hover:bg-primary active:scale-100 " +
  "disabled:opacity-30 disabled:shadow-none disabled:cursor-not-allowed disabled:hover:scale-100";
