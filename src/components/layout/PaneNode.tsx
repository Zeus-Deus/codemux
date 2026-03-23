import { TerminalPane } from "@/components/terminal/TerminalPane";
import { splitPane, closePane, activatePane } from "@/tauri/commands";
import { Globe, SplitSquareHorizontal, SplitSquareVertical, X } from "lucide-react";
import type { PaneNodeSnapshot } from "@/tauri/types";

interface Props {
  node: PaneNodeSnapshot;
  activePaneId: string;
  visible: boolean;
}

export function PaneNode({ node, activePaneId, visible }: Props) {
  if (node.kind === "split") {
    const sizes = node.child_sizes.map((s) => `${Math.max(s, 0.1)}fr`);
    const gridStyle: React.CSSProperties =
      node.direction === "horizontal"
        ? { display: "grid", gridTemplateColumns: sizes.join(" "), gap: "2px", height: "100%", width: "100%" }
        : { display: "grid", gridTemplateRows: sizes.join(" "), gap: "2px", height: "100%", width: "100%" };

    return (
      <div style={gridStyle}>
        {node.children.map((child) => (
          <PaneNode
            key={child.pane_id}
            node={child}
            activePaneId={activePaneId}
            visible={visible}
          />
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
        <header className="flex h-7 shrink-0 items-center gap-1 border-b border-border/50 bg-card px-2">
          <span className="flex-1 truncate text-xs text-muted-foreground">
            {node.title}
          </span>
          <div className="flex items-center gap-0.5 opacity-0 transition-opacity group-hover/pane:opacity-100 [div:hover>&]:opacity-100">
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

  // Browser pane placeholder
  if (node.kind === "browser") {
    return (
      <div
        className={`group/pane flex h-full w-full flex-col min-w-0 min-h-0 overflow-hidden border ${
          isActive ? "border-primary/50" : "border-border/50"
        }`}
        data-pane-drop-id={node.pane_id}
        onPointerDown={handleActivate}
      >
        <header className="flex h-7 shrink-0 items-center gap-1 border-b border-border/50 bg-card px-2">
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
