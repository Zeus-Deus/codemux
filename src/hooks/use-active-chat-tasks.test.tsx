import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { useAgentChatStore } from "@/stores/agent-chat-store";
import type { WorkspaceSnapshot } from "@/tauri/types";

import { useActiveChatTasks } from "./use-active-chat-tasks";

function workspaceWithFocus(activePaneId: string): WorkspaceSnapshot {
  return {
    active_surface_id: "surface-1",
    surfaces: [
      {
        surface_id: "surface-1",
        title: "Split chat",
        active_pane_id: activePaneId,
        root: {
          kind: "split",
          pane_id: "split-1",
          direction: "horizontal",
          child_sizes: [0.5, 0.5],
          children: [
            {
              kind: "agent_chat",
              pane_id: "chat-pane",
              title: "Agent Chat",
              thread_id: "thread-1",
              provider: "codex",
              cwd: "/repo",
            },
            {
              kind: "terminal",
              pane_id: "terminal-pane",
              session_id: "session-1",
              title: "Terminal",
            },
          ],
        },
      },
    ],
  } as WorkspaceSnapshot;
}

beforeEach(() => {
  useAgentChatStore.setState({ threads: {} });
  act(() => {
    const store = useAgentChatStore.getState();
    store.ensureThread("thread-1");
    store.applyEvent("thread-1", {
      type: "tasks_updated",
      thread_id: "thread-1",
      tasks: {
        explanation: null,
        tasks: [
          {
            task_id: "inspect",
            title: "Inspect the implementation",
            status: "in_progress",
            detail: null,
            blocked_by: [],
          },
        ],
      },
    });
  });
});

describe("useActiveChatTasks", () => {
  it("returns the task snapshot for the focused chat leaf", () => {
    const { result } = renderHook(() =>
      useActiveChatTasks(workspaceWithFocus("chat-pane")),
    );

    expect(result.current.threadId).toBe("thread-1");
    expect(result.current.tasks?.tasks[0]?.title).toBe(
      "Inspect the implementation",
    );
  });

  it("does not leak a sibling chat's tasks when focus moves to a terminal", () => {
    const { result } = renderHook(() =>
      useActiveChatTasks(workspaceWithFocus("terminal-pane")),
    );

    expect(result.current).toEqual({
      threadId: null,
      tasks: null,
      updatedAt: null,
      streaming: false,
    });
  });
});
