/**
 * One live connection to another environment.
 *
 * The same `RemoteTransport` a browser uses to drive a desktop, carried over
 * iroh by `createIrohConnection`: each (re)connect mints a fresh account grant,
 * dials the machine and performs the account handshake. The connection keeps
 * the machine's app state current by reading it on every connect and following
 * snapshots, revisioned deltas and revision heartbeats.
 */
import type { DeviceRegistry, RegisteredDevice } from "@/remote/device-registry";
import { createIrohConnection } from "@/remote/iroh-connection";
import type { IrohDialer } from "@/remote/iroh-transport";
import { RemoteTransport } from "@/remote/transport";
import {
  DELTA_BUFFER_LIMIT,
  DELTA_REORDER_WINDOW_MS,
  drainDeltaBuffer,
} from "@/stores/app-store";
import type {
  AppStateDelta,
  AppStateSnapshot,
  RevisionedDelta,
  RevisionHeartbeat,
} from "@/tauri/types";

import type { EnvironmentView } from "./store";

const APP_STATE_EVENT = "app-state-changed";
const APP_STATE_DELTA_EVENT = "app-state-delta";
const APP_STATE_REVISION_EVENT = "app-state-revision";

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
  // Each machine has its own revision stream; never apply its deltas to the
  // desktop's app store. Reuse the local stream's domain and buffer reducer.
  let appState: AppStateSnapshot | null = null;
  let revision = 0;
  let instance: string | undefined;
  let buffer = new Map<number, AppStateDelta>();
  let gapTimer: ReturnType<typeof setTimeout> | undefined;
  let connected = false;
  let fetching = false;
  let fetchEpoch = 0;
  let requiredRevision = 0;

  const clearGap = () => {
    clearTimeout(gapTimer);
    gapTimer = undefined;
  };
  const followGap = () => {
    if (buffer.size === 0) clearGap();
    else if (!fetching && gapTimer === undefined && connected && !closed) {
      gapTimer = setTimeout(() => {
        gapTimer = undefined;
        resync();
      }, DELTA_REORDER_WINDOW_MS);
    }
  };
  const drain = () => {
    if (appState) {
      const drained = drainDeltaBuffer(appState, revision, buffer);
      revision = drained.lastSeenRevision;
      buffer = drained.buffer;
      // Even a no-op delta advances the stream's revision floor.
      appState = drained.appState.snapshot_revision === revision
        ? drained.appState
        : { ...drained.appState, snapshot_revision: revision };
      update({ appState });
    }
    followGap();
  };
  const observeInstance = (next: string | undefined) => {
    if (!next) return;
    if (instance && instance !== next) {
      appState = null;
      revision = 0;
      requiredRevision = 0;
      buffer = new Map();
      clearGap();
      // A fetch started against the old process cannot restore its baseline.
      fetchEpoch += 1;
      fetching = false;
    }
    instance = next;
  };
  const acceptSnapshot = (snapshot: AppStateSnapshot) => {
    observeInstance(snapshot.snapshot_instance);
    const incoming = snapshot.snapshot_revision ?? 0;
    if (appState && incoming > 0 && incoming <= revision) return;
    appState = snapshot;
    revision = incoming > 0 ? incoming : revision;
    drain();
  };
  const resync = () => {
    if (closed || !connected || fetching) return;
    clearGap();
    fetching = true;
    const epoch = fetchEpoch;
    let succeeded = false;
    void transport
      .invoke("get_app_state")
      .then((snapshot) => {
        if (closed || epoch !== fetchEpoch) return;
        acceptSnapshot(snapshot as AppStateSnapshot);
        update({ error: null });
        succeeded = true;
      })
      .catch((error: unknown) => {
        if (epoch === fetchEpoch) update({ error: message(error) });
      })
      .finally(() => {
        if (closed || epoch !== fetchEpoch) return;
        fetching = false;
        drain();
        // A heartbeat or buffer overflow may have overtaken this baseline.
        if (succeeded && requiredRevision > revision) resync();
      });
  };
  const acceptDelta = (payload: RevisionedDelta) => {
    observeInstance(payload.instance);
    if (payload.revision <= revision) return;
    buffer = new Map(buffer).set(payload.revision, payload.delta);
    if (buffer.size > DELTA_BUFFER_LIMIT) {
      requiredRevision = Math.max(requiredRevision, payload.revision);
      buffer = new Map();
      resync();
    } else if (!appState) resync();
    else if (!fetching) drain();
  };
  const acceptHeartbeat = (payload: RevisionHeartbeat) => {
    observeInstance(payload.instance);
    requiredRevision = Math.max(requiredRevision, payload.revision);
    if (!appState || requiredRevision > revision) resync();
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
          acceptSnapshot(payload as AppStateSnapshot);
        } else if (event === APP_STATE_DELTA_EVENT) {
          acceptDelta(payload as RevisionedDelta);
        } else if (event === APP_STATE_REVISION_EVENT) {
          acceptHeartbeat(payload as RevisionHeartbeat);
        }
      },
      onStatusChange: (status) => {
        update({ status, ...(status === "connected" ? { error: null } : {}) });
        connected = status === "connected";
        fetchEpoch += 1;
        fetching = false;
        clearGap();
        if (connected) resync();
      },
      onUnauthorized: () =>
        update({ status: "unauthorized", error: "Sign in again to reach this machine." }),
    },
  });
  transport.subscribe(APP_STATE_EVENT);
  transport.subscribe(APP_STATE_DELTA_EVENT);
  transport.subscribe(APP_STATE_REVISION_EVENT);
  // Transient failures retry inside the transport; only an unauthorized first
  // attempt rejects, which `onUnauthorized` already reports.
  void transport.connect().catch(() => {});

  return {
    transport,
    close: () => {
      closed = true;
      clearGap();
      transport.close();
    },
  };
}
