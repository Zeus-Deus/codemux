/// <reference types="@testing-library/jest-dom/vitest" />
import type * as React from "react";
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { LegendListRef } from "@legendapp/list/react";

import { MessageTrail } from "./MessageTrail";
import { buildTranscriptSlots } from "./transcript-slots";

const scrollToIndex = vi.fn(() => Promise.resolve());
let viewport: HTMLDivElement;
let listRef: React.RefObject<LegendListRef | null>;
let scrollTop = 0;

afterEach(() => {
  cleanup();
  scrollToIndex.mockClear();
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
  viewport = document.createElement("div");
  Object.defineProperties(viewport, {
    clientHeight: { configurable: true, value: 120 },
    scrollTop: {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    },
  });
  scrollTop = 0;
  listRef = {
    current: {
      getScrollableNode: () => viewport,
      getState: () => ({
        positionAtIndex: (index: number) => index * 100,
        sizeAtIndex: () => 80,
      }),
      scrollToIndex,
    } as unknown as LegendListRef,
  };
  return render(<MessageTrail slots={slots} listRef={listRef} />);
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
    expect(scrollToIndex).toHaveBeenCalledWith({
      index: 2,
      animated: false,
      viewOffset: 10,
    });
  });

  it("shows a hover preview card with the prompt and reply snippet", () => {
    renderTrail(turns(3));
    const button = screen.getByRole("button", { name: /Jump to turn 2:/ });
    fireEvent.mouseEnter(button);
    expect(screen.getByText("Question number 2 about the transcript")).toBeInTheDocument();
    expect(screen.getByText("Reply to question 2")).toBeInTheDocument();
  });

  it("immediately expands the hovered tick and its nearest neighbors", () => {
    renderTrail(turns(5));
    const buttons = screen.getAllByRole("button");

    fireEvent.mouseEnter(buttons[2]);

    expect(buttons[2].querySelector("span")).toHaveClass("w-6");
    expect(buttons[1].querySelector("span")).toHaveClass("w-4");
    expect(buttons[3].querySelector("span")).toHaveClass("w-4");
    expect(buttons[0].querySelector("span")).toHaveClass("w-2.5");
    expect(buttons[4].querySelector("span")).toHaveClass("w-2.5");
  });

  it("forwards rail wheel as a native wheel event on the viewport plus a scrollTop write", () => {
    renderTrail(turns(3));

    // A raw scrollTop write fires only `scroll` — the scroller engine unpins
    // its stick-to-bottom state only on a real `wheel` event reaching the
    // viewport, so the forwarder must dispatch one before scrolling.
    const wheelSpy = vi.fn();
    viewport.addEventListener("wheel", wheelSpy);
    const before = viewport.scrollTop;

    fireEvent.wheel(
      screen.getByRole("navigation", { name: "Conversation turns" }),
      { deltaY: 120 },
    );

    expect(wheelSpy).toHaveBeenCalledTimes(1);
    const forwarded = wheelSpy.mock.calls[0][0] as WheelEvent;
    expect(forwarded.deltaY).toBe(120);
    expect(viewport.scrollTop).toBe(before + 120);
  });

  it("updates tick visibility from the virtualizer without a React render", async () => {
    renderTrail(turns(4));
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

    const buttons = screen.getAllByRole("button");
    expect(buttons[0]).toHaveAttribute("aria-current", "true");
    expect(buttons[0].querySelector("span")).toHaveAttribute(
      "data-in-view",
      "true",
    );

    scrollTop = 405;
    fireEvent.scroll(viewport);
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

    expect(buttons[0]).not.toHaveAttribute("aria-current");
    expect(buttons[2]).toHaveAttribute("aria-current", "true");
    expect(buttons[2].querySelector("span")).toHaveAttribute(
      "data-in-view",
      "true",
    );
  });
});
