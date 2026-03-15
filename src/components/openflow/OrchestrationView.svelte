<script lang="ts">
    import { onDestroy } from 'svelte';
    import { openflowRuntime, advanceOpenFlowRunPhase, retryOpenFlowRun, runOpenFlowAutonomousLoop, stopOpenFlowRun, applyOpenFlowReviewResult, getAgentSessionsForRun, triggerOrchestratorCycle, getCommunicationLog, commLogStore, type AgentSessionState, type CommLogEntry, appState } from '../../stores/appState';
    import type { OpenFlowRunRecord, WorkspaceSnapshot, PaneNodeSnapshot } from '../../stores/appState';
    import CommunicationPanel from './CommunicationPanel.svelte';
    import NodeGraph, { type AgentNodeData, type Connection } from './NodeGraph.svelte';
    import BrowserPane from '../panes/BrowserPane.svelte';

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
            }, 3000);

            // Start auto-orchestration loop - keep running even after completion to process user injections
            orchestratorInterval = setInterval(() => {
                if (runId && run) {
                    triggerOrchestratorCycle(runId).catch(e =>
                        console.error('[OpenFlow] Orchestration error:', e)
                    );
                }
            }, 10000);
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
            getAgentSessionsForRun(runId).then(sessions => {
                agentSessions = sessions;
            }).catch(console.error);
            getCommunicationLog(runId).then(entries => {
                commLogStore.set(entries);
            }).catch(console.error);
            
            // Single polling interval — CommunicationPanel subscribes to the store
            // instead of making its own parallel IPC calls.
            commLogInterval = setInterval(() => {
                if (runId) {
                    getCommunicationLog(runId).then(entries => {
                        // Skip the store update when nothing has changed to avoid
                        // cascading reactive recalculations.
                        commLogStore.update(prev => {
                            if (prev.length === entries.length &&
                                prev[prev.length - 1]?.message === entries[entries.length - 1]?.message) {
                                return prev;
                            }
                            return entries;
                        });
                    }).catch(console.error);
                }
            }, 2000);
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
        run?.workers.map((w, index) => {
            const session = agentSessions.find(s => s.config.role === w.role);
            const roleLower = w.role.toLowerCase();
            
            // Dynamically determine status based on recent comm log activity
            let dynamicStatus = w.status;
            const recentEntries = commLogEntries.slice(-15);
            const roleEntries = recentEntries.filter(e => 
                e.role.toLowerCase() === roleLower
            );
            
            if (roleEntries.length > 0) {
                const lastMsg = roleEntries[roleEntries.length - 1].message.toLowerCase();
                
                if (lastMsg.includes('done:') || lastMsg.includes('run complete')) {
                    dynamicStatus = 'done';
                } else if (lastMsg.includes('blocked:')) {
                    dynamicStatus = 'blocked';
                } else if (roleEntries.length >= recentEntries.length - 3) {
                    dynamicStatus = 'active';
                }
            }
            
            return {
                id: `${w.role}-${index}`,
                role: w.role,
                status: dynamicStatus,
                model: session?.config.model ?? null,
                thinkingMode: session?.config.thinking_mode ?? null
            };
        }) ?? []
    );

    const activeConnections = $derived.by(() => {
        if (!run || run.workers.length === 0) return [];
        
        const conns: Connection[] = [];
        
        // Build a position map keyed by role for O(1) lookups.
        const workerIndexByRole = new Map<string, number[]>();
        run.workers.forEach((w, i) => {
            const r = w.role.toLowerCase();
            const arr = workerIndexByRole.get(r) ?? [];
            arr.push(i);
            workerIndexByRole.set(r, arr);
        });

        const getWorkerId = (role: string, instanceIndex: number = 0) => {
            const indices = workerIndexByRole.get(role.toLowerCase());
            if (!indices || indices.length === 0) return `${role.toLowerCase()}-${instanceIndex}`;
            const idx = instanceIndex < indices.length ? instanceIndex : 0;
            return `${role.toLowerCase()}-${indices[idx]}`;
        };

        const uniqueRoles = [...workerIndexByRole.keys()];
        const recentEntries = commLogEntries.slice(-10);
        
        // Find the most recent non-system sender
        let mostRecentSender: string | null = null;
        let mostRecentTime = -1;
        
        for (const entry of recentEntries) {
            const role = entry.role.toLowerCase();
            if (role === 'system' || role === 'orchestrator') continue;
            
            const m = entry.timestamp.match(/(\d+)-(\d+)-(\d+)\s+(\d+):(\d+):(\d+)/);
            if (m) {
                const entryTime = new Date(
                    parseInt(m[1]!, 10),
                    parseInt(m[2]!, 10) - 1,
                    parseInt(m[3]!, 10),
                    parseInt(m[4]!, 10),
                    parseInt(m[5]!, 10),
                    parseInt(m[6]!, 10)
                ).getTime();
                if (entryTime > mostRecentTime) {
                    mostRecentTime = entryTime;
                    mostRecentSender = role;
                }
            }
        }
        
        // ASSIGN messages → directed connections from orchestrator
        for (const entry of recentEntries) {
            if (entry.role.toLowerCase() !== 'orchestrator') continue;
            const msg = entry.message.toLowerCase();
            if (!msg.includes('assign ') && !msg.includes('assign:')) continue;
            const roles = ['researcher', 'planner', 'builder', 'tester', 'debugger', 'reviewer'];
            for (const role of roles) {
                if (msg.includes(`assign ${role}`) || msg.includes(`assign:${role}`)) {
                    conns.push({ from: getWorkerId('orchestrator'), to: getWorkerId(role), label: role });
                }
            }
        }
        
        // Most-recent sender connections
        if (mostRecentSender && uniqueRoles.includes(mostRecentSender)) {
            const senderEntries = recentEntries.filter(e => e.role.toLowerCase() === mostRecentSender);
            const lastMsg = senderEntries[senderEntries.length - 1]?.message.toLowerCase() ?? '';
            
            if (lastMsg.includes('done:') || lastMsg.includes('run complete')) {
                const currentIndex = uniqueRoles.indexOf(mostRecentSender);
                if (currentIndex < uniqueRoles.length - 1) {
                    conns.push({ from: getWorkerId(mostRecentSender), to: getWorkerId(uniqueRoles[currentIndex + 1]!), label: 'done → next' });
                } else {
                    conns.push({ from: getWorkerId(mostRecentSender), to: getWorkerId('orchestrator'), label: 'complete' });
                }
            } else if (lastMsg.includes('blocked:')) {
                conns.push({ from: getWorkerId(mostRecentSender), to: getWorkerId('debugger'), label: 'blocked' });
            }
        }
        
        // Phase-based defaults when no log connections found
        if (conns.length === 0) {
            const phase = run.current_phase;
            if (phase === 'plan' || phase === 'planning') {
                conns.push({ from: getWorkerId('orchestrator'), to: getWorkerId('planner'), label: 'planning' });
                if (uniqueRoles.includes('researcher')) {
                    conns.push({ from: getWorkerId('orchestrator'), to: getWorkerId('researcher'), label: 'research' });
                }
            } else if (phase === 'execute' || phase === 'executing') {
                conns.push({ from: getWorkerId('planner'), to: getWorkerId('builder'), label: 'tasks' });
                if (uniqueRoles.includes('tester')) {
                    conns.push({ from: getWorkerId('builder'), to: getWorkerId('tester'), label: 'testing' });
                }
            } else if (phase === 'verify' || phase === 'verifying') {
                if (uniqueRoles.includes('tester')) {
                    conns.push({ from: getWorkerId('builder'), to: getWorkerId('tester'), label: 'testing' });
                }
                if (uniqueRoles.includes('reviewer')) {
                    conns.push({ from: getWorkerId('tester') ?? getWorkerId('builder'), to: getWorkerId('reviewer'), label: 'results' });
                }
            } else if (phase === 'review' || phase === 'reviewing') {
                if (uniqueRoles.includes('reviewer')) {
                    conns.push({ from: getWorkerId('builder'), to: getWorkerId('reviewer'), label: 'review' });
                    conns.push({ from: getWorkerId('reviewer'), to: getWorkerId('orchestrator'), label: 'feedback' });
                }
            }
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
