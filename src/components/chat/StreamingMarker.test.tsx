/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type { ChatViewItem } from "@/lib/agent-chat/types";

import { StreamingMarker, deriveTurnStartedAt } from "./StreamingMarker";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function userMsg(seq: number, overrides: Partial<ChatViewItem> = {}): ChatViewItem {
  return { kind: "user_message", id: `u${seq}`, seq, text: "go", ...overrides } as ChatViewItem;
}

function toolCall(seq: number, startedAt?: number): ChatViewItem {
  return {
    kind: "tool_call",
    id: `tc${seq}`,
    seq,
    tool_use_id: `tu${seq}`,
    tool_name: "Read",
    input: {},
    status: "running",
    result_content: null,
    approval_request_id: null,
    started_at: startedAt,
  };
}

describe("deriveTurnStartedAt", () => {
  it("takes the earliest step start after the last non-queued user message", () => {
    expect(
      deriveTurnStartedAt([userMsg(0), toolCall(1, 5_000), toolCall(2, 3_000)]),
    ).toBe(3_000);
  });

  it("ignores queued follow-up prompts when locating the active turn", () => {
    const msgs = [
      userMsg(0),
      toolCall(1, 7_000),
      userMsg(1_000_000_050, { queued: { queuedId: "q1" } }),
    ];
    expect(deriveTurnStartedAt(msgs)).toBe(7_000);
  });

  it("returns null when no step after the prompt carries a start", () => {
    expect(deriveTurnStartedAt([userMsg(0)])).toBeNull();
    expect(deriveTurnStartedAt([userMsg(0), toolCall(1, undefined)])).toBeNull();
  });
});

describe("StreamingMarker elapsed time", () => {
  it("renders a live elapsed suffix that ticks each second without re-rendering", () => {
    vi.useFakeTimers();
    vi.setSystemTime(10_000);
    render(<StreamingMarker messages={[userMsg(0), toolCall(1, 4_000)]} />);
    // 10_000 − 4_000 = 6_000ms → "· 6s".
    expect(screen.getByText("· 6s")).toBeInTheDocument();
    vi.advanceTimersByTime(2_000);
    // The interval writes textContent directly (no React re-render).
    expect(screen.getByText("· 8s")).toBeInTheDocument();
  });

  it("renders no elapsed suffix when no turn start is derivable", () => {
    render(<StreamingMarker messages={[userMsg(0)]} />);
    expect(screen.getByText("Working…")).toBeInTheDocument();
    expect(screen.queryByText(/·\s*\d+s/)).toBeNull();
  });
});
