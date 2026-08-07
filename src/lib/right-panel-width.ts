/**
 * How wide the right panel is allowed to get.
 *
 * The panel used to cap at a flat 500px, which was fine when it only ever
 * held a file tree or a diff. The deck now hosts panes that genuinely want
 * the room — a browser most of all — so the cap is expressed as a fraction
 * of the space the panel is actually competing for rather than a pixel
 * number that means "half the window" on a laptop and "a sliver" on a 4K
 * display.
 *
 * Two rules, whichever binds first:
 *   - the panel may take at most {@link RIGHT_PANEL_MAX_FRACTION} of the
 *     available width, and
 *   - it must leave {@link MIN_CONTENT_WIDTH} for the chat/terminal side,
 *     so dragging can never squeeze the conversation to nothing.
 *
 * `available` is the width of the row the panel shares with the workspace
 * content — *not* `window.innerWidth`. The left sidebar is separately
 * resizable and sits outside that row, so measuring the window would let a
 * wide sidebar plus a wide panel crush the content between them.
 */

/**
 * Never narrower than this — below it the tab strip stops being usable.
 *
 * It used to be 240, back when the panel's tab row had the panel's full
 * width to itself. That row is now the window's titlebar band, so its right
 * end is spoken for by the fixed panel cluster and the native window buttons
 * drawn above it — `topRightReserve()` in `@/lib/titlebar-geometry`, 166px on
 * desktop. At 240 that left about 60px for the tabs *and* the active pane's
 * actions, i.e. no visible tab at all. 360 leaves roughly one full tab plus
 * stubs beside a pane's controls, which is the least this row can do and
 * still be a tab row.
 *
 * Deliberately larger than {@link MIN_CONTENT_WIDTH} now: the two floors stop
 * meaning the same thing. The chat side's floor is about *reading*; this one
 * is about the panel's own chrome fitting.
 */
export const RIGHT_PANEL_MIN_WIDTH = 360;

/** The panel's share of the content row when the fraction rule binds. */
export const RIGHT_PANEL_MAX_FRACTION = 0.75;

/**
 * Breathing room reserved for the chat/terminal side of the split.
 *
 * This is a *reading* floor — how narrow the conversation may get before
 * dragging has to stop — which is why it stayed at 240 when
 * {@link RIGHT_PANEL_MIN_WIDTH} rose to fit the panel's chrome. It keeps the
 * fraction rule the one that binds at any realistic window size: at a 1280px
 * window with the sidebar open the row is ~990px, and 75% of that still
 * clears this floor.
 */
export const MIN_CONTENT_WIDTH = 240;

/**
 * A generous absolute ceiling for the *stored* width.
 *
 * The store deliberately keeps the width the user asked for rather than the
 * width that fits right now: a panel dragged to 1400px on an external
 * monitor must still be 1400px when that monitor comes back, not
 * permanently shrunk because the app happened to launch on a laptop screen
 * once. {@link clampRightPanelWidth} applies the real, layout-aware limit at
 * render time. This bound only stops a corrupt persisted value from
 * producing an absurd inline style.
 */
export const RIGHT_PANEL_MAX_STORED_WIDTH = 4000;

/** The widest the panel may render given the space it has to share. */
export function maxRightPanelWidth(available: number): number {
  if (!Number.isFinite(available) || available <= 0) {
    return RIGHT_PANEL_MAX_STORED_WIDTH;
  }
  return Math.max(
    RIGHT_PANEL_MIN_WIDTH,
    Math.min(
      available * RIGHT_PANEL_MAX_FRACTION,
      available - MIN_CONTENT_WIDTH,
    ),
  );
}

/**
 * Clamp a desired panel width to what actually fits.
 *
 * Called at render time and during a drag, never on the way into the store —
 * see {@link RIGHT_PANEL_MAX_STORED_WIDTH} for why the stored value stays
 * un-narrowed.
 */
export function clampRightPanelWidth(
  width: number,
  available: number,
): number {
  if (!Number.isFinite(width)) return RIGHT_PANEL_MIN_WIDTH;
  return Math.max(
    RIGHT_PANEL_MIN_WIDTH,
    Math.min(maxRightPanelWidth(available), width),
  );
}
