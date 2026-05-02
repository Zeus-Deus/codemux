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
// This is the only way to exercise the PTY-output routing path because the
// channel object the cache constructs is otherwise opaque to callers.
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
    detachPtyOutput: vi.fn().mockImplementation(async (sid: string) => {
      handlers.delete(sid);
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
  // Park keeps the xterm instance alive but detaches the Rust output channel.
  // That restores v0.1.29 input latency: hidden terminals do not push bytes
  // through Tauri IPC or xterm parsing. Reattach replays pending_output from
  // Rust into the same parser state.

  it("detach parks the wrapper and detaches PTY output", async () => {
    const sid = "session-park-detach-output";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    const detachMock = vi.mocked(commands.detachPtyOutput);
    detachMock.mockClear();

    detachFromContainer(sid);
    expect(entry.parked).toBe(true);
    expect(detachMock).toHaveBeenCalledWith(sid);
    expect(() =>
      emitChannelBytes(sid, new TextEncoder().encode("hidden-output")),
    ).toThrow("no channel handler");

    disposeTerminal(sid);
  });

  it("detachFromContainer is idempotent — second call is a no-op", async () => {
    const sid = "session-detach-idem";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);
    detachFromContainer(sid);
    expect(entry.parked).toBe(true);

    const parkingNode = document.getElementById("codemux-terminal-parking");
    const childCountBefore = parkingNode?.children.length ?? 0;

    detachFromContainer(sid);
    expect(entry.parked).toBe(true);
    expect(parkingNode?.children.length ?? 0).toBe(childCountBefore);

    disposeTerminal(sid);
  });

  it("kitty query while parked still triggers writeToPty (response to agent)", async () => {
    // Hidden terminals detach output, but replay on the next attach still
    // flows through scanKittyProtocol before xterm.write, so agent queries
    // that arrived while hidden get answered during the catch-up replay.
    const sid = "session-kitty-query";
    const container = document.createElement("div");
    document.body.appendChild(container);
    await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    const writeMock = vi.mocked(commands.writeToPty);
    writeMock.mockClear();

    detachFromContainer(sid);
    attachToContainer(sid, container);
    await Promise.resolve();
    await Promise.resolve();
    emitChannelBytes(sid, new TextEncoder().encode("\x1b[?u"));

    expect(writeMock).toHaveBeenCalledTimes(1);
    expect(writeMock.mock.calls[0]?.[0]).toBe(sid);

    disposeTerminal(sid);
  });

  it("warm reattach (not previously parked) does not duplicate work", async () => {
    // attachToContainer can be called when we're already attached (e.g. a
    // resize observer that re-runs the mount effect). In that case wasParked
    // is false and we must not churn terminal.write or WebGL reload.
    const sid = "session-warm-reattach";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);
    expect(entry.parked).toBe(false);

    const writeSpy = vi.spyOn(entry.terminal, "write");
    const loadAddonSpy = vi.spyOn(Terminal.prototype, "loadAddon");
    attachToContainer(sid, container); // re-attach without detach
    expect(writeSpy).not.toHaveBeenCalled();
    expect(loadAddonSpy).not.toHaveBeenCalled();
    expect(entry.parked).toBe(false);

    writeSpy.mockRestore();
    loadAddonSpy.mockRestore();
    disposeTerminal(sid);
  });

  it("park / activate does NOT load addons", async () => {
    // v0.1.30 added WebGL; on WebKitGTK/Wayland that is a likely source of
    // terminal input latency. The cache now sticks to xterm's DOM renderer,
    // matching v0.1.29's performance profile.
    const sid = "session-no-webgl";
    const container = document.createElement("div");
    document.body.appendChild(container);
    await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);

    const loadAddonSpy = vi.spyOn(Terminal.prototype, "loadAddon");
    detachFromContainer(sid);
    attachToContainer(sid, container);
    detachFromContainer(sid);
    attachToContainer(sid, container);

    expect(loadAddonSpy).not.toHaveBeenCalled();

    loadAddonSpy.mockRestore();
    disposeTerminal(sid);
  });

  it("reattach reconnects PTY output and writes replay into the same xterm", async () => {
    const sid = "session-reattach-write";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const { entry } = await getOrCreateTerminal(sid, baseOptions);
    attachToContainer(sid, container);
    detachFromContainer(sid);

    const writeSpy = vi.spyOn(entry.terminal, "write");
    attachToContainer(sid, container);
    await Promise.resolve();
    await Promise.resolve();
    emitChannelBytes(sid, new TextEncoder().encode("replayed-output"));

    expect(writeSpy).toHaveBeenCalledTimes(1);

    writeSpy.mockRestore();
    disposeTerminal(sid);
  });
});
