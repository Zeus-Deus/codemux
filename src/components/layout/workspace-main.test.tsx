/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import type { WorkflowRunItem } from "@/lib/agent-chat/types";
import type { TasksSnapshot } from "@/tauri/events";
import type { WorkspaceSnapshot } from "@/tauri/types";

// ── Mutable mock state ──
const state = {
  workspace: null as WorkspaceSnapshot | null,
  enableAgentChat: false,
  enableLazy: false,
  activeDraftId: null as string | null,
  workflowRun: null as WorkflowRunItem | null,
  tasks: null as TasksSnapshot | null,
  rightPanelTabs: {} as Record<string, string | null>,
  rightPanelMaximized: false,
};

vi.mock("@/components/workflow/use-workspace-workflow", () => ({
  useWorkspaceWorkflow: () => ({ run: state.workflowRun, threadId: null }),
}));

vi.mock("@/hooks/use-active-chat-tasks", () => ({
  useActiveChatTasks: () => ({ threadId: null, tasks: state.tasks }),
}));

// Child rows → sentinels so we can assert which chrome rendered.
vi.mock("./tab-bar", () => ({
  TabBar: () => <div data-testid="tab-bar" />,
}));
vi.mock("./preset-bar", () => ({
  PresetBar: () => <div data-testid="preset-bar" />,
}));
vi.mock("./pane-container", () => ({
  PaneContainer: () => <div data-testid="pane-container" />,
}));
vi.mock("./right-panel", () => ({
  RightPanel: ({ activeTab }: { activeTab: string | null }) => (
    <div data-testid="right-panel" data-active-tab={activeTab ?? ""} />
  ),
}));
vi.mock("@/components/diff/DiffPane", () => ({
  DiffPane: () => <div data-testid="diff-pane" />,
}));
vi.mock("@/components/chat/DraftChatSurface", () => ({
  DraftChatSurface: () => <div data-testid="draft-surface" />,
}));
vi.mock("@/components/editor/EditorPane", () => ({
  EditorPane: () => <div data-testid="editor-pane" />,
}));
vi.mock("@/components/overlays/project-onboarding", () => ({
  ProjectOnboarding: () => <div data-testid="onboarding" />,
}));

vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => state.workspace,
  useAppStore: vi.fn((sel: (s: unknown) => unknown) =>
    sel({ appState: { workspaces: [] } }),
  ),
}));

vi.mock("@/stores/chat-draft-store", () => ({
  useChatDraftStore: vi.fn((sel: (s: unknown) => unknown) =>
    sel({
      activeDraftId: state.activeDraftId,
      draftsById: state.activeDraftId
        ? { [state.activeDraftId]: { draftId: state.activeDraftId } }
        : {},
    }),
  ),
}));

vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: vi.fn((sel: (s: unknown) => unknown) =>
    sel({
      enableAgentChat: state.enableAgentChat,
      enableLazyWorkspaceCreation: state.enableLazy,
    }),
  ),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((sel: (s: unknown) => unknown) =>
      sel({
        onboardingProjectDir: null,
        setOnboardingProjectDir: vi.fn(),
        rightPanelTabs: state.rightPanelTabs,
        rightPanelWidth: 320,
        rightPanelRowWidth: 0,
        rightPanelMaximized: state.rightPanelMaximized,
      }),
    ),
    {
      getState: () => ({
        setRightPanelWidth: vi.fn(),
        setRightPanelRowWidth: vi.fn(),
      }),
    },
  ),
}));

vi.mock("@/tauri/commands", () => ({
  dbGetUiState: vi.fn().mockResolvedValue(null),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
}));

import { WorkspaceMain } from "./workspace-main";

function makeWorkspace(
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
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
      {
        tab_id: "tab-1",
        kind: "terminal",
        title: "sh",
        surface_id: "surface-1",
        browser_id: null,
        icon: null,
      },
    ],
    active_tab_id: "tab-1",
    active_surface_id: "surface-1",
    surfaces: [
      {
        surface_id: "surface-1",
        title: "s",
        active_pane_id: "pane-1",
        root: { kind: "terminal", pane_id: "pane-1", session_id: "sess-1", title: "sh" },
      },
    ],
    ...overrides,
  };
}

beforeEach(() => {
  state.workspace = makeWorkspace();
  state.enableAgentChat = false;
  state.enableLazy = false;
  state.activeDraftId = null;
  state.workflowRun = null;
  state.tasks = null;
  state.rightPanelTabs = {};
  state.rightPanelMaximized = false;
});

afterEach(cleanup);

describe("WorkspaceMain chrome rows", () => {
  it("renders legacy TabBar + PresetBar when the Agent Chat Beta is OFF", () => {
    state.enableAgentChat = false;
    const { getByTestId } = render(<WorkspaceMain />);
    expect(getByTestId("tab-bar")).toBeInTheDocument();
    expect(getByTestId("preset-bar")).toBeInTheDocument();
    expect(getByTestId("pane-container")).toBeInTheDocument();
  });

  it("drops TabBar + PresetBar when the Agent Chat Beta is ON (GUI chrome)", () => {
    state.enableAgentChat = true;
    const { queryByTestId, getByTestId } = render(<WorkspaceMain />);
    expect(queryByTestId("tab-bar")).toBeNull();
    expect(queryByTestId("preset-bar")).toBeNull();
    // Pane content still renders — only the stacked chrome rows drop.
    expect(getByTestId("pane-container")).toBeInTheDocument();
  });

  it("reserves only local top clearance for a non-chat GUI surface", () => {
    state.enableAgentChat = true;
    const { getByTestId } = render(<WorkspaceMain />);
    expect(getByTestId("workspace-content-surface")).toHaveClass("pt-10");
  });

  it("lets a sole-root chat reclaim the top edge beneath the floating islands", () => {
    state.enableAgentChat = true;
    state.workspace = makeWorkspace({
      surfaces: [
        {
          surface_id: "surface-1",
          title: "Agent Chat",
          active_pane_id: "pane-chat",
          root: {
            kind: "agent_chat",
            pane_id: "pane-chat",
            title: "Agent Chat",
            thread_id: "thread-1",
            provider: "claude",
            cwd: "/p",
          },
        },
      ],
    });

    const { getByTestId } = render(<WorkspaceMain />);
    expect(getByTestId("workspace-content-surface")).not.toHaveClass("pt-10");
  });
});

describe("WorkspaceMain draft branch chrome", () => {
  it("renders the legacy PresetBar above the draft surface when the Beta is OFF", async () => {
    state.enableLazy = true;
    state.activeDraftId = "draft-1";
    state.enableAgentChat = false;
    const { getByTestId, findByTestId } = render(<WorkspaceMain />);
    expect(getByTestId("preset-bar")).toBeInTheDocument();
    expect(await findByTestId("draft-surface")).toBeInTheDocument();
  });

  it("drops the legacy PresetBar for a draft when the Beta is ON (the GUI draft titlebar owns preset launch)", () => {
    // Regression coverage: the draft branch used to render PresetBar
    // unconditionally, so pressing "+" in GUI mode flashed the legacy
    // preset strip until the workspace materialised.
    state.enableLazy = true;
    state.activeDraftId = "draft-1";
    state.enableAgentChat = true;
    const { queryByTestId, getByTestId } = render(<WorkspaceMain />);
    expect(queryByTestId("preset-bar")).toBeNull();
    expect(getByTestId("draft-surface")).toBeInTheDocument();
  });
});

describe("WorkspaceMain right-panel stale-tab guard", () => {
  it("coerces a persisted 'orchestration' tab to 'files' when no workflow run exists", async () => {
    state.rightPanelTabs = { "ws-1": "orchestration" };
    state.workflowRun = null;
    const { findByTestId } = render(<WorkspaceMain />);
    expect((await findByTestId("right-panel")).dataset.activeTab).toBe("files");
  });

  it("keeps 'orchestration' active when a workflow run exists", () => {
    state.rightPanelTabs = { "ws-1": "orchestration" };
    state.workflowRun = {
      kind: "workflow_run",
      id: "wf-item-1",
      seq: 0,
      workflowId: "wf-1",
      status: "running",
      name: null,
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
    };
    const { getByTestId } = render(<WorkspaceMain />);
    expect(getByTestId("right-panel").dataset.activeTab).toBe("orchestration");
  });

  it("leaves a non-orchestration persisted tab untouched", () => {
    state.rightPanelTabs = { "ws-1": "changes" };
    state.workflowRun = null;
    const { getByTestId } = render(<WorkspaceMain />);
    expect(getByTestId("right-panel").dataset.activeTab).toBe("changes");
  });

  it("coerces a persisted 'tasks' tab to 'files' when the focused pane has no tasks", () => {
    state.rightPanelTabs = { "ws-1": "tasks" };
    const { getByTestId } = render(<WorkspaceMain />);
    expect(getByTestId("right-panel").dataset.activeTab).toBe("files");
  });

  it("keeps 'tasks' active while the focused chat has a task snapshot", () => {
    state.rightPanelTabs = { "ws-1": "tasks" };
    state.tasks = {
      explanation: null,
      tasks: [
        {
          task_id: "one",
          title: "First task",
          status: "pending",
          detail: null,
          blocked_by: [],
        },
      ],
    };
    const { getByTestId } = render(<WorkspaceMain />);
    expect(getByTestId("right-panel").dataset.activeTab).toBe("tasks");
  });
});

// Full expand ("⤢" in the titlebar's fixed cluster) hands the panel the whole
// content row. The workspace column collapses to zero width rather than
// unmounting, so terminals keep their PTYs and the transcript keeps its
// scroll position — and the panel drops its inline width rather than
// overwriting it, which is what makes the restore exact and free.
describe("WorkspaceMain right-panel full expand", () => {
  it("gives the panel a fixed width beside a flexible workspace column normally", () => {
    state.enableAgentChat = true;
    state.rightPanelTabs = { "ws-1": "files" };
    const { getByTestId } = render(<WorkspaceMain />);

    expect(getByTestId("right-panel-column").style.width).toBe("320px");
    expect(getByTestId("right-panel-column")).toHaveClass("shrink-0");
    expect(getByTestId("workspace-content-column")).toHaveClass("flex-1");
    expect(getByTestId("right-panel-resizer")).toBeInTheDocument();
  });

  it("swaps to a flexible panel and a zero-width workspace column when expanded", () => {
    state.enableAgentChat = true;
    state.rightPanelTabs = { "ws-1": "files" };
    state.rightPanelMaximized = true;
    const { getByTestId, queryByTestId } = render(<WorkspaceMain />);

    const panel = getByTestId("right-panel-column");
    // No inline width at all — the stored one is left untouched, so
    // restoring returns to it exactly.
    expect(panel.style.width).toBe("");
    expect(panel).toHaveClass("flex-1");

    const workspace = getByTestId("workspace-content-column");
    expect(workspace).toHaveClass("w-0", "flex-none");
    // Still mounted: the pane tree keeps its terminals and scroll state.
    expect(workspace.querySelector("[data-testid=\'pane-container\']")).not.toBeNull();

    // Nothing left to drag the boundary against.
    expect(queryByTestId("right-panel-resizer")).toBeNull();
  });

  it("ignores a stale expand flag while the panel is collapsed", () => {
    state.enableAgentChat = true;
    state.rightPanelTabs = {};
    state.rightPanelMaximized = true;
    const { getByTestId } = render(<WorkspaceMain />);
    expect(getByTestId("workspace-content-column")).toHaveClass("flex-1");
  });
});
