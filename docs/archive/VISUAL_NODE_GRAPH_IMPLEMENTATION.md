# Visual Node Graph Design Note (Archived)

- Purpose: Preserve the earlier design direction for the OpenFlow visual node graph.
- Audience: Anyone revisiting the node-graph UI idea later.
- Authority: Archived design note only; not current plan.
- Update when: The note is revived, superseded again, or no longer useful.
- Read next: `docs/features/openflow.md`, `docs/plans/openflow.md`

## Status

This design note is archived. The repo now uses feature docs plus active plan docs, so this file is kept only as historical context.

## Archived Direction

The earlier recommendation was to build the node graph with custom SVG edges plus HTML and CSS nodes instead of adding a heavy graph dependency.

Key ideas:

- keep orchestrator at the top and workers below in a simple hierarchical layout
- render nodes as normal components for easier styling and accessibility
- render connections in SVG so active communication can be highlighted
- add drag, zoom, or richer layout logic only after the first useful version exists

## If This Work Is Revived

Treat this as a sketch, not a source of truth. Re-check the current OpenFlow UI, priorities, and component structure before implementing the graph.
