import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), info: vi.fn(), success: vi.fn(), warning: vi.fn() },
}));
vi.mock("@/tauri/commands", () => ({ dbDeleteSetting: vi.fn(async () => {}) }));
vi.mock("@/stores/settings-store", () => ({
  useSettingsStore: { getState: () => ({ settings: {}, set: vi.fn() }), setState: vi.fn() },
}));

import { activatePlugin, adoptPlugin, resetActivationForTests, satisfiesMinVersion, unloadPlugin } from "./activation";
import { registry } from "./registry";
import { resetPluginsStoreForTests, setPluginEnabled, usePluginsStore } from "./plugins-store";
import type { CodemuxPlugin } from "./plugin";

const record = { id: "demo", name: "Demo", kind: "disk" as const, tier: "local" as const };

afterEach(() => {
  resetActivationForTests();
  resetPluginsStoreForTests();
  vi.spyOn(console, "error").mockRestore();
});

describe("plugin activation", () => {
  it("scopes ids and removes everything on unload", () => {
    const cleanup = vi.fn();
    const plugin: CodemuxPlugin = {
      id: "demo",
      register(ctx) {
        ctx.register({ id: "cmd", area: "test.act" });
        ctx.onDispose(cleanup);
        ctx.setInterval(() => {}, 1000);
      },
    };
    expect(activatePlugin(plugin, record)).toBe(true);
    const [c] = registry.getArea("test.act");
    expect(c.id).toBe("demo:cmd");
    expect(c.source).toBe("plugin:demo");
    expect(usePluginsStore.getState().records.demo.status).toBe("loaded");
    unloadPlugin("demo");
    expect(registry.getArea("test.act")).toEqual([]);
    expect(cleanup).toHaveBeenCalledOnce();
  });

  it("rolls back a throwing register and shows an error row", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const plugin: CodemuxPlugin = {
      id: "demo",
      register(ctx) {
        ctx.register({ id: "a", area: "test.rb" });
        throw new Error("boom");
      },
    };
    expect(activatePlugin(plugin, record)).toBe(false);
    expect(registry.getArea("test.rb")).toEqual([]);
    const row = usePluginsStore.getState().records.demo;
    expect(row.status).toBe("error");
    expect(row.error).toBe("boom");
  });

  it("rolls back a rejected async register", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const plugin: CodemuxPlugin = {
      id: "demo",
      async register(ctx) {
        ctx.register({ id: "a", area: "test.async" });
        await Promise.resolve();
        throw new Error("late");
      },
    };
    activatePlugin(plugin, record);
    await new Promise((r) => setTimeout(r, 0));
    expect(registry.getArea("test.async")).toEqual([]);
    expect(usePluginsStore.getState().records.demo.status).toBe("error");
  });

  it("a throwing disposer doesn't strand the others", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const after = vi.fn();
    activatePlugin(
      {
        id: "demo",
        register(ctx) {
          ctx.onDispose(after);
          ctx.onDispose(() => {
            throw new Error("bad cleanup");
          });
          ctx.register({ id: "x", area: "test.wedge" });
        },
      },
      record,
    );
    unloadPlugin("demo");
    expect(after).toHaveBeenCalledOnce();
    expect(registry.getArea("test.wedge")).toEqual([]);
  });

  it("toggles live through the store handle", async () => {
    const plugin: CodemuxPlugin = {
      id: "demo",
      register(ctx) {
        ctx.register({ id: "x", area: "test.toggle" });
      },
    };
    adoptPlugin(plugin, record, false);
    expect(registry.getArea("test.toggle")).toEqual([]);
    await setPluginEnabled("demo", true);
    expect(registry.getArea("test.toggle")).toHaveLength(1);
    await setPluginEnabled("demo", false);
    expect(registry.getArea("test.toggle")).toEqual([]);
    expect(usePluginsStore.getState().records.demo.status).toBe("disabled");
  });

  it("compares minCodemux numerically", () => {
    expect(satisfiesMinVersion("0.25.0", "0.25.0")).toBe(true);
    expect(satisfiesMinVersion("0.25.1", "0.25.0")).toBe(true);
    expect(satisfiesMinVersion("0.24.9", "0.25.0")).toBe(false);
    expect(satisfiesMinVersion("1.0.0", "0.99.99")).toBe(true);
    expect(satisfiesMinVersion("0.10.0", "0.9.0")).toBe(true);
    expect(satisfiesMinVersion("0.25.0", undefined)).toBe(true);
  });
});

describe("kill switch", () => {
  it("saves a decision but loads nothing while plugins are disabled by environment", async () => {
    const plugin: CodemuxPlugin = { id: "demo", register: (ctx) => void ctx.register({ id: "x", area: "test.kill" }) };
    adoptPlugin(plugin, record, false);
    usePluginsStore.setState({ disabledByEnv: true });
    await setPluginEnabled("demo", true);
    expect(registry.getArea("test.kill")).toEqual([]);
    expect(usePluginsStore.getState().decisions.demo).toBe(true);
  });
});
