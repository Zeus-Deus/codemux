import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { useFeatureFlags } from "./feature-flags";

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

function resetStore() {
  useFeatureFlags.setState({
    enableAgentChat: false,
    enableLazyWorkspaceCreation: false,
    loaded: false,
  });
}

describe("useFeatureFlags", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    resetStore();
  });

  afterEach(() => {
    resetStore();
  });

  it("initial state is OFF for both flags and not loaded", () => {
    const s = useFeatureFlags.getState();
    expect(s.enableAgentChat).toBe(false);
    expect(s.enableLazyWorkspaceCreation).toBe(false);
    expect(s.loaded).toBe(false);
  });

  it("refresh() pulls from Tauri and mirrors both fields plus `loaded`", async () => {
    invokeMock.mockResolvedValueOnce({
      enable_agent_chat: true,
      enable_lazy_workspace_creation: true,
      unstable_openflow: true,
      unstable_browser_automation: true,
      unstable_indexing: true,
    });

    await useFeatureFlags.getState().refresh();

    expect(invokeMock).toHaveBeenCalledWith("get_feature_flags");
    const s = useFeatureFlags.getState();
    expect(s.enableAgentChat).toBe(true);
    expect(s.enableLazyWorkspaceCreation).toBe(true);
    expect(s.loaded).toBe(true);
  });

  it("refresh() falls back to OFF on error and still flips loaded", async () => {
    invokeMock.mockRejectedValueOnce(new Error("backend boom"));

    await useFeatureFlags.getState().refresh();

    const s = useFeatureFlags.getState();
    expect(s.enableAgentChat).toBe(false);
    expect(s.enableLazyWorkspaceCreation).toBe(false);
    expect(s.loaded).toBe(true);
  });

  it("setAgentChatEnabled(true) calls set_agent_chat_enabled and flips both flags", async () => {
    invokeMock.mockResolvedValueOnce(undefined);

    await useFeatureFlags.getState().setAgentChatEnabled(true);

    expect(invokeMock).toHaveBeenCalledWith("set_agent_chat_enabled", {
      enabled: true,
    });
    const s = useFeatureFlags.getState();
    expect(s.enableAgentChat).toBe(true);
    expect(s.enableLazyWorkspaceCreation).toBe(true);
  });

  it("setAgentChatEnabled(false) flips both flags off in lockstep", async () => {
    // Start with the toggle on.
    useFeatureFlags.setState({
      enableAgentChat: true,
      enableLazyWorkspaceCreation: true,
      loaded: true,
    });
    invokeMock.mockResolvedValueOnce(undefined);

    await useFeatureFlags.getState().setAgentChatEnabled(false);

    expect(invokeMock).toHaveBeenCalledWith("set_agent_chat_enabled", {
      enabled: false,
    });
    const s = useFeatureFlags.getState();
    expect(s.enableAgentChat).toBe(false);
    expect(s.enableLazyWorkspaceCreation).toBe(false);
  });

  it("setAgentChatEnabled propagates the backend error and does not mutate state", async () => {
    invokeMock.mockRejectedValueOnce(new Error("disk full"));

    await expect(
      useFeatureFlags.getState().setAgentChatEnabled(true),
    ).rejects.toThrow("disk full");

    // State unchanged on failure — the user sees the error toast and
    // the toggle springs back. If we updated state optimistically the
    // UI would lie about the persisted value.
    const s = useFeatureFlags.getState();
    expect(s.enableAgentChat).toBe(false);
    expect(s.enableLazyWorkspaceCreation).toBe(false);
  });
});
