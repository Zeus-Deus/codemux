/// <reference types="@testing-library/jest-dom/vitest" />
/** Prop-stability contract for the virtual list: every value handed to
 *  LegendList that is not derived from the transcript itself must keep its
 *  identity across renders, or the list re-does work the memo boundaries
 *  above it were meant to prevent. */
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import type {
  AssistantMessageItem,
  ChatViewItem,
} from "@/lib/agent-chat/types";

const { listPropsLog } = vi.hoisted(() => ({
  listPropsLog: { entries: [] as Record<string, any>[] },
}));

vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return {
    LegendList: React.forwardRef(function LegendListMock(
      props: Record<string, any>,
      ref: React.ForwardedRef<any>,
    ) {
      listPropsLog.entries.push(props);
      React.useImperativeHandle(ref, () => ({
        getScrollableNode: () => document.createElement("div"),
        getState: () => ({ isAtEnd: true, listen: () => () => undefined }),
        scrollToEnd: () => Promise.resolve(),
        scrollToIndex: () => Promise.resolve(),
      }));
      return <div data-slot="transcript-list" />;
    }),
  };
});

const { MessageList } = await import("./MessageList");

afterEach(() => {
  cleanup();
  listPropsLog.entries = [];
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

describe("MessageList prop stability", () => {
  it("keeps keyExtractor, itemsAreEqual and getItemType at module identity", () => {
    const messages: ChatViewItem[] = [assistantMsg(0, "a")];
    const { rerender } = render(
      <MessageList messages={messages} {...noopHandlers} />,
    );
    rerender(
      <MessageList
        messages={[...messages, assistantMsg(1, "b")]}
        {...noopHandlers}
      />,
    );

    const [first, ...rest] = listPropsLog.entries;
    expect(rest.length).toBeGreaterThan(0);
    for (const props of rest) {
      expect(props.keyExtractor).toBe(first.keyExtractor);
      expect(props.itemsAreEqual).toBe(first.itemsAreEqual);
      expect(props.getItemType).toBe(first.getItemType);
    }
  });

  it("keeps the list header element stable while its own input is unchanged", () => {
    const messages: ChatViewItem[] = [assistantMsg(0, "a")];
    const { rerender } = render(
      <MessageList
        messages={messages}
        sessionStartedAt={1700000000000}
        {...noopHandlers}
      />,
    );
    rerender(
      <MessageList
        messages={[...messages, assistantMsg(1, "b")]}
        sessionStartedAt={1700000000000}
        {...noopHandlers}
      />,
    );

    const [first, ...rest] = listPropsLog.entries;
    for (const props of rest) {
      expect(props.ListHeaderComponent).toBe(first.ListHeaderComponent);
    }
  });

  it("rebuilds the list header when the session start actually moves", () => {
    const messages: ChatViewItem[] = [assistantMsg(0, "a")];
    const { rerender } = render(
      <MessageList
        messages={messages}
        sessionStartedAt={1700000000000}
        {...noopHandlers}
      />,
    );
    const lastProps = () =>
      listPropsLog.entries[listPropsLog.entries.length - 1];
    const before = lastProps().ListHeaderComponent;
    rerender(
      <MessageList
        messages={messages}
        sessionStartedAt={1700000009999}
        {...noopHandlers}
      />,
    );
    expect(lastProps().ListHeaderComponent).not.toBe(before);
  });

  it("skips entirely when every prop is unchanged (memo boundary)", () => {
    const messages: ChatViewItem[] = [assistantMsg(0, "a")];
    const element = <MessageList messages={messages} {...noopHandlers} />;
    const { rerender } = render(element);
    const renders = listPropsLog.entries.length;
    rerender(<MessageList messages={messages} {...noopHandlers} />);
    expect(listPropsLog.entries.length).toBe(renders);
  });
});
