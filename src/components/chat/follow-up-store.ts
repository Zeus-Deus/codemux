import { useEffect } from "react";
import { create } from "zustand";

import {
  followUpList,
  followUpProviders,
  type FollowUpProvider,
  type FollowUpTaskRow,
} from "@/tauri/commands";
import { onFollowUpChanged } from "@/tauri/events";

/**
 * Started suggested tasks per parent thread, read from the backend's durable
 * rows (`follow_up_list`) and re-read on `follow-up-changed`. Shared by every
 * suggestions card of a thread. The providers a task can run on are read
 * once per app session.
 */
interface FollowUpStore {
  rows: Record<string, FollowUpTaskRow[]>;
  providers: FollowUpProvider[] | null;
  /** Re-read the thread's rows. A call while a read is in flight queues one
   *  trailing read, so a change the in-flight read may have missed still
   *  lands; the promise settles once the thread's rows are current. */
  refresh: (parentThreadId: string) => Promise<void>;
  loadProviders: () => Promise<void>;
}

/** Threads someone has asked for: change events keep these fresh, even
 *  before their first read has returned. */
const watched = new Set<string>();
/** The thread's current read loop, while one runs. */
const inFlight = new Map<string, Promise<void>>();
/** Threads that changed after their in-flight read started. */
const dirty = new Set<string>();
let providersInFlight: Promise<void> | null = null;

export const useFollowUpStore = create<FollowUpStore>((set) => ({
  rows: {},
  providers: null,
  refresh: (parentThreadId) => {
    watched.add(parentThreadId);
    const running = inFlight.get(parentThreadId);
    if (running) {
      dirty.add(parentThreadId);
      return running;
    }
    const loop = async () => {
      try {
        do {
          dirty.delete(parentThreadId);
          try {
            const rows = await followUpList(parentThreadId);
            // Shown at once; a queued read below overwrites them.
            set((state) => ({
              rows: { ...state.rows, [parentThreadId]: rows },
            }));
          } catch (error) {
            console.warn("[follow-ups] list failed", error);
          }
        } while (dirty.has(parentThreadId));
      } finally {
        inFlight.delete(parentThreadId);
      }
    };
    const next = loop();
    inFlight.set(parentThreadId, next);
    return next;
  },
  loadProviders: () => {
    providersInFlight ??= followUpProviders()
      .then((providers) => set({ providers }))
      .catch((error) => {
        providersInFlight = null;
        console.warn("[follow-ups] providers failed", error);
      });
    return providersInFlight;
  },
}));

let listening: Promise<unknown> | null = null;
function listenOnce() {
  listening ??= onFollowUpChanged(({ parent_thread_id }) => {
    // Only threads someone has looked at are kept fresh.
    if (watched.has(parent_thread_id)) {
      void useFollowUpStore.getState().refresh(parent_thread_id);
    }
  }).catch((error) => {
    listening = null;
    console.warn("[follow-ups] listen failed", error);
  });
}

const EMPTY: FollowUpTaskRow[] = [];

/** The thread's started suggestions, loaded on first use. */
export function useFollowUpRows(
  parentThreadId: string | null,
): FollowUpTaskRow[] {
  const rows = useFollowUpStore((state) =>
    parentThreadId ? (state.rows[parentThreadId] ?? EMPTY) : EMPTY,
  );
  useEffect(() => {
    if (!parentThreadId) return;
    listenOnce();
    void useFollowUpStore.getState().refresh(parentThreadId);
  }, [parentThreadId]);
  return rows;
}

export function useFollowUpProviders(): FollowUpProvider[] | null {
  const providers = useFollowUpStore((state) => state.providers);
  useEffect(() => {
    void useFollowUpStore.getState().loadProviders();
  }, []);
  return providers;
}

/** Test seam: forget module-level state between tests. */
export function resetFollowUpStoreForTests() {
  watched.clear();
  inFlight.clear();
  dirty.clear();
  providersInFlight = null;
  listening = null;
  useFollowUpStore.setState({ rows: {}, providers: null });
}
