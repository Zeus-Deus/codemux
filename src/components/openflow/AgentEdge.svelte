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
        // Connect bottom-center of source to top-center of target (vertical layout)
        const startX = sourceX + nodeWidth / 2;
        const startY = sourceY + nodeHeight;
        const endX = targetX + nodeWidth / 2;
        const endY = targetY;

        const dy = endY - startY;

        if (dy >= 0) {
            // Normal downward flow: smooth vertical bezier
            const controlOffset = Math.max(dy * 0.4, 20);
            return `M ${startX} ${startY} C ${startX} ${startY + controlOffset}, ${endX} ${endY - controlOffset}, ${endX} ${endY}`;
        } else {
            // Feedback loop going upward: route around the right side so it doesn't overlap nodes
            const sideOffset = 90;
            const verticalPad = 30;
            return `M ${startX} ${startY} C ${startX + sideOffset} ${startY + verticalPad}, ${endX + sideOffset} ${endY - verticalPad}, ${endX} ${endY}`;
        }
    });

    const midpoint = $derived.by(() => {
        return {
            x: (sourceX + nodeWidth / 2 + targetX + nodeWidth / 2) / 2,
            y: (sourceY + nodeHeight + targetY) / 2
        };
    });

    // Use derived IDs so marker references always match the rendered <marker> elements
    const markerId = $derived(`arrow-${sourceX}-${sourceY}-${targetX}-${targetY}`);
    const markerActiveId = $derived(`arrow-active-${sourceX}-${sourceY}-${targetX}-${targetY}`);
</script>

<g class="agent-edge" class:active={isActive}>
    <defs>
        <marker
            id={markerId}
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
            id={markerActiveId}
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
        marker-end={isActive ? undefined : `url(#${markerId})`}
    />

    {#if isActive}
        <path
            class="edge-path-active"
            d={path}
            marker-end={`url(#${markerActiveId})`}
        />
    {/if}

    {#if label}
        <text
            class="edge-label"
            x={midpoint.x}
            y={midpoint.y - 6}
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
        stroke-width: 1.5;
        transition: stroke 0.3s ease, opacity 0.3s ease;
    }

    .edge-path-bg.dashed {
        stroke-dasharray: 4 6;
        opacity: 0.5;
    }

    .edge-path-active {
        fill: none;
        stroke: var(--ui-accent, #6b8aff);
        stroke-width: 2.5;
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
