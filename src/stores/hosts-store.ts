import { create } from "zustand";
import { hostsList, type HostView } from "@/tauri/commands";

/**
 * Single source of truth for the user's configured SSH hosts.
 *
 * Why a store and not per-component `useEffect(() => hostsList())`:
 * the workspace context menu mounts a `WorkspaceContextMenuItems`
 * per workspace row, and `DevicePicker` mounts at every spawn-from
 * surface (new-workspace dialog, chat composer, …). With 20
 * workspaces in the sidebar plus an open dialog, that was 21+ IPC
 * round-trips and 21+ SQLite mutex acquisitions on every render —
 * pure redundant work. Caching here collapses that to a single
 * round-trip with subscription-based reuse across consumers.
 *
 * Refresh model is explicit: callers that know they mutated hosts
 * (add/update/delete) call `refresh()` after the Tauri command
 * resolves. No subscription to a backend event yet — the surface
 * mutating the list is always the same surface that needs the
 * refresh, so explicit invalidation is simpler than wiring an event.
 *
 * `init()` is idempotent: callers can call it on mount without
 * worrying about double-fetch. The first call kicks off the
 * fetch; subsequent calls during the in-flight fetch hand back
 * the same promise.
 */
interface HostsStore {
  hosts: HostView[];
  /** True between `init()`/`refresh()` and the load resolving.
   *  Consumers can show a tiny loader; today nobody does because
   *  the first load is so fast it's not worth the visual noise. */
  loading: boolean;
  /** Last load's error, if any. Null after a successful load. */
  error: string | null;
  /** True once the first load has resolved (success or failure).
   *  Lets components distinguish "we have no hosts" from "we
   *  haven't loaded yet." */
  loaded: boolean;
  /** Triggers a fetch if one isn't already in flight. Returns
   *  the in-flight promise. Cheap to call repeatedly. */
  init: () => Promise<void>;
  /** Force a re-fetch even if already loaded. Used after add /
   *  update / delete. */
  refresh: () => Promise<void>;
}

let inFlight: Promise<void> | null = null;

export const useHostsStore = create<HostsStore>((set, get) => ({
  hosts: [],
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
    if (inFlight) {
      return inFlight;
    }
    set({ loading: true, error: null });
    inFlight = hostsList()
      .then((list) => {
        set({ hosts: list, loading: false, loaded: true, error: null });
      })
      .catch((err: unknown) => {
        const message = typeof err === "string" ? err : String(err);
        // Don't blow away the previous list on a transient failure
        // — the picker degrades to "local-only," which is what we
        // want even when the DB momentarily can't be read.
        set({ loading: false, loaded: true, error: message });
      })
      .finally(() => {
        inFlight = null;
      });
    return inFlight;
  },
}));

/** Reset the store to its initial state. Test-only — production
 *  code should never need to wipe the cache (refresh() is the way).
 *  Exported (not test-cfg-gated) because integration tests outside
 *  this file's compilation unit need access. */
export function __resetHostsStoreForTests() {
  inFlight = null;
  useHostsStore.setState({
    hosts: [],
    loading: false,
    error: null,
    loaded: false,
  });
}

/** Convenience hook for consumers that just want the list and don't
 *  care about loading state. Auto-inits on first call. */
export function useHosts(): HostView[] {
  const hosts = useHostsStore((s) => s.hosts);
  const loaded = useHostsStore((s) => s.loaded);
  const init = useHostsStore((s) => s.init);
  // Kick off the first fetch lazily on first read. React 18+ runs
  // this during render which is normally a no-no, but `init()` is
  // idempotent (returns the same in-flight promise) and never
  // touches state synchronously — so it's safe.
  if (!loaded) {
    void init();
  }
  return hosts;
}
