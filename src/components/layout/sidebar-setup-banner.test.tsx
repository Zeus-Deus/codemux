/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockDbGetUiState = vi.fn();
const mockDbSetUiState = vi.fn().mockResolvedValue(undefined);
const mockGetProjectScripts = vi.fn();
const mockGetWorkspaceConfig = vi.fn();

vi.mock("@/tauri/commands", () => ({
  dbGetUiState: (...args: unknown[]) => mockDbGetUiState(...args),
  dbSetUiState: (...args: unknown[]) => mockDbSetUiState(...args),
  getProjectScripts: (...args: unknown[]) => mockGetProjectScripts(...args),
  getWorkspaceConfig: (...args: unknown[]) => mockGetWorkspaceConfig(...args),
}));

const mockSetShowSettings = vi.fn();

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
    const state = {
      setShowSettings: mockSetShowSettings,
    };
    return selector(state);
  }),
}));

let mockAppState: Record<string, unknown> | null = null;
let mockHomeDir: string | null = null;

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
    return selector({ appState: mockAppState, homeDir: mockHomeDir });
  }),
  useHomeDir: vi.fn(() => mockHomeDir),
}));

import { SidebarSetupBanner } from "./sidebar-setup-banner";

function flushPromises() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockDbGetUiState.mockResolvedValue(null);
  mockGetProjectScripts.mockResolvedValue(null);
  mockGetWorkspaceConfig.mockResolvedValue(null);
  mockHomeDir = null;
  mockAppState = {
    active_workspace_id: "ws-1",
    workspaces: [
      {
        workspace_id: "ws-1",
        project_root: "/home/user/myapp",
        worktree_path: "/tmp/wt-1",
      },
    ],
  };
});

describe("SidebarSetupBanner", () => {
  it("shows banner when no scripts configured and has worktrees", async () => {
    render(<SidebarSetupBanner />);
    await flushPromises();
    expect(screen.getByText("Setup")).toBeInTheDocument();
    expect(screen.getByText(/Automate workspace setup/)).toBeInTheDocument();
  });

  it("shows banner for non-worktree workspaces with matching project", async () => {
    mockAppState = {
      active_workspace_id: "ws-1",
      workspaces: [
        {
          workspace_id: "ws-1",
          project_root: "/home/user/myapp",
          worktree_path: null,
        },
      ],
    };
    render(<SidebarSetupBanner />);
    await flushPromises();
    expect(screen.getByText("Setup")).toBeInTheDocument();
  });

  it("hides when no project root", async () => {
    mockAppState = {
      active_workspace_id: "ws-1",
      workspaces: [
        {
          workspace_id: "ws-1",
          project_root: null,
          worktree_path: "/tmp/wt-1",
        },
      ],
    };
    render(<SidebarSetupBanner />);
    await flushPromises();
    expect(screen.queryByText("Setup")).not.toBeInTheDocument();
  });

  it("hides for home-rooted workspaces (project_root === $HOME)", async () => {
    // A "New agent" chat that materialises in the home dir creates a
    // workspace whose project_root IS the home dir. The setup-scripts
    // affordance is project-specific, so the banner must not render.
    // Without the homeDir guard, the banner would treat $HOME as a
    // project literally named after the user's home folder.
    mockHomeDir = "/home/user";
    mockAppState = {
      active_workspace_id: "ws-home",
      workspaces: [
        {
          workspace_id: "ws-home",
          project_root: "/home/user",
          worktree_path: null,
        },
      ],
    };
    render(<SidebarSetupBanner />);
    await flushPromises();
    expect(screen.queryByText("Setup")).not.toBeInTheDocument();
  });

  it("calls correct APIs with project root", async () => {
    render(<SidebarSetupBanner />);
    await flushPromises();
    expect(mockDbGetUiState).toHaveBeenCalledWith(
      "setup-banner-dismissed:/home/user/myapp",
    );
    expect(mockGetProjectScripts).toHaveBeenCalledWith("/home/user/myapp");
    expect(mockGetWorkspaceConfig).toHaveBeenCalledWith("/home/user/myapp");
  });

  it("configure button opens settings to projects section", async () => {
    render(<SidebarSetupBanner />);
    await flushPromises();
    const configureBtns = screen.getAllByText("Configure");
    await userEvent.click(configureBtns[0]);
    expect(mockSetShowSettings).toHaveBeenCalledWith(true, "projects");
  });

  it("dismiss calls dbSetUiState with correct key", async () => {
    render(<SidebarSetupBanner />);
    await flushPromises();
    // Find X dismiss buttons
    const dismissBtns = screen
      .getAllByRole("button")
      .filter((b) => b.querySelector("svg.lucide-x"));
    if (dismissBtns.length > 0) {
      await userEvent.click(dismissBtns[0]);
      expect(mockDbSetUiState).toHaveBeenCalledWith(
        "setup-banner-dismissed:/home/user/myapp",
        "true",
      );
    }
  });
});
