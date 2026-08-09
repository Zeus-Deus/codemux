import { ChevronRight, Globe } from "lucide-react";
import { memo } from "react";

import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import type { AgentBrowserSession } from "@/tauri/types";

interface Props {
  session: AgentBrowserSession;
  workspaceId: string;
  /** Omit when an immediately preceding subagent row already introduced the
   *  shared work-log stretch. */
  showLabel?: boolean;
}

/**
 * Compact conversation-row event for a GUI-mode background browser session.
 *
 * Rendered by `MessageList` as a derived row (not a reducer/`ChatViewItem`)
 * appended after the transcript whenever this pane's workspace has a live,
 * pane-less agent browser session — the browser opened but the chat kept
 * full width instead of splitting into a pane. Clicking opens the peek
 * overlay (`BrowserPeekOverlay`); it never splits the chat on its own.
 *
 * It deliberately uses the same 32px grammar as subagent work: the thread
 * records *when* the browser opened, while the context-row indicator remains
 * the live browser surface.
 */
export const BackgroundBrowserChip = memo(function BackgroundBrowserChip({
  session,
  workspaceId,
  showLabel = true,
}: Props) {
  const open = useBrowserPeekStore((s) => s.open);
  const live = session.is_active;

  return (
    <div className="min-w-0">
      {showLabel && (
        <div className="mb-0.5 pl-0.5 font-mono text-[9px] leading-none lowercase tracking-[0.08em] text-muted-foreground/55">
          work log
        </div>
      )}
      <button
        type="button"
        onClick={() => open(workspaceId)}
        className="group/browser -mx-2 flex h-8 w-[calc(100%+1rem)] min-w-0 items-center gap-2 rounded-md px-2 text-left transition-colors hover:bg-foreground/[0.05]"
      >
        <span className="relative flex size-5 shrink-0 items-center justify-center text-status-remote">
          <Globe className="size-3.5" strokeWidth={1.7} aria-hidden />
          {live && (
            <span
              className="cm-blink absolute right-0 top-0 size-1.5 rounded-full bg-status-working ring-1 ring-background"
              aria-hidden
            />
          )}
        </span>
        <span className="shrink-0 text-[12px] font-semibold text-foreground/85">
          Opened the browser
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground">
          {session.current_url ?? "about:blank"}
          {live ? " · navigating" : ""}
        </span>
        <span className="flex shrink-0 items-center gap-0.5 text-[10px] font-medium text-foreground/70">
          View
          <ChevronRight
            className="size-3 transition-transform group-hover/browser:translate-x-0.5"
            strokeWidth={1.7}
            aria-hidden
          />
        </span>
      </button>
    </div>
  );
});
