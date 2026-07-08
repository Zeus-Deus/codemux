/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type {
  ChatViewItem,
  PermissionRequestItem,
  ToolCallItem,
  WorkflowRunItem,
} from "@/lib/agent-chat/types";

import { MessageList } from "./MessageList";

// The assistant-turn avatar renders the provider's branded mark via
// ProviderLogo, which imports the SVG assets at module load. vitest's
// jsdom env doesn't serve `?import` URLs, so stub the marks for a
// predictable path string (same pattern as provider-logo.test.tsx).
vi.mock("@/assets/preset-icons/claude.svg", () => ({
  default: "/mock/claude.svg",
}));
vi.mock("@/assets/preset-icons/codex.svg", () => ({
  default: "/mock/codex.svg",
}));
vi.mock("@/assets/preset-icons/opencode.svg", () => ({
  default: "/mock/opencode.svg",
}));

afterEach(() => cleanup());

// The MessageScroller renders every row into the DOM (perf comes from
// `content-visibility:auto`, not row unmounting), so a plain render is
// enough to assert on grouping / dispatch / marker DOM — no virtualizer
// mock context is needed.
function renderList(
  messages: ChatViewItem[],
  extra?: { showThinking?: boolean; streaming?: boolean; sessionStartedAt?: number },
) {
  return render(
    <MessageList messages={messages} {...extra} {...noopHandlers} />,
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

function workflowRunItem(overrides: Partial<WorkflowRunItem> = {}): WorkflowRunItem {
  return {
    kind: "workflow_run",
    id: "wf-1",
    seq: 0,
    workflowId: "wf-1",
    status: "pending_approval",
    name: "Audit route auth",
    description: null,
    script: null,
    plannedPhases: [{ title: "Discover route files", detail: null }],
    phases: [{ title: "Discover route files", detail: null, agents: [] }],
    resultText: null,
    totalTokens: null,
    agentCount: null,
    startedAt: Date.now(),
    durationMs: null,
    approvalRequestId: "req-wf",
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
    renderList([planReq()]);
    expect(screen.getByText("Plan proposed")).toBeInTheDocument();
    expect(screen.getByText("Accept & execute")).toBeInTheDocument();
  });

  it("reduces request_kind=user-input to a transcript marker; full panel lives with the composer", () => {
    renderList([askReq()]);
    expect(
      screen.getByText(/Input requested — answer above the composer/),
    ).toBeInTheDocument();
    expect(screen.queryByText("React")).toBeNull();
    expect(screen.queryByText("Submit")).toBeNull();
    expect(screen.queryByText(/Approval requested/)).toBeNull();
  });

  it("falls back to PermissionRequestBlock for unknown request_kind", () => {
    renderList([genericReq()]);
    expect(screen.getByText(/Approval requested/)).toBeInTheDocument();
  });

  describe("specialized request kinds are not swallowed by the tool_use_id merge", () => {
    it("plan PermissionRequestItem renders PlanProposalBlock even with an orphan ExitPlanMode ToolCallItem", () => {
      const tool: ToolCallItem = {
        kind: "tool_call",
        id: "orphan-tool",
        seq: 0,
        tool_use_id: "tu-plan-x",
        tool_name: "ExitPlanMode",
        input: { plan: "# Refactor" },
        status: "running",
        result_content: null,
        approval_request_id: null,
      };
      const req = planReq({ request_id: "tu-plan-x", tool_use_id: "tu-plan-x" });
      renderList([tool, req]);
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
      const req = askReq({ request_id: "tu-ask-x", tool_use_id: "tu-ask-x" });
      renderList([tool, req]);
      expect(
        screen.getByText(/Input requested — answer above the composer/),
      ).toBeInTheDocument();
      expect(screen.queryByText(/Approval requested/)).toBeNull();
    });
  });
});

describe("MessageList activity blocks", () => {
  it("folds a run of ≥2 completed tool calls into one settled activity block", () => {
    const messages: ChatViewItem[] = [
      readCall(0, "/a"),
      readCall(1, "/b"),
      readCall(2, "/c"),
    ];
    renderList(messages);
    // Settled header: derived summary + step meta + Details toggle. Step
    // rows stay hidden until expand.
    expect(screen.getByText("Explored the codebase")).toBeInTheDocument();
    expect(screen.getByText("3 steps")).toBeInTheDocument();
    expect(screen.getByText("Details")).toBeInTheDocument();
    expect(screen.queryByText("/a")).toBeNull();

    // Expand → per-step rows appear; the toggle flips to "Hide".
    fireEvent.click(screen.getByText("Explored the codebase"));
    expect(screen.getByText("/a")).toBeInTheDocument();
    expect(screen.getByText("/c")).toBeInTheDocument();
    expect(screen.getByText("Hide")).toBeInTheDocument();
  });

  it("renders a lone completed tool call as a single card, not an activity block", () => {
    renderList([readCall(0, "/only")]);
    expect(screen.queryByText(/steps/)).toBeNull();
    expect(screen.queryByText("Details")).toBeNull();
    // Single card shows the mono summary via ToolCallStatus.
    expect(screen.getByText("Read")).toBeInTheDocument();
    expect(screen.getByText("/only")).toBeInTheDocument();
  });

  it("shows a WORKING header (live action + counter) for the streaming tail run", () => {
    const messages: ChatViewItem[] = [
      readCall(0, "/a"),
      {
        kind: "tool_call",
        id: "tc-run",
        seq: 1,
        tool_use_id: "tu-run",
        tool_name: "Bash",
        input: { command: "cargo test" },
        status: "running",
        result_content: null,
        approval_request_id: null,
      },
    ];
    renderList(messages, { streaming: true, showThinking: false });
    expect(screen.getByText("Working")).toBeInTheDocument();
    // Live action = the running step; counter = 1 done · 1 running.
    expect(screen.getByText("1 done · 1 running")).toBeInTheDocument();
    expect(screen.getByText("run cargo test")).toBeInTheDocument();
  });

  it("a non-tool row breaks the run into two independent activity blocks", () => {
    const messages: ChatViewItem[] = [
      readCall(0, "/x0"),
      readCall(1, "/x1"),
      {
        kind: "assistant_message",
        id: "am-1",
        seq: 2,
        turn_id: "t1",
        text: "between bursts",
        streaming: false,
      },
      readCall(3, "/y0"),
      readCall(4, "/y1"),
    ];
    renderList(messages);
    expect(screen.getAllByText("Explored the codebase")).toHaveLength(2);
    expect(screen.getByText("between bursts")).toBeInTheDocument();
  });

  it("never swallows a pending-approval tool call into an activity block", () => {
    const pending: ToolCallItem = {
      kind: "tool_call",
      id: "tc-guard",
      seq: 2,
      tool_use_id: "tu-guard",
      tool_name: "Bash",
      input: { command: "rm -rf build" },
      status: "running",
      result_content: null,
      approval_request_id: "req-1",
    };
    const approval: PermissionRequestItem = {
      kind: "permission_request",
      id: "req-1",
      seq: 3,
      request_id: "req-1",
      turn_id: "turn-1",
      request_kind: "command",
      payload: { tool_name: "Bash", tool_input: { command: "rm -rf build" } },
      tool_use_id: "tu-guard",
      resolution: { state: "pending" },
    };
    renderList([readCall(0, "/a"), readCall(1, "/b"), pending, approval]);
    // Two completed reads fold into an activity block; the gated Bash call
    // stays a standalone card with its approval footer visible.
    expect(screen.getByText("Explored the codebase")).toBeInTheDocument();
    expect(screen.getByText("Allow")).toBeInTheDocument();
    expect(screen.getByText("Deny")).toBeInTheDocument();
  });
});

describe("MessageList provider avatar", () => {
  const assistantTurn: ChatViewItem[] = [
    {
      kind: "assistant_message",
      id: "am-1",
      seq: 0,
      turn_id: "t1",
      text: "hello from the agent",
      streaming: false,
    },
  ];

  it("threads the provider through to the assistant-turn avatar mark", () => {
    const { container } = render(
      <MessageList messages={assistantTurn} provider="codex" {...noopHandlers} />,
    );
    const img = container.querySelector(
      "img[data-provider]",
    ) as HTMLImageElement;
    expect(img).not.toBeNull();
    expect(img.getAttribute("data-provider")).toBe("codex");
    expect(img.getAttribute("src")).toContain("codex.svg");
  });

  it("falls back to the sparkle avatar when no provider is passed", () => {
    const { container } = render(
      <MessageList messages={assistantTurn} {...noopHandlers} />,
    );
    // No branded mark image; the ember sparkle (inline svg) shows instead.
    expect(container.querySelector("img[data-provider]")).toBeNull();
  });
});

describe("MessageList chrome", () => {
  it("renders a session-start divider (plain label without a timestamp)", () => {
    renderList([readCall(0, "/a")]);
    expect(screen.getByText("Session started")).toBeInTheDocument();
  });

  it("renders a dated session-start marker when sessionStartedAt is provided", () => {
    renderList([readCall(0, "/a")], {
      sessionStartedAt: new Date().getTime(),
    });
    expect(screen.queryByText("Session started")).toBeNull();
    expect(screen.getByText(/Today ·/)).toBeInTheDocument();
  });

  it("wires up a jump-to-latest control", () => {
    renderList([readCall(0, "/a")]);
    expect(screen.getByText("Jump to latest")).toBeInTheDocument();
  });

  it("renders the streaming marker as a row when showThinking is set", () => {
    renderList(
      [
        {
          kind: "user_message",
          id: "um-1",
          seq: 0,
          text: "do the thing",
        },
      ],
      { showThinking: true },
    );
    expect(screen.getByRole("status", { name: "Agent is working" })).toBeInTheDocument();
    expect(screen.getByText("Working…")).toBeInTheDocument();
  });

  it("does not render the streaming marker when showThinking is unset", () => {
    renderList([readCall(0, "/a")]);
    expect(screen.queryByRole("status", { name: "Agent is working" })).toBeNull();
  });
});

describe("MessageList workflow_run dispatch", () => {
  it("renders the approval card full-width and suppresses the generic permission_request row it owns", () => {
    const req: PermissionRequestItem = {
      kind: "permission_request",
      id: "req-wf",
      seq: 1,
      request_id: "req-wf",
      turn_id: "t1",
      request_kind: "workflow",
      payload: {},
      tool_use_id: null,
      resolution: { state: "pending" },
    };
    renderList([workflowRunItem(), req]);
    expect(screen.getByTestId("workflow-approval-card")).toBeInTheDocument();
    expect(screen.getByText("Run as a workflow?")).toBeInTheDocument();
    // The generic PermissionRequestBlock fallback ("Approval requested…")
    // must not also render for the request the workflow card owns.
    expect(screen.queryByText(/Approval requested/)).toBeNull();
  });

  it("still renders an unrelated permission_request normally alongside a workflow card", () => {
    const unrelated: PermissionRequestItem = {
      kind: "permission_request",
      id: "req-other",
      seq: 1,
      request_id: "req-other",
      turn_id: "t1",
      request_kind: "mcp-tool-use",
      payload: { tool_name: "SomeTool" },
      tool_use_id: null,
      resolution: { state: "pending" },
    };
    renderList([workflowRunItem(), unrelated]);
    expect(screen.getByTestId("workflow-approval-card")).toBeInTheDocument();
    expect(screen.getByText(/Approval requested/)).toBeInTheDocument();
  });

  it("routes a running workflow to the inline progress row", () => {
    renderList([workflowRunItem({ status: "running", approvalRequestId: null })]);
    expect(screen.getByTestId("workflow-run-card")).toBeInTheDocument();
    expect(screen.getByText("Workflow running")).toBeInTheDocument();
  });

  it("routes a completed workflow to the summary row", () => {
    renderList([
      workflowRunItem({
        status: "completed",
        approvalRequestId: null,
        agentCount: 3,
        durationMs: 5000,
      }),
    ]);
    expect(screen.getByText(/Workflow complete/)).toBeInTheDocument();
  });
});
