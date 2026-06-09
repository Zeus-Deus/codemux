/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { VirtuosoMockContext } from "react-virtuoso";

import type {
  ChatViewItem,
  PermissionRequestItem,
  ToolCallItem,
} from "@/lib/agent-chat/types";

import { MessageList } from "./MessageList";

afterEach(() => cleanup());

/** Render the (virtualized) MessageList inside react-virtuoso's mock
 *  context — jsdom has no layout, so Virtuoso needs synthetic viewport
 *  and row heights to decide what to mount. 2000/100 → a 20-row
 *  window, comfortably larger than any fixture in this file except
 *  the windowing test (which relies on the bound). */
function renderList(messages: ChatViewItem[]) {
  return render(
    <VirtuosoMockContext.Provider
      value={{ viewportHeight: 2000, itemHeight: 100 }}
    >
      <MessageList messages={messages} {...noopHandlers} />
    </VirtuosoMockContext.Provider>,
  );
}

function planReq(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-p",
    seq: 0,
    request_id: "req-p",
    turn_id: "turn-1",
    request_kind: "plan",
    payload: { plan: "# Refactor\n\n- Step one" },
    tool_use_id: null,
    resolution: { state: "pending" },
    ...overrides,
  };
}

function askReq(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-a",
    seq: 0,
    request_id: "req-a",
    turn_id: "turn-1",
    request_kind: "user-input",
    payload: {
      questions: [
        {
          header: "F",
          question: "Framework?",
          multiSelect: false,
          options: [{ label: "React", description: "" }],
        },
      ],
    },
    tool_use_id: null,
    resolution: { state: "pending" },
    ...overrides,
  };
}

function genericReq(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-g",
    seq: 0,
    request_id: "req-g",
    turn_id: "turn-1",
    request_kind: "mcp-tool-use", // hypothetical future kind
    payload: { tool_name: "SomeFutureTool" },
    tool_use_id: null,
    resolution: { state: "pending" },
    ...overrides,
  };
}

const noopHandlers = {
  onRespondToRequest: vi.fn(),
  onAcceptPlan: vi.fn(),
  onRejectPlan: vi.fn(),
};

describe("MessageList dispatch", () => {
  it("routes request_kind=plan to PlanProposalBlock", () => {
    const messages: ChatViewItem[] = [planReq()];
    renderList(messages);
    // PlanProposalBlock's "Plan proposed" label is a stable marker.
    expect(screen.getByText("Plan proposed")).toBeInTheDocument();
    expect(screen.getByText("Accept & execute")).toBeInTheDocument();
  });

  it("reduces request_kind=user-input to a transcript marker; full panel lives with the composer", () => {
    const messages: ChatViewItem[] = [askReq()];
    renderList(messages);
    // The inline transcript row is just a one-line pointer — the
    // actual interactive panel is mounted above the composer by
    // AgentChatPane (see ComposerPendingInputPanel).
    expect(
      screen.getByText(/Input requested — answer above the composer/),
    ).toBeInTheDocument();
    // Neither the options nor a submit button should render inline.
    expect(screen.queryByText("React")).toBeNull();
    expect(screen.queryByText("Submit")).toBeNull();
    expect(screen.queryByText(/Approval requested/)).toBeNull();
  });

  it("falls back to PermissionRequestBlock for unknown request_kind", () => {
    const messages: ChatViewItem[] = [genericReq()];
    renderList(messages);
    // Generic fallback renders the classic "Approval requested" label.
    expect(screen.getByText(/Approval requested/)).toBeInTheDocument();
  });

  describe("Stage 3 fix — specialized request kinds are not swallowed by the tool_use_id merge", () => {
    // With the reducer's SPECIALIZED_TOOLS allowlist in place,
    // ExitPlanMode and AskUserQuestion tool_use blocks never produce
    // a ToolCallItem. But defensively, even if one somehow lands in
    // the slice (e.g. old persisted fixture pre-fix), MessageList
    // should still route the plan/user-input PermissionRequestItem
    // to its specialized renderer — the merge would have been
    // rightly skipped in the reducer because approval_request_id is
    // null on the orphan ToolCallItem.
    it("plan PermissionRequestItem renders PlanProposalBlock even with an orphan ExitPlanMode ToolCallItem in the slice", () => {
      const tool: ToolCallItem = {
        kind: "tool_call",
        id: "orphan-tool",
        seq: 0,
        tool_use_id: "tu-plan-x",
        tool_name: "ExitPlanMode",
        input: { plan: "# Refactor" },
        status: "running",
        result_content: null,
        // null on purpose — the Stage 3 fix keeps the link empty.
        approval_request_id: null,
      };
      const req = planReq({
        request_id: "tu-plan-x",
        tool_use_id: "tu-plan-x",
      });
      renderList([tool, req]);
      // PlanProposalBlock renders, not the generic approval block.
      expect(screen.getByText("Plan proposed")).toBeInTheDocument();
      expect(screen.getByText("Accept & execute")).toBeInTheDocument();
      expect(screen.queryByText(/Approval requested/)).toBeNull();
    });

    it("user-input PermissionRequestItem renders the transcript marker even with an orphan AskUserQuestion ToolCallItem", () => {
      const tool: ToolCallItem = {
        kind: "tool_call",
        id: "orphan-tool",
        seq: 0,
        tool_use_id: "tu-ask-x",
        tool_name: "AskUserQuestion",
        input: { questions: [] },
        status: "running",
        result_content: null,
        approval_request_id: null,
      };
      const req = askReq({
        request_id: "tu-ask-x",
        tool_use_id: "tu-ask-x",
      });
      renderList([tool, req]);
      // The dispatch survives the orphan-ToolCallItem scenario: the
      // user-input request still resolves to the transcript marker
      // (not the generic PermissionRequestBlock fallback).
      expect(
        screen.getByText(/Input requested — answer above the composer/),
      ).toBeInTheDocument();
      expect(screen.queryByText(/Approval requested/)).toBeNull();
    });
  });

  describe("tool-call run collapse", () => {
    // Minimal ToolCallItem factory. `Read` renders as a status line in
    // the transcript; each unique path keeps the rows independently
    // searchable via getByText.
    function readCall(seq: number, path: string): ToolCallItem {
      return {
        kind: "tool_call",
        id: `tc-${seq}`,
        seq,
        tool_use_id: `tu-${seq}`,
        tool_name: "Read",
        input: { file_path: path },
        status: "done",
        result_content: null,
        approval_request_id: null,
      };
    }

    it("renders all tool calls inline when the run is below threshold", () => {
      // 5 consecutive calls < RUN_COLLAPSE_THRESHOLD (6) → no toggle,
      // every row is visible.
      const messages: ChatViewItem[] = [
        readCall(0, "/a"),
        readCall(1, "/b"),
        readCall(2, "/c"),
        readCall(3, "/d"),
        readCall(4, "/e"),
      ];
      renderList(messages);
      expect(screen.queryByText(/earlier tool call/)).toBeNull();
      expect(screen.getByText("/a")).toBeInTheDocument();
      expect(screen.getByText("/e")).toBeInTheDocument();
    });

    it("collapses a run of 8 tool calls to the last 4 with a toggle, and expands on click", () => {
      // 8 ≥ threshold → toggle appears, earliest 4 are hidden by
      // default, latest 4 stay visible.
      const messages: ChatViewItem[] = [];
      for (let i = 0; i < 8; i++) messages.push(readCall(i, `/f${i}`));
      renderList(messages);

      // Collapsed state: f0..f3 hidden, f4..f7 visible.
      expect(
        screen.getByText(/Show 4 earlier tool calls/),
      ).toBeInTheDocument();
      expect(screen.queryByText("/f0")).toBeNull();
      expect(screen.queryByText("/f3")).toBeNull();
      expect(screen.getByText("/f4")).toBeInTheDocument();
      expect(screen.getByText("/f7")).toBeInTheDocument();

      // Click → expanded; all rows present, label flips to Hide.
      fireEvent.click(screen.getByText(/Show 4 earlier tool calls/));
      expect(screen.getByText(/Hide 4 earlier tool calls/)).toBeInTheDocument();
      expect(screen.getByText("/f0")).toBeInTheDocument();
      expect(screen.getByText("/f3")).toBeInTheDocument();
      expect(screen.getByText("/f7")).toBeInTheDocument();
    });

    it("a non-tool-call row (e.g. assistant message) breaks the run into separate groups", () => {
      // 5 + assistant + 5 → two runs, each under threshold, so no
      // collapse toggle appears at all.
      const messages: ChatViewItem[] = [];
      for (let i = 0; i < 5; i++) messages.push(readCall(i, `/x${i}`));
      messages.push({
        kind: "assistant_message",
        id: "am-1",
        seq: 5,
        turn_id: "t1",
        text: "between bursts",
        streaming: false,
      });
      for (let i = 6; i < 11; i++) messages.push(readCall(i, `/y${i}`));
      renderList(messages);
      expect(screen.queryByText(/earlier tool call/)).toBeNull();
      expect(screen.getByText("between bursts")).toBeInTheDocument();
      // Every row from both runs is rendered.
      expect(screen.getByText("/x0")).toBeInTheDocument();
      expect(screen.getByText("/y10")).toBeInTheDocument();
    });

    it("two long runs separated by an assistant message produce two independent toggles", () => {
      const messages: ChatViewItem[] = [];
      for (let i = 0; i < 7; i++) messages.push(readCall(i, `/p${i}`));
      messages.push({
        kind: "assistant_message",
        id: "am-1",
        seq: 7,
        turn_id: "t1",
        text: "between",
        streaming: false,
      });
      for (let i = 8; i < 15; i++) messages.push(readCall(i, `/q${i}`));
      renderList(messages);
      // Each run has 7 items → hides 3, shows 4 → two independent
      // "Show 3 earlier tool calls" toggles.
      const toggles = screen.getAllByText(/Show 3 earlier tool calls/);
      expect(toggles).toHaveLength(2);
    });
  });
});
