/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactElement } from "react";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  AdoptableAgentSession,
  AdoptExternalSessionResult,
} from "@/tauri/commands";
import type { AgentChatProviderKind, AppStateSnapshot } from "@/tauri/types";

// ── Module mocks ──
//
// The real command module, with every call the draft surface + composer
// + resume path can reach stubbed. `agent_chat_start_session` echoes the
// thread id it was given, as the real backend does for an adopted row.

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return {
    ...actual,
    getHomeDir: vi.fn().mockResolvedValue("/home/user"),
    dbGetRecentProjects: vi.fn().mockResolvedValue([]),
    dbGetUiState: vi.fn().mockResolvedValue(null),
    activateWorkspace: vi.fn().mockResolvedValue(undefined),
    activatePane: vi.fn().mockResolvedValue(undefined),
    agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
    agentChatStartSession: vi.fn(
      (
        _paneId: string,
        _provider: AgentChatProviderKind,
        input: { thread_id: string },
      ) => Promise.resolve(input.thread_id),
    ),
    agentChatSendTurn: vi.fn(),
    agentChatProviderHealth: vi.fn(() => new Promise(() => {})),
    primeChatMcp: vi.fn().mockResolvedValue(undefined),
    listProjectFiles: vi.fn().mockResolvedValue([]),
    agentChatListSessionMentions: vi.fn().mockResolvedValue([]),
    getMcpRuntimeStatus: vi.fn().mockResolvedValue([]),
    setMcpDisabledIds: vi.fn().mockResolvedValue(undefined),
    listMcpServers: vi.fn().mockResolvedValue([]),
    listSkills: vi.fn().mockResolvedValue([]),
    startSkillsWatcher: vi.fn().mockResolvedValue(0),
    listChatSlashCommands: vi.fn().mockResolvedValue([]),
    agentChatListAdoptableSessions: vi.fn().mockResolvedValue([]),
    agentChatAdoptExternalSession: vi.fn(),
    agentChatListMessagesAfter: vi.fn().mockResolvedValue([]),
    agentChatGetSession: vi.fn().mockResolvedValue(null),
    createEmptyWorkspaceResult: vi.fn(),
    createWorktreeWorkspaceResult: vi.fn(),
    createWorktreeWorkspace: vi.fn(),
    importWorktreeWorkspace: vi.fn(),
    // A plain repository: the session's folder is its only checkout.
    listWorktrees: vi.fn((path: string) =>
      Promise.resolve([{ path, branch: "main", is_bare: false }]),
    ),
    generateBranchName: vi.fn(),
    generateRandomBranchName: vi.fn(),
    renameWorkspace: vi.fn().mockResolvedValue(undefined),
  };
});

vi.mock("@/lib/provider-auth", () => ({
  fetchProviderAuth: vi.fn().mockResolvedValue({
    supported: false,
    installed: false,
    authenticated: false,
  }),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

// Provider capabilities — minimal stub (see DraftChatSurface.test).
const STUB_CAP_STATE = {
  claude: null,
  codex: null,
  opencode: null,
  claudeError: null,
  codexError: null,
  opencodeError: null,
  loaded: false,
  refresh: vi.fn(),
  refreshAll: vi.fn(),
};
vi.mock("@/stores/provider-capabilities-store", () => ({
  useProviderCapabilities: Object.assign(
    vi.fn((selector?: (s: unknown) => unknown) =>
      typeof selector === "function" ? selector(STUB_CAP_STATE) : STUB_CAP_STATE,
    ),
    { getState: () => STUB_CAP_STATE },
  ),
  selectCapabilities: () => null,
  selectError: () => null,
  selectModel: () => null,
}));

// Stub ThreadScopeRow: these tests are about the row BELOW it and the
// resume dispatch, not the location/checkout/branch popovers. The stub
// still exposes the props so the branch-seeding landmine and the pinned
// destination are observable.
type ThreadScopeRowStubProps = {
  target: import("@/stores/chat-draft-store").ChatDraft["target"];
  projectPath: string | null;
  checkoutMode: "current" | "worktree";
  baseBranch: string;
  disabled?: boolean;
  pinnedCheckout?: {
    projectPath: string | null;
    cwd: string;
    worktreeName: string | null;
    branch: string | null;
  } | null;
};
vi.mock("@/components/chat/pickers/ThreadScopeRow", () => ({
  SCOPE_STRIP_INSET: "w-full px-5",
  ThreadScopeRow: (props: ThreadScopeRowStubProps) => (
    <button
      data-testid="thread-scope-row-stub"
      data-target-kind={props.target.kind}
      data-project-path={props.projectPath ?? ""}
      data-checkout-mode={props.checkoutMode}
      data-base-branch={props.baseBranch}
      data-pinned-project={props.pinnedCheckout?.projectPath ?? ""}
      data-pinned-cwd={props.pinnedCheckout?.cwd ?? ""}
      data-pinned-worktree={props.pinnedCheckout?.worktreeName ?? ""}
    >
      scope:{props.projectPath ?? "home"}
    </button>
  ),
}));

// Wrap the real Composer so the props the draft hands it for `/resume`
// are observable without reaching into the picker's own DOM.
type ResumeProps = {
  resumeScope?: { cwd: string | null; projectRoot: string | null } | null;
  resumeOpenSignal?: number;
};
const composerResumeProps: ResumeProps = {};
vi.mock("@/components/chat/Composer", async (importActual) => {
  const actual = (await importActual()) as {
    Composer: (props: ResumeProps) => ReactElement;
  };
  return {
    ...actual,
    Composer: (props: ResumeProps) => {
      composerResumeProps.resumeScope = props.resumeScope;
      composerResumeProps.resumeOpenSignal = props.resumeOpenSignal;
      return actual.Composer(props);
    },
  };
});

import { DraftChatSurface } from "./DraftChatSurface";
import { toast } from "@/lib/toast";
import {
  agentChatAdoptExternalSession,
  agentChatCreatePane,
  agentChatListAdoptableSessions,
  agentChatListMessagesAfter,
  agentChatSendTurn,
  agentChatStartSession,
  createEmptyWorkspaceResult,
  createWorktreeWorkspace,
  createWorktreeWorkspaceResult,
  importWorktreeWorkspace,
  listWorktrees,
} from "@/tauri/commands";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import {
  useChatDraftStore,
  type ChatDraft,
} from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useProviderCommandsStore } from "@/stores/provider-commands-store";
import { useSkillsStore } from "@/stores/skills-store";

const listAdoptableMock =
  agentChatListAdoptableSessions as unknown as ReturnType<typeof vi.fn>;

const DAY_MS = 24 * 60 * 60 * 1000;

function makeSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return {
    session_id: "sdk-ext-1",
    title: "Refactor the splitter",
    cwd: "/projects/foo",
    git_branch: "main",
    last_modified: new Date().toISOString(),
    created_at: new Date().toISOString(),
    file_size: 4096,
    title_source: "summary",
    existing_thread_id: null,
    same_repo: true,
    project_root: "/projects/foo",
    worktree_name: null,
    ...overrides,
  };
}

/** A session in an existing linked worktree of another project. */
function makeWorktreeSession(): AdoptableAgentSession {
  return makeSession({
    session_id: "sdk-ext-wt",
    title: "Resolve PR conflicts",
    cwd: "/projects/codemux-worktrees/resolve-pr-conflicts",
    git_branch: "resolve-pr-conflicts",
    project_root: "/projects/codemux",
    worktree_name: "resolve-pr-conflicts",
    same_repo: false,
  });
}

function makeResult(
  overrides: Partial<AdoptExternalSessionResult> = {},
): AdoptExternalSessionResult {
  return {
    thread_id: "chat-adopted-1",
    workspace_id: "ws-new",
    pane_id: "pane-new",
    cwd: "/projects/foo",
    title: "Refactor the splitter",
    sdk_session_id: "sdk-ext-1",
    existing_thread_id: null,
    foreign_project: false,
    resume_divider_written: true,
    ...overrides,
  };
}

const DIVIDER_ROWS = [
  {
    id: 1,
    payload: JSON.stringify({
      type: "resume_divider",
      source: "external_cli",
      session_started_at: new Date().toISOString(),
      branch: "main",
    }),
    created_at_ms: Date.now(),
  },
];

/** A project-rooted workspace active in the sidebar — the condition
 *  under which a Home draft is normally re-pointed at it. */
function seedActiveSidebarWorkspace() {
  useAppStore.setState({
    homeDir: "/home/user",
    appState: {
      schema_version: 1,
      active_workspace_id: "ws-other",
      workspaces: [
        {
          workspace_id: "ws-other",
          title: "other",
          workspace_type: "standard",
          cwd: "/projects/other",
          project_root: "/projects/other",
          worktree_path: null,
          git_branch: "main",
          git_ahead: 0,
          git_behind: 0,
          git_additions: 0,
          git_deletions: 0,
          git_changed_files: 0,
          notification_count: 0,
          notifications_muted: false,
          latest_agent_state: null,
          pr_number: null,
          pr_state: null,
          pr_url: null,
          linked_issue: null,
          tabs: [],
          active_tab_id: "",
          active_surface_id: "",
          surfaces: [],
          host_id: null,
          remote_cwd: null,
          attach_only: false,
        },
      ],
    } as unknown as AppStateSnapshot,
  });
}

function renderSurface() {
  return render(
    <TooltipProvider>
      <DraftChatSurface />
    </TooltipProvider>,
  );
}

function getTextarea(container: HTMLElement) {
  return container.querySelector("textarea") as HTMLTextAreaElement;
}

function type(textarea: HTMLTextAreaElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    "value",
  )?.set;
  setter?.call(textarea, value);
  textarea.setSelectionRange(value.length, value.length);
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
}

function seedHomeDraft(opts: { lockedToHome?: boolean } = {}) {
  const draft = useChatDraftStore.getState().getOrCreateHomeDraft(opts);
  useChatDraftStore.getState().setActiveDraft(draft.draftId);
  return draft;
}

function seedProjectDraft(projectPath = "/projects/foo") {
  const draft = useChatDraftStore
    .getState()
    .getOrCreateProjectDraft(projectPath);
  useChatDraftStore.getState().setActiveDraft(draft.draftId);
  return draft;
}

beforeEach(() => {
  useChatDraftStore.setState({
    draftsById: {},
    activeHomeDraftId: null,
    projectDraftIdByPath: {},
    activeDraftId: null,
  });
  useAgentChatStore.setState({ threads: {} });
  useAppStore.setState({ appState: null, homeDir: "/home/user" });
  useFeatureFlags.setState({
    enableAgentChat: false,
    enableLazyWorkspaceCreation: false,
  });
  useSkillsStore.setState({
    skills: [],
    loaded: false,
    loading: false,
    error: null,
    adapterErrors: [],
    loadedAt: 0,
    includePlugins: true,
    inventoryCache: {},
    activeContextKey: null,
    inFlightContexts: {},
    nextRequestId: 1,
    cacheGeneration: 0,
  });
  useProviderCommandsStore.getState().invalidate();
  composerResumeProps.resumeScope = undefined;
  composerResumeProps.resumeOpenSignal = undefined;
  listAdoptableMock.mockReset();
  listAdoptableMock.mockResolvedValue([]);
  vi.mocked(agentChatAdoptExternalSession).mockReset();
  vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(makeResult());
  vi.mocked(agentChatListMessagesAfter).mockReset();
  vi.mocked(agentChatListMessagesAfter).mockResolvedValue(DIVIDER_ROWS);
  vi.mocked(agentChatCreatePane).mockClear();
  vi.mocked(agentChatStartSession).mockClear();
  vi.mocked(agentChatSendTurn).mockClear();
  vi.mocked(createEmptyWorkspaceResult).mockReset();
  vi.mocked(createEmptyWorkspaceResult).mockImplementation((cwd: string) =>
    Promise.resolve({ workspaceId: "ws-new", cwd, adopted: false }),
  );
  vi.mocked(createWorktreeWorkspaceResult).mockClear();
  vi.mocked(createWorktreeWorkspace).mockClear();
  vi.mocked(importWorktreeWorkspace).mockReset();
  vi.mocked(listWorktrees).mockReset();
  vi.mocked(listWorktrees).mockImplementation((path: string) =>
    Promise.resolve([{ path, branch: "main", is_bare: false }]),
  );
  vi.mocked(toast.error).mockClear();
  vi.mocked(toast.success).mockClear();
});

afterEach(() => cleanup());

describe("DraftChatSurface · continue a terminal session", () => {
  it("offers /resume in the draft composer's slash popup", async () => {
    seedHomeDraft();
    const { container } = renderSurface();
    type(getTextarea(container), "/");
    await waitFor(() => {
      expect(
        document.querySelector('[data-testid="slash-item-composer:resume"]'),
      ).not.toBeNull();
    });
  });

  it("discovers across every project for both Home and project drafts", async () => {
    seedHomeDraft();
    const first = renderSurface();
    await waitFor(() => expect(listAdoptableMock).toHaveBeenCalled());
    expect(listAdoptableMock.mock.calls[0]![1]).toMatchObject({
      current_cwd: "/home/user",
      all_projects: true,
      include_worktrees: true,
    });
    first.unmount();
    listAdoptableMock.mockClear();

    seedProjectDraft();
    renderSurface();
    await waitFor(() => expect(listAdoptableMock).toHaveBeenCalled());
    expect(listAdoptableMock.mock.calls[0]![1]).toMatchObject({
      current_cwd: "/projects/foo",
      all_projects: true,
      include_worktrees: true,
    });
  });

  it("hands the composer the selected project as the picker's open group", async () => {
    seedProjectDraft();
    renderSurface();
    await waitFor(() => {
      expect(composerResumeProps.resumeScope).toEqual({
        cwd: "/projects/foo",
        projectRoot: "/projects/foo",
      });
    });
    cleanup();

    seedHomeDraft();
    renderSurface();
    await waitFor(() => {
      expect(composerResumeProps.resumeScope).toEqual({
        cwd: "/home/user",
        projectRoot: null,
      });
    });
  });

  it("renders the row only when discovery found something in scope", async () => {
    seedHomeDraft();
    const empty = renderSurface();
    await waitFor(() => expect(listAdoptableMock).toHaveBeenCalled());
    // Discovery resolved to nothing — give the effect a tick to settle.
    await waitFor(() => {
      expect(
        empty.queryByTestId("draft-continue-terminal-session"),
      ).toBeNull();
    });
    expect(empty.getByText("What should we do today?")).toBeInTheDocument();
    empty.unmount();

    // A project draft ignores sessions from other projects entirely.
    listAdoptableMock.mockResolvedValue([
      makeSession({ cwd: "/projects/bar", project_root: "/projects/bar" }),
    ]);
    seedProjectDraft();
    const other = renderSurface();
    await waitFor(() => expect(listAdoptableMock).toHaveBeenCalled());
    await waitFor(() => {
      expect(
        other.queryByTestId("draft-continue-terminal-session"),
      ).toBeNull();
    });
    other.unmount();

    listAdoptableMock.mockResolvedValue([makeSession()]);
    seedHomeDraft();
    const { findByTestId } = renderSurface();
    const row = await findByTestId("draft-continue-terminal-session");
    expect(row.dataset.variant).toBe("featured");
    expect(row.textContent).toContain("Refactor the splitter");
  });

  it("the quiet row and the featured row's 'more' both open the composer's picker", async () => {
    listAdoptableMock.mockResolvedValue([
      makeSession({
        last_modified: new Date(Date.now() - 3 * DAY_MS).toISOString(),
      }),
    ]);
    seedHomeDraft();
    const quiet = renderSurface();
    const row = await quiet.findByTestId("draft-continue-terminal-session");
    expect(row.dataset.variant).toBe("quiet");
    expect(composerResumeProps.resumeOpenSignal).toBe(0);
    fireEvent.click(row);
    await waitFor(() =>
      expect(composerResumeProps.resumeOpenSignal).toBe(1),
    );
    quiet.unmount();

    listAdoptableMock.mockResolvedValue([
      makeSession(),
      makeSession({ session_id: "sdk-ext-2", title: "Second" }),
    ]);
    seedHomeDraft();
    const featured = renderSurface();
    fireEvent.click(
      await featured.findByTestId("draft-continue-terminal-session-more"),
    );
    await waitFor(() =>
      expect(composerResumeProps.resumeOpenSignal).toBe(1),
    );
    expect(vi.mocked(agentChatAdoptExternalSession)).not.toHaveBeenCalled();
  });

  it("Continue resumes the featured session in its own folder — no prompt, no worktree, no sidebar re-point", async () => {
    // The sidebar has a project workspace active. A Home draft opened
    // from the "New agent" button stays on Home (so the landing spans
    // every project and names this session); the resume must still
    // land in the SESSION's folder, never the sidebar's.
    seedActiveSidebarWorkspace();
    listAdoptableMock.mockResolvedValue([makeSession()]);
    const draft = seedHomeDraft({ lockedToHome: true });
    const { findByTestId } = renderSurface();

    // The draft must already be pinned to the session's workspace by the
    // time the provider starts — nothing may re-point it mid-flight.
    let targetAtStart: ChatDraft["target"] | undefined;
    vi.mocked(agentChatStartSession).mockImplementationOnce(
      (_pane, _provider, input) => {
        targetAtStart =
          useChatDraftStore.getState().draftsById[draft.draftId]?.target;
        return Promise.resolve(input.thread_id);
      },
    );

    fireEvent.click(
      await findByTestId("draft-continue-terminal-session-continue"),
    );

    await waitFor(() => {
      expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    });
    expect(targetAtStart).toEqual({
      kind: "existing_workspace",
      workspaceId: "ws-new",
    });

    // Workspace at the session's folder; the sidebar's never touched.
    expect(vi.mocked(createEmptyWorkspaceResult)).toHaveBeenCalledWith(
      "/projects/foo",
    );
    expect(vi.mocked(createWorktreeWorkspaceResult)).not.toHaveBeenCalled();
    expect(vi.mocked(createWorktreeWorkspace)).not.toHaveBeenCalled();
    // The main checkout is not a linked worktree — nothing to import.
    expect(vi.mocked(importWorktreeWorkspace)).not.toHaveBeenCalled();
    expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledWith(
      "ws-new",
      "claude",
      "/projects/foo",
    );
    // Adopted on that pane, started from the session's cursor.
    expect(vi.mocked(agentChatAdoptExternalSession).mock.calls[0]![0]).toBe(
      "pane-new",
    );
    const [startPane, , startInput] = vi.mocked(agentChatStartSession).mock
      .calls[0]!;
    expect(startPane).toBe("pane-new");
    expect(startInput).toMatchObject({
      thread_id: "chat-adopted-1",
      cwd: "/projects/foo",
      resume_cursor: { resume: "sdk-ext-1" },
    });
    // No first prompt.
    expect(vi.mocked(agentChatSendTurn)).not.toHaveBeenCalled();

    // Pinned to the resolved workspace, never forked, branch untouched.
    await waitFor(() => {
      expect(useChatDraftStore.getState().activeDraftId).toBeNull();
    });
    const after = useChatDraftStore.getState().draftsById[draft.draftId];
    expect(after?.target).toEqual({
      kind: "existing_workspace",
      workspaceId: "ws-new",
    });
    expect(after?.lockedToHome).toBe(true);
    expect(after?.checkoutMode).toBe("current");
    expect(after?.baseBranch).toBe("");
    expect(after?.promotedTo).toEqual({
      workspaceId: "ws-new",
      paneId: "pane-new",
      threadId: "chat-adopted-1",
    });
    expect(vi.mocked(toast.success)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.error)).not.toHaveBeenCalled();
  });

  it("re-points the scope strip to the session's project and worktree before anything runs", async () => {
    // A worktree session from ANOTHER project, picked from a web-snake
    // draft. Hold the adoption open so the in-flight state is visible.
    listAdoptableMock.mockResolvedValue([makeWorktreeSession()]);
    vi.mocked(agentChatAdoptExternalSession).mockReturnValue(
      new Promise(() => {}),
    );
    vi.mocked(listWorktrees).mockResolvedValue([
      { path: "/projects/codemux", branch: "main", is_bare: false },
      {
        path: "/projects/codemux-worktrees/resolve-pr-conflicts",
        branch: "resolve-pr-conflicts",
        is_bare: false,
      },
    ]);
    vi.mocked(importWorktreeWorkspace).mockResolvedValue("ws-wt");
    seedHomeDraft();
    const { findByTestId, getByTestId, getByText } = renderSurface();

    fireEvent.click(
      await findByTestId("draft-continue-terminal-session-continue"),
    );

    // Strip: destination named at once, from the session itself.
    const strip = getByTestId("thread-scope-row-stub");
    expect(strip.dataset.pinnedProject).toBe("/projects/codemux");
    expect(strip.dataset.pinnedWorktree).toBe("resolve-pr-conflicts");
    expect(strip.dataset.pinnedCwd).toBe(
      "/projects/codemux-worktrees/resolve-pr-conflicts",
    );
    // Headline → status line, and the landing row is gone.
    expect(getByText('Resuming "Resolve PR conflicts"…')).toBeInTheDocument();
    expect(
      document.querySelector('[data-testid="draft-continue-terminal-session"]'),
    ).toBeNull();

    // The existing worktree is imported in place; none is created (R1).
    await waitFor(() => {
      expect(vi.mocked(importWorktreeWorkspace)).toHaveBeenCalledWith(
        "/projects/codemux-worktrees/resolve-pr-conflicts",
        "resolve-pr-conflicts",
        "empty",
      );
    });
    expect(vi.mocked(createWorktreeWorkspace)).not.toHaveBeenCalled();
    expect(vi.mocked(createWorktreeWorkspaceResult)).not.toHaveBeenCalled();
    expect(vi.mocked(createEmptyWorkspaceResult)).not.toHaveBeenCalled();
    // No confirmation dialog stands between the press and the move.
    expect(document.querySelector('[role="alertdialog"]')).toBeNull();
  });

  it("a failed adoption hands the composer back with an error toast", async () => {
    listAdoptableMock.mockResolvedValue([makeSession()]);
    vi.mocked(agentChatAdoptExternalSession).mockRejectedValue("gone");
    const draft = seedHomeDraft();
    const { findByTestId, container, getByTestId } = renderSurface();

    fireEvent.click(
      await findByTestId("draft-continue-terminal-session-continue"),
    );

    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(vi.mocked(toast.error).mock.calls[0]![0]).toContain(
      'Failed to resume "Refactor the splitter"',
    );
    expect(vi.mocked(agentChatStartSession)).not.toHaveBeenCalled();
    expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);
    expect(useChatDraftStore.getState().draftsById[draft.draftId]?.promoting).toBe(
      false,
    );
    // The composer is back and usable — no "Send failed" banner either,
    // and the strip is no longer pinned to the failed destination.
    expect(container.querySelector("textarea")).not.toBeNull();
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(getByTestId("thread-scope-row-stub").dataset.pinnedCwd).toBe("");
  });
});
