import { describe, expect, it } from "vitest";
import type { WorkspaceSnapshot } from "@/tauri/types";
import type { ProjectGroup } from "@/stores/app-store";
import {
  buildProjectsModel,
  isChatWorkspace,
  nestRows,
  visibleLiveIds,
} from "./sidebar-projects-model";

const HOME = "/home/u";
const APP = "/home/u/projects/app";
const LIB = "/home/u/projects/lib";

function ws(
  id: string,
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  return {
    workspace_id: id,
    title: id,
    workspace_type: "standard",
    cwd: APP,
    project_root: APP,
    is_git: true,
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
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

function group(path: string, workspaces: WorkspaceSnapshot[]): ProjectGroup {
  return {
    projectName: path === HOME ? "Home" : path.split("/").pop()!,
    projectPath: path,
    workspaces,
  };
}

function build(
  all: WorkspaceSnapshot[],
  opts: { parked?: string[]; liveOverride?: string[]; filter?: string | null } = {},
) {
  const byPath = new Map<string, WorkspaceSnapshot[]>();
  for (const w of all) {
    const p = w.project_root || w.cwd;
    byPath.set(p, [...(byPath.get(p) ?? []), w]);
  }
  return buildProjectsModel({
    projectGroups: [...byPath.entries()].map(([p, list]) => group(p, list)),
    homeDir: HOME,
    storedIndex: new Map(all.map((w, i) => [w.workspace_id, i])),
    parkedIds: new Set(opts.parked ?? []),
    liveOverride: new Set(opts.liveOverride ?? []),
    filter: opts.filter ?? null,
  });
}

const ids = (rows: { workspace: WorkspaceSnapshot }[]) =>
  rows.map((r) => r.workspace.workspace_id);

describe("isChatWorkspace", () => {
  it("treats home-rooted and Home-type workspaces as chats", () => {
    expect(isChatWorkspace(ws("a", { cwd: HOME, project_root: HOME }), HOME)).toBe(true);
    expect(isChatWorkspace(ws("b", { workspace_type: "home" }), HOME)).toBe(true);
    expect(isChatWorkspace(ws("c", { cwd: HOME, project_root: null }), HOME)).toBe(true);
  });

  it("treats a loose non-git folder with no project root as a chat", () => {
    expect(
      isChatWorkspace(ws("d", { cwd: "/tmp/x", project_root: null, is_git: false }), HOME),
    ).toBe(true);
  });

  it("keeps git repos and opened non-git projects as projects", () => {
    expect(isChatWorkspace(ws("e"), HOME)).toBe(false);
    expect(
      isChatWorkspace(ws("f", { cwd: "/srv/pad", project_root: "/srv/pad", is_git: false }), HOME),
    ).toBe(false);
  });
});

describe("buildProjectsModel", () => {
  it("puts no-project chats in Chats and one section per project, newest first", () => {
    const model = build([
      ws("app-1"),
      ws("chat-1", { cwd: HOME, project_root: HOME, is_git: false }),
      ws("lib-1", { cwd: LIB, project_root: LIB }),
      ws("app-2"),
      ws("chat-2", { cwd: HOME, project_root: HOME, is_git: false }),
    ]);
    expect(model.chats?.name).toBe("Chats");
    expect(ids(model.chats!.rows)).toEqual(["chat-2", "chat-1"]);
    expect(model.projects.map((p) => p.name)).toEqual(["app", "lib"]);
    expect(ids(model.projects[0].rows)).toEqual(["app-2", "app-1"]);
    expect(model.projects[0].liveCount).toBe(2);
  });

  it("omits Chats when every workspace belongs to a project", () => {
    expect(build([ws("app-1")]).chats).toBeNull();
  });

  it("nests follow-up children under their parent, not at top level", () => {
    const model = build([
      ws("parent"),
      ws("other"),
      ws("child-a", { parent_workspace_id: "parent" }),
      ws("child-b", { parent_workspace_id: "parent" }),
      ws("grandchild", { parent_workspace_id: "child-a" }),
    ]);
    const app = model.projects[0];
    expect(ids(app.rows)).toEqual(["other", "parent"]);
    const parent = app.rows.find((r) => r.workspace.workspace_id === "parent")!;
    expect(ids(parent.children)).toEqual(["child-b", "child-a"]);
    const childA = parent.children.find((r) => r.workspace.workspace_id === "child-a")!;
    expect(ids(childA.children)).toEqual(["grandchild"]);
    // Children count toward the project's live total.
    expect(app.liveCount).toBe(5);
  });

  it("renders a child normally when its parent is missing, settled, or in another project", () => {
    const model = build(
      [
        ws("parked-parent"),
        ws("orphan", { parent_workspace_id: "gone" }),
        ws("kid-of-parked", { parent_workspace_id: "parked-parent" }),
        ws("lib-parent", { cwd: LIB, project_root: LIB }),
        ws("kid-of-lib", { parent_workspace_id: "lib-parent" }),
      ],
      { parked: ["parked-parent"] },
    );
    const app = model.projects.find((p) => p.name === "app")!;
    expect(ids(app.rows)).toEqual(["kid-of-lib", "kid-of-parked", "orphan"]);
    expect(app.rows.every((r) => r.children.length === 0)).toBe(true);
    expect(app.parked.map((w) => w.workspace_id)).toEqual(["parked-parent"]);
  });

  it("never drops rows caught in a parent cycle", () => {
    const rows = nestRows([
      ws("a", { parent_workspace_id: "b" }),
      ws("b", { parent_workspace_id: "a" }),
      ws("self", { parent_workspace_id: "self" }),
    ]);
    const all: string[] = [];
    const walk = (list: typeof rows) =>
      list.forEach((r) => {
        all.push(r.workspace.workspace_id);
        walk(r.children);
      });
    walk(rows);
    expect(all.sort()).toEqual(["a", "b", "self"]);
  });

  it("folds settled and snoozed workspaces out of the live rows per project", () => {
    const model = build([ws("live"), ws("settled"), ws("pinned", { pinned_at: 1 })], {
      parked: ["settled", "pinned"],
    });
    const app = model.projects[0];
    expect(ids(app.rows)).toEqual(["pinned", "live"]);
    expect(app.parked.map((w) => w.workspace_id)).toEqual(["settled"]);
    expect(app.liveCount).toBe(2);
  });

  it("keeps a parked workspace live while its agent needs attention", () => {
    const model = build([ws("live"), ws("settled-busy")], {
      parked: ["settled-busy"],
      liveOverride: ["settled-busy"],
    });
    expect(ids(model.projects[0].rows)).toEqual(["settled-busy", "live"]);
    expect(model.projects[0].parked).toEqual([]);
  });

  it("scopes to the project filter", () => {
    const model = build(
      [ws("app-1"), ws("lib-1", { cwd: LIB, project_root: LIB })],
      { filter: LIB },
    );
    expect(model.projects.map((p) => p.name)).toEqual(["lib"]);
  });
});

describe("visibleLiveIds", () => {
  it("lists chats then project rows depth-first, skipping folded groups", () => {
    const model = build([
      ws("parent"),
      ws("child", { parent_workspace_id: "parent" }),
      ws("lib-1", { cwd: LIB, project_root: LIB }),
      ws("chat", { cwd: HOME, project_root: HOME, is_git: false }),
    ]);
    expect(visibleLiveIds(model, {})).toEqual(["chat", "parent", "child", "lib-1"]);
    expect(visibleLiveIds(model, { [LIB]: true })).toEqual(["chat", "parent", "child"]);
  });
});
