import { usePluginsStore } from "./plugins-store";

/** `<pluginId>:<localId>` → `pluginId`. Core ids have no prefix. */
export function pluginIdOf(contributionId: string): string {
  const i = contributionId.indexOf(":");
  return i < 0 ? contributionId : contributionId.slice(0, i);
}

/** The plugin's display name, falling back to its id. */
export function pluginLabel(pluginId: string): string {
  return usePluginsStore.getState().records[pluginId]?.name ?? pluginId;
}
