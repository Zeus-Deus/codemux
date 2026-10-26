import { describe, expect, it } from "vitest";
import { pluginStartPageInstallationId, readPluginStartPage, resolvePluginStartPage, type PluginStartPage } from "./start-page";
import { installation, manifest } from "./test-fixtures";

const withPage = (overrides: Parameters<typeof installation>[0] = {}) =>
  installation({
    manifest: manifest({ views: [{ id: "home", title: "Home", entry: "home.html", placement: "page" }] }),
    ...overrides,
  });

const resolve = (preference: PluginStartPage | null, installations = [withPage()], executionDisabled = false) =>
  resolvePluginStartPage({ preference, catalogue: "ready", installations, executionDisabled });

describe("plugin start page", () => {
  it("opens the installation it names, never another source's plugin with the same id", () => {
    const own = { installationId: "inst-1", pluginId: "acme.realm", viewId: "home" };
    const otherSource = withPage({ installationId: "inst-2" });
    expect(resolve(own, [otherSource, withPage()])).toEqual({
      kind: "open",
      installationId: "inst-1",
      viewId: "home",
      upgraded: null,
    });
    expect(resolve(own, [otherSource])).toEqual({ kind: "core" });
  });

  it("upgrades an older plugin-id preference only while that id is unambiguous", () => {
    const legacy = { installationId: null, pluginId: "acme.realm", viewId: "home" };
    expect(resolve(legacy)).toEqual({
      kind: "open",
      installationId: "inst-1",
      viewId: "home",
      upgraded: { installationId: "inst-1", pluginId: "acme.realm", viewId: "home" },
    });
    // A second source with the same id, even turned off, makes the old preference ambiguous.
    const ambiguous = [withPage(), withPage({ installationId: "inst-2", enabled: false })];
    expect(resolve(legacy, ambiguous)).toEqual({ kind: "core" });
    expect(pluginStartPageInstallationId(legacy, ambiguous)).toBeNull();
  });

  it("starts the usual way in safe mode, without ui, or for a view that is not a page", () => {
    const own = { installationId: "inst-1", pluginId: "acme.realm", viewId: "home" };
    expect(resolve(own, [withPage()], true)).toEqual({ kind: "core" });
    expect(resolve(own, [withPage({ grants: { capabilities: [], projectIds: [] } })])).toEqual({ kind: "core" });
    expect(resolve({ ...own, viewId: "missing" })).toEqual({ kind: "core" });
    expect(resolvePluginStartPage({ preference: own, catalogue: "pending", installations: [], executionDisabled: false })).toEqual({
      kind: "pending",
    });
  });

  it("reads the older stored shape and ignores malformed storage", () => {
    const storage = (value: string | null) => ({ getItem: () => value });
    expect(readPluginStartPage(storage(JSON.stringify({ pluginId: "acme.realm", viewId: "home" })))).toEqual({
      installationId: null,
      pluginId: "acme.realm",
      viewId: "home",
    });
    expect(readPluginStartPage(storage("{"))).toBeNull();
    expect(readPluginStartPage(storage(JSON.stringify({ installationId: "inst-1" })))).toBeNull();
  });
});
