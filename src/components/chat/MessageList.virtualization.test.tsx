/// <reference types="@testing-library/jest-dom/vitest" />
/**
 * Rendering + memoization guarantees for the transcript on MessageScroller:
 *
 *  1. Every message row mounts into the DOM — the scroller keeps rows
 *     cheap with `content-visibility:auto` row containment (not by
 *     unmounting off-screen rows), so jsdom sees them all.
 *  2. A streaming token delta (tail row replaced in place by the reducer)
 *     re-renders exactly one row — the memoized-row win holds on the
 *     MessageScroller.
 *
 * The scroller renders plain rows: no external virtualization context to
 * mock.
 */
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type {
  AssistantMessageItem,
  ChatViewItem,
} from "@/lib/agent-chat/types";

import { MessageList } from "./MessageList";

// Replace the prose renderer with a counting stub so we can observe
// per-row render counts without markdown noise.
const renderCounts = new Map<string, number>();
vi.mock("./AssistantMessage", () => ({
  AssistantMessage: ({ item }: { item: AssistantMessageItem }) => {
    renderCounts.set(item.id, (renderCounts.get(item.id) ?? 0) + 1);
    return <div data-am-row>{item.text}</div>;
  },
}));

afterEach(() => {
  cleanup();
  renderCounts.clear();
});

function assistantMsg(seq: number, text: string): AssistantMessageItem {
  return {
    kind: "assistant_message",
    id: `am-${seq}`,
    seq,
    turn_id: "turn-1",
    text,
    streaming: false,
  };
}

const noopHandlers = {
  onRespondToRequest: vi.fn(),
  onAcceptPlan: vi.fn(),
  onRejectPlan: vi.fn(),
};

function renderList(messages: ChatViewItem[]) {
  return render(<MessageList messages={messages} {...noopHandlers} />);
}

describe("MessageList rendering & memoization", () => {
  it("mounts every message row (content-visibility, not row unmounting)", () => {
    const messages: ChatViewItem[] = [];
    for (let i = 0; i < 40; i++) {
      messages.push(assistantMsg(i, `message body ${i}`));
    }
    renderList(messages);
    expect(document.querySelectorAll("[data-am-row]").length).toBe(40);
  });

  it("re-renders exactly one row when a token delta mutates the tail", () => {
    const initial: ChatViewItem[] = [];
    for (let i = 0; i < 12; i++) {
      initial.push(assistantMsg(i, `message body ${i}`));
    }
    const { rerender } = renderList(initial);
    expect(screen.getByText("message body 11")).toBeInTheDocument();

    const before = new Map(renderCounts);

    // Simulate the reducer's `replaceItem` for a streaming delta: a fresh
    // array, every row identity preserved except the tail.
    const next = initial.slice();
    next[11] = { ...assistantMsg(11, "message body 11 plus-token") };
    rerender(<MessageList messages={next} {...noopHandlers} />);
    expect(
      screen.getByText("message body 11 plus-token"),
    ).toBeInTheDocument();

    for (let i = 0; i < 11; i++) {
      expect(renderCounts.get(`am-${i}`)).toBe(before.get(`am-${i}`));
    }
    expect(renderCounts.get("am-11")).toBe((before.get("am-11") ?? 0) + 1);
  });
});
