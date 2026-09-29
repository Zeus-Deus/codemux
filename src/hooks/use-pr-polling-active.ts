import { useSyncExternalStore } from "react";

/** A visible window left unattended should stop spending the host's quota. */
export const PR_POLL_IDLE_MS = 6 * 60_000;

const listeners = new Set<() => void>();
let active = true;
let stopWatching: (() => void) | undefined;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (!stopWatching) {
    let lastInteraction = Date.now();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const visible = () => document.visibilityState !== "hidden";
    const update = () => {
      const next = visible() && Date.now() - lastInteraction < PR_POLL_IDLE_MS;
      if (next === active) return;
      active = next;
      for (const notify of listeners) notify();
    };
    const scheduleIdle = () => {
      clearTimeout(timer);
      if (visible()) timer = setTimeout(update, PR_POLL_IDLE_MS);
    };
    const onInteraction = () => {
      lastInteraction = Date.now();
      update();
      scheduleIdle();
    };
    const onVisibility = () => {
      if (visible()) lastInteraction = Date.now();
      update();
      scheduleIdle();
    };
    const events = ["pointerdown", "pointermove", "keydown", "wheel"] as const;
    for (const event of events) document.addEventListener(event, onInteraction, { passive: true });
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("focus", onVisibility);
    update();
    scheduleIdle();
    stopWatching = () => {
      clearTimeout(timer);
      for (const event of events) document.removeEventListener(event, onInteraction);
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("focus", onVisibility);
    };
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      stopWatching?.();
      stopWatching = undefined;
      active = true;
    }
  };
}

/** Shared by every PR surface; mounting another observer does not restart the idle clock. */
export function usePrPollingActive(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => active && document.visibilityState !== "hidden",
    () => false,
  );
}
