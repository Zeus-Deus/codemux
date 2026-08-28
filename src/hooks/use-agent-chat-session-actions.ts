import { useCallback } from "react";

import { defaultModelForProvider } from "@/components/chat/pickers/ModelPicker";
import { defaultPermissionModeForProvider } from "@/lib/agent-chat/capability-defaults";
import { sessionDisplayTitle } from "@/lib/agent-chat/session-history";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import {
  agentChatListMessagesAfter,
  agentChatStartSession,
  agentChatDetachSession,
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
   *  provider with the record's `sdk_session_id` as the resume cursor.
   *  The record's persisted picker config (model / effort / context /
   *  permission mode) rides through to the launch so the resumed session
   *  runs in the same mode the user last chose — a NULL `permission_mode`
   *  heals to the provider default (matching what the footer pill shows
   *  for NULL rows). */
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
          // Detach, not stop: the session being swapped away from goes
          // back to the history dropdown and must stay resumable.
          await agentChatDetachSession(provider, threadId).catch(() => {
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
          // Cursor read, single replay: the rows carry their durable ids,
          // so the new slice starts with a resume cursor instead of
          // having to cold-replay again on its first remount. Row ids are
          // table-wide monotonic, so every row this thread writes from
          // here on sorts above the resumed history's head.
          const rows = await agentChatListMessagesAfter(record.thread_id, null);
          if (rows.length > 0) {
            useAgentChatStore
              .getState()
              .hydrateThread(newLocalThreadId, rows, { provider });
          }
        } catch (err) {
          // Hydration failure is non-fatal — the SDK still has the
          // server-side context, the user just won't see the
          // historical transcript. Log so it's debuggable.
          console.warn("[agent-chat] hydrate on resume failed:", err);
        }
        // Resolve the launch config from the record's persisted per-thread
        // columns (all nullable). A NULL `permission_mode` heals to the
        // provider default: the footer pill renders that default for NULL
        // rows, so the session MUST actually launch in it — otherwise the
        // provider boots in `default` (prompt-for-every-tool) while the UI
        // advertises "Full access", the exact drift this hook exists to
        // prevent. Model falls back to the provider default the same way
        // the pane's on-mount seed effect does; effort/context ride through
        // as-is (null means "use the model default").
        const resolvedModel = record.model ?? defaultModelForProvider(provider);
        // OpenCode has no chat-side permission picker: launch with null even
        // if the record carries a stale cross-provider token.
        const resolvedMode =
          provider === "opencode"
            ? null
            : (record.permission_mode ??
              defaultPermissionModeForProvider(provider));
        const newThreadId = await agentChatStartSession(paneId, provider, {
          thread_id: newLocalThreadId,
          cwd,
          model: record.model,
          resume_cursor: { resume: record.sdk_session_id },
          permission_mode: resolvedMode,
          effort: record.effort,
          context_window: record.context_window,
          fast_mode: record.fast_mode ?? false,
          additional_directories: [],
          env: null,
        });
        // Seed the store slice for the freshly-started thread so the footer
        // pickers reflect the launched config. `permissionMode` and
        // `sessionLaunchMode` MUST agree — a mismatch is read as "the user
        // changed the mode" and triggers a spurious silent restart.
        const store = useAgentChatStore.getState();
        store.ensureThread(newThreadId);
        store.setModel(newThreadId, resolvedModel);
        store.setEffort(newThreadId, record.effort);
        store.setContextWindow(newThreadId, record.context_window);
        store.setFastMode(newThreadId, record.fast_mode ?? false);
        if (resolvedMode !== null) {
          store.setPermissionMode(newThreadId, resolvedMode);
        }
        store.setSessionLaunchMode(newThreadId, resolvedMode);
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
        // Detach, not stop: the chat being abandoned stays listed in the
        // history dropdown, so it must stay resumable.
        await agentChatDetachSession(provider, threadId).catch(() => {});
        // Clear the old slice so the transcript doesn't flash the
        // previous chat's messages while the new session boots.
        useAgentChatStore.getState().resetThread(threadId);
      }
      const newLocalThreadId = `chat-${paneId}-${Date.now()}`;
      // Launch in the provider default mode (Full access) — the same mode
      // the fresh store slice advertises in the footer pill. Passing `null`
      // here would boot the provider in `default` (prompt-for-every-tool)
      // while the pill still reads "Full access", the drift this hook exists
      // to close.
      const startMode = defaultPermissionModeForProvider(provider);
      const startModel = defaultModelForProvider(provider);
      const newThreadId = await agentChatStartSession(paneId, provider, {
        thread_id: newLocalThreadId,
        cwd,
        model: null,
        resume_cursor: null,
        permission_mode: startMode,
        fast_mode: false,
        additional_directories: [],
        env: null,
      });
      // Seed the new slice the same way the pane's fresh-boot path does, so
      // the pickers render immediately and `permissionMode` /
      // `sessionLaunchMode` agree (a mismatch is read as a user mode change
      // and triggers a spurious silent restart).
      const store = useAgentChatStore.getState();
      store.ensureThread(newThreadId);
      store.setModel(newThreadId, startModel);
      if (startMode !== null) {
        store.setPermissionMode(newThreadId, startMode);
      }
      store.setFastMode(newThreadId, false);
      store.setSessionLaunchMode(newThreadId, startMode);
    } catch (error) {
      toast.error(`Failed to start new chat: ${error}`);
    }
  }, [cwd, paneId, threadId, provider]);

  return { cwd, provider, handleSelect, handleNewChat };
}
