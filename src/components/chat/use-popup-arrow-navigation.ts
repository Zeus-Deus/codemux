import { useCallback, useEffect, useRef } from "react";

type ArrowKey = "ArrowUp" | "ArrowDown";

const STALE_REPEAT_MS = 100;

/** Coalesce OS repeats, rather than rendering once for every queued key event. */
export function usePopupArrowNavigation(
  enabled: boolean,
  move: (direction: -1 | 1) => void,
) {
  const moveRef = useRef(move);
  moveRef.current = move;
  const heldKeyRef = useRef<ArrowKey | null>(null);
  const frameRef = useRef<number | null>(null);

  const cancel = useCallback(() => {
    heldKeyRef.current = null;
    if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
    frameRef.current = null;
  }, []);

  useEffect(() => {
    if (!enabled) {
      cancel();
      return;
    }
    const release = (event: KeyboardEvent) => {
      if (event.key === heldKeyRef.current) cancel();
    };
    const hide = () => {
      if (document.hidden) cancel();
    };
    // Capture release before any other application key handlers run.
    window.addEventListener("keyup", release, true);
    window.addEventListener("blur", cancel);
    document.addEventListener("visibilitychange", hide);
    return () => {
      cancel();
      window.removeEventListener("keyup", release, true);
      window.removeEventListener("blur", cancel);
      document.removeEventListener("visibilitychange", hide);
    };
  }, [enabled, cancel]);

  const navigate = ({ key, repeat, timeStamp }: { key: ArrowKey; repeat: boolean; timeStamp: number }) => {
    if (!enabled) return;
    if (!repeat) {
      cancel();
      heldKeyRef.current = key;
      moveRef.current(key === "ArrowDown" ? 1 : -1);
      return;
    }
    // WebKitGTK delivers key events one at a time, so when frames are slow
    // repeats back up and keep arriving long after release (the keyup waits
    // behind them). Dropping repeats older than a few frames drains that
    // backlog, so movement tracks what the webview can paint.
    if (performance.now() - timeStamp > STALE_REPEAT_MS) return;
    // Late repeats after release/blur must not start navigation again.
    if (heldKeyRef.current !== key || frameRef.current !== null) return;
    frameRef.current = requestAnimationFrame(() => {
      frameRef.current = null;
      if (heldKeyRef.current === key) moveRef.current(key === "ArrowDown" ? 1 : -1);
    });
  };

  return { navigate, cancel };
}
