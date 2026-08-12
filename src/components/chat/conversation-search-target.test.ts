import { describe, expect, it } from "vitest";

import type { ChatViewItem } from "@/lib/agent-chat/types";

import { resolveConversationSearchTargetIndex } from "./conversation-search-target";
import type { TranscriptSlot } from "./transcript-slots";

function itemSlot(item: ChatViewItem): TranscriptSlot {
  return {
    key: item.id,
    messageId: item.id,
    scrollAnchor: item.kind === "user_message",
    side: item.kind === "user_message" ? "user" : "assistant",
    showAvatar: false,
    turnStart: false,
    body: { kind: "item", item },
  };
}

describe("resolveConversationSearchTargetIndex", () => {
  it("targets the exact persisted user or assistant event id", () => {
    const slots = [
      itemSlot({
        kind: "user_message",
        id: "user-1",
        seq: 0,
        text: "first",
        source_event_id: 41,
      }),
      itemSlot({
        kind: "assistant_message",
        id: "assistant-1",
        seq: 1,
        turn_id: "turn-1",
        text: "answer",
        streaming: false,
        source_event_id: 42,
      }),
    ];

    expect(
      resolveConversationSearchTargetIndex(slots, {
        messageId: 42,
        turnId: "turn-1",
      }),
    ).toBe(1);
  });

  it("falls back to a settled turn fold when exact prose is hidden", () => {
    const slots: TranscriptSlot[] = [
      itemSlot({
        kind: "user_message",
        id: "user-1",
        seq: 0,
        text: "prompt",
      }),
      {
        key: "fold-turn-9",
        messageId: "fold-turn-9",
        scrollAnchor: false,
        side: "assistant",
        showAvatar: false,
        turnStart: false,
        body: {
          kind: "turn_fold",
          turnId: "turn-9",
          label: "Worked",
          expanded: false,
          hiddenCount: 3,
          failedCount: 0,
        },
      },
    ];

    expect(
      resolveConversationSearchTargetIndex(slots, {
        messageId: 999,
        turnId: "turn-9",
      }),
    ).toBe(1);
  });

  it("maps title-only hits to the first transcript row", () => {
    const slots = [
      itemSlot({
        kind: "user_message",
        id: "user-1",
        seq: 0,
        text: "prompt",
      }),
    ];
    expect(
      resolveConversationSearchTargetIndex(slots, {
        messageId: null,
        turnId: null,
      }),
    ).toBe(0);
  });
});
