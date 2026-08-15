/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ChatViewItem, SubagentView } from "@/lib/agent-chat/types";
import { useUIStore } from "@/stores/ui-store";

import { SubagentsPane } from "./subagents-pane";

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

function messages(subagents: SubagentView[]): ChatViewItem[] {
  return [
    {
      kind: "subagent_run",
      id: "run-1",
      seq: 0,
      turn_id: "turn-1",
      subagents,
    },
  ];
}

beforeEach(() => useUIStore.setState({ subagentEnterRequest: null }));
afterEach(cleanup);

describe("SubagentsPane — focused watch surface", () => {
  it("shows aggregate progress, live cards, and compact finished rows", () => {
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([
          subagent({
            id: "live",
            name: "Pricing audit",
            model: "anthropic/claude-opus-4-8",
            toolUseCount: 12,
          }),
          subagent({
            id: "done",
            name: "Verification pass",
            model: "openai/gpt-5.4",
            status: "completed",
            activity: "All checks pass",
            durationMs: 16_000,
            toolUseCount: 4,
          }),
        ])}
      />,
    );

    expect(screen.getByText("1 / 2")).toBeInTheDocument();
    expect(screen.getByLabelText("1 live subagents")).toBeInTheDocument();
    expect(screen.getByText("Finished · 1")).toBeInTheDocument();
    expect(screen.getByText("Pricing audit")).toBeInTheDocument();
    expect(screen.getByText("Verification pass")).toBeInTheDocument();
    expect(
      screen.getByTitle("Model: anthropic/claude-opus-4-8"),
    ).toHaveTextContent("anthropic/claude-opus-4-8");
    expect(screen.getByTitle("Model: openai/gpt-5.4")).toHaveTextContent(
      "openai/gpt-5.4",
    );
    expect(screen.getByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "1",
    );
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

  it("opens a live subagent thread from the panel", () => {
    render(
      <SubagentsPane
        threadId="thread-1"
        messages={messages([subagent({ id: "sub-42" })])}
      />,
    );
    fireEvent.click(screen.getByText("Open thread"));
    expect(useUIStore.getState().subagentEnterRequest).toMatchObject({
      threadId: "thread-1",
      subagentId: "sub-42",
    });
  });
});
