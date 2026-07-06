import { useState } from "react";
import { History, SplitSquareHorizontal, SplitSquareVertical, X } from "lucide-react";

import { SessionSelector } from "@/components/chat/SessionSelector";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { useAgentChatCheckpoint } from "@/hooks/use-agent-chat-checkpoint";
import { sessionDisplayTitle } from "@/lib/agent-chat/session-history";
import { countRunningSubagents } from "@/lib/agent-chat/subagents";
import { toast } from "@/lib/toast";
import { selectThread, useAgentChatStore } from "@/stores/agent-chat-store";
import { findWorkspaceIdForPane, useAppStore } from "@/stores/app-store";
import {
  agentChatListMessages,
  agentChatRestoreCheckpoint,
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

  // Run-start rollback checkpoint (issue #80). `checkpoint` stays null
  // when the opt-in setting is off / the snapshot hasn't landed, which
  // hides the restore affordance entirely.
  const checkpoint = useAgentChatCheckpoint(pane.thread_id ?? null);
  const turnActive = useAgentChatStore((s) => {
    const slice = pane.thread_id ? selectThread(pane.thread_id)(s) : null;
    return slice ? slice.activeTurnId != null || slice.streaming : false;
  });
  // Orchestrator-mode pane sub-header pill: how many subagents are live
  // in this thread right now. Store-driven off the reduced transcript
  // (backend-state-driven), so it reflects hydrate + live events alike.
  const runningSubagents = useAgentChatStore((s) => {
    const slice = pane.thread_id ? selectThread(pane.thread_id)(s) : null;
    return slice ? countRunningSubagents(slice.messages) : 0;
  });
  const [confirmRestoreOpen, setConfirmRestoreOpen] = useState(false);
  const [restoring, setRestoring] = useState(false);

  const handleRestoreConfirmed = async () => {
    if (!pane.thread_id) return;
    setRestoring(true);
    try {
      await agentChatRestoreCheckpoint(pane.thread_id);
      toast.success(
        "Workspace restored to the snapshot taken when this run started.",
      );
    } catch (error) {
      toast.error(`Restore failed: ${error}`);
    } finally {
      setRestoring(false);
      setConfirmRestoreOpen(false);
    }
  };

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
      // Hydrate the new slice with the picked session's persisted
      // transcript BEFORE we kick off the provider — that way the
      // pane renders the full history immediately, instead of going
      // blank for the second or two it takes the SDK to boot. We
      // hydrate against the picked thread id (where the messages
      // live in SQLite); on the upcoming `ResumeCursorUpdated`,
      // `collapse_duplicate_agent_chat_sessions` migrates those
      // rows over to the new thread id so future history-list calls
      // still see them.
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
      // up automatically. The replayed transcript is already in the
      // store under `newLocalThreadId`, so MessageList renders it
      // the moment the pane re-reads `pane.thread_id`.
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
        "flex h-7 shrink-0 items-center gap-1 border-b border-border/50 px-1.5 transition-colors",
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
      {/* "N subagents running" pill (design) — amber, blinking dot,
          always visible while any subagent in this thread is live. */}
      {runningSubagents > 0 && (
        <span
          className="mr-1 inline-flex shrink-0 items-center gap-1.5 text-[11px] font-semibold text-status-working"
          data-testid="subagents-running-pill"
        >
          <span
            className="cm-blink h-1.5 w-1.5 rounded-full bg-status-working"
            aria-hidden
          />
          {runningSubagents} subagent{runningSubagents === 1 ? "" : "s"} running
        </span>
      )}
      {/* Match the muted-at-rest / lift-on-hover dialect the rest of
          the app's pane headers landed in c11a3fc + e96619e: 28px hit
          target, 14px glyph, drop close to destructive-foreground when
          its red hover bg kicks in. */}
      <div className="flex items-center gap-0.5 opacity-0 transition-opacity duration-150 group-hover/pane:opacity-100">
        {checkpoint && (
          <Button
            variant="ghost"
            size="icon-sm"
            className="text-muted-foreground hover:text-foreground"
            onClick={() => setConfirmRestoreOpen(true)}
            // Restoring under a running agent would yank files out
            // from under its tools — stop the turn first.
            disabled={turnActive || restoring}
            aria-label="Restore checkpoint"
            title={
              turnActive
                ? "Stop the running turn before restoring"
                : "Restore workspace to before this run"
            }
            data-testid="restore-checkpoint-button"
          >
            <History className="h-3.5 w-3.5" />
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon-sm"
          className="text-muted-foreground hover:text-foreground"
          onClick={() => handleSplit("horizontal")}
          aria-label="Split right"
          title="Split right"
        >
          <SplitSquareHorizontal className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className="text-muted-foreground hover:text-foreground"
          onClick={() => handleSplit("vertical")}
          aria-label="Split down"
          title="Split down"
        >
          <SplitSquareVertical className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className="text-muted-foreground hover:bg-destructive/80 hover:text-destructive-foreground"
          onClick={handleClose}
          aria-label="Close pane"
          title="Close pane"
        >
          <X className="h-3.5 w-3.5" />
        </Button>
      </div>
      <AlertDialog open={confirmRestoreOpen} onOpenChange={setConfirmRestoreOpen}>
        <AlertDialogContent
          // The header's pointer-down handler starts drag-to-swap;
          // keep dialog interactions out of it.
          onPointerDown={(e) => e.stopPropagation()}
        >
          <AlertDialogHeader>
            <AlertDialogTitle>Restore workspace to before this run?</AlertDialogTitle>
            <AlertDialogDescription>
              Files go back to the snapshot taken when this chat session
              started{checkpoint?.branch ? ` (branch ${checkpoint.branch})` : ""}.
              Commits made during the run are undone, files the run created are
              deleted, and your pre-run changes come back as unstaged edits. A
              safety snapshot of the current state is kept under{" "}
              <code className="font-mono text-[11px]">refs/codemux/pre-restore</code>.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={restoring}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={restoring}
              onClick={(e) => {
                // Keep the dialog open while the restore runs; we
                // close it ourselves in the finally block.
                e.preventDefault();
                void handleRestoreConfirmed();
              }}
              data-testid="restore-checkpoint-confirm"
            >
              {restoring ? "Restoring…" : "Restore"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </header>
  );
}
