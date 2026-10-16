/**
 * Regressions for the plugin lifecycle edge cases: ownership across async
 * registration, late ctx resources, host callback isolation, and decisions
 * re-read after install/remove.
 */
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
const listen = vi.fn(async (..._args: unknown[]) => () => {});
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...a: unknown[]) => listen(...a) }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: async () => "9.9.9" }));
vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), info: vi.fn(), success: vi.fn(), warning: vi.fn() },
}));
vi.mock("@/tauri/commands", () => ({ dbDeleteSetting: vi.fn(async () => {}) }));
vi.mock("@/stores/settings-store", () => ({
  useSettingsStore: {
    getState: () => ({ settings: {}, set: vi.fn() }),
    setState: vi.fn(),
    subscribe: vi.fn(() => () => {}),
  },
  selectReduceMotion: () => false,
  SETTINGS_DEFAULTS: {},
}));

const themeListeners = new Set<() => void>();
vi.mock("@/lib/themes", () => ({
  getActiveTheme: () => ({ id: "t" }),
  subscribeActiveTheme: (fn: () => void) => {
    themeListeners.add(fn);
    return () => themeListeners.delete(fn);
  },
  THEME_ROLE_VARIABLES: {},
}));

import { activatePlugin, liveDisposers, resetActivationForTests, unloadPlugin } from "./activation";
import { chatListenerCount } from "./chat-events";
import { resetLifetimeForTests } from "./lifetime";
import { pluginActive, resetPluginsStoreForTests, setPluginEnabled, usePluginsStore } from "./plugins-store";
import { resetRuntimeLoaderForTests, scanDiskPlugins } from "./runtime-loader";
import type { CodemuxPlugin } from "./plugin";
import { host } from "@/sdk/host";

const record = { id: "demo", name: "Demo", kind: "disk" as const, tier: "local" as const };
const tick = () => new Promise((r) => setTimeout(r, 0));

afterEach(() => {
  resetActivationForTests();
  resetPluginsStoreForTests();
  resetRuntimeLoaderForTests();
  resetLifetimeForTests();
  themeListeners.clear();
  invoke.mockReset();
  invoke.mockImplementation(async () => undefined);
  vi.useRealTimers();
});

describe("plugin ownership", () => {
  it("ctx.host owns subscriptions made after an await in async register()", async () => {
    let resume!: () => void;
    const gate = new Promise<void>((r) => (resume = r));
    const plugin: CodemuxPlugin = {
      id: "demo",
      async register(ctx) {
        await gate;
        ctx.host.chat.onEvent(() => {});
      },
    };
    activatePlugin(plugin, record);
    resume();
    await tick();
    expect(chatListenerCount()).toBe(1);
    unloadPlugin("demo");
    expect(chatListenerCount()).toBe(0);
  });

  it("overlapping async registrations each own their own subscriptions", async () => {
    const gates: Record<string, () => void> = {};
    const seen: string[] = [];
    const plugin = (id: string): CodemuxPlugin => ({
      id,
      async register(ctx) {
        await new Promise<void>((r) => (gates[id] = r));
        ctx.host.chat.onEvent(() => seen.push(id));
      },
    });
    activatePlugin(plugin("a"), { ...record, id: "a" });
    activatePlugin(plugin("b"), { ...record, id: "b" });
    // A resumes while B is still waiting; then B.
    gates.a();
    await tick();
    gates.b();
    await tick();
    expect(chatListenerCount()).toBe(2);
    unloadPlugin("a");
    expect(chatListenerCount()).toBe(1);
    unloadPlugin("b");
    expect(chatListenerCount()).toBe(0);
  });

  it("an async ctx callback subscribing after an await is owned through ctx.host", async () => {
    vi.useFakeTimers();
    const plugin: CodemuxPlugin = {
      id: "demo",
      register(ctx) {
        ctx.setTimeout(async () => {
          await Promise.resolve();
          ctx.host.chat.onEvent(() => {});
        }, 5);
      },
    };
    activatePlugin(plugin, record);
    await vi.advanceTimersByTimeAsync(5);
    expect(chatListenerCount()).toBe(1);
    unloadPlugin("demo");
    expect(chatListenerCount()).toBe(0);
  });

  it("a bare host subscription with no certain owner is not given to another plugin", async () => {
    let resume!: () => void;
    const pending = new Promise<void>((r) => (resume = r));
    activatePlugin(
      { id: "waiting", async register() { await pending; } },
      { ...record, id: "waiting" },
    );
    const off = host.chat.onEvent(() => {});
    unloadPlugin("waiting");
    expect(chatListenerCount()).toBe(1);
    off();
    expect(chatListenerCount()).toBe(0);
    resume();
  });

  it("finished and cancelled ctx resources are released, not kept until unload", () => {
    vi.useFakeTimers();
    let ownerSize = () => -1;
    const target = new EventTarget();
    const plugin: CodemuxPlugin = {
      id: "demo",
      register(ctx) {
        const tick = () => ctx.setTimeout(tick, 1);
        tick();
        for (let i = 0; i < 20; i += 1) ctx.addEventListener(target, "x", () => {})();
      },
    };
    activatePlugin(plugin, record);
    // Probe the owner through the module's live set size.
    ownerSize = () => liveDisposers("demo");
    vi.advanceTimersByTime(200);
    // One registration-free timer is live at any moment.
    expect(ownerSize()).toBe(1);
  });

  it("owns subscriptions made inside ctx timer callbacks", () => {
    vi.useFakeTimers();
    const plugin: CodemuxPlugin = {
      id: "demo",
      register(ctx) {
        ctx.setTimeout(() => host.chat.onEvent(() => {}), 10);
      },
    };
    activatePlugin(plugin, record);
    vi.advanceTimersByTime(10);
    expect(chatListenerCount()).toBe(1);
    unloadPlugin("demo");
    expect(chatListenerCount()).toBe(0);
  });

  it("cancels ctx resources started after the plugin was turned off", async () => {
    vi.useFakeTimers();
    let resume!: () => void;
    const gate = new Promise<void>((r) => (resume = r));
    const fired = vi.fn();
    const target = new EventTarget();
    const heard = vi.fn();
    const plugin: CodemuxPlugin = {
      id: "demo",
      async register(ctx) {
        ctx.setTimeout(() => {}, 1); // an earlier resource, already cleaned up
        await gate;
        ctx.setInterval(fired, 5);
        ctx.addEventListener(target, "ping", heard);
      },
    };
    activatePlugin(plugin, record);
    unloadPlugin("demo");
    resume();
    await vi.runOnlyPendingTimersAsync();
    vi.advanceTimersByTime(50);
    target.dispatchEvent(new Event("ping"));
    expect(fired).not.toHaveBeenCalled();
    expect(heard).not.toHaveBeenCalled();
  });
});

describe("host callback isolation", () => {
  it("a throwing theme subscriber never unwinds the theme update or other listeners", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const later = vi.fn();
    const plugin: CodemuxPlugin = {
      id: "demo",
      register() {
        host.theme.subscribe(() => {
          throw new Error("boom");
        });
        host.theme.subscribe(later);
      },
    };
    activatePlugin(plugin, record);
    expect(() => themeListeners.forEach((fn) => fn())).not.toThrow();
    expect(later).toHaveBeenCalledOnce();
  });
});

describe("decisions after install and remove", () => {
  const entry = {
    id: "hello",
    file: "/plugins/hello/plugin.js",
    marker: { source: "git", repo: "https://github.com/o/r", sha: "a".repeat(40), tier: "unverified", installedAt: "x" },
    error: null,
  };

  it("a reinstall left off does not run on the removed copy's old 'on'", async () => {
    usePluginsStore.setState({ decisionsLoaded: true, decisions: { hello: true } });
    // Native truth after remove + plain Install: off.
    invoke.mockImplementation(async (cmd: unknown) => {
      if (cmd === "plugins_get_decisions") return { hello: false };
      if (cmd === "plugins_list") return [entry];
      if (cmd === "plugins_read_source") throw new Error("must not read a plugin that is off");
      return undefined;
    });
    await scanDiskPlugins(true);
    expect(pluginActive("hello", false)).toBe(false);
    expect(invoke).not.toHaveBeenCalledWith("plugins_read_source", expect.anything());
    expect(usePluginsStore.getState().records.hello.status).toBe("disabled");
  });

  it("'Install and turn on' takes effect without a restart", async () => {
    usePluginsStore.setState({ decisionsLoaded: true, decisions: {} });
    invoke.mockImplementation(async (cmd: unknown) => {
      if (cmd === "plugins_get_decisions") return { hello: true };
      if (cmd === "plugins_list") return [entry];
      return undefined;
    });
    await scanDiskPlugins(true);
    expect(pluginActive("hello", false)).toBe(true);
    expect(invoke).toHaveBeenCalledWith("plugins_read_source", { id: "hello" });
  });

  it("a disable whose save failed stays off across later rescans", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    invoke.mockImplementation(async (cmd: unknown) => {
      if (cmd === "plugins_get_decisions") return { hello: true };
      if (cmd === "plugins_list") return [entry];
      if (cmd === "plugins_read_source") throw new Error("must not read a plugin the user turned off");
      return undefined;
    });
    const { setDecisionPersistence } = await import("./plugins-store");
    setDecisionPersistence(async () => {
      throw new Error("disk full");
    });
    usePluginsStore.setState({ decisionsLoaded: true, decisions: { hello: true } });
    await setPluginEnabled("hello", false);
    await tick();
    expect(usePluginsStore.getState().unsavedDecisions.hello).toBe("disk full");
    await scanDiskPlugins(true);
    expect(pluginActive("hello", false)).toBe(false);
    expect(invoke).not.toHaveBeenCalledWith("plugins_read_source", expect.anything());
  });

  it("a block that lands during a scan is applied once that scan ends", async () => {
    let finishRead!: (source: string) => void;
    let blocked = false;
    invoke.mockImplementation(async (cmd: unknown) => {
      if (cmd === "plugins_get_decisions") return { hello: true };
      if (cmd === "plugins_list") return [blocked ? { ...entry, error: "Blocked by the Codemux catalog: test" } : entry];
      if (cmd === "plugins_read_source") return new Promise<string>((r) => (finishRead = r));
      return undefined;
    });
    usePluginsStore.setState({ decisionsLoaded: true, decisions: { hello: true } });
    const first = scanDiskPlugins();
    await vi.waitFor(() => expect(finishRead).toBeTypeOf("function"));
    // The catalog now blocks it; the native side emits { id: null }.
    blocked = true;
    const second = scanDiskPlugins();
    finishRead("export default { id: 'hello', register() {} }");
    await first;
    await second;
    const row = usePluginsStore.getState().records.hello;
    expect(row.status).toBe("error");
    expect(row.error).toContain("Blocked");
  });

  it("a toggle still being saved wins over the native copy", async () => {
    let finish!: () => void;
    invoke.mockImplementation(async (cmd: unknown) => {
      if (cmd === "plugins_get_decisions") return { hello: false };
      if (cmd === "plugins_list") return [];
      return undefined;
    });
    const { setDecisionPersistence } = await import("./plugins-store");
    setDecisionPersistence(() => new Promise<void>((r) => (finish = r)));
    usePluginsStore.setState({ decisionsLoaded: true });
    void setPluginEnabled("hello", true);
    await scanDiskPlugins();
    expect(usePluginsStore.getState().decisions.hello).toBe(true);
    finish();
  });
});
