// Live sync-status hook for the Stage 5 dashboard.
//
// The Tauri side emits `skills-sync-state-changed` whenever
// `skills_sync_now` is invoked (one event before the engine's
// pull/push cycle and one after — see
// `src-tauri/src/commands/skills_sync.rs`). This hook subscribes,
// stitches the events together with the initial poll, and turns
// the wire-format millis fields into JS Date objects so the
// display component doesn't have to.
//
// `syncNow` is exposed so the "Sync now" button can call it
// without dragging in the Tauri command import. The hook also
// owns the optimistic `isSyncing` flag — flipped true on click,
// reset by the next event payload — so the button's disabled
// state is responsive even if the network is slow.

import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@/tauri/events";

import { useTauriEvent } from "./use-tauri-event";
import {
  skillsSyncNow,
  skillsSyncStatus,
  type SkillsSyncStateSnapshot,
} from "@/tauri/commands";

export type SyncStateKind = "idle" | "syncing" | "error";

export interface SyncStatusUI {
  state: SyncStateKind;
  /** Populated when `state === "idle"` and the engine has run at
   *  least once this process; null on first launch. */
  lastSyncAt: Date | null;
  /** Populated only while `state === "syncing"`. */
  startedAt: Date | null;
  /** Populated only when `state === "error"`. */
  lastError: string | null;
  /** Wall-clock time of the failure. */
  errorAt: Date | null;
}

/// Listen factory for the wrapper-emitted state events. Kept
/// inline rather than in `events.ts` because Stage 5 is the only
/// caller and the payload shape varies by state. Adapts the raw
/// Tauri Event<T> to the (payload) callback the rest of the
/// codebase's listeners take.
const onSkillsSyncStateChanged = (
  cb: (payload: SkillsSyncStateSnapshot) => void,
): Promise<UnlistenFn> =>
  listen<SkillsSyncStateSnapshot>("skills-sync-state-changed", (e) =>
    cb(e.payload),
  );

function snapshotToUi(s: SkillsSyncStateSnapshot | null): SyncStatusUI | null {
  if (!s) return null;
  switch (s.state) {
    case "idle":
      return {
        state: "idle",
        lastSyncAt:
          s.lastSyncAtMillis !== null ? new Date(s.lastSyncAtMillis) : null,
        startedAt: null,
        lastError: null,
        errorAt: null,
      };
    case "syncing":
      return {
        state: "syncing",
        lastSyncAt: null,
        startedAt: new Date(s.startedAtMillis),
        lastError: null,
        errorAt: null,
      };
    case "error":
      return {
        state: "error",
        lastSyncAt: null,
        startedAt: null,
        lastError: s.lastError,
        errorAt: new Date(s.atMillis),
      };
  }
}

export function useSkillsSyncStatus(): {
  status: SyncStatusUI | null;
  syncNow: () => Promise<void>;
  isSyncing: boolean;
} {
  const [status, setStatus] = useState<SyncStatusUI | null>(null);
  const [optimisticSyncing, setOptimisticSyncing] = useState(false);
  const cancelledRef = useRef(false);

  // Initial fetch on mount. The Tauri command is cheap (in-memory
  // snapshot read), so a single round-trip is fine; no caching
  // needed.
  useEffect(() => {
    cancelledRef.current = false;
    skillsSyncStatus()
      .then((snap) => {
        if (cancelledRef.current) return;
        setStatus(snapshotToUi(snap));
      })
      .catch((err) => {
        // Tauri unavailable in tests / dev harness — surface as
        // null so the display falls back to a skeleton.
        // eslint-disable-next-line no-console
        console.warn("[skills_sync] initial status fetch failed:", err);
      });
    return () => {
      cancelledRef.current = true;
    };
  }, []);

  // Live updates from the Tauri command wrapper. Every payload
  // overwrites the local state — no merging, the engine is the
  // source of truth.
  useTauriEvent(
    onSkillsSyncStateChanged,
    (payload) => {
      setStatus(snapshotToUi(payload));
      // The wrapper's pre/post emits handle this transition for
      // us: a "syncing" event clears the post-click optimism
      // (stays true), and the post-cycle "idle"/"error" event
      // turns it off. Belt-and-braces: also drop optimism when
      // the engine reports anything other than syncing.
      if (payload.state !== "syncing") {
        setOptimisticSyncing(false);
      }
    },
    [],
  );

  const syncNow = useCallback(async () => {
    setOptimisticSyncing(true);
    try {
      await skillsSyncNow();
    } catch (err) {
      // The engine records the error in its state snapshot, which
      // arrives via the post-cycle event and turns isSyncing
      // off. No need to re-throw.
      // eslint-disable-next-line no-console
      console.warn("[skills_sync] sync_now failed:", err);
    }
  }, []);

  // The button shows a spinner whenever the engine OR the user's
  // most recent click hasn't resolved yet. Either source is
  // sufficient — they're additive.
  const isSyncing = optimisticSyncing || status?.state === "syncing";

  return { status, syncNow, isSyncing };
}
