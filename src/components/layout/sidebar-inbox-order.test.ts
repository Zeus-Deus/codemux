import { describe, expect, it } from "vitest";
import { orderInboxWorkspaces, planInboxDrop } from "./sidebar-inbox-order";

describe("inbox presentation order", () => {
  it("keeps newest-first fallback and puts new work above saved ranks", () => {
    const workspaces = ["old", "middle", "new"].map((workspace_id) => ({
      workspace_id,
    }));
    expect(
      orderInboxWorkspaces(workspaces, []).map((ws) => ws.workspace_id),
    ).toEqual(["new", "middle", "old"]);
    expect(
      orderInboxWorkspaces(workspaces, ["old", "middle"]).map(
        (ws) => ws.workspace_id,
      ),
    ).toEqual(["new", "old", "middle"]);
    expect(workspaces.map((ws) => ws.workspace_id)).toEqual([
      "old",
      "middle",
      "new",
    ]);
  });
  const base = {
    id: "a",
    from: "active" as const,
    to: "pinned" as const,
    index: 0,
    visible: ["b", "c"],
    destinationIds: ["b", "hidden", "c"],
    allIds: ["a", "b", "hidden", "c", "other"],
    order: ["b", "hidden", "c", "a", "other"],
    wraps: false,
  };

  it("pins before the chosen neighbour and retains filtered-out ids", () => {
    expect(planInboxDrop({ ...base, index: 1 })).toEqual({
      id: "a",
      pinned: true,
      order: ["b", "hidden", "a", "c", "other"],
      destination: "pinned",
    });
  });
  it("inserts after the last visible neighbour without moving hidden peers", () => {
    expect(planInboxDrop({ ...base, index: 2 })?.order).toEqual([
      "b",
      "hidden",
      "c",
      "a",
      "other",
    ]);
  });
  it("creates a first pin and an empty Active destination", () => {
    expect(
      planInboxDrop({ ...base, visible: [], destinationIds: [] }),
    ).toMatchObject({
      pinned: true,
      order: ["a", "b", "hidden", "c", "other"],
    });
    expect(
      planInboxDrop({
        ...base,
        from: "pinned",
        to: "active",
        visible: [],
        destinationIds: [],
      }),
    ).toMatchObject({ pinned: false, destination: "active" });
  });
  it("reorders up and down, and ignores a drop back into its original slot", () => {
    const input = {
      ...base,
      from: "pinned" as const,
      visible: ["b", "a", "c"],
      destinationIds: ["b", "a", "c"],
    };
    expect(planInboxDrop({ ...input, index: 0 })?.order[0]).toBe("a");
    expect(planInboxDrop({ ...input, index: 2 })?.order).toEqual([
      "b",
      "hidden",
      "c",
      "a",
      "other",
    ]);
    expect(planInboxDrop({ ...input, index: 1 })).toBeNull();
  });
  it("names the automatic Wrapping up destination rather than promising an Active slot", () => {
    expect(
      planInboxDrop({ ...base, from: "pinned", to: "active", wraps: true }),
    ).toMatchObject({ pinned: false, destination: "wrapping" });
  });
  it("rejects a source removed during the gesture", () => {
    expect(planInboxDrop({ ...base, id: "removed" })).toBeNull();
  });
  it("does not move a card across hidden peers when its visible slot is unchanged", () => {
    expect(
      planInboxDrop({
        ...base,
        from: "pinned",
        visible: ["a", "b"],
        destinationIds: ["a", "hidden", "b"],
      }),
    ).toBeNull();
  });
});
