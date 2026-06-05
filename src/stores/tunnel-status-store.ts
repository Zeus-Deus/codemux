import { create } from "zustand";

import type { TunnelStatus } from "@/tauri/events";

/**
 * Live SSH-tunnel health per remote workspace, fed by the backend's
 * `tunnel-status-changed` event (see `src-tauri/src/ssh/registry.rs`).
 *
 * The UI reads this to render a compact status pill on remote workspaces
 * — "Reconnecting…" on a sleep/wake or WiFi flap, "Connection lost" once
 * the supervisor's circuit breaker trips. Without it the workspace just
 * appears frozen and the user is never told they must re-push.
 *
 * `connected` / `pending` deliberately resolve to "no pill" (see
 * `tunnelStatusKind`): a healthy tunnel needs no chrome.
 */
interface TunnelStatusStore {
  /** Keyed by workspace_id. Absent = unknown (local, or never pushed). */
  byWorkspace: Record<string, TunnelStatus>;
  setStatus: (workspaceId: string, status: TunnelStatus) => void;
  /** Drop a workspace's status (e.g. on pull-back / close). */
  clear: (workspaceId: string) => void;
}

export const useTunnelStatusStore = create<TunnelStatusStore>()((set) => ({
  byWorkspace: {},
  setStatus: (workspaceId, status) =>
    set((s) => ({
      byWorkspace: { ...s.byWorkspace, [workspaceId]: status },
    })),
  clear: (workspaceId) =>
    set((s) => {
      if (!(workspaceId in s.byWorkspace)) return s;
      const next = { ...s.byWorkspace };
      delete next[workspaceId];
      return { byWorkspace: next };
    }),
}));

/** Narrow a TunnelStatus to the only two states worth showing chrome for.
 *  `null` = healthy/unknown → render nothing. */
export function tunnelStatusKind(
  status: TunnelStatus | undefined,
): "reconnecting" | "lost" | null {
  if (!status) return null;
  if (status.kind === "reconnecting") return "reconnecting";
  if (status.kind === "circuit_open") return "lost";
  return null;
}
