/**
 * Learn which renderer the backend picked for this process.
 *
 * The Linux webview normally runs with accelerated compositing, but it can end
 * up on the legacy CPU renderer instead — through the crash-sentinel fallback
 * or an explicit `CODEMUX_WEBKIT_COMPAT=1` (see
 * `src-tauri/src/webview_tuning.rs`). Composited-only effects have to know:
 * the transcript edge-fade mask is free with a compositor and roughly doubles
 * frame time without one.
 *
 * The answer is fetched once at boot and pushed into the transcript-fade
 * module cache, which notifies its subscribers — so a transcript that mounted
 * before the round trip finished still drops the mask. Off Linux the command
 * answers `"accelerated"`; in the dev mock it is absent, and the default
 * `"accelerated"` stands.
 */
import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

import { setRendererMode } from "@/components/chat/transcript-fade";

/**
 * Probe the backend and cache the result. Failures are swallowed — a missing
 * command means the dev mock or a platform without the compatibility flags,
 * and neither is worth an error path.
 */
export async function loadRendererMode(): Promise<void> {
  try {
    const mode = await invoke<string>("get_renderer_mode");
    setRendererMode(mode === "compatibility" ? "compatibility" : "accelerated");
  } catch {
    // No handler (dev mock) — the accelerated default already applies.
  }
}

/** Boot hook: run the probe once per app session. */
export function useRendererModeInit(): void {
  useEffect(() => {
    void loadRendererMode();
  }, []);
}
