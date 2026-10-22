/**
 * Motion vocabulary. The CSS side (`ease-drawer`, `--motion-quick/base/slow`)
 * lives in `globals.css`; this is the same set for class strings shared by
 * several primitives and for `motion/react`, which never sees CSS tokens.
 *
 * Reduced motion needs nothing here: CSS durations collapse globally in
 * `globals.css`, and `ReducedMotionProvider` hands `motion/react` the same
 * answer through `MotionConfig`.
 */

/** The decelerating curve for surfaces that open, slide or pop. */
export const EASE_DRAWER = [0.32, 0.72, 0, 1] as const;

/** The 100 / 150 / 250ms ladder, in seconds for `motion/react`. */
export const DURATION = {
  quick: 0.1,
  base: 0.15,
  slow: 0.25,
} as const;

/**
 * Enter/exit for anchored overlays: popovers, menus, hover cards, tooltips.
 * A fade plus a slight scale from the trigger, so pair it with the
 * primitive's `origin-(--radix-…-transform-origin)`. Opening takes `base` on
 * the drawer curve; closing is `quick`, so a dismissed surface never lingers
 * over whatever the user reaches for next. Transform and opacity only.
 */
export const POP_MOTION =
  "data-[state=open]:animate-in data-[state=closed]:animate-out fade-in-0 fade-out-0 zoom-in-97 zoom-out-97 duration-150 ease-drawer data-[state=closed]:duration-100 data-[state=closed]:ease-out";
