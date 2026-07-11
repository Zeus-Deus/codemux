import { useEffect, useState } from "react";

import { getPresets } from "@/tauri/commands";
import { onPresetsChanged } from "@/tauri/events";
import type { PresetStoreSnapshot } from "@/tauri/types";

/**
 * Live preset-store snapshot: fetches once on mount and re-syncs on
 * every `presets-changed` event. Shared by the GUI-chrome agent launcher
 * and the pinned chat favorite so both track the same source without
 * forking the fetch/subscribe boilerplate (also used inline by
 * `preset-bar.tsx`, which predates this hook).
 */
export function usePresetStore(): PresetStoreSnapshot | null {
  const [presetStore, setPresetStore] = useState<PresetStoreSnapshot | null>(
    null,
  );

  useEffect(() => {
    getPresets()
      .then((s) => setPresetStore(s))
      .catch(console.error);
    const unlisten = onPresetsChanged((snapshot) => setPresetStore(snapshot));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return presetStore;
}
