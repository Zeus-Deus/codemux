import { ChevronDown, Plus, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  groupSessionsByDate,
  sessionDisplayTitle,
} from "@/lib/agent-chat/session-history";
import { cn } from "@/lib/utils";
import {
  agentChatDeleteSession,
  agentChatListSessions,
  type AgentChatSessionRecord,
} from "@/tauri/commands";

export interface SessionSelectorProps {
  /** Workspace the dropdown is scoped to. */
  workspaceId: string;
  /** Cwd of the pane. `null` widens the scope to every session in
   *  the workspace (used for the Home pane). */
  cwd: string | null;
  /** Thread id of the session currently shown in the pane — used to
   *  highlight the active row and to suppress selection when the
   *  user re-clicks it. */
  activeThreadId: string | null;
  /** Called when the user picks a past session to reopen. The caller
   *  is responsible for stopping the current session and starting a
   *  fresh one with the record's `sdk_session_id` passed through as
   *  the SDK `resume` field. */
  onSelect: (record: AgentChatSessionRecord) => void;
  /** Called when the user clicks "New Chat" at the bottom of the
   *  dropdown. */
  onNewChat: () => void;
  /**
   * Test seam — when provided, skip the `agent_chat_list_sessions`
   * invoke and render the given sessions synchronously. Lets unit
   * tests drive the dropdown without stubbing out Tauri.
   */
  sessionsOverride?: AgentChatSessionRecord[];
  /** Same purpose as `sessionsOverride` — skips the delete invoke so
   *  tests can assert behaviour without a real backend. */
  onDeleteOverride?: (threadId: string) => void;
}

const LIST_LIMIT = 50;

export function SessionSelector({
  workspaceId,
  cwd,
  activeThreadId,
  onSelect,
  onNewChat,
  sessionsOverride,
  onDeleteOverride,
}: SessionSelectorProps) {
  const [open, setOpen] = useState(false);
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

  const buckets = useMemo(
    () => groupSessionsByDate(sessions),
    [sessions],
  );

  const active = useMemo(
    () => sessions.find((s) => s.thread_id === activeThreadId) ?? null,
    [sessions, activeThreadId],
  );

  const triggerLabel = active
    ? sessionDisplayTitle(active)
    : activeThreadId
    ? "New Chat"
    : "History";

  const handleDelete = (threadId: string) => {
    setSessions((prev) => prev.filter((s) => s.thread_id !== threadId));
    if (onDeleteOverride) {
      onDeleteOverride(threadId);
      return;
    }
    agentChatDeleteSession(threadId).catch((err) => {
      console.warn("[agent-chat] failed to delete session", err);
    });
  };

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="h-6 gap-1 px-1.5 text-xs font-normal text-muted-foreground hover:text-foreground"
          aria-label="Chat history"
          data-testid="session-selector-trigger"
        >
          <span className="max-w-[200px] truncate">{triggerLabel}</span>
          <ChevronDown className="h-3 w-3" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="w-80 max-h-[400px] overflow-y-auto"
        data-testid="session-selector-content"
      >
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
                    setOpen(false);
                    return;
                  }
                  onSelect(session);
                  setOpen(false);
                }}
                onDelete={() => handleDelete(session.thread_id)}
              />
            ))}
          </div>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={() => {
            onNewChat();
            setOpen(false);
          }}
          data-testid="session-selector-new-chat"
        >
          <Plus className="h-3.5 w-3.5" />
          New Chat
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
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
        // Active-row highlight stays neutral — the chat-ui skill
        // reserves accent for the app shell, not in-pane surfaces.
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
