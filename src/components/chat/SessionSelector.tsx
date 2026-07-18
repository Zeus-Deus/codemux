import { ChevronDown } from "lucide-react";
import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  SessionHistoryList,
  useSessionHistory,
} from "@/components/chat/session-history-menu";
import { sessionDisplayTitle } from "@/lib/agent-chat/session-history";
import {
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
  /** Called when the user clicks "New Chat" at the top of the
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
  const { sessions, loading, handleDelete } = useSessionHistory({
    open,
    workspaceId,
    cwd,
    sessionsOverride,
    onDeleteOverride,
  });

  const active = useMemo(
    () => sessions.find((s) => s.thread_id === activeThreadId) ?? null,
    [sessions, activeThreadId],
  );

  const triggerLabel = active
    ? sessionDisplayTitle(active)
    : activeThreadId
    ? "New Chat"
    : "History";

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
        <SessionHistoryList
          sessions={sessions}
          loading={loading}
          activeThreadId={activeThreadId}
          onSelect={onSelect}
          onNewChat={onNewChat}
          onDelete={handleDelete}
          closeMenu={() => setOpen(false)}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
