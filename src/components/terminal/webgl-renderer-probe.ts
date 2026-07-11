/**
 * Decide whether xterm's WebGL renderer addon should load, before loading it.
 *
 * Why this exists: the WebGL addon is the fastest xterm renderer **only when
 * the GL stack is hardware-accelerated and the engine composites it without
 * extra latency**. Two failure modes ship as per-keystroke input lag while
 * `new WebglAddon()` succeeds (so the addon's own try/catch + onContextLoss
 * fallbacks never trigger):
 *
 * 1. **Software WebGL.** The WebView silently backs WebGL2 with a software
 *    rasterizer (SwiftShader / llvmpipe / swrast — VMs without GPU
 *    passthrough, blocklisted drivers). Every frame is CPU-rasterized plus a
 *    full-canvas composite; for interactive typing that is *slower* than
 *    xterm's DOM renderer. Detectable on engines with honest
 *    `WEBGL_debug_renderer_info` strings (Chromium-family: Windows WebView2,
 *    the dev-mock browser).
 *
 * 2. **WebKitGTK — the Linux app webview.** Modern WebKitGTK *masks* the
 *    renderer string for fingerprinting resistance (it reports literally
 *    "Apple GPU"/"Apple Inc." on Linux boxes), so hardware backing is
 *    unverifiable from inside the page — and this app has been burned twice
 *    by WebGL-addon input lag on this engine (fixed in `acd1566` "restore
 *    low-latency pane input" v0.1.33; regressed again in v0.9.0 when the
 *    addon was re-enabled). On WebKitGTK the DOM renderer is the known-good
 *    latency profile, so it wins by policy.
 *
 * Verdict policy:
 * - explicit `localStorage` override        → honored (live A/B escape hatch)
 * - no WebGL2 context                       → DOM (the addon would throw anyway)
 * - software renderer string                → DOM (lower input latency)
 * - Linux WebKitGTK (non-Chromium WebKit)   → DOM (unverifiable + lag history)
 * - hardware / unknown elsewhere            → WebGL (keep the perf win; the
 *   addon's try/catch and onContextLoss still cover hard failures)
 *
 * macOS WKWebView intentionally stays on WebGL: its "Apple GPU" string is
 * true there and WebKit's GL compositing is Metal-backed and well-integrated.
 *
 * The decision runs once per app session (module-level cache): renderer
 * backing doesn't change at runtime, and creating a throwaway GL context per
 * pane mount would be wasteful on the very machines this protects.
 */

import { isLinuxWebKitGtk } from "@/lib/webkit";

// `isLinuxWebKitGtk` moved to `@/lib/webkit` (shared with the chat transcript
// fade gate, issue #129). Re-exported here so existing importers/tests that
// pull it from this module keep working.
export { isLinuxWebKitGtk };

/**
 * Known software-rasterizer markers in `UNMASKED_RENDERER_WEBGL` strings.
 *
 * - SwiftShader / Subzero — Chromium-family software GL
 *   (e.g. "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero)
 *   (0x0000C0DE)), SwiftShader driver)")
 * - llvmpipe / softpipe / swrast — Mesa software paths
 * - "software" — generic ("Software Rasterizer", "Apple Software Renderer",
 *   ANGLE "... software adapter" strings)
 * - "basic render" — Windows "Microsoft Basic Render Driver"
 *
 * A false positive only costs falling back to the DOM renderer — the safe,
 * pre-v0.9.0 behavior — so the pattern errs toward matching.
 */
export const SOFTWARE_GL_RENDERER_PATTERN =
  /swiftshader|subzero|llvmpipe|softpipe|swrast|software|basic render/i;

/**
 * `localStorage` key for the manual renderer override. Values: `"webgl"`
 * forces the addon (bypasses every probe check except WebGL2 existing),
 * `"dom"` forces the DOM renderer. Anything else is ignored. Lets a lag
 * report be A/B-tested live from devtools without a rebuild.
 */
export const RENDERER_OVERRIDE_STORAGE_KEY = "codemux:terminal-renderer";

export type WebglProbeResult =
  | { use: true; renderer: string | null; reason: "hardware" | "override" }
  | {
      use: false;
      reason: "unavailable" | "software" | "webkitgtk" | "override";
      renderer: string | null;
    };

/** Minimal slice of WebGL2RenderingContext the probe touches (injectable in tests). */
export interface ProbeGlContext {
  getExtension(name: string): { UNMASKED_RENDERER_WEBGL?: number; loseContext?: () => void } | null;
  getParameter(pname: number): unknown;
  RENDERER: number;
}

export type ProbeContextFactory = () => ProbeGlContext | null;

function defaultContextFactory(): ProbeGlContext | null {
  if (typeof document === "undefined") return null;
  const canvas = document.createElement("canvas");
  // Match the addon's requirement: it renders with WebGL2 specifically.
  return canvas.getContext("webgl2") as ProbeGlContext | null;
}

function readRendererOverride(): "webgl" | "dom" | null {
  try {
    const value = globalThis.localStorage?.getItem(RENDERER_OVERRIDE_STORAGE_KEY);
    return value === "webgl" || value === "dom" ? value : null;
  } catch {
    return null; // storage unavailable (private mode, sandbox) — no override
  }
}

/**
 * Probe the WebGL2 stack and decide whether xterm's WebGL addon should load.
 * Pure given the injected inputs; see `shouldLoadWebglAddon` for the cached
 * production entry point.
 */
export function probeWebglRenderer(
  createContext: ProbeContextFactory = defaultContextFactory,
  userAgent: string = typeof navigator !== "undefined" ? navigator.userAgent : "",
  override: "webgl" | "dom" | null = readRendererOverride(),
): WebglProbeResult {
  if (override === "dom") {
    return { use: false, reason: "override", renderer: null };
  }

  let gl: ProbeGlContext | null = null;
  try {
    gl = createContext();
  } catch {
    gl = null;
  }
  if (!gl) return { use: false, reason: "unavailable", renderer: null };

  let renderer: string | null = null;
  try {
    const debugInfo = gl.getExtension("WEBGL_debug_renderer_info");
    const pname =
      debugInfo?.UNMASKED_RENDERER_WEBGL !== undefined
        ? debugInfo.UNMASKED_RENDERER_WEBGL
        : gl.RENDERER;
    const value = gl.getParameter(pname);
    renderer = typeof value === "string" ? value : null;
  } catch {
    renderer = null;
  } finally {
    // Release the throwaway context's GPU/driver resources promptly instead
    // of waiting for canvas GC.
    try {
      gl.getExtension("WEBGL_lose_context")?.loseContext?.();
    } catch {
      // Best-effort cleanup only.
    }
  }

  if (override === "webgl") {
    return { use: true, renderer, reason: "override" };
  }
  if (renderer !== null && SOFTWARE_GL_RENDERER_PATTERN.test(renderer)) {
    return { use: false, reason: "software", renderer };
  }
  if (isLinuxWebKitGtk(userAgent)) {
    return { use: false, reason: "webkitgtk", renderer };
  }
  return { use: true, renderer, reason: "hardware" };
}

let cachedResult: WebglProbeResult | null = null;

const DECLINE_REASON_DETAIL: Record<
  Exclude<WebglProbeResult["reason"], "hardware">,
  (renderer: string | null) => string
> = {
  unavailable: () => "WebGL2 is unavailable",
  software: (r) =>
    `WebGL2 is software-rendered (${r}); the DOM renderer has lower input latency there`,
  webkitgtk: (r) =>
    `Linux WebKitGTK masks the GL renderer (reported: ${r}) and has a known WebGL input-lag history; ` +
    `set localStorage["${RENDERER_OVERRIDE_STORAGE_KEY}"]="webgl" to force WebGL`,
  override: () => `localStorage["${RENDERER_OVERRIDE_STORAGE_KEY}"] override`,
};

/**
 * Cached production entry point: decide once per app session and log the
 * verdict once so a lag report can be matched to the renderer in use.
 */
export function shouldLoadWebglAddon(): WebglProbeResult {
  if (cachedResult) return cachedResult;
  cachedResult = probeWebglRenderer();
  if (cachedResult.use) {
    console.info(
      `[codemux::terminal] WebGL renderer enabled (${cachedResult.renderer ?? "renderer string unavailable"}${
        cachedResult.reason === "override" ? "; forced by localStorage override" : ""
      })`,
    );
  } else {
    console.info(
      `[codemux::terminal] using DOM renderer — ${DECLINE_REASON_DETAIL[cachedResult.reason](
        cachedResult.renderer,
      )}`,
    );
  }
  return cachedResult;
}

/** Test hook: clear the module-level probe cache. */
export function resetWebglProbeCacheForTests(): void {
  cachedResult = null;
}
