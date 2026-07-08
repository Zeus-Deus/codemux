import { useCallback } from "react";

import { sessionDisplayTitle } from "@/lib/agent-chat/session-history";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import {
  agentChatListMessages,
  agentChatStartSession,
  agentChatStopSession,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import type {
  AgentChatProviderKind,
  PaneNodeSnapshot,
} from "@/tauri/types";

type AgentChatPaneNode = Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;

export interface AgentChatSessionActions {
  /** Resolved working directory (pane cwd, or the active workspace's cwd). */
  cwd: string | null;
  /** Provider driving the pane (defaults to `claude`). */
  provider: AgentChatProviderKind;
  /** Resume a persisted session: stop the current session, hydrate the
   *  new local thread with the picked chat's transcript, then start the
   *  provider with the record's `sdk_session_id` as the resume cursor. */
  handleSelect: (record: AgentChatSessionRecord) => Promise<void>;
  /** Stop the current session and start a fresh one on the same pane. */
  handleNewChat: () => Promise<void>;
}

/**
 * Shared session-switch orchestration for an agent-chat pane. Extracted
 * from {@link AgentChatPaneHeader} so the per-pane header (split layouts)
 * and the merged title-bar chat tab (GUI chrome) drive resume / new-chat
 * through ONE implementation instead of forking the SDK wiring.
 *
 * The side effect (stop current → hydrate → start with `resume`) lives
 * here because the pane snapshot already carries every input
 * `agent_chat_start_session` needs (`provider`, `cwd`, `pane_id`,
 * `thread_id`). Callers just render a trigger + list around it.
 */
export function useAgentChatSessionActions(
  pane: AgentChatPaneNode,
): AgentChatSessionActions {
  const provider: AgentChatProviderKind = pane.provider ?? "claude";

  // Fall back to the active workspace's cwd when the pane snapshot
  // hasn't stamped its own (Home pane, pre-hydrate race).
  const fallbackCwd = useAppStore((s) => {
    if (!s.appState) return null;
    const active = s.appState.active_workspace_id;
    const ws = s.appState.workspaces.find((w) => w.workspace_id === active);
    return ws?.cwd ?? null;
  });
  const cwd = pane.cwd ?? fallbackCwd;

  const paneId = pane.pane_id;
  const threadId = pane.thread_id;

  const handleSelect = useCallback(
    async (record: AgentChatSessionRecord) => {
      if (!cwd) {
        toast.error("Cannot resume: no working directory.");
        return;
      }
      if (!record.sdk_session_id) {
        toast.warning(
          "This chat hasn't finished its first turn yet — can't resume.",
        );
        return;
      }
      try {
        if (threadId) {
          await agentChatStopSession(provider, threadId).catch(() => {
            // Non-fatal: a stale session may already be dead. Proceed
            // with the resume regardless.
          });
          // Clear the old slice so the transcript doesn't flash the
          // previous chat's messages while the resumed session boots.
          useAgentChatStore.getState().resetThread(threadId);
        }
        const newLocalThreadId = `chat-${paneId}-${Date.now()}`;
        // Hydrate the new slice with the picked session's persisted
        // transcript BEFORE we kick off the provider — that way the
        // pane renders the full history immediately, instead of going
        // blank for the second or two it takes the SDK to boot.
        try {
          const payloads = await agentChatListMessages(record.thread_id);
          if (payloads.length > 0) {
            useAgentChatStore
              .getState()
              .hydrateThread(newLocalThreadId, payloads);
          }
        } catch (err) {
          // Hydration failure is non-fatal — the SDK still has the
          // server-side context, the user just won't see the
          // historical transcript. Log so it's debuggable.
          console.warn("[agent-chat] hydrate on resume failed:", err);
        }
        await agentChatStartSession(paneId, provider, {
          thread_id: newLocalThreadId,
          cwd,
          model: null,
          resume_cursor: { resume: record.sdk_session_id },
          permission_mode: null,
          additional_directories: [],
          env: null,
        });
        toast.success(
          `Resumed "${sessionDisplayTitle(record)}" — agent has the full history`,
        );
      } catch (error) {
        toast.error(`Failed to reopen chat: ${error}`);
      }
    },
    [cwd, paneId, threadId, provider],
  );

  const handleNewChat = useCallback(async () => {
    if (!cwd) {
      toast.error("Cannot start a new chat: no working directory.");
      return;
    }
    try {
      if (threadId) {
        await agentChatStopSession(provider, threadId).catch(() => {});
        // Clear the old slice so the transcript doesn't flash the
        // previous chat's messages while the new session boots.
        useAgentChatStore.getState().resetThread(threadId);
      }
      const newLocalThreadId = `chat-${paneId}-${Date.now()}`;
      await agentChatStartSession(paneId, provider, {
        thread_id: newLocalThreadId,
        cwd,
        model: null,
        resume_cursor: null,
        permission_mode: null,
        additional_directories: [],
        env: null,
      });
    } catch (error) {
      toast.error(`Failed to start new chat: ${error}`);
    }
  }, [cwd, paneId, threadId, provider]);

  return { cwd, provider, handleSelect, handleNewChat };
}
