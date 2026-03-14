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

    const run = $derived(
        $openflowRuntime?.active_runs.find(
            r => r.status !== 'completed' && r.status !== 'failed' && r.status !== 'cancelled'
        ) ?? null
    );

    $effect(() => {
        if (run) {
            currentRunId = run.run_id;
            view = 'orchestration';
        }
    });

    async function handleStartRun(
        event: CustomEvent<{ title: string; goal: string; agentConfigs: AgentConfig[] }>
    ) {
        spawnError = null;
        try {
            const created = await createOpenFlowRun({
                title: event.detail.title,
                goal: event.detail.goal,
            });
            currentRunId = created.run_id;

            // Phase 2: spawn one terminal pane per agent config.
            await spawnOpenflowAgents(
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
            );

            view = 'orchestration';
        } catch (error) {
            console.error('Failed to start run:', error);
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
