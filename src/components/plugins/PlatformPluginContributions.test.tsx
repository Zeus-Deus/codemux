import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";
import { registry } from "@/contrib/registry";
import { COMPOSER_STATUS_AREA, PALETTE_AREA, RIGHT_PANEL_AREA } from "@/sdk/areas";
import type { PluginCatalogue } from "./contract";
import { installation, manifest } from "./test-fixtures";

const toast = vi.hoisted(() => ({ info: vi.fn(), success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@/lib/toast", () => ({ toast }));
vi.mock("@/tauri/commands", () => ({
  dbGetUiState: vi.fn(() => Promise.resolve(null)),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
}));

import { resetPlatformPluginsForTests, usePlatformPluginsStore } from "./platform-adapter";
import { PlatformPluginContributions } from "./PlatformPluginContributions";
import { resetPluginThreadStatusesForTests } from "./PluginThreadStatuses";

const plugin = installation({
  manifest: manifest({
    actions: [{ id: "sync", title: "Sync now", placements: ["palette"] }],
    views: [
      { id: "panel", title: "Realm panel", entry: "panel.html", placement: "side-panel" },
      { id: "home", title: "Realm home", entry: "home.html", placement: "page" },
    ],
  }),
});

const ids = (area: string) => registry.getArea(area).map((entry) => entry.id);

function setCatalogue(catalogue: PluginCatalogue) {
  act(() => usePlatformPluginsStore.setState({ catalogue, loaded: true }));
}

beforeEach(() => {
  localStorage.clear();
  resetPlatformPluginsForTests({ catalogue: { installations: [plugin] }, loaded: true });
});
afterEach(() => {
  cleanup();
  resetPluginThreadStatusesForTests();
  vi.clearAllMocks();
});

describe("PlatformPluginContributions", () => {
  it("offers a running plugin's actions, views and side panels through the existing slots", () => {
    render(<PlatformPluginContributions />);
    expect(ids(PALETTE_AREA)).toEqual(
      expect.arrayContaining(["@inst-1:action.sync", "@inst-1:open.panel", "@inst-1:open.home"]),
    );
    expect(ids(RIGHT_PANEL_AREA)).toContain("@inst-1:view.panel");
    expect(registry.getArea(RIGHT_PANEL_AREA).find((entry) => entry.id === "@inst-1:view.panel")?.source).toBe(
      "platform",
    );
  });

  it("leaves renderer contributions intact even when their ids match platform names", () => {
    const legacy = [
      { id: "platform:thread-status", area: COMPOSER_STATUS_AREA, source: "plugin:platform" },
      { id: "inst-1:action.sync", area: PALETTE_AREA, source: "plugin:inst-1" },
      { id: "inst-1:view.panel", area: RIGHT_PANEL_AREA, source: "plugin:inst-1" },
    ];
    const dispose = registry.registerMany(legacy);
    try {
      const mounted = render(<PlatformPluginContributions />);
      for (const entry of legacy) expect(registry.getArea(entry.area)).toContain(entry);
      expect(ids(COMPOSER_STATUS_AREA)).toContain("@platform:thread-status");
      mounted.unmount();
      for (const entry of legacy) expect(registry.getArea(entry.area)).toContain(entry);
    } finally {
      dispose();
    }
  });

  it("withdraws every entry point at once when the plugin stops, and in safe mode", () => {
    render(<PlatformPluginContributions />);
    setCatalogue({ installations: [{ ...plugin, status: "quarantined", error: "Crashed 3 times" }] });
    expect(ids(PALETTE_AREA).some((id) => id.startsWith("@inst-1:"))).toBe(false);
    expect(ids(RIGHT_PANEL_AREA)).not.toContain("@inst-1:view.panel");
    setCatalogue({ installations: [plugin], executionDisabled: true });
    expect(ids(PALETTE_AREA).some((id) => id.startsWith("@inst-1:"))).toBe(false);
  });

  it("says once that a plugin stopped, with a way to its details", () => {
    render(<PlatformPluginContributions />);
    setCatalogue({ installations: [{ ...plugin, status: "quarantined", error: "Crashed 3 times" }] });
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.error.mock.calls[0]![0]).toBe("Realm needs attention");
    setCatalogue({ installations: [{ ...plugin, status: "quarantined", error: "Crashed 3 times" }] });
    expect(toast.error).toHaveBeenCalledTimes(1);
  });
});
