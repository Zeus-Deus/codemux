import React from "react";
import { DisabledFeaturePlaceholder } from "@/components/layout/disabled-feature-placeholder";
import { Button } from "@/components/ui/button";
import { PresetIcon } from "@/components/icons/preset-icon";
import { cn } from "@/lib/utils";
import { splitPane, closePane, activatePane, resizeSplit, swapPanes } from "@/tauri/commands";
import { SplitSquareHorizontal, SplitSquareVertical, X } from "lucide-react";
import type { PaneNodeSnapshot, PaneStatus } from "@/tauri/types";
import {
  useAppStore,
  useHomeDir,
  useWorkspaceCwdForSession,
} from "@/stores/app-store";
import { useTerminalCwd } from "@/stores/terminal-cwd-store";
import { formatCwdHint } from "@/lib/terminal-cwd";
import { resizeKeyAction } from "@/lib/resize-keys";
import { useFeatureFlags } from "@/stores/feature-flags";
import { StatusIndicator } from "@/components/ui/status-indicator";
import { TerminalBackgroundBrowserIndicator } from "@/components/browser/background-browser-indicator";
import { LazyBoundary } from "@/components/ui/lazy-boundary";
import { PanelHeader } from "@/components/ui/panel-header";

const TerminalPane = React.lazy(() =>
  import("@/components/terminal/TerminalPane").then((module) => ({
    default: module.TerminalPane,
  })),
);
const BrowserPane = React.lazy(() =>
  import("@/components/browser/BrowserPane").then((module) => ({
    default: module.BrowserPane,
  })),
);
const AgentChatPane = React.lazy(() =>
  import("@/components/chat/AgentChatPane").then((module) => ({
    default: module.AgentChatPane,
  })),
);
const AgentChatPaneHeader = React.lazy(() =>
  import("@/components/chat/AgentChatPaneHeader").then((module) => ({
    default: module.AgentChatPaneHeader,
  })),
);

// Map known preset names to their icon identifiers
const PRESET_TITLE_TO_ICON: Record<string, string> = {
  "Claude Code": "claude",
  "Codex": "codex",
  "OpenCode": "opencode",
  "Gemini": "gemini",
  "Antigravity": "antigravity",
  "Copilot": "copilot",
  "Cursor Agent": "cursor-agent",
  "Amp": "amp",
  "Grok": "grok",
  "Droid": "factory",
  "Mastracode": "mastracode",
  "Shell": "terminal",
};

interface Props {
  node: PaneNodeSnapshot;
  activePaneId: string;
  visible: boolean;
  /** Owning workspace, threaded down the pane tree without rescanning a
   *  production-sized snapshot at every split node. */
  workspaceId?: string;
  /** True only for the top-level pane of a surface (not split children).
   *  In GUI chrome a sole-root agent_chat pane drops its header — session
   *  history + close live on the title-bar tab instead. A sole-root terminal
   *  keeps only useful cwd/status context plus pane actions; split children
   *  retain their local title because it identifies the pane. */
  isSurfaceRoot?: boolean;
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

/** Smallest share either side of a split seam may shrink to. */
const MIN_SPLIT_FRACTION = 0.05;

/**
 * Keyboard and double-click control for a split seam. Arrows move the seam
 * by a pixel step converted to the split's ratio; Home/End push it to either
 * end; double-click evens out every child of the split.
 */
function SplitResizeHandle({
  node,
  index,
}: {
  node: PaneNodeSnapshot & { kind: "split" };
  index: number;
}) {
  const sizes = normalizeChildSizes(node.child_sizes, node.children.length);
  const columns = node.direction === "horizontal";
  // A seam between columns is a vertical line.
  const orientation = columns ? "vertical" : "horizontal";

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const action = resizeKeyAction(e, orientation);
    if (!action) return;
    e.preventDefault();
    e.stopPropagation();
    const container = e.currentTarget.closest<HTMLElement>(
      "[data-split-container]",
    );
    if (!container) return;
    // Read the layout from the grid itself, as a drag leaves it: repeated
    // presses land before the backend echoes the previous step back.
    const template = columns
      ? container.style.gridTemplateColumns
      : container.style.gridTemplateRows;
    const parsed = template.split(" ").map((v) => Number.parseFloat(v));
    const current =
      parsed.length === sizes.length && parsed.every(Number.isFinite)
        ? normalizeChildSizes(parsed, parsed.length)
        : sizes;
    const pair = current[index] + current[index + 1];
    const rect = container.getBoundingClientRect();
    const axisSize = columns ? rect.width : rect.height;
    const target =
      action.kind === "min"
        ? 0
        : action.kind === "max"
          ? pair
          : axisSize > 0
            ? current[index] + action.px / axisSize
            : current[index];
    const first = Math.max(
      MIN_SPLIT_FRACTION,
      Math.min(pair - MIN_SPLIT_FRACTION, target),
    );
    const next = [...current];
    next[index] = first;
    next[index + 1] = pair - first;
    const nextTemplate = next.map((s) => `${s}fr`).join(" ");
    if (columns) container.style.gridTemplateColumns = nextTemplate;
    else container.style.gridTemplateRows = nextTemplate;
    resizeSplit(node.pane_id, next).catch(console.error);
  };

  let before = 0;
  for (let i = 0; i <= index; i++) before += sizes[i];

  return (
    <div
      role="separator"
      tabIndex={0}
      aria-orientation={orientation}
      aria-label="Resize split"
      aria-valuenow={Math.round(before * 100)}
      aria-valuemin={0}
      aria-valuemax={100}
      data-testid="split-resize-handle"
      className={cn(
        "group/split absolute z-20 outline-none",
        columns
          ? "top-1 bottom-1 -right-[6.5px] w-3 cursor-col-resize"
          : "left-1 right-1 -bottom-[6.5px] h-3 cursor-row-resize",
      )}
      onPointerDown={(e) => startResize(e, node, index)}
      onKeyDown={handleKeyDown}
      onDoubleClick={() =>
        resizeSplit(
          node.pane_id,
          node.children.map(() => 1 / node.children.length),
        ).catch(console.error)
      }
    >
      {/* The pane borders already draw the seam at rest; this 3px line is
          the hover, drag and focus state, centred on the 1px grid gap. The
          keyboard-focus line is ember, the same "your keys drive this" cue
          as the active pane's border, so where the two meet it continues
          that border instead of greying it out. The handle is inset 4px at
          each end; the line overhangs by the same 4px so it runs the full
          seam and meets the pane borders without a notch. */}
      <span
        aria-hidden
        className={cn(
          "pointer-events-none absolute rounded-full bg-transparent transition-colors duration-100",
          "group-hover/split:bg-foreground/30 group-focus-visible/split:bg-accent-ember/60 group-data-[dragging=true]/split:bg-foreground/40",
          columns
            ? "-inset-y-1 left-1/2 w-[3px] -translate-x-1/2"
            : "-inset-x-1 top-1/2 h-[3px] -translate-y-1/2",
        )}
      />
    </div>
  );
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

function PaneNodeImpl({
  node,
  activePaneId,
  visible,
  workspaceId,
  isSurfaceRoot = false,
}: Props) {
  // #127: hooks hoisted above the split branch so hook order stays stable if a
  // fiber flips between split↔leaf at the same position (the old code called
  // these AFTER an early `return` for split nodes — a conditional-hook bug that
  // would throw on such a swap). For split nodes the pane_statuses selector just
  // returns undefined — harmless — and it stays a stable primitive slice under
  // structural sharing.
  const paneStatus: PaneStatus | undefined = useAppStore(
    (s) => s.appState?.pane_statuses[node.pane_id],
  );
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);

  // Terminal pane cwd hint. Hoisted above the split branch with the other
  // hooks (#127) — non-terminal nodes pass an id no session can match, so
  // every selector returns null and the hint is skipped.
  const terminalSessionId = node.kind === "terminal" ? node.session_id : "";
  const sessionCwd = useTerminalCwd(terminalSessionId);
  const workspaceCwd = useWorkspaceCwdForSession(terminalSessionId);
  const homeDir = useHomeDir();
  const cwdHint = React.useMemo(
    () => formatCwdHint(sessionCwd, workspaceCwd, homeDir),
    [sessionCwd, workspaceCwd, homeDir],
  );

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
          // The cell itself is not clipped, so the seam handle can reach past
          // its edge and its centre, not just its near half, is hit-testable.
          // The pane is clipped by its own wrapper instead: a terminal sized
          // wider than its cell would otherwise paint over the neighbour.
          <div key={child.pane_id} className="relative min-w-0 min-h-0">
            <div data-split-cell-clip className="size-full min-w-0 min-h-0 overflow-hidden">
              <PaneNode
                node={child}
                activePaneId={activePaneId}
                visible={visible}
                workspaceId={workspaceId}
              />
            </div>
            {i < node.children.length - 1 && (
              <SplitResizeHandle node={node} index={i} />
            )}
          </div>
        ))}
      </div>
    );
  }

  const isActive = node.pane_id === activePaneId;
  // In a split, near-identical panes make it easy to type into the wrong
  // one, so the pane that takes keystrokes carries an accent border. A sole
  // pane has nothing to be confused with and stays chrome-free.
  const paneShell = cn(
    "group/pane flex h-full w-full flex-col min-w-0 min-h-0 overflow-hidden border transition-[border-color] duration-150",
    isSurfaceRoot
      ? "border-border/30"
      : isActive
        ? "border-accent-ember/45"
        : // The other panes still need an edge, or the seam between two
          // dark panes disappears into the background.
          "border-hairline-strong",
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
    const showTerminalTitle = !isSurfaceRoot;
    const showTerminalStatus = paneStatus && paneStatus !== "idle";
    const showTerminalContext = showTerminalTitle || cwdHint || showTerminalStatus;

    return (
      <div
        className={paneShell}
        data-pane-drop-id={node.pane_id}
        data-pane-title={node.title}
        onPointerDown={handleActivate}
      >
        <header
          className="relative flex h-7 shrink-0 items-center gap-1 px-1.5 cursor-grab active:cursor-grabbing"
          data-terminal-pane-chrome
          onPointerDown={(e) => handleDragStart(e, node.pane_id)}
        >
          {showTerminalContext && (
            <span
              className={cn(
                "flex h-6 min-w-0 max-w-[70%] items-center gap-1.5 rounded-md px-2 text-label shadow-sm ring-1 ring-border/30 backdrop-blur-md",
                isActive
                  ? "bg-card/70 text-muted-foreground"
                  : "bg-background/65 text-muted-foreground/70",
              )}
              data-terminal-pane-context
            >
              {showTerminalTitle && PRESET_TITLE_TO_ICON[node.title] && (
                <PresetIcon icon={PRESET_TITLE_TO_ICON[node.title]} className="size-3" />
              )}
              {/* A split pane needs a local identity. A sole-root terminal is
                  already named by the workspace tab, so repeating "Terminal"
                  here only recreates the second full-width header visually. */}
              {showTerminalTitle && (
                <span className="shrink-0 text-foreground/80">{node.title}</span>
              )}
              {/* Live working directory, rendered only when it differs from the
                  workspace root (see `formatCwdHint`). Mono per the design
                  system's rule that path-like metadata is code-like. The
                  untrimmed path is on the tooltip since the label elides its
                  head to keep the meaningful tail. */}
              {cwdHint && (
                <span
                  className="min-w-0 truncate font-mono text-label text-muted-foreground/70"
                  title={cwdHint.full}
                  data-pane-cwd={cwdHint.full}
                >
                  {cwdHint.label}
                </span>
              )}
              {showTerminalStatus && (
                <span className="shrink-0">
                  <StatusIndicator status={paneStatus} />
                </span>
              )}
            </span>
          )}
          <div
            className={cn(
              "ml-auto flex h-6 items-center gap-0.5 rounded-md bg-card/65 px-0.5 shadow-sm ring-1 ring-border/30 backdrop-blur-md transition-opacity duration-150",
              isActive ? "opacity-70" : "opacity-0",
              "group-hover/pane:opacity-100 focus-within:opacity-100",
            )}
            data-terminal-pane-actions
          >
            <TerminalBackgroundBrowserIndicator active={isActive} />
            <Button variant="ghost" size="icon-xs" className="text-muted-foreground hover:text-foreground" onClick={() => handleSplit("horizontal")} aria-label="Split right" title="Split right">
              <SplitSquareHorizontal />
            </Button>
            <Button variant="ghost" size="icon-xs" className="text-muted-foreground hover:text-foreground" onClick={() => handleSplit("vertical")} aria-label="Split down" title="Split down">
              <SplitSquareVertical />
            </Button>
            <Button variant="ghost" size="icon-xs" className="text-muted-foreground hover:bg-destructive/80 hover:text-destructive-foreground" onClick={handleClose} aria-label="Close pane" title="Close pane">
              <X />
            </Button>
          </div>
        </header>
        <div className="flex-1 min-h-0 overflow-hidden">
          <LazyBoundary label="terminal" className="h-full">
            <TerminalPane
              sessionId={node.session_id}
              paneId={node.pane_id}
              focused={isActive}
              visible={visible}
              title={node.title}
            />
          </LazyBoundary>
        </div>
      </div>
    );
  }

  if (node.kind === "agent_chat") {
    // Step 13 — render a placeholder when the master Beta toggle is
    // off and the persisted layout still references this pane. The
    // pane node stays in the tree (data preservation) — the user can
    // re-enable the Beta toggle and the pane remounts with its
    // session intact.
    if (!enableAgentChat) {
      return (
        <div
          className={paneShell}
          data-pane-drop-id={node.pane_id}
          onPointerDown={handleActivate}
        >
          <DisabledFeaturePlaceholder feature="Agent Chat" />
        </div>
      );
    }
    // GUI chrome (Agent Chat Beta on) collapses a sole-root chat pane's
    // header into the title-bar tab: history + close move up there, so
    // rendering the per-pane header would double the chrome. Split panes
    // (isSurfaceRoot false) always keep their header for per-pane
    // split/close/drag.
    const hideChatHeader = enableAgentChat && isSurfaceRoot;
    return (
      <div
        className={paneShell}
        data-pane-drop-id={node.pane_id}
        onPointerDown={handleActivate}
      >
        {!hideChatHeader && (
          <LazyBoundary label="chat header" className="h-7 min-h-7">
            <AgentChatPaneHeader
              pane={node}
              isActive={isActive}
              onPointerDown={(e) => handleDragStart(e, node.pane_id)}
            />
          </LazyBoundary>
        )}
        <div className="flex-1 min-h-0 overflow-hidden">
          {/*
            `key={node.pane_id}` is REQUIRED for per-pane isolation.
            Without it, React reconciles the existing `AgentChatPane`
            Fiber across pane swaps (e.g. switching tabs in a workspace
            with multiple chat panes, or a Chat-Agent preset click that
            opens a new tab): the JSX shape is the same so the prior
            pane's `useState<threadId>` value survives onto the new
            pane, the mount effect's `if (threadId) { ensureThread;
            return; }` early-exit fires, and the new pane subscribes to
            the previous pane's Zustand slice — so both panes render
            the same chat. Keying by pane_id forces a clean unmount/
            remount and matches CLI panes' per-session_id isolation.
          */}
          <LazyBoundary label="agent chat" className="h-full">
            <AgentChatPane key={node.pane_id} pane={node} />
          </LazyBoundary>
        </div>
      </div>
    );
  }

  if (node.kind === "browser") {
    return (
      <div
        className={paneShell}
        data-pane-drop-id={node.pane_id}
        onPointerDown={handleActivate}
      >
        <PanelHeader
          className={cn("gap-1 cursor-grab active:cursor-grabbing transition-colors duration-150", isActive ? "bg-card" : "bg-background")}
          onPointerDown={(e) => handleDragStart(e, node.pane_id)}
        >
          <span className="flex-1 truncate text-label text-muted-foreground">
            {node.title}
          </span>
          <div className="flex items-center gap-0.5 opacity-0 transition-opacity duration-150 group-hover/pane:opacity-100">
            <Button variant="ghost" size="icon-sm" className="text-muted-foreground hover:text-foreground" onClick={() => handleSplit("horizontal")} aria-label="Split right" title="Split right">
              <SplitSquareHorizontal className="size-3.5" />
            </Button>
            <Button variant="ghost" size="icon-sm" className="text-muted-foreground hover:text-foreground" onClick={() => handleSplit("vertical")} aria-label="Split down" title="Split down">
              <SplitSquareVertical className="size-3.5" />
            </Button>
            <Button variant="ghost" size="icon-sm" className="text-muted-foreground hover:bg-destructive/80 hover:text-destructive-foreground" onClick={handleClose} aria-label="Close pane" title="Close pane">
              <X className="size-3.5" />
            </Button>
          </div>
        </PanelHeader>
        <div className="flex-1 min-h-0 overflow-hidden">
          <LazyBoundary label="browser" className="h-full">
            <BrowserPane
              browserId={node.browser_id}
              focused={isActive}
              visible={visible}
              traceWorkspaceId={workspaceId}
            />
          </LazyBoundary>
        </div>
      </div>
    );
  }

  return null;
}

// #127: React.memo pays off because setAppState now performs structural sharing
// — unchanged pane subtrees keep a stable `node` ref across the backend's
// ~60Hz snapshot re-emits, so the default shallow prop compare skips
// reconciliation. The recursive `<PaneNode>` in the split branch references
// this memoized const (not `PaneNodeImpl`), so nested panes memoize too.
export const PaneNode = React.memo(PaneNodeImpl);
PaneNode.displayName = "PaneNode";
