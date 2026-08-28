/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, renderHook } from "@testing-library/react";

import type {
  AgentChatProviderKind,
  PaneNodeSnapshot,
} from "@/tauri/types";

// ── Module mocks ──
//
// Mock every Tauri command the hook touches so it runs under jsdom
// without a real backend. `agent_chat_start_session` echoes the input
// thread_id — the real backend returns `session.thread_id`, which is
// minted from the local thread id the caller passes — so the store
// slice we seed lands on the same thread the transcript hydrates into.

vi.mock("@/tauri/commands", () => ({
  agentChatListMessages: vi.fn().mockResolvedValue([]),
  agentChatStartSession: vi.fn(
    (
      _paneId: string,
      _provider: AgentChatProviderKind,
      input: { thread_id: string },
    ) => Promise.resolve(input.thread_id),
  ),
  agentChatStopSession: vi.fn().mockResolvedValue(undefined),
  agentChatDetachSession: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

// The pane snapshot carries its own cwd, so the fallback branch never
// fires; still, stub the store hook so the selector doesn't reach into
// a real (empty) app state.
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
import {
  agentChatListMessages,
  agentChatStartSession,
  agentChatDetachSession,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import {
  DEFAULT_THREAD_PERMISSION_MODE,
  useAgentChatStore,
} from "@/stores/agent-chat-store";

type AgentChatPane = Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;

// `defaultModelForProvider` falls back to this when the provider
// capabilities store hasn't hydrated (its behaviour under jsdom).
const CLAUDE_DEFAULT_MODEL = "claude-opus-4-8";

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

function makeRecord(
  overrides: Partial<AgentChatSessionRecord> = {},
): AgentChatSessionRecord {
  return {
    thread_id: "thread-resume-source",
    sdk_session_id: "sdk-uuid",
    workspace_id: "ws-1",
    cwd: "/projects/foo",
    provider: "claude",
    title: "Old chat",
    created_at: "2026-04-24 12:00:00",
    last_active_at: "2026-04-24 12:00:00",
    model: null,
    effort: null,
    context_window: null,
    permission_mode: null,
    ...overrides,
  };
}

/** Read back the single slice a start-session flow created (the mock
 *  echoes the input thread id, so exactly one non-`thread-old` slice
 *  exists after a flow). */
function newestSlice() {
  const threads = useAgentChatStore.getState().threads;
  const key = Object.keys(threads).find((k) => k !== "thread-old");
  return key ? threads[key] : undefined;
}

beforeEach(() => {
  useAgentChatStore.setState({ threads: {} });
  vi.mocked(agentChatListMessages).mockClear();
  vi.mocked(agentChatStartSession).mockClear();
  vi.mocked(agentChatDetachSession).mockClear();
  vi.mocked(agentChatListMessages).mockResolvedValue([]);
  vi.mocked(agentChatDetachSession).mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
});

describe("useAgentChatSessionActions — handleNewChat", () => {
  it("starts the session in bypassPermissions, never null", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleNewChat();

    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0];
    expect((input as { permission_mode: string | null }).permission_mode).toBe(
      "bypassPermissions",
    );
  });

  it("starts a Codex New Chat in Codex Full access", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane({ provider: "codex" })),
    );
    await result.current.handleNewChat();

    const [, provider, input] = vi.mocked(agentChatStartSession).mock.calls[0];
    expect(provider).toBe("codex");
    expect((input as { permission_mode: string | null }).permission_mode).toBe(
      "danger-full-access",
    );
    const slice = newestSlice();
    expect(slice!.permissionMode).toBe("danger-full-access");
    expect(slice!.sessionLaunchMode).toBe("danger-full-access");
  });

  it("seeds the slice with matching permissionMode and sessionLaunchMode", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleNewChat();

    const slice = newestSlice();
    expect(slice).toBeDefined();
    expect(slice!.permissionMode).toBe("bypassPermissions");
    expect(slice!.sessionLaunchMode).toBe("bypassPermissions");
    // The two MUST agree — a mismatch is read as a user mode change and
    // triggers a spurious silent restart.
    expect(slice!.permissionMode).toBe(slice!.sessionLaunchMode);
    expect(slice!.model).toBe(CLAUDE_DEFAULT_MODEL);
  });

  it("keeps OpenCode's provider-native launch mode null", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane({ provider: "opencode" })),
    );
    await result.current.handleNewChat();

    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0];
    expect((input as { permission_mode: string | null }).permission_mode).toBeNull();
    const slice = newestSlice();
    expect(slice!.permissionMode).toBe(DEFAULT_THREAD_PERMISSION_MODE);
    expect(slice!.sessionLaunchMode).toBeNull();
  });
});

describe("useAgentChatSessionActions — handleSelect (resume)", () => {
  it("passes the record's persisted config through to start_session", async () => {
    const record = makeRecord({
      model: "claude-sonnet-4-6",
      effort: "high",
      context_window: "1m",
      permission_mode: "acceptEdits",
    });
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleSelect(record);

    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0];
    const typed = input as {
      model: string | null;
      effort?: string | null;
      context_window?: string | null;
      permission_mode: string | null;
    };
    expect(typed.model).toBe("claude-sonnet-4-6");
    expect(typed.effort).toBe("high");
    expect(typed.context_window).toBe("1m");
    expect(typed.permission_mode).toBe("acceptEdits");

    const slice = newestSlice();
    expect(slice!.model).toBe("claude-sonnet-4-6");
    expect(slice!.effort).toBe("high");
    expect(slice!.contextWindow).toBe("1m");
    expect(slice!.permissionMode).toBe("acceptEdits");
    expect(slice!.sessionLaunchMode).toBe("acceptEdits");
  });

  it("heals a NULL record.permission_mode to bypassPermissions", async () => {
    const record = makeRecord({ permission_mode: null, model: null });
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleSelect(record);

    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0];
    expect((input as { permission_mode: string | null }).permission_mode).toBe(
      "bypassPermissions",
    );
    // Model stayed null on the wire (backend heals it), but the store
    // seeds the resolved provider default so the picker renders.
    expect((input as { model: string | null }).model).toBeNull();

    const slice = newestSlice();
    expect(slice!.permissionMode).toBe("bypassPermissions");
    expect(slice!.sessionLaunchMode).toBe("bypassPermissions");
    expect(slice!.permissionMode).toBe(slice!.sessionLaunchMode);
    expect(slice!.model).toBe(CLAUDE_DEFAULT_MODEL);
  });

  it("heals a NULL Codex record to Codex Full access", async () => {
    const record = makeRecord({
      provider: "codex",
      permission_mode: null,
      model: null,
    });
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane({ provider: "codex" })),
    );
    await result.current.handleSelect(record);

    const [, provider, input] = vi.mocked(agentChatStartSession).mock.calls[0];
    expect(provider).toBe("codex");
    expect((input as { permission_mode: string | null }).permission_mode).toBe(
      "danger-full-access",
    );
    const slice = newestSlice();
    expect(slice!.permissionMode).toBe("danger-full-access");
    expect(slice!.sessionLaunchMode).toBe("danger-full-access");
  });

  it("resumes with the record's sdk_session_id as the cursor", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane()),
    );
    await result.current.handleSelect(makeRecord({ sdk_session_id: "sdk-42" }));
    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0];
    expect((input as { resume_cursor: unknown }).resume_cursor).toEqual({
      resume: "sdk-42",
    });
  });

  it("does not reuse a stale permission token when resuming OpenCode", async () => {
    const { result } = renderHook(() =>
      useAgentChatSessionActions(makePane({ provider: "opencode" })),
    );
    await result.current.handleSelect(
      makeRecord({ provider: "opencode", permission_mode: "bypassPermissions" }),
    );

    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0];
    expect((input as { permission_mode: string | null }).permission_mode).toBeNull();
    expect(newestSlice()!.sessionLaunchMode).toBeNull();
  });
});
