<script lang="ts">
    import type { TerminalPreset } from '../../stores/types';
    import { applyPreset, setPresetPinned } from '../../stores/presets';
    import { showUiNotice, errorMessage } from '../../stores/uiNotice';
    import PresetIcon from './PresetIcon.svelte';

    let {
        workspaceId,
        presets,
        onEditPreset,
    }: {
        workspaceId: string;
        presets: TerminalPreset[];
        onEditPreset: (preset: TerminalPreset | null) => void;
    } = $props();

    let showGearMenu = $state(false);
    let contextPreset = $state<TerminalPreset | null>(null);
    let contextPos = $state({ x: 0, y: 0 });

    const pinnedPresets = $derived(presets.filter((p) => p.pinned));
    const unpinnedBuiltins = $derived(presets.filter((p) => p.is_builtin && !p.pinned));

    async function handlePresetClick(preset: TerminalPreset) {
        try {
            await applyPreset(workspaceId, preset.id);
        } catch (e) {
            showUiNotice(errorMessage(e), 'error');
        }
    }

    function handleContextMenu(event: MouseEvent, preset: TerminalPreset) {
        event.preventDefault();
        event.stopPropagation();
        contextPreset = preset;
        contextPos = { x: event.clientX, y: event.clientY };
    }

    async function handleContextAction(mode: 'current_terminal' | 'new_tab' | 'split_pane') {
        if (contextPreset) {
            try {
                await applyPreset(workspaceId, contextPreset.id, mode);
            } catch (e) {
                showUiNotice(errorMessage(e), 'error');
            }
        }
        contextPreset = null;
    }

    function handleEdit() {
        if (contextPreset) onEditPreset(contextPreset);
        contextPreset = null;
    }

    function handleUnpin() {
        if (contextPreset) {
            void setPresetPinned(contextPreset.id, false);
        }
        contextPreset = null;
    }

    function handleQuickAdd(preset: TerminalPreset) {
        void setPresetPinned(preset.id, true);
        showGearMenu = false;
    }

    function handleWindowClick() {
        if (showGearMenu) showGearMenu = false;
        if (contextPreset) contextPreset = null;
    }

    $effect(() => {
        if (showGearMenu || contextPreset) {
            window.addEventListener('click', handleWindowClick);
            return () => window.removeEventListener('click', handleWindowClick);
        }
    });
</script>

<div class="preset-bar">
    <div class="preset-gear">
        <button
            class="gear-btn"
            onclick={(e) => { e.stopPropagation(); showGearMenu = !showGearMenu; }}
            aria-label="Preset settings"
        >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M12.22 2h-.44a2 2 0 00-2 2v.18a2 2 0 01-1 1.73l-.43.25a2 2 0 01-2 0l-.15-.08a2 2 0 00-2.73.73l-.22.38a2 2 0 00.73 2.73l.15.1a2 2 0 011 1.72v.51a2 2 0 01-1 1.74l-.15.09a2 2 0 00-.73 2.73l.22.38a2 2 0 002.73.73l.15-.08a2 2 0 012 0l.43.25a2 2 0 011 1.73V20a2 2 0 002 2h.44a2 2 0 002-2v-.18a2 2 0 011-1.73l.43-.25a2 2 0 012 0l.15.08a2 2 0 002.73-.73l.22-.39a2 2 0 00-.73-2.73l-.15-.08a2 2 0 01-1-1.74v-.5a2 2 0 011-1.74l.15-.09a2 2 0 00.73-2.73l-.22-.38a2 2 0 00-2.73-.73l-.15.08a2 2 0 01-2 0l-.43-.25a2 2 0 01-1-1.73V4a2 2 0 00-2-2z" stroke="currentColor" stroke-width="1.5"/>
                <circle cx="12" cy="12" r="3" stroke="currentColor" stroke-width="1.5"/>
            </svg>
        </button>

        {#if showGearMenu}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="gear-dropdown" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
                <div class="dropdown-section-label">All presets</div>
                {#each presets as preset (preset.id)}
                    <button class="dropdown-item" onclick={() => { handlePresetClick(preset); showGearMenu = false; }}>
                        <PresetIcon icon={preset.icon} size={14} />
                        <span>{preset.name}</span>
                        {#if preset.pinned}
                            <span class="pin-badge">pinned</span>
                        {/if}
                    </button>
                {/each}

                <div class="dropdown-divider"></div>

                <button class="dropdown-item" onclick={() => { onEditPreset(null); showGearMenu = false; }}>
                    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
                        <path d="M7 2v10M2 7h10" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
                    </svg>
                    <span>Add preset...</span>
                </button>

                {#if unpinnedBuiltins.length > 0}
                    <div class="dropdown-divider"></div>
                    <div class="dropdown-section-label">Quick add</div>
                    {#each unpinnedBuiltins as preset (preset.id)}
                        <button class="dropdown-item" onclick={() => handleQuickAdd(preset)}>
                            <PresetIcon icon={preset.icon} size={14} />
                            <span>{preset.name}</span>
                        </button>
                    {/each}
                {/if}
            </div>
        {/if}
    </div>

    <div class="preset-items">
        {#each pinnedPresets as preset (preset.id)}
            <button
                class="preset-btn"
                onclick={() => handlePresetClick(preset)}
                oncontextmenu={(e) => handleContextMenu(e, preset)}
                title={preset.description || preset.name}
            >
                <PresetIcon icon={preset.icon} size={14} />
                <span class="preset-name">{preset.name}</span>
            </button>
        {/each}
    </div>
</div>

<!-- Context menu -->
{#if contextPreset}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="context-menu"
        style="left: {contextPos.x}px; top: {contextPos.y}px;"
        onclick={(e) => e.stopPropagation()}
        onkeydown={() => {}}
    >
        <button class="dropdown-item" onclick={() => handleContextAction('current_terminal')}>Run in current terminal</button>
        <button class="dropdown-item" onclick={() => handleContextAction('new_tab')}>Run in new tab</button>
        <button class="dropdown-item" onclick={() => handleContextAction('split_pane')}>Run in split pane</button>
        <div class="dropdown-divider"></div>
        {#if !contextPreset.is_builtin}
            <button class="dropdown-item" onclick={handleEdit}>Edit...</button>
        {/if}
        <button class="dropdown-item" onclick={handleUnpin}>Unpin</button>
    </div>
{/if}

<style>
    .preset-bar {
        display: flex;
        align-items: center;
        height: 32px;
        min-height: 32px;
        background: var(--ui-layer-1);
        border-bottom: 1px solid var(--ui-border-soft);
        padding: 0 4px;
        gap: 2px;
    }

    .preset-gear {
        position: relative;
        display: flex;
        align-items: center;
        flex-shrink: 0;
        padding: 0 2px;
    }

    .gear-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 26px;
        height: 26px;
        padding: 0;
        border: none;
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .gear-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .gear-dropdown {
        position: absolute;
        top: 100%;
        left: 0;
        z-index: 20;
        min-width: 200px;
        max-height: 400px;
        overflow-y: auto;
        padding: 4px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-md);
        box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
    }

    .preset-items {
        display: flex;
        align-items: center;
        gap: 4px;
        flex: 1;
        min-width: 0;
        overflow-x: auto;
        overflow-y: hidden;
        scrollbar-width: none;
        padding: 0 4px;
    }

    .preset-items::-webkit-scrollbar {
        display: none;
    }

    .preset-btn {
        display: flex;
        align-items: center;
        gap: 5px;
        padding: 3px 10px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-secondary);
        font-size: 0.75rem;
        font-family: var(--ui-font-sans);
        cursor: pointer;
        white-space: nowrap;
        flex-shrink: 0;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .preset-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-primary);
    }

    .preset-name {
        max-width: 100px;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .context-menu {
        position: fixed;
        z-index: 100;
        min-width: 180px;
        padding: 4px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-md);
        box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
    }

    .dropdown-section-label {
        padding: 4px 10px 2px;
        font-size: 0.7rem;
        font-family: var(--ui-font-sans);
        color: var(--ui-text-muted);
        text-transform: uppercase;
        letter-spacing: 0.04em;
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
        font-family: var(--ui-font-sans);
        cursor: pointer;
        text-align: left;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .dropdown-item:hover {
        background: var(--ui-layer-3);
        color: var(--ui-text-primary);
    }

    .dropdown-divider {
        height: 1px;
        margin: 4px 6px;
        background: var(--ui-border-soft);
    }

    .pin-badge {
        margin-left: auto;
        font-size: 0.65rem;
        color: var(--ui-text-muted);
        opacity: 0.7;
    }
</style>
