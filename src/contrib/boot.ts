/**
 * Plugin system start. Runs after the first render (never blocks it); a
 * loading plugin's areas simply appear once it registers.
 */
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { onPluginsChanged, pluginsRoot, pluginsWatch } from "./ipc";
import { onDiskChange, scanDiskPlugins } from "./runtime-loader";
import { usePluginsStore } from "./plugins-store";

let started = false;

/** The web remote client never loads plugins (desktop only). */
export function pluginsSupported(): boolean {
  return typeof window !== "undefined" && !isRemoteClient();
}

export async function startPlugins(): Promise<void> {
  if (started || !pluginsSupported()) return;
  started = true;
  let root;
  try {
    root = await pluginsRoot();
  } catch (error) {
    console.error("[plugins] plugin system unavailable", error);
    return;
  }
  usePluginsStore.setState({ rootPath: root.path, disabledByEnv: root.disabledByEnv });
  if (root.disabledByEnv) return;

  await onPluginsChanged((id) => void onDiskChange(id));
  try {
    await pluginsWatch();
  } catch (error) {
    // Unwatchable root: Reload plugins still rescans.
    console.warn("[plugins] cannot watch the plugins folder", error);
  }
  await scanDiskPlugins();
}

/** Settings → Plugins → Reload: rescan and re-read every enabled plugin. */
export function reloadPlugins(): Promise<void> {
  return scanDiskPlugins(true);
}
