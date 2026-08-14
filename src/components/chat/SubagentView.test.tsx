/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type {
  ReasoningItem,
  SubagentView as SubagentViewModel,
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
});
