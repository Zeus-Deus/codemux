import React from "react";
import { TerminalPane } from "@/components/terminal/TerminalPane";
import { BrowserPane } from "@/components/browser/BrowserPane";
import { Button } from "@/components/ui/button";
import { PresetIcon } from "@/components/icons/preset-icon";
import { cn } from "@/lib/utils";
import { splitPane, closePane, activatePane, resizeSplit, swapPanes } from "@/tauri/commands";
import { SplitSquareHorizontal, SplitSquareVertical, X } from "lucide-react";
import type { PaneNodeSnapshot, PaneStatus } from "@/tauri/types";
import { useAppStore } from "@/stores/app-store";
import { StatusIndicator } from "@/components/ui/status-indicator";

// Map known preset names to their icon identifiers
const PRESET_TITLE_TO_ICON: Record<string, string> = {
  "Claude Code": "claude",
  "Codex": "codex",
  "OpenCode": "opencode",
  "Gemini": "gemini",
  "Shell": "terminal",
};

interface Props {
  node: PaneNodeSnapshot;
  activePaneId: string;
  visible: boolean;
}

function normalizeChildSizes(raw: number[], count: number): number[] {
  const sizes = raw.length === count ? [...raw] : Array(count).fill(1 / count);
  const total = sizes.reduce((s, v) => s + v, 0) || 1;
  return sizes.map((v) => v / total);
}

// ── Resize handle logic (ported from old PaneNode.svelte) ──

function startResize(
  e: React.PointerEvent,
  node: PaneNodeSnapshot & { kind: "split" },
  index: number,
) {
  e.preventDefault();
  e.stopPropagation();

  // Safety: clear stale class from a prior drag interrupted by unmount
  document.body.classList.remove("pane-resizing");

  const container = (e.target as HTMLElement).closest("[data-split-container]");
  if (!container) return;

  const rect = container.getBoundingClientRect();
  const sizes = normalizeChildSizes(node.child_sizes, node.children.length);
  const handle = e.currentTarget as HTMLElement;
  handle.dataset.dragging = "true";
  document.body.classList.add("pane-resizing");
  let lastSizes: number[] | null = null;

  const onMove = (ev: PointerEvent) => {
    const axisSize = node.direction === "horizontal" ? rect.width : rect.height;
    if (axisSize === 0) return;
    const pos =
      node.direction === "horizontal"
        ? ev.clientX - rect.left
        : ev.clientY - rect.top;

    let cumBefore = 0;
    for (let i = 0; i < index; i++) cumBefore += sizes[i];
    const pair = sizes[index] + sizes[index + 1];

    const fraction = pos / axisSize - cumBefore;
    const first = Math.max(0.05, Math.min(fraction, pair - 0.05));
    const second = Math.max(0.05, pair - first);
    const next = [...sizes];
    next[index] = first;
    next[index + 1] = second;
    lastSizes = next;

    // Optimistic: update grid CSS directly for instant feedback (skip Tauri IPC)
    const template = next.map((s) => `${Math.max(s, 0.05)}fr`).join(" ");
    const el = container as HTMLElement;
    if (node.direction === "horizontal") {
      el.style.gridTemplateColumns = template;
    } else {
      el.style.gridTemplateRows = template;
    }
  };

  const onUp = () => {
    handle.dataset.dragging = "false";
    document.body.classList.remove("pane-resizing");
    // Persist final sizes to backend once on release
    if (lastSizes) {
      resizeSplit(node.pane_id, lastSizes).catch(console.error);
    }
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
  };

  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
}

// ── Drag-to-swap logic (ported from old PaneNode.svelte) ──

function handleDragStart(
  e: React.PointerEvent,
  sourcePaneId: string,
) {
  // Only primary button, skip action buttons
  if (e.button !== 0) return;
  if ((e.target as HTMLElement).closest("button")) return;

  const startX = e.clientX;
  const startY = e.clientY;
  let dragging = false;
  let highlighted: HTMLElement | null = null;
  let targetPaneId: string | null = null;

  const clearHighlight = () => {
    if (highlighted) {
      highlighted.querySelector(".pane-drop-overlay")?.remove();
      highlighted = null;
    }
    targetPaneId = null;
  };

  const showOverlay = (target: HTMLElement) => {
    const overlay = document.createElement("div");
    overlay.className = "pane-drop-overlay";
    overlay.innerHTML = "<span>Drop to swap</span>";
    target.appendChild(overlay);
  };

  const findDropTarget = (cx: number, cy: number): HTMLElement | null => {
    const shells = document.querySelectorAll<HTMLElement>("[data-pane-drop-id]");
    let best: HTMLElement | null = null;
    let smallestArea = Infinity;

    for (const shell of shells) {
      const id = shell.dataset.paneDropId;
      if (!id || id === sourcePaneId) continue;
      const r = shell.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      if (cx < r.left || cx > r.right || cy < r.top || cy > r.bottom) continue;
      const area = r.width * r.height;
      if (area < smallestArea) {
        smallestArea = area;
        best = shell;
      }
    }
    return best;
  };

  const onMove = (ev: PointerEvent) => {
    if (!dragging) {
      const dx = ev.clientX - startX;
      const dy = ev.clientY - startY;
      if (dx * dx + dy * dy < 64) return; // 8px threshold
      dragging = true;
      document.body.style.cursor = "grabbing";
    }

    clearHighlight();
    const target = findDropTarget(ev.clientX, ev.clientY);
    if (target) {
      showOverlay(target);
      highlighted = target;
      targetPaneId = target.dataset.paneDropId ?? null;
    }
  };

  const onUp = () => {
    document.body.style.cursor = "";
    if (dragging && targetPaneId) {
      swapPanes(sourcePaneId, targetPaneId).catch(console.error);
    }
    clearHighlight();
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onUp);
  };

  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onUp);
}

// ── Component ──

export function PaneNode({ node, activePaneId, visible }: Props) {
  if (node.kind === "split") {
    const sizes = normalizeChildSizes(node.child_sizes, node.children.length);
    const sizesFr = sizes.map((s) => `${Math.max(s, 0.05)}fr`);
    const gridStyle: React.CSSProperties =
      node.direction === "horizontal"
        ? { display: "grid", gridTemplateColumns: sizesFr.join(" "), gap: "1px", height: "100%", width: "100%" }
        : { display: "grid", gridTemplateRows: sizesFr.join(" "), gap: "1px", height: "100%", width: "100%" };

    return (
      <div style={gridStyle} data-split-container data-split-pane-id={node.pane_id}>
        {node.children.map((child, i) => (
          <div key={child.pane_id} className="relative min-w-0 min-h-0 overflow-hidden">
            <PaneNode node={child} activePaneId={activePaneId} visible={visible} />
            {i < node.children.length - 1 && (
              <div
                className={`absolute z-20 opacity-0 hover:opacity-100 data-[dragging=true]:opacity-100 transition-opacity duration-100 ${
                  node.direction === "horizontal"
                    ? "top-1 bottom-1 -right-[6px] w-3 cursor-col-resize"
                    : "left-1 right-1 -bottom-[6px] h-3 cursor-row-resize"
                } bg-primary/30 rounded-full`}
                onPointerDown={(e) => startResize(e, node as PaneNodeSnapshot & { kind: "split" }, i)}
              />
            )}
          </div>
        ))}
      </div>
    );
  }

  const isActive = node.pane_id === activePaneId;
  const paneStatus: PaneStatus | undefined = useAppStore(
    (s) => s.appState?.pane_statuses[node.pane_id],
  );

  const handleActivate = () => {
    if (!isActive) activatePane(node.pane_id).catch(console.error);
  };

  const handleSplit = (direction: "horizontal" | "vertical") => {
    splitPane(node.pane_id, direction).catch(console.error);
  };

  const handleClose = () => {
    closePane(node.pane_id).catch(console.error);
  };

  if (node.kind === "terminal") {
    return (
      <div
        className="group/pane flex h-full w-full flex-col min-w-0 min-h-0 overflow-hidden border border-border/30"
        data-pane-drop-id={node.pane_id}
        data-pane-title={node.title}
        onPointerDown={handleActivate}
      >
        <header
          className={cn("flex h-7 shrink-0 items-center gap-1 border-b border-border/30 px-2 cursor-grab active:cursor-grabbing transition-colors", isActive ? "bg-card" : "bg-background")}
          onPointerDown={(e) => handleDragStart(e, node.pane_id)}
        >
          <span className="flex items-center gap-1.5 flex-1 truncate text-xs text-muted-foreground">
            {PRESET_TITLE_TO_ICON[node.title] && (
              <PresetIcon icon={PRESET_TITLE_TO_ICON[node.title]} className="h-3 w-3" />
            )}
            {node.title}
            {paneStatus && paneStatus !== "idle" && (
              <StatusIndicator status={paneStatus} />
            )}
          </span>
          <div className="flex items-center gap-0.5 opacity-0 transition-opacity duration-150 group-hover/pane:opacity-100">
            <Button variant="ghost" size="icon-xs" onClick={() => handleSplit("horizontal")} aria-label="Split right" title="Split right">
              <SplitSquareHorizontal className="h-3 w-3" />
            </Button>
            <Button variant="ghost" size="icon-xs" onClick={() => handleSplit("vertical")} aria-label="Split down" title="Split down">
              <SplitSquareVertical className="h-3 w-3" />
            </Button>
            <Button variant="ghost" size="icon-xs" className="hover:bg-destructive/80" onClick={handleClose} aria-label="Close pane" title="Close pane">
              <X className="h-3 w-3" />
            </Button>
          </div>
        </header>
        <div className="flex-1 min-h-0 overflow-hidden">
          <TerminalPane
            sessionId={node.session_id}
            paneId={node.pane_id}
            focused={isActive}
            visible={visible}
            title={node.title}
          />
        </div>
      </div>
    );
  }

  if (node.kind === "browser") {
    return (
      <div
        className="group/pane flex h-full w-full flex-col min-w-0 min-h-0 overflow-hidden border border-border/30"
        data-pane-drop-id={node.pane_id}
        onPointerDown={handleActivate}
      >
        <header
          className={cn("flex h-7 shrink-0 items-center gap-1 border-b border-border/30 px-2 cursor-grab active:cursor-grabbing transition-colors", isActive ? "bg-card" : "bg-background")}
          onPointerDown={(e) => handleDragStart(e, node.pane_id)}
        >
          <span className="flex-1 truncate text-xs text-muted-foreground">
            {node.title}
          </span>
          <div className="flex items-center gap-0.5 opacity-0 transition-opacity duration-150 group-hover/pane:opacity-100">
            <Button variant="ghost" size="icon-xs" onClick={() => handleSplit("horizontal")} aria-label="Split right" title="Split right">
              <SplitSquareHorizontal className="h-3 w-3" />
            </Button>
            <Button variant="ghost" size="icon-xs" onClick={() => handleSplit("vertical")} aria-label="Split down" title="Split down">
              <SplitSquareVertical className="h-3 w-3" />
            </Button>
            <Button variant="ghost" size="icon-xs" className="hover:bg-destructive/80" onClick={handleClose} aria-label="Close pane" title="Close pane">
              <X className="h-3 w-3" />
            </Button>
          </div>
        </header>
        <div className="flex-1 min-h-0 overflow-hidden">
          <BrowserPane browserId={node.browser_id} focused={isActive} visible={visible} />
        </div>
      </div>
    );
  }

  return null;
}
