import type { PositionedNode } from "@/lib/openflow-graph";
import { NODE_W, NODE_H } from "@/lib/openflow-graph";
import type { EdgeData } from "@/lib/openflow-graph";

interface AgentEdgeProps {
  edge: EdgeData;
  nodes: Map<string, PositionedNode>;
}

export function AgentEdge({ edge, nodes }: AgentEdgeProps) {
  const source = nodes.get(edge.from);
  const target = nodes.get(edge.to);
  if (!source || !target) return null;

  const sx = source.x + NODE_W / 2;
  const sy = source.y + NODE_H;
  const tx = target.x + NODE_W / 2;
  const ty = target.y;

  const isDownward = ty > sy;
  const isActive = edge.label === "assign" || edge.label === "build" || edge.label === "test";

  let d: string;
  if (isDownward) {
    // Standard downward flow: vertical cubic bezier
    const cy1 = sy + (ty - sy) * 0.4;
    const cy2 = sy + (ty - sy) * 0.6;
    d = `M ${sx} ${sy} C ${sx} ${cy1}, ${tx} ${cy2}, ${tx} ${ty}`;
  } else {
    // Feedback loop: route around the right side
    const offsetX = Math.max(NODE_W, Math.abs(tx - sx)) + 40;
    const midX = Math.max(sx, tx) + offsetX;
    d = `M ${sx} ${sy} C ${midX} ${sy}, ${midX} ${ty}, ${tx} ${ty}`;
  }

  // Label position at midpoint
  const mx = (sx + tx) / 2;
  const my = (sy + ty) / 2;

  return (
    <g>
      <path
        d={d}
        fill="none"
        stroke={isActive ? "var(--color-primary)" : "var(--color-border)"}
        strokeWidth={isActive ? 1.5 : 1}
        strokeDasharray={isActive ? undefined : "4 3"}
        opacity={isActive ? 0.8 : 0.4}
        markerEnd="url(#arrowhead)"
      />
      {isActive && (
        <path
          d={d}
          fill="none"
          stroke="var(--color-primary)"
          strokeWidth={1.5}
          strokeDasharray="8 4"
          opacity={0.6}
          style={{
            animation: "dash-flow 1s linear infinite",
          }}
        />
      )}
      <style>{`@keyframes dash-flow { to { stroke-dashoffset: -12; } }`}</style>
      {edge.label && (
        <text
          x={mx}
          y={my - 6}
          textAnchor="middle"
          fill="var(--color-muted-foreground)"
          fontSize={9}
          className="select-none"
        >
          {edge.label}
        </text>
      )}
    </g>
  );
}
