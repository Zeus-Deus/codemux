import type { PositionedNode } from "@/lib/openflow-graph";
import { NODE_W, NODE_H } from "@/lib/openflow-graph";
import {
  getStatusDotClass,
  getRoleEmoji,
  formatRole,
} from "@/lib/openflow-utils";
import { cn } from "@/lib/utils";

interface AgentNodeProps {
  node: PositionedNode;
  onClick?: () => void;
}

export function AgentNode({ node, onClick }: AgentNodeProps) {
  const isActive = node.status === "active";
  const shortModel = node.model?.split("/").pop() ?? null;

  return (
    <button
      className={cn(
        "absolute rounded-lg border bg-card/80 backdrop-blur-sm p-2 text-left transition-all hover:bg-card cursor-pointer",
        isActive && "border-primary/40 shadow-[0_0_8px_-2px] shadow-primary/20",
        node.status === "done" && "border-emerald-400/30",
        node.status === "blocked" && "border-red-400/30",
        node.status === "dead" && "border-red-400/30 opacity-50",
        !isActive &&
          node.status !== "done" &&
          node.status !== "blocked" &&
          node.status !== "dead" &&
          "border-border/60",
      )}
      style={{
        left: node.x,
        top: node.y,
        width: NODE_W,
        height: NODE_H,
      }}
      onClick={onClick}
    >
      {/* Active indicator */}
      {isActive && (
        <span className="absolute -top-1 -right-1 flex h-2.5 w-2.5">
          <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-primary/60" />
          <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-primary" />
        </span>
      )}

      <div className="flex items-center gap-1.5">
        <span className="text-sm leading-none">{getRoleEmoji(node.role)}</span>
        <span className="text-[11px] font-semibold text-foreground truncate">
          {formatRole(node.id)}
        </span>
      </div>

      <div className="mt-1 flex items-center gap-1.5">
        <span
          className={cn(
            "h-1.5 w-1.5 shrink-0 rounded-full",
            getStatusDotClass(node.status),
          )}
        />
        <span className="text-[10px] text-muted-foreground capitalize truncate">
          {node.status}
        </span>
        {shortModel && (
          <>
            <span className="text-[10px] text-muted-foreground/40">·</span>
            <span className="text-[10px] text-muted-foreground/70 truncate">
              {shortModel}
            </span>
          </>
        )}
      </div>
    </button>
  );
}
