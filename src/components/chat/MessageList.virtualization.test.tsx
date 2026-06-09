/// <reference types="@testing-library/jest-dom/vitest" />
/**
 * Virtualization guarantees for the transcript (issue #77):
 *
 *  1. A long session mounts only a bounded window of rows in the DOM,
 *     not one node per message.
 *  2. A streaming token delta (tail row replaced in place by the
 *     reducer) re-renders exactly one row — the React.memo win from
 *     before virtualization is preserved.
 *
 * Rendered inside `VirtuosoMockContext` because jsdom has no layout:
 * the mock provides synthetic viewport/row heights so Virtuoso can
 * decide what to mount.
 */
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { VirtuosoMockContext } from "react-virtuoso";

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
  return render(
    <VirtuosoMockContext.Provider
      value={{ viewportHeight: 2000, itemHeight: 100 }}
    >
      <MessageList messages={messages} {...noopHandlers} />
    </VirtuosoMockContext.Provider>,
  );
}

describe("MessageList virtualization", () => {
  it("mounts only a bounded window of rows for a 1,000-message session", () => {
    const messages: ChatViewItem[] = [];
    for (let i = 0; i < 1000; i++) {
      messages.push(assistantMsg(i, `message body ${i}`));
    }
    renderList(messages);

    const mounted = document.querySelectorAll("[data-am-row]").length;
    // 2000px viewport / 100px rows = 20 visible (+ a little overscan).
    expect(mounted).toBeGreaterThan(0);
    expect(mounted).toBeLessThan(60);
  });

  it("re-renders exactly one row when a token delta mutates the tail", () => {
    // 12 rows — all inside the mock 20-row window, so every row is
    // mounted and a memo miss anywhere would be observable.
    const initial: ChatViewItem[] = [];
    for (let i = 0; i < 12; i++) {
      initial.push(assistantMsg(i, `message body ${i}`));
    }
    const { rerender } = renderList(initial);
    expect(screen.getByText("message body 11")).toBeInTheDocument();

    const before = new Map(renderCounts);

    // Simulate the reducer's `replaceItem` for a streaming delta: a
    // fresh array, every row identity preserved except the tail.
    const next = initial.slice();
    next[11] = { ...assistantMsg(11, "message body 11 plus-token") };
    rerender(
      <VirtuosoMockContext.Provider
        value={{ viewportHeight: 2000, itemHeight: 100 }}
      >
        <MessageList messages={next} {...noopHandlers} />
      </VirtuosoMockContext.Provider>,
    );
    expect(
      screen.getByText("message body 11 plus-token"),
    ).toBeInTheDocument();

    for (let i = 0; i < 11; i++) {
      expect(renderCounts.get(`am-${i}`)).toBe(before.get(`am-${i}`));
    }
    expect(renderCounts.get("am-11")).toBe((before.get("am-11") ?? 0) + 1);
  });
});
