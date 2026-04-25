/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi, beforeEach } from "vitest";
import { cleanup, render } from "@testing-library/react";

let currentMessages: unknown[] = [];
// Overridable per-test: thread -> messages map so the new race-fix
// tests can observe which slice AgentChatPane subscribes to.
let currentThreadsMap: Record<string, unknown[]> = {};
// Per-thread slice field overrides (mode, permissionMode, etc.)
// used by the mode-pill removal tests to seed an "ask" or "plan"
// pill state before the X-button click.
type SliceOverrides = {
  mode?: "default" | "plan" | "ask" | "debug";
  modePriorPermissionMode?: string | null;
  permissionMode?: string;
  sessionLaunchMode?: string;
  model?: string | null;
  effort?: string | null;
  contextWindow?: string | null;
};
let currentSliceOverrides: Record<string, SliceOverrides> = {};
let currentDraftsById: Record<
  string,
  { draftId: string; threadId: string; promotedTo: { workspaceId: string } | null }
> = {};
let workspaceIdForPaneOverride: string | null = "ws-home";
const setShowNewWorkspaceDialogMock = vi.fn();
const setActiveDraftMock = vi.fn();
// Hoisted so the model-seed effect tests can observe whether (and
// with what) setModel was called from inside AgentChatPane's mount.
// Stable across selector calls — the agent-chat-store mock below
// reuses this exact spy in every state object it produces.
const setModelMock = vi.fn();
// Same hoisting pattern for the mode-pill removal tests so they can
// assert which slice setters were called with which values.
const setModeMock = vi.fn();
const setModePriorMock = vi.fn();
const setPermissionModeMock = vi.fn();
const markRequestResolvedMock = vi.fn();

vi.mock("./ChatHomeLanding", () => ({
  ChatHomeLanding: ({ composer }: { composer: React.ReactNode }) => (
    <div data-testid="home-landing">{composer}</div>
  ),
}));

vi.mock("./ChatTranscript", () => ({
  ChatTranscript: ({
    messages,
    onAcceptPlan,
  }: {
    messages: unknown[];
    onAcceptPlan: (requestId: string) => void;
  }) => (
    <div data-testid="transcript" data-message-count={messages.length}>
      <button
        data-testid="accept-plan"
        onClick={() => onAcceptPlan("req-1")}
      />
    </div>
  ),
}));

vi.mock("./Composer", () => ({
  Composer: ({
    zone1Override,
    onModeRemove,
    onModeActivate,
  }: {
    zone1Override?: React.ReactNode;
    onModeRemove: () => void;
    onModeActivate: (mode: "plan" | "ask" | "debug") => void;
  }) => (
    <div data-testid="composer">
      <div data-testid="zone1">{zone1Override}</div>
      <button data-testid="mode-remove" onClick={() => onModeRemove()} />
      <button
        data-testid="mode-activate-plan"
        onClick={() => onModeActivate("plan")}
      />
      <button
        data-testid="mode-activate-ask"
        onClick={() => onModeActivate("ask")}
      />
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
  agentChatSetPermissionMode: vi.fn().mockResolvedValue(undefined),
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
  function makeSlice(messages: unknown[], overrides: SliceOverrides = {}) {
    return {
      messages,
      inputDraft: "",
      streaming: false,
      activeTurnId: null,
      model: overrides.model ?? null,
      permissionMode: overrides.permissionMode ?? "bypassPermissions",
      sessionLaunchMode:
        overrides.sessionLaunchMode ?? "bypassPermissions",
      resumeCursor: null,
      mode: overrides.mode ?? "default",
      modePriorPermissionMode: overrides.modePriorPermissionMode ?? null,
      effort: overrides.effort ?? null,
      contextWindow: overrides.contextWindow ?? null,
    };
  }
  function buildThreads() {
    // Merge the legacy `thread-x` (used by existing tests) with any
    // per-test `currentThreadsMap` entries so new tests can seed a
    // specific thread id like `draft-thread-42`. Per-thread slice
    // overrides come from `currentSliceOverrides` so a test can seed
    // mode / modePriorPermissionMode without touching the messages map.
    const threads: Record<string, unknown> = {
      "thread-x": makeSlice(
        currentMessages,
        currentSliceOverrides["thread-x"],
      ),
    };
    for (const [tid, msgs] of Object.entries(currentThreadsMap)) {
      threads[tid] = makeSlice(msgs, currentSliceOverrides[tid]);
    }
    return threads;
  }
  const mockStore = Object.assign(
    vi.fn((selector: (state: unknown) => unknown) => {
      const state = {
        threads: buildThreads(),
        ensureThread: vi.fn(),
        setInputDraft: vi.fn(),
        setModel: setModelMock,
        setPermissionMode: setPermissionModeMock,
        setSessionLaunchMode: vi.fn(),
        setEffort: vi.fn(),
        setContextWindow: vi.fn(),
        setMode: setModeMock,
        setModePriorPermissionMode: setModePriorMock,
        migrateThreadId: vi.fn(),
        appendUserMessage: vi.fn(),
        markRequestResponding: vi.fn(),
        markRequestResolved: markRequestResolvedMock,
        applyEvent: vi.fn(),
      };
      return selector(state);
    }),
    {
      getState: () => ({
        threads: buildThreads(),
        applyEvent: vi.fn(),
        // prestartWorktreeSession reads these three to seed the
        // agent-chat slice after start_session resolves.
        ensureThread: vi.fn(),
        setPermissionMode: vi.fn(),
        setSessionLaunchMode: vi.fn(),
      }),
    },
  );
  return {
    useAgentChatStore: mockStore,
    DEFAULT_THREAD_PERMISSION_MODE: "bypassPermissions",
  };
});

import { AgentChatPane } from "./AgentChatPane";
import {
  agentChatSetPermissionMode,
  agentChatStartSession,
  agentChatStopSession,
} from "@/tauri/commands";

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

  it("WorktreePicker onWorktreeCreated pre-starts the session (create_pane + start_session with real cwd) before activation", async () => {
    // The new worktree workspace must be in the store so
    // prestartWorktreeSession can read its cwd. In production
    // emit_app_state fires inside create_worktree_workspace before
    // the Tauri invoke returns, so the workspace is present by the
    // time onWorktreeCreated runs.
    mockAppState.appState = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          workspace_type: "standard",
          project_root: "/projects/foo",
          cwd: "/projects/foo",
        },
        {
          workspace_id: "ws-new",
          workspace_type: "standard",
          project_root: "/projects/foo",
          cwd: "/projects/foo-feat",
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
    const { activateWorkspace, agentChatCreatePane, agentChatStartSession } =
      await import("@/tauri/commands");
    vi.mocked(activateWorkspace).mockClear();
    vi.mocked(agentChatCreatePane).mockClear();
    vi.mocked(agentChatStartSession).mockClear();
    await lastWorktreePickerProps!.onWorktreeCreated("ws-new");
    // create_pane receives the REAL workspace cwd (closes the
    // `if (!cwd) return` mount-effect guard that caused
    // session_not_found). start_session runs BEFORE activation so the
    // adapter HashMap already holds the thread_id when AgentChatPane
    // mounts.
    expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledWith(
      "ws-new",
      "claude",
      "/projects/foo-feat",
    );
    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    const [paneId, provider, input] =
      vi.mocked(agentChatStartSession).mock.calls[0];
    expect(paneId).toBe("pane-new");
    expect(provider).toBe("claude");
    expect(input.cwd).toBe("/projects/foo-feat");
    expect(input.permission_mode).toBe("bypassPermissions");
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

// ── Default-model seed effect ─────────────────────────────────────────
//
// Regression coverage for the bug where the ReasoningPicker disappeared
// after app restart / resume / any other path that left the slice with
// `model: null` under a pre-existing thread_id. The mount effect at the
// top of AgentChatPane short-circuits when threadId is set, so without
// a separate seed effect the slice's model would stay null and
// ReasoningPicker (`if (!model) return null`) would render nothing.

describe("AgentChatPane default-model seed effect", () => {
  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    setModelMock.mockClear();
    vi.mocked(agentChatStartSession).mockClear();
    mockAppState.appState = HOME_APP_STATE.appState;
  });
  afterEach(() => {
    // Without an explicit unmount the prior test's pane stays in the
    // DOM and its async session-start `.then(...)` callback fires its
    // setStoreModel call into the next test's accumulated mock.calls
    // — exactly the pollution that hid this bug initially. cleanup()
    // unmounts every render() from this test file.
    cleanup();
  });

  it("seeds slice.model with the provider default when slice.model is null on mount", () => {
    // Pane already has a thread_id (existing pane reopened, or app
    // restart hydrated the pane snapshot but the in-memory store is
    // empty). Slice exists with model: null. The mount effect that
    // starts a session short-circuits in this branch, so the new
    // seed effect must compensate.
    const existingPane = {
      ...pane,
      thread_id: "thread-x",
    };
    render(<AgentChatPane pane={existingPane} />);
    expect(setModelMock).toHaveBeenCalled();
    const [threadId, model] = setModelMock.mock.calls[0];
    expect(threadId).toBe("thread-x");
    // defaultModelForProvider("claude") falls back to "claude-opus-4-7"
    // when capabilities aren't in the test environment.
    expect(model).toBe("claude-opus-4-7");
  });

  it("does not call setModel when there is no thread_id yet", () => {
    // The "new pane" mount path takes a different branch — it starts
    // a session and the .then() handler seeds the model. This effect
    // must not fire pre-thread, otherwise we'd write to the wrong key.
    const draftlessPane = {
      ...pane,
      thread_id: null as unknown as string,
    };
    render(<AgentChatPane pane={draftlessPane} />);
    expect(setModelMock).not.toHaveBeenCalled();
  });

  it("uses codex default when the pane is on the codex provider", () => {
    // The fallback table maps each provider to its own default. A
    // pane on codex must not get seeded with the claude default.
    const codexPane = {
      ...pane,
      thread_id: "thread-x",
      provider: "codex" as const,
    };
    render(<AgentChatPane pane={codexPane} />);
    expect(setModelMock).toHaveBeenCalled();
    const [, model] = setModelMock.mock.calls[0];
    expect(model).toBe("gpt-5.4");
  });
});

// ─────────────────────────────────────────────────────────────────────
// Mode pill removal — the SDK rejects live `setPermissionMode` calls
// that would land on `bypassPermissions` even when the session was
// originally launched with `--dangerously-skip-permissions`. The
// removal handler therefore restores the prior mode via a silent
// session restart instead of the live setter. These tests guard that
// routing decision and the slice cleanup that follows.
// ─────────────────────────────────────────────────────────────────────

describe("AgentChatPane handleModeRemove silent-restart", () => {
  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    setModeMock.mockClear();
    setModePriorMock.mockClear();
    setPermissionModeMock.mockClear();
    setModelMock.mockClear();
    markRequestResolvedMock.mockClear();
    vi.mocked(agentChatStartSession).mockClear().mockResolvedValue("thread-restarted");
    vi.mocked(agentChatStopSession).mockClear().mockResolvedValue(undefined);
    vi.mocked(agentChatSetPermissionMode).mockClear().mockResolvedValue(undefined);
  });
  afterEach(() => cleanup());

  it("removing Ask pill with prior bypassPermissions restarts the session — does NOT call agentChatSetPermissionMode", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "ask",
        modePriorPermissionMode: "bypassPermissions",
        permissionMode: "plan", // session is currently in plan
        sessionLaunchMode: "plan",
      },
    };

    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    // restartSessionWith fires-and-forgets an async IIFE — yield once
    // to let the agentChatStartSession await resolve.
    await Promise.resolve();
    await Promise.resolve();

    // The bug we are guarding against: live setter would reject with
    // "Cannot set permission mode to bypassPermissions because the
    // session was not launched with --dangerously-skip-permissions".
    expect(agentChatSetPermissionMode).not.toHaveBeenCalled();
    // Silent restart re-launches with the prior mode in launch params
    // so the SDK re-applies `--dangerously-skip-permissions`.
    expect(agentChatStartSession).toHaveBeenCalled();
    const startInput = vi.mocked(agentChatStartSession).mock.calls[0][2];
    expect(startInput.permission_mode).toBe("bypassPermissions");
    // UI snap: slice.permissionMode flips immediately, slice.mode
    // returns to "default", priorPermissionMode is cleared.
    expect(setPermissionModeMock).toHaveBeenCalledWith(
      "thread-x",
      "bypassPermissions",
    );
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
    expect(setModePriorMock).toHaveBeenCalledWith("thread-x", null);
  });

  it("removing Plan pill with prior 'default' uses the same restart pattern", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "plan",
        modePriorPermissionMode: "default",
        permissionMode: "plan",
        sessionLaunchMode: "plan",
      },
    };

    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    await Promise.resolve();

    expect(agentChatSetPermissionMode).not.toHaveBeenCalled();
    expect(agentChatStartSession).toHaveBeenCalled();
    const startInput = vi.mocked(agentChatStartSession).mock.calls[0][2];
    expect(startInput.permission_mode).toBe("default");
    expect(setPermissionModeMock).toHaveBeenCalledWith("thread-x", "default");
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
  });

  it("regression guard: handleAcceptPlan still uses the live setPermissionMode (plan → default is allowed by the SDK)", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "plan",
        modePriorPermissionMode: "bypassPermissions",
        permissionMode: "plan",
        sessionLaunchMode: "plan",
      },
    };

    const { container } = render(<AgentChatPane pane={pane} />);
    const acceptBtn = container.querySelector(
      '[data-testid="accept-plan"]',
    ) as HTMLButtonElement;
    acceptBtn.click();
    await Promise.resolve();
    await Promise.resolve();

    // The accept-plan path deliberately stays on the live setter
    // because the SDK only blocks live transitions TO bypassPermissions
    // — going plan → default is allowed. Routing this through the
    // restart helper would needlessly tear down the session right
    // before the synthetic "Proceed with the plan." turn fires.
    expect(agentChatSetPermissionMode).toHaveBeenCalled();
    const [, , acceptMode] = vi.mocked(agentChatSetPermissionMode).mock.calls[0];
    expect(acceptMode).toBe("default");
    expect(agentChatStartSession).not.toHaveBeenCalled();
    expect(agentChatStopSession).not.toHaveBeenCalled();
  });
});
