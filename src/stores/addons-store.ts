import { create } from "zustand";
import type {
  AddonInventory,
  AddonTreeEvent,
  AddonReview,
} from "@/lib/addons/types";
interface AddonsState extends AddonInventory {
  developmentReview: AddonReview | null;
  revoking: Record<string, number>;
  hostEpochs: Record<string, number>;
  loaded: boolean;
  ready: boolean;
  contextRevision: number;
  trees: Record<string, AddonTreeEvent>;
  failures: Record<string, string>;
  accessory: {
    pluginId: string;
    view: string;
    composerId: string;
    workspaceId: string;
  } | null;
}
export const useAddonsStore = create<AddonsState>(() => ({
  developmentReview: null,
  revoking: {},
  hostEpochs: {},
  paused: false,
  installed: [],
  error: null,
  loaded: false,
  ready: false,
  contextRevision: 0,
  trees: {},
  failures: {},
  accessory: null,
}));
export const addonTreeKey = (generation: string, viewId: string) =>
  `${generation}/${viewId}`;
export function clearAddonContext() {
  useAddonsStore.setState((s) => ({
    ready: false,
    trees: {},
    accessory: null,
    contextRevision: s.contextRevision + 1,
  }));
}

/** Fence frontend effects synchronously when the user starts a stop operation. */
export function beginAddonRevocation(id: string) {
  useAddonsStore.setState((s) => ({
    revoking: { ...s.revoking, [id]: (s.revoking[id] ?? 0) + 1 },
  }));
  let released = false;
  return () => {
    if (released) return;
    released = true;
    useAddonsStore.setState((s) => {
      const revoking = { ...s.revoking };
      if ((revoking[id] ?? 0) <= 1) delete revoking[id];
      else revoking[id] -= 1;
      return { revoking };
    });
  };
}
