/** Shared quiet trigger for the composer footer's session-config
 *  controls (model / reasoning / speed / permission). Borderless ghost
 *  text on the card surface — separation between controls comes from
 *  the hairline pipes interleaved in the footer, not from per-pill
 *  borders, so the row reads as one calm strip instead of a bank of
 *  chips.
 *
 *  13px type with 14px icons keeps the strip light next to the 34px
 *  attach / send circles; the 30px hover plate stays vertically centred
 *  in the 42px row, so it never crowds the pill's edge.
 *
 *  Line height is `leading-5`, not `leading-none`: the labels inside are
 *  `truncate` spans (overflow hidden), and a line box the same height as
 *  the font size clips descenders — "High · 1M" lost the tail of its "g". */
export const FOOTER_TRIGGER =
  "inline-flex h-[30px] shrink-0 items-center gap-1.5 rounded-md px-2 text-body-sm font-medium leading-5 text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground disabled:opacity-50";

/** Hairline pipe between footer controls. */
export const FOOTER_SEPARATOR =
  "mx-0.5 h-3.5 w-px shrink-0 self-center bg-border/60";

/** Popover shell for the footer pickers. Height is bounded by the space
 *  Radix measures between the trigger and the window edge — not a fixed
 *  cap — so a menu that fits on screen shows every row without a
 *  scrollbar, and only scrolls when the window genuinely is too short. */
export const PICKER_CONTENT =
  "flex w-[368px] max-w-[calc(100vw-16px)] max-h-(--radix-popover-content-available-height) flex-col overflow-hidden p-0";

/** Gap kept between a picker menu and the window edge. */
export const PICKER_COLLISION_PADDING = 8;

/** Command root / list overrides that let the list shrink inside
 *  `PICKER_CONTENT` instead of applying cmdk's default fixed max height. */
export const PICKER_COMMAND = "min-h-0";
export const PICKER_LIST = "thin-scrollbar max-h-none min-h-0";

/** Groups drop their own side padding (the Command root already insets
 *  4px) and get smaller, quieter section headings. The width above is
 *  sized so provider blurbs ("Balances speed and reasoning depth for
 *  everyday tasks") sit on one line; longer text wraps instead of
 *  truncating. */
export const PICKER_GROUP =
  "px-0 py-0.5 **:[[cmdk-group-heading]]:px-2 **:[[cmdk-group-heading]]:pt-1.5 **:[[cmdk-group-heading]]:pb-1 **:[[cmdk-group-heading]]:text-caption **:[[cmdk-group-heading]]:text-muted-foreground/70";

/** Two-line option row: title over a muted description, check on the right. */
export const PICKER_ROW = "h-auto items-start gap-2 px-2 py-[7px]";
export const PICKER_ROW_TITLE = "text-label font-medium leading-4 text-foreground";
export const PICKER_ROW_DESCRIPTION =
  "text-label leading-4 text-pretty text-muted-foreground/70";
export const PICKER_ROW_CHECK = "mt-px size-3.5 text-foreground/70";
export const PICKER_SEPARATOR = "mx-0 my-1 bg-border/60";

/** A model label's leaf name — what the composer's narrow width ladder
 *  shows instead of the full label: the last `/` segment of routed ids
 *  ("openrouter/qwen3-coder" → "qwen3-coder"); plain labels pass through. */
export function leafModelName(label: string): string {
  const leaf = label.split("/").pop()?.trim();
  return leaf ? leaf : label;
}
