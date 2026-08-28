/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import type {
  AgentChatProviderKind,
  PaneNodeSnapshot,
} from "@/tauri/types";

// ── Module mocks ──
//
// We mock every Tauri command the header touches so the component can
// run under jsdom without a real backend. Each spy is observable from
// the tests via `vi.mocked(...)`.

vi.mock("@/tauri/commands", () => ({
  agentChatListMessages: vi.fn().mockResolvedValue([]),
  agentChatListMessagesAfter: vi.fn().mockResolvedValue([]),
  agentChatStartSession: vi.fn().mockResolvedValue("thread-new"),
  agentChatStopSession: vi.fn().mockResolvedValue(undefined),
  agentChatDetachSession: vi.fn().mockResolvedValue(undefined),
  closePane: vi.fn().mockResolvedValue(undefined),
  splitPane: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

// Stub SessionSelector so the test focuses on the resume-flow
// orchestration in handleSelect, not the dropdown's internals.
type SessionSelectorStubProps = {
  workspaceId: string;
  cwd: string | null;
  activeThreadId: string | null;
  onSelect: (record: import("@/tauri/commands").AgentChatSessionRecord) => void;
  onNewChat: () => void;
};
const lastSessionSelectorProps: { current: SessionSelectorStubProps | null } = {
  current: null,
};
vi.mock("@/components/chat/SessionSelector", () => ({
  SessionSelector: (props: SessionSelectorStubProps) => {
    lastSessionSelectorProps.current = props;
    return <button data-testid="session-selector-stub">stub</button>;
  },
}));

// app-store hooks: workspaceId resolution + fallback cwd. Both need to
// return something stable so the SessionSelector branch renders.
vi.mock("@/stores/app-store", async () => {
  const actual = await vi.importActual<typeof import("@/stores/app-store")>(
    "@/stores/app-store",
  );
  return {
    ...actual,
    findWorkspaceIdForPane: () => "ws-1",
    useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
      selector({
        appState: {
          active_workspace_id: "ws-1",
          workspaces: [
            {
              workspace_id: "ws-1",
              cwd: "/projects/foo",
            },
          ],
        },
      }),
    ),
  };
});

import { AgentChatPaneHeader } from "./AgentChatPaneHeader";
import {
  agentChatListMessages,
  agentChatListMessagesAfter,
  agentChatStartSession,
  agentChatDetachSession,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";

afterEach(() => {
  cleanup();
  lastSessionSelectorProps.current = null;
});

function makeAgentChatPane(): Extract<PaneNodeSnapshot, { kind: "agent_chat" }> {
  return {
    kind: "agent_chat",
    pane_id: "pane-1",
    provider: "claude" as AgentChatProviderKind,
    cwd: "/projects/foo",
    thread_id: "thread-old",
  } as Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;
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

function renderHeader() {
  const pane = makeAgentChatPane();
  return render(
    <AgentChatPaneHeader
      pane={pane}
      isActive
      onPointerDown={() => {}}
    />,
  );
}

describe("AgentChatPaneHeader — resume hydration", () => {
  beforeEach(() => {
    useAgentChatStore.setState({ threads: {} });
    vi.mocked(agentChatListMessages).mockReset();
    vi.mocked(agentChatListMessagesAfter).mockReset();
    vi.mocked(agentChatStartSession).mockReset();
    vi.mocked(agentChatDetachSession).mockReset();
    vi.mocked(toast.error).mockReset();
    vi.mocked(toast.success).mockReset();
    vi.mocked(toast.warning).mockReset();
    vi.mocked(agentChatListMessages).mockResolvedValue([]);
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue([]);
    vi.mocked(agentChatStartSession).mockResolvedValue("thread-new");
    vi.mocked(agentChatDetachSession).mockResolvedValue(undefined);
  });

  it("reads the picked record's transcript BEFORE start_session", async () => {
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue([
      {
        id: 1,
        payload: JSON.stringify({ type: "user_message", thread_id: "x", text: "hi" }),
      },
    ]);
    renderHeader();
    expect(lastSessionSelectorProps.current).not.toBeNull();
    const onSelect = lastSessionSelectorProps.current!.onSelect;
    await onSelect(makeRecord({ thread_id: "thread-source" }));

    // Both calls happened. The resume path reads by cursor (from the
    // start of the source thread) so the new slice inherits a cursor.
    expect(vi.mocked(agentChatListMessagesAfter)).toHaveBeenCalledWith(
      "thread-source",
      null,
    );
    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    // Ordering: list_messages was called before start_session. We
    // can't compare invocation order via mock.calls indexes alone
    // (different mocks), so use invocation-call-order via the
    // .mock.invocationCallOrder array Vitest exposes.
    const listOrder =
      vi.mocked(agentChatListMessagesAfter).mock.invocationCallOrder[0];
    const startOrder =
      vi.mocked(agentChatStartSession).mock.invocationCallOrder[0];
    expect(listOrder).toBeLessThan(startOrder);
  });

  it("hydrates the new local thread id with the loaded payloads", async () => {
    const payload = JSON.stringify({
      type: "user_message",
      thread_id: "x",
      text: "hello from history",
    });
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue([
      { id: 7, payload },
    ]);

    renderHeader();
    const onSelect = lastSessionSelectorProps.current!.onSelect;
    await onSelect(makeRecord());

    // start_session was invoked with a freshly-minted local thread id.
    const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0];
    const newThreadId = (input as { thread_id: string }).thread_id;
    expect(newThreadId).toMatch(/^chat-pane-1-\d+$/);

    // The slice for that thread id now carries the replayed user msg.
    const slice = useAgentChatStore.getState().threads[newThreadId];
    expect(slice).toBeDefined();
    expect(slice.messages).toHaveLength(1);
    if (slice.messages[0].kind === "user_message") {
      expect(slice.messages[0].text).toBe("hello from history");
    }
    // …and a resume cursor, so its first remount is a warm tail read.
    expect(slice.lastPersistedEventId).toBe(7);
  });

  it("does NOT hydrate when the picked record has no payloads", async () => {
    vi.mocked(agentChatListMessagesAfter).mockResolvedValue([]);
    renderHeader();
    await lastSessionSelectorProps.current!.onSelect(makeRecord());

    // With no payloads the transcript hydrate is skipped, but the
    // post-start config seed still creates the slice (so the footer
    // pickers render). The mock echoes the returned thread id
    // ("thread-new"); assert that slice exists but carries NO replayed
    // messages — the hydrate skip is what we're proving here.
    const threads = useAgentChatStore.getState().threads;
    const seeded = threads["thread-new"];
    expect(seeded).toBeDefined();
    expect(seeded.messages).toHaveLength(0);
  });

  it("continues with start_session when the transcript read rejects", async () => {
    // Hydration is best-effort. A failed list call must NOT skip the
    // resume — the SDK still has server-side context, the user just
    // won't see the local transcript.
    vi.mocked(agentChatListMessagesAfter).mockRejectedValue(
      new Error("db locked"),
    );
    renderHeader();
    await lastSessionSelectorProps.current!.onSelect(makeRecord());
    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
    // No error toast — the warning is logged to console only.
    expect(vi.mocked(toast.error)).not.toHaveBeenCalled();
  });

  it("surfaces an error toast when start_session itself fails", async () => {
    vi.mocked(agentChatStartSession).mockRejectedValue(
      new Error("provider down"),
    );
    renderHeader();
    await lastSessionSelectorProps.current!.onSelect(makeRecord());
    expect(vi.mocked(toast.error)).toHaveBeenCalled();
  });

  it("warns and bails when the picked record has no sdk_session_id", async () => {
    // The dropdown filters these out at the SQL layer, but defend
    // against drift: a row without an sdk_session_id can't be
    // resumed, so we must NOT call list_messages or start_session.
    renderHeader();
    await lastSessionSelectorProps.current!.onSelect(
      makeRecord({ sdk_session_id: null }),
    );
    expect(vi.mocked(agentChatListMessagesAfter)).not.toHaveBeenCalled();
    expect(vi.mocked(agentChatStartSession)).not.toHaveBeenCalled();
    expect(vi.mocked(toast.warning)).toHaveBeenCalled();
  });

  it("detaches the existing live session before resuming", async () => {
    renderHeader();
    await lastSessionSelectorProps.current!.onSelect(makeRecord());
    // DETACH, not stop: the session being swapped away from stays in the
    // history dropdown and must remain resumable.
    expect(vi.mocked(agentChatDetachSession)).toHaveBeenCalledWith(
      "claude",
      "thread-old",
    );
    // Detach before start.
    const detachOrder =
      vi.mocked(agentChatDetachSession).mock.invocationCallOrder[0];
    const startOrder =
      vi.mocked(agentChatStartSession).mock.invocationCallOrder[0];
    expect(detachOrder).toBeLessThan(startOrder);
  });

  it("succeeds when detach_session rejects (stale dead session)", async () => {
    // Common case: the user is resuming AFTER the prior session
    // already crashed. The detach call rejects, we proceed anyway.
    vi.mocked(agentChatDetachSession).mockRejectedValueOnce(
      new Error("session_not_found"),
    );
    renderHeader();
    await lastSessionSelectorProps.current!.onSelect(makeRecord());
    expect(vi.mocked(agentChatStartSession)).toHaveBeenCalledTimes(1);
  });
});
