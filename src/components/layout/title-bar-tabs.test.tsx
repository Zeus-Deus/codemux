/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import type {
  AgentChatSessionRecord,
} from "@/tauri/commands";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";

// ── Mocks ──
const mocks = vi.hoisted(() => ({
  listSessions: vi.fn(),
  deleteSession: vi.fn().mockResolvedValue(undefined),
  activateTab: vi.fn().mockResolvedValue(undefined),
  closeTab: vi.fn().mockResolvedValue(undefined),
  getCheckpoint: vi.fn().mockResolvedValue(null),
}));

vi.mock("@/tauri/commands", () => ({
  agentChatListSessions: (...a: unknown[]) => mocks.listSessions(...a),
  agentChatDeleteSession: (...a: unknown[]) => mocks.deleteSession(...a),
  agentChatGetCheckpoint: (...a: unknown[]) => mocks.getCheckpoint(...a),
  activateTab: (...a: unknown[]) => mocks.activateTab(...a),
  closeTab: (...a: unknown[]) => mocks.closeTab(...a),
  // Resume/new-chat SDK calls — unused in these tests but imported by the
  // shared session-actions hook.
  agentChatStartSession: vi.fn().mockResolvedValue("thread-new"),
  agentChatStopSession: vi.fn().mockResolvedValue(undefined),
  agentChatListMessages: vi.fn().mockResolvedValue([]),
}));

vi.mock("@/tauri/events", () => ({
  onAgentChatCheckpoint: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), success: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

import { TitleBarTabs } from "./title-bar-tabs";

function makeSession(
  overrides: Partial<AgentChatSessionRecord>,
): AgentChatSessionRecord {
  return {
    thread_id: "thread-x",
    sdk_session_id: "sdk-x",
    workspace_id: "ws-1",
    cwd: "/p",
    provider: "claude",
    title: null,
    created_at: "2026-04-24 12:00:00",
    last_active_at: "2026-04-24 12:00:00",
    model: null,
    effort: null,
    context_window: null,
    permission_mode: null,
    ...overrides,
  };
}

function chatPane(threadId: string | null): PaneNodeSnapshot {
  return {
    kind: "agent_chat",
    pane_id: "pane-chat",
    title: "Agent Chat",
    thread_id: threadId,
    provider: "claude",
    cwd: "/p",
  };
}

function makeWorkspace(
  root: PaneNodeSnapshot,
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
        tab_id: "tab-chat",
        kind: "terminal",
        title: "Agent Chat",
        surface_id: "surface-1",
        browser_id: null,
        icon: null,
      },
    ],
    active_tab_id: "tab-chat",
    active_surface_id: "surface-1",
    surfaces: [
      { surface_id: "surface-1", title: "s", active_pane_id: root.pane_id, root },
    ],
    ...overrides,
  };
}

beforeEach(() => {
  mocks.listSessions.mockReset();
  mocks.activateTab.mockClear();
  mocks.closeTab.mockClear();
  mocks.getCheckpoint.mockReset();
  mocks.getCheckpoint.mockResolvedValue(null);
});

afterEach(cleanup);

describe("TitleBarTabs", () => {
  it("renders the active chat tab with its history chevron trigger", () => {
    mocks.listSessions.mockResolvedValue([]);
    render(<TitleBarTabs workspace={makeWorkspace(chatPane("thread-1"))} />);
    expect(
      screen.getByTestId("titlebar-chat-tab-trigger"),
    ).toBeInTheDocument();
    expect(screen.getByText("Agent Chat")).toBeInTheDocument();
  });

  it("opens the history dropdown with grouped sessions + New Chat", async () => {
    const nowSpy = vi
      .spyOn(Date, "now")
      .mockReturnValue(new Date("2026-04-24T15:00:00Z").getTime());
    try {
      mocks.listSessions.mockResolvedValue([
        makeSession({
          thread_id: "t-today",
          title: "Today chat",
          last_active_at: "2026-04-24 10:00:00",
        }),
        makeSession({
          thread_id: "t-yday",
          title: "Yesterday chat",
          last_active_at: "2026-04-23 20:00:00",
        }),
      ]);
      render(<TitleBarTabs workspace={makeWorkspace(chatPane("t-today"))} />);
      act(() => {
        const trigger = screen.getByTestId("titlebar-chat-tab-trigger");
        fireEvent.pointerDown(trigger, { button: 0, pointerType: "mouse" });
        fireEvent.click(trigger);
      });

      await waitFor(() =>
        expect(screen.getByText("Today chat")).toBeInTheDocument(),
      );
      expect(screen.getByText("Today")).toBeInTheDocument();
      expect(screen.getByText("Yesterday")).toBeInTheDocument();
      expect(screen.getByText("Yesterday chat")).toBeInTheDocument();
      expect(
        screen.getByTestId("session-selector-new-chat"),
      ).toBeInTheDocument();
      expect(mocks.listSessions).toHaveBeenCalledWith("ws-1", "/p", 50);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("translates a vertical wheel delta into horizontal scroll once tabs overflow", () => {
    mocks.listSessions.mockResolvedValue([]);
    render(<TitleBarTabs workspace={makeWorkspace(chatPane("thread-1"))} />);
    const scroller = screen.getByTestId("titlebar-tabs-scroll");
    // jsdom reports 0 for both by default; stub an overflowing layout so
    // the guard (`scrollWidth <= clientWidth`) doesn't short-circuit —
    // mirrors the WebKit-webview quirk noted in the handler: a plain
    // vertical wheel doesn't natively scroll an `overflow-x: auto` box.
    Object.defineProperty(scroller, "scrollWidth", { value: 800, configurable: true });
    Object.defineProperty(scroller, "clientWidth", { value: 400, configurable: true });
    Object.defineProperty(scroller, "scrollLeft", { value: 0, writable: true, configurable: true });
    fireEvent.wheel(scroller, { deltaY: 120 });
    expect(scroller.scrollLeft).toBe(120);
  });

  it("activates an inactive tab on click", () => {
    mocks.listSessions.mockResolvedValue([]);
    const ws = makeWorkspace(chatPane("thread-1"), {
      tabs: [
        {
          tab_id: "tab-chat",
          kind: "terminal",
          title: "Agent Chat",
          surface_id: "surface-1",
          browser_id: null,
          icon: null,
        },
        {
          tab_id: "tab-term",
          kind: "terminal",
          title: "zsh",
          surface_id: "surface-2",
          browser_id: null,
          icon: null,
        },
      ],
      surfaces: [
        {
          surface_id: "surface-1",
          title: "s",
          active_pane_id: "pane-chat",
          root: chatPane("thread-1"),
        },
        {
          surface_id: "surface-2",
          title: "t",
          active_pane_id: "pane-term",
          root: {
            kind: "terminal",
            pane_id: "pane-term",
            session_id: "sess-2",
            title: "zsh",
          },
        },
      ],
    });
    render(<TitleBarTabs workspace={ws} />);
    act(() => {
      fireEvent.click(screen.getByText("zsh"));
    });
    expect(mocks.activateTab).toHaveBeenCalledWith("ws-1", "tab-term");
  });
});
