<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import PaneNode from './PaneNode.svelte';
    import TerminalPane from './TerminalPane.svelte';
    import BrowserPane from './BrowserPane.svelte';
    import type { PaneNodeSnapshot } from '../../stores/types';
    import { paneDragState } from '../../stores/paneDrag';

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
        swap: { sourcePaneId: string; targetPaneId: string };
    }>();

    let container = $state<HTMLElement | null>(null);
    let dragIndex = $state<number | null>(null);
    let draggingSelf = $state(false);

    let activePointerDrag = $state<{
        sourcePaneId: string;
        sourceTitle: string;
        pointerId: number;
        startX: number;
        startY: number;
        dragging: boolean;
        targetPaneId: string | null;
        targetTitle: string | null;
        highlightedElement: HTMLElement | null;
        cleanup?: () => void;
    } | null>(null);

    function clearDropHighlight() {
        if (activePointerDrag?.highlightedElement) {
            activePointerDrag.highlightedElement.classList.remove('codemux-pane-drop-target');
            activePointerDrag.highlightedElement = null;
        }
    }

    function paneDropTargetAtPoint(clientX: number, clientY: number, sourcePaneId: string) {
        const paneShells = Array.from(document.querySelectorAll<HTMLElement>('.pane-shell[data-pane-drop-id]'));
        let dropHandle: HTMLElement | null = null;
        let smallestArea = Number.POSITIVE_INFINITY;

        for (const paneShell of paneShells) {
            const targetPaneId = paneShell.dataset.paneDropId;
            if (!targetPaneId || targetPaneId === sourcePaneId) {
                continue;
            }

            const style = getComputedStyle(paneShell);
            if (style.visibility !== 'visible' || style.pointerEvents === 'none') {
                continue;
            }

            const rect = paneShell.getBoundingClientRect();
            if (rect.width <= 0 || rect.height <= 0) {
                continue;
            }

            const containsPoint =
                clientX >= rect.left &&
                clientX <= rect.right &&
                clientY >= rect.top &&
                clientY <= rect.bottom;

            if (!containsPoint) {
                continue;
            }

            const area = rect.width * rect.height;
            if (area < smallestArea) {
                smallestArea = area;
                dropHandle = paneShell;
            }
        }

        const targetPaneId = dropHandle?.dataset.paneDropId;
        const targetTitle = dropHandle?.dataset.paneTitle ?? 'pane';

        if (!dropHandle || !targetPaneId) {
            clearDropHighlight();
            if (activePointerDrag && activePointerDrag.targetPaneId !== null) {
                activePointerDrag.targetPaneId = null;
                activePointerDrag.targetTitle = null;
                paneDragState.set({
                    sourcePaneId: activePointerDrag.sourcePaneId,
                    sourceTitle: activePointerDrag.sourceTitle,
                    dragging: activePointerDrag.dragging,
                    targetPaneId: null,
                    targetTitle: null,
                });
            }
            return null;
        }

        if (activePointerDrag?.highlightedElement !== dropHandle) {
            clearDropHighlight();
            dropHandle.classList.add('codemux-pane-drop-target');
            if (activePointerDrag) {
                activePointerDrag.highlightedElement = dropHandle;
            }
        }

        if (activePointerDrag && activePointerDrag.targetPaneId !== targetPaneId) {
            activePointerDrag.targetPaneId = targetPaneId;
            activePointerDrag.targetTitle = targetTitle;
            paneDragState.set({
                sourcePaneId: activePointerDrag.sourcePaneId,
                sourceTitle: activePointerDrag.sourceTitle,
                dragging: activePointerDrag.dragging,
                targetPaneId,
                targetTitle,
            });
        }

        return targetPaneId;
    }

    function clearPaneDragState() {
        clearDropHighlight();
        draggingSelf = false;
        paneDragState.set(null);
        if (activePointerDrag?.cleanup) {
            activePointerDrag.cleanup();
        }
        activePointerDrag = null;
    }

    function handlePanePointerDown(event: PointerEvent, paneId: string, title: string) {
        if (event.button !== 0) {
            return;
        }

        const target = event.target as HTMLElement | null;
        if (target?.closest('.pane-actions, .pane-icon-btn, button')) {
            return;
        }

        const onPointerMove = (moveEvent: PointerEvent) => {
            if (!activePointerDrag || activePointerDrag.pointerId !== moveEvent.pointerId) {
                return;
            }

            const distance = Math.hypot(
                moveEvent.clientX - activePointerDrag.startX,
                moveEvent.clientY - activePointerDrag.startY,
            );

            if (!activePointerDrag.dragging && distance > 8) {
                activePointerDrag.dragging = true;
                draggingSelf = true;
                paneDragState.set({
                    sourcePaneId: activePointerDrag.sourcePaneId,
                    sourceTitle: activePointerDrag.sourceTitle,
                    dragging: true,
                    targetPaneId: null,
                    targetTitle: null,
                });
            }

            if (activePointerDrag.dragging) {
                moveEvent.preventDefault();
                paneDropTargetAtPoint(moveEvent.clientX, moveEvent.clientY, activePointerDrag.sourcePaneId);
            }
        };

        const onPointerUp = (upEvent: PointerEvent) => {
            if (!activePointerDrag || activePointerDrag.pointerId !== upEvent.pointerId) {
                return;
            }

            const targetPaneId = activePointerDrag.dragging
                ? paneDropTargetAtPoint(upEvent.clientX, upEvent.clientY, activePointerDrag.sourcePaneId)
                : null;

            if (targetPaneId) {
                dispatch('swap', {
                    sourcePaneId: activePointerDrag.sourcePaneId,
                    targetPaneId,
                });
            }

            clearPaneDragState();
        };

        const onPointerCancel = (cancelEvent: PointerEvent) => {
            if (!activePointerDrag || activePointerDrag.pointerId !== cancelEvent.pointerId) {
                return;
            }

            clearPaneDragState();
        };

        const onWindowBlur = () => {
            if (!activePointerDrag || activePointerDrag.pointerId !== event.pointerId) {
                return;
            }

            clearPaneDragState();
        };

        activePointerDrag = {
            sourcePaneId: paneId,
            sourceTitle: title,
            pointerId: event.pointerId,
            startX: event.clientX,
            startY: event.clientY,
            dragging: false,
            targetPaneId: null,
            targetTitle: null,
            highlightedElement: null,
            cleanup: () => {
                window.removeEventListener('pointermove', onPointerMove);
                window.removeEventListener('pointerup', onPointerUp);
                window.removeEventListener('pointercancel', onPointerCancel);
                window.removeEventListener('blur', onWindowBlur);
            },
        };

        window.addEventListener('pointermove', onPointerMove);
        window.addEventListener('pointerup', onPointerUp);
        window.addEventListener('pointercancel', onPointerCancel);
        window.addEventListener('blur', onWindowBlur);
        event.preventDefault();
    }

    function isActive(paneId: string) {
        return paneId === activePaneId;
    }

    function isSwapTarget(paneId: string) {
        const state = $paneDragState;
        return state !== null && state.targetPaneId === paneId && state.sourcePaneId !== paneId;
    }

    function getChildSizes() {
        if (node.kind !== 'split') return [];
        const raw = node.child_sizes?.length === node.children.length
            ? [...node.child_sizes]
            : Array.from({ length: node.children.length }, () => 1 / node.children.length);
        const total = raw.reduce((s, v) => s + v, 0) || 1;
        return raw.map((v) => v / total);
    }

    function splitStyle() {
        if (node.kind !== 'split') return '';
        const sizes = getChildSizes().map((s) => `${Math.max(s, 0.1)}fr`).join(' ');
        return node.direction === 'horizontal'
            ? `grid-template-columns: ${sizes};`
            : `grid-template-rows: ${sizes};`;
    }

    function startResize(event: PointerEvent, index: number) {
        if (node.kind !== 'split' || !container) return;
        dragIndex = index;
        const rect = container.getBoundingClientRect();
        const sizes = getChildSizes();
        const axisSize = node.direction === 'horizontal' ? rect.width : rect.height;
        if (axisSize <= 0) { dragIndex = null; return; }

        const onMove = (e: PointerEvent) => {
            const pos = node.direction === 'horizontal' ? e.clientX - rect.left : e.clientY - rect.top;
            const before = Math.max(0.1, Math.min(pos / axisSize, 0.9));
            const pair = sizes[index] + sizes[index + 1];
            const first = Math.max(0.1, Math.min(before, pair - 0.1));
            const second = Math.max(0.1, pair - first);
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
        <section
            class="pane-shell"
            class:active={isActive(node.pane_id)}
            class:dragging={draggingSelf}
            class:swap-target={isSwapTarget(node.pane_id)}
        data-pane-drop-id={node.pane_id}
        data-pane-title={node.title}
    >
        <header 
            class="pane-header"
            role="presentation"
            data-pane-drop-id={node.pane_id}
            data-pane-title={node.title}
            onpointerdown={(event) => handlePanePointerDown(event, node.pane_id, node.title)}
        >
            <div
                class="pane-title-block"
                role="button"
                tabindex="0"
                onkeydown={(event) => {
                    if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault();
                        dispatch('activate', { paneId: node.pane_id });
                    }
                }}
            >
                <span class="pane-title">{node.title}</span>
                <span class="pane-subtitle">shell</span>
            </div>
            <div class="pane-actions">
                <!-- Split vertical (down) -->
                <button
                    class="pane-icon-btn"
                    type="button"
                    title="Split down"
                    onclick={() => dispatch('split', { paneId: node.pane_id, direction: 'vertical' })}
                    aria-label="Split pane down"
                >
                    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
                        <rect x="1" y="1" width="12" height="5.5" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
                        <rect x="1" y="7.5" width="12" height="5.5" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
                    </svg>
                </button>
                <!-- Split horizontal (right) -->
                <button
                    class="pane-icon-btn"
                    type="button"
                    title="Split right"
                    onclick={() => dispatch('split', { paneId: node.pane_id, direction: 'horizontal' })}
                    aria-label="Split pane right"
                >
                    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
                        <rect x="1" y="1" width="5.5" height="12" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
                        <rect x="7.5" y="1" width="5.5" height="12" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
                    </svg>
                </button>
                <!-- Open browser -->
                <button
                    class="pane-icon-btn"
                    type="button"
                    title="Open browser pane"
                    onclick={() => dispatch('browser', { paneId: node.pane_id })}
                    aria-label="Open browser pane"
                >
                    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
                        <rect x="1" y="2.5" width="12" height="9" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
                        <path d="M1 5h12" stroke="currentColor" stroke-width="1.2"/>
                        <circle cx="3.5" cy="3.75" r="0.7" fill="currentColor"/>
                        <circle cx="5.5" cy="3.75" r="0.7" fill="currentColor"/>
                    </svg>
                </button>
                <!-- Close -->
                <button
                    class="pane-icon-btn close"
                    type="button"
                    title="Close pane"
                    onclick={() => dispatch('close', { paneId: node.pane_id })}
                    aria-label="Close pane"
                >
                    <svg width="11" height="11" viewBox="0 0 11 11" fill="none" aria-hidden="true">
                        <path d="M1.5 1.5l8 8M9.5 1.5l-8 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
                    </svg>
                </button>
            </div>
        </header>

        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div
            class="pane-content"
            data-pane-drop-id={node.pane_id}
            data-pane-title={node.title}
            onclick={() => dispatch('activate', { paneId: node.pane_id })}
        >
            <TerminalPane sessionId={node.session_id} />
        </div>

        {#if draggingSelf && activePointerDrag?.sourcePaneId === node.pane_id}
            <div class="drag-overlay drag-origin" aria-hidden="true">
                <div class="drag-chip">Moving {activePointerDrag.sourceTitle}</div>
                <p>Drop on any other pane to swap places</p>
            </div>
        {:else if isSwapTarget(node.pane_id)}
            <div class="drag-overlay drag-origin" aria-hidden="true">
                <div class="drag-chip">Swap with {node.title}</div>
                <p>Drop to swap places</p>
            </div>
        {/if}

    </section>

{:else if node.kind === 'browser'}
    <section
        class="pane-shell browser"
        class:active={isActive(node.pane_id)}
        class:dragging={draggingSelf}
        class:swap-target={isSwapTarget(node.pane_id)}
        data-pane-drop-id={node.pane_id}
        data-pane-title={node.title}
    >
        <header 
            class="pane-header"
            role="presentation"
            data-pane-drop-id={node.pane_id}
            data-pane-title={node.title}
            onpointerdown={(event) => handlePanePointerDown(event, node.pane_id, node.title)}
        >
            <div
                class="pane-title-block"
                role="button"
                tabindex="0"
                onkeydown={(event) => {
                    if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault();
                        dispatch('activate', { paneId: node.pane_id });
                    }
                }}
            >
                <span class="pane-title">{node.title}</span>
                <span class="pane-subtitle">browser</span>
            </div>
            <div class="pane-actions always-visible">
                <button
                    class="pane-icon-btn close"
                    type="button"
                    title="Close pane"
                    onclick={() => dispatch('close', { paneId: node.pane_id })}
                    aria-label="Close browser pane"
                >
                    <svg width="11" height="11" viewBox="0 0 11 11" fill="none" aria-hidden="true">
                        <path d="M1.5 1.5l8 8M9.5 1.5l-8 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
                    </svg>
                </button>
            </div>
        </header>

        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div
            class="pane-content browser-content"
            data-pane-drop-id={node.pane_id}
            data-pane-title={node.title}
            onclick={() => dispatch('activate', { paneId: node.pane_id })}
        >
            <BrowserPane browserId={node.browser_id} />
        </div>

        {#if draggingSelf && activePointerDrag?.sourcePaneId === node.pane_id}
            <div class="drag-overlay drag-origin" aria-hidden="true">
                <div class="drag-chip">Moving {activePointerDrag.sourceTitle}</div>
                <p>Drop on any other pane to swap places</p>
            </div>
        {:else if isSwapTarget(node.pane_id)}
            <div class="drag-overlay drag-origin" aria-hidden="true">
                <div class="drag-chip">Swap with {node.title}</div>
                <p>Drop to swap places</p>
            </div>
        {/if}

    </section>

{:else}
    <section
        bind:this={container}
        class="split-pane {node.direction}"
        style={splitStyle()}
    >
        {#each node.children as child, index (child.pane_id)}
            <div class="split-child">
                <PaneNode
                    node={child}
                    {activePaneId}
                    on:activate={(e) => dispatch('activate', e.detail)}
                    on:split={(e) => dispatch('split', e.detail)}
                    on:close={(e) => dispatch('close', e.detail)}
                    on:resize={(e) => dispatch('resize', e.detail)}
                    on:browser={(e) => dispatch('browser', e.detail)}
                    on:swap={(e) => dispatch('swap', e.detail)}
                />
                {#if index < node.children.length - 1}
                    <button
                        class="split-handle {node.direction}"
                        class:dragging={dragIndex === index}
                        style={node.direction === 'horizontal' ? 'right: -5px;' : 'bottom: -5px;'}
                        type="button"
                        aria-label="Resize split"
                        onpointerdown={(e) => startResize(e, index)}
                    ></button>
                {/if}
            </div>
        {/each}
    </section>
{/if}

<style>
    /* ---- Split container ---- */

    .split-pane {
        display: grid;
        gap: 4px;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        padding: 4px;
        box-sizing: border-box;
        background: var(--ui-layer-0);
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

    .split-child {
        position: relative;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    /* ---- Pane shell ---- */

    .pane-shell {
        position: relative;
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        overflow: hidden;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-lg, 10px);
        background: var(--ui-layer-0);
        transition:
            border-color var(--ui-motion-fast),
            box-shadow var(--ui-motion-fast);
    }

    .pane-shell.active {
        border-color: color-mix(in srgb, var(--ui-accent) 32%, transparent);
        box-shadow:
            0 0 0 1px color-mix(in srgb, var(--ui-accent) 14%, transparent),
            inset 0 0 0 1px color-mix(in srgb, var(--ui-accent) 8%, transparent);
    }

    .pane-shell.dragging {
        opacity: 0.92;
    }

    .pane-shell.swap-target {
        opacity: 0.92;
        border-color: rgba(122, 162, 247, 0.88);
        background: rgba(122, 162, 247, 0.06);
        box-shadow:
            0 0 0 2px rgba(122, 162, 247, 0.44),
            inset 0 0 0 1px rgba(122, 162, 247, 0.22);
        transition:
            border-color 80ms ease,
            box-shadow 80ms ease,
            background 80ms ease;
    }

    .pane-shell.swap-target .drag-overlay {
        border: 1px dashed rgba(122, 162, 247, 0.6);
        background: rgba(12, 16, 28, 0.72);
        backdrop-filter: blur(6px);
    }

    .pane-shell.swap-target .pane-header {
        background: rgba(122, 162, 247, 0.22);
    }

    .drag-overlay {
        position: absolute;
        inset: 8px;
        z-index: 50;
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        gap: 8px;
        border-radius: 12px;
        pointer-events: none;
        text-align: center;
        backdrop-filter: blur(4px);
    }

    .pane-shell.swap-target .drag-overlay .drag-chip {
        border-color: rgba(122, 162, 247, 0.72);
        background: rgba(122, 162, 247, 0.18);
        color: rgba(180, 200, 255, 1);
    }

    .pane-shell.swap-target .drag-overlay p {
        color: rgba(160, 185, 255, 0.8);
    }

    .drag-origin {
        border: 1px dashed rgba(122, 162, 247, 0.34);
        background: rgba(12, 16, 28, 0.36);
    }

    .drag-chip {
        padding: 6px 10px;
        border: 1px solid rgba(122, 162, 247, 0.28);
        border-radius: 999px;
        background: rgba(14, 18, 31, 0.9);
        color: var(--ui-text-primary);
        font-size: 0.76rem;
        font-weight: 600;
    }

    .drag-overlay p {
        margin: 0;
        color: var(--ui-text-secondary);
        font-size: 0.78rem;
        max-width: 28ch;
    }

    /* ---- Pane header ---- */

    .pane-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        padding: 6px 8px 6px 10px;
        border-bottom: 1px solid var(--ui-border-soft);
        background: color-mix(in srgb, var(--ui-layer-1) 80%, transparent 20%);
        flex: 0 0 auto;
        min-height: 34px;
        cursor: grab;
        transition: background var(--ui-motion-fast);
    }

    .pane-header:active {
        cursor: grabbing;
    }

    .pane-shell.active .pane-header {
        background: color-mix(in srgb, var(--ui-accent) 5%, var(--ui-layer-1) 95%);
    }

    .pane-title-block {
        display: flex;
        align-items: baseline;
        gap: 8px;
        min-width: 0;
        flex: 1;
        user-select: none;
    }

    :global(.codemux-pane-drop-target) {
        outline: none;
        background: transparent;
    }

    .pane-title {
        font-size: 0.8rem;
        font-weight: 600;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .pane-subtitle {
        font-size: 0.7rem;
        color: var(--ui-text-muted);
        white-space: nowrap;
    }

    /* ---- Pane icon actions ---- */

    .pane-actions {
        display: flex;
        align-items: center;
        gap: 2px;
        opacity: 0;
        pointer-events: none;
        transition:
            opacity var(--ui-motion-fast),
            transform var(--ui-motion-fast);
        transform: translateX(4px);
        flex-shrink: 0;
    }

    .pane-shell:hover .pane-actions,
    .pane-shell.active .pane-actions,
    .pane-actions.always-visible {
        opacity: 1;
        pointer-events: auto;
        transform: translateX(0);
    }

    .pane-icon-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 24px;
        height: 24px;
        border: 1px solid transparent;
        border-radius: 5px;
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast),
            border-color var(--ui-motion-fast);
        padding: 0;
        font: inherit;
    }

    .pane-icon-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-primary);
        border-color: var(--ui-border-soft);
    }

    .pane-icon-btn.close:hover {
        background: color-mix(in srgb, var(--ui-danger) 12%, transparent);
        color: var(--ui-danger);
        border-color: color-mix(in srgb, var(--ui-danger) 22%, transparent);
    }

    /* ---- Pane content ---- */

    .pane-content {
        position: relative;
        flex: 1;
        width: 100%;
        height: 100%;
        min-height: 0;
        overflow: hidden;
        cursor: text;
        background: var(--ui-layer-0);
    }

    .browser-content {
        cursor: default;
    }

    /* ---- Resize handle ---- */

    .split-handle {
        position: absolute;
        z-index: 20;
        border: 0;
        border-radius: 4px;
        background: transparent;
        transition: background var(--ui-motion-fast);
        padding: 0;
        cursor: col-resize;
    }

    .split-pane.horizontal > .split-child > .split-handle {
        width: 10px;
        height: calc(100% - 8px);
        top: 4px;
        cursor: col-resize;
    }

    .split-pane.vertical > .split-child > .split-handle {
        width: calc(100% - 8px);
        height: 10px;
        left: 4px;
        cursor: row-resize;
    }

    .split-handle:hover,
    .split-handle.dragging {
        background: color-mix(in srgb, var(--ui-accent) 42%, transparent);
    }
</style>
