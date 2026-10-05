/** Default short wheel glides in the Linux desktop webview. */
import { useEffect } from "react";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { isLinuxWebKitGtk } from "@/lib/webkit";
import { installWheelScrolling } from "@/lib/wheel-scrolling";

export function useSmoothScrollingInit(): void {
  useEffect(() => {
    // Other platforms and remote browsers own their native wheel animation.
    // WebKit's animation stays off in the backend; the app handles short
    // wheel glides without requiring a setting or waiting for persisted data.
    if (isRemoteClient() || !isLinuxWebKitGtk(navigator.userAgent)) return;
    return installWheelScrolling();
  }, []);
}
