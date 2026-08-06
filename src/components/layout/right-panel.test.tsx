/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type { WorkflowRunItem } from "@/lib/agent-chat/types";
import type { TasksSnapshot } from "@/tauri/events";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";

// ── Mocks ──
// Sub-panels are irrelevant to right-panel tab wiring — stub them out.
vi.mock("@/components/workspace/file-tree-panel", () => ({
  FileTreePanel: () => <div data-testid="file-tree-panel" />,
}));
vi.mock("@/components/workspace/changes-panel", () => ({
  ChangesPanel: () => <div data-testid="changes-panel" />,
}));
vi.mock("@/components/workspace/review-panel", () => ({
  ReviewPanel: () => <div data-testid="review-panel" />,
}));
vi.mock("@/components/workflow/orchestration-panel", () => ({
  OrchestrationPanel: () => <div data-testid="orchestration-panel-stub" />,
}));

const mocks = vi.hoisted(() => ({
  workflow: { run: null as WorkflowRunItem | null, threadId: null as string | null },
  tasks: null as TasksSnapshot | null,
  // Whether the focused chat thread is mid-run. The live tab dot is gated
  // on it so a plan the provider left with an `in_progress` row can't blink
  // forever after the turn settled.
  tasksStreaming: true,
  // `titlebarOverlay` = "TitleBar renders the floating overlay, not the
  // in-flow legacy h-9 bar"; `remote` = the web remote client, which has no
  // native window controls and (therefore) no overlay drag layer either.
  titlebarOverlay: true,
  remote: false,
}));
vi.mock("@/components/workflow/use-workspace-workflow", () => ({
  useWorkspaceWorkflow: () => mocks.workflow,
}));
vi.mock("@/hooks/use-active-chat-tasks", () => ({
  useActiveChatTasks: () => ({
    threadId: "thread-1",
    tasks: mocks.tasks,
    streaming: mocks.tasksStreaming,
  }),
}));
vi.mock("@/hooks/use-gui-chrome", () => ({
  useTitlebarOverlay: () => mocks.titlebarOverlay,
}));
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => mocks.remote,
}));

import { RightPanel } from "./right-panel";

function chatPane(threadId: string | null): PaneNodeSnapshot {
  return {
    kind: "agent_chat",
    pane_id: "pane-chat",
    title: "Agent Chat",
    thread_id: threadId,
    provider: "claude",
    cwd: "/p",
  };
}

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  const root = chatPane("thread-1");
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
    tabs: [
      { tab_id: "tab-chat", kind: "terminal", title: "Agent Chat", surface_id: "surface-1", browser_id: null, icon: null },
    ],
    active_tab_id: "tab-chat",
    active_surface_id: "surface-1",
    surfaces: [{ surface_id: "surface-1", title: "s", active_pane_id: root.pane_id, root }],
    ...overrides,
  };
}

function makeRun(overrides: Partial<WorkflowRunItem> = {}): WorkflowRunItem {
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

afterEach(() => {
  cleanup();
  mocks.workflow = { run: null, threadId: null };
  mocks.tasks = null;
  mocks.tasksStreaming = true;
  mocks.titlebarOverlay = true;
  mocks.remote = false;
});

describe("RightPanel floating-chrome clearance", () => {
  it("keeps its surface full-height while dropping the desktop tab row below window controls", () => {
    render(<RightPanel workspace={makeWorkspace()} activeTab="files" />);
    const header = screen.getByTestId("right-panel-tabs-header");
    expect(header).toHaveClass("mt-10");
    expect(header).not.toHaveClass("pr-[104px]");
  });

  // Regression: the clearance was gated only on `!isRemoteClient()`, so with
  // the GUI flag off the in-flow legacy `h-9` bar already pushed the panel
  // down and this margin rendered as a 40px blank band above the tabs.
  it("adds no clearance when the title bar is the in-flow legacy bar", () => {
    mocks.titlebarOverlay = false;
    render(<RightPanel workspace={makeWorkspace()} activeTab="files" />);
    expect(screen.getByTestId("right-panel-tabs-header")).not.toHaveClass(
      "mt-10",
    );
  });

  // Safe only because the overlay also drops its full-width drag layer on
  // the web client — see the `titlebar-drag-layer` coverage in
  // `title-bar.test.tsx`. Otherwise that strip would cover the top 40px of
  // this 45px tab row and swallow every trigger's clickable center.
  it("adds no clearance on the web client, which renders no window controls", () => {
    mocks.remote = true;
    render(<RightPanel workspace={makeWorkspace()} activeTab="files" />);
    expect(screen.getByTestId("right-panel-tabs-header")).not.toHaveClass(
      "mt-10",
    );
  });
});

describe("RightPanel Tasks tab", () => {
  it("is absent until the focused chat has tasks", () => {
    render(<RightPanel workspace={makeWorkspace()} activeTab="files" />);
    expect(screen.queryByTestId("tasks-tab")).toBeNull();
  });

  it("shows progress and renders the task body for the focused chat", () => {
    mocks.tasks = {
      tasks: [
        { task_id: "1", title: "Done", status: "completed", blocked_by: [] },
        { task_id: "2", title: "Working", status: "in_progress", blocked_by: [] },
      ],
    };
    render(<RightPanel workspace={makeWorkspace()} activeTab="tasks" />);
    expect(screen.getByTestId("tasks-tab")).toHaveTextContent("1/2");
    expect(screen.getByTestId("tasks-panel")).toBeInTheDocument();
  });

  it("shows a live dot for an in-flight step only while another tab is active", () => {
    mocks.tasks = {
      tasks: [
        { task_id: "1", title: "Done", status: "completed", blocked_by: [] },
        { task_id: "2", title: "Working", status: "in_progress", blocked_by: [] },
      ],
    };
    const { rerender } = render(
      <RightPanel workspace={makeWorkspace()} activeTab="files" />,
    );
    expect(screen.getByTestId("tasks-live-dot")).toBeInTheDocument();

    rerender(<RightPanel workspace={makeWorkspace()} activeTab="tasks" />);
    expect(screen.queryByTestId("tasks-live-dot")).toBeNull();
  });

  // The snapshot is durable state that outlives the run, so a plan the
  // provider left with an `in_progress` row must not blink forever once
  // the turn settled (it would also survive a restart via hydrate-replay).
  it("stops the live dot once the thread is no longer streaming", () => {
    mocks.tasks = {
      tasks: [
        { task_id: "1", title: "Done", status: "completed", blocked_by: [] },
        { task_id: "2", title: "Working", status: "in_progress", blocked_by: [] },
      ],
    };
    mocks.tasksStreaming = false;
    render(<RightPanel workspace={makeWorkspace()} activeTab="files" />);
    expect(screen.queryByTestId("tasks-live-dot")).toBeNull();
    // The tab itself stays — the plan is still worth showing.
    expect(screen.getByTestId("tasks-tab")).toHaveTextContent("1/2");
  });
});

describe("RightPanel Orchestration tab", () => {
  it("hides the Orchestration tab when the workspace has no workflow run", () => {
    mocks.workflow = { run: null, threadId: null };
    render(<RightPanel workspace={makeWorkspace()} activeTab="files" />);
    expect(screen.queryByTestId("orchestration-tab")).toBeNull();
  });

  it("hides the Orchestration tab while the run is pending approval", () => {
    // The in-thread approval card owns the pending_approval state; the
    // panel only appears once the run is approved (design mock: the
    // approval state renders no side panel).
    mocks.workflow = {
      run: makeRun({ status: "pending_approval", approvalRequestId: "req-1" }),
      threadId: "thread-1",
    };
    render(<RightPanel workspace={makeWorkspace()} activeTab="files" />);
    expect(screen.queryByTestId("orchestration-tab")).toBeNull();
  });

  it("shows the Orchestration tab with a pulsing dot while the run is active", () => {
    mocks.workflow = { run: makeRun({ status: "running" }), threadId: "thread-1" };
    render(<RightPanel workspace={makeWorkspace()} activeTab="files" />);
    const tab = screen.getByTestId("orchestration-tab");
    expect(tab).toBeInTheDocument();
    expect(tab.querySelector(".cm-blink")).not.toBeNull();
  });

  it("shows the Orchestration tab without a pulsing dot once the run is terminal", () => {
    mocks.workflow = { run: makeRun({ status: "completed" }), threadId: "thread-1" };
    render(<RightPanel workspace={makeWorkspace()} activeTab="files" />);
    const tab = screen.getByTestId("orchestration-tab");
    expect(tab).toBeInTheDocument();
    expect(tab.querySelector(".cm-blink")).toBeNull();
  });

  it("renders the OrchestrationPanel body when the orchestration tab is active", () => {
    mocks.workflow = { run: makeRun({ status: "running" }), threadId: "thread-1" };
    render(<RightPanel workspace={makeWorkspace()} activeTab="orchestration" />);
    expect(screen.getByTestId("orchestration-panel-stub")).toBeInTheDocument();
  });
});
