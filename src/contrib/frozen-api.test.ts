import { createHash } from "node:crypto";
import { afterEach, expect, it, vi } from "vitest";
import source from "../../examples/plugins/frozen-v1/plugin.js?raw";

const state = vi.hoisted(() => ({ settings: {} as Record<string, string>, notify: vi.fn() }));
vi.mock("@/lib/toast", () => ({ toast: { info: state.notify, error: vi.fn(), success: vi.fn(), warning: vi.fn() } }));
vi.mock("@/tauri/commands", () => ({ dbDeleteSetting: vi.fn(async () => {}) }));
vi.mock("@/stores/settings-store", () => ({
  useSettingsStore: {
    getState: () => ({ settings: state.settings, set: (key: string, value: string) => { state.settings[key] = value; } }),
    setState: vi.fn(),
  },
}));

import { activatePlugin, resetActivationForTests, unloadPlugin } from "./activation";
import { resetPluginsStoreForTests } from "./plugins-store";
import { registry } from "./registry";
import type { CodemuxPlugin } from "./plugin";
import type { PaletteCommandData } from "@/sdk/host";

afterEach(() => {
  resetActivationForTests();
  resetPluginsStoreForTests();
  state.settings = {};
  state.notify.mockClear();
});

it("executes the unchanged external renderer artifact across unload and reactivation", async () => {
  expect(createHash("sha256").update(source).digest("hex")).toBe("d6fa210830470deebef7966f798d21ceb1f04299b20a691b171c1dcfa21e8ae5");
  const url = `data:text/javascript;base64,${btoa(source)}`;
  const module: { default: CodemuxPlugin } = await import(/* @vite-ignore */ url);
  const record = { id: "frozen-v1", name: "Frozen v1", kind: "disk" as const, tier: "local" as const };
  expect(activatePlugin(module.default, record)).toBe(true);
  const command = registry.getArea("palette.commands").find((entry) => entry.id === "frozen-v1:hello");
  expect(command).toBeDefined();
  const data = command?.data as PaletteCommandData;
  data.run({ workspaceId: null });
  expect(state.notify).toHaveBeenCalledWith("Frozen v1 works", undefined);
  unloadPlugin("frozen-v1");
  expect(registry.getArea("palette.commands").some((entry) => entry.id === "frozen-v1:hello")).toBe(false);
  expect(state.settings["plugin.frozen-v1.lastDisposed"]).toBe("true");
  expect(activatePlugin(module.default, record)).toBe(true);
  expect(state.settings["plugin.frozen-v1.activations"]).toBe("2");
});
