<script lang="ts">
    import { onMount, tick } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';
    import { detectEditors, openInEditor } from '../../stores/workspace';
    import type { EditorInfo } from '../../stores/types';

    interface SearchResult {
        file_path: string;
        line_number: number;
        line_content: string;
        match_start: number;
        match_end: number;
    }

    interface FileGroup {
        filePath: string;
        matches: SearchResult[];
    }

    let { workspaceCwd, onClose }: {
        workspaceCwd: string;
        onClose: () => void;
    } = $props();

    let query = $state('');
    let results = $state<SearchResult[]>([]);
    let searching = $state(false);
    let selectedIndex = $state(0);
    let inputEl = $state<HTMLInputElement | null>(null);
    let listEl = $state<HTMLDivElement | null>(null);
    let editors = $state<EditorInfo[]>([]);
    let debounceTimer: ReturnType<typeof setTimeout> | null = null;

    const grouped = $derived.by(() => {
        const groups: FileGroup[] = [];
        const map = new Map<string, SearchResult[]>();
        for (const r of results) {
            const existing = map.get(r.file_path);
            if (existing) {
                existing.push(r);
            } else {
                const arr = [r];
                map.set(r.file_path, arr);
                groups.push({ filePath: r.file_path, matches: arr });
            }
        }
        return groups;
    });

    const flatResults = $derived(results);

    function relativePath(fullPath: string): string {
        if (fullPath.startsWith(workspaceCwd)) {
            const rel = fullPath.slice(workspaceCwd.length);
            return rel.startsWith('/') ? rel.slice(1) : rel;
        }
        return fullPath;
    }

    async function doSearch(q: string) {
        if (!q.trim()) {
            results = [];
            return;
        }
        searching = true;
        try {
            const res = await invoke<SearchResult[]>('search_in_files', {
                path: workspaceCwd,
                query: q,
                maxResults: 100,
            });
            results = res;
            selectedIndex = 0;
        } catch (e) {
            console.error('search_in_files:', e);
            results = [];
        } finally {
            searching = false;
        }
    }

    function handleInput() {
        if (debounceTimer) clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => {
            void doSearch(query);
        }, 300);
    }

    async function openResult(result: SearchResult) {
        if (editors.length > 0) {
            try {
                await openInEditor(editors[0].id, result.file_path);
            } catch (e) {
                console.error('open in editor:', e);
            }
        }
        onClose();
    }

    function scrollToSelected() {
        void tick().then(() => {
            const el = listEl?.querySelector('.match-row.selected');
            el?.scrollIntoView({ block: 'nearest' });
        });
    }

    function handleKeydown(event: KeyboardEvent) {
        if (event.key === 'Escape') {
            event.preventDefault();
            onClose();
        } else if (event.key === 'ArrowDown') {
            event.preventDefault();
            if (flatResults.length > 0) {
                selectedIndex = (selectedIndex + 1) % flatResults.length;
                scrollToSelected();
            }
        } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            if (flatResults.length > 0) {
                selectedIndex = (selectedIndex - 1 + flatResults.length) % flatResults.length;
                scrollToSelected();
            }
        } else if (event.key === 'Enter') {
            event.preventDefault();
            if (flatResults[selectedIndex]) {
                void openResult(flatResults[selectedIndex]);
            }
        }
    }

    function handleBackdropClick(event: MouseEvent) {
        if ((event.target as HTMLElement).classList.contains('search-backdrop')) {
            onClose();
        }
    }

    onMount(() => {
        inputEl?.focus();
        detectEditors().then(eds => { editors = eds; }).catch(() => {});
        return () => {
            if (debounceTimer) clearTimeout(debounceTimer);
        };
    });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="search-backdrop" onclick={handleBackdropClick} onkeydown={handleKeydown}>
    <div class="search-dialog" role="dialog" aria-label="Search in files">
        <div class="search-input-wrap">
            <svg class="search-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <circle cx="11" cy="11" r="8" stroke="currentColor" stroke-width="2"/>
                <path d="M21 21l-4.35-4.35" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
            </svg>
            <input
                bind:this={inputEl}
                class="search-input"
                type="text"
                placeholder="Search in files..."
                bind:value={query}
                oninput={handleInput}
            />
            {#if searching}
                <div class="search-spinner"></div>
            {/if}
        </div>
        <div class="search-divider"></div>
        <div class="search-results" bind:this={listEl}>
            {#if flatResults.length === 0 && query.trim() && !searching}
                <div class="search-empty">No results found</div>
            {:else if flatResults.length === 0 && !query.trim()}
                <div class="search-empty">Type to search across files</div>
            {:else}
                {#each grouped as group}
                    <div class="file-group-header">{relativePath(group.filePath)}</div>
                    {#each group.matches as match}
                        {@const globalIdx = flatResults.indexOf(match)}
                        <!-- svelte-ignore a11y_no_static_element_interactions -->
                        <div
                            class="match-row"
                            class:selected={globalIdx === selectedIndex}
                            onmouseenter={() => { selectedIndex = globalIdx; }}
                            onclick={() => void openResult(match)}
                            onkeydown={() => {}}
                            role="option"
                            aria-selected={globalIdx === selectedIndex}
                        >
                            <span class="match-line-num">{match.line_number}</span>
                            <span class="match-content">
                                {#if match.match_start < match.match_end}
                                    {match.line_content.slice(0, match.match_start)}<span class="match-highlight">{match.line_content.slice(match.match_start, match.match_end)}</span>{match.line_content.slice(match.match_end)}
                                {:else}
                                    {match.line_content}
                                {/if}
                            </span>
                        </div>
                    {/each}
                {/each}
            {/if}
        </div>
    </div>
</div>

<style>
    .search-backdrop {
        position: fixed;
        inset: 0;
        z-index: 100;
        display: flex;
        align-items: flex-start;
        justify-content: center;
        padding-top: 12vh;
        background: rgba(0, 0, 0, 0.4);
    }

    .search-dialog {
        width: 100%;
        max-width: 600px;
        max-height: 70vh;
        display: flex;
        flex-direction: column;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-lg);
        box-shadow: 0 24px 60px rgba(0, 0, 0, 0.5);
        overflow: hidden;
    }

    .search-input-wrap {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 0 16px;
        height: 44px;
        flex-shrink: 0;
    }

    .search-icon {
        flex-shrink: 0;
        color: var(--ui-text-muted);
    }

    .search-input {
        flex: 1;
        height: 100%;
        background: transparent;
        border: none;
        outline: none;
        color: var(--ui-text-primary);
        font-size: 0.95rem;
        font-family: var(--ui-font-sans);
    }

    .search-input::placeholder {
        color: var(--ui-text-muted);
    }

    .search-spinner {
        width: 14px;
        height: 14px;
        border: 2px solid var(--ui-border-soft);
        border-top-color: var(--ui-accent);
        border-radius: 50%;
        animation: spin 0.6s linear infinite;
        flex-shrink: 0;
    }

    @keyframes spin {
        to { transform: rotate(360deg); }
    }

    .search-divider {
        height: 1px;
        background: var(--ui-border-soft);
        flex-shrink: 0;
    }

    .search-results {
        flex: 1;
        overflow-y: auto;
        overflow-x: hidden;
        min-height: 0;
        padding: 4px 0;
    }

    .search-empty {
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 24px 16px;
        color: var(--ui-text-muted);
        font-size: 0.82rem;
    }

    .file-group-header {
        padding: 8px 16px 4px;
        font-family: var(--ui-font-mono);
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        user-select: none;
        position: sticky;
        top: 0;
        background: var(--ui-layer-2);
        z-index: 1;
    }

    .match-row {
        display: flex;
        align-items: center;
        height: 30px;
        padding: 0 16px;
        cursor: pointer;
        transition: background var(--ui-motion-fast);
        border-left: 2px solid transparent;
        gap: 8px;
    }

    .match-row:hover,
    .match-row.selected {
        background: var(--ui-layer-3);
    }

    .match-row.selected {
        border-left-color: var(--ui-accent);
    }

    .match-line-num {
        flex-shrink: 0;
        width: 40px;
        font-family: var(--ui-font-mono);
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        text-align: right;
    }

    .match-content {
        flex: 1;
        min-width: 0;
        font-family: var(--ui-font-mono);
        font-size: 0.78rem;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .match-highlight {
        background: color-mix(in srgb, var(--ui-accent) 25%, transparent);
        color: var(--ui-accent);
        border-radius: 2px;
        padding: 1px 0;
    }
</style>
