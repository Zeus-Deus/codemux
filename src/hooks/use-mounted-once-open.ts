import { useState } from "react";

/**
 * True from the first render where `open` is true, and forever after.
 *
 * For overlays rendered behind a store flag: mounting only while the flag is
 * true unmounts the dialog the instant it closes, so its exit animation never
 * plays. Keeping it mounted after the first open lets Radix run the animation
 * and unmount the content itself, while a never-opened overlay still costs
 * nothing (not even its lazy chunk).
 */
export function useMountedOnceOpen(open: boolean): boolean {
  const [mounted, setMounted] = useState(open);
  if (open && !mounted) setMounted(true);
  return mounted || open;
}
