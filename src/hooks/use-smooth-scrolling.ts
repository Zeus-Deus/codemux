/**
 * Smooth (animated) wheel scrolling for the Linux WebKitGTK webview.
 *
 * The webview ships with kinetic wheel animation off by default: the animation
 * runs on a fixed timeline, so a high-resolution wheel that emits many small
 * deltas per flick queues up animations and the page ends up moving *slower*
 * the faster you scroll. Users who prefer the animated feel can turn it back
 * on in Settings → Appearance; the preference lives in the machine-local
 * settings store (`appearance.smooth_scrolling`) and is pushed to the webview
 * through the `set_smooth_scrolling` command. No effect off Linux.
 */
import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";

import { useSettingsStore, selectSmoothScrolling } from "@/stores/settings-store";

/**
 * Push the preference to the webview. Failures are logged, never thrown — the
 * command is Linux-only and absent from the dev mock, and a webview setting is
 * never worth breaking a render or a settings write over.
 */
export async function applySmoothScrolling(enabled: boolean): Promise<void> {
  try {
    await invoke<void>("set_smooth_scrolling", { enabled });
  } catch (err) {
    console.error("[smooth-scrolling] set_smooth_scrolling failed:", err);
  }
}

/**
 * Boot hook: once the machine-local settings have loaded, re-apply a persisted
 * "on" to the fresh webview. Off is the native default, so it needs no call.
 * Runs at most once per app session — later changes are pushed by the Settings
 * toggle itself.
 */
export function useSmoothScrollingInit(): void {
  const loaded = useSettingsStore((s) => s.loaded);
  const enabled = useSettingsStore(selectSmoothScrolling);
  const applied = useRef(false);

  useEffect(() => {
    if (!loaded || applied.current) return;
    applied.current = true;
    if (!enabled) return;
    void applySmoothScrolling(true);
  }, [loaded, enabled]);
}
