/**
 * Composer overlay presentation.
 *
 * The composer region floats over the bottom of the transcript (see
 * `AgentChatPane`). Two consequences every card docked in that region has to
 * opt into:
 *
 *  1. The region itself is `pointer-events-none` so the empty column gutters
 *     beside the cards stay transparent — a click or a text-selection drag
 *     in that dead space belongs to the transcript row behind it. Each
 *     visible card re-enables pointer events on its own box.
 *
 *  2. While the reader is scrolled back off the live edge, the whole cluster
 *     dims so the thread reads through it. The trigger is a
 *     `data-reading-back` attribute stamped on the pane root by the
 *     transcript's scroll listener, so this is pure CSS — flipping it costs
 *     no React render in a subtree that re-renders on every keystroke. The
 *     rules themselves live in `globals.css` under `.composer-overlay-card`.
 *
 * Dimming the card *including its background* is the point: unlike a
 * translucent-fill-plus-blur treatment, it puts the transcript's own text
 * behind the composer at full legibility. Hover or focus anywhere in the
 * card brings it straight back, and pointer events are never disabled — the
 * dimmed composer stays fully clickable and typable.
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
