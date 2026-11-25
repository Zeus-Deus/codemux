import { create } from "zustand";

import type { RegisteredDevice } from "@/remote/device-registry";
import type { AppStateSnapshot } from "@/tauri/types";

/** This backend's identity (`environment_info`). */
export interface EnvironmentDescriptor {
  id: string;
  name: string;
  platform: string;
  app_version: string;
  sharing: boolean;
}

/**
 * Where a connection to another environment stands.
 * - `pending-approval`: the machine has not yet approved this device.
 * - `offline`: the machine is unreachable; the client keeps retrying.
 * - `revoked`: the machine blocked this browser; Retry is a requester action.
 * - `unauthorized`: the account session ended or the client must be updated.
 */
export type EnvironmentStatus =
  | "connecting"
  | "connected"
  | "reconnecting"
  | "pending-approval"
  | "offline"
  | "revoked"
  | "unauthorized";

/** One of the account's other machines, live. */
export interface EnvironmentView {
  device: RegisteredDevice;
  status: EnvironmentStatus;
  pendingBrowserLabel: string | null;
  /** Last app state received from the machine; kept while it is offline. */
  appState: AppStateSnapshot | null;
  error: string | null;
}

interface EnvironmentsState {
  self: EnvironmentDescriptor | null;
  /** Other machines by their stable environment id (`deviceId`). */
  environments: Record<string, EnvironmentView>;
  setSelf(self: EnvironmentDescriptor | null): void;
  /** Replace the catalog: unknown machines are added, missing ones removed. */
  setCatalog(devices: RegisteredDevice[]): void;
  update(id: string, patch: Partial<Omit<EnvironmentView, "device">>): void;
  reset(): void;
}

export const useEnvironmentsStore = create<EnvironmentsState>((set) => ({
  self: null,
  environments: {},
  setSelf: (self) => set({ self }),
  setCatalog: (devices) =>
    set((state) => {
      const environments: Record<string, EnvironmentView> = {};
      for (const device of devices) {
        const existing = state.environments[device.deviceId];
        environments[device.deviceId] = existing
          ? { ...existing, device }
          : { device, status: "connecting", pendingBrowserLabel: null, appState: null, error: null };
      }
      return { environments };
    }),
  update: (id, patch) =>
    set((state) => {
      const existing = state.environments[id];
      if (!existing) return state;
      const next = { ...existing, ...patch };
      if (next.status !== "pending-approval") next.pendingBrowserLabel = null;
      // A snapshot from a restarted or older backend is accepted; within one
      // backend instance a lower revision is a stale delivery.
      const incoming = patch.appState;
      const current = existing.appState;
      if (
        incoming &&
        current &&
        incoming.snapshot_instance &&
        incoming.snapshot_instance === current.snapshot_instance &&
        (incoming.snapshot_revision ?? 0) < (current.snapshot_revision ?? 0)
      ) {
        next.appState = current;
      }
      return { environments: { ...state.environments, [id]: next } };
    }),
  reset: () => set({ self: null, environments: {} }),
}));
