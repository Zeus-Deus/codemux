/**
 * Shared chat column rails.
 *
 * The transcript, the docked activity bar, the pending-input panel and the
 * composer card all sit on one centered column so their left/right edges
 * line up at every pane width. The rule is: the horizontal gutter lives
 * *outside* the max-width box, so the effective content width is always
 *
 *   min(760px, paneWidth - 2 * 16px)
 *
 * Tailwind only sees literal class strings, so the two numbers are spelled
 * out here (once) rather than computed. `CHAT_COLUMN` folds the gutter into
 * a single element via `max-w-[792px] px-4` (792 - 2*16 = 760, since the
 * box is border-box); the OUTER/INNER pair is the same rails split across
 * two elements, for containers whose inner box is a visible card.
 */

/** One-element form: content resolves to min(760px, paneWidth - 32px). */
export const CHAT_COLUMN = "mx-auto w-full max-w-[792px] px-4";

/** Two-element form — gutter element. Pair with `CHAT_COLUMN_INNER`. */
export const CHAT_COLUMN_OUTER = "w-full px-4";

/** Two-element form — the centered 760px box itself. */
export const CHAT_COLUMN_INNER = "mx-auto w-full max-w-[760px]";
