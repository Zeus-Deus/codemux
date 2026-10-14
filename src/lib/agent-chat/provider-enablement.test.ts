import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: vi.fn(),
  dbSetSetting: vi.fn(() => Promise.resolve()),
}));

import { useSettingsStore } from "@/stores/settings-store";
import {
  DISABLED_PROVIDERS_SETTING,
  isProviderEnabled,
  parseDisabledProviders,
  setProviderEnabled,
} from "./provider-enablement";

afterEach(() => useSettingsStore.setState({ settings: {} }));

describe("provider enablement", () => {
  it("every provider is on until turned off", () => {
    expect(isProviderEnabled("claude")).toBe(true);
    expect(isProviderEnabled("hermes")).toBe(true);
  });

  it("stores the turned-off set as a stable machine-local list", () => {
    setProviderEnabled("grok", false);
    setProviderEnabled("cursor", false);
    expect(useSettingsStore.getState().settings[DISABLED_PROVIDERS_SETTING]).toBe("cursor,grok");
    expect(isProviderEnabled("grok")).toBe(false);
    setProviderEnabled("grok", true);
    expect(useSettingsStore.getState().settings[DISABLED_PROVIDERS_SETTING]).toBe("cursor");
  });

  it("ignores unknown or blank entries", () => {
    expect([...parseDisabledProviders(" grok, ,nonsense,codex")].sort()).toEqual(["codex", "grok"]);
    expect(parseDisabledProviders(undefined).size).toBe(0);
  });
});
