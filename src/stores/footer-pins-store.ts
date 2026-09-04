import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import {
  getFooterAction,
  isFooterIconId,
  type FooterActionId,
  type FooterIconId,
} from "@/lib/footer-actions";

export interface FooterPin {
  id: FooterActionId;
  iconId?: FooterIconId;
}
export const DEFAULT_FOOTER_PINS: FooterPin[] = [
  { id: "codemux.automations.open" },
  { id: "codemux.devices.open" },
  { id: "codemux.pull-requests.open" },
  { id: "codemux.ports.open" },
];
export const FOOTER_STORAGE_KEY = "codemux:footer-pins:v1";

/** Keep known but temporarily unavailable pins in place; discard stale IDs,
 * duplicates and untrusted icon values without sorting the user's order. */
export function validateFooterPins(value: unknown): FooterPin[] {
  if (!Array.isArray(value)) return [...DEFAULT_FOOTER_PINS];
  const seen = new Set<string>();
  const pins: FooterPin[] = [];
  for (const entry of value) {
    if (!entry || typeof entry !== "object" || typeof entry.id !== "string")
      continue;
    const action = getFooterAction(entry.id);
    if (!action || seen.has(action.id)) continue;
    seen.add(action.id);
    pins.push({
      id: action.id,
      ...(typeof entry.iconId === "string" && isFooterIconId(entry.iconId)
        ? { iconId: entry.iconId }
        : {}),
    });
  }
  return pins;
}
interface FooterPinsState {
  pins: FooterPin[];
  togglePin: (id: FooterActionId) => void;
  movePin: (id: FooterActionId, offset: -1 | 1) => void;
  setIcon: (id: FooterActionId, iconId?: FooterIconId) => void;
  reset: () => void;
}
export const useFooterPinsStore = create<FooterPinsState>()(
  persist(
    (set) => ({
      pins: [...DEFAULT_FOOTER_PINS],
      togglePin: (id) =>
        set((state) => ({
          pins: state.pins.some((pin) => pin.id === id)
            ? state.pins.filter((pin) => pin.id !== id)
            : validateFooterPins([...state.pins, { id }]),
        })),
      movePin: (id, offset) =>
        set((state) => {
          const pins = [...state.pins];
          const from = pins.findIndex((pin) => pin.id === id);
          const to = from + offset;
          if (from < 0 || to < 0 || to >= pins.length) return state;
          [pins[from], pins[to]] = [pins[to], pins[from]];
          return { pins };
        }),
      setIcon: (id, iconId) =>
        set((state) => ({
          pins: validateFooterPins(
            state.pins.map((pin) => (pin.id === id ? { id, iconId } : pin)),
          ),
        })),
      reset: () => set({ pins: [...DEFAULT_FOOTER_PINS] }),
    }),
    {
      name: FOOTER_STORAGE_KEY,
      version: 1,
      storage: createJSONStorage(() => ({
        getItem: (key) => {
          try {
            return localStorage.getItem(key);
          } catch {
            return null;
          }
        },
        setItem: (key, value) => {
          try {
            localStorage.setItem(key, value);
          } catch {
            /* Keep working when browser storage is unavailable. */
          }
        },
        removeItem: (key) => {
          try {
            localStorage.removeItem(key);
          } catch {
            /* No storage. */
          }
        },
      })),
      partialize: (state) => ({ pins: state.pins }),
      migrate: () => ({ pins: [...DEFAULT_FOOTER_PINS] }),
      merge: (persisted, current) => ({
        ...current,
        pins: validateFooterPins(
          (persisted as { pins?: unknown } | null)?.pins,
        ),
      }),
    },
  ),
);
