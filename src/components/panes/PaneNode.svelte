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
    <section class:active={isActivePane(node.pane_id)} class="pane-shell terminal-pane-shell">
        <header class="pane-header">
            <div class="pane-header-main">
                <strong>{node.title}</strong>
                <span>Live shell</span>
            </div>

            <div class="pane-actions">
                <button class="pane-action" type="button" onclick={() => dispatch('split', { paneId: node.pane_id, direction: 'vertical' })}>Split down</button>
                <button class="pane-action" type="button" onclick={() => dispatch('split', { paneId: node.pane_id, direction: 'horizontal' })}>Split right</button>
                <button class="pane-action" type="button" onclick={() => dispatch('browser', { paneId: node.pane_id })}>Browser</button>
                <button class="pane-action danger" type="button" onclick={() => dispatch('close', { paneId: node.pane_id })}>Close</button>
            </div>
        </header>

        <div
            class="pane-content"
            role="button"
            tabindex="0"
            onclick={() => dispatch('activate', { paneId: node.pane_id })}
            onkeydown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    dispatch('activate', { paneId: node.pane_id });
                }
            }}
        >
            <TerminalPane sessionId={node.session_id} />
        </div>
    </section>
{:else if node.kind === 'browser'}
    <section class:active={isActivePane(node.pane_id)} class="pane-shell browser-pane-shell">
        <header class="pane-header">
            <div class="pane-header-main">
                <strong>{node.title}</strong>
                <span>Browser tools</span>
            </div>

            <div class="pane-actions always-visible">
                <button class="pane-action danger" type="button" onclick={() => dispatch('close', { paneId: node.pane_id })}>Close</button>
            </div>
        </header>

        <div
            class="pane-content browser-pane-content"
            role="button"
            tabindex="0"
            onclick={() => dispatch('activate', { paneId: node.pane_id })}
            onkeydown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    dispatch('activate', { paneId: node.pane_id });
                }
            }}
        >
            <BrowserPane browserId={node.browser_id} />
        </div>
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
                {#if index < node.children.length - 1}
                    <button
                        class={`split-handle ${node.direction} ${dragIndex === index ? 'dragging' : ''}`}
                        style={node.direction === 'horizontal' ? 'right: -4px;' : 'bottom: -4px;'}
                        type="button"
                        aria-label="Resize split"
                        onpointerdown={(event) => startResize(event, index)}
                    ></button>
                {/if}
            </div>
        {/each}
    </section>
{/if}

<style>
    .split-pane {
        display: grid;
        gap: 6px;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        padding: 6px;
        box-sizing: border-box;
    }

    .split-pane.horizontal {
        grid-auto-flow: column;
        align-items: stretch;
        grid-auto-columns: minmax(0, 1fr);
    }

    .split-pane.vertical {
        grid-auto-flow: row;
        align-items: stretch;
        grid-auto-rows: minmax(0, 1fr);
    }

    .split-child,
    .pane-shell,
    .pane-content,
    .browser-pane-content {
        min-width: 0;
        min-height: 0;
    }

    .split-child {
        position: relative;
        overflow: hidden;
    }

    .pane-shell {
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        overflow: hidden;
        border: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 10%, transparent);
        border-radius: 10px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 94%, black 6%);
        transition:
            border-color 140ms ease-out,
            background 140ms ease-out;
    }

    .pane-shell.active {
        border-color: color-mix(in srgb, var(--theme-accent, #7aa2f7) 30%, transparent);
        background: color-mix(in srgb, var(--theme-accent, #7aa2f7) 6%, var(--theme-background, #1a1b26) 94%);
    }

    .pane-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        padding: 8px 10px;
        border-bottom: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 8%, transparent);
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 92%, transparent);
        flex: 0 0 auto;
    }

    .pane-header-main {
        min-width: 0;
    }

    .pane-header-main strong {
        display: block;
        font-size: 0.84rem;
        font-weight: 600;
    }

    .pane-header-main span {
        display: block;
        margin-top: 2px;
        font-size: 0.7rem;
        color: color-mix(in srgb, var(--theme-foreground, #c0caf5) 56%, transparent);
    }

    .pane-actions {
        display: flex;
        align-items: center;
        flex-wrap: wrap;
        justify-content: flex-end;
        gap: 6px;
        opacity: 0;
        pointer-events: none;
        transform: translateY(-2px);
        transition:
            opacity 140ms ease-out,
            transform 140ms ease-out;
    }

    .pane-shell:hover .pane-actions,
    .pane-shell.active .pane-actions,
    .pane-actions.always-visible {
        opacity: 1;
        pointer-events: auto;
        transform: translateY(0);
    }

    .pane-action {
        border: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 12%, transparent);
        border-radius: 8px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 82%, transparent);
        color: color-mix(in srgb, var(--theme-foreground, #c0caf5) 82%, white 18%);
        padding: 5px 8px;
        font: inherit;
        font-size: 0.7rem;
        cursor: pointer;
        transition:
            border-color 100ms ease-out,
            background 100ms ease-out,
            color 100ms ease-out;
    }

    .pane-action:hover {
        border-color: color-mix(in srgb, var(--theme-accent, #7aa2f7) 24%, transparent);
        background: color-mix(in srgb, var(--theme-accent, #7aa2f7) 10%, transparent);
        color: var(--theme-foreground, #c0caf5);
    }

    .pane-action.danger:hover {
        border-color: color-mix(in srgb, var(--theme-color1, #f7768e) 26%, transparent);
        background: color-mix(in srgb, var(--theme-color1, #f7768e) 10%, transparent);
        color: color-mix(in srgb, var(--theme-color1, #f7768e) 82%, white 18%);
    }

    .pane-content {
        flex: 1;
        width: 100%;
        height: 100%;
        min-height: 0;
        overflow: hidden;
        cursor: text;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 98%, black 2%);
    }

    .browser-pane-content {
        cursor: default;
    }

    .split-handle {
        position: absolute;
        z-index: 20;
        border: 0;
        border-radius: 6px;
        background: transparent;
        transition: background 100ms ease-out;
    }

    .split-pane.horizontal > .split-child > .split-handle {
        width: 8px;
        height: calc(100% - 12px);
        top: 6px;
        cursor: col-resize;
    }

    .split-pane.vertical > .split-child > .split-handle {
        width: calc(100% - 12px);
        height: 8px;
        left: 6px;
        cursor: row-resize;
    }

    .split-handle:hover,
    .split-handle.dragging {
        background: color-mix(in srgb, var(--theme-accent, #7aa2f7) 48%, transparent);
    }

    @media (max-width: 840px) {
        .split-pane {
            padding: 4px;
            gap: 4px;
        }

        .pane-header {
            flex-direction: column;
            align-items: flex-start;
        }

        .pane-actions {
            opacity: 1;
            pointer-events: auto;
            transform: none;
            width: 100%;
            justify-content: flex-start;
        }
    }
</style>
