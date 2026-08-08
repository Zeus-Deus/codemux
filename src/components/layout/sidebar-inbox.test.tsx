/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render,
  screen,
  fireEvent,
  act,
  cleanup,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  AppStateSnapshot,
  PaneStatus,
  PortInfoSnapshot,
  SurfaceSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

const mockOpenUrl = vi.fn().mockResolvedValue(undefined);
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => mockOpenUrl(...args),
}));

// UI-state persistence for the settled list (plus project appearance keys,
// which resolve to null). Individual tests override the settled key.
let persistedSettled: string | null = null;

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  archiveWorkspace: vi.fn().mockResolvedValue("archive-1"),
  unarchiveWorkspace: vi.fn().mockResolvedValue("ws-1"),
  checkoutDefaultBranchInWorkspace: vi.fn().mockResolvedValue("main"),
  closeWorkspace: vi.fn().mockResolvedValue(undefined),
  closeWorkspaceWithWorktree: vi.fn().mockResolvedValue(undefined),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  setWorkspaceMuted: vi.fn().mockResolvedValue(undefined),
  setWorkspacePinned: vi.fn().mockResolvedValue(undefined),
  detectEditors: vi.fn().mockResolvedValue([]),
  getDefaultBranch: vi.fn().mockResolvedValue("main"),
  openInEditor: vi.fn().mockResolvedValue(undefined),
  runWorkspaceSetup: vi.fn().mockResolvedValue(undefined),
  dbGetUiState: vi.fn((key: string) =>
    Promise.resolve(key === "sidebar.inbox.settled" ? persistedSettled : null),
  ),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
  revealInFileManager: vi.fn().mockResolvedValue(undefined),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-new"),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  getGithubIssue: vi.fn().mockResolvedValue({
    number: 92, title: "Test", state: "Open", labels: [], assignees: [],
    url: "https://github.com/u/r/issues/92", body: null,
  }),
  hostsList: vi.fn().mockResolvedValue([]),
  workspacePushToHost: vi
    .fn()
    .mockResolvedValue({ ok: true, message: "", remote_path: null, rsync_summary: null }),
  workspacePullBack: vi
    .fn()
    .mockResolvedValue({ ok: true, message: "", rsync_summary: null }),
}));

vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: vi.fn() }),
}));

// Record the status each row hands its hover card (the card content itself is
// unmounted while closed, so a render-through wrapper is the observable seam),
// then delegate to the real component.
let hoverCardStatus: Record<string, unknown> = {};
vi.mock("./workspace-hover-card", async (importOriginal) => {
  const mod =
    await importOriginal<typeof import("./workspace-hover-card")>();
  return {
    ...mod,
    WorkspaceHoverCard: (
      props: Parameters<typeof mod.WorkspaceHoverCard>[0],
    ) => {
      hoverCardStatus[props.workspace.workspace_id] = props.status;
      return <mod.WorkspaceHoverCard {...props} />;
    },
  };
});

vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => [],
}));

// App store: a plain object driven per-test. The inbox derives everything
// (cards, chips, statuses) from `appState`.
let workspaces: WorkspaceSnapshot[] = [];
let paneStatuses: Record<string, PaneStatus> = {};
let activeWorkspaceId = "";
let detectedPorts: PortInfoSnapshot[] = [];

function appStoreState() {
  return {
    appState: {
      workspaces,
      pane_statuses: paneStatuses,
      active_workspace_id: activeWorkspaceId,
      detected_ports: detectedPorts,
    } as unknown as AppStateSnapshot,
    homeDir: "/home/u",
    workspacePushPullInFlight: null,
    workspacePushPullStartedAt: null,
    setWorkspacePushPullInFlight: vi.fn(),
    // Optimistic selection: the activation helper writes these before the
    // invoke (docs/plans/gui-responsiveness.md, Phase 1).
    pendingActiveWorkspaceId: null,
    pendingActivationAt: null,
    beginPendingActivation: vi.fn(),
    clearPendingActivation: vi.fn(),
  };
}

vi.mock("@/stores/app-store", () => {
  const useAppStore = Object.assign(
    vi.fn((selector: (s: unknown) => unknown) => selector(appStoreState())),
    { getState: () => appStoreState() },
  );
  return {
    useAppStore,
    // Mirrors the real pending-aware selector: the optimistic id wins while
    // its workspace is present in the snapshot.
    selectActiveWorkspaceId: (s: {
      appState: AppStateSnapshot | null;
      pendingActiveWorkspaceId: string | null;
    }) =>
      (s.pendingActiveWorkspaceId !== null &&
      s.appState?.workspaces.some(
        (w) => w.workspace_id === s.pendingActiveWorkspaceId,
      )
        ? s.pendingActiveWorkspaceId
        : s.appState?.active_workspace_id) ?? null,
    useHomeDir: () => "/home/u",
    useProjectGroupedWorkspaces: (all: WorkspaceSnapshot[]) => {
      const byPath = new Map<string, WorkspaceSnapshot[]>();
      for (const ws of all) {
        const path = ws.project_root ?? ws.cwd;
        byPath.set(path, [...(byPath.get(path) ?? []), ws]);
      }
      return [...byPath.entries()].map(([projectPath, list]) => ({
        projectName: projectPath.split("/").pop() ?? projectPath,
        projectPath,
        workspaces: list,
      }));
    },
  };
});

import {
  SidebarInbox,
  clampTimerDelay,
  compareNewestFirst,
  effectiveActivityAt,
  isSnoozeable,
  isWorkspaceUnread,
  isWrappingUp,
  nextWorkspaceAfterPark,
  selectRange,
  shouldAutoSettle,
  MAX_TIMER_DELAY_MS,
} from "./sidebar-inbox";
import { META_CLUSTER_MIN_WIDTH } from "./sidebar-inbox-card";
import { isPrOnCurrentBranch } from "@/components/github/pr-status-icon";
import {
  __resetSidebarInboxStoreForTests,
  useSidebarInboxStore,
} from "@/stores/sidebar-inbox-store";
import { useSettingsStore } from "@/stores/settings-store";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";
import {
  activateWorkspace,
  dbSetUiState,
  setWorkspacePinned,
} from "@/tauri/commands";
import { getJumpTarget } from "./sidebar-inbox-jump";

let wsCounter = 0;
function makeWorkspace(
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  wsCounter += 1;
  return {
    workspace_id: `ws-${wsCounter}`,
    title: `Workspace ${wsCounter}`,
    workspace_type: "standard",
    cwd: "/home/u/projects/myapp",
    project_root: "/home/u/projects/myapp",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    worktree_path: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    last_active_at: null,
    last_visited_at: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

/** A single-pane surface so `getWorkspaceStatus` sees `paneStatuses[paneId]`. */
function surfaceWithPane(paneId: string): SurfaceSnapshot[] {
  return [
    { root: { kind: "pane", pane_id: paneId } },
  ] as unknown as SurfaceSnapshot[];
}

async function renderInbox() {
  const utils = render(
    <TooltipProvider>
      <SidebarInbox />
    </TooltipProvider>,
  );
  // Flush the settled-list load promise.
  await act(async () => {
    await Promise.resolve();
  });
  return utils;
}

/** Render and settle the background effect cascade (load → activity stamps →
 *  auto-settle sweep). Each step is a store write that re-runs the next one,
 *  so a single microtask flush isn't always enough. */
async function flushRender() {
  const utils = await renderInbox();
  for (let i = 0; i < 3; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
  return utils;
}

/** Re-render the inbox in place (the app-store mock is read fresh each time). */
async function rerenderInbox(rerender: (ui: React.ReactElement) => void) {
  rerender(
    <TooltipProvider>
      <SidebarInbox />
    </TooltipProvider>,
  );
  for (let i = 0; i < 3; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

/** Open the project filter dropdown. Radix menus react to pointer events, so
 *  use userEvent rather than fireEvent. */
async function openFilterMenu() {
  await userEvent.click(
    screen.getByRole("button", { name: "Filter by project" }),
  );
}

/** Open the filter dropdown and pick a project (or "All projects") by name. */
async function pickFilter(name: string) {
  await openFilterMenu();
  await userEvent.click(await screen.findByRole("menuitem", { name }));
}

beforeEach(() => {
  vi.clearAllMocks();
  __resetSidebarInboxStoreForTests();
  useSidebarDensityStore.setState({
    statusSince: {},
    settledAt: {},
    lastSeenAt: {},
    workHistory: {},
  });
  useSettingsStore.setState({ loaded: true, settings: {} });
  persistedSettled = null;
  workspaces = [];
  paneStatuses = {};
  activeWorkspaceId = "";
  detectedPorts = [];
  wsCounter = 0;
  hoverCardStatus = {};
});

afterEach(async () => {
  await act(async () => {
    await Promise.resolve();
  });
  cleanup();
});

describe("SidebarInbox — cards", () => {
  it("renders one card per workspace with its repo eyebrow", async () => {
    workspaces = [
      makeWorkspace({ title: "Fix scroll pinning" }),
      makeWorkspace({
        title: "Client connection overhaul",
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
    ];
    await renderInbox();

    expect(screen.getByText("Fix scroll pinning")).toBeInTheDocument();
    expect(screen.getByText("Client connection overhaul")).toBeInTheDocument();
    // One repo eyebrow per card; the project names live in the (closed) filter
    // dropdown, not inline, so each appears exactly once.
    expect(screen.getAllByText("myapp").length).toBe(1);
    expect(screen.getAllByText("vexis").length).toBe(1);
    // The filter defaults to All projects.
    expect(
      screen.getByRole("button", { name: "Filter by project" }),
    ).toHaveTextContent("All projects");
  });

  it("shows agent state on the right: Working / Needs you / Done · review", async () => {
    workspaces = [
      makeWorkspace({ surfaces: surfaceWithPane("p1") }),
      makeWorkspace({ surfaces: surfaceWithPane("p2") }),
      makeWorkspace({ surfaces: surfaceWithPane("p3") }),
    ];
    paneStatuses = { p1: "working", p2: "permission", p3: "review" };
    await renderInbox();

    expect(screen.getByText("Working")).toBeInTheDocument();
    expect(screen.getByText("Needs you")).toBeInTheDocument();
    expect(screen.getByText("Done · review")).toBeInTheDocument();
    // Needs-you card carries the blocker line — twice over, because the
    // pinned "needs you" strip above the list mirrors the same text as a
    // jump-link. Duplication is the point: the blocker stays one click away
    // however far down its card sits.
    expect(screen.getAllByText("Waiting for your input")).toHaveLength(2);
  });

  it("git stats obey the sidebar.show_git_stats setting (branch always shows)", async () => {
    workspaces = [
      makeWorkspace({
        git_branch: "cm/scroll-pinning",
        git_ahead: 3,
        git_additions: 124,
        git_deletions: 38,
      }),
    ];
    const { unmount } = await renderInbox();
    expect(screen.getByText("cm/scroll-pinning")).toBeInTheDocument();
    expect(screen.getByText("↑3")).toBeInTheDocument();
    expect(screen.getByText("+124")).toBeInTheDocument();
    expect(screen.getByText("−38")).toBeInTheDocument();
    unmount();

    useSettingsStore.setState({
      loaded: true,
      settings: { "sidebar.show_git_stats": "false" },
    });
    await renderInbox();
    expect(screen.getByText("cm/scroll-pinning")).toBeInTheDocument();
    expect(screen.queryByText("↑3")).not.toBeInTheDocument();
    expect(screen.queryByText("+124")).not.toBeInTheDocument();
  });

  it("shows a borderless PR chip (icon + number) on active cards", async () => {
    workspaces = [
      makeWorkspace({
        worktree_path: "/wt/a",
        pr_number: 87,
        pr_state: "OPEN",
        pr_url: "https://github.com/u/r/pull/87",
      }),
      makeWorkspace({
        worktree_path: "/wt/b",
        surfaces: surfaceWithPane("p1"),
        pr_number: 203,
        pr_state: "MERGED",
        pr_url: "https://github.com/u/r/pull/203",
      }),
    ];
    // Live work is a settlement blocker, so the merged card's full meta line
    // remains available while its agent is running.
    paneStatuses = { p1: "working" };
    await renderInbox();
    // Both states render the same way settled rows do: a state-colored icon
    // plus "#<number>", with no "PR" prefix, no "merged" word and none of the
    // old bordered-pill chrome riding along.
    const open = screen.getByRole("button", {
      name: "Pull request #87 — open",
    });
    expect(open).toHaveTextContent("#87");
    expect(open.querySelector("svg")).not.toBeNull();
    expect(open.className).not.toMatch(/\bborder\b/);
    const merged = screen.getByRole("button", {
      name: "Pull request #203 — merged",
    });
    expect(merged).toHaveTextContent("#203");
    expect(merged.querySelector("svg")).not.toBeNull();
    expect(merged.className).not.toMatch(/\bborder\b/);
    expect(screen.queryByText("PR #87")).not.toBeInTheDocument();
    expect(screen.queryByText("merged")).not.toBeInTheDocument();
  });

  it("renders the PR chip icon alone when the number is unknown", async () => {
    workspaces = [
      makeWorkspace({
        worktree_path: "/wt/a",
        pr_number: null,
        pr_state: "OPEN",
        pr_url: "https://github.com/u/r/pull/87",
      }),
    ];
    await renderInbox();

    // A state with no number still says something worth showing, but it must
    // not degrade into a bare "#".
    const pr = screen.getByRole("button", { name: "Pull request — open" });
    expect(pr.querySelector("svg")).not.toBeNull();
    expect(pr.textContent).not.toContain("#");
  });

  it("a PR chip with no URL is inert without leaking the click to the card", async () => {
    workspaces = [
      makeWorkspace({
        worktree_path: "/wt/a",
        pr_number: 87,
        pr_state: "OPEN",
        pr_url: null,
      }),
    ];
    await renderInbox();

    const pr = screen.getByRole("button", { name: "Pull request #87 — open" });
    expect(pr).toBeDisabled();
    // The disabled button has to stay the hit target for that to matter:
    // `pointer-events-none` would pass the click straight through to the card
    // root, which would select and activate the workspace — and show the
    // card's `cursor-pointer` over a chip that does nothing. jsdom does no hit
    // testing, so the class is the observable seam for that half.
    expect(pr.className).toContain("cursor-default");
    expect(pr.className).not.toContain("pointer-events-none");
    await userEvent.click(pr);
    expect(mockOpenUrl).not.toHaveBeenCalled();
    expect(activateWorkspace).not.toHaveBeenCalled();
  });

  it("Enter and Space on an active card's PR chip do not also activate the card", async () => {
    workspaces = [
      makeWorkspace({
        worktree_path: "/wt/a",
        pr_number: 87,
        pr_state: "OPEN",
        pr_url: "https://github.com/u/r/pull/87",
      }),
    ];
    await renderInbox();

    const card = screen
      .getByRole("button", { name: "Pull request #87 — open" })
      .closest("[data-inbox-card]") as HTMLElement;
    const pr = within(card).getByRole("button", {
      name: "Pull request #87 — open",
    });

    // Same contract as a settled row: the card is a `role="button"` container,
    // so a keydown on the chip bubbles into it. Opening the PR and yanking the
    // main pane onto the workspace are two different intents.
    fireEvent.keyDown(pr, { key: "Enter" });
    expect(activateWorkspace).not.toHaveBeenCalled();

    // Space must also survive untouched — the card cancels Space to stop the
    // sidebar scrolling, and doing that here would eat the button's own native
    // activation, so the chip would look focusable but never open anything.
    // A `true` return means nothing called `preventDefault`.
    expect(fireEvent.keyDown(pr, { key: " " })).toBe(true);
    expect(activateWorkspace).not.toHaveBeenCalled();

    // Positive control: the same keys on the card itself still open it (and
    // Space is still cancelled there), so the guard narrows activation rather
    // than removing it.
    expect(fireEvent.keyDown(card, { key: " " })).toBe(false);
    expect(activateWorkspace).toHaveBeenCalledWith("ws-1");
  });

  it("shows the provider logo for agent-chat panes in the workspace", async () => {
    workspaces = [
      makeWorkspace({
        title: "Chatting with Claude",
        surfaces: [
          {
            root: {
              kind: "agent_chat",
              pane_id: "p1",
              title: "Agent Chat",
              thread_id: "t1",
              provider: "claude",
              cwd: null,
            },
          },
        ] as unknown as SurfaceSnapshot[],
      }),
      makeWorkspace({ title: "No chat here" }),
    ];
    await renderInbox();

    const claudeCard = screen
      .getByText("Chatting with Claude")
      .closest("[data-inbox-card]") as HTMLElement;
    expect(
      within(claudeCard).getByAltText("Claude"),
    ).toBeInTheDocument();

    const plainCard = screen
      .getByText("No chat here")
      .closest("[data-inbox-card]") as HTMLElement;
    expect(within(plainCard).queryByAltText("Claude")).not.toBeInTheDocument();
  });

  it("clicking a card activates its workspace", async () => {
    workspaces = [makeWorkspace({ title: "Open me" })];
    await renderInbox();
    fireEvent.click(screen.getByText("Open me"));
    expect(activateWorkspace).toHaveBeenCalledWith("ws-1");
  });

  it("renders pinned workspaces first while preserving creation order within each block", async () => {
    workspaces = [
      makeWorkspace({ title: "Old pinned", pinned_at: 100 }),
      makeWorkspace({ title: "Middle normal" }),
      makeWorkspace({ title: "New normal" }),
    ];

    const { container } = await renderInbox();
    const order = [...container.querySelectorAll("[data-inbox-card]")].map(
      (card) => card.getAttribute("data-inbox-card"),
    );

    expect(order).toEqual(["ws-1", "ws-3", "ws-2"]);
    expect(container.querySelector("[data-pinned-divider]")).not.toBeNull();
    expect(screen.getByRole("img", { name: "Pinned workspace" })).toBeInTheDocument();
  });

  it("draws no pinned divider when nothing is pinned", async () => {
    workspaces = [makeWorkspace({ title: "One" }), makeWorkspace({ title: "Two" })];

    const { container } = await renderInbox();

    expect(container.querySelector("[data-pinned-divider]")).toBeNull();
  });

  it("draws no pinned divider when every card is pinned", async () => {
    // The hairline separates two blocks. With nothing below it, it would
    // promise more list and then end.
    workspaces = [
      makeWorkspace({ title: "Pinned one", pinned_at: 100 }),
      makeWorkspace({ title: "Pinned two", pinned_at: 200 }),
    ];

    const { container } = await renderInbox();

    expect(container.querySelectorAll("[data-inbox-card]")).toHaveLength(2);
    expect(container.querySelector("[data-pinned-divider]")).toBeNull();
  });

  it("lets a pinned card unpin directly and offers Pin in an unpinned card menu", async () => {
    workspaces = [
      makeWorkspace({ title: "Pinned work", pinned_at: 100 }),
      makeWorkspace({ title: "Regular work" }),
    ];
    await renderInbox();

    fireEvent.click(screen.getByRole("button", { name: 'Unpin "Pinned work"' }));
    expect(setWorkspacePinned).toHaveBeenCalledWith("ws-1", false);

    const regularCard = screen
      .getByText("Regular work")
      .closest("[data-inbox-card]") as HTMLElement;
    fireEvent.contextMenu(regularCard);
    fireEvent.click(await screen.findByText("Pin workspace"));
    expect(setWorkspacePinned).toHaveBeenCalledWith("ws-2", true);
  });

  it("pinning overrides settled and snoozed presentation without erasing either shelf", async () => {
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-1", at: 1_000 }],
      snoozed: [{ id: "ws-2", at: 1_000, until: Date.now() + 60_000 }],
      keepActive: [],
      activity: {},
    });
    workspaces = [
      makeWorkspace({ title: "Settled underneath", pinned_at: 100 }),
      makeWorkspace({ title: "Snoozed underneath", pinned_at: 200 }),
    ];

    const { container, rerender } = await renderInbox();
    expect(container.querySelectorAll("[data-inbox-card]")).toHaveLength(2);
    expect(container.querySelector("[data-settled-row]")).toBeNull();
    expect(container.querySelector("[data-snoozed-row]")).toBeNull();
    expect(useSidebarInboxStore.getState().settled.map((entry) => entry.id)).toEqual(["ws-1"]);
    expect(useSidebarInboxStore.getState().snoozed.map((entry) => entry.id)).toEqual(["ws-2"]);

    workspaces = workspaces.map((workspace) =>
      workspace.workspace_id === "ws-1"
        ? { ...workspace, pinned_at: null }
        : workspace,
    );
    await rerenderInbox(rerender);
    expect(container.querySelector('[data-settled-row="ws-1"]')).not.toBeNull();
    expect(container.querySelector('[data-inbox-card="ws-2"]')).not.toBeNull();

    workspaces = workspaces.map((workspace) => ({ ...workspace, pinned_at: null }));
    await rerenderInbox(rerender);
    fireEvent.click(screen.getByRole("button", { name: "Snoozed (1)" }));
    expect(container.querySelector('[data-snoozed-row="ws-2"]')).not.toBeNull();
  });
});

describe("SidebarInbox — project filter", () => {
  it("defaults to All projects and shows the picked project on the trigger", async () => {
    workspaces = [
      makeWorkspace({ title: "In myapp" }),
      makeWorkspace({
        title: "In vexis",
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
    ];
    await renderInbox();

    const trigger = screen.getByRole("button", { name: "Filter by project" });
    expect(trigger).toHaveTextContent("All projects");

    await pickFilter("vexis");
    expect(trigger).toHaveTextContent("vexis");
  });

  it("lists each project's active workspace count (and a total on All)", async () => {
    // myapp: 2 active + 1 settled; vexis: 1 active → 2 / 1, All = 3.
    persistedSettled = JSON.stringify([{ id: "ws-3", at: Date.now() }]);
    workspaces = [
      makeWorkspace({ title: "myapp one" }),
      makeWorkspace({ title: "myapp two" }),
      makeWorkspace({ title: "myapp settled" }),
      makeWorkspace({
        title: "vexis one",
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
    ];
    await renderInbox();

    await openFilterMenu();
    expect(
      await screen.findByRole("menuitem", { name: "All projects" }),
    ).toHaveTextContent("3");
    expect(screen.getByRole("menuitem", { name: "myapp" })).toHaveTextContent(
      "2",
    );
    expect(screen.getByRole("menuitem", { name: "vexis" })).toHaveTextContent(
      "1",
    );
  });

  it("filters both active cards and settled rows by project", async () => {
    persistedSettled = JSON.stringify([{ id: "ws-3", at: Date.now() }]);
    workspaces = [
      makeWorkspace({ title: "In myapp" }),
      makeWorkspace({
        title: "In vexis",
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
      makeWorkspace({ title: "Settled myapp thing" }),
    ];
    await renderInbox();

    // All: both actives + the settled row visible.
    expect(screen.getByText("In myapp")).toBeInTheDocument();
    expect(screen.getByText("In vexis")).toBeInTheDocument();
    expect(screen.getByText("Settled myapp thing")).toBeInTheDocument();

    // Filter to vexis → myapp card + myapp settled row disappear.
    await pickFilter("vexis");
    expect(screen.queryByText("In myapp")).not.toBeInTheDocument();
    expect(screen.getByText("In vexis")).toBeInTheDocument();
    expect(screen.queryByText("Settled myapp thing")).not.toBeInTheDocument();
    expect(screen.queryByText("Settled")).not.toBeInTheDocument();
  });

  it("shows the filtered empty state when a repo has nothing active", async () => {
    workspaces = [
      makeWorkspace({ title: "In myapp" }),
      makeWorkspace({
        title: "In vexis",
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
    ];
    persistedSettled = JSON.stringify([{ id: "ws-2", at: Date.now() }]);
    await renderInbox();

    await pickFilter("vexis");
    expect(screen.getByText(/Nothing active/)).toBeInTheDocument();
    // The settled vexis row still shows under the divider.
    expect(screen.getByText("In vexis")).toBeInTheDocument();
  });

  it("falls back to All projects when the filtered project disappears", async () => {
    workspaces = [
      makeWorkspace({ title: "In myapp" }),
      makeWorkspace({
        title: "In vexis",
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
    ];
    const { rerender } = await renderInbox();

    await pickFilter("vexis");
    const trigger = screen.getByRole("button", { name: "Filter by project" });
    expect(trigger).toHaveTextContent("vexis");

    // That project's last workspace goes away (archived / deleted). Leaving
    // the filter pointed at it would render a blank label over an empty list.
    workspaces = workspaces.filter((w) => w.workspace_id !== "ws-2");
    rerender(
      <TooltipProvider>
        <SidebarInbox />
      </TooltipProvider>,
    );
    await act(async () => {
      await Promise.resolve();
    });

    expect(trigger).toHaveTextContent("All projects");
    expect(screen.getByText("In myapp")).toBeInTheDocument();
  });
});

describe("SidebarInbox — settle / un-settle", () => {
  it("Settle moves a card below the Settled divider (persisted)", async () => {
    vi.useFakeTimers();
    try {
      workspaces = [
        makeWorkspace({ title: "Ship it" }),
        makeWorkspace({ title: "Keep me active" }),
      ];
      render(
        <TooltipProvider>
          <SidebarInbox />
        </TooltipProvider>,
      );
      await act(async () => {
        await Promise.resolve();
      });

      expect(screen.queryByText("Settled")).not.toBeInTheDocument();

      fireEvent.click(
        screen.getByRole("button", { name: 'Settle "Ship it"' }),
      );
      // Collapse animation (~200ms), then the flag flips.
      await act(async () => {
        vi.advanceTimersByTime(250);
      });

      expect(screen.getByText("Settled")).toBeInTheDocument();
      const row = screen
        .getByText("Ship it")
        .closest("[data-settled-row]") as HTMLElement;
      expect(row).not.toBeNull();
      // Un-settle brings it back as a card.
      fireEvent.click(
        within(row).getByRole("button", { name: 'Un-settle "Ship it"' }),
      );
      await act(async () => {
        vi.advanceTimersByTime(500);
      });
      expect(screen.queryByText("Settled")).not.toBeInTheDocument();
      expect(
        screen
          .getByText("Ship it")
          .closest("[data-inbox-card]"),
      ).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("dims a settled row at rest and defers the PR color to hover", async () => {
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [
      makeWorkspace({
        title: "Shipped",
        pr_number: 87,
        pr_state: "OPEN",
        pr_url: "https://github.com/u/r/pull/87",
      }),
    ];
    const { container } = await flushRender();

    const row = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(row).not.toBeNull();
    // Title recedes past the plain muted tone the active cards use, and comes
    // back on hover *and* keyboard focus.
    const title = within(row).getByText("Shipped");
    expect(title.className).toContain("text-muted-foreground/60");
    expect(title.className).toContain("group-hover/settled:text-foreground");
    expect(title.className).toContain(
      "group-focus-within/settled:text-foreground",
    );
    // Repo avatar desaturates rather than disappearing, on the same restore.
    const avatar = within(row).getByText("M");
    expect(avatar.className).toContain("grayscale");
    expect(avatar.className).toContain("group-hover/settled:grayscale-0");
    expect(avatar.className).toContain("group-hover/settled:opacity-100");
    expect(avatar.className).toContain(
      "group-focus-within/settled:grayscale-0",
    );
    // The PR badge is neutral at rest; its state color is a hover variant, so
    // the settled shelf reads as one grey block until you point at a row. It
    // rests one step brighter than the avatar because "#87" is its only label.
    const badge = within(row).getByRole("button", {
      name: "Open PR #87 on GitHub — open",
    });
    expect(badge.className).toContain("text-muted-foreground/55");
    expect(badge.className).not.toMatch(/(^|\s)text-status-open\b/);
    expect(badge.className).toContain("group-hover/settled:text-status-open");
    expect(badge.className).toContain(
      "group-focus-within/settled:text-status-open",
    );
    // The glyph defers to the button's color rather than keeping its own, so
    // number and icon can never light up at different times.
    const glyph = badge.querySelector("svg") as SVGElement;
    expect(glyph.getAttribute("class")).toContain("text-current");
    expect(glyph.getAttribute("class")).not.toMatch(
      /(^|\s)text-status-open\b/,
    );
  });

  it("keeps the settled row you are standing in at full prominence", async () => {
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [makeWorkspace({ title: "Shipped" })];
    activeWorkspaceId = "ws-1";
    const { container } = await flushRender();

    const row = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(row).not.toBeNull();
    expect(within(row).getByText("Shipped").className).toContain(
      "text-foreground",
    );
    expect(within(row).getByText("M").className).not.toContain("grayscale");
  });

  it("keeps a multi-selected settled row at full prominence", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({ title: "Ticked" }),
      makeWorkspace({ title: "Untouched" }),
    ];
    persistInbox({
      settled: workspaces.map((w, i) => ({
        id: w.workspace_id,
        at: base - i * 1000,
      })),
    });
    const { container } = await flushRender();

    const rows = [...container.querySelectorAll("[data-settled-row]")];
    fireEvent.click(rows[0], { ctrlKey: true });

    const ticked = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(ticked.getAttribute("data-selected")).toBe("true");
    expect(within(ticked).getByText("Ticked").className).toContain(
      "text-foreground",
    );
    expect(within(ticked).getByText("M").className).not.toContain("grayscale");
    // The row beside it is untouched by the tick and still recedes.
    const other = container.querySelector(
      '[data-settled-row="ws-2"]',
    ) as HTMLElement;
    expect(within(other).getByText("Untouched").className).toContain(
      "text-muted-foreground/60",
    );
  });

  it("never dims a settled row whose agent is finished and wants review", async () => {
    // The settle safety net leaves review work parked on purpose, so these
    // rows are the one thing on the shelf still asking for a human.
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [
      makeWorkspace({
        title: "Wants review",
        pr_number: 87,
        pr_state: "OPEN",
        pr_url: "https://github.com/u/r/pull/87",
        surfaces: surfaceWithPane("p1"),
      }),
    ];
    paneStatuses = { p1: "review" };
    const { container } = await flushRender();

    const row = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(row).not.toBeNull();
    expect(within(row).getByText("Wants review").className).toContain(
      "text-foreground",
    );
    expect(within(row).getByText("M").className).not.toContain("grayscale");
    // Its PR keeps the state color outright — no hover deferral.
    const badge = within(row).getByRole("button", {
      name: "Open PR #87 on GitHub — open",
    });
    expect(badge.className).toContain("text-status-open");
    expect(badge.className).not.toContain("text-muted-foreground/55");
  });

  it("never dims a settled row holding unread output", async () => {
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [
      makeWorkspace({
        title: "Unseen news",
        last_active_at: Date.now(),
        last_visited_at: null,
      }),
    ];
    const { container } = await flushRender();

    const row = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(row).not.toBeNull();
    expect(within(row).getByText("Unseen news").className).toContain(
      "text-foreground",
    );
    expect(within(row).getByText("M").className).not.toContain("grayscale");
  });

  it("hides the Settle button for live / blocked cards, keeps it for idle / review", async () => {
    workspaces = [
      makeWorkspace({ title: "Working card", surfaces: surfaceWithPane("p1") }),
      makeWorkspace({ title: "Blocked card", surfaces: surfaceWithPane("p2") }),
      makeWorkspace({ title: "Idle card" }),
      makeWorkspace({ title: "Review card", surfaces: surfaceWithPane("p3") }),
    ];
    paneStatuses = { p1: "working", p2: "permission", p3: "review" };
    await renderInbox();

    // Live and blocked agents can never be swept out of sight — no button.
    expect(
      screen.queryByRole("button", { name: /^Settle "Working card"/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /^Settle "Blocked card"/ }),
    ).not.toBeInTheDocument();
    // Idle and finished ("review") cards keep the Settle affordance (CSS-hidden
    // at rest, but present in the DOM).
    expect(
      screen.getByRole("button", { name: /^Settle "Idle card"/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^Settle "Review card"/ }),
    ).toBeInTheDocument();
  });

  it("auto-un-settles a settled workspace whose agent becomes live (persisted)", async () => {
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [
      makeWorkspace({ title: "Resurfacing work", surfaces: surfaceWithPane("p1") }),
    ];
    paneStatuses = { p1: "working" };
    const { container } = await renderInbox();
    // Let the auto-un-settle effect run + flush its store update.
    await act(async () => {
      await Promise.resolve();
    });

    // Back in the active list, no longer a settled row, divider gone.
    expect(
      container.querySelector('[data-inbox-card="ws-1"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-settled-row="ws-1"]'),
    ).toBeNull();
    expect(screen.queryByText("Settled")).not.toBeInTheDocument();
    // The removal was persisted to the settled UI-state key.
    const { dbSetUiState } = await import("@/tauri/commands");
    expect(dbSetUiState).toHaveBeenCalledWith(
      "sidebar.inbox.settled",
      expect.any(String),
    );
  });

  it("keeps a settled 'review' (done) workspace settled — only live work resurfaces", async () => {
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [
      makeWorkspace({ title: "Finished work", surfaces: surfaceWithPane("p1") }),
    ];
    paneStatuses = { p1: "review" };
    const { container } = await renderInbox();
    await act(async () => {
      await Promise.resolve();
    });

    // A finished card must stick under the divider — otherwise the guardrail
    // and auto-un-settle would fight each other.
    expect(
      container.querySelector('[data-settled-row="ws-1"]'),
    ).not.toBeNull();
    expect(screen.getByText("Settled")).toBeInTheDocument();
  });

  it("auto-settles a freshly merged PR without requiring an idle stamp", async () => {
    persistedSettled = JSON.stringify({
      settled: [],
      keepActive: [],
      activity: {},
    });
    workspaces = [
      makeWorkspace({
        title: "Just merged",
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    const { container } = await flushRender();

    expect(screen.getByText("Settled")).toBeInTheDocument();
    expect(container.querySelector('[data-settled-row="ws-1"]')).not.toBeNull();
  });

  it("auto-settles a closed PR immediately", async () => {
    workspaces = [
      makeWorkspace({
        title: "Closed without merge",
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "CLOSED",
      }),
    ];
    const { container } = await flushRender();

    expect(screen.getByText("Settled")).toBeInTheDocument();
    expect(
      container.querySelector('[data-settled-row="ws-1"]'),
    ).not.toBeNull();
    const { dbSetUiState } = await import("@/tauri/commands");
    expect(dbSetUiState).toHaveBeenCalledWith(
      "sidebar.inbox.settled",
      expect.any(String),
    );
  });

  it("shows a side-branch PR badge but never settles the card on it", async () => {
    // The workspace is checked out on `fix-ui-borders`; the PR it opened came
    // off `appimage-child-env-hygiene`, a branch it has since left. The badge
    // is the whole point of the fallback and must render like any other. The
    // merge, however, describes a branch the user is not standing on, so the
    // card stays active — its own checkout is still unfinished.
    workspaces = [
      makeWorkspace({
        title: "Side-branch PR",
        worktree_path: "/wt/a",
        git_branch: "fix-ui-borders",
        pr_number: 250,
        pr_state: "MERGED",
        pr_head_branch: "appimage-child-env-hygiene",
        last_active_at: Date.now(),
      }),
    ];
    const { container } = await flushRender();

    expect(container.querySelector('[data-settled-row="ws-1"]')).toBeNull();
    expect(screen.queryByText("Settled")).not.toBeInTheDocument();
    // Since the settled-treatment chip landed, an active-card badge renders as
    // the state icon + #number rather than the word "merged".
    expect(
      screen.getByRole("button", { name: "Pull request #250 — merged" }),
    ).toBeInTheDocument();
    expect(screen.getByText("#250")).toBeInTheDocument();
  });

  it("still settles on a PR whose head branch IS the checked-out branch", async () => {
    // The control for the test above: same badge, same state, association
    // pointing at the branch actually checked out.
    workspaces = [
      makeWorkspace({
        title: "Own-branch PR",
        worktree_path: "/wt/a",
        git_branch: "fix-ui-borders",
        pr_number: 251,
        pr_state: "MERGED",
        pr_head_branch: "fix-ui-borders",
        last_active_at: Date.now(),
      }),
    ];
    const { container } = await flushRender();

    expect(container.querySelector('[data-settled-row="ws-1"]')).not.toBeNull();
  });

  it("auto-settles completed review work when its PR is merged", async () => {
    workspaces = [
      makeWorkspace({
        title: "Merged and ready",
        surfaces: surfaceWithPane("p1"),
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    paneStatuses = { p1: "review" };
    const { container } = await flushRender();

    expect(container.querySelector('[data-settled-row="ws-1"]')).not.toBeNull();
    expect(hoverCardStatus["ws-1"]).toBe("review");
  });

  it("settles the currently open merged workspace but keeps its row visible", async () => {
    workspaces = [
      makeWorkspace({
        title: "Open merged result",
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    activeWorkspaceId = "ws-1";
    const { container } = await flushRender();

    // Settlement is classification, not navigation: the main workspace stays
    // open and the forced-visible row remains the user's "you are here".
    const row = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(row).not.toBeNull();
    expect(row).toHaveClass("bg-foreground/[0.06]");
    expect(activateWorkspace).not.toHaveBeenCalled();
  });

  it("does NOT auto-settle a merged-PR card while its agent is working", async () => {
    workspaces = [
      makeWorkspace({
        title: "Merged but working",
        surfaces: surfaceWithPane("p1"),
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    paneStatuses = { p1: "working" };
    const { container } = await renderInbox();
    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.queryByText("Settled")).not.toBeInTheDocument();
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
  });

  it("does NOT auto-settle a merged-PR card while it needs permission", async () => {
    workspaces = [
      makeWorkspace({
        title: "Merged but blocked",
        surfaces: surfaceWithPane("p1"),
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    paneStatuses = { p1: "permission" };
    const { container } = await flushRender();

    expect(screen.queryByText("Settled")).not.toBeInTheDocument();
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
  });

  it("keeps a merged-PR card active after the user un-settles it (keep-active pin)", async () => {
    persistedSettled = JSON.stringify({
      settled: [],
      keepActive: [],
      activity: { "ws-1": Date.now() - 2 * 3_600_000 },
    });
    workspaces = [
      makeWorkspace({
        title: "Kept active",
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    const { container, rerender } = await flushRender();

    // It auto-settles first…
    const row = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(row).not.toBeNull();

    // …the user un-settles it, pinning it active.
    fireEvent.click(
      within(row).getByRole("button", { name: 'Un-settle "Kept active"' }),
    );
    await act(async () => {
      await Promise.resolve();
    });
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(container.querySelector('[data-settled-row="ws-1"]')).toBeNull();

    // A later render must NOT re-settle it — the pin holds.
    rerender(
      <TooltipProvider>
        <SidebarInbox />
      </TooltipProvider>,
    );
    await act(async () => {
      await Promise.resolve();
    });
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(container.querySelector('[data-settled-row="ws-1"]')).toBeNull();
  });

  it("keeps the pin on an un-settled card that is also the selected workspace", async () => {
    // Regression: selecting a workspace is not agent activity. If the
    // selection stamp cleared the keep-active pin, the auto-settle sweep
    // would yank the card the user just kept back under the divider.
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-1", at: Date.now() }],
      keepActive: [],
      activity: { "ws-1": Date.now() - 2 * 3_600_000 },
    });
    workspaces = [
      makeWorkspace({
        title: "Kept active",
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    activeWorkspaceId = "ws-1";
    const { container, rerender } = await flushRender();

    // It starts under the divider…
    const row = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(row).not.toBeNull();

    // …the user un-settles it, pinning it active.
    fireEvent.click(
      within(row).getByRole("button", { name: 'Un-settle "Kept active"' }),
    );
    await act(async () => {
      await Promise.resolve();
    });
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(useSidebarInboxStore.getState().keepActive["ws-1"]).toBe(true);

    // A fresh status snapshot re-runs the activity + auto-settle effects the
    // same way the coarse clock tick does. The pin — and the card — must hold.
    paneStatuses = { ...paneStatuses };
    rerender(
      <TooltipProvider>
        <SidebarInbox />
      </TooltipProvider>,
    );
    await act(async () => {
      await Promise.resolve();
    });

    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(container.querySelector('[data-settled-row="ws-1"]')).toBeNull();
    expect(screen.queryByText("Settled")).not.toBeInTheDocument();
    expect(useSidebarInboxStore.getState().keepActive["ws-1"]).toBe(true);
  });

  it("does not clear an un-settle pin by repeatedly observing the same review status", async () => {
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-1", at: Date.now() }],
      keepActive: [],
      activity: {},
    });
    workspaces = [
      makeWorkspace({
        title: "Inspect merged result",
        surfaces: surfaceWithPane("p1"),
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    paneStatuses = { p1: "review" };
    const { container, rerender } = await flushRender();

    const row = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    fireEvent.click(
      within(row).getByRole("button", {
        name: 'Un-settle "Inspect merged result"',
      }),
    );
    await act(async () => {
      await Promise.resolve();
    });

    // A fresh snapshot carrying the same completed state is not a new run.
    paneStatuses = { p1: "review" };
    await rerenderInbox(rerender);

    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(container.querySelector('[data-settled-row="ws-1"]')).toBeNull();
    expect(useSidebarInboxStore.getState().keepActive["ws-1"]).toBe(true);

    // Genuine new activity clears the pin; the merged completion signal may
    // settle it again only after that run is no longer live.
    paneStatuses = { p1: "working" };
    await rerenderInbox(rerender);
    expect(useSidebarInboxStore.getState().keepActive["ws-1"]).toBeUndefined();
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();

    paneStatuses = { p1: "review" };
    await rerenderInbox(rerender);
    expect(container.querySelector('[data-settled-row="ws-1"]')).not.toBeNull();
  });

  it("preserves a persisted keep-active override on the first status snapshot", async () => {
    persistedSettled = JSON.stringify({
      settled: [],
      keepActive: ["ws-1"],
      activity: {},
    });
    workspaces = [
      makeWorkspace({
        title: "Still inspecting after restart",
        surfaces: surfaceWithPane("p1"),
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    paneStatuses = { p1: "review" };
    const { container } = await flushRender();

    // The first snapshot is a baseline, not proof that activity happened
    // after the persisted user override. That explicit override keeps
    // precedence until a later real status edge clears it.
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(container.querySelector('[data-settled-row="ws-1"]')).toBeNull();
    expect(useSidebarInboxStore.getState().keepActive["ws-1"]).toBe(true);
  });

  it("flushes a settle that is still animating when the inbox unmounts", async () => {
    vi.useFakeTimers();
    try {
      workspaces = [makeWorkspace({ title: "Ship it" })];
      const { unmount } = render(
        <TooltipProvider>
          <SidebarInbox />
        </TooltipProvider>,
      );
      await act(async () => {
        await Promise.resolve();
      });

      fireEvent.click(
        screen.getByRole("button", { name: 'Settle "Ship it"' }),
      );
      // Collapsing the sidebar inside the ~200ms collapse window must not
      // silently discard the settle.
      unmount();

      expect(
        useSidebarInboxStore.getState().settled.map((e) => e.id),
      ).toEqual(["ws-1"]);
      const blobs = vi
        .mocked(dbSetUiState)
        .mock.calls.filter(([key]) => key === "sidebar.inbox.settled")
        .map(
          ([, value]) =>
            JSON.parse(value) as { settled: { id: string }[] },
        );
      const lastBlob = blobs[blobs.length - 1];
      expect(lastBlob.settled.map((e) => e.id)).toEqual(["ws-1"]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("auto-settles a card left idle past the auto-settle window", async () => {
    const old = Date.now() - 4 * 86_400_000; // 4 days ago, past the 3-day default
    persistedSettled = JSON.stringify({
      settled: [],
      keepActive: [],
      activity: { "ws-1": old },
    });
    workspaces = [makeWorkspace({ title: "Stale idle work" })];
    const { container } = await renderInbox();
    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.getByText("Settled")).toBeInTheDocument();
    expect(
      container.querySelector('[data-settled-row="ws-1"]'),
    ).not.toBeNull();
  });

  it("never auto-settles a pinned workspace", async () => {
    const old = Date.now() - 4 * 86_400_000;
    persistedSettled = JSON.stringify({
      settled: [],
      keepActive: [],
      activity: { "ws-1": old },
    });
    workspaces = [
      makeWorkspace({ title: "Pinned stale work", pinned_at: Date.now() }),
    ];
    const { container } = await renderInbox();
    await act(async () => {
      await Promise.resolve();
    });

    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(container.querySelector('[data-settled-row="ws-1"]')).toBeNull();
    expect(useSidebarInboxStore.getState().settled).toEqual([]);
  });

  it("does not idle-settle when Auto-settle idle work is off", async () => {
    useSettingsStore.setState({
      loaded: true,
      settings: { "sidebar.auto_settle_days": "off" },
    });
    const old = Date.now() - 4 * 86_400_000;
    persistedSettled = JSON.stringify({
      settled: [],
      keepActive: [],
      activity: { "ws-1": old },
    });
    workspaces = [makeWorkspace({ title: "Stale idle work" })];
    const { container } = await renderInbox();
    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.queryByText("Settled")).not.toBeInTheDocument();
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
  });

  it("settled rows pass the workspace's live agent status to the hover card", async () => {
    // Regression: settled rows hard-coded status={null}, so the hover card
    // read "Idle" even while the workspace's agent was in "review". A settled
    // "review" workspace stays settled (only working/permission resurface),
    // so its row must still surface the real computed status.
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [
      makeWorkspace({ title: "Finished work", surfaces: surfaceWithPane("p1") }),
    ];
    paneStatuses = { p1: "review" };
    const { container } = await renderInbox();
    await act(async () => {
      await Promise.resolve();
    });

    expect(container.querySelector('[data-settled-row="ws-1"]')).not.toBeNull();
    expect(hoverCardStatus["ws-1"]).toBe("review");
  });

  it("settled rows have the workspace context menu with Un-settle on top", async () => {
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-1", at: Date.now() }],
      keepActive: [],
      activity: {},
    });
    workspaces = [makeWorkspace({ title: "Swept aside" })];
    await renderInbox();

    const row = screen
      .getByText("Swept aside")
      .closest("[data-settled-row]") as HTMLElement;
    fireEvent.contextMenu(row);

    // Un-settle entry plus the standard workspace actions.
    expect(await screen.findByText("Un-settle workspace")).toBeInTheDocument();
    expect(screen.getByText("Rename workspace")).toBeInTheDocument();
    expect(screen.getByText("Archive Workspace")).toBeInTheDocument();

    // Choosing Un-settle returns the row to the active card list.
    fireEvent.click(screen.getByText("Un-settle workspace"));
    await act(async () => {
      await Promise.resolve();
    });
    expect(
      screen.getByText("Swept aside").closest("[data-inbox-card]"),
    ).not.toBeNull();
  });

  it("card context menu offers Settle only when the guardrail allows it", async () => {
    workspaces = [
      makeWorkspace({ title: "Idle card" }),
      makeWorkspace({
        title: "Busy card",
        surfaces: surfaceWithPane("p1"),
      }),
    ];
    paneStatuses = { p1: "working" };
    await renderInbox();

    fireEvent.contextMenu(
      screen.getByText("Idle card").closest("[data-inbox-card]") as HTMLElement,
    );
    expect(await screen.findByText("Settle workspace")).toBeInTheDocument();
    // Close the menu before opening the next one.
    fireEvent.keyDown(document.body, { key: "Escape" });

    fireEvent.contextMenu(
      screen.getByText("Busy card").closest("[data-inbox-card]") as HTMLElement,
    );
    expect(await screen.findByText("Rename workspace")).toBeInTheDocument();
    expect(screen.queryByText("Settle workspace")).not.toBeInTheDocument();
  });

  it("a persisted settled workspace renders as a settled row on load", async () => {
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [
      makeWorkspace({
        title: "Implement sidebar v2",
        worktree_path: "/wt/a",
        pr_number: 203,
        pr_state: "MERGED",
        pr_url: "https://github.com/u/r/pull/203",
      }),
    ];
    await renderInbox();

    expect(screen.getByText("Settled")).toBeInTheDocument();
    const row = screen
      .getByText("Implement sidebar v2")
      .closest("[data-settled-row]") as HTMLElement;
    expect(row).not.toBeNull();
    // In a slim settled-history row, the PR remains visible and directly
    // reachable instead of degrading to an unlabeled merge glyph.
    const pr = within(row).getByRole("button", {
      name: "Open PR #203 on GitHub — merged",
    });
    expect(pr).toHaveTextContent("#203");
    expect(pr.querySelector("svg")).not.toBeNull();
    fireEvent.click(pr);
    expect(mockOpenUrl).toHaveBeenCalledWith(
      "https://github.com/u/r/pull/203",
    );
  });

  it("Enter on the PR badge does not also activate the settled row", async () => {
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [
      makeWorkspace({
        title: "Implement sidebar v2",
        worktree_path: "/wt/a",
        pr_number: 203,
        pr_state: "MERGED",
        pr_url: "https://github.com/u/r/pull/203",
      }),
    ];
    await renderInbox();

    const row = screen
      .getByText("Implement sidebar v2")
      .closest("[data-settled-row]") as HTMLElement;
    const pr = within(row).getByRole("button", {
      name: "Open PR #203 on GitHub — merged",
    });

    // The row is itself a `role="button"`, so a keydown on the badge bubbles
    // into it. Opening the PR and yanking the main pane onto the workspace are
    // two different intents — the inner button owns the key press.
    fireEvent.keyDown(pr, { key: "Enter" });
    expect(activateWorkspace).not.toHaveBeenCalled();

    // Positive control: the same key on the row itself still opens it, so the
    // guard narrows activation rather than removing it.
    fireEvent.keyDown(row, { key: "Enter" });
    expect(activateWorkspace).toHaveBeenCalledWith("ws-1");
  });
});

describe("SidebarInbox — jump-to-card shortcuts", () => {
  it("publishes the visible active-card ids to the jump module in view order, scoped by the filter", async () => {
    workspaces = [
      makeWorkspace({ title: "First" }),
      makeWorkspace({
        title: "Second",
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
      makeWorkspace({ title: "Third" }),
    ];
    await renderInbox();

    // Targets mirror the on-screen active-card order — newest workspace first,
    // so Alt+1 lands on the most recently created card.
    expect(getJumpTarget(1)).toBe("ws-3");
    expect(getJumpTarget(2)).toBe("ws-2");
    expect(getJumpTarget(3)).toBe("ws-1");
    expect(getJumpTarget(4)).toBeNull();

    // Filtering to a repo shrinks the target list to that repo's cards.
    await pickFilter("vexis");
    expect(getJumpTarget(1)).toBe("ws-2");
    expect(getJumpTarget(2)).toBeNull();
  });

  it("shows a numbered badge on each visible card while the modifier is held, and hides it on release", async () => {
    workspaces = [
      makeWorkspace({ title: "Alpha" }),
      makeWorkspace({ title: "Beta" }),
    ];
    await renderInbox();

    // No badges at rest.
    expect(screen.queryByText("1")).not.toBeInTheDocument();
    expect(screen.queryByText("2")).not.toBeInTheDocument();

    // Holding the jump modifier reveals a 1..N badge per visible card.
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Alt" }));
    });
    expect(screen.getByText("1")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();

    // Releasing it hides them again — no stuck-open state.
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keyup", { key: "Alt" }));
    });
    expect(screen.queryByText("1")).not.toBeInTheDocument();
    expect(screen.queryByText("2")).not.toBeInTheDocument();
  });
});

describe("SidebarInbox — settled-tail pagination", () => {
  it("collapses a long settled list to a head with a Show-more button", async () => {
    const at = Date.now();
    workspaces = Array.from({ length: 14 }, () => makeWorkspace());
    persistedSettled = JSON.stringify({
      settled: workspaces.map((w) => ({ id: w.workspace_id, at })),
      keepActive: [],
      activity: {},
    });
    const { container } = await renderInbox();

    // Only the initial head renders; the tail hides behind the button.
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(10);
    const more = screen.getByRole("button", {
      name: /Show 4 more settled workspaces \(4 hidden\)/,
    });
    expect(more).toHaveTextContent("Show 4 more (4 hidden)");

    // Clicking reveals the rest and retires the button.
    fireEvent.click(more);
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(14);
    expect(container.querySelector("[data-settled-more]")).toBeNull();
  });

  it("drops the button when a repo filter scopes down, and resets paging on filter change", async () => {
    const at = Date.now();
    const myapp = Array.from({ length: 8 }, () => makeWorkspace());
    const vexis = Array.from({ length: 4 }, () =>
      makeWorkspace({
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
    );
    workspaces = [...myapp, ...vexis];
    persistedSettled = JSON.stringify({
      settled: workspaces.map((w) => ({ id: w.workspace_id, at })),
      keepActive: [],
      activity: {},
    });
    const { container } = await renderInbox();

    // All (12 settled): the head plus a Show-more button.
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(10);
    fireEvent.click(
      container.querySelector("[data-settled-more]") as HTMLElement,
    );
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(12);

    // Filtering to a repo with ≤ head settled rows removes the button entirely.
    await pickFilter("vexis");
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(4);
    expect(container.querySelector("[data-settled-more]")).toBeNull();

    // Switching filters resets paging: back on All only the head renders again.
    await pickFilter("All projects");
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(10);
    expect(container.querySelector("[data-settled-more]")).not.toBeNull();
  });
});

/** Write the persisted inbox blob with only the parts a test cares about. */
function persistInbox(blob: {
  settled?: unknown[];
  snoozed?: unknown[];
  keepActive?: string[];
  activity?: Record<string, number>;
}) {
  persistedSettled = JSON.stringify({
    settled: [],
    snoozed: [],
    keepActive: [],
    activity: {},
    ...blob,
  });
}

function cardOrder(container: HTMLElement): (string | null)[] {
  return [...container.querySelectorAll("[data-inbox-card]")].map((el) =>
    el.getAttribute("data-inbox-card"),
  );
}

function settledOrder(container: HTMLElement): (string | null)[] {
  return [...container.querySelectorAll("[data-settled-row]")].map((el) =>
    el.getAttribute("data-settled-row"),
  );
}

describe("SidebarInbox — card ordering", () => {
  it("puts the newest workspace on top", async () => {
    workspaces = [
      makeWorkspace({ title: "Oldest" }),
      makeWorkspace({ title: "Middle" }),
      makeWorkspace({ title: "Newest" }),
    ];
    const { container } = await flushRender();

    // The backend appends new workspaces, so the last stored entry is the one
    // the user just created — it belongs where they are already looking.
    expect(cardOrder(container)).toEqual(["ws-3", "ws-2", "ws-1"]);
  });

  it("does not reorder cards when an agent's status changes", async () => {
    workspaces = [
      makeWorkspace({ title: "One", surfaces: surfaceWithPane("p1") }),
      makeWorkspace({ title: "Two", surfaces: surfaceWithPane("p2") }),
      makeWorkspace({ title: "Three", surfaces: surfaceWithPane("p3") }),
    ];
    const { container, rerender } = await flushRender();
    expect(cardOrder(container)).toEqual(["ws-3", "ws-2", "ws-1"]);

    // The oldest card's agent starts working and the newest blocks. If status
    // fed the sort, both rows would jump under the pointer mid-read.
    paneStatuses = { p1: "working", p3: "permission" };
    await rerenderInbox(rerender);

    expect(cardOrder(container)).toEqual(["ws-3", "ws-2", "ws-1"]);
    expect(getJumpTarget(1)).toBe("ws-3");
  });

  it("keeps the jump targets in the same order the cards render", async () => {
    workspaces = [
      makeWorkspace({ title: "One" }),
      makeWorkspace({ title: "Two" }),
    ];
    const { container } = await flushRender();

    const rendered = cardOrder(container);
    expect([getJumpTarget(1), getJumpTarget(2)]).toEqual(rendered);
  });
});

describe("SidebarInbox — wrapping-up tier", () => {
  /** Stamps that read as "the agent ran, and the user has since looked". */
  function readStamps(base: number) {
    return { last_active_at: base - 600_000, last_visited_at: base - 60_000 };
  }
  /** Stamps that read as "the agent ran and nobody has looked yet". */
  function unreadStamps(base: number) {
    return { last_active_at: base - 60_000, last_visited_at: base - 600_000 };
  }

  function divider(container: HTMLElement) {
    return container.querySelector("[data-wrapping-up-divider]");
  }

  /** Whether a card renders after the divider rather than merely somewhere in
   *  the list — the whole point of the tier is where it sits. */
  function belowDivider(container: HTMLElement, id: string) {
    const rule = divider(container);
    const card = container.querySelector(`[data-inbox-card="${id}"]`);
    if (!rule || !card) return false;
    return Boolean(
      rule.compareDocumentPosition(card) & Node.DOCUMENT_POSITION_FOLLOWING,
    );
  }

  it("keeps a draft-PR card in the top tier", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({ title: "Plain work" }),
      makeWorkspace({
        title: "Still drafting",
        worktree_path: "/wt/a",
        pr_number: 12,
        pr_state: "DRAFT",
        ...readStamps(base),
      }),
    ];
    const { container } = await flushRender();

    // A draft PR is the author saying the work is still theirs.
    expect(divider(container)).toBeNull();
    expect(cardOrder(container)).toEqual(["ws-2", "ws-1"]);
  });

  it("drops an open-PR card that is idle and already read below the divider", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({ title: "Handed off", worktree_path: "/wt/a", pr_number: 12, pr_state: "OPEN", ...readStamps(base) }),
      makeWorkspace({ title: "Live work" }),
    ];
    const { container } = await flushRender();

    expect(screen.getByText("Wrapping up")).toBeInTheDocument();
    // Still a full card with its own actions — demoted, not parked.
    expect(belowDivider(container, "ws-1")).toBe(true);
    expect(
      screen.getByRole("button", { name: 'Settle "Handed off"' }),
    ).toBeInTheDocument();
    // Newest-first still decides the top tier; the demoted card just left it.
    expect(cardOrder(container)).toEqual(["ws-2", "ws-1"]);
  });

  it("keeps an unread open-PR card on top — unseen news outranks the PR", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({
        title: "PR open, unread",
        worktree_path: "/wt/a",
        pr_number: 12,
        pr_state: "OPEN",
        ...unreadStamps(base),
      }),
    ];
    const { container } = await flushRender();

    expect(divider(container)).toBeNull();
    expect(
      screen.getByLabelText('Unread — "PR open, unread"'),
    ).toBeInTheDocument();
  });

  it("returns an open-PR card to the top while its agent works, and back down when it goes idle", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({
        title: "Follow-up sent",
        surfaces: surfaceWithPane("p1"),
        worktree_path: "/wt/a",
        pr_number: 12,
        pr_state: "OPEN",
        ...readStamps(base),
      }),
      makeWorkspace({ title: "Other work" }),
    ];
    // The user reopened the PR workspace and sent a follow-up message.
    paneStatuses = { p1: "working" };
    const { container, rerender } = await flushRender();

    expect(divider(container)).toBeNull();
    expect(cardOrder(container)).toEqual(["ws-2", "ws-1"]);

    // The agent finishes and the user has seen it: back to winding down, with
    // no flag to unset — the status is the whole mechanism.
    paneStatuses = {};
    await rerenderInbox(rerender);
    expect(belowDivider(container, "ws-1")).toBe(true);
  });

  it("renders no divider when nothing qualifies", async () => {
    workspaces = [makeWorkspace({ title: "One" }), makeWorkspace({ title: "Two" })];
    const { container } = await flushRender();

    expect(divider(container)).toBeNull();
    expect(screen.queryByText("Wrapping up")).not.toBeInTheDocument();
  });

  it("numbers the jump targets straight through the divider", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({
        title: "Winding down",
        worktree_path: "/wt/a",
        pr_number: 12,
        pr_state: "OPEN",
        ...readStamps(base),
      }),
      makeWorkspace({ title: "Middle" }),
      makeWorkspace({ title: "Newest" }),
    ];
    const { container } = await flushRender();

    // Alt+N must follow what the eye sees, or the digits stop matching the
    // badges the moment a card crosses the divider.
    expect(cardOrder(container)).toEqual(["ws-3", "ws-2", "ws-1"]);
    expect([getJumpTarget(1), getJumpTarget(2), getJumpTarget(3)]).toEqual(
      cardOrder(container),
    );
  });

  it("shift-click ranges span the divider in visual order", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({
        title: "Winding down",
        worktree_path: "/wt/a",
        pr_number: 12,
        pr_state: "OPEN",
        ...readStamps(base),
      }),
      makeWorkspace({ title: "Middle" }),
      makeWorkspace({ title: "Newest" }),
    ];
    const { container } = await flushRender();

    const cards = [...container.querySelectorAll("[data-inbox-card]")];
    fireEvent.click(cards[0], { ctrlKey: true });
    fireEvent.click(cards[2], { shiftKey: true });
    fireEvent.contextMenu(cards[2]);

    // Visual top-to-bottom is ws-3, ws-2, ws-1 — a range anchored on the top
    // card and closed on the demoted one covers all three.
    expect(await screen.findByText("Settle (3)")).toBeInTheDocument();
  });

  it("still auto-settles a merged PR rather than demoting it", async () => {
    persistInbox({ activity: { "ws-1": Date.now() - 2 * 3_600_000 } });
    workspaces = [
      makeWorkspace({
        title: "Merged and idle",
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    const { container } = await flushRender();

    // The two rules read different PR states on purpose; a merged PR is a
    // finished claim, not a winding-down one.
    expect(divider(container)).toBeNull();
    expect(container.querySelector('[data-settled-row="ws-1"]')).not.toBeNull();
  });

  it("keeps counting wrapping-up workspaces in the project filter counts", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({
        title: "Winding down",
        worktree_path: "/wt/a",
        pr_number: 12,
        pr_state: "OPEN",
        ...readStamps(base),
      }),
      makeWorkspace({ title: "Live work" }),
    ];
    await flushRender();
    await openFilterMenu();

    // Demotion is presentation only — a count that dropped would report the
    // workspace as gone.
    const all = await screen.findByRole("menuitem", { name: "All projects" });
    expect(all).toHaveTextContent("2");
  });
});

describe("SidebarInbox — idle-clock seeding", () => {
  it("seeds the first-seen baseline from the backend's last_active_at", async () => {
    // Stamping Date.now() here would restart every workspace's idle clock on
    // each app update, so month-old work would look brand new.
    workspaces = [
      makeWorkspace({
        title: "Untouched for days",
        last_active_at: Date.now() - 4 * 86_400_000,
      }),
    ];
    const { container } = await flushRender();

    expect(
      container.querySelector('[data-settled-row="ws-1"]'),
    ).not.toBeNull();
    expect(useSidebarInboxStore.getState().activity["ws-1"]).toBeLessThan(
      Date.now() - 3 * 86_400_000,
    );
  });

  it("settles stale work on upgrade even though the old build already stamped it", async () => {
    // The regression this guards: an install predating `last_active_at` wrote a
    // synthetic "now" baseline into the client activity map for every workspace
    // it already had. That stamp is not a first sighting, so a first-seen-only
    // backfill never replaces it and the user's month-old worktrees sit in the
    // active list for another full idle window after every update.
    const installedAt = Date.now() - 60_000;
    persistedSettled = JSON.stringify({
      settled: [],
      snoozed: [],
      keepActive: [],
      activity: { "ws-1": installedAt },
    });
    workspaces = [
      makeWorkspace({
        title: "Dead for two months",
        last_active_at: Date.now() - 60 * 86_400_000,
      }),
    ];
    const { container } = await flushRender();

    expect(
      container.querySelector('[data-settled-row="ws-1"]'),
    ).not.toBeNull();
    expect(container.querySelector('[data-inbox-card="ws-1"]')).toBeNull();
  });

  it("never instantly settles a workspace with no usable timestamp", async () => {
    workspaces = [makeWorkspace({ title: "No history at all" })];
    const { container } = await flushRender();

    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(screen.queryByText("Settled")).not.toBeInTheDocument();
  });

  it("may idle-settle the focused workspace without navigating away", async () => {
    workspaces = [
      makeWorkspace({
        title: "Open right now",
        last_active_at: Date.now() - 9 * 86_400_000,
      }),
    ];
    activeWorkspaceId = "ws-1";
    const { container } = await flushRender();

    // Merely reading old work does not rewrite its work-activity
    // clock. Settlement changes the row classification, not the open surface.
    expect(container.querySelector('[data-inbox-card="ws-1"]')).toBeNull();
    const row = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(row).not.toBeNull();
    expect(row).toHaveClass("bg-foreground/[0.06]");
    expect(activateWorkspace).not.toHaveBeenCalled();
  });
});

describe("SidebarInbox — snooze shelf", () => {
  it("renders snoozed workspaces on a shelf that is collapsed by default", async () => {
    const base = Date.now();
    persistInbox({
      snoozed: [{ id: "ws-1", at: base, until: base + 3 * 3_600_000 + 30_000 }],
    });
    workspaces = [
      makeWorkspace({ title: "Come back later" }),
      makeWorkspace({ title: "Still active" }),
    ];
    const { container } = await flushRender();

    // Collapsed: header + count only. The rows are work the user explicitly
    // asked not to see, so re-showing them every launch would undo the gesture.
    const header = screen.getByRole("button", { name: "Snoozed (1)" });
    expect(header).toHaveAttribute("aria-expanded", "false");
    expect(header).toHaveTextContent("(1)");
    expect(container.querySelector("[data-snoozed-row]")).toBeNull();
    expect(container.querySelector('[data-inbox-card="ws-1"]')).toBeNull();

    fireEvent.click(header);
    expect(
      container.querySelector('[data-snoozed-row="ws-1"]'),
    ).not.toBeNull();
    // The row's whole story is its return ticket, not time-since.
    expect(screen.getByLabelText(/^Wakes in 3h/)).toBeInTheDocument();
    expect(screen.getByText("Come back later")).toBeInTheDocument();
  });

  it("snoozed rows pass the workspace's live agent status to the hover card", async () => {
    // Same contract as the settled rows: a snoozed "review" workspace stays
    // snoozed (only working/permission wake it early), so its row must hand
    // the hover card the real computed status rather than hard-coding idle.
    const base = Date.now();
    persistInbox({
      snoozed: [{ id: "ws-1", at: base, until: base + 3 * 3_600_000 }],
    });
    workspaces = [
      makeWorkspace({ title: "Deferred work", surfaces: surfaceWithPane("p1") }),
    ];
    paneStatuses = { p1: "review" };
    const { container } = await flushRender();

    fireEvent.click(screen.getByRole("button", { name: "Snoozed (1)" }));
    expect(
      container.querySelector('[data-snoozed-row="ws-1"]'),
    ).not.toBeNull();
    expect(hoverCardStatus["ws-1"]).toBe("review");
  });

  it("wakes a snoozed workspace whose wake time has already passed", async () => {
    const base = Date.now();
    persistInbox({ snoozed: [{ id: "ws-1", at: base - 1000, until: base - 1 }] });
    workspaces = [makeWorkspace({ title: "Due back" })];
    const { container } = await flushRender();

    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(screen.queryByText("Snoozed")).not.toBeInTheDocument();
    expect(useSidebarInboxStore.getState().snoozed).toEqual([]);
  });

  it("arms a precise timer so a wake is not late by up to a coarse tick", async () => {
    vi.useFakeTimers();
    try {
      const base = Date.now();
      persistInbox({ snoozed: [{ id: "ws-1", at: base, until: base + 5_000 }] });
      workspaces = [makeWorkspace({ title: "Five seconds out" })];
      const { container } = render(
        <TooltipProvider>
          <SidebarInbox />
        </TooltipProvider>,
      );
      await act(async () => {
        await Promise.resolve();
      });
      expect(container.querySelector('[data-inbox-card="ws-1"]')).toBeNull();

      // The coarse clock only ticks every 30s — without the boundary timer the
      // card would come back 25 seconds late.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(6_000);
      });
      expect(
        container.querySelector('[data-inbox-card="ws-1"]'),
      ).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("survives a far-future wake instead of firing the timer immediately", async () => {
    // A delay past 2^31-1 ms overflows setTimeout's signed 32-bit field and
    // fires on the spot — a "next year" snooze would wake instantly.
    const base = Date.now();
    const until = base + 400 * 86_400_000;
    persistInbox({ snoozed: [{ id: "ws-1", at: base, until }] });
    workspaces = [makeWorkspace({ title: "Far future" })];
    const { container } = await flushRender();

    expect(container.querySelector('[data-inbox-card="ws-1"]')).toBeNull();
    expect(useSidebarInboxStore.getState().snoozed).toHaveLength(1);
    expect(clampTimerDelay(until - base)).toBe(MAX_TIMER_DELAY_MS);
  });

  it("wakes a snoozed workspace immediately when its agent raises a hand", async () => {
    const base = Date.now();
    persistInbox({
      snoozed: [{ id: "ws-1", at: base, until: base + 7 * 86_400_000 }],
    });
    workspaces = [
      makeWorkspace({ title: "Blocked behind a shelf", surfaces: surfaceWithPane("p1") }),
    ];
    paneStatuses = { p1: "permission" };
    const { container } = await flushRender();

    // Blocked work can never stay hidden, wake time or not.
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    expect(screen.queryByText("Snoozed")).not.toBeInTheDocument();
  });

  it("Wake now returns a snoozed row to the active list", async () => {
    const base = Date.now();
    persistInbox({
      snoozed: [{ id: "ws-1", at: base, until: base + 7 * 86_400_000 }],
    });
    workspaces = [makeWorkspace({ title: "Deferred" })];
    const { container } = await flushRender();

    fireEvent.click(screen.getByRole("button", { name: "Snoozed (1)" }));
    fireEvent.click(
      screen.getByRole("button", { name: 'Wake "Deferred" now' }),
    );
    await act(async () => {
      await Promise.resolve();
    });

    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
    // An explicit wake is a keep-active decision, same as un-settle.
    expect(useSidebarInboxStore.getState().keepActive["ws-1"]).toBe(true);
  });

  it("keeps the open workspace visible even while the snooze shelf is collapsed", async () => {
    const base = Date.now();
    persistInbox({
      snoozed: [
        { id: "ws-1", at: base, until: base + 3_600_000 },
        { id: "ws-2", at: base, until: base + 7_200_000 },
      ],
    });
    workspaces = [
      makeWorkspace({ title: "Hidden one" }),
      makeWorkspace({ title: "The one you are looking at" }),
    ];
    activeWorkspaceId = "ws-2";
    const { container } = await flushRender();

    expect(container.querySelectorAll("[data-snoozed-row]").length).toBe(1);
    expect(
      container.querySelector('[data-snoozed-row="ws-2"]'),
    ).not.toBeNull();
  });
});

describe("SidebarInbox — settled shelf polish", () => {
  it("orders settled rows by when the work ended, and labels them from the same stamp", async () => {
    const base = Date.now();
    persistInbox({
      settled: [
        // Swept aside just now, but the work itself ended five hours ago.
        { id: "ws-1", at: base, workEndedAt: base - 5 * 3_600_000 - 30_000 },
        { id: "ws-2", at: base - 3_600_000, workEndedAt: base - 630_000 },
      ],
    });
    workspaces = [
      makeWorkspace({ title: "Old work, swept late" }),
      makeWorkspace({ title: "Recent work" }),
    ];
    const { container } = await flushRender();

    // Sweep time would have put ws-1 first; work-end time is the honest order.
    expect(settledOrder(container)).toEqual(["ws-2", "ws-1"]);
    const oldRow = container.querySelector(
      '[data-settled-row="ws-1"]',
    ) as HTMLElement;
    expect(within(oldRow).getByText("5h")).toBeInTheDocument();
    const recentRow = container.querySelector(
      '[data-settled-row="ws-2"]',
    ) as HTMLElement;
    expect(within(recentRow).getByText("10m")).toBeInTheDocument();
  });

  it("shows a count in the Settled header and collapses on click", async () => {
    const base = Date.now();
    workspaces = Array.from({ length: 3 }, () => makeWorkspace());
    persistInbox({
      settled: workspaces.map((w, i) => ({ id: w.workspace_id, at: base - i })),
    });
    const { container } = await flushRender();

    const header = screen.getByRole("button", { name: "Settled (3)" });
    expect(header).toHaveAttribute("aria-expanded", "true");
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(3);

    fireEvent.click(header);
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(0);
  });

  it("forces the open workspace's settled row past the Show-more cut", async () => {
    const base = Date.now();
    workspaces = Array.from({ length: 14 }, () => makeWorkspace());
    persistInbox({
      settled: workspaces.map((w, i) => ({
        id: w.workspace_id,
        at: base - i * 1000,
      })),
    });
    // ws-14 sorts last, well past the 10-row head.
    activeWorkspaceId = "ws-14";
    const { container } = await flushRender();

    const rows = settledOrder(container);
    expect(rows).toHaveLength(11);
    expect(rows).toContain("ws-14");
    // Its un-settle affordance and "you are here" highlight stay reachable,
    // and the "+N hidden" count stays truthful: the escaped row is rendered,
    // so it is not counted as hidden (14 settled, 11 visible, 3 hidden).
    expect(
      screen.getByRole("button", {
        name: /Show 3 more settled workspaces \(3 hidden\)/,
      }),
    ).toBeInTheDocument();
  });
});

describe("SidebarInbox — needs-you strip × settled shelf", () => {
  it("a blocked settled workspace keeps a strip jump-link AND a rendered card", async () => {
    // The strip walks every workspace with no settled filter, so a settled
    // workspace that lands on `permission` gets a jump-link. That link is only
    // honest because the permission safety net resurfaces the workspace into
    // the active card list — pin the pair so a future change to either half
    // (a settled filter on the strip, or narrowing the resurface statuses)
    // can't leave the strip pointing at a card that never renders.
    workspaces = [
      ...Array.from({ length: 11 }, () => makeWorkspace()),
      makeWorkspace({
        title: "Blocked but settled",
        surfaces: surfaceWithPane("p-blocked"),
      }),
    ];
    paneStatuses = { "p-blocked": "permission" };
    // All 12 settled; the blocked one is LAST, past the 10-row settled fold,
    // so if it stayed settled its row would not even be paged in.
    persistInbox({
      settled: workspaces.map((w) => ({ id: w.workspace_id, at: Date.now() })),
    });
    const { container } = await flushRender();

    // The pinned strip lists the blocker as a jump-link…
    const link = screen.getByRole("button", {
      name: /Jump to Blocked but settled/,
    });
    // …and its card is genuinely in the DOM: the safety net pulled it out of
    // the settled shelf and into the active list.
    expect(container.querySelector('[data-inbox-card="ws-12"]')).not.toBeNull();
    expect(container.querySelector('[data-settled-row="ws-12"]')).toBeNull();

    // Clicking the strip entry activates the workspace, exactly like a card
    // click would.
    fireEvent.click(link);
    await act(async () => {
      await Promise.resolve();
    });
    expect(activateWorkspace).toHaveBeenCalledWith("ws-12");
  });
});

describe("SidebarInbox — multi-select + bulk actions", () => {
  /** Shift/ctrl-click a shelf row (cards route selection through their own
   *  props; the shelves are wired here). */
  function clickRow(el: Element, opts: MouseEventInit = {}) {
    fireEvent.click(el, opts);
  }

  it("shift-click selects a range over the rendered rows only", async () => {
    const base = Date.now();
    workspaces = Array.from({ length: 12 }, () => makeWorkspace());
    persistInbox({
      settled: workspaces.map((w, i) => ({
        id: w.workspace_id,
        at: base - i * 1000,
      })),
    });
    const { container } = await flushRender();

    const rows = [...container.querySelectorAll("[data-settled-row]")];
    expect(rows).toHaveLength(10);
    clickRow(rows[0]);
    clickRow(rows[9], { shiftKey: true });

    // Two rows are hidden behind Show-more; a count of 12 would be a lie about
    // what the user is acting on.
    fireEvent.contextMenu(rows[9]);
    expect(await screen.findByText("Settle (10)")).toBeInTheDocument();
    expect(screen.queryByText("Settle (12)")).not.toBeInTheDocument();
  });

  it("bulk-settles every selected row and clears the selection", async () => {
    const base = Date.now();
    workspaces = Array.from({ length: 4 }, () => makeWorkspace());
    persistInbox({
      snoozed: workspaces.map((w) => ({
        id: w.workspace_id,
        at: base,
        until: base + 7 * 86_400_000,
      })),
    });
    const { container } = await flushRender();

    fireEvent.click(screen.getByRole("button", { name: "Snoozed (4)" }));
    const rows = [...container.querySelectorAll("[data-snoozed-row]")];
    clickRow(rows[0]);
    clickRow(rows[2], { shiftKey: true });

    fireEvent.contextMenu(rows[1]);
    // Every selected workspace is idle, so Snooze is offered too.
    expect(await screen.findByText("Snooze (3)")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Settle (3)"));
    await act(async () => {
      await Promise.resolve();
    });

    const state = useSidebarInboxStore.getState();
    expect(state.settled.map((e) => e.id).sort()).toEqual([
      "ws-1",
      "ws-2",
      "ws-3",
    ]);
    expect(state.snoozed.map((e) => e.id)).toEqual(["ws-4"]);
  });

  it("ctrl-click toggles a row in and out of the selection", async () => {
    const base = Date.now();
    workspaces = Array.from({ length: 3 }, () => makeWorkspace());
    persistInbox({
      settled: workspaces.map((w, i) => ({
        id: w.workspace_id,
        at: base - i * 1000,
      })),
    });
    const { container } = await flushRender();

    const rows = [...container.querySelectorAll("[data-settled-row]")];
    clickRow(rows[0], { ctrlKey: true });
    clickRow(rows[1], { ctrlKey: true });
    clickRow(rows[2], { ctrlKey: true });
    clickRow(rows[2], { ctrlKey: true });

    fireEvent.contextMenu(rows[0]);
    expect(await screen.findByText("Settle (2)")).toBeInTheDocument();
  });

  it("collapses a multi-selection when a settled row is activated by keyboard", async () => {
    const base = Date.now();
    workspaces = Array.from({ length: 3 }, () => makeWorkspace());
    persistInbox({
      settled: workspaces.map((w, i) => ({
        id: w.workspace_id,
        at: base - i * 1000,
      })),
    });
    const { container } = await flushRender();

    const rows = [...container.querySelectorAll("[data-settled-row]")];
    clickRow(rows[0], { ctrlKey: true });
    clickRow(rows[1], { ctrlKey: true });
    expect(container.querySelectorAll("[data-selected]")).toHaveLength(2);

    // Enter behaves exactly like a plain click: navigate AND collapse the
    // selection, so the next bulk gesture cannot act on rows the user
    // forgot were still ticked behind their navigation.
    fireEvent.keyDown(rows[2], { key: "Enter" });
    expect(container.querySelectorAll("[data-selected]")).toHaveLength(0);
  });

  it("collapses a multi-selection when a snoozed row is activated by keyboard", async () => {
    const base = Date.now();
    workspaces = Array.from({ length: 3 }, () => makeWorkspace());
    persistInbox({
      snoozed: workspaces.map((w) => ({
        id: w.workspace_id,
        at: base,
        until: base + 7 * 86_400_000,
      })),
    });
    const { container } = await flushRender();

    fireEvent.click(screen.getByRole("button", { name: "Snoozed (3)" }));
    const rows = [...container.querySelectorAll("[data-snoozed-row]")];
    clickRow(rows[0], { ctrlKey: true });
    clickRow(rows[1], { ctrlKey: true });
    expect(container.querySelectorAll("[data-selected]")).toHaveLength(2);

    fireEvent.keyDown(rows[2], { key: " " });
    expect(container.querySelectorAll("[data-selected]")).toHaveLength(0);
  });

  it("clears the selection when the project filter changes", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({ title: "myapp one" }),
      makeWorkspace({ title: "myapp two" }),
      makeWorkspace({
        title: "vexis one",
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
    ];
    persistInbox({
      settled: workspaces.map((w, i) => ({
        id: w.workspace_id,
        at: base - i * 1000,
      })),
    });
    const { container } = await flushRender();

    const rows = [...container.querySelectorAll("[data-settled-row]")];
    clickRow(rows[0], { ctrlKey: true });
    clickRow(rows[1], { ctrlKey: true });

    await pickFilter("myapp");
    const scoped = [...container.querySelectorAll("[data-settled-row]")];
    fireEvent.contextMenu(scoped[0]);
    // A selection made under another filter describes rows that may no longer
    // be on screen, so the bulk menu must not appear at all.
    expect(await screen.findByText("Rename workspace")).toBeInTheDocument();
    expect(screen.queryByText(/^Settle \(/)).not.toBeInTheDocument();
  });

  it("survives the shelf chrome — collapsing or paging is not a click on the list", async () => {
    const base = Date.now();
    const active = [
      makeWorkspace({ title: "Active one" }),
      makeWorkspace({ title: "Active two" }),
    ];
    const parked = Array.from({ length: 12 }, () => makeWorkspace());
    workspaces = [...active, ...parked];
    persistInbox({
      settled: parked.map((w, i) => ({ id: w.workspace_id, at: base - i * 1000 })),
    });
    const { container } = await flushRender();

    const cards = [...container.querySelectorAll("[data-inbox-card]")];
    clickRow(cards[0], { ctrlKey: true });
    clickRow(cards[1], { ctrlKey: true });

    // Show-more and the shelf disclosure change what is *rendered*; a user
    // reaching for either is looking for more rows to add to the selection, so
    // dropping it here would make building a bulk action across a long shelf
    // impossible.
    fireEvent.click(
      screen.getByRole("button", { name: /^Show 2 more settled/ }),
    );
    fireEvent.click(screen.getByRole("button", { name: /^Settled \(/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Settled \(/ }));

    fireEvent.contextMenu(cards[0]);
    expect(await screen.findByText("Settle (2)")).toBeInTheDocument();
  });

  it("un-hides, rather than un-ticks, rows a collapsed shelf took off screen", async () => {
    const base = Date.now();
    workspaces = Array.from({ length: 3 }, () => makeWorkspace());
    persistInbox({
      settled: workspaces.map((w, i) => ({
        id: w.workspace_id,
        at: base - i * 1000,
      })),
    });
    const { container } = await flushRender();

    const rows = [...container.querySelectorAll("[data-settled-row]")];
    clickRow(rows[0], { ctrlKey: true });
    clickRow(rows[1], { ctrlKey: true });
    expect(
      container.querySelectorAll('[data-settled-row][data-selected="true"]'),
    ).toHaveLength(2);

    // A collapsed shelf drops its rows out of `renderedIds`, which is what
    // keeps a bulk count honest about what is on screen. The ticks themselves
    // are held separately, so re-opening the shelf brings them straight back.
    const header = screen.getByRole("button", { name: /^Settled \(/ });
    fireEvent.click(header);
    fireEvent.click(header);
    expect(
      container.querySelectorAll('[data-settled-row][data-selected="true"]'),
    ).toHaveLength(2);
  });
});

describe("SidebarInbox — forward navigation on park", () => {
  it("moves you to the next card when you settle the workspace you are viewing", async () => {
    vi.useFakeTimers();
    try {
      workspaces = [
        makeWorkspace({ title: "Older" }),
        makeWorkspace({ title: "Newer" }),
      ];
      activeWorkspaceId = "ws-2";
      render(
        <TooltipProvider>
          <SidebarInbox />
        </TooltipProvider>,
      );
      await act(async () => {
        await Promise.resolve();
      });

      fireEvent.click(screen.getByRole("button", { name: 'Settle "Newer"' }));
      await act(async () => {
        vi.advanceTimersByTime(250);
      });

      expect(activateWorkspace).toHaveBeenCalledWith("ws-1");
    } finally {
      vi.useRealTimers();
    }
  });

  it("never navigates when you park a background workspace", async () => {
    vi.useFakeTimers();
    try {
      workspaces = [
        makeWorkspace({ title: "Older" }),
        makeWorkspace({ title: "Newer" }),
      ];
      activeWorkspaceId = "ws-1";
      render(
        <TooltipProvider>
          <SidebarInbox />
        </TooltipProvider>,
      );
      await act(async () => {
        await Promise.resolve();
      });

      fireEvent.click(screen.getByRole("button", { name: 'Settle "Newer"' }));
      await act(async () => {
        vi.advanceTimersByTime(250);
      });

      // Yanking the user out of what they are reading because some other row
      // was swept would be the most hostile thing this list could do.
      expect(activateWorkspace).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("sidebar-inbox pure helpers", () => {
  it("compareNewestFirst reverses the stored workspace order", () => {
    const rows = [
      { storedIndex: 0, id: "a" },
      { storedIndex: 1, id: "b" },
      { storedIndex: 2, id: "c" },
    ];
    expect([...rows].sort(compareNewestFirst).map((r) => r.id)).toEqual([
      "c",
      "b",
      "a",
    ]);
  });

  it("clampTimerDelay keeps setTimeout inside its signed 32-bit range", () => {
    expect(clampTimerDelay(5_000)).toBe(5_000);
    expect(clampTimerDelay(MAX_TIMER_DELAY_MS + 1)).toBe(MAX_TIMER_DELAY_MS);
    expect(clampTimerDelay(-10)).toBe(0);
    expect(clampTimerDelay(Number.NaN)).toBe(0);
    expect(clampTimerDelay(Number.POSITIVE_INFINITY)).toBe(0);
  });

  it("effectiveActivityAt lets the backend stamp beat a stale client baseline", () => {
    const installedAt = 1_700_000_000_000;
    const realWorkMonthsAgo = installedAt - 60 * 86_400_000;
    // The upgrade case: an install predating `last_active_at` left a synthetic
    // "now" baseline in the client map for every workspace it already had.
    // Honouring it would hide two months of real idleness behind one update.
    expect(effectiveActivityAt(realWorkMonthsAgo, installedAt)).toBe(
      realWorkMonthsAgo,
    );
    // Fallback only when the backend genuinely could not date the checkout.
    expect(effectiveActivityAt(null, installedAt)).toBe(installedAt);
    expect(effectiveActivityAt(undefined, installedAt)).toBe(installedAt);
    // Nothing anywhere stays unknown rather than becoming "idle since epoch".
    expect(effectiveActivityAt(null, undefined)).toBeUndefined();
  });

  it("shouldAutoSettle mirrors server-side completion blockers and idle fallback", () => {
    const now = 10 * 86_400_000;
    const stale = now - 4 * 86_400_000;
    const fresh = now - 60_000;

    for (const prState of ["merged", "closed"] as const) {
      expect(shouldAutoSettle(prState, null, undefined, now, null)).toBe(true);
      expect(shouldAutoSettle(prState, "review", fresh, now, null)).toBe(true);
      expect(shouldAutoSettle(prState, "working", stale, now, 3)).toBe(false);
      expect(shouldAutoSettle(prState, "permission", stale, now, 3)).toBe(false);
    }

    expect(shouldAutoSettle("open", null, stale, now, 3)).toBe(true);
    expect(shouldAutoSettle("draft", "review", stale, now, 3)).toBe(true);
    expect(shouldAutoSettle("open", null, fresh, now, 3)).toBe(false);
    expect(shouldAutoSettle(null, null, undefined, now, 3)).toBe(false);
    expect(shouldAutoSettle(null, null, stale, now, null)).toBe(false);
  });

  it("shouldAutoSettle refuses to complete a workspace on a side-branch PR", () => {
    // The real case: an agent working in a workspace branched off, pushed,
    // opened a PR, and checked the worktree back — leaving eleven uncommitted
    // files behind. The badge is real and belongs on the card. Settling that
    // workspace when the side branch merges would file away work the user
    // never finished.
    const now = 10 * 86_400_000;
    const stale = now - 4 * 86_400_000;
    const fresh = now - 60_000;

    for (const prState of ["merged", "closed"] as const) {
      expect(shouldAutoSettle(prState, null, fresh, now, 3, false)).toBe(false);
      expect(shouldAutoSettle(prState, "review", fresh, now, null, false)).toBe(
        false,
      );
      // Same inputs, current-branch association: settles as it always has.
      expect(shouldAutoSettle(prState, null, fresh, now, 3, true)).toBe(true);
      // The guard removes only the PR shortcut. Idle work still ages out —
      // an unfinished workspace is not made immortal by a side-branch badge.
      expect(shouldAutoSettle(prState, null, stale, now, 3, false)).toBe(true);
    }
  });

  it("isPrOnCurrentBranch treats an unknown head branch as the pre-field case", () => {
    // Matching association — the ordinary badge.
    expect(isPrOnCurrentBranch("fix-ui-borders", "fix-ui-borders")).toBe(true);
    // Side-branch association — badge only.
    expect(isPrOnCurrentBranch("appimage-child-env-hygiene", "fix-ui-borders")).toBe(
      false,
    );
    // Additive field: absent means "predates the field", not "side branch",
    // so an old persisted snapshot keeps settling exactly as it used to.
    expect(isPrOnCurrentBranch(null, "fix-ui-borders")).toBe(true);
    expect(isPrOnCurrentBranch(undefined, "fix-ui-borders")).toBe(true);
  });

  it("isPrOnCurrentBranch keeps the association when the local branch is unknown", () => {
    // `git_branch_info` reports no branch on a detached HEAD — mid-rebase,
    // mid-bisect, `gh pr checkout` of a SHA — while the stored head branch
    // survives. Treating that unknown as a mismatch would flip `hasPr` off
    // and offer "Create PR" for a workspace that already has an open one,
    // for as long as the rebase ran. Missing information must never
    // un-associate a workspace from its own PR.
    expect(isPrOnCurrentBranch("fix-ui-borders", null)).toBe(true);
    expect(isPrOnCurrentBranch("fix-ui-borders", undefined)).toBe(true);
    expect(isPrOnCurrentBranch("fix-ui-borders", "")).toBe(true);
    // Both unknown is still a match, for the same reason.
    expect(isPrOnCurrentBranch(null, null)).toBe(true);
    // Two *known* names that differ is the only real mismatch.
    expect(isPrOnCurrentBranch("side-branch", "fix-ui-borders")).toBe(false);
  });

  it("nextWorkspaceAfterPark steps forward, wraps, and stays put for background parks", () => {
    const ids = ["a", "b", "c"];
    expect(nextWorkspaceAfterPark(["a"], "a", ids)).toBe("b");
    // Wraps past the end.
    expect(nextWorkspaceAfterPark(["c"], "c", ids)).toBe("a");
    // Parking something you aren't looking at must never move you.
    expect(nextWorkspaceAfterPark(["b"], "a", ids)).toBeNull();
    // Bulk park skips the rows going with it.
    expect(nextWorkspaceAfterPark(["a", "b"], "a", ids)).toBe("c");
    // Nothing left to move to.
    expect(nextWorkspaceAfterPark(["a", "b", "c"], "a", ids)).toBeNull();
    expect(nextWorkspaceAfterPark(["a"], "a", ["a"])).toBeNull();
  });

  it("isWorkspaceUnread derives from the two backend stamps", () => {
    expect(isWorkspaceUnread(null, null)).toBe(false);
    expect(isWorkspaceUnread(100, null)).toBe(true);
    expect(isWorkspaceUnread(100, 50)).toBe(true);
    expect(isWorkspaceUnread(100, 100)).toBe(false);
    expect(isWorkspaceUnread(100, 150)).toBe(false);
    // The manual override is the only way to be unread without fresh output.
    expect(isWorkspaceUnread(100, 150, true)).toBe(true);
    expect(isWorkspaceUnread(null, null, true)).toBe(true);
  });

  it("selectRange spans only the rows that are actually rendered", () => {
    // `hidden-1` sits between b and c in the full list but is not rendered.
    const rendered = ["a", "b", "c", "d"];
    expect(selectRange(rendered, "b", "d")).toEqual(["b", "c", "d"]);
    // Direction-agnostic.
    expect(selectRange(rendered, "d", "b")).toEqual(["b", "c", "d"]);
    // A hidden id can never be selected, from either end.
    expect(selectRange(rendered, "hidden-1", "c")).toEqual(["c"]);
    expect(selectRange(rendered, "a", "hidden-1")).toEqual([]);
  });

  it("isWrappingUp demotes only an open, idle, already-read PR", () => {
    const prStates = ["open", "merged", "closed", "draft", null] as const;
    const statuses = [null, "working", "permission", "review"] as const;
    for (const prState of prStates) {
      for (const status of statuses) {
        for (const unread of [false, true]) {
          expect(isWrappingUp(prState, status, unread)).toBe(
            prState === "open" && status === null && !unread,
          );
        }
      }
    }
    // Spelled out, because each of these is a separate promise to the user:
    // a draft PR is work still in progress…
    expect(isWrappingUp("draft", null, false)).toBe(false);
    // …merged and closed belong to immediate completion auto-settle…
    expect(isWrappingUp("merged", null, false)).toBe(false);
    expect(isWrappingUp("closed", null, false)).toBe(false);
    // …a follow-up message puts the agent back to work and the card back up…
    expect(isWrappingUp("open", "working", false)).toBe(false);
    // …and output the user has not seen must never be pushed down.
    expect(isWrappingUp("open", null, true)).toBe(false);
    expect(isWrappingUp("open", null, false)).toBe(true);
    // …and a PR opened off a side branch is not this workspace winding down:
    // the checkout in front of the user may not have shipped anything yet.
    expect(isWrappingUp("open", null, false, false)).toBe(false);
  });

  it("isSnoozeable withholds the gesture from live and blocked agents", () => {
    expect(isSnoozeable(null)).toBe(true);
    expect(isSnoozeable("review")).toBe(true);
    expect(isSnoozeable("working")).toBe(false);
    expect(isSnoozeable("permission")).toBe(false);
  });
});

describe("SidebarInbox — unread + woke markers", () => {
  it("marks a card unread when the agent worked since your last visit", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({
        title: "New output",
        last_active_at: base - 60_000,
        last_visited_at: base - 600_000,
      }),
      makeWorkspace({
        title: "Already read",
        last_active_at: base - 600_000,
        last_visited_at: base - 60_000,
      }),
      makeWorkspace({ title: "Never ran" }),
    ];
    await flushRender();

    expect(
      screen.getByLabelText('Unread — "New output"'),
    ).toBeInTheDocument();
    expect(
      screen.queryByLabelText('Unread — "Already read"'),
    ).not.toBeInTheDocument();
    // No history is not news.
    expect(
      screen.queryByLabelText('Unread — "Never ran"'),
    ).not.toBeInTheDocument();
  });

  it("never marks the workspace you are looking at as unread", async () => {
    const base = Date.now();
    workspaces = [
      makeWorkspace({
        title: "Open right now",
        last_active_at: base,
        last_visited_at: base - 600_000,
      }),
    ];
    activeWorkspaceId = "ws-1";
    await flushRender();

    // The visit is happening; the backend stamp just hasn't caught up.
    expect(
      screen.queryByLabelText('Unread — "Open right now"'),
    ).not.toBeInTheDocument();
  });

  it("badges a card that woke from its snooze and clears it on visit", async () => {
    const base = Date.now();
    persistInbox({ snoozed: [{ id: "ws-1", at: base - 1000, until: base - 1 }] });
    workspaces = [makeWorkspace({ title: "Back again" })];
    const { rerender } = await flushRender();

    // The list order is static, so a returning card needs to say so.
    expect(
      screen.getByLabelText('"Back again" woke from snooze'),
    ).toBeInTheDocument();

    activeWorkspaceId = "ws-1";
    await rerenderInbox(rerender);
    expect(
      screen.queryByLabelText('"Back again" woke from snooze'),
    ).not.toBeInTheDocument();
  });

  it("suppresses the woke badge on a pinned card, whose snooze still expires", async () => {
    // A pin overrides where the card is shown, not whether its snooze ticket
    // comes due — so the shelf entry is still cleared. But the badge announces
    // a *return* to the list, and a pinned card never left it.
    const base = Date.now();
    persistInbox({ snoozed: [{ id: "ws-1", at: base - 1000, until: base - 1 }] });
    workspaces = [makeWorkspace({ title: "Pinned sleeper", pinned_at: 100 })];
    const { container } = await flushRender();

    expect(
      screen.queryByLabelText('"Pinned sleeper" woke from snooze'),
    ).not.toBeInTheDocument();
    // The card is on top either way; what changed is the preserved lifecycle.
    expect(useSidebarInboxStore.getState().snoozed).toEqual([]);
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
  });
});

describe("SidebarInbox — bulk actions across cards", () => {
  it("ctrl-clicking cards builds a selection the right-click menu acts on", async () => {
    workspaces = [
      makeWorkspace({ title: "One" }),
      makeWorkspace({ title: "Two" }),
      makeWorkspace({ title: "Three" }),
    ];
    const { container } = await flushRender();

    const cards = [...container.querySelectorAll("[data-inbox-card]")];
    fireEvent.click(cards[0], { ctrlKey: true });
    fireEvent.click(cards[1], { ctrlKey: true });

    fireEvent.contextMenu(cards[1]);
    // The bulk menu replaces the per-workspace one — otherwise the gesture
    // would quietly narrow back to a single row.
    expect(await screen.findByText("Settle (2)")).toBeInTheDocument();
    expect(screen.queryByText("Rename workspace")).not.toBeInTheDocument();

    fireEvent.click(screen.getByText("Settle (2)"));
    await act(async () => {
      await Promise.resolve();
    });
    expect(
      useSidebarInboxStore.getState().settled.map((e) => e.id).sort(),
    ).toEqual(["ws-2", "ws-3"]);
  });

  it("exempts pinned cards from a mixed bulk park operation", async () => {
    workspaces = [
      makeWorkspace({ title: "Pinned", pinned_at: Date.now() }),
      makeWorkspace({ title: "Park me" }),
    ];
    const { container } = await flushRender();

    const cards = [...container.querySelectorAll("[data-inbox-card]")];
    fireEvent.click(cards[0], { ctrlKey: true });
    fireEvent.click(cards[1], { ctrlKey: true });
    fireEvent.contextMenu(cards[0]);

    expect(await screen.findByText("Settle (1)")).toBeInTheDocument();
    expect(screen.queryByText("Settle (2)")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Settle (1)"));
    await act(async () => {
      await Promise.resolve();
    });

    expect(useSidebarInboxStore.getState().settled.map((entry) => entry.id)).toEqual([
      "ws-2",
    ]);
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
  });

  it("exempts pinned cards from a mixed bulk snooze", async () => {
    // Mirror of the bulk-Settle exemption above: both park verbs share one
    // guardrail, so they must count the selection the same way.
    workspaces = [
      makeWorkspace({ title: "Pinned", pinned_at: Date.now() }),
      makeWorkspace({ title: "Snooze me" }),
    ];
    const { container } = await flushRender();

    const cards = [...container.querySelectorAll("[data-inbox-card]")];
    fireEvent.click(cards[0], { ctrlKey: true });
    fireEvent.click(cards[1], { ctrlKey: true });
    fireEvent.contextMenu(cards[0]);

    expect(await screen.findByText("Snooze (1)")).toBeInTheDocument();
    expect(screen.queryByText("Snooze (2)")).not.toBeInTheDocument();

    await userEvent.hover(screen.getByText("Snooze (1)"));
    await userEvent.click(await screen.findByText("In 1 hour"));
    await act(async () => {
      await Promise.resolve();
    });

    expect(useSidebarInboxStore.getState().snoozed.map((e) => e.id)).toEqual([
      "ws-2",
    ]);
    expect(container.querySelector('[data-inbox-card="ws-1"]')).not.toBeNull();
  });

  it("bulk-snoozes a selection when every workspace can take it", async () => {
    workspaces = [
      makeWorkspace({ title: "One" }),
      makeWorkspace({ title: "Two" }),
    ];
    const { container } = await flushRender();

    const cards = [...container.querySelectorAll("[data-inbox-card]")];
    fireEvent.click(cards[0], { ctrlKey: true });
    fireEvent.click(cards[1], { ctrlKey: true });
    fireEvent.contextMenu(cards[0]);

    await userEvent.hover(await screen.findByText("Snooze (2)"));
    await userEvent.click(await screen.findByText("In 1 hour"));
    await act(async () => {
      await Promise.resolve();
    });

    expect(
      useSidebarInboxStore.getState().snoozed.map((e) => e.id).sort(),
    ).toEqual(["ws-1", "ws-2"]);
  });

  it("withholds bulk Settle and Snooze when any selected workspace is live", async () => {
    workspaces = [
      makeWorkspace({ title: "Idle one" }),
      makeWorkspace({ title: "Busy one", surfaces: surfaceWithPane("p1") }),
    ];
    paneStatuses = { p1: "working" };
    const { container } = await flushRender();

    const cards = [...container.querySelectorAll("[data-inbox-card]")];
    fireEvent.click(cards[0], { ctrlKey: true });
    fireEvent.click(cards[1], { ctrlKey: true });
    fireEvent.contextMenu(cards[0]);

    // Settle rides the same guardrail as the per-row action and as Snooze:
    // live or blocked work can never be parked, and a bulk action that
    // silently skipped the busy half would make its own count a lie. With
    // both verbs withheld, the menu explains itself rather than rendering
    // empty.
    expect(
      await screen.findByText(
        "Selection includes working or blocked workspaces",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText("Settle (2)")).not.toBeInTheDocument();
    expect(screen.queryByText("Snooze (2)")).not.toBeInTheDocument();
    expect(useSidebarInboxStore.getState().settled).toEqual([]);
  });

  it("withholds bulk Settle when a selected workspace is blocked on a permission prompt", async () => {
    workspaces = [
      makeWorkspace({ title: "Idle one" }),
      makeWorkspace({ title: "Blocked one", surfaces: surfaceWithPane("p1") }),
    ];
    paneStatuses = { p1: "permission" };
    const { container } = await flushRender();

    const cards = [...container.querySelectorAll("[data-inbox-card]")];
    fireEvent.click(cards[0], { ctrlKey: true });
    fireEvent.click(cards[1], { ctrlKey: true });
    fireEvent.contextMenu(cards[0]);

    expect(
      await screen.findByText(
        "Selection includes working or blocked workspaces",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText("Settle (2)")).not.toBeInTheDocument();
    expect(useSidebarInboxStore.getState().settled).toEqual([]);
  });
});

describe("SidebarInbox — running-process indicator", () => {
  function port(overrides: Partial<PortInfoSnapshot>): PortInfoSnapshot {
    return {
      port: 3000,
      pid: 42,
      process_name: "node",
      workspace_id: null,
      label: null,
      source: null,
      ...overrides,
    };
  }

  it("marks the workspace the port scan attributed a listener to", async () => {
    const ws = makeWorkspace({ title: "Has a dev server" });
    workspaces = [ws, makeWorkspace({ title: "Quiet one" })];
    detectedPorts = [port({ port: 5173, workspace_id: ws.workspace_id })];
    await flushRender();

    expect(
      screen.getByRole("img", { name: "Long-running process on :5173" }),
    ).toBeInTheDocument();
    // One indicator, on one card — the quiet workspace stays quiet.
    expect(screen.getAllByRole("img", { name: /Long-running process/ })).toHaveLength(
      1,
    );
  });

  it("ignores ports the scan could not attribute to a workspace", async () => {
    // The OS scan reaches processes that belong to no open worktree at all.
    // Badging every card for those would make the indicator meaningless.
    workspaces = [makeWorkspace({ title: "Only one" })];
    detectedPorts = [port({ port: 8080, workspace_id: null })];
    await flushRender();

    expect(
      screen.queryByRole("img", { name: /Long-running process/ }),
    ).toBeNull();
  });

  it("names one stable port when a workspace holds several", async () => {
    // Which port is named is arbitrary; that it does not flip as the scanner
    // reorders its results is not. The hover card carries the full list.
    const ws = makeWorkspace({ title: "Busy" });
    workspaces = [ws];
    detectedPorts = [
      port({ port: 5173, workspace_id: ws.workspace_id }),
      port({ port: 3000, workspace_id: ws.workspace_id }),
    ];
    await flushRender();

    expect(
      screen.getByRole("img", { name: "Long-running process on :3000" }),
    ).toBeInTheDocument();
  });

  it("reserves one trailing column for every card, indicator or not", async () => {
    // The reservation is per list, not per card: without it the card carrying
    // the indicator would push its PR chip left of its neighbours'. (A lone
    // 12px run glyph still fits inside the 15px floor, so the shared value
    // here IS the floor — what matters is that both cards quote the same one.
    // The arithmetic that grows it is unit-tested on `metaClusterWidth`.)
    const ws = makeWorkspace({ title: "Serving" });
    workspaces = [ws, makeWorkspace({ title: "Not serving" })];
    detectedPorts = [port({ port: 4173, workspace_id: ws.workspace_id })];
    const { container } = await flushRender();

    const widths = [...container.querySelectorAll("[data-meta-line]")].map(
      (line) =>
        (line.lastElementChild as HTMLElement | null)?.style.minWidth ?? "",
    );
    expect(widths).toEqual([
      `${META_CLUSTER_MIN_WIDTH}px`,
      `${META_CLUSTER_MIN_WIDTH}px`,
    ]);
  });
});
