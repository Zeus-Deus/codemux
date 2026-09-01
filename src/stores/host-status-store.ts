import { useEffect } from "react";
import { create } from "zustand";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import {
  HOSTS_STATUS_CHANGED_EVENT,
  hostsStatusList,
  type HostStatusView,
} from "@/tauri/commands";

/**
 * Reachability + inventory facts per configured device, as the background
 * SSH poller last observed them. One fetch on first use, then kept fresh by
 * the `hosts-status-changed` event; consumers never poll.
 *
 * Deliberately separate from `hosts-store` (identity, synced to the account):
 * a host's online/offline state is a fact about this install's view of the
 * network, not account data, and it changes on its own cadence.
 */
interface HostStatusStore {
  /** Keyed by `HostView.id`. Every configured host has a row once the
   *  list has loaded; one the poller hasn't reached yet carries
   *  `probed: false`. A host absent here is one the list predates. */
  statuses: Record<number, HostStatusView>;
  loaded: boolean;
  init: () => Promise<void>;
  refresh: () => Promise<void>;
  /** Replace the whole map from an event payload. */
  applyAll: (rows: HostStatusView[]) => void;
}

let inFlight: Promise<void> | null = null;

function toMap(rows: HostStatusView[]): Record<number, HostStatusView> {
  const map: Record<number, HostStatusView> = {};
  for (const row of rows) map[row.host_id] = row;
  return map;
}

export const useHostStatusStore = create<HostStatusStore>((set, get) => ({
  statuses: {},
  loaded: false,

  init: () => {
    if (get().loaded || inFlight) return inFlight ?? Promise.resolve();
    return get().refresh();
  },

  refresh: () => {
    if (inFlight) return inFlight;
    inFlight = hostsStatusList()
      .then((rows) => set({ statuses: toMap(rows), loaded: true }))
      .catch(() => set({ loaded: true }))
      .finally(() => {
        inFlight = null;
      });
    return inFlight;
  },

  applyAll: (rows) => set({ statuses: toMap(rows), loaded: true }),
}));

// ── Shared event subscription ──────────────────────────────────────
//
// One Tauri listener serves every mounted consumer. `listen()` resolves
// asynchronously, so the consumer that started it may already be gone by
// the time the unlisten handle arrives; whoever receives the handle
// therefore checks the live subscriber count, not its own mount state,
// and drops the listener at once when nobody is left.

let subscribers = 0;
let unlisten: UnlistenFn | null = null;
let attaching: Promise<void> | null = null;

function attachListener(): void {
  if (unlisten || attaching) return;
  attaching = listen<HostStatusView[]>(HOSTS_STATUS_CHANGED_EVENT, (event) => {
    useHostStatusStore.getState().applyAll(event.payload);
  })
    .then((fn) => {
      if (subscribers === 0) fn();
      else unlisten = fn;
    })
    .catch(() => {
      // Without the event the store still holds the initial fetch; a
      // later consumer will retry the subscription.
    })
    .finally(() => {
      attaching = null;
    });
}

function releaseListener(): void {
  unlisten?.();
  unlisten = null;
}

/**
 * Subscribe to live host status. Mount-scoped: the event listener is shared
 * across all mounted consumers and released when the last one unmounts.
 */
export function useHostStatuses(): Record<number, HostStatusView> {
  const statuses = useHostStatusStore((s) => s.statuses);
  const loaded = useHostStatusStore((s) => s.loaded);
  const init = useHostStatusStore((s) => s.init);

  useEffect(() => {
    if (!loaded) void init();
  }, [loaded, init]);

  useEffect(() => {
    subscribers += 1;
    attachListener();
    return () => {
      subscribers -= 1;
      if (subscribers === 0) releaseListener();
    };
  }, []);

  return statuses;
}

/** Test-only reset. */
export function __resetHostStatusStoreForTests() {
  inFlight = null;
  attaching = null;
  releaseListener();
  subscribers = 0;
  useHostStatusStore.setState({ statuses: {}, loaded: false });
}
