/** Shared quiet trigger for the composer footer's session-config
 *  controls (model / reasoning / speed / permission). Borderless ghost
 *  text on the card surface — separation between controls comes from
 *  the hairline pipes interleaved in the footer, not from per-pill
 *  borders, so the row reads as one calm strip instead of a bank of
 *  chips.
 *
 *  Line height is `leading-5`, not `leading-none`: the labels inside are
 *  `truncate` spans (overflow hidden), and a line box the same height as
 *  the font size clips descenders — "High · 1M" lost the tail of its "g".
 *  The button's fixed `h-8` + `items-center` keeps the row geometry
 *  identical either way. */
export const FOOTER_TRIGGER =
  "inline-flex h-8 shrink-0 items-center gap-2 rounded-lg px-2.5 text-sm font-medium leading-5 text-muted-foreground transition-colors outline-none hover:bg-foreground/[0.06] hover:text-foreground disabled:opacity-50";
