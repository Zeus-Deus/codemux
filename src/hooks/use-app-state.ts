import { useEffect, useRef, useCallback } from "react";
import { getAppState } from "@/tauri/commands";
import { onAppStateChanged } from "@/tauri/events";
import { useAppStore } from "@/stores/app-store";
import { useTauriEvent } from "./use-tauri-event";
import type { AppStateSnapshot } from "@/tauri/types";

export function useAppStateInit(skip = false) {
  const setAppState = useAppStore((s) => s.setAppState);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Fetch initial state on mount (skip if not authenticated yet)
  useEffect(() => {
    if (skip) return;
    getAppState()
      .then((snapshot) => {
        setAppState(snapshot);
      })
      .catch((err) => console.error("Failed to fetch app state:", err));
  }, [setAppState, skip]);

  // Subscribe to state changes with a 16ms debounce. The backend already
  // coalesces app-state mutations, so the previous full-snapshot
  // JSON.stringify dedup was paying a multi-KB serialize cost on every
  // tick to catch the rare identical-payload case. Dropping it lets
  // Zustand's selector-level reference equality at the component layer
  // handle no-op fan-out — workspace state can grow with each opened
  // surface/pane/session, so the stringify scaled with usage and was
  // a real freeze contributor under sustained backend churn.
  //
  // Reference stability is now restored *without* stringifying: `setAppState`
  // (src/stores/app-store.ts) runs `shareStructural(prev, next)`, a single
  // O(n) structural-sharing walk that allocates NO strings and reuses prev
  // references for every unchanged subtree. It runs at most once per this
  // 16ms debounce window — distinct from the removed stringify dedup, which
  // serialized the whole snapshot to a multi-KB string on every tick.
  const handleStateChanged = useCallback(
    (payload: AppStateSnapshot) => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        setAppState(payload);
      }, 16);
    },
    [setAppState],
  );

  useTauriEvent(onAppStateChanged, handleStateChanged, [handleStateChanged]);
}
