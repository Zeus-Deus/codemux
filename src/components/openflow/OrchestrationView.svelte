<script lang="ts">
    import { openflowRuntime, advanceOpenFlowRunPhase, retryOpenFlowRun, runOpenFlowAutonomousLoop, stopOpenFlowRun, applyOpenFlowReviewResult, getAgentSessionsForRun, type AgentSessionState } from '../../stores/appState';
    import type { OpenFlowRunRecord } from '../../stores/appState';
    import CommunicationPanel from './CommunicationPanel.svelte';
    import NodeGraph, { type AgentNodeData, type Connection } from './NodeGraph.svelte';
    import { onMount } from 'svelte';

    let { workspaceTitle, runId }: { workspaceTitle: string; runId: string | null } = $props();

    // Find run by runId directly - this is more reliable than deriving from runtime
    const run = $derived(
        runId && $openflowRuntime 
            ? $openflowRuntime.active_runs.find(r => r.run_id === runId) ?? null 
            : null
    );

    let agentSessions = $state<AgentSessionState[]>([]);

    $effect(() => {
        if (runId) {
            agentSessions = []; // Clear when switching runs
            getAgentSessionsForRun(runId).then(sessions => {
                agentSessions = sessions;
            }).catch(console.error);
        } else {
            agentSessions = [];
        }
    });

    const agentNodes = $derived(
        run?.workers.map(w => {
            const session = agentSessions.find(s => s.config.role === w.role);
            return {
                id: w.role,
                role: w.role,
                status: w.status,
                model: session?.config.model ?? null,
                thinkingMode: session?.config.thinking_mode ?? null
            };
        }) ?? []
    );

    const activeConnections = $derived.by(() => {
        if (!run) return [];
        const phase = run.current_phase;
        const conns: Connection[] = [];
        
        if (phase === 'plan' || phase === 'execute') {
            conns.push({ from: 'orchestrator', to: 'builder', label: 'assigning tasks' });
            conns.push({ from: 'orchestrator', to: 'planner', label: 'planning' });
        }
        if (phase === 'execute') {
            conns.push({ from: 'builder', to: 'tester', label: 'building' });
        }
        if (phase === 'verify') {
            conns.push({ from: 'builder', to: 'tester', label: 'testing' });
            conns.push({ from: 'tester', to: 'reviewer', label: 'results' });
        }
        if (phase === 'review') {
            conns.push({ from: 'builder', to: 'reviewer', label: 'review' });
            conns.push({ from: 'reviewer', to: 'orchestrator', label: 'feedback' });
        }
        
        return conns;
    });

    async function handleLoop() {
        if (!runId) return;
        try {
            await runOpenFlowAutonomousLoop(runId);
        } catch (e) {
            console.error('Loop error:', e);
        }
    }

    async function handleNext() {
        if (!runId) return;
        try {
            await advanceOpenFlowRunPhase(runId);
        } catch (e) {
            console.error('Advance error:', e);
        }
    }

    async function handlePause() {
        if (!runId) return;
        try {
            await stopOpenFlowRun(runId, 'awaiting_approval', 'Paused by user');
        } catch (e) {
            console.error('Pause error:', e);
        }
    }

    async function handleCancel() {
        if (!runId) return;
        try {
            await stopOpenFlowRun(runId, 'cancelled', 'Cancelled by user');
        } catch (e) {
            console.error('Cancel error:', e);
        }
    }

    async function handleApprove() {
        if (!runId) return;
        try {
            await applyOpenFlowReviewResult(runId, 95, true, null);
        } catch (e) {
            console.error('Approve error:', e);
        }
    }

    async function handleRetry() {
        if (!runId) return;
        try {
            await retryOpenFlowRun(runId);
        } catch (e) {
            console.error('Retry error:', e);
        }
    }

    function getStatusColor(status: string): string {
        if (status === 'done' || status === 'passed') return 'var(--ui-success)';
        if (status === 'active' || status === 'ready') return 'var(--ui-accent)';
        if (status === 'pending') return 'var(--ui-text-muted)';
        if (status === 'blocked') return 'var(--ui-danger)';
        return 'var(--ui-text-muted)';
    }

    function getRoleIcon(role: string): string {
        const icons: Record<string, string> = {
            orchestrator: '⚙️',
            planner: '📋',
            builder: '🔨',
            reviewer: '👀',
            tester: '🧪',
            debugger: '🔧',
            researcher: '🔍'
        };
        return icons[role] || '🤖';
    }

    let commPanelWidth = $state(320);
    let isDragging = $state(false);
    let startX = $state(0);
    let startWidth = $state(0);

    function startResize(e: MouseEvent) {
        isDragging = true;
        startX = e.clientX;
        startWidth = commPanelWidth;
        window.addEventListener('mousemove', onResize);
        window.addEventListener('mouseup', stopResize);
    }

    function onResize(e: MouseEvent) {
        if (!isDragging) return;
        const delta = startX - e.clientX;
        const newWidth = Math.max(200, Math.min(600, startWidth + delta));
        commPanelWidth = newWidth;
    }

    function stopResize() {
        isDragging = false;
        window.removeEventListener('mousemove', onResize);
        window.removeEventListener('mouseup', stopResize);
    }

    function shortenModel(modelId: string): string {
        const parts = modelId.split('/');
        return parts.length > 1 ? parts[parts.length - 1] : modelId;
    }
</script>

<div class="orchestration-view">
    <div class="orchestration-main">
        <header class="orch-header">
            <div class="orch-info">
                <h2>{workspaceTitle}</h2>
                {#if run}
                    <span class="phase-badge" class:busy={run.status === 'executing' || run.status === 'planning'}>
                        {run.current_phase}
                    </span>
                {/if}
            </div>
            <div class="orch-controls">
                {#if run && run.status !== 'completed' && run.status !== 'cancelled' && run.status !== 'failed'}
                    <button class="control-btn" type="button" onclick={handleLoop}>Loop</button>
                    <button class="control-btn" type="button" onclick={handleNext}>Next</button>
                    <button class="control-btn" type="button" onclick={handlePause}>Pause</button>
                    <button class="control-btn danger" type="button" onclick={handleCancel}>Cancel</button>
                {:else if run}
                    <button class="control-btn" type="button" onclick={handleRetry}>Retry</button>
                {/if}
                {#if run && (run.status === 'awaiting_approval' || run.current_phase === 'review')}
                    <button class="control-btn accent" type="button" onclick={handleApprove}>Approve</button>
                {/if}
            </div>
        </header>

        <div class="node-graph">
            {#if run && agentNodes.length > 0}
                <NodeGraph 
                    nodes={agentNodes} 
                    activeConnections={activeConnections}
                />
            {:else}
                <p class="no-run">No active run</p>
            {/if}
        </div>

        {#if run}
            <div class="timeline">
                <h3>Timeline</h3>
                <div class="timeline-entries">
                    {#each run.timeline as entry}
                        <div class="timeline-entry" class:warning={entry.level === 'warning'} class:error={entry.level === 'error'}>
                            <span class="timeline-time"></span>
                            <span class="timeline-message">{entry.message}</span>
                        </div>
                    {/each}
                </div>
            </div>
        {/if}
    </div>

    <!-- Resizable divider -->
    <div 
        class="panel-resizer" 
        class:active={isDragging}
        onmousedown={startResize}
        role="separator"
        aria-orientation="vertical"
    ></div>

    <div class="comm-panel-wrapper" style="width: {commPanelWidth}px">
        <CommunicationPanel {runId} />
    </div>
</div>

<style>
    .orchestration-view {
        display: flex;
        width: 100%;
        height: 100%;
        overflow: hidden;
    }

    .orchestration-main {
        flex: 1;
        display: flex;
        flex-direction: column;
        padding: 24px;
        overflow-y: auto;
        min-width: 0;
    }

    .panel-resizer {
        width: 6px;
        background: var(--ui-border-soft);
        cursor: col-resize;
        transition: background 0.15s;
        flex-shrink: 0;
    }

    .panel-resizer:hover,
    .panel-resizer.active {
        background: var(--ui-accent);
    }

    .comm-panel-wrapper {
        flex-shrink: 0;
        overflow: hidden;
    }

    .orch-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 24px;
        padding-bottom: 16px;
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .orch-info {
        display: flex;
        align-items: center;
        gap: 12px;
    }

    .orch-info h2 {
        margin: 0;
        font-size: 1.25rem;
        font-weight: 600;
        color: var(--ui-text-primary);
    }

    .phase-badge {
        padding: 4px 10px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: 12px;
        font-size: 0.75rem;
        font-weight: 600;
        color: var(--ui-text-muted);
        text-transform: uppercase;
    }

    .phase-badge.busy {
        color: var(--ui-accent);
        border-color: color-mix(in srgb, var(--ui-accent) 30%, transparent);
        background: color-mix(in srgb, var(--ui-accent) 10%, transparent);
    }

    .orch-controls {
        display: flex;
        gap: 8px;
    }

    .control-btn {
        padding: 8px 16px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: 6px;
        color: var(--ui-text-secondary);
        font: inherit;
        font-size: 0.85rem;
        cursor: pointer;
        transition: all var(--ui-motion-fast);
    }

    .control-btn:hover {
        background: var(--ui-layer-3);
        color: var(--ui-text-primary);
    }

    .control-btn.danger:hover {
        color: var(--ui-danger);
        border-color: color-mix(in srgb, var(--ui-danger) 30%, transparent);
    }

    .control-btn.accent {
        color: var(--ui-accent);
        border-color: color-mix(in srgb, var(--ui-accent) 30%, transparent);
    }

    .control-btn.accent:hover {
        background: color-mix(in srgb, var(--ui-accent) 15%, transparent);
    }

    .node-graph {
        width: 100%;
        min-height: 300px;
        margin-bottom: 24px;
    }

    .no-run {
        color: var(--ui-text-muted);
        font-size: 0.9rem;
    }

    .timeline h3 {
        margin: 0 0 12px;
        font-size: 0.9rem;
        font-weight: 600;
        color: var(--ui-text-secondary);
    }

    .timeline-entries {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .timeline-entry {
        display: flex;
        gap: 12px;
        padding: 8px 12px;
        background: var(--ui-layer-2);
        border-radius: 6px;
        font-size: 0.85rem;
    }

    .timeline-entry.warning {
        background: color-mix(in srgb, var(--ui-attention) 10%, var(--ui-layer-2));
    }

    .timeline-entry.error {
        background: color-mix(in srgb, var(--ui-danger) 10%, var(--ui-layer-2));
    }

    .timeline-message {
        color: var(--ui-text-secondary);
    }
</style>
