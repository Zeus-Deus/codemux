/**
 * Should the transcript viewport's edge-fade mask (`WS_FADE_STYLE` in
 * `MessageList.tsx`) be applied? (issue #129)
 *
 * The fade is a CSS `mask-image` on the scroll viewport. On a COMPOSITED
 * webview the mask is a cheap GPU layer effect. It used to be gated off on
 * Linux, where the app ran WebKitGTK in non-composited (CPU) mode and a
 * viewport mask forced a full-viewport re-rasterization on every scroll frame.
 * Accelerated compositing is now restored on Linux (see the webview env setup
 * in `src-tauri/src/lib.rs`), and profiling shows the mask costs no measurable
 * frame time there — same ~16ms frames with and without it. So the fade is on
 * by default everywhere; the localStorage override below stays as the escape
 * hatch for anyone on a driver stack where it still hurts.
 *
 * The one automatic exception is the compatibility (CPU) renderer — the crash
 * sentinel fallback or `CODEMUX_WEBKIT_COMPAT=1`. There is no compositor to
 * hand the mask to, so it costs roughly double the frame time and is switched
 * off. The backend reports which renderer it picked through the
 * `get_renderer_mode` command; `use-renderer-mode.ts` pushes the answer into
 * the module cache below at boot.
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
 * Which renderer the backend picked for this process. `"accelerated"` until
 * `get_renderer_mode` says otherwise — the accelerated path is both the
 * default and the only mode that exists off Linux, so an unanswered probe
 * (dev mock, missing command) leaves the design intent intact.
 */
export type RendererMode = "accelerated" | "compatibility";

let rendererMode: RendererMode = "accelerated";

/** Consumers re-reading the decision when the renderer mode lands. */
const fadeListeners = new Set<() => void>();

/**
 * Record the renderer the backend actually ended up on. Invalidates the
 * cached decision and notifies subscribers, so a mode that arrives after the
 * transcript has mounted still turns the mask off.
 */
export function setRendererMode(mode: RendererMode): void {
  if (mode === rendererMode) return;
  rendererMode = mode;
  cachedFade = null;
  for (const listener of fadeListeners) listener();
}

/** Current renderer mode as last reported by the backend. */
export function getRendererMode(): RendererMode {
  return rendererMode;
}

/**
 * `useSyncExternalStore` subscribe half; `transcriptFadeEnabled` is the
 * matching snapshot getter (cached, so it is referentially stable).
 */
export function subscribeTranscriptFade(listener: () => void): () => void {
  fadeListeners.add(listener);
  return () => {
    fadeListeners.delete(listener);
  };
}

/**
 * Pure fade decision. Override wins first (both directions); otherwise the
 * fade is on wherever the webview is composited and off on the compatibility
 * (CPU) renderer, where a full-viewport mask forces a re-rasterization every
 * scroll frame. The renderer mode is the only engine axis: it can only ever
 * say "compatibility" on the Linux desktop webview (remote clients skip the
 * probe — their browser is composited), so no user-agent input is needed.
 * Injectable inputs with production defaults so it is unit-testable.
 */
export function decideTranscriptFade(
  override: "on" | "off" | null = readTranscriptFadeOverride(),
  mode: RendererMode = rendererMode,
): boolean {
  if (override === "on") return true;
  if (override === "off") return false;
  return mode !== "compatibility";
}

let cachedFade: boolean | null = null;

/**
 * Cached production entry point: decide once per app session (the webview
 * engine doesn't change at runtime; the renderer mode is settled once, at
 * boot, and invalidates this cache when it lands). Logs the verdict once —
 * but only when an override is in play, so the default path stays quiet.
 * Mirrors `shouldLoadWebglAddon`'s one-time logging style.
 */
export function transcriptFadeEnabled(): boolean {
  if (cachedFade !== null) return cachedFade;
  const override = readTranscriptFadeOverride();
  cachedFade = decideTranscriptFade(override, rendererMode);
  if (override !== null && import.meta.env?.MODE !== "test") {
    console.info(
      `[codemux::transcript] edge-fade mask ${
        cachedFade ? "enabled" : "disabled"
      } — forced by localStorage["${TRANSCRIPT_FADE_STORAGE_KEY}"]="${override}"`,
    );
  }
  return cachedFade;
}

/** Test hook: clear the module-level fade cache and renderer mode. */
export function resetTranscriptFadeCacheForTests(): void {
  cachedFade = null;
  rendererMode = "accelerated";
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
