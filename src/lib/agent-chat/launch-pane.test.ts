import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({
  agentChatCreatePane: vi.fn(async () => "pane-new"),
}));

import { agentChatCreatePane } from "@/tauri/commands";
import { useProviderRuntimeIntent } from "@/stores/provider-runtime-intent-store";
import { launchAgentChatPane } from "./launch-pane";

describe("launchAgentChatPane", () => {
  beforeEach(() => {
    useProviderRuntimeIntent.getState().reset();
    vi.mocked(agentChatCreatePane).mockClear();
  });

  it("records runtime intent for the launched provider before creating the pane", async () => {
    let intentAtCreate: boolean | undefined;
    vi.mocked(agentChatCreatePane).mockImplementationOnce(async () => {
      intentAtCreate = useProviderRuntimeIntent.getState().providers.codex === true;
      return "pane-new";
    });

    await expect(
      launchAgentChatPane("ws-1", "codex", "/repo", "new_tab"),
    ).resolves.toBe("pane-new");
    expect(intentAtCreate).toBe(true);
    expect(agentChatCreatePane).toHaveBeenCalledWith(
      "ws-1",
      "codex",
      "/repo",
      "new_tab",
    );
  });

  it("treats a null provider as the pane's Claude default", async () => {
    await launchAgentChatPane("ws-1", null, "/repo");
    expect(useProviderRuntimeIntent.getState().providers.claude).toBe(true);
    expect(useProviderRuntimeIntent.getState().providers.codex).toBeUndefined();
    expect(agentChatCreatePane).toHaveBeenCalledWith("ws-1", null, "/repo");
  });
});
