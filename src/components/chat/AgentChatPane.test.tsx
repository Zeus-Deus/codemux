/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";

let currentMessages: unknown[] = [];

vi.mock("./ChatHomeLanding", () => ({
  ChatHomeLanding: ({ composer }: { composer: React.ReactNode }) => (
    <div data-testid="home-landing">{composer}</div>
  ),
}));

vi.mock("./ChatTranscript", () => ({
  ChatTranscript: () => <div data-testid="transcript" />,
}));

vi.mock("./Composer", () => ({
  Composer: () => <div data-testid="composer" />,
}));

vi.mock("@/hooks/use-agent-chat-events", () => ({
  useAgentChatEvents: () => {},
}));

vi.mock("@/tauri/commands", () => ({
  agentChatInterruptTurn: vi.fn().mockResolvedValue(undefined),
  agentChatRespondToRequest: vi.fn().mockResolvedValue(undefined),
  agentChatSendTurn: vi.fn().mockResolvedValue(undefined),
  agentChatSetModel: vi.fn().mockResolvedValue(undefined),
  agentChatStartSession: vi.fn().mockResolvedValue("thread-new"),
  agentChatStopSession: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: Object.assign(
    vi.fn((selector) =>
      selector({
        appState: {
          active_workspace_id: "ws-home",
          workspaces: [
            {
              workspace_id: "ws-home",
              workspace_type: "home",
              cwd: "/home/user",
            },
          ],
        },
      }),
    ),
    {
      getState: () => ({
        appState: {
          active_workspace_id: "ws-home",
          workspaces: [
            {
              workspace_id: "ws-home",
              workspace_type: "home",
              cwd: "/home/user",
            },
          ],
        },
      }),
    },
  ),
}));

vi.mock("@/stores/agent-chat-store", () => {
  const mockStore = Object.assign(
    vi.fn((selector: (state: unknown) => unknown) => {
      const slice = {
        messages: currentMessages,
        inputDraft: "",
        streaming: false,
        activeTurnId: null,
        model: null,
        permissionMode: "bypassPermissions",
        sessionLaunchMode: "bypassPermissions",
        resumeCursor: null,
      };
      const state = {
        threads: { "thread-x": slice },
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
        threads: {
          "thread-x": {
            messages: currentMessages,
            model: null,
            permissionMode: "bypassPermissions",
            sessionLaunchMode: "bypassPermissions",
            resumeCursor: null,
          },
        },
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
