/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type {
  ReasoningItem,
  SubagentView as SubagentViewModel,
  ToolCallItem,
} from "@/lib/agent-chat/types";

import { SubagentView } from "./SubagentView";

afterEach(cleanup);

function reasoning(overrides: Partial<ReasoningItem> = {}): ReasoningItem {
  return {
    kind: "reasoning",
    id: "r-1",
    seq: 1,
    turn_id: "t1",
    text: "Checking whether the fixture still lines up.",
    streaming: false,
    ...overrides,
  };
}

function runningTool(): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tc-1",
    seq: 1,
    tool_use_id: "tu-1",
    tool_name: "Bash",
    input: { command: "npm test" },
    status: "running",
    result_content: null,
    approval_request_id: null,
    started_at: Date.now(),
  };
}

function subagent(
  overrides: Partial<SubagentViewModel> = {},
): SubagentViewModel {
  return {
    id: "s1",
    name: "explorer",
    status: "running",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

describe("SubagentView", () => {
  it("shows exactly one orb when a lone streaming thought is the whole run", () => {
    const { container } = render(
      <SubagentView
        subagent={subagent({ items: [reasoning({ streaming: true })] })}
      />,
    );
    // The drill-in's live tail owns liveness; the reasoning row must not
    // animate a second orb (one-orb doctrine).
    expect(container.querySelectorAll("[data-orb-state]")).toHaveLength(1);
    expect(screen.getByText("Thinking…")).toBeInTheDocument();
  });

  it("shows no orb once the subagent has settled", () => {
    const { container } = render(
      <SubagentView
        subagent={subagent({
          status: "completed",
          items: [reasoning({ duration_ms: 4000 })],
        })}
      />,
    );
    expect(container.querySelector("[data-orb-state]")).toBeNull();
    expect(screen.getByText("Thought for 4s")).toBeInTheDocument();
  });

  it("ticks a running step while the subagent works", () => {
    render(<SubagentView subagent={subagent({ items: [runningTool()] })} />);
    expect(screen.getByTestId("step-elapsed")).toBeInTheDocument();
  });

  it("does not tick a step left running by a settled subagent", () => {
    render(
      <SubagentView
        subagent={subagent({ status: "failed", items: [runningTool()] })}
      />,
    );
    expect(screen.queryByTestId("step-elapsed")).toBeNull();
  });
});
