import type { PaneNodeSnapshot } from "@/tauri/types";

export type PaneDirection = "left" | "right" | "up" | "down";

export interface PaneRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Every leaf pane id in a surface's layout tree, in tree order. */
export function leafPaneIds(node: PaneNodeSnapshot): string[] {
  return node.kind === "split" ? node.children.flatMap(leafPaneIds) : [node.pane_id];
}

/** Whether `paneId` is `node` itself or any leaf beneath it. */
export function containsPane(node: PaneNodeSnapshot, paneId: string): boolean {
  if (node.pane_id === paneId) return true;
  return node.kind === "split" && node.children.some((c) => containsPane(c, paneId));
}

/** Panes are separated by a 1px grid gap plus borders, so an edge shared by two
 *  neighbours can read a few pixels apart. */
const EDGE_SLACK_PX = 4;

function overlap(aStart: number, aEnd: number, bStart: number, bEnd: number): number {
  return Math.max(0, Math.min(aEnd, bEnd) - Math.max(aStart, bStart));
}

/**
 * The pane a directional move from `active` lands on, the way tmux and editor
 * splits resolve it: only panes entirely on that side count, a pane in line
 * with the active one beats a diagonal one, the nearest edge wins, and among
 * equally near panes the one sharing the most of the active pane's span (then
 * the closest centre) wins. Null when nothing is that way.
 */
export function findPaneInDirection(
  active: PaneRect,
  candidates: ReadonlyArray<{ id: string; rect: PaneRect }>,
  direction: PaneDirection,
): string | null {
  const horizontal = direction === "left" || direction === "right";
  const activeCenter = horizontal
    ? (active.top + active.bottom) / 2
    : (active.left + active.right) / 2;

  let best: { id: string; rank: number[] } | null = null;
  for (const { id, rect } of candidates) {
    let gap: number;
    if (direction === "left") gap = active.left - rect.right;
    else if (direction === "right") gap = rect.left - active.right;
    else if (direction === "up") gap = active.top - rect.bottom;
    else gap = rect.top - active.bottom;
    if (gap < -EDGE_SLACK_PX) continue;

    const shared = horizontal
      ? overlap(active.top, active.bottom, rect.top, rect.bottom)
      : overlap(active.left, active.right, rect.left, rect.right);
    const center = horizontal ? (rect.top + rect.bottom) / 2 : (rect.left + rect.right) / 2;
    const drift = Math.abs(center - activeCenter);
    const roundedGap = Math.max(0, Math.round(gap / EDGE_SLACK_PX));
    // Lower wins, compared left to right: a pane in line with the active one
    // beats a diagonal one, then nearer beats farther.
    const rank = [shared > 0 ? 0 : 1, roundedGap, -shared, drift];
    if (!best || isLower(rank, best.rank)) best = { id, rank };
  }
  return best?.id ?? null;
}

function isLower(a: number[], b: number[]): boolean {
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return a[i] < b[i];
  }
  return false;
}

/** On-screen rect of each rendered pane in `ids`, read from the pane shells
 *  `PaneNode` marks with `data-pane-drop-id`. Hidden panes are skipped. */
export function measurePaneRects(ids: readonly string[]): Map<string, PaneRect> {
  const wanted = new Set(ids);
  const rects = new Map<string, PaneRect>();
  for (const el of document.querySelectorAll<HTMLElement>("[data-pane-drop-id]")) {
    const id = el.dataset.paneDropId;
    if (!id || !wanted.has(id)) continue;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) continue;
    rects.set(id, { left: r.left, top: r.top, right: r.right, bottom: r.bottom });
  }
  return rects;
}
