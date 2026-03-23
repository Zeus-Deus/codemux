<script lang="ts">
    import { onMount } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';
    import { detectEditors, openInEditor } from '../../stores/workspace';
    import type { EditorInfo } from '../../stores/types';

    interface FileEntry {
        name: string;
        path: string;
        is_dir: boolean;
        size: number | null;
    }

    let { workspaceCwd }: {
        workspaceCwd: string;
    } = $props();

    let expandedDirs = $state(new Set<string>());
    let dirContents = $state(new Map<string, FileEntry[]>());
    let loading = $state(new Set<string>());
    let selectedPath = $state<string | null>(null);
    let editors = $state<EditorInfo[]>([]);

    const EXT_COLORS: Record<string, string> = {
        rs: '#e07040',
        ts: '#e0c040',
        js: '#e0c040',
        tsx: '#e0c040',
        jsx: '#e0c040',
        svelte: '#e04820',
        json: 'var(--ui-success)',
        toml: 'var(--ui-success)',
        yaml: 'var(--ui-success)',
        yml: 'var(--ui-success)',
        md: '#6090e0',
        css: '#a060d0',
        scss: '#a060d0',
    };

    function getExtColor(name: string): string {
        const ext = name.split('.').pop()?.toLowerCase() ?? '';
        return EXT_COLORS[ext] ?? 'var(--ui-text-muted)';
    }

    async function fetchDir(dirPath: string) {
        if (loading.has(dirPath)) return;
        loading = new Set([...loading, dirPath]);
        try {
            const entries = await invoke<FileEntry[]>('list_directory', { path: dirPath });
            dirContents = new Map(dirContents).set(dirPath, entries);
        } catch (e) {
            console.error('list_directory:', e);
        } finally {
            const next = new Set(loading);
            next.delete(dirPath);
            loading = next;
        }
    }

    async function toggleDir(dirPath: string) {
        if (expandedDirs.has(dirPath)) {
            const next = new Set(expandedDirs);
            next.delete(dirPath);
            expandedDirs = next;
        } else {
            const next = new Set(expandedDirs);
            next.add(dirPath);
            expandedDirs = next;
            if (!dirContents.has(dirPath)) {
                await fetchDir(dirPath);
            }
        }
    }

    async function handleFileClick(filePath: string) {
        selectedPath = filePath;
        if (editors.length > 0) {
            try {
                await openInEditor(editors[0].id, filePath);
            } catch (e) {
                console.error('open in editor:', e);
            }
        }
    }

    async function refresh() {
        dirContents = new Map();
        expandedDirs = new Set();
        await fetchDir(workspaceCwd);
    }

    onMount(() => {
        fetchDir(workspaceCwd);
        detectEditors().then(eds => { editors = eds; }).catch(() => {});
    });

    function getEntries(dirPath: string): FileEntry[] {
        return dirContents.get(dirPath) ?? [];
    }
</script>

<div class="file-tree-panel">
    <div class="tree-toolbar">
        <button class="toolbar-btn" onclick={() => void refresh()} title="Refresh file tree">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M21 2v6h-6M3 12a9 9 0 0115.36-6.36L21 8M3 22v-6h6M21 12a9 9 0 01-15.36 6.36L3 16" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
        </button>
    </div>
    <div class="tree-content">
        {#snippet treeLevel(dirPath: string, depth: number)}
            {#each getEntries(dirPath) as entry (entry.path)}
                {#if entry.is_dir}
                    <!-- svelte-ignore a11y_no_static_element_interactions -->
                    <div
                        class="tree-row"
                        class:selected={selectedPath === entry.path}
                        style="padding-left: {8 + depth * 16}px"
                        onclick={() => void toggleDir(entry.path)}
                        onkeydown={(e) => { if (e.key === 'Enter') void toggleDir(entry.path); }}
                        role="treeitem"
                        tabindex="0"
                        aria-expanded={expandedDirs.has(entry.path)}
                    >
                        <svg class="tree-chevron" class:expanded={expandedDirs.has(entry.path)} width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                            <path d="M3 2l4 3-4 3" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                        <svg class="tree-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" style="color: var(--ui-accent)" aria-hidden="true">
                            <path d="M2 6a2 2 0 012-2h5l2 2h9a2 2 0 012 2v10a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" stroke="currentColor" stroke-width="1.5"/>
                        </svg>
                        <span class="tree-name">{entry.name}</span>
                    </div>
                    {#if expandedDirs.has(entry.path)}
                        {#if loading.has(entry.path)}
                            <div class="tree-loading" style="padding-left: {8 + (depth + 1) * 16}px">Loading...</div>
                        {:else}
                            {@render treeLevel(entry.path, depth + 1)}
                        {/if}
                    {/if}
                {:else}
                    <!-- svelte-ignore a11y_no_static_element_interactions -->
                    <div
                        class="tree-row"
                        class:selected={selectedPath === entry.path}
                        style="padding-left: {8 + depth * 16}px"
                        onclick={() => void handleFileClick(entry.path)}
                        onkeydown={(e) => { if (e.key === 'Enter') void handleFileClick(entry.path); }}
                        role="treeitem"
                        tabindex="0"
                    >
                        <svg class="tree-icon file-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" style="color: {getExtColor(entry.name)}" aria-hidden="true">
                            <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8l-6-6z" stroke="currentColor" stroke-width="1.5"/>
                            <path d="M14 2v6h6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                        </svg>
                        <span class="tree-name">{entry.name}</span>
                    </div>
                {/if}
            {/each}
        {/snippet}

        {@render treeLevel(workspaceCwd, 0)}

        {#if getEntries(workspaceCwd).length === 0 && !loading.has(workspaceCwd)}
            <div class="tree-empty">No files found</div>
        {/if}
    </div>
</div>

<style>
    .file-tree-panel {
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        background: var(--ui-layer-1);
    }

    .tree-toolbar {
        display: flex;
        align-items: center;
        justify-content: flex-end;
        padding: 4px 8px 0;
        flex-shrink: 0;
    }

    .toolbar-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 22px;
        height: 22px;
        padding: 0;
        border: none;
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .toolbar-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-primary);
    }

    .tree-content {
        flex: 1;
        overflow-y: auto;
        overflow-x: hidden;
        min-height: 0;
        padding: 4px 0;
    }

    .tree-row {
        display: flex;
        align-items: center;
        gap: 4px;
        height: 28px;
        padding-right: 8px;
        cursor: pointer;
        transition: background var(--ui-motion-fast);
        border-left: 2px solid transparent;
        user-select: none;
    }

    .tree-row:hover {
        background: var(--ui-layer-2);
    }

    .tree-row.selected {
        background: var(--ui-layer-2);
        border-left-color: var(--ui-accent);
    }

    .tree-chevron {
        flex-shrink: 0;
        color: var(--ui-text-muted);
        transition: transform var(--ui-motion-fast);
    }

    .tree-chevron.expanded {
        transform: rotate(90deg);
    }

    .tree-icon {
        flex-shrink: 0;
    }

    .file-icon {
        margin-left: 14px;
    }

    .tree-name {
        flex: 1;
        min-width: 0;
        font-size: 0.78rem;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .tree-loading {
        height: 28px;
        display: flex;
        align-items: center;
        font-size: 0.72rem;
        color: var(--ui-text-muted);
    }

    .tree-empty {
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 24px 16px;
        font-size: 0.78rem;
        color: var(--ui-text-muted);
    }
</style>
