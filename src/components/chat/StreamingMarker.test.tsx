/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ChatViewItem } from "@/lib/agent-chat/types";
import { useUIStore } from "@/stores/ui-store";

import {
  StreamingMarker,
  deriveStreamingStatus,
  deriveTurnStartedAt,
} from "./StreamingMarker";

const deriveStreamingLabel = (messages: ChatViewItem[]) =>
  deriveStreamingStatus(messages).label;

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

describe("deriveStreamingLabel — waiting on background work", () => {
  const subagentRun = (
    subagents: Array<{ id: string; status: "running" | "completed"; backgroundTask?: boolean; taskKind?: "monitor" }>,
  ): ChatViewItem => ({
    kind: "subagent_run",
    id: "sr-1",
    seq: 1,
    turn_id: "t1",
    subagents: subagents.map((s) => ({ ...s, items: [] })),
  }) as unknown as ChatViewItem;
  const sealedText = (seq: number): ChatViewItem => ({
    kind: "assistant_message",
    id: `a${seq}`,
    seq,
    turn_id: "t1",
    text: "Waiting on the report…",
    streaming: false,
  });

  it("names the wait when the tail is settled and tasks are still running", () => {
    expect(
      deriveStreamingLabel([
        userMsg(0),
        subagentRun([
          { id: "catalog", status: "running" },
          { id: "npm-ci", status: "running", backgroundTask: true },
        ]),
        sealedText(2),
      ]),
    ).toBe("Waiting on 2 background tasks…");
    expect(
      deriveStreamingLabel([
        userMsg(0),
        subagentRun([{ id: "catalog", status: "running" }]),
        sealedText(2),
      ]),
    ).toBe("Waiting on a background task…");
  });

  it("ignores watch loops and finished tasks, and defers to a live tail", () => {
    expect(
      deriveStreamingLabel([
        userMsg(0),
        subagentRun([
          { id: "ci", status: "running", taskKind: "monitor" },
          { id: "done", status: "completed" },
        ]),
        sealedText(2),
      ]),
    ).toBe("Working…");
    expect(
      deriveStreamingLabel([
        userMsg(0),
        subagentRun([{ id: "catalog", status: "running" }]),
        toolCall(2),
      ]),
    ).toBe("Running Read…");
  });
});


it("announces summarization ahead of transcript-derived activity", () => {
  const { rerender } = render(<StreamingMarker messages={[toolCall(1)]} compacting />);
  expect(screen.getByRole("status", { name: "Agent is summarizing context" })).toHaveTextContent("Summarizing context…");
  expect(screen.queryByText("Running Read…")).toBeNull();
  rerender(<StreamingMarker messages={[toolCall(1)]} />);
  expect(screen.getByText("Running Read…")).toBeInTheDocument();
});

describe("StreamingMarker — waiting label opens the Subagents panel", () => {
  const waitingMessages = (): ChatViewItem[] => [
    userMsg(0),
    {
      kind: "subagent_run",
      id: "sr-1",
      seq: 1,
      turn_id: "t1",
      subagents: [
        { id: "a", status: "running", items: [] },
        { id: "b", status: "running", backgroundTask: true, items: [] },
      ],
    } as unknown as ChatViewItem,
    {
      kind: "assistant_message",
      id: "a2",
      seq: 2,
      turn_id: "t1",
      text: "Waiting…",
      streaming: false,
    },
  ];

  const initialState = useUIStore.getState();
  afterEach(() => {
    useUIStore.setState(initialState, true);
  });

  it("opens the Subagents tab for the workspace on click", () => {
    render(<StreamingMarker messages={waitingMessages()} workspaceId="ws-1" />);
    const button = screen.getByRole("button", { name: "View 2 background tasks" });
    expect(button).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(button);
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("subagents");
    expect(button).toHaveAttribute("aria-pressed", "true");
  });

  it("stays plain text without a workspace or while the tail is live", () => {
    render(<StreamingMarker messages={waitingMessages()} />);
    expect(screen.getByText("Waiting on 2 background tasks…")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
    cleanup();
    render(
      <StreamingMarker messages={[userMsg(0), toolCall(1)]} workspaceId="ws-1" />,
    );
    expect(screen.getByText("Running Read…")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
