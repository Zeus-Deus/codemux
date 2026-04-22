/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook } from "@testing-library/react";

let enableAgentChatFlag = false;
let flagsLoaded = false;
let appStateSnapshot: unknown = null;

vi.mock("@/tauri/commands", () => ({
  getHomeDir: vi.fn().mockResolvedValue("/home/user"),
  getOrCreateHomeWorkspace: vi.fn().mockResolvedValue("ws-home"),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-home"),
}));

vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: vi.fn((selector) =>
    selector({ enableAgentChat: enableAgentChatFlag, loaded: flagsLoaded }),
  ),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: Object.assign(
    vi.fn((selector) => selector({ appState: appStateSnapshot })),
    { getState: () => ({ appState: appStateSnapshot }) },
  ),
}));

import { useBootIntoHome } from "./use-boot-into-home";
import { getOrCreateHomeWorkspace, activateWorkspace } from "@/tauri/commands";

const terminalSurface = {
  surface_id: "surf-1",
  root: {
    kind: "terminal",
    pane_id: "pane-t",
    session_id: "sess-1",
    title: "term",
  },
};

const emptySplitSurface = {
  surface_id: "surf-empty",
  root: {
    kind: "split",
    pane_id: "s-empty",
    direction: "horizontal",
    child_sizes: [],
    children: [],
  },
};

describe("useBootIntoHome", () => {
  beforeEach(() => {
    vi.mocked(getOrCreateHomeWorkspace).mockClear().mockResolvedValue("ws-home");
    vi.mocked(activateWorkspace).mockClear().mockResolvedValue(undefined);
    enableAgentChatFlag = false;
    flagsLoaded = false;
    appStateSnapshot = null;
  });

  it("does nothing while app state is still null", () => {
    enableAgentChatFlag = true;
    flagsLoaded = true;
    appStateSnapshot = null;
    renderHook(() => useBootIntoHome());
    expect(getOrCreateHomeWorkspace).not.toHaveBeenCalled();
  });

  it("does nothing while feature flags are still loading", () => {
    enableAgentChatFlag = true;
    flagsLoaded = false;
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    renderHook(() => useBootIntoHome());
    expect(getOrCreateHomeWorkspace).not.toHaveBeenCalled();
  });

  it("does nothing when flag is OFF", () => {
    enableAgentChatFlag = false;
    flagsLoaded = true;
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    renderHook(() => useBootIntoHome());
    expect(getOrCreateHomeWorkspace).not.toHaveBeenCalled();
  });

  it("does nothing when the restored workspace has a real terminal pane", () => {
    enableAgentChatFlag = true;
    flagsLoaded = true;
    appStateSnapshot = {
      active_workspace_id: "ws-restored",
      workspaces: [
        {
          workspace_id: "ws-restored",
          active_surface_id: "surf-1",
          surfaces: [terminalSurface],
        },
      ],
    };
    renderHook(() => useBootIntoHome());
    expect(getOrCreateHomeWorkspace).not.toHaveBeenCalled();
  });

  it("opens Home when no active workspace", async () => {
    enableAgentChatFlag = true;
    flagsLoaded = true;
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    renderHook(() => useBootIntoHome());
    await vi.waitFor(() => {
      expect(getOrCreateHomeWorkspace).toHaveBeenCalledTimes(1);
    });
    expect(activateWorkspace).toHaveBeenCalledWith("ws-home");
  });

  it("opens Home when active workspace exists but its active surface has an empty pane tree", async () => {
    enableAgentChatFlag = true;
    flagsLoaded = true;
    appStateSnapshot = {
      active_workspace_id: "ws-restored",
      workspaces: [
        {
          workspace_id: "ws-restored",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
        },
      ],
    };
    renderHook(() => useBootIntoHome());
    await vi.waitFor(() => {
      expect(getOrCreateHomeWorkspace).toHaveBeenCalledTimes(1);
    });
  });

  it("opens Home when active workspace has NO surfaces at all", async () => {
    enableAgentChatFlag = true;
    flagsLoaded = true;
    appStateSnapshot = {
      active_workspace_id: "ws-restored",
      workspaces: [
        {
          workspace_id: "ws-restored",
          active_surface_id: "",
          surfaces: [],
        },
      ],
    };
    renderHook(() => useBootIntoHome());
    await vi.waitFor(() => {
      expect(getOrCreateHomeWorkspace).toHaveBeenCalledTimes(1);
    });
  });

  it("opens Home when active_workspace_id points to a missing workspace", async () => {
    enableAgentChatFlag = true;
    flagsLoaded = true;
    appStateSnapshot = {
      active_workspace_id: "ws-stale",
      workspaces: [],
    };
    renderHook(() => useBootIntoHome());
    await vi.waitFor(() => {
      expect(getOrCreateHomeWorkspace).toHaveBeenCalledTimes(1);
    });
  });

  it("fires only once even if the hook re-renders", async () => {
    enableAgentChatFlag = true;
    flagsLoaded = true;
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    const { rerender } = renderHook(() => useBootIntoHome());
    await vi.waitFor(() => {
      expect(getOrCreateHomeWorkspace).toHaveBeenCalledTimes(1);
    });
    rerender();
    rerender();
    expect(getOrCreateHomeWorkspace).toHaveBeenCalledTimes(1);
  });
});
