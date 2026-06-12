import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  probeWebglRenderer,
  shouldLoadWebglAddon,
  resetWebglProbeCacheForTests,
  isLinuxWebKitGtk,
  SOFTWARE_GL_RENDERER_PATTERN,
  type ProbeGlContext,
} from "./webgl-renderer-probe";

// Real-world user-agent strings for each engine Codemux can meet.
const UA_WEBKITGTK =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
const UA_CHROME_LINUX =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const UA_WEBVIEW2_WINDOWS =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0";
const UA_WKWEBVIEW_MACOS =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";
// The Codemux agent-browser pane (Chromium with a trimmed version token).
const UA_AGENT_BROWSER =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/Linux Safari/537.36";

/** Build a fake WebGL2 context that reports the given unmasked renderer. */
function fakeContext(
  renderer: string,
  options: {
    debugExtension?: boolean;
    loseContext?: () => void;
  } = {},
): ProbeGlContext {
  const { debugExtension = true, loseContext } = options;
  const UNMASKED_RENDERER_WEBGL = 0x9246;
  const RENDERER = 0x1f01;
  return {
    RENDERER,
    getExtension(name: string) {
      if (name === "WEBGL_debug_renderer_info") {
        return debugExtension ? { UNMASKED_RENDERER_WEBGL } : null;
      }
      if (name === "WEBGL_lose_context") {
        return loseContext ? { loseContext } : null;
      }
      return null;
    },
    getParameter(pname: number) {
      if (pname === UNMASKED_RENDERER_WEBGL && debugExtension) return renderer;
      if (pname === RENDERER) {
        // The masked string a browser reports without the debug extension.
        return debugExtension ? renderer : "WebKit WebGL";
      }
      return null;
    },
  };
}

describe("probeWebglRenderer", () => {
  it("declines when WebGL2 is unavailable (context factory returns null)", () => {
    const result = probeWebglRenderer(() => null, UA_CHROME_LINUX, null);
    expect(result).toEqual({ use: false, reason: "unavailable", renderer: null });
  });

  it("declines when the context factory throws", () => {
    const result = probeWebglRenderer(
      () => {
        throw new Error("canvas creation blocked");
      },
      UA_CHROME_LINUX,
      null,
    );
    expect(result).toEqual({ use: false, reason: "unavailable", renderer: null });
  });

  it("declines SwiftShader (the Chromium-family software fallback)", () => {
    // Real string observed in this machine's agent-browser pane.
    const result = probeWebglRenderer(
      () =>
        fakeContext(
          "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)",
        ),
      UA_WEBVIEW2_WINDOWS,
      null,
    );
    expect(result.use).toBe(false);
    if (!result.use) expect(result.reason).toBe("software");
  });

  it("declines llvmpipe (the Mesa software fallback)", () => {
    const result = probeWebglRenderer(
      () => fakeContext("llvmpipe (LLVM 17.0.6, 256 bits)"),
      UA_CHROME_LINUX,
      null,
    );
    expect(result.use).toBe(false);
    if (!result.use) expect(result.reason).toBe("software");
  });

  it("declines Microsoft Basic Render Driver (Windows software fallback)", () => {
    const result = probeWebglRenderer(
      () =>
        fakeContext(
          "ANGLE (Microsoft, Microsoft Basic Render Driver Direct3D11 vs_5_0 ps_5_0)",
        ),
      UA_WEBVIEW2_WINDOWS,
      null,
    );
    expect(result.use).toBe(false);
    if (!result.use) expect(result.reason).toBe("software");
  });

  it("accepts a hardware Mesa renderer in a Chromium-family engine", () => {
    const result = probeWebglRenderer(
      () => fakeContext("Mesa Intel(R) Xe Graphics (TGL GT2)"),
      UA_CHROME_LINUX,
      null,
    );
    expect(result).toEqual({
      use: true,
      renderer: "Mesa Intel(R) Xe Graphics (TGL GT2)",
      reason: "hardware",
    });
  });

  it("accepts a hardware ANGLE/D3D renderer on Windows WebView2", () => {
    const result = probeWebglRenderer(
      () =>
        fakeContext(
          "ANGLE (NVIDIA, NVIDIA GeForce RTX 3080 Direct3D11 vs_5_0 ps_5_0, D3D11)",
        ),
      UA_WEBVIEW2_WINDOWS,
      null,
    );
    expect(result.use).toBe(true);
  });

  it("declines Linux WebKitGTK even with a masked 'hardware-looking' string", () => {
    // The real-app case: WebKitGTK 2.52 masks the renderer as "Apple GPU" on
    // Linux for fingerprinting resistance (verified live on this machine), so
    // hardware backing is unverifiable — and the engine has a documented
    // WebGL input-lag history (acd1566, v0.9.0 regression).
    const result = probeWebglRenderer(
      () => fakeContext("Apple GPU"),
      UA_WEBKITGTK,
      null,
    );
    expect(result).toEqual({ use: false, reason: "webkitgtk", renderer: "Apple GPU" });
  });

  it("keeps WebGL on macOS WKWebView (its 'Apple GPU' string is true there)", () => {
    const result = probeWebglRenderer(
      () => fakeContext("Apple GPU"),
      UA_WKWEBVIEW_MACOS,
      null,
    );
    expect(result).toEqual({ use: true, renderer: "Apple GPU", reason: "hardware" });
  });

  it("gives a masked renderer string the benefit of the doubt on Chromium engines", () => {
    // No WEBGL_debug_renderer_info → only the generic masked string is
    // visible; keep the WebGL perf win and rely on the addon's own
    // try/catch + onContextLoss fallbacks for hard failures.
    const result = probeWebglRenderer(
      () => fakeContext("ignored", { debugExtension: false }),
      UA_CHROME_LINUX,
      null,
    );
    expect(result).toEqual({ use: true, renderer: "WebKit WebGL", reason: "hardware" });
  });

  it("honors the 'dom' override without creating a GL context", () => {
    const factory = vi.fn(() => fakeContext("Mesa Intel(R) Xe Graphics (TGL GT2)"));
    const result = probeWebglRenderer(factory, UA_CHROME_LINUX, "dom");
    expect(result).toEqual({ use: false, reason: "override", renderer: null });
    expect(factory).not.toHaveBeenCalled();
  });

  it("honors the 'webgl' override on WebKitGTK (escape hatch)", () => {
    const result = probeWebglRenderer(
      () => fakeContext("Apple GPU"),
      UA_WEBKITGTK,
      "webgl",
    );
    expect(result).toEqual({ use: true, renderer: "Apple GPU", reason: "override" });
  });

  it("ignores the 'webgl' override when WebGL2 does not exist (addon would throw)", () => {
    const result = probeWebglRenderer(() => null, UA_WEBKITGTK, "webgl");
    expect(result).toEqual({ use: false, reason: "unavailable", renderer: null });
  });

  it("releases the throwaway probe context via WEBGL_lose_context", () => {
    const loseContext = vi.fn();
    probeWebglRenderer(
      () => fakeContext("Mesa Intel(R) Xe Graphics (TGL GT2)", { loseContext }),
      UA_CHROME_LINUX,
      null,
    );
    expect(loseContext).toHaveBeenCalledTimes(1);
  });

  it("tolerates a throwing getParameter and still returns a verdict", () => {
    const result = probeWebglRenderer(
      () => ({
        RENDERER: 0x1f01,
        getExtension: () => null,
        getParameter: () => {
          throw new Error("driver hiccup");
        },
      }),
      UA_CHROME_LINUX,
      null,
    );
    // Unknown renderer → benefit of the doubt (hard failures are covered by
    // the addon's own fallbacks).
    expect(result).toEqual({ use: true, renderer: null, reason: "hardware" });
  });
});

describe("isLinuxWebKitGtk", () => {
  it.each([
    [UA_WEBKITGTK, true],
    [UA_CHROME_LINUX, false],
    [UA_AGENT_BROWSER, false],
    [UA_WEBVIEW2_WINDOWS, false],
    [UA_WKWEBVIEW_MACOS, false],
    ["Mozilla/5.0 (X11; Linux x86_64; rv:127.0) Gecko/20100101 Firefox/127.0", false],
  ])("%s → %s", (ua, expected) => {
    expect(isLinuxWebKitGtk(ua)).toBe(expected);
  });
});

describe("SOFTWARE_GL_RENDERER_PATTERN", () => {
  it.each([
    "SwiftShader",
    "Google SwiftShader",
    "llvmpipe (LLVM 15.0.7, 256 bits)",
    "softpipe",
    "Mesa OffScreen (swrast)",
    "Software Rasterizer",
    "Apple Software Renderer",
    "Microsoft Basic Render Driver",
  ])("matches software renderer %j", (s) => {
    expect(SOFTWARE_GL_RENDERER_PATTERN.test(s)).toBe(true);
  });

  it.each([
    "Mesa Intel(R) Xe Graphics (TGL GT2)",
    "AMD Radeon RX 6800 (radeonsi, navi21, LLVM 17.0.6, DRM 3.57)",
    "NVIDIA GeForce RTX 4090/PCIe/SSE2",
    "Apple M3 Max",
    "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)",
  ])("does not match hardware renderer %j", (s) => {
    expect(SOFTWARE_GL_RENDERER_PATTERN.test(s)).toBe(false);
  });
});

describe("shouldLoadWebglAddon", () => {
  beforeEach(() => {
    resetWebglProbeCacheForTests();
    vi.spyOn(console, "info").mockImplementation(() => {});
  });
  afterEach(() => {
    resetWebglProbeCacheForTests();
    vi.restoreAllMocks();
  });

  it("probes once and caches the verdict for subsequent panes", () => {
    // jsdom has no WebGL2: the default factory yields null → "unavailable".
    const first = shouldLoadWebglAddon();
    expect(first).toEqual({ use: false, reason: "unavailable", renderer: null });
    const again = shouldLoadWebglAddon();
    expect(again).toBe(first);
    // The verdict is logged exactly once, not per pane mount.
    expect(console.info).toHaveBeenCalledTimes(1);
  });
});
