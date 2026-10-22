import {
  AnimatePresence,
  motion,
  useIsPresent,
  useReducedMotionConfig,
} from "motion/react";
import { createContext, useContext, useRef, type ReactNode } from "react";

import { DURATION, EASE_DRAWER } from "@/lib/motion";

/**
 * Enter/exit for rows in a short dynamic list (sidebar shelves, project
 * rows). Wrap the mapped rows in `PresenceList` and each keyed row in
 * `PresenceRow`: a row that joins opens its own height and fades in, so the
 * rows below slide down instead of jumping; a row that leaves fades and
 * closes the gap. Rows already there on mount don't animate.
 *
 * Pass `animate={false}` while the list is still hydrating, so rows that
 * arrive or leave as stored state loads don't play at boot; only changes
 * after that animate.
 *
 * Height is animated, so keep it to lists of a few dozen rows. Not for
 * virtualized lists or rows that carry their own drag handling.
 */
const PresenceAnimateContext = createContext(true);

export function PresenceList({
  animate = true,
  children,
}: {
  animate?: boolean;
  children: ReactNode;
}) {
  return (
    <PresenceAnimateContext.Provider value={animate}>
      <AnimatePresence initial={false}>{children}</AnimatePresence>
    </PresenceAnimateContext.Provider>
  );
}

export function PresenceRow({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  // Frozen once the row starts leaving: a row removed by the same update
  // that finishes hydration must leave as it was rendered, without an exit.
  // `MotionConfig`'s reduced motion only stops transforms; height and
  // opacity would still play, so reduced motion opts the row out entirely.
  const reduceMotion = useReducedMotionConfig();
  const listAnimate = useContext(PresenceAnimateContext) && !reduceMotion;
  const isPresent = useIsPresent();
  const animateRef = useRef(listAnimate);
  if (isPresent) animateRef.current = listAnimate;
  const animate = animateRef.current;
  return (
    <motion.div
      data-slot="presence-row"
      className={className}
      initial={animate ? { height: 0, opacity: 0, overflow: "hidden" } : false}
      animate={{
        height: "auto",
        opacity: 1,
        transitionEnd: { overflow: "visible" },
      }}
      exit={
        animate
          ? {
              height: 0,
              opacity: 0,
              overflow: "hidden",
              transition: { duration: DURATION.quick, ease: "easeOut" },
            }
          : undefined
      }
      transition={{ duration: DURATION.base, ease: EASE_DRAWER }}
    >
      {children}
    </motion.div>
  );
}
