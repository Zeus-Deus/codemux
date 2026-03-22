<script lang="ts">
    import { onMount } from 'svelte';
    import { createEventDispatcher } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';
    import { appState } from '../../stores/core';
    import { createWorkspaceWithPreset, createWorktreeWorkspace } from '../../stores/workspace';
    import { presetStore, applyPreset } from '../../stores/presets';
    import { listBranches, getGitBranchInfo } from '../../stores/git';
    import type { LayoutPreset, WorkspaceTemplateKind } from '../../stores/types';

    let {
        initialKind = 'codemux',
        initialLayout = 'single'
    }: {
        initialKind?: WorkspaceTemplateKind;
        initialLayout?: LayoutPreset;
    } = $props();

    const dispatch = createEventDispatcher<{ close: void }>();

    type WorkspaceSource = 'new_branch' | 'existing_branch' | 'folder' | 'openflow';

    let source = $state<WorkspaceSource>('new_branch');
    let selectedLayout = $state<LayoutPreset>('single');
    let selectedPresetId = $state('');
    let creating = $state(false);

    // Git state
    let isGitRepo = $state(true);
    let currentBranch = $state('main');
    let localBranches = $state<string[]>([]);
    let remoteBranches = $state<string[]>([]);
    let branchSearch = $state('');
    let selectedBranch = $state('');

    // New branch
    let newBranchName = $state('');
    let baseBranch = $state('');

    // Folder
    let selectedFolder = $state('');

    // OpenFlow
    let openflowTitle = $state('');
    let openflowGoal = $state('');

    // Get project cwd
    let projectCwd = $state('');

    const layoutOptions: Array<{ layout: LayoutPreset; label: string; slots: number }> = [
        { layout: 'single', label: '1', slots: 1 },
        { layout: 'pair', label: '2', slots: 2 },
        { layout: 'quad', label: '4', slots: 4 },
        { layout: 'six', label: '6', slots: 6 },
        { layout: 'eight', label: '8', slots: 8 },
        { layout: 'shell_browser', label: 'S+B', slots: 2 },
    ];

    const filteredBranches = $derived.by(() => {
        const q = branchSearch.toLowerCase();
        if (!q) return { local: localBranches, remote: remoteBranches };
        return {
            local: localBranches.filter(b => b.toLowerCase().includes(q)),
            remote: remoteBranches.filter(b => b.toLowerCase().includes(q) && !localBranches.includes(b)),
        };
    });

    function canCreate(): boolean {
        if (source === 'new_branch') return newBranchName.trim().length > 0;
        if (source === 'existing_branch') return selectedBranch.length > 0;
        if (source === 'folder') return selectedFolder.trim().length > 0;
        if (source === 'openflow') return openflowTitle.trim().length > 0 && openflowGoal.trim().length > 0;
        return false;
    }

    async function handleCreate() {
        if (!canCreate() || creating) return;
        creating = true;
        try {
            if (source === 'new_branch') {
                const wsId = await createWorktreeWorkspace(projectCwd, newBranchName.trim(), true, selectedLayout, baseBranch || null);
                if (selectedPresetId) await applyPreset(wsId, selectedPresetId, 'existing_panes');
            } else if (source === 'existing_branch') {
                const wsId = await createWorktreeWorkspace(projectCwd, selectedBranch, false, selectedLayout);
                if (selectedPresetId) await applyPreset(wsId, selectedPresetId, 'existing_panes');
            } else if (source === 'folder') {
                const result = await createWorkspaceWithPreset({
                    kind: 'folder' as WorkspaceTemplateKind,
                    layout: selectedLayout,
                    cwd: selectedFolder,
                });
                if (selectedPresetId && result.workspaceId) await applyPreset(result.workspaceId, selectedPresetId, 'existing_panes');
            } else if (source === 'openflow') {
                await createWorkspaceWithPreset({
                    kind: 'openflow' as WorkspaceTemplateKind,
                    layout: selectedLayout,
                    openflowTitle,
                    openflowGoal,
                });
            }
            dispatch('close');
        } catch (error) {
            console.error('Failed to create workspace:', error);
        } finally {
            creating = false;
        }
    }

    async function chooseFolder() {
        const selection = await invoke<string | null>('pick_folder_dialog', { title: 'Choose workspace folder' });
        if (typeof selection === 'string') selectedFolder = selection;
    }

    onMount(async () => {
        // Get cwd from the active workspace
        const activeWs = $appState?.workspaces.find(w => w.workspace_id === $appState?.active_workspace_id);
        projectCwd = activeWs?.cwd ?? '';

        if (projectCwd) {
            try {
                const branchInfo = await getGitBranchInfo(projectCwd);
                currentBranch = branchInfo.branch ?? 'main';
                baseBranch = currentBranch;
                const [local, remote] = await Promise.all([
                    listBranches(projectCwd, false),
                    listBranches(projectCwd, true),
                ]);
                localBranches = local;
                remoteBranches = remote.filter(b => !local.includes(b));
                isGitRepo = true;
            } catch {
                isGitRepo = false;
                source = 'folder';
            }
        } else {
            isGitRepo = false;
            source = 'folder';
        }

        if (initialKind === 'openflow') source = 'openflow';
    });
</script>

<div class="launcher-backdrop" role="presentation" onclick={() => dispatch('close')}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
        class="launcher-shell"
        role="dialog"
        aria-modal="true"
        aria-label="New workspace"
        tabindex="-1"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => { if (e.key === 'Escape') dispatch('close'); }}
    >
        <header class="launcher-header">
            <h2>New Workspace</h2>
            <button class="close-btn" type="button" onclick={() => dispatch('close')} aria-label="Close">
                <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                    <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                </svg>
            </button>
        </header>

        <!-- Source selection -->
        <div class="source-row">
            {#if isGitRepo}
                <button class="source-btn" class:active={source === 'new_branch'} onclick={() => { source = 'new_branch'; }}>
                    New branch
                </button>
                <button class="source-btn" class:active={source === 'existing_branch'} onclick={() => { source = 'existing_branch'; }}>
                    Existing branch
                </button>
            {/if}
            <button class="source-btn" class:active={source === 'folder'} onclick={() => { source = 'folder'; }}>
                Local folder
            </button>
            <button class="source-btn" class:active={source === 'openflow'} onclick={() => { source = 'openflow'; }}>
                OpenFlow
            </button>
        </div>

        <div class="launcher-body">
            <!-- Branch config -->
            {#if source === 'new_branch'}
                <div class="field-group">
                    <label class="field-label">Branch name</label>
                    <input
                        class="field-input"
                        type="text"
                        placeholder="feature/my-feature"
                        bind:value={newBranchName}
                    />
                </div>
                <div class="field-group">
                    <label class="field-label">Base branch</label>
                    <select class="field-select" bind:value={baseBranch}>
                        {#each localBranches as branch}
                            <option value={branch}>{branch}</option>
                        {/each}
                    </select>
                </div>
            {:else if source === 'existing_branch'}
                <div class="field-group">
                    <label class="field-label">Search branches</label>
                    <input
                        class="field-input"
                        type="text"
                        placeholder="Filter branches..."
                        bind:value={branchSearch}
                    />
                </div>
                <div class="branch-list">
                    {#if filteredBranches.local.length > 0}
                        <div class="branch-section-label">Local</div>
                        {#each filteredBranches.local as branch}
                            <button
                                class="branch-row"
                                class:selected={selectedBranch === branch}
                                onclick={() => { selectedBranch = branch; }}
                            >
                                {branch}
                            </button>
                        {/each}
                    {/if}
                    {#if filteredBranches.remote.length > 0}
                        <div class="branch-section-label">Remote</div>
                        {#each filteredBranches.remote as branch}
                            <button
                                class="branch-row"
                                class:selected={selectedBranch === branch}
                                onclick={() => { selectedBranch = branch; }}
                            >
                                {branch}
                            </button>
                        {/each}
                    {/if}
                    {#if filteredBranches.local.length === 0 && filteredBranches.remote.length === 0}
                        <div class="branch-empty">No matching branches</div>
                    {/if}
                </div>
            {:else if source === 'folder'}
                <div class="field-group">
                    <label class="field-label">Folder</label>
                    <div class="folder-row">
                        <input class="field-input" type="text" bind:value={selectedFolder} placeholder="Choose a directory" readonly />
                        <button class="secondary-btn" type="button" onclick={chooseFolder}>Browse</button>
                    </div>
                </div>
            {:else if source === 'openflow'}
                <div class="field-group">
                    <label class="field-label">Run title</label>
                    <input class="field-input" type="text" bind:value={openflowTitle} placeholder="Release polish" />
                </div>
                <div class="field-group">
                    <label class="field-label">Run goal</label>
                    <textarea class="field-textarea" bind:value={openflowGoal} rows="3" placeholder="Describe the mission..."></textarea>
                </div>
            {/if}

            <!-- Layout strip -->
            {#if source !== 'openflow'}
                <div class="field-group">
                    <label class="field-label">Layout</label>
                    <div class="layout-strip">
                        {#each layoutOptions as opt}
                            <button
                                class="layout-chip"
                                class:active={selectedLayout === opt.layout}
                                onclick={() => { selectedLayout = opt.layout; }}
                            >
                                {opt.label}
                            </button>
                        {/each}
                    </div>
                </div>
            {/if}

            <!-- Preset -->
            {#if source !== 'openflow' && $presetStore && $presetStore.presets.length > 0}
                <div class="field-group">
                    <label class="field-label">Preset <span class="optional">(optional)</span></label>
                    <select class="field-select" bind:value={selectedPresetId}>
                        <option value="">None</option>
                        {#each $presetStore.presets.filter(p => p.pinned) as preset (preset.id)}
                            <option value={preset.id}>{preset.name}</option>
                        {/each}
                    </select>
                </div>
            {/if}
        </div>

        <footer class="launcher-footer">
            <button class="secondary-btn" type="button" onclick={() => dispatch('close')}>Cancel</button>
            <button class="primary-btn" type="button" onclick={handleCreate} disabled={!canCreate() || creating}>
                {creating ? 'Creating...' : 'Create workspace'}
            </button>
        </footer>
    </div>
</div>

<style>
    .launcher-backdrop {
        position: fixed;
        inset: 0;
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 24px;
        background: rgba(5, 7, 12, 0.72);
        z-index: 1200;
    }

    .launcher-shell {
        width: min(500px, 100%);
        max-height: min(640px, calc(100dvh - 48px));
        display: flex;
        flex-direction: column;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-lg);
        background: var(--ui-layer-1);
        color: var(--ui-text-primary);
        box-shadow: 0 24px 60px rgba(0, 0, 0, 0.35);
        overflow: hidden;
    }

    .launcher-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 14px 16px;
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .launcher-header h2 {
        margin: 0;
        font-size: 0.92rem;
        font-weight: 600;
    }

    .close-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 24px;
        height: 24px;
        padding: 0;
        border: none;
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
    }

    .close-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    /* Source tabs */
    .source-row {
        display: flex;
        gap: 4px;
        padding: 8px 16px;
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .source-btn {
        padding: 4px 10px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        font-size: 0.78rem;
        cursor: pointer;
        transition: all 120ms ease-out;
    }

    .source-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .source-btn.active {
        background: color-mix(in srgb, var(--ui-accent) 12%, transparent);
        color: var(--ui-accent);
        border-color: color-mix(in srgb, var(--ui-accent) 30%, transparent);
    }

    /* Body */
    .launcher-body {
        flex: 1;
        overflow-y: auto;
        padding: 12px 16px;
        display: flex;
        flex-direction: column;
        gap: 12px;
        min-height: 0;
    }

    .field-group {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .field-label {
        font-size: 0.75rem;
        font-weight: 600;
        color: var(--ui-text-secondary);
    }

    .optional {
        font-weight: 400;
        color: var(--ui-text-muted);
    }

    .field-input,
    .field-textarea,
    .field-select {
        width: 100%;
        box-sizing: border-box;
        padding: 8px 10px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: var(--ui-layer-0);
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.82rem;
        outline: none;
        transition: border-color 120ms ease-out;
    }

    .field-input:focus,
    .field-textarea:focus {
        border-color: color-mix(in srgb, var(--ui-accent) 36%, transparent);
    }

    .field-input::placeholder,
    .field-textarea::placeholder {
        color: var(--ui-text-muted);
    }

    .field-select {
        appearance: auto;
    }

    .folder-row {
        display: flex;
        gap: 8px;
    }

    .folder-row .field-input {
        flex: 1;
    }

    /* Branch list */
    .branch-list {
        max-height: 200px;
        overflow-y: auto;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: var(--ui-layer-0);
    }

    .branch-section-label {
        padding: 6px 10px 2px;
        font-size: 0.68rem;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--ui-text-muted);
        user-select: none;
    }

    .branch-row {
        display: block;
        width: 100%;
        padding: 6px 10px;
        border: none;
        background: transparent;
        color: var(--ui-text-primary);
        font-family: var(--ui-font-mono);
        font-size: 0.78rem;
        text-align: left;
        cursor: pointer;
        transition: background 120ms ease-out;
    }

    .branch-row:hover {
        background: color-mix(in srgb, var(--ui-layer-2) 50%, transparent);
    }

    .branch-row.selected {
        background: color-mix(in srgb, var(--ui-accent) 12%, transparent);
        color: var(--ui-accent);
    }

    .branch-empty {
        padding: 16px;
        text-align: center;
        color: var(--ui-text-muted);
        font-size: 0.78rem;
    }

    /* Layout strip */
    .layout-strip {
        display: flex;
        gap: 4px;
    }

    .layout-chip {
        padding: 4px 10px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        font-family: var(--ui-font-mono);
        font-size: 0.75rem;
        cursor: pointer;
        transition: all 120ms ease-out;
    }

    .layout-chip:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .layout-chip.active {
        background: color-mix(in srgb, var(--ui-accent) 12%, transparent);
        color: var(--ui-accent);
        border-color: color-mix(in srgb, var(--ui-accent) 30%, transparent);
    }

    /* Footer */
    .launcher-footer {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        padding: 12px 16px;
        border-top: 1px solid var(--ui-border-soft);
    }

    .secondary-btn,
    .primary-btn {
        padding: 6px 14px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: var(--ui-layer-2);
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.8rem;
        cursor: pointer;
        transition: all 120ms ease-out;
    }

    .secondary-btn:hover,
    .primary-btn:hover {
        border-color: var(--ui-border-strong);
    }

    .primary-btn {
        background: color-mix(in srgb, var(--ui-accent) 14%, var(--ui-layer-2) 86%);
        border-color: color-mix(in srgb, var(--ui-accent) 24%, transparent);
    }

    .primary-btn:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }
</style>
