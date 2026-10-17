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
  refresh: (parentThreadId: string) => Promise<void>;
  loadProviders: () => Promise<void>;
}

const inFlight = new Map<string, Promise<void>>();
let providersInFlight: Promise<void> | null = null;

export const useFollowUpStore = create<FollowUpStore>((set) => ({
  rows: {},
  providers: null,
  refresh: (parentThreadId) => {
    const running = inFlight.get(parentThreadId);
    if (running) return running;
    const next = followUpList(parentThreadId)
      .then((rows) => set((state) => ({ rows: { ...state.rows, [parentThreadId]: rows } })))
      .catch((error) => console.warn("[follow-ups] list failed", error))
      .finally(() => inFlight.delete(parentThreadId));
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
    if (parent_thread_id in useFollowUpStore.getState().rows) {
      void useFollowUpStore.getState().refresh(parent_thread_id);
    }
  }).catch((error) => {
    listening = null;
    console.warn("[follow-ups] listen failed", error);
  });
}

const EMPTY: FollowUpTaskRow[] = [];

/** The thread's started suggestions, loaded on first use. */
export function useFollowUpRows(parentThreadId: string | null): FollowUpTaskRow[] {
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
