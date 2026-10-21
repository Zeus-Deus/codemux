/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

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
  it("renders a flat whole-row link with status, result and elapsed time", () => {
    render(<DelegationCard entries={[entry(call(),view())]} />);
    expect(screen.getByTestId("delegation-row-status")).toHaveTextContent("Running");
    fireEvent.click(screen.getByRole("button", {name:"Open Add slugify helper"}));
    expect(openSearchResult).toHaveBeenCalledWith("child-1");
    expect(screen.queryByTestId("delegation-row-stop")).toBeNull();
    expect(screen.queryByTestId("delegation-row-meta")).toBeNull();
  });
  it("shows a flattened completed report and freezes the elapsed time", () => {
    render(<DelegationCard entries={[entry(call(),view({status:"completed",resultText:"Added `slugify`.\nChecked 9 tests.",durationMs:372000}))]} />);
    expect(screen.getByTestId("delegation-row-line")).toHaveTextContent("Added slugify. Checked 9 tests.");
    expect(screen.getByTestId("delegation-row-elapsed")).toHaveTextContent("6m 12s");
  });
  it("collapses parallel work into a group and expands exact child links", () => {
    render(<DelegationCard entries={[entry(call(),view()),entry(null,view({id:"delegate:child-2",description:"Review parser"}))]} />);
    const group=screen.getByRole("button", {expanded:false});
    expect(group).toHaveTextContent("2 subagents");
    fireEvent.click(group);expect(screen.getAllByTestId("delegation-row")).toHaveLength(2);
    expect(screen.getByRole("button", {name:"Open Review parser"})).toBeInTheDocument();
  });
  it("shows failed admission without inventing a child link", () => {
    render(<DelegationCard entries={[entry(call({status:"error",result_content:"Mode denied"}),null)]} />);
    expect(screen.getByTestId("delegation-row-status")).toHaveTextContent("Failed");
    expect(screen.queryByRole("button")).toBeNull();
  });
});
