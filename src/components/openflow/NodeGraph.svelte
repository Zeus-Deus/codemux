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
        levelIndex: number;
    }

    const nodeWidth = 160;
    const nodeHeight = 70;
    const horizontalGap = 40;
    const verticalGap = 60;
    const padding = 40;

    const positionedNodes = $derived.by(() => {
        if (nodes.length === 0) return [];

        const result: PositionedNode[] = [];
        
        // Strict role hierarchy - each role gets its own row, in workflow order:
        // Orchestrator → Researcher → Planner → Builder → Tester → Debugger → Reviewer
        const roleOrder = [
            'orchestrator',
            'researcher',
            'planner',
            'builder',
            'tester',
            'debugger',
            'reviewer',
        ];

        // Build levels using only roles that are actually present, preserving order
        const levels: AgentNodeData[][] = [];
        roleOrder.forEach(role => {
            const roleNodes = nodes.filter(n => n.role === role);
            if (roleNodes.length > 0) levels.push(roleNodes);
        });

        // Any unrecognized roles go on their own row at the bottom
        const knownRoles = new Set(roleOrder);
        const unknown = nodes.filter(n => !knownRoles.has(n.role));
        if (unknown.length > 0) levels.push(unknown);

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
                    y,
                    levelIndex: rowIndex
                });
            });
        });

        return result;
    });

    const structuralConnections = $derived.by(() => {
        if (positionedNodes.length < 2) return [];
        
        const conns: Connection[] = [];
        
        // Group by levelIndex (most reliable, avoids float comparison issues)
        const levelMap = new Map<number, PositionedNode[]>();
        positionedNodes.forEach(node => {
            if (!levelMap.has(node.levelIndex)) levelMap.set(node.levelIndex, []);
            levelMap.get(node.levelIndex)!.push(node);
        });

        const levelKeys = Array.from(levelMap.keys()).sort((a, b) => a - b);
        const numLevels = levelKeys.length;

        // Connect each level ONLY to the immediately next level (no skip connections)
        for (let i = 0; i < numLevels - 1; i++) {
            const currentLevel = levelMap.get(levelKeys[i])!;
            const nextLevel = levelMap.get(levelKeys[i + 1])!;
            
            currentLevel.forEach(fromNode => {
                nextLevel.forEach(toNode => {
                    conns.push({ from: fromNode.id, to: toNode.id });
                });
            });
        }
        
        // Connect the last level back to Orchestrator only (feedback loop)
        const lastLevel = levelMap.get(levelKeys[numLevels - 1])!;
        const firstLevel = levelMap.get(levelKeys[0])!;
        
        // Only add feedback if it's not already covered (e.g. 2-level graph)
        if (numLevels > 1) {
            lastLevel.forEach(lastNode => {
                firstLevel.forEach(orchNode => {
                    conns.push({ from: lastNode.id, to: orchNode.id });
                });
            });
        }

        return conns;
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

    // Calculate offsets to center the graph in the container if it's smaller
    const offsetX = $derived(Math.max(0, (containerWidth - graphWidth) / 2));
    const offsetY = $derived(Math.max(0, (containerHeight - graphHeight) / 2));

    function handleNodeDrag(id: string, x: number, y: number) {
        console.log('Node dragged:', id, x, y);
    }
</script>

<div class="node-graph-container" bind:this={containerEl}>
    <div class="graph-scroll-area" style="width: {containerWidth}px; height: {containerHeight}px;">
        <div class="graph-content" style="transform: translate({offsetX}px, {offsetY}px)">
            <svg 
                class="edges-layer"
                width={graphWidth}
                height={graphHeight}
            >
            {#each structuralConnections as conn}
                {@const sourcePos = getNodePosition(conn.from)}
                {@const targetPos = getNodePosition(conn.to)}
                {#if sourcePos && targetPos}
                    <AgentEdge
                        sourceX={sourcePos.x}
                        sourceY={sourcePos.y}
                        targetX={targetPos.x}
                        targetY={targetPos.y}
                        isActive={isConnectionActive(conn.from, conn.to)}
                        label={getConnectionLabel(conn.from, conn.to)}
                    />
                {/if}
            {/each}
            {#each activeConnections as conn}
                <!-- Render any active connections that might not be in the default structural connections -->
                {#if !structuralConnections.some(sc => sc.from === conn.from && sc.to === conn.to)}
                    {@const sourcePos = getNodePosition(conn.from)}
                    {@const targetPos = getNodePosition(conn.to)}
                    {#if sourcePos && targetPos}
                        <AgentEdge
                            sourceX={sourcePos.x}
                            sourceY={sourcePos.y}
                            targetX={targetPos.x}
                            targetY={targetPos.y}
                            isActive={true}
                            label={getConnectionLabel(conn.from, conn.to)}
                        />
                    {/if}
                {/if}
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
        background: var(--ui-layer-1, #161616);
        border-radius: 12px;
        border: 1px solid var(--ui-border-soft, #333);
        overflow: auto;
    }

    .graph-scroll-area {
        position: relative;
        min-width: 100%;
        min-height: 100%;
        /* Subtle dot grid background for the canvas */
        background-image: radial-gradient(var(--ui-border-soft) 1px, transparent 1px);
        background-size: 24px 24px;
        background-position: center;
    }

    .graph-content {
        position: absolute;
        top: 0;
        left: 0;
        width: 100%;
        height: 100%;
        transition: transform 0.3s ease-out;
    }

    .edges-layer {
        position: absolute;
        top: 0;
        left: 0;
        z-index: 0;
        pointer-events: none;
        overflow: visible;
    }

    .nodes-layer {
        position: absolute;
        top: 0;
        left: 0;
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
