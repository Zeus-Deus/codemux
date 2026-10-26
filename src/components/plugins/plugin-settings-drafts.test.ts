import { beforeEach, describe, expect, it } from "vitest";
import {
  discardPluginSettings,
  editPluginSettings,
  pluginSettingsView,
  prunePluginSettingsDrafts,
  usePluginSettingsDrafts,
} from "./plugin-settings-drafts";
import { DIGEST_B, installation, manifest } from "./test-fixtures";

const configurable = (overrides: Parameters<typeof installation>[0] = {}) =>
  installation({
    manifest: manifest({
      settings: [
        { key: "label", label: "Label", type: "string", default: "Board" },
        { key: "token", label: "Token", type: "secret" },
      ],
    }),
    settings: { label: "Board" },
    storedSecrets: ["token"],
    ...overrides,
  });

const entry = (subject = configurable()) =>
  usePluginSettingsDrafts.getState().drafts[subject.installationId] ?? null;

beforeEach(() => usePluginSettingsDrafts.setState({ drafts: {}, saving: {} }));

describe("plugin settings drafts", () => {
  it("keeps an edit against the values it started from, and drops one that changes nothing", () => {
    const plugin = configurable();
    editPluginSettings(plugin, { draft: { label: "Sprint", token: "" }, clearedSecrets: new Set() });
    expect(entry()?.draft.label).toBe("Sprint");
    // The host's values move on elsewhere; the edit still diffs against where it started.
    const moved = configurable({ settings: { label: "Renamed" } });
    expect(pluginSettingsView(moved, entry(moved)).base).toEqual({ label: "Board" });
    editPluginSettings(moved, { draft: { label: "Board", token: "" }, clearedSecrets: new Set() });
    expect(entry()).toBeNull();
  });

  it("counts deleting a stored secret as an edit", () => {
    editPluginSettings(configurable(), { draft: { label: "Board", token: "" }, clearedSecrets: new Set(["token"]) });
    expect(entry()?.clearedSecrets.has("token")).toBe(true);
    discardPluginSettings("inst-1");
    expect(entry()).toBeNull();
  });

  it("drops a saved draft only while it is still the one that was submitted", () => {
    editPluginSettings(configurable(), { draft: { label: "Sprint", token: "" }, clearedSecrets: new Set() });
    const submitted = entry();
    editPluginSettings(configurable(), { draft: { label: "Newer", token: "" }, clearedSecrets: new Set() });
    discardPluginSettings("inst-1", submitted);
    expect(entry()?.draft.label).toBe("Newer");
    discardPluginSettings("inst-1", entry());
    expect(entry()).toBeNull();
  });

  it("follows the stored values while nothing is edited", () => {
    const view = pluginSettingsView(configurable({ settings: { label: "Elsewhere" } }), null);
    expect(view.draft).toEqual({ label: "Elsewhere", token: "" });
  });

  it("drops drafts for removed plugins and for other files", () => {
    editPluginSettings(configurable(), { draft: { label: "Sprint", token: "" }, clearedSecrets: new Set() });
    prunePluginSettingsDrafts([configurable()]);
    expect(entry()).not.toBeNull();
    // A draft for different files is never shown against these.
    expect(pluginSettingsView(configurable({ digest: DIGEST_B }), null).draft.label).toBe("Board");
    prunePluginSettingsDrafts([configurable({ digest: DIGEST_B })]);
    expect(entry()).toBeNull();
    editPluginSettings(configurable(), { draft: { label: "Sprint", token: "" }, clearedSecrets: new Set() });
    prunePluginSettingsDrafts([]);
    expect(entry()).toBeNull();
  });
});
