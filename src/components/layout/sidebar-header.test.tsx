/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SidebarProvider } from "@/components/ui/sidebar";

// shadcn SidebarProvider uses useIsMobile → window.matchMedia, which
// jsdom doesn't implement. Stub a minimal shape so the effect runs.
if (typeof window !== "undefined" && !window.matchMedia) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

const setShowDialogMock = vi.fn();
let enableAgentChatFlag = false;
let appStateSnapshot: unknown = null;

vi.mock("@/tauri/commands", () => ({
  getHomeDir: vi.fn().mockResolvedValue("/home/user"),
  getOrCreateHomeWorkspace: vi.fn().mockResolvedValue("ws-home"),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-home"),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector) => {
    const state = {
      showNewWorkspaceDialog: false,
      setShowNewWorkspaceDialog: setShowDialogMock,
    };
    return selector(state);
  }),
}));

vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: vi.fn((selector) => {
    const state = { enableAgentChat: enableAgentChatFlag, loaded: true };
    return selector(state);
  }),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: Object.assign(
    vi.fn((selector) => selector({ appState: appStateSnapshot })),
    { getState: () => ({ appState: appStateSnapshot }) },
  ),
}));

import { SidebarHeader } from "./sidebar-header";
import {
  activateWorkspace,
  agentChatCreatePane,
  getHomeDir,
  getOrCreateHomeWorkspace,
} from "@/tauri/commands";

function renderHeader() {
  const utils = render(
    <TooltipProvider>
      <SidebarProvider>
        <SidebarHeader />
      </SidebarProvider>
    </TooltipProvider>,
  );
  const plus = utils.container.querySelector(
    'button[aria-label="New workspace"]',
  ) as HTMLElement;
  return { ...utils, plus };
}

describe("SidebarHeader + button", () => {
  beforeEach(() => {
    setShowDialogMock.mockClear();
    vi.mocked(getHomeDir).mockClear();
    vi.mocked(getHomeDir).mockResolvedValue("/home/user");
    vi.mocked(getOrCreateHomeWorkspace).mockClear();
    vi.mocked(getOrCreateHomeWorkspace).mockResolvedValue("ws-home");
    vi.mocked(activateWorkspace).mockClear();
    vi.mocked(agentChatCreatePane).mockClear();
    enableAgentChatFlag = false;
    appStateSnapshot = null;
  });

  it("flag OFF + plain click → opens NewWorkspaceDialog", () => {
    enableAgentChatFlag = false;
    const { plus } = renderHeader();
    fireEvent.click(plus);
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    expect(getOrCreateHomeWorkspace).not.toHaveBeenCalled();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("flag OFF + Shift+click → opens NewWorkspaceDialog", () => {
    enableAgentChatFlag = false;
    const { plus } = renderHeader();
    fireEvent.click(plus, { shiftKey: true });
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    expect(getOrCreateHomeWorkspace).not.toHaveBeenCalled();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("flag ON + plain click (no prior chat pane) → gets Home, activates, spawns chat pane", async () => {
    enableAgentChatFlag = true;
    // Home exists but has no chat pane on its active surface.
    appStateSnapshot = {
      active_workspace_id: "ws-home",
      workspaces: [
        {
          workspace_id: "ws-home",
          active_surface_id: "surf-1",
          surfaces: [
            {
              surface_id: "surf-1",
              root: {
                kind: "terminal",
                pane_id: "pane-term",
                session_id: "sess-1",
                title: "Terminal",
              },
            },
          ],
        },
      ],
    };
    const { plus } = renderHeader();
    fireEvent.click(plus);
    await vi.waitFor(() => {
      expect(agentChatCreatePane).toHaveBeenCalled();
    });
    expect(getOrCreateHomeWorkspace).toHaveBeenCalled();
    expect(activateWorkspace).toHaveBeenCalledWith("ws-home");
    expect(getHomeDir).toHaveBeenCalled();
    expect(agentChatCreatePane).toHaveBeenCalledWith("ws-home", null, "/home/user");
    expect(setShowDialogMock).not.toHaveBeenCalled();
  });

  it("flag ON + plain click (Home already has chat pane) → activates only, no duplicate pane", async () => {
    enableAgentChatFlag = true;
    appStateSnapshot = {
      active_workspace_id: "ws-home",
      workspaces: [
        {
          workspace_id: "ws-home",
          active_surface_id: "surf-1",
          surfaces: [
            {
              surface_id: "surf-1",
              root: {
                kind: "agent_chat",
                pane_id: "pane-chat",
                title: "Chat",
                thread_id: null,
                provider: null,
                cwd: null,
              },
            },
          ],
        },
      ],
    };
    const { plus } = renderHeader();
    fireEvent.click(plus);
    await vi.waitFor(() => {
      expect(activateWorkspace).toHaveBeenCalledWith("ws-home");
    });
    expect(getOrCreateHomeWorkspace).toHaveBeenCalled();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
    expect(setShowDialogMock).not.toHaveBeenCalled();
  });

  it("flag ON + Shift+click → opens dialog, does NOT call chat commands", () => {
    enableAgentChatFlag = true;
    const { plus } = renderHeader();
    fireEvent.click(plus, { shiftKey: true });
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    expect(getOrCreateHomeWorkspace).not.toHaveBeenCalled();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("flag ON + getOrCreateHomeWorkspace rejects → falls back to dialog", async () => {
    enableAgentChatFlag = true;
    vi.mocked(getOrCreateHomeWorkspace).mockRejectedValueOnce(new Error("boom"));
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const { plus } = renderHeader();
    fireEvent.click(plus);
    await vi.waitFor(() => {
      expect(setShowDialogMock).toHaveBeenCalledWith(true);
    });
    expect(agentChatCreatePane).not.toHaveBeenCalled();
    errSpy.mockRestore();
  });
});
