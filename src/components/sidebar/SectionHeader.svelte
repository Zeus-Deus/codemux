<script lang="ts">
    import type { WorkspaceSectionSnapshot } from '../../stores/types';
    import { SECTION_PRESET_COLORS } from '../../stores/types';
    import SectionColorPicker from './SectionColorPicker.svelte';
    import { onMount } from 'svelte';

    let {
        section,
        workspaceCount,
        onToggleCollapse,
        onRename,
        onChangeColor,
        onDelete,
        onDragStart,
        isDragging = false,
    }: {
        section: WorkspaceSectionSnapshot;
        workspaceCount: number;
        onToggleCollapse: () => void;
        onRename: (name: string) => void;
        onChangeColor: (color: string) => void;
        onDelete: () => void;
        onDragStart?: (e: DragEvent) => void;
        isDragging?: boolean;
    } = $props();

    let renaming = $state(false);
    let renameDraft = $state('');
    let renameInputEl = $state<HTMLInputElement | null>(null);

    let contextMenu = $state<{ x: number; y: number } | null>(null);
    let colorPicker = $state<{ x: number; y: number } | null>(null);

    $effect(() => {
        if (renaming && renameInputEl) {
            renameInputEl.focus();
            renameInputEl.select();
        }
    });

    function startRename() {
        renameDraft = section.name;
        renaming = true;
        contextMenu = null;
    }

    function commitRename() {
        if (renaming && renameDraft.trim()) {
            onRename(renameDraft.trim());
        }
        renaming = false;
    }

    function handleContextMenu(e: MouseEvent) {
        e.preventDefault();
        e.stopPropagation();
        contextMenu = { x: e.clientX, y: e.clientY };
    }

    function handleColorAction(e: MouseEvent) {
        colorPicker = { x: e.clientX, y: e.clientY };
        contextMenu = null;
    }

    function handleDeleteAction() {
        contextMenu = null;
        onDelete();
    }

    onMount(() => {
        function dismiss(e: MouseEvent) {
            if (contextMenu) contextMenu = null;
        }
        window.addEventListener('mousedown', dismiss);
        return () => window.removeEventListener('mousedown', dismiss);
    });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="section-header-wrap"
    class:dragging={isDragging}
    draggable={onDragStart ? 'true' : undefined}
    ondragstart={(e) => { e.stopPropagation(); onDragStart?.(e); }}
    oncontextmenu={handleContextMenu}
>
    <button
        class="section-header"
        type="button"
        onclick={onToggleCollapse}
        ondblclick={(e) => { e.stopPropagation(); startRename(); }}
    >
        <span class="color-dot" style="background: {section.color};"></span>
        {#if renaming}
            <!-- svelte-ignore a11y_autofocus -->
            <input
                class="rename-input"
                bind:this={renameInputEl}
                bind:value={renameDraft}
                onclick={(e) => e.stopPropagation()}
                onblur={commitRename}
                onkeydown={(e) => {
                    e.stopPropagation();
                    if (e.key === 'Enter') commitRename();
                    if (e.key === 'Escape') { renaming = false; }
                }}
            />
        {:else}
            <span class="section-name">{section.name}</span>
        {/if}
        <span class="ws-count">{workspaceCount}</span>
        <span class="spacer"></span>
        <span class="chevron" class:open={!section.collapsed}>›</span>
    </button>
</div>

{#if contextMenu}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="ctx-menu"
        style="left: {contextMenu.x}px; top: {contextMenu.y}px;"
        onclick={(e) => e.stopPropagation()}
        onmousedown={(e) => e.stopPropagation()}
    >
        <button class="ctx-item" type="button" onclick={startRename}>Rename</button>
        <button class="ctx-item" type="button" onclick={handleColorAction}>Change color</button>
        <div class="ctx-divider"></div>
        <button class="ctx-item ctx-danger" type="button" onclick={handleDeleteAction}>Delete section</button>
    </div>
{/if}

{#if colorPicker}
    <SectionColorPicker
        colors={SECTION_PRESET_COLORS}
        selected={section.color}
        position={colorPicker}
        onPick={(color) => { onChangeColor(color); colorPicker = null; }}
        onClose={() => { colorPicker = null; }}
    />
{/if}

<style>
    .section-header-wrap {
        min-width: 0;
    }

    .section-header-wrap.dragging {
        opacity: 0.4;
    }

    .section-header {
        display: flex;
        align-items: center;
        gap: 6px;
        width: 100%;
        padding: 6px 8px;
        background: transparent;
        border: none;
        cursor: pointer;
        min-width: 0;
        transition: background var(--ui-motion-fast);
    }

    .section-header:hover {
        background: color-mix(in srgb, var(--ui-layer-3) 50%, transparent);
    }

    .color-dot {
        width: 8px;
        height: 8px;
        border-radius: 50%;
        flex-shrink: 0;
    }

    .section-name {
        font-size: 0.72rem;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--ui-text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        min-width: 0;
    }

    .rename-input {
        width: 100%;
        box-sizing: border-box;
        background: var(--ui-layer-2);
        border: 1px solid color-mix(in srgb, var(--ui-accent) 36%, transparent);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.72rem;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        padding: 2px 6px;
        outline: none;
        min-width: 0;
    }

    .ws-count {
        font-size: 0.65rem;
        color: var(--ui-text-muted);
        flex-shrink: 0;
    }

    .spacer {
        flex: 1;
    }

    .chevron {
        font-size: 0.8rem;
        color: var(--ui-text-muted);
        flex-shrink: 0;
        transition: transform var(--ui-motion-fast);
        transform: rotate(0deg);
    }

    .chevron.open {
        transform: rotate(90deg);
    }

    /* Context menu */
    .ctx-menu {
        position: fixed;
        z-index: 100;
        min-width: 160px;
        padding: 4px 0;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-md);
        box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    }

    .ctx-item {
        display: block;
        width: 100%;
        padding: 6px 12px;
        background: transparent;
        border: none;
        color: var(--ui-text-secondary);
        font: inherit;
        font-size: 0.78rem;
        text-align: left;
        cursor: pointer;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .ctx-item:hover {
        background: var(--ui-layer-3);
        color: var(--ui-text-primary);
    }

    .ctx-danger:hover {
        color: var(--ui-danger);
    }

    .ctx-divider {
        height: 1px;
        margin: 4px 0;
        background: var(--ui-border-soft);
    }
</style>
