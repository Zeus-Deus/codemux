<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import type { GitFileStatus } from '../../stores/types';

    let { files, selectedFile }: {
        files: GitFileStatus[];
        selectedFile: string | null;
    } = $props();

    const dispatch = createEventDispatcher<{
        select: { path: string; staged: boolean };
        stage: { files: string[] };
        unstage: { files: string[] };
    }>();

    // Defensive: treat undefined is_staged as false, undefined is_unstaged as true
    const stagedFiles = $derived(files.filter(f => f.is_staged === true));
    const unstagedFiles = $derived(files.filter(f => f.is_unstaged !== false));

    function statusLetter(status: string): string {
        switch (status) {
            case 'added': return 'A';
            case 'modified': return 'M';
            case 'deleted': return 'D';
            case 'renamed': return 'R';
            case 'untracked': return 'U';
            case 'copied': return 'C';
            default: return '?';
        }
    }

    function statusClass(status: string): string {
        switch (status) {
            case 'added': return 'status-added';
            case 'modified': return 'status-modified';
            case 'deleted': return 'status-deleted';
            case 'renamed': return 'status-renamed';
            default: return 'status-muted';
        }
    }

    function fileName(path: string): string {
        const parts = path.split('/');
        return parts[parts.length - 1];
    }

    function handleAction(event: MouseEvent, action: 'stage' | 'unstage', path: string) {
        event.stopPropagation();
        dispatch(action, { files: [path] });
    }
</script>

<div class="file-list">
    <div class="file-entries">
        <!-- Staged section -->
        {#if stagedFiles.length > 0}
            <div class="section-header">
                <span class="section-label">Staged</span>
                <span class="file-count">{stagedFiles.length}</span>
                <button class="section-action" onclick={() => dispatch('unstage', { files: stagedFiles.map(f => f.path) })}>
                    Unstage All
                </button>
            </div>
            {#each stagedFiles as file (file.path + ':staged')}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                    class="file-row"
                    class:active={selectedFile === file.path}
                    onclick={() => dispatch('select', { path: file.path, staged: true })}
                    onkeydown={(e) => { if (e.key === 'Enter') dispatch('select', { path: file.path, staged: true }); }}
                    role="option"
                    tabindex="0"
                    aria-selected={selectedFile === file.path}
                    title={file.path}
                >
                    <span class="status-badge {statusClass(file.status)}">{statusLetter(file.status)}</span>
                    <span class="file-name">{fileName(file.path)}</span>
                    <button
                        class="action-btn unstage-btn"
                        onclick={(e) => handleAction(e, 'unstage', file.path)}
                        title="Unstage"
                    >
                        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                            <path d="M1.5 5h7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                        </svg>
                    </button>
                </div>
            {/each}
        {/if}

        <!-- Changes (unstaged) section -->
        <div class="section-header">
            <span class="section-label">Changes</span>
            <span class="file-count">{unstagedFiles.length}</span>
            {#if unstagedFiles.length > 0}
                <button class="section-action" onclick={() => dispatch('stage', { files: unstagedFiles.map(f => f.path) })}>
                    Stage All
                </button>
            {/if}
        </div>
        {#each unstagedFiles as file (file.path + ':unstaged')}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="file-row"
                class:active={selectedFile === file.path}
                onclick={() => dispatch('select', { path: file.path, staged: false })}
                onkeydown={(e) => { if (e.key === 'Enter') dispatch('select', { path: file.path, staged: false }); }}
                role="option"
                tabindex="0"
                aria-selected={selectedFile === file.path}
                title={file.path}
            >
                <span class="status-badge {statusClass(file.status)}">{statusLetter(file.status)}</span>
                <span class="file-name">{fileName(file.path)}</span>
                <button
                    class="action-btn stage-btn"
                    onclick={(e) => handleAction(e, 'stage', file.path)}
                    title="Stage"
                >
                    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                        <path d="M5 1.5v7M1.5 5h7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                    </svg>
                </button>
            </div>
        {/each}

        {#if stagedFiles.length === 0 && unstagedFiles.length === 0}
            <div class="empty-state">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                    <path d="M20 6L9 17l-5-5" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
                <span>Working tree clean</span>
            </div>
        {/if}
    </div>
</div>

<style>
    .file-list {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--ui-layer-1);
        overflow: hidden;
    }

    .file-entries {
        flex: 1;
        overflow-y: auto;
        overflow-x: hidden;
    }

    .section-header {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 8px 12px 4px;
        flex-shrink: 0;
    }

    .section-label {
        font-size: 0.72rem;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--ui-text-muted);
    }

    .file-count {
        font-size: 0.68rem;
        font-weight: 600;
        color: var(--ui-text-muted);
        background: var(--ui-layer-2);
        padding: 1px 6px;
        border-radius: 8px;
    }

    .section-action {
        margin-left: auto;
        padding: 2px 8px;
        border: 1px solid var(--ui-border-soft);
        border-radius: 4px;
        background: transparent;
        color: var(--ui-text-muted);
        font-size: 0.68rem;
        cursor: pointer;
        transition: all var(--ui-motion-fast);
    }

    .section-action:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .file-row {
        display: flex;
        align-items: center;
        gap: 8px;
        height: 32px;
        padding: 0 12px;
        cursor: pointer;
        transition: background var(--ui-motion-fast);
    }

    .file-row:hover {
        background: color-mix(in srgb, var(--ui-layer-2) 50%, transparent);
    }

    .file-row.active {
        background: var(--ui-layer-2);
    }

    .status-badge {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 16px;
        height: 16px;
        border-radius: 3px;
        font-family: var(--ui-font-mono);
        font-size: 0.65rem;
        font-weight: 700;
        flex-shrink: 0;
    }

    .status-added { color: var(--ui-success); background: color-mix(in srgb, var(--ui-success) 15%, transparent); }
    .status-modified { color: var(--ui-attention); background: color-mix(in srgb, var(--ui-attention) 15%, transparent); }
    .status-deleted { color: var(--ui-danger); background: color-mix(in srgb, var(--ui-danger) 15%, transparent); }
    .status-renamed { color: var(--ui-accent); background: color-mix(in srgb, var(--ui-accent) 15%, transparent); }
    .status-muted { color: var(--ui-text-muted); background: var(--ui-layer-2); }

    .file-name {
        flex: 1;
        min-width: 0;
        font-family: var(--ui-font-mono);
        font-size: 0.78rem;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .action-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 20px;
        height: 20px;
        padding: 0;
        border: none;
        border-radius: 4px;
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        flex-shrink: 0;
        opacity: 0;
        transition: all var(--ui-motion-fast);
    }

    .file-row:hover .action-btn {
        opacity: 1;
    }

    .stage-btn:hover {
        background: color-mix(in srgb, var(--ui-success) 15%, transparent);
        color: var(--ui-success);
    }

    .unstage-btn:hover {
        background: color-mix(in srgb, var(--ui-danger) 15%, transparent);
        color: var(--ui-danger);
    }

    .empty-state {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 8px;
        padding: 32px 16px;
        color: var(--ui-text-muted);
        font-size: 0.78rem;
    }
</style>
