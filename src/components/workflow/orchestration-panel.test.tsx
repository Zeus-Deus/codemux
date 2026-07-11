/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";

import type {
  SubagentView,
  ToolCallItem,
  UserMessageItem,
  WorkflowPhaseView,
  WorkflowRunItem,
} from "@/lib/agent-chat/types";
import type { WorkspaceSnapshot } from "@/tauri/types";

// Polyfill ResizeObserver for ScrollArea (not available in jsdom)
globalThis.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const mocks = vi.hoisted(() => ({
  interruptTurn: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/tauri/commands", () => ({
  agentChatInterruptTurn: (...a: unknown[]) => mocks.interruptTurn(...a),
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { OrchestrationPanel } from "./orchestration-panel";

function renderPanel(props: React.ComponentProps<typeof OrchestrationPanel>) {
  return render(
    <TooltipProvider>
      <OrchestrationPanel {...props} />
    </TooltipProvider>,
  );
}

// ── Fixtures ──

function userMsg(text: string, seq = 0): UserMessageItem {
  return { kind: "user_message", id: `u-${seq}`, seq, text };
}

function toolCall(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: `t-${Math.random()}`,
    seq: 1,
    tool_use_id: "tu-1",
    tool_name: "Read",
    input: {},
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

function agent(overrides: Partial<SubagentView> = {}): SubagentView {
  return {
    id: `a-${Math.random()}`,
    status: "running",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

function phase(overrides: Partial<WorkflowPhaseView> = {}): WorkflowPhaseView {
  return { title: "Phase", detail: null, agents: [], ...overrides };
}

function run(overrides: Partial<WorkflowRunItem> = {}): WorkflowRunItem {
  return {
    kind: "workflow_run",
    id: "wf-item-1",
    seq: 0,
    workflowId: "wf-1",
    status: "running",
    name: "Audit route auth",
    description: null,
    script: null,
    plannedPhases: [],
    phases: [],
    resultText: null,
    totalTokens: null,
    agentCount: null,
    startedAt: 0,
    durationMs: null,
    approvalRequestId: null,
    ...overrides,
  };
}

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "demo",
    workspace_type: "standard",
    cwd: "/p",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/p",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "tab-1",
    active_surface_id: "surface-1",
    surfaces: [],
    ...overrides,
  };
}

const agentDone = agent({
  id: "a1",
  status: "completed",
  name: "routes/auth.ts",
  resultText: JSON.stringify({ findings: [] }),
  totalTokens: 12_000,
  startedAt: 0,
  durationMs: 9_000,
  items: [userMsg("Audit src/routes/auth.ts for missing auth.")],
});

const agentRunning = agent({
  id: "a2",
  status: "running",
  name: "routes/orders.ts",
  activity: "grepping requireAuth",
  totalTokens: 500,
  startedAt: 0,
});

const agentIssue = agent({
  id: "a3",
  status: "completed",
  name: "routes/billing.ts",
  resultText: JSON.stringify({ findings: [{ n: 1 }, { n: 2 }] }),
  totalTokens: 2_000,
  startedAt: 0,
  durationMs: 5_000,
  items: [
    userMsg("Audit src/routes/billing.ts for endpoints missing requireAuth."),
    toolCall({
      seq: 1,
      tool_name: "Grep",
      input: { pattern: "requireAuth", path: "src/routes/billing.ts" },
      status: "done",
    }),
  ],
});

const agentPending = agent({ id: "a4", status: "pending", name: "routes/admin.ts" });

function makeRun(): WorkflowRunItem {
  return run({
    plannedPhases: [
      { title: "Discover route files", detail: null },
      { title: "Audit for missing auth", detail: null },
    ],
    phases: [
      phase({ title: "Discover route files", agents: [agentDone] }),
      phase({
        title: "Audit for missing auth",
        agents: [agentRunning, agentIssue, agentPending],
      }),
    ],
  });
}

afterEach(() => {
  cleanup();
  mocks.interruptTurn.mockClear();
});

describe("OrchestrationPanel — phases level", () => {
  it("renders one phase row per phase with agent counts and mono stats", () => {
    renderPanel({ workspace: makeWorkspace(), run: makeRun(), threadId: "thread-1" });

    const rows = screen.getAllByTestId("workflow-phase-row");
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText(/1 · Discover route files/)).toBeInTheDocument();
    expect(within(rows[0]).getByText("1 agent")).toBeInTheDocument();
    expect(within(rows[0]).getByText("12K · 0m 09s")).toBeInTheDocument();

    expect(within(rows[1]).getByText(/2 · Audit for missing auth/)).toBeInTheDocument();
    // running phase defaults open — its running/done sub-line and agent
    // rows should already be visible without clicking.
    expect(within(rows[1]).getByText("1 done · 2 running")).toBeInTheDocument();
    expect(within(rows[1]).getAllByTestId("workflow-agent-row")).toHaveLength(3);
  });

  it("filters the open phase's agents by the status chips", () => {
    renderPanel({ workspace: makeWorkspace(), run: makeRun(), threadId: "thread-1" });
    const row = screen.getAllByTestId("workflow-phase-row")[1];

    fireEvent.click(within(row).getByText("issues"));
    expect(within(row).getAllByTestId("workflow-agent-row")).toHaveLength(1);
    expect(within(row).getByText("routes/billing.ts")).toBeInTheDocument();

    fireEvent.click(within(row).getByText("running"));
    expect(within(row).getAllByTestId("workflow-agent-row")).toHaveLength(1);
    expect(within(row).getByText("routes/orders.ts")).toBeInTheDocument();

    fireEvent.click(within(row).getByText("all"));
    expect(within(row).getAllByTestId("workflow-agent-row")).toHaveLength(3);
  });

  it("shows a muted empty state when a filter matches nothing", () => {
    renderPanel({ workspace: makeWorkspace(), run: makeRun(), threadId: "thread-1" });
    const cleanPhaseRow = screen.getAllByTestId("workflow-phase-row")[0];

    // Phase A ("Discover route files") is collapsed by default (only the
    // running phase auto-opens) — open it, then filter to "issues": its
    // one agent is clean, so nothing should match.
    fireEvent.click(within(cleanPhaseRow).getByRole("button"));
    fireEvent.click(within(cleanPhaseRow).getByText("issues"));
    expect(within(cleanPhaseRow).getByText("No agents match.")).toBeInTheDocument();
  });
});

describe("OrchestrationPanel — agent detail drill-in", () => {
  it("shows the prompt, recent tool calls, and result on row click", () => {
    renderPanel({ workspace: makeWorkspace(), run: makeRun(), threadId: "thread-1" });
    const row = screen.getAllByTestId("workflow-phase-row")[1];
    fireEvent.click(within(row).getByText("routes/billing.ts"));

    const detail = screen.getByTestId("workflow-agent-detail");
    expect(detail).toBeInTheDocument();
    expect(
      within(detail).getByText("Audit src/routes/billing.ts for endpoints missing requireAuth."),
    ).toBeInTheDocument();
    expect(within(detail).getByText("grep")).toBeInTheDocument();
    expect(within(detail).getByText("requireAuth")).toBeInTheDocument();
    expect(
      within(detail).getByText(JSON.stringify({ findings: [{ n: 1 }, { n: 2 }] })),
    ).toBeInTheDocument();
    // Phases level is gone while drilled in.
    expect(screen.queryByTestId("workflow-phase-row")).toBeNull();
  });

  it("shows the live activity line instead of a result while the agent is running", () => {
    renderPanel({ workspace: makeWorkspace(), run: makeRun(), threadId: "thread-1" });
    const row = screen.getAllByTestId("workflow-phase-row")[1];
    fireEvent.click(within(row).getByText("routes/orders.ts"));

    const detail = screen.getByTestId("workflow-agent-detail");
    expect(within(detail).getByText("grepping requireAuth")).toBeInTheDocument();
  });
});

describe("OrchestrationPanel — controls", () => {
  it("disables Pause and calls the interrupt command from Stop while running", () => {
    renderPanel({ workspace: makeWorkspace(), run: makeRun(), threadId: "thread-1" });
    expect(screen.getByTestId("workflow-pause")).toBeDisabled();

    const stop = screen.getByTestId("workflow-stop");
    expect(stop).not.toBeDisabled();
    fireEvent.click(stop);
    expect(mocks.interruptTurn).toHaveBeenCalledWith("claude", "thread-1", null);
  });

  it("disables Stop once the run is terminal", () => {
    renderPanel({
      workspace: makeWorkspace(),
      run: run({ status: "completed", phases: [] }),
      threadId: "thread-1",
    });
    expect(screen.getByTestId("workflow-stop")).toBeDisabled();
  });

  it("disables Restart agent in the drill-in", () => {
    renderPanel({ workspace: makeWorkspace(), run: makeRun(), threadId: "thread-1" });
    const row = screen.getAllByTestId("workflow-phase-row")[1];
    fireEvent.click(within(row).getByText("routes/billing.ts"));
    expect(screen.getByRole("button", { name: "Restart agent" })).toBeDisabled();
  });
});
