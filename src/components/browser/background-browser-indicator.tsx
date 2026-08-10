import { Globe } from "lucide-react";
import { useGuiChrome } from "@/hooks/use-gui-chrome";
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
 * The background-browser indicator — a "Browser" control whose presence
 * already communicates that the session is live, so it needs no tint,
 * border or blinking dot. Click opens the floating peek overlay
 * (`BrowserPeekOverlay`) for the workspace. The Agent Chat Context Row
 * gets the labelled chip; the terminal pane header gets a bare globe
 * button sized to sit beside the other pane actions. Render it only
 * when `useBackgroundBrowserSession` resolves a session; the control
 * itself is presentation + click only.
 */
export function BackgroundBrowserIndicator({
  workspaceId,
  variant = "chip",
}: {
  workspaceId: string;
  variant?: "chip" | "pane-header";
}) {
  const openPeek = useBrowserPeekStore((s) => s.open);

  if (variant === "pane-header") {
    return (
      <button
        type="button"
        onClick={() => openPeek(workspaceId)}
        aria-label="Browser running in background — view"
        title="View background browser"
        className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-status-remote transition-colors hover:bg-status-remote/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-status-remote/50"
      >
        <Globe className="size-3.5" strokeWidth={1.75} aria-hidden />
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={() => openPeek(workspaceId)}
      aria-label="Browser running in background — view"
      className="inline-flex h-[26px] shrink-0 items-center gap-1.5 px-1.5 text-[11px] font-semibold text-status-remote transition-opacity hover:opacity-80"
    >
      <Globe className="size-3.5" strokeWidth={1.75} aria-hidden />
      Browser
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
