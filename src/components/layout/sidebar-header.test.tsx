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

vi.mock("@/tauri/commands", () => ({
  getHomeDir: vi.fn().mockResolvedValue("/home/user"),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-home"),
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

import { SidebarHeader } from "./sidebar-header";
import {
  activateWorkspace,
  agentChatCreatePane,
  createEmptyWorkspace,
  getHomeDir,
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
    vi.mocked(createEmptyWorkspace).mockClear();
    vi.mocked(createEmptyWorkspace).mockResolvedValue("ws-home");
    vi.mocked(activateWorkspace).mockClear();
    vi.mocked(agentChatCreatePane).mockClear();
    enableAgentChatFlag = false;
  });

  it("flag OFF + plain click → opens NewWorkspaceDialog", () => {
    enableAgentChatFlag = false;
    const { plus } = renderHeader();
    fireEvent.click(plus);
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    expect(getHomeDir).not.toHaveBeenCalled();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("flag OFF + Shift+click → opens NewWorkspaceDialog", () => {
    enableAgentChatFlag = false;
    const { plus } = renderHeader();
    fireEvent.click(plus, { shiftKey: true });
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    expect(getHomeDir).not.toHaveBeenCalled();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("flag ON + plain click → creates empty workspace at home, activates, opens chat pane", async () => {
    enableAgentChatFlag = true;
    const { plus } = renderHeader();
    fireEvent.click(plus);
    await vi.waitFor(() => {
      expect(agentChatCreatePane).toHaveBeenCalled();
    });
    expect(getHomeDir).toHaveBeenCalled();
    expect(createEmptyWorkspace).toHaveBeenCalledWith("/home/user", { skipSetup: true });
    expect(activateWorkspace).toHaveBeenCalledWith("ws-home");
    expect(agentChatCreatePane).toHaveBeenCalledWith("ws-home", null, "/home/user");
    expect(setShowDialogMock).not.toHaveBeenCalled();
  });

  it("flag ON + Shift+click → opens dialog, does NOT call chat commands", () => {
    enableAgentChatFlag = true;
    const { plus } = renderHeader();
    fireEvent.click(plus, { shiftKey: true });
    expect(setShowDialogMock).toHaveBeenCalledWith(true);
    expect(getHomeDir).not.toHaveBeenCalled();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("flag ON + getHomeDir rejects → falls back to dialog", async () => {
    enableAgentChatFlag = true;
    vi.mocked(getHomeDir).mockRejectedValueOnce(new Error("no home"));
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
