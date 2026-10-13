import { useLayoutEffect, type ReactNode } from "react";
import { MotionConfig } from "motion/react";
import { selectReduceMotion, useSettingsStore } from "@/stores/settings-store";

/**
 * Applies the in-app "Reduce motion" setting on top of the OS preference.
 *
 * CSS animations are handled by the `reduce-motion` class on <html> (see the
 * reduced-motion block in globals.css). `motion/react` animations are driven
 * from JS and never see that class, so they get the same answer through
 * MotionConfig: "always" when the setting is on, otherwise the OS preference.
 */
export function ReducedMotionProvider({ children }: { children: ReactNode }) {
  const reduceMotion = useSettingsStore(selectReduceMotion);

  useLayoutEffect(() => {
    document.documentElement.classList.toggle("reduce-motion", reduceMotion);
  }, [reduceMotion]);

  return (
    <MotionConfig reducedMotion={reduceMotion ? "always" : "user"}>
      {children}
    </MotionConfig>
  );
}
