import { Plus, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import {
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { groupSessionsByDate, sessionDisplayTitle } from "@/lib/agent-chat/session-history";
import { cn } from "@/lib/utils";
import {
  agentChatDeleteSession,
  agentChatListSessions,
  agentChatStopSession,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

const LIST_LIMIT = 50;

export interface UseSessionHistoryArgs {
  /** Whether the surrounding menu is open — the fetch only fires while
   *  open so a closed dropdown never hits the backend. */
  open: boolean;
  workspaceId: string;
  cwd: string | null;
  /** Test seam — render the given sessions synchronously and skip the
   *  `agent_chat_list_sessions` invoke. */
  sessionsOverride?: AgentChatSessionRecord[];
  /** Test seam — skip the delete invoke. */
  onDeleteOverride?: (threadId: string) => void;
}

export interface UseSessionHistory {
  sessions: AgentChatSessionRecord[];
  loading: boolean;
  /** Optimistically drop a row and fire the delete (or the override). */
  handleDelete: (threadId: string) => void;
}

/**
 * Fetch + delete state for the session-history dropdown. Shared by the
 * legacy per-pane {@link SessionSelector} and the GUI-chrome title-bar
 * chat tab so both drive the same list off one implementation.
 */
export function useSessionHistory({
  open,
  workspaceId,
  cwd,
  sessionsOverride,
  onDeleteOverride,
}: UseSessionHistoryArgs): UseSessionHistory {
  const [sessions, setSessions] = useState<AgentChatSessionRecord[]>(
    sessionsOverride ?? [],
  );
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (sessionsOverride) {
      setSessions(sessionsOverride);
      return;
    }
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    agentChatListSessions(workspaceId, cwd, LIST_LIMIT)
      .then((rows) => {
        if (!cancelled) setSessions(rows);
      })
      .catch((err) => {
        console.warn("[agent-chat] failed to list sessions", err);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, workspaceId, cwd, sessionsOverride]);

  const handleDelete = (threadId: string) => {
    // Grab the row before the optimistic filter drops it — its `provider`
    // is what the terminate below has to be addressed to.
    const row = sessions.find((s) => s.thread_id === threadId);
    setSessions((prev) => prev.filter((s) => s.thread_id !== threadId));
    if (onDeleteOverride) {
      onDeleteOverride(threadId);
      return;
    }
    // Deleting the row IS the explicit terminate — the only one in the
    // product. Every other teardown path detaches, so without this a
    // deleted session's provider-side conversation would be orphaned
    // (for OpenCode, left on the server with no row pointing at it).
    // Idempotent on the backend: an unknown/already-dead thread is Ok.
    if (row) {
      agentChatStopSession(
        row.provider as AgentChatProviderKind,
        threadId,
      ).catch(() => {
        // Non-fatal: the transcript delete below is the user-visible part.
      });
    }
    agentChatDeleteSession(threadId).catch((err) => {
      console.warn("[agent-chat] failed to delete session", err);
    });
  };

  return { sessions, loading, handleDelete };
}

export interface SessionHistoryListProps {
  sessions: AgentChatSessionRecord[];
  loading: boolean;
  activeThreadId: string | null;
  onSelect: (record: AgentChatSessionRecord) => void;
  onNewChat: () => void;
  onDelete: (threadId: string) => void;
  /** Close the surrounding menu after an action. */
  closeMenu: () => void;
  /** Optional item (e.g. "Restore checkpoint") rendered at the top of the
   *  menu, just below the New Chat row and above the history separator.
   *  Provide a full `DropdownMenuItem` node. */
  footerItem?: React.ReactNode;
}

/**
 * The New Chat row + grouped session list. Renders INSIDE a
 * `DropdownMenuContent`, so both consumers wrap it in their own
 * `DropdownMenu`. One source of truth for the list markup + row
 * behavior (delete-on-hover, active highlight, resume-on-click).
 */
export function SessionHistoryList({
  sessions,
  loading,
  activeThreadId,
  onSelect,
  onNewChat,
  onDelete,
  closeMenu,
  footerItem,
}: SessionHistoryListProps) {
  const buckets = useMemo(() => groupSessionsByDate(sessions), [sessions]);

  return (
    <>
      <DropdownMenuItem
        onSelect={() => {
          onNewChat();
          closeMenu();
        }}
        data-testid="session-selector-new-chat"
      >
        <Plus className="h-3.5 w-3.5" />
        New Chat
      </DropdownMenuItem>
      {footerItem}
      <DropdownMenuSeparator />
      {loading && sessions.length === 0 && (
        <div className="px-2 py-4 text-center text-xs text-muted-foreground">
          Loading…
        </div>
      )}
      {!loading && sessions.length === 0 && (
        <div className="px-2 py-4 text-center text-xs text-muted-foreground">
          No previous chats
        </div>
      )}
      {buckets.map((bucket) => (
        <div key={bucket.key}>
          <DropdownMenuLabel className="text-[10px] uppercase tracking-wider text-muted-foreground/70">
            {bucket.label}
          </DropdownMenuLabel>
          {bucket.sessions.map((session) => (
            <SessionRow
              key={session.thread_id}
              session={session}
              isActive={session.thread_id === activeThreadId}
              onSelect={() => {
                if (session.thread_id === activeThreadId) {
                  closeMenu();
                  return;
                }
                onSelect(session);
                closeMenu();
              }}
              onDelete={() => onDelete(session.thread_id)}
            />
          ))}
        </div>
      ))}
    </>
  );
}

interface SessionRowProps {
  session: AgentChatSessionRecord;
  isActive: boolean;
  onSelect: () => void;
  onDelete: () => void;
}

function SessionRow({ session, isActive, onSelect, onDelete }: SessionRowProps) {
  return (
    <DropdownMenuItem
      onSelect={(e) => {
        // Prevent the trash-icon path from bubbling into a row select.
        if ((e.target as HTMLElement)?.closest("[data-session-delete]")) {
          e.preventDefault();
          return;
        }
        onSelect();
      }}
      className={cn(
        "group/row flex items-center justify-between gap-2 pr-1",
        // Active-row highlight stays neutral — accent is reserved for the
        // app shell, not in-pane surfaces.
        isActive && "bg-muted/70 text-foreground",
      )}
      data-testid={`session-row-${session.thread_id}`}
    >
      <span className="flex-1 truncate text-xs">
        {sessionDisplayTitle(session)}
      </span>
      <button
        type="button"
        aria-label="Delete chat"
        data-session-delete
        data-testid={`session-delete-${session.thread_id}`}
        className="rounded p-1 opacity-0 transition-opacity hover:bg-destructive/20 group-hover/row:opacity-100"
        onClick={(e) => {
          e.stopPropagation();
          e.preventDefault();
          onDelete();
        }}
      >
        <Trash2 className="h-3 w-3" />
      </button>
    </DropdownMenuItem>
  );
}
