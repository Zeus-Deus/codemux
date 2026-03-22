<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import type { TabSnapshot } from '../../stores/types';

    let { tabs, activeTabId, workspaceId }: {
        tabs: TabSnapshot[];
        activeTabId: string;
        workspaceId: string;
    } = $props();

    const dispatch = createEventDispatcher<{
        activate: { tabId: string };
        close: { tabId: string };
        create: { kind: string };
    }>();

    let showDropdown = $state(false);
    let addBtnEl = $state<HTMLButtonElement | null>(null);
    let dropdownPos = $state({ x: 0, y: 0 });

    function handleTabClick(tabId: string) {
        if (tabId !== activeTabId) {
            dispatch('activate', { tabId });
        }
    }

    function handleTabClose(event: MouseEvent, tabId: string) {
        event.stopPropagation();
        dispatch('close', { tabId });
    }

    function handleAddClick(event: MouseEvent) {
        event.stopPropagation();
        if (!showDropdown && addBtnEl) {
            const rect = addBtnEl.getBoundingClientRect();
            dropdownPos = { x: rect.left, y: rect.bottom + 4 };
        }
        showDropdown = !showDropdown;
    }

    function handleCreateTab(kind: string) {
        dispatch('create', { kind });
        showDropdown = false;
    }

    function handleWindowClick() {
        if (showDropdown) showDropdown = false;
    }

    $effect(() => {
        if (showDropdown) {
            window.addEventListener('click', handleWindowClick);
            return () => window.removeEventListener('click', handleWindowClick);
        }
    });
</script>

<div class="tab-bar">
    <div class="tab-list">
        {#each tabs as tab (tab.tab_id)}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="tab-item"
                class:active={tab.tab_id === activeTabId}
                onclick={() => handleTabClick(tab.tab_id)}
                onkeydown={(e) => { if (e.key === 'Enter') handleTabClick(tab.tab_id); }}
                role="tab"
                tabindex="0"
                aria-selected={tab.tab_id === activeTabId}
            >
                <svg class="tab-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                    {#if tab.kind === 'terminal'}
                        <rect x="2" y="4" width="20" height="16" rx="3" stroke="currentColor" stroke-width="1.5"/>
                        <path d="M6 9l3 3-3 3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                    {:else if tab.kind === 'browser'}
                        <circle cx="12" cy="12" r="10" stroke="currentColor" stroke-width="1.5"/>
                        <path d="M2 12h20M12 2a15.3 15.3 0 014 10 15.3 15.3 0 01-4 10 15.3 15.3 0 01-4-10 15.3 15.3 0 014-10z" stroke="currentColor" stroke-width="1.5"/>
                    {:else if tab.kind === 'diff'}
                        <path d="M6 3v12M18 9v12M6 3C6 3 6 9 12 9s6-6 6-6M6 15c0 0 0 6 6 6s6-6 6-6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                    {/if}
                </svg>
                <span class="tab-title">{tab.title}</span>
                {#if tabs.length > 1}
                    <button
                        class="tab-close"
                        onclick={(e) => handleTabClose(e, tab.tab_id)}
                        aria-label="Close tab"
                    >
                        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                            <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                        </svg>
                    </button>
                {/if}
            </div>
        {/each}

        <div class="add-tab-wrap">
            <button bind:this={addBtnEl} class="add-tab-btn" onclick={handleAddClick} aria-label="Add tab">
                <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
                    <path d="M7 2v10M2 7h10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
                </svg>
            </button>
        </div>
    </div>
</div>

{#if showDropdown}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="add-tab-dropdown"
        style="left: {dropdownPos.x}px; top: {dropdownPos.y}px;"
        onclick={(e) => e.stopPropagation()}
        onkeydown={() => {}}
    >
        <button class="dropdown-item" onclick={() => handleCreateTab('terminal')}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <rect x="2" y="4" width="20" height="16" rx="3" stroke="currentColor" stroke-width="1.5"/>
                <path d="M6 9l3 3-3 3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
            Terminal
        </button>
        <button class="dropdown-item" onclick={() => handleCreateTab('browser')}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <circle cx="12" cy="12" r="10" stroke="currentColor" stroke-width="1.5"/>
                <path d="M2 12h20M12 2a15.3 15.3 0 014 10 15.3 15.3 0 01-4 10 15.3 15.3 0 01-4-10 15.3 15.3 0 014-10z" stroke="currentColor" stroke-width="1.5"/>
            </svg>
            Browser
        </button>
        <button class="dropdown-item" onclick={() => handleCreateTab('diff')}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M6 3v12M18 9v12M6 3C6 3 6 9 12 9s6-6 6-6M6 15c0 0 0 6 6 6s6-6 6-6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
            </svg>
            Changes
        </button>
    </div>
{/if}

<style>
    .tab-bar {
        display: flex;
        align-items: stretch;
        height: 36px;
        min-height: 36px;
        background: var(--ui-layer-1);
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .tab-list {
        display: flex;
        align-items: stretch;
        overflow-x: auto;
        overflow-y: hidden;
        scrollbar-width: none;
        min-width: 0;
        flex: 1;
    }

    .tab-list::-webkit-scrollbar {
        display: none;
    }

    .tab-item {
        display: flex;
        align-items: center;
        gap: 6px;
        width: 160px;
        min-width: 160px;
        padding: 0 10px;
        border: none;
        border-right: 1px solid var(--ui-border-soft);
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        position: relative;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .tab-item:hover {
        background: color-mix(in srgb, var(--ui-layer-2) 50%, transparent);
        color: var(--ui-text-secondary);
    }

    .tab-item.active {
        background: var(--ui-layer-2);
        color: var(--ui-text-primary);
        border-bottom: 2px solid var(--ui-accent);
    }

    .tab-icon {
        flex-shrink: 0;
        opacity: 0.7;
    }

    .tab-item.active .tab-icon {
        opacity: 1;
    }

    .tab-title {
        flex: 1;
        min-width: 0;
        font-size: 0.8rem;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        text-align: left;
    }

    .tab-close {
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
        transition: opacity var(--ui-motion-fast), background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .tab-item:hover .tab-close {
        opacity: 1;
    }

    .tab-close:hover {
        background: color-mix(in srgb, var(--ui-danger) 12%, transparent);
        color: var(--ui-danger);
    }

    .add-tab-wrap {
        position: relative;
        display: flex;
        align-items: center;
        padding: 0 6px;
    }

    .add-tab-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 28px;
        height: 28px;
        padding: 0;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .add-tab-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    /* Rendered at component root with position:fixed to escape .tab-list overflow:hidden */
    .add-tab-dropdown {
        position: fixed;
        z-index: 50;
        min-width: 180px;
        padding: 4px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-md);
        box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
    }

    .dropdown-item {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        padding: 6px 10px;
        border: none;
        border-radius: 4px;
        background: transparent;
        color: var(--ui-text-secondary);
        font-size: 0.8rem;
        cursor: pointer;
        text-align: left;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .dropdown-item:hover {
        background: var(--ui-layer-3);
        color: var(--ui-text-primary);
    }
</style>
