/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// ── Mocks ──
//
// `vi.mock()` factories hoist above `import`s, so spies they reference
// come from `vi.hoisted`. `runDeleteWithForceToast` stays REAL — the
// force-escalation path is exactly what these tests exercise — so the
// dirty-worktree case drives the actual helper and we assert on the
// toast action it wires up.
const {
  mockCloseWorkspace,
  mockCloseWorkspaceWithWorktree,
  mockToast,
  mockState,
} = vi.hoisted(() => ({
  mockCloseWorkspace: vi.fn(),
  mockCloseWorkspaceWithWorktree: vi.fn(),
  mockToast: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
    undoable: vi.fn(),
  },
  mockState: { appState: null as unknown },
}));

vi.mock("@/tauri/commands", () => ({
  createTab: vi.fn(),
  createBrowserPane: vi.fn(),
  openInEditor: vi.fn(),
  detectEditors: vi.fn().mockResolvedValue([]),
  closeWorkspace: (...args: unknown[]) => mockCloseWorkspace(...args),
  closeWorkspaceWithWorktree: (...args: unknown[]) =>
    mockCloseWorkspaceWithWorktree(...args),
}));

vi.mock("@/lib/toast", () => ({ toast: mockToast }));

vi.mock("@/stores/app-store", () => ({
  useAppStore: (selector: (s: typeof mockState) => unknown) =>
    selector(mockState),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: () => vi.fn(),
}));

import { EmptyWorkspaceState } from "./empty-workspace-state";

interface WsOverrides {
  worktree_path?: string | null;
  protected?: boolean;
  title?: string;
}

function setActiveWorkspace(overrides: WsOverrides = {}) {
  mockState.appState = {
    active_workspace_id: "ws-1",
    workspaces: [
      {
        workspace_id: "ws-1",
        title: overrides.title ?? "my-workspace",
        worktree_path:
          overrides.worktree_path === undefined
            ? null
            : overrides.worktree_path,
        protected: overrides.protected ?? false,
        surfaces: [],
        cwd: "/home/user/project",
      },
    ],
  };
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockCloseWorkspace.mockResolvedValue(undefined);
  mockCloseWorkspaceWithWorktree.mockResolvedValue(undefined);
  vi.spyOn(window, "confirm").mockReturnValue(true);
});

describe("EmptyWorkspaceState — delete/close affordance", () => {
  it("closes (non-destructive) a workspace with no worktree of its own", async () => {
    // Primary checkout / plain folder / Home: worktree_path is null. A
    // destructive close would be refused by the backend guard, so the UI
    // must issue a plain non-destructive close instead.
    setActiveWorkspace({ worktree_path: null });
    render(<EmptyWorkspaceState />);

    const button = screen.getByRole("button", { name: /close workspace/i });
    await userEvent.click(button);

    expect(mockCloseWorkspace).toHaveBeenCalledWith("ws-1", false);
    expect(mockCloseWorkspaceWithWorktree).not.toHaveBeenCalled();
  });

  it("does not offer a destructive delete for a protected repo root", async () => {
    // A protected root has a worktree_path but must never be deleted like
    // a disposable worktree — it falls through to the close path.
    setActiveWorkspace({
      worktree_path: "/home/user/project",
      protected: true,
    });
    render(<EmptyWorkspaceState />);

    const button = screen.getByRole("button", { name: /close workspace/i });
    await userEvent.click(button);

    expect(mockCloseWorkspace).toHaveBeenCalledWith("ws-1", false);
    expect(mockCloseWorkspaceWithWorktree).not.toHaveBeenCalled();
  });

  it("deletes a per-branch worktree with removeWorktree=true", async () => {
    setActiveWorkspace({
      worktree_path: "/home/user/.codemux/worktrees/app/feature",
    });
    render(<EmptyWorkspaceState />);

    const button = screen.getByRole("button", { name: /delete workspace/i });
    await userEvent.click(button);

    expect(mockCloseWorkspaceWithWorktree).toHaveBeenCalledWith(
      "ws-1",
      true,
      false,
      false,
    );
    expect(mockCloseWorkspace).not.toHaveBeenCalled();
  });

  it("escalates a dirty-worktree rejection to the force path", async () => {
    setActiveWorkspace({
      worktree_path: "/home/user/.codemux/worktrees/app/feature",
    });
    // First (non-forced) attempt is refused with the /use force/i
    // contract message; the retry (force=true) succeeds.
    mockCloseWorkspaceWithWorktree
      .mockRejectedValueOnce(
        new Error("Worktree has 1 uncommitted change(s). Use force to override."),
      )
      .mockResolvedValueOnce(undefined);

    render(<EmptyWorkspaceState />);
    await userEvent.click(
      screen.getByRole("button", { name: /delete workspace/i }),
    );

    // The non-forced attempt refused → an error toast with a "Force
    // delete" action was raised.
    expect(mockCloseWorkspaceWithWorktree).toHaveBeenNthCalledWith(
      1,
      "ws-1",
      true,
      false,
      false,
    );
    const calls = mockToast.error.mock.calls;
    const errorCall = calls[calls.length - 1];
    const action = (
      errorCall?.[1] as { action?: { label: string; onClick: () => void } }
    )?.action;
    expect(action?.label).toBe("Force delete");

    // Invoking the toast's "Force delete" action reissues with force=true.
    action?.onClick();
    expect(mockCloseWorkspaceWithWorktree).toHaveBeenNthCalledWith(
      2,
      "ws-1",
      true,
      false,
      true,
    );
  });
});
