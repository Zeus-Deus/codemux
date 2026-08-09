import { Globe } from "lucide-react";
import { useGuiChrome } from "@/hooks/use-gui-chrome";
import { cn } from "@/lib/utils";
import { useActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import type { AgentBrowserSession, AppStateSnapshot } from "@/tauri/types";

/**
 * GUI-mode background browser session lookup for a workspace: a detached
 * agent browser session that is live (`is_active`) but has no
 * visible surface of its own.
 *
 * "No surface" is the union of both hosts, mirroring
 * `AgentBrowserSession::is_surfaced()` in `state_impl.rs`: not attached to
 * a pane-tree node (`pane_id === null`) *and* not docked in the
 * right-panel deck (`right_panel_docked !== true`). Every surface that
 * offers to *reveal* a background browser — the terminal pane header, the
 * Context Row's `WorkspaceStatusCluster`, the inline chat chip and the
 * peek overlay — resolves it here so none of them can drift into offering
 * to reveal a browser the user is already looking at.
 */
export function selectBackgroundBrowserSession(
  appState: AppStateSnapshot | null | undefined,
  workspaceId: string | null | undefined,
): AgentBrowserSession | null {
  if (!workspaceId) return null;
  const session = appState?.agent_browser_sessions?.find(
    (abs) => abs.workspace_id === workspaceId,
  );
  if (!session) return null;
  if (!session.is_active) return null;
  if (session.pane_id || session.right_panel_docked) return null;
  return session;
}

export function useBackgroundBrowserSession(
  workspaceId: string | null | undefined,
): AgentBrowserSession | null {
  return useAppStore((s) =>
    selectBackgroundBrowserSession(s.appState, workspaceId),
  );
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
