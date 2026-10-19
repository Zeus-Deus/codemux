import { PanelRightOpen, X } from "lucide-react";
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
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
  const shown = content ?? (!exitDone && !appReducedMotion && !reducedMotion() ? lastShown.current : null);
  if (!shown) return null;
  const size = clampPeekSize(dragSize ?? storedSize, winSize);
  const startResize = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    resizeCleanup.current?.();
    const handle = e.currentTarget;
    const origin = { x: e.clientX, y: e.clientY, ...size };
    let latest = size;
    try { handle.setPointerCapture(e.pointerId); } catch { /* Best effort. */ }
    const onMove = (ev: PointerEvent) => {
      latest = clampPeekSize({ width: origin.width + origin.x - ev.clientX, height: origin.height + ev.clientY - origin.y }, windowSize());
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
      style={{ "--peek-w": `${size.width}px`, "--peek-h": `${size.height}px` } as CSSProperties}
      className={cn(
        "absolute right-3.5 top-3.5 z-30 flex h-(--peek-h) w-(--peek-w) flex-col overflow-hidden",
        "rounded-lg border border-border bg-popover shadow-2xl",
        "animate-in fade-in slide-in-from-top-1 duration-250 ease-out motion-reduce:animate-none",
        "data-[state=closed]:pointer-events-none data-[state=closed]:animate-out data-[state=closed]:fade-out data-[state=closed]:slide-out-to-top-1 data-[state=closed]:fill-mode-forwards",
      )}>
      <PanelHeader>
        <PeekStatus status={shown.status} label={shown.statusLabel} />
        {shown.icon}
        <span className="min-w-0 flex-1 truncate rounded-md border border-border px-2 py-1 font-mono text-label text-muted-foreground">{shown.title}</span>
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
        className="absolute bottom-0 left-0 z-20 size-3 cursor-nesw-resize" />
    </div>
  );
}
