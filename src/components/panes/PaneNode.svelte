<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import PaneNode from './PaneNode.svelte';
    import TerminalPane from './TerminalPane.svelte';
    import BrowserPane from './BrowserPane.svelte';
    import type { PaneNodeSnapshot } from '../../stores/appState';

    let {
        node,
        activePaneId
    }: {
        node: PaneNodeSnapshot;
        activePaneId: string;
    } = $props();

    const dispatch = createEventDispatcher<{
        activate: { paneId: string };
        split: { paneId: string; direction: 'horizontal' | 'vertical' };
        close: { paneId: string };
        resize: { paneId: string; childSizes: number[] };
        browser: { paneId: string };
    }>();

    let container = $state<HTMLElement | null>(null);
    let dragIndex = $state<number | null>(null);

    function isActivePane(paneId: string) {
        return paneId === activePaneId;
    }

    function getChildSizes() {
        if (node.kind !== 'split') {
            return [];
        }

        const raw = node.child_sizes?.length === node.children.length
            ? [...node.child_sizes]
            : Array.from({ length: node.children.length }, () => 1 / node.children.length);
        const total = raw.reduce((sum, value) => sum + value, 0) || 1;
        return raw.map((value) => value / total);
    }

    function splitStyle() {
        if (node.kind !== 'split') {
            return '';
        }

        const sizes = getChildSizes().map((size) => `${Math.max(size, 0.1)}fr`).join(' ');
        return node.direction === 'horizontal'
            ? `grid-template-columns: ${sizes};`
            : `grid-template-rows: ${sizes};`;
    }

    function startResize(event: PointerEvent, index: number) {
        if (node.kind !== 'split' || !container) {
            return;
        }

        dragIndex = index;
        const rect = container.getBoundingClientRect();
        const sizes = getChildSizes();
        const axisSize = node.direction === 'horizontal' ? rect.width : rect.height;
        if (axisSize <= 0) {
            dragIndex = null;
            return;
        }

        const onMove = (moveEvent: PointerEvent) => {
            const pointerOffset = node.direction === 'horizontal'
                ? moveEvent.clientX - rect.left
                : moveEvent.clientY - rect.top;
            const before = Math.max(0.1, Math.min(pointerOffset / axisSize, 0.9));
            const totalPair = sizes[index] + sizes[index + 1];
            const first = Math.max(0.1, Math.min(before, totalPair - 0.1));
            const second = Math.max(0.1, totalPair - first);
            const next = [...sizes];
            next[index] = first;
            next[index + 1] = second;
            dispatch('resize', { paneId: node.pane_id, childSizes: next });
        };

        const onUp = () => {
            dragIndex = null;
            window.removeEventListener('pointermove', onMove);
            window.removeEventListener('pointerup', onUp);
        };

        window.addEventListener('pointermove', onMove);
        window.addEventListener('pointerup', onUp);
        event.preventDefault();
    }
</script>

{#if node.kind === 'terminal'}
    <section class:active={isActivePane(node.pane_id)} class="pane leaf-pane terminal-pane">
        <header class="pane-header">
            <div>
                <strong>{node.title}</strong>
                <span>{node.session_id}</span>
            </div>
            <div class="pane-actions">
                <button type="button" onclick={() => dispatch('split', { paneId: node.pane_id, direction: 'vertical' })}>Split Down</button>
                <button type="button" onclick={() => dispatch('split', { paneId: node.pane_id, direction: 'horizontal' })}>Split Right</button>
                <button type="button" onclick={() => dispatch('browser', { paneId: node.pane_id })}>Browser</button>
                <button type="button" onclick={() => dispatch('close', { paneId: node.pane_id })}>Close</button>
            </div>
        </header>
        <button class="focus-hitbox" type="button" onclick={() => dispatch('activate', { paneId: node.pane_id })}>
            <TerminalPane sessionId={node.session_id} />
        </button>
    </section>
{:else if node.kind === 'browser'}
    <section class:active={isActivePane(node.pane_id)} class="pane leaf-pane browser-pane">
        <header class="pane-header">
            <div>
                <strong>{node.title}</strong>
                <span>{node.browser_id}</span>
            </div>
            <div class="pane-actions">
                <button type="button" onclick={() => dispatch('close', { paneId: node.pane_id })}>Close</button>
            </div>
        </header>
        <button class="focus-hitbox browser-content" type="button" onclick={() => dispatch('activate', { paneId: node.pane_id })}>
            <BrowserPane browserId={node.browser_id} />
        </button>
    </section>
{:else}
    <section bind:this={container} class={`split-pane ${node.direction}`} style={splitStyle()}>
        {#each node.children as child, index (child.pane_id)}
            <div class="split-child">
                <PaneNode
                    node={child}
                    {activePaneId}
                    on:activate={(event) => dispatch('activate', event.detail)}
                    on:split={(event) => dispatch('split', event.detail)}
                    on:close={(event) => dispatch('close', event.detail)}
                    on:resize={(event) => dispatch('resize', event.detail)}
                    on:browser={(event) => dispatch('browser', event.detail)}
                />
            </div>
            {#if index < node.children.length - 1}
                <button
                    class={`split-handle ${node.direction} ${dragIndex === index ? 'dragging' : ''}`}
                    type="button"
                    aria-label="Resize split"
                    onpointerdown={(event) => startResize(event, index)}
                ></button>
            {/if}
        {/each}
    </section>
{/if}

<style>
    .split-pane {
        display: grid;
        gap: 10px;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
    }

    .split-pane.horizontal {
        grid-auto-flow: column;
        align-items: stretch;
    }

    .split-pane.vertical {
        grid-auto-flow: row;
        align-items: stretch;
    }

    .split-child,
    .pane,
    .focus-hitbox,
    .browser-content {
        min-width: 0;
        min-height: 0;
    }

    .leaf-pane {
        display: flex;
        flex-direction: column;
        border: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 10%, transparent);
        border-radius: 16px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 92%, white 8%);
        overflow: hidden;
    }

    .leaf-pane.active {
        border-color: color-mix(in srgb, var(--theme-accent, #7aa2f7) 50%, transparent);
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--theme-accent, #7aa2f7) 28%, transparent);
    }

    .pane-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        padding: 10px 12px;
        border-bottom: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 10%, transparent);
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 86%, black 14%);
    }

    .pane-header strong,
    .pane-header span {
        display: block;
    }

    .pane-header span {
        font-size: 0.75rem;
        color: color-mix(in srgb, var(--theme-foreground, #c0caf5) 72%, white 28%);
    }

    .pane-actions {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
    }

    .pane-actions button,
    .focus-hitbox {
        border: 0;
        background: transparent;
        color: inherit;
    }

    .pane-actions button {
        padding: 6px 9px;
        border-radius: 999px;
        border: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 14%, transparent);
        cursor: pointer;
    }

    .focus-hitbox {
        flex: 1;
        padding: 0;
        text-align: left;
        cursor: pointer;
    }

    .browser-content {
        display: block;
    }

    .split-handle {
        border: 0;
        background: color-mix(in srgb, var(--theme-accent, #7aa2f7) 24%, transparent);
        border-radius: 999px;
        align-self: stretch;
        justify-self: stretch;
        min-width: 8px;
        min-height: 8px;
        cursor: col-resize;
    }

    .split-handle.vertical {
        cursor: row-resize;
    }

    .split-handle.dragging {
        background: color-mix(in srgb, var(--theme-accent, #7aa2f7) 48%, transparent);
    }
</style>
