import { invoke } from '@tauri-apps/api/core';
import { writable } from 'svelte/store';
import type {
    AgentConfig,
    AgentSessionState,
    CliToolInfo,
    CommLogEntry,
    ModelInfo,
    OpenFlowCreateRunRequest,
    OpenFlowRunRecord,
    OpenFlowRuntimeSnapshot,
    OrchestratorTriggerResult,
    ThinkingModeInfo,
} from './types';

export const openflowRuntime = writable<OpenFlowRuntimeSnapshot | null>(null);
export const commLogStore = writable<CommLogEntry[]>([]);

const commLogOffsets = new Map<string, number>();

export function clearCommLogOffset(runId: string) {
    commLogOffsets.delete(runId);
}

function clearOrchestratorOffset(runId: string) {
    commLogOffsets.delete(runId);
}

export async function syncOpenFlowRuntime() {
    const snapshot = await invoke<OpenFlowRuntimeSnapshot>('get_openflow_runtime_snapshot');
    openflowRuntime.set(snapshot);
    return snapshot;
}

export async function initOpenFlowRuntime() {
    try {
        await syncOpenFlowRuntime();
    } catch (error) {
        console.error('Failed to fetch OpenFlow runtime:', error);
    }
}

async function refreshOpenFlowRuntime() {
    return syncOpenFlowRuntime();
}

export async function createOpenFlowRun(request: OpenFlowCreateRunRequest) {
    const run = await invoke<OpenFlowRunRecord>('create_openflow_run', { request });
    await refreshOpenFlowRuntime();
    return run;
}

export async function advanceOpenFlowRunPhase(runId: string) {
    const run = await invoke<OpenFlowRunRecord>('advance_openflow_run_phase', { runId });
    await refreshOpenFlowRuntime();
    return run;
}

export async function retryOpenFlowRun(runId: string) {
    const run = await invoke<OpenFlowRunRecord>('retry_openflow_run', { runId });
    await refreshOpenFlowRuntime();
    return run;
}

export async function runOpenFlowAutonomousLoop(runId: string) {
    const run = await invoke<OpenFlowRunRecord>('run_openflow_autonomous_loop', { runId });
    await refreshOpenFlowRuntime();
    return run;
}

export async function applyOpenFlowReviewResult(
    runId: string,
    reviewerScore: number,
    accepted: boolean,
    issue?: string | null
) {
    const run = await invoke<OpenFlowRunRecord>('apply_openflow_review_result', {
        runId,
        reviewerScore,
        accepted,
        issue,
    });
    await refreshOpenFlowRuntime();
    return run;
}

export async function stopOpenFlowRun(
    runId: string,
    status: 'failed' | 'cancelled' | 'awaiting_approval',
    reason: string
) {
    const run = await invoke<OpenFlowRunRecord>('stop_openflow_run', {
        runId,
        status,
        reason,
    });

    clearCommLogOffset(runId);
    clearOrchestratorOffset(runId);

    await refreshOpenFlowRuntime();
    return run;
}

export async function listAvailableCliTools(): Promise<CliToolInfo[]> {
    return invoke<CliToolInfo[]>('list_available_cli_tools');
}

export async function listModelsForTool(toolId: string): Promise<ModelInfo[]> {
    return invoke<ModelInfo[]>('list_models_for_tool', { toolId });
}

export async function listThinkingModesForTool(toolId: string): Promise<ThinkingModeInfo[]> {
    return invoke<ThinkingModeInfo[]>('list_thinking_modes_for_tool', { toolId });
}

export async function spawnOpenflowAgents(
    workspaceId: string,
    runId: string,
    goal: string,
    workingDirectory: string,
    agentConfigs: AgentConfig[],
): Promise<string[]> {
    return invoke<string[]>('spawn_openflow_agents', {
        workspaceId,
        runId,
        goal,
        workingDirectory,
        agentConfigs,
    });
}

export async function getAgentSessionsForRun(runId: string): Promise<AgentSessionState[]> {
    return invoke<AgentSessionState[]>('get_agent_sessions_for_run', { runId });
}

export async function getCommunicationLog(runId: string): Promise<CommLogEntry[]> {
    const offset = commLogOffsets.get(runId) ?? 0;
    const [entries, newOffset] = await invoke<[CommLogEntry[], number]>('get_communication_log', { runId, offset });
    commLogOffsets.set(runId, newOffset);
    return entries;
}

export async function injectOrchestratorMessage(runId: string, message: string): Promise<number> {
    return invoke<number>('inject_orchestrator_message', { runId, message });
}

export async function triggerOrchestratorCycle(runId: string): Promise<OrchestratorTriggerResult> {
    const offset = commLogOffsets.get(runId) ?? 0;
    const result = await invoke<OrchestratorTriggerResult>('trigger_orchestrator_cycle', { runId, offset });

    if (result.rotation_baseline > 0) {
        commLogOffsets.set(runId, 0);
    } else if (result.comm_log_offset > 0) {
        commLogOffsets.set(runId, result.comm_log_offset);
    }

    if (result.next_phase !== null) {
        await refreshOpenFlowRuntime();
    }

    return result;
}
