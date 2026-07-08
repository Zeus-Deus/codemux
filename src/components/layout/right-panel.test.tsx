/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type { WorkflowRunItem } from "@/lib/agent-chat/types";
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
}));
vi.mock("@/components/workflow/use-workspace-workflow", () => ({
  useWorkspaceWorkflow: () => mocks.workflow,
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
});

describe("RightPanel Orchestration tab", () => {
  it("hides the Orchestration tab when the workspace has no workflow run", () => {
    mocks.workflow = { run: null, threadId: null };
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
