import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { useFeatureFlags } from "./feature-flags";

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

function resetStore() {
  const initial = useFeatureFlags.getInitialState();
  useFeatureFlags.setState({
    enableAgentChat: initial.enableAgentChat,
    enableLazyWorkspaceCreation: initial.enableLazyWorkspaceCreation,
    loaded: false,
  });
}

/** Simulate a user who opted back out to the classic CLI interface. */
function setOptedOut() {
  useFeatureFlags.setState({
    enableAgentChat: false,
    enableLazyWorkspaceCreation: false,
    loaded: true,
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

  // The pre-refresh state must match the backend default (GUI on).
  // Booting these OFF flashes the legacy CLI chrome for the frames
  // before `get_feature_flags` resolves.
  it("initial state is ON for both flags and not loaded", () => {
    const s = useFeatureFlags.getInitialState();
    expect(s.enableAgentChat).toBe(true);
    expect(s.enableLazyWorkspaceCreation).toBe(true);
    expect(s.loaded).toBe(false);
  });

  it("refresh() pulls from Tauri and mirrors both fields plus `loaded`", async () => {
    setOptedOut();
    invokeMock.mockResolvedValueOnce({
      enable_agent_chat: true,
      enable_lazy_workspace_creation: true,
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

  // A persisted opt-out must survive a refresh — the on-default only
  // applies until the backend reports otherwise.
  it("refresh() mirrors a persisted opt-out over the on-default", async () => {
    invokeMock.mockResolvedValueOnce({
      enable_agent_chat: false,
      enable_lazy_workspace_creation: false,
      unstable_browser_automation: true,
      unstable_indexing: true,
    });

    await useFeatureFlags.getState().refresh();

    const s = useFeatureFlags.getState();
    expect(s.enableAgentChat).toBe(false);
    expect(s.enableLazyWorkspaceCreation).toBe(false);
    expect(s.loaded).toBe(true);
  });

  // Falling back to OFF here would strand a default-mode user in the
  // CLI chrome just because one invoke rejected.
  it("refresh() falls back to the on-default on error and still flips loaded", async () => {
    setOptedOut();
    invokeMock.mockRejectedValueOnce(new Error("backend boom"));

    await useFeatureFlags.getState().refresh();

    const s = useFeatureFlags.getState();
    expect(s.enableAgentChat).toBe(true);
    expect(s.enableLazyWorkspaceCreation).toBe(true);
    expect(s.loaded).toBe(true);
  });

  it("setAgentChatEnabled(true) calls set_agent_chat_enabled and flips both flags", async () => {
    setOptedOut();
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
    setOptedOut();
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
