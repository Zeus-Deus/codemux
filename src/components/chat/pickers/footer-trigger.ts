/** Shared quiet trigger for the composer footer's session-config
 *  controls (model / reasoning / speed / permission). Borderless ghost
 *  text on the card surface — separation between controls comes from
 *  the hairline pipes interleaved in the footer, not from per-pill
 *  borders, so the row reads as one calm strip instead of a bank of
 *  chips. */
export const FOOTER_TRIGGER =
  "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-lg px-2 text-xs font-medium text-muted-foreground transition-colors outline-none hover:bg-foreground/[0.06] hover:text-foreground disabled:opacity-50";
