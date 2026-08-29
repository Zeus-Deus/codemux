/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";

import type {
  AgentChatProviderKind,
  PaneNodeSnapshot,
} from "@/tauri/types";

// ── Module mocks ──
//
// Everything the adopt path touches, so the hook runs under jsdom with
// no backend. `agent_chat_start_session` echoes the thread id it was
// given — the real backend returns the same value for an adopted row,
// which the command already minted.

vi.mock("@/tauri/commands", () => ({
  agentChatAdoptExternalSession: vi.fn(),
  agentChatGetSession: vi.fn().mockResolvedValue(null),
  agentChatListMessagesAfter: vi.fn().mockResolvedValue([]),
  agentChatStartSession: vi.fn(
    (
      _paneId: string,
      _provider: AgentChatProviderKind,
      input: { thread_id: string },
    ) => Promise.resolve(input.thread_id),
  ),
  agentChatStopSession: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      appState: {
        active_workspace_id: "ws-1",
        workspaces: [{ workspace_id: "ws-1", cwd: "/projects/foo" }],
      },
    }),
  ),
}));

import { useAgentChatSessionActions } from "./use-agent-chat-session-actions";
import { toast } from "@/lib/toast";
import {
  agentChatAdoptExternalSession,
  agentChatGetSession,
  agentChatListMessagesAfter,
  agentChatStartSession,
  agentChatStopSession,
  type AdoptableAgentSession,
  type AdoptExternalSessionResult,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import { useAgentChatStore } from "@/stores/agent-chat-store";

type AgentChatPane = Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;

function makePane(overrides: Partial<AgentChatPane> = {}): AgentChatPane {
  return {
    kind: "agent_chat",
    pane_id: "pane-1",
    provider: "claude" as AgentChatProviderKind,
    cwd: "/projects/foo",
    thread_id: "thread-old",
    ...overrides,
  } as AgentChatPane;
}

function makeSession(
  overrides: Partial<AdoptableAgentSession> = {},
): AdoptableAgentSession {
  return {
    session_id: "sdk-ext-1",
    title: "Terminal conversation",
    cwd: "/projects/foo",
    git_branch: "main",
    last_modified: "2026-04-24T12:00:00.000Z",
    created_at: "2026-04-24T10:00:00.000Z",
    file_size: 8192,
    title_source: "summary",
    existing_thread_id: null,
    same_repo: true,
    ...overrides,
  };
}

function makeResult(
  overrides: Partial<AdoptExternalSessionResult> = {},
): AdoptExternalSessionResult {
  return {
    thread_id: "chat-adopted-1",
    workspace_id: "ws-1",
    // Same folder as the pane, so the backend leaves the conversation
    // where it was requested from.
    pane_id: "pane-1",
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
    workspace_id: "ws-1",
    cwd: "/projects/foo",
    provider: "claude",
    title: "Terminal conversation",
    created_at: "2026-04-24 12:00:00",
    last_active_at: "2026-04-24 12:00:00",
    model: null,
    effort: null,
    context_window: null,
    permission_mode: null,
    origin: "external_cli",
    ...overrides,
  };
}

/** One persisted row standing in for the backend's "resumed outside
 *  Codemux" divider, so the adopted thread is not blank. */
const DIVIDER_ROWS = [
  {
    id: 1,
    payload: JSON.stringify({
      type: "resume_divider",
      source: "external_cli",
      session_started_at: "2026-04-24T10:00:00.000Z",
      branch: "main",
    }),
  },
];

beforeEach(() => {
  useAgentChatStore.setState({ threads: {} });
  vi.mocked(agentChatAdoptExternalSession).mockReset();
  vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(makeResult());
  vi.mocked(agentChatGetSession).mockReset();
  vi.mocked(agentChatGetSession).mockResolvedValue(null);
  vi.mocked(agentChatListMessagesAfter).mockReset();
  vi.mocked(agentChatListMessagesAfter).mockResolvedValue(DIVIDER_ROWS);
  vi.mocked(agentChatStartSession).mockClear();
  vi.mocked(agentChatStopSession).mockClear();
  vi.mocked(agentChatStopSession).mockResolvedValue(undefined);
  vi.mocked(toast.success).mockClear();
  vi.mocked(toast.warning).mockClear();
  vi.mocked(toast.error).mockClear();
});

afterEach(() => cleanup());

describe("handleAdoptExternalSession — adoption", () => {
  it("adopts, then starts the adopted thread with the session's resume cursor", async () => {
    const session = makeSession();
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(session);

    expect(vi.mocked(agentChatAdoptExternalSession)).toHaveBeenCalledTimes(1);
    const [paneId, provider, payload] = vi.mocked(
      agentChatAdoptExternalSession,
    ).mock.calls[0]!;
    expect(paneId).toBe("pane-1");
    expect(provider).toBe("claude");
    // Only the provider-side descriptor crosses the boundary.
    expect(payload).not.toHaveProperty("same_repo");
    expect(payload.session_id).toBe("sdk-ext-1");

    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0]!;
    const typed = input as {
      thread_id: string;
      cwd: string;
      resume_cursor: unknown;
    };
    expect(typed.thread_id).toBe("chat-adopted-1");
    expect(typed.resume_cursor).toEqual({ resume: "sdk-ext-1" });
    // R1 — attaches to the session's own folder; no worktree is created.
    expect(typed.cwd).toBe("/projects/foo");
    expect(vi.mocked(toast.success)).toHaveBeenCalledTimes(1);
  });

  it("stops the pane's current session and hydrates the divider first", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(makeSession());

    expect(vi.mocked(agentChatStopSession)).toHaveBeenCalledWith(
      "claude",
      "thread-old",
    );
    expect(vi.mocked(agentChatListMessagesAfter)).toHaveBeenCalledWith(
      "chat-adopted-1",
      null,
    );
    expect(
      useAgentChatStore.getState().threads["chat-adopted-1"],
    ).toBeDefined();
  });

  it("keeps Codemux's current permission mode, not the external session's", async () => {
    const store = useAgentChatStore.getState();
    store.ensureThread("thread-old");
    store.setPermissionMode("thread-old", "plan");

    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(makeSession());

    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0]!;
    expect((input as { permission_mode: string | null }).permission_mode).toBe(
      "plan",
    );
    const slice = useAgentChatStore.getState().threads["chat-adopted-1"];
    expect(slice!.permissionMode).toBe("plan");
    expect(slice!.sessionLaunchMode).toBe("plan");
  });

  it("starts the thread in the pane the backend re-homed it to", async () => {
    // A sibling worktree of the same repo: one click, no confirmation,
    // but the conversation's folder is not this pane's. The backend has
    // already opened a chat pane rooted there and bound the thread to
    // it, so starting on the requesting pane would leave that tab blank
    // AND hide the thread from this pane's history dropdown.
    vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(
      makeResult({ pane_id: "pane-worktree", cwd: "/projects/foo-wt" }),
    );
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(
      makeSession({ cwd: "/projects/foo-wt" }),
    );

    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    const [startPaneId, , input] = vi.mocked(agentChatStartSession).mock
      .calls[0]!;
    expect(startPaneId).toBe("pane-worktree");
    expect((input as { cwd: string }).cwd).toBe("/projects/foo-wt");
    // The pane the user clicked from is not the one that took the
    // conversation, so its own session is left running.
    expect(vi.mocked(agentChatStopSession)).not.toHaveBeenCalled();
    // The toast says where the conversation went.
    expect(vi.mocked(toast.success).mock.calls[0]![0]).toContain(
      "/projects/foo-wt",
    );
  });

  it("launches a re-homed pane in the provider default, not this pane's mode", async () => {
    // The re-homed pane is brand new: it has no mode of its own, so it
    // boots like any other fresh chat tab rather than inheriting the
    // mode of a pane it has nothing to do with.
    const store = useAgentChatStore.getState();
    store.ensureThread("thread-old");
    store.setPermissionMode("thread-old", "plan");
    vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(
      makeResult({ pane_id: "pane-worktree", cwd: "/projects/foo-wt" }),
    );

    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(
      makeSession({ cwd: "/projects/foo-wt" }),
    );

    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0]!;
    expect((input as { permission_mode: string | null }).permission_mode).toBe(
      "bypassPermissions",
    );
  });

  it("falls back to the provider default when the pane has no mode yet", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane({ thread_id: null })),
    );
    await result.current.handleAdoptExternalSession(makeSession());

    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0]!;
    expect((input as { permission_mode: string | null }).permission_mode).toBe(
      "bypassPermissions",
    );
  });
});

describe("handleAdoptExternalSession — already in Codemux", () => {
  it("switches to the owning thread instead of adopting twice", async () => {
    vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(
      makeResult({ existing_thread_id: "thread-existing" }),
    );
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ sdk_session_id: "sdk-ext-1" }),
    );

    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(makeSession());

    expect(vi.mocked(agentChatAdoptExternalSession)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(agentChatGetSession)).toHaveBeenCalledWith(
      "thread-existing",
    );
    // Resumed through the ordinary path: one start, cursor from the row.
    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0]!;
    expect((input as { resume_cursor: unknown }).resume_cursor).toEqual({
      resume: "sdk-ext-1",
    });
    // The adopted-thread id is never launched — the existing thread's
    // own history is what hydrates.
    expect((input as { thread_id: string }).thread_id).not.toBe(
      "chat-adopted-1",
    );
  });

  it("resumes the owning thread in ITS directory, never this pane's", async () => {
    // A conversation adopted for /projects/ledger, switched to from a
    // pane rooted at /projects/foo. `upsert_agent_chat_session` writes
    // back whatever cwd the start carries, so passing the pane's would
    // silently re-point the ledger thread and run the agent in the
    // wrong tree.
    vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(
      makeResult({
        existing_thread_id: "thread-existing",
        pane_id: "pane-ledger",
        cwd: "/projects/ledger",
      }),
    );
    vi.mocked(agentChatGetSession).mockResolvedValue(
      makeRecord({ cwd: "/projects/ledger" }),
    );

    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(
      makeSession({ existing_thread_id: "thread-existing" }),
    );

    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    const [startPaneId, , input] = vi.mocked(agentChatStartSession).mock
      .calls[0]!;
    expect((input as { cwd: string }).cwd).toBe("/projects/ledger");
    // The thread and the pane running it agree on the directory.
    expect(startPaneId).toBe("pane-ledger");
    // Local thread ids are minted per pane, so the id names the pane the
    // session actually runs in.
    expect((input as { thread_id: string }).thread_id).toMatch(
      /^chat-pane-ledger-/,
    );
    // This pane's own conversation is left alone.
    expect(vi.mocked(agentChatStopSession)).not.toHaveBeenCalled();
  });

  it("does not claim full history when the switched-to thread cannot hydrate", async () => {
    // The switch reuses the ordinary resume path, so it has to be as
    // honest as adoption is: a failed transcript read leaves the pane
    // blank, and a green "full history" toast over a blank transcript
    // is a lie.
    vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(
      makeResult({ existing_thread_id: "thread-existing" }),
    );
    vi.mocked(agentChatGetSession).mockResolvedValue(makeRecord());
    vi.mocked(agentChatListMessagesAfter).mockRejectedValue("db locked");

    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(makeSession());

    // The resume still happens — the agent keeps the server-side
    // context — but the user is told the transcript is missing.
    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.warning)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.warning).mock.calls[0]![0]).toContain(
      "transcript isn't shown here",
    );
  });

  it("reports an unopenable existing thread instead of starting anything", async () => {
    vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(
      makeResult({ existing_thread_id: "thread-existing" }),
    );
    vi.mocked(agentChatGetSession).mockResolvedValue(null);

    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(makeSession());

    expect(vi.mocked(agentChatStartSession)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.error)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled();
  });
});

describe("handleAdoptExternalSession — failure surfaces", () => {
  it("reports an adoption failure and never starts a session", async () => {
    vi.mocked(agentChatAdoptExternalSession).mockRejectedValue(
      "no workspace bound to pane",
    );
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(makeSession());

    expect(vi.mocked(agentChatStartSession)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.error)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.error).mock.calls[0]![0]).toContain(
      "no workspace bound to pane",
    );
  });

  it("warns instead of celebrating when the divider was not written", async () => {
    vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(
      makeResult({ resume_divider_written: false }),
    );
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(makeSession());

    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.warning)).toHaveBeenCalledTimes(1);
  });

  it("warns when the transcript could not be hydrated at all", async () => {
    vi.mocked(agentChatListMessagesAfter).mockRejectedValue("db locked");
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleAdoptExternalSession(makeSession());

    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.warning)).toHaveBeenCalledTimes(1);
  });

});

describe("handleAdoptExternalSession — another project's directory", () => {
  const foreignSession = () =>
    makeSession({ cwd: "/projects/ledger", same_repo: false });

  beforeEach(() => {
    vi.mocked(agentChatAdoptExternalSession).mockResolvedValue(
      makeResult({ cwd: "/projects/ledger", foreign_project: true }),
    );
  });

  it("asks first instead of re-pointing the pane on the click (R4)", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await act(async () => {
      await result.current.handleAdoptExternalSession(foreignSession());
    });

    // Nothing is minted and nothing moves until the user says yes.
    expect(vi.mocked(agentChatAdoptExternalSession)).not.toHaveBeenCalled();
    expect(vi.mocked(agentChatStartSession)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled();
    expect(result.current.foreignProjectPrompt).toMatchObject({
      cwd: "/projects/ledger",
    });
  });

  it("adopts in that directory once the user confirms", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await act(async () => {
      await result.current.handleAdoptExternalSession(foreignSession());
    });
    await act(async () => {
      await result.current.confirmForeignProjectAdopt();
    });

    expect(vi.mocked(agentChatAdoptExternalSession)).toHaveBeenCalledTimes(1);
    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0]!;
    expect((input as { cwd: string }).cwd).toBe("/projects/ledger");
    // The toast still names the directory the pane moved to.
    expect(vi.mocked(toast.success).mock.calls[0]![0]).toContain(
      "/projects/ledger",
    );
    expect(result.current.foreignProjectPrompt).toBeNull();
  });

  it("stays put when the confirmation is dismissed", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await act(async () => {
      await result.current.handleAdoptExternalSession(foreignSession());
    });
    act(() => result.current.dismissForeignProjectAdopt());

    expect(result.current.foreignProjectPrompt).toBeNull();
    expect(vi.mocked(agentChatAdoptExternalSession)).not.toHaveBeenCalled();
    expect(vi.mocked(agentChatStartSession)).not.toHaveBeenCalled();
  });

  it("gates too when only the adopt result knows the session is foreign", async () => {
    // Discovery said "this checkout", the backend disagrees. The thread
    // exists by then, but starting it would still move the pane, so the
    // confirmation runs before the launch — and confirming must not
    // adopt the same conversation a second time.
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await act(async () => {
      await result.current.handleAdoptExternalSession(makeSession());
    });

    expect(vi.mocked(agentChatAdoptExternalSession)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(agentChatStartSession)).not.toHaveBeenCalled();
    expect(result.current.foreignProjectPrompt).toMatchObject({
      cwd: "/projects/ledger",
    });

    await act(async () => {
      await result.current.confirmForeignProjectAdopt();
    });
    expect(vi.mocked(agentChatAdoptExternalSession)).toHaveBeenCalledTimes(1);
    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0]!;
    expect((input as { cwd: string }).cwd).toBe("/projects/ledger");
  });
});
