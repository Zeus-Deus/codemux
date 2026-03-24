import { useMemo, useRef, useState, useEffect } from "react";
import type { AgentSessionState, CommLogEntry, OpenFlowRunRecord } from "@/tauri/types";
import {
  buildAgentNodes,
  buildActiveConnections,
  positionNodes,
  getTotalGraphHeight,
} from "@/lib/openflow-graph";
import { AgentNode } from "./agent-node";
import { AgentEdge } from "./agent-edge";

interface AgentGraphProps {
  run: OpenFlowRunRecord;
  agentSessions: AgentSessionState[];
  commLog: CommLogEntry[];
  onAgentClick?: (instanceId: string) => void;
}

export function AgentGraph({
  run,
  agentSessions,
  commLog,
  onAgentClick,
}: AgentGraphProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerWidth, setContainerWidth] = useState(600);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        setContainerWidth(entry.contentRect.width);
      }
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const agentNodes = useMemo(
    () => buildAgentNodes(run, agentSessions, commLog),
    [run, agentSessions, commLog],
  );

  const positioned = useMemo(
    () => positionNodes(agentNodes, containerWidth),
    [agentNodes, containerWidth],
  );

  const edges = useMemo(
    () => buildActiveConnections(run, agentNodes, commLog, agentSessions),
    [run, agentNodes, commLog, agentSessions],
  );

  const nodeMap = useMemo(
    () => new Map(positioned.map((n) => [n.id, n])),
    [positioned],
  );

  const graphHeight = getTotalGraphHeight(agentNodes);

  return (
    <div
      ref={containerRef}
      className="relative h-full w-full overflow-auto"
      style={{
        backgroundImage:
          "radial-gradient(circle, hsl(var(--border) / 0.3) 1px, transparent 1px)",
        backgroundSize: "20px 20px",
      }}
    >
      <div
        className="relative"
        style={{ minHeight: graphHeight, minWidth: 300 }}
      >
        {/* SVG edge layer */}
        <svg
          className="absolute inset-0 pointer-events-none"
          width="100%"
          height={graphHeight}
        >
          <defs>
            <marker
              id="arrowhead"
              markerWidth="8"
              markerHeight="6"
              refX="7"
              refY="3"
              orient="auto"
            >
              <polygon
                points="0 0, 8 3, 0 6"
                fill="var(--color-muted-foreground)"
                opacity="0.5"
              />
            </marker>
          </defs>
          {edges.map((edge, i) => (
            <AgentEdge key={`${edge.from}-${edge.to}-${i}`} edge={edge} nodes={nodeMap} />
          ))}
        </svg>

        {/* DOM node layer */}
        {positioned.map((node) => (
          <AgentNode
            key={node.id}
            node={node}
            onClick={() => onAgentClick?.(node.id)}
          />
        ))}
      </div>
    </div>
  );
}
