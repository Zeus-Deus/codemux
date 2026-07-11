/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type {
  PermissionRequestItem,
  SubagentView,
  WorkflowPhaseView,
  WorkflowRunItem,
} from "@/lib/agent-chat/types";
import { useUIStore } from "@/stores/ui-store";

import { formatCompactTokens, WorkflowRunCard } from "./WorkflowRunCard";

afterEach(() => {
  cleanup();
  useUIStore.setState({ rightPanelTabs: {} });
});

function workflowItem(overrides: Partial<WorkflowRunItem> = {}): WorkflowRunItem {
  return {
    kind: "workflow_run",
    id: "wf-1",
    seq: 0,
    workflowId: "wf-1",
    status: "pending_approval",
    name: "Audit route auth",
    description: "Audit every route handler for missing auth checks.",
    script: "console.log('discover then audit then verify')",
    plannedPhases: [
      { title: "Discover route files", detail: "~1 agent" },
      { title: "Audit each file for missing auth", detail: "~42 agents" },
    ],
    phases: [
      { title: "Discover route files", detail: "~1 agent", agents: [] },
      { title: "Audit each file for missing auth", detail: "~42 agents", agents: [] },
    ],
    resultText: null,
    totalTokens: null,
    agentCount: null,
    startedAt: Date.now(),
    durationMs: null,
    approvalRequestId: "req-wf-1",
    ...overrides,
  };
}

function pendingRequest(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-wf-1",
    seq: 0,
    request_id: "req-wf-1",
    turn_id: "t1",
    request_kind: "workflow",
    payload: {},
    tool_use_id: null,
    resolution: { state: "pending" },
    ...overrides,
  };
}

function agent(overrides: Partial<SubagentView> = {}): SubagentView {
  return {
    id: "a1",
    status: "completed",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

function phase(overrides: Partial<WorkflowPhaseView> = {}): WorkflowPhaseView {
  return { title: "Phase", detail: null, agents: [], ...overrides };
}

describe("WorkflowRunCard — pending_approval", () => {
  it("renders planned phases and all four action buttons", () => {
    render(
      <WorkflowRunCard
        item={workflowItem()}
        approval={pendingRequest()}
        onDecide={vi.fn()}
        workspaceId="ws-1"
      />,
    );
    expect(screen.getByTestId("workflow-approval-card")).toBeInTheDocument();
    expect(screen.getByText("Run as a workflow?")).toBeInTheDocument();
    expect(screen.getByText("Discover route files")).toBeInTheDocument();
    expect(
      screen.getByText("Audit each file for missing auth"),
    ).toBeInTheDocument();
    expect(screen.getByText("Run once")).toBeInTheDocument();
    expect(screen.getByText("Always for this project")).toBeInTheDocument();
    expect(screen.getByText("View script")).toBeInTheDocument();
    expect(screen.getByText("Deny")).toBeInTheDocument();
  });

  it("Run once sends a plain allow decision", () => {
    const onDecide = vi.fn();
    render(
      <WorkflowRunCard
        item={workflowItem()}
        approval={pendingRequest()}
        onDecide={onDecide}
      />,
    );
    fireEvent.click(screen.getByText("Run once"));
    expect(onDecide).toHaveBeenCalledWith({ decision: "allow" });
  });

  it("Always for this project attaches a project-scoped permission update", () => {
    const onDecide = vi.fn();
    render(
      <WorkflowRunCard
        item={workflowItem()}
        approval={pendingRequest()}
        onDecide={onDecide}
      />,
    );
    fireEvent.click(screen.getByText("Always for this project"));
    expect(onDecide).toHaveBeenCalledWith({
      decision: "allow",
      updated_permissions: [
        {
          type: "addRules",
          rules: [{ toolName: "Workflow" }],
          behavior: "allow",
          destination: "localSettings",
        },
      ],
    });
  });

  it("Deny sends a deny decision with a message", () => {
    const onDecide = vi.fn();
    render(
      <WorkflowRunCard
        item={workflowItem()}
        approval={pendingRequest()}
        onDecide={onDecide}
      />,
    );
    fireEvent.click(screen.getByText("Deny"));
    expect(onDecide).toHaveBeenCalledWith({
      decision: "deny",
      message: expect.any(String),
    });
  });

  it("View script opens a dialog showing the script text", () => {
    render(
      <WorkflowRunCard
        item={workflowItem({ script: "console.log('hi from the plan')" })}
        approval={pendingRequest()}
        onDecide={vi.fn()}
      />,
    );
    expect(screen.queryByText("console.log('hi from the plan')")).toBeNull();
    fireEvent.click(screen.getByText("View script"));
    expect(
      screen.getByText("console.log('hi from the plan')"),
    ).toBeInTheDocument();
  });

  it("disables the action buttons while the decision is in flight", () => {
    render(
      <WorkflowRunCard
        item={workflowItem()}
        approval={pendingRequest({ resolution: { state: "responding", decision: { decision: "allow" } } })}
        onDecide={vi.fn()}
      />,
    );
    expect(screen.getByText("Run once")).toBeDisabled();
    expect(screen.getByText("Deny")).toBeDisabled();
    expect(screen.getByText("Submitting decision…")).toBeInTheDocument();
  });
});

describe("WorkflowRunCard — running", () => {
  it("shows phase progress + active-agent count and opens the panel on click", () => {
    const item = workflowItem({
      status: "running",
      phases: [
        phase({ title: "Discover route files", agents: [agent({ id: "a1", status: "completed" })] }),
        phase({
          title: "Audit each file for missing auth",
          agents: [
            agent({ id: "a2", status: "running", startedAt: Date.now() }),
            agent({ id: "a3", status: "running", startedAt: Date.now() }),
          ],
        }),
      ],
    });
    render(
      <WorkflowRunCard item={item} approval={null} onDecide={vi.fn()} workspaceId="ws-1" />,
    );
    expect(screen.getByTestId("workflow-run-card")).toHaveAttribute(
      "data-status",
      "running",
    );
    expect(screen.getByText("Workflow running")).toBeInTheDocument();
    expect(screen.getByText(/Phase 2 of 2/)).toBeInTheDocument();
    expect(screen.getByText(/2 agents active/)).toBeInTheDocument();
    expect(screen.getByTestId("workflow-open-panel")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("workflow-run-card"));
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("orchestration");
  });

  it("is inert (no throw, no panel change) when no workspaceId is supplied", () => {
    const item = workflowItem({ status: "running" });
    render(<WorkflowRunCard item={item} approval={null} onDecide={vi.fn()} />);
    fireEvent.click(screen.getByTestId("workflow-run-card"));
    expect(useUIStore.getState().rightPanelTabs).toEqual({});
  });
});

describe("WorkflowRunCard — terminal summary rows", () => {
  it("renders a completed summary row and opens the panel on click", () => {
    const item = workflowItem({
      status: "completed",
      agentCount: 44,
      durationMs: 291000,
      phases: [phase({ title: "p1" }), phase({ title: "p2" }), phase({ title: "p3" })],
      plannedPhases: [
        { title: "p1", detail: null },
        { title: "p2", detail: null },
        { title: "p3", detail: null },
      ],
    });
    render(
      <WorkflowRunCard item={item} approval={null} onDecide={vi.fn()} workspaceId="ws-1" />,
    );
    expect(screen.getByTestId("workflow-run-card")).toHaveAttribute(
      "data-status",
      "completed",
    );
    const text = screen.getByText(/Workflow complete/);
    expect(text.textContent).toContain("44 agents");
    expect(text.textContent).toContain("3 phases");
    expect(text.textContent).toContain("4m 51s");
    expect(screen.getByText("View run")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("workflow-run-card"));
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("orchestration");
  });

  it("renders a failed summary row with the attention tone", () => {
    const item = workflowItem({ status: "failed", agentCount: 5, durationMs: 12000 });
    const { container } = render(
      <WorkflowRunCard item={item} approval={null} onDecide={vi.fn()} />,
    );
    expect(screen.getByText(/Workflow failed/)).toBeInTheDocument();
    expect(container.querySelector(".text-status-attention")).not.toBeNull();
  });

  it("renders a stopped summary row with a muted tone", () => {
    const item = workflowItem({ status: "stopped", agentCount: 2, durationMs: 4000 });
    const { container } = render(
      <WorkflowRunCard item={item} approval={null} onDecide={vi.fn()} />,
    );
    expect(screen.getByText(/Workflow stopped/)).toBeInTheDocument();
    expect(container.querySelector(".text-status-attention")).toBeNull();
  });
});

describe("formatCompactTokens", () => {
  it("renders small counts as plain integers", () => {
    expect(formatCompactTokens(42)).toBe("42");
    expect(formatCompactTokens(0)).toBe("0");
  });

  it("renders thousands with a K suffix", () => {
    expect(formatCompactTokens(12000)).toBe("12K");
  });

  it("renders millions with an M suffix, dropping a trailing .0", () => {
    expect(formatCompactTokens(2_900_000)).toBe("2.9M");
    expect(formatCompactTokens(3_000_000)).toBe("3M");
  });
});
