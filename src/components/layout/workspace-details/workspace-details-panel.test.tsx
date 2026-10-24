/// <reference types="@testing-library/jest-dom/vitest" />
import { useSyncExternalStore } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { WorkspaceSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  workspace: null as WorkspaceSnapshot | null,
  openEditor: vi.fn(),
  editorsAvailable: true,
  hasUpstream: true,
  gitPushChanges: vi.fn().mockResolvedValue(undefined),
  gitPullChanges: vi.fn().mockResolvedValue(undefined),
  gitFetchChanges: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/stores/app-store", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/stores/app-store")>()),
  useActiveWorkspace: () => mocks.workspace,
  useHomeDir: () => "/home/user",
}));
vi.mock("@/stores/hosts-store", () => ({ useHosts: () => [] }));
vi.mock("@/stores/local-device-store", () => ({
  useLocalDeviceName: () => "ai-node",
}));
vi.mock("@/hooks/use-editor-launcher", () => ({
  useEditorLauncher: () => ({
    editors: [{ id: "cursor", name: "Cursor", command: "cursor" }],
    defaultEditor: { id: "cursor", name: "Cursor", command: "cursor" },
    defaultEditorId: "cursor",
    workspacePath: "/p",
    isLoading: false,
    open: mocks.openEditor,
    unavailable: !mocks.editorsAvailable,
  }),
}));
vi.mock("../run-button", () => ({
  RunButton: ({ variant }: { variant?: string }) => (
    <div data-testid="run-row" data-variant={variant} />
  ),
}));
vi.mock("@/tauri/commands", () => ({
  getGitBranchInfo: vi.fn(async () => ({
    branch: "feat/x",
    ahead: 0,
    behind: 0,
    has_upstream: mocks.hasUpstream,
  })),
  gitPushChanges: mocks.gitPushChanges,
  gitPullChanges: mocks.gitPullChanges,
  gitFetchChanges: mocks.gitFetchChanges,
  initGitRepoNoCommit: vi.fn(),
  getGithubIssue: vi.fn(() => new Promise(() => {})),
  undockBrowserFromRightPanel: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), failure: vi.fn(), info: vi.fn() },
  reportFailure: () => () => {},
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { useAppStore } from "@/stores/app-store";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import { useUIStore } from "@/stores/ui-store";
import { registry } from "@/contrib/registry";
import { COMPOSER_STATUS_AREA } from "@/sdk/areas";
import type { ComposerStatusData } from "@/sdk/host";
import type { AgentBrowserSession, AppStateSnapshot } from "@/tauri/types";
import {
  WorkspaceDetailsHost,
  WorkspaceDetailsToggle,
  primaryGitAction,
  resolveDetailsCardPlacement,
} from "./workspace-details-panel";

const CHAT_SURFACE: WorkspaceSnapshot["surfaces"][number] = {
  surface_id: "surface-1",
  title: "Agent Chat",
  active_pane_id: "pane-1",
  root: {
    kind: "agent_chat",
    pane_id: "pane-1",
    title: "Agent Chat",
    thread_id: null,
    provider: null,
    cwd: "/p",
  },
};

const CHAT_TAB: WorkspaceSnapshot["tabs"][number] = {
  tab_id: "tab-1",
  kind: "terminal",
  title: "Agent Chat",
  surface_id: "surface-1",
  browser_id: null,
  icon: null,
};

/** Editor and diff tabs have no surface of their own. */
const EDITOR_TAB: WorkspaceSnapshot["tabs"][number] = {
  tab_id: "tab-editor",
  kind: "editor",
  title: "main.rs",
  surface_id: null,
  browser_id: null,
  icon: null,
};

const TERMINAL_SURFACE: WorkspaceSnapshot["surfaces"][number] = {
  surface_id: "surface-1",
  title: "Terminal",
  active_pane_id: "pane-1",
  root: { kind: "terminal", pane_id: "pane-1", session_id: "s-1", title: "zsh" },
};

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "demo",
    workspace_type: "standard",
    cwd: "/p",
    git_branch: "feat/x",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 12,
    git_deletions: 3,
    git_changed_files: 2,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: "/p/.worktrees/feat-x",
    project_root: "/p",
    pr_number: 172,
    pr_state: "open",
    pr_url: "https://github.com/example/repo/pull/172",
    linked_issue: null,
    notifications_muted: false,
    tabs: [CHAT_TAB, EDITOR_TAB],
    active_tab_id: "tab-1",
    active_surface_id: "surface-1",
    surfaces: [CHAT_SURFACE],
    ...overrides,
  };
}

/** Toggle + host as the title bar mounts them. 1600px leaves room to pin
 *  the card beside the 760px chat column; 900px does not. */
function renderToggle({ columnWidth = 1600 }: { columnWidth?: number } = {}) {
  return render(
    <TooltipProvider delayDuration={0}>
      <WorkspaceDetailsToggle />
      <WorkspaceDetailsHost
        workspaceId="ws-1"
        columnWidth={columnWidth}
        rightInset={0}
        edgeGap={4}
        remPx={16}
      />
    </TooltipProvider>,
  );
}

const flush = () => act(() => new Promise((r) => setTimeout(r, 0)));

beforeEach(() => {
  mocks.workspace = makeWorkspace();
  mocks.editorsAvailable = true;
  mocks.hasUpstream = true;
  mocks.openEditor.mockClear();
  mocks.gitPushChanges.mockClear();
  mocks.gitPullChanges.mockClear();
  mocks.gitFetchChanges.mockClear();
  useUIStore.setState({
    workspaceDetailsOpen: true,
    workspaceDetailsMode: "pinned",
    workspaceDetailsPopoverOpen: false,
    rightPanelTabs: {},
  });
});

const contributionDisposers: (() => void)[] = [];
afterEach(() => {
  cleanup();
  for (const dispose of contributionDisposers.splice(0)) dispose();
});

it("shows plugin contributions in Live without a browser and follows the focused chat", () => {
  const renderStatus = vi.fn(({ workspaceId, threadId }: { workspaceId: string; threadId: string | null }) =>
    <button>{workspaceId}:{threadId}</button>,
  );
  mocks.workspace = makeWorkspace({ surfaces: [{ ...CHAT_SURFACE, root: {
    kind: "agent_chat", pane_id: "pane-1", title: "Agent Chat",
    thread_id: "thread-live", provider: null, cwd: "/p",
  } }] });
  const view = renderToggle();
  expect(screen.queryByTestId("workspace-details-live")).toBeNull();
  let dispose!: () => void;
  act(() => {
    dispose = registry.register({ id: "demo:status", source: "plugin:demo", area: COMPOSER_STATUS_AREA, render: renderStatus });
    contributionDisposers.push(dispose);
  });
  const live = screen.getByTestId("workspace-details-live");
  expect(within(live).getByRole("button", { name: "ws-1:thread-live" })).toBeInTheDocument();
  expect(renderStatus).toHaveBeenLastCalledWith(expect.objectContaining({ workspaceId: "ws-1", threadId: "thread-live" }), undefined);
  mocks.workspace = makeWorkspace();
  view.rerender(<TooltipProvider><WorkspaceDetailsHost workspaceId="ws-1" columnWidth={1600} rightInset={0} edgeGap={4} remPx={16} /></TooltipProvider>);
  expect(within(screen.getByTestId("workspace-details-live")).getByRole("button", { name: "ws-1:" })).toBeInTheDocument();
  act(dispose);
  expect(screen.queryByTestId("workspace-details-live")).toBeNull();
});

it("leaves Live out while every plugin chip is idle and shows it once one turns active", () => {
  // A chip that renders nothing until a session starts, as computer use does.
  let active = false;
  const listeners = new Set<() => void>();
  const status = {
    subscribe: (fn: () => void) => {
      listeners.add(fn);
      return () => { listeners.delete(fn); };
    },
    isVisible: () => active,
  } satisfies ComposerStatusData;
  const Chip = () => {
    const shown = useSyncExternalStore(status.subscribe, () => active);
    return shown ? <button>Computer</button> : null;
  };
  contributionDisposers.push(registry.register({
    id: "demo:status", source: "plugin:demo", area: COMPOSER_STATUS_AREA, render: Chip, data: status,
  }));
  renderToggle();
  expect(screen.queryByTestId("workspace-details-live")).toBeNull();
  expect(screen.queryByText("Live")).toBeNull();

  act(() => {
    active = true;
    for (const fn of listeners) fn();
  });
  const live = screen.getByTestId("workspace-details-live");
  expect(within(live).getByText("Live")).toBeInTheDocument();
  expect(within(live).getByRole("button", { name: "Computer" })).toBeInTheDocument();

  act(() => {
    active = false;
    for (const fn of listeners) fn();
  });
  expect(screen.queryByTestId("workspace-details-live")).toBeNull();
});

describe("primaryGitAction", () => {
  const base = { changedFiles: 0, ahead: 0, behind: 0, hasUpstream: true };

  it("commits first while the tree has changes", () => {
    expect(primaryGitAction({ ...base, changedFiles: 3, ahead: 2 })).toEqual({
      kind: "commit",
    });
  });

  it("syncs, pushes or pulls on a clean tree", () => {
    expect(primaryGitAction({ ...base, ahead: 2, behind: 1 }).kind).toBe("sync");
    expect(primaryGitAction({ ...base, ahead: 2 })).toEqual({ kind: "push", label: "Push 2" });
    expect(primaryGitAction({ ...base, behind: 4 })).toEqual({ kind: "pull", label: "Pull 4" });
  });

  it("publishes a branch with no upstream, and fetches otherwise", () => {
    expect(primaryGitAction({ ...base, hasUpstream: false }).kind).toBe("publish");
    expect(primaryGitAction({ ...base, hasUpstream: null }).kind).toBe("fetch");
    expect(primaryGitAction(base).kind).toBe("fetch");
  });
});

describe("resolveDetailsCardPlacement", () => {
  const base = { edgeGap: 4, viewportHeight: 900, remPx: 16, soleChat: true };

  it("pins while the card fits beside the centered chat column", () => {
    // 760px chat centered in 1400px ends at 1080; the card starts at 1116.
    expect(resolveDetailsCardPlacement({ ...base, columnWidth: 1400 })).toEqual({
      top: 44,
      width: 280,
      maxHeight: 900 - 44 - 12,
    });
  });

  it("scales the card and its corner with the interface size", () => {
    // 22px root: 47.5rem = 1045px chat, 17.5rem = 385px card.
    expect(
      resolveDetailsCardPlacement({ ...base, remPx: 22, columnWidth: 1900 }),
    ).toEqual({ top: 60.5, width: 385, maxHeight: 900 - 60.5 - 16.5 });
    expect(resolveDetailsCardPlacement({ ...base, remPx: 22, columnWidth: 1400 })).toBeNull();
  });

  it("never pins over a surface that is not a single chat pane", () => {
    expect(
      resolveDetailsCardPlacement({ ...base, columnWidth: 2400, soleChat: false }),
    ).toBeNull();
  });

  it("falls back to the popover when the card would crowd the chat", () => {
    expect(resolveDetailsCardPlacement({ ...base, columnWidth: 1300 })).toBeNull();
  });

  it("falls back to the popover in a window too short for the card", () => {
    expect(
      resolveDetailsCardPlacement({ ...base, columnWidth: 1600, viewportHeight: 200 }),
    ).toBeNull();
  });
});

describe("WorkspaceDetailsToggle — pinned", () => {
  it("is on by default, pinned in the column's top-right corner", () => {
    renderToggle();
    const card = screen.getByTestId("workspace-details-card");
    expect(card).toHaveAttribute("data-mode", "pinned");
    expect(card.style.right).toBe("4px");
    expect(card.style.top).toBe("44px");
    expect(screen.getByRole("button", { name: "Workspace details" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  it("closes from the toggle, and that choice is what persists", async () => {
    renderToggle();
    const toggle = screen.getByRole("button", { name: "Workspace details" });
    await userEvent.click(toggle);
    expect(screen.queryByTestId("workspace-details-card")).toBeNull();
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(useUIStore.getState().workspaceDetailsOpen).toBe(false);
    await userEvent.click(toggle);
    expect(screen.getByTestId("workspace-details-card")).toBeInTheDocument();
  });

  it("stays open when the user works elsewhere", () => {
    renderToggle();
    fireEvent.pointerDown(document.body);
    fireEvent.focusIn(document.body);
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(screen.getByTestId("workspace-details-card")).toBeInTheDocument();
  });

  it("lists where it runs, the editor, run, branch, PR, git action and changes", async () => {
    renderToggle();
    await flush();
    const card = screen.getByTestId("workspace-details-card");
    expect(card).toHaveTextContent("ai-node");
    expect(card).toHaveTextContent("Worktree");
    expect(screen.getByText("Open in Cursor")).toBeInTheDocument();
    expect(screen.getByTestId("run-row")).toHaveAttribute("data-variant", "panel");
    expect(screen.getByText("feat/x")).toBeInTheDocument();
    expect(screen.getByText("Pull request #172")).toBeInTheDocument();
    expect(screen.getByText("Commit")).toBeInTheDocument();
    expect(screen.getByLabelText("12 lines added, 3 removed")).toBeInTheDocument();
  });

  it("opens the Changes pane and stays pinned beside it", async () => {
    renderToggle();
    await userEvent.click(screen.getByRole("button", { name: /Changes/ }));
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("changes");
    expect(useUIStore.getState().workspaceDetailsOpen).toBe(true);
  });

  it("opens the default editor from the Open in row", async () => {
    renderToggle();
    await userEvent.click(screen.getByText("Open in Cursor"));
    expect(mocks.openEditor).toHaveBeenCalledWith("cursor");
  });

  it("hides the editor row when nothing can be opened", () => {
    mocks.editorsAvailable = false;
    renderToggle();
    expect(screen.queryByText(/Open in/)).toBeNull();
  });

  it("pushes from the primary git action on a clean tree that is ahead", async () => {
    mocks.workspace = makeWorkspace({
      git_changed_files: 0,
      git_additions: 0,
      git_deletions: 0,
      git_ahead: 2,
    });
    renderToggle();
    await flush();
    await userEvent.click(screen.getByText("Push 2"));
    expect(mocks.gitPushChanges).toHaveBeenCalledWith("/p/.worktrees/feat-x", false);
  });

  it("publishes a branch that has no upstream yet", async () => {
    mocks.hasUpstream = false;
    mocks.workspace = makeWorkspace({
      git_changed_files: 0,
      git_additions: 0,
      git_deletions: 0,
    });
    renderToggle();
    await flush();
    await userEvent.click(screen.getByText("Publish branch"));
    expect(mocks.gitPushChanges).toHaveBeenCalledWith("/p/.worktrees/feat-x", true);
  });

  it("offers Initialize Git for a project folder that is not a repository", () => {
    mocks.workspace = makeWorkspace({
      git_branch: null,
      is_git: false,
      worktree_path: null,
      pr_number: null,
      pr_state: null,
      pr_url: null,
    } as Partial<WorkspaceSnapshot>);
    renderToggle();
    expect(screen.getByText("Initialize Git")).toBeInTheDocument();
    expect(screen.queryByText("Changes")).toBeNull();
  });
});

describe("WorkspaceDetailsToggle — popover (narrow column)", () => {
  it("starts closed and leaves the pinned preference alone", async () => {
    renderToggle({ columnWidth: 900 });
    await flush();
    expect(useUIStore.getState().workspaceDetailsMode).toBe("popover");
    expect(screen.queryByTestId("workspace-details-card")).toBeNull();
    const toggle = screen.getByRole("button", { name: "Workspace details" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");

    await userEvent.click(toggle);
    expect(screen.getByTestId("workspace-details-card")).toHaveAttribute(
      "data-mode",
      "popover",
    );
    expect(useUIStore.getState().workspaceDetailsOpen).toBe(true);

    await userEvent.click(toggle);
    expect(screen.queryByTestId("workspace-details-card")).toBeNull();
  });

  it("closes on an outside press, and after taking the user to a pane", async () => {
    renderToggle({ columnWidth: 900 });
    await flush();
    await userEvent.click(screen.getByRole("button", { name: "Workspace details" }));
    fireEvent.pointerDown(document.body);
    expect(screen.queryByTestId("workspace-details-card")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Workspace details" }));
    await userEvent.click(screen.getByRole("button", { name: /Changes/ }));
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("changes");
    expect(screen.queryByTestId("workspace-details-card")).toBeNull();
  });
});

describe("WorkspaceDetailsToggle — non-chat surfaces", () => {
  it("does not pin over a terminal surface, however wide the column", async () => {
    mocks.workspace = makeWorkspace({ surfaces: [TERMINAL_SURFACE] });
    renderToggle({ columnWidth: 2400 });
    await flush();
    expect(useUIStore.getState().workspaceDetailsMode).toBe("popover");
    expect(screen.queryByTestId("workspace-details-card")).toBeNull();
  });
});

describe("WorkspaceDetailsToggle — keyboard and stale results", () => {
  it("moves focus into the popover and hands it back to the toggle on Escape", async () => {
    renderToggle({ columnWidth: 900 });
    await flush();
    const toggle = screen.getByRole("button", { name: "Workspace details" });
    toggle.focus();
    await userEvent.keyboard("{Enter}");
    const card = await screen.findByTestId("workspace-details-card");
    await flush();
    expect(card.contains(document.activeElement)).toBe(true);
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByTestId("workspace-details-card")).toBeNull();
    expect(toggle).toHaveFocus();
  });

  it("re-reads the upstream when the branch changes under the same checkout", async () => {
    mocks.hasUpstream = true;
    mocks.workspace = makeWorkspace({
      git_changed_files: 0,
      git_additions: 0,
      git_deletions: 0,
    });
    const view = renderToggle();
    await flush();
    expect(screen.getByText("Fetch")).toBeInTheDocument();

    mocks.hasUpstream = false;
    mocks.workspace = makeWorkspace({
      git_branch: "feat/unpublished",
      git_changed_files: 0,
      git_additions: 0,
      git_deletions: 0,
    });
    view.rerender(
      <TooltipProvider delayDuration={0}>
        <WorkspaceDetailsToggle />
        <WorkspaceDetailsHost
          workspaceId="ws-1"
          columnWidth={1600}
          rightInset={0}
          edgeGap={4}
          remPx={16}
        />
      </TooltipProvider>,
    );
    await flush();
    expect(screen.getByText("Publish branch")).toBeInTheDocument();
  });

  it("counts files when the change has no line stats (untracked, binary)", () => {
    mocks.workspace = makeWorkspace({
      git_changed_files: 3,
      git_additions: 0,
      git_deletions: 0,
    });
    renderToggle();
    expect(screen.getByText("3 files")).toBeInTheDocument();
    expect(screen.queryByText("clean")).toBeNull();
  });

  it("disables the toggle while the panel is fully expanded", () => {
    render(
      <TooltipProvider delayDuration={0}>
        <WorkspaceDetailsToggle disabled />
      </TooltipProvider>,
    );
    const toggle = screen.getByRole("button", { name: "Workspace details" });
    expect(toggle).toBeDisabled();
    expect(toggle).toHaveAttribute("aria-pressed", "false");
  });
});

describe("WorkspaceDetailsToggle — round-2 regressions", () => {
  function hostTree() {
    return (
      <TooltipProvider delayDuration={0}>
        <WorkspaceDetailsToggle />
        <WorkspaceDetailsHost
          workspaceId="ws-1"
          columnWidth={1600}
          rightInset={0}
          edgeGap={4}
          remPx={16}
        />
      </TooltipProvider>
    );
  }

  it("marks itself hosted while mounted, so the shortcut can tell", () => {
    const view = renderToggle();
    expect(useUIStore.getState().workspaceDetailsHosted).toBe(true);
    view.unmount();
    expect(useUIStore.getState().workspaceDetailsHosted).toBe(false);
  });

  it("hands focus back to the toggle when the shortcut closes the popover", async () => {
    renderToggle({ columnWidth: 900 });
    await flush();
    const toggle = screen.getByRole("button", { name: "Workspace details" });
    toggle.focus();
    await userEvent.keyboard("{Enter}");
    const card = await screen.findByTestId("workspace-details-card");
    await flush();
    expect(card.contains(document.activeElement)).toBe(true);
    act(() => useUIStore.getState().toggleWorkspaceDetails());
    await flush();
    expect(screen.queryByTestId("workspace-details-card")).toBeNull();
    expect(toggle).toHaveFocus();
  });

  it("does not let a publish of the old branch mark the new branch published", async () => {
    let finishPublish!: () => void;
    mocks.gitPushChanges.mockImplementationOnce(
      () => new Promise<void>((resolve) => (finishPublish = resolve)),
    );
    mocks.hasUpstream = false;
    const clean = { git_changed_files: 0, git_additions: 0, git_deletions: 0 };
    mocks.workspace = makeWorkspace(clean);
    const view = render(hostTree());
    await flush();
    await userEvent.click(screen.getByText("Publish branch"));

    // Switch to another unpublished branch while the first publish runs.
    mocks.workspace = makeWorkspace({ ...clean, git_branch: "feat/other" });
    view.rerender(hostTree());
    await flush();
    expect(screen.getByText("Publish branch")).toBeInTheDocument();

    await act(async () => finishPublish());
    await flush();
    expect(screen.getByText("Publish branch")).toBeInTheDocument();
    expect(screen.queryByText("Fetch")).toBeNull();
  });
});

describe("WorkspaceDetailsToggle — Mario review regressions", () => {
  it("does not pin over an editor tab selected after a chat tab", async () => {
    // Selecting an editor/diff tab leaves `active_surface_id` on the chat.
    mocks.workspace = makeWorkspace({ active_tab_id: "tab-editor" });
    renderToggle();
    await flush();
    expect(useUIStore.getState().workspaceDetailsMode).toBe("popover");
    expect(screen.queryByTestId("workspace-details-card")).toBeNull();
  });

  it("offers no local git actions for a workspace that lives on another device", async () => {
    mocks.workspace = makeWorkspace({ host_id: 7 } as Partial<WorkspaceSnapshot>);
    renderToggle();
    await flush();
    expect(screen.queryByRole("button", { name: "Git actions" })).toBeNull();
    expect(screen.queryByText("Commit")).toBeNull();
    // Branch and Changes still describe it.
    expect(screen.getByText("feat/x")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Changes/ })).toBeInTheDocument();
  });

  it("offers no local git actions for an attach-in-place workspace", async () => {
    mocks.workspace = makeWorkspace({ attach_only: true } as Partial<WorkspaceSnapshot>);
    renderToggle();
    await flush();
    expect(screen.queryByRole("button", { name: "Git actions" })).toBeNull();
  });
});

describe("WorkspaceDetailsToggle — issue and live rows", () => {
  const browserSession: AgentBrowserSession = {
    session_id: "b-1",
    workspace_id: "ws-1",
    cli_session_name: "codemux-ws-1",
    stream_url: "ws://127.0.0.1:9000",
    current_url: "http://localhost:1420/settings",
    is_active: true,
    pane_id: null,
    browser_id: null,
    user_dismissed: false,
  };

  function withBrowser(session: AgentBrowserSession | null) {
    useAppStore.setState({
      appState: { agent_browser_sessions: session ? [session] : [] } as unknown as AppStateSnapshot,
    });
  }

  afterEach(() => {
    withBrowser(null);
    useBrowserPeekStore.setState({ openWorkspaceId: null });
  });

  it("lists the linked issue and opens its details from the row", async () => {
    mocks.workspace = makeWorkspace({
      linked_issue: { number: 412, title: "Chin is cluttered", state: "Open", labels: [] },
    });
    renderToggle();
    const row = screen.getByTestId("workspace-details-issue");
    expect(row).toHaveTextContent("Issue #412");
    expect(row).toHaveTextContent("Open");
    await userEvent.click(row);
    expect(await screen.findByTestId("issue-detail-content")).toBeInTheDocument();
  });

  it("keeps the issue row for a workspace without a git branch", () => {
    mocks.workspace = makeWorkspace({
      git_branch: null,
      pr_number: null,
      pr_state: null,
      pr_url: null,
      linked_issue: { number: 7, title: "x", state: "Closed", labels: [] },
    });
    renderToggle();
    expect(screen.getByTestId("workspace-details-issue")).toHaveTextContent("Closed");
  });

  it("shows a live background browser and opens its peek from the row", async () => {
    withBrowser(browserSession);
    renderToggle();
    const row = screen.getByTestId("workspace-details-browser");
    expect(screen.getByTestId("workspace-details-live")).toHaveTextContent("Live");
    expect(row).toHaveTextContent("localhost:1420");
    await userEvent.click(row);
    expect(useBrowserPeekStore.getState().openWorkspaceId).toBe("ws-1");
    expect(row).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(row);
    expect(useBrowserPeekStore.getState().openWorkspaceId).toBeNull();
  });

  it("leaves the Live section out when the browser is already on screen", () => {
    withBrowser({ ...browserSession, right_panel_docked: true });
    renderToggle();
    expect(screen.queryByTestId("workspace-details-live")).toBeNull();
  });

  it("keeps plugin contributions beside Browser in the same Live section", () => {
    withBrowser(browserSession);
    contributionDisposers.push(registry.register({
      id: "demo:status", source: "plugin:demo", area: COMPOSER_STATUS_AREA,
      render: () => <button>Unity</button>,
    }));
    renderToggle();
    const live = within(screen.getByTestId("workspace-details-live"));
    expect(live.getByTestId("workspace-details-browser")).toBeInTheDocument();
    expect(live.getByRole("button", { name: "Unity" })).toBeInTheDocument();
  });

  it("dots the toggle for a live browser only while the card is hidden", async () => {
    withBrowser(browserSession);
    renderToggle();
    expect(screen.queryByTestId("workspace-details-activity-dot")).toBeNull();
    await userEvent.click(screen.getByTestId("workspace-details-toggle"));
    expect(screen.getByTestId("workspace-details-activity-dot")).toHaveClass("bg-status-remote");
  });
});
