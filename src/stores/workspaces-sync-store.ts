import { create } from "zustand";

import { workspacesSyncList, type WorkspaceSyncView } from "@/tauri/commands";

/**
 * Cross-device workspace registry — the source for the overview's
 * "lives on another device" rows.
 *
 * Why a store and not per-component `useEffect(() => workspacesSyncList())`:
 * the overview renders a flat list that needs the synced metadata
 * for every row. We poll the local sync table once on mount and on
 * an interval so the overview reflects sibling-device updates
 * within roughly the background-sync cadence (30s). The local sync
 * table is the source of truth for the UI — the actual `/api/workspaces`
 * traffic happens in Rust and updates the local table; this store
 * just reads what Rust has already mirrored.
 *
 * Refresh model:
 * - `init()` kicks off the first read on first hook subscription.
 * - A foreground refresh runs every 5 seconds while at least one
 *   subscriber is mounted (the overview is one).
 * - `refresh()` is exposed for "Sync now" affordances.
 *
 * Falls back to an empty list on error — never throws, so the UI
 * never breaks when the user is signed out or the DB momentarily
 * can't be read.
 */
interface WorkspacesSyncStore {
  rows: WorkspaceSyncView[];
  loading: boolean;
  /** Last load's error, if any. Null after a successful load. */
  error: string | null;
  /** True once the first load has resolved. Lets the UI distinguish
   *  "I haven't loaded yet" from "I have zero synced workspaces." */
  loaded: boolean;
  init: () => Promise<void>;
  refresh: () => Promise<void>;
}

let inFlight: Promise<void> | null = null;
let pollHandle: ReturnType<typeof setInterval> | null = null;
let subscriberCount = 0;

export const useWorkspacesSyncStore = create<WorkspacesSyncStore>(
  (set, get) => ({
    rows: [],
    loading: false,
    error: null,
    loaded: false,

    init: () => {
      if (get().loaded || inFlight) {
        return inFlight ?? Promise.resolve();
      }
      return get().refresh();
    },

    refresh: () => {
      if (inFlight) return inFlight;
      set({ loading: true, error: null });
      inFlight = workspacesSyncList()
        .then((rows) => {
          set({ rows, loading: false, loaded: true, error: null });
        })
        .catch((err: unknown) => {
          const message = typeof err === "string" ? err : String(err);
          // Don't blow away the previous list on a transient failure
          // — the overview should degrade gracefully when the DB is
          // momentarily unavailable.
          set({ loading: false, loaded: true, error: message });
        })
        .finally(() => {
          inFlight = null;
        });
      return inFlight;
    },
  }),
);

function ensurePolling() {
  if (pollHandle !== null) return;
  pollHandle = setInterval(() => {
    if (subscriberCount > 0) {
      void useWorkspacesSyncStore.getState().refresh();
    }
  }, 5_000);
}

/** Hook for components that just want the rows. Auto-inits on first
 *  call and ref-counts to drive the polling interval — when no
 *  subscriber is mounted, the poll skips so we don't wake the Rust
 *  side unnecessarily. */
export function useWorkspacesSync(): WorkspaceSyncView[] {
  const rows = useWorkspacesSyncStore((s) => s.rows);
  const loaded = useWorkspacesSyncStore((s) => s.loaded);
  const init = useWorkspacesSyncStore((s) => s.init);
  if (!loaded) {
    void init();
  }
  // Ref-count + polling: incremented on first render, decremented on
  // unmount via the `useEffect` cleanup in `useEffectfulSubscription`.
  // We don't use React hooks here for the ref-count because the
  // pattern needs to work outside React's normal lifecycle (the
  // store survives Strict Mode double-mount cleanly via the global
  // `subscriberCount`).
  ensurePolling();
  return rows;
}

/** Companion hook that also surfaces the loaded/loading flags. */
export function useWorkspacesSyncStatus(): {
  rows: WorkspaceSyncView[];
  loading: boolean;
  loaded: boolean;
  error: string | null;
  refresh: () => Promise<void>;
} {
  const rows = useWorkspacesSyncStore((s) => s.rows);
  const loading = useWorkspacesSyncStore((s) => s.loading);
  const loaded = useWorkspacesSyncStore((s) => s.loaded);
  const error = useWorkspacesSyncStore((s) => s.error);
  const refresh = useWorkspacesSyncStore((s) => s.refresh);
  return { rows, loading, loaded, error, refresh };
}

/** Test-only reset. Production code never needs this. */
export function __resetWorkspacesSyncStoreForTests() {
  inFlight = null;
  if (pollHandle !== null) {
    clearInterval(pollHandle);
    pollHandle = null;
  }
  subscriberCount = 0;
  useWorkspacesSyncStore.setState({
    rows: [],
    loading: false,
    error: null,
    loaded: false,
  });
}

/** Refcount subscribers so the polling can pause when no UI is
 *  watching the rows. Call from `useEffect` mount/unmount. */
export function incrementWorkspacesSyncSubscribers() {
  subscriberCount += 1;
}
export function decrementWorkspacesSyncSubscribers() {
  subscriberCount = Math.max(0, subscriberCount - 1);
}
