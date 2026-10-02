import { useState, useSyncExternalStore } from "react";

type HighlightUpdate = string | null | ((current: string | null) => string | null);

export interface PopupHighlightStore {
  get: () => string | null;
  set: (next: HighlightUpdate) => void;
  subscribe: (listener: () => void) => () => void;
}

/**
 * Popup highlight held outside composer state. Arrow-key moves then
 * re-render only the popup subscribed to it instead of the whole
 * composer, which is what lets a held key keep up with OS repeat.
 */
export function usePopupHighlightStore(): PopupHighlightStore {
  const [store] = useState<PopupHighlightStore>(() => {
    let current: string | null = null;
    const listeners = new Set<() => void>();
    return {
      get: () => current,
      set: (next) => {
        const value = typeof next === "function" ? next(current) : next;
        if (value === current) return;
        current = value;
        listeners.forEach((listener) => listener());
      },
      subscribe: (listener) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    };
  });
  return store;
}

export function usePopupHighlight(store: PopupHighlightStore): string | null {
  return useSyncExternalStore(store.subscribe, store.get);
}

/** Re-renders only when `select`'s primitive result changes, not on every move. */
export function usePopupHighlightSelector<T extends string | boolean | null>(
  store: PopupHighlightStore,
  select: (id: string | null) => T,
): T {
  return useSyncExternalStore(store.subscribe, () => select(store.get()));
}
