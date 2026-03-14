<script lang="ts">
    interface Props {
        sourceX: number;
        sourceY: number;
        targetX: number;
        targetY: number;
        isActive?: boolean;
        label?: string;
    }

    let { 
        sourceX, 
        sourceY, 
        targetX, 
        targetY, 
        isActive = false,
        label = ''
    }: Props = $props();

    const nodeWidth = 160;
    const nodeHeight = 70;

    const path = $derived.by(() => {
        const startX = sourceX + nodeWidth;
        const startY = sourceY + nodeHeight / 2;
        const endX = targetX;
        const endY = targetY + nodeHeight / 2;

        const dx = endX - startX;
        const controlOffset = Math.min(Math.abs(dx) * 0.5, 100);

        return `M ${startX} ${startY} C ${startX + controlOffset} ${startY}, ${endX - controlOffset} ${endY}, ${endX} ${endY}`;
    });

    const midpoint = $derived.by(() => {
        return {
            x: (sourceX + nodeWidth + targetX) / 2,
            y: (sourceY + nodeHeight / 2 + targetY + nodeHeight / 2) / 2
        };
    });
</script>

<g class="agent-edge" class:active={isActive}>
    <defs>
        <marker
            id="arrow-{sourceX}-{sourceY}-{targetX}-{targetY}"
            viewBox="0 0 10 10"
            refX="9"
            refY="5"
            markerWidth="6"
            markerHeight="6"
            orient="auto-start-reverse"
        >
            <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--ui-border)" />
        </marker>
        <marker
            id="arrow-active-{sourceX}-{sourceY}-{targetX}-{targetY}"
            viewBox="0 0 10 10"
            refX="9"
            refY="5"
            markerWidth="6"
            markerHeight="6"
            orient="auto-start-reverse"
        >
            <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--ui-accent)" />
        </marker>
    </defs>
    
    <path
        class="edge-path-bg"
        class:dashed={!isActive}
        d={path}
        marker-end={isActive ? "" : "url(#arrow-{sourceX}-{sourceY}-{targetX}-{targetY})"}
    />
    
    {#if isActive}
        <path
            class="edge-path-active"
            d={path}
            marker-end="url(#arrow-active-{sourceX}-{sourceY}-{targetX}-{targetY})"
        />
    {/if}
    
    {#if label}
        <text
            class="edge-label"
            x={midpoint.x}
            y={midpoint.y}
        >
            {label}
        </text>
    {/if}
</g>

<style>
    .agent-edge {
        pointer-events: none;
    }

    .edge-path-bg {
        fill: none;
        stroke: var(--ui-border-soft, #3a3a3a);
        stroke-width: 2;
        transition: stroke 0.3s ease, opacity 0.3s ease;
    }

    .edge-path-bg.dashed {
        stroke-dasharray: 4 6;
        opacity: 0.4;
    }

    .edge-path-active {
        fill: none;
        stroke: var(--ui-accent, #6b8aff);
        stroke-width: 3;
        stroke-dasharray: 8 4;
        animation: flow 0.8s linear infinite;
        opacity: 0.9;
    }

    @keyframes flow {
        from {
            stroke-dashoffset: 24;
        }
        to {
            stroke-dashoffset: 0;
        }
    }

    .edge-label {
        fill: var(--ui-text-muted, #888);
        font-size: 11px;
        text-anchor: middle;
        dominant-baseline: middle;
    }
</style>
