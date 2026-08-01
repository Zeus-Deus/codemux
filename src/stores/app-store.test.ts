import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  applyDeltaToSnapshot,
  buildSessionWorkspaceIndex,
  DELTA_BUFFER_LIMIT,
  getSessionWorkspaceId,
  groupWorkspacesByProject,
  resolveProjectRoot,
  selectActiveWorkspaceId,
  useAppStore,
} from "./app-store";
import type {
  AppStateDelta,
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
    notifications_muted: false,
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

  it("extracts the basename from Windows backslash paths", () => {
    const workspaces = [
      makeWs({ workspace_id: "ws-1", project_root: "C:\\Users\\muso4\\projects\\codemux" }),
      makeWs({ workspace_id: "ws-2", project_root: "C:\\Users\\muso4\\projects\\codemux" }),
    ];

    const groups = groupWorkspacesByProject(workspaces, null);

    expect(groups).toHaveLength(1);
    expect(groups[0].projectName).toBe("codemux");
    expect(groups[0].projectPath).toBe("C:\\Users\\muso4\\projects\\codemux");
  });

  it("disambiguates duplicate Windows project basenames", () => {
    const workspaces = [
      makeWs({ workspace_id: "ws-1", project_root: "C:\\Users\\muso4\\work\\app" }),
      makeWs({ workspace_id: "ws-2", project_root: "C:\\Users\\muso4\\personal\\app" }),
    ];

    const groups = groupWorkspacesByProject(workspaces, null);

    const names = groups.map((g) => g.projectName).sort();
    expect(names).toEqual(["personal/app", "work/app"]);
  });

  it("grows the path tail until duplicate labels are actually unique", () => {
    // Both roots end in the same two segments (`projects/app`), so a
    // fixed 2-segment tail would leave them identical. The tail must
    // grow to three segments to disambiguate.
    const workspaces = [
      makeWs({ workspace_id: "ws-1", project_root: "/home/alice/projects/app" }),
      makeWs({ workspace_id: "ws-2", project_root: "/home/bob/projects/app" }),
    ];

    const groups = groupWorkspacesByProject(workspaces, null);

    const names = groups.map((g) => g.projectName).sort();
    expect(names).toEqual(["alice/projects/app", "bob/projects/app"]);
    // Crucially, the broken identical "projects/app" label never appears.
    expect(names).not.toContain("projects/app");
  });

  it("tags the remote copy with its host and keeps the local copy clean", () => {
    // Same repo basename on two machines: local `/home/zeus/...` and the
    // remote `/home/deus/...` on host id 7 ("pandora"). Local must stay
    // "partpilot"; the remote gets a " · pandora" suffix.
    const workspaces = [
      makeWs({
        workspace_id: "ws-local",
        project_root: "/home/zeus/projects/partpilot",
        host_id: null,
      }),
      makeWs({
        workspace_id: "ws-remote",
        project_root: "/home/deus/projects/partpilot",
        host_id: 7,
      }),
    ];

    const groups = groupWorkspacesByProject(
      workspaces,
      null,
      new Map([[7, "pandora"]]),
    );

    const byName = new Map(groups.map((g) => [g.projectName, g]));
    expect(byName.has("partpilot")).toBe(true);
    expect(byName.get("partpilot")!.projectPath).toBe(
      "/home/zeus/projects/partpilot",
    );
    expect(byName.has("partpilot · pandora")).toBe(true);
    expect(byName.get("partpilot · pandora")!.projectPath).toBe(
      "/home/deus/projects/partpilot",
    );
    // The old broken "projects/partpilot" label is gone.
    expect(groups.map((g) => g.projectName)).not.toContain(
      "projects/partpilot",
    );
  });

  it("falls back to path tails when host names are unavailable", () => {
    // host_id is set but no name map is provided — disambiguation can't
    // use the host, so it must still produce unique path-based labels.
    const workspaces = [
      makeWs({
        workspace_id: "ws-local",
        project_root: "/home/zeus/projects/partpilot",
        host_id: null,
      }),
      makeWs({
        workspace_id: "ws-remote",
        project_root: "/home/deus/projects/partpilot",
        host_id: 7,
      }),
    ];

    const groups = groupWorkspacesByProject(workspaces, null);

    const names = groups.map((g) => g.projectName).sort();
    expect(names).toEqual(["deus/projects/partpilot", "zeus/projects/partpilot"]);
  });

  it("disambiguates two same-named projects on the same remote host by path", () => {
    // Both remote on host 7: host alone can't tell them apart, so we
    // grow the path tail. Neither should keep a bare "app" label.
    const workspaces = [
      makeWs({
        workspace_id: "ws-1",
        project_root: "/home/deus/work/app",
        host_id: 7,
      }),
      makeWs({
        workspace_id: "ws-2",
        project_root: "/home/deus/personal/app",
        host_id: 7,
      }),
    ];

    const groups = groupWorkspacesByProject(
      workspaces,
      null,
      new Map([[7, "pandora"]]),
    );

    const names = groups.map((g) => g.projectName).sort();
    expect(names).toEqual(["personal/app", "work/app"]);
  });
});

describe("groupWorkspacesByProject — Home labelling (Stage A)", () => {
  it("falls back to path-basename grouping when homeDir is null", () => {
    const workspaces = [
      makeWs({
        workspace_id: "ws-home",
        cwd: "/home/user",
        project_root: "/home/user",
      }),
      makeWs({
        workspace_id: "ws-proj",
        cwd: "/home/user/projects/myapp",
        project_root: "/home/user/projects/myapp",
      }),
    ];

    const groups = groupWorkspacesByProject(workspaces, null);
    const names = groups.map((g) => g.projectName).sort();
    // path-basename grouping derives "user" from the basename of
    // "/home/user" — kept generic so this fixture stays portable.
    expect(names).toEqual(["myapp", "user"]);
    // No "Home" group is produced when homeDir is unknown.
    expect(names).not.toContain("Home");
  });

  it("labels the $HOME-rooted group 'Home' when homeDir matches", () => {
    const workspaces = [
      makeWs({
        workspace_id: "ws-home",
        cwd: "/home/user",
        project_root: "/home/user",
      }),
      makeWs({
        workspace_id: "ws-proj",
        cwd: "/home/user/projects/myapp",
        project_root: "/home/user/projects/myapp",
      }),
    ];

    const groups = groupWorkspacesByProject(workspaces, "/home/user");
    const byName = new Map(groups.map((g) => [g.projectName, g]));
    expect(byName.has("Home")).toBe(true);
    expect(byName.get("Home")!.projectPath).toBe("/home/user");
    expect(byName.get("Home")!.workspaces.map((w) => w.workspace_id)).toEqual([
      "ws-home",
    ]);
    expect(byName.has("myapp")).toBe(true);
    expect(byName.get("myapp")!.workspaces.map((w) => w.workspace_id)).toEqual([
      "ws-proj",
    ]);
  });

  it("returns an empty array for no workspaces, regardless of homeDir", () => {
    expect(groupWorkspacesByProject([], "/home/user")).toEqual([]);
    expect(groupWorkspacesByProject([], null)).toEqual([]);
  });

  it("collects multiple $HOME-rooted workspaces into a single Home group", () => {
    const workspaces = [
      makeWs({
        workspace_id: "ws-home-1",
        cwd: "/home/user",
        project_root: "/home/user",
        title: "Identity inquiry",
      }),
      makeWs({
        workspace_id: "ws-home-2",
        cwd: "/home/user",
        project_root: "/home/user",
        title: "Friendship inquiry",
      }),
      makeWs({
        workspace_id: "ws-home-3",
        cwd: "/home/user",
        project_root: "/home/user",
        title: "Dev env setup",
      }),
    ];

    const groups = groupWorkspacesByProject(workspaces, "/home/user");
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
      cwd: "/home/user",
      project_root: "/home/user",
    });

    const groups = groupWorkspacesByProject([legacyHome], "/home/user");
    expect(groups).toHaveLength(1);
    expect(groups[0].projectName).toBe("Home");
    expect(groups[0].projectPath).toBe("/home/user");
  });
});

describe("useAppStore.setHomeDir", () => {
  it("writes and reads back the home dir", () => {
    useAppStore.setState({ homeDir: null });
    expect(useAppStore.getState().homeDir).toBeNull();
    useAppStore.getState().setHomeDir("/home/user");
    expect(useAppStore.getState().homeDir).toBe("/home/user");
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

describe("useAppStore.setAppState — structural sharing identity", () => {
  it("returns the previous snapshot ref when a deep-equal snapshot arrives", () => {
    const ws = makeWs({
      workspace_id: "ws-1",
      surfaces: [makeSurface(termPane("p1", "s1"))],
    });
    const a = makeAppState([ws]);
    useAppStore.setState({ appState: null });
    try {
      useAppStore.getState().setAppState(a);
      const first = useAppStore.getState().appState;
      expect(first).toBe(a); // first store has no prev → stored as-is.

      // A fresh-ref deep clone simulates the Rust snapshot rebuild over IPC.
      const clone = JSON.parse(JSON.stringify(a)) as AppStateSnapshot;
      expect(clone).not.toBe(a);
      useAppStore.getState().setAppState(clone);
      // Nothing changed → same top-level ref as before, zero fan-out.
      expect(useAppStore.getState().appState).toBe(first);
    } finally {
      useAppStore.setState({ appState: null });
    }
  });

  it("gives a new top ref but keeps unchanged workspace identity on change", () => {
    const wsA = makeWs({
      workspace_id: "ws-A",
      surfaces: [makeSurface(termPane("pa", "sa"))],
    });
    const wsB = makeWs({
      workspace_id: "ws-B",
      surfaces: [makeSurface(termPane("pb", "sb"))],
    });
    const first = makeAppState([wsA, wsB]);
    useAppStore.setState({ appState: null });
    try {
      useAppStore.getState().setAppState(first);
      const prev = useAppStore.getState().appState!;

      // Fresh-ref clone with only ws-A's pane_statuses changed.
      const changed = JSON.parse(JSON.stringify(first)) as AppStateSnapshot;
      changed.pane_statuses = { pa: "working" };
      useAppStore.getState().setAppState(changed);
      const next = useAppStore.getState().appState!;

      expect(next).not.toBe(prev); // top ref is new
      // Unchanged workspaces keep prev identity.
      expect(next.workspaces).toBe(prev.workspaces);
      expect(next.workspaces[0]).toBe(prev.workspaces[0]);
      expect(next.workspaces[1]).toBe(prev.workspaces[1]);
      expect(next.pane_statuses).toEqual({ pa: "working" });
    } finally {
      useAppStore.setState({ appState: null });
    }
  });
});

// ── Optimistic selection (Phase 1) ──────────────────────────────────────

describe("useAppStore — optimistic pending activation", () => {
  const wsA = makeWs({ workspace_id: "ws-A" });
  const wsB = makeWs({ workspace_id: "ws-B" });

  function reset(): void {
    useAppStore.setState({
      appState: null,
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
    });
  }

  beforeEach(reset);
  afterEach(reset);

  it("prefers a pending id whose workspace is already in the snapshot", () => {
    const snapshot = makeAppState([wsA, wsB]);
    snapshot.active_workspace_id = "ws-A";
    useAppStore.setState({ appState: snapshot });

    // Before the click, the snapshot's own active id wins.
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-A");

    useAppStore.getState().beginPendingActivation("ws-B");

    // The optimistic paint: no snapshot has arrived, but selection already
    // reads as ws-B.
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-B");
    expect(useAppStore.getState().pendingActivationAt).not.toBeNull();
  });

  it("ignores a pending id the current snapshot knows nothing about", () => {
    // A just-created workspace has no local data to paint from, so the
    // optimistic path must defer to the backend rather than blank the pane.
    const snapshot = makeAppState([wsA]);
    snapshot.active_workspace_id = "ws-A";
    useAppStore.setState({ appState: snapshot });
    useAppStore.getState().beginPendingActivation("ws-brand-new");

    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-A");
  });

  it("clears the pending id when a snapshot confirms it", () => {
    useAppStore.getState().setAppState(makeAppState([wsA, wsB]));
    useAppStore.getState().beginPendingActivation("ws-B");

    const confirming = makeAppState([wsA, wsB]);
    confirming.active_workspace_id = "ws-B";
    useAppStore.getState().setAppState(confirming);

    expect(useAppStore.getState().pendingActiveWorkspaceId).toBeNull();
    expect(useAppStore.getState().pendingActivationAt).toBeNull();
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-B");
  });

  it("keeps the pending id while a non-confirming snapshot arrives", () => {
    // A background emit built before the activation still says ws-A. It is
    // applied (it may carry fresh git/PR data) but must not un-select ws-B.
    const first = makeAppState([wsA, wsB]);
    first.active_workspace_id = "ws-A";
    useAppStore.getState().setAppState(first);
    useAppStore.getState().beginPendingActivation("ws-B");

    const background = makeAppState([wsA, wsB]);
    background.active_workspace_id = "ws-A";
    background.pane_statuses = { "pane-x": "working" };
    useAppStore.getState().setAppState(background);

    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-B");
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-B");
    expect(useAppStore.getState().appState!.pane_statuses).toEqual({
      "pane-x": "working",
    });
  });

  it("clearPendingActivation rolls selection back to the snapshot", () => {
    const snapshot = makeAppState([wsA, wsB]);
    snapshot.active_workspace_id = "ws-A";
    useAppStore.setState({ appState: snapshot });
    useAppStore.getState().beginPendingActivation("ws-B");
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-B");

    useAppStore.getState().clearPendingActivation("ws-B");

    expect(useAppStore.getState().pendingActiveWorkspaceId).toBeNull();
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-A");
  });

  it("clearPendingActivation scoped to another id leaves the newer selection", () => {
    useAppStore.setState({ appState: makeAppState([wsA, wsB]) });
    useAppStore.getState().beginPendingActivation("ws-B");

    // A late rollback for a superseded activation must not cancel ws-B.
    useAppStore.getState().clearPendingActivation("ws-A");

    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-B");
  });
});

describe("useAppStore.setAppState — snapshot revision ordering", () => {
  const wsA = makeWs({ workspace_id: "ws-A" });
  const wsB = makeWs({ workspace_id: "ws-B" });

  function reset(): void {
    useAppStore.setState({
      appState: null,
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
    });
  }

  beforeEach(reset);
  afterEach(reset);

  it("tracks the highest applied revision", () => {
    const first = makeAppState([wsA]);
    first.snapshot_revision = 7;
    useAppStore.getState().setAppState(first);
    expect(useAppStore.getState().lastSeenRevision).toBe(7);

    const second = makeAppState([wsA, wsB]);
    second.snapshot_revision = 8;
    useAppStore.getState().setAppState(second);
    expect(useAppStore.getState().lastSeenRevision).toBe(8);
    expect(useAppStore.getState().appState!.workspaces).toHaveLength(2);
  });

  it("drops a snapshot at or below the applied revision", () => {
    const applied = makeAppState([wsA, wsB]);
    applied.active_workspace_id = "ws-B";
    applied.snapshot_revision = 10;
    useAppStore.getState().setAppState(applied);

    // An older background emit delivered late still names ws-A as active.
    const stale = makeAppState([wsA, wsB]);
    stale.active_workspace_id = "ws-A";
    stale.snapshot_revision = 9;
    useAppStore.getState().setAppState(stale);

    expect(useAppStore.getState().appState!.active_workspace_id).toBe("ws-B");
    expect(useAppStore.getState().lastSeenRevision).toBe(10);

    // Same revision twice is equally stale.
    const duplicate = makeAppState([wsA, wsB]);
    duplicate.active_workspace_id = "ws-A";
    duplicate.snapshot_revision = 10;
    useAppStore.getState().setAppState(duplicate);
    expect(useAppStore.getState().appState!.active_workspace_id).toBe("ws-B");
  });

  it("always applies an unrevisioned snapshot", () => {
    const applied = makeAppState([wsA, wsB]);
    applied.snapshot_revision = 10;
    useAppStore.getState().setAppState(applied);

    // Restored state / older backends / the dev mock carry no revision.
    const unrevisioned = makeAppState([wsA, wsB]);
    unrevisioned.active_workspace_id = "ws-B";
    useAppStore.getState().setAppState(unrevisioned);

    expect(useAppStore.getState().appState!.active_workspace_id).toBe("ws-B");
    // The high-water mark survives so a genuinely stale revision still drops.
    expect(useAppStore.getState().lastSeenRevision).toBe(10);
  });

  it("does not let a stale snapshot cancel a pending activation", () => {
    const applied = makeAppState([wsA, wsB]);
    applied.active_workspace_id = "ws-A";
    applied.snapshot_revision = 4;
    useAppStore.getState().setAppState(applied);
    useAppStore.getState().beginPendingActivation("ws-B");

    const stale = makeAppState([wsA, wsB]);
    stale.active_workspace_id = "ws-B";
    stale.snapshot_revision = 3;
    useAppStore.getState().setAppState(stale);

    // The stale snapshot never applied, so it cannot count as confirmation.
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-B");
    expect(useAppStore.getState().appState!.active_workspace_id).toBe("ws-A");
  });
});

describe("useAppStore — a backend restart resets the revision baseline", () => {
  // The revision counter is process-lifetime and restarts at 0. A desktop
  // webview dies with the process so it never notices; a web-remote browser
  // survives the restart holding a `lastSeenRevision` from the DEAD process,
  // and every message from the new one then looks stale. The ordering guards
  // discarded all of them and the page sat frozen — showing the pre-restart
  // world — until the new counter climbed past the old number, which on a
  // long-lived session is minutes or never. The instance token is what makes
  // "the counter restarted" distinguishable from "this message is stale".
  const wsA = makeWs({ workspace_id: "ws-A" });
  const wsB = makeWs({ workspace_id: "ws-B" });

  const portsDelta: AppStateDelta = {
    domain: "detected_ports",
    ports: [
      {
        port: 5173,
        pid: 42,
        process_name: "vite",
        workspace_id: null,
        label: null,
        source: null,
      },
    ],
  };

  function reset(): void {
    useAppStore.setState({
      appState: null,
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
      backendInstance: null,
      resyncInFlight: false,
      resyncRequestId: 0,
      deltaBuffer: new Map(),
      gapWindowId: 0,
    });
  }

  function seed(revision: number, instance: string): void {
    const snapshot = makeAppState([wsA, wsB]);
    snapshot.snapshot_revision = revision;
    snapshot.snapshot_instance = instance;
    useAppStore.getState().setAppState(snapshot);
  }

  beforeEach(reset);
  afterEach(reset);

  it("adopts the instance that stamped the snapshot", () => {
    seed(500, "instance-1");
    expect(useAppStore.getState().backendInstance).toBe("instance-1");
    expect(useAppStore.getState().lastSeenRevision).toBe(500);
  });

  it("applies a low-revision snapshot from a NEW instance instead of dropping it", () => {
    // This is the freeze, exactly: the reconnect's reseeded snapshot arrives
    // at revision 3 while the page still holds 500 from the previous process.
    seed(500, "instance-1");

    const reseeded = makeAppState([wsA, wsB]);
    reseeded.active_workspace_id = "ws-B";
    reseeded.snapshot_revision = 3;
    reseeded.snapshot_instance = "instance-2";
    useAppStore.getState().setAppState(reseeded);

    const state = useAppStore.getState();
    expect(state.appState!.active_workspace_id).toBe("ws-B");
    expect(state.lastSeenRevision).toBe(3);
    expect(state.backendInstance).toBe("instance-2");
  });

  it("still drops a stale snapshot from the SAME instance", () => {
    // The restart escape hatch must not weaken the ordering guard it sits in
    // front of — a late background emit from the live process is still stale.
    seed(500, "instance-1");

    const stale = makeAppState([wsA, wsB]);
    stale.active_workspace_id = "ws-B";
    stale.snapshot_revision = 499;
    stale.snapshot_instance = "instance-1";
    useAppStore.getState().setAppState(stale);

    expect(useAppStore.getState().appState!.active_workspace_id).toBe("ws-A");
    expect(useAppStore.getState().lastSeenRevision).toBe(500);
  });

  it("discards deltas buffered against the dead instance's counter", () => {
    // A gap opened just before the restart. Those revisions belong to a
    // process that no longer exists and will never arrive, so holding them
    // would keep a reorder window open against a counter that reset.
    seed(500, "instance-1");
    useAppStore.getState().applyAppStateDelta(505, portsDelta, "instance-1");
    expect(useAppStore.getState().deltaBuffer.size).toBe(1);
    expect(useAppStore.getState().gapWindowId).not.toBe(0);

    const reseeded = makeAppState([wsA, wsB]);
    reseeded.snapshot_revision = 2;
    reseeded.snapshot_instance = "instance-2";
    useAppStore.getState().setAppState(reseeded);

    const state = useAppStore.getState();
    expect(state.deltaBuffer.size).toBe(0);
    expect(state.gapWindowId).toBe(0);
    expect(state.lastSeenRevision).toBe(2);
  });

  it("a delta from a new instance opens a resync rather than patching a dead baseline", () => {
    // If a delta beats the reseeded snapshot, applying it would patch the
    // PREVIOUS process's world with the new one's data — a snapshot that
    // never existed anywhere.
    seed(500, "instance-1");
    const before = useAppStore.getState().appState;

    useAppStore.getState().applyAppStateDelta(1, portsDelta, "instance-2");

    const state = useAppStore.getState();
    expect(state.appState).toBe(before); // untouched
    expect(state.resyncInFlight).toBe(true);
    expect(state.resyncRequestId).toBe(1);
    expect(state.lastSeenRevision).toBe(0);
    expect(state.deltaBuffer.size).toBe(0);
  });

  it("leaves the baseline alone for an unstamped snapshot", () => {
    // A restored layout, the dev mock, or an older backend carries neither a
    // revision nor a token. It must not be read as a restart — that would
    // reset the high-water mark and let a genuinely stale emit back in.
    seed(500, "instance-1");

    const unstamped = makeAppState([wsA, wsB]);
    unstamped.active_workspace_id = "ws-B";
    useAppStore.getState().setAppState(unstamped);

    const state = useAppStore.getState();
    expect(state.appState!.active_workspace_id).toBe("ws-B"); // applied
    expect(state.lastSeenRevision).toBe(500); // but the mark survives
    expect(state.backendInstance).toBe("instance-1");
  });

  it("the first stamped snapshot is not a restart", () => {
    // Boot: no baseline yet, so there is nothing to invalidate.
    const first = makeAppState([wsA]);
    first.snapshot_revision = 9;
    first.snapshot_instance = "instance-1";
    useAppStore.getState().setAppState(first);

    expect(useAppStore.getState().lastSeenRevision).toBe(9);
    expect(useAppStore.getState().resyncRequestId).toBe(0);
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

// ── Domain deltas (Phase 6) ─────────────────────────────────────────────

describe("applyDeltaToSnapshot", () => {
  const wsA = makeWs({ workspace_id: "ws-A", git_branch: "main" });
  const wsB = makeWs({ workspace_id: "ws-B", git_branch: "other" });

  it("replaces only the named workspace and keeps every other reference", () => {
    const before = makeAppState([wsA, wsB]);
    const after = applyDeltaToSnapshot(
      before,
      {
        domain: "workspace_git",
        workspace_id: "ws-A",
        git: {
          is_git: true,
          git_branch: "feature",
          git_ahead: 2,
          git_behind: 0,
          git_additions: 0,
          git_deletions: 0,
          git_changed_files: 0,
        },
      },
      9,
    );

    expect(after).not.toBe(before);
    expect(after.workspaces).not.toBe(before.workspaces);
    // The whole point: the untouched workspace keeps its identity, so the
    // memoized card for it bails out.
    expect(after.workspaces[1]).toBe(before.workspaces[1]);
    expect(after.workspaces[0]).not.toBe(before.workspaces[0]);
    expect(after.workspaces[0].git_branch).toBe("feature");
    expect(after.workspaces[0].git_ahead).toBe(2);
    // Domains the delta didn't name keep their references too.
    expect(after.pane_statuses).toBe(before.pane_statuses);
    expect(after.detected_ports).toBe(before.detected_ports);
    expect(after.config).toBe(before.config);
    expect(after.snapshot_revision).toBe(9);
    // Immutability: the input snapshot is untouched.
    expect(before.workspaces[0].git_branch).toBe("main");
  });

  it("keeps nested workspace subtrees by reference", () => {
    const surfaces = [makeSurface(termPane("p1", "s1"))];
    const ws = makeWs({ workspace_id: "ws-A", surfaces });
    const before = makeAppState([ws]);
    const after = applyDeltaToSnapshot(
      before,
      {
        domain: "workspace_git",
        workspace_id: "ws-A",
        git: {
          is_git: true,
          git_branch: "feature",
          git_ahead: 0,
          git_behind: 0,
          git_additions: 0,
          git_deletions: 0,
          git_changed_files: 0,
        },
      },
      2,
    );
    expect(after.workspaces[0].surfaces).toBe(surfaces);
  });

  it("returns the same snapshot for a no-op delta", () => {
    const before = makeAppState([wsA]);
    const after = applyDeltaToSnapshot(
      before,
      { domain: "detected_ports", ports: before.detected_ports },
      4,
    );
    expect(after).toBe(before);
  });

  it("ignores a delta for a workspace it doesn't have", () => {
    const before = makeAppState([wsA]);
    const after = applyDeltaToSnapshot(
      before,
      {
        domain: "workspace_git",
        workspace_id: "ws-gone",
        git: {
          is_git: true,
          git_branch: "x",
          git_ahead: 0,
          git_behind: 0,
          git_additions: 0,
          git_deletions: 0,
          git_changed_files: 0,
        },
      },
      5,
    );
    expect(after).toBe(before);
  });

  it("sets and clears a pane status without touching the workspaces", () => {
    const before = makeAppState([wsA]);
    const set = applyDeltaToSnapshot(
      before,
      { domain: "pane_status", pane_id: "pane-1", status: "working" },
      1,
    );
    expect(set.pane_statuses).toEqual({ "pane-1": "working" });
    expect(set.workspaces).toBe(before.workspaces);
    expect(before.pane_statuses).toEqual({});

    const cleared = applyDeltaToSnapshot(
      set,
      { domain: "pane_status", pane_id: "pane-1", status: null },
      2,
    );
    expect(cleared.pane_statuses).toEqual({});
    expect(cleared.workspaces).toBe(before.workspaces);

    // Clearing an absent pane is a no-op.
    expect(
      applyDeltaToSnapshot(
        cleared,
        { domain: "pane_status", pane_id: "pane-1", status: null },
        3,
      ),
    ).toBe(cleared);
  });

  it("swaps the whole detected-ports array", () => {
    const before = makeAppState([wsA]);
    const ports = [
      {
        port: 5173,
        pid: 42,
        process_name: "vite",
        workspace_id: null,
        label: null,
        source: null,
      },
    ];
    const after = applyDeltaToSnapshot(
      before,
      { domain: "detected_ports", ports },
      6,
    );
    expect(after.detected_ports).toBe(ports);
    expect(after.workspaces).toBe(before.workspaces);
  });
});

describe("useAppStore — delta ordering and gap recovery", () => {
  const wsA = makeWs({ workspace_id: "ws-A" });
  const wsB = makeWs({ workspace_id: "ws-B" });

  const gitDelta: AppStateDelta = {
    domain: "workspace_git",
    workspace_id: "ws-A",
    git: {
      is_git: true,
      git_branch: "feature",
      git_ahead: 1,
      git_behind: 0,
      git_additions: 0,
      git_deletions: 0,
      git_changed_files: 0,
    },
  };

  const staleGitDelta: AppStateDelta = {
    domain: "workspace_git",
    workspace_id: "ws-A",
    git: {
      is_git: true,
      git_branch: "stale",
      git_ahead: 1,
      git_behind: 0,
      git_additions: 0,
      git_deletions: 0,
      git_changed_files: 0,
    },
  };

  const portsDelta: AppStateDelta = {
    domain: "detected_ports",
    ports: [
      {
        port: 5173,
        pid: 42,
        process_name: "vite",
        workspace_id: null,
        label: null,
        source: null,
      },
    ],
  };

  function reset(): void {
    useAppStore.setState({
      appState: null,
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
      resyncInFlight: false,
      resyncRequestId: 0,
      deltaBuffer: new Map(),
      gapWindowId: 0,
    });
  }

  function seed(revision: number): void {
    const snapshot = makeAppState([wsA, wsB]);
    snapshot.snapshot_revision = revision;
    useAppStore.getState().setAppState(snapshot);
  }

  beforeEach(reset);
  afterEach(reset);

  it("applies a contiguous delta and advances the revision", () => {
    seed(10);
    useAppStore.getState().applyAppStateDelta(11, gitDelta);
    const state = useAppStore.getState();
    expect(state.lastSeenRevision).toBe(11);
    expect(state.appState!.workspaces[0].git_branch).toBe("feature");
    expect(state.resyncRequestId).toBe(0);
  });

  it("drops a delta that lost the race to a newer snapshot", () => {
    seed(10);
    const applied = useAppStore.getState().appState;
    useAppStore.getState().applyAppStateDelta(10, gitDelta);
    expect(useAppStore.getState().appState).toBe(applied);
    expect(useAppStore.getState().lastSeenRevision).toBe(10);
    expect(useAppStore.getState().resyncRequestId).toBe(0);
  });

  it("buffers a small forward gap instead of resyncing on the spot", () => {
    // The backend stamps under the state lock but emits after releasing it, so
    // a delta can overtake a snapshot that is still being serialized. That is
    // a reordering, not a loss.
    seed(10);
    const applied = useAppStore.getState().appState;
    useAppStore.getState().applyAppStateDelta(13, gitDelta);

    const state = useAppStore.getState();
    expect(state.resyncInFlight).toBe(false);
    expect(state.resyncRequestId).toBe(0);
    expect(state.gapWindowId).not.toBe(0);
    // Held, not applied: applying it would advance the baseline past 11 and 12.
    expect(state.appState).toBe(applied);
    expect(state.lastSeenRevision).toBe(10);
  });

  it("applies a buffered delta once the revision before it arrives", () => {
    seed(10);
    useAppStore.getState().applyAppStateDelta(12, gitDelta);
    useAppStore.getState().applyAppStateDelta(11, portsDelta);

    const state = useAppStore.getState();
    expect(state.lastSeenRevision).toBe(12);
    expect(state.appState!.workspaces[0].git_branch).toBe("feature");
    // Nothing is left waiting, so the window closed without a resync.
    expect(state.gapWindowId).toBe(0);
    expect(state.resyncRequestId).toBe(0);
  });

  it("lets a snapshot inside the window close the gap and replay above it", () => {
    // Delta N+2 first, then the snapshot at N+1 it overtook.
    seed(10);
    useAppStore.getState().applyAppStateDelta(12, gitDelta);
    seed(11);

    const state = useAppStore.getState();
    expect(state.lastSeenRevision).toBe(12);
    expect(state.appState!.workspaces[0].git_branch).toBe("feature");
    expect(state.gapWindowId).toBe(0);
    expect(state.resyncInFlight).toBe(false);
    expect(state.resyncRequestId).toBe(0);
  });

  it("opens exactly one resync when the reorder window expires", () => {
    seed(10);
    useAppStore.getState().applyAppStateDelta(13, gitDelta);
    const { gapWindowId } = useAppStore.getState();

    useAppStore.getState().expireGapWindow(gapWindowId);
    expect(useAppStore.getState().resyncInFlight).toBe(true);
    expect(useAppStore.getState().resyncRequestId).toBe(1);
    expect(useAppStore.getState().gapWindowId).toBe(0);

    // A timer for a window that has already been resolved must do nothing.
    useAppStore.getState().expireGapWindow(gapWindowId);
    expect(useAppStore.getState().resyncRequestId).toBe(1);
  });

  it("buffers deltas while a resync is in flight and stays single-flight", () => {
    seed(10);
    useAppStore.getState().requestResync();
    const baseline = useAppStore.getState().appState;

    useAppStore.getState().applyAppStateDelta(11, gitDelta);
    useAppStore.getState().applyAppStateDelta(12, portsDelta);
    useAppStore.getState().requestResync();

    const state = useAppStore.getState();
    // Held until the baseline lands — the snapshot may be older than these.
    expect(state.appState).toBe(baseline);
    expect(state.resyncRequestId).toBe(1);
    expect(state.deltaBuffer.size).toBe(2);
  });

  it("replays deltas above the resync baseline and discards those below it", () => {
    seed(10);
    useAppStore.getState().requestResync();
    useAppStore.getState().applyAppStateDelta(11, staleGitDelta);
    useAppStore.getState().applyAppStateDelta(13, gitDelta);

    // The fetch reports revision 12, so 11 is already in it and 13 is not.
    seed(12);
    useAppStore.getState().endResync();

    const state = useAppStore.getState();
    expect(state.lastSeenRevision).toBe(13);
    expect(state.appState!.workspaces[0].git_branch).toBe("feature");
    expect(state.deltaBuffer.size).toBe(0);
  });

  it("does not strand buffered deltas when the resync fetch fails", () => {
    seed(10);
    useAppStore.getState().requestResync();
    useAppStore.getState().applyAppStateDelta(11, gitDelta);
    // The fetch rejected: no baseline ever landed.
    useAppStore.getState().endResync();

    const state = useAppStore.getState();
    // 11 continued the sequence, so it applies rather than sitting forever.
    expect(state.lastSeenRevision).toBe(11);
    expect(state.appState!.workspaces[0].git_branch).toBe("feature");
    expect(state.deltaBuffer.size).toBe(0);
    expect(state.gapWindowId).toBe(0);
  });

  it("reopens a reorder window for deltas a failed resync left behind", () => {
    seed(10);
    useAppStore.getState().requestResync();
    useAppStore.getState().applyAppStateDelta(13, gitDelta);
    useAppStore.getState().endResync();

    const state = useAppStore.getState();
    // Still gapped, so it must be waiting on a window, not stranded.
    expect(state.lastSeenRevision).toBe(10);
    expect(state.deltaBuffer.size).toBe(1);
    expect(state.gapWindowId).not.toBe(0);
  });

  it("abandons the buffer for a resync once it overflows", () => {
    seed(10);
    // Every one of these is non-contiguous, so nothing ever drains.
    for (let i = 0; i <= DELTA_BUFFER_LIMIT; i++) {
      useAppStore.getState().applyAppStateDelta(12 + i, gitDelta);
    }

    const state = useAppStore.getState();
    expect(state.deltaBuffer.size).toBe(0);
    expect(state.resyncInFlight).toBe(true);
    expect(state.resyncRequestId).toBe(1);
    expect(state.gapWindowId).toBe(0);
  });

  it("resumes applying deltas once the resync snapshot rebases the baseline", () => {
    seed(10);
    useAppStore.getState().applyAppStateDelta(13, gitDelta);
    useAppStore.getState().expireGapWindow(useAppStore.getState().gapWindowId);
    seed(20);
    useAppStore.getState().endResync();

    useAppStore.getState().applyAppStateDelta(21, gitDelta);
    const state = useAppStore.getState();
    expect(state.lastSeenRevision).toBe(21);
    expect(state.appState!.workspaces[0].git_branch).toBe("feature");
  });

  it("reopens the resync when a rejected fetch left the baseline behind", () => {
    seed(10);
    useAppStore.getState().applyAppStateDelta(13, gitDelta);
    useAppStore.getState().expireGapWindow(useAppStore.getState().gapWindowId);
    // The fetch failed: endResync runs, no snapshot landed.
    useAppStore.getState().endResync();

    useAppStore.getState().applyAppStateDelta(14, gitDelta);
    useAppStore.getState().expireGapWindow(useAppStore.getState().gapWindowId);
    expect(useAppStore.getState().resyncRequestId).toBe(2);
  });

  it("never clobbers an optimistic pending selection", () => {
    seed(10);
    useAppStore.getState().beginPendingActivation("ws-B");
    useAppStore.getState().applyAppStateDelta(11, gitDelta);

    const state = useAppStore.getState();
    expect(state.pendingActiveWorkspaceId).toBe("ws-B");
    expect(selectActiveWorkspaceId(state)).toBe("ws-B");
    // The snapshot's own active id is untouched by the delta.
    expect(state.appState!.active_workspace_id).toBe("ws-A");
  });

  it("ignores deltas before the first snapshot", () => {
    useAppStore.getState().applyAppStateDelta(1, gitDelta);
    const state = useAppStore.getState();
    expect(state.appState).toBeNull();
    expect(state.resyncRequestId).toBe(0);
  });
});
