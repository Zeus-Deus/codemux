import { create } from "zustand";
import type {
  AddonInventory,
  AddonTreeEvent,
  AddonReview,
} from "@/lib/addons/types";
interface AddonsState extends AddonInventory {
  developmentReview: AddonReview | null;
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
