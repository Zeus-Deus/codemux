import { describe, it, expect } from "vitest";
import { computeLiveEntries } from "./sidebar-live-grouping";
import { SETTLED_FADE_MS } from "@/stores/sidebar-density-store";
import type {
  WorkspaceSnapshot,
  SurfaceSnapshot,
  PaneStatus,
} from "@/tauri/types";
import type { ProjectGroup } from "@/stores/app-store";

function surfaceWithPane(paneId: string): SurfaceSnapshot {
  return {
    surface_id: `sf-${paneId}`,
    title: "",
    root: { kind: "terminal", pane_id: paneId, session_id: "s", title: "" },
    active_pane_id: paneId,
  };
}

function ws(id: string, paneId: string): WorkspaceSnapshot {
  return {
    workspace_id: id,
    title: id,
    workspace_type: "standard",
    cwd: `/p/${id}`,
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
    project_root: "/p",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [surfaceWithPane(paneId)],
  } as WorkspaceSnapshot;
}

function group(
  name: string,
  path: string,
  workspaces: WorkspaceSnapshot[],
): ProjectGroup {
  return { projectName: name, projectPath: path, workspaces };
}

const NOW = 1_000_000_000;

describe("computeLiveEntries", () => {
  it("keeps working, permission and unseen-fresh review rows; drops idle", () => {
    const groups = [
      group("Alpha", "/p/alpha", [
        ws("work", "p-work"),
        ws("idle", "p-idle"),
        ws("perm", "p-perm"),
        ws("done", "p-done"),
      ]),
    ];
    const statuses: Record<string, PaneStatus> = {
      "p-work": "working",
      "p-idle": "idle",
      "p-perm": "permission",
      "p-done": "review",
    };
    const entries = computeLiveEntries(
      groups,
      statuses,
      { done: NOW }, // settled just now → still fresh
      {},
      NOW,
    );
    const ids = entries.map((e) => e.workspace.workspace_id);
    expect(ids).not.toContain("idle");
    expect(ids).toEqual(["perm", "work", "done"]);
  });

  it("sorts permission → working → review, stable by tree order per bucket", () => {
    const groups = [
      group("Alpha", "/p/alpha", [ws("w1", "pw1"), ws("r1", "pr1")]),
      group("Beta", "/p/beta", [ws("perm1", "pp1"), ws("w2", "pw2")]),
    ];
    const statuses: Record<string, PaneStatus> = {
      pw1: "working",
      pr1: "review",
      pp1: "permission",
      pw2: "working",
    };
    const entries = computeLiveEntries(
      groups,
      statuses,
      { r1: NOW },
      {},
      NOW,
    );
    // permission (perm1) first, then working in tree order (w1, w2), then review.
    expect(entries.map((e) => e.workspace.workspace_id)).toEqual([
      "perm1",
      "w1",
      "w2",
      "r1",
    ]);
  });

  it("excludes a review row that has been seen since it settled", () => {
    const groups = [group("A", "/p/a", [ws("done", "p-done")])];
    const statuses: Record<string, PaneStatus> = { "p-done": "review" };
    const entries = computeLiveEntries(
      groups,
      statuses,
      { done: NOW - 1000 },
      { done: NOW }, // seen after it settled → no longer live
      NOW,
    );
    expect(entries).toHaveLength(0);
  });

  it("excludes a review row older than the settle-fade window", () => {
    const groups = [group("A", "/p/a", [ws("done", "p-done")])];
    const statuses: Record<string, PaneStatus> = { "p-done": "review" };
    const entries = computeLiveEntries(
      groups,
      statuses,
      { done: NOW - SETTLED_FADE_MS - 1 },
      {},
      NOW,
    );
    expect(entries).toHaveLength(0);
  });

  it("tags each entry with its project origin", () => {
    const groups = [group("Alpha", "/p/alpha", [ws("w1", "pw1")])];
    const entries = computeLiveEntries(groups, { pw1: "working" }, {}, {}, NOW);
    expect(entries[0].projectName).toBe("Alpha");
    expect(entries[0].projectPath).toBe("/p/alpha");
  });
});
