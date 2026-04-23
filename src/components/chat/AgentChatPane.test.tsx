/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";

let currentMessages: unknown[] = [];
// Overridable per-test: thread -> messages map so the new race-fix
// tests can observe which slice AgentChatPane subscribes to.
let currentThreadsMap: Record<string, unknown[]> = {};
let currentDraftsById: Record<
  string,
  { draftId: string; threadId: string; promotedTo: { workspaceId: string } | null }
> = {};
let workspaceIdForPaneOverride: string | null = "ws-home";
const setShowNewWorkspaceDialogMock = vi.fn();
const setActiveDraftMock = vi.fn();

vi.mock("./ChatHomeLanding", () => ({
  ChatHomeLanding: ({ composer }: { composer: React.ReactNode }) => (
    <div data-testid="home-landing">{composer}</div>
  ),
}));

vi.mock("./ChatTranscript", () => ({
  ChatTranscript: ({ messages }: { messages: unknown[] }) => (
    <div data-testid="transcript" data-message-count={messages.length} />
  ),
}));

vi.mock("./Composer", () => ({
  Composer: ({ zone1Override }: { zone1Override?: React.ReactNode }) => (
    <div data-testid="composer">
      <div data-testid="zone1">{zone1Override}</div>
    </div>
  ),
}));

type WorktreePickerStubProps = {
  projectPath: string;
  currentWorkspaceId?: string;
  derivativeBranch: string;
  onSwitchWorkspace?: (id: string) => void;
  onWorktreeCreated: (id: string) => void;
};
let lastWorktreePickerProps: WorktreePickerStubProps | null = null;
vi.mock("./pickers/WorktreePicker", () => ({
  WorktreePicker: (props: WorktreePickerStubProps) => {
    lastWorktreePickerProps = props;
    return (
      <button
        data-testid="worktree-picker-stub"
        data-project-path={props.projectPath}
        data-current-workspace={props.currentWorkspaceId ?? ""}
        data-derivative-branch={props.derivativeBranch}
      />
    );
  },
}));

// DerivativeBranchPicker is a sibling in Zone 1; stub it out since these
// tests only exercise the AgentChatPane dispatch logic.
type DerivativeBranchPickerStubProps = {
  projectPath: string;
  value: string;
  onChange: (branch: string) => void;
};
let lastDerivativePickerProps: DerivativeBranchPickerStubProps | null = null;
vi.mock("./pickers/DerivativeBranchPicker", () => ({
  DerivativeBranchPicker: (props: DerivativeBranchPickerStubProps) => {
    lastDerivativePickerProps = props;
    return (
      <button
        data-testid="derivative-branch-picker-stub"
        data-value={props.value}
      />
    );
  },
}));

let lastProjectPickerOnChange:
  | ((path: string, name: string) => void)
  | null = null;

vi.mock("@/components/overlays/project-picker", () => ({
  ProjectPicker: (props: {
    onChange: (path: string, name: string) => void;
  }) => {
    lastProjectPickerOnChange = props.onChange;
    return <button data-testid="project-picker-stub" />;
  },
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((selector: (s: unknown) => unknown) =>
      selector({ setShowNewWorkspaceDialog: setShowNewWorkspaceDialogMock }),
    ),
    {
      getState: () => ({
        setShowNewWorkspaceDialog: setShowNewWorkspaceDialogMock,
      }),
    },
  ),
}));

vi.mock("@/hooks/use-agent-chat-events", () => ({
  useAgentChatEvents: () => {},
}));

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  agentChatInterruptTurn: vi.fn().mockResolvedValue(undefined),
  agentChatRespondToRequest: vi.fn().mockResolvedValue(undefined),
  agentChatSendTurn: vi.fn().mockResolvedValue(undefined),
  agentChatSetModel: vi.fn().mockResolvedValue(undefined),
  agentChatStartSession: vi.fn().mockResolvedValue("thread-new"),
  agentChatStopSession: vi.fn().mockResolvedValue(undefined),
}));

const HOME_APP_STATE = {
  appState: {
    active_workspace_id: "ws-home",
    workspaces: [
      {
        workspace_id: "ws-home",
        // Home detection after the Stage B rework is path-based:
        // `project_root === homeDir`. `workspace_type` is irrelevant.
        workspace_type: "standard",
        project_root: "/home/user",
        cwd: "/home/user",
      },
    ],
  },
  homeDir: "/home/user",
};
const mockAppState: { appState: unknown; homeDir: string | null } =
  { ...HOME_APP_STATE };

vi.mock("@/stores/app-store", () => ({
  useAppStore: Object.assign(
    vi.fn((selector) => selector(mockAppState)),
    { getState: () => mockAppState },
  ),
  useHomeDir: () => "/home/user",
  findWorkspaceIdForPane: () => workspaceIdForPaneOverride,
  // Pure helper used inside the Home-target ProjectPicker onChange to
  // resolve the workspace to activate. Mirrors the real
  // groupWorkspacesByProject's signature.
  groupWorkspacesByProject: (
    workspaces: Array<{ project_root: string; workspace_id: string }>,
  ) =>
    Array.from(
      workspaces
        .reduce(
          (acc, ws) => {
            const path = ws.project_root;
            if (!acc.has(path)) {
              acc.set(path, { projectPath: path, workspaces: [] });
            }
            acc.get(path)!.workspaces.push(ws);
            return acc;
          },
          new Map<
            string,
            {
              projectPath: string;
              workspaces: Array<{ workspace_id: string }>;
            }
          >(),
        )
        .values(),
    ),
}));

vi.mock("@/stores/chat-draft-store", () => ({
  useChatDraftStore: Object.assign(
    vi.fn((selector) =>
      selector({
        draftsById: currentDraftsById,
        setActiveDraft: setActiveDraftMock,
      }),
    ),
    {
      getState: () => ({
        draftsById: currentDraftsById,
        setActiveDraft: setActiveDraftMock,
      }),
    },
  ),
}));

vi.mock("@/stores/agent-chat-store", () => {
  function makeSlice(messages: unknown[]) {
    return {
      messages,
      inputDraft: "",
      streaming: false,
      activeTurnId: null,
      model: null,
      permissionMode: "bypassPermissions",
      sessionLaunchMode: "bypassPermissions",
      resumeCursor: null,
    };
  }
  function buildThreads() {
    // Merge the legacy `thread-x` (used by existing tests) with any
    // per-test `currentThreadsMap` entries so new tests can seed a
    // specific thread id like `draft-thread-42`.
    const threads: Record<string, unknown> = {
      "thread-x": makeSlice(currentMessages),
    };
    for (const [tid, msgs] of Object.entries(currentThreadsMap)) {
      threads[tid] = makeSlice(msgs);
    }
    return threads;
  }
  const mockStore = Object.assign(
    vi.fn((selector: (state: unknown) => unknown) => {
      const state = {
        threads: buildThreads(),
        ensureThread: vi.fn(),
        setInputDraft: vi.fn(),
        setModel: vi.fn(),
        setPermissionMode: vi.fn(),
        setSessionLaunchMode: vi.fn(),
        migrateThreadId: vi.fn(),
        appendUserMessage: vi.fn(),
        markRequestResponding: vi.fn(),
        applyEvent: vi.fn(),
      };
      return selector(state);
    }),
    {
      getState: () => ({
        threads: buildThreads(),
        applyEvent: vi.fn(),
      }),
    },
  );
  return {
    useAgentChatStore: mockStore,
    DEFAULT_THREAD_PERMISSION_MODE: "bypassPermissions",
  };
});

import { AgentChatPane } from "./AgentChatPane";
import { agentChatStartSession } from "@/tauri/commands";

const pane = {
  kind: "agent_chat" as const,
  pane_id: "pane-1",
  title: "Chat",
  thread_id: "thread-x",
  provider: "claude" as const,
  cwd: "/home/user",
};

describe("AgentChatPane empty-state branch", () => {
  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatStartSession).mockClear();
  });

  it("renders ChatHomeLanding when messages.length === 0", () => {
    currentMessages = [];
    const { container } = render(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="home-landing"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="transcript"]'),
    ).toBeNull();
  });

  it("renders ChatTranscript + Composer when messages.length >= 1", () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    const { container } = render(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="transcript"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="composer"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="home-landing"]'),
    ).toBeNull();
  });
});

describe("AgentChatPane Stage C race fix", () => {
  const paneNoThread = {
    kind: "agent_chat" as const,
    pane_id: "pane-new",
    title: "Chat",
    thread_id: null,
    provider: "claude" as const,
    cwd: "/home/user",
  };

  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatStartSession).mockClear();
  });

  it("adopts a promoted draft's thread_id when pane.thread_id is null, and does NOT start a fresh session", () => {
    // Simulate the race window: pane created but start_session's
    // state emit hasn't landed yet, so `pane.thread_id` is still
    // null. A promoted draft claims this workspace and carries the
    // pre-minted thread id that materialize already seeded.
    currentDraftsById = {
      "draft-1": {
        draftId: "draft-1",
        threadId: "draft-thread-42",
        promotedTo: { workspaceId: "ws-home" },
      },
    };
    currentThreadsMap = {
      "draft-thread-42": [{ kind: "user_message", id: "m1", text: "hello" }],
    };

    const { container } = render(<AgentChatPane pane={paneNoThread} />);

    // Transcript renders the seeded slice (one message), not the
    // empty default.
    const transcript = container.querySelector(
      '[data-testid="transcript"]',
    ) as HTMLElement | null;
    expect(transcript).not.toBeNull();
    expect(transcript!.getAttribute("data-message-count")).toBe("1");
    // Critical: no duplicate session started.
    expect(agentChatStartSession).not.toHaveBeenCalled();
  });

  it("starts a fresh session when pane.thread_id is null AND no promoted draft claims this workspace", () => {
    // No drafts → the pane is a true blank slate, so the existing
    // mint-and-start branch should still fire.
    currentDraftsById = {};
    render(<AgentChatPane pane={paneNoThread} />);
    expect(agentChatStartSession).toHaveBeenCalledTimes(1);
  });

  it("does not start a fresh session when pane.thread_id is already populated", () => {
    const paneWithThread = { ...paneNoThread, thread_id: "thread-x" };
    render(<AgentChatPane pane={paneWithThread} />);
    expect(agentChatStartSession).not.toHaveBeenCalled();
  });

  it("syncs local threadId state when pane.thread_id transitions from null to set after mount", () => {
    // Mount with thread_id null and no promoted draft → fresh session
    // branch fires.
    currentDraftsById = {};
    const { rerender, container } = render(
      <AgentChatPane pane={paneNoThread} />,
    );
    expect(agentChatStartSession).toHaveBeenCalledTimes(1);

    // Rerender with thread_id populated (simulating the delayed
    // state-emit from Rust landing after mount). The prop-sync effect
    // should update local state; the existing mint-branch's
    // `startAttempted` ref prevents a second call.
    currentThreadsMap = {
      "thread-late-arrival": [{ kind: "user_message", id: "m1" }],
    };
    rerender(
      <AgentChatPane pane={{ ...paneNoThread, thread_id: "thread-late-arrival" }} />,
    );
    // No additional session start.
    expect(agentChatStartSession).toHaveBeenCalledTimes(1);
    // Transcript now reflects the new slice's messages.
    const transcript = container.querySelector(
      '[data-testid="transcript"]',
    ) as HTMLElement | null;
    expect(transcript).not.toBeNull();
    expect(transcript!.getAttribute("data-message-count")).toBe("1");
  });
});

describe("AgentChatPane Stage D — Zone 1 dispatch", () => {
  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    setShowNewWorkspaceDialogMock.mockClear();
    setActiveDraftMock.mockClear();
    lastWorktreePickerProps = null;
    lastDerivativePickerProps = null;
    lastProjectPickerOnChange = null;
    Object.assign(mockAppState, HOME_APP_STATE);
  });

  it("renders ProjectPicker in Zone 1 when the workspace is home-rooted", () => {
    Object.assign(mockAppState, HOME_APP_STATE);
    workspaceIdForPaneOverride = "ws-home";
    const { container } = render(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="project-picker-stub"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="worktree-picker-stub"]'),
    ).toBeNull();
  });

  it("renders WorktreePicker in Zone 1 when the workspace is not home-rooted", () => {
    mockAppState.appState = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          workspace_type: "standard",
          project_root: "/projects/foo",
          cwd: "/projects/foo",
        },
      ],
    };
    workspaceIdForPaneOverride = "ws-foo";
    const projectPane = {
      ...pane,
      pane_id: "pane-foo",
      thread_id: "thread-x",
      cwd: "/projects/foo",
    };
    const { container } = render(<AgentChatPane pane={projectPane} />);
    const stub = container.querySelector(
      '[data-testid="worktree-picker-stub"]',
    ) as HTMLElement | null;
    expect(stub).not.toBeNull();
    expect(stub!.dataset.projectPath).toBe("/projects/foo");
    expect(stub!.dataset.currentWorkspace).toBe("ws-foo");
    expect(
      container.querySelector('[data-testid="project-picker-stub"]'),
    ).toBeNull();
  });

  it("WorktreePicker onSwitchWorkspace clears the active draft and activates the workspace", async () => {
    mockAppState.appState = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          workspace_type: "standard",
          project_root: "/projects/foo",
          cwd: "/projects/foo",
        },
      ],
    };
    workspaceIdForPaneOverride = "ws-foo";
    const projectPane = {
      ...pane,
      pane_id: "pane-foo",
      thread_id: "thread-x",
      cwd: "/projects/foo",
    };
    render(<AgentChatPane pane={projectPane} />);
    expect(lastWorktreePickerProps).not.toBeNull();
    const { activateWorkspace } = await import("@/tauri/commands");
    vi.mocked(activateWorkspace).mockClear();
    lastWorktreePickerProps!.onSwitchWorkspace?.("ws-foo-feat");
    expect(setActiveDraftMock).toHaveBeenCalledWith(null);
    expect(vi.mocked(activateWorkspace)).toHaveBeenCalledWith("ws-foo-feat");
  });

  it("WorktreePicker onWorktreeCreated spawns an agent_chat pane, clears the draft, and activates the workspace", async () => {
    mockAppState.appState = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          workspace_type: "standard",
          project_root: "/projects/foo",
          cwd: "/projects/foo",
        },
      ],
    };
    workspaceIdForPaneOverride = "ws-foo";
    const projectPane = {
      ...pane,
      pane_id: "pane-foo",
      thread_id: "thread-x",
      cwd: "/projects/foo",
    };
    render(<AgentChatPane pane={projectPane} />);
    expect(lastWorktreePickerProps).not.toBeNull();
    const { activateWorkspace, agentChatCreatePane } = await import(
      "@/tauri/commands"
    );
    vi.mocked(activateWorkspace).mockClear();
    vi.mocked(agentChatCreatePane).mockClear();
    await lastWorktreePickerProps!.onWorktreeCreated("ws-new");
    // Pane must be created BEFORE the draft clears / activate fires,
    // otherwise useEnsureDraftWhenEmpty races and injects a Home
    // draft over the empty workspace.
    expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledWith(
      "ws-new",
      "claude",
      null,
    );
    expect(setActiveDraftMock).toHaveBeenCalledWith(null);
    expect(vi.mocked(activateWorkspace)).toHaveBeenCalledWith("ws-new");
    // The legacy dialog must NOT open — inline-input flow is self-
    // contained.
    expect(setShowNewWorkspaceDialogMock).not.toHaveBeenCalled();
  });

  it("renders DerivativeBranchPicker alongside WorktreePicker for project panes, seeded to 'main'", () => {
    mockAppState.appState = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          workspace_type: "standard",
          project_root: "/projects/foo",
          cwd: "/projects/foo",
        },
      ],
    };
    workspaceIdForPaneOverride = "ws-foo";
    const projectPane = {
      ...pane,
      pane_id: "pane-foo",
      thread_id: "thread-x",
      cwd: "/projects/foo",
    };
    const { container } = render(<AgentChatPane pane={projectPane} />);
    expect(
      container.querySelector('[data-testid="derivative-branch-picker-stub"]'),
    ).not.toBeNull();
    expect(lastDerivativePickerProps).not.toBeNull();
    expect(lastDerivativePickerProps!.projectPath).toBe("/projects/foo");
    expect(lastDerivativePickerProps!.value).toBe("main");
    expect(lastWorktreePickerProps!.derivativeBranch).toBe("main");
  });

  it("Home ProjectPicker activates the first workspace of the picked project", async () => {
    // Seed app-state with both the home workspace AND another project
    // workspace so the dispatch can resolve a target.
    mockAppState.appState = {
      active_workspace_id: "ws-home",
      workspaces: [
        {
          workspace_id: "ws-home",
          workspace_type: "standard",
          project_root: "/home/user",
          cwd: "/home/user",
        },
        {
          workspace_id: "ws-bar-main",
          workspace_type: "standard",
          project_root: "/projects/bar",
          cwd: "/projects/bar",
        },
      ],
    };
    workspaceIdForPaneOverride = "ws-home";
    render(<AgentChatPane pane={pane} />);
    expect(lastProjectPickerOnChange).not.toBeNull();
    const { activateWorkspace } = await import("@/tauri/commands");
    vi.mocked(activateWorkspace).mockClear();
    lastProjectPickerOnChange!("/projects/bar", "bar");
    expect(setActiveDraftMock).toHaveBeenCalledWith(null);
    expect(vi.mocked(activateWorkspace)).toHaveBeenCalledWith("ws-bar-main");
    expect(setShowNewWorkspaceDialogMock).not.toHaveBeenCalled();
  });

  it("Home ProjectPicker opens NewWorkspaceDialog when the picked project has no workspaces yet", async () => {
    // Only the home workspace exists; picking /projects/bar has no
    // existing workspace to activate, so the dialog must open.
    Object.assign(mockAppState, HOME_APP_STATE);
    workspaceIdForPaneOverride = "ws-home";
    render(<AgentChatPane pane={pane} />);
    expect(lastProjectPickerOnChange).not.toBeNull();
    const { activateWorkspace } = await import("@/tauri/commands");
    vi.mocked(activateWorkspace).mockClear();
    lastProjectPickerOnChange!("/projects/bar", "bar");
    expect(setActiveDraftMock).toHaveBeenCalledWith(null);
    expect(vi.mocked(activateWorkspace)).not.toHaveBeenCalled();
    expect(setShowNewWorkspaceDialogMock).toHaveBeenCalledWith(
      true,
      "/projects/bar",
    );
  });

  it("renders no override when appState is null (early-boot fallback)", () => {
    mockAppState.appState = null;
    workspaceIdForPaneOverride = null;
    const { container } = render(<AgentChatPane pane={pane} />);
    // Composer mounts but neither picker is rendered.
    expect(
      container.querySelector('[data-testid="composer"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="worktree-picker-stub"]'),
    ).toBeNull();
    expect(
      container.querySelector('[data-testid="project-picker-stub"]'),
    ).toBeNull();
  });

  it("WorktreePicker projectPath comes from project_root, not cwd, when they differ", () => {
    // Worktree workspaces typically have cwd === '/projects/foo-feat-x'
    // (the worktree path) but project_root === '/projects/foo' (the
    // canonical root). The picker MUST scope by project_root, otherwise
    // sibling worktrees would never appear in the list.
    mockAppState.appState = {
      active_workspace_id: "ws-foo-feat",
      workspaces: [
        {
          workspace_id: "ws-foo-feat",
          workspace_type: "standard",
          project_root: "/projects/foo",
          cwd: "/projects/foo-feat-x",
        },
      ],
    };
    workspaceIdForPaneOverride = "ws-foo-feat";
    const featPane = {
      ...pane,
      pane_id: "pane-foo-feat",
      thread_id: "thread-x",
      cwd: "/projects/foo-feat-x",
    };
    const { container } = render(<AgentChatPane pane={featPane} />);
    const stub = container.querySelector(
      '[data-testid="worktree-picker-stub"]',
    ) as HTMLElement | null;
    expect(stub).not.toBeNull();
    expect(stub!.dataset.projectPath).toBe("/projects/foo");
    expect(stub!.dataset.currentWorkspace).toBe("ws-foo-feat");
  });

  it("falls back to active_workspace_id project_root when workspaceIdForPane is null", () => {
    // Edge case: the pane is in the tree but the recursive lookup
    // returns null (e.g. mid-update). The dispatch should still produce
    // a sensible scope by reading the active workspace.
    mockAppState.appState = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          workspace_type: "standard",
          project_root: "/projects/foo",
          cwd: "/projects/foo",
        },
      ],
    };
    workspaceIdForPaneOverride = null;
    const projectPane = {
      ...pane,
      pane_id: "pane-foo",
      thread_id: "thread-x",
      cwd: "/projects/foo",
    };
    const { container } = render(<AgentChatPane pane={projectPane} />);
    const stub = container.querySelector(
      '[data-testid="worktree-picker-stub"]',
    ) as HTMLElement | null;
    expect(stub).not.toBeNull();
    expect(stub!.dataset.projectPath).toBe("/projects/foo");
    // currentWorkspaceId is undefined since workspaceIdForPane is null.
    expect(stub!.dataset.currentWorkspace).toBe("");
  });

  it("project_root null on the workspace falls back to cwd for scope", () => {
    // Some workspaces (e.g. ad-hoc) have a null project_root. The
    // workspaceProjectRoot selector falls back to cwd in that case so
    // the picker still has a scope to filter by.
    mockAppState.appState = {
      active_workspace_id: "ws-adhoc",
      workspaces: [
        {
          workspace_id: "ws-adhoc",
          workspace_type: "standard",
          project_root: null,
          cwd: "/tmp/adhoc",
        },
      ],
    };
    workspaceIdForPaneOverride = "ws-adhoc";
    const adhocPane = {
      ...pane,
      pane_id: "pane-adhoc",
      thread_id: "thread-x",
      cwd: "/tmp/adhoc",
    };
    const { container } = render(<AgentChatPane pane={adhocPane} />);
    const stub = container.querySelector(
      '[data-testid="worktree-picker-stub"]',
    ) as HTMLElement | null;
    expect(stub).not.toBeNull();
    expect(stub!.dataset.projectPath).toBe("/tmp/adhoc");
  });
});
