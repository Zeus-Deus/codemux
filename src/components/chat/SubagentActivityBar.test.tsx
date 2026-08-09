/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import type { ChatViewItem, SubagentRunItem, SubagentView } from "@/lib/agent-chat/types";

import { SubagentActivityBar } from "./SubagentActivityBar";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function subagent(overrides: Partial<SubagentView>): SubagentView {
  return {
    id: "s1",
    name: "Subagent",
    status: "running",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

function card(id: string, subagents: SubagentView[]): SubagentRunItem {
  return {
    kind: "subagent_run",
    id,
    seq: 0,
    turn_id: "t1",
    subagents,
  };
}

describe("SubagentActivityBar", () => {
  it("is a 32px strip for the composer's top edge, with a 1px accent sweep and no filled bar", () => {
    const messages: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", status: "running" })]),
    ];
    const { container } = render(
      <SubagentActivityBar
        messages={messages}
        threadId="t1"
        streaming
        onJump={() => {}}
      />,
    );
    const strip = screen.getByTestId("subagent-activity-bar");
    // Welded inside the composer card: strip height, top corners only,
    // hairline bottom border, faint fg-mix background (well under the 8%
    // "tints above this are a bug" ceiling).
    expect(strip.className).toContain("h-8");
    expect(strip.className).toContain("rounded-t-[19px]");
    expect(strip.className).toContain("border-b");
    expect(strip.className).toContain("bg-foreground/[0.03]");
    // The moving mark is a 1px accent light, not a saturated progress bar.
    const sweep = container.querySelector(".cm-sweep");
    expect(sweep).not.toBeNull();
    expect(sweep?.className).toContain("h-px");
    expect(sweep?.className).toContain("via-accent-ember");
  });

  it("renders nothing when no subagent is running", () => {
    const messages: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", status: "completed" })]),
    ];
    const { container } = render(
      <SubagentActivityBar messages={messages} threadId="t1" streaming onJump={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("stays hidden while the matching transcript work-log row is visible", async () => {
    class VisibleIntersectionObserver {
      constructor(
        private readonly callback: IntersectionObserverCallback,
      ) {}
      observe(target: Element) {
        this.callback(
          [{ isIntersecting: true, target } as IntersectionObserverEntry],
          this as unknown as IntersectionObserver,
        );
      }
      disconnect() {}
      unobserve() {}
      takeRecords() {
        return [];
      }
      readonly root = null;
      readonly rootMargin = "0px";
      readonly thresholds = [0];
    }
    vi.stubGlobal("IntersectionObserver", VisibleIntersectionObserver);
    const messages: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", status: "running" })]),
    ];
    render(
      <>
        <div data-subagent-card="run-1">
          <span data-subagent-run-id="run-1" />
        </div>
        <SubagentActivityBar
          messages={messages}
          threadId="t1"
          streaming
          onJump={() => {}}
        />
      </>,
    );
    await waitFor(() => {
      expect(screen.queryByTestId("subagent-activity-bar")).toBeNull();
    });
  });

  it("single running: shows the count + View, and clicking the bar jumps to its card", () => {
    const onJump = vi.fn();
    const messages: ChatViewItem[] = [
      card("run-1", [
        subagent({
          id: "a",
          name: "Verify",
          status: "running",
          activity: "reading diff for timing regressions…",
        }),
      ]),
    ];
    render(<SubagentActivityBar messages={messages} threadId="t1" streaming onJump={onJump} />);

    expect(screen.getByText("1 subagent running")).toBeInTheDocument();
    expect(screen.getByText("View")).toBeInTheDocument();
    // The mono label names what KIND of busy the run is, from the shared
    // orb-state vocabulary — not a name + free-text activity line.
    expect(screen.getByText("working")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("subagent-activity-bar"));
    expect(onJump).toHaveBeenCalledWith("run-1");
    // No expand list for a single running subagent.
    expect(screen.queryByTestId("subagent-activity-bar-list")).toBeNull();
  });

  it("multi running: shows the count + Show all, toggles the expand list, and a row jump collapses + fires with its own card id", () => {
    const onJump = vi.fn();
    const messages: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", name: "Explore", status: "running" })]),
      card("run-2", [subagent({ id: "b", name: "Implement", status: "running" })]),
    ];
    render(<SubagentActivityBar messages={messages} threadId="t1" streaming onJump={onJump} />);

    expect(screen.getByText("2 subagents running")).toBeInTheDocument();
    expect(screen.getByText("Show all")).toBeInTheDocument();
    expect(screen.queryByTestId("subagent-activity-bar-list")).toBeNull();
    expect(screen.getByTestId("subagent-activity-bar")).toHaveAttribute(
      "aria-expanded",
      "false",
    );

    fireEvent.click(screen.getByTestId("subagent-activity-bar"));
    expect(screen.getByTestId("subagent-activity-bar-list")).toBeInTheDocument();
    expect(screen.getByText("Hide")).toBeInTheDocument();
    expect(screen.getByTestId("subagent-activity-bar")).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    // Two-card thread: rows carry a from-label to disambiguate.
    expect(screen.getByText("from task 1")).toBeInTheDocument();
    expect(screen.getByText("from task 2")).toBeInTheDocument();

    const rows = screen.getAllByTestId("subagent-activity-bar-row");
    expect(rows).toHaveLength(2);
    fireEvent.click(rows[1]);

    expect(onJump).toHaveBeenCalledWith("run-2");
    // Collapses back after a row jump.
    expect(screen.queryByTestId("subagent-activity-bar-list")).toBeNull();
  });

  it("does not auto-reopen the list when the count dips to 1 and climbs back (stale open reset)", () => {
    const two: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", name: "Explore", status: "running" })]),
      card("run-2", [subagent({ id: "b", name: "Implement", status: "running" })]),
    ];
    const one: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", name: "Explore", status: "completed" })]),
      card("run-2", [subagent({ id: "b", name: "Implement", status: "running" })]),
    ];
    const twoAgain: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", name: "Explore", status: "completed" })]),
      card("run-2", [subagent({ id: "b", name: "Implement", status: "running" })]),
      card("run-3", [subagent({ id: "c", name: "Verify", status: "running" })]),
    ];
    const { rerender } = render(
      <SubagentActivityBar messages={two} threadId="t1" streaming onJump={() => {}} />,
    );

    // Open the expand list while two are running.
    fireEvent.click(screen.getByTestId("subagent-activity-bar"));
    expect(screen.getByTestId("subagent-activity-bar-list")).toBeInTheDocument();

    // One finishes: single-running state, list hidden by design.
    rerender(
      <SubagentActivityBar messages={one} threadId="t1" streaming onJump={() => {}} />,
    );
    expect(screen.queryByTestId("subagent-activity-bar-list")).toBeNull();

    // A new subagent starts (back to 2): the list must NOT pop open with
    // no click — the stale `open` was reset on the multi → single drop.
    rerender(
      <SubagentActivityBar messages={twoAgain} threadId="t1" streaming onJump={() => {}} />,
    );
    expect(screen.queryByTestId("subagent-activity-bar-list")).toBeNull();
    expect(screen.getByText("Show all")).toBeInTheDocument();
  });

  it("flashes green then disappears when the running count transitions to zero, and the flash is a clickable jump", () => {
    vi.useFakeTimers();
    const onJump = vi.fn();
    const running: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", status: "running" })]),
    ];
    const finished: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", status: "completed" })]),
    ];
    const { rerender, queryByTestId, getByTestId } = render(
      <SubagentActivityBar messages={running} threadId="t1" streaming onJump={onJump} />,
    );
    expect(getByTestId("subagent-activity-bar")).toHaveAttribute(
      "data-tone",
      "running",
    );

    rerender(
      <SubagentActivityBar messages={finished} threadId="t1" streaming onJump={onJump} />,
    );
    expect(getByTestId("subagent-activity-bar")).toHaveAttribute(
      "data-tone",
      "finished",
    );
    expect(screen.getByText("Subagents finished")).toBeInTheDocument();
    expect(
      screen.getByText("all tasks complete · results are in the thread"),
    ).toBeInTheDocument();

    // Design gallery gives the finished state a "Jump" CTA: clicking the
    // green flash jumps to the card of the last subagent that finished.
    fireEvent.click(getByTestId("subagent-activity-bar"));
    expect(onJump).toHaveBeenCalledWith("run-1");

    act(() => {
      vi.advanceTimersByTime(2500);
    });
    expect(queryByTestId("subagent-activity-bar")).toBeNull();
  });

  it("does not flash on initial mount / thread hydrate even when the thread has no running subagents", () => {
    const messages: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", status: "completed" })]),
    ];
    render(<SubagentActivityBar messages={messages} threadId="t1" streaming onJump={() => {}} />);
    expect(screen.queryByTestId("subagent-activity-bar")).toBeNull();
    expect(screen.queryByText("Subagents finished")).toBeNull();
  });

  it("ignores a background task once the run is over", () => {
    const messages: ChatViewItem[] = [
      card("run-1", [
        subagent({ id: "bg", status: "running", backgroundTask: true }),
      ]),
    ];
    // Mid-run a background job still reads as activity.
    const { rerender, queryByTestId } = render(
      <SubagentActivityBar messages={messages} threadId="t1" streaming onJump={() => {}} />,
    );
    expect(queryByTestId("subagent-activity-bar")).not.toBeNull();
    expect(screen.getByText("1 subagent running")).toBeInTheDocument();

    // Turn over: a background shell command that never reports a terminal
    // status must not keep the live count (and its spinner) up. The count
    // going to zero is a normal finish, so the bar plays its green flash
    // and then unmounts — it never sits there claiming live activity.
    rerender(
      <SubagentActivityBar
        messages={messages}
        threadId="t1"
        streaming={false}
        onJump={() => {}}
      />,
    );
    expect(screen.queryByText("1 subagent running")).toBeNull();
  });

  it("does not flash when switching to a different thread with zero running subagents", () => {
    const runningThreadA: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", status: "running" })]),
    ];
    const idleThreadB: ChatViewItem[] = [
      card("run-2", [subagent({ id: "b", status: "completed" })]),
    ];
    const { rerender, queryByTestId } = render(
      <SubagentActivityBar messages={runningThreadA} threadId="thread-a" streaming onJump={() => {}} />,
    );
    expect(queryByTestId("subagent-activity-bar")).not.toBeNull();

    rerender(
      <SubagentActivityBar messages={idleThreadB} threadId="thread-b" streaming onJump={() => {}} />,
    );
    // Different thread — this is a hydrate, not an observed transition.
    expect(queryByTestId("subagent-activity-bar")).toBeNull();
  });
});

// ── Agent orbs (one per live thing) ──

describe("SubagentActivityBar — agent orbs", () => {
  function tool(name: string, status: "running" | "done" | "error", input: unknown = {}) {
    return {
      kind: "tool_call" as const,
      id: `tc-${name}-${status}`,
      seq: 1,
      tool_use_id: `tu-${name}`,
      tool_name: name,
      input,
      status,
      result_content: null,
      approval_request_id: null,
    };
  }
  const orbStates = () =>
    [...document.querySelectorAll("canvas[data-orb-state]")].map((c) =>
      c.getAttribute("data-orb-state"),
    );

  it("keeps the bar's own orb neutral — it stands for the whole run", () => {
    const messages: ChatViewItem[] = [
      card("run-1", [
        subagent({ id: "a", status: "running", items: [tool("Grep", "running")] }),
      ]),
    ];
    render(<SubagentActivityBar messages={messages} threadId="t1" streaming onJump={() => {}} />);
    // Collapsed: only the bar orb is mounted, and it must not borrow the
    // single subagent's "searching".
    expect(orbStates()).toEqual(["working"]);
  });

  it("gives each expanded row its own activity-matched orb", () => {
    const messages: ChatViewItem[] = [
      card("run-1", [
        subagent({ id: "a", status: "running", items: [tool("Grep", "running")] }),
        subagent({
          id: "b",
          status: "running",
          items: [tool("Bash", "running", { command: "git push origin HEAD" })],
        }),
      ]),
    ];
    render(<SubagentActivityBar messages={messages} threadId="t1" streaming onJump={() => {}} />);
    fireEvent.click(screen.getByText("2 subagents running"));
    // Bar orb stays neutral; the two rows describe themselves.
    expect(orbStates()).toEqual(["searching", "connecting", "working"]);
  });

  it("shows a flat check and no orb once the run finishes", () => {
    vi.useFakeTimers();
    const running: ChatViewItem[] = [card("run-1", [subagent({ id: "a", status: "running" })])];
    const { rerender } = render(
      <SubagentActivityBar messages={running} threadId="t1" streaming onJump={() => {}} />,
    );
    expect(orbStates()).toHaveLength(1);

    const done: ChatViewItem[] = [card("run-1", [subagent({ id: "a", status: "completed" })])];
    act(() => {
      rerender(
        <SubagentActivityBar messages={done} threadId="t1" streaming onJump={() => {}} />,
      );
    });
    expect(screen.getByText("Subagents finished")).toBeInTheDocument();
    expect(orbStates()).toHaveLength(0);
  });
});
