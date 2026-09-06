import { describe, expect, it } from "vitest";

import { adoptItemIds } from "./adopt-item-ids";
import type {
  AssistantMessageItem,
  ChatViewItem,
  SubagentRunItem,
  SubagentView,
  ToolCallItem,
  UserMessageItem,
} from "./types";

/**
 * `adoptItemIds` is what lets the full-history replay of a tail-first
 * open land UNDER the preview the user is reading without remounting
 * it: the reducer mints fresh ids per replay, and ids are the
 * virtualised list's row keys. These tests pin the matching contract —
 * suffix-wise, identity by kind + correlation key, nested through
 * subagent cards — and that only the id is carried over.
 */

let seq = 0;
function user(id: string, text: string): UserMessageItem {
  return { kind: "user_message", id, seq: seq++, text };
}
function assistant(id: string, text: string, turnId = "t1"): AssistantMessageItem {
  return {
    kind: "assistant_message",
    id,
    seq: seq++,
    turn_id: turnId,
    text,
    streaming: false,
  };
}
function tool(id: string, toolUseId: string, status: ToolCallItem["status"] = "done"): ToolCallItem {
  return {
    kind: "tool_call",
    id,
    seq: seq++,
    tool_use_id: toolUseId,
    tool_name: "Read",
    input: null,
    status,
    result_content: null,
    approval_request_id: null,
  };
}
function subagent(id: string, items: ChatViewItem[]): SubagentView {
  return { id, status: "completed", items, toneIndex: 0 };
}
function run(id: string, subagents: SubagentView[]): SubagentRunItem {
  return { kind: "subagent_run", id, seq: seq++, turn_id: "t1", subagents };
}

const ids = (items: readonly ChatViewItem[]) => items.map((item) => item.id);

describe("adoptItemIds", () => {
  it("carries the previous ids onto the matching suffix and leaves the new prefix alone", () => {
    const previous = [user("p1", "hi"), assistant("p2", "hello"), tool("p3", "tu-1")];
    const next = [
      user("n0", "much earlier"),
      assistant("n1", "older reply"),
      user("n2", "hi"),
      assistant("n3", "hello"),
      tool("n4", "tu-1"),
    ];
    const out = adoptItemIds(previous, next);
    expect(ids(out)).toEqual(["n0", "n1", "p1", "p2", "p3"]);
    // Only the id moved: content, seq and everything else are the new
    // replay's, which is the authority.
    expect(out[4]).toEqual({ ...next[4], id: "p3" });
    expect(out[2].seq).toBe(next[2].seq);
    // Input arrays are not mutated.
    expect(ids(next)).toEqual(["n0", "n1", "n2", "n3", "n4"]);
  });

  it("stops at the first pair that is not the same item, from the tail backwards", () => {
    // The previous transcript's older half diverges (a different tool
    // call): everything above the divergence is treated as new even
    // where a later coincidence would match again.
    const previous = [
      tool("p0", "tu-same"),
      tool("p1", "tu-OLD"),
      assistant("p2", "done"),
    ];
    const next = [
      tool("n0", "tu-same"),
      tool("n1", "tu-NEW"),
      assistant("n2", "done"),
    ];
    expect(ids(adoptItemIds(previous, next))).toEqual(["n0", "n1", "p2"]);
  });

  it("uses each kind's correlation identity, not the id", () => {
    // Same kinds, same slots, but a different tool_use_id / text — none
    // of these are the same item, so none adopt.
    const previous = [tool("p0", "tu-a"), assistant("p1", "x")];
    const next = [tool("n0", "tu-b"), assistant("n1", "x")];
    expect(ids(adoptItemIds(previous, next))).toEqual(["n0", "p1"]);

    const differentText = adoptItemIds([assistant("p", "a")], [assistant("n", "b")]);
    expect(ids(differentText)).toEqual(["n"]);
    const differentKind = adoptItemIds([user("p", "a")], [assistant("n", "a")]);
    expect(ids(differentKind)).toEqual(["n"]);
  });

  it("handles a previous transcript longer than the next, and empty inputs", () => {
    const previous = [user("p0", "a"), user("p1", "b"), user("p2", "c")];
    const next = [user("n1", "b"), user("n2", "c")];
    expect(ids(adoptItemIds(previous, next))).toEqual(["p1", "p2"]);
    expect(adoptItemIds([], next)).toEqual(next);
    expect(adoptItemIds(previous, [])).toEqual([]);
  });

  it("adopts the nested items of a subagent_run card, per subagent", () => {
    const previous = [
      run("pRun", [
        subagent("sa-1", [tool("pa1", "tu-a1"), assistant("pa2", "a-report")]),
        subagent("sa-2", [tool("pb1", "tu-b1")]),
      ]),
    ];
    const next = [
      user("n0", "older"),
      run("nRun", [
        // sa-1 gained an earlier item in the full replay; its suffix
        // still matches.
        subagent("sa-1", [
          tool("na0", "tu-a0"),
          tool("na1", "tu-a1"),
          assistant("na2", "a-report"),
        ]),
        subagent("sa-2", [tool("nb1", "tu-b1")]),
      ]),
    ];
    const out = adoptItemIds(previous, next);
    expect(ids(out)).toEqual(["n0", "pRun"]);
    const card = out[1] as SubagentRunItem;
    expect(card.subagents.map((view) => view.id)).toEqual(["sa-1", "sa-2"]);
    expect(ids(card.subagents[0].items)).toEqual(["na0", "pa1", "pa2"]);
    expect(ids(card.subagents[1].items)).toEqual(["pb1"]);
    // The card's own content comes from the new replay.
    expect(card.seq).toBe(next[1].seq);
  });

  it("does not treat subagent_run cards with a different subagent set as the same card", () => {
    const previous = [run("pRun", [subagent("sa-1", []), subagent("sa-2", [])])];
    const sameLengthDifferentIds = [run("nRun", [subagent("sa-1", []), subagent("sa-X", [])])];
    expect(ids(adoptItemIds(previous, sameLengthDifferentIds))).toEqual(["nRun"]);
    const differentLength = [run("nRun", [subagent("sa-1", [])])];
    expect(ids(adoptItemIds(previous, differentLength))).toEqual(["nRun"]);
  });
});