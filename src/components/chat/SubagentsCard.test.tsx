/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type {
  SubagentRunItem,
  SubagentView,
  ToolCallItem,
} from "@/lib/agent-chat/types";

import { SubagentsCard } from "./SubagentsCard";

afterEach(() => cleanup());

function tool(id: string, tool_name: string, input: unknown): ToolCallItem {
  return {
    kind: "tool_call",
    id,
    seq: 0,
    tool_use_id: id,
    tool_name,
    input,
    status: "done",
    result_content: null,
    approval_request_id: null,
  };
}

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

function card(subagents: SubagentView[]): SubagentRunItem {
  return {
    kind: "subagent_run",
    id: "run-1",
    seq: 0,
    turn_id: "t1",
    subagents,
  };
}

describe("SubagentsCard — running group (rail)", () => {
  it("renders the header line with done/active counts", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({ id: "a", name: "Implement", status: "completed" }),
          subagent({ id: "b", name: "Verify", status: "running" }),
        ])}
        onEnter={() => {}}
      />,
    );
    expect(screen.getByText("Subagents")).toBeInTheDocument();
    expect(
      screen.getByText("2 tasks · running in parallel"),
    ).toBeInTheDocument();
    expect(screen.getByText("1 done · 1 active")).toBeInTheDocument();
  });

  it("carries the group's state on the rail, not on a card border or a row tint", () => {
    const { container } = render(
      <SubagentsCard
        item={card([subagent({ id: "b", name: "Verify", status: "running" })])}
        onEnter={() => {}}
      />,
    );
    const rail = screen.getByTestId("subagent-group-rail");
    expect(rail).toHaveAttribute("data-state", "running");
    // Accent while running, fading to near-transparent down its length.
    expect(rail.className).toContain("from-accent-ember");
    expect(rail.className).toContain("to-accent-ember/[0.12]");
    // The bordered card is gone: nothing in the block draws a box.
    expect(container.querySelector(".border-border.rounded-\\[13px\\]")).toBeNull();
  });

  it("shows a running row's live activity and a done row's result line, both with mono meta", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({
            id: "a",
            name: "Implement",
            model: "opus · xhigh",
            status: "completed",
            resultText: "Done · 6 files changed",
            durationMs: 161000,
            toolUseCount: 28,
          }),
          subagent({
            id: "b",
            name: "Verify",
            model: "opus · xhigh",
            status: "running",
            activity: "reading diff for timing regressions…",
            durationMs: 52000,
            toolUseCount: 11,
          }),
        ])}
        onEnter={() => {}}
      />,
    );
    expect(screen.getByText("Done · 6 files changed")).toBeInTheDocument();
    expect(
      screen.getByText("reading diff for timing regressions…"),
    ).toBeInTheDocument();
    expect(screen.getByText("2m 41s · 28 tools")).toBeInTheDocument();
    expect(screen.getByText("0m 52s · 11 tools")).toBeInTheDocument();
  });

  it("has no `Enter ›` buttons — the drill-in moved into the row expansion", () => {
    render(
      <SubagentsCard
        item={card([subagent({ id: "a", name: "Explore", status: "running" })])}
        onEnter={() => {}}
      />,
    );
    expect(screen.queryByText("Enter")).toBeNull();
    expect(screen.queryByText("Enter subagent")).toBeNull();
    // Collapsed by default.
    expect(screen.queryByText("Open thread")).toBeNull();
  });
});

describe("SubagentsCard — inline row expansion", () => {
  it("expands a row to its latest output, an Open thread button and the model label", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({
            id: "a",
            name: "Implement",
            model: "opus-5 · high",
            status: "running",
            activity: "Dim predicate now matches the card recede rule.",
          }),
        ])}
        onEnter={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Implement"));
    // Once in the collapsed row's ellipsized summary, once in the
    // expansion's pre-wrap output block.
    expect(
      screen.getAllByText("Dim predicate now matches the card recede rule."),
    ).toHaveLength(2);
    expect(screen.getByText("Open thread")).toBeInTheDocument();
    expect(screen.getByText("opus-5 · high")).toBeInTheDocument();
  });

  it("falls back to the latest tool call when the provider reported no output", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({
            id: "a",
            name: "Implement",
            status: "running",
            items: [
              tool("t1", "Edit", {
                file_path: "src/Composer.tsx",
                old_string: "x",
                new_string: "x\ny",
              }),
              tool("t2", "Bash", { command: "cargo test" }),
            ],
          }),
        ])}
        onEnter={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Implement"));
    expect(screen.getByText("run cargo test · ok")).toBeInTheDocument();
  });

  it("keeps only one row expanded at a time", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({ id: "a", name: "Explore", model: "opus · a" }),
          subagent({ id: "b", name: "Implement", model: "opus · b" }),
        ])}
        onEnter={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Explore"));
    expect(screen.getByText("opus · a")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Implement"));
    expect(screen.getByText("opus · b")).toBeInTheDocument();
    expect(screen.queryByText("opus · a")).toBeNull();
  });

  it("routes Open thread to the drill-in with the row's subagent id, without collapsing the row", () => {
    const onEnter = vi.fn();
    render(
      <SubagentsCard
        item={card([subagent({ id: "sub-42", name: "Explore" })])}
        onEnter={onEnter}
      />,
    );
    fireEvent.click(screen.getByText("Explore"));
    fireEvent.click(screen.getByText("Open thread"));
    expect(onEnter).toHaveBeenCalledWith("sub-42");
    // stopPropagation: the click must not also toggle the row shut.
    expect(screen.getByText("Open thread")).toBeInTheDocument();
  });
});

describe("SubagentsCard — settled collapse", () => {
  it("collapses a finished group to one line with a real-data rollup", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({
            id: "a",
            name: "Implement",
            status: "completed",
            durationMs: 74000,
            toolUseCount: 6,
            totalTokens: 20000,
          }),
          subagent({
            id: "b",
            name: "Verify",
            status: "completed",
            durationMs: 41000,
            toolUseCount: 3,
            totalTokens: 18800,
          }),
        ])}
        onEnter={() => {}}
      />,
    );
    expect(screen.getByText("Ran 2 subagents")).toBeInTheDocument();
    // Longest row (not the sum — they ran in parallel), summed usage,
    // summed tools.
    expect(screen.getByText("1m 14s · Σ 38.8K · 9 tools")).toBeInTheDocument();
    expect(screen.getByText("View")).toBeInTheDocument();
    // Rows stay out of the transcript until asked for.
    expect(screen.queryByText("Implement")).toBeNull();
  });

  it("omits a metric the providers never reported rather than fabricating it", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({
            id: "a",
            status: "completed",
            durationMs: 41000,
            toolUseCount: 3,
          }),
        ])}
        onEnter={() => {}}
      />,
    );
    // No usage reported → no `Σ` segment at all.
    expect(screen.getByText("0m 41s · 3 tools")).toBeInTheDocument();
  });

  it("expands the settled line back to the rail view", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({ id: "a", name: "Implement", status: "completed" }),
        ])}
        onEnter={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Ran 1 subagent"));
    expect(screen.getByText("Subagents")).toBeInTheDocument();
    expect(screen.getByText("1 task · complete")).toBeInTheDocument();
    expect(screen.getByText("Implement")).toBeInTheDocument();
    // Settled rail styling: the success token, not the accent.
    expect(screen.getByTestId("subagent-group-rail").className).toContain(
      "from-status-open",
    );
  });

  it("reads a failed group as attention on both the settled line and the rail", () => {
    const { container } = render(
      <SubagentsCard
        item={card([subagent({ id: "a", name: "Build", status: "failed" })])}
        onEnter={() => {}}
      />,
    );
    expect(container.querySelector(".text-status-attention")).not.toBeNull();

    fireEvent.click(screen.getByText("Ran 1 subagent"));
    expect(screen.getByTestId("subagent-group-rail")).toHaveAttribute(
      "data-state",
      "failed",
    );
  });

  it("does not collapse a group whose work is only queued", () => {
    render(
      <SubagentsCard
        item={card([subagent({ id: "a", name: "Queued", status: "pending" })])}
        onEnter={() => {}}
      />,
    );
    // `pending` has not settled — the rail view stays up.
    expect(screen.getByText("Subagents")).toBeInTheDocument();
    expect(screen.queryByText("Ran 1 subagent")).toBeNull();
  });
});

// ── Agent orbs ──
//
// Design rule: one orb per live thing. Running rows animate and describe
// what they are doing; finished rows drop back to a flat check, and the
// group header carries no orb of its own (the whole run's orb lives on
// the composer strip).

describe("SubagentsCard — agent orbs", () => {
  const orbStates = () =>
    [...document.querySelectorAll("canvas[data-orb-state]")].map((c) =>
      c.getAttribute("data-orb-state"),
    );

  it("matches each running row's orb to its current tool, with no header orb", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({
            id: "a",
            name: "Explore",
            status: "running",
            items: [{ ...tool("t1", "Grep", {}), status: "running" }],
          }),
          subagent({
            id: "b",
            name: "Ship",
            status: "running",
            items: [
              {
                ...tool("t2", "Bash", { command: "git push" }),
                status: "running",
              },
            ],
          }),
        ])}
        onEnter={() => {}}
      />,
    );
    expect(orbStates()).toEqual(["searching", "connecting"]);
  });

  it("gives a queued subagent the breathing orb", () => {
    render(
      <SubagentsCard
        item={card([subagent({ id: "a", name: "Queued", status: "pending" })])}
        onEnter={() => {}}
      />,
    );
    expect(orbStates()).toEqual(["breathing"]);
  });

  it("drops the orb entirely once every row has settled", () => {
    render(
      <SubagentsCard
        item={card([
          subagent({ id: "a", name: "Implement", status: "completed" }),
          subagent({ id: "b", name: "Verify", status: "failed" }),
        ])}
        onEnter={() => {}}
      />,
    );
    expect(orbStates()).toEqual([]);
    // …and it stays gone when the settled group is expanded.
    fireEvent.click(screen.getByText("Ran 2 subagents"));
    expect(orbStates()).toEqual([]);
  });
});
