import { useEffect } from "react";
import type { UnlistenFn } from "@/tauri/events";

export function useTauriEvent<T>(
  subscribe: (cb: (payload: T) => void) => Promise<UnlistenFn>,
  handler: (payload: T) => void,
  deps: React.DependencyList = [],
  enabled = true,
) {
  useEffect(() => {
    if (!enabled) return;
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    void (async () => {
      try {
        const fn = await subscribe((payload) => {
          if (!cancelled) handler(payload);
        });
        if (cancelled) fn();
        else unlisten = fn;
      } catch (error) {
        if (!cancelled) console.error("[tauri-event] Failed to register listener:", error);
      }
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, ...deps]);
}
