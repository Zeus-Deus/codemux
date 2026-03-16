<script lang="ts">
    import { onDestroy } from 'svelte';
    import { openflowRuntime, advanceOpenFlowRunPhase, retryOpenFlowRun, runOpenFlowAutonomousLoop, stopOpenFlowRun, applyOpenFlowReviewResult, getAgentSessionsForRun, triggerOrchestratorCycle, getCommunicationLog, commLogStore, clearCommLogOffset, type AgentSessionState, type CommLogEntry, appState } from '../../stores/appState';
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
            // Using incremental log reading now, so 10s is fine
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
            clearCommLogOffset(runId); // Reset offset for new run
            getAgentSessionsForRun(runId).then(sessions => {
                agentSessions = sessions;
            }).catch(console.error);
            getCommunicationLog(runId).then(entries => {
                commLogStore.set(entries);
            }).catch(console.error);
            
            // Single polling interval — CommunicationPanel subscribes to the store
            // instead of making its own parallel IPC calls.
            // Using incremental reading (only fetches new entries), so 3s is efficient
            // Also limits store to max 500 entries to prevent memory bloat
            const MAX_STORE_ENTRIES = 500;
            commLogInterval = setInterval(() => {
                if (runId) {
                    getCommunicationLog(runId).then(entries => {
                        // Only update if we got new entries (incremental read)
                        if (entries.length > 0) {
                            commLogStore.update(prev => {
                                const combined = [...prev, ...entries];
                                // Keep only the most recent entries to prevent memory bloat
                                if (combined.length > MAX_STORE_ENTRIES) {
                                    return combined.slice(-MAX_STORE_ENTRIES);
                                }
                                return combined;
                            });
                        }
                    }).catch(console.error);
                }
            }, 3000);
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

    // Build a list of agent sessions indexed by instance ID so we can do O(1) lookups.
    // Instance IDs are like "builder-0", "orchestrator" (bare for orchestrator).
    const sessionByInstanceId = $derived(
        new Map(agentSessions.map(s => {
            const instanceId = s.config.role === 'orchestrator'
                ? 'orchestrator'
                : `${s.config.role}-${s.config.agent_index}`;
            return [instanceId, s];
        }))
    );

    const agentNodes = $derived(
        run?.workers.map((w, index) => {
            const roleLower = w.role.toLowerCase();
            // Instance ID: orchestrator has no index suffix; others use worker index
            const instanceId = roleLower === 'orchestrator' ? 'orchestrator' : `${roleLower}-${index}`;
            const session = sessionByInstanceId.get(instanceId)
                ?? agentSessions.find(s => s.config.role === w.role);
            
            // Determine status from the most recent log entries for THIS specific instance.
            // We look at all entries (not just recent ones) for the most authoritative final state,
            // but use recent entries for the "active" signal.
            let dynamicStatus = w.status;
            
            // Only check recent entries (last 10) instead of all entries for performance
            const recentEntries = commLogEntries.slice(-50);
            const instanceEntries = recentEntries.filter(e => 
                e.role.toLowerCase() === instanceId || e.role.toLowerCase() === roleLower
            );
            const recentInstanceEntries = instanceEntries.slice(-3);

            if (recentInstanceEntries.length > 0) {
                const lastMsg = recentInstanceEntries[recentInstanceEntries.length - 1].message.toLowerCase();
                if (lastMsg.includes('done:') || lastMsg.includes('run complete')) {
                    dynamicStatus = 'done';
                } else if (lastMsg.includes('blocked:')) {
                    dynamicStatus = 'blocked';
                } else {
                    // Check if this instance wrote something in the last 5 overall entries
                    const last5 = commLogEntries.slice(-5);
                    const recentlyActive = last5.some(e =>
                        e.role.toLowerCase() === instanceId || e.role.toLowerCase() === roleLower
                    );
                    if (recentlyActive) dynamicStatus = 'active';
                }
            }
            
            return {
                id: instanceId,
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

        // Build a set of all valid node IDs from the current agent nodes for fast lookup.
        const nodeIds = new Set(agentNodes.map(n => n.id));
        const uniqueRoles = [...new Set(agentNodes.map(n => n.role.toLowerCase()))];

        // Helper: resolve a node ID from a raw instance/role string.
        // Accepts "builder-0", "builder" (bare role → picks first instance), "orchestrator".
        const resolveNodeId = (raw: string): string | null => {
            const lower = raw.toLowerCase().trim();
            if (nodeIds.has(lower)) return lower;
            // Bare role → pick the first node whose role matches
            const match = agentNodes.find(n => n.role.toLowerCase() === lower);
            return match ? match.id : null;
        };

        // Helper: get first node of a given role
        const firstOfRole = (role: string): string | null => resolveNodeId(role);

        // Reduced from -20 to -10 for performance with 20+ agents
        const recentEntries = commLogEntries.slice(-10);

        // ── 1. ASSIGN messages from orchestrator → directed connections ──────────
        // Parse both instance-level "ASSIGN BUILDER-0: ..." and legacy "ASSIGN BUILDER: ..."
        for (const entry of recentEntries) {
            if (entry.role.toLowerCase() !== 'orchestrator') continue;
            const msg = entry.message;
            const assignMatch = msg.match(/ASSIGN\s+([A-Z]+-\d+|[A-Z]+)\s*:/i);
            if (!assignMatch) continue;
            const target = assignMatch[1]!.toLowerCase();
            const targetId = resolveNodeId(target);
            const orchId = resolveNodeId('orchestrator');
            if (orchId && targetId) {
                conns.push({ from: orchId, to: targetId, label: 'assign' });
            }
        }

        // ── 2. DONE / BLOCKED signals — show the agent reporting back ──────────
        for (const entry of recentEntries) {
            const senderLower = entry.role.toLowerCase();
            if (senderLower === 'system' || senderLower === 'orchestrator') continue;
            const senderId = resolveNodeId(senderLower);
            if (!senderId) continue;
            const msgLower = entry.message.toLowerCase();
            if (msgLower.includes('done:') || msgLower.includes('run complete')) {
                const orchId = resolveNodeId('orchestrator');
                if (orchId) conns.push({ from: senderId, to: orchId, label: 'done' });
            } else if (msgLower.includes('blocked:')) {
                const debugId = firstOfRole('debugger');
                if (debugId) conns.push({ from: senderId, to: debugId, label: 'blocked' });
            }
        }

        // ── 3. Phase-based defaults when no log connections found ────────────
        if (conns.length === 0) {
            const phase = run.current_phase;
            const orchId = firstOfRole('orchestrator');
            if (phase === 'plan' || phase === 'planning') {
                if (orchId) {
                    const plannerIds = agentNodes.filter(n => n.role.toLowerCase() === 'planner').map(n => n.id);
                    plannerIds.forEach(pid => conns.push({ from: orchId, to: pid, label: 'planning' }));
                    const researcherIds = agentNodes.filter(n => n.role.toLowerCase() === 'researcher').map(n => n.id);
                    researcherIds.forEach(rid => conns.push({ from: orchId, to: rid, label: 'research' }));
                }
            } else if (phase === 'execute' || phase === 'executing') {
                if (orchId) {
                    const builderIds = agentNodes.filter(n => n.role.toLowerCase() === 'builder').map(n => n.id);
                    builderIds.forEach(bid => conns.push({ from: orchId, to: bid, label: 'build' }));
                }
            } else if (phase === 'verify' || phase === 'verifying') {
                if (orchId) {
                    const testerIds = agentNodes.filter(n => n.role.toLowerCase() === 'tester').map(n => n.id);
                    testerIds.forEach(tid => conns.push({ from: orchId, to: tid, label: 'test' }));
                }
            } else if (phase === 'review' || phase === 'reviewing') {
                if (orchId) {
                    const reviewerIds = agentNodes.filter(n => n.role.toLowerCase() === 'reviewer').map(n => n.id);
                    reviewerIds.forEach(rid => {
                        conns.push({ from: orchId, to: rid, label: 'review' });
                        conns.push({ from: rid, to: orchId, label: 'feedback' });
                    });
                }
            }
        }

        // De-duplicate connections (same from+to pair) while preserving labels
        const seen = new Set<string>();
        return conns.filter(c => {
            const key = `${c.from}→${c.to}`;
            if (seen.has(key)) return false;
            seen.add(key);
            return true;
        });
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
