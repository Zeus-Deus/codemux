import { describe, expect, it } from "vitest";

import type { ProjectGroup } from "@/stores/app-store";
import type { SettledEntry, SnoozeEntry } from "@/stores/sidebar-inbox-store";
import type { WorkspaceSnapshot } from "@/tauri/types";

import {
  partitionProjectScopes,
  visibleSettledProjects,
  SETTLED_COLLAPSED_COUNT,
} from "./project-scope-list";

function ws(id: string): WorkspaceSnapshot {
  return { workspace_id: id } as WorkspaceSnapshot;
}

function group(path: string, workspaceIds: string[]): ProjectGroup {
  return {
    projectName: path.split("/").pop() ?? path,
    projectPath: path,
    workspaces: workspaceIds.map(ws),
  } as ProjectGroup;
}

function settledEntry(id: string, at: number): SettledEntry {
  return { id, at };
}

function snoozeEntry(id: string, at: number): SnoozeEntry {
  return { id, at, until: at + 60_000 };
}

const paths = (groups: ProjectGroup[]) => groups.map((g) => g.projectPath);

describe("partitionProjectScopes", () => {
  it("puts every project in Active when nothing is settled", () => {
    const groups = [group("/p/a", ["a1"]), group("/p/b", ["b1"])];
    const { active, settled } = partitionProjectScopes(groups, [], [], {});
    expect(paths(active)).toEqual(["/p/a", "/p/b"]);
    expect(settled).toEqual([]);
  });

  it("moves a project to Settled only when ALL of its workspaces are settled", () => {
    const groups = [
      // Two worktrees, only one settled → still drawing a sidebar card.
      group("/p/partly", ["p1", "p2"]),
      group("/p/fully", ["f1", "f2"]),
    ];
    const { active, settled } = partitionProjectScopes(
      groups,
      [
        settledEntry("p1", 1_000),
        settledEntry("f1", 1_000),
        settledEntry("f2", 2_000),
      ],
      [],
      {},
    );
    expect(paths(active)).toEqual(["/p/partly"]);
    expect(paths(settled)).toEqual(["/p/fully"]);
  });

  it("orders Active by most-recent activity stamp", () => {
    const groups = [
      group("/p/stale", ["s1"]),
      group("/p/fresh", ["f1"]),
      group("/p/mid", ["m1"]),
    ];
    const { active } = partitionProjectScopes(groups, [], [], {
      s1: 1_000,
      f1: 9_000,
      m1: 5_000,
    });
    expect(paths(active)).toEqual(["/p/fresh", "/p/mid", "/p/stale"]);
  });

  it("ranks a multi-worktree project by its most recently active workspace", () => {
    const groups = [group("/p/multi", ["m1", "m2"]), group("/p/solo", ["s1"])];
    const { active } = partitionProjectScopes(groups, [], [], {
      m1: 1_000,
      m2: 9_000,
      s1: 5_000,
    });
    expect(paths(active)).toEqual(["/p/multi", "/p/solo"]);
  });

  it("sorts unstamped Active projects last, preserving app-state order", () => {
    const groups = [
      group("/p/none1", ["n1"]),
      group("/p/stamped", ["s1"]),
      group("/p/none2", ["n2"]),
    ];
    const { active } = partitionProjectScopes(groups, [], [], { s1: 5_000 });
    expect(paths(active)).toEqual(["/p/stamped", "/p/none1", "/p/none2"]);
  });

  it("orders Settled most-recently-settled first", () => {
    const groups = [
      group("/p/old", ["o1"]),
      group("/p/new", ["n1"]),
      group("/p/mid", ["m1"]),
    ];
    const { settled } = partitionProjectScopes(
      groups,
      [
        settledEntry("o1", 1_000),
        settledEntry("n1", 9_000),
        settledEntry("m1", 5_000),
      ],
      [],
      {},
    );
    expect(paths(settled)).toEqual(["/p/new", "/p/mid", "/p/old"]);
  });

  it("keeps a workspace-less group Active rather than treating it as settled", () => {
    const groups = [group("/p/empty", [])];
    const { active, settled } = partitionProjectScopes(groups, [], [], {});
    expect(paths(active)).toEqual(["/p/empty"]);
    expect(settled).toEqual([]);
  });

  it("ignores settled entries for workspaces that no longer exist", () => {
    const groups = [group("/p/a", ["a1"])];
    const { active, settled } = partitionProjectScopes(
      groups,
      [settledEntry("ghost", 1_000)],
      [],
      {},
    );
    expect(paths(active)).toEqual(["/p/a"]);
    expect(settled).toEqual([]);
  });

  // Snooze is the sidebar's second parked lifecycle: a snoozed card leaves
  // the active list just like a settled one, so the partition must fold it
  // in — otherwise a fully-snoozed project would read as Active here.
  describe("snoozed workspaces fold into the parked partition", () => {
    it("moves a project whose every workspace is snoozed out of Active", () => {
      const groups = [group("/p/napping", ["n1", "n2"]), group("/p/live", ["l1"])];
      const { active, settled } = partitionProjectScopes(
        groups,
        [],
        [snoozeEntry("n1", 1_000), snoozeEntry("n2", 2_000)],
        {},
      );
      expect(paths(active)).toEqual(["/p/live"]);
      expect(paths(settled)).toEqual(["/p/napping"]);
    });

    it("keeps a project Active while any workspace is neither settled nor snoozed", () => {
      const groups = [group("/p/mixed", ["m1", "m2"])];
      const { active, settled } = partitionProjectScopes(
        groups,
        [],
        [snoozeEntry("m1", 1_000)],
        {},
      );
      expect(paths(active)).toEqual(["/p/mixed"]);
      expect(settled).toEqual([]);
    });

    it("parks a project whose workspaces are split between settled and snoozed", () => {
      const groups = [group("/p/parked", ["p1", "p2"])];
      const { active, settled } = partitionProjectScopes(
        groups,
        [settledEntry("p1", 1_000)],
        [snoozeEntry("p2", 2_000)],
        {},
      );
      expect(active).toEqual([]);
      expect(paths(settled)).toEqual(["/p/parked"]);
    });

    it("ranks snooze-parked projects by their park time alongside settled ones", () => {
      const groups = [
        group("/p/settled-old", ["so1"]),
        group("/p/snoozed-new", ["sn1"]),
      ];
      const { settled } = partitionProjectScopes(
        groups,
        [settledEntry("so1", 1_000)],
        [snoozeEntry("sn1", 9_000)],
        {},
      );
      expect(paths(settled)).toEqual(["/p/snoozed-new", "/p/settled-old"]);
    });

    it("ignores snooze entries for workspaces that no longer exist", () => {
      const groups = [group("/p/a", ["a1"])];
      const { active, settled } = partitionProjectScopes(
        groups,
        [],
        [snoozeEntry("ghost", 1_000)],
        {},
      );
      expect(paths(active)).toEqual(["/p/a"]);
      expect(settled).toEqual([]);
    });
  });
});

describe("visibleSettledProjects", () => {
  const many = Array.from({ length: 9 }, (_, i) => group(`/p/s${i}`, [`s${i}`]));

  it("collapses to the cap by default", () => {
    const visible = visibleSettledProjects(many, {
      expanded: false,
      searching: false,
      activeProjectPath: null,
    });
    expect(visible).toHaveLength(SETTLED_COLLAPSED_COUNT);
    expect(paths(visible)).toEqual(paths(many.slice(0, SETTLED_COLLAPSED_COUNT)));
  });

  it("reveals everything once expanded", () => {
    const visible = visibleSettledProjects(many, {
      expanded: true,
      searching: false,
      activeProjectPath: null,
    });
    expect(visible).toHaveLength(many.length);
  });

  it("reveals everything while searching, so the tail stays reachable", () => {
    const visible = visibleSettledProjects(many, {
      expanded: false,
      searching: true,
      activeProjectPath: null,
    });
    expect(visible).toHaveLength(many.length);
  });

  it("always includes the targeted project even from the hidden tail", () => {
    const visible = visibleSettledProjects(many, {
      expanded: false,
      searching: false,
      activeProjectPath: "/p/s8",
    });
    expect(paths(visible)).toContain("/p/s8");
    expect(visible).toHaveLength(SETTLED_COLLAPSED_COUNT + 1);
  });

  it("does not duplicate the targeted project when it is already in the head", () => {
    const visible = visibleSettledProjects(many, {
      expanded: false,
      searching: false,
      activeProjectPath: "/p/s0",
    });
    expect(visible).toHaveLength(SETTLED_COLLAPSED_COUNT);
    expect(paths(visible).filter((p) => p === "/p/s0")).toHaveLength(1);
  });

  it("tolerates a targeted project that is not settled at all", () => {
    const visible = visibleSettledProjects(many, {
      expanded: false,
      searching: false,
      activeProjectPath: "/p/not-settled",
    });
    expect(visible).toHaveLength(SETTLED_COLLAPSED_COUNT);
  });
});
