/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { WorkspaceSnapshot } from "@/tauri/types";

// Mock Tauri commands
vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  closeWorkspace: vi.fn().mockResolvedValue(undefined),
  closeWorkspaceWithWorktree: vi.fn().mockResolvedValue(undefined),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  detectEditors: vi.fn().mockResolvedValue([]),
  openInEditor: vi.fn().mockResolvedValue(undefined),
  dbGetUiState: vi.fn().mockResolvedValue(null),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
}));

// Mock stores
vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector) => {
    const state = {
      showNewWorkspaceDialog: false,
      setShowNewWorkspaceDialog: vi.fn(),
    };
    return selector(state);
  }),
}));

import { SidebarProjectGroup } from "./sidebar-project-group";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test Workspace",
    workspace_type: "standard",
    cwd: "/home/user/projects/myapp",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

describe("SidebarProjectGroup", () => {
  it("shows letter avatar with first letter of project name", () => {
    render(
      <TooltipProvider>
        <SidebarProjectGroup
          projectName="codemux"
          projectPath="/home/user/codemux"
          workspaces={[]}
          activeWorkspaceId=""
        />
      </TooltipProvider>,
    );
    // First letter uppercase
    expect(screen.getByText("C")).toBeInTheDocument();
  });

  it("default avatar has neutral styling (no custom color)", () => {
    render(
      <TooltipProvider>
        <SidebarProjectGroup
          projectName="myproject"
          projectPath="/home/user/myproject"
          workspaces={[]}
          activeWorkspaceId=""
        />
      </TooltipProvider>,
    );
    const avatar = screen.getByText("M").closest("div");
    // Should have muted classes, no inline style for color
    expect(avatar).toHaveClass("bg-muted");
    expect(avatar).toHaveClass("text-muted-foreground");
    expect(avatar?.style.color).toBeFalsy();
  });
});

describe("SidebarWorkspaceRow", () => {
  it("shows Laptop icon for primary checkout (no worktree_path)", () => {
    const ws = makeWorkspace({ worktree_path: null });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    // Laptop icon renders as an SVG — check for the lucide class
    const laptopIcon = container.querySelector("svg.lucide-laptop");
    expect(laptopIcon).toBeInTheDocument();
  });

  it("shows GitBranch icon for worktree checkout", () => {
    const ws = makeWorkspace({ worktree_path: "/home/user/.worktrees/feature" });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    const branchIcon = container.querySelector("svg.lucide-git-branch");
    expect(branchIcon).toBeInTheDocument();
  });

  it("shows remove button for primary checkout", () => {
    const ws = makeWorkspace({ worktree_path: null });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(container.querySelector("[aria-label='Remove workspace']")).not.toBeNull();
  });

  it("shows remove button for worktree checkout", () => {
    const ws = makeWorkspace({ worktree_path: "/home/user/.worktrees/feature" });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(container.querySelector("[aria-label='Remove workspace']")).not.toBeNull();
  });

  it("shows ahead/behind indicators when counts > 0", () => {
    const ws = makeWorkspace({ git_ahead: 3, git_behind: 1 });
    render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(screen.getByText("↑3")).toBeInTheDocument();
    expect(screen.getByText("↓1")).toBeInTheDocument();
  });

  it("hides ahead/behind when both are 0", () => {
    const ws = makeWorkspace({ git_ahead: 0, git_behind: 0 });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(container.textContent).not.toMatch(/↑\d/);
    expect(container.textContent).not.toMatch(/↓\d/);
  });

  it("shows diff counts on non-active workspaces", () => {
    const ws = makeWorkspace({ git_additions: 42, git_deletions: 7 });
    render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(screen.getByText("+42")).toBeInTheDocument();
    expect(screen.getByText("−7")).toBeInTheDocument();
  });

  it("shows diff counts on active workspace too", () => {
    const ws = makeWorkspace({ git_additions: 10, git_deletions: 3 });
    render(
      <SidebarWorkspaceRow workspace={ws} isActive={true} />,
    );
    expect(screen.getByText("+10")).toBeInTheDocument();
    expect(screen.getByText("−3")).toBeInTheDocument();
  });
});
