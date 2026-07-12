import { describe, it, expect } from "vitest";
import {
  cdpButtonFromEvent,
  heldButtonFromButtons,
  getModifiers,
  nextClickState,
  mapToViewport,
  sanitizeCursor,
  httpBaseFromStreamUrl,
  portFromLoopbackStreamUrl,
  remoteBrowserEndpoints,
  parseDaemonResult,
  chunkString,
  buildCursorProbeScript,
  buildInsertTextScript,
  SELECTION_SCRIPT,
  PASTE_CHUNK_SIZE,
} from "./stream-protocol";

describe("cdpButtonFromEvent", () => {
  it("maps standard button indices", () => {
    expect(cdpButtonFromEvent(0)).toBe("left");
    expect(cdpButtonFromEvent(1)).toBe("middle");
    expect(cdpButtonFromEvent(2)).toBe("right");
  });

  it("maps unknown indices to none", () => {
    expect(cdpButtonFromEvent(3)).toBe("none");
    expect(cdpButtonFromEvent(4)).toBe("none");
    expect(cdpButtonFromEvent(-1)).toBe("none");
  });
});

describe("heldButtonFromButtons", () => {
  it("returns none when nothing is held", () => {
    expect(heldButtonFromButtons(0)).toBe("none");
  });

  it("maps single held buttons", () => {
    expect(heldButtonFromButtons(1)).toBe("left");
    expect(heldButtonFromButtons(2)).toBe("right");
    expect(heldButtonFromButtons(4)).toBe("middle");
  });

  it("prefers left when multiple buttons are held", () => {
    expect(heldButtonFromButtons(1 | 2)).toBe("left");
    expect(heldButtonFromButtons(2 | 4)).toBe("right");
  });
});

describe("getModifiers", () => {
  const ev = (overrides: Partial<Parameters<typeof getModifiers>[0]>) => ({
    shiftKey: false,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    ...overrides,
  });

  it("encodes the CDP modifier bitmask (Alt=1, Ctrl=2, Meta=4, Shift=8)", () => {
    // Bit values are fixed by CDP and must match the Rust sender
    // (stream_input.rs `parse_key_combo`), which drives the same daemon.
    expect(getModifiers(ev({}))).toBe(0);
    expect(getModifiers(ev({ altKey: true }))).toBe(1);
    expect(getModifiers(ev({ ctrlKey: true }))).toBe(2);
    expect(getModifiers(ev({ metaKey: true }))).toBe(4);
    expect(getModifiers(ev({ shiftKey: true }))).toBe(8);
    expect(getModifiers(ev({ shiftKey: true, ctrlKey: true }))).toBe(10);
  });
});

describe("nextClickState", () => {
  it("starts at 1 with no prior click", () => {
    expect(nextClickState(null, 10, 10, "left", 1000).count).toBe(1);
  });

  it("chains rapid same-position clicks 1 → 2 → 3", () => {
    let s = nextClickState(null, 10, 10, "left", 1000);
    s = nextClickState(s, 11, 10, "left", 1200);
    expect(s.count).toBe(2);
    s = nextClickState(s, 11, 11, "left", 1400);
    expect(s.count).toBe(3);
  });

  it("wraps back to 1 after a triple click", () => {
    let s = nextClickState(null, 10, 10, "left", 1000);
    s = nextClickState(s, 10, 10, "left", 1100);
    s = nextClickState(s, 10, 10, "left", 1200);
    s = nextClickState(s, 10, 10, "left", 1300);
    expect(s.count).toBe(1);
  });

  it("resets when too slow", () => {
    const first = nextClickState(null, 10, 10, "left", 1000);
    expect(nextClickState(first, 10, 10, "left", 1600).count).toBe(1);
  });

  it("resets when too far away", () => {
    const first = nextClickState(null, 10, 10, "left", 1000);
    expect(nextClickState(first, 30, 10, "left", 1100).count).toBe(1);
  });

  it("resets on a different button", () => {
    const first = nextClickState(null, 10, 10, "left", 1000);
    expect(nextClickState(first, 10, 10, "right", 1100).count).toBe(1);
  });
});

describe("mapToViewport", () => {
  const rect = { left: 0, top: 0, width: 1280, height: 720 };
  const canvasSize = { width: 1280, height: 720 };
  const viewport = { width: 1280, height: 720 };
  const fullDraw = { x: 0, y: 0, w: 1280, h: 720 };

  it("maps 1:1 when canvas, draw area, and viewport match", () => {
    expect(mapToViewport({ clientX: 100, clientY: 200 }, rect, canvasSize, viewport, fullDraw)).toEqual({
      x: 100,
      y: 200,
    });
  });

  it("accounts for CSS scaling of the canvas", () => {
    const cssRect = { left: 0, top: 0, width: 640, height: 360 };
    expect(mapToViewport({ clientX: 320, clientY: 180 }, cssRect, canvasSize, viewport, fullDraw)).toEqual({
      x: 640,
      y: 360,
    });
  });

  it("accounts for letterboxing offsets", () => {
    const draw = { x: 140, y: 0, w: 1000, h: 720 };
    const vp = { width: 500, height: 360 };
    const mapped = mapToViewport({ clientX: 640, clientY: 360 }, rect, canvasSize, vp, draw);
    expect(mapped.x).toBe(250);
    expect(mapped.y).toBe(180);
  });

  it("clamps positions outside the viewport (captured drags)", () => {
    expect(mapToViewport({ clientX: -50, clientY: -50 }, rect, canvasSize, viewport, fullDraw)).toEqual({
      x: 0,
      y: 0,
    });
    expect(mapToViewport({ clientX: 5000, clientY: 5000 }, rect, canvasSize, viewport, fullDraw)).toEqual({
      x: 1279,
      y: 719,
    });
  });
});

describe("sanitizeCursor", () => {
  it("passes through known keywords", () => {
    expect(sanitizeCursor("pointer")).toBe("pointer");
    expect(sanitizeCursor("text")).toBe("text");
    expect(sanitizeCursor("nwse-resize")).toBe("nwse-resize");
  });

  it("normalizes case and whitespace", () => {
    expect(sanitizeCursor(" Pointer ")).toBe("pointer");
  });

  it("uses the trailing keyword of url() cursor lists", () => {
    expect(sanitizeCursor("url(custom.png), pointer")).toBe("pointer");
    expect(sanitizeCursor("url(a.png), url(b.png), move")).toBe("move");
  });

  it("falls back to default for unknown or unsafe values", () => {
    expect(sanitizeCursor("auto")).toBe("default");
    expect(sanitizeCursor("url(javascript:alert(1))")).toBe("default");
    expect(sanitizeCursor("")).toBe("default");
    expect(sanitizeCursor(null)).toBe("default");
    expect(sanitizeCursor(42)).toBe("default");
    expect(sanitizeCursor("x".repeat(500))).toBe("default");
  });
});

describe("httpBaseFromStreamUrl", () => {
  it("converts ws:// to http://", () => {
    expect(httpBaseFromStreamUrl("ws://localhost:9223")).toBe("http://localhost:9223");
    expect(httpBaseFromStreamUrl("ws://127.0.0.1:9250/")).toBe("http://127.0.0.1:9250");
  });

  it("converts wss:// to https://", () => {
    expect(httpBaseFromStreamUrl("wss://example.com:9223")).toBe("https://example.com:9223");
  });

  it("rejects invalid or non-websocket URLs", () => {
    expect(httpBaseFromStreamUrl(undefined)).toBeNull();
    expect(httpBaseFromStreamUrl(null)).toBeNull();
    expect(httpBaseFromStreamUrl("")).toBeNull();
    expect(httpBaseFromStreamUrl("not a url")).toBeNull();
    expect(httpBaseFromStreamUrl("file:///etc/passwd")).toBeNull();
  });
});

describe("portFromLoopbackStreamUrl", () => {
  it("extracts the port from a loopback ws:// URL", () => {
    expect(portFromLoopbackStreamUrl("ws://127.0.0.1:9223")).toBe(9223);
    expect(portFromLoopbackStreamUrl("ws://127.0.0.1:9299/")).toBe(9299);
    expect(portFromLoopbackStreamUrl("wss://127.0.0.1:9250")).toBe(9250);
  });

  it("rejects URLs without an explicit port or a websocket scheme", () => {
    expect(portFromLoopbackStreamUrl("ws://127.0.0.1")).toBeNull();
    expect(portFromLoopbackStreamUrl("http://127.0.0.1:9223")).toBeNull();
    expect(portFromLoopbackStreamUrl("not a url")).toBeNull();
    expect(portFromLoopbackStreamUrl(null)).toBeNull();
    expect(portFromLoopbackStreamUrl(undefined)).toBeNull();
    expect(portFromLoopbackStreamUrl("")).toBeNull();
  });
});

describe("remoteBrowserEndpoints (loopback → served-origin proxy mapping)", () => {
  it("maps a loopback stream URL onto the origin's /proxy/browser/<port> routes", () => {
    const ep = remoteBrowserEndpoints("ws://127.0.0.1:9250", "http://192.168.1.5:4377");
    expect(ep).toEqual({
      wsUrl: "ws://192.168.1.5:4377/proxy/browser/9250/ws",
      httpBase: "http://192.168.1.5:4377/proxy/browser/9250",
    });
  });

  // The HTTP base must NOT be just the origin — evalOnDaemon/probeDaemon append
  // `/api/status` and `/api/command`, which have to land under the port path.
  it("keeps the port path on the HTTP base so /api/* resolves through the proxy", () => {
    const ep = remoteBrowserEndpoints("ws://127.0.0.1:9260", "http://host:4377");
    expect(`${ep?.httpBase}/api/command`).toBe(
      "http://host:4377/proxy/browser/9260/api/command",
    );
    expect(`${ep?.httpBase}/api/status`).toBe(
      "http://host:4377/proxy/browser/9260/api/status",
    );
  });

  // A secure served origin (mesh serve / user proxy) must upgrade both schemes.
  it("uses wss/https when the served origin is https", () => {
    const ep = remoteBrowserEndpoints("ws://127.0.0.1:9223", "https://host.example.ts.net");
    expect(ep).toEqual({
      wsUrl: "wss://host.example.ts.net/proxy/browser/9223/ws",
      httpBase: "https://host.example.ts.net/proxy/browser/9223",
    });
  });

  // Port edge cases: both ends of the agent-browser range (9223–9299).
  it("handles the range endpoints 9223 and 9299", () => {
    expect(remoteBrowserEndpoints("ws://127.0.0.1:9223", "http://h:4377")?.wsUrl).toBe(
      "ws://h:4377/proxy/browser/9223/ws",
    );
    expect(remoteBrowserEndpoints("ws://127.0.0.1:9299", "http://h:4377")?.httpBase).toBe(
      "http://h:4377/proxy/browser/9299",
    );
  });

  it("returns null when the daemon port can't be parsed", () => {
    expect(remoteBrowserEndpoints("ws://127.0.0.1", "http://h:4377")).toBeNull();
    expect(remoteBrowserEndpoints(undefined, "http://h:4377")).toBeNull();
  });

  it("returns null when the page origin is unparseable", () => {
    expect(remoteBrowserEndpoints("ws://127.0.0.1:9250", "not an origin")).toBeNull();
  });
});

describe("parseDaemonResult", () => {
  it("unwraps the {data: {result}} envelope", () => {
    expect(parseDaemonResult({ id: "1", success: true, data: { result: "pointer" } })).toBe("pointer");
  });

  it("unwraps a bare {result}", () => {
    expect(parseDaemonResult({ result: true })).toBe(true);
  });

  it("returns null for failures and junk", () => {
    expect(parseDaemonResult({ success: false, error: "nope" })).toBeNull();
    expect(parseDaemonResult(null)).toBeNull();
    expect(parseDaemonResult("oops")).toBeNull();
    expect(parseDaemonResult({})).toBeNull();
  });
});

describe("chunkString", () => {
  it("splits text into fixed-size chunks", () => {
    expect(chunkString("abcdef", 2)).toEqual(["ab", "cd", "ef"]);
    expect(chunkString("abcde", 2)).toEqual(["ab", "cd", "e"]);
  });

  it("handles empty input", () => {
    expect(chunkString("", 10)).toEqual([]);
  });

  it("never splits a surrogate pair at a chunk boundary", () => {
    // "a🌍b" is 4 UTF-16 units; a naive 2-unit split would cut the pair.
    expect(chunkString("a🌍b", 2)).toEqual(["a", "🌍", "b"]);
    // Reassembly must always be lossless for emoji-dense text.
    const text = "🌍".repeat(50);
    for (const size of [1, 2, 3, 5, 7]) {
      const chunks = chunkString(text, size);
      expect(chunks.join("")).toBe(text);
      for (const c of chunks) {
        const last = c.charCodeAt(c.length - 1);
        expect(last >= 0xd800 && last <= 0xdbff).toBe(false);
      }
    }
  });
});

// The daemon reads each HTTP request with a single peek, so the whole
// request (headers + body) must fit one TCP segment (~1.4KB). Bodies
// ≤ ~870 bytes were measured to always survive with browser-sized
// headers; bodies ≥ ~970 always failed. Stay under the lower bound.
const MAX_EVAL_BODY_BYTES = 850;

function evalBodyBytes(script: string): number {
  return new TextEncoder().encode(
    JSON.stringify({ id: "pane-9999", action: "evaluate", script }),
  ).length;
}

describe("buildCursorProbeScript", () => {
  it("embeds rounded integer coordinates only", () => {
    const script = buildCursorProbeScript(12.7, 99.2);
    expect(script).toContain("elementFromPoint(13,99)");
    expect(script).not.toContain("12.7");
  });

  it("stays inside the daemon's single-segment HTTP read", () => {
    expect(evalBodyBytes(buildCursorProbeScript(99999, 99999))).toBeLessThan(MAX_EVAL_BODY_BYTES);
  });
});

describe("SELECTION_SCRIPT", () => {
  it("stays inside the daemon's single-segment HTTP read", () => {
    expect(evalBodyBytes(SELECTION_SCRIPT)).toBeLessThan(MAX_EVAL_BODY_BYTES);
  });
});

describe("buildInsertTextScript", () => {
  it("JSON-escapes the inserted text", () => {
    const script = buildInsertTextScript('he said "hi"\n\\done');
    expect(script).toContain('"he said \\"hi\\"\\n\\\\done"');
    expect(script).toContain('execCommand("insertText"');
  });

  it("keeps a worst-case paste chunk inside the single-segment read", () => {
    // ASCII worst case: every char JSON-escapes to two bytes.
    const quoted = '"'.repeat(PASTE_CHUNK_SIZE);
    expect(evalBodyBytes(buildInsertTextScript(quoted))).toBeLessThan(MAX_EVAL_BODY_BYTES);
    // Unicode worst case: 3 UTF-8 bytes per UTF-16 code unit (CJK).
    const cjk = "漢".repeat(PASTE_CHUNK_SIZE);
    expect(evalBodyBytes(buildInsertTextScript(cjk))).toBeLessThan(MAX_EVAL_BODY_BYTES);
  });
});
