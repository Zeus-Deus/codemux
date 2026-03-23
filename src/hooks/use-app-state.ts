import { useEffect, useRef, useCallback } from "react";
import { getAppState } from "@/tauri/commands";
import { onAppStateChanged } from "@/tauri/events";
import { useAppStore } from "@/stores/app-store";
import { useTauriEvent } from "./use-tauri-event";
import type { AppStateSnapshot } from "@/tauri/types";

export function useAppStateInit() {
  const setAppState = useAppStore((s) => s.setAppState);
  const lastJsonRef = useRef("");
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Fetch initial state on mount
  useEffect(() => {
    getAppState()
      .then((snapshot) => {
        lastJsonRef.current = JSON.stringify(snapshot);
        setAppState(snapshot);
      })
      .catch((err) => console.error("Failed to fetch app state:", err));
  }, [setAppState]);

  // Subscribe to state changes with 16ms debounce + JSON dedup
  const handleStateChanged = useCallback(
    (payload: AppStateSnapshot) => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        const json = JSON.stringify(payload);
        if (json === lastJsonRef.current) return;
        lastJsonRef.current = json;
        setAppState(payload);
      }, 16);
    },
    [setAppState],
  );

  useTauriEvent(onAppStateChanged, handleStateChanged, [handleStateChanged]);
}
