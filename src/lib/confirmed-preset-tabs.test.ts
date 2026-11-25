import { beforeEach, describe, expect, it, vi } from "vitest";

const commands = vi.hoisted(() => ({
  activateTab: vi.fn(),
  getAppState: vi.fn(),
}));
vi.mock("@/tauri/commands", () => commands);

import { focusConfirmedPresetTab } from "./confirmed-preset-tabs";

beforeEach(() => {
  commands.activateTab.mockReset().mockResolvedValue(undefined);
  commands.getAppState.mockReset();
});

describe("focusConfirmedPresetTab", () => {
  it("skips a closed tab and a tab that closes during activation", async () => {
    commands.getAppState.mockResolvedValue({
      workspaces: [{
        workspace_id: "ws-new",
        tabs: [
          { tab_id: "tab-closing", kind: "terminal" },
          { tab_id: "tab-surviving", kind: "terminal" },
        ],
      }],
    });
    commands.activateTab.mockRejectedValueOnce(new Error("tab closed"));

    await expect(focusConfirmedPresetTab("ws-new", ["tab-already-closed", "tab-closing", "tab-surviving"]))
      .resolves.toEqual({ tabId: "tab-surviving", openTabCount: 1 });
    expect(commands.activateTab).toHaveBeenNthCalledWith(1, "ws-new", "tab-closing");
    expect(commands.activateTab).toHaveBeenNthCalledWith(2, "ws-new", "tab-surviving");
  });

  it("omits navigation when live tab state cannot be confirmed", async () => {
    commands.getAppState.mockRejectedValue(new Error("state unavailable"));

    await expect(focusConfirmedPresetTab("ws-new", ["tab-old"]))
      .resolves.toEqual({ tabId: null, openTabCount: 0 });
    expect(commands.activateTab).not.toHaveBeenCalled();
  });
});
