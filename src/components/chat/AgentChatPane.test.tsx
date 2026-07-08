/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi, beforeEach } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

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
  hasDebugActivity?: boolean;
  debugActivityResolved?: boolean;
};
let currentSliceOverrides: Record<string, SliceOverrides> = {};
let currentDraftsById: Record<
  string,
  {
    draftId: string;
    threadId: string;
    promotedTo: { workspaceId: string; paneId: string } | null;
    materializedTo?: { workspaceId: string; paneId: string; threadId: string } | null;
  }
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
const setHasDebugActivityMock = vi.fn();
const setDebugActivityResolvedMock = vi.fn();
// Hoisted so the mount-seed effect tests (design F) can assert which
// picker fields the pane restored from the persisted session row.
const setEffortMock = vi.fn();
const setContextWindowMock = vi.fn();
const setResumeCursorMock = vi.fn();
// Hoisted mock for the bug/chat-agent-empty regression test (an
// unmount/remount that lands on an existing thread should pull the
// persisted transcript and overlay it onto the in-memory slice when
// disk has more rendered messages than memory).
const hydrateThreadMock = vi.fn();

vi.mock("./ChatHomeLanding", () => ({
  ChatHomeLanding: ({ composer }: { composer: React.ReactNode }) => (
    <div data-testid="home-landing">{composer}</div>
  ),
}));

vi.mock("./ChatTranscript", () => ({
  ChatTranscript: ({
    messages,
    sessionStartedAt,
    onAcceptPlan,
    onEnterSubagent,
  }: {
    messages: unknown[];
    sessionStartedAt?: number;
    onAcceptPlan: (requestId: string) => void;
    onEnterSubagent?: (subagentId: string) => void;
  }) => (
    <div
      data-testid="transcript"
      data-message-count={messages.length}
      // Empty string encodes "no marker timestamp" (plain divider); a
      // numeric string is the parsed session `created_at` the pane wired
      // through for the D2 marker.
      data-session-started-at={
        sessionStartedAt == null ? "" : String(sessionStartedAt)
      }
    >
      <button
        data-testid="accept-plan"
        onClick={() => onAcceptPlan("req-1")}
      />
      {/* Lets the viewMode-swap test trigger the pane's real
          onEnterSubagent handler without mounting the full card. */}
      <button
        data-testid="enter-subagent"
        onClick={() => onEnterSubagent?.("sub-1")}
      />
    </div>
  ),
}));

vi.mock("./DebugCleanupBanner", () => ({
  DebugCleanupBanner: ({
    onCleanup,
    busy,
  }: {
    onCleanup: () => void;
    busy?: boolean;
  }) => (
    <button
      data-testid="debug-cleanup-banner"
      data-busy={busy ? "true" : "false"}
      onClick={onCleanup}
    />
  ),
}));

let lastDebugExitOpen = false;
let lastDebugExitOnChoose:
  | ((choice: "cleanup" | "leave" | "cancel") => void)
  | null = null;
vi.mock("./DebugExitDialog", () => ({
  DebugExitDialog: ({
    open,
    onChoose,
  }: {
    open: boolean;
    onChoose: (choice: "cleanup" | "leave" | "cancel") => void;
  }) => {
    lastDebugExitOpen = open;
    lastDebugExitOnChoose = onChoose;
    return open ? (
      <div data-testid="debug-exit-dialog">
        <button
          data-testid="debug-exit-cleanup"
          onClick={() => onChoose("cleanup")}
        />
        <button
          data-testid="debug-exit-leave"
          onClick={() => onChoose("leave")}
        />
        <button
          data-testid="debug-exit-cancel"
          onClick={() => onChoose("cancel")}
        />
      </div>
    ) : null;
  },
}));

vi.mock("./Composer", () => ({
  Composer: ({
    zone1Override,
    onModeRemove,
    onModeActivate,
    onModelChange,
    onContextWindowChange,
  }: {
    zone1Override?: React.ReactNode;
    onModeRemove: () => void;
    onModeActivate: (mode: "plan" | "ask" | "debug") => void;
    onModelChange: (model: string) => void;
    onContextWindowChange: (contextWindow: string) => void;
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
      <button
        data-testid="mode-activate-debug"
        onClick={() => onModeActivate("debug")}
      />
      {/* Design G persist coverage: exercise the pane's real picker
          handlers so the test can assert the fire-and-forget
          `agentChatUpdateSessionConfig` write. */}
      <button
        data-testid="model-change"
        onClick={() => onModelChange("claude-sonnet-4-6")}
      />
      <button
        data-testid="context-window-change"
        onClick={() => onContextWindowChange("1m")}
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
  // The pane's mount-seed effect (design F) fetches the persisted
  // session row to restore picker config + resume cursor. Default to
  // `null` (no persisted row) so tests that don't seed a record fall
  // back to the provider default; the seed tests override per-case.
  agentChatGetSession: vi.fn().mockResolvedValue(null),
  agentChatInterruptTurn: vi.fn().mockResolvedValue(undefined),
  // The pane's mount-time hydrate effect calls this whenever it lands
  // with a truthy threadId — return an empty transcript so the effect
  // short-circuits without exercising the resume path in unit tests.
  agentChatListMessages: vi.fn().mockResolvedValue([]),
  // The D2 session-start marker effect looks the active thread up in the
  // persisted sessions list; default to empty so it clears to the plain
  // divider unless a test seeds a record.
  agentChatListSessions: vi.fn().mockResolvedValue([]),
  agentChatRespondToRequest: vi.fn().mockResolvedValue(undefined),
  agentChatSendTurn: vi.fn().mockResolvedValue(undefined),
  agentChatSetModel: vi.fn().mockResolvedValue(undefined),
  agentChatSetPermissionMode: vi.fn().mockResolvedValue(undefined),
  agentChatStartSession: vi.fn().mockResolvedValue("thread-new"),
  agentChatStopSession: vi.fn().mockResolvedValue(undefined),
  // Picker handlers (design G) mirror every change into this DB-only
  // persist command, fire-and-forget. Default no-op.
  agentChatUpdateSessionConfig: vi.fn().mockResolvedValue(undefined),
  grepCountPattern: vi.fn().mockResolvedValue(0),
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
      hasDebugActivity: overrides.hasDebugActivity ?? false,
      debugActivityResolved: overrides.debugActivityResolved ?? false,
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
        setEffort: setEffortMock,
        setContextWindow: setContextWindowMock,
        setResumeCursor: setResumeCursorMock,
        setMode: setModeMock,
        setModePriorPermissionMode: setModePriorMock,
        setHasDebugActivity: setHasDebugActivityMock,
        setDebugActivityResolved: setDebugActivityResolvedMock,
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
        // Exposed on getState so the mount-time hydrate effect can
        // imperatively replace the slice when disk leads memory.
        hydrateThread: hydrateThreadMock,
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
  agentChatGetSession,
  agentChatListMessages,
  agentChatListSessions,
  agentChatSendTurn,
  agentChatSetPermissionMode,
  agentChatStartSession,
  agentChatStopSession,
  agentChatUpdateSessionConfig,
  grepCountPattern,
  type AgentChatSessionRecord,
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

describe("AgentChatPane subagent drill-in (viewMode swap)", () => {
  const subagentMessage = {
    kind: "subagent_run",
    id: "run-1",
    seq: 1,
    turn_id: "turn-1",
    subagents: [
      {
        id: "sub-1",
        name: "Explore",
        model: "opus · xhigh",
        status: "running",
        items: [],
        toneIndex: 0,
      },
    ],
  };

  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1", seq: 0 }, subagentMessage];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatListMessages).mockReset();
    vi.mocked(agentChatListMessages).mockResolvedValue([]);
  });

  it("swaps the transcript for the breadcrumb + read-only drill-in on enter, and Esc returns", () => {
    const { container } = render(<AgentChatPane pane={pane} />);

    // Orchestrator mode: the (mocked) transcript is shown, no breadcrumb.
    expect(container.querySelector('[data-testid="transcript"]')).not.toBeNull();

    // Enter the subagent via the real onEnterSubagent handler.
    fireEvent.click(container.querySelector('[data-testid="enter-subagent"]')!);

    // Drill-in mode: transcript gone, breadcrumb + read-only banner shown.
    expect(container.querySelector('[data-testid="transcript"]')).toBeNull();
    expect(container.textContent).toContain("Orchestrator");
    expect(container.textContent).toContain("Read-only view of the");
    // The drill-in shows the subagent name in both the banner and breadcrumb.
    expect(container.textContent).toContain("Explore");

    // Esc returns to the orchestrator transcript.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(container.querySelector('[data-testid="transcript"]')).not.toBeNull();
  });

  it("keeps the parent-bound composer mounted while entered", () => {
    const { container } = render(<AgentChatPane pane={pane} />);
    expect(container.querySelector('[data-testid="composer"]')).not.toBeNull();
    fireEvent.click(container.querySelector('[data-testid="enter-subagent"]')!);
    // Drill-in swaps the transcript body but the composer stays parent-bound
    // (design: "steering goes to the orchestrator").
    expect(container.querySelector('[data-testid="transcript"]')).toBeNull();
    expect(container.querySelector('[data-testid="composer"]')).not.toBeNull();
  });
});

describe("AgentChatPane hydrate-on-mount (workspace swap recovery)", () => {
  // Regression test for the bug where: the user submits a turn, switches
  // workspaces (the inactive workspace's pane tree unmounts entirely per
  // workspace-main.tsx), the live event broadcaster drops events with
  // no listener attached, and the user comes back to a chat that shows
  // only the optimistic user message — even though the agent ran (file
  // changes prove it) and the backend persisted the full transcript to
  // SQLite. The fix pulls `agent_chat_list_messages` on every mount that
  // lands with a truthy thread id and overlays disk onto memory whenever
  // disk has more rendered messages.
  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatListMessages).mockReset();
    vi.mocked(agentChatListMessages).mockResolvedValue([]);
    hydrateThreadMock.mockClear();
  });

  it("reads persisted messages on mount when the pane has an existing thread id", async () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListMessages)).toHaveBeenCalledWith("thread-x");
    });
  });

  it("hydrates the slice when disk has strictly more rendered messages than memory", async () => {
    // In-memory slice carries only the optimistic user message — the
    // exact snapshot the user sees after returning from another workspace.
    currentMessages = [{ kind: "user_message", id: "m1" }];
    const userPayload = JSON.stringify({
      type: "user_message",
      thread_id: "thread-x",
      text: "hello",
    });
    // Persisted assistant reply, shaped per the reducer's
    // `item_completed` / `assistant_text` arms (see reducer.ts:254).
    // The crucial bit for this test is that this payload, when
    // replayed, ADDS a message to the rebuilt state — driving disk
    // count strictly above the in-memory count.
    const assistantPayload = JSON.stringify({
      type: "item_completed",
      thread_id: "thread-x",
      turn_id: "turn-1",
      item: { kind: "assistant_text", text: "hi back" },
    });
    vi.mocked(agentChatListMessages).mockResolvedValue([
      userPayload,
      assistantPayload,
    ]);
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(hydrateThreadMock).toHaveBeenCalledWith("thread-x", [
        userPayload,
        assistantPayload,
      ]);
    });
  });

  it("does NOT hydrate when memory already has at least as many messages as disk", async () => {
    // Steady-state path: the live stream kept up, so memory is at least
    // as fresh as disk. Hydrating would clobber events that are queued
    // for persistence but not yet on disk.
    currentMessages = [
      { kind: "user_message", id: "m1" },
      { kind: "assistant_message", id: "m2", turn_id: "t-1" },
    ];
    const userPayload = JSON.stringify({
      type: "user_message",
      thread_id: "thread-x",
      text: "hello",
    });
    vi.mocked(agentChatListMessages).mockResolvedValue([userPayload]);
    render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListMessages)).toHaveBeenCalled();
    });
    expect(hydrateThreadMock).not.toHaveBeenCalled();
  });

  it("swallows hydrate failures so a flaky list_messages call cannot blank the pane", async () => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    vi.mocked(agentChatListMessages).mockRejectedValue(
      new Error("simulated SQLite read failure"),
    );
    expect(() => render(<AgentChatPane pane={pane} />)).not.toThrow();
    await waitFor(() => {
      expect(vi.mocked(agentChatListMessages)).toHaveBeenCalled();
    });
    expect(hydrateThreadMock).not.toHaveBeenCalled();
  });
});

describe("AgentChatPane session-start marker wiring (D2)", () => {
  // The pane resolves the active thread's `created_at` from the persisted
  // sessions list and hands it to ChatTranscript as `sessionStartedAt`;
  // the transcript stub echoes it into `data-session-started-at`.
  function makeSessionRecord(overrides: {
    thread_id: string;
    created_at: string;
  }) {
    return {
      thread_id: overrides.thread_id,
      sdk_session_id: "sdk-1",
      workspace_id: "ws-home",
      cwd: "/home/user",
      provider: "claude",
      title: "Chat",
      created_at: overrides.created_at,
      last_active_at: overrides.created_at,
      model: null,
      effort: null,
      context_window: null,
      permission_mode: null,
    };
  }

  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    workspaceIdForPaneOverride = "ws-home";
    // Reset the mount-time hydrate mock a sibling block left rejecting,
    // so its caught-and-logged failure doesn't spam this block's stderr.
    vi.mocked(agentChatListMessages).mockReset();
    vi.mocked(agentChatListMessages).mockResolvedValue([]);
    vi.mocked(agentChatListSessions).mockReset();
    vi.mocked(agentChatListSessions).mockResolvedValue([]);
  });

  it("passes the matching session's parsed created_at through to the transcript", async () => {
    const createdAt = "2026-07-03T14:52:00Z";
    vi.mocked(agentChatListSessions).mockResolvedValue([
      makeSessionRecord({ thread_id: "thread-x", created_at: createdAt }),
    ]);
    const { container } = render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListSessions)).toHaveBeenCalledWith(
        "ws-home",
        "/home/user",
      );
    });
    await waitFor(() => {
      const transcript = container.querySelector('[data-testid="transcript"]');
      expect(transcript?.getAttribute("data-session-started-at")).toBe(
        String(Date.parse(createdAt)),
      );
    });
  });

  it("falls back to the plain divider (no timestamp) when no record matches the thread", async () => {
    vi.mocked(agentChatListSessions).mockResolvedValue([
      makeSessionRecord({
        thread_id: "some-other-thread",
        created_at: "2026-07-03T14:52:00Z",
      }),
    ]);
    const { container } = render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListSessions)).toHaveBeenCalled();
    });
    // No await-able change to observe, so assert the steady state: the
    // stub kept its empty marker attribute.
    const transcript = container.querySelector('[data-testid="transcript"]');
    expect(transcript?.getAttribute("data-session-started-at")).toBe("");
  });

  it("ignores an unparseable created_at and renders the plain divider", async () => {
    vi.mocked(agentChatListSessions).mockResolvedValue([
      makeSessionRecord({ thread_id: "thread-x", created_at: "not-a-date" }),
    ]);
    const { container } = render(<AgentChatPane pane={pane} />);
    await waitFor(() => {
      expect(vi.mocked(agentChatListSessions)).toHaveBeenCalled();
    });
    const transcript = container.querySelector('[data-testid="transcript"]');
    expect(transcript?.getAttribute("data-session-started-at")).toBe("");
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
        // paneId must match `paneNoThread.pane_id` — the selector is
        // pane-scoped (not just workspace-scoped) so a fresh second
        // pane in the same workspace doesn't accidentally adopt the
        // first pane's draft.
        promotedTo: { workspaceId: "ws-home", paneId: "pane-new" },
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

  it("hides the Zone 1 scope pills once the conversation has messages", () => {
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
    currentMessages = [{ kind: "user_message", id: "m1" }];
    const projectPane = {
      ...pane,
      pane_id: "pane-foo",
      thread_id: "thread-x",
      cwd: "/projects/foo",
    };
    const { container } = render(<AgentChatPane pane={projectPane} />);
    expect(
      container.querySelector('[data-testid="worktree-picker-stub"]'),
    ).toBeNull();
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

describe("AgentChatPane mount-seed effect (design F)", () => {
  function makeRecord(
    overrides: Partial<AgentChatSessionRecord> = {},
  ): AgentChatSessionRecord {
    return {
      thread_id: "thread-x",
      sdk_session_id: null,
      workspace_id: "ws-home",
      cwd: "/home/user",
      provider: "claude",
      title: "Chat",
      created_at: "2026-07-03T14:52:00Z",
      last_active_at: "2026-07-03T14:52:00Z",
      model: null,
      effort: null,
      context_window: null,
      permission_mode: null,
      ...overrides,
    };
  }

  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    setModelMock.mockClear();
    setEffortMock.mockClear();
    setContextWindowMock.mockClear();
    setResumeCursorMock.mockClear();
    setPermissionModeMock.mockClear();
    vi.mocked(agentChatStartSession).mockClear();
    vi.mocked(agentChatGetSession).mockReset().mockResolvedValue(null);
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

  it("falls back to the provider default when no persisted row exists", async () => {
    // Pane already has a thread_id (existing pane reopened, or app
    // restart hydrated the pane snapshot but the in-memory store is
    // empty). Slice exists with model: null. The mount effect that
    // starts a session short-circuits in this branch, so the seed
    // effect must compensate. `agentChatGetSession` resolves null →
    // provider default.
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() => expect(setModelMock).toHaveBeenCalled());
    expect(agentChatGetSession).toHaveBeenCalledWith("thread-x");
    const [threadId, model] = setModelMock.mock.calls[0];
    expect(threadId).toBe("thread-x");
    // defaultModelForProvider("claude") falls back to "claude-opus-4-8"
    // when capabilities aren't in the test environment.
    expect(model).toBe("claude-opus-4-8");
  });

  it("seeds the persisted model over the Opus default", async () => {
    // Bug #2: after restart the picker must show the user's chosen
    // model, not the Opus default. The persisted row wins.
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ model: "claude-sonnet-4-6" }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() => expect(setModelMock).toHaveBeenCalled());
    const [, model] = setModelMock.mock.calls[0];
    expect(model).toBe("claude-sonnet-4-6");
  });

  it("seeds effort / contextWindow / permissionMode from the record", async () => {
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({
        model: "claude-sonnet-4-6",
        effort: "high",
        context_window: "1m",
        permission_mode: "plan",
      }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() =>
      expect(setEffortMock).toHaveBeenCalledWith("thread-x", "high"),
    );
    expect(setContextWindowMock).toHaveBeenCalledWith("thread-x", "1m");
    expect(setPermissionModeMock).toHaveBeenCalledWith("thread-x", "plan");
  });

  it("does not clobber a permission-mode change the user made during the in-flight fetch", async () => {
    // Finding 2: the post-fetch re-check must guard EACH field, not just
    // `model`. Here the slice's model is still null (so the effect
    // proceeds and seeds the model), but the user changed permission mode
    // to "acceptEdits" while `agent_chat_get_session` was in flight. The
    // stale persisted "plan" must NOT be written back over it.
    currentSliceOverrides = { "thread-x": { permissionMode: "acceptEdits" } };
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ model: "claude-sonnet-4-6", permission_mode: "plan" }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    // The model still seeds (the user didn't touch it).
    await waitFor(() =>
      expect(setModelMock).toHaveBeenCalledWith(
        "thread-x",
        "claude-sonnet-4-6",
      ),
    );
    // ...but the user's in-flight permission-mode pick is preserved.
    expect(setPermissionModeMock).not.toHaveBeenCalledWith("thread-x", "plan");
  });

  it("restores resumeCursor from the persisted sdk_session_id", async () => {
    // Bug #1 support: a picker-triggered silent restart must resume the
    // durable SDK session rather than start fresh, so the mount seed
    // rebuilds { resume: sdk_session_id } into the slice.
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ model: "claude-sonnet-4-6", sdk_session_id: "sdk-abc" }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() => expect(setResumeCursorMock).toHaveBeenCalled());
    expect(setResumeCursorMock).toHaveBeenCalledWith("thread-x", {
      resume: "sdk-abc",
    });
  });

  it("seeds resumeCursor null when the row has no sdk_session_id", async () => {
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ model: "claude-sonnet-4-6", sdk_session_id: null }),
    );
    const existingPane = { ...pane, thread_id: "thread-x" };
    render(<AgentChatPane pane={existingPane} />);
    await waitFor(() => expect(setResumeCursorMock).toHaveBeenCalled());
    expect(setResumeCursorMock).toHaveBeenCalledWith("thread-x", null);
  });

  it("does not fetch the session row when there is no thread_id yet", () => {
    // The "new pane" mount path takes a different branch — it starts
    // a session and the .then() handler seeds the model. The seed
    // effect must not fire its fetch pre-thread, otherwise we'd read
    // (and later write) the wrong key. Assert synchronously: the seed
    // effect calls `agentChatGetSession` before its first await, so a
    // guard failure surfaces without yielding — and yielding here would
    // race the null-thread start path, which legitimately seeds a model
    // once its own session resolves.
    const draftlessPane = {
      ...pane,
      thread_id: null as unknown as string,
    };
    render(<AgentChatPane pane={draftlessPane} />);
    expect(agentChatGetSession).not.toHaveBeenCalled();
  });

  it("uses codex default when the pane is on the codex provider", async () => {
    // The fallback table maps each provider to its own default. A
    // pane on codex must not get seeded with the claude default.
    const codexPane = {
      ...pane,
      thread_id: "thread-x",
      provider: "codex" as const,
    };
    render(<AgentChatPane pane={codexPane} />);
    await waitFor(() => expect(setModelMock).toHaveBeenCalled());
    const [, model] = setModelMock.mock.calls[0];
    expect(model).toBe("gpt-5.4");
  });
});

describe("AgentChatPane picker-config persistence (design G)", () => {
  beforeEach(() => {
    currentMessages = [];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    vi.mocked(agentChatGetSession).mockReset().mockResolvedValue(null);
    vi.mocked(agentChatUpdateSessionConfig).mockClear();
    mockAppState.appState = HOME_APP_STATE.appState;
  });
  afterEach(() => cleanup());

  it("persists the model on a picker change (fire-and-forget)", async () => {
    const existingPane = { ...pane, thread_id: "thread-x" };
    const { container } = render(<AgentChatPane pane={existingPane} />);
    const btn = container.querySelector(
      '[data-testid="model-change"]',
    ) as HTMLButtonElement;
    btn.click();
    await waitFor(() =>
      expect(agentChatUpdateSessionConfig).toHaveBeenCalled(),
    );
    const [threadId, config] = vi.mocked(agentChatUpdateSessionConfig).mock
      .calls[0];
    expect(threadId).toBe("thread-x");
    expect(config).toMatchObject({ model: "claude-sonnet-4-6" });
  });

  it("persists the context window on a picker change", async () => {
    const existingPane = { ...pane, thread_id: "thread-x" };
    const { container } = render(<AgentChatPane pane={existingPane} />);
    const btn = container.querySelector(
      '[data-testid="context-window-change"]',
    ) as HTMLButtonElement;
    btn.click();
    await waitFor(() =>
      expect(agentChatUpdateSessionConfig).toHaveBeenCalledWith("thread-x", {
        context_window: "1m",
      }),
    );
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

  it("activating Debug pill flips slice.mode without an SDK setPermissionMode call", async () => {
    const { container } = render(<AgentChatPane pane={pane} />);
    const activateBtn = container.querySelector(
      '[data-testid="mode-activate-debug"]',
    ) as HTMLButtonElement;
    activateBtn.click();
    await Promise.resolve();
    await Promise.resolve();

    expect(agentChatSetPermissionMode).not.toHaveBeenCalled();
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "debug");
    expect(setPermissionModeMock).not.toHaveBeenCalledWith(
      "thread-x",
      "plan",
    );
  });

  it("activating the Plan pill re-persists the durable permission mode so a restart isn't left plan-locked (finding 6)", async () => {
    // The pill flips the LIVE session to read-only "plan" (in-memory
    // only), but `agent_chat_set_permission_mode` just wrote "plan" to
    // the DB row. Without the re-persist, a restart auto-resumes a
    // read-only "plan" session with the pill gone and the prior mode
    // unrecoverable. The handler must write the user's DURABLE mode back
    // to the DB.
    vi.mocked(agentChatUpdateSessionConfig).mockClear().mockResolvedValue(
      undefined,
    );
    currentSliceOverrides = { "thread-x": { permissionMode: "acceptEdits" } };
    const { container } = render(<AgentChatPane pane={pane} />);
    const activateBtn = container.querySelector(
      '[data-testid="mode-activate-plan"]',
    ) as HTMLButtonElement;
    activateBtn.click();

    await waitFor(() =>
      expect(agentChatSetPermissionMode).toHaveBeenCalledWith(
        "claude",
        "thread-x",
        "plan",
      ),
    );
    await waitFor(() =>
      expect(agentChatUpdateSessionConfig).toHaveBeenCalledWith("thread-x", {
        permission_mode: "acceptEdits",
      }),
    );
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

// ─────────────────────────────────────────────────────────────────────
// Stage 6 — Debug-mode cleanup affordances. These tests cover the
// grep-on-mount hook that seeds hasDebugActivity, the exit-dialog
// branch on pill-removal, the cleanup turn synthesis, and the banner
// visibility gate.
// ─────────────────────────────────────────────────────────────────────

describe("AgentChatPane Stage 6 — Debug-mode cleanup", () => {
  beforeEach(() => {
    currentMessages = [{ kind: "user_message", id: "m1" }];
    currentThreadsMap = {};
    currentDraftsById = {};
    currentSliceOverrides = {};
    workspaceIdForPaneOverride = "ws-home";
    setModeMock.mockClear();
    setHasDebugActivityMock.mockClear();
    setDebugActivityResolvedMock.mockClear();
    lastDebugExitOpen = false;
    lastDebugExitOnChoose = null;
    vi.mocked(grepCountPattern).mockClear().mockResolvedValue(0);
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue({ turn_id: "turn-1", queued_id: null });
  });
  afterEach(() => cleanup());

  it("grep-on-mount fires with the workspace project root and CODEMUX_DEBUG pattern", async () => {
    render(<AgentChatPane pane={pane} />);
    await Promise.resolve();
    expect(grepCountPattern).toHaveBeenCalledWith(
      "/home/user",
      "CODEMUX_DEBUG",
    );
  });

  it("grep-on-mount with hits flips hasDebugActivity true and marks resolved", async () => {
    vi.mocked(grepCountPattern).mockResolvedValue(3);
    render(<AgentChatPane pane={pane} />);
    await Promise.resolve();
    await Promise.resolve();
    expect(setHasDebugActivityMock).toHaveBeenCalledWith("thread-x", true);
    expect(setDebugActivityResolvedMock).toHaveBeenCalledWith(
      "thread-x",
      true,
    );
  });

  it("grep-on-mount failure soft-fails: hasDebugActivity stays false, resolved still flips", async () => {
    vi.mocked(grepCountPattern).mockRejectedValue(new Error("rg missing"));
    render(<AgentChatPane pane={pane} />);
    await Promise.resolve();
    await Promise.resolve();
    expect(setHasDebugActivityMock).toHaveBeenCalledWith("thread-x", false);
    expect(setDebugActivityResolvedMock).toHaveBeenCalledWith(
      "thread-x",
      true,
    );
  });

  it("Debug-pill removal with no detected markers skips the dialog and just flips slice.mode", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: false,
        debugActivityResolved: true,
      },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    await Promise.resolve();

    expect(lastDebugExitOpen).toBe(false);
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
  });

  it("Debug-pill removal with markers opens the exit dialog and pauses on Cancel", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    expect(lastDebugExitOpen).toBe(true);
    // Cancel keeps the pill: setMode("default") never fires for this thread.
    setModeMock.mockClear();
    lastDebugExitOnChoose?.("cancel");
    await Promise.resolve();
    await Promise.resolve();
    expect(setModeMock).not.toHaveBeenCalledWith("thread-x", "default");
  });

  it("Debug-pill removal — Leave them path drops the pill without firing a cleanup turn", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    setModeMock.mockClear();
    lastDebugExitOnChoose?.("leave");
    await Promise.resolve();
    await Promise.resolve();

    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
    expect(agentChatSendTurn).not.toHaveBeenCalled();
  });

  it("Debug-pill removal — Remove markers path fires the cleanup turn and clears hasDebugActivity", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    lastDebugExitOnChoose?.("cleanup");
    // Two awaits for setMode + send-turn promise + finally.
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    // Cleanup flips slice.mode to default first so the wrapper isn't
    // re-applied to the cleanup prompt itself, then sends the turn.
    expect(setModeMock).toHaveBeenCalledWith("thread-x", "default");
    expect(agentChatSendTurn).toHaveBeenCalled();
    const [, sendInput] = vi.mocked(agentChatSendTurn).mock.calls[0];
    expect(sendInput.text).toContain("CODEMUX_DEBUG");
    expect(setHasDebugActivityMock).toHaveBeenCalledWith("thread-x", false);
  });

  it("DebugCleanupBanner only renders when mode=debug AND hasDebugActivity AND debugActivityResolved", async () => {
    // Combo 1: all three true → banner visible.
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const { container, rerender } = render(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="debug-cleanup-banner"]'),
    ).not.toBeNull();

    // Combo 2: resolution still pending → banner hidden.
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: false,
      },
    };
    rerender(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="debug-cleanup-banner"]'),
    ).toBeNull();

    // Combo 3: mode=debug, resolved=true, but hasDebugActivity=false
    // (no markers in workspace) → banner hidden.
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: false,
        debugActivityResolved: true,
      },
    };
    rerender(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="debug-cleanup-banner"]'),
    ).toBeNull();

    // Combo 4: mode=default with markers → still hidden (banner is
    // strictly debug-mode-only — it's an exit affordance, not a global
    // janitor).
    currentSliceOverrides = {
      "thread-x": {
        mode: "default",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    rerender(<AgentChatPane pane={pane} />);
    expect(
      container.querySelector('[data-testid="debug-cleanup-banner"]'),
    ).toBeNull();
  });

  it("triggerDebugCleanup is single-fire — clicking the banner twice in the same tick fires only one cleanup turn", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    // Slow-walk the send-turn promise so the second click definitely
    // lands while the first is still in flight.
    const sendDeferred: {
      resolve: ((v: { turn_id: string; queued_id: string | null }) => void) | null;
    } = {
      resolve: null,
    };
    vi.mocked(agentChatSendTurn).mockImplementation(
      () =>
        new Promise<{ turn_id: string; queued_id: string | null }>((resolve) => {
          sendDeferred.resolve = resolve;
        }),
    );
    const { container } = render(<AgentChatPane pane={pane} />);
    const banner = container.querySelector(
      '[data-testid="debug-cleanup-banner"]',
    ) as HTMLButtonElement;
    expect(banner).not.toBeNull();
    // Two synchronous clicks — same tick. The ref guard must reject
    // the second.
    banner.click();
    banner.click();
    expect(agentChatSendTurn).toHaveBeenCalledTimes(1);
    sendDeferred.resolve?.({ turn_id: "turn-done", queued_id: null });
  });

  it("cleanup ordering: setMode('default') is called BEFORE agentChatSendTurn so the wrapper does not re-instruct", async () => {
    currentSliceOverrides = {
      "thread-x": {
        mode: "debug",
        hasDebugActivity: true,
        debugActivityResolved: true,
      },
    };
    const callOrder: string[] = [];
    setModeMock.mockImplementation((_tid: string, mode: string) => {
      if (mode === "default") callOrder.push("setMode(default)");
    });
    vi.mocked(agentChatSendTurn).mockImplementation(async () => {
      callOrder.push("agentChatSendTurn");
      return { turn_id: "turn-1", queued_id: null };
    });

    const { container } = render(<AgentChatPane pane={pane} />);
    const removeBtn = container.querySelector(
      '[data-testid="mode-remove"]',
    ) as HTMLButtonElement;
    removeBtn.click();
    await Promise.resolve();
    lastDebugExitOnChoose?.("cleanup");
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    const setModeIdx = callOrder.indexOf("setMode(default)");
    const sendTurnIdx = callOrder.indexOf("agentChatSendTurn");
    expect(setModeIdx).toBeGreaterThanOrEqual(0);
    expect(sendTurnIdx).toBeGreaterThanOrEqual(0);
    expect(setModeIdx).toBeLessThan(sendTurnIdx);
  });

  it("grep cancellation: the cancelled-flag cleanup prevents stale slice writes after unmount", async () => {
    // Hold the grep promise so we can unmount BEFORE it resolves.
    const grepDeferred: { resolve: ((v: number) => void) | null } = {
      resolve: null,
    };
    vi.mocked(grepCountPattern).mockImplementation(
      () =>
        new Promise<number>((resolve) => {
          grepDeferred.resolve = resolve;
        }),
    );
    const { unmount } = render(<AgentChatPane pane={pane} />);
    // Effect ran and called setDebugActivityResolved(threadId, false)
    // synchronously to mark "in flight". Clear the spy so we only
    // observe writes that happen AFTER unmount.
    setHasDebugActivityMock.mockClear();
    setDebugActivityResolvedMock.mockClear();
    unmount();
    // Now resolve the grep — the cleanup ran, `cancelled` is true,
    // and neither setter should fire.
    grepDeferred.resolve?.(7);
    await Promise.resolve();
    await Promise.resolve();
    expect(setHasDebugActivityMock).not.toHaveBeenCalled();
    expect(setDebugActivityResolvedMock).not.toHaveBeenCalled();
  });
});
