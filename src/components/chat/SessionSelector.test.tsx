/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";

import type { AgentChatSessionRecord } from "@/tauri/commands";

import { SessionSelector } from "./SessionSelector";

function makeSession(
  overrides: Partial<AgentChatSessionRecord>,
): AgentChatSessionRecord {
  return {
    thread_id: "thread-1",
    sdk_session_id: "sdk-1",
    workspace_id: "ws-1",
    cwd: "/p",
    provider: "claude",
    title: null,
    created_at: "2026-04-24 12:00:00",
    last_active_at: "2026-04-24 12:00:00",
    ...overrides,
  };
}

function openMenu() {
  const trigger = screen.getByTestId("session-selector-trigger");
  act(() => {
    fireEvent.pointerDown(trigger, {
      button: 0,
      pointerType: "mouse",
    });
    fireEvent.click(trigger);
  });
}

describe("SessionSelector", () => {
  afterEach(() => {
    cleanup();
  });

  it("renders 'History' on the trigger when no active session", () => {
    render(
      <SessionSelector
        workspaceId="ws-1"
        cwd="/p"
        activeThreadId={null}
        onSelect={vi.fn()}
        onNewChat={vi.fn()}
        sessionsOverride={[]}
      />,
    );
    expect(screen.getByTestId("session-selector-trigger")).toHaveTextContent(
      "History",
    );
  });

  it("renders the active session's title on the trigger", () => {
    render(
      <SessionSelector
        workspaceId="ws-1"
        cwd="/p"
        activeThreadId="thread-1"
        onSelect={vi.fn()}
        onNewChat={vi.fn()}
        sessionsOverride={[
          makeSession({ thread_id: "thread-1", title: "Refactor auth" }),
        ]}
      />,
    );
    expect(screen.getByTestId("session-selector-trigger")).toHaveTextContent(
      "Refactor auth",
    );
  });

  it("shows 'No previous chats' when the list is empty", () => {
    render(
      <SessionSelector
        workspaceId="ws-1"
        cwd="/p"
        activeThreadId={null}
        onSelect={vi.fn()}
        onNewChat={vi.fn()}
        sessionsOverride={[]}
      />,
    );
    openMenu();
    expect(screen.getByText("No previous chats")).toBeInTheDocument();
  });

  it("groups visible sessions into date buckets", () => {
    // The component uses Date.now(); pin it so the buckets are
    // deterministic.
    const nowSpy = vi
      .spyOn(Date, "now")
      .mockReturnValue(new Date("2026-04-24T15:00:00Z").getTime());
    try {
      render(
        <SessionSelector
          workspaceId="ws-1"
          cwd="/p"
          activeThreadId={null}
          onSelect={vi.fn()}
          onNewChat={vi.fn()}
          sessionsOverride={[
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
          ]}
        />,
      );
      openMenu();
      expect(screen.getByText("Today")).toBeInTheDocument();
      expect(screen.getByText("Yesterday")).toBeInTheDocument();
      expect(screen.getByText("Today chat")).toBeInTheDocument();
      expect(screen.getByText("Yesterday chat")).toBeInTheDocument();
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("fires onSelect with the picked session record", () => {
    const onSelect = vi.fn();
    const target = makeSession({
      thread_id: "t-pick",
      title: "Pick me",
    });
    render(
      <SessionSelector
        workspaceId="ws-1"
        cwd="/p"
        activeThreadId={null}
        onSelect={onSelect}
        onNewChat={vi.fn()}
        sessionsOverride={[target]}
      />,
    );
    openMenu();
    fireEvent.click(screen.getByText("Pick me"));
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect.mock.calls[0][0].thread_id).toBe("t-pick");
  });

  it("does NOT fire onSelect when the user clicks the already-active session", () => {
    const onSelect = vi.fn();
    render(
      <SessionSelector
        workspaceId="ws-1"
        cwd="/p"
        activeThreadId="t-active"
        onSelect={onSelect}
        onNewChat={vi.fn()}
        sessionsOverride={[
          makeSession({ thread_id: "t-active", title: "Active" }),
        ]}
      />,
    );
    openMenu();
    // Click the row (not the trigger — the trigger also shows
    // "Active" since it mirrors the active session's title).
    fireEvent.click(screen.getByTestId("session-row-t-active"));
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("fires onNewChat when the New Chat footer is clicked", () => {
    const onNewChat = vi.fn();
    render(
      <SessionSelector
        workspaceId="ws-1"
        cwd="/p"
        activeThreadId={null}
        onSelect={vi.fn()}
        onNewChat={onNewChat}
        sessionsOverride={[]}
      />,
    );
    openMenu();
    fireEvent.click(screen.getByTestId("session-selector-new-chat"));
    expect(onNewChat).toHaveBeenCalledTimes(1);
  });

  it("deletes a row without firing onSelect", () => {
    const onSelect = vi.fn();
    const onDelete = vi.fn();
    render(
      <SessionSelector
        workspaceId="ws-1"
        cwd="/p"
        activeThreadId={null}
        onSelect={onSelect}
        onNewChat={vi.fn()}
        sessionsOverride={[
          makeSession({ thread_id: "t-del", title: "Delete me" }),
        ]}
        onDeleteOverride={onDelete}
      />,
    );
    openMenu();
    fireEvent.click(screen.getByTestId("session-delete-t-del"));
    expect(onDelete).toHaveBeenCalledWith("t-del");
    expect(onSelect).not.toHaveBeenCalled();
  });
});
