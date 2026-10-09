/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";

import type { SubagentView, ToolCallItem } from "@/lib/agent-chat/types";

import { DelegationCard } from "./DelegationCard";
import type { DelegationEntry } from "./transcript-slots";

vi.mock("@/assets/preset-icons/claude.svg", () => ({ default: "/mock/claude.svg" }));
vi.mock("@/assets/preset-icons/codex.svg", () => ({ default: "/mock/codex.svg" }));

const { interruptTurn, openSearchResult } = vi.hoisted(() => ({
  interruptTurn: vi.fn(),
  openSearchResult: vi.fn(),
}));
vi.mock("@/tauri/commands", () => ({
  agentChatInterruptTurn: interruptTurn,
  agentChatOpenSearchResult: openSearchResult,
}));
vi.mock("@/lib/toast", () => ({ toast: { error: vi.fn() } }));

beforeEach(() => {
  interruptTurn.mockReset().mockResolvedValue(true);
  openSearchResult.mockReset().mockResolvedValue({ pane_id: "p", workspace_id: "w" });
});

afterEach(() => {
  cleanup();
});

function call(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tc-1",
    seq: 1,
    turn_id: "t1",
    tool_use_id: "tu-1",
    tool_name: "mcp__codemux__delegate_task",
    input: {
      provider: "codex",
      title: "Add slugify helper",
      task: "Add slugify(text) with table tests.",
      model: "gpt-6.1-codex",
      effort: "high",
    },
    status: "done",
    result_content: [
      {
        type: "text",
        text: JSON.stringify({ started: { provider: "codex", thread: "child-1" } }),
      },
    ],
    approval_request_id: null,
    started_at: 1_000,
    ...overrides,
  };
}

function view(overrides: Partial<SubagentView> = {}): SubagentView {
  return {
    id: "delegate:child-1",
    name: "Codex",
    agentType: "codex",
    description: "Add slugify helper",
    model: "gpt-6.1-codex",
    effort: "high",
    status: "running",
    activity: "Working in its tab",
    items: [],
    toneIndex: 0,
    startedAt: Date.now() - 134_000,
    ...overrides,
  };
}

function entry(c: ToolCallItem | null, v: SubagentView | null): DelegationEntry {
  return { key: c?.id ?? v?.id ?? "k", call: c, view: v };
}

describe("DelegationCard", () => {
  it("shows who, what and where it stands for a running task", () => {
    render(<DelegationCard entries={[entry(call(), view())]} />);
    const row = screen.getByTestId("delegation-row");
    expect(row).toHaveAttribute("data-phase", "working");
    expect(within(row).getByText("Add slugify helper")).toBeInTheDocument();
    expect(within(row).getByTestId("delegation-row-meta")).toHaveTextContent(
      "Codex · gpt-6.1-codex · high",
    );
    expect(within(row).getByTestId("delegation-row-status")).toHaveTextContent("Working");
    expect(within(row).getByTestId("delegation-row-line")).toHaveTextContent(
      "Working in its tab",
    );
    expect(within(row).getByTestId("delegation-row-elapsed")).toHaveTextContent("2m 14s");
    expect(within(row).getByRole("img", { name: "Codex" })).toHaveAttribute(
      "src",
      "/mock/codex.svg",
    );
    // A single delegation needs no group header.
    expect(screen.queryByTestId("delegation-group-header")).toBeNull();
  });

  it("opens the child chat from the row and from Open", () => {
    render(<DelegationCard entries={[entry(call(), view())]} />);
    fireEvent.click(screen.getByRole("button", { name: "Open Codex · Add slugify helper" }));
    fireEvent.click(screen.getByTestId("delegation-row-open"));
    expect(openSearchResult).toHaveBeenCalledTimes(2);
    expect(openSearchResult).toHaveBeenCalledWith("child-1");
  });

  it("stops through the child's interrupt and holds Stopping… until it settles", () => {
    const { rerender } = render(<DelegationCard entries={[entry(call(), view())]} />);
    fireEvent.click(screen.getByTestId("delegation-row-stop"));
    expect(interruptTurn).toHaveBeenCalledWith("codex", "child-1");
    expect(screen.getByTestId("delegation-row-stop")).toHaveTextContent("Stopping…");
    expect(screen.getByTestId("delegation-row-stop")).toBeDisabled();
    rerender(
      <DelegationCard
        entries={[entry(call(), view({ status: "stopped", resultText: "Stopped from Codemux" }))]}
      />,
    );
    expect(screen.queryByTestId("delegation-row-stop")).toBeNull();
    expect(screen.getByTestId("delegation-row-status")).toHaveTextContent("Stopped");
  });

  it("swallows a stop error such as 'no active turn'", async () => {
    interruptTurn.mockRejectedValue(new Error("no active turn"));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    render(<DelegationCard entries={[entry(call(), view())]} />);
    fireEvent.click(screen.getByTestId("delegation-row-stop"));
    await vi.waitFor(() => expect(warn).toHaveBeenCalled());
    warn.mockRestore();
  });

  it("reads Starting… from the call alone, with nothing to open or stop yet", () => {
    render(
      <DelegationCard entries={[entry(call({ status: "running", result_content: null }), null)]} />,
    );
    expect(screen.getByTestId("delegation-row")).toHaveAttribute("data-phase", "starting");
    expect(screen.getByTestId("delegation-row-line")).toHaveTextContent("Starting…");
    expect(screen.queryByTestId("delegation-row-open")).toBeNull();
    expect(screen.queryByTestId("delegation-row-stop")).toBeNull();
  });

  it("shows a refused call as Failed with the reason", () => {
    render(
      <DelegationCard
        entries={[
          entry(
            call({
              status: "error",
              result_content: [
                {
                  type: "text",
                  text: "Delegation needs this chat in Full access; it is in plan. Switch to Full access to delegate.",
                },
              ],
            }),
            null,
          ),
        ]}
      />,
    );
    expect(screen.getByTestId("delegation-row-status")).toHaveTextContent("Failed");
    expect(screen.getByTestId("delegation-row-line")).toHaveTextContent(
      "Delegation needs this chat in Full access",
    );
    expect(screen.getByTestId("delegation-row-elapsed")).toHaveTextContent("");
  });

  it("freezes on the report excerpt once done, keeping only Open", () => {
    render(
      <DelegationCard
        entries={[
          entry(
            call(),
            view({
              status: "completed",
              resultText: "Added slugify to src/lib/strings.ts.\nChecked: 9 passed.",
              durationMs: 372_000,
            }),
          ),
        ]}
      />,
    );
    expect(screen.getByTestId("delegation-row-status")).toHaveTextContent("Done");
    expect(screen.getByTestId("delegation-row-line")).toHaveTextContent(
      "Added slugify to src/lib/strings.ts. Checked: 9 passed.",
    );
    expect(screen.getByTestId("delegation-row-elapsed")).toHaveTextContent("6m 12s");
    // The row carries the tooltip: the open overlay covers the truncated text.
    expect(screen.getByTestId("delegation-row")).toHaveAttribute(
      "title",
      "Add slugify helper\nCodex · gpt-6.1-codex · high\nAdded slugify to src/lib/strings.ts. Checked: 9 passed.",
    );
    expect(screen.queryByTestId("delegation-row-stop")).toBeNull();
    expect(screen.getByTestId("delegation-row-open")).toBeInTheDocument();
  });

  it("shows no timer for a task Codemux was closed on (no duration)", () => {
    render(
      <DelegationCard
        entries={[
          entry(
            call(),
            view({
              status: "stopped",
              resultText: "Codemux closed before this task finished.",
              finishedAt: Date.now(),
              startedAt: Date.now() - 5 * 3_600_000,
            }),
          ),
        ]}
      />,
    );
    expect(screen.getByTestId("delegation-row-status")).toHaveTextContent("Stopped");
    expect(screen.getByTestId("delegation-row-elapsed")).toHaveTextContent(/^$/);
  });

  it("heads a group with a status rollup and one row per agent", () => {
    render(
      <DelegationCard
        entries={[
          entry(call(), view()),
          entry(
            null,
            view({
              id: "delegate:child-2",
              name: "Claude",
              agentType: "claude",
              description: "Review the parser",
              model: "claude-opus-4-7",
              effort: "max",
              activity: "Waiting for your answer in its tab",
            }),
          ),
        ]}
      />,
    );
    expect(screen.getByTestId("delegation-group-header")).toHaveTextContent(
      "2 agents · 1 working · 1 waiting for you",
    );
    const rows = screen.getAllByTestId("delegation-row");
    expect(rows.map((r) => r.getAttribute("data-phase"))).toEqual(["working", "waiting"]);
    expect(within(rows[1]).getByTestId("delegation-row-status")).toHaveTextContent(
      "Waiting for you",
    );
  });
});
