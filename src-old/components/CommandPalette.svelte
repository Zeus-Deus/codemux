<script lang="ts">
    import { onMount, tick } from 'svelte';
    import { appState } from '../stores/core';
    import { presetStore, applyPreset } from '../stores/presets';
    import {
        activateWorkspace,
        createTab,
        closeTab,
        detectEditors,
        openInEditor,
        createSection,
    } from '../stores/workspace';
    import { SECTION_PRESET_COLORS } from '../stores/types';
    import { getGitStatus, stageFiles, pushChanges } from '../stores/git';
    import { ghStatus } from '../stores/github';
    import type { EditorInfo } from '../stores/types';

    let { onClose }: { onClose: () => void } = $props();

    let query = $state('');
    let selectedIndex = $state(0);
    let inputEl = $state<HTMLInputElement | null>(null);
    let listEl = $state<HTMLDivElement | null>(null);
    let editors = $state<EditorInfo[]>([]);

    interface PaletteAction {
        id: string;
        group: string;
        label: string;
        shortcut?: string;
        execute: () => void;
    }

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

    function activeWs() {
        if (!$appState) return null;
        return $appState.workspaces.find(w => w.workspace_id === $appState!.active_workspace_id) ?? null;
    }

    const allActions = $derived.by(() => {
        const actions: PaletteAction[] = [];
        const ws = activeWs();

        // Workspaces
        actions.push({
            id: 'new-workspace',
            group: 'Workspaces',
            label: 'New Workspace',
            execute: () => { /* handled by dispatching event — App.svelte opens the launcher */ }
        });
        actions.push({
            id: 'new-section',
            group: 'Workspaces',
            label: 'New Section',
            execute: () => {
                const color = SECTION_PRESET_COLORS[
                    ($appState?.sections.length ?? 0) % SECTION_PRESET_COLORS.length
                ];
                void createSection('New Section', color);
            }
        });
        if ($appState) {
            for (const w of $appState.workspaces) {
                if (w.workspace_id === $appState.active_workspace_id) continue;
                actions.push({
                    id: `switch-ws-${w.workspace_id}`,
                    group: 'Workspaces',
                    label: `Switch to: ${w.title}`,
                    execute: () => { void activateWorkspace(w.workspace_id); }
                });
            }
        }

        // Tabs
        if (ws && ws.workspace_type === 'standard') {
            actions.push({
                id: 'new-terminal-tab',
                group: 'Tabs',
                label: 'New Terminal Tab',
                shortcut: 'Ctrl+T',
                execute: () => { void createTab(ws.workspace_id, 'terminal'); }
            });
            actions.push({
                id: 'new-browser-tab',
                group: 'Tabs',
                label: 'New Browser Tab',
                shortcut: 'Ctrl+Shift+B',
                execute: () => { void createTab(ws.workspace_id, 'browser'); }
            });
            if (ws.tabs.length > 1) {
                actions.push({
                    id: 'close-tab',
                    group: 'Tabs',
                    label: 'Close Tab',
                    shortcut: 'Ctrl+W',
                    execute: () => { void closeTab(ws.workspace_id, ws.active_tab_id); }
                });
            }
        }

        // Panels
        actions.push({
            id: 'toggle-file-tree',
            group: 'Panels',
            label: 'Toggle File Tree',
            shortcut: 'Ctrl+B',
            execute: () => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'b', ctrlKey: true, bubbles: true })); }
        });
        actions.push({
            id: 'toggle-changes',
            group: 'Panels',
            label: 'Toggle Changes Panel',
            shortcut: 'Ctrl+Shift+G',
            execute: () => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'g', ctrlKey: true, shiftKey: true, bubbles: true })); }
        });

        // Git
        if (ws && ws.git_changed_files > 0) {
            actions.push({
                id: 'stage-all',
                group: 'Git',
                label: 'Stage All Changes',
                execute: () => {
                    void getGitStatus(ws.cwd).then(files => {
                        const unstaged = files.filter(f => f.is_unstaged !== false).map(f => f.path);
                        if (unstaged.length > 0) void stageFiles(ws.cwd, unstaged);
                    });
                }
            });
        }
        if (ws && ws.git_ahead > 0) {
            actions.push({
                id: 'push',
                group: 'Git',
                label: 'Push',
                execute: () => { void pushChanges(ws.cwd); }
            });
        }

        // Pull Request (only when gh is authenticated)
        if ($ghStatus.status === 'Authenticated') {
            actions.push({
                id: 'pr-panel',
                group: 'Pull Request',
                label: ws?.pr_number ? 'View Pull Request' : 'Create Pull Request',
                shortcut: 'Ctrl+Shift+R',
                execute: () => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'r', ctrlKey: true, shiftKey: true, bubbles: true })); }
            });
            if (ws?.pr_number && ws?.pr_state === 'OPEN') {
                actions.push({
                    id: 'merge-pr',
                    group: 'Pull Request',
                    label: 'Merge Pull Request',
                    execute: () => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'r', ctrlKey: true, shiftKey: true, bubbles: true })); }
                });
            }
        }

        // Search
        actions.push({
            id: 'search-in-files',
            group: 'Search',
            label: 'Search in Files',
            shortcut: 'Ctrl+Shift+F',
            execute: () => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, shiftKey: true, bubbles: true })); }
        });
        actions.push({
            id: 'find-file',
            group: 'Search',
            label: 'Find File by Name',
            shortcut: 'Ctrl+P',
            execute: () => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'p', ctrlKey: true, bubbles: true })); }
        });

        // Tools
        if (editors.length > 0 && ws) {
            actions.push({
                id: 'open-in-editor',
                group: 'Tools',
                label: `Open in ${editors[0].name}`,
                shortcut: 'Ctrl+Shift+E',
                execute: () => { void openInEditor(editors[0].id, ws.cwd); }
            });
        }
        if ($appState) {
            for (const port of $appState.detected_ports) {
                actions.push({
                    id: `open-port-${port.port}`,
                    group: 'Tools',
                    label: `Open Browser for Port: ${port.port}`,
                    execute: () => { window.open(`http://localhost:${port.port}`, '_blank'); }
                });
            }
        }

        // Presets
        if ($presetStore && ws && ws.workspace_type === 'standard') {
            for (const preset of $presetStore.presets) {
                actions.push({
                    id: `preset-${preset.id}`,
                    group: 'Presets',
                    label: `Run: ${preset.name}`,
                    execute: () => { void applyPreset(ws.workspace_id, preset.id); }
                });
            }
        }

        // OpenFlow
        actions.push({
            id: 'new-openflow',
            group: 'OpenFlow',
            label: 'New OpenFlow Run',
            execute: () => { /* handled by App.svelte */ }
        });

        return actions;
    });

    const filtered = $derived.by(() => {
        if (!query.trim()) return allActions;
        return allActions.filter(a => fuzzyMatch(query, a.label).match);
    });

    $effect(() => {
        // Clamp selectedIndex when filtered list changes
        if (selectedIndex >= filtered.length) {
            selectedIndex = Math.max(0, filtered.length - 1);
        }
    });

    function scrollToSelected() {
        void tick().then(() => {
            const el = listEl?.querySelector('.result-row.selected');
            el?.scrollIntoView({ block: 'nearest' });
        });
    }

    function handleKeydown(event: KeyboardEvent) {
        if (event.key === 'Escape') {
            event.preventDefault();
            onClose();
        } else if (event.key === 'ArrowDown') {
            event.preventDefault();
            selectedIndex = filtered.length > 0 ? (selectedIndex + 1) % filtered.length : 0;
            scrollToSelected();
        } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            selectedIndex = filtered.length > 0 ? (selectedIndex - 1 + filtered.length) % filtered.length : 0;
            scrollToSelected();
        } else if (event.key === 'Enter') {
            event.preventDefault();
            if (filtered[selectedIndex]) {
                filtered[selectedIndex].execute();
                onClose();
            }
        }
    }

    function handleBackdropClick(event: MouseEvent) {
        if ((event.target as HTMLElement).classList.contains('palette-backdrop')) {
            onClose();
        }
    }

    function renderLabel(label: string): Array<{ text: string; highlight: boolean }> {
        if (!query.trim()) return [{ text: label, highlight: false }];
        const { indices } = fuzzyMatch(query, label);
        const parts: Array<{ text: string; highlight: boolean }> = [];
        let last = 0;
        for (const idx of indices) {
            if (idx > last) parts.push({ text: label.slice(last, idx), highlight: false });
            parts.push({ text: label[idx], highlight: true });
            last = idx + 1;
        }
        if (last < label.length) parts.push({ text: label.slice(last), highlight: false });
        return parts;
    }

    // Group headers logic — track which groups have been seen
    function groupLabel(index: number): string | null {
        const action = filtered[index];
        if (!action) return null;
        if (index === 0) return action.group;
        const prev = filtered[index - 1];
        return prev.group !== action.group ? action.group : null;
    }

    onMount(() => {
        inputEl?.focus();
        detectEditors().then(eds => { editors = eds; }).catch(() => {});
    });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="palette-backdrop" onclick={handleBackdropClick} onkeydown={handleKeydown}>
    <div class="palette-dialog" role="dialog" aria-label="Command palette">
        <div class="palette-input-wrap">
            <svg class="search-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <circle cx="11" cy="11" r="8" stroke="currentColor" stroke-width="2"/>
                <path d="M21 21l-4.35-4.35" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
            </svg>
            <input
                bind:this={inputEl}
                class="palette-input"
                type="text"
                placeholder="Type a command..."
                bind:value={query}
                oninput={() => { selectedIndex = 0; }}
            />
        </div>
        <div class="palette-divider"></div>
        <div class="palette-results" bind:this={listEl}>
            {#if filtered.length === 0}
                <div class="palette-empty">No matching commands</div>
            {:else}
                {#each filtered as action, i (action.id)}
                    {@const group = groupLabel(i)}
                    {#if group}
                        <div class="group-header">{group}</div>
                    {/if}
                    <!-- svelte-ignore a11y_no_static_element_interactions -->
                    <div
                        class="result-row"
                        class:selected={i === selectedIndex}
                        onmouseenter={() => { selectedIndex = i; }}
                        onclick={() => { action.execute(); onClose(); }}
                        onkeydown={() => {}}
                        role="option"
                        aria-selected={i === selectedIndex}
                    >
                        <span class="result-label">
                            {#each renderLabel(action.label) as part}
                                {#if part.highlight}
                                    <span class="match">{part.text}</span>
                                {:else}
                                    {part.text}
                                {/if}
                            {/each}
                        </span>
                        {#if action.shortcut}
                            <span class="result-shortcut">{action.shortcut}</span>
                        {/if}
                    </div>
                {/each}
            {/if}
        </div>
    </div>
</div>

<style>
    .palette-backdrop {
        position: fixed;
        inset: 0;
        z-index: 100;
        display: flex;
        align-items: flex-start;
        justify-content: center;
        padding-top: 15vh;
        background: rgba(0, 0, 0, 0.4);
    }

    .palette-dialog {
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

    .palette-input-wrap {
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

    .palette-input {
        flex: 1;
        height: 100%;
        background: transparent;
        border: none;
        outline: none;
        color: var(--ui-text-primary);
        font-size: 0.95rem;
        font-family: var(--ui-font-sans);
    }

    .palette-input::placeholder {
        color: var(--ui-text-muted);
    }

    .palette-divider {
        height: 1px;
        background: var(--ui-border-soft);
        flex-shrink: 0;
    }

    .palette-results {
        flex: 1;
        overflow-y: auto;
        overflow-x: hidden;
        min-height: 0;
        padding: 4px 0;
    }

    .palette-empty {
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 24px 16px;
        color: var(--ui-text-muted);
        font-size: 0.82rem;
    }

    .group-header {
        padding: 8px 16px 4px;
        font-size: 0.68rem;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--ui-text-muted);
        user-select: none;
    }

    .result-row {
        display: flex;
        align-items: center;
        height: 36px;
        padding: 0 16px;
        cursor: pointer;
        transition: background var(--ui-motion-fast);
        border-left: 2px solid transparent;
    }

    .result-row:hover,
    .result-row.selected {
        background: var(--ui-layer-3);
    }

    .result-row.selected {
        border-left-color: var(--ui-accent);
    }

    .result-label {
        flex: 1;
        min-width: 0;
        font-size: 0.85rem;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .result-label .match {
        color: var(--ui-accent);
        font-weight: 600;
    }

    .result-shortcut {
        flex-shrink: 0;
        margin-left: 12px;
        padding: 2px 6px;
        font-family: var(--ui-font-mono);
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        background: var(--ui-layer-3);
        border-radius: var(--ui-radius-sm);
    }

    .result-row.selected .result-shortcut {
        background: color-mix(in srgb, var(--ui-accent) 12%, transparent);
    }
</style>
