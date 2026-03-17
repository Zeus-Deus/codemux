<script lang="ts">
    import { onDestroy } from 'svelte';
    import { openflowRuntime, advanceOpenFlowRunPhase, retryOpenFlowRun, runOpenFlowAutonomousLoop, stopOpenFlowRun, applyOpenFlowReviewResult, getAgentSessionsForRun, triggerOrchestratorCycle, getCommunicationLog, commLogStore, clearCommLogOffset } from '../../stores/openflow';
    import type { AgentSessionState } from '../../stores/types';
    import CommunicationPanel from './CommunicationPanel.svelte';
    import NodeGraph from './NodeGraph.svelte';
    import BrowserPane from '../panes/BrowserPane.svelte';
    import { buildActiveConnections, buildAgentNodes } from '../../lib/openflowGraph';
    import {
        commLogPollInterval,
        INITIAL_ORCHESTRATOR_DELAY_MS,
        mergeCommLogEntries,
        ORCHESTRATOR_INTERVAL_MS,
    } from '../../lib/openflowPolling';

    let { workspaceTitle, runId }: { workspaceTitle: string; runId: string | null } = $props();

    // Find run by runId directly - this is more reliable than deriving from runtime
    const run = $derived(
        runId && $openflowRuntime 
            ? $openflowRuntime.active_runs.find(r => r.run_id === runId) ?? null 
            : null
    );

    let agentSessions = $state<AgentSessionState[]>([]);
    // Subscribe to the shared store instead of maintaining local state
    let commLogEntries = $derived($commLogStore);

    let orchestratorInterval: ReturnType<typeof setInterval> | null = null;
    let commLogInterval: ReturnType<typeof setInterval> | null = null;
    let initialTimeoutId: ReturnType<typeof setTimeout> | null = null;
    let showBrowser = $state(false);
    
    // Resizable panel state
    let isDragging = $state(false);
    let commPanelWidth = $state(350);
    let startResize = (e: MouseEvent) => {
        isDragging = true;
        document.addEventListener('mousemove', handleResize);
        document.addEventListener('mouseup', stopResize);
    };
    let handleResize = (e: MouseEvent) => {
        if (isDragging) {
            commPanelWidth = Math.max(200, Math.min(600, window.innerWidth - e.clientX));
        }
    };
    let stopResize = () => {
        isDragging = false;
        document.removeEventListener('mousemove', handleResize);
        document.removeEventListener('mouseup', stopResize);
    };

    // Guarantee resize listeners are removed even if the component is destroyed mid-drag.
    onDestroy(() => {
        document.removeEventListener('mousemove', handleResize);
        document.removeEventListener('mouseup', stopResize);
    });

    // Auto-trigger orchestration on mount
    $effect(() => {
        if (runId) {
            // Store the handle so we can cancel it if runId changes before it fires.
            initialTimeoutId = setTimeout(() => {
                initialTimeoutId = null;
                if (runId) {
                    triggerOrchestratorCycle(runId).catch(console.error);
                }
            }, INITIAL_ORCHESTRATOR_DELAY_MS);

            orchestratorInterval = setInterval(() => {
                if (runId && run) {
                    triggerOrchestratorCycle(runId).catch(e =>
                        console.error('[OpenFlow] Orchestration error:', e)
                    );
                }
            }, ORCHESTRATOR_INTERVAL_MS);
        }

        return () => {
            if (initialTimeoutId !== null) {
                clearTimeout(initialTimeoutId);
                initialTimeoutId = null;
            }
            if (orchestratorInterval) {
                clearInterval(orchestratorInterval);
                orchestratorInterval = null;
            }
        };
    });

    $effect(() => {
        if (runId) {
            agentSessions = [];
            commLogStore.set([]); // Clear shared store when switching runs
            clearCommLogOffset(runId); // Reset offset for new run
            getAgentSessionsForRun(runId).then(sessions => {
                agentSessions = sessions;
            }).catch(console.error);
            getCommunicationLog(runId).then(entries => {
                commLogStore.set(entries);
            }).catch(console.error);
            
            const intervalMs = commLogPollInterval(run);
            commLogInterval = setInterval(() => {
                if (runId) {
                    getCommunicationLog(runId).then(entries => {
                        if (entries.length > 0) {
                            commLogStore.update((previous) => mergeCommLogEntries(previous, entries));
                        }
                    }).catch(console.error);
                }
            }, intervalMs);
        } else {
            agentSessions = [];
            commLogStore.set([]);
        }
        
        return () => {
            if (commLogInterval) {
                clearInterval(commLogInterval);
                commLogInterval = null;
            }
        };
    });

    const agentNodes = $derived(
        buildAgentNodes(run, agentSessions, commLogEntries)
    );

    const activeConnections = $derived.by(() => {
        return buildActiveConnections(run, agentNodes, commLogEntries);
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

    async function handleOrchestrate() {
        if (!runId) return;
        try {
            const result = await triggerOrchestratorCycle(runId);
            console.log('Orchestrator result:', result);
        } catch (e) {
            console.error('Orchestrator error:', e);
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

    function shortenModel(modelId: string): string {
        const parts = modelId.split('/');
        return parts.length > 1 ? parts[parts.length - 1] : modelId;
    }

    function toggleBrowser() {
        showBrowser = !showBrowser;
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
                <button class="control-btn" type="button" onclick={toggleBrowser}>{showBrowser ? 'Orchestration' : 'Browser'}</button>
                {#if run && run.status !== 'completed' && run.status !== 'cancelled' && run.status !== 'failed'}
                    <button class="control-btn" type="button" onclick={handleOrchestrate}>Orchestrate</button>
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
            {#if showBrowser}
                <div class="browser-view">
                    <BrowserPane browserId="default" />
                </div>
            {:else if run && agentNodes.length > 0}
                <NodeGraph 
                    nodes={agentNodes} 
                    activeConnections={activeConnections}
                />
            {:else}
                <p class="no-run">No active run</p>
            {/if}
        </div>
    </div>

    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
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
        padding-bottom: 20px;
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .orch-info {
        display: flex;
        align-items: center;
        gap: 16px;
    }

    .orch-info h2 {
        margin: 0;
        font-size: 1.5rem;
        font-weight: 700;
        color: var(--ui-text-primary);
        letter-spacing: -0.02em;
    }

    .phase-badge {
        padding: 6px 12px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: 20px;
        font-size: 0.75rem;
        font-weight: 700;
        color: var(--ui-text-muted);
        text-transform: uppercase;
        letter-spacing: 0.05em;
        box-shadow: 0 2px 8px rgba(0,0,0,0.1);
        transition: all 0.3s ease;
    }

    .phase-badge.busy {
        color: var(--ui-accent);
        border-color: color-mix(in srgb, var(--ui-accent) 40%, transparent);
        background: color-mix(in srgb, var(--ui-accent) 15%, transparent);
        box-shadow: 0 0 12px color-mix(in srgb, var(--ui-accent) 30%, transparent);
    }

    .orch-controls {
        display: flex;
        gap: 12px;
    }

    .control-btn {
        padding: 10px 20px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: 8px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.9rem;
        font-weight: 500;
        cursor: pointer;
        transition: all var(--ui-motion-fast);
        box-shadow: 0 2px 4px rgba(0,0,0,0.05);
    }

    .control-btn:hover {
        background: var(--ui-layer-3);
        transform: translateY(-1px);
        box-shadow: 0 4px 8px rgba(0,0,0,0.1);
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
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: column;
        overflow: hidden;
    }

    .no-run {
        color: var(--ui-text-muted);
        font-size: 0.9rem;
    }

    .browser-view {
        width: 100%;
        height: 100%;
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: column;
        overflow: hidden;
        border-radius: 8px;
    }

</style>
