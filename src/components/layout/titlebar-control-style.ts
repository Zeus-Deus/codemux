/**
 * Shared look for the controls living in the floating title bar band.
 *
 * The band used to be three visual families at once: bordered chips for
 * Run and the editor launcher, 10px-radius pills for the tabs and the
 * pinned preset tiles, and borderless 8px icon buttons for the panel
 * toggle / sidebar toggle / launcher. The mock collapses that into one
 * family — every control 28px tall on the same radius, most of them
 * carrying no border or resting fill at all, so the band reads as a row
 * of affordances floating over the content rather than a toolbar.
 *
 * The panel toggle (`PaneActionButton size="titlebar"`) and every 28px
 * icon `Button size="icon-sm"` already used these exact values, so the
 * constants below are named re-exports of that existing look rather than
 * a new one. Keeping them as tokens (not a hardcoded pixel radius) is
 * what lets a theme preset re-skin `--radius` and re-skin the whole band
 * with it.
 */

/** 28px controls' corner radius — matches `Button size="icon-sm"`. */
export const BAND_CONTROL_RADIUS = "rounded-[min(var(--radius-md),12px)]";

/**
 * Resting-transparent control: no border, no fill, a quiet hover fill
 * only. The hover step of the surface ladder (`surface-2`), so hover in
 * the band is the same jump as hover in the right-panel strip beside it.
 */
export const BAND_CONTROL_HOVER =
  "transition-colors duration-100 hover:bg-surface-2 hover:text-foreground";

/**
 * The "this one is selected" fill, used by the active tab pill and its
 * draft-mode twin. The selected step of the surface ladder (`surface-3`),
 * the same fill the right-panel strip gives its active tab: the main tabs
 * and the panel tabs share one 40px band, so "selected" has to read the
 * same in both, and it has to sit one step above hover. Still a neutral
 * translucent ink, so it separates the active tab from its neighbours
 * without drawing an opaque box over the transcript behind it.
 */
export const BAND_ACTIVE_FILL = "bg-surface-3";
