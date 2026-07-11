import { create } from "zustand";

/**
 * Live web-remote connection state, surfaced to React.
 *
 * The pre-app bootstrap (`bootstrap.tsx`) drives this from the transport's
 * status callbacks — it runs before React mounts, so it writes through
 * `getState()` and the app's chrome reads it once it comes alive. Two
 * surfaces consume it (`remote-connection-indicator.tsx`):
 *
 *   - `connected`   → a quiet chip in the title bar's right cluster (the
 *                     slot freed by the hidden native window controls).
 *   - `reconnecting`/`offline` → a loud, centered banner overlay.
 *
 * `null` is the desktop / pre-connection default: nothing to show. Desktop
 * never runs the bootstrap, so the store stays `null` there and both
 * surfaces render nothing — behavior is byte-identical to before.
 *
 * (Replaces the plain-DOM floating pill that used to anchor bottom-left and
 * overlapped the sidebar footer, setup hint, and notification bell.)
 */
export type RemoteConnectionStatus = "connected" | "reconnecting" | "offline";

interface RemoteConnectionState {
  status: RemoteConnectionStatus | null;
  /** Endpoint host the client paired to, e.g. `127.0.0.1:4379`. */
  host: string;
  /** Prominent copy for the `offline` state (revoked / session gone). */
  offlineMessage: string | null;
  /** Steady connected state → quiet title-bar chip. */
  setConnected: (host: string) => void;
  /** Backoff/reconnect → loud amber banner. */
  setReconnecting: (host: string) => void;
  /** Session revoked / gone → loud red banner (bootstrap reloads after). */
  setOffline: (message?: string) => void;
}

export const useRemoteConnectionStore = create<RemoteConnectionState>((set) => ({
  status: null,
  host: "",
  offlineMessage: null,
  setConnected: (host) =>
    set({ status: "connected", host, offlineMessage: null }),
  setReconnecting: (host) =>
    set((s) => ({ status: "reconnecting", host: host || s.host })),
  setOffline: (message) =>
    set({
      status: "offline",
      offlineMessage: message ?? "Remote access revoked",
    }),
}));
