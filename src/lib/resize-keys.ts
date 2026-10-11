/**
 * Keyboard control for resize handles, following the WAI-ARIA window
 * splitter pattern: arrows move the seam, Shift moves it further, and
 * Home/End jump to the primary side's minimum and maximum size.
 */

export const RESIZE_KEY_STEP_PX = 16;
export const RESIZE_KEY_STEP_LARGE_PX = 64;

export type ResizeKeyAction =
  | { kind: "move"; px: number }
  | { kind: "min" }
  | { kind: "max" };

/**
 * Maps a key pressed on a focused separator to a move, or null for keys the
 * separator does not own. `px` is signed in screen direction: positive is
 * right for a vertical separator and down for a horizontal one, so each
 * handle decides whether that grows or shrinks the side it sizes.
 */
export function resizeKeyAction(
  event: { key: string; shiftKey: boolean },
  orientation: "vertical" | "horizontal",
): ResizeKeyAction | null {
  if (event.key === "Home") return { kind: "min" };
  if (event.key === "End") return { kind: "max" };
  const step = event.shiftKey ? RESIZE_KEY_STEP_LARGE_PX : RESIZE_KEY_STEP_PX;
  const [back, forward] =
    orientation === "vertical"
      ? ["ArrowLeft", "ArrowRight"]
      : ["ArrowUp", "ArrowDown"];
  if (event.key === back) return { kind: "move", px: -step };
  if (event.key === forward) return { kind: "move", px: step };
  return null;
}
