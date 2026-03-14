# Visual Node Graph Implementation Plan

## Current State

The current `OrchestrationView.svelte` uses a simple flexbox layout with card-style agent nodes:
- Nodes are rendered as simple `div` elements in a flex wrap container
- No actual graph positioning or connections
- No animations
- No highlighting of active communications
- No `AgentNode.svelte` or `AgentEdge.svelte` components exist

---

## Implementation Options

### Option A: Custom SVG + HTML (Recommended)
Build a lightweight custom node graph using:
- SVG layer for edges (connection lines)
- HTML/CSS for agent nodes (easier styling, accessibility)
- Simple force-directed or hierarchical layout

**Pros:** No new dependencies, full control, lightweight  
**Cons:** More custom code to write

### Option B: Svelte Flow
Use `@xyflow/svelte` (formerly Svelte Flow) library

**Pros:** Full-featured, handles drag/zoom/pan, professional look  
**Cons:** New dependency, may be overkill for simple needs

### Option C: D3.js
Use D3 for graph layout and rendering

**Pros:** Powerful graph algorithms  
**Cons:** Complex, steep learning curve, heavy

---

## Recommended Approach: Option A (Custom SVG + HTML)

This keeps dependencies minimal and gives us full control. We can always upgrade to a library later if needed.

---

## Implementation Steps

### Step 1: Create AgentNode.svelte
```
src/components/openflow/AgentNode.svelte
```

**Purpose:** Reusable component for displaying an agent in the graph

**Props:**
- `id`: string - unique identifier
- `role`: string - agent role (orchestrator, builder, etc.)
- `status`: string - current status (pending, active, done, blocked)
- `model`: string | null - model name
- `thinkingMode`: string | null - thinking mode
- `isActive`: boolean - is currently communicating
- `x`, `y`: number - position coordinates

**Features:**
- Role icon + name
- Status indicator (colored border/glow)
- Model + thinking mode display
- Active state glow animation
- Hover state with tooltip

**Styling:**
- Use existing CSS variables from the codebase
- Status colors: success (done), accent (active), muted (pending), danger (blocked)
- Smooth transitions (0.2s ease)

---

### Step 2: Create AgentEdge.svelte
```
src/components/openflow/AgentEdge.svelte
```

**Purpose:** SVG connection line between two agents

**Props:**
- `sourceId`: string - source node ID
- `targetId`: string - target node ID
- `sourceX`, `sourceY`: number - source position
- `targetX`, `targetY`: number - target position
- `isActive`: boolean - show active communication highlight

**Features:**
- Curved bezier path (smooth S-curve)
- Animated dash pattern when active (flowing effect)
- Arrow marker at target
- Different colors for different communication types

**Animation:**
- CSS keyframes for flowing dashes
- Color pulse when actively communicating

---

### Step 3: Create NodeGraph.svelte
```
src/components/openflow/NodeGraph.svelte
```

**Purpose:** Main graph container that orchestrates nodes and edges

**Props:**
- `nodes`: AgentNode[] - array of agent data
- `activeConnections`: { from: string, to: string }[] - active communications

**Features:**
- SVG layer for edges (z-index: 0)
- HTML layer for nodes (z-index: 1)
- Simple grid/hierarchical layout algorithm
- Auto-layout (orchestrator at top, workers below)
- Manual repositioning (drag nodes)
- Zoom/pan (optional, can use CSS transform)

**Layout Algorithm:**
```
1. Place orchestrator at top center
2. Place other agents in rows below
3. Distribute evenly across width
4. Connect based on role relationships:
   - orchestrator → all workers
   - reviewer ← builder (after build)
   - tester ← builder (after build)
```

---

### Step 4: Update OrchestrationView.svelte

**Changes:**
1. Import `NodeGraph` component
2. Replace `.node-graph` div with `<NodeGraph />`
3. Pass agent data and active connections
4. Extract current phase to highlight active communications

**Data Flow:**
```typescript
// Get active communications from run state
const activeConnections = $derived(
    run?.current_phase === 'execute' 
        ? [{ from: 'orchestrator', to: 'builder' }]
        : run?.current_phase === 'review'
        ? [{ from: 'builder', to: 'reviewer' }]
        : []
);
```

---

### Step 5: Add Animations

**Node Animations:**
- Fade in on mount (staggered)
- Pulse glow when status changes to active
- Scale up slightly on hover
- Smooth position transitions when layout changes

**Edge Animations:**
- Flowing dash animation (CSS `stroke-dasharray` + `stroke-dashoffset`)
- Color intensity based on communication frequency

---

## File Structure After Implementation

```
src/components/openflow/
├── AgentNode.svelte      # NEW - Individual node component
├── AgentEdge.svelte      # NEW - Connection line component
├── NodeGraph.svelte      # NEW - Graph container + layout
├── OrchestrationView.svelte  # UPDATED - Use NodeGraph
├── AgentConfigPanel.svelte
└── CommunicationPanel.svelte
```

---

## Key Design Decisions

### 1. Coordinate System
- Use percentage-based or fixed pixel coordinates
- Start with fixed layout, add drag later if needed
- Container size: 100% width, min-height 400px

### 2. Layout Strategy
- **Initial:** Hierarchical (orchestrator top, workers below)
- **Future:** Force-directed for organic look

### 3. Edge Routing
- Simple bezier curves: `M x1 y1 C cx1 cy1 cx2 cy2 x2 y2`
- Control points calculated from source/target positions

### 4. Active Communication Detection
- Track recent messages in communication log
- If agent A sent message to agent B in last 5 seconds → highlight edge

---

## Implementation Order

1. ✅ Plan complete
2. ⏳ Create `AgentNode.svelte`
3. ⏳ Create `AgentEdge.svelte` 
4. ⏳ Create `NodeGraph.svelte`
5. ⏳ Update `OrchestrationView.svelte`
6. ⏳ Add animations
7. ⏳ Test and refine

---

## Notes

- Keep it simple first - MVP with basic layout and connections
- Use existing CSS variables from codebase
- Ensure accessibility (keyboard navigation, screen reader labels)
- Consider performance for 20+ agent nodes
- Add zoom/pan only if needed (can use CSS transform)

---

## Dependencies

No new dependencies required for MVP. If we need more features later:
- `@xyflow/svelte` - for professional node graph (Svelte Flow)
- `d3-shape` - for advanced edge routing
