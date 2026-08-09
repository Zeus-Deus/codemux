import { useEffect, useRef, useCallback } from "react";
import { getAppState } from "@/tauri/commands";
import { onAppStateChanged, onAppStateDelta, onAppStateRevision } from "@/tauri/events";
import { useAppStore, DELTA_REORDER_WINDOW_MS } from "@/stores/app-store";
import { useTauriEvent } from "./use-tauri-event";
import { markOpenInteraction } from "@/lib/perf/interaction-trace";
import type {
  AppStateSnapshot,
  RevisionedDelta,
  RevisionHeartbeat,
} from "@/tauri/types";

/**
 * Whether an incoming snapshot is the one that confirms the optimistic
 * selection, and so must bypass the emit debounce. Pure — exported for tests.
 */
export function confirmsPendingActivation(
  payload: Pick<AppStateSnapshot, "active_workspace_id">,
  pendingActiveWorkspaceId: string | null,
): boolean {
  return (
    pendingActiveWorkspaceId !== null &&
    payload.active_workspace_id === pendingActiveWorkspaceId
  );
}

export function useAppStateInit(skip = false) {
  const setAppState = useAppStore((s) => s.setAppState);
  const applyAppStateDelta = useAppStore((s) => s.applyAppStateDelta);
  const resyncRequestId = useAppStore((s) => s.resyncRequestId);
  const gapWindowId = useAppStore((s) => s.gapWindowId);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingCommitRef = useRef<(() => void) | null>(null);

  // Run a snapshot that is still sitting in the coalescing window, now.
  //
  // It was delivered BEFORE whatever prompted the flush and carries a lower
  // revision, so committing it afterwards would leave `lastSeenRevision`
  // behind the stream and make the next delta look like a gap. The debounce
  // is a render-cost optimization; it must never reorder the revision stream.
  const flushPendingSnapshot = useCallback(() => {
    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
      debounceRef.current = null;
    }
    const commit = pendingCommitRef.current;
    pendingCommitRef.current = null;
    commit?.();
  }, []);

  // Fetch initial state on mount (skip if not authenticated yet)
  useEffect(() => {
    if (skip) return;
    getAppState()
      .then((snapshot) => {
        setAppState(snapshot);
      })
      .catch((err) => console.error("Failed to fetch app state:", err));
  }, [setAppState, skip]);

  // Subscribe to state changes with a 16ms debounce. The backend already
  // coalesces app-state mutations, so the previous full-snapshot
  // JSON.stringify dedup was paying a multi-KB serialize cost on every
  // tick to catch the rare identical-payload case. Dropping it lets
  // Zustand's selector-level reference equality at the component layer
  // handle no-op fan-out — workspace state can grow with each opened
  // surface/pane/session, so the stringify scaled with usage and was
  // a real freeze contributor under sustained backend churn.
  //
  // Reference stability is now restored *without* stringifying: `setAppState`
  // (src/stores/app-store.ts) runs `shareStructural(prev, next)`, a single
  // O(n) structural-sharing walk that allocates NO strings and reuses prev
  // references for every unchanged subtree. It runs at most once per this
  // 16ms debounce window — distinct from the removed stringify dedup, which
  // serialized the whole snapshot to a multi-KB string on every tick.
  //
  // The two `markOpenInteraction` calls are the delivery/commit boundary of an
  // in-flight interaction trace.
  // They are no-ops unless a trace is open, and scoping them to the snapshot's
  // own `active_workspace_id` keeps an unrelated streaming emit from claiming
  // the stamp.
  //
  // Phase 1 adds one exception to the debounce: the snapshot that confirms the
  // workspace the user just selected is applied immediately. A trailing-edge
  // debounce is not a throttle — under sustained churn (agent streaming emits
  // at ~60 Hz) each new event clears and reschedules the timer, so the
  // activation snapshot could be starved indefinitely while the optimistic
  // selection waits to be reconciled. Every other snapshot keeps the coalescing
  // window.
  const handleStateChanged = useCallback(
    (payload: AppStateSnapshot) => {
      const target = payload.active_workspace_id ?? undefined;
      markOpenInteraction("snapshot-received", { target });
      const commit = () => {
        debounceRef.current = null;
        pendingCommitRef.current = null;
        setAppState(payload);
        markOpenInteraction("state-committed", { target });
      };
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
        debounceRef.current = null;
      }
      if (
        confirmsPendingActivation(
          payload,
          useAppStore.getState().pendingActiveWorkspaceId,
        )
      ) {
        commit();
        return;
      }
      pendingCommitRef.current = commit;
      debounceRef.current = setTimeout(commit, 16);
    },
    [setAppState],
  );

  // Deltas bypass the debounce: they are a few hundred bytes and touch one
  // domain, so the coalescing window would only add latency to a change the
  // renderer can absorb in a single targeted commit.
  const handleDelta = useCallback(
    (payload: RevisionedDelta) => {
      flushPendingSnapshot();
      applyAppStateDelta(payload.revision, payload.delta, payload.instance);
    },
    [applyAppStateDelta, flushPendingSnapshot],
  );

  // The heartbeat carries no state — only "the backend is at revision N". If
  // we are behind it, an emit never reached us (a dropped event, or a mutation
  // path that stamped without emitting) and only a full snapshot can say by
  // how much.
  const handleRevision = useCallback(
    (payload: RevisionHeartbeat) => {
      flushPendingSnapshot();
      const { appState, lastSeenRevision, backendInstance, requestResync } =
        useAppStore.getState();
      if (appState === null) return; // the boot fetch is the baseline
      // A heartbeat from a DIFFERENT backend process means the counter
      // restarted, so `payload.revision > lastSeenRevision` is the wrong
      // question — after a restart it is almost always false, which is
      // precisely why a reconnected page used to sit frozen with a heartbeat
      // arriving every minute and never triggering the resync that would have
      // healed it. Any instance change is a resync, whatever the numbers say.
      if (
        payload.instance &&
        backendInstance !== null &&
        backendInstance !== payload.instance
      ) {
        requestResync();
        return;
      }
      if (payload.revision > lastSeenRevision) requestResync();
    },
    [flushPendingSnapshot],
  );

  // One timer per reorder window. `gapWindowId` is non-zero only while a delta
  // is waiting on a revision that has not arrived; it keeps its value while the
  // window stays open (so the window is measured from the first out-of-order
  // delta, not restarted by each one) and returns to 0 the moment the buffer
  // drains — which unmounts this timer without it ever firing. When it does
  // fire, the missing revision never came and the gap is real.
  useEffect(() => {
    if (skip || gapWindowId === 0) return;
    const timer = setTimeout(() => {
      useAppStore.getState().expireGapWindow(gapWindowId);
    }, DELTA_REORDER_WINDOW_MS);
    return () => clearTimeout(timer);
  }, [gapWindowId, skip]);

  // Single-flight full resync. `resyncRequestId` only moves when the store
  // opens a resync window (gap or heartbeat-behind), and `endResync` closes it
  // whether the fetch resolved or rejected — a rejected one must not wedge the
  // delta path permanently shut.
  useEffect(() => {
    if (skip || resyncRequestId === 0) return;
    getAppState()
      .then(setAppState)
      .catch((err) => console.error("Failed to resync app state:", err))
      .finally(() => {
        useAppStore.getState().endResync();
      });
  }, [resyncRequestId, setAppState, skip]);

  useTauriEvent(onAppStateChanged, handleStateChanged, [handleStateChanged]);
  useTauriEvent(onAppStateDelta, handleDelta, [handleDelta]);
  useTauriEvent(onAppStateRevision, handleRevision, [handleRevision]);
}
