<script lang="ts">
    import type { WorkspaceSnapshot } from '../../stores/appState';
    import { openflowRuntime, createOpenFlowRun, spawnOpenflowAgents } from '../../stores/appState';
    import AgentConfigPanel from './AgentConfigPanel.svelte';
    import OrchestrationView from './OrchestrationView.svelte';
    import type { AgentConfig } from './AgentConfigPanel.svelte';

    let { workspace }: { workspace: WorkspaceSnapshot } = $props();

    let view = $state<'config' | 'orchestration'>('config');
    let currentRunId = $state<string | null>(null);
    let spawnError = $state<string | null>(null);

    // Run is determined by currentRunId directly (set when user clicks Start)
    const run = $derived(
        currentRunId && $openflowRuntime 
            ? $openflowRuntime.active_runs.find(r => r.run_id === currentRunId) ?? null 
            : null
    );

    // Switch to orchestration when we have a run
    $effect(() => {
        if (run) {
            view = 'orchestration';
        }
    });

    // Reset when workspace changes - track previous workspace ID
    let prevWorkspaceId: string | null = null;
    $effect(() => {
        const wsId = workspace.workspace_id;
        if (prevWorkspaceId !== null && prevWorkspaceId !== wsId) {
            view = 'config';
            currentRunId = null;
            spawnError = null;
        }
        prevWorkspaceId = wsId;
    });

    async function handleStartRun(
        event: CustomEvent<{ title: string; goal: string; agentConfigs: AgentConfig[] }>
    ) {
        spawnError = null;
        try {
            // Create the run
            const created = await createOpenFlowRun({
                title: event.detail.title,
                goal: event.detail.goal,
                agent_roles: event.detail.agentConfigs.map(c => c.role)
            });
            console.log('[OpenFlow] Created NEW run with ID:', created.run_id, 'title:', created.title);
            
            // Switch to orchestration immediately
            currentRunId = created.run_id;
            view = 'orchestration';
            
            // Try to spawn agents (non-blocking)
            spawnOpenflowAgents(
                workspace.workspace_id,
                created.run_id,
                event.detail.agentConfigs.map((cfg, i) => ({
                    agent_index: i,
                    cli_tool: cfg.cliTool,
                    model: cfg.model,
                    provider: cfg.model.includes('/') ? cfg.model.split('/')[0] : '',
                    thinking_mode: cfg.thinkingMode ?? '',
                    role: cfg.role,
                })),
            ).catch(e => console.error('[OpenFlow] Spawn error:', e));
            
        } catch (error) {
            console.error('[OpenFlow] Failed to start run:', error);
            spawnError = String(error);
        }
    }
</script>

<div class="openflow-workspace">
    {#if view === 'config'}
        {#if spawnError}
            <div class="spawn-error">Failed to start agents: {spawnError}</div>
        {/if}
        <AgentConfigPanel on:start={handleStartRun} />
    {:else if view === 'orchestration'}
        <OrchestrationView
            workspaceTitle={workspace.title}
            runId={currentRunId}
        />
    {/if}
</div>

<style>
    .openflow-workspace {
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        background: var(--ui-layer-0);
    }

    .spawn-error {
        padding: 8px 16px;
        background: rgba(220, 53, 69, 0.15);
        border-left: 3px solid rgb(220, 53, 69);
        color: rgb(220, 53, 69);
        font-size: 13px;
        font-family: monospace;
    }
</style>
