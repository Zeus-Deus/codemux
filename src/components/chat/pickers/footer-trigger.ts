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
 *  The button's fixed 34px height + `items-center` keeps the row geometry
 *  identical either way — the same 34px as the attach / send circles, so
 *  every control sits concentric inside the 44px composer pill. */
export const FOOTER_TRIGGER =
  "inline-flex h-[34px] shrink-0 items-center gap-2 rounded-lg px-2.5 text-body font-medium leading-5 text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground disabled:opacity-50";

/** A model label's leaf name — what the composer's narrow width ladder
 *  shows instead of the full label: the last `/` segment of routed ids
 *  ("openrouter/qwen3-coder" → "qwen3-coder"); plain labels pass through. */
export function leafModelName(label: string): string {
  const leaf = label.split("/").pop()?.trim();
  return leaf ? leaf : label;
}
