import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  AgentChatProviderKind,
  AppStateSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

// ── Module mocks ──
//
// Every command the draft resume path can reach. `agent_chat_start_session`
// echoes the thread id it was given, like the real backend does for an
// adopted row the adopt command already minted.

vi.mock("@/tauri/commands", () => ({
  activatePane: vi.fn().mockResolvedValue(undefined),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  agentChatAdoptExternalSession: vi.fn(),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  agentChatGetSession: vi.fn().mockResolvedValue(null),
  agentChatListMessagesAfter: vi.fn().mockResolvedValue([]),
  agentChatSendTurn: vi.fn(),
  agentChatStartSession: vi.fn(
    (
      _paneId: string,
      _provider: AgentChatProviderKind,
      input: { thread_id: string },
    ) => Promise.resolve(input.thread_id),
  ),
  createEmptyWorkspaceResult: vi.fn(),
  createWorktreeWorkspaceResult: vi.fn(),
  createEmptyWorkspace: vi.fn(),
  createWorktreeWorkspace: vi.fn(),
  generateBranchName: vi.fn(),
  generateRandomBranchName: vi.fn(),
  getHomeDir: vi.fn().mockResolvedValue("/home/user"),
  importWorktreeWorkspace: vi.fn(),
  listWorktrees: vi.fn(),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  applyPreset: vi.fn(),
  discardStagedChatImage: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

import { adoptedSessionLastActiveAt } from "./adopt-external-session";
import {
  findWorkspaceAtDirectory,
  resumeExternalSessionFromDraft,
} from "./draft-resume";
import { toast } from "@/lib/toast";
import {
  activateWorkspace,
  agentChatAdoptExternalSession,
  agentChatCreatePane,
  agentChatGetSession,
  agentChatListMessagesAfter,
  agentChatSendTurn,
  agentChatStartSession,
  createEmptyWorkspace,
  createEmptyWorkspaceResult,
  createWorktreeWorkspace,
  createWorktreeWorkspaceResult,
  importWorktreeWorkspace,
  listWorktrees,
  type AdoptableAgentSession,
  type AdoptExternalSessionResult,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import {
  useChatDraftStore,
  type ChatDraft,
} from "@/stores/chat-draft-store";

function makeSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return {
    session_id: "sdk-ext-1",
    title: "Terminal conversation",
    cwd: "/projects/foo",
    git_branch: "main",
    last_modified: "2026-08-24T12:00:00.000Z",
    created_at: "2026-08-24T10:00:00.000Z",
    file_size: 8192,
    title_source: "summary",
    existing_thread_id: null,
    same_repo: true,
    project_root: "/projects/foo",
    worktree_name: null,
    ...overrides,
  };
}

function makeResult(
  overrides: Partial<AdoptExternalSessionResult> = {},
): AdoptExternalSessionResult {
  return {
    thread_id: "chat-adopted-1",
    workspace_id: "ws-new",
    pane_id: "pane-new",
    cwd: "/projects/foo",
    title: "Terminal conversation",
    sdk_session_id: "sdk-ext-1",
    existing_thread_id: null,
    foreign_project: false,
    resume_divider_written: true,
    ...overrides,
  };
}

function makeRecord(
  overrides: Partial<AgentChatSessionRecord> = {},
): AgentChatSessionRecord {
  return {
    thread_id: "thread-existing",
    sdk_session_id: "sdk-ext-1",
    workspace_id: "ws-foo",
    cwd: "/projects/foo",
    provider: "claude",
    title: "Terminal conversation",
    created_at: "2026-08-24 12:00:00",
    last_active_at: "2026-08-24 12:00:00",
    model: null,
    effort: null,
    context_window: null,
    permission_mode: null,
    origin: "external_cli",
    ...overrides,
  };
}

function makeWorkspace(
  overrides: Partial<WorkspaceSnapshot> & { workspace_id: string; cwd: string },
): WorkspaceSnapshot {
  return {
    title: overrides.cwd.split("/").pop() ?? "ws",
    workspace_type: "standard",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: overrides.cwd,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    host_id: null,
    remote_cwd: null,
    attach_only: false,
    ...overrides,
  } as WorkspaceSnapshot;
}

function seedAppState(
  workspaces: WorkspaceSnapshot[],
  activeWorkspaceId: string | null = workspaces[0]?.workspace_id ?? null,
) {
  useAppStore.setState({
    homeDir: "/home/user",
    appState: {
      schema_version: 1,
      active_workspace_id: activeWorkspaceId,
      workspaces,
    } as unknown as AppStateSnapshot,
  });
}

/** `git worktree list` for the demo repo: the main checkout first, then
 *  its linked worktrees — the order git reports and the code relies on. */
const FOO_WORKTREES = [
  { path: "/projects/foo", branch: "main", is_bare: false },
  { path: "/worktrees/foo/feature", branch: "feature", is_bare: false },
];

/** One persisted row standing in for the backend's "resumed outside
 *  Codemux" divider, so the adopted thread is not blank. */
const DIVIDER_ROWS = [
  {
    id: 1,
    payload: JSON.stringify({
      type: "resume_divider",
      source: "external_cli",
      session_started_at: "2026-08-24T10:00:00.000Z",
      branch: "main",
    }),
    created_at_ms: Date.parse("2026-08-24T12:00:00.000Z"),
  },
];

function startInput() {
  const [paneId, provider, input] = vi.mocked(agentChatStartSession).mock
    .calls[0]!;
  return {
    paneId,
    provider,
    input: input as unknown as Record<string, unknown>,
  };
}

beforeEach(() => {
  useChatDraftStore.setState({
    draftsById: {},
    activeHomeDraftId: null,
    projectDraftIdByPath: {},
    activeDraftId: null,
  });
  useAgentChatStore.setState({ threads: {} });
  seedAppState([]);
  vi.mocked(agentChatAdoptExternalSession).mockReset();
  vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(makeResult());
  vi.mocked(agentChatCreatePane).mockClear();
  vi.mocked(agentChatCreatePane).mockResolvedValue("pane-new");
  vi.mocked(agentChatGetSession).mockReset();
  vi.mocked(agentChatGetSession).mockResolvedValue(null);
  vi.mocked(agentChatListMessagesAfter).mockReset();
  vi.mocked(agentChatListMessagesAfter).mockResolvedValue(DIVIDER_ROWS);
  vi.mocked(agentChatSendTurn).mockClear();
  vi.mocked(agentChatStartSession).mockClear();
  vi.mocked(activateWorkspace).mockClear();
  vi.mocked(createEmptyWorkspaceResult).mockReset();
  vi.mocked(createEmptyWorkspaceResult).mockImplementation((cwd: string) =>
    Promise.resolve({ workspaceId: "ws-new", cwd, adopted: false }),
  );
  vi.mocked(createWorktreeWorkspaceResult).mockClear();
  vi.mocked(createWorktreeWorkspace).mockClear();
  vi.mocked(createEmptyWorkspace).mockClear();
  // A plain repository by default: the folder is its only checkout.
  vi.mocked(listWorktrees).mockReset();
  vi.mocked(listWorktrees).mockImplementation((path: string) =>
    Promise.resolve([{ path, branch: "main", is_bare: false }]),
  );
  vi.mocked(importWorktreeWorkspace).mockReset();
  vi.mocked(importWorktreeWorkspace).mockResolvedValue("ws-wt");
  vi.mocked(toast.success).mockClear();
  vi.mocked(toast.warning).mockClear();
  vi.mocked(toast.error).mockClear();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("findWorkspaceAtDirectory", () => {
  it("matches a workspace by its cwd, ignoring a trailing separator", () => {
    const ws = makeWorkspace({ workspace_id: "ws-foo", cwd: "/projects/foo" });
    seedAppState([ws]);
    expect(
      findWorkspaceAtDirectory(useAppStore.getState().appState, "/projects/foo/"),
    ).toBe(ws);
  });

  it("matches a worktree workspace by its worktree path", () => {
    const ws = makeWorkspace({
      workspace_id: "ws-wt",
      cwd: "/worktrees/foo/feature",
      worktree_path: "/worktrees/foo/feature",
      project_root: "/projects/foo",
    });
    seedAppState([ws]);
    expect(
      findWorkspaceAtDirectory(
        useAppStore.getState().appState,
        "/worktrees/foo/feature",
      ),
    ).toBe(ws);
  });

  it("never matches a remote workspace, even with the same path string", () => {
    const ws = makeWorkspace({
      workspace_id: "ws-remote",
      cwd: "/projects/foo",
      host_id: 7,
    });
    seedAppState([ws]);
    expect(
      findWorkspaceAtDirectory(useAppStore.getState().appState, "/projects/foo"),
    ).toBeNull();
  });
});

describe("resumeExternalSessionFromDraft", () => {
  it("opens a workspace AT the session's folder, adopts, and starts with the resume cursor — no turn sent", async () => {
    vi.useFakeTimers();
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const result = await resumeExternalSessionFromDraft(
      draft.draftId,
      makeSession(),
    );

    expect(result).toEqual({
      success: true,
      workspaceId: "ws-new",
      paneId: "pane-new",
      threadId: "chat-adopted-1",
    });
    // Workspace anchored at the session's own directory.
    expect(vi.mocked(createEmptyWorkspaceResult)).toHaveBeenCalledWith(
      "/projects/foo",
    );
    // Pane rooted there, then adoption on THAT pane.
    expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledWith(
      "ws-new",
      "claude",
      "/projects/foo",
    );
    const [adoptPane, adoptProvider, payload] = vi.mocked(
      agentChatAdoptExternalSession,
    ).mock.calls[0]!;
    expect(adoptPane).toBe("pane-new");
    expect(adoptProvider).toBe("claude");
    expect(payload).not.toHaveProperty("same_repo");
    expect(payload.session_id).toBe("sdk-ext-1");
    // Started on the pane the backend bound, from the session's cursor.
    const { paneId, input } = startInput();
    expect(paneId).toBe("pane-new");
    expect(input.thread_id).toBe("chat-adopted-1");
    expect(input.cwd).toBe("/projects/foo");
    expect(input.resume_cursor).toEqual({ resume: "sdk-ext-1" });
    // No first prompt — ever.
    expect(vi.mocked(agentChatSendTurn)).not.toHaveBeenCalled();
    // The workspace is shown and the draft retired like a submit.
    expect(vi.mocked(activateWorkspace)).toHaveBeenCalledWith("ws-new");
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
    const promoted = useChatDraftStore.getState().draftsById[draft.draftId];
    expect(promoted?.promotedTo).toEqual({
      workspaceId: "ws-new",
      paneId: "pane-new",
      threadId: "chat-adopted-1",
    });
    expect(promoted?.promoting).toBe(false);
    vi.advanceTimersByTime(5000);
    expect(useChatDraftStore.getState().draftsById[draft.draftId]).toBeUndefined();
    expect(vi.mocked(toast.success)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.success).mock.calls[0]![0]).toContain(
      "in /projects/foo",
    );
    // The divider can say when the terminal last touched it.
    expect(adoptedSessionLastActiveAt("chat-adopted-1")).toBe(
      Date.parse("2026-08-24T12:00:00.000Z"),
    );
  });

  it("reuses the workspace already open at the session's folder", async () => {
    seedAppState([
      makeWorkspace({ workspace_id: "ws-foo", cwd: "/projects/foo" }),
    ]);
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();

    await resumeExternalSessionFromDraft(draft.draftId, makeSession());

    expect(vi.mocked(createEmptyWorkspaceResult)).not.toHaveBeenCalled();
    expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledWith(
      "ws-foo",
      "claude",
      "/projects/foo",
    );
  });

  it("imports an existing linked worktree in place — never creates one (R1)", async () => {
    // The project root is open; the session ran in a linked worktree
    // that has no workspace yet.
    seedAppState([
      makeWorkspace({ workspace_id: "ws-root", cwd: "/projects/foo" }),
    ]);
    const draft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/projects/foo");
    // Even a draft the user had set to fork a worktree must not.
    useChatDraftStore.getState().updateDraftConfig(draft.draftId, {
      checkoutMode: "worktree",
      worktreeName: "scratch",
      baseBranch: "release",
    });
    const session = makeSession({
      cwd: "/worktrees/foo/feature",
      git_branch: "feature",
    });
    vi.mocked(listWorktrees).mockResolvedValue(FOO_WORKTREES);
    vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(
      makeResult({ cwd: "/worktrees/foo/feature", workspace_id: "ws-wt" }),
    );

    const result = await resumeExternalSessionFromDraft(draft.draftId, session);

    expect(result).toMatchObject({ success: true, workspaceId: "ws-wt" });
    expect(vi.mocked(createWorktreeWorkspaceResult)).not.toHaveBeenCalled();
    expect(vi.mocked(createWorktreeWorkspace)).not.toHaveBeenCalled();
    // Identified from the session's own folder; adopted with the branch
    // git reports for it, and with no terminal beside the chat.
    expect(vi.mocked(listWorktrees)).toHaveBeenCalledWith("/worktrees/foo/feature");
    expect(vi.mocked(importWorktreeWorkspace)).toHaveBeenCalledWith(
      "/worktrees/foo/feature",
      "feature",
      "empty",
    );
    // Not a plain workspace over the worktree.
    expect(vi.mocked(createEmptyWorkspaceResult)).not.toHaveBeenCalled();
    // Anchored at the worktree's own path, not the root it belongs to.
    expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledWith(
      "ws-wt",
      "claude",
      "/worktrees/foo/feature",
    );
    expect(startInput().input.cwd).toBe("/worktrees/foo/feature");
    expect(vi.mocked(activateWorkspace)).toHaveBeenCalledWith("ws-wt");
  });

  it("opens the main checkout as a plain workspace even when the repo has linked worktrees", async () => {
    vi.mocked(listWorktrees).mockResolvedValue(FOO_WORKTREES);
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();

    const result = await resumeExternalSessionFromDraft(
      draft.draftId,
      makeSession({ cwd: "/projects/foo" }),
    );

    expect(result).toMatchObject({ success: true, workspaceId: "ws-new" });
    expect(vi.mocked(importWorktreeWorkspace)).not.toHaveBeenCalled();
    expect(vi.mocked(createWorktreeWorkspaceResult)).not.toHaveBeenCalled();
    expect(vi.mocked(createWorktreeWorkspace)).not.toHaveBeenCalled();
    expect(vi.mocked(createEmptyWorkspaceResult)).toHaveBeenCalledWith(
      "/projects/foo",
    );
  });

  it("falls back to a plain workspace when the worktree listing fails", async () => {
    vi.mocked(listWorktrees).mockRejectedValue("not a git repository");
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();

    const result = await resumeExternalSessionFromDraft(
      draft.draftId,
      makeSession({ cwd: "/worktrees/foo/feature", git_branch: "feature" }),
    );

    expect(result).toMatchObject({ success: true, workspaceId: "ws-new" });
    expect(vi.mocked(importWorktreeWorkspace)).not.toHaveBeenCalled();
    expect(vi.mocked(createWorktreeWorkspace)).not.toHaveBeenCalled();
    expect(vi.mocked(createEmptyWorkspaceResult)).toHaveBeenCalledWith(
      "/worktrees/foo/feature",
    );
    expect(warn).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.error)).not.toHaveBeenCalled();
    warn.mockRestore();
  });

  it("does not import a worktree with a detached HEAD — nothing to name the workspace after", async () => {
    vi.mocked(listWorktrees).mockResolvedValue([
      FOO_WORKTREES[0]!,
      { path: "/worktrees/foo/feature", branch: null, is_bare: false },
    ]);
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();

    await resumeExternalSessionFromDraft(
      draft.draftId,
      makeSession({ cwd: "/worktrees/foo/feature", git_branch: null }),
    );

    expect(vi.mocked(importWorktreeWorkspace)).not.toHaveBeenCalled();
    expect(vi.mocked(createEmptyWorkspaceResult)).toHaveBeenCalledWith(
      "/worktrees/foo/feature",
    );
  });

  it("pins the draft to the resolved workspace: locked, current checkout, base branch untouched (L1/L2)", async () => {
    // A project-rooted workspace is active in the sidebar — the exact
    // condition under which a Home draft would otherwise be re-pointed.
    seedAppState(
      [makeWorkspace({ workspace_id: "ws-other", cwd: "/projects/other" })],
      "ws-other",
    );
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore
      .getState()
      .updateDraftConfig(draft.draftId, { baseBranch: "release" });

    let observedMidFlight: ChatDraft | undefined;
    vi.mocked(agentChatCreatePane).mockImplementation(() => {
      observedMidFlight = useChatDraftStore.getState().draftsById[draft.draftId];
      return Promise.resolve("pane-new");
    });

    await resumeExternalSessionFromDraft(draft.draftId, makeSession());

    // Pinned BEFORE the pane was even created.
    expect(observedMidFlight?.target).toEqual({
      kind: "existing_workspace",
      workspaceId: "ws-new",
    });
    expect(observedMidFlight?.lockedToHome).toBe(true);
    expect(observedMidFlight?.checkoutMode).toBe("current");
    expect(observedMidFlight?.baseBranch).toBe("release");
    // Nothing ever ran against the sidebar's workspace.
    expect(vi.mocked(agentChatCreatePane)).not.toHaveBeenCalledWith(
      "ws-other",
      expect.anything(),
      expect.anything(),
    );
  });

  it("launches in the draft's permission mode, never the terminal session's", async () => {
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore
      .getState()
      .updateDraftConfig(draft.draftId, { permissionMode: "plan" });

    await resumeExternalSessionFromDraft(draft.draftId, makeSession());

    expect(startInput().input.permission_mode).toBe("plan");
    const slice = useAgentChatStore.getState().threads["chat-adopted-1"];
    expect(slice?.permissionMode).toBe("plan");
    expect(slice?.sessionLaunchMode).toBe("plan");
  });

  it("does not claim full history when the divider could not be hydrated", async () => {
    vi.mocked(agentChatListMessagesAfter).mockRejectedValue("db locked");
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();

    const result = await resumeExternalSessionFromDraft(
      draft.draftId,
      makeSession(),
    );

    expect(result.success).toBe(true);
    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.warning)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.warning).mock.calls[0]![0]).toContain(
      "transcript isn't shown",
    );
  });

  it("reports an adoption failure, starts nothing, and hands the draft back", async () => {
    vi.mocked(agentChatAdoptExternalSession).mockRejectedValue(
      "session file vanished",
    );
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const result = await resumeExternalSessionFromDraft(
      draft.draftId,
      makeSession(),
    );

    expect(result).toEqual({
      success: false,
      error: "session file vanished",
    });
    expect(vi.mocked(agentChatStartSession)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled();
    const after = useChatDraftStore.getState().draftsById[draft.draftId];
    expect(after?.promoting).toBe(false);
    // Not a send that failed — no retry banner.
    expect(after?.lastSendError).toBeNull();
    expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);
  });

  it("switches to a conversation Codemux already owns instead of adopting it twice", async () => {
    const record = makeRecord();
    vi.mocked(agentChatGetSession).mockResolvedValue(record);
    seedAppState([
      makeWorkspace({
        workspace_id: "ws-foo",
        cwd: "/projects/foo",
        surfaces: [
          {
            surface_id: "surface-1",
            title: "chat",
            root: {
              kind: "agent_chat",
              pane_id: "pane-owner",
              title: "chat",
              thread_id: "thread-existing",
              provider: "claude",
              cwd: "/projects/foo",
            },
            active_pane_id: "pane-owner",
          },
        ],
      }),
    ]);
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();

    const result = await resumeExternalSessionFromDraft(
      draft.draftId,
      makeSession({ existing_thread_id: "thread-existing" }),
    );

    expect(result).toEqual({
      success: true,
      workspaceId: "ws-foo",
      paneId: "pane-owner",
      threadId: "thread-existing",
    });
    expect(vi.mocked(agentChatAdoptExternalSession)).not.toHaveBeenCalled();
    expect(vi.mocked(createEmptyWorkspaceResult)).not.toHaveBeenCalled();
    expect(vi.mocked(agentChatStartSession)).not.toHaveBeenCalled();
    expect(vi.mocked(activateWorkspace)).toHaveBeenCalledWith("ws-foo");
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("reopens an owned thread with no pane in its own directory", async () => {
    const record = makeRecord({ cwd: "/projects/ledger", workspace_id: "ws-gone" });
    vi.mocked(agentChatGetSession).mockResolvedValue(record);
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();

    const result = await resumeExternalSessionFromDraft(
      draft.draftId,
      makeSession({
        existing_thread_id: "thread-existing",
        cwd: "/projects/ledger",
        same_repo: false,
      }),
    );

    expect(result.success).toBe(true);
    expect(vi.mocked(agentChatAdoptExternalSession)).not.toHaveBeenCalled();
    expect(vi.mocked(createEmptyWorkspaceResult)).toHaveBeenCalledWith(
      "/projects/ledger",
    );
    const { input } = startInput();
    expect(input.cwd).toBe("/projects/ledger");
    expect(input.resume_cursor).toEqual({ resume: "sdk-ext-1" });
    expect(input.thread_id).not.toBe("thread-existing");
  });
});
