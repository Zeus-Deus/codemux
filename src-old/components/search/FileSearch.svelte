<script lang="ts">
    import { onMount, tick } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';
    import { detectEditors, openInEditor } from '../../stores/workspace';
    import type { EditorInfo } from '../../stores/types';

    let { workspaceCwd, onClose }: {
        workspaceCwd: string;
        onClose: () => void;
    } = $props();

    let query = $state('');
    let results = $state<string[]>([]);
    let searching = $state(false);
    let selectedIndex = $state(0);
    let inputEl = $state<HTMLInputElement | null>(null);
    let listEl = $state<HTMLDivElement | null>(null);
    let editors = $state<EditorInfo[]>([]);
    let debounceTimer: ReturnType<typeof setTimeout> | null = null;

    function fuzzyMatch(q: string, text: string): { match: boolean; indices: number[] } {
        if (!q) return { match: true, indices: [] };
        const lower = text.toLowerCase();
        const ql = q.toLowerCase();
        const indices: number[] = [];
        let qi = 0;
        for (let i = 0; i < lower.length && qi < ql.length; i++) {
            if (lower[i] === ql[qi]) { indices.push(i); qi++; }
        }
        return { match: qi === ql.length, indices };
    }

    function fileName(path: string): string {
        const parts = path.split('/');
        return parts[parts.length - 1] || path;
    }

    function dirPath(path: string): string {
        const idx = path.lastIndexOf('/');
        return idx > 0 ? path.slice(0, idx) : '';
    }

    function renderHighlighted(text: string): Array<{ text: string; highlight: boolean }> {
        if (!query.trim()) return [{ text, highlight: false }];
        const { indices } = fuzzyMatch(query, text);
        const parts: Array<{ text: string; highlight: boolean }> = [];
        let last = 0;
        for (const idx of indices) {
            if (idx > last) parts.push({ text: text.slice(last, idx), highlight: false });
            parts.push({ text: text[idx], highlight: true });
            last = idx + 1;
        }
        if (last < text.length) parts.push({ text: text.slice(last), highlight: false });
        return parts;
    }

    async function doSearch(q: string) {
        if (!q.trim()) {
            results = [];
            return;
        }
        searching = true;
        try {
            const res = await invoke<string[]>('search_file_names', {
                path: workspaceCwd,
                query: q,
                maxResults: 50,
            });
            results = res;
            selectedIndex = 0;
        } catch (e) {
            console.error('search_file_names:', e);
            results = [];
        } finally {
            searching = false;
        }
    }

    function handleInput() {
        if (debounceTimer) clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => {
            void doSearch(query);
        }, 200);
    }

    async function openFile(relativePath: string) {
        if (editors.length > 0) {
            const fullPath = workspaceCwd + '/' + relativePath;
            try {
                await openInEditor(editors[0].id, fullPath);
            } catch (e) {
                console.error('open in editor:', e);
            }
        }
        onClose();
    }

    function scrollToSelected() {
        void tick().then(() => {
            const el = listEl?.querySelector('.file-row.selected');
            el?.scrollIntoView({ block: 'nearest' });
        });
    }

    function handleKeydown(event: KeyboardEvent) {
        if (event.key === 'Escape') {
            event.preventDefault();
            onClose();
        } else if (event.key === 'ArrowDown') {
            event.preventDefault();
            if (results.length > 0) {
                selectedIndex = (selectedIndex + 1) % results.length;
                scrollToSelected();
            }
        } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            if (results.length > 0) {
                selectedIndex = (selectedIndex - 1 + results.length) % results.length;
                scrollToSelected();
            }
        } else if (event.key === 'Enter') {
            event.preventDefault();
            if (results[selectedIndex]) {
                void openFile(results[selectedIndex]);
            }
        }
    }

    function handleBackdropClick(event: MouseEvent) {
        if ((event.target as HTMLElement).classList.contains('filesearch-backdrop')) {
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
<div class="filesearch-backdrop" onclick={handleBackdropClick} onkeydown={handleKeydown}>
    <div class="filesearch-dialog" role="dialog" aria-label="Find file">
        <div class="filesearch-input-wrap">
            <svg class="filesearch-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8l-6-6z" stroke="currentColor" stroke-width="1.5"/>
                <path d="M14 2v6h6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
            </svg>
            <input
                bind:this={inputEl}
                class="filesearch-input"
                type="text"
                placeholder="Find file by name..."
                bind:value={query}
                oninput={handleInput}
            />
            {#if searching}
                <div class="filesearch-spinner"></div>
            {/if}
        </div>
        <div class="filesearch-divider"></div>
        <div class="filesearch-results" bind:this={listEl}>
            {#if results.length === 0 && query.trim() && !searching}
                <div class="filesearch-empty">No files found</div>
            {:else if results.length === 0 && !query.trim()}
                <div class="filesearch-empty">Type a file name to search</div>
            {:else}
                {#each results as path, i (path)}
                    <!-- svelte-ignore a11y_no_static_element_interactions -->
                    <div
                        class="file-row"
                        class:selected={i === selectedIndex}
                        onmouseenter={() => { selectedIndex = i; }}
                        onclick={() => void openFile(path)}
                        onkeydown={() => {}}
                        role="option"
                        aria-selected={i === selectedIndex}
                    >
                        <div class="file-row-content">
                            <span class="file-name">
                                {#each renderHighlighted(fileName(path)) as part}
                                    {#if part.highlight}
                                        <span class="match">{part.text}</span>
                                    {:else}
                                        {part.text}
                                    {/if}
                                {/each}
                            </span>
                            {#if dirPath(path)}
                                <span class="file-dir">{dirPath(path)}</span>
                            {/if}
                        </div>
                    </div>
                {/each}
            {/if}
        </div>
    </div>
</div>

<style>
    .filesearch-backdrop {
        position: fixed;
        inset: 0;
        z-index: 100;
        display: flex;
        align-items: flex-start;
        justify-content: center;
        padding-top: 15vh;
        background: rgba(0, 0, 0, 0.4);
    }

    .filesearch-dialog {
        width: 100%;
        max-width: 560px;
        max-height: 60vh;
        display: flex;
        flex-direction: column;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-md);
        box-shadow: var(--ui-shadow-lg);
        overflow: hidden;
    }

    .filesearch-input-wrap {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 0 16px;
        height: 44px;
        flex-shrink: 0;
    }

    .filesearch-icon {
        flex-shrink: 0;
        color: var(--ui-text-muted);
    }

    .filesearch-input {
        flex: 1;
        height: 100%;
        background: transparent;
        border: none;
        outline: none;
        color: var(--ui-text-primary);
        font-size: 0.95rem;
        font-family: var(--ui-font-sans);
    }

    .filesearch-input::placeholder {
        color: var(--ui-text-muted);
    }

    .filesearch-spinner {
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

    .filesearch-divider {
        height: 1px;
        background: var(--ui-border-soft);
        flex-shrink: 0;
    }

    .filesearch-results {
        flex: 1;
        overflow-y: auto;
        overflow-x: hidden;
        min-height: 0;
        padding: 4px 0;
    }

    .filesearch-empty {
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 24px 16px;
        color: var(--ui-text-muted);
        font-size: 0.82rem;
    }

    .file-row {
        display: flex;
        align-items: center;
        height: 40px;
        padding: 0 16px;
        cursor: pointer;
        transition: background var(--ui-motion-fast);
        border-left: 2px solid transparent;
    }

    .file-row:hover,
    .file-row.selected {
        background: var(--ui-layer-3);
    }

    .file-row.selected {
        border-left-color: var(--ui-accent);
    }

    .file-row-content {
        display: flex;
        flex-direction: column;
        min-width: 0;
        gap: 1px;
    }

    .file-name {
        font-size: 0.82rem;
        font-weight: 600;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .file-name .match {
        color: var(--ui-accent);
    }

    .file-dir {
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
</style>
