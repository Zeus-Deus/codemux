import { create } from "zustand";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { getOmarchyTheme, onOmarchyThemeChanged } from "@/tauri/omarchy";
import { omarchyToTheme } from "@/lib/omarchy-theme";
import type { ThemeDefinition } from "@/lib/themes";

interface OmarchyState {
  loaded: boolean;
  theme: ThemeDefinition | null;
  load: () => Promise<void>;
}

let loading: Promise<void> | undefined;

/** One subscription for the lifetime of this desktop renderer, not per pane. */
export const useOmarchyStore = create<OmarchyState>()((set) => ({
  loaded: false,
  theme: null,
  load: () => {
    if (loading) return loading;
    loading = (async () => {
      if (isRemoteClient()) { set({ loaded: true }); return; }
      let received = false;
      try {
        await onOmarchyThemeChanged((palette) => {
          received = true;
          set({ theme: omarchyToTheme(palette), loaded: true });
        });
        const palette = await getOmarchyTheme();
        if (!received && palette) set({ theme: omarchyToTheme(palette) });
      } catch {
        // Older backends and desktops without Omarchy keep their manual theme.
      } finally {
        set({ loaded: true });
      }
    })();
    return loading;
  },
}));
