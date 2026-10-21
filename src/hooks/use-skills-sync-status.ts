// Combine live sync events with an initial snapshot and a refresh after
// each manual command settles. Skipped syncs and IPC failures may emit
// no completion event, so optimism tracks pending commands separately.
// Convert wire-format millis fields to Dates for the display component.

import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@/tauri/events";

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

// Adapt the wrapper's Tauri events to payload callbacks.
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
          s.lastSyncAtMillis != null ? new Date(s.lastSyncAtMillis) : null,
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
  const [pendingSyncs, setPendingSyncs] = useState(0);
  const lifecycleRef = useRef<{ revision: number } | null>(null);

  const refreshStatus = useCallback(async () => {
    const lifecycle = lifecycleRef.current;
    if (!lifecycle) return;
    // A newer read or live event supersedes this snapshot. The lifecycle
    // identity also rejects requests from before a Strict Mode remount.
    const revision = ++lifecycle.revision;
    try {
      const snap = await skillsSyncStatus();
      if (lifecycleRef.current === lifecycle && lifecycle.revision === revision) {
        setStatus(snapshotToUi(snap));
      }
    } catch (err) {
      if (lifecycleRef.current === lifecycle && lifecycle.revision === revision) {
        // Keep the last known state, or the skeleton before the first read.
        // eslint-disable-next-line no-console
        console.warn("[skills_sync] status fetch failed:", err);
      }
    }
  }, []);

  useEffect(() => {
    const lifecycle = { revision: 0 };
    lifecycleRef.current = lifecycle;
    setPendingSyncs(0);
    let unlisten: UnlistenFn | null = null;
    let hasLiveStatus = false;
    void onSkillsSyncStateChanged((payload) => {
      if (lifecycleRef.current !== lifecycle) return;
      hasLiveStatus = true;
      lifecycle.revision += 1;
      setStatus(snapshotToUi(payload));
    }).then((fn) => {
      if (lifecycleRef.current !== lifecycle) {
        fn();
        return;
      }
      unlisten = fn;
      // Subscribe before reading so a completion cannot fall in the gap.
      // A live event already received during registration takes precedence.
      if (!hasLiveStatus) void refreshStatus();
    }).catch((err) => {
      if (lifecycleRef.current !== lifecycle) return;
      console.error("[skills_sync] listener registration failed:", err);
      void refreshStatus();
    });
    return () => {
      lifecycleRef.current = null;
      unlisten?.();
    };
  }, [refreshStatus]);

  const syncNow = useCallback(async () => {
    const lifecycle = lifecycleRef.current;
    if (!lifecycle) return;
    setPendingSyncs((count) => count + 1);
    try {
      await skillsSyncNow();
    } catch (err) {
      if (lifecycleRef.current === lifecycle) {
        // eslint-disable-next-line no-console
        console.warn("[skills_sync] sync_now failed:", err);
      }
    } finally {
      if (lifecycleRef.current === lifecycle) {
        setPendingSyncs((count) => count - 1);
        await refreshStatus();
      }
    }
  }, [refreshStatus]);

  // A settled command can still observe another sync in progress.
  const isSyncing = pendingSyncs > 0 || status?.state === "syncing";

  return { status, syncNow, isSyncing };
}
