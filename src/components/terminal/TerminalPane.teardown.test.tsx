import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, cleanup, render } from "@testing-library/react";
import {
  flushAllTeardowns,
  setTeardownScheduler,
  pendingTeardownSessionIds,
  type TeardownHandle,
} from "./deferred-teardown";

/**
 * Phase 4 coverage: the expensive tail of TerminalPane's unmount (scrollback
 * serialize + persist, WebGL dispose, term.dispose) must not run before the
 * incoming pane paints, and must run exactly once afterwards.
 *
 * xterm and every IPC/store dependency are stubbed — this asserts lifecycle
 * ORDER, not rendering. `log` is a single ordered event stream so a test can
 * state "serialize before create" directly.
 */
const h = vi.hoisted(() => ({
  log: [] as string[],
  terminals: [] as { index: number; disposed: number }[],
  webgls: [] as { index: number; disposed: number }[],
  serializeCalls: 0,
  serializeAddons: 0,
}));

vi.mock("@xterm/xterm", () => {
  class Terminal {
    index: number;
    disposed = 0;
    cols = 80;
    rows = 24;
    options: Record<string, unknown>;
    buffer = { active: { type: "normal" } };
    parser = { registerOscHandler: () => ({ dispose: () => {} }) };
    constructor(options: Record<string, unknown>) {
      this.options = options;
      this.index = h.terminals.length;
      h.terminals.push(this);
      h.log.push(`create:${this.index}`);
    }
    loadAddon() {}
    open() {}
    attachCustomKeyEventHandler() {}
    onData() {
      return { dispose: () => {} };
    }
    onResize() {
      return { dispose: () => {} };
    }
    onWriteParsed() {
      return { dispose: () => {} };
    }
    write() {}
    focus() {}
    getSelection() {
      return "";
    }
    paste() {}
    dispose() {
      this.disposed++;
      h.log.push(`dispose:${this.index}`);
    }
  }
  return { Terminal };
});

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));

vi.mock("@xterm/addon-serialize", () => ({
  // One SerializeAddon per Terminal, constructed in the same order, so the
  // addon's index identifies the terminal it serializes.
  SerializeAddon: class {
    index = h.serializeAddons++;
    serialize() {
      h.serializeCalls++;
      h.log.push(`serialize:${this.index}`);
      return "SERIALIZED";
    }
  },
}));

vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    index: number;
    disposed = 0;
    constructor() {
      this.index = h.webgls.length;
      h.webgls.push(this);
      h.log.push(`webgl-create:${this.index}`);
    }
    onContextLoss() {}
    dispose() {
      this.disposed++;
      h.log.push(`webgl-dispose:${this.index}`);
    }
  },
}));

vi.mock("./webgl-renderer-probe", () => ({
  shouldLoadWebglAddon: () => ({ use: true, reason: "test" }),
}));

vi.mock("./terminal-write-pump", () => ({
  createWritePump: () => ({
    enqueue: () => {},
    enqueueString: () => {},
    cancel: () => h.log.push("pump-cancel"),
  }),
}));

const cacheTerminalScrollback = vi.fn(async () => {
  h.log.push("cache");
});
const detachPtyOutput = vi.fn(async () => {
  h.log.push("detach");
});

vi.mock("@/tauri/commands", () => ({
  getCurrentTheme: vi.fn(async () => ({
    accent: "#7aa2f7", cursor: "#c0caf5", foreground: "#c0caf5", background: "#1a1b26",
    selection_foreground: "#c0caf5", selection_background: "#283457",
    color0: "#15161e", color1: "#f7768e", color2: "#9ece6a", color3: "#e0af68",
    color4: "#7aa2f7", color5: "#bb9af7", color6: "#7dcfff", color7: "#a9b1d6",
    color8: "#414868", color9: "#f7768e", color10: "#9ece6a", color11: "#e0af68",
    color12: "#7aa2f7", color13: "#bb9af7", color14: "#7dcfff", color15: "#c0caf5",
  })),
  getShellAppearance: vi.fn(async () => ({ font_family: "monospace" })),
  writeToPty: vi.fn(async () => {}),
  resizePty: vi.fn(async () => {}),
  detachPtyOutput: (...args: unknown[]) => detachPtyOutput(...(args as [])),
  attachPtyOutput: vi.fn(async () => 1),
  pausePtyOutput: vi.fn(async () => {}),
  resumePtyOutput: vi.fn(async () => {}),
  getTerminalStatus: vi.fn(async (session_id: string) => ({
    session_id,
    state: "ready",
    message: null,
    exit_code: null,
  })),
  clearAgentStatus: vi.fn(async () => {}),
  getTerminalScrollback: vi.fn(async () => null),
  cacheTerminalScrollback: (...args: unknown[]) =>
    cacheTerminalScrollback(...(args as [])),
  uncacheTerminalScrollback: vi.fn(async () => {
    h.log.push("uncache");
  }),
  Channel: class {
    constructor(_onmessage: (payload: unknown) => void) {}
  },
}));

vi.mock("@/tauri/events", () => ({
  onTerminalStatus: vi.fn(),
  onThemeChanged: vi.fn(async () => () => {}),
}));
vi.mock("@/hooks/use-tauri-event", () => ({ useTauriEvent: () => {} }));
vi.mock("@/hooks/use-scrollback-serializer", () => ({
  registerTerminalForSerialize: () => () => {},
}));
vi.mock("@/hooks/use-resolved-keybinds", () => ({
  // `keybindMap` is read at module load by `app-shortcuts`.
  resolveKeybinds: () => ({
    keybindMap: new Map(),
    getKeysForAction: () => null,
  }),
}));

let restoreEnabled = true;
vi.mock("@/stores/synced-settings-store", () => {
  const state = () => ({
    settings: {
      appearance: {
        theme: "default",
        shell_font: null,
        typography_mode: "simple",
        interface_font_family: null,
        interface_font_size: 16,
        conversation_font_family: null,
        conversation_font_size: 14,
        code_font_family: null,
        code_font_size: 13,
        terminal_font_family: null,
        terminal_font_size: 13,
      },
      session_restore: { enabled: restoreEnabled, scrollback_lines: 1000 },
      keyboard: { shortcuts: {} },
    },
  });
  const useSyncedSettingsStore = Object.assign(
    (selector: (value: ReturnType<typeof state>) => unknown) => selector(state()),
    {
      getState: () => ({
        ...state(),
      }),
    },
  );
  return { useSyncedSettingsStore };
});

vi.mock("@/stores/settings-store", () => ({
  getTerminalCursorStyle: () => "bar",
  selectLegacyTerminalFontFamily: () => undefined,
  useSettingsStore: (selector: (state: { settings: Record<string, string> }) => unknown) =>
    selector({ settings: {} }),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: {
    getState: () => ({
      appState: {
        pane_statuses: {},
        terminal_sessions: [
          {
            session_id: "sess-a",
            cwd: "/tmp/a",
            original_command: null,
            cols: 80,
            rows: 24,
            adapter_captures: {},
          },
          {
            session_id: "sess-b",
            cwd: "/tmp/b",
            original_command: null,
            cols: 80,
            rows: 24,
            adapter_captures: {},
          },
        ],
      },
    }),
  },
  getSessionWorkspaceId: () => "ws-1",
}));

vi.mock("@/stores/terminal-cwd-store", () => ({
  useTerminalCwdStore: { getState: () => ({ setCwd: () => {} }) },
  parseOsc7: () => null,
}));

// Imported after the mocks so the pane picks them up.
const { TerminalPane } = await import("./TerminalPane");

function schedulerHarness() {
  const frames = new Map<number, () => void>();
  const idles = new Map<number, () => void>();
  let next = 1;
  setTeardownScheduler({
    requestFrame: (cb) => {
      const handle = next++;
      frames.set(handle, cb);
      return handle;
    },
    cancelFrame: (handle: TeardownHandle) => {
      frames.delete(handle as number);
    },
    requestIdle: (cb) => {
      const handle = next++;
      idles.set(handle, cb);
      return handle;
    },
    cancelIdle: (handle: TeardownHandle) => {
      idles.delete(handle as number);
    },
  });
  const drain = (map: Map<number, () => void>) => {
    const pending = [...map.values()];
    map.clear();
    for (const cb of pending) cb();
  };
  return {
    /** Paint boundary + idle slot: what the real scheduler grants after the
     *  incoming pane is on screen. */
    settle: () => {
      drain(frames);
      drain(frames);
      drain(idles);
    },
  };
}

function renderPane(sessionId: string) {
  return render(
    <TerminalPane
      sessionId={sessionId}
      paneId="pane-1"
      focused={false}
      visible
      title="term"
    />,
  );
}

describe("TerminalPane deferred teardown", () => {
  beforeEach(() => {
    h.log.length = 0;
    h.terminals.length = 0;
    h.webgls.length = 0;
    h.serializeCalls = 0;
    h.serializeAddons = 0;
    cacheTerminalScrollback.mockClear();
    detachPtyOutput.mockClear();
    restoreEnabled = true;
  });

  afterEach(() => {
    cleanup();
    flushAllTeardowns();
    setTeardownScheduler(null);
  });

  it("does not serialize, persist or dispose during unmount", async () => {
    const harness = schedulerHarness();
    const view = renderPane("sess-a");
    await act(async () => {});

    await act(async () => {
      view.unmount();
    });

    // Synchronous half: output/input flow is stopped immediately.
    expect(detachPtyOutput).toHaveBeenCalledTimes(1);
    expect(h.log).toContain("pump-cancel");
    // Deferred half: nothing expensive ran inside the switch.
    expect(h.serializeCalls).toBe(0);
    expect(cacheTerminalScrollback).not.toHaveBeenCalled();
    expect(h.terminals[0].disposed).toBe(0);
    expect(h.webgls[0].disposed).toBe(0);
    expect(pendingTeardownSessionIds()).toEqual(["sess-a"]);

    harness.settle();

    expect(h.serializeCalls).toBe(1);
    expect(cacheTerminalScrollback).toHaveBeenCalledTimes(1);
    expect(h.terminals[0].disposed).toBe(1);
    expect(h.webgls[0].disposed).toBe(1);
    expect(pendingTeardownSessionIds()).toEqual([]);
    // Serialize must precede disposal — the addon reads the live buffer.
    expect(h.log.indexOf("serialize:0")).toBeLessThan(h.log.indexOf("dispose:0"));
  });

  it("still defers disposal when session restore is disabled", async () => {
    restoreEnabled = false;
    const harness = schedulerHarness();
    const view = renderPane("sess-a");
    await act(async () => {});

    await act(async () => {
      view.unmount();
    });
    expect(h.terminals[0].disposed).toBe(0);

    harness.settle();
    expect(h.serializeCalls).toBe(0);
    expect(cacheTerminalScrollback).not.toHaveBeenCalled();
    expect(h.terminals[0].disposed).toBe(1);
    expect(h.webgls[0].disposed).toBe(1);
  });

  it("flushes a parked job before remounting the same session", async () => {
    schedulerHarness();
    const view = renderPane("sess-a");
    await act(async () => {});

    // A → B: A's teardown parks.
    await act(async () => {
      view.rerender(
        <TerminalPane
          sessionId="sess-b"
          paneId="pane-1"
          focused={false}
          visible
          title="term"
        />,
      );
    });
    expect(pendingTeardownSessionIds()).toEqual(["sess-a"]);
    expect(h.terminals[0].disposed).toBe(0);

    // B → A: the parked job must complete before the replacement Terminal (and
    // its WebGL context) exists.
    await act(async () => {
      view.rerender(
        <TerminalPane
          sessionId="sess-a"
          paneId="pane-1"
          focused={false}
          visible
          title="term"
        />,
      );
    });

    expect(h.terminals[0].disposed).toBe(1);
    expect(h.terminals.length).toBe(3);
    expect(h.log.indexOf("serialize:0")).toBeLessThan(h.log.indexOf("create:2"));
    expect(h.log.indexOf("dispose:0")).toBeLessThan(h.log.indexOf("create:2"));
    expect(h.log.indexOf("webgl-dispose:0")).toBeLessThan(
      h.log.indexOf("webgl-create:2"),
    );
    // Only B is still parked, so at most one abandoned terminal is alive.
    expect(pendingTeardownSessionIds()).toEqual(["sess-b"]);
  });

  it("disposes every terminal exactly once across rapid A→B→A switches", async () => {
    const harness = schedulerHarness();
    const view = renderPane("sess-a");
    await act(async () => {});

    for (const sessionId of ["sess-b", "sess-a", "sess-b", "sess-a"]) {
      await act(async () => {
        view.rerender(
          <TerminalPane
            sessionId={sessionId}
            paneId="pane-1"
            focused={false}
            visible
            title="term"
          />,
        );
      });
      // Never more than the queue bound, regardless of switch rate.
      expect(pendingTeardownSessionIds().length).toBeLessThanOrEqual(2);
    }

    await act(async () => {
      view.unmount();
    });
    harness.settle();
    flushAllTeardowns();

    expect(h.terminals.length).toBe(5);
    for (const term of h.terminals) expect(term.disposed).toBe(1);
    for (const webgl of h.webgls) expect(webgl.disposed).toBe(1);
    expect(h.serializeCalls).toBe(5);
  });
});
