import React from "react";
import { TerminalPane } from "@/components/terminal/TerminalPane";
import { BrowserPane } from "@/components/browser/BrowserPane";
import { AgentChatPane } from "@/components/chat/AgentChatPane";
import { AgentChatPaneHeader } from "@/components/chat/AgentChatPaneHeader";
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
import { useFeatureFlags } from "@/stores/feature-flags";
import { StatusIndicator } from "@/components/ui/status-indicator";

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

function PaneNodeImpl({ node, activePaneId, visible, isSurfaceRoot = false }: Props) {
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
          <div key={child.pane_id} className="relative min-w-0 min-h-0 overflow-hidden">
            <PaneNode node={child} activePaneId={activePaneId} visible={visible} />
            {i < node.children.length - 1 && (
              <div
                className={`absolute z-20 opacity-0 hover:opacity-100 data-[dragging=true]:opacity-100 transition-opacity duration-100 ${
                  node.direction === "horizontal"
                    ? "top-1 bottom-1 -right-[6px] w-3 cursor-col-resize"
                    : "left-1 right-1 -bottom-[6px] h-3 cursor-row-resize"
                } bg-foreground/20 hover:bg-foreground/30 data-[dragging=true]:bg-foreground/30 rounded-full`}
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
    const showTerminalTitle = !isSurfaceRoot;
    const showTerminalStatus = paneStatus && paneStatus !== "idle";
    const showTerminalContext = showTerminalTitle || cwdHint || showTerminalStatus;

    return (
      <div
        className="group/pane flex h-full w-full flex-col min-w-0 min-h-0 overflow-hidden border border-border/30"
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
                "flex h-6 min-w-0 max-w-[70%] items-center gap-1.5 rounded-md px-2 text-xs shadow-sm ring-1 ring-border/30 backdrop-blur-md",
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
                  className="min-w-0 truncate font-mono text-[11px] text-muted-foreground/70"
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

  if (node.kind === "agent_chat") {
    // Step 13 — render a placeholder when the master Beta toggle is
    // off and the persisted layout still references this pane. The
    // pane node stays in the tree (data preservation) — the user can
    // re-enable the Beta toggle and the pane remounts with its
    // session intact.
    if (!enableAgentChat) {
      return (
        <div
          className="group/pane flex h-full w-full flex-col min-w-0 min-h-0 overflow-hidden border border-border/30"
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
        className="group/pane flex h-full w-full flex-col min-w-0 min-h-0 overflow-hidden border border-border/30"
        data-pane-drop-id={node.pane_id}
        onPointerDown={handleActivate}
      >
        {!hideChatHeader && (
          <AgentChatPaneHeader
            pane={node}
            isActive={isActive}
            onPointerDown={(e) => handleDragStart(e, node.pane_id)}
          />
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
          <AgentChatPane key={node.pane_id} pane={node} />
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
            <Button variant="ghost" size="icon-sm" className="text-muted-foreground hover:text-foreground" onClick={() => handleSplit("horizontal")} aria-label="Split right" title="Split right">
              <SplitSquareHorizontal className="h-3.5 w-3.5" />
            </Button>
            <Button variant="ghost" size="icon-sm" className="text-muted-foreground hover:text-foreground" onClick={() => handleSplit("vertical")} aria-label="Split down" title="Split down">
              <SplitSquareVertical className="h-3.5 w-3.5" />
            </Button>
            <Button variant="ghost" size="icon-sm" className="text-muted-foreground hover:bg-destructive/80 hover:text-destructive-foreground" onClick={handleClose} aria-label="Close pane" title="Close pane">
              <X className="h-3.5 w-3.5" />
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

// #127: React.memo pays off because setAppState now performs structural sharing
// — unchanged pane subtrees keep a stable `node` ref across the backend's
// ~60Hz snapshot re-emits, so the default shallow prop compare skips
// reconciliation. The recursive `<PaneNode>` in the split branch references
// this memoized const (not `PaneNodeImpl`), so nested panes memoize too.
export const PaneNode = React.memo(PaneNodeImpl);
PaneNode.displayName = "PaneNode";
