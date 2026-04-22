/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent } from "@testing-library/react";

let enableAgentChatFlag = false;
let appStateSnapshot: unknown = null;

vi.mock("@/tauri/commands", () => ({
  getHomeDir: vi.fn().mockResolvedValue("/home/user"),
  getOrCreateHomeWorkspace: vi.fn().mockResolvedValue("ws-home"),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-home"),
}));

vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: vi.fn((selector) =>
    selector({ enableAgentChat: enableAgentChatFlag, loaded: true }),
  ),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: Object.assign(
    vi.fn((selector) => selector({ appState: appStateSnapshot })),
    { getState: () => ({ appState: appStateSnapshot }) },
  ),
}));

import { SidebarHomeRow } from "./sidebar-home-row";
import {
  activateWorkspace,
  agentChatCreatePane,
  getOrCreateHomeWorkspace,
} from "@/tauri/commands";

describe("SidebarHomeRow", () => {
  beforeEach(() => {
    vi.mocked(getOrCreateHomeWorkspace).mockClear().mockResolvedValue("ws-home");
    vi.mocked(activateWorkspace).mockClear().mockResolvedValue(undefined);
    vi.mocked(agentChatCreatePane).mockClear().mockResolvedValue("pane-home");
    enableAgentChatFlag = false;
    appStateSnapshot = null;
  });

  it("renders when flag is ON", () => {
    enableAgentChatFlag = true;
    const { container } = render(<SidebarHomeRow />);
    const row = container.querySelector('[aria-label="Home"]');
    expect(row).not.toBeNull();
  });

  it("renders nothing when flag is OFF", () => {
    enableAgentChatFlag = false;
    const { container } = render(<SidebarHomeRow />);
    expect(container.querySelector('[aria-label="Home"]')).toBeNull();
  });

  it("click invokes the shared home-chat flow", async () => {
    enableAgentChatFlag = true;
    appStateSnapshot = {
      active_workspace_id: "ws-home",
      workspaces: [
        {
          workspace_id: "ws-home",
          workspace_type: "home",
          active_surface_id: "surf-1",
          surfaces: [
            {
              surface_id: "surf-1",
              root: {
                kind: "terminal",
                pane_id: "pane-t",
                session_id: "sess",
                title: "t",
              },
            },
          ],
        },
      ],
    };
    const { container } = render(<SidebarHomeRow />);
    const row = container.querySelector('[aria-label="Home"]') as HTMLElement;
    fireEvent.click(row);
    await vi.waitFor(() => {
      expect(getOrCreateHomeWorkspace).toHaveBeenCalled();
    });
    expect(activateWorkspace).toHaveBeenCalledWith("ws-home");
  });

  it("applies active styling when active workspace is Home", () => {
    enableAgentChatFlag = true;
    appStateSnapshot = {
      active_workspace_id: "ws-home",
      workspaces: [
        { workspace_id: "ws-home", workspace_type: "home" },
      ],
    };
    const { container } = render(<SidebarHomeRow />);
    const row = container.querySelector('[aria-label="Home"]') as HTMLElement;
    expect(row.className).toContain("bg-muted");
  });

  it("does NOT apply active styling when active workspace is not Home", () => {
    enableAgentChatFlag = true;
    appStateSnapshot = {
      active_workspace_id: "ws-project",
      workspaces: [
        { workspace_id: "ws-project", workspace_type: "standard" },
        { workspace_id: "ws-home", workspace_type: "home" },
      ],
    };
    const { container } = render(<SidebarHomeRow />);
    const row = container.querySelector('[aria-label="Home"]') as HTMLElement;
    // "bg-muted/50" in hover should be present, but "bg-muted" (solid
    // active) should not appear as a bare class.
    expect(row.className).not.toMatch(/\bbg-muted\b(?!\/)/);
  });
});
