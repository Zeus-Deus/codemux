import { useEffect } from "react";
import type { UnlistenFn } from "@/tauri/events";

export function useTauriEvent<T>(
  subscribe: (cb: (payload: T) => void) => Promise<UnlistenFn>,
  handler: (payload: T) => void,
  deps: React.DependencyList = [],
) {
  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    subscribe(handler).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}
