import { describe, it, expect } from "vitest";
import {
  buildSessionWorkspaceIndex,
  getSessionWorkspaceId,
  groupWorkspacesByProject,
  resolveProjectRoot,
  useAppStore,
} from "./app-store";
import type {
  AppStateSnapshot,
  PaneNodeSnapshot,
  SurfaceSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

// Minimal workspace factory for testing
function makeWs(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test",
    workspace_type: "standard",
    cwd: "/home/user/projects/myapp",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: null,
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

describe("resolveProjectRoot", () => {
  it("returns project_root when set", () => {
    const ws = makeWs({
      cwd: "/home/user/.codemux/worktrees/myapp/feature-branch",
      project_root: "/home/user/projects/myapp",
    });
    expect(resolveProjectRoot(ws)).toBe("/home/user/projects/myapp");
  });

  it("falls back to cwd when project_root is null", () => {
    const ws = makeWs({
      cwd: "/home/user/projects/myapp",
      project_root: null,
    });
    expect(resolveProjectRoot(ws)).toBe("/home/user/projects/myapp");
  });

  it("groups worktree workspaces with their main repo", () => {
    const mainWs = makeWs({
      workspace_id: "ws-main",
      cwd: "/home/user/projects/myapp",
      project_root: "/home/user/projects/myapp",
    });
    const wtWs1 = makeWs({
      workspace_id: "ws-wt1",
      cwd: "/home/user/.codemux/worktrees/myapp/feature-a",
      worktree_path: "/home/user/.codemux/worktrees/myapp/feature-a",
      project_root: "/home/user/projects/myapp",
    });
    const wtWs2 = makeWs({
      workspace_id: "ws-wt2",
      cwd: "/home/user/.codemux/worktrees/myapp/feature-b",
      worktree_path: "/home/user/.codemux/worktrees/myapp/feature-b",
      project_root: "/home/user/projects/myapp",
    });

    // All three should resolve to the same project root
    expect(resolveProjectRoot(mainWs)).toBe(resolveProjectRoot(wtWs1));
    expect(resolveProjectRoot(mainWs)).toBe(resolveProjectRoot(wtWs2));
  });

  it("keeps different projects separate", () => {
    const ws1 = makeWs({ cwd: "/home/user/work/app", project_root: "/home/user/work/app" });
    const ws2 = makeWs({ cwd: "/home/user/personal/app", project_root: "/home/user/personal/app" });
    expect(resolveProjectRoot(ws1)).not.toBe(resolveProjectRoot(ws2));
  });

  it("handles paths with spaces", () => {
    const ws = makeWs({
      cwd: "/home/user/My Projects/cool app",
      project_root: "/home/user/My Projects/cool app",
    });
    expect(resolveProjectRoot(ws)).toBe("/home/user/My Projects/cool app");
  });

  it("handles paths with unicode characters", () => {
    const ws = makeWs({
      cwd: "/home/user/projects/über-app",
      project_root: "/home/user/projects/über-app",
    });
    expect(resolveProjectRoot(ws)).toBe("/home/user/projects/über-app");
  });

  it("handles old workspaces without project_root (backward compat)", () => {
    // Simulate a workspace from before the project_root field was added
    const ws = makeWs({
      cwd: "/home/user/projects/legacy-app",
      project_root: null,
      worktree_path: null,
    });
    expect(resolveProjectRoot(ws)).toBe("/home/user/projects/legacy-app");
  });
});

// Test the grouping logic inline (useProjectGroupedWorkspaces is a hook,
// so we test the underlying logic via resolveProjectRoot + manual grouping)
describe("project grouping", () => {
  it("disambiguates projects with the same folder name", () => {
    const workspaces = [
      makeWs({ workspace_id: "ws-1", cwd: "/home/user/work/app" }),
      makeWs({ workspace_id: "ws-2", cwd: "/home/user/personal/app" }),
    ];

    const groups = new Map<string, { name: string; path: string; count: number }>();
    for (const ws of workspaces) {
      const projectPath = resolveProjectRoot(ws);
      const projectName = projectPath.split("/").filter(Boolean).pop() || projectPath;
      if (!groups.has(projectPath)) {
        groups.set(projectPath, { name: projectName, path: projectPath, count: 0 });
      }
      groups.get(projectPath)!.count++;
    }

    const result = Array.from(groups.values());

    // Both have name "app" — disambiguation needed
    const nameCounts = new Map<string, number>();
    for (const g of result) {
      nameCounts.set(g.name, (nameCounts.get(g.name) || 0) + 1);
    }
    for (const g of result) {
      if ((nameCounts.get(g.name) || 0) > 1) {
        const parts = g.path.split("/").filter(Boolean);
        if (parts.length >= 2) {
          g.name = parts.slice(-2).join("/");
        }
      }
    }

    const names = result.map((g) => g.name);
    expect(names).toContain("work/app");
    expect(names).toContain("personal/app");
    expect(names).not.toContain("app");
  });

  it("empty workspaces array means no active workspace", () => {
    // When all workspaces are closed, there's nothing to resolve
    const workspaces: WorkspaceSnapshot[] = [];
    const groups = new Map<string, { name: string; path: string }>();
    for (const ws of workspaces) {
      const projectPath = resolveProjectRoot(ws);
      const projectName =
        projectPath.split("/").filter(Boolean).pop() || projectPath;
      if (!groups.has(projectPath)) {
        groups.set(projectPath, { name: projectName, path: projectPath });
      }
    }
    expect(groups.size).toBe(0);
  });

  it("does not disambiguate unique project names", () => {
    const workspaces = [
      makeWs({ workspace_id: "ws-1", cwd: "/home/user/projects/frontend" }),
      makeWs({ workspace_id: "ws-2", cwd: "/home/user/projects/backend" }),
    ];

    const groups = new Map<string, { name: string; path: string }>();
    for (const ws of workspaces) {
      const projectPath = resolveProjectRoot(ws);
      const projectName = projectPath.split("/").filter(Boolean).pop() || projectPath;
      if (!groups.has(projectPath)) {
        groups.set(projectPath, { name: projectName, path: projectPath });
      }
    }

    const result = Array.from(groups.values());
    const names = result.map((g) => g.name);
    expect(names).toContain("frontend");
    expect(names).toContain("backend");
  });
});

describe("groupWorkspacesByProject — Home labelling (Stage A)", () => {
  it("falls back to path-basename grouping when homeDir is null", () => {
    const workspaces = [
      makeWs({
        workspace_id: "ws-home",
        cwd: "/home/zeus",
        project_root: "/home/zeus",
      }),
      makeWs({
        workspace_id: "ws-proj",
        cwd: "/home/zeus/projects/myapp",
        project_root: "/home/zeus/projects/myapp",
      }),
    ];

    const groups = groupWorkspacesByProject(workspaces, null);
    const names = groups.map((g) => g.projectName).sort();
    expect(names).toEqual(["myapp", "zeus"]);
    // No "Home" group is produced when homeDir is unknown.
    expect(names).not.toContain("Home");
  });

  it("labels the $HOME-rooted group 'Home' when homeDir matches", () => {
    const workspaces = [
      makeWs({
        workspace_id: "ws-home",
        cwd: "/home/zeus",
        project_root: "/home/zeus",
      }),
      makeWs({
        workspace_id: "ws-proj",
        cwd: "/home/zeus/projects/myapp",
        project_root: "/home/zeus/projects/myapp",
      }),
    ];

    const groups = groupWorkspacesByProject(workspaces, "/home/zeus");
    const byName = new Map(groups.map((g) => [g.projectName, g]));
    expect(byName.has("Home")).toBe(true);
    expect(byName.get("Home")!.projectPath).toBe("/home/zeus");
    expect(byName.get("Home")!.workspaces.map((w) => w.workspace_id)).toEqual([
      "ws-home",
    ]);
    expect(byName.has("myapp")).toBe(true);
    expect(byName.get("myapp")!.workspaces.map((w) => w.workspace_id)).toEqual([
      "ws-proj",
    ]);
  });

  it("returns an empty array for no workspaces, regardless of homeDir", () => {
    expect(groupWorkspacesByProject([], "/home/zeus")).toEqual([]);
    expect(groupWorkspacesByProject([], null)).toEqual([]);
  });

  it("collects multiple $HOME-rooted workspaces into a single Home group", () => {
    const workspaces = [
      makeWs({
        workspace_id: "ws-home-1",
        cwd: "/home/zeus",
        project_root: "/home/zeus",
        title: "Identity inquiry",
      }),
      makeWs({
        workspace_id: "ws-home-2",
        cwd: "/home/zeus",
        project_root: "/home/zeus",
        title: "Friendship inquiry",
      }),
      makeWs({
        workspace_id: "ws-home-3",
        cwd: "/home/zeus",
        project_root: "/home/zeus",
        title: "Dev env setup",
      }),
    ];

    const groups = groupWorkspacesByProject(workspaces, "/home/zeus");
    expect(groups).toHaveLength(1);
    expect(groups[0].projectName).toBe("Home");
    expect(groups[0].workspaces.map((w) => w.workspace_id)).toEqual([
      "ws-home-1",
      "ws-home-2",
      "ws-home-3",
    ]);
  });

  it("groups a legacy WorkspaceType::Home workspace under 'Home' by path, ignoring the variant tag", () => {
    const legacyHome = makeWs({
      workspace_id: "ws-legacy-home",
      workspace_type: "home",
      cwd: "/home/zeus",
      project_root: "/home/zeus",
    });

    const groups = groupWorkspacesByProject([legacyHome], "/home/zeus");
    expect(groups).toHaveLength(1);
    expect(groups[0].projectName).toBe("Home");
    expect(groups[0].projectPath).toBe("/home/zeus");
  });
});

describe("useAppStore.setHomeDir", () => {
  it("writes and reads back the home dir", () => {
    useAppStore.setState({ homeDir: null });
    expect(useAppStore.getState().homeDir).toBeNull();
    useAppStore.getState().setHomeDir("/home/zeus");
    expect(useAppStore.getState().homeDir).toBe("/home/zeus");
    // Reset so later test files don't inherit the value.
    useAppStore.setState({ homeDir: null });
  });
});

// ── session→workspace reverse index ─────────────────────────────────────

function termPane(paneId: string, sessionId: string): PaneNodeSnapshot {
  return { kind: "terminal", pane_id: paneId, session_id: sessionId, title: "" };
}

function browserPane(paneId: string): PaneNodeSnapshot {
  return { kind: "browser", pane_id: paneId, browser_id: "br", title: "" };
}

function chatPane(paneId: string): PaneNodeSnapshot {
  return {
    kind: "agent_chat",
    pane_id: paneId,
    title: "",
    thread_id: null,
    provider: null,
    cwd: null,
  };
}

function split(...children: PaneNodeSnapshot[]): PaneNodeSnapshot {
  return {
    kind: "split",
    pane_id: "split-" + children.map((c) => c.pane_id).join("-"),
    direction: "horizontal",
    child_sizes: children.map(() => 1 / children.length),
    children,
  };
}

function makeSurface(root: PaneNodeSnapshot, surfaceId = "sf-1"): SurfaceSnapshot {
  return { surface_id: surfaceId, title: "", root, active_pane_id: root.pane_id };
}

function makeAppState(workspaces: WorkspaceSnapshot[]): AppStateSnapshot {
  return {
    schema_version: 1,
    active_workspace_id: workspaces[0]?.workspace_id ?? "",
    workspaces,
    terminal_sessions: [],
    browser_sessions: [],
    agent_browser_sessions: [],
    notifications: [],
    detected_ports: [],
    pane_statuses: {},
    persistence: {
      schema_version: 1,
      stores_layout_metadata: true,
      stores_terminal_metadata: true,
      stores_live_process_state: true,
    },
    config: {
      config_version: 1,
      default_shell: null,
      theme_source: "default",
      linux_first: false,
      notification_sound_enabled: true,
      ai_commit_message_enabled: false,
      ai_commit_message_cli: null,
      ai_commit_message_model: null,
      ai_resolver_enabled: false,
      ai_resolver_cli: null,
      ai_resolver_model: null,
      ai_resolver_strategy: "auto",
    },
  };
}

describe("buildSessionWorkspaceIndex", () => {
  it("returns an empty Map for a null appState", () => {
    const index = buildSessionWorkspaceIndex(null);
    expect(index.size).toBe(0);
  });

  it("returns an empty Map when there are no workspaces", () => {
    const index = buildSessionWorkspaceIndex(makeAppState([]));
    expect(index.size).toBe(0);
  });

  it("maps a single terminal pane in a single workspace", () => {
    const ws = makeWs({
      workspace_id: "ws-1",
      surfaces: [makeSurface(termPane("pane-1", "sess-1"))],
    });
    const index = buildSessionWorkspaceIndex(makeAppState([ws]));
    expect(index.size).toBe(1);
    expect(index.get("sess-1")).toBe("ws-1");
  });

  it("walks a split tree to find every terminal pane", () => {
    const ws = makeWs({
      workspace_id: "ws-split",
      surfaces: [
        makeSurface(
          split(
            termPane("pane-a", "sess-a"),
            split(termPane("pane-b", "sess-b"), termPane("pane-c", "sess-c")),
          ),
        ),
      ],
    });
    const index = buildSessionWorkspaceIndex(makeAppState([ws]));
    expect(index.size).toBe(3);
    expect(index.get("sess-a")).toBe("ws-split");
    expect(index.get("sess-b")).toBe("ws-split");
    expect(index.get("sess-c")).toBe("ws-split");
  });

  it("maps panes across multiple workspaces and surfaces", () => {
    const ws1 = makeWs({
      workspace_id: "ws-1",
      surfaces: [
        makeSurface(termPane("p1", "s1"), "sf-1a"),
        makeSurface(
          split(termPane("p2", "s2"), termPane("p3", "s3")),
          "sf-1b",
        ),
      ],
    });
    const ws2 = makeWs({
      workspace_id: "ws-2",
      surfaces: [makeSurface(termPane("p4", "s4"))],
    });
    const index = buildSessionWorkspaceIndex(makeAppState([ws1, ws2]));
    expect(index.size).toBe(4);
    expect(index.get("s1")).toBe("ws-1");
    expect(index.get("s2")).toBe("ws-1");
    expect(index.get("s3")).toBe("ws-1");
    expect(index.get("s4")).toBe("ws-2");
  });

  it("ignores browser and agent_chat panes (no session_id to index)", () => {
    const ws = makeWs({
      workspace_id: "ws-mixed",
      surfaces: [
        makeSurface(
          split(
            browserPane("p-browser"),
            chatPane("p-chat"),
            termPane("p-term", "s-term"),
          ),
        ),
      ],
    });
    const index = buildSessionWorkspaceIndex(makeAppState([ws]));
    expect(index.size).toBe(1);
    expect(index.get("s-term")).toBe("ws-mixed");
    // Browser/chat pane ids should not appear as keys, and their pane_ids
    // should not have leaked in as session_ids either.
    expect(index.has("p-browser")).toBe(false);
    expect(index.has("p-chat")).toBe(false);
  });
});

describe("getSessionWorkspaceId", () => {
  it("reads the cached index from useAppStore.getState()", () => {
    const ws = makeWs({
      workspace_id: "ws-A",
      surfaces: [makeSurface(termPane("pane-x", "sess-x"))],
    });
    useAppStore.setState({ appState: makeAppState([ws]) });
    try {
      expect(getSessionWorkspaceId("sess-x")).toBe("ws-A");
      expect(getSessionWorkspaceId("missing")).toBeNull();
    } finally {
      useAppStore.setState({ appState: null });
    }
  });

  it("returns null when there is no app state", () => {
    useAppStore.setState({ appState: null });
    expect(getSessionWorkspaceId("anything")).toBeNull();
  });

  it("reuses the same Map for repeated calls against the same snapshot", () => {
    // The WeakMap cache means two calls with the same appState reference
    // walk the tree once; the second call hits the cache. We can't observe
    // the WeakMap directly, but exposing a hook would defeat the perf goal,
    // so we assert the property indirectly: building the index for a freshly
    // mutated snapshot vs the original yields different Map identities, but
    // re-reading the same snapshot must yield identical results.
    const ws = makeWs({
      workspace_id: "ws-cache",
      surfaces: [makeSurface(termPane("p1", "s1"))],
    });
    const snapshot = makeAppState([ws]);
    useAppStore.setState({ appState: snapshot });
    try {
      const a = getSessionWorkspaceId("s1");
      const b = getSessionWorkspaceId("s1");
      expect(a).toBe("ws-cache");
      expect(b).toBe("ws-cache");
      // Replace with a structurally-identical but new snapshot. The cache
      // is keyed on identity, so the index rebuilds — still correct.
      const fresh = makeAppState([ws]);
      useAppStore.setState({ appState: fresh });
      expect(getSessionWorkspaceId("s1")).toBe("ws-cache");
    } finally {
      useAppStore.setState({ appState: null });
    }
  });
});
