import { describe, it, expect, vi, beforeEach } from "vitest";
import { Terminal } from "@xterm/xterm";

// ── Mocks ──
//
// The cache calls a handful of Tauri commands during cold-start
// initialization (uncacheTerminalScrollback, getTerminalScrollback,
// attachPtyOutput, writeToPty). Under jsdom there is no Tauri runtime, so
// each is stubbed to a resolved promise. We do not assert on their argument
// shapes here — that's covered by the Rust integration tests. This test's
// job is the cache-identity invariant: same sessionId → same Terminal
// instance across mount/unmount cycles.

// Stub out the entire commands module — pulling in actual breaks because
// @tauri-apps/api/core's Channel constructor reads window.__TAURI_INTERNALS__,
// which jsdom doesn't have. The factory body runs hoisted, so MockChannel
// is defined inline to keep the closure self-contained.
//
// `attachPtyOutput` records the channel's handler keyed by sessionId so tests
// can drive bytes into the cache via `__test_emitChannelBytes(sid, bytes)`.
// This is the only way to exercise the parked-buffer / drain code paths
// because the channel object the cache constructs is otherwise opaque to
// callers.
vi.mock("@/tauri/commands", () => {
  type Handler = (payload: unknown) => void;
  const handlers = new Map<string, Handler>();
  let lastConstructedHandler: Handler | null = null;

  class MockChannel<T> {
    constructor(handler?: (payload: T) => void) {
      if (handler) lastConstructedHandler = handler as Handler;
    }
    send(_data: T) {
      return Promise.resolve();
    }
  }

  return {
    Channel: MockChannel,
    writeToPty: vi.fn().mockResolvedValue(undefined),
    attachPtyOutput: vi.fn().mockImplementation(async (sid: string) => {
      if (lastConstructedHandler) {
        handlers.set(sid, lastConstructedHandler);
        lastConstructedHandler = null;
      }
    }),
    getTerminalScrollback: vi.fn().mockResolvedValue(null),
    cacheTerminalScrollback: vi.fn().mockResolvedValue(undefined),
    uncacheTerminalScrollback: vi.fn().mockResolvedValue(undefined),
    __test_emitChannelBytes: (sid: string, bytes: Uint8Array) => {
      const h = handlers.get(sid);
      if (!h) throw new Error(`no channel handler for session ${sid}`);
      h(bytes);
    },
    __test_resetChannels: () => {
      handlers.clear();
      lastConstructedHandler = null;
    },
  };
});

vi.mock("@/stores/app-store", () => ({
  useAppStore: {
    getState: () => ({ appState: null }),
  },
}));

vi.mock("@/stores/synced-settings-store", () => ({
  useSyncedSettingsStore: {
    getState: () => ({
      settings: {
        session_restore: { enabled: false, scrollback_lines: 1000 },
      },
    }),
  },
}));

// register/unregister can be no-ops; the test does not exercise the
// app-close serialization path.
vi.mock("@/hooks/use-scrollback-serializer", () => ({
  registerTerminalForSerialize: () => () => {},
}));

// xterm's WebGL addon throws under jsdom because there's no real canvas
// context. The cache catches the error and falls back to DOM rendering, so
// the mock just lets the throw happen naturally — but jsdom canvas APIs
// vary across versions, so silence the warn.
vi.spyOn(console, "warn").mockImplementation(() => {});

// xterm's CoreBrowserService calls window.matchMedia during open() to track
// devicePixelRatio. jsdom does not implement matchMedia by default, so we
// stub a no-op MediaQueryList — its only consumer here is xterm's DPR sync.
if (!window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as unknown as MediaQueryList;
}

import {
  getOrCreateTerminal,
  attachToContainer,
  detachFromContainer,
  disposeTerminal,
  hasCachedTerminal,
  peekCachedTerminal,
  applyThemeToAllTerminals,
  _cacheSize,
  _setParkedBufferCapForTest,
} from "./terminal-cache";
import * as commands from "@/tauri/commands";

// Re-typed handles to the test-only escape hatches the mock exposes.
const emitChannelBytes = (commands as unknown as {
  __test_emitChannelBytes: (sid: string, bytes: Uint8Array) => void;
}).__test_emitChannelBytes;
const resetChannels = (commands as unknown as {
  __test_resetChannels: () => void;
}).__test_resetChannels;

const baseOptions = {
  paneId: "pane-1",
  fontFamily: "monospace",
  fontSize: 13,
  cursorStyle: "block" as const,
  theme: {},
};

describe("terminal-cache", () => {
  beforeEach(() => {
    // Each test starts from a clean cache and DOM. The cache's parking node
    // is body-mounted on first use; we wipe the body to drop it.
    document.body.innerHTML = "";
    resetChannels();
    _setParkedBufferCapForTest(null);
  });

  it("getOrCreateTerminal returns the same instance for repeat calls with the same sessionId", async () => {
    const sid = "session-A";
    const first = await getOrCreateTerminal(sid, baseOptions);
    const second = await getOrCreateTerminal(sid, baseOptions);

    expect(first.isNew).toBe(true);
    expect(second.isNew).toBe(false);
    expect(second.entry).toBe(first.entry);
    expect(second.entry.terminal).toBe(first.entry.terminal);
    expect(second.entry.wrapperEl).toBe(first.entry.wrapperEl);

    disposeTerminal(sid);
  });

  it("attach/detach reparents the same wrapper across mount cycles", async () => {
    const sid = "session-B";
    const containerA = document.createElement("div");
    const containerB = document.createElement("div");
    document.body.append(containerA, containerB);

    // Cold start: cache miss creates the terminal and parks the wrapper.
    const { entry } = await getOrCreateTerminal(sid, baseOptions);

    // Simulate mount #1
    attachToContainer(sid, containerA);
    expect(entry.wrapperEl.parentElement).toBe(containerA);

    // Simulate unmount (workspace switch) — wrapper goes to parking node.
    detachFromContainer(sid);
    expect(entry.wrapperEl.parentElement?.id).toBe("codemux-terminal-parking");

    // Simulate remount under a different container (could be a different
    // workspace returning, or even a different pane in the same layout).
    // The cached entry still owns the same wrapper.
    attachToContainer(sid, containerB);
    expect(entry.wrapperEl.parentElement).toBe(containerB);
    expect(peekCachedTerminal(sid)).toBe(entry);

    disposeTerminal(sid);
  });

  it("disposeTerminal removes the cache entry and detaches the wrapper", async () => {
    const sid = "session-C";
    await getOrCreateTerminal(sid, baseOptions);
    expect(hasCachedTerminal(sid)).toBe(true);

    disposeTerminal(sid);
    expect(hasCachedTerminal(sid)).toBe(false);
    expect(peekCachedTerminal(sid)).toBeNull();

    // The parking node should be empty after disposal — no leaked wrappers.
    const parking = document.getElementById("codemux-terminal-parking");
    expect(parking?.children.length ?? 0).toBe(0);
  });

  it("disposeTerminal is idempotent", async () => {
    const sid = "session-D";
    await getOrCreateTerminal(sid, baseOptions);
    disposeTerminal(sid);
    expect(() => disposeTerminal(sid)).not.toThrow();
  });

  it("different sessionIds get independent terminal instances", async () => {
    const a = await getOrCreateTerminal("session-X", baseOptions);
    const b = await getOrCreateTerminal("session-Y", baseOptions);
    expect(a.entry).not.toBe(b.entry);
    expect(a.entry.terminal).not.toBe(b.entry.terminal);
    disposeTerminal("session-X");
    disposeTerminal("session-Y");
  });

  it("disposeTerminal on an unknown sessionId is a safe no-op", () => {
    expect(() => disposeTerminal("never-existed")).not.toThrow();
    expect(_cacheSize()).toBe(0);
  });

  it("attachToContainer on an unknown sessionId returns null", () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    expect(attachToContainer("phantom", container)).toBeNull();
  });

  it("detachFromContainer on an unknown sessionId is a safe no-op", () => {
    expect(() => detachFromContainer("phantom")).not.toThrow();
  });

  it("getOrCreateTerminal updates the cached entry's paneId on reuse", async () => {
    const sid = "session-pane-update";
    const first = await getOrCreateTerminal(sid, {
      ...baseOptions,
      paneId: "pane-original",
    });
    expect(first.entry.paneId).toBe("pane-original");

    const second = await getOrCreateTerminal(sid, {
      ...baseOptions,
      paneId: "pane-renamed",
    });
    expect(second.entry).toBe(first.entry);
    expect(second.entry.paneId).toBe("pane-renamed");

    disposeTerminal(sid);
  });

  it("getOrCreateTerminal rolls back the cache entry if attach fails", async () => {
    const attachMock = vi.mocked(commands.attachPtyOutput);
    attachMock.mockRejectedValueOnce(new Error("simulated backend failure"));

    const sid = "session-fail";
    await expect(getOrCreateTerminal(sid, baseOptions)).rejects.toThrow(
      "simulated backend failure",
    );
    // The next call should be a cold start again (cache rolled back) — if
    // the entry stuck, we'd see isNew=false here and a stuck broken xterm.
    expect(hasCachedTerminal(sid)).toBe(false);

    attachMock.mockResolvedValueOnce(undefined);
    const retry = await getOrCreateTerminal(sid, baseOptions);
    expect(retry.isNew).toBe(true);
    disposeTerminal(sid);
  });

  it("applyThemeToAllTerminals updates every cached entry's theme", async () => {
    const a = await getOrCreateTerminal("theme-A", baseOptions);
    const b = await getOrCreateTerminal("theme-B", baseOptions);

    const updated = { background: "#ff0000", foreground: "#00ff00" };
    applyThemeToAllTerminals(updated);

    expect(a.entry.terminal.options.theme?.background).toBe("#ff0000");
    expect(b.entry.terminal.options.theme?.background).toBe("#ff0000");

    disposeTerminal("theme-A");
    disposeTerminal("theme-B");
  });

  it("applyThemeToAllTerminals skips disposed entries without throwing", async () => {
    await getOrCreateTerminal("theme-skip", baseOptions);
    disposeTerminal("theme-skip");
    expect(() =>
      applyThemeToAllTerminals({ background: "#000000" }),
    ).not.toThrow();
  });

  it("attachToContainer is idempotent — calling it twice does not duplicate the wrapper", async () => {
    const sid = "session-idem-attach";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);

    attachToContainer(sid, container);
    attachToContainer(sid, container);

    // Wrapper should appear exactly once in containerEl, never duplicated.
    const matches = container.querySelectorAll(".codemux-terminal-wrapper");
    expect(matches.length).toBe(1);
    expect(entry.wrapperEl.parentElement).toBe(container);

    disposeTerminal(sid);
  });

  // ── Park-mode behavior ──────────────────────────────────────────────
  // The lag regression fix in this commit gates two things on the `parked`
  // flag: (a) PTY bytes received while parked accumulate in
  // `pendingParkedBytes` instead of being written to xterm, and (b) the
  // WebGL renderer is dropped on park and reloaded on activate. These tests
  // pin the byte-routing contract — WebGL specifics aren't observable in
  // jsdom (the addon throws on construct, so webglAddon is always null).

  it("bytes received while parked accumulate in the parked buffer instead of pendingPtyWrites", async () => {
    const sid = "session-park-buffer";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    // Park the entry — bytes should now route into pendingParkedBytes.
    detachFromContainer(sid);
    expect(entry.parked).toBe(true);

    const chunkA = new TextEncoder().encode("hello ");
    const chunkB = new TextEncoder().encode("world");
    emitChannelBytes(sid, chunkA);
    emitChannelBytes(sid, chunkB);

    expect(entry.pendingParkedBytes.length).toBe(2);
    expect(entry.pendingParkedSize).toBe(chunkA.length + chunkB.length);
    // Live RAF queue must stay empty so a stale flush can't fire on the
    // parked terminal between detach and reattach.
    expect(entry.pendingPtyWrites.length).toBe(0);
    expect(entry.ptyWriteFrame).toBeNull();

    disposeTerminal(sid);
  });

  it("attachToContainer drains the parked buffer into a single terminal.write", async () => {
    const sid = "session-drain";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    detachFromContainer(sid);
    const writeSpy = vi.spyOn(entry.terminal, "write");
    emitChannelBytes(sid, new TextEncoder().encode("alpha"));
    emitChannelBytes(sid, new TextEncoder().encode("beta"));
    emitChannelBytes(sid, new TextEncoder().encode("gamma"));

    // No write yet — we're parked.
    expect(writeSpy).not.toHaveBeenCalled();

    attachToContainer(sid, container);

    // Exactly one write — the whole point of the buffer is to coalesce.
    expect(writeSpy).toHaveBeenCalledTimes(1);
    const arg = writeSpy.mock.calls[0]?.[0] as Uint8Array;
    expect(arg).toBeInstanceOf(Uint8Array);
    expect(new TextDecoder().decode(arg)).toBe("alphabetagamma");

    // Buffer state has to reset so the next park cycle starts clean.
    expect(entry.parked).toBe(false);
    expect(entry.pendingParkedBytes.length).toBe(0);
    expect(entry.pendingParkedSize).toBe(0);
    expect(entry.parkedOverflow).toBe(false);

    disposeTerminal(sid);
  });

  it("parked-buffer overflow drops the oldest chunk and sets parkedOverflow", async () => {
    const sid = "session-overflow";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);
    detachFromContainer(sid);

    // Tiny cap so we don't have to pump megabytes through the mock.
    _setParkedBufferCapForTest(8);

    emitChannelBytes(sid, new TextEncoder().encode("AAAA")); // size 4 → total 4
    emitChannelBytes(sid, new TextEncoder().encode("BBBB")); // size 4 → total 8 (at cap, no drop)
    emitChannelBytes(sid, new TextEncoder().encode("CCCC")); // size 4 → total 12, drops "AAAA"

    expect(entry.parkedOverflow).toBe(true);
    expect(entry.pendingParkedSize).toBe(8);

    const concatenated = entry.pendingParkedBytes
      .map((c) => new TextDecoder().decode(c))
      .join("");
    expect(concatenated).toBe("BBBBCCCC");

    disposeTerminal(sid);
  });

  it("kitty stack still advances on bytes received while parked", async () => {
    // Kitty CSI-u state has to track the agent even when we're not rendering.
    // The escape sequence below is `CSI > 1 u` which pushes flag value 1 onto
    // the stack — picked because scanKittySequences recognizes it without
    // needing a full DA reply.
    const sid = "session-kitty-park";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    expect(entry.kittyStack.length).toBe(0);
    expect(entry.kittyLevel).toBe(0);

    detachFromContainer(sid);
    emitChannelBytes(sid, new TextEncoder().encode("\x1b[>1u"));

    // Even though we're parked, kitty bookkeeping has run.
    expect(entry.kittyStack.length).toBeGreaterThan(0);
    expect(entry.kittyLevel).toBeGreaterThan(0);

    disposeTerminal(sid);
  });

  it("detachFromContainer is idempotent — second call does not re-process state", async () => {
    const sid = "session-detach-idem";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);
    detachFromContainer(sid);
    emitChannelBytes(sid, new TextEncoder().encode("x"));
    expect(entry.pendingParkedBytes.length).toBe(1);

    // Second detach should be a no-op — must not flush, clear, or move
    // the wrapper since it's already parked.
    detachFromContainer(sid);
    expect(entry.parked).toBe(true);
    expect(entry.pendingParkedBytes.length).toBe(1);

    disposeTerminal(sid);
  });

  it("disposeTerminal clears any buffered parked bytes", async () => {
    const sid = "session-park-dispose";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);
    detachFromContainer(sid);
    emitChannelBytes(sid, new TextEncoder().encode("buffered"));
    expect(entry.pendingParkedBytes.length).toBeGreaterThan(0);

    disposeTerminal(sid);

    expect(entry.disposed).toBe(true);
    expect(entry.pendingParkedBytes.length).toBe(0);
    expect(entry.pendingParkedSize).toBe(0);
  });

  it("detachFromContainer migrates unflushed pendingPtyWrites into the parked queue", async () => {
    // Cold start has parked=false, so bytes from the channel queue into
    // pendingPtyWrites + a RAF. If the user switches workspaces before that
    // RAF fires, detach has to migrate the in-flight bytes — otherwise they
    // disappear silently and the next reactivate shows a stale frame.
    const sid = "session-migrate";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    emitChannelBytes(sid, new TextEncoder().encode("preempted"));
    expect(entry.pendingPtyWrites.length).toBe(1);
    expect(entry.ptyWriteFrame).not.toBeNull();

    detachFromContainer(sid);

    // RAF cancelled, queue empty, bytes now in the parked queue.
    expect(entry.ptyWriteFrame).toBeNull();
    expect(entry.pendingPtyWrites.length).toBe(0);
    expect(entry.pendingParkedBytes.length).toBe(1);
    expect(new TextDecoder().decode(entry.pendingParkedBytes[0])).toBe(
      "preempted",
    );

    disposeTerminal(sid);
  });

  it("kitty query while parked still triggers writeToPty (response to agent)", async () => {
    // The agent issues `\x1b[?u` to ask what kitty level we support. Even
    // when the workspace isn't visible, we still have to answer or the agent
    // will hang waiting for the reply. This pins the contract that scanning
    // happens at receive time, not drain time.
    const sid = "session-kitty-query";
    const container = document.createElement("div");
    document.body.appendChild(container);
    await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    const writeMock = vi.mocked(commands.writeToPty);
    writeMock.mockClear();

    detachFromContainer(sid);
    emitChannelBytes(sid, new TextEncoder().encode("\x1b[?u"));

    expect(writeMock).toHaveBeenCalledTimes(1);
    expect(writeMock.mock.calls[0]?.[0]).toBe(sid);

    disposeTerminal(sid);
  });

  it("warm reattach (not previously parked) does not double-drain", async () => {
    // attachToContainer can be called when we're already attached (e.g. a
    // resize observer that re-runs the mount effect). In that case wasParked
    // is false and we must skip drainParkedBytes — otherwise an empty drain
    // would still call entry.parkedOverflow=false (harmless) but the more
    // important guarantee is no spurious WebGL reload churn.
    const sid = "session-warm-reattach";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);
    expect(entry.parked).toBe(false);

    const writeSpy = vi.spyOn(entry.terminal, "write");
    attachToContainer(sid, container); // re-attach without detach
    expect(writeSpy).not.toHaveBeenCalled();
    expect(entry.parked).toBe(false);

    disposeTerminal(sid);
  });

  it("attachToContainer transitioning out of parked invokes loadAddon (WebGL reload)", async () => {
    // jsdom has no WebGL context so entry.webglAddon is always null after
    // load — but Terminal.prototype.loadAddon is still called. Spying on it
    // is the only way to verify the reload path actually runs without
    // standing up a real GPU.
    const sid = "session-webgl-reload";
    const container = document.createElement("div");
    document.body.appendChild(container);
    await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    const loadAddonSpy = vi.spyOn(Terminal.prototype, "loadAddon");
    detachFromContainer(sid);
    expect(loadAddonSpy).not.toHaveBeenCalled(); // detach unloads, doesn't load

    attachToContainer(sid, container);
    // Exactly one loadAddon call on un-park: the WebGL reload.
    expect(loadAddonSpy).toHaveBeenCalledTimes(1);

    loadAddonSpy.mockRestore();
    disposeTerminal(sid);
  });

  it("flushPtyWrites short-circuits if the entry was parked between RAF schedule and fire", async () => {
    // Defensive contract: if a RAF was scheduled before park (and somehow
    // survived cancelAnimationFrame — single-threaded JS makes this almost
    // impossible but we guard anyway), it must not write into a parked
    // terminal. Instead it migrates its bytes into the parked queue.
    const sid = "session-flush-park";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    emitChannelBytes(sid, new TextEncoder().encode("ghost"));
    expect(entry.pendingPtyWrites.length).toBe(1);

    // Flip park manually without going through detachFromContainer so we can
    // observe what flushPtyWrites does on its own — modeling the race.
    entry.parked = true;
    const writeSpy = vi.spyOn(entry.terminal, "write");

    // Internal flushPtyWrites isn't exported; trigger it via a fresh emit
    // that schedules a new RAF, then manually invoke through the existing
    // frame. Easier path: import flushPtyWrites? No — keep it black-box and
    // rely on the public observable: the parked queue should have absorbed
    // ghost bytes when we dispatch a fresh emit (the channel callback
    // already routes parked-state correctly), but the existing pendingPtyWrites
    // should also drain via flushPtyWrites's parked branch on next RAF.
    // Simulate the RAF by directly calling a fresh emit (which is parked-
    // routed) plus a wait for the RAF callback our test environment runs.
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => resolve()),
    );

    // Either the RAF ran and migrated, or cancelAnimationFrame dropped it —
    // both outcomes leave no terminal.write call against the parked terminal.
    expect(writeSpy).not.toHaveBeenCalled();
    // ghost bytes have to be reachable on next attach: either still in
    // pendingPtyWrites (RAF cancelled) or in pendingParkedBytes (RAF migrated).
    const reachable =
      entry.pendingPtyWrites.length + entry.pendingParkedBytes.length;
    expect(reachable).toBeGreaterThan(0);

    writeSpy.mockRestore();
    disposeTerminal(sid);
  });
});
