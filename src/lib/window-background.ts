import { invoke } from "@tauri-apps/api/core";

import { isRemoteClient } from "@/components/remote/is-remote-client";

/**
 * Keeps the native window's fill in step with the active palette.
 *
 * `tauri.conf.json` can only name one `backgroundColor`, and that value is
 * what the OS paints from the moment the window maps until the webview hands
 * over its first frame. It stays Graphite's near-black — the right answer for
 * a first launch, where nothing is stored yet — so on every launch after the
 * user picks a light theme the window would flash black before going white.
 * Pushing the palette's background down to the window closes that gap.
 *
 * Fire-and-forget on purpose: this is chrome, not state. It is also skipped
 * for the web remote, where the "window" belongs to a desktop somewhere else
 * and must not follow this browser's theme.
 */
export function syncWindowBackground(background: string): void {
  if (typeof window === "undefined") return;
  if (isRemoteClient()) return;
  if (!("__TAURI_INTERNALS__" in window)) return;
  void invoke("set_window_background", { color: background }).catch(() => {});
}
