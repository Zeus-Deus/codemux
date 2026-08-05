import { Globe } from "lucide-react";
import { useGuiChrome } from "@/hooks/use-gui-chrome";
import { cn } from "@/lib/utils";
import { useActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import type { AgentBrowserSession } from "@/tauri/types";

/**
 * GUI-mode background browser session lookup for a workspace (see
 * docs/features/browser.md "Background browser in GUI mode") — a
 * detached agent browser session that is live (`is_active`) but not
 * attached to a pane (`pane_id === null`). Shared by
 * the terminal pane header and the Context Row's `WorkspaceStatusCluster`
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
 * (`BrowserPeekOverlay`) for the workspace. Shared by the Agent Chat
 * Context Row and the compact terminal-header control. Render it only
 * when `useBackgroundBrowserSession` resolves a session; the chip itself
 * is presentation + click only.
 */
export function BackgroundBrowserIndicator({
  workspaceId,
  variant = "chip",
}: {
  workspaceId: string;
  variant?: "chip" | "pane-header";
}) {
  const openPeek = useBrowserPeekStore((s) => s.open);
  return (
    <button
      type="button"
      onClick={() => openPeek(workspaceId)}
      aria-label="Browser running in background — view"
      title={variant === "pane-header" ? "View background browser" : undefined}
      className={cn(
        "inline-flex shrink-0 items-center rounded-md border border-status-remote/30 bg-status-remote/10 font-semibold text-status-remote transition-colors hover:bg-status-remote/16 hover:text-foreground",
        variant === "pane-header"
          ? "h-5 gap-1 px-1.5 text-[10px]"
          : "h-[26px] gap-1.5 px-2.5 text-[11px]",
      )}
    >
      <Globe
        className={variant === "pane-header" ? "size-2.5" : "size-3"}
        aria-hidden
      />
      Browser
      <span
        className={cn(
          "cm-blink rounded-full bg-status-working",
          variant === "pane-header" ? "size-1" : "size-1.5",
        )}
        aria-hidden
      />
    </button>
  );
}

/**
 * Terminal-header home for the detached browser affordance. The retired
 * workspace context bar used to reserve 42px across every terminal just to
 * keep this control reachable. The pane header already exists, so the active
 * terminal can surface the same peek action without consuming work-surface
 * height or covering terminal output.
 */
export function TerminalBackgroundBrowserIndicator({
  active,
}: {
  active: boolean;
}) {
  const guiChrome = useGuiChrome();
  const workspaceId = useActiveWorkspaceId();
  const session = useBackgroundBrowserSession(workspaceId);

  if (!active || !guiChrome || !workspaceId || !session) return null;

  return (
    <BackgroundBrowserIndicator
      workspaceId={workspaceId}
      variant="pane-header"
    />
  );
}
