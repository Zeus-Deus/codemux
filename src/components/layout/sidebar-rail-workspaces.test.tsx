/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, fireEvent, act, cleanup } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  AppStateSnapshot,
  PaneStatus,
  SurfaceSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

// UI-state persistence for the settled list (plus project appearance keys,
// which resolve to null). Individual tests override the settled key.
let persistedSettled: string | null = null;

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  dbGetUiState: vi.fn((key: string) =>
    Promise.resolve(key === "sidebar.inbox.settled" ? persistedSettled : null),
  ),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => [],
}));

// App store: a plain object driven per-test. The rail derives everything
// (buttons, statuses) from `appState`.
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

import { SidebarRailWorkspaces } from "./sidebar-rail-workspaces";
import {
  __resetSidebarInboxStoreForTests,
  useSidebarInboxStore,
} from "@/stores/sidebar-inbox-store";
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
  return [{ root: { kind: "pane", pane_id: paneId } }] as unknown as SurfaceSnapshot[];
}

async function renderRail() {
  const utils = render(
    <TooltipProvider>
      <SidebarRailWorkspaces />
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

describe("SidebarRailWorkspaces", () => {
  it("renders one button per active workspace, in appState order", async () => {
    workspaces = [
      makeWorkspace({ title: "Alpha" }),
      makeWorkspace({ title: "Beta" }),
      makeWorkspace({ title: "Gamma" }),
    ];
    const { container } = await renderRail();

    const buttons = [...container.querySelectorAll("[data-rail-ws]")];
    expect(buttons.map((b) => b.getAttribute("data-rail-ws"))).toEqual([
      "ws-1",
      "ws-2",
      "ws-3",
    ]);
    expect(buttons.map((b) => b.getAttribute("aria-label"))).toEqual([
      "Alpha",
      "Beta",
      "Gamma",
    ]);
  });

  it("excludes settled workspaces (repo filter never applies)", async () => {
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-2", at: Date.now() }],
      keepActive: [],
      activity: {},
    });
    workspaces = [
      makeWorkspace({ title: "Active one" }),
      makeWorkspace({ title: "Settled one" }),
      makeWorkspace({ title: "Active two" }),
    ];
    const { container } = await renderRail();

    const ids = [...container.querySelectorAll("[data-rail-ws]")].map((b) =>
      b.getAttribute("data-rail-ws"),
    );
    expect(ids).toEqual(["ws-1", "ws-3"]);
  });

  it("excludes snoozed workspaces the same way it excludes settled ones", async () => {
    // A snooze that still leaves a rail button is the gesture undone: the
    // deferred work is out of the inbox but back in the user's face.
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-2", at: Date.now() }],
      snoozed: [{ id: "ws-3", at: Date.now(), until: Date.now() + 3_600_000 }],
      keepActive: [],
      activity: {},
    });
    workspaces = [
      makeWorkspace({ title: "Active" }),
      makeWorkspace({ title: "Settled" }),
      makeWorkspace({ title: "Snoozed" }),
      makeWorkspace({ title: "Also active" }),
    ];
    const { container } = await renderRail();

    const ids = [...container.querySelectorAll("[data-rail-ws]")].map((b) =>
      b.getAttribute("data-rail-ws"),
    );
    expect(ids).toEqual(["ws-1", "ws-4"]);
  });

  it("keeps the currently-open workspace visible even while it is parked", async () => {
    // Matches the expanded inbox's forced-visible row: the button's selection
    // fill is the collapsed sidebar's only "you are here".
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-2", at: Date.now() }],
      snoozed: [{ id: "ws-3", at: Date.now(), until: Date.now() + 3_600_000 }],
      keepActive: [],
      activity: {},
    });
    workspaces = [
      makeWorkspace({ title: "Active" }),
      makeWorkspace({ title: "Settled" }),
      makeWorkspace({ title: "Snoozed and open" }),
    ];
    activeWorkspaceId = "ws-3";
    const { container } = await renderRail();

    const ids = [...container.querySelectorAll("[data-rail-ws]")].map((b) =>
      b.getAttribute("data-rail-ws"),
    );
    expect(ids).toEqual(["ws-1", "ws-3"]);
    expect(container.querySelector('[data-rail-ws="ws-3"]')).toHaveClass(
      "bg-foreground/[0.09]",
    );
  });

  it("shows a per-workspace status dot (needs-you / working / review), none when idle", async () => {
    workspaces = [
      makeWorkspace({ title: "Needs", surfaces: surfaceWithPane("p1") }),
      makeWorkspace({ title: "Working", surfaces: surfaceWithPane("p2") }),
      makeWorkspace({ title: "Review", surfaces: surfaceWithPane("p3") }),
      makeWorkspace({ title: "Idle" }),
    ];
    paneStatuses = { p1: "permission", p2: "working", p3: "review" };
    const { container } = await renderRail();

    const dotIn = (id: string) =>
      container
        .querySelector(`[data-rail-ws="${id}"]`)!
        .querySelector("span.rounded-full");

    expect(dotIn("ws-1")).toHaveClass("bg-status-attention", "animate-pulse");
    expect(dotIn("ws-2")).toHaveClass("bg-status-working");
    expect(dotIn("ws-3")).toHaveClass("bg-status-open");
    // Idle workspace carries no dot.
    expect(dotIn("ws-4")).toBeNull();
  });

  it("marks the active workspace's button with the neutral selection fill", async () => {
    workspaces = [
      makeWorkspace({ title: "Alpha" }),
      makeWorkspace({ title: "Beta" }),
    ];
    activeWorkspaceId = "ws-2";
    const { container } = await renderRail();

    const active = container.querySelector('[data-rail-ws="ws-2"]')!;
    const inactive = container.querySelector('[data-rail-ws="ws-1"]')!;
    expect(active).toHaveClass("border-border", "bg-foreground/[0.09]");
    expect(inactive).not.toHaveClass("bg-foreground/[0.09]");
    // Selection is neutral now — no ember on any rail button.
    for (const btn of container.querySelectorAll("[data-rail-ws]")) {
      expect(btn.className).not.toMatch(/accent-ember/);
    }
  });

  it("prunes persisted entries whose workspace no longer exists", async () => {
    // A session spent entirely in the collapsed rail must still trim the
    // persisted blob, otherwise it grows without bound.
    persistedSettled = JSON.stringify({
      settled: [{ id: "ws-gone", at: Date.now() }],
      keepActive: ["ws-gone"],
      activity: { "ws-gone": 1_000 },
    });
    workspaces = [makeWorkspace({ title: "Alpha" })];
    await renderRail();
    await act(async () => {
      await Promise.resolve();
    });

    const state = useSidebarInboxStore.getState();
    expect(state.settled).toEqual([]);
    expect(state.keepActive).toEqual({});
    expect(state.activity).toEqual({});
  });

  it("clicking a button activates its workspace", async () => {
    workspaces = [makeWorkspace({ title: "Open me" })];
    const { container } = await renderRail();

    fireEvent.click(container.querySelector('[data-rail-ws="ws-1"]')!);
    expect(activateWorkspace).toHaveBeenCalledWith("ws-1");
  });
});
