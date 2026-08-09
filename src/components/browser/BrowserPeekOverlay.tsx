import { ExternalLink, X } from "lucide-react";
import { useEffect, useRef } from "react";

import { selectBackgroundBrowserSession } from "@/components/browser/background-browser-indicator";
import { BrowserPane } from "@/components/browser/BrowserPane";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useGuiChrome } from "@/hooks/use-gui-chrome";
import { cn } from "@/lib/utils";
import { useActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import {
  parseViewportString,
  selectBackgroundBrowserDesktopViewport,
  selectBrowserDefaultViewport,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";
import { dockBrowserInRightPanel } from "@/tauri/commands";

/** Fallback pinned viewport for the "Desktop-size background browser"
 *  setting when no `browser.default_viewport` is configured. Matches
 *  the `desktop` preset / `RESET_SPEC` in
 *  `src-tauri/src/browser_viewport.rs` (1280×800 @ 1× DPR), so the peek
 *  shows pages at the same baseline agents get from
 *  `codemux browser viewport reset`. With a configured default (e.g.
 *  `"2560x1440"` to match the user's monitor) the peek pins to that
 *  instead — same size the backend applies to fresh daemons — so the
 *  peek and the agent's screenshots stay consistent. */
const DESKTOP_PEEK_VIEWPORT = { width: 1280, height: 800 };

/**
 * Floating "peek" overlay for a GUI-mode background browser session: clicking the
 * inline chat chip or terminal-header indicator opens this instead of
 * splitting the chat into a pane. Renders the live browser stream as a
 * top-right floating panel absolutely positioned over the chat surface — it
 * never resizes or reflows the chat. "Open in side panel" graduates the
 * session into the right-panel deck's `browser` pane via
 * `dock_browser_in_right_panel` — the deck is the one persistent home for a
 * browser, so the peek stays a transient look rather than a second place a
 * browser can permanently live. Closing the overlay just hides it; the
 * background session keeps running.
 *
 * Mounted once at the app-shell level, inside `SidebarInset` (already a
 * `position: relative` anchor) so `absolute` positioning here never
 * affects layout. Scoped to the active workspace — GUI chrome (and
 * therefore this overlay) only ever applies to the active workspace's chat
 * surface.
 */
export function BrowserPeekOverlay() {
  const guiChrome = useGuiChrome();
  const activeWorkspaceId = useActiveWorkspaceId();
  const isOpen = useBrowserPeekStore((s) =>
    activeWorkspaceId ? s.isOpen(activeWorkspaceId) : false,
  );
  const close = useBrowserPeekStore((s) => s.close);
  const session = useAppStore((s) =>
    selectBackgroundBrowserSession(s.appState, activeWorkspaceId),
  );
  const desktopViewport = useSyncedSettingsStore(
    selectBackgroundBrowserDesktopViewport,
  );
  const defaultViewportRaw = useSyncedSettingsStore(selectBrowserDefaultViewport);
  const pinnedViewport =
    parseViewportString(defaultViewportRaw) ?? DESKTOP_PEEK_VIEWPORT;
  const panelRef = useRef<HTMLDivElement>(null);

  const open = guiChrome && isOpen && !!session && !!activeWorkspaceId;

  // Switching workspaces dismisses the peek: it is a transient "look at
  // this now" affordance, so navigating A → B → back to A must not pop it
  // open again unprompted. The store holds a single `openWorkspaceId`;
  // whenever the active workspace no longer matches it, clear it.
  useEffect(() => {
    const { openWorkspaceId, closeAll } = useBrowserPeekStore.getState();
    if (openWorkspaceId !== null && openWorkspaceId !== activeWorkspaceId) {
      closeAll();
    }
  }, [activeWorkspaceId]);

  // Escape closes.
  useEffect(() => {
    if (!open || !activeWorkspaceId) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") close(activeWorkspaceId);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, activeWorkspaceId, close]);

  // Click-outside closes (an invisible full-surface backdrop would also
  // work, but a document listener keeps the overlay from having to sit
  // above a synthetic scrim that could itself intercept chat clicks).
  useEffect(() => {
    if (!open || !activeWorkspaceId) return;
    const onPointerDown = (e: PointerEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        close(activeWorkspaceId);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open, activeWorkspaceId, close]);

  if (!open || !activeWorkspaceId || !session) return null;

  const handlePromote = async () => {
    // Promote into the right-panel deck, not a pane-tree split — the same
    // action the deck's "+" ▸ Browser item performs, so there is exactly
    // one persistent home for a browser and the peek stays what it is: a
    // transient look that graduates into that home. Splitting the chat
    // in half to show a browser was the old model; the deck gives the
    // browser real estate without reflowing the conversation.
    //
    // `dock_browser_in_right_panel` docks *this* session (same
    // `cli_session_name`, same daemon), so the agent keeps driving the
    // browser the user just took hold of.
    try {
      await dockBrowserInRightPanel(activeWorkspaceId);
      useUIStore.getState().setRightPanelTab(activeWorkspaceId, "browser");
      close(activeWorkspaceId);
    } catch (err) {
      console.error("[BrowserPeekOverlay] promote to panel failed:", err);
    }
  };

  return (
    <div
      ref={panelRef}
      role="dialog"
      aria-label="Background browser preview"
      className={cn(
        "absolute right-3.5 top-3.5 z-30 flex h-[300px] w-[440px] flex-col overflow-hidden",
        "rounded-xl border border-border bg-popover shadow-2xl",
        "animate-in fade-in slide-in-from-top-1 duration-150 ease-out",
      )}
    >
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-2.5">
        <span
          className="h-[7px] w-[7px] shrink-0 rounded-full bg-status-open"
          aria-hidden
        />
        <span className="min-w-0 flex-1 truncate rounded-md border border-border px-2 py-1 font-mono text-[11px] text-muted-foreground">
          {session.current_url ?? "about:blank"}
        </span>
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={handlePromote}
              aria-label="Open in side panel"
              className="flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-foreground/[0.09] hover:text-foreground"
            >
              <ExternalLink className="h-3.5 w-3.5" aria-hidden />
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4}>
            Open in side panel
          </TooltipContent>
        </Tooltip>
        <button
          type="button"
          onClick={() => close(activeWorkspaceId)}
          aria-label="Close preview"
          className="flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-foreground/[0.09] hover:text-foreground"
        >
          <X className="h-3.5 w-3.5" aria-hidden />
        </button>
      </div>
      <div className="min-h-0 flex-1">
        <BrowserPane
          browserId={session.cli_session_name}
          workspaceId={activeWorkspaceId}
          focused={false}
          visible={open}
          hideToolbar
          fixedViewport={desktopViewport ? pinnedViewport : undefined}
        />
      </div>
    </div>
  );
}
