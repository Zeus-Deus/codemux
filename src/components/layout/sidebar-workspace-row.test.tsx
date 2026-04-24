/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { WorkspaceSnapshot } from "@/tauri/types";

// ── Mocks ──
//
// `vi.mock()` factories are hoisted above `import`s, so any spies they
// reference must be created via `vi.hoisted` to survive that hoist.
const { mockCheckoutDefault, mockGetDefaultBranch, mockToast } = vi.hoisted(
  () => ({
    mockCheckoutDefault: vi.fn(),
    mockGetDefaultBranch: vi.fn().mockResolvedValue("main"),
    mockToast: {
      success: vi.fn(),
      error: vi.fn(),
      info: vi.fn(),
      warning: vi.fn(),
    },
  }),
);

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  checkoutDefaultBranchInWorkspace: (...args: unknown[]) =>
    mockCheckoutDefault(...args),
  closeWorkspace: vi.fn().mockResolvedValue(undefined),
  closeWorkspaceWithWorktree: vi.fn().mockResolvedValue(undefined),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  detectEditors: vi.fn().mockResolvedValue([]),
  getDefaultBranch: (...args: unknown[]) => mockGetDefaultBranch(...args),
  openInEditor: vi.fn().mockResolvedValue(undefined),
  runWorkspaceSetup: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/lib/toast", () => ({
  toast: mockToast,
}));

// Radix UI's context menu uses a Portal that doesn't play nicely with
// jsdom (pointer events, body-level rendering). Replace the primitives
// with transparent wrappers that render children inline — the items and
// handlers are what we actually care about testing.
vi.mock("@/components/ui/context-menu", () => {
  const passthrough = ({ children }: { children?: React.ReactNode }) => <>{children}</>;
  const ContextMenuItem = ({
    children,
    onClick,
    disabled,
  }: {
    children?: React.ReactNode;
    onClick?: () => void;
    disabled?: boolean;
  }) => (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      disabled={disabled}
      data-disabled={disabled ? "true" : undefined}
    >
      {children}
    </button>
  );
  return {
    ContextMenu: passthrough,
    ContextMenuTrigger: passthrough,
    ContextMenuContent: ({ children }: { children?: React.ReactNode }) => (
      <div role="menu">{children}</div>
    ),
    ContextMenuItem,
    ContextMenuSeparator: () => <hr />,
    ContextMenuSub: passthrough,
    ContextMenuSubTrigger: passthrough,
    ContextMenuSubContent: passthrough,
  };
});

// App store — the row reads `workspaceStatus` from here; default to null.
vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) =>
    selector({ appState: null }),
  ),
}));

// Late imports so the mocks above apply.
import {
  SidebarWorkspaceRow,
  WorkspaceContextMenuItems,
} from "./sidebar-workspace-row";
import { __resetDefaultBranchCacheForTests } from "./sidebar-workspace-row.test-utils";

function makeWorkspace(
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test Workspace",
    workspace_type: "standard",
    cwd: "/home/user/projects/myapp",
    git_branch: "feature/x",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/home/user/projects/myapp",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

/** Wait for the async `useDefaultBranch` fetch to resolve and propagate. */
async function flushDefaultBranchFetch() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  cleanup();
  mockCheckoutDefault.mockReset();
  mockGetDefaultBranch.mockReset();
  mockGetDefaultBranch.mockResolvedValue("main");
  mockToast.success.mockReset();
  mockToast.error.mockReset();
  __resetDefaultBranchCacheForTests();
});

describe("Checkout default branch menu item", () => {
  it("renders for a primary workspace on a non-default branch", async () => {
    const ws = makeWorkspace({ worktree_path: null, git_branch: "feature/x" });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );

    await flushDefaultBranchFetch();

    const item = screen.getByRole("menuitem", { name: /Checkout default branch/i });
    expect(item).toBeInTheDocument();
    expect(item).not.toBeDisabled();
  });

  it("does NOT render for a worktree workspace", async () => {
    const ws = makeWorkspace({
      worktree_path: "/home/user/.codemux/worktrees/myapp/feature-x",
      git_branch: "feature/x",
    });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );

    await flushDefaultBranchFetch();

    expect(
      screen.queryByRole("menuitem", { name: /Checkout default branch/i }),
    ).not.toBeInTheDocument();
  });

  it("is disabled when the primary workspace is already on the default branch", async () => {
    const ws = makeWorkspace({ worktree_path: null, git_branch: "main" });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );

    await flushDefaultBranchFetch();

    const item = await screen.findByRole("menuitem", {
      name: /Checkout default branch/i,
    });
    // Wait for the default-branch fetch to resolve and the row to re-render
    // with the disabled state.
    await waitFor(() => expect(item).toBeDisabled());
  });

  it("calls checkoutDefaultBranchInWorkspace with the workspace id on click", async () => {
    mockCheckoutDefault.mockResolvedValueOnce("main");
    const ws = makeWorkspace({
      workspace_id: "ws-xyz",
      worktree_path: null,
      git_branch: "feature/x",
    });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );

    await flushDefaultBranchFetch();

    const item = screen.getByRole("menuitem", { name: /Checkout default branch/i });
    await userEvent.click(item);

    expect(mockCheckoutDefault).toHaveBeenCalledTimes(1);
    expect(mockCheckoutDefault).toHaveBeenCalledWith("ws-xyz");
  });

  it("surfaces a success toast on successful checkout", async () => {
    mockCheckoutDefault.mockResolvedValueOnce("main");
    const ws = makeWorkspace({ worktree_path: null, git_branch: "feature/x" });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );

    await flushDefaultBranchFetch();
    await userEvent.click(
      screen.getByRole("menuitem", { name: /Checkout default branch/i }),
    );

    await waitFor(() =>
      expect(mockToast.success).toHaveBeenCalledWith("Switched to main"),
    );
    expect(mockToast.error).not.toHaveBeenCalled();
  });

  it("surfaces an error toast with git's stderr when the checkout fails", async () => {
    mockCheckoutDefault.mockRejectedValueOnce(
      "error: The following untracked working tree files would be overwritten by checkout",
    );
    const ws = makeWorkspace({ worktree_path: null, git_branch: "feature/x" });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );

    await flushDefaultBranchFetch();
    await userEvent.click(
      screen.getByRole("menuitem", { name: /Checkout default branch/i }),
    );

    await waitFor(() => expect(mockToast.error).toHaveBeenCalledTimes(1));
    const [msg] = mockToast.error.mock.calls[0];
    expect(msg).toMatch(/Couldn't switch:/);
    expect(msg).toMatch(/would be overwritten/);
    expect(mockToast.success).not.toHaveBeenCalled();
  });

  // Smoke test: the row itself should render without crashing with the new
  // menu wiring in place. Guards against regressions from the default-branch
  // fetch effect firing during the row's initial mount.
  it("renders SidebarWorkspaceRow without crashing for a primary workspace", () => {
    const ws = makeWorkspace({ worktree_path: null });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(container.querySelector("svg.lucide-laptop")).toBeInTheDocument();
  });

  // ── Edge cases ──

  it("keeps the item clickable while the default-branch fetch is still in flight", async () => {
    // Never-resolving promise so `defaultBranch` stays null for the whole
    // render. Confirms the "don't block on fetch" rule from the design:
    // the handler must be callable before we know the default branch, and
    // the backend's `Ok(None)` guard is what catches the "actually on
    // default" edge — not a pre-click UI gate.
    mockGetDefaultBranch.mockImplementationOnce(
      () => new Promise<string>(() => {}),
    );
    const ws = makeWorkspace({ worktree_path: null, git_branch: "feature/x" });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );

    const item = screen.getByRole("menuitem", { name: /Checkout default branch/i });
    expect(item).toBeInTheDocument();
    expect(item).not.toBeDisabled();
  });

  it("treats an empty string from getDefaultBranch as unknown (item not pre-disabled)", async () => {
    // Backend returns "" when git_default_branch couldn't resolve anything
    // meaningful. The hook's `branch || null` normalization should coerce
    // that into `null` so the disabled gate doesn't trigger.
    mockGetDefaultBranch.mockResolvedValueOnce("");
    const ws = makeWorkspace({ worktree_path: null, git_branch: "" });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );
    await flushDefaultBranchFetch();

    const item = screen.getByRole("menuitem", { name: /Checkout default branch/i });
    expect(item).not.toBeDisabled();
  });

  it("extracts .message when the checkout rejects with an Error instance", async () => {
    // The Tauri plugin normally rejects with the raw Err(String) from Rust,
    // but `invoke` can also surface an Error-typed value (e.g. if a middle
    // layer throws). The handler's `err instanceof Error ? err.message
    // : String(err)` branch must pull `.message` out cleanly — otherwise
    // the toast shows `String(err)` which prefixes "Error: " (and, for
    // object-shaped errors, would produce "[object Object]").
    mockCheckoutDefault.mockRejectedValueOnce(
      new Error("index.lock exists, another git process seems to be running"),
    );
    const ws = makeWorkspace({ worktree_path: null, git_branch: "feature/x" });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );

    await flushDefaultBranchFetch();
    await userEvent.click(
      screen.getByRole("menuitem", { name: /Checkout default branch/i }),
    );

    await waitFor(() => expect(mockToast.error).toHaveBeenCalledTimes(1));
    const [msg] = mockToast.error.mock.calls[0];
    // Exact-match the shape: no "Error: " prefix from String(err), no
    // "[object Object]" from a non-Error object.
    expect(msg).toBe(
      "Couldn't switch: index.lock exists, another git process seems to be running",
    );
  });

  it("falls back to cwd when project_root is not yet stamped on a primary workspace", async () => {
    // During the brief window between `create_workspace_at_path` and
    // `set_workspace_project_root`, primary rows can momentarily have
    // `project_root: null`. The hook's `project_root ?? cwd` fallback means
    // the default-branch fetch still uses *some* path — and for primary
    // workspaces the cwd is the repo root anyway, so it's the right answer.
    const ws = makeWorkspace({
      worktree_path: null,
      project_root: null,
      cwd: "/home/user/projects/myapp",
      git_branch: "feature/x",
    });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );
    await flushDefaultBranchFetch();

    expect(mockGetDefaultBranch).toHaveBeenCalledWith(
      "/home/user/projects/myapp",
    );
  });

  it("does not fetch getDefaultBranch for worktree workspaces (project_root-scoped no-op)", async () => {
    // Worktree rows don't expose the checkout action, so triggering the
    // default-branch fetch would be wasted work. The fallback in
    // `WorkspaceContextMenuItems` passes `null` for worktree workspaces
    // when `project_root` is unset, and the hook no-ops on a null key.
    const ws = makeWorkspace({
      worktree_path: "/home/user/.codemux/worktrees/myapp/feature-x",
      project_root: null,
      cwd: "/home/user/.codemux/worktrees/myapp/feature-x",
      git_branch: "feature-x",
    });
    render(
      <WorkspaceContextMenuItems workspace={ws} onRemoveRequest={() => {}} />,
    );
    await flushDefaultBranchFetch();

    expect(mockGetDefaultBranch).not.toHaveBeenCalled();
  });
});
