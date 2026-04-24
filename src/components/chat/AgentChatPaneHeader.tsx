import { SplitSquareHorizontal, SplitSquareVertical, X } from "lucide-react";

import { SessionSelector } from "@/components/chat/SessionSelector";
import { Button } from "@/components/ui/button";
import { sessionDisplayTitle } from "@/lib/agent-chat/session-history";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { findWorkspaceIdForPane, useAppStore } from "@/stores/app-store";
import {
  agentChatStartSession,
  agentChatStopSession,
  closePane,
  splitPane,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import type {
  AgentChatProviderKind,
  PaneNodeSnapshot,
} from "@/tauri/types";
import { cn } from "@/lib/utils";

type AgentChatPaneNode = Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;

interface Props {
  pane: AgentChatPaneNode;
  isActive: boolean;
  onPointerDown: (event: React.PointerEvent) => void;
}

/**
 * Per-pane chrome for the agent-chat surface. Mirrors the terminal /
 * browser header (28-ish px, hover-reveal right-side controls) but
 * replaces the static title with the {@link SessionSelector}
 * dropdown so the user can reopen past chats.
 *
 * The session-switching side effect (stop current → start a new
 * session with the picked chat's `sdk_session_id` passed through as
 * `resume`) lives here because the pane snapshot already carries
 * every input the start_session command needs (`provider`, `cwd`,
 * `pane_id`, current `thread_id`). That keeps PaneNode agnostic of
 * provider wiring.
 */
export function AgentChatPaneHeader({ pane, isActive, onPointerDown }: Props) {
  const provider: AgentChatProviderKind = pane.provider ?? "claude";
  const workspaceId = useAppStore((s) =>
    findWorkspaceIdForPane(s, pane.pane_id),
  );
  const fallbackCwd = useAppStore((s) => {
    if (!s.appState) return null;
    const active = s.appState.active_workspace_id;
    const ws = s.appState.workspaces.find((w) => w.workspace_id === active);
    return ws?.cwd ?? null;
  });
  const cwd = pane.cwd ?? fallbackCwd;

  const handleSelect = async (record: AgentChatSessionRecord) => {
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
      if (pane.thread_id) {
        await agentChatStopSession(provider, pane.thread_id).catch(() => {
          // Non-fatal: a stale session may already be dead. Proceed
          // with the resume regardless.
        });
        // Clear the old slice so the transcript doesn't flash the
        // previous chat's messages while the resumed session boots.
        // Matches the handleNewChat pattern.
        useAgentChatStore.getState().resetThread(pane.thread_id);
      }
      const newLocalThreadId = `chat-${pane.pane_id}-${Date.now()}`;
      await agentChatStartSession(pane.pane_id, provider, {
        thread_id: newLocalThreadId,
        cwd,
        model: null,
        resume_cursor: { resume: record.sdk_session_id },
        permission_mode: null,
        additional_directories: [],
        env: null,
      });
      // `agent_chat_start_session` writes the new thread id back onto
      // the pane snapshot; AgentChatPane's prop-sync effect picks it
      // up automatically.
      //
      // The SDK restores context server-side (the model will
      // remember the prior conversation) but does NOT replay the
      // transcript locally — so the pane goes empty. Surface a
      // toast so the user has visible confirmation the resume
      // actually took effect.
      toast.success(
        `Resumed "${sessionDisplayTitle(record)}" — agent has the full history`,
      );
    } catch (error) {
      toast.error(`Failed to reopen chat: ${error}`);
    }
  };

  const handleNewChat = async () => {
    if (!cwd) {
      toast.error("Cannot start a new chat: no working directory.");
      return;
    }
    try {
      if (pane.thread_id) {
        await agentChatStopSession(provider, pane.thread_id).catch(() => {});
      }
      // Clear the old slice so the transcript doesn't flash the
      // previous chat's messages while the new session boots.
      if (pane.thread_id) {
        useAgentChatStore.getState().resetThread(pane.thread_id);
      }
      const newLocalThreadId = `chat-${pane.pane_id}-${Date.now()}`;
      await agentChatStartSession(pane.pane_id, provider, {
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
  };

  const handleSplit = (direction: "horizontal" | "vertical") => {
    splitPane(pane.pane_id, direction).catch(console.error);
  };
  const handleClose = () => {
    closePane(pane.pane_id).catch(console.error);
  };

  return (
    <header
      className={cn(
        "flex h-7 shrink-0 items-center gap-1 border-b border-border/30 px-1.5 transition-colors",
        isActive ? "bg-card" : "bg-background",
      )}
      onPointerDown={onPointerDown}
      data-testid="agent-chat-pane-header"
    >
      <div
        className="flex-1 min-w-0"
        // Prevent the SessionSelector trigger from initiating
        // drag-to-swap — otherwise the dropdown never opens.
        onPointerDown={(e) => e.stopPropagation()}
      >
        {workspaceId ? (
          <SessionSelector
            workspaceId={workspaceId}
            cwd={cwd}
            activeThreadId={pane.thread_id}
            onSelect={handleSelect}
            onNewChat={handleNewChat}
          />
        ) : (
          <span className="px-1.5 text-xs text-muted-foreground">Agent Chat</span>
        )}
      </div>
      <div className="flex items-center gap-0.5 opacity-0 transition-opacity duration-150 group-hover/pane:opacity-100">
        <Button
          variant="ghost"
          size="icon-xs"
          onClick={() => handleSplit("horizontal")}
          aria-label="Split right"
          title="Split right"
        >
          <SplitSquareHorizontal className="h-3 w-3" />
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          onClick={() => handleSplit("vertical")}
          aria-label="Split down"
          title="Split down"
        >
          <SplitSquareVertical className="h-3 w-3" />
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          className="hover:bg-destructive/80"
          onClick={handleClose}
          aria-label="Close pane"
          title="Close pane"
        >
          <X className="h-3 w-3" />
        </Button>
      </div>
    </header>
  );
}
