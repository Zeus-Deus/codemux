/**
 * Cards in the floating composer region remain opaque while reading history.
 * The transcript's viewport mask dissolves text before the docked controls;
 * the shared card backdrop protects them when masks are disabled too.
 *
 * The region itself is pointer-events-none, so empty column gutters still
 * belong to the transcript. Each visible card re-enables pointer events on
 * its own box. Reading-back geometry remains independent of presentation.
 */
export const COMPOSER_OVERLAY_CARD =
  "pointer-events-auto composer-overlay-card";

/** How far off the bottom counts as "reading back". Small on purpose — the
 *  composer should get out of the way as soon as the reader deliberately
 *  leaves the live edge — but above the couple of pixels of slack that
 *  sub-pixel layout and the end-pin correction leave behind, so a settled
 *  transcript at the tail never sits dimmed. */
export const READING_BACK_THRESHOLD_PX = 24;

/** Whether a viewport's geometry counts as scrolled back off the live edge.
 *  Pure so the threshold is testable without a layout engine. */
export function isReadingBack(geometry: {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}): boolean {
  const distance =
    geometry.scrollHeight - geometry.scrollTop - geometry.clientHeight;
  return distance > READING_BACK_THRESHOLD_PX;
}
