<script module lang="ts">
    export interface AgentConfig {
        cliTool: string;
        model: string;
        role: string;
        thinkingMode: string;
    }
</script>

<script lang="ts">
    import { onMount } from 'svelte';
    import { createEventDispatcher } from 'svelte';
    import {
        listAvailableCliTools,
        listModelsForTool,
        listThinkingModesForTool,
        type CliToolInfo,
        type ModelInfo,
        type ThinkingModeInfo,
    } from '../../stores/appState';
    import { invoke } from '@tauri-apps/api/core';

    const dispatch = createEventDispatcher<{
        start: { title: string; goal: string; directory: string; agentConfigs: AgentConfig[] };
    }>();

    // ── local state ──────────────────────────────────────────────────────────
    let agentCount = $state(5);
    let titleDraft = $state('');
    let goalDraft = $state('');
    let selectedDirectory = $state<string | null>(null);

    // Discovery state
    let availableTools = $state<CliToolInfo[]>([]);
    let modelsByTool = $state<Record<string, ModelInfo[]>>({});
    let thinkingModesByTool = $state<Record<string, ThinkingModeInfo[]>>({});
    let loading = $state(true);
    let loadError = $state<string | null>(null);

    // Agent rows - use $state so mutations work correctly in Svelte 5
    let agents = $state<AgentConfig[]>([]);

    const availableRoles = [
        { id: 'orchestrator', name: 'Orchestrator' },
        { id: 'builder', name: 'Builder' },
        { id: 'reviewer', name: 'Reviewer' },
        { id: 'tester', name: 'Tester' },
        { id: 'debugger', name: 'Debugger' },
        { id: 'researcher', name: 'Researcher' },
    ];

    // ── helpers ──────────────────────────────────────────────────────────────
    // Orchestrator is always agent 0. For all subsequent agents cycle through
    // the non-orchestrator roles so we never accidentally assign a second orchestrator.
    function getDefaultRole(index: number): string {
        if (index === 0) return 'orchestrator';
        const otherRoles = ['researcher', 'planner', 'builder', 'tester', 'debugger', 'reviewer'];
        return otherRoles[(index - 1) % otherRoles.length];
    }

    // Returns true if a second orchestrator would be added if agent `forIndex` is set to orchestrator
    function orchestratorTaken(forIndex: number): boolean {
        return agents.some((a, i) => i !== forIndex && a.role === 'orchestrator');
    }

    function defaultToolId(): string {
        // Prefer first available tool; fall back to 'opencode'
        const first = availableTools.find(t => t.available);
        return first?.id ?? 'opencode';
    }

    function defaultModelForTool(toolId: string): string {
        const models = modelsByTool[toolId];
        return models?.[0]?.id ?? '';
    }

    function defaultThinkingMode(toolId: string): string {
        const modes = thinkingModesByTool[toolId];
        return modes?.[0]?.id ?? 'auto';
    }

    function buildNewAgent(index: number): AgentConfig {
        const toolId = defaultToolId();
        return {
            cliTool: toolId,
            model: defaultModelForTool(toolId),
            role: getDefaultRole(index),
            thinkingMode: defaultThinkingMode(toolId),
        };
    }

    // Resize agents array when agentCount changes (preserve existing configs)
    function syncAgentsToCount(newCount: number) {
        if (newCount > agents.length) {
            for (let i = agents.length; i < newCount; i++) {
                agents.push(buildNewAgent(i));
            }
        } else if (newCount < agents.length) {
            agents.splice(newCount);
        }
    }

    // When a tool is changed for an agent, reload its model options if not cached
    async function onToolChange(agentIndex: number, toolId: string) {
        agents[agentIndex].cliTool = toolId;
        // Load models for this tool if not already loaded
        if (!modelsByTool[toolId]) {
            try {
                const models = await listModelsForTool(toolId);
                modelsByTool[toolId] = models;
            } catch {
                modelsByTool[toolId] = [];
            }
        }
        // Reset model/thinking to first available
        agents[agentIndex].model = defaultModelForTool(toolId);
        agents[agentIndex].thinkingMode = defaultThinkingMode(toolId);
    }

    // ── discovery on mount ───────────────────────────────────────────────────
    onMount(async () => {
        try {
            // Discover available tools
            const tools = await listAvailableCliTools();
            availableTools = tools;

            // For each available tool, fetch its models and thinking modes in parallel
            const availList = tools.filter(t => t.available);
            // Always include opencode even if not technically "available" on this machine
            const toolsToLoad = availList.length > 0 ? availList : [{ id: 'opencode', name: 'OpenCode', available: true, path: null }];

            await Promise.all(
                toolsToLoad.map(async (tool) => {
                    const [models, modes] = await Promise.all([
                        listModelsForTool(tool.id).catch(() => []),
                        listThinkingModesForTool(tool.id).catch(() => []),
                    ]);
                    modelsByTool[tool.id] = models;
                    thinkingModesByTool[tool.id] = modes;
                })
            );

            // Build initial agent list after discovery
            agents = Array.from({ length: agentCount }, (_, i) => buildNewAgent(i));
        } catch (err) {
            loadError = err instanceof Error ? err.message : String(err);
            // Still build agents with empty defaults so the UI isn't stuck
            agents = Array.from({ length: agentCount }, (_, i) => ({
                cliTool: 'opencode',
                model: '',
                role: getDefaultRole(i),
                thinkingMode: 'auto',
            }));
        } finally {
            loading = false;
        }
    });

    // ── form submit ──────────────────────────────────────────────────────────
    function handleStart() {
        if (!titleDraft.trim() || !goalDraft.trim() || !selectedDirectory) return;
        dispatch('start', {
            title: titleDraft.trim(),
            goal: goalDraft.trim(),
            directory: selectedDirectory,
            agentConfigs: agents.map(a => ({ ...a })),
        });
    }

    async function chooseFolder() {
        const selection = await invoke<string | null>('pick_folder_dialog', {
            title: 'Choose project folder for OpenFlow'
        });

        if (typeof selection === 'string') {
            selectedDirectory = selection;
        }
    }

    // ── derived helpers ──────────────────────────────────────────────────────
    function hasThinkingModes(toolId: string): boolean {
        return (thinkingModesByTool[toolId]?.length ?? 0) > 0;
    }
</script>

<div class="agent-config-panel">
    <div class="config-header">
        <h2>Configure OpenFlow</h2>
        <p>Set up your agent swarm</p>
    </div>

    {#if loading}
        <div class="loading-state">
            <span class="spinner"></span>
            <span>Discovering available tools and models…</span>
        </div>
    {:else}
        {#if loadError}
            <div class="error-banner">
                Could not fully discover tools: {loadError}. Using defaults.
            </div>
        {/if}

        <div class="config-section">
            <label class="section-label">
                <span>Number of agents</span>
                <input
                    type="range"
                    min="2"
                    max="20"
                    bind:value={agentCount}
                    oninput={() => syncAgentsToCount(agentCount)}
                />
                <span class="range-value">{agentCount}</span>
            </label>
        </div>

        <div class="agents-table-container">
            <div class="agents-list">
                <div class="agents-list-header">
                    <span class="col-num">#</span>
                    <span class="col-tool">CLI Tool</span>
                    <span class="col-model">Model</span>
                    <span class="col-role">Role</span>
                    <span class="col-thinking">Thinking</span>
                </div>

                {#each agents as agent, i (i)}
                    <div class="agent-row">
                        <span class="agent-number">#{i + 1}</span>

                        <!-- CLI Tool -->
                        <select
                            value={agent.cliTool}
                            onchange={(e) => onToolChange(i, (e.target as HTMLSelectElement).value)}
                        >
                            {#each availableTools as tool}
                                <option value={tool.id} disabled={!tool.available}>
                                    {tool.name}{tool.available ? '' : ' (not installed)'}
                                </option>
                            {/each}
                            {#if availableTools.length === 0}
                                <option value="opencode">OpenCode</option>
                            {/if}
                        </select>

                        <!-- Model -->
                        <select bind:value={agent.model}>
                            {#each modelsByTool[agent.cliTool] ?? [] as model}
                                <option value={model.id}>{model.name}{model.provider ? ` (${model.provider})` : ''}</option>
                            {/each}
                            {#if (modelsByTool[agent.cliTool]?.length ?? 0) === 0}
                                <option value="">Loading…</option>
                            {/if}
                        </select>

                        <!-- Role -->
                        <select bind:value={agent.role}>
                            {#each availableRoles as role}
                                <option
                                    value={role.id}
                                    disabled={role.id === 'orchestrator' && orchestratorTaken(i)}
                                >
                                    {role.name}{role.id === 'orchestrator' && orchestratorTaken(i) ? ' (taken)' : ''}
                                </option>
                            {/each}
                        </select>

                        <!-- Thinking mode (only for tools that support it) -->
                        {#if hasThinkingModes(agent.cliTool)}
                            <select bind:value={agent.thinkingMode}>
                                {#each thinkingModesByTool[agent.cliTool] as mode}
                                    <option value={mode.id} title={mode.description}>{mode.name}</option>
                                {/each}
                            </select>
                        {:else}
                            <span class="no-thinking">—</span>
                        {/if}
                    </div>
                {/each}
            </div>
        </div>

        <div class="config-section directory-section">
            <h3>Project Directory</h3>
            <div class="directory-picker">
                {#if selectedDirectory}
                    <span class="selected-directory">{selectedDirectory}</span>
                {:else}
                    <span class="no-directory">No directory selected</span>
                {/if}
                <button class="folder-btn" type="button" onclick={chooseFolder}>
                    Choose Folder
                </button>
            </div>
        </div>

        <div class="config-section goal-section">
            <h3>What do you want to build?</h3>
            <input
                class="goal-input"
                bind:value={titleDraft}
                placeholder="Run title (e.g., Login Page Project)"
            />
            <textarea
                class="goal-textarea"
                bind:value={goalDraft}
                rows="4"
                placeholder="Describe what you want to build in detail…"
            ></textarea>
        </div>

        <div class="config-actions">
            <button
                class="start-btn"
                type="button"
                onclick={handleStart}
                disabled={!titleDraft.trim() || !goalDraft.trim() || !selectedDirectory}
            >
                Start Orchestration
            </button>
        </div>
    {/if}
</div>

<style>
    .agent-config-panel {
        display: flex;
        flex-direction: column;
        gap: 32px;
        padding: clamp(16px, 4vw, 48px);
        max-width: 1000px;
        margin: 0 auto;
        width: 100%;
        overflow-y: auto;
        box-sizing: border-box;
    }

    .config-header {
        text-align: center;
        margin-bottom: 16px;
    }

    .config-header h2 {
        margin: 0 0 12px;
        font-size: 2rem;
        font-weight: 700;
        color: var(--ui-text-primary);
        letter-spacing: -0.02em;
    }

    .config-header p {
        margin: 0;
        color: var(--ui-text-muted);
        font-size: 1rem;
        max-width: 600px;
        margin: 0 auto;
        line-height: 1.5;
    }

    .loading-state {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 12px;
        padding: 48px;
        color: var(--ui-text-muted);
        font-size: 0.9rem;
    }

    .spinner {
        display: inline-block;
        width: 16px;
        height: 16px;
        border: 2px solid var(--ui-border-soft);
        border-top-color: var(--ui-accent);
        border-radius: 50%;
        animation: spin 0.8s linear infinite;
    }

    @keyframes spin {
        to { transform: rotate(360deg); }
    }

    .error-banner {
        padding: 10px 14px;
        background: color-mix(in srgb, var(--ui-accent-error, #e55) 12%, transparent);
        border: 1px solid var(--ui-accent-error, #e55);
        border-radius: 6px;
        font-size: 0.85rem;
        color: var(--ui-text-secondary);
    }

    .config-section {
        display: flex;
        flex-direction: column;
        gap: 16px;
        background: var(--ui-layer-2);
        padding: 24px;
        border-radius: 12px;
        border: 1px solid var(--ui-border-soft);
    }

    .section-label {
        display: flex;
        align-items: center;
        gap: 16px;
        font-size: 1rem;
        font-weight: 500;
        color: var(--ui-text-primary);
    }

    .section-label input[type="range"] {
        flex: 1;
        max-width: 300px;
        accent-color: var(--ui-accent);
    }

    .range-value {
        font-weight: 700;
        color: var(--ui-accent);
        min-width: 24px;
        font-size: 1.1rem;
    }

    .agents-table-container {
        width: 100%;
        overflow-x: auto;
        padding-bottom: 8px; /* space for scrollbar */
    }

    .agents-list {
        display: flex;
        flex-direction: column;
        gap: 8px;
        min-width: 600px;
    }

    .agents-list-header {
        display: grid;
        grid-template-columns: 40px 1fr 2fr 1.5fr 1fr;
        gap: 12px;
        padding: 0 16px 8px;
        font-size: 0.75rem;
        font-weight: 700;
        color: var(--ui-text-secondary);
        text-transform: uppercase;
        letter-spacing: 0.05em;
    }

    .agent-row {
        display: grid;
        grid-template-columns: 40px 1fr 2fr 1.5fr 1fr;
        gap: 12px;
        align-items: center;
        padding: 12px 16px;
        background: var(--ui-layer-1);
        border-radius: 8px;
        border: 1px solid var(--ui-border-soft);
        transition: border-color 0.2s ease, background 0.2s ease;
    }

    .agent-row:hover {
        border-color: color-mix(in srgb, var(--ui-accent) 40%, transparent);
        background: var(--ui-layer-2);
    }

    .agent-number {
        font-weight: 600;
        color: var(--ui-accent);
        font-size: 0.85rem;
    }

    .agent-row select {
        padding: 8px 12px;
        background: var(--ui-layer-0);
        border: 1px solid var(--ui-border-soft);
        border-radius: 6px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.95rem;
        font-weight: 500;
        cursor: pointer;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        transition: border-color 0.2s ease;
    }

    .agent-row select:focus {
        outline: none;
        border-color: var(--ui-accent);
    }

    .no-thinking {
        color: var(--ui-text-muted);
        font-size: 0.85rem;
        text-align: center;
    }

    .directory-section h3 {
        margin: 0 0 16px;
        font-size: 1.1rem;
        font-weight: 600;
        color: var(--ui-text-primary);
    }

    .directory-picker {
        display: flex;
        align-items: center;
        gap: 16px;
        padding: 16px;
        background: var(--ui-layer-0);
        border: 1px solid var(--ui-border-soft);
        border-radius: 8px;
    }

    .selected-directory {
        flex: 1;
        color: var(--ui-text-primary);
        font-size: 0.9rem;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .no-directory {
        flex: 1;
        color: var(--ui-text-muted);
        font-size: 0.9rem;
    }

    .folder-btn {
        padding: 10px 20px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: 6px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.9rem;
        cursor: pointer;
        transition: background 0.2s ease;
    }

    .folder-btn:hover {
        background: var(--ui-layer-3);
    }

    .goal-section h3 {
        margin: 0 0 16px;
        font-size: 1.1rem;
        font-weight: 600;
        color: var(--ui-text-primary);
    }

    .goal-input,
    .goal-textarea {
        width: 100%;
        padding: 16px;
        background: var(--ui-layer-0);
        border: 1px solid var(--ui-border-soft);
        border-radius: 8px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 1rem;
        outline: none;
        box-sizing: border-box;
        transition: border-color 0.2s ease, box-shadow 0.2s ease;
    }

    .goal-input:focus,
    .goal-textarea:focus {
        border-color: var(--ui-accent);
        box-shadow: 0 0 0 2px color-mix(in srgb, var(--ui-accent) 20%, transparent);
    }

    .goal-textarea {
        resize: vertical;
        min-height: 140px;
        line-height: 1.5;
    }

    .config-actions {
        display: flex;
        justify-content: flex-end;
        padding-top: 16px;
    }

    .start-btn {
        padding: 16px 40px;
        background: var(--ui-accent);
        border: none;
        border-radius: 8px;
        color: #fff;
        font: inherit;
        font-size: 1.1rem;
        font-weight: 600;
        cursor: pointer;
        transition: transform 0.2s ease, box-shadow 0.2s ease, opacity 0.2s;
        box-shadow: 0 4px 12px color-mix(in srgb, var(--ui-accent) 30%, transparent);
    }

    .start-btn:hover:not(:disabled) {
        transform: translateY(-2px);
        box-shadow: 0 6px 16px color-mix(in srgb, var(--ui-accent) 40%, transparent);
    }

    .start-btn:active:not(:disabled) {
        transform: translateY(0);
    }

    .start-btn:disabled {
        opacity: 0.5;
        cursor: not-allowed;
        box-shadow: none;
        transform: none;
    }
</style>
