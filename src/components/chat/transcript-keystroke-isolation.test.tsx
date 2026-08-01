/// <reference types="@testing-library/jest-dom/vitest" />
/**
 * Exit gate for the chat-update isolation work: typing in the composer must
 * render ZERO timeline rows.
 *
 * The harness mirrors `AgentChatPane`'s store subscriptions (field-level
 * groups over the real store) rather than mounting the 2,800-line pane and
 * its ~40 Tauri commands. What is under test is the causal chain the pane
 * relies on: a draft write leaves the timeline group reference-equal, and
 * the memo boundary on `ChatTranscript` turns that into no row renders.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";
import { useShallow } from "zustand/react/shallow";

import type {
  AssistantMessageItem,
  ChatViewItem,
} from "@/lib/agent-chat/types";
import { useAgentChatStore } from "@/stores/agent-chat-store";

const rowRenders = new Map<string, number>();
vi.mock("./AssistantMessage", () => ({
  AssistantMessage: ({ item }: { item: AssistantMessageItem }) => {
    rowRenders.set(item.id, (rowRenders.get(item.id) ?? 0) + 1);
    return <div data-am-row>{item.text}</div>;
  },
}));

// jsdom has no layout; mount a bounded tail window like the transcript's
// virtualizer would.
vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return {
    LegendList: React.forwardRef(function LegendListMock(
      props: Record<string, any>,
      ref: React.ForwardedRef<any>,
    ) {
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

const { ChatTranscript } = await import("./ChatTranscript");

const THREAD = "t1";
const EMPTY_MESSAGES = Object.freeze([]) as unknown as ChatViewItem[];

const noop = () => undefined;
const paneRenders = { count: 0 };

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

/** The subscription shape `AgentChatPane` uses, minus everything the
 *  transcript does not consume. */
function Pane({ threadId }: { threadId: string }) {
  const draft = useAgentChatStore(
    (s) => s.threads[threadId]?.inputDraft ?? "",
  );
  const timeline = useAgentChatStore(
    useShallow((s) => {
      const t = s.threads[threadId];
      return {
        messages: t?.messages ?? EMPTY_MESSAGES,
        streaming: t?.streaming ?? false,
        stalled: t?.stalled ?? null,
        interrupted: t?.interrupted ?? false,
      };
    }),
  );
  paneRenders.count += 1;
  return (
    <>
      <textarea data-testid="composer" value={draft} readOnly />
      <ChatTranscript
        messages={timeline.messages}
        streaming={timeline.streaming}
        stalled={timeline.stalled}
        interrupted={timeline.interrupted}
        onRespondToRequest={noop}
        onAcceptPlan={noop}
        onRejectPlan={noop}
      />
    </>
  );
}

function seedThread(count: number) {
  const messages: ChatViewItem[] = [];
  for (let i = 0; i < count; i += 1) {
    messages.push(assistantMsg(i, `message body ${i}`));
  }
  useAgentChatStore.getState().ensureThread(THREAD);
  useAgentChatStore.setState((s) => ({
    threads: {
      ...s.threads,
      [THREAD]: { ...s.threads[THREAD], messages, nextSeq: count },
    },
  }));
}

beforeEach(() => {
  useAgentChatStore.setState({ threads: {} });
  rowRenders.clear();
  paneRenders.count = 0;
});

afterEach(() => {
  cleanup();
});

describe("keystroke isolation", () => {
  it("a draft write leaves every timeline field reference-equal", () => {
    seedThread(20);
    const before = useAgentChatStore.getState().threads[THREAD];

    useAgentChatStore.getState().setInputDraft(THREAD, "hello");

    const after = useAgentChatStore.getState().threads[THREAD];
    // The slice object itself is replaced (that is what re-renders the
    // composer) but nothing the transcript reads moved.
    expect(after).not.toBe(before);
    expect(after.messages).toBe(before.messages);
    expect(after.streaming).toBe(before.streaming);
    expect(after.stalled).toBe(before.stalled);
    expect(after.interrupted).toBe(before.interrupted);
    expect(after.inputDraft).toBe("hello");
  });

  it("renders no timeline rows when the draft changes", () => {
    seedThread(2500);
    render(<Pane threadId={THREAD} />);

    const paneBefore = paneRenders.count;
    const rowsBefore = new Map(rowRenders);
    expect(rowsBefore.size).toBeGreaterThan(0);

    act(() => {
      for (const char of "hello") {
        const current =
          useAgentChatStore.getState().threads[THREAD]?.inputDraft ?? "";
        useAgentChatStore.getState().setInputDraft(THREAD, current + char);
      }
    });

    // The pane DOES re-render — it owns the composer.
    expect(paneRenders.count).toBeGreaterThan(paneBefore);
    // The transcript does not.
    expect(rowRenders.size).toBe(rowsBefore.size);
    for (const [id, count] of rowsBefore) {
      expect(rowRenders.get(id)).toBe(count);
    }
  });

  it("still renders the changed row when a message actually changes", () => {
    seedThread(12);
    render(<Pane threadId={THREAD} />);
    const before = new Map(rowRenders);

    act(() => {
      useAgentChatStore.setState((s) => {
        const slice = s.threads[THREAD];
        const messages = slice.messages.slice();
        messages[11] = assistantMsg(11, "message body 11 plus-token");
        return { threads: { ...s.threads, [THREAD]: { ...slice, messages } } };
      });
    });

    for (let i = 0; i < 11; i += 1) {
      expect(rowRenders.get(`am-${i}`)).toBe(before.get(`am-${i}`));
    }
    expect(rowRenders.get("am-11")).toBe((before.get("am-11") ?? 0) + 1);
  });

  it("mounts a bounded row window for both a short and a very long history", () => {
    seedThread(50);
    render(<Pane threadId={THREAD} />);
    const shortHistoryRows = document.querySelectorAll("[data-am-row]").length;

    cleanup();
    useAgentChatStore.setState({ threads: {} });
    rowRenders.clear();

    seedThread(5000);
    render(<Pane threadId={THREAD} />);
    const longHistoryRows = document.querySelectorAll("[data-am-row]").length;

    expect(shortHistoryRows).toBe(longHistoryRows);
    expect(longHistoryRows).toBeLessThanOrEqual(40);
  });
});
