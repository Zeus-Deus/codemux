/**
 * Keeps one live connection to each of the account's other machines.
 *
 * The catalog is the account device registry: every machine that shares
 * itself registers there and refreshes `lastSeenAt` every five minutes. The
 * runtime re-reads it every minute, connects to machines seen recently and
 * marks the rest offline without dialing them, so a machine that is switched
 * off does not cost a grant and a dial every few seconds.
 */
import type { DeviceRegistry, RegisteredDevice } from "@/remote/device-registry";
import { DeviceRegistryError } from "@/remote/device-registry";
import type { IrohDialer } from "@/remote/iroh-transport";

import { openEnvironmentConnection, type EnvironmentConnection } from "./connection";
import { useEnvironmentsStore } from "./store";

const CATALOG_REFRESH_MS = 60_000;
/** Three missed registration refreshes: the machine is off or not sharing. */
const STALE_AFTER_MS = 15 * 60_000;

export interface EnvironmentRuntimeOptions {
  registry: Pick<DeviceRegistry, "listDevices" | "connectDevice">;
  dialer: IrohDialer;
  /** This client's own environment id, excluded from the catalog. */
  selfId: string | null;
  now?(): number;
}

export interface EnvironmentRuntime {
  refresh(): Promise<void>;
  stop(): void;
}

export function isRecentlySeen(device: RegisteredDevice, now: number): boolean {
  if (!device.lastSeenAt) return false;
  const seen = Date.parse(device.lastSeenAt);
  return Number.isFinite(seen) && now - seen < STALE_AFTER_MS;
}

export function startEnvironmentRuntime(opts: EnvironmentRuntimeOptions): EnvironmentRuntime {
  const now = opts.now ?? Date.now;
  const connections = new Map<string, { device: RegisteredDevice; connection: EnvironmentConnection }>();
  let stopped = false;

  const disconnect = (id: string) => {
    connections.get(id)?.connection.close();
    connections.delete(id);
  };

  const refresh = async () => {
    let devices: RegisteredDevice[];
    try {
      devices = await opts.registry.listDevices();
    } catch (error) {
      // Signed out or no account: there is no catalog to show.
      if (error instanceof DeviceRegistryError && error.kind === "unauthorized") {
        for (const id of [...connections.keys()]) disconnect(id);
        if (!stopped) useEnvironmentsStore.getState().setCatalog([]);
      }
      return;
    }
    if (stopped) return;
    const others = devices.filter((device) => device.deviceId !== opts.selfId);
    useEnvironmentsStore.getState().setCatalog(others);

    const wanted = new Set<string>();
    for (const device of others) {
      if (!isRecentlySeen(device, now())) {
        disconnect(device.deviceId);
        useEnvironmentsStore.getState().update(device.deviceId, { status: "offline" });
        continue;
      }
      wanted.add(device.deviceId);
      const existing = connections.get(device.deviceId);
      // A re-registration with a new address needs a fresh connection.
      if (existing && existing.device.nodeId === device.nodeId && existing.device.id === device.id) {
        continue;
      }
      disconnect(device.deviceId);
      connections.set(device.deviceId, {
        device,
        connection: openEnvironmentConnection({
          device,
          registry: opts.registry,
          dialer: opts.dialer,
          onUpdate: (patch) => useEnvironmentsStore.getState().update(device.deviceId, patch),
        }),
      });
    }
    for (const id of [...connections.keys()]) {
      if (!wanted.has(id)) disconnect(id);
    }
  };

  void refresh();
  const timer = setInterval(() => void refresh(), CATALOG_REFRESH_MS);

  return {
    refresh,
    stop: () => {
      stopped = true;
      clearInterval(timer);
      for (const id of [...connections.keys()]) disconnect(id);
      useEnvironmentsStore.getState().reset();
    },
  };
}
