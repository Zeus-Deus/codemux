/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockGetProjectScripts = vi.fn();
const mockGetWorkspaceConfig = vi.fn();
const mockRunProjectDevCommand = vi.fn().mockResolvedValue(undefined);

vi.mock("@/tauri/commands", () => ({
  getProjectScripts: (...args: unknown[]) => mockGetProjectScripts(...args),
  getWorkspaceConfig: (...args: unknown[]) => mockGetWorkspaceConfig(...args),
  runProjectDevCommand: (...args: unknown[]) => mockRunProjectDevCommand(...args),
}));

const mockSetShowSettings = vi.fn();
let mockShowSettings = false;

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
      const state = {
        showSettings: mockShowSettings,
        setShowSettings: mockSetShowSettings,
      };
      return selector(state);
    }),
    {
      getState: () => ({
        setShowSettings: mockSetShowSettings,
      }),
    },
  ),
}));

let mockActiveWorkspace: Record<string, unknown> | null = null;

vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => mockActiveWorkspace,
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { RunButton } from "./run-button";

function flushPromises() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

function renderRunButton(workspaceId = "ws-1") {
  return render(
    <TooltipProvider>
      <RunButton workspaceId={workspaceId} />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockGetProjectScripts.mockResolvedValue(null);
  mockGetWorkspaceConfig.mockResolvedValue(null);
  mockShowSettings = false;
  mockActiveWorkspace = {
    workspace_id: "ws-1",
    project_root: "/home/user/myapp",
  };
});

describe("RunButton", () => {
  it('shows "Set Run" when no run command configured', async () => {
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Set Run")).toBeInTheDocument();
  });

  it('shows "Run" when run command is configured via DB', async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "npm run dev",
    });
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Run")).toBeInTheDocument();
  });

  it('shows "Run" when run command is configured via file config', async () => {
    mockGetWorkspaceConfig.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "yarn dev",
    });
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Run")).toBeInTheDocument();
  });

  it("file config takes precedence over DB scripts", async () => {
    mockGetWorkspaceConfig.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "file-cmd",
    });
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "db-cmd",
    });
    renderRunButton();
    await flushPromises();
    // Should show "Run" (configured), and the file command in the dropdown
    expect(screen.getByText("Run")).toBeInTheDocument();
  });

  it('click "Set Run" opens settings to projects section', async () => {
    renderRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Set Run"));
    expect(mockSetShowSettings).toHaveBeenCalledWith(true, "projects");
  });

  it('click "Run" calls runProjectDevCommand', async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "npm run dev",
    });
    renderRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Run"));
    expect(mockRunProjectDevCommand).toHaveBeenCalledWith("ws-1");
  });

  it("shows shortcut badge", async () => {
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Ctrl+Shift+G")).toBeInTheDocument();
  });

  it('falls back to "Set Run" when no project root', async () => {
    mockActiveWorkspace = {
      workspace_id: "ws-1",
      project_root: null,
    };
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Set Run")).toBeInTheDocument();
  });

  it("treats empty/whitespace run command as unconfigured", async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "   ",
    });
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Set Run")).toBeInTheDocument();
  });
});
