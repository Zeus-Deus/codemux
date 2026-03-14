<script lang="ts">
    import AgentNode from './AgentNode.svelte';
    import AgentEdge from './AgentEdge.svelte';

    export interface AgentNodeData {
        id: string;
        role: string;
        status: string;
        model?: string | null;
        thinkingMode?: string | null;
    }

    export interface Connection {
        from: string;
        to: string;
        label?: string;
    }

    interface Props {
        nodes: AgentNodeData[];
        activeConnections?: Connection[];
    }

    let { nodes, activeConnections = [] }: Props = $props();

    interface PositionedNode extends AgentNodeData {
        x: number;
        y: number;
    }

    const nodeWidth = 160;
    const nodeHeight = 70;
    const horizontalGap = 40;
    const verticalGap = 60;
    const padding = 40;

    const positionedNodes = $derived.by(() => {
        if (nodes.length === 0) return [];

        const orchestrator = nodes.find(n => n.role === 'orchestrator');
        const others = nodes.filter(n => n.role !== 'orchestrator');

        const result: PositionedNode[] = [];
        
        // Define hierarchy levels based on typical workflow
        const levels: AgentNodeData[][] = [];
        
        if (orchestrator) {
            levels.push([orchestrator]);
        }
        
        // Level 2: Planners and Researchers
        const planners = others.filter(n => n.role === 'planner' || n.role === 'researcher');
        if (planners.length > 0) levels.push(planners);
        
        // Level 3: Builders
        const builders = others.filter(n => n.role === 'builder');
        if (builders.length > 0) levels.push(builders);
        
        // Level 4: Testers, Reviewers, Debuggers
        const qa = others.filter(n => n.role === 'tester' || n.role === 'reviewer' || n.role === 'debugger');
        if (qa.length > 0) levels.push(qa);
        
        // Any other unrecognized roles go to the last level
        const remaining = others.filter(n => 
            n.role !== 'planner' && n.role !== 'researcher' && 
            n.role !== 'builder' && n.role !== 'tester' && 
            n.role !== 'reviewer' && n.role !== 'debugger'
        );
        if (remaining.length > 0) {
            if (levels.length === 0) levels.push(remaining);
            else levels[levels.length - 1].push(...remaining);
        }

        // Calculate max width required
        let maxRowWidth = 0;
        levels.forEach(row => {
            const rowWidth = row.length * nodeWidth + Math.max(0, row.length - 1) * horizontalGap;
            if (rowWidth > maxRowWidth) maxRowWidth = rowWidth;
        });

        // Position each level
        levels.forEach((row, rowIndex) => {
            const rowWidth = row.length * nodeWidth + Math.max(0, row.length - 1) * horizontalGap;
            const startX = padding + (maxRowWidth - rowWidth) / 2;
            const y = padding + rowIndex * (nodeHeight + verticalGap);
            
            row.forEach((node, colIndex) => {
                result.push({
                    ...node,
                    x: startX + colIndex * (nodeWidth + horizontalGap),
                    y
                });
            });
        });

        return result;
    });

    function getNodePosition(id: string): { x: number; y: number } | null {
        const node = positionedNodes.find(n => n.id === id);
        return node ? { x: node.x, y: node.y } : null;
    }

    function isConnectionActive(from: string, to: string): boolean {
        return activeConnections.some(c => c.from === from && c.to === to);
    }

    function getConnectionLabel(from: string, to: string): string {
        const conn = activeConnections.find(c => c.from === from && c.to === to);
        return conn?.label || '';
    }

    let graphWidth = $derived.by(() => {
        if (positionedNodes.length === 0) return 0;
        const maxRight = Math.max(...positionedNodes.map(n => n.x + nodeWidth));
        return maxRight + padding;
    });

    let graphHeight = $derived.by(() => {
        if (positionedNodes.length === 0) return 0;
        const maxBottom = Math.max(...positionedNodes.map(n => n.y + nodeHeight));
        return maxBottom + padding;
    });

    let containerWidth = $state(800);
    let containerHeight = $state(400);

    let containerEl: HTMLDivElement;

    $effect(() => {
        if (containerEl) {
            containerWidth = Math.max(containerEl.clientWidth || 800, graphWidth);
            containerHeight = Math.max(containerEl.clientHeight || 400, graphHeight, 300);
        }
    });

    function handleNodeDrag(id: string, x: number, y: number) {
        console.log('Node dragged:', id, x, y);
    }
</script>

<div class="node-graph-container" bind:this={containerEl}>
    <div class="graph-scroll-area" style="width: {containerWidth}px; height: {containerHeight}px;">
        <svg 
            class="edges-layer"
            width={containerWidth}
            height={containerHeight}
        >
        {#each positionedNodes as node}
            {#each activeConnections.filter(c => c.from === node.id) as conn}
                {@const targetPos = getNodePosition(conn.to)}
                {#if targetPos}
                    <AgentEdge
                        sourceX={node.x}
                        sourceY={node.y}
                        targetX={targetPos.x}
                        targetY={targetPos.y}
                        isActive={isConnectionActive(node.id, conn.to)}
                        label={getConnectionLabel(node.id, conn.to)}
                    />
                {/if}
            {/each}
        {/each}
    </svg>
    
    <div class="nodes-layer">
        {#each positionedNodes as node (node.id)}
            <AgentNode
                id={node.id}
                role={node.role}
                status={node.status}
                model={node.model}
                thinkingMode={node.thinkingMode}
                isActive={activeConnections.some(c => c.from === node.id || c.to === node.id)}
                x={node.x}
                y={node.y}
                ondrag={(x, y) => handleNodeDrag(node.id, x, y)}
            />
        {/each}
    </div>

    {#if positionedNodes.length === 0}
        <div class="empty-state">
            <p>No agents configured</p>
        </div>
    {/if}
    </div>
</div>

<style>
    .node-graph-container {
        position: relative;
        width: 100%;
        min-height: 300px;
        height: 100%;
        background: var(--ui-layer-2, #1f1f1f);
        border-radius: 12px;
        border: 1px solid var(--ui-border-soft, #333);
        overflow: auto;
    }

    .graph-scroll-area {
        position: relative;
        min-width: 100%;
        min-height: 100%;
    }

    .edges-layer {
        position: absolute;
        top: 0;
        left: 0;
        z-index: 0;
        pointer-events: none;
    }

    .nodes-layer {
        position: relative;
        z-index: 1;
        width: 100%;
        height: 100%;
    }

    .empty-state {
        position: absolute;
        top: 50%;
        left: 50%;
        transform: translate(-50%, -50%);
        color: var(--ui-text-muted, #888);
        font-size: 0.9rem;
    }
</style>
