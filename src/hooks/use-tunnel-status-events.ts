import { useCallback } from "react";

import { useTauriEvent } from "./use-tauri-event";
import { onTunnelStatusChanged, type TunnelStatusPayload } from "@/tauri/events";
import { useTunnelStatusStore } from "@/stores/tunnel-status-store";

/**
 * Bridges the backend `tunnel-status-changed` event into the
 * tunnel-status store. Mount once at the app root (next to
 * `useAuthEvents`). See `src/stores/tunnel-status-store.ts`.
 */
export function useTunnelStatusEvents() {
  const handle = useCallback((payload: TunnelStatusPayload) => {
    useTunnelStatusStore
      .getState()
      .setStatus(payload.workspace_id, payload.status);
  }, []);

  useTauriEvent(onTunnelStatusChanged, handle, [handle]);
}
