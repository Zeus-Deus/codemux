/// <reference types="@testing-library/jest-dom/vitest" />
/** Regression coverage for the transcript's virtual-list contract. */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";

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

/** Rows LegendList keeps mounted around the viewport in this double. */
const WINDOW_SIZE = 12;

const { lastListProps, listView } = vi.hoisted(() => ({
  lastListProps: { current: null as Record<string, any> | null },
  // `null` = pinned to the tail, a number = the first mounted row index.
  // Only the component's own scroll calls move it, so "the prompt is in the
  // mounted window" is a real consequence of what the component did.
  listView: { start: null as number | null, rowHeight: 100 },
}));

// jsdom has no layout. This double models a windowed viewport: it mounts a
// bounded range, moves that range only when the component scrolls, and
// reports measured geometry — enough to exercise the new-turn scroll contract
// end to end while exposing the production configuration for assertions.
vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return {
    LegendList: React.forwardRef(function LegendListMock(
      props: Record<string, any>,
      ref: React.ForwardedRef<any>,
    ) {
      lastListProps.current = props;
      const [, forceRender] = React.useReducer((n: number) => n + 1, 0);
      const nodeRef = React.useRef<HTMLDivElement>(null);
      React.useImperativeHandle(ref, () => ({
        getScrollableNode: () => nodeRef.current,
        getState: () => ({
          isAtEnd: listView.start === null,
          data: props.data,
          scroll: (listView.start ?? 0) * listView.rowHeight,
          scrollLength: 500,
          positionAtIndex: (index: number) => index * listView.rowHeight,
          sizeAtIndex: () => listView.rowHeight,
          elementAtIndex: () => null,
          listen: () => () => undefined,
        }),
        scrollToEnd: () => {
          listView.start = null;
          forceRender();
          return Promise.resolve();
        },
        scrollToIndex: (params: { index: number }) => {
          listView.start = params.index;
          forceRender();
          return Promise.resolve();
        },
        scrollToOffset: (params: { offset: number }) => {
          listView.start = Math.floor(params.offset / listView.rowHeight);
          forceRender();
          return Promise.resolve();
        },
      }));
      const maxStart = Math.max(0, props.data.length - WINDOW_SIZE);
      const start =
        listView.start === null
          ? maxStart
          : Math.max(0, Math.min(listView.start, maxStart));
      return (
        <div ref={nodeRef} data-slot="transcript-list">
          {props.ListHeaderComponent}
          {props.data
            .slice(start, start + WINDOW_SIZE)
            .map((item: unknown, offset: number) => {
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
  listView.start = null;
  listView.rowHeight = 100;
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

describe("MessageList anchored new-turn contract (windowed list)", () => {
  /** Let the component's nested rAF chains run. */
  async function flushFrames(count = 4) {
    for (let i = 0; i < count; i++) {
      await act(
        () => new Promise<void>((r) => requestAnimationFrame(() => r())),
      );
    }
  }

  function hydratedTranscript(count: number): ChatViewItem[] {
    const messages: ChatViewItem[] = [];
    for (let i = 0; i < count; i++) {
      messages.push(assistantMsg(i, `message body ${i}`));
    }
    return messages;
  }

  const prompt: ChatViewItem = {
    kind: "user_message",
    id: "um-sent",
    seq: 10_000,
    text: "the prompt I just sent",
    clientNonce: "nonce-sent",
  };

  /** Drive the anchor's measurement callback the way LegendList would once
   *  it has laid the row out and sized the reserved space below it. */
  function measureAnchor() {
    const config = lastListProps.current?.anchoredEndSpace;
    expect(config).toBeDefined();
    act(() => {
      config.onReady?.({
        anchorIndex: config.anchorIndex,
        anchorKey: "um-sent",
        size: 320,
      });
    });
  }

  it("surfaces a send from deep history, even after a workspace away/back", async () => {
    // A long hydrated thread the reader has scrolled up into.
    const history = hydratedTranscript(500);
    const { unmount } = render(
      <MessageList messages={history} threadKey="thread-1" {...noopHandlers} />,
    );
    listView.start = 40; // deep in history, far from the tail
    unmount();

    // Workspace switched away and back: the pane remounts on the same
    // thread. This must behave exactly like a fresh hydrated open.
    const sent = [...history, prompt];
    render(
      <MessageList
        messages={sent}
        threadKey="thread-1"
        sendAnchor={{ clientNonce: "nonce-sent", nonce: 1 }}
        showThinking
        streaming
        {...noopHandlers}
      />,
    );
    measureAnchor();
    await flushFrames();

    // The new prompt is mounted and the live tail marker sits below it in
    // the reserved space — not stranded off-screen at index 500.
    expect(screen.getByText("the prompt I just sent")).toBeInTheDocument();
    expect(screen.getByText(/Working|Writing|Thinking/)).toBeInTheDocument();
    // Nothing about this is a reader navigating away.
    expect(screen.queryByText("Jump to latest")).toBeNull();
  });

  it("stays windowed while the anchor reserves response space", async () => {
    const sent = [...hydratedTranscript(500), prompt];
    render(
      <MessageList
        messages={sent}
        threadKey="thread-1"
        sendAnchor={{ clientNonce: "nonce-sent", nonce: 1 }}
        {...noopHandlers}
      />,
    );
    measureAnchor();
    await flushFrames();

    // Bounded DOM is the whole point of the virtualizer: reserving end space
    // must not force the tail of a 500-row thread to mount.
    expect(
      document.querySelectorAll("[data-am-row]").length,
    ).toBeLessThanOrEqual(12);
    expect(lastListProps.current).toMatchObject({
      drawDistance: 800,
      recycleItems: false,
    });
    expect(lastListProps.current?.anchoredEndSpace).toMatchObject({
      anchorIndex: 500,
      anchorOffset: 16,
    });
    // The built-in end pin is off while we own the position.
    expect(lastListProps.current?.maintainScrollAtEnd).toBe(false);
  });

  it("opens a settled thread at the latest row on remount, with no stale re-park", async () => {
    const sent = [...hydratedTranscript(500), prompt];
    const { unmount } = render(
      <MessageList
        messages={sent}
        threadKey="thread-1"
        sendAnchor={{ clientNonce: "nonce-sent", nonce: 1 }}
        showThinking
        streaming
        {...noopHandlers}
      />,
    );
    measureAnchor();
    await flushFrames();
    // Mid-turn the window is parked on the prompt, not the tail.
    expect(screen.getByText("the prompt I just sent")).toBeInTheDocument();
    unmount();

    // The turn settled, so the pane expired the anchor. A later remount on
    // the same thread (a subagent drill-in and back, say) must open at the
    // latest row like any hydrated thread — not re-park a finished prompt.
    listView.start = null; // `initialScrollAtEnd`
    render(
      <MessageList
        messages={sent}
        threadKey="thread-1"
        sendAnchor={null}
        {...noopHandlers}
      />,
    );
    await flushFrames();

    expect(lastListProps.current?.anchoredEndSpace).toBeUndefined();
    expect(lastListProps.current?.maintainScrollAtEnd).not.toBe(false);
    expect(screen.getByText("the prompt I just sent")).toBeInTheDocument();
    expect(screen.getByText("message body 499")).toBeInTheDocument();
    // Nothing re-parked the view: the mounted window is still the tail.
    expect(screen.queryByText("message body 480")).toBeNull();
  });

  it("resolves the anchor by nonce when a queued follow-up lands after it", async () => {
    const queued: ChatViewItem = {
      kind: "user_message",
      id: "um-queued",
      seq: 10_001,
      text: "queued follow-up",
      clientNonce: "nonce-queued",
      queued: { queuedId: "q-1" },
    };
    render(
      <MessageList
        messages={[...hydratedTranscript(20), prompt, queued]}
        threadKey="thread-1"
        sendAnchor={{ clientNonce: "nonce-sent", nonce: 1 }}
        {...noopHandlers}
      />,
    );
    // Index 20 is the sent prompt; 21 is the queued row that follows it.
    expect(lastListProps.current?.anchoredEndSpace?.anchorIndex).toBe(20);
  });
});
