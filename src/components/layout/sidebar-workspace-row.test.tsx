/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useSettingsStore } from "@/stores/settings-store";
import type { WorkspaceSnapshot } from "@/tauri/types";

// ── Mocks ──
//
// `vi.mock()` factories are hoisted above `import`s, so any spies they
// reference must be created via `vi.hoisted` to survive that hoist.
const {
  mockArchiveWorkspace,
  mockUnarchiveWorkspace,
  mockCheckoutDefault,
  mockCloseWorkspace,
  mockCloseWorkspaceWithWorktree,
  mockGetDefaultBranch,
  mockSetWorkspaceMuted,
  mockToast,
} = vi.hoisted(() => ({
  mockArchiveWorkspace: vi.fn(),
  mockUnarchiveWorkspace: vi.fn(),
  mockCheckoutDefault: vi.fn(),
  mockCloseWorkspace: vi.fn(),
  mockCloseWorkspaceWithWorktree: vi.fn(),
  mockGetDefaultBranch: vi.fn().mockResolvedValue("main"),
  mockSetWorkspaceMuted: vi.fn().mockResolvedValue(undefined),
  mockToast: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
    undoable: vi.fn(),
  },
}));

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  archiveWorkspace: (...args: unknown[]) => mockArchiveWorkspace(...args),
  unarchiveWorkspace: (...args: unknown[]) =>
    mockUnarchiveWorkspace(...args),
  checkoutDefaultBranchInWorkspace: (...args: unknown[]) =>
    mockCheckoutDefault(...args),
  closeWorkspace: (...args: unknown[]) => mockCloseWorkspace(...args),
  closeWorkspaceWithWorktree: (...args: unknown[]) =>
    mockCloseWorkspaceWithWorktree(...args),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  setWorkspaceMuted: (...args: unknown[]) => mockSetWorkspaceMuted(...args),
  detectEditors: vi.fn().mockResolvedValue([]),
  getDefaultBranch: (...args: unknown[]) => mockGetDefaultBranch(...args),
  openInEditor: vi.fn().mockResolvedValue(undefined),
  runWorkspaceSetup: vi.fn().mockResolvedValue(undefined),
  // Added in cloud-push step 2: the workspace row's context menu
  // now lists configured hosts under "Move to host…" and surfaces
  // Pull back / push handlers. Mock them as no-ops so the existing
  // checkout-default tests keep passing.
  hostsList: vi.fn().mockResolvedValue([]),
  setWorkspaceHost: vi.fn().mockResolvedValue(undefined),
  workspacePushToHost: vi
    .fn()
    .mockResolvedValue({ ok: true, message: "", remote_path: null, rsync_summary: null }),
  workspacePullBack: vi
    .fn()
    .mockResolvedValue({ ok: true, message: "", rsync_summary: null }),
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
    notifications_muted: false,
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
  // Pin the sidebar detail level to "detailed" for the broad suite so
  // tests that assert on branch / git-stat / indicator rendering don't
  // depend on the product default (which is "clean"). The detail-level
  // describe block overrides this per-case.
  useSettingsStore.setState({
    settings: { "sidebar.workspace_detail": "detailed" },
  });
  mockArchiveWorkspace.mockReset();
  mockArchiveWorkspace.mockResolvedValue("archive-1");
  mockUnarchiveWorkspace.mockReset();
  mockUnarchiveWorkspace.mockResolvedValue("ws-1");
  mockCheckoutDefault.mockReset();
  mockCloseWorkspace.mockReset();
  mockCloseWorkspace.mockResolvedValue(undefined);
  mockCloseWorkspaceWithWorktree.mockReset();
  mockCloseWorkspaceWithWorktree.mockResolvedValue(undefined);
  mockGetDefaultBranch.mockReset();
  mockGetDefaultBranch.mockResolvedValue("main");
  mockSetWorkspaceMuted.mockReset();
  mockSetWorkspaceMuted.mockResolvedValue(undefined);
  mockToast.success.mockReset();
  mockToast.error.mockReset();
  mockToast.undoable.mockReset();
  __resetDefaultBranchCacheForTests();
});

describe("Checkout default branch menu item", () => {
  it("renders for a primary workspace on a non-default branch", async () => {
    const ws = makeWorkspace({ worktree_path: null, git_branch: "feature/x" });
    render(
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
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
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
    );

    await flushDefaultBranchFetch();

    expect(
      screen.queryByRole("menuitem", { name: /Checkout default branch/i }),
    ).not.toBeInTheDocument();
  });

  it("is disabled when the primary workspace is already on the default branch", async () => {
    const ws = makeWorkspace({ worktree_path: null, git_branch: "main" });
    render(
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
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
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
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
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
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
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
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
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
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
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
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
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
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
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
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
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
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
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
    );
    await flushDefaultBranchFetch();

    expect(mockGetDefaultBranch).not.toHaveBeenCalled();
  });
});

describe("Mute notifications menu item", () => {
  it("shows 'Mute notifications' when the workspace is not muted", async () => {
    const ws = makeWorkspace({ notifications_muted: false });
    render(
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
    );
    await flushDefaultBranchFetch();

    expect(
      screen.getByRole("menuitem", { name: /^Mute notifications$/i }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("menuitem", { name: /Unmute notifications/i }),
    ).not.toBeInTheDocument();
  });

  it("shows 'Unmute notifications' when the workspace is already muted", async () => {
    const ws = makeWorkspace({ notifications_muted: true });
    render(
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
    );
    await flushDefaultBranchFetch();

    expect(
      screen.getByRole("menuitem", { name: /Unmute notifications/i }),
    ).toBeInTheDocument();
  });

  it("calls setWorkspaceMuted(id, true) when muting an unmuted workspace", async () => {
    const ws = makeWorkspace({
      workspace_id: "ws-mute",
      notifications_muted: false,
    });
    render(
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
    );
    await flushDefaultBranchFetch();

    await userEvent.click(
      screen.getByRole("menuitem", { name: /^Mute notifications$/i }),
    );

    expect(mockSetWorkspaceMuted).toHaveBeenCalledTimes(1);
    expect(mockSetWorkspaceMuted).toHaveBeenCalledWith("ws-mute", true);
  });

  it("calls setWorkspaceMuted(id, false) when unmuting a muted workspace", async () => {
    const ws = makeWorkspace({
      workspace_id: "ws-mute",
      notifications_muted: true,
    });
    render(
      <WorkspaceContextMenuItems workspace={ws} onArchiveRequest={() => {}} onDeleteRequest={() => {}} />,
    );
    await flushDefaultBranchFetch();

    await userEvent.click(
      screen.getByRole("menuitem", { name: /Unmute notifications/i }),
    );

    expect(mockSetWorkspaceMuted).toHaveBeenCalledTimes(1);
    expect(mockSetWorkspaceMuted).toHaveBeenCalledWith("ws-mute", false);
  });
});

describe("Muted indicator on the workspace row", () => {
  it("renders the BellOff indicator only when the workspace is muted", () => {
    const muted = render(
      <TooltipProvider>
        <SidebarWorkspaceRow
          workspace={makeWorkspace({ worktree_path: null, notifications_muted: true })}
          isActive={false}
        />
      </TooltipProvider>,
    );
    expect(
      muted.container.querySelector("svg.lucide-bell-off"),
    ).toBeInTheDocument();
    cleanup();

    const unmuted = render(
      <TooltipProvider>
        <SidebarWorkspaceRow
          workspace={makeWorkspace({ worktree_path: null, notifications_muted: false })}
          isActive={false}
        />
      </TooltipProvider>,
    );
    expect(
      unmuted.container.querySelector("svg.lucide-bell-off"),
    ).not.toBeInTheDocument();
  });
});

describe("Archive context-menu items", () => {
  it("renders 'Archive Workspace' and forwards clicks to onArchiveRequest", async () => {
    const onArchive = vi.fn();
    const ws = makeWorkspace({ worktree_path: null });
    render(
      <WorkspaceContextMenuItems
        workspace={ws}
        onArchiveRequest={onArchive}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();

    await userEvent.click(
      screen.getByRole("menuitem", { name: /Archive Workspace/i }),
    );
    expect(onArchive).toHaveBeenCalledTimes(1);
  });

  it("offers 'Delete Worktree…' only for deletable worktrees", async () => {
    const ws = makeWorkspace({
      worktree_path: "/home/user/.codemux/worktrees/myapp/feature-x",
      protected: false,
    });
    render(
      <WorkspaceContextMenuItems
        workspace={ws}
        onArchiveRequest={() => {}}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();

    expect(
      screen.getByRole("menuitem", { name: /Delete Worktree…/i }),
    ).toBeInTheDocument();
  });

  it("hides 'Delete Worktree…' for the primary workspace and the protected root", async () => {
    const primary = render(
      <WorkspaceContextMenuItems
        workspace={makeWorkspace({ worktree_path: null })}
        onArchiveRequest={() => {}}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();
    expect(
      screen.queryByRole("menuitem", { name: /Delete Worktree…/i }),
    ).not.toBeInTheDocument();
    primary.unmount();
    cleanup();

    render(
      <WorkspaceContextMenuItems
        workspace={makeWorkspace({
          worktree_path: "/home/user/.codemux/worktrees/myapp/root-copy",
          protected: true,
        })}
        onArchiveRequest={() => {}}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();
    expect(
      screen.queryByRole("menuitem", { name: /Delete Worktree…/i }),
    ).not.toBeInTheDocument();
  });
});

describe("Hover-reveal archive button", () => {
  const renderRow = (ws: WorkspaceSnapshot) =>
    render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );

  it("renders for every row — including the primary/protected root", () => {
    renderRow(makeWorkspace({ worktree_path: null, protected: true }));
    expect(
      screen.getByRole("button", { name: /Archive workspace/i }),
    ).toBeInTheDocument();
  });

  it("plain click archives and shows an undoable toast", async () => {
    mockArchiveWorkspace.mockResolvedValueOnce("archive-42");
    renderRow(makeWorkspace({ workspace_id: "ws-arch", title: "My Feature" }));

    await userEvent.click(
      screen.getByRole("button", { name: /Archive workspace/i }),
    );

    expect(mockArchiveWorkspace).toHaveBeenCalledWith("ws-arch");
    await waitFor(() =>
      expect(mockToast.undoable).toHaveBeenCalledWith(
        expect.objectContaining({ message: 'Archived "My Feature"' }),
      ),
    );

    // The toast's Undo closure restores via the archive id the backend
    // returned.
    const { onUndo } = mockToast.undoable.mock.calls[0][0];
    await onUndo();
    expect(mockUnarchiveWorkspace).toHaveBeenCalledWith("archive-42");
  });

  it("surfaces an error toast when archiving fails", async () => {
    mockArchiveWorkspace.mockRejectedValueOnce(
      "This workspace is attached in place on its host — close it instead of archiving.",
    );
    renderRow(makeWorkspace({}));

    await userEvent.click(
      screen.getByRole("button", { name: /Archive workspace/i }),
    );

    await waitFor(() => expect(mockToast.error).toHaveBeenCalledTimes(1));
    expect(mockToast.undoable).not.toHaveBeenCalled();
  });

  it("shift-click on a deletable worktree opens the delete dialog instead of archiving", async () => {
    const user = userEvent.setup();
    renderRow(
      makeWorkspace({
        worktree_path: "/home/user/.codemux/worktrees/myapp/feature-x",
        protected: false,
      }),
    );

    await user.keyboard("{Shift>}");
    await user.click(
      screen.getByRole("button", { name: /Archive workspace/i }),
    );
    await user.keyboard("{/Shift}");

    expect(mockArchiveWorkspace).not.toHaveBeenCalled();
    // Query by heading role — the (inline-mocked) context menu also
    // renders a "Delete Worktree…" item, so plain text would be
    // ambiguous.
    expect(
      await screen.findByRole("heading", { name: /Delete worktree/i }),
    ).toBeInTheDocument();
  });

  it("shift-click on the protected root archives like a plain click", async () => {
    const user = userEvent.setup();
    renderRow(
      makeWorkspace({ workspace_id: "ws-root", worktree_path: null, protected: true }),
    );

    await user.keyboard("{Shift>}");
    await user.click(
      screen.getByRole("button", { name: /Archive workspace/i }),
    );
    await user.keyboard("{/Shift}");

    expect(mockArchiveWorkspace).toHaveBeenCalledWith("ws-root");
  });

  // Safety invariant: no interaction on a protected/primary root row may
  // ever open the destructive delete dialog — shift-click included. The
  // root checkout owns the repo's shared history; its only removal path
  // is the non-destructive archive.
  it("NEVER opens the delete dialog for a protected root — shift-click archives, no destructive call", async () => {
    const user = userEvent.setup();
    renderRow(
      makeWorkspace({
        workspace_id: "ws-root",
        worktree_path: null,
        protected: true,
      }),
    );

    await user.keyboard("{Shift>}");
    await user.click(
      screen.getByRole("button", { name: /Archive workspace/i }),
    );
    await user.keyboard("{/Shift}");

    expect(
      screen.queryByRole("heading", { name: /Delete worktree/i }),
    ).not.toBeInTheDocument();
    expect(mockCloseWorkspaceWithWorktree).not.toHaveBeenCalled();
    expect(mockArchiveWorkspace).toHaveBeenCalledWith("ws-root");
  });
});

describe("Remote / attach-only rows: close instead of archive", () => {
  const renderRow = (ws: WorkspaceSnapshot) =>
    render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );

  it("the hover button on a remote (host-backed) row is 'Close workspace' and detaches non-destructively", async () => {
    renderRow(
      makeWorkspace({
        workspace_id: "ws-remote",
        title: "On host",
        worktree_path: null,
        host_id: 3,
      }),
    );

    await userEvent.click(
      screen.getByRole("button", { name: /Close workspace/i }),
    );

    expect(mockArchiveWorkspace).not.toHaveBeenCalled();
    expect(mockCloseWorkspace).toHaveBeenCalledWith("ws-remote", false);
    await waitFor(() =>
      expect(mockToast.success).toHaveBeenCalledWith(
        'Closed "On host" — it stays available on its host in the Workspaces Overview',
      ),
    );
  });

  it("a remote worktree row closes WITHOUT removing the worktree (removeWorktree=false, no force)", async () => {
    renderRow(
      makeWorkspace({
        workspace_id: "ws-remote-wt",
        worktree_path: "/home/user/.codemux/worktrees/myapp/pushed",
        host_id: 3,
      }),
    );

    await userEvent.click(
      screen.getByRole("button", { name: /Close workspace/i }),
    );

    expect(mockArchiveWorkspace).not.toHaveBeenCalled();
    expect(mockCloseWorkspaceWithWorktree).toHaveBeenCalledWith(
      "ws-remote-wt",
      false,
      false,
      false,
    );
  });

  it("an attach-only row closes instead of archiving", async () => {
    renderRow(
      makeWorkspace({
        workspace_id: "ws-attach",
        worktree_path: null,
        attach_only: true,
      }),
    );

    await userEvent.click(
      screen.getByRole("button", { name: /Close workspace/i }),
    );

    expect(mockArchiveWorkspace).not.toHaveBeenCalled();
    expect(mockCloseWorkspace).toHaveBeenCalledWith("ws-attach", false);
  });

  it("shift-click on a remote worktree row does NOT open the delete dialog", async () => {
    const user = userEvent.setup();
    renderRow(
      makeWorkspace({
        workspace_id: "ws-remote-wt",
        worktree_path: "/home/user/.codemux/worktrees/myapp/pushed",
        host_id: 3,
        protected: false,
      }),
    );

    await user.keyboard("{Shift>}");
    await user.click(
      screen.getByRole("button", { name: /Close workspace/i }),
    );
    await user.keyboard("{/Shift}");

    expect(
      screen.queryByRole("heading", { name: /Delete worktree/i }),
    ).not.toBeInTheDocument();
    // Shift-click behaves like a plain click: the non-destructive close.
    expect(mockCloseWorkspaceWithWorktree).toHaveBeenCalledWith(
      "ws-remote-wt",
      false,
      false,
      false,
    );
  });

  it("the context menu offers 'Close workspace' (no Archive, no Delete Worktree…) for attach/remote rows", async () => {
    const onArchive = vi.fn();
    render(
      <WorkspaceContextMenuItems
        workspace={makeWorkspace({
          worktree_path: "/home/user/.codemux/worktrees/myapp/pushed",
          host_id: 3,
        })}
        onArchiveRequest={onArchive}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();

    expect(
      screen.queryByRole("menuitem", { name: /Archive Workspace/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("menuitem", { name: /Delete Worktree…/i }),
    ).not.toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("menuitem", { name: /^Close workspace$/i }),
    );
    expect(onArchive).toHaveBeenCalledTimes(1);
  });
});

describe("Delete worktree dialog escalation", () => {
  const worktreeWs = () =>
    makeWorkspace({
      workspace_id: "ws-del",
      title: "Doomed",
      worktree_path: "/home/user/.codemux/worktrees/myapp/doomed",
      protected: false,
    });

  const openDialog = async (user: ReturnType<typeof userEvent.setup>) => {
    render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={worktreeWs()} isActive={false} />
      </TooltipProvider>,
    );
    await user.keyboard("{Shift>}");
    await user.click(
      screen.getByRole("button", { name: /Archive workspace/i }),
    );
    await user.keyboard("{/Shift}");
    await screen.findByRole("heading", { name: /Delete worktree/i });
  };

  it("deletes with forceDelete=false first and keeps the branch checkbox default (true)", async () => {
    const user = userEvent.setup();
    await openDialog(user);

    await user.click(screen.getByRole("button", { name: /^Delete$/i }));

    await waitFor(() =>
      expect(mockCloseWorkspaceWithWorktree).toHaveBeenCalledWith(
        "ws-del",
        true,
        true,
        false,
      ),
    );
  });

  it("escalates to 'Force delete' on a /use force/i rejection, showing the backend message verbatim", async () => {
    const dirtyMessage =
      "Worktree has 3 uncommitted change(s). Use force to override.";
    mockCloseWorkspaceWithWorktree.mockRejectedValueOnce(dirtyMessage);
    const user = userEvent.setup();
    await openDialog(user);

    await user.click(screen.getByRole("button", { name: /^Delete$/i }));

    // Dialog stays open in the escalated state.
    expect(await screen.findByText(dirtyMessage)).toBeInTheDocument();
    expect(mockToast.error).not.toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", { name: /Force delete/i }),
    );
    await waitFor(() =>
      expect(mockCloseWorkspaceWithWorktree).toHaveBeenLastCalledWith(
        "ws-del",
        true,
        true,
        true,
      ),
    );
  });

  it("closes and toasts on any other rejection", async () => {
    mockCloseWorkspaceWithWorktree.mockRejectedValueOnce(
      "worktree path is locked by another process",
    );
    const user = userEvent.setup();
    await openDialog(user);

    await user.click(screen.getByRole("button", { name: /^Delete$/i }));

    await waitFor(() => expect(mockToast.error).toHaveBeenCalledTimes(1));
    expect(
      screen.queryByRole("button", { name: /Force delete/i }),
    ).not.toBeInTheDocument();
  });
});

describe("Workspace row detail level (Settings → Appearance → Sidebar)", () => {
  const setDetail = (level: "clean" | "branch" | "detailed") =>
    act(() => {
      useSettingsStore.setState({
        settings: { "sidebar.workspace_detail": level },
      });
    });

  // The store is process-global; restore the default after each case so
  // the rest of the suite (which relies on "detailed") is unaffected.
  beforeEach(() => setDetail("detailed"));

  const wsWithStats = () =>
    makeWorkspace({
      worktree_path: null,
      git_branch: "feature/x",
      git_ahead: 2,
      git_additions: 5,
    });

  it("'detailed' shows the branch name and the git stats", () => {
    setDetail("detailed");
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={wsWithStats()} isActive={false} />
      </TooltipProvider>,
    );
    expect(container.textContent).toContain("feature/x");
    expect(container.textContent).toContain("↑2");
    expect(container.textContent).toContain("+5");
  });

  it("'branch' shows the branch name but hides the git stats", () => {
    setDetail("branch");
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={wsWithStats()} isActive={false} />
      </TooltipProvider>,
    );
    expect(container.textContent).toContain("feature/x");
    expect(container.textContent).not.toContain("↑2");
    expect(container.textContent).not.toContain("+5");
  });

  it("'clean' hides the whole metadata line (no branch, no stats)", () => {
    setDetail("clean");
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={wsWithStats()} isActive={false} />
      </TooltipProvider>,
    );
    // Title (line 1) still renders…
    expect(container.textContent).toContain("Test Workspace");
    // …but the metadata line (branch + stats) is gone.
    expect(container.textContent).not.toContain("feature/x");
    expect(container.textContent).not.toContain("↑2");
    expect(container.textContent).not.toContain("+5");
  });
});
