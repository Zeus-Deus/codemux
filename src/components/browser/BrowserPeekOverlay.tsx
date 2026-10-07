import { PanelRightOpen, X } from "lucide-react";
import { useEffect, useRef, useState, type CSSProperties } from "react";

import { selectBackgroundBrowserSession } from "@/components/browser/background-browser-indicator";
import { BrowserPane, type BrowserStreamStatus } from "@/components/browser/BrowserPane";
import { toast } from "@/lib/toast";
import type { AgentBrowserSession } from "@/tauri/types";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useGuiChrome } from "@/hooks/use-gui-chrome";
import { cn } from "@/lib/utils";
import { useActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import {
  clampPeekSize,
  useBrowserPeekStore,
  type PeekSize,
} from "@/stores/browser-peek-store";
import {
  parseViewportString,
  selectBackgroundBrowserDesktopViewport,
  selectBrowserDefaultViewport,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";
import { dockBrowserInRightPanel } from "@/tauri/commands";
import { PanelHeader } from "@/components/ui/panel-header";

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

/** Matches the `duration-250` (overlay) exit animation below. */
const EXIT_MS = 250;

const STATUS_DOT: Record<BrowserStreamStatus, { className: string; label: string }> = {
  live: { className: "bg-status-open", label: "Live" },
  starting: { className: "bg-muted-foreground/60 motion-safe:animate-pulse", label: "Connecting" },
  connecting: { className: "bg-muted-foreground/60 motion-safe:animate-pulse", label: "Connecting" },
  waiting: { className: "bg-muted-foreground/60 motion-safe:animate-pulse", label: "Connecting" },
  error: { className: "bg-destructive", label: "Disconnected" },
};

function prefersReducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

type Shown = { workspaceId: string; session: AgentBrowserSession };

function windowSize(): PeekSize {
  return { width: window.innerWidth, height: window.innerHeight };
}

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
  const [streamStatus, setStreamStatus] = useState<BrowserStreamStatus>("starting");
  const storedSize = useBrowserPeekStore((s) => s.size);
  const setStoredSize = useBrowserPeekStore((s) => s.setSize);
  // Live size while a resize drag is in progress; committed on release.
  const [dragSize, setDragSize] = useState<PeekSize | null>(null);
  // The 60%-of-window cap has to follow the window, not just re-renders.
  const [winSize, setWinSize] = useState<PeekSize>(windowSize);

  const open = guiChrome && isOpen && !!session && !!activeWorkspaceId;

  // Closing keeps the last shown peek mounted for the exit animation.
  // Switching workspaces skips it: the content no longer belongs here.
  const current: Shown | null =
    open && activeWorkspaceId && session
      ? { workspaceId: activeWorkspaceId, session }
      : null;
  const lastShownRef = useRef<Shown | null>(null);
  if (current) lastShownRef.current = current;
  else if (lastShownRef.current?.workspaceId !== activeWorkspaceId) lastShownRef.current = null;
  const [exitDone, setExitDone] = useState(true);
  useEffect(() => {
    if (open) {
      setExitDone(false);
      return;
    }
    const timer = setTimeout(() => {
      lastShownRef.current = null;
      setExitDone(true);
    }, EXIT_MS);
    return () => clearTimeout(timer);
  }, [open, activeWorkspaceId]);

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

  useEffect(() => {
    if (!open) return;
    const onResize = () => setWinSize(windowSize());
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [open]);

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

  const last = lastShownRef.current;
  const exiting = !current && !exitDone && last !== null && !prefersReducedMotion();
  const shown = current ?? (exiting ? last : null);
  if (!shown) return null;
  const closing = !open;
  const workspaceId = shown.workspaceId;
  const size = clampPeekSize(dragSize ?? storedSize, winSize);
  const dot = STATUS_DOT[streamStatus];

  // Anchored top-right, so the handle sits bottom-left and the peek
  // grows toward the chat it floats over.
  const startResize = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const handle = e.currentTarget;
    const origin = { x: e.clientX, y: e.clientY, ...size };
    let latest = size;
    try {
      handle.setPointerCapture(e.pointerId);
    } catch {
      // Capture is best-effort; the drag still tracks over the handle.
    }
    const onMove = (ev: PointerEvent) => {
      latest = clampPeekSize(
        {
          width: origin.width + (origin.x - ev.clientX),
          height: origin.height + (ev.clientY - origin.y),
        },
        windowSize(),
      );
      setDragSize(latest);
    };
    const onEnd = () => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onEnd);
      handle.removeEventListener("pointercancel", onEnd);
      setDragSize(null);
      setStoredSize(latest);
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onEnd);
    handle.addEventListener("pointercancel", onEnd);
  };

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
      await dockBrowserInRightPanel(workspaceId);
      useUIStore.getState().setRightPanelTab(workspaceId, "browser");
      close(workspaceId);
    } catch (err) {
      toast.error("Couldn't open the browser in the side panel", {
        description: String(err),
      });
    }
  };

  return (
    <div
      ref={panelRef}
      role="dialog"
      aria-label="Background browser preview"
      data-state={closing ? "closed" : "open"}
      style={
        {
          "--peek-w": `${size.width}px`,
          "--peek-h": `${size.height}px`,
        } as CSSProperties
      }
      className={cn(
        "absolute right-3.5 top-3.5 z-30 flex h-(--peek-h) w-(--peek-w) flex-col overflow-hidden",
        "rounded-lg border border-border bg-popover shadow-2xl",
        "animate-in fade-in slide-in-from-top-1 duration-250 ease-out motion-reduce:animate-none",
        "data-[state=closed]:pointer-events-none data-[state=closed]:animate-out data-[state=closed]:fade-out data-[state=closed]:slide-out-to-top-1 data-[state=closed]:fill-mode-forwards",
      )}
    >
      <PanelHeader>
        <span
          role="img"
          aria-label={dot.label}
          title={dot.label}
          className={cn("h-[7px] w-[7px] shrink-0 rounded-full", dot.className)}
        />
        <span className="min-w-0 flex-1 truncate rounded-md border border-border px-2 py-1 font-mono text-label text-muted-foreground">
          {shown.session.current_url ?? "about:blank"}
        </span>
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={handlePromote}
              aria-label="Open in side panel"
              className="flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
            >
              <PanelRightOpen className="size-3.5" aria-hidden />
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4}>
            Open in side panel
          </TooltipContent>
        </Tooltip>
        <button
          type="button"
          onClick={() => close(workspaceId)}
          aria-label="Close preview"
          className="flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
        >
          <X className="size-3.5" aria-hidden />
        </button>
      </PanelHeader>
      <div className="min-h-0 flex-1">
        <BrowserPane
          browserId={shown.session.cli_session_name}
          workspaceId={workspaceId}
          focused={false}
          visible={open}
          hideToolbar
          fixedViewport={desktopViewport ? pinnedViewport : undefined}
          onStatusChange={setStreamStatus}
        />
      </div>
      <div
        data-peek-resize
        aria-hidden
        title="Drag to resize"
        onPointerDown={startResize}
        className="absolute bottom-0 left-0 z-20 size-3 cursor-nesw-resize"
      />
    </div>
  );
}
