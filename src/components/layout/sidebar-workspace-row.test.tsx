/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  WorkspaceSnapshot,
  PaneStatus,
  SurfaceSnapshot,
} from "@/tauri/types";

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
  mockSetWorkspacePinned,
  mockSetWorkspaceMuted,
  mockToast,
} = vi.hoisted(() => ({
  mockArchiveWorkspace: vi.fn(),
  mockUnarchiveWorkspace: vi.fn(),
  mockCheckoutDefault: vi.fn(),
  mockCloseWorkspace: vi.fn(),
  mockCloseWorkspaceWithWorktree: vi.fn(),
  mockGetDefaultBranch: vi.fn().mockResolvedValue("main"),
  mockSetWorkspacePinned: vi.fn().mockResolvedValue(undefined),
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
  setWorkspacePinned: (...args: unknown[]) => mockSetWorkspacePinned(...args),
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
    ...rest
  }: {
    children?: React.ReactNode;
    onClick?: () => void;
    disabled?: boolean;
  } & React.HTMLAttributes<HTMLButtonElement>) => (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      disabled={disabled}
      data-disabled={disabled ? "true" : undefined}
      {...rest}
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
    // Section labels are chrome, not menuitems — the role-based queries in
    // this suite must not see them.
    ContextMenuLabel: ({ children }: { children?: React.ReactNode }) => (
      <div>{children}</div>
    ),
    ContextMenuGroup: passthrough,
    ContextMenuSeparator: () => <hr />,
    ContextMenuSub: passthrough,
    ContextMenuSubTrigger: passthrough,
    ContextMenuSubContent: passthrough,
  };
});

// HoverCard uses a Radix Portal + pointer-driven open that jsdom doesn't
// exercise cleanly. Render trigger + content inline so the "n shipped" tally
// and its popover list are directly assertable (same pattern as the
// context-menu mock above).
vi.mock("@/components/ui/hover-card", () => {
  const passthrough = ({ children }: { children?: React.ReactNode }) => (
    <>{children}</>
  );
  return {
    HoverCard: passthrough,
    HoverCardTrigger: passthrough,
    HoverCardContent: ({ children }: { children?: React.ReactNode }) => (
      <div>{children}</div>
    ),
  };
});

// App store — the row derives `workspaceStatus` from `appState.pane_statuses`.
// A hoisted holder lets individual tests inject a snapshot (with pane
// statuses) to drive the working / permission / review density states.
const { appStateHolder } = vi.hoisted(() => ({
  appStateHolder: { current: null as unknown },
}));
vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) =>
    selector({
      appState: appStateHolder.current,
      setWorkspacePushPullInFlight: vi.fn(),
      workspacePushPullInFlight: null,
      workspacePushPullStartedAt: null,
    }),
  ),
}));

// Late imports so the mocks above apply.
import {
  SidebarWorkspaceRow,
  WorkspaceContextMenuItems,
} from "./sidebar-workspace-row";
import { __resetDefaultBranchCacheForTests } from "./sidebar-workspace-row.test-utils";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";

/** Build a single-terminal-pane surface carrying a known pane id. */
function surfaceWithPane(paneId: string): SurfaceSnapshot {
  return {
    surface_id: "sf-1",
    title: "",
    root: { kind: "terminal", pane_id: paneId, session_id: "sess-1", title: "" },
    active_pane_id: paneId,
  };
}

/** Point the mocked app store at a snapshot whose single pane carries the
 *  given agent status, so the row derives that density state. */
function setPaneStatus(paneId: string, status: PaneStatus) {
  appStateHolder.current = {
    active_workspace_id: "",
    pane_statuses: { [paneId]: status } as Record<string, PaneStatus>,
  };
}

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
  // Default every row to idle (no agent status). Density tests set a pane
  // status explicitly. Reset the non-persisted density store so seen /
  // settled timestamps never leak between cases.
  appStateHolder.current = null;
  useSidebarDensityStore.setState({
    statusSince: {},
    settledAt: {},
    lastSeenAt: {},
    workHistory: {},
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
  mockSetWorkspacePinned.mockReset();
  mockSetWorkspacePinned.mockResolvedValue(undefined);
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
    // Scoped to the row: the workspace context menu also carries a BellOff
    // (its mute entry), and the mock renders that menu inline.
    expect(
      muted.container.querySelector('[role="button"] svg.lucide-bell-off'),
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
      unmuted.container.querySelector('[role="button"] svg.lucide-bell-off'),
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

describe("State-driven row density", () => {
  const renderRow = (ws: WorkspaceSnapshot, isActive = false) =>
    render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={isActive} />
      </TooltipProvider>,
    );

  const wsWithStats = (overrides: Partial<WorkspaceSnapshot> = {}) =>
    makeWorkspace({
      worktree_path: null,
      git_branch: "feature/x",
      git_ahead: 2,
      git_additions: 5,
      ...overrides,
    });

  it("idle + clean: renders a one-liner (title only, no git line for a bare branch)", () => {
    const { container } = renderRow(
      makeWorkspace({ worktree_path: null, git_branch: "feature/x" }),
    );
    expect(container.textContent).toContain("Test Workspace");
    // A bare branch alone does not expand a calm idle row.
    expect(container.textContent).not.toContain("feature/x");
  });

  it("idle + dirty worktree: shows the mono git line even with no agent", () => {
    const { container } = renderRow(wsWithStats({ git_changed_files: 3 }));
    expect(container.textContent).toContain("feature/x");
    expect(container.textContent).toContain("↑2");
    expect(container.textContent).toContain("+5");
  });

  it("idle + ahead/behind: surfaces the git line", () => {
    const { container } = renderRow(
      makeWorkspace({
        worktree_path: null,
        git_branch: "feature/x",
        git_ahead: 2,
      }),
    );
    expect(container.textContent).toContain("feature/x");
    expect(container.textContent).toContain("↑2");
  });

  it("working: 3-line card — spinner glyph, activity fallback, and git line", () => {
    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "working", at: Date.now() - 6 * 60_000 } },
      settledAt: {},
      lastSeenAt: {},
    });
    setPaneStatus("p-work", "working");
    const { container } = renderRow(
      wsWithStats({ surfaces: [surfaceWithPane("p-work")] }),
    );
    // The braille spinner replaces the leading icon (agent-working glyph).
    expect(
      container.querySelector("[aria-label='Agent working']"),
    ).not.toBeNull();
    // Activity fallback line with a coarse elapsed.
    expect(container.textContent).toMatch(/Working · 6m/);
    // Git line (line 3).
    expect(container.textContent).toContain("feature/x");
    expect(container.textContent).toContain("+5");
  });

  it("permission: 2-line 'needs you' card with the blocker fallback + red dot", () => {
    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "permission", at: Date.now() } },
      settledAt: {},
      lastSeenAt: {},
    });
    setPaneStatus("p-perm", "permission");
    const { container } = renderRow(
      makeWorkspace({
        worktree_path: null,
        surfaces: [surfaceWithPane("p-perm")],
      }),
    );
    expect(container.textContent).toMatch(/Waiting for your input/);
    // Red corner dot (StatusIndicator, permission tone).
    expect(container.querySelector(".bg-status-attention")).not.toBeNull();
  });

  it("review (unseen): 2-line done card; 'PR opened' when a PR is open", () => {
    const now = Date.now();
    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "review", at: now } },
      settledAt: { "ws-1": now },
      lastSeenAt: {},
    });
    setPaneStatus("p-rev", "review");
    const { container } = renderRow(
      makeWorkspace({
        worktree_path: null,
        pr_state: "OPEN",
        surfaces: [surfaceWithPane("p-rev")],
      }),
    );
    expect(container.textContent).toContain("Done");
    expect(container.textContent).toContain("PR opened");
    expect(container.textContent).toContain("review when ready");
  });

  it("review collapses to a one-liner once the workspace has been seen", () => {
    const now = Date.now();
    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "review", at: now } },
      settledAt: { "ws-1": now - 1000 },
      lastSeenAt: { "ws-1": now }, // seen after it settled
    });
    setPaneStatus("p-rev", "review");
    const { container } = renderRow(
      makeWorkspace({
        worktree_path: null,
        surfaces: [surfaceWithPane("p-rev")],
      }),
    );
    expect(container.textContent).not.toContain("review when ready");
  });

  it("re-marks an active workspace seen when it settles into review while active, collapsing the Done card", () => {
    // Regression for the "active row that settles never auto-collapses" bug:
    // markSeen only fired on the isActive false→true edge, so a workspace that
    // finished while it was ALREADY the active one kept its 2-line Done card
    // (and LIVE membership) until navigation or the ~1h fade.
    vi.useFakeTimers();
    try {
      vi.setSystemTime(1_000_000);
      const ws = makeWorkspace({
        worktree_path: null,
        surfaces: [surfaceWithPane("p-rev")],
      });
      // Active + working: the user is looking at it while the agent runs.
      useSidebarDensityStore.setState({
        statusSince: { "ws-1": { status: "working", at: 1_000_000 } },
        settledAt: {},
        lastSeenAt: {},
      });
      setPaneStatus("p-rev", "working");
      const utils = render(
        <TooltipProvider>
          <SidebarWorkspaceRow workspace={ws} isActive />
        </TooltipProvider>,
      );
      // Opening it stamped lastSeenAt at mount; no settle yet.
      expect(
        useSidebarDensityStore.getState().lastSeenAt["ws-1"],
      ).toBe(1_000_000);

      // The agent finishes later, while the workspace is still the active one.
      vi.setSystemTime(1_005_000);
      setPaneStatus("p-rev", "review");
      act(() => {
        utils.rerender(
          <TooltipProvider>
            <SidebarWorkspaceRow workspace={ws} isActive />
          </TooltipProvider>,
        );
      });

      // The row's observeStatus stamped the settle; the markSeen effect re-ran
      // (settledAt advanced while active) and re-stamped lastSeenAt to the
      // settle time — so the Done card collapses at once.
      const st = useSidebarDensityStore.getState();
      expect(st.settledAt["ws-1"]).toBe(1_005_000);
      expect(st.lastSeenAt["ws-1"]).toBeGreaterThanOrEqual(st.settledAt["ws-1"]!);
      expect(utils.container.textContent).not.toContain("review when ready");
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows a fading green ✓ on a fresh settle; it is gone after the ~1h window", () => {
    const now = Date.now();
    const ws = makeWorkspace({
      worktree_path: null,
      surfaces: [surfaceWithPane("p-rev")],
    });
    setPaneStatus("p-rev", "review");

    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "review", at: now } },
      settledAt: { "ws-1": now },
      lastSeenAt: {},
    });
    const fresh = renderRow(ws);
    expect(fresh.getByLabelText("Recently finished")).toBeInTheDocument();
    cleanup();

    // Settled >1h ago: the ✓ has faded out and the row is a plain one-liner.
    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "review", at: now } },
      settledAt: { "ws-1": now - 2 * 60 * 60_000 },
      lastSeenAt: {},
    });
    const stale = renderRow(ws);
    expect(stale.queryByLabelText("Recently finished")).not.toBeInTheDocument();
    expect(stale.container.textContent).not.toContain("review when ready");
  });

  it("is purely state-driven — no removed sidebar.workspace_detail setting", () => {
    // A dirty idle row shows its git stats regardless of any (now-absent)
    // appearance setting.
    const { container } = renderRow(wsWithStats({ git_changed_files: 1 }));
    expect(container.textContent).toContain("feature/x");
    expect(container.textContent).toContain("↑2");
    expect(container.textContent).toContain("+5");
  });
});

describe("Living row — work titles + shipped tally", () => {
  const renderRow = (ws: WorkspaceSnapshot, isActive = false) =>
    render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={isActive} />
      </TooltipProvider>,
    );

  const issue = (number: number, title: string) => ({
    number,
    title,
    state: "Open" as const,
    labels: [],
  });

  it("a live row's title IS the linked-issue work title, not the worktree name", () => {
    const now = Date.now();
    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "review", at: now } },
      settledAt: { "ws-1": now },
      lastSeenAt: {},
      workHistory: {},
    });
    setPaneStatus("p-rev", "review");
    const { container } = renderRow(
      makeWorkspace({
        title: "auth-refactor", // worktree name
        worktree_path: "/home/user/.codemux/worktrees/myapp/auth",
        git_branch: "feature/auth",
        linked_issue: issue(50, "Scroll pinning broken on send"),
        surfaces: [surfaceWithPane("p-rev")],
      }),
    );
    // Scoped to the row itself — the context menu's identity header names the
    // workspace on purpose, and the mock renders that menu inline.
    const row = container.querySelector('[role="button"]')!;
    expect(row.textContent).toContain("Scroll pinning broken on send");
    // The worktree name is not the title anymore (it lives in the tooltip).
    expect(row.textContent).not.toContain("auth-refactor");
  });

  it("falls back to the workspace title when a live row has no linked issue", () => {
    const now = Date.now();
    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "review", at: now } },
      settledAt: { "ws-1": now },
      lastSeenAt: {},
      workHistory: {},
    });
    setPaneStatus("p-rev", "review");
    const { container } = renderRow(
      makeWorkspace({
        title: "orphan-worktree",
        worktree_path: "/home/user/.codemux/worktrees/myapp/orphan",
        linked_issue: null,
        surfaces: [surfaceWithPane("p-rev")],
      }),
    );
    expect(container.textContent).toContain("orphan-worktree");
  });

  it("an idle row keeps the worktree name even with a linked issue", () => {
    const { container } = renderRow(
      makeWorkspace({
        title: "auth-refactor",
        worktree_path: "/home/user/.codemux/worktrees/myapp/auth",
        git_branch: "feature/auth",
        linked_issue: issue(50, "Scroll pinning broken on send"),
      }),
    );
    // Worktree name stays as the title; the issue title is NOT surfaced (only
    // its #chip is, down in the git line).
    expect(container.textContent).toContain("auth-refactor");
    expect(container.textContent).not.toContain(
      "Scroll pinning broken on send",
    );
  });

  it("retires the merged-PR icon and shows an 'n shipped' tally when new work is linked", async () => {
    const base = makeWorkspace({
      workspace_id: "ws-ship",
      title: "auth-refactor",
      worktree_path: "/home/user/.codemux/worktrees/myapp/auth",
      git_branch: "feature/auth",
      pr_number: 128,
      pr_state: "merged",
      linked_issue: issue(50, "Work A title"),
    });

    const { container, rerender } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={base} isActive={false} />
      </TooltipProvider>,
    );

    // Before new work: a merged PR shows the violet merge icon, no tally.
    expect(container.querySelector("svg.lucide-git-merge")).not.toBeNull();
    expect(
      screen.queryByRole("button", { name: /shipped/i }),
    ).not.toBeInTheDocument();

    // New work starts in the same workspace → different linked issue.
    rerender(
      <TooltipProvider>
        <SidebarWorkspaceRow
          workspace={{ ...base, linked_issue: issue(60, "Work B title") }}
          isActive={false}
        />
      </TooltipProvider>,
    );

    // The retired merge falls back to the plain branch icon…
    await waitFor(() =>
      expect(container.querySelector("svg.lucide-git-branch")).not.toBeNull(),
    );
    expect(container.querySelector("svg.lucide-git-merge")).toBeNull();

    // …and a "1 shipped" tally appears, whose popover lists the shipped work.
    expect(
      screen.getByRole("button", { name: /^1 shipped$/ }),
    ).toBeInTheDocument();
    expect(screen.getByText("#50")).toBeInTheDocument();
    expect(screen.getByText("Work A title")).toBeInTheDocument();
  });

  it("baseline-promotion: merged PR + issue A present at mount, issue → B ships 1", async () => {
    const base = makeWorkspace({
      workspace_id: "ws-ship",
      title: "auth-refactor",
      worktree_path: "/home/user/.codemux/worktrees/myapp/auth",
      pr_number: 200,
      pr_state: "merged",
      linked_issue: issue(11, "First shipped work"),
    });
    const { rerender } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={base} isActive={false} />
      </TooltipProvider>,
    );
    rerender(
      <TooltipProvider>
        <SidebarWorkspaceRow
          workspace={{ ...base, linked_issue: issue(22, "Second work") }}
          isActive={false}
        />
      </TooltipProvider>,
    );

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /^1 shipped$/ }),
      ).toBeInTheDocument(),
    );
    expect(
      useSidebarDensityStore.getState().workHistory["ws-ship"]?.shipped,
    ).toEqual([{ prNumber: 200, issueNumber: 11, title: "First shipped work" }]);
  });
});

describe("Workspace pin and lifecycle context-menu block", () => {
  it("pins and unpins through an idempotent workspace command", async () => {
    const { rerender } = render(
      <WorkspaceContextMenuItems
        workspace={makeWorkspace()}
        onArchiveRequest={() => {}}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();

    await userEvent.click(screen.getByRole("menuitem", { name: "Pin workspace" }));
    expect(mockSetWorkspacePinned).toHaveBeenCalledWith("ws-1", true);

    rerender(
      <WorkspaceContextMenuItems
        workspace={makeWorkspace({ pinned_at: Date.now() })}
        onArchiveRequest={() => {}}
        onDeleteRequest={() => {}}
      />,
    );
    await userEvent.click(screen.getByRole("menuitem", { name: "Unpin workspace" }));
    expect(mockSetWorkspacePinned).toHaveBeenCalledWith("ws-1", false);
  });

  it("hides settle and snooze actions while pinned", async () => {
    render(
      <WorkspaceContextMenuItems
        workspace={makeWorkspace({ pinned_at: Date.now() })}
        settleAction={{ kind: "settle", onAction: () => {} }}
        snoozeAction={{
          kind: "snooze",
          offered: true,
          onSnooze: () => {},
          onWake: () => {},
        }}
        unreadAction={{ onMarkUnread: () => {} }}
        onArchiveRequest={() => {}}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();

    expect(screen.getByRole("menuitem", { name: "Unpin workspace" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Settle workspace" })).not.toBeInTheDocument();
    expect(screen.queryByText("Snooze")).not.toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Mark unread" })).toBeInTheDocument();
  });

  it("puts the bring-it-back entries above the deferral ones", async () => {
    render(
      <WorkspaceContextMenuItems
        workspace={makeWorkspace({ worktree_path: null })}
        settleAction={{ kind: "unsettle", onAction: () => {} }}
        snoozeAction={{
          kind: "wake",
          offered: false,
          onSnooze: () => {},
          onWake: () => {},
        }}
        unreadAction={{ onMarkUnread: () => {} }}
        onArchiveRequest={() => {}}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();

    // Compared by identity against name-based lookups rather than by
    // textContent: rows now also carry a decorative keycap, which is not part
    // of what the entry is called.
    expect(screen.getAllByRole("menuitem").slice(0, 4)).toEqual([
      screen.getByRole("menuitem", { name: "Pin workspace" }),
      screen.getByRole("menuitem", { name: "Un-settle workspace" }),
      screen.getByRole("menuitem", { name: "Wake now" }),
      screen.getByRole("menuitem", { name: "Mark unread" }),
    ]);
  });

  it("resolves its own wake times rather than taking them from the caller", async () => {
    const onSnooze = vi.fn();
    const mountedAt = Date.now();
    render(
      <WorkspaceContextMenuItems
        workspace={makeWorkspace({ worktree_path: null })}
        settleAction={{ kind: "settle", onAction: () => {} }}
        snoozeAction={{
          kind: "snooze",
          offered: true,
          onSnooze,
          onWake: () => {},
        }}
        onArchiveRequest={() => {}}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();

    await userEvent.click(screen.getByText("Snooze until…"));
    await userEvent.click(await screen.findByText("In 1 hour"));

    // Nothing upstream hands this menu a clock any more: the wake time is an
    // hour past the moment the menu itself came into existence.
    expect(onSnooze).toHaveBeenCalledTimes(1);
    expect(onSnooze.mock.calls[0][0]).toBeGreaterThanOrEqual(
      mountedAt + 3_600_000,
    );
  });

  it("names the concrete wake time beside each relative label", async () => {
    // Pinned to a Wednesday: on Sundays the "Next week" preset is
    // deliberately withheld (it would duplicate "Tomorrow"), and this test
    // is about the label, not that rule.
    const nowSpy = vi
      .spyOn(Date, "now")
      .mockReturnValue(new Date(2026, 5, 10, 12, 0, 0).getTime());
    try {
      render(
        <WorkspaceContextMenuItems
          workspace={makeWorkspace({ worktree_path: null })}
          snoozeAction={{
            kind: "snooze",
            offered: true,
            onSnooze: () => {},
            onWake: () => {},
          }}
          onArchiveRequest={() => {}}
          onDeleteRequest={() => {}}
        />,
      );
      await flushDefaultBranchFetch();

      await userEvent.click(screen.getByText("Snooze until…"));
      const nextWeek = await screen.findByText("Next week");
      // The row the user actually reads: "Next week" plus the Monday it
      // lands on.
      expect(nextWeek.closest('[role="menuitem"]')?.textContent).toMatch(
        /Next week.+0?9[:.]00/,
      );
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("renders no Snooze submenu when the caller's guardrail withholds it", async () => {
    render(
      <WorkspaceContextMenuItems
        workspace={makeWorkspace({ worktree_path: null })}
        snoozeAction={{
          kind: "snooze",
          offered: false,
          onSnooze: () => {},
          onWake: () => {},
        }}
        onArchiveRequest={() => {}}
        onDeleteRequest={() => {}}
      />,
    );
    await flushDefaultBranchFetch();

    expect(screen.queryByText("Snooze until…")).not.toBeInTheDocument();
  });
});
