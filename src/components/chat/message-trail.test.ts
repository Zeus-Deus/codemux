import { describe, it, expect } from "vitest";

import type { ChatViewItem, ToolCallItem } from "@/lib/agent-chat/types";

import {
  buildTrailEntries,
  findTrailEntryAtOffset,
  sampleTrailIndices,
} from "./message-trail";
import { buildTranscriptSlots } from "./transcript-slots";

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

describe("buildTrailEntries", () => {
  it("keeps in-flight question answers inside their existing turn", () => {
    const slots = buildTranscriptSlots([
      userMsg(0, "Build the sample"),
      {
        kind: "user_message",
        id: "reply",
        seq: 1,
        text: "SQLite",
        inflight: true,
      },
      assistantMsg(2, "Using SQLite"),
    ]);
    const entries = buildTrailEntries(slots);
    expect(entries).toHaveLength(1);
    expect(entries[0].replySnippet).toBe("Using SQLite");
    expect(slots.find((slot) => slot.messageId === "reply")?.scrollAnchor).toBe(
      false,
    );
  });
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

describe("findTrailEntryAtOffset", () => {
  const entries = buildTrailEntries(
    buildTranscriptSlots([
      assistantMsg(0),
      userMsg(1),
      assistantMsg(2),
      userMsg(3),
      assistantMsg(4),
      userMsg(5),
    ]),
  );
  const positions = [0, 100, 200, 300, 400, 500];
  const positionAtIndex = (slotIndex: number) => positions[slotIndex];

  it("finds the latest user turn above the viewport offset", () => {
    expect(findTrailEntryAtOffset(entries, 99, positionAtIndex)).toBe(-1);
    expect(findTrailEntryAtOffset(entries, 100, positionAtIndex)).toBe(0);
    expect(findTrailEntryAtOffset(entries, 299, positionAtIndex)).toBe(0);
    expect(findTrailEntryAtOffset(entries, 300, positionAtIndex)).toBe(1);
    expect(findTrailEntryAtOffset(entries, 999, positionAtIndex)).toBe(2);
  });

  it("does logarithmic position lookups for a long trail", () => {
    const longEntries = Array.from({ length: 1024 }, (_, i) => ({
      ...entries[0],
      messageId: `turn-${i}`,
      slotIndex: i * 2,
      turnIndex: i,
    }));
    let lookups = 0;
    const found = findTrailEntryAtOffset(longEntries, 1701, (slotIndex) => {
      lookups += 1;
      return slotIndex;
    });
    expect(found).toBe(850);
    expect(lookups).toBeLessThanOrEqual(11);
  });
});
