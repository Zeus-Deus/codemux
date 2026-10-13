/** Default short wheel glides in the Linux desktop webview. */
import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { isLinuxWebKitGtk } from "@/lib/webkit";
import { installWheelScrolling } from "@/lib/wheel-scrolling";

export function useSmoothScrollingInit(): void {
  useEffect(() => {
    // Other platforms and remote browsers own their native wheel animation.
    // WebKit's animation stays off in the backend; the app handles short
    // wheel glides without requiring a setting or waiting for persisted data.
    if (isRemoteClient() || !isLinuxWebKitGtk(navigator.userAgent)) return;
    let live = true;
    let precise = false;
    const stop = installWheelScrolling(document, () => precise);
    void invoke<boolean>("get_precise_wheel_available").then((available) => {
      if (live) precise = available === true;
    }).catch(() => { /* Older backends and the dev mock keep the baseline. */ });
    return () => { live = false; stop(); };
  }, []);
}
