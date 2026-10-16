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

import { activatePlugin, resetActivationForTests, unloadPlugin } from "./activation";
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
  it("owns host subscriptions made after an await in async register()", async () => {
    let resume!: () => void;
    const gate = new Promise<void>((r) => (resume = r));
    const plugin: CodemuxPlugin = {
      id: "demo",
      async register() {
        await gate;
        host.chat.onEvent(() => {});
      },
    };
    activatePlugin(plugin, record);
    resume();
    await tick();
    expect(chatListenerCount()).toBe(1);
    unloadPlugin("demo");
    expect(chatListenerCount()).toBe(0);
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
