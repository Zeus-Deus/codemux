/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SidebarProvider } from "@/components/ui/sidebar";
import type {
  AppStateSnapshot,
  PaneStatus,
  SurfaceSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

// shadcn SidebarProvider uses useIsMobile → window.matchMedia, which jsdom
// doesn't implement. Stub a minimal shape so the provider can mount.
if (typeof window !== "undefined" && !window.matchMedia) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

let persistedSettled: string | null = null;
vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  archiveWorkspace: vi.fn().mockResolvedValue("archive-1"),
  unarchiveWorkspace: vi.fn().mockResolvedValue("ws-1"),
  closeWorkspace: vi.fn().mockResolvedValue(undefined),
  closeWorkspaceWithWorktree: vi.fn().mockResolvedValue(undefined),
  setWorkspacePinned: vi.fn().mockResolvedValue(undefined),
  dbGetUiState: vi.fn((key: string) =>
    Promise.resolve(key === "sidebar.inbox.settled" ? persistedSettled : null),
  ),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-new"),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  hostsList: vi.fn().mockResolvedValue([]),
  workspacePushToHost: vi.fn(),
  workspacePullBack: vi.fn(),
}));

vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: vi.fn() }),
}));

vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => [],
}));

let workspaces: WorkspaceSnapshot[] = [];
let paneStatuses: Record<string, PaneStatus> = {};
let activeWorkspaceId = "";

function appStoreState() {
  return {
    appState: {
      workspaces,
      pane_statuses: paneStatuses,
      active_workspace_id: activeWorkspaceId,
      detected_ports: [],
    } as unknown as AppStateSnapshot,
    homeDir: "/home/u",
    pendingActiveWorkspaceId: null,
    pendingActivationAt: null,
    beginPendingActivation: vi.fn(),
    clearPendingActivation: vi.fn(),
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
    selectActiveWorkspaceId: (s: { appState: AppStateSnapshot | null }) =>
      s.appState?.active_workspace_id ?? null,
    useHomeDir: () => "/home/u",
    projectDisplayName: (p: string) => p.split("/").pop() ?? p,
    useProjectGroupedWorkspaces: (all: WorkspaceSnapshot[]) => {
      const byPath = new Map<string, WorkspaceSnapshot[]>();
      for (const ws of all) {
        const path = ws.project_root || ws.cwd;
        byPath.set(path, [...(byPath.get(path) ?? []), ws]);
      }
      return [...byPath.entries()].map(([projectPath, list]) => ({
        projectName:
          projectPath === "/home/u" ? "Home" : projectPath.split("/").pop()!,
        projectPath,
        workspaces: list,
      }));
    },
  };
});

import { SidebarProjectsView } from "./sidebar-projects-view";
import { SidebarViewSwitch } from "./sidebar-action-row";
import { SidebarWorkspaceList } from "./sidebar-workspace-list";
import {
  __resetSidebarInboxStoreForTests,
  useSidebarInboxStore,
} from "@/stores/sidebar-inbox-store";
import {
  __resetSidebarViewStoreForTests,
  useSidebarViewStore,
} from "./sidebar-view-store";
import { activateWorkspace, dbSetUiState } from "@/tauri/commands";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { getJumpTarget } from "./sidebar-inbox-jump";

const APP = "/home/u/projects/app";
const LIB = "/home/u/projects/lib";
let counter = 0;

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  counter += 1;
  return {
    workspace_id: `ws-${counter}`,
    title: `Workspace ${counter}`,
    workspace_type: "standard",
    cwd: APP,
    project_root: APP,
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

function withPane(paneId: string): SurfaceSnapshot[] {
  return [{ root: { kind: "pane", pane_id: paneId } }] as unknown as SurfaceSnapshot[];
}

async function renderView() {
  const utils = render(
    <TooltipProvider>
      <SidebarProjectsView />
    </TooltipProvider>,
  );
  for (let i = 0; i < 3; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
  return utils;
}

/** Render the real sidebar mount point (view switch + lifecycle host) with the
 *  Projects view selected, so the inbox view is never mounted. */
function sidebarTree() {
  return (
    <SidebarProvider>
      <TooltipProvider>
        <SidebarWorkspaceList />
      </TooltipProvider>
    </SidebarProvider>
  );
}

async function flush() {
  for (let i = 0; i < 3; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

async function renderSidebar() {
  const utils = render(sidebarTree());
  await flush();
  return utils;
}

/** Re-render in place: the app-store mock is read fresh on every render. */
async function rerenderSidebar(rerender: (ui: React.ReactElement) => void) {
  rerender(sidebarTree());
  await flush();
}

beforeEach(() => {
  vi.clearAllMocks();
  __resetSidebarInboxStoreForTests();
  __resetSidebarViewStoreForTests();
  useSidebarViewStore.setState({ loaded: true, view: "projects" });
  persistedSettled = null;
  workspaces = [];
  paneStatuses = {};
  activeWorkspaceId = "";
  counter = 0;
});

afterEach(async () => {
  await act(async () => {
    await Promise.resolve();
  });
  cleanup();
});

describe("SidebarProjectsView", () => {
  it("lists chats first, then one group per project with one-line rows", async () => {
    workspaces = [
      makeWorkspace({ title: "fix-scroll" }),
      makeWorkspace({ title: "Explain frame clocks", cwd: "/home/u", project_root: "/home/u", is_git: false }),
      makeWorkspace({ title: "lib-work", cwd: LIB, project_root: LIB }),
    ];
    const { container } = await renderView();

    const chats = screen.getByRole("region", { name: "Chats" });
    expect(within(chats).getByText("Explain frame clocks")).toBeInTheDocument();
    expect(within(chats).queryByText("fix-scroll")).toBeNull();

    const headers = [...container.querySelectorAll("[data-project-header]")].map(
      (el) => el.getAttribute("data-project-header"),
    );
    expect(headers).toEqual([APP, LIB]);
    expect(screen.getByRole("button", { name: "app (1)" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
  });

  it("shows the same status labels as the inbox", async () => {
    workspaces = [
      makeWorkspace({ title: "blocked", surfaces: withPane("p1") }),
      makeWorkspace({ title: "done", surfaces: withPane("p2") }),
    ];
    paneStatuses = { p1: "permission", p2: "review" };
    const { container } = await renderView();
    const row = (id: string) =>
      container.querySelector(`[data-projects-row="${id}"]`) as HTMLElement;
    expect(row("ws-1")).toHaveTextContent("Needs you");
    expect(row("ws-2")).toHaveTextContent("Done · review");
  });

  it("nests a follow-up child under its parent with a guide line", async () => {
    const parent = makeWorkspace({ title: "sidebar-view-toggle" });
    const child = makeWorkspace({
      title: "Nest delegated chats",
      parent_workspace_id: parent.workspace_id,
    });
    workspaces = [parent, child];
    const { container } = await renderView();

    const nest = container.querySelector(
      `[data-ws-children="${parent.workspace_id}"]`,
    ) as HTMLElement;
    expect(nest).not.toBeNull();
    expect(nest.className).toContain("border-l");
    // Built on the shared sidebar menu: the child list is a sub menu and
    // each row a list item, so the tree reads as nested lists.
    expect(nest).toHaveAttribute("data-sidebar", "menu-sub");
    expect(nest.tagName).toBe("UL");
    expect(
      nest.querySelector(`[data-projects-row="${child.workspace_id}"]`)!.closest("li"),
    ).toHaveAttribute("data-sidebar", "menu-sub-item");
    expect(
      nest.querySelector(`[data-projects-row="${child.workspace_id}"]`),
    ).not.toBeNull();
  });

  it("renders a child at top level when its parent is settled", async () => {
    const parent = makeWorkspace({ title: "old parent" });
    const child = makeWorkspace({
      title: "still running",
      parent_workspace_id: parent.workspace_id,
    });
    workspaces = [parent, child];
    persistedSettled = JSON.stringify({
      settled: [{ id: parent.workspace_id, at: 1 }],
      snoozed: [],
      keepActive: [],
      activity: {},
    });
    const { container } = await renderView();
    expect(container.querySelector("[data-ws-children]")).toBeNull();
    expect(
      container.querySelector(`[data-projects-row="${child.workspace_id}"]`),
    ).not.toBeNull();
    expect(
      container.querySelector(`[data-projects-row="${parent.workspace_id}"]`),
    ).toBeNull();
  });

  it("collects settled workspaces into one Settled shelf grouped by project", async () => {
    workspaces = [
      makeWorkspace({ title: "live" }),
      makeWorkspace({ title: "parked one" }),
      makeWorkspace({ title: "parked two", cwd: LIB, project_root: LIB }),
    ];
    persistedSettled = JSON.stringify({
      settled: [
        { id: "ws-2", at: 1 },
        { id: "ws-3", at: 2 },
      ],
      snoozed: [],
      keepActive: [],
      activity: {},
    });
    const { container } = await renderView();
    expect(screen.queryByText("parked one")).toBeNull();
    expect(screen.queryByRole("button", { name: /Show \d+ settled/ })).toBeNull();
    const toggle = screen.getByRole("button", { name: "Settled 2" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const groups = [...container.querySelectorAll("[data-shelf-group]")].map((el) =>
      el.getAttribute("data-shelf-group"),
    );
    // Most recently settled first: lib's workspace was settled last.
    expect(groups).toEqual([LIB, APP]);
    expect(screen.getByText("parked one")).toBeInTheDocument();
    expect(screen.getByText("parked two")).toBeInTheDocument();
  });

  it("folds projects with nothing live into N more projects, opening onto their history", async () => {
    workspaces = [
      makeWorkspace({ title: "live" }),
      makeWorkspace({ title: "old lib work", cwd: LIB, project_root: LIB }),
    ];
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-2", at: 1 }],
      snoozed: [],
      keepActive: [],
      activity: {},
    });
    const { container } = await renderView();
    const headers = () =>
      [...container.querySelectorAll("[data-project-header]")].map((el) =>
        el.getAttribute("data-project-header"),
      );
    expect(headers()).toEqual([APP]);
    fireEvent.click(screen.getByRole("button", { name: "1 more project" }));
    expect(headers()).toEqual([APP, LIB]);
    const lib = screen.getByRole("button", { name: "lib" });
    expect(lib).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("old lib work")).toBeNull();
    fireEvent.click(lib);
    expect(lib).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("old lib work")).toBeInTheDocument();
    // A session-only fold: nothing persisted for a project with no live rows.
    expect(useSidebarViewStore.getState().folded).toEqual({});
  });

  it("keeps a recently active project with nothing live, showing its age instead of 0", async () => {
    const recent = Date.now() - 2 * 60 * 60 * 1000;
    workspaces = [
      makeWorkspace({ title: "done lib", cwd: LIB, project_root: LIB, last_active_at: recent }),
    ];
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-1", at: 1 }],
      snoozed: [],
      keepActive: [],
      activity: {},
    });
    const { container } = await renderView();
    const header = screen.getByRole("button", { name: "lib" });
    expect(header.closest("[data-project-group]")).toHaveTextContent("2h");
    expect(container.querySelector("[data-more-projects-toggle]")).toBeNull();
  });

  it("activates a workspace when its row is clicked", async () => {
    workspaces = [makeWorkspace({ title: "click me" })];
    await renderView();
    fireEvent.click(screen.getByText("click me"));
    await act(async () => {
      await Promise.resolve();
    });
    expect(activateWorkspace).toHaveBeenCalledWith("ws-1");
  });

  it("persists folding and marks a folded group hiding a needs-you workspace", async () => {
    workspaces = [makeWorkspace({ title: "blocked", surfaces: withPane("p1") })];
    paneStatuses = { p1: "permission" };
    const { container } = await renderView();
    fireEvent.click(screen.getByRole("button", { name: "app (1)" }));
    expect(useSidebarViewStore.getState().folded).toEqual({ [APP]: true });
    expect(dbSetUiState).toHaveBeenCalledWith(
      "sidebar.projects.folded",
      JSON.stringify([APP]),
    );
    expect(container.querySelector('[data-projects-row="ws-1"]')).toBeNull();
    expect(container.querySelector("[data-folded-needs-you]")).not.toBeNull();
  });

  it("starts a chat in the project from the header +", async () => {
    useFeatureFlags.setState({ enableAgentChat: true, enableLazyWorkspaceCreation: true });
    useChatDraftStore.setState({
      draftsById: {},
      activeHomeDraftId: null,
      projectDraftIdByPath: {},
      activeDraftId: null,
    });
    workspaces = [makeWorkspace({ title: "existing" })];
    await renderView();
    const toggle = screen.getByRole("button", { name: "app (1)" });
    const create = screen.getByRole("button", { name: "New chat in app" });
    expect(create.parentElement).toBe(toggle.parentElement);
    expect(toggle.querySelector("button")).toBeNull();
    fireEvent.click(create);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    await act(async () => {
      await Promise.resolve();
    });
    const store = useChatDraftStore.getState();
    expect(store.activeDraftId).not.toBeNull();
    expect(store.projectDraftIdByPath[APP]).toBe(store.activeDraftId);
  });

  it("publishes visible rows in order as Alt+N jump targets", async () => {
    const parent = makeWorkspace({ title: "parent" });
    const child = makeWorkspace({ title: "child", parent_workspace_id: parent.workspace_id });
    workspaces = [parent, child];
    await renderView();
    expect(getJumpTarget(1)).toBe(parent.workspace_id);
    expect(getJumpTarget(2)).toBe(child.workspace_id);
  });

  it("uses the shared settled list from the inbox store", async () => {
    workspaces = [makeWorkspace({ title: "row" })];
    await renderView();
    act(() => {
      useSidebarInboxStore.getState().settle("ws-1");
    });
    expect(screen.queryByText("row")).toBeNull();
    expect(screen.getByRole("button", { name: "Settled 1" })).toBeInTheDocument();
  });
});

describe("SidebarViewSwitch", () => {
  it("defaults to Activity and switches views with persistence", async () => {
    __resetSidebarViewStoreForTests();
    render(
      <TooltipProvider>
        <SidebarViewSwitch />
      </TooltipProvider>,
    );
    const activity = screen.getByRole("radio", { name: "Activity view" });
    const projects = screen.getByRole("radio", { name: "Projects view" });
    expect(activity).toHaveAttribute("aria-checked", "true");
    expect(projects).toHaveAttribute("aria-checked", "false");

    fireEvent.click(projects);
    expect(useSidebarViewStore.getState().view).toBe("projects");
    expect(projects).toHaveAttribute("aria-checked", "true");
    expect(dbSetUiState).toHaveBeenCalledWith("sidebar.view", "projects");
  });

  it("is one labelled group whose selection never clears", () => {
    __resetSidebarViewStoreForTests();
    render(
      <TooltipProvider>
        <SidebarViewSwitch />
      </TooltipProvider>,
    );
    // ToggleGroup type=single: role=group holding role=radio items, the
    // selected one also marked data-state=on (the raised segment).
    const group = screen.getByRole("group", { name: "Sidebar view" });
    const activity = within(group).getByRole("radio", { name: "Activity view" });
    expect(activity).toHaveAttribute("data-state", "on");

    // Pressing the selected view again must not leave no view selected.
    fireEvent.click(activity);
    expect(useSidebarViewStore.getState().view).toBe("activity");
    expect(activity).toHaveAttribute("aria-checked", "true");
    expect(
      within(group).getByRole("radio", { name: "Projects view" }),
    ).toHaveAttribute("data-state", "off");
  });
});

describe("SidebarProjectsView — inbox lifecycle while Projects is shown", () => {
  it("resurfaces a settled workspace whose agent starts working, and keeps it out after it finishes", async () => {
    workspaces = [makeWorkspace({ title: "parked", surfaces: withPane("p1") })];
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-1", at: 1 }],
      snoozed: [],
      keepActive: ["ws-1"],
      activity: {},
    });
    const { container, rerender } = await renderSidebar();
    expect(container.querySelector("[data-inbox-card]")).toBeNull();
    expect(screen.getByRole("button", { name: "Settled 1" })).toBeInTheDocument();

    paneStatuses = { p1: "working" };
    await rerenderSidebar(rerender);
    // The durable write landed, not just a presentation override.
    expect(useSidebarInboxStore.getState().settled).toEqual([]);
    // Resurfacing is activity, not a user decision: it clears the pin.
    expect(useSidebarInboxStore.getState().keepActive["ws-1"]).toBeUndefined();

    paneStatuses = { p1: "review" };
    await rerenderSidebar(rerender);
    expect(useSidebarInboxStore.getState().settled).toEqual([]);
    expect(container.querySelector('[data-projects-row="ws-1"]')).not.toBeNull();
    expect(container.querySelector("[data-settled-shelf-toggle]")).toBeNull();
  });

  it("wakes a snoozed workspace whose agent raises a hand, and keeps it awake after", async () => {
    const base = Date.now();
    workspaces = [makeWorkspace({ title: "deferred", surfaces: withPane("p1") })];
    persistedSettled = JSON.stringify({
      settled: [],
      snoozed: [{ id: "ws-1", at: base, until: base + 7 * 86_400_000 }],
      keepActive: [],
      activity: {},
    });
    const { container, rerender } = await renderSidebar();
    expect(useSidebarInboxStore.getState().snoozed).toHaveLength(1);
    expect(container.querySelector('[data-projects-row="ws-1"]')).toBeNull();

    paneStatuses = { p1: "permission" };
    await rerenderSidebar(rerender);
    expect(useSidebarInboxStore.getState().snoozed).toEqual([]);

    paneStatuses = {};
    await rerenderSidebar(rerender);
    expect(useSidebarInboxStore.getState().snoozed).toEqual([]);
    expect(container.querySelector('[data-projects-row="ws-1"]')).not.toBeNull();
  });

  it("wakes a snoozed workspace on time via the boundary timer", async () => {
    vi.useFakeTimers();
    try {
      const base = Date.now();
      workspaces = [makeWorkspace({ title: "five seconds out" })];
      persistedSettled = JSON.stringify({
        settled: [],
        snoozed: [{ id: "ws-1", at: base, until: base + 5_000 }],
        keepActive: [],
        activity: {},
      });
      const { container } = await renderSidebar();
      expect(container.querySelector('[data-projects-row="ws-1"]')).toBeNull();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(6_000);
      });
      expect(useSidebarInboxStore.getState().snoozed).toEqual([]);
      expect(container.querySelector('[data-projects-row="ws-1"]')).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps running after switching to Activity", async () => {
    workspaces = [makeWorkspace({ title: "parked", surfaces: withPane("p1") })];
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-1", at: 1 }],
      snoozed: [],
      keepActive: [],
      activity: {},
    });
    const { rerender } = await renderSidebar();
    act(() => {
      useSidebarViewStore.setState({ view: "activity" });
    });
    await flush();
    paneStatuses = { p1: "working" };
    await rerenderSidebar(rerender);
    expect(useSidebarInboxStore.getState().settled).toEqual([]);
  });
});
