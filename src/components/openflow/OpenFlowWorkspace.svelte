<script lang="ts">
    import { fade } from 'svelte/transition';
    import { openflowRuntime, createOpenFlowRun, spawnOpenflowAgents } from '../../stores/openflow';
    import { updateWorkspaceCwd } from '../../stores/workspace';
    import type { WorkspaceSnapshot } from '../../stores/types';
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
        event: CustomEvent<{ title: string; goal: string; directory: string; agentConfigs: AgentConfig[] }>
    ) {
        spawnError = null;
        try {
            // Update workspace cwd to the selected directory
            await updateWorkspaceCwd(workspace.workspace_id, event.detail.directory);
            console.log('[OpenFlow] Updated workspace cwd to:', event.detail.directory);

            // Create the run
            const created = await createOpenFlowRun({
                title: event.detail.title,
                goal: event.detail.goal,
                agent_roles: event.detail.agentConfigs.map(c => c.role)
            });
            console.log('[OpenFlow] Created NEW run with ID:', created.run_id, 'title:', created.title, 'directory:', event.detail.directory);
            
            // Switch to orchestration immediately
            currentRunId = created.run_id;
            view = 'orchestration';
            
            // Browser pane will be created when user clicks "Browser" button in OrchestrationView
            
            // Try to spawn agents (non-blocking)
            console.log('[OpenFlow] Spawning agents with configs:', JSON.stringify(event.detail.agentConfigs));
            spawnOpenflowAgents(
                workspace.workspace_id,
                created.run_id,
                event.detail.goal,  // Pass the goal
                event.detail.directory,  // Pass the working directory
                event.detail.agentConfigs.map((cfg, i) => ({
                    agent_index: i,
                    cli_tool: cfg.cliTool,
                    model: cfg.model,
                    provider: cfg.model.includes('/') ? cfg.model.split('/')[0] : '',
                    thinking_mode: cfg.thinkingMode ?? '',
                    role: cfg.role,
                })),
            ).then(sessionIds => {
                console.log('[OpenFlow] Agents spawned, session IDs:', sessionIds);
            }).catch(e => console.error('[OpenFlow] Spawn error:', e));
            
        } catch (error) {
            console.error('[OpenFlow] Failed to start run:', error);
            spawnError = String(error);
        }
    }
</script>

<div class="openflow-workspace">
    {#if view === 'config'}
        <div class="view-wrapper" in:fade={{duration: 200, delay: 200}} out:fade={{duration: 200}}>
            {#if spawnError}
                <div class="spawn-error">Failed to start agents: {spawnError}</div>
            {/if}
            <AgentConfigPanel on:start={handleStartRun} />
        </div>
    {:else if view === 'orchestration'}
        <div class="view-wrapper" in:fade={{duration: 300, delay: 200}} out:fade={{duration: 200}}>
            <OrchestrationView
                workspaceTitle={workspace.title}
                runId={currentRunId}
            />
        </div>
    {/if}
</div>

<style>
    .openflow-workspace {
        position: relative;
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        background: var(--ui-layer-0);
        overflow: hidden;
    }

    .view-wrapper {
        position: absolute;
        top: 0;
        left: 0;
        width: 100%;
        height: 100%;
        display: flex;
        flex-direction: column;
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
