import { describe, expect, it } from "vitest";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { showNoGitState } from "./use-initialize-git";

function makeWorkspace(
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "scratch",
    workspace_type: "standard",
    cwd: "/home/dev/projects/scratch",
    is_git: false,
    git_branch: null,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/home/dev/projects/scratch",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

describe("showNoGitState", () => {
  it("shows the affordance for a local non-git project", () => {
    expect(showNoGitState(makeWorkspace(), "/home/dev")).toBe(true);
  });

  it("excludes a standard workspace rooted at home", () => {
    const workspace = makeWorkspace({
      cwd: "/home/dev",
      project_root: "/home/dev",
    });

    expect(showNoGitState(workspace, "/home/dev")).toBe(false);
  });

  it("uses cwd when an older snapshot has no project root", () => {
    const workspace = makeWorkspace({
      cwd: "/home/dev",
      project_root: null,
    });

    expect(showNoGitState(workspace, "/home/dev")).toBe(false);
  });

  it("fails closed while the home directory is unresolved", () => {
    expect(showNoGitState(makeWorkspace(), null)).toBe(false);
  });

  it.each([
    ["git workspace", { is_git: true }],
    ["legacy Home workspace", { workspace_type: "home" as const }],
    ["attach-only workspace", { attach_only: true }],
    ["remote workspace", { host_id: 7 }],
  ])("excludes a %s", (_label, overrides) => {
    expect(showNoGitState(makeWorkspace(overrides), "/home/dev")).toBe(false);
  });
});
