import type {
  AgentSessionState,
  CommLogEntry,
  OpenFlowRunRecord,
  OpenFlowRole,
} from "@/tauri/types";

// ── Types ──

export interface AgentNodeData {
  id: string;
  role: OpenFlowRole;
  status: string;
  model: string | null;
  thinkingMode: string | null;
}

export interface EdgeData {
  from: string;
  to: string;
  label: string;
}

export interface PositionedNode extends AgentNodeData {
  x: number;
  y: number;
}

// ── Constants ──

export const NODE_W = 160;
export const NODE_H = 70;
const GAP_X = 40;
const GAP_Y = 60;

const ROLE_ORDER: OpenFlowRole[] = [
  "orchestrator",
  "researcher",
  "planner",
  "builder",
  "tester",
  "debugger",
  "reviewer",
];

// ── Node Building ──

function resolveNodeId(
  nodes: AgentNodeData[],
  raw: string,
): string | null {
  const lower = raw.toLowerCase().trim();
  const exact = nodes.find((n) => n.id === lower);
  if (exact) return exact.id;
  const byRole = nodes.find((n) => n.role.toLowerCase() === lower);
  return byRole ? byRole.id : null;
}

export function buildAgentNodes(
  run: OpenFlowRunRecord | null,
  agentSessions: AgentSessionState[],
  commLogEntries: CommLogEntry[],
): AgentNodeData[] {
  if (!run) return [];

  const sessionByInstanceId = new Map(
    agentSessions.map((s) => {
      const instanceId =
        s.config.role === "orchestrator"
          ? "orchestrator"
          : `${s.config.role}-${s.config.agent_index}`;
      return [instanceId, s];
    }),
  );

  return run.workers.map((worker, index) => {
    const roleLower = worker.role.toLowerCase() as OpenFlowRole;
    const instanceId =
      roleLower === "orchestrator" ? "orchestrator" : `${roleLower}-${index}`;
    const session =
      sessionByInstanceId.get(instanceId) ??
      agentSessions.find((c) => c.config.role === worker.role);

    let dynamicStatus = worker.status;
    const hasSession = session !== undefined;
    const isRunTerminal =
      run.status === "completed" ||
      run.status === "failed" ||
      run.status === "cancelled";

    if (!isRunTerminal) {
      const instanceEntries = commLogEntries.filter(
        (e) =>
          e.role.toLowerCase() === instanceId ||
          e.role.toLowerCase() === roleLower,
      );
      const lastEntry =
        instanceEntries.length > 0
          ? instanceEntries[instanceEntries.length - 1]
          : null;

      if (
        !hasSession &&
        worker.status !== "done" &&
        worker.status !== "completed"
      ) {
        dynamicStatus = "dead";
      } else if (lastEntry) {
        const msg = lastEntry.message.toLowerCase();
        if (msg.includes("done:") || msg.includes("run complete")) {
          dynamicStatus = "done";
        } else if (msg.includes("blocked:")) {
          dynamicStatus = "blocked";
        } else {
          const ageMs = Date.now() - new Date(lastEntry.timestamp).getTime();
          dynamicStatus = ageMs < 15_000 ? "active" : "idle";
        }
      }
    }

    return {
      id: instanceId,
      role: roleLower,
      status: dynamicStatus,
      model: session?.config.model ?? null,
      thinkingMode: session?.config.thinking_mode ?? null,
    };
  });
}

// ── Positioning ──

export function positionNodes(
  nodes: AgentNodeData[],
  containerWidth: number,
): PositionedNode[] {
  if (nodes.length === 0) return [];

  // Group nodes by role, ordered by ROLE_ORDER
  const rows: AgentNodeData[][] = [];
  for (const role of ROLE_ORDER) {
    const group = nodes.filter(
      (n) => n.role.toLowerCase() === role,
    );
    if (group.length > 0) rows.push(group);
  }

  const positioned: PositionedNode[] = [];
  let y = 20;

  for (const row of rows) {
    const totalWidth = row.length * NODE_W + (row.length - 1) * GAP_X;
    let x = Math.max(20, (containerWidth - totalWidth) / 2);

    for (const node of row) {
      positioned.push({ ...node, x, y });
      x += NODE_W + GAP_X;
    }
    y += NODE_H + GAP_Y;
  }

  return positioned;
}

export function getTotalGraphHeight(nodes: AgentNodeData[]): number {
  const roleSet = new Set(nodes.map((n) => n.role.toLowerCase()));
  const rowCount = ROLE_ORDER.filter((r) => roleSet.has(r)).length;
  return rowCount * (NODE_H + GAP_Y) + 20;
}

// ── Connection Building ──

export function buildActiveConnections(
  run: OpenFlowRunRecord | null,
  agentNodes: AgentNodeData[],
  commLogEntries: CommLogEntry[],
  agentSessions: AgentSessionState[] = [],
): EdgeData[] {
  if (!run || agentNodes.length === 0) return [];

  const isRunTerminal =
    run.status === "completed" ||
    run.status === "failed" ||
    run.status === "cancelled";
  if (isRunTerminal || agentSessions.length === 0) return [];

  const connections: EdgeData[] = [];
  const now = Date.now();
  const stalenessMs = 30_000;
  const recentEntries = commLogEntries
    .filter((e) => {
      const t = new Date(e.timestamp).getTime();
      return !isNaN(t) && now - t < stalenessMs;
    })
    .slice(-10);

  // Parse ASSIGN directives from orchestrator
  for (const entry of recentEntries) {
    if (entry.role.toLowerCase() !== "orchestrator") continue;
    const assignMatch = entry.message.match(
      /ASSIGN\s+([A-Z]+-\d+|[A-Z]+)\s*:/i,
    );
    if (!assignMatch) continue;

    const orchestratorId = resolveNodeId(agentNodes, "orchestrator");
    const targetId = resolveNodeId(agentNodes, assignMatch[1]!);
    if (orchestratorId && targetId) {
      connections.push({ from: orchestratorId, to: targetId, label: "assign" });
    }
  }

  // Parse worker completions and blockages
  for (const entry of recentEntries) {
    const senderRole = entry.role.toLowerCase();
    if (senderRole === "system" || senderRole === "orchestrator") continue;

    const senderId = resolveNodeId(agentNodes, senderRole);
    if (!senderId) continue;

    const message = entry.message.toLowerCase();
    if (message.includes("done:") || message.includes("run complete")) {
      const orchestratorId = resolveNodeId(agentNodes, "orchestrator");
      if (orchestratorId)
        connections.push({ from: senderId, to: orchestratorId, label: "done" });
    } else if (message.includes("blocked:")) {
      const debuggerId = resolveNodeId(agentNodes, "debugger");
      if (debuggerId)
        connections.push({
          from: senderId,
          to: debuggerId,
          label: "blocked",
        });
    }
  }

  // Fallback: phase-based default connections
  if (connections.length === 0) {
    const orchestratorId = resolveNodeId(agentNodes, "orchestrator");
    if (orchestratorId) {
      const phase = run.current_phase.toLowerCase();
      if (phase === "plan" || phase === "planning") {
        for (const n of agentNodes.filter((c) => c.role === "planner"))
          connections.push({ from: orchestratorId, to: n.id, label: "planning" });
        for (const n of agentNodes.filter((c) => c.role === "researcher"))
          connections.push({ from: orchestratorId, to: n.id, label: "research" });
      } else if (phase === "execute" || phase === "executing") {
        for (const n of agentNodes.filter((c) => c.role === "builder"))
          connections.push({ from: orchestratorId, to: n.id, label: "build" });
      } else if (phase === "verify" || phase === "verifying") {
        for (const n of agentNodes.filter((c) => c.role === "tester"))
          connections.push({ from: orchestratorId, to: n.id, label: "test" });
      } else if (phase === "review" || phase === "reviewing") {
        for (const n of agentNodes.filter((c) => c.role === "reviewer")) {
          connections.push({ from: orchestratorId, to: n.id, label: "review" });
          connections.push({ from: n.id, to: orchestratorId, label: "feedback" });
        }
      }
    }
  }

  // Deduplicate
  const seen = new Set<string>();
  return connections.filter((c) => {
    const key = `${c.from}->${c.to}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
