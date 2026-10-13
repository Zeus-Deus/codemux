import { describe, expect, it } from "vitest";
import type { PaneStatus, WorkspaceSnapshot } from "@/tauri/types";
import { needsYouWorkspaceIds, nextNeedsYouTarget } from "./needs-you";

const ws = (id: string) =>
  ({
    workspace_id: id,
    surfaces: [{ surface_id: `s-${id}`, root: { kind: "terminal", pane_id: `p-${id}` } }],
  }) as unknown as WorkspaceSnapshot;

describe("needsYouWorkspaceIds", () => {
  const workspaces = ["a", "b", "c", "d"].map(ws);
  const statuses: Record<string, PaneStatus> = {
    "p-a": "permission",
    "p-b": "working",
    "p-c": "permission",
    "p-d": "permission",
  };

  it("lists only blocked workspaces, longest-waiting first", () => {
    const since = {
      a: { status: "permission" as const, at: 300 },
      c: { status: "permission" as const, at: 100 },
      d: { status: "permission" as const, at: 200 },
    };
    expect(needsYouWorkspaceIds(workspaces, statuses, since)).toEqual(["c", "d", "a"]);
  });

  it("ranks a blocker with no timestamp yet as the newest, keeping input order", () => {
    const since = { d: { status: "permission" as const, at: 100 } };
    expect(needsYouWorkspaceIds(workspaces, statuses, since)).toEqual(["d", "a", "c"]);
  });
});

describe("nextNeedsYouTarget", () => {
  it("starts at the longest-waiting workspace", () => {
    expect(nextNeedsYouTarget(["x", "y"], "elsewhere")).toBe("x");
    expect(nextNeedsYouTarget(["x", "y"], null)).toBe("x");
  });

  it("walks on from the blocked workspace the user is on, wrapping", () => {
    expect(nextNeedsYouTarget(["x", "y", "z"], "x")).toBe("y");
    expect(nextNeedsYouTarget(["x", "y", "z"], "z")).toBe("x");
  });

  it("has nowhere to go when nothing is blocked or the user is on the only one", () => {
    expect(nextNeedsYouTarget([], "x")).toBeNull();
    expect(nextNeedsYouTarget(["x"], "x")).toBeNull();
  });
});
