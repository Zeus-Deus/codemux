import { describe, it, expect } from "vitest";

import type { ChatViewItem, ToolCallItem } from "@/lib/agent-chat/types";

import {
  buildTrailEntries,
  deriveActiveTrailIndex,
  sampleTrailIndices,
  withActiveIndex,
} from "./message-trail";
import { buildTranscriptSlots, type TranscriptSlot } from "./transcript-slots";

function userMsg(seq: number, text = "hi"): ChatViewItem {
  return { kind: "user_message", id: `um-${seq}`, seq, text };
}

function assistantMsg(seq: number, text = "ok"): ChatViewItem {
  return {
    kind: "assistant_message",
    id: `am-${seq}`,
    seq,
    turn_id: "t1",
    text,
    streaming: false,
  };
}

function tool(seq: number): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${seq}`,
    seq,
    tool_use_id: `tu-${seq}`,
    tool_name: "Read",
    input: { file_path: `/f${seq}` },
    status: "done",
    result_content: null,
    approval_request_id: null,
  };
}

function slotIndexMap(slots: TranscriptSlot[]): Map<string, number> {
  const map = new Map<string, number>();
  slots.forEach((s, i) => map.set(s.messageId, i));
  return map;
}

describe("buildTrailEntries", () => {
  it("returns no entries when there are no user turns", () => {
    const slots = buildTranscriptSlots([assistantMsg(0), tool(1)]);
    expect(buildTrailEntries(slots)).toEqual([]);
  });

  it("emits one entry per user turn, in order, with reply snippets", () => {
    const slots = buildTranscriptSlots([
      userMsg(0, "  First question here  "),
      assistantMsg(1, "First reply body"),
      userMsg(2, "Second question"),
      // Two groupable reads fold into one toolGroup slot between the user
      // turn and its assistant reply — the snippet skips it.
      tool(3),
      tool(4),
      assistantMsg(5, "Second reply body"),
      userMsg(6, "Third question"),
    ]);
    const entries = buildTrailEntries(slots);

    expect(entries).toHaveLength(3);
    expect(entries[0]).toMatchObject({
      messageId: "um-0",
      slotIndex: 0,
      turnIndex: 0,
      userText: "First question here", // trimmed
      replySnippet: "First reply body",
    });
    expect(entries[1]).toMatchObject({
      messageId: "um-2",
      turnIndex: 1,
      userText: "Second question",
      replySnippet: "Second reply body", // found past the folded tool group
    });
    // Second turn's slot index is 2 (user), reply lives at slot 4.
    expect(entries[1].slotIndex).toBe(2);
  });

  it("leaves the reply snippet empty when a turn has no assistant reply", () => {
    const slots = buildTranscriptSlots([
      userMsg(0, "no reply yet"),
      userMsg(1, "asked again"),
    ]);
    const entries = buildTrailEntries(slots);
    expect(entries).toHaveLength(2);
    expect(entries[0].replySnippet).toBe("");
    expect(entries[1].replySnippet).toBe("");
  });

  it("stops the reply search at the next user turn", () => {
    const slots = buildTranscriptSlots([
      userMsg(0, "q1"),
      userMsg(1, "q2"),
      assistantMsg(2, "reply to q2"),
    ]);
    const entries = buildTrailEntries(slots);
    expect(entries[0].replySnippet).toBe(""); // q1 has no reply before q2
    expect(entries[1].replySnippet).toBe("reply to q2");
  });
});

describe("deriveActiveTrailIndex", () => {
  // user(0) am(1) user(2) group(3,4) am(5) user(6)
  const slots = buildTranscriptSlots([
    userMsg(0),
    assistantMsg(1),
    userMsg(2),
    tool(3),
    tool(4),
    assistantMsg(5),
    userMsg(6),
  ]);
  const entries = buildTrailEntries(slots); // slotIndex 0, 2, 5
  const byId = slotIndexMap(slots);

  it("returns -1 with no entries", () => {
    expect(deriveActiveTrailIndex([], ["um-0"], byId)).toBe(-1);
  });

  it("returns -1 when nothing is visible", () => {
    expect(deriveActiveTrailIndex(entries, [], byId)).toBe(-1);
  });

  it("returns -1 when the first visible id is unknown", () => {
    expect(deriveActiveTrailIndex(entries, ["ghost"], byId)).toBe(-1);
  });

  it("maps the first visible user row to its own turn", () => {
    expect(deriveActiveTrailIndex(entries, ["um-0"], byId)).toBe(0);
    expect(deriveActiveTrailIndex(entries, ["um-6"], byId)).toBe(2);
  });

  it("maps a visible assistant row to the enclosing turn", () => {
    // am-1 (slot 1) is still turn 0; am-5 (slot 4) is turn 1.
    expect(deriveActiveTrailIndex(entries, ["am-1"], byId)).toBe(0);
    expect(deriveActiveTrailIndex(entries, ["am-5"], byId)).toBe(1);
  });

  it("maps a visible folded tool-group id to the enclosing turn", () => {
    // run:tc-3 is slot 3, inside turn 1 (user at slot 2).
    expect(
      deriveActiveTrailIndex(entries, ["run:tc-3", "am-5"], byId),
    ).toBe(1);
  });

  it("stays -1 while the viewport sits above the first user turn", () => {
    const lead = buildTranscriptSlots([
      assistantMsg(0),
      userMsg(1),
      assistantMsg(2),
      userMsg(3),
    ]);
    const leadEntries = buildTrailEntries(lead); // slotIndex 1, 3
    // am-0 is slot 0, before the first turn's slot 1.
    expect(
      deriveActiveTrailIndex(leadEntries, ["am-0"], slotIndexMap(lead)),
    ).toBe(-1);
  });

  it("clamps to the last turn once scrolled past it", () => {
    const past = buildTranscriptSlots([
      userMsg(0),
      assistantMsg(1),
      userMsg(2),
      assistantMsg(3),
      // trailing assistant continuation well past the last user turn
      assistantMsg(4),
    ]);
    const pastEntries = buildTrailEntries(past); // slotIndex 0, 2
    // am-4 (slot 4) is beyond the last user turn (slot 2) → clamp to index 1.
    expect(
      deriveActiveTrailIndex(pastEntries, ["am-4"], slotIndexMap(past)),
    ).toBe(1);
  });
});

describe("sampleTrailIndices", () => {
  it("returns every index when under the cap", () => {
    expect(sampleTrailIndices(5, 60)).toEqual([0, 1, 2, 3, 4]);
  });

  it("returns an empty list for an empty thread", () => {
    expect(sampleTrailIndices(0, 60)).toEqual([]);
  });

  it("downsamples to the cap, keeping both endpoints and staying sorted", () => {
    const s = sampleTrailIndices(220, 60);
    expect(s.length).toBeLessThanOrEqual(60);
    expect(s[0]).toBe(0);
    expect(s[s.length - 1]).toBe(219);
    for (let i = 1; i < s.length; i++) expect(s[i]).toBeGreaterThan(s[i - 1]);
  });
});

describe("withActiveIndex", () => {
  it("returns the sample unchanged when active is already present or unset", () => {
    const sample = [0, 3, 6, 9];
    expect(withActiveIndex(sample, 6)).toBe(sample);
    expect(withActiveIndex(sample, -1)).toBe(sample);
  });

  it("injects a dropped active index without growing the list", () => {
    const sample = [0, 3, 6, 9];
    const out = withActiveIndex(sample, 4);
    expect(out).toHaveLength(sample.length);
    expect(out).toContain(4);
    // stays sorted
    for (let i = 1; i < out.length; i++) expect(out[i]).toBeGreaterThan(out[i - 1]);
  });
});
