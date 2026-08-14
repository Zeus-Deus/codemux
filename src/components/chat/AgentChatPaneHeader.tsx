import { SplitSquareHorizontal, SplitSquareVertical, X } from "lucide-react";

import { SessionSelector } from "@/components/chat/SessionSelector";
import { Button } from "@/components/ui/button";
import { useAgentChatSessionActions } from "@/hooks/use-agent-chat-session-actions";
import { findWorkspaceIdForPane, useAppStore } from "@/stores/app-store";
import { closePane, splitPane } from "@/tauri/commands";
import type { PaneNodeSnapshot } from "@/tauri/types";
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
  const workspaceId = useAppStore((s) =>
    findWorkspaceIdForPane(s, pane.pane_id),
  );
  // Resume / new-chat orchestration is shared with the GUI-chrome
  // title-bar chat tab so both paths stop → hydrate → resume identically.
  const { cwd, handleSelect, handleNewChat } = useAgentChatSessionActions(pane);

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
      {/* Match the muted-at-rest / lift-on-hover dialect the rest of
          the app's pane headers landed in c11a3fc + e96619e: 28px hit
          target, 14px glyph, drop close to destructive-foreground when
          its red hover bg kicks in. */}
      <div className="flex items-center gap-0.5 opacity-0 transition-opacity duration-150 group-hover/pane:opacity-100">
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
    </header>
  );
}
