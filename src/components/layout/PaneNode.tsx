import React from "react";
import { TerminalPane } from "@/components/terminal/TerminalPane";
import { splitPane, closePane, activatePane, resizeSplit, swapPanes } from "@/tauri/commands";
import { Globe, SplitSquareHorizontal, SplitSquareVertical, X } from "lucide-react";
import type { PaneNodeSnapshot } from "@/tauri/types";

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

  const container = (e.target as HTMLElement).closest("[data-split-container]");
  if (!container) return;

  const rect = container.getBoundingClientRect();
  const sizes = normalizeChildSizes(node.child_sizes, node.children.length);
  const handle = e.currentTarget as HTMLElement;
  handle.dataset.dragging = "true";

  const onMove = (ev: PointerEvent) => {
    const axisSize = node.direction === "horizontal" ? rect.width : rect.height;
    if (axisSize === 0) return;
    const pos =
      node.direction === "horizontal"
        ? ev.clientX - rect.left
        : ev.clientY - rect.top;

    // Calculate cumulative size up to this index
    let cumBefore = 0;
    for (let i = 0; i < index; i++) cumBefore += sizes[i];
    const pair = sizes[index] + sizes[index + 1];

    const fraction = pos / axisSize - cumBefore;
    const first = Math.max(0.05, Math.min(fraction, pair - 0.05));
    const second = Math.max(0.05, pair - first);
    const next = [...sizes];
    next[index] = first;
    next[index + 1] = second;

    resizeSplit(node.pane_id, next).catch(console.error);
  };

  const onUp = () => {
    handle.dataset.dragging = "false";
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
        ? { display: "grid", gridTemplateColumns: sizesFr.join(" "), gap: "2px", height: "100%", width: "100%" }
        : { display: "grid", gridTemplateRows: sizesFr.join(" "), gap: "2px", height: "100%", width: "100%" };

    return (
      <div style={gridStyle} data-split-container data-split-pane-id={node.pane_id}>
        {node.children.map((child, i) => (
          <div key={child.pane_id} className="relative min-w-0 min-h-0 overflow-hidden">
            <PaneNode node={child} activePaneId={activePaneId} visible={visible} />
            {i < node.children.length - 1 && (
              <div
                className={`absolute z-20 opacity-0 hover:opacity-100 data-[dragging=true]:opacity-100 transition-opacity ${
                  node.direction === "horizontal"
                    ? "top-1 bottom-1 -right-[6px] w-3 cursor-col-resize"
                    : "left-1 right-1 -bottom-[6px] h-3 cursor-row-resize"
                } bg-primary/40 rounded-full`}
                onPointerDown={(e) => startResize(e, node as PaneNodeSnapshot & { kind: "split" }, i)}
              />
            )}
          </div>
        ))}
      </div>
    );
  }

  const isActive = node.pane_id === activePaneId;

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
        className={`group/pane flex h-full w-full flex-col min-w-0 min-h-0 overflow-hidden border ${
          isActive ? "border-primary/50" : "border-border/50"
        }`}
        data-pane-drop-id={node.pane_id}
        data-pane-title={node.title}
        onPointerDown={handleActivate}
      >
        <header
          className="flex h-7 shrink-0 items-center gap-1 border-b border-border/50 bg-card px-2 cursor-grab active:cursor-grabbing"
          onPointerDown={(e) => handleDragStart(e, node.pane_id)}
        >
          <span className="flex-1 truncate text-xs text-muted-foreground">
            {node.title}
          </span>
          <div className="flex items-center gap-0.5 opacity-0 transition-opacity group-hover/pane:opacity-100">
            <button
              className="inline-flex h-5 w-5 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
              onClick={() => handleSplit("horizontal")}
              title="Split right"
            >
              <SplitSquareHorizontal className="h-3 w-3" />
            </button>
            <button
              className="inline-flex h-5 w-5 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
              onClick={() => handleSplit("vertical")}
              title="Split down"
            >
              <SplitSquareVertical className="h-3 w-3" />
            </button>
            <button
              className="inline-flex h-5 w-5 items-center justify-center rounded-sm text-muted-foreground hover:bg-destructive/80 hover:text-foreground"
              onClick={handleClose}
              title="Close pane"
            >
              <X className="h-3 w-3" />
            </button>
          </div>
        </header>
        <div className="flex-1 min-h-0 overflow-hidden">
          <TerminalPane
            sessionId={node.session_id}
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
        className={`group/pane flex h-full w-full flex-col min-w-0 min-h-0 overflow-hidden border ${
          isActive ? "border-primary/50" : "border-border/50"
        }`}
        data-pane-drop-id={node.pane_id}
        onPointerDown={handleActivate}
      >
        <header
          className="flex h-7 shrink-0 items-center gap-1 border-b border-border/50 bg-card px-2 cursor-grab active:cursor-grabbing"
          onPointerDown={(e) => handleDragStart(e, node.pane_id)}
        >
          <Globe className="h-3 w-3 text-muted-foreground" />
          <span className="flex-1 truncate text-xs text-muted-foreground">
            {node.title}
          </span>
        </header>
        <div className="flex flex-1 items-center justify-center text-xs text-muted-foreground">
          Browser pane — coming soon
        </div>
      </div>
    );
  }

  return null;
}
