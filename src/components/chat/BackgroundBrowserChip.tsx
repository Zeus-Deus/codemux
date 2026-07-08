import { ChevronRight, Globe } from "lucide-react";
import { memo } from "react";

import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import type { AgentBrowserSession } from "@/tauri/types";

interface Props {
  session: AgentBrowserSession;
  workspaceId: string;
}

/**
 * Inline conversation-row chip for a GUI-mode background browser session
 * (design "Background browser in GUI mode" — see `docs/features/browser.md`).
 *
 * Rendered by `MessageList` as a derived row (not a reducer/`ChatViewItem`)
 * appended after the transcript whenever this pane's workspace has a live,
 * pane-less agent browser session — the browser opened but the chat kept
 * full width instead of splitting into a pane. Clicking opens the peek
 * overlay (`BrowserPeekOverlay`); it never splits the chat on its own.
 *
 * Indented past the assistant avatar gutter (`w-[29px]` spacer) so it
 * aligns with the rest of the transcript's assistant-side rows.
 */
export const BackgroundBrowserChip = memo(function BackgroundBrowserChip({
  session,
  workspaceId,
}: Props) {
  const open = useBrowserPeekStore((s) => s.open);
  const live = session.is_active;

  return (
    <div className="flex gap-[13px]">
      <div className="w-[29px] shrink-0" aria-hidden />
      <button
        type="button"
        onClick={() => open(workspaceId)}
        className="flex min-w-0 flex-1 items-center gap-2.5 rounded-[11px] border border-border bg-status-remote/5 px-3 py-2.5 text-left transition-colors hover:border-status-remote/45"
      >
        <span className="relative flex h-[30px] w-[30px] shrink-0 items-center justify-center rounded-lg bg-status-remote/15 text-status-remote">
          <Globe className="h-4 w-4" aria-hidden />
          {live && (
            <span
              className="cm-blink absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-status-working ring-2 ring-card"
              aria-hidden
            />
          )}
        </span>

        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-1.5">
            <span className="text-xs font-semibold text-foreground">
              Browser opened in background
            </span>
            {live && (
              <span className="rounded bg-status-working/15 px-1.5 text-[9px] font-bold uppercase tracking-wide text-status-working">
                Live
              </span>
            )}
          </span>
          <span className="block truncate font-mono text-[11px] text-muted-foreground">
            {session.current_url ?? "about:blank"}
            {live ? " · agent is navigating…" : ""}
          </span>
        </span>

        <span className="flex h-7 shrink-0 items-center gap-1 rounded-full border border-border px-2.5 text-[11px] font-medium text-muted-foreground">
          View
          <ChevronRight className="h-3 w-3" aria-hidden />
        </span>
      </button>
    </div>
  );
});
