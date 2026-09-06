import { describe, expect, it } from "vitest";
import { selectWorkspaceNavigationTarget } from "./workspace-navigation";

const WORKSPACES = ["ws-a", "ws-b", "ws-c"].map((workspace_id) => ({
  workspace_id,
}));

describe("selectWorkspaceNavigationTarget", () => {
  it("uses snapshot order for next and previous targets", () => {
    expect(selectWorkspaceNavigationTarget(WORKSPACES, "ws-b", 1)).toBe("ws-c");
    expect(selectWorkspaceNavigationTarget(WORKSPACES, "ws-b", -1)).toBe("ws-a");
  });

  it("wraps in both directions", () => {
    expect(selectWorkspaceNavigationTarget(WORKSPACES, "ws-c", 1)).toBe("ws-a");
    expect(selectWorkspaceNavigationTarget(WORKSPACES, "ws-a", -1)).toBe("ws-c");
  });

  it("uses modular steps larger than the collection", () => {
    expect(selectWorkspaceNavigationTarget(WORKSPACES, "ws-a", 7)).toBe("ws-b");
    expect(selectWorkspaceNavigationTarget(WORKSPACES, "ws-a", -7)).toBe("ws-c");
  });

  it("returns the only workspace for every finite step", () => {
    const only = [{ workspace_id: "ws-only" }];
    expect(selectWorkspaceNavigationTarget(only, "ws-only", 1)).toBe("ws-only");
    expect(selectWorkspaceNavigationTarget(only, "ws-only", -99)).toBe("ws-only");
  });

  it("returns null for an empty list or a missing active workspace", () => {
    expect(selectWorkspaceNavigationTarget([], "ws-a", 1)).toBeNull();
    expect(selectWorkspaceNavigationTarget(WORKSPACES, null, 1)).toBeNull();
    expect(selectWorkspaceNavigationTarget(WORKSPACES, "ws-missing", 1)).toBeNull();
  });
});
