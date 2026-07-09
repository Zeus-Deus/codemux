/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ChatViewItem, SubagentRunItem, SubagentView } from "@/lib/agent-chat/types";

import { SubagentActivityBar } from "./SubagentActivityBar";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
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
  it("renders nothing when no subagent is running", () => {
    const messages: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", status: "completed" })]),
    ];
    const { container } = render(
      <SubagentActivityBar messages={messages} threadId="t1" onJump={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
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
    render(<SubagentActivityBar messages={messages} threadId="t1" onJump={onJump} />);

    expect(screen.getByText("1 subagent running")).toBeInTheDocument();
    expect(screen.getByText("View")).toBeInTheDocument();
    expect(
      screen.getByText("Verify · reading diff for timing regressions…"),
    ).toBeInTheDocument();

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
    render(<SubagentActivityBar messages={messages} threadId="t1" onJump={onJump} />);

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
      <SubagentActivityBar messages={two} threadId="t1" onJump={() => {}} />,
    );

    // Open the expand list while two are running.
    fireEvent.click(screen.getByTestId("subagent-activity-bar"));
    expect(screen.getByTestId("subagent-activity-bar-list")).toBeInTheDocument();

    // One finishes: single-running state, list hidden by design.
    rerender(
      <SubagentActivityBar messages={one} threadId="t1" onJump={() => {}} />,
    );
    expect(screen.queryByTestId("subagent-activity-bar-list")).toBeNull();

    // A new subagent starts (back to 2): the list must NOT pop open with
    // no click — the stale `open` was reset on the multi → single drop.
    rerender(
      <SubagentActivityBar messages={twoAgain} threadId="t1" onJump={() => {}} />,
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
      <SubagentActivityBar messages={running} threadId="t1" onJump={onJump} />,
    );
    expect(getByTestId("subagent-activity-bar")).toHaveAttribute(
      "data-tone",
      "running",
    );

    rerender(
      <SubagentActivityBar messages={finished} threadId="t1" onJump={onJump} />,
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
    render(<SubagentActivityBar messages={messages} threadId="t1" onJump={() => {}} />);
    expect(screen.queryByTestId("subagent-activity-bar")).toBeNull();
    expect(screen.queryByText("Subagents finished")).toBeNull();
  });

  it("does not flash when switching to a different thread with zero running subagents", () => {
    const runningThreadA: ChatViewItem[] = [
      card("run-1", [subagent({ id: "a", status: "running" })]),
    ];
    const idleThreadB: ChatViewItem[] = [
      card("run-2", [subagent({ id: "b", status: "completed" })]),
    ];
    const { rerender, queryByTestId } = render(
      <SubagentActivityBar messages={runningThreadA} threadId="thread-a" onJump={() => {}} />,
    );
    expect(queryByTestId("subagent-activity-bar")).not.toBeNull();

    rerender(
      <SubagentActivityBar messages={idleThreadB} threadId="thread-b" onJump={() => {}} />,
    );
    // Different thread — this is a hydrate, not an observed transition.
    expect(queryByTestId("subagent-activity-bar")).toBeNull();
  });
});
