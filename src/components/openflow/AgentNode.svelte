<script lang="ts">
    import { onDestroy } from 'svelte';
    import type { Snippet } from 'svelte';

    interface Props {
        id: string;
        role: string;
        status: string;
        model?: string | null;
        thinkingMode?: string | null;
        isActive?: boolean;
        x?: number;
        y?: number;
        ondrag?: (x: number, y: number) => void;
    }

    let { 
        id, 
        role, 
        status, 
        model = null, 
        thinkingMode = null, 
        isActive = false,
        x = 0,
        y = 0,
        ondrag
    }: Props = $props();

    let isDragging = $state(false);
    let dragStartX = $state(0);
    let dragStartY = $state(0);
    let nodeStartX = $state(0);
    let nodeStartY = $state(0);

    const roleIcons: Record<string, string> = {
        orchestrator: '⚙️',
        planner: '📋',
        builder: '🔨',
        reviewer: '👀',
        tester: '🧪',
        debugger: '🔧',
        researcher: '🔍'
    };

    function getStatusColor(s: string): string {
        if (s === 'done' || s === 'passed') return 'var(--ui-success)';
        if (s === 'active' || s === 'ready') return 'var(--ui-accent)';
        if (s === 'pending') return 'var(--ui-text-muted)';
        if (s === 'blocked') return 'var(--ui-danger)';
        return 'var(--ui-text-muted)';
    }

    function shortenModel(modelId: string): string {
        const parts = modelId.split('/');
        return parts.length > 1 ? parts[parts.length - 1] : modelId;
    }

    function handleMouseDown(e: MouseEvent) {
        if (!ondrag) return;
        isDragging = true;
        dragStartX = e.clientX;
        dragStartY = e.clientY;
        nodeStartX = x;
        nodeStartY = y;
        window.addEventListener('mousemove', handleMouseMove);
        window.addEventListener('mouseup', handleMouseUp);
    }

    function handleMouseMove(e: MouseEvent) {
        if (!isDragging || !ondrag) return;
        const dx = e.clientX - dragStartX;
        const dy = e.clientY - dragStartY;
        ondrag(nodeStartX + dx, nodeStartY + dy);
    }

    function handleMouseUp() {
        isDragging = false;
        window.removeEventListener('mousemove', handleMouseMove);
        window.removeEventListener('mouseup', handleMouseUp);
    }

    // Guarantee cleanup if the component is destroyed while a drag is in progress.
    onDestroy(() => {
        window.removeEventListener('mousemove', handleMouseMove);
        window.removeEventListener('mouseup', handleMouseUp);
    });
</script>

<div 
    class="agent-node" 
    class:active={isActive}
    class:dragging={isDragging}
    style="
        --status-color: {getStatusColor(status)};
        left: {x}px;
        top: {y}px;
    "
    role="button"
    tabindex="0"
    aria-label="{role} agent, {status} status"
    onmousedown={handleMouseDown}
>
    <div class="node-icon">{roleIcons[role] || '🤖'}</div>
    <div class="node-info">
        <span class="node-role">{role}</span>
        <span class="node-status" style="color: {getStatusColor(status)}">{status}</span>
        {#if model}
            <span class="node-model">
                {shortenModel(model)}
                {#if thinkingMode && thinkingMode !== 'auto'}
                    ({thinkingMode})
                {/if}
            </span>
        {/if}
    </div>
    {#if isActive}
        <div class="active-indicator"></div>
    {/if}
</div>

<style>
    .agent-node {
        position: absolute;
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 14px 18px;
        background: var(--ui-layer-1, #1a1a1a);
        border: 2px solid var(--status-color, var(--ui-border-soft));
        border-radius: 10px;
        min-width: 140px;
        cursor: grab;
        transition: transform 0.15s ease, box-shadow 0.15s ease, border-color 0.15s ease;
        user-select: none;
        z-index: 1;
    }

    .agent-node:hover {
        transform: scale(1.02);
        box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    }

    .agent-node.dragging {
        cursor: grabbing;
        transform: scale(1.05);
        box-shadow: 0 8px 30px rgba(0, 0, 0, 0.4);
        z-index: 10;
    }

    .agent-node.active {
        border-color: var(--ui-accent);
        box-shadow: 0 0 20px color-mix(in srgb, var(--ui-accent) 40%, transparent);
        animation: pulse 2s ease-in-out infinite;
    }

    @keyframes pulse {
        0%, 100% { box-shadow: 0 0 15px color-mix(in srgb, var(--ui-accent) 30%, transparent); }
        50% { box-shadow: 0 0 25px color-mix(in srgb, var(--ui-accent) 50%, transparent); }
    }

    .node-icon {
        font-size: 1.5rem;
        flex-shrink: 0;
    }

    .node-info {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .node-role {
        font-weight: 600;
        font-size: 0.9rem;
        color: var(--ui-text-primary, #e0e0e0);
        text-transform: capitalize;
    }

    .node-status {
        font-size: 0.75rem;
        text-transform: capitalize;
        font-weight: 500;
    }

    .node-model {
        font-size: 0.7rem;
        color: var(--ui-accent, #6b8aff);
        font-weight: 500;
    }

    .active-indicator {
        position: absolute;
        top: -4px;
        right: -4px;
        width: 10px;
        height: 10px;
        background: var(--ui-accent);
        border-radius: 50%;
        animation: blink 1s ease-in-out infinite;
    }

    @keyframes blink {
        0%, 100% { opacity: 1; transform: scale(1); }
        50% { opacity: 0.5; transform: scale(0.8); }
    }
</style>
