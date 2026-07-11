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

describe("SubagentsCard", () => {
  it("renders the aggregate header with done/active counts", () => {
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
    expect(screen.getByText("2 tasks · running in parallel")).toBeInTheDocument();
    expect(screen.getByText("1 done · 1 active")).toBeInTheDocument();
  });

  it("shows a running row's live activity + model, done row's result line", () => {
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

  it("renders a failed aggregate + row when a subagent failed", () => {
    const { container } = render(
      <SubagentsCard
        item={card([subagent({ id: "a", name: "Build", status: "failed" })])}
        onEnter={() => {}}
      />,
    );
    // Failed rows read status-attention (red) rather than status-open.
    expect(
      container.querySelector(".text-status-attention"),
    ).not.toBeNull();
    expect(screen.getByText("1 task · complete")).toBeInTheDocument();
  });

  it("toggles the inline peek and lists recent child tool calls", () => {
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
    // Peek collapsed by default.
    expect(screen.queryByText("Recent activity")).toBeNull();
    // Click the row (the name) to open the peek.
    fireEvent.click(screen.getByText("Implement"));
    expect(screen.getByText("Recent activity")).toBeInTheDocument();
    expect(screen.getByText("src/Composer.tsx")).toBeInTheDocument();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
    expect(screen.getByText("Enter subagent")).toBeInTheDocument();
  });

  it("fires onEnter with the subagent id from the row Enter button", () => {
    const onEnter = vi.fn();
    render(
      <SubagentsCard
        item={card([subagent({ id: "sub-42", name: "Explore" })])}
        onEnter={onEnter}
      />,
    );
    fireEvent.click(screen.getByText("Enter"));
    expect(onEnter).toHaveBeenCalledWith("sub-42");
  });

  it("Enter click does not also toggle the row peek (stopPropagation)", () => {
    render(
      <SubagentsCard
        item={card([subagent({ id: "a", name: "Explore" })])}
        onEnter={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Enter"));
    expect(screen.queryByText("Recent activity")).toBeNull();
  });
});
