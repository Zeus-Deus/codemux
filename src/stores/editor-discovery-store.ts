import { useEffect } from "react";
import { create } from "zustand";
import { detectEditors } from "@/tauri/commands";
import type { EditorInfo } from "@/tauri/types";

interface EditorDiscoveryState {
  editors: EditorInfo[] | null;
  loading: boolean;
  error: string | null;
}

const EMPTY_EDITORS: EditorInfo[] = [];
let discoveryInFlight: Promise<EditorInfo[]> | null = null;

export const useEditorDiscoveryStore = create<EditorDiscoveryState>(() => ({
  editors: null,
  loading: false,
  error: null,
}));

/** One process-wide detection request. All title bars, settings views and
 * context menus share both the resolved value and an in-flight promise. */
export function ensureEditorsDetected(options?: {
  force?: boolean;
}): Promise<EditorInfo[]> {
  const force = options?.force === true;
  const current = useEditorDiscoveryStore.getState();
  if (!force && current.editors !== null) return Promise.resolve(current.editors);
  if (discoveryInFlight) return discoveryInFlight;

  useEditorDiscoveryStore.setState({ loading: true, error: null });
  const previousEditors = current.editors;
  const request = detectEditors()
    .then((editors) => {
      useEditorDiscoveryStore.setState({ editors, loading: false, error: null });
      return editors;
    })
    .catch((error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      // Keep a previously successful list on a forced-refresh failure. On the
      // initial attempt leave the cache unresolved (`null`): all consumers of
      // this in-flight promise still receive one shared empty result, while a
      // later-mounted surface can make one fresh attempt instead of treating a
      // transient startup IPC failure as authoritative for renderer lifetime.
      useEditorDiscoveryStore.setState({
        editors: previousEditors,
        loading: false,
        error: message,
      });
      return EMPTY_EDITORS;
    })
    .finally(() => {
      discoveryInFlight = null;
    });
  discoveryInFlight = request;
  return request;
}

/** React view over the shared cache. `load=false` is useful to observe a
 * previously detected list without creating intent on mount. */
export function useDetectedEditors(load = true): EditorInfo[] {
  const editors = useEditorDiscoveryStore((state) => state.editors);
  useEffect(() => {
    if (load) void ensureEditorsDetected();
  }, [load]);
  return editors ?? EMPTY_EDITORS;
}

/** Test-only reset for modules whose process-wide cache spans many cases. */
export function _resetEditorDiscoveryForTests(): void {
  discoveryInFlight = null;
  useEditorDiscoveryStore.setState({
    editors: null,
    loading: false,
    error: null,
  });
}
