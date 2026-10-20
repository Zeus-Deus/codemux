import { PanelRightOpen, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { PanelHeader } from "@/components/ui/panel-header";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { selectReduceMotion, useSettingsStore } from "@/stores/settings-store";
import { clampPeekSize, useBrowserPeekStore, type PeekSize, type PeekStatusKind } from "@/stores/browser-peek-store";
import { PeekStatus } from "./PeekStatus";

export interface PeekContent {
  id: string;
  workspaceId: string;
  title: string;
  titlePresentation?: "text" | "url";
  ariaLabel: string;
  icon?: ReactNode;
  status: PeekStatusKind;
  statusLabel?: string;
  close: () => void;
  dock?: () => void;
  render: (props: { size: PeekSize; visible: boolean }) => ReactNode;
  /** An unloaded contribution must disappear without retaining its renderer. */
  available?: () => boolean;
}

const EXIT_MS = 250;

/** The peek's inset from its container's edges (`right-3.5` / `bottom-3.5`). */
const PEEK_INSET_PX = 14;
/** Air the peek keeps above a composer it would otherwise cover. */
const COMPOSER_GAP_PX = 12;

/**
 * Bottom offset that keeps a bottom-right peek of `width` × `height` off
 * every composer (`[data-composer-dock]`) it would overlap horizontally,
 * so the send button and the queue stay usable while the peek is up. When
 * there is not enough height to clear one, the peek stays at its corner
 * and overlaps rather than leaving its container.
 */
export function peekBottomOffset(
  container: { right: number; top: number; bottom: number },
  docks: ReadonlyArray<{ left: number; right: number; top: number }>,
  size: PeekSize,
): number {
  const right = container.right - PEEK_INSET_PX;
  const left = right - size.width;
  let bottom = PEEK_INSET_PX;
  for (const dock of docks) {
    if (dock.right <= left || dock.left >= right) continue;
    bottom = Math.max(bottom, container.bottom - dock.top + COMPOSER_GAP_PX);
  }
  const roomAbove = container.bottom - container.top - PEEK_INSET_PX - size.height;
  return bottom <= roomAbove ? bottom : PEEK_INSET_PX;
}

function windowSize(): PeekSize { return { width: window.innerWidth, height: window.innerHeight }; }
function reducedMotion() {
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** One chrome implementation for browser and plugin bodies. */
export function PeekShell({ content, workspaceId, allowExit = true }: {
  content: PeekContent | null;
  workspaceId: string | null;
  allowExit?: boolean;
}) {
  const panelRef = useRef<HTMLDivElement>(null);
  const resizeCleanup = useRef<(() => void) | null>(null);
  const appReducedMotion = useSettingsStore(selectReduceMotion);
  const lastShown = useRef<PeekContent | null>(null);
  if (content) lastShown.current = content;
  else if (lastShown.current?.workspaceId !== workspaceId || !allowExit || lastShown.current?.available?.() === false) lastShown.current = null;
  const open = content !== null;
  const [exitDone, setExitDone] = useState(true);
  const storedSize = useBrowserPeekStore((s) => s.size);
  const setStoredSize = useBrowserPeekStore((s) => s.setSize);
  const [dragSize, setDragSize] = useState<PeekSize | null>(null);
  const [winSize, setWinSize] = useState<PeekSize>(windowSize);
  const [bottom, setBottom] = useState(PEEK_INSET_PX);
  useEffect(() => {
    setDragSize(null);
    return () => {
      resizeCleanup.current?.();
      resizeCleanup.current = null;
    };
  }, [open, content?.id, workspaceId]);
  useEffect(() => {
    if (open) { setExitDone(false); return; }
    const timer = setTimeout(() => { lastShown.current = null; setExitDone(true); }, EXIT_MS);
    return () => clearTimeout(timer);
  }, [open, workspaceId]);
  useEffect(() => {
    if (!open) return;
    const onResize = () => setWinSize(windowSize());
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [open]);
  const close = content?.close;
  useEffect(() => {
    if (!close) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      close();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [close]);
  useEffect(() => {
    if (!close) return;
    const onPointerDown = (e: PointerEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) close();
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [close]);
  // Lift the peek over any composer under its corner, following the
  // composer as it grows with a draft or a queue, and picking up composers
  // that mount while the peek is open (a keyboard tab switch, a new split).
  const liveSize = clampPeekSize(dragSize ?? storedSize, winSize);
  const { width: liveWidth, height: liveHeight } = liveSize;
  useLayoutEffect(() => {
    const panel = panelRef.current;
    const container = panel?.offsetParent ?? panel?.parentElement;
    if (!open || !panel || !container) return;
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => schedule());
    let observed = new Set<HTMLElement>();
    const sync = () => {
      const docks = new Set(container.querySelectorAll<HTMLElement>("[data-composer-dock]"));
      for (const dock of docks) if (!observed.has(dock)) resize?.observe(dock);
      for (const dock of observed) if (!docks.has(dock)) resize?.unobserve(dock);
      observed = docks;
      setBottom(
        peekBottomOffset(
          container.getBoundingClientRect(),
          Array.from(docks, (dock) => dock.getBoundingClientRect()),
          { width: liveWidth, height: liveHeight },
        ),
      );
    };
    // Coalesce bursts (a streaming transcript mutates every frame).
    let frame = 0;
    const schedule = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        sync();
      });
    };
    sync();
    resize?.observe(container);
    const mutations = new MutationObserver(schedule);
    mutations.observe(container, { childList: true, subtree: true });
    return () => {
      cancelAnimationFrame(frame);
      mutations.disconnect();
      resize?.disconnect();
    };
  }, [open, liveWidth, liveHeight]);

  const shown = content ?? (!exitDone && !appReducedMotion && !reducedMotion() ? lastShown.current : null);
  if (!shown) return null;
  const size = liveSize;
  const startResize = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    resizeCleanup.current?.();
    const handle = e.currentTarget;
    const origin = { x: e.clientX, y: e.clientY, ...size };
    let latest = size;
    try { handle.setPointerCapture(e.pointerId); } catch { /* Best effort. */ }
    const onMove = (ev: PointerEvent) => {
      latest = clampPeekSize({ width: origin.width + origin.x - ev.clientX, height: origin.height + origin.y - ev.clientY }, windowSize());
      setDragSize(latest);
    };
    const cleanup = () => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onEnd);
      handle.removeEventListener("pointercancel", onEnd);
      handle.removeEventListener("lostpointercapture", onEnd);
      try {
        if (handle.hasPointerCapture(e.pointerId)) handle.releasePointerCapture(e.pointerId);
      } catch { /* The element or pointer may already be gone. */ }
      resizeCleanup.current = null;
    };
    const onEnd = () => {
      cleanup();
      setDragSize(null);
      setStoredSize(latest);
    };
    resizeCleanup.current = cleanup;
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onEnd);
    handle.addEventListener("pointercancel", onEnd);
    handle.addEventListener("lostpointercapture", onEnd);
  };
  return (
    <div ref={panelRef} role="dialog" aria-label={shown.ariaLabel} data-state={open ? "open" : "closed"}
      style={{
        "--peek-w": `${size.width}px`, "--peek-h": `${size.height}px`,
        bottom,
      } as CSSProperties}
      className={cn(
        "absolute right-3.5 z-30 flex h-(--peek-h) w-(--peek-w) flex-col overflow-hidden",
        "rounded-lg border border-border bg-popover shadow-2xl",
        "animate-in fade-in slide-in-from-bottom-1 duration-250 ease-out motion-reduce:animate-none",
        "data-[state=closed]:pointer-events-none data-[state=closed]:animate-out data-[state=closed]:fade-out data-[state=closed]:slide-out-to-bottom-1 data-[state=closed]:fill-mode-forwards",
      )}>
      <PanelHeader>
        <PeekStatus status={shown.status} label={shown.statusLabel} />
        {shown.icon}
        <span className={cn("min-w-0 flex-1 truncate text-label", shown.titlePresentation === "url"
          ? "rounded-md border border-border px-2 py-1 font-mono text-muted-foreground"
          : "font-medium text-foreground")}>{shown.title}</span>
        {shown.dock && <Tooltip>
          <TooltipTrigger asChild>
            <button type="button" onClick={shown.dock} aria-label="Open in side panel"
              className="flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground">
              <PanelRightOpen className="size-3.5" aria-hidden />
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4}>Open in side panel</TooltipContent>
        </Tooltip>}
        <button type="button" onClick={shown.close} aria-label="Close preview"
          className="flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground">
          <X className="size-3.5" aria-hidden />
        </button>
      </PanelHeader>
      <div className="min-h-0 flex-1" key={`${shown.id}:${shown.workspaceId}`}>{shown.render({ size, visible: open })}</div>
      <div data-peek-resize aria-hidden title="Drag to resize" onPointerDown={startResize}
        className="absolute left-0 top-0 z-20 size-3 cursor-nwse-resize" />
    </div>
  );
}
