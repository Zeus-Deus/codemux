/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ReasoningItem } from "@/lib/agent-chat/types";

import { ReasoningBlock } from "./ReasoningBlock";

afterEach(() => cleanup());

function reasoning(overrides: Partial<ReasoningItem> = {}): ReasoningItem {
  return {
    kind: "reasoning",
    id: "r-1",
    seq: 0,
    turn_id: "t1",
    text: "Line numbers rarely survive across versions.",
    streaming: false,
    ...overrides,
  };
}

describe("ReasoningBlock", () => {
  it("shows a shimmering 'Thinking…' header and streams the body while streaming", () => {
    const { container } = render(
      <ReasoningBlock item={reasoning({ streaming: true })} />,
    );
    expect(screen.getByText("Thinking…")).toBeInTheDocument();
    expect(container.querySelector("[data-orb-state]")).toBeInTheDocument();
    // Auto-open while streaming → the body text is visible.
    expect(
      screen.getByText(/Line numbers rarely survive/),
    ).toBeInTheDocument();
  });

  it("shows 'Thought for Ns' once sealed and collapses the body by default", () => {
    const { container } = render(
      <ReasoningBlock item={reasoning({ duration_ms: 6000 })} />,
    );
    expect(screen.getByText("Thought for 6s")).toBeInTheDocument();
    // Settled thoughts keep their recognizable lightbulb, not a live orb.
    expect(container.querySelector("[data-orb-state]")).toBeNull();
    // Collapsed by default once sealed.
    expect(screen.queryByText(/Line numbers rarely survive/)).toBeNull();
    // Toggling reveals the body.
    fireEvent.click(screen.getByText("Thought for 6s"));
    expect(
      screen.getByText(/Line numbers rarely survive/),
    ).toBeInTheDocument();
  });

  it("falls back to 'Thought' when no duration was captured", () => {
    render(<ReasoningBlock item={reasoning({ duration_ms: undefined })} />);
    expect(screen.getByText("Thought")).toBeInTheDocument();
  });
});
