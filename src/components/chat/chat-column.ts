/**
 * Shared chat column rails.
 *
 * The transcript, the docked activity bar, the pending-input panel and the
 * composer card all sit on one centered column so their left/right edges
 * line up at every pane width. The rule is: the horizontal gutter lives
 * *outside* the max-width box, so the effective content width is always
 *
 *   min(47.5rem, paneWidth - 2 * 1rem)   (760px at the default 16px root)
 *
 * The widths are rem so the column grows with the interface size setting
 * instead of squeezing larger text into a fixed pixel box.
 *
 * Tailwind only sees literal class strings, so the two numbers are spelled
 * out here (once) rather than computed. `CHAT_COLUMN` folds the gutter into
 * a single element via `max-w-[49.5rem] px-4` (49.5 - 2*1 = 47.5, since the
 * box is border-box); the OUTER/INNER pair is the same rails split across
 * two elements, for containers whose inner box is a visible card.
 */

/** One-element form: content resolves to min(47.5rem, paneWidth - 2rem). */
export const CHAT_COLUMN = "mx-auto w-full max-w-[49.5rem] px-4";

/** Two-element form — gutter element. Pair with `CHAT_COLUMN_INNER`. */
export const CHAT_COLUMN_OUTER = "w-full px-4";

/** Two-element form — the centered 47.5rem box itself. */
export const CHAT_COLUMN_INNER = "mx-auto w-full max-w-[47.5rem]";
