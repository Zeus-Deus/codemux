import type { Connection, AgentNodeData } from '../components/openflow/NodeGraph.svelte';
import type { AgentSessionState, CommLogEntry, OpenFlowRunRecord } from '../stores/types';

function resolveNodeId(agentNodes: AgentNodeData[], raw: string): string | null {
    const lower = raw.toLowerCase().trim();
    const exact = agentNodes.find((node) => node.id === lower);
    if (exact) {
        return exact.id;
    }

    const byRole = agentNodes.find((node) => node.role.toLowerCase() === lower);
    return byRole ? byRole.id : null;
}

export function buildAgentNodes(
    run: OpenFlowRunRecord | null,
    agentSessions: AgentSessionState[],
    commLogEntries: CommLogEntry[],
): AgentNodeData[] {
    if (!run) {
        return [];
    }

    const sessionByInstanceId = new Map(agentSessions.map((session) => {
        const instanceId = session.config.role === 'orchestrator'
            ? 'orchestrator'
            : `${session.config.role}-${session.config.agent_index}`;
        return [instanceId, session];
    }));

    return run.workers.map((worker, index) => {
        const roleLower = worker.role.toLowerCase();
        const instanceId = roleLower === 'orchestrator' ? 'orchestrator' : `${roleLower}-${index}`;
        const session = sessionByInstanceId.get(instanceId)
            ?? agentSessions.find((candidate) => candidate.config.role === worker.role);

        let dynamicStatus = worker.status;
        const hasSession = session !== undefined;

        // Skip dynamic status detection for terminal runs
        const isRunTerminal = run.status === 'completed' || run.status === 'failed' || run.status === 'cancelled';

        if (!isRunTerminal) {
            const instanceEntries = commLogEntries.filter((entry) =>
                entry.role.toLowerCase() === instanceId || entry.role.toLowerCase() === roleLower
            );
            const lastEntry = instanceEntries.length > 0 ? instanceEntries[instanceEntries.length - 1] : null;

            if (!hasSession && worker.status !== 'done' && worker.status !== 'completed') {
                dynamicStatus = 'dead';
            } else if (lastEntry) {
                const lastMessage = lastEntry.message.toLowerCase();
                if (lastMessage.includes('done:') || lastMessage.includes('run complete')) {
                    dynamicStatus = 'done';
                } else if (lastMessage.includes('blocked:')) {
                    dynamicStatus = 'blocked';
                } else {
                    // Timestamp-based active/idle detection (15s threshold)
                    const entryTime = new Date(lastEntry.timestamp).getTime();
                    const ageMs = Date.now() - entryTime;
                    if (ageMs < 15_000) {
                        dynamicStatus = 'active';
                    } else {
                        dynamicStatus = 'idle';
                    }
                }
            }
        }

        return {
            id: instanceId,
            role: worker.role,
            status: dynamicStatus,
            model: session?.config.model ?? null,
            thinkingMode: session?.config.thinking_mode ?? null,
        };
    });
}

export function buildActiveConnections(
    run: OpenFlowRunRecord | null,
    agentNodes: AgentNodeData[],
    commLogEntries: CommLogEntry[],
    agentSessions: AgentSessionState[] = [],
): Connection[] {
    if (!run || agentNodes.length === 0) {
        return [];
    }

    const isRunTerminal = run.status === 'completed' || run.status === 'failed' || run.status === 'cancelled';
    const hasActiveSessions = agentSessions.length > 0;

    if (isRunTerminal || !hasActiveSessions) {
        return [];
    }

    const connections: Connection[] = [];
    // 30s staleness window for connections
    const now = Date.now();
    const stalenessMs = 30_000;
    const recentEntries = commLogEntries
        .filter((entry) => {
            const t = new Date(entry.timestamp).getTime();
            return !isNaN(t) && now - t < stalenessMs;
        })
        .slice(-10);

    for (const entry of recentEntries) {
        if (entry.role.toLowerCase() !== 'orchestrator') {
            continue;
        }

        const assignMatch = entry.message.match(/ASSIGN\s+([A-Z]+-\d+|[A-Z]+)\s*:/i);
        if (!assignMatch) {
            continue;
        }

        const orchestratorId = resolveNodeId(agentNodes, 'orchestrator');
        const targetId = resolveNodeId(agentNodes, assignMatch[1]!);
        if (orchestratorId && targetId) {
            connections.push({ from: orchestratorId, to: targetId, label: 'assign' });
        }
    }

    for (const entry of recentEntries) {
        const senderRole = entry.role.toLowerCase();
        if (senderRole === 'system' || senderRole === 'orchestrator') {
            continue;
        }

        const senderId = resolveNodeId(agentNodes, senderRole);
        if (!senderId) {
            continue;
        }

        const message = entry.message.toLowerCase();
        if (message.includes('done:') || message.includes('run complete')) {
            const orchestratorId = resolveNodeId(agentNodes, 'orchestrator');
            if (orchestratorId) {
                connections.push({ from: senderId, to: orchestratorId, label: 'done' });
            }
        } else if (message.includes('blocked:')) {
            const debuggerId = resolveNodeId(agentNodes, 'debugger');
            if (debuggerId) {
                connections.push({ from: senderId, to: debuggerId, label: 'blocked' });
            }
        }
    }

    if (connections.length === 0) {
        const orchestratorId = resolveNodeId(agentNodes, 'orchestrator');
        if (orchestratorId) {
            if (run.current_phase === 'plan' || run.current_phase === 'planning') {
                for (const node of agentNodes.filter((candidate) => candidate.role.toLowerCase() === 'planner')) {
                    connections.push({ from: orchestratorId, to: node.id, label: 'planning' });
                }
                for (const node of agentNodes.filter((candidate) => candidate.role.toLowerCase() === 'researcher')) {
                    connections.push({ from: orchestratorId, to: node.id, label: 'research' });
                }
            } else if (run.current_phase === 'execute' || run.current_phase === 'executing') {
                for (const node of agentNodes.filter((candidate) => candidate.role.toLowerCase() === 'builder')) {
                    connections.push({ from: orchestratorId, to: node.id, label: 'build' });
                }
            } else if (run.current_phase === 'verify' || run.current_phase === 'verifying') {
                for (const node of agentNodes.filter((candidate) => candidate.role.toLowerCase() === 'tester')) {
                    connections.push({ from: orchestratorId, to: node.id, label: 'test' });
                }
            } else if (run.current_phase === 'review' || run.current_phase === 'reviewing') {
                for (const node of agentNodes.filter((candidate) => candidate.role.toLowerCase() === 'reviewer')) {
                    connections.push({ from: orchestratorId, to: node.id, label: 'review' });
                    connections.push({ from: node.id, to: orchestratorId, label: 'feedback' });
                }
            }
        }
    }

    const seen = new Set<string>();
    return connections.filter((connection) => {
        const key = `${connection.from}->${connection.to}`;
        if (seen.has(key)) {
            return false;
        }
        seen.add(key);
        return true;
    });
}
