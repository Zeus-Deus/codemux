/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { AppStateSnapshot, SurfaceSnapshot, WorkspaceSnapshot } from "@/tauri/types";
import type { PluginTreeNode } from "./contract";
import { installation, manifest } from "./test-fixtures";

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

let readTree: () => Promise<unknown> = async () => ({ nodes: [] });
let persistedSettled: string | null = null;
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (_command: string, args: { method: string }) => (args.method === "readTree" ? readTree() : undefined)),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn().mockResolvedValue(undefined) }));
vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  dbGetUiState: vi.fn((key: string) => Promise.resolve(key === "sidebar.inbox.settled" ? persistedSettled : null)),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
  hostsList: vi.fn().mockResolvedValue([]),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-new"),
}));
vi.mock("@/hooks/use-project-actions", () => ({ useProjectActions: () => ({ openProject: vi.fn() }) }));
vi.mock("@/stores/hosts-store", () => ({ useHosts: () => [] }));

let workspaces: WorkspaceSnapshot[] = [];
function appStoreState() {
  return {
    appState: { workspaces, pane_statuses: {}, active_workspace_id: "", detected_ports: [] } as unknown as AppStateSnapshot,
    homeDir: "/home/u",
    pendingActiveWorkspaceId: null,
  };
}
vi.mock("@/stores/app-store", () => {
  const useAppStore = Object.assign(
    vi.fn((selector: (s: unknown) => unknown) => selector(appStoreState())),
    { getState: () => appStoreState() },
  );
  return {
    useAppStore,
    selectActiveWorkspaceId: () => null,
    useActiveWorkspace: () => null,
    useHomeDir: () => "/home/u",
    projectDisplayName: (p: string) => p.split("/").pop() ?? p,
    useProjectGroupedWorkspaces: (all: WorkspaceSnapshot[]) => {
      const byPath = new Map<string, WorkspaceSnapshot[]>();
      for (const ws of all) byPath.set(ws.project_root || ws.cwd, [...(byPath.get(ws.project_root || ws.cwd) ?? []), ws]);
      return [...byPath.entries()].map(([projectPath, list]) => ({
        projectName: projectPath.split("/").pop()!,
        projectPath,
        workspaces: list,
      }));
    },
  };
});

import { SidebarProjectsView } from "@/components/layout/sidebar-projects-view";
import { __resetSidebarViewStoreForTests, useSidebarViewStore } from "@/components/layout/sidebar-view-store";
import { __resetSidebarInboxStoreForTests } from "@/stores/sidebar-inbox-store";
import { resetPlatformPluginsForTests } from "./platform-adapter";
import { resetPluginSidebarLayoutForTests } from "./plugin-sidebar-layout";

const APP = "/home/u/projects/app";

function workspace(id: string, threadId: string, overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: id,
    title: `Title ${id}`,
    workspace_type: "standard",
    cwd: APP,
    project_root: APP,
    project_uid: "proj-app",
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
    last_active_at: Date.now(),
    last_visited_at: Date.now(),
    tabs: [],
    active_tab_id: "",
    active_surface_id: "s1",
    surfaces: [
      {
        surface_id: "s1",
        title: "",
        active_pane_id: `pane-${id}`,
        root: { kind: "agent_chat", pane_id: `pane-${id}`, title: "", thread_id: threadId, provider: null, cwd: null },
      },
    ] as SurfaceSnapshot[],
    ...overrides,
  };
}

const treePlugin = installation({
  manifest: manifest({ nativeTrees: [{ id: "work", title: "Work" }] }),
  grants: { capabilities: ["ui", "ui.native-trees", "threads.read"], projectIds: ["proj-app"] },
});

async function renderView() {
  const utils = render(
    <TooltipProvider>
      <SidebarProjectsView />
    </TooltipProvider>,
  );
  for (let i = 0; i < 4; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
  return utils;
}

beforeEach(() => {
  __resetSidebarInboxStoreForTests();
  __resetSidebarViewStoreForTests();
  resetPluginSidebarLayoutForTests();
  resetPlatformPluginsForTests({ catalogue: { installations: [treePlugin] }, loaded: true });
  useSidebarViewStore.setState({ loaded: true, view: "projects", pluginLayout: "inst-1/tree:work" });
  persistedSettled = null;
  workspaces = [workspace("ws-1", "t-1"), workspace("ws-2", "t-2")];
});
afterEach(() => cleanup());

describe("plugin trees in the Projects view", () => {
  it("arranges the host's own rows and keeps unplaced chats reachable", async () => {
    const nodes: PluginTreeNode[] = [
      { id: "g", kind: "group", title: "Shipping" },
      { id: "n1", kind: "thread", threadId: "t-1", parentId: "g" },
    ];
    readTree = async () => ({ nodes });
    await renderView();
    const tree = screen.getByRole("region", { name: "Work" });
    const shipping = within(tree).getByRole("button", { name: /Shipping/ }).closest("[data-plugin-tree-section]") as HTMLElement;
    // The host's row: same activation target and hover card as the built-in view.
    expect(within(shipping).getByText("Title ws-1").closest("[data-projects-row]")).toHaveAttribute("data-projects-row", "ws-1");
    const other = within(tree).getByRole("button", { name: /Other chats/ }).closest("[data-plugin-tree-section]") as HTMLElement;
    expect(within(other).getByText("Title ws-2")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Projects" })).toBeNull();
  });

  it("falls back to the built-in grouping with a notice when the tree can't be read", async () => {
    readTree = async () => {
      throw "crashed: worker exited";
    };
    await renderView();
    expect(screen.getByRole("status")).toHaveTextContent("Couldn't load Work. Showing Projects.");
    expect(screen.getByRole("region", { name: "Projects" })).toBeInTheDocument();
    expect(screen.getByText("Title ws-1")).toBeInTheDocument();
  });

  it("shows the built-in grouping while the plugin can't serve the remembered tree", async () => {
    resetPlatformPluginsForTests({ catalogue: { installations: [{ ...treePlugin, enabled: false }] }, loaded: true });
    await renderView();
    expect(screen.queryByRole("region", { name: "Work" })).toBeNull();
    expect(screen.getByRole("region", { name: "Projects" })).toBeInTheDocument();
    // The choice waits for the plugin to come back.
    expect(useSidebarViewStore.getState().pluginLayout).toBe("inst-1/tree:work");
  });

  it("applies a declared view's rules to the host's own grouping", async () => {
    const LIB = "/home/u/projects/lib";
    const viewPlugin = installation({
      manifest: manifest({ sidebarViews: [{ id: "all", title: "All projects", groupBy: "project" }] }),
      grants: { capabilities: ["ui", "ui.sidebar-views"], projectIds: [] },
    });
    workspaces = [
      workspace("ws-1", "t-1"),
      // A quiet project: settled long ago, so the built-in grouping folds it into "N more projects".
      workspace("ws-old", "t-old", { cwd: LIB, project_root: LIB, project_uid: "proj-lib", last_active_at: 1, last_visited_at: 1 }),
    ];
    persistedSettled = JSON.stringify({ settled: [{ id: "ws-old", at: 1 }], snoozed: [], keepActive: [], activity: {} });

    useSidebarViewStore.setState({ pluginLayout: null });
    resetPlatformPluginsForTests({ catalogue: { installations: [viewPlugin] }, loaded: true });
    await renderView();
    expect(screen.getByRole("region", { name: "More projects" })).toBeInTheDocument();
    cleanup();
    __resetSidebarInboxStoreForTests();

    // `quietAfterDays` absent: nothing folds away.
    useSidebarViewStore.setState({ pluginLayout: "inst-1/all" });
    await renderView();
    expect(screen.queryByRole("region", { name: "More projects" })).toBeNull();
    expect(within(screen.getByRole("region", { name: "Projects" })).getByText("lib")).toBeInTheDocument();
  });
});

describe("pluginSidebarLayoutOptions", () => {
  it("offers views only with their grant, and drops rule values from a newer contract", async () => {
    const { pluginSidebarLayoutOptions } = await import("./plugin-sidebar-layout");
    const views = [
      { id: "ok", title: "Ok", groupBy: "project" as const, quietAfterDays: 7 },
      { id: "future", title: "Future", groupBy: "owner" as never },
    ];
    const granted = installation({
      manifest: manifest({ sidebarViews: views }),
      grants: { capabilities: ["ui.sidebar-views"], projectIds: [] },
    });
    expect(pluginSidebarLayoutOptions([granted]).map((option) => option.key)).toEqual(["inst-1/ok"]);
    expect(pluginSidebarLayoutOptions([{ ...granted, grants: { capabilities: [], projectIds: [] } }])).toEqual([]);
  });
});
