/**
 * One live connection to another environment.
 *
 * The same `RemoteTransport` a browser uses to drive a desktop, carried over
 * iroh by `createIrohConnection`: each (re)connect mints a fresh account grant,
 * dials the machine and performs the account handshake. The connection keeps
 * the machine's app state current by reading it on every connect and following
 * `app-state-changed`.
 */
import type { DeviceRegistry, RegisteredDevice } from "@/remote/device-registry";
import { createIrohConnection } from "@/remote/iroh-connection";
import type { IrohDialer } from "@/remote/iroh-transport";
import { RemoteTransport } from "@/remote/transport";
import type { AppStateSnapshot } from "@/tauri/types";

import type { EnvironmentView } from "./store";

const APP_STATE_EVENT = "app-state-changed";

export interface EnvironmentConnectionOptions {
  device: RegisteredDevice;
  registry: Pick<DeviceRegistry, "connectDevice">;
  dialer: IrohDialer;
  onUpdate(patch: Partial<Omit<EnvironmentView, "device">>): void;
}

export interface EnvironmentConnection {
  transport: RemoteTransport;
  close(): void;
}

function message(error: unknown): string | null {
  if (error instanceof Error) return error.message;
  return typeof error === "string" ? error : null;
}

export function openEnvironmentConnection(
  opts: EnvironmentConnectionOptions,
): EnvironmentConnection {
  let closed = false;
  const update = (patch: Partial<Omit<EnvironmentView, "device">>) => {
    if (!closed) opts.onUpdate(patch);
  };
  const deps = createIrohConnection({
    registry: opts.registry,
    deviceId: opts.device.id,
    dialer: opts.dialer,
    onPending: () => update({ status: "pending-approval", error: null }),
    onMintError: (error) => {
      if (error.kind === "unauthorized" || error.kind === "not_found") return;
      update({ status: "offline", error: error.message });
    },
    onTransportError: (error) => update({ status: "offline", error: message(error) }),
  });

  const transport: RemoteTransport = new RemoteTransport({
    // Only used to build ticket and socket URLs, which the iroh deps intercept.
    baseUrl: `https://${opts.device.deviceId}.environment.invalid`,
    getToken: () => null,
    deps,
    hooks: {
      onChannelMessage: () => {},
      onEvent: (event, payload) => {
        if (event === APP_STATE_EVENT) {
          update({ appState: payload as AppStateSnapshot });
        }
      },
      onStatusChange: (status) => {
        update({ status, ...(status === "connected" ? { error: null } : {}) });
        if (status === "connected") {
          transport
            .invoke("get_app_state")
            .then((appState) => update({ appState: appState as AppStateSnapshot }))
            .catch((error: unknown) => update({ error: message(error) }));
        }
      },
      onUnauthorized: () =>
        update({ status: "unauthorized", error: "Sign in again to reach this machine." }),
    },
  });
  transport.subscribe(APP_STATE_EVENT);
  // Transient failures retry inside the transport; only an unauthorized first
  // attempt rejects, which `onUnauthorized` already reports.
  void transport.connect().catch(() => {});

  return {
    transport,
    close: () => {
      closed = true;
      transport.close();
    },
  };
}
