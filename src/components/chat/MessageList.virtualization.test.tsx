/// <reference types="@testing-library/jest-dom/vitest" />
/** Regression coverage for the transcript's virtual-list contract. */
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type {
  AssistantMessageItem,
  ChatViewItem,
} from "@/lib/agent-chat/types";

import { MessageList } from "./MessageList";

const renderCounts = new Map<string, number>();
vi.mock("./AssistantMessage", () => ({
  AssistantMessage: ({ item }: { item: AssistantMessageItem }) => {
    renderCounts.set(item.id, (renderCounts.get(item.id) ?? 0) + 1);
    return <div data-am-row>{item.text}</div>;
  },
}));

const { lastListProps } = vi.hoisted(() => ({
  lastListProps: { current: null as Record<string, any> | null },
}));

// jsdom has no layout. This double models a bottom-pinned viewport by
// mounting a bounded tail window while exposing the production configuration
// for assertions.
vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return {
    LegendList: React.forwardRef(function LegendListMock(
      props: Record<string, any>,
      ref: React.ForwardedRef<any>,
    ) {
      lastListProps.current = props;
      React.useImperativeHandle(ref, () => ({
        getScrollableNode: () => document.createElement("div"),
        getState: () => ({ isAtEnd: true, listen: () => () => undefined }),
        scrollToEnd: () => Promise.resolve(),
        scrollToIndex: () => Promise.resolve(),
      }));
      const start = Math.max(0, props.data.length - 12);
      return (
        <div data-slot="transcript-list">
          {props.ListHeaderComponent}
          {props.data.slice(start).map((item: unknown, offset: number) => {
            const index = start + offset;
            return (
              <React.Fragment key={props.keyExtractor(item, index)}>
                {props.renderItem({ item, index })}
              </React.Fragment>
            );
          })}
          {props.ListFooterComponent}
        </div>
      );
    }),
  };
});

afterEach(() => {
  cleanup();
  renderCounts.clear();
  lastListProps.current = null;
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

describe("MessageList virtualization & memoization", () => {
  it("keeps the mounted DOM bounded for a long transcript", () => {
    const messages: ChatViewItem[] = [];
    for (let i = 0; i < 1000; i++) {
      messages.push(assistantMsg(i, `message body ${i}`));
    }
    renderList(messages);

    expect(document.querySelectorAll("[data-am-row]")).toHaveLength(12);
    expect(screen.getByText("message body 999")).toBeInTheDocument();
    expect(screen.queryByText("message body 0")).not.toBeInTheDocument();
    expect(lastListProps.current).toMatchObject({
      drawDistance: 800,
      estimatedItemSize: 112,
      initialScrollAtEnd: true,
      recycleItems: false,
      maintainVisibleContentPosition: { data: true, size: false },
    });
  });

  it("re-renders exactly one mounted leaf for a streaming token delta", () => {
    const initial: ChatViewItem[] = [];
    for (let i = 0; i < 12; i++) {
      initial.push(assistantMsg(i, `message body ${i}`));
    }
    const { rerender } = renderList(initial);
    const before = new Map(renderCounts);

    const next = initial.slice();
    next[11] = assistantMsg(11, "message body 11 plus-token");
    rerender(<MessageList messages={next} {...noopHandlers} />);

    expect(screen.getByText("message body 11 plus-token")).toBeInTheDocument();
    for (let i = 0; i < 11; i++) {
      expect(renderCounts.get(`am-${i}`)).toBe(before.get(`am-${i}`));
    }
    expect(renderCounts.get("am-11")).toBe((before.get("am-11") ?? 0) + 1);
  });

  it("puts every windowed row on the shared chat column with symmetric gutters", () => {
    // The composer lives outside this scroller on the same CHAT_COLUMN
    // rails. Reserving the scrollbar gutter on one edge only would shrink
    // the box the rows are centered in and slide the transcript half a
    // scrollbar off-centre from the composer.
    renderList([assistantMsg(0, "message body 0")]);

    expect(lastListProps.current?.className).toContain(
      "[scrollbar-gutter:stable_both-edges]",
    );
    const rails = document.querySelectorAll(`.${CSS.escape("max-w-[792px]")}`);
    // Header, the single row, and the footer.
    expect(rails).toHaveLength(3);
    for (const rail of rails) {
      expect(rail).toHaveClass("mx-auto", "w-full", "max-w-[792px]", "px-4");
    }
  });

  it("pins data, item, viewport, and footer growth while following", () => {
    renderList([assistantMsg(0, "message body 0")]);
    expect(lastListProps.current?.maintainScrollAtEnd).toEqual({
      animated: false,
      on: {
        dataChange: true,
        footerLayout: true,
        itemLayout: true,
        layout: true,
      },
    });
    expect(lastListProps.current?.maintainScrollAtEndThreshold).toBe(0.03);
  });
});
