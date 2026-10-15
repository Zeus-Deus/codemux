import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: vi.fn(),
  dbSetSetting: vi.fn(() => Promise.resolve()),
}));

import { useSettingsStore } from "@/stores/settings-store";
import {
  DISABLED_PROVIDERS_SETTING,
  isLastDefaultProvider,
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

  it("refuses to turn off the last provider a new chat can start on", () => {
    for (const p of ["codex", "cursor", "grok", "opencode"] as const) expect(setProviderEnabled(p, false)).toBe(true);
    expect(isLastDefaultProvider("claude")).toBe(true);
    expect(setProviderEnabled("claude", false)).toBe(false);
    expect(isProviderEnabled("claude")).toBe(true);
    // Hermes and custom agents can still be turned off freely.
    expect(setProviderEnabled("hermes", false)).toBe(true);
    expect(setProviderEnabled("acp", false)).toBe(true);
    expect(isLastDefaultProvider("hermes")).toBe(false);
  });

  it("ignores unknown or blank entries", () => {
    expect([...parseDisabledProviders(" grok, ,nonsense,codex")].sort()).toEqual(["codex", "grok"]);
    expect(parseDisabledProviders(undefined).size).toBe(0);
  });
});
