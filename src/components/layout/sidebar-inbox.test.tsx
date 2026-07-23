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
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  AppStateSnapshot,
  PaneStatus,
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

vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => [],
}));

// App store: a plain object driven per-test. The inbox derives everything
// (cards, chips, statuses) from `appState`.
let workspaces: WorkspaceSnapshot[] = [];
let paneStatuses: Record<string, PaneStatus> = {};
let activeWorkspaceId = "";

function appStoreState() {
  return {
    appState: {
      workspaces,
      pane_statuses: paneStatuses,
      active_workspace_id: activeWorkspaceId,
    } as unknown as AppStateSnapshot,
    homeDir: "/home/u",
    workspacePushPullInFlight: null,
    workspacePushPullStartedAt: null,
    setWorkspacePushPullInFlight: vi.fn(),
  };
}

vi.mock("@/stores/app-store", () => {
  const useAppStore = Object.assign(
    vi.fn((selector: (s: unknown) => unknown) => selector(appStoreState())),
    { getState: () => appStoreState() },
  );
  return {
    useAppStore,
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

import { SidebarInbox } from "./sidebar-inbox";
import { __resetSidebarInboxStoreForTests } from "@/stores/sidebar-inbox-store";
import { useSettingsStore } from "@/stores/settings-store";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";
import { activateWorkspace } from "@/tauri/commands";
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
  wsCounter = 0;
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
    // Repo eyebrows (once per card) + repo filter chips (once per repo).
    expect(screen.getAllByText("myapp").length).toBe(2);
    expect(screen.getAllByText("vexis").length).toBe(2);
    expect(
      screen.getByRole("button", { name: "Show all repositories" }),
    ).toBeInTheDocument();
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
    // Needs-you card carries the blocker line.
    expect(screen.getByText("Waiting for your input")).toBeInTheDocument();
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

  it("shows a PR chip (merged → violet label)", async () => {
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
    // A "review" status keeps the merged card active (idle merged-PR cards
    // auto-settle) so its meta-line chip stays on screen.
    paneStatuses = { p1: "review" };
    await renderInbox();
    expect(screen.getByText("PR #87")).toBeInTheDocument();
    expect(screen.getByText("merged")).toBeInTheDocument();
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
});

describe("SidebarInbox — repo filter chips", () => {
  it("filters both active cards and settled rows by repo", async () => {
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
    fireEvent.click(screen.getByRole("button", { name: "Filter by vexis" }));
    expect(screen.queryByText("In myapp")).not.toBeInTheDocument();
    expect(screen.getByText("In vexis")).toBeInTheDocument();
    expect(screen.queryByText("Settled myapp thing")).not.toBeInTheDocument();
    expect(screen.queryByText("Settled")).not.toBeInTheDocument();
  });

  it("translates vertical wheel motion into horizontal chip-strip scroll", async () => {
    workspaces = [
      makeWorkspace(),
      makeWorkspace({
        cwd: "/home/u/projects/vexis",
        project_root: "/home/u/projects/vexis",
      }),
    ];
    const { container } = await renderInbox();
    const strip = container.querySelector("[data-chip-strip]") as HTMLElement;
    expect(strip).not.toBeNull();

    // jsdom has no layout — simulate an overflowing strip.
    Object.defineProperty(strip, "scrollWidth", { value: 400, configurable: true });
    Object.defineProperty(strip, "clientWidth", { value: 200, configurable: true });
    strip.scrollLeft = 0;

    // Vertical wheel → horizontal scroll, and the event is consumed so the
    // card list underneath doesn't scroll too.
    const down = new WheelEvent("wheel", {
      deltaY: 40,
      deltaX: 0,
      bubbles: true,
      cancelable: true,
    });
    strip.dispatchEvent(down);
    expect(strip.scrollLeft).toBe(40);
    expect(down.defaultPrevented).toBe(true);

    // Predominantly-horizontal wheel (tilt / trackpad) is left to native
    // scrolling untouched.
    const sideways = new WheelEvent("wheel", {
      deltaY: 2,
      deltaX: 30,
      bubbles: true,
      cancelable: true,
    });
    strip.dispatchEvent(sideways);
    expect(strip.scrollLeft).toBe(40);
    expect(sideways.defaultPrevented).toBe(false);
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

    fireEvent.click(screen.getByRole("button", { name: "Filter by vexis" }));
    expect(screen.getByText(/Nothing active/)).toBeInTheDocument();
    // The settled vexis row still shows under the divider.
    expect(screen.getByText("In vexis")).toBeInTheDocument();
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

  it("auto-settles an idle merged-PR card on render (persisted)", async () => {
    workspaces = [
      makeWorkspace({
        title: "Merged and idle",
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    const { container } = await renderInbox();
    // Flush the background settle store update.
    await act(async () => {
      await Promise.resolve();
    });

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

  it("keeps a merged-PR card active after the user un-settles it (keep-active pin)", async () => {
    workspaces = [
      makeWorkspace({
        title: "Kept active",
        worktree_path: "/wt/a",
        pr_number: 5,
        pr_state: "MERGED",
      }),
    ];
    const { container, rerender } = await renderInbox();
    await act(async () => {
      await Promise.resolve();
    });

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

  it("a persisted settled workspace renders as a settled row on load", async () => {
    persistedSettled = JSON.stringify([{ id: "ws-1", at: Date.now() }]);
    workspaces = [
      makeWorkspace({
        title: "Implement sidebar v2",
        worktree_path: "/wt/a",
        pr_number: 203,
        pr_state: "MERGED",
      }),
    ];
    await renderInbox();

    expect(screen.getByText("Settled")).toBeInTheDocument();
    const row = screen
      .getByText("Implement sidebar v2")
      .closest("[data-settled-row]") as HTMLElement;
    expect(row).not.toBeNull();
    // Merged-PR settled rows carry the violet merge glyph.
    expect(within(row).getByLabelText("PR merged")).toBeInTheDocument();
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

    // Targets mirror the on-screen active-card order.
    expect(getJumpTarget(1)).toBe("ws-1");
    expect(getJumpTarget(2)).toBe("ws-2");
    expect(getJumpTarget(3)).toBe("ws-3");
    expect(getJumpTarget(4)).toBeNull();

    // Filtering to a repo shrinks the target list to that repo's cards.
    fireEvent.click(screen.getByRole("button", { name: "Filter by vexis" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Filter by vexis" }));
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(4);
    expect(container.querySelector("[data-settled-more]")).toBeNull();

    // Switching filters resets paging: back on All only the head renders again.
    fireEvent.click(
      screen.getByRole("button", { name: "Show all repositories" }),
    );
    expect(container.querySelectorAll("[data-settled-row]").length).toBe(10);
    expect(container.querySelector("[data-settled-more]")).not.toBeNull();
  });
});
