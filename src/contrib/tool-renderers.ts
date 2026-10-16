/**
 * `transcript.toolRenderer`: a plugin can draw a tool call itself. The first
 * enabled contribution whose `data.tool` matches wins; otherwise the core
 * card renders. Read imperatively from the row renderer: with no plugin
 * renderer registered this is one Map lookup against an empty area.
 */
import { useSyncExternalStore, type ReactNode } from "react";
import { registry } from "./registry";
import { TOOL_RENDERER_AREA } from "@/sdk/areas";
import type { ToolCallView, ToolRendererData } from "@/sdk/host";

export interface PluginToolRenderer {
  id: string;
  render: (props: { call: ToolCallView }) => ReactNode;
}

let cachedFor: readonly unknown[] | null = null;
let byTool = new Map<string, PluginToolRenderer>();

export function findPluginToolRenderer(tool: string): PluginToolRenderer | null {
  const items = registry.getArea(TOOL_RENDERER_AREA);
  if (items.length === 0) return null;
  if (items !== cachedFor) {
    cachedFor = items;
    byTool = new Map();
    for (const c of items) {
      const data = c.data as ToolRendererData | undefined;
      if (!data || typeof data.tool !== "string" || typeof data.render !== "function") continue;
      if (!byTool.has(data.tool)) byTool.set(data.tool, { id: c.id, render: data.render });
    }
  }
  return byTool.get(tool) ?? null;
}

const subscribeRenderers = (fn: () => void) => registry.subscribeArea(TOOL_RENDERER_AREA, fn);
const snapshotRenderers = () => registry.getArea(TOOL_RENDERER_AREA);

/** Reactive lookup for one tool: re-renders when a renderer plugin turns on or off. */
export function usePluginToolRenderer(tool: string): PluginToolRenderer | null {
  useSyncExternalStore(subscribeRenderers, snapshotRenderers, snapshotRenderers);
  return findPluginToolRenderer(tool);
}
