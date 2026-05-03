import { describe, it, expect } from "vitest";
import {
  groupWorkspacesByProject,
  resolveProjectRoot,
  useAppStore,
} from "./app-store";
import type { WorkspaceSnapshot } from "@/tauri/types";

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
