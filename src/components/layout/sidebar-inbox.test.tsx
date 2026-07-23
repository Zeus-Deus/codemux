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
        pr_number: 203,
        pr_state: "MERGED",
        pr_url: "https://github.com/u/r/pull/203",
      }),
    ];
    await renderInbox();
    expect(screen.getByText("PR #87")).toBeInTheDocument();
    expect(screen.getByText("merged")).toBeInTheDocument();
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
