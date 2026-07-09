import { Globe } from "lucide-react";
import { useAppStore } from "@/stores/app-store";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import type { AgentBrowserSession } from "@/tauri/types";

/**
 * GUI-mode background browser session lookup for a workspace (see
 * docs/features/browser.md "Background browser in GUI mode") — a
 * detached agent browser session that is live (`is_active`) but not
 * attached to a pane (`pane_id === null`). Shared by
 * `WorkspaceContextBar` and the Context Row's `WorkspaceStatusCluster`
 * so both surfaces resolve the indicator from the exact same predicate.
 */
export function useBackgroundBrowserSession(
  workspaceId: string | null | undefined,
): AgentBrowserSession | null {
  return useAppStore((s) => {
    if (!workspaceId) return null;
    const session = s.appState?.agent_browser_sessions?.find(
      (abs) => abs.workspace_id === workspaceId,
    );
    if (!session || !session.is_active || session.pane_id) return null;
    return session;
  });
}

/**
 * The background-browser indicator pill — a sky-tinted "Browser" chip
 * with a blinking working dot; click opens the floating peek overlay
 * (`BrowserPeekOverlay`) for the workspace. Extracted verbatim from
 * `WorkspaceContextBar` so the Context Row's status cluster (which
 * replaces the bar while an Agent Chat pane is active — see
 * `docs/features/agent-chat.md` "Context Row") shows the identical
 * affordance. Render it only when `useBackgroundBrowserSession`
 * resolves a session; the chip itself is presentation + click only.
 */
export function BackgroundBrowserIndicator({
  workspaceId,
}: {
  workspaceId: string;
}) {
  const openPeek = useBrowserPeekStore((s) => s.open);
  return (
    <button
      type="button"
      onClick={() => openPeek(workspaceId)}
      aria-label="Browser running in background — view"
      className="inline-flex h-[26px] items-center gap-1.5 rounded-md border border-status-remote/30 bg-status-remote/10 px-2.5 text-[11px] font-semibold text-status-remote transition-colors hover:bg-status-remote/16 hover:text-foreground"
    >
      <Globe className="h-3 w-3" aria-hidden />
      Browser
      <span
        className="cm-blink h-1.5 w-1.5 rounded-full bg-status-working"
        aria-hidden
      />
    </button>
  );
}
