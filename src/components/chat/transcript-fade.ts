import { isLinuxWebKitGtk } from "@/lib/webkit";

/**
 * Should the transcript viewport's edge-fade mask (`WS_FADE_STYLE` in
 * `MessageList.tsx`) be applied? (issue #129)
 *
 * The fade is a CSS `mask-image` on the scroll viewport. On a COMPOSITED
 * webview the mask is a cheap GPU layer effect, so it stays on (macOS
 * WKWebView, Windows WebView2, the dev-mock Chromium — byte-identical
 * rendering). But on Linux the app forces WebKitGTK into NON-COMPOSITED (CPU)
 * mode via `WEBKIT_DISABLE_COMPOSITING_MODE=1` / `WEBKIT_DISABLE_DMABUF_RENDERER=1`
 * (`src-tauri/src/lib.rs`), where a viewport mask forces a full-viewport CPU
 * re-rasterization on every scroll frame — the transcript-scroll jank in the
 * issue. So the fade is gated OFF on Linux WebKitGTK; the design intent is
 * kept everywhere it is composited.
 */

/**
 * `localStorage` key for the manual fade override. Values: `"on"` forces the
 * mask, `"off"` disables it; anything else is ignored. Mirrors
 * `RENDERER_OVERRIDE_STORAGE_KEY` in `webgl-renderer-probe.ts` — a live A/B
 * escape hatch so a profiling session can toggle the fade from devtools
 * without a rebuild.
 */
export const TRANSCRIPT_FADE_STORAGE_KEY = "codemux:transcript-fade";

/** Read the manual override, tolerating unavailable storage (private mode,
 *  sandbox) — no override then. */
export function readTranscriptFadeOverride(): "on" | "off" | null {
  try {
    const value = globalThis.localStorage?.getItem(TRANSCRIPT_FADE_STORAGE_KEY);
    return value === "on" || value === "off" ? value : null;
  } catch {
    return null; // storage unavailable — no override
  }
}

/**
 * Pure fade decision. Override wins first (both directions); otherwise the
 * fade is off on Linux WebKitGTK and on everywhere else. Injectable inputs
 * with production defaults so it is unit-testable.
 */
export function decideTranscriptFade(
  userAgent: string = typeof navigator !== "undefined"
    ? navigator.userAgent
    : "",
  override: "on" | "off" | null = readTranscriptFadeOverride(),
): boolean {
  if (override === "on") return true;
  if (override === "off") return false;
  return !isLinuxWebKitGtk(userAgent);
}

let cachedFade: boolean | null = null;

/**
 * Cached production entry point: decide once per app session (the webview
 * engine doesn't change at runtime). Logs the verdict once — but only when
 * the fade is disabled or overridden, so the default composited path stays
 * quiet. Mirrors `shouldLoadWebglAddon`'s one-time logging style.
 */
export function transcriptFadeEnabled(): boolean {
  if (cachedFade !== null) return cachedFade;
  const override = readTranscriptFadeOverride();
  cachedFade = decideTranscriptFade(
    typeof navigator !== "undefined" ? navigator.userAgent : "",
    override,
  );
  // Suppress the log under vitest: jsdom's UA (`AppleWebKit/… jsdom/…`, no
  // Chrome/Mac token) classifies as WebKitGTK, so the "disabled" branch would
  // otherwise fire on every test run.
  if ((!cachedFade || override !== null) && import.meta.env?.MODE !== "test") {
    console.info(
      `[codemux::transcript] edge-fade mask ${
        cachedFade ? "enabled" : "disabled"
      } — ${
        override !== null
          ? `forced by localStorage["${TRANSCRIPT_FADE_STORAGE_KEY}"]="${override}"`
          : `Linux WebKitGTK runs non-composited (see src-tauri/src/lib.rs), where the ` +
            `viewport mask forces a full-viewport CPU re-rasterization per scroll frame; ` +
            `set localStorage["${TRANSCRIPT_FADE_STORAGE_KEY}"]="on" to force it`
      }`,
    );
  }
  return cachedFade;
}

/** Test hook: clear the module-level fade cache. */
export function resetTranscriptFadeCacheForTests(): void {
  cachedFade = null;
}

/**
 * Change C (issue #129): one-shot content-visibility support diagnostic. Row
 * containment (`[content-visibility:auto]` on each `MessageScrollerItem`) is
 * what keeps thousands of rows cheap; on old WebKitGTK builds the property may
 * be a silent no-op, voiding that assumption. Warn once (grep-able on the
 * "content-visibility" string) so a profiling session can spot it. Suppressed
 * under vitest — jsdom's `CSS.supports` reports false, which would warn on
 * every run — and fires at most once per module load.
 */
function warnIfContentVisibilityUnsupported(): void {
  if (import.meta.env?.MODE === "test") return;
  if (typeof CSS !== "undefined" && CSS.supports?.("content-visibility", "auto")) {
    return;
  }
  console.warn(
    "[codemux::transcript] content-visibility:auto is unsupported in this " +
      "webview engine — transcript row containment is inactive, so long " +
      "threads may not stay cheap.",
  );
}

warnIfContentVisibilityUnsupported();
