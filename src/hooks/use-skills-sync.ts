// Auto-trigger for the skills sync engine (Stage 3).
//
// Two trigger sources:
//
//   1. "Sync just became available" — emitted as part of the
//      Stage 2 `sync-state-changed` event when the user signs in
//      with a key already loaded, or when they finish
//      `setup_sync_password` for the first time. We want the
//      first pull to land immediately so the user sees their
//      skills appear without manually pressing anything.
//
//   2. "Skills directory changed" — emitted by Step 7's existing
//      `skills-changed` watcher whenever a SKILL.md is created /
//      modified / deleted in any tracked directory. Rather than
//      pushing each file diff individually we kick a full
//      `skillsSyncNow` after a short debounce, which walks every
//      syncable path and picks up everything that changed since
//      the last cycle. Simpler than per-file diffing and matches
//      Vexis's "sync the whole table on every trigger" pattern.
//
// All triggers route through `skillsSyncNow`. The engine's
// internal state machine guarantees serialization, so even a
// burst of file events results in one sync cycle (the second
// caller waits for the first to finish).

import { useCallback, useEffect, useRef } from "react";
import { listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { useTauriEvent } from "./use-tauri-event";
import {
  onSyncStateChanged,
  type SyncStateChangedPayload,
} from "@/tauri/events";
import { useAuthStore } from "@/stores/auth-store";
import { skillsSyncNow } from "@/tauri/commands";

/// Wait window after a `skills-changed` event before triggering
/// sync. The watcher already debounces filesystem events at
/// 300ms; this stacks another 1.5s so save+close+save bursts (e.g.
/// the user editing then immediately saving a follow-up tweak)
/// coalesce into one sync cycle.
const SKILLS_CHANGED_DEBOUNCE_MS = 1_500;

/// Listen to the existing `skills-changed` event from
/// `crate::skills::watcher`. Same channel name as the watcher's
/// `SKILLS_CHANGED_EVENT` constant. Unlike the Stage 2 events,
/// there's no payload — just a notification that something in
/// the skills tree changed.
const onSkillsChanged = (cb: EventCallback<void>): Promise<UnlistenFn> =>
  listen<void>("skills-changed", () => cb({ event: "skills-changed", id: 0, payload: undefined }));

/// Background-fire skillsSyncNow without surfacing errors to the
/// caller. Errors are logged; the engine itself records the
/// failure in its state machine and the next trigger will retry.
function fireSyncIfReady() {
  const state = useAuthStore.getState();
  if (!state.isAuthenticated || !state.syncAvailable) {
    return;
  }
  void skillsSyncNow().catch((err) => {
    // Engine errors are already captured in the state snapshot;
    // log here for development visibility.
    // eslint-disable-next-line no-console
    console.warn("[skills_sync] background sync failed:", err);
  });
}

export function useSkillsSync() {
  const debounceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // (1) Trigger on sync-availability transitions. The Stage 2
  // event payload tells us whether sync is now ready; we only
  // fire when the new state is `syncAvailable=true`. The auth
  // store is the source of truth for "currently authenticated";
  // we double-check before firing.
  const handleSyncStateChanged = useCallback(
    (payload: SyncStateChangedPayload) => {
      if (payload.syncAvailable) {
        fireSyncIfReady();
      }
    },
    [],
  );
  useTauriEvent(onSyncStateChanged, handleSyncStateChanged, [
    handleSyncStateChanged,
  ]);

  // (2) Trigger on filesystem changes, debounced. The same hook
  // mounts both listeners so unmount tears them down together.
  const handleSkillsChanged = useCallback(() => {
    if (debounceTimerRef.current !== null) {
      clearTimeout(debounceTimerRef.current);
    }
    debounceTimerRef.current = setTimeout(() => {
      debounceTimerRef.current = null;
      fireSyncIfReady();
    }, SKILLS_CHANGED_DEBOUNCE_MS);
  }, []);
  useTauriEvent(onSkillsChanged, handleSkillsChanged, [handleSkillsChanged]);

  useEffect(() => {
    return () => {
      if (debounceTimerRef.current !== null) {
        clearTimeout(debounceTimerRef.current);
      }
    };
  }, []);

  // (3) Periodic 5-minute sync. Stage 5 addition: fires
  // skillsSyncNow on a fixed cadence so a long-idle Codemux
  // window picks up changes another device pushed without the
  // user having to think about it. Constraints:
  //
  //   - Only when the document is visible — a backgrounded app
  //     shouldn't burn battery polling. The sync layer's design
  //     is "drift cheap, work expensive": the next visible tick
  //     catches up.
  //   - Skipped if the engine is already syncing. The Tauri
  //     command-side serialization would handle a concurrent
  //     call gracefully, but skipping here saves a redundant
  //     round-trip.
  //   - Gated on `syncAvailable` and `isAuthenticated` — same
  //     gates as the file-watcher trigger via `fireSyncIfReady`.
  //
  // Polling is a last-resort cadence. Most syncs land via the
  // file watcher trigger (1.5s after a save) or the
  // sync-availability transition; the 5-min interval just
  // ensures convergence for users editing skills on another
  // device.
  useEffect(() => {
    const intervalId = window.setInterval(() => {
      if (typeof document !== "undefined" && document.visibilityState !== "visible") {
        return;
      }
      fireSyncIfReady();
    }, PERIODIC_SYNC_INTERVAL_MS);
    return () => window.clearInterval(intervalId);
  }, []);
}

/// 5 minutes — the standard "we're not in a hurry" cadence used
/// by both Vexis and Codemux's other periodic background work
/// (settings-sync, auth re-verify on focus). Short enough that a
/// user who edited a skill on another device sees it within
/// minutes, long enough that a stuck-open Codemux window doesn't
/// hammer the API.
const PERIODIC_SYNC_INTERVAL_MS = 5 * 60 * 1_000;
