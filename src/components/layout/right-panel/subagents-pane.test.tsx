/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";

import type { ChatViewItem, SubagentView } from "@/lib/agent-chat/types";
import { useUIStore } from "@/stores/ui-store";

import { COMPLETION_LINGER_MS, SubagentsPane } from "./subagents-pane";

function subagent(overrides: Partial<SubagentView>): SubagentView {
  return {
    id: "sub-1",
    name: "Explore",
    status: "running",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

function prompt(id: string, seq: number, text: string): ChatViewItem {
  return { kind: "user_message", id, seq, text };
}

function run(id: string, seq: number, subagents: SubagentView[]): ChatViewItem {
  return { kind: "subagent_run", id, seq, turn_id: `turn-${id}`, subagents };
}

/** One wave: the shape every single-wave test uses. */
function messages(subagents: SubagentView[]): ChatViewItem[] {
  return [
    prompt("u-1", 0, "Implement clipboard-paste fallback"),
    run("run-1", 1, subagents),
  ];
}

/** Two waves — the design fixture: an older implement wave, then a newer
 *  analysis wave with one failure. */
function twoWaves(): ChatViewItem[] {
  return [
    prompt("u-1", 0, "Implement + verify\nsecond line is not the title"),
    run("run-1", 1, [
      subagent({
        id: "impl",
        name: undefined,
        model: "haiku",
        status: "completed",
        resultText: "Patched session.ts, all 14 tests green",
        durationMs: 371_000,
        totalTokens: 400_000,
      }),
      subagent({
        id: "verify",
        name: undefined,
        model: "haiku",
        status: "stopped",
        resultText: "Stopped — superseded by wave 2 findings",
        durationMs: 67_000,
        totalTokens: 240_000,
      }),
    ]),
    prompt("u-2", 2, "Issue analysis"),
    run("run-2", 3, [
      subagent({
        id: "explore-1",
        model: "opus",
        status: "completed",
        resultText:
          "Root cause: applyRuntimeInfo never re-registers the tile delegate\nmore detail",
        durationMs: 207_000,
      }),
      subagent({
        id: "explore-2",
        model: "opus",
        status: "failed",
        resultText: "Failed — sandbox denied network access during fetch",
        durationMs: 185_000,
      }),
    ]),
  ];
}

beforeEach(() =>
  useUIStore.setState({
    subagentEnterRequest: null,
    subagentsHistoryOpen: false,
    dismissedSubagentAttention: [],
  }),
);
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("SubagentsPane — live list", () => {
  it("shows only running agents, newest first, under the wave that spawned them", () => {
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([
          subagent({
            id: "done",
            name: "Verification pass",
            model: "openai/gpt-5.4",
            status: "completed",
            resultText: "All checks pass\nDetails follow",
            durationMs: 16_000,
          }),
          subagent({
            id: "live-a",
            description: "Audit pricing",
            model: "anthropic/claude-opus-4-8",
            activity: "reading pricing.ts…",
          }),
          subagent({
            id: "live-b",
            description: "Trace the regression",
            model: "anthropic/claude-opus-4-8",
            activity: "grep applyRuntimeInfo",
          }),
        ])}
      />,
    );

    expect(screen.getByText("WORKING · 2")).toBeInTheDocument();
    expect(screen.getByTestId("live-wave-title")).toHaveTextContent(
      "Verification pass · Audit pricing · Trace the regression",
    );
    // Newest spawn first, and the settled row is not in the live list.
    expect(
      screen
        .getAllByTestId("live-row")
        .map((row) => row.getAttribute("data-subagent-id")),
    ).toEqual(["live-b", "live-a"]);
    expect(screen.queryByText("Verification pass")).toBeNull();
    expect(screen.queryByText("All checks pass")).toBeNull();

    // Each live row: title, activity line and the model capsule.
    expect(screen.getByText("Audit pricing")).toBeInTheDocument();
    expect(screen.getByText("reading pricing.ts…")).toBeInTheDocument();
    expect(screen.getByText("grep applyRuntimeInfo")).toBeInTheDocument();
    expect(
      screen.getAllByTitle("Model: anthropic/claude-opus-4-8"),
    ).toHaveLength(2);

    // The lifetime counter and its progress bar are gone: this pane is
    // about now, not about the thread's whole history.
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.queryByText("1 / 3")).toBeNull();
  });

  it("numbers repeated titles inside a wave", () => {
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([subagent({ id: "a" }), subagent({ id: "b" })])}
      />,
    );
    expect(
      screen.getByRole("button", { name: "Open Explore 1 thread" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Open Explore 2 thread" }),
    ).toBeInTheDocument();
  });

  it("omits the model capsule when the provider has not reported one", () => {
    const { container } = render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([subagent({ model: undefined })])}
      />,
    );
    expect(container.querySelector("[data-subagent-model]")).toBeNull();
  });

  it("opens a subagent thread from a live row", () => {
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([subagent({ id: "sub-42" })])}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Open Explore thread" }),
    );
    expect(useUIStore.getState().subagentEnterRequest).toMatchObject({
      threadId: "thread-1",
      subagentId: "sub-42",
    });
  });
});

describe("SubagentsPane — completion linger", () => {
  it("holds a just-finished row for 8s, pauses on hover, then collapses it out", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(100_000));
    const { rerender } = render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([
          subagent({
            id: "live",
            description: "Audit pricing",
            startedAt: 90_000,
          }),
        ])}
      />,
    );
    expect(screen.getByTestId("live-row")).not.toHaveAttribute("data-linger");

    act(() => {
      rerender(
        <SubagentsPane
          threadId="thread-1"
          messages={messages([
            subagent({
              id: "live",
              description: "Audit pricing",
              status: "completed",
              startedAt: 90_000,
              finishedAt: 100_000,
              resultText: "Pricing table matches the plan\nmore detail",
            }),
          ])}
        />,
      );
    });

    // Still in place — with a settled glyph and its report's first line.
    const row = screen.getByTestId("live-row");
    expect(row).toHaveAttribute("data-linger", "hold");
    expect(
      within(row).getByText("Pricing table matches the plan"),
    ).toBeInTheDocument();
    expect(row.querySelector('[data-row-glyph="completed"]')).not.toBeNull();

    act(() => {
      vi.advanceTimersByTime(COMPLETION_LINGER_MS - 1);
    });
    expect(screen.getByTestId("live-row")).toHaveAttribute(
      "data-linger",
      "hold",
    );

    // Hovering the row is exactly when it must not disappear.
    fireEvent.mouseEnter(screen.getByTestId("live-row"));
    act(() => {
      vi.advanceTimersByTime(10_000);
    });
    expect(screen.getByTestId("live-row")).toHaveAttribute(
      "data-linger",
      "hold",
    );

    // Leaving restarts a fresh hold rather than resuming the old one.
    fireEvent.mouseLeave(screen.getByTestId("live-row"));
    act(() => {
      vi.advanceTimersByTime(COMPLETION_LINGER_MS - 1);
    });
    expect(screen.getByTestId("live-row")).toHaveAttribute(
      "data-linger",
      "hold",
    );
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(screen.getByTestId("live-row")).toHaveAttribute(
      "data-linger",
      "collapsing",
    );
    act(() => {
      vi.advanceTimersByTime(400);
    });
    expect(screen.queryByTestId("live-row")).toBeNull();
  });

  it("does not linger rows that were already settled when the pane mounted", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(200_000));
    render(<SubagentsPane threadId="thread-1" messages={twoWaves()} />);
    expect(screen.queryByTestId("live-row")).toBeNull();
    expect(
      screen
        .getAllByTestId("receipt-row")
        .some((row) => row.hasAttribute("data-linger")),
    ).toBe(false);
  });
});

describe("SubagentsPane — needs attention", () => {
  it("holds a failure above the live list until it is dismissed", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(300_000));
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([
          subagent({
            id: "boom",
            description: "Fetch the changelog",
            model: "opus",
            status: "failed",
            resultText: "Failed — sandbox denied network access during fetch",
            durationMs: 185_000,
          }),
          subagent({ id: "live", description: "Audit pricing" }),
        ])}
      />,
    );

    const card = screen.getByTestId("attention-card");
    expect(screen.getByText("NEEDS ATTENTION · 1")).toBeInTheDocument();
    expect(card).toHaveClass("border-status-attention/25");
    expect(card.querySelector('[data-row-glyph="failed"]')).not.toBeNull();
    expect(
      within(card).getByText(
        "Failed — sandbox denied network access during fetch",
      ),
    ).toBeInTheDocument();
    expect(within(card).getByTitle("Model: opus")).toBeInTheDocument();
    expect(within(card).getByText("3m 05s")).toBeInTheDocument();

    // A failure never times itself out.
    act(() => {
      vi.advanceTimersByTime(30_000);
    });
    expect(screen.getByTestId("attention-card")).toBeInTheDocument();

    fireEvent.click(within(card).getByRole("button", { name: "Open thread" }));
    expect(useUIStore.getState().subagentEnterRequest).toMatchObject({
      threadId: "thread-1",
      subagentId: "boom",
    });

    fireEvent.click(within(card).getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByTestId("attention-card")).toBeNull();
    expect(useUIStore.getState().dismissedSubagentAttention).toEqual(["boom"]);
  });
});

describe("SubagentsPane — idle receipt", () => {
  it("reads back the last wave instead of going blank", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(1_000_000));
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([
          subagent({
            id: "impl",
            description: "Patch the session store",
            status: "completed",
            startedAt: 600_000,
            finishedAt: 760_000,
            resultText: "Patched session.ts, all 14 tests green\nmore detail",
          }),
        ])}
      />,
    );

    expect(screen.queryByText(/^WORKING/)).toBeNull();
    expect(screen.getByTestId("receipt-title")).toHaveTextContent(
      "Patch the session store",
    );
    expect(
      screen.getByText("Patched session.ts, all 14 tests green"),
    ).toBeInTheDocument();
    // 1_000_000 - 760_000 = 240s → 4m.
    expect(screen.getByText("settled 4m ago")).toBeInTheDocument();
  });

  it("keeps the empty state when the thread has never spawned an agent", () => {
    render(<SubagentsPane threadId="thread-1" messages={[]} />);
    expect(
      screen.getByText("No subagents in this thread yet."),
    ).toBeInTheDocument();
  });
});

describe("SubagentsPane — elapsed", () => {
  it("freezes a settled row's clock and keeps a running one ticking", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(100_000));
    const { unmount } = render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([
          subagent({
            id: "done",
            description: "Patch the session store",
            status: "completed",
            startedAt: 40_000,
            finishedAt: 70_000,
          }),
        ])}
      />,
    );
    const frozen = within(screen.getByTestId("receipt-row")).getByText(
      "0m 30s",
    );
    act(() => {
      vi.advanceTimersByTime(5_000);
    });
    expect(frozen).toHaveTextContent("0m 30s");
    unmount();

    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([
          subagent({
            id: "live",
            description: "Audit pricing",
            startedAt: 95_000,
          }),
        ])}
      />,
    );
    const live = within(screen.getByTestId("live-row")).getByText("0m 10s");
    act(() => {
      vi.advanceTimersByTime(3_000);
    });
    expect(live).toHaveTextContent("0m 13s");
  });
});

describe("SubagentsPane — history", () => {
  function openHistory(messages: ChatViewItem[]) {
    useUIStore.setState({ subagentsHistoryOpen: true });
    return render(<SubagentsPane threadId="thread-1" messages={messages} />);
  }

  it("lists every turn's waves newest first behind a breadcrumb", () => {
    openHistory(twoWaves());

    expect(
      screen.getByRole("button", { name: "Back to Subagents" }),
    ).toBeInTheDocument();
    expect(screen.getByText("History")).toBeInTheDocument();
    expect(
      screen.getAllByTestId("wave-prompt").map((n) => n.textContent),
    ).toEqual(["›Issue analysis", "›Implement + verify"]);
    expect(screen.getAllByTestId("history-row")).toHaveLength(4);
  });

  it("filters the record down to one outcome at a time", () => {
    openHistory(twoWaves());
    const filters = screen.getByRole("group", { name: "History filter" });
    const chip = (name: string) =>
      within(filters).getByRole("button", { name });
    expect(chip("All")).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(chip("Failed"));
    expect(chip("Failed")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getAllByTestId("history-row")).toHaveLength(1);
    expect(
      screen.getByText("Failed — sandbox denied network access during fetch"),
    ).toBeInTheDocument();
    // The turn with nothing matching drops its divider too.
    expect(
      screen.getAllByTestId("wave-prompt").map((n) => n.textContent),
    ).toEqual(["›Issue analysis"]);

    fireEvent.click(chip("Stopped"));
    expect(screen.getAllByTestId("history-row")).toHaveLength(1);
    expect(
      screen.getByText("Stopped — superseded by wave 2 findings"),
    ).toBeInTheDocument();

    fireEvent.click(chip("Done"));
    expect(screen.getAllByTestId("history-row")).toHaveLength(2);
    expect(
      screen.getByText("Patched session.ts, all 14 tests green"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Root cause: applyRuntimeInfo never re-registers the tile delegate",
      ),
    ).toBeInTheDocument();
  });

  it("opens a thread from a history row and returns to the live list", () => {
    openHistory(twoWaves());
    fireEvent.click(
      screen.getByRole("button", { name: "Open Explore 2 thread" }),
    );
    expect(useUIStore.getState().subagentEnterRequest).toMatchObject({
      threadId: "thread-1",
      subagentId: "explore-2",
    });

    fireEvent.click(screen.getByRole("button", { name: "Back to Subagents" }));
    expect(useUIStore.getState().subagentsHistoryOpen).toBe(false);
  });
});
