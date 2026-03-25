import { describe, it, expect } from "vitest";
import { resolveProjectRoot } from "./app-store";
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
    pr_number: null,
    pr_state: null,
    pr_url: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

describe("resolveProjectRoot", () => {
  it("returns cwd for regular (non-worktree) workspaces", () => {
    const ws = makeWs({ cwd: "/home/user/projects/myapp" });
    expect(resolveProjectRoot(ws)).toBe("/home/user/projects/myapp");
  });

  it("extracts repo name from worktree_path when set", () => {
    const ws = makeWs({
      cwd: "/home/user/.codemux/worktrees/myapp/feature-branch",
      worktree_path: "/home/user/.codemux/worktrees/myapp/feature-branch",
    });
    expect(resolveProjectRoot(ws)).toBe("/home/user/.codemux/worktrees/myapp");
  });

  it("groups multiple worktrees of the same repo together", () => {
    const ws1 = makeWs({
      cwd: "/home/user/.codemux/worktrees/myapp/feature-a",
      worktree_path: "/home/user/.codemux/worktrees/myapp/feature-a",
    });
    const ws2 = makeWs({
      cwd: "/home/user/.codemux/worktrees/myapp/feature-b",
      worktree_path: "/home/user/.codemux/worktrees/myapp/feature-b",
    });
    expect(resolveProjectRoot(ws1)).toBe(resolveProjectRoot(ws2));
  });

  it("prefers worktree_path over cwd for detection", () => {
    const ws = makeWs({
      cwd: "/some/other/path",
      worktree_path: "/home/user/.codemux/worktrees/myapp/branch",
    });
    expect(resolveProjectRoot(ws)).toBe("/home/user/.codemux/worktrees/myapp");
  });

  it("falls back to cwd when worktree_path has no marker", () => {
    const ws = makeWs({
      cwd: "/home/user/projects/myapp",
      worktree_path: "/home/user/projects/myapp",
    });
    expect(resolveProjectRoot(ws)).toBe("/home/user/projects/myapp");
  });

  it("handles null worktree_path gracefully", () => {
    const ws = makeWs({
      cwd: "/home/user/projects/myapp",
      worktree_path: null,
    });
    expect(resolveProjectRoot(ws)).toBe("/home/user/projects/myapp");
  });

  it("keeps different projects separate even with similar names", () => {
    const ws1 = makeWs({ cwd: "/home/user/work/app" });
    const ws2 = makeWs({ cwd: "/home/user/personal/app" });
    expect(resolveProjectRoot(ws1)).not.toBe(resolveProjectRoot(ws2));
  });
});

// Test the grouping logic inline (useProjectGroupedWorkspaces is a hook,
// so we test the underlying logic via resolveProjectRoot + manual grouping)
describe("project grouping", () => {
  it("disambiguates projects with the same folder name", () => {
    // Simulate what useProjectGroupedWorkspaces does
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
