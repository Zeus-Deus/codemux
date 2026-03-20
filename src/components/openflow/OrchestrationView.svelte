<script lang="ts">
    import { onDestroy } from 'svelte';
    import { openflowRuntime, retryOpenFlowRun, applyOpenFlowReviewResult, getAgentSessionsForRun, triggerOrchestratorCycle, getCommunicationLog, commLogStore, clearCommLogOffset, syncOpenFlowRuntime } from '../../stores/openflow';
    import type { AgentSessionState, OrchestratorTriggerResult } from '../../stores/types';
    import CommunicationPanel from './CommunicationPanel.svelte';
    import NodeGraph from './NodeGraph.svelte';
    import BrowserPane from '../panes/BrowserPane.svelte';
    import { buildActiveConnections, buildAgentNodes } from '../../lib/openflowGraph';
    import {
        commLogPollInterval,
        mergeCommLogEntries,
    } from '../../lib/openflowPolling';
    import { listen } from '@tauri-apps/api/event';

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

    let commLogInterval: ReturnType<typeof setInterval> | null = null;
    let runtimePollInterval: ReturnType<typeof setInterval> | null = null;
    let showBrowser = $state(false);
    let commLogPollingInProgress = false;
    let lastOrchestratorResult = $state<OrchestratorTriggerResult | null>(null);
    
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

    async function runOrchestratorCycle(currentRunId: string) {
        const result = await triggerOrchestratorCycle(currentRunId);
        lastOrchestratorResult = result;
        return result;
    }

    // Listen for backend-driven orchestration cycle events.
    // The backend loop drives orchestration; the frontend just observes.
    $effect(() => {
        if (!runId) return;

        let cancelled = false;
        const unlistenPromise = listen<OrchestratorTriggerResult>('openflow-cycle', (event) => {
            if (cancelled) return;
            lastOrchestratorResult = event.payload;
            syncOpenFlowRuntime().catch(console.error);
        });

        // Also poll runtime snapshot periodically as a fallback
        runtimePollInterval = setInterval(() => {
            if (!cancelled) {
                syncOpenFlowRuntime().catch(console.error);
            }
        }, 10_000);

        return () => {
            cancelled = true;
            unlistenPromise.then(fn => fn()).catch(() => {});
            if (runtimePollInterval) {
                clearInterval(runtimePollInterval);
                runtimePollInterval = null;
            }
        };
    });

    $effect(() => {
        if (runId) {
            agentSessions = [];
            lastOrchestratorResult = null;
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
                if (runId && !commLogPollingInProgress) {
                    commLogPollingInProgress = true;
                    getCommunicationLog(runId).then(entries => {
                        if (entries.length > 0) {
                            commLogStore.update((previous) => mergeCommLogEntries(previous, entries));
                        }
                    }).catch(console.error).finally(() => {
                        commLogPollingInProgress = false;
                    });
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
        return buildActiveConnections(run, agentNodes, commLogEntries, agentSessions);
    });

    const appUrl = $derived.by(() => {
        for (const entry of [...commLogEntries].reverse()) {
            if (entry.role.toLowerCase() === 'system' && entry.message.startsWith('APP_URL: ')) {
                return entry.message.slice('APP_URL: '.length).trim();
            }
        }

        return null;
    });

    const orchestrationHealth = $derived.by(() => {
        const state = run ? ((run as any).orchestration_state as string | null) : null;
        const detail = run ? ((run as any).orchestration_detail as string | null) : null;
        if (!state) return null;

        switch (state) {
            case 'correcting_delegation':
                return { tone: 'warning', label: 'Correcting Orchestrator', detail: detail ?? 'Fixing invalid delegation pattern' };
            case 'waiting_for_response':
                return { tone: 'info', label: 'Waiting On Reply', detail: detail ?? 'The last user message is pending an orchestrator response' };
            case 'stalled':
                return { tone: 'warning', label: 'Orchestrator Stalled', detail: detail ?? 'The orchestrator has not made progress recently' };
            case 'blocked':
                return { tone: 'danger', label: 'Orchestrator Blocked', detail: detail ?? 'An agent reported a blocking issue' };
            case 'error':
                return { tone: 'danger', label: 'Orchestrator Error', detail: detail ?? 'The orchestrator hit an error' };
            case 'active':
                return { tone: 'info', label: 'OpenFlow Active', detail: detail ?? 'Orchestration is active' };
            case 'idle':
                return { tone: 'info', label: 'OpenFlow Idle', detail: detail ?? 'Waiting for the next action' };
            case 'initializing':
            default:
                return { tone: 'info', label: 'OpenFlow Starting', detail: detail ?? 'Run is initializing' };
        }
    });

    async function handleOrchestrate() {
        if (!runId) return;
        try {
            const result = await runOrchestratorCycle(runId);
            console.log('Orchestrator result:', result);
        } catch (e) {
            console.error('Orchestrator error:', e);
        }
    }

    async function refreshOrchestrator() {
        if (!runId) return;
        try {
            const result = await runOrchestratorCycle(runId);
            console.log('Refresh result:', result);
        } catch (e) {
            console.error('Refresh error:', e);
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
                {#if appUrl}
                    <span class="app-url-badge">{appUrl}</span>
                {/if}
                {#if orchestrationHealth}
                    <span class="health-badge {orchestrationHealth.tone}" title={orchestrationHealth.detail}>
                        {orchestrationHealth.label}
                    </span>
                {/if}
            </div>
            <div class="orch-controls">
                <button class="control-btn" type="button" onclick={toggleBrowser}>{showBrowser ? 'Orchestration' : 'Browser'}</button>
                <button class="control-btn" type="button" onclick={refreshOrchestrator}>Refresh</button>
                {#if run && ((run as any).orchestration_state === 'blocked' || (run as any).orchestration_state === 'stalled')}
                    <button class="control-btn accent" type="button" onclick={handleOrchestrate}>Re-prime</button>
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

    .app-url-badge {
        display: inline-flex;
        align-items: center;
        margin-left: 12px;
        padding: 6px 10px;
        border-radius: 999px;
        background: color-mix(in srgb, var(--ui-accent) 12%, var(--ui-layer-1));
        border: 1px solid color-mix(in srgb, var(--ui-accent) 30%, transparent);
        color: var(--ui-text-primary);
        font-size: 0.78rem;
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    }

    .health-badge {
        display: inline-flex;
        align-items: center;
        margin-left: 12px;
        padding: 6px 10px;
        border-radius: 999px;
        font-size: 0.78rem;
        color: var(--ui-text-primary);
        border: 1px solid transparent;
    }

    .health-badge.info {
        background: color-mix(in srgb, var(--ui-accent) 12%, var(--ui-layer-1));
        border-color: color-mix(in srgb, var(--ui-accent) 30%, transparent);
    }

    .health-badge.warning {
        background: color-mix(in srgb, var(--ui-attention) 14%, var(--ui-layer-1));
        border-color: color-mix(in srgb, var(--ui-attention) 30%, transparent);
    }

    .health-badge.danger {
        background: color-mix(in srgb, var(--ui-danger) 14%, var(--ui-layer-1));
        border-color: color-mix(in srgb, var(--ui-danger) 30%, transparent);
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
