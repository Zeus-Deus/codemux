/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ChatViewItem } from "@/lib/agent-chat/types";

import { MessageTrail } from "./MessageTrail";
import { buildTranscriptSlots } from "./transcript-slots";

// Drive the jump path through a spy while keeping the rest of the engine
// (Provider / Root / the visibility hook) real — the same partial-mock
// approach the chat tests use for module-level engine access. jsdom has no
// IntersectionObserver, so the visibility hook falls back to rect math and
// reports nothing visible; that's fine here (we assert structure + click).
const scrollToMessage = vi.fn(
  (_messageId: string, _opts?: Record<string, unknown>) => true,
);
vi.mock("@/components/ui/message-scroller", async (importActual) => {
  const actual =
    await importActual<typeof import("@/components/ui/message-scroller")>();
  return {
    ...actual,
    useMessageScroller: () => ({
      scrollToMessage,
      scrollToEnd: vi.fn(() => true),
      scrollToStart: vi.fn(() => true),
    }),
  };
});

// Imported after the mock is registered.
import {
  MessageScroller,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";

afterEach(() => {
  cleanup();
  scrollToMessage.mockClear();
});

function userMsg(seq: number, text: string): ChatViewItem {
  return { kind: "user_message", id: `um-${seq}`, seq, text };
}

function assistantMsg(seq: number, text: string): ChatViewItem {
  return {
    kind: "assistant_message",
    id: `am-${seq}`,
    seq,
    turn_id: "t1",
    text,
    streaming: false,
  };
}

/** N user turns, each followed by an assistant reply. */
function turns(n: number): ChatViewItem[] {
  const out: ChatViewItem[] = [];
  for (let i = 0; i < n; i++) {
    out.push(userMsg(i * 2, `Question number ${i + 1} about the transcript`));
    out.push(assistantMsg(i * 2 + 1, `Reply to question ${i + 1}`));
  }
  return out;
}

function renderTrail(messages: ChatViewItem[]) {
  const slots = buildTranscriptSlots(messages);
  return render(
    <MessageScrollerProvider>
      <MessageScroller>
        <MessageTrail slots={slots} />
      </MessageScroller>
    </MessageScrollerProvider>,
  );
}

describe("MessageTrail", () => {
  it("renders nothing below the turn threshold", () => {
    const { container } = renderTrail(turns(2));
    expect(container.querySelector("nav")).toBeNull();
  });

  it("renders one tick per turn with jump aria-labels at >= 3 turns", () => {
    renderTrail(turns(3));
    const nav = screen.getByRole("navigation", { name: "Conversation turns" });
    expect(nav).toBeInTheDocument();

    const buttons = screen.getAllByRole("button");
    expect(buttons).toHaveLength(3);
    expect(
      screen.getByRole("button", { name: /Jump to turn 1: Question number 1/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Jump to turn 3: Question number 3/ }),
    ).toBeInTheDocument();
  });

  it("jumps to the turn's row on click (align start)", () => {
    renderTrail(turns(4));
    fireEvent.click(
      screen.getByRole("button", { name: /Jump to turn 2:/ }),
    );
    expect(scrollToMessage).toHaveBeenCalled();
    const [messageId, opts] = scrollToMessage.mock.calls[0];
    // Turn 2 is the second user message (seq 2 → id um-2).
    expect(messageId).toBe("um-2");
    expect(opts).toMatchObject({ align: "start", behavior: "auto" });
  });

  it("shows a hover preview card with the prompt and reply snippet", () => {
    renderTrail(turns(3));
    const button = screen.getByRole("button", { name: /Jump to turn 2:/ });
    fireEvent.focus(button); // focus shows the preview immediately (no delay)
    expect(screen.getByText("Question number 2 about the transcript")).toBeInTheDocument();
    expect(screen.getByText("Reply to question 2")).toBeInTheDocument();
  });

  it("forwards rail wheel as a native wheel event on the viewport plus a scrollTop write", () => {
    // The rail resolves the viewport as a sibling under the scroller root,
    // so this render includes a real Viewport (the other tests omit it).
    const slots = buildTranscriptSlots(turns(3));
    const { container } = render(
      <MessageScrollerProvider>
        <MessageScroller>
          <MessageScrollerViewport />
          <MessageTrail slots={slots} />
        </MessageScroller>
      </MessageScrollerProvider>,
    );

    const viewport = container.querySelector<HTMLElement>(
      '[data-slot="message-scroller-viewport"]',
    );
    expect(viewport).not.toBeNull();

    // A raw scrollTop write fires only `scroll` — the scroller engine unpins
    // its stick-to-bottom state only on a real `wheel` event reaching the
    // viewport, so the forwarder must dispatch one before scrolling.
    const wheelSpy = vi.fn();
    viewport!.addEventListener("wheel", wheelSpy);
    const before = viewport!.scrollTop;

    fireEvent.wheel(
      screen.getByRole("navigation", { name: "Conversation turns" }),
      { deltaY: 120 },
    );

    expect(wheelSpy).toHaveBeenCalledTimes(1);
    const forwarded = wheelSpy.mock.calls[0][0] as WheelEvent;
    expect(forwarded.deltaY).toBe(120);
    expect(viewport!.scrollTop).toBe(before + 120);
  });
});
