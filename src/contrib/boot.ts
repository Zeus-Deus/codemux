/**
 * Plugin system start. Runs after the first render (never blocks it); a
 * loading plugin's areas simply appear once it registers.
 *
 * Order: decisions → safe-mode status → bundled plugins → disk plugins.
 * With every plugin off this reads two small native values, publishes
 * Settings rows, and imports no plugin code at all.
 */
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { adoptBundledPlugins } from "./bundled";
import {
  onPluginsChanged,
  pluginsGetDecisions,
  pluginsResume,
  pluginsRoot,
  pluginsSafeModeStatus,
  pluginsSetEnabled,
  pluginsStartupCheckpoint,
  pluginsWatch,
} from "./ipc";
import { onDiskChange, scanDiskPlugins } from "./runtime-loader";
import { setDecisionPersistence, usePluginsStore } from "./plugins-store";

/** How long the app must stay up for a launch to count as clean. */
const STARTUP_OK_MS = 10_000;

let started = false;
let watching = false;

/** The web remote client never loads plugins (desktop only). */
export function pluginsSupported(): boolean {
  return typeof window !== "undefined" && !isRemoteClient();
}

async function watchDisk(): Promise<void> {
  if (watching) return;
  watching = true;
  await onPluginsChanged((id) => void onDiskChange(id));
  try {
    await pluginsWatch();
  } catch (error) {
    // Unwatchable root: Reload plugins still rescans.
    console.warn("[plugins] cannot watch the plugins folder", error);
  }
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
  const [decisions, safe] = await Promise.all([
    pluginsGetDecisions().catch(() => ({}) as Record<string, boolean>),
    pluginsSafeModeStatus().catch(() => null),
  ]);
  usePluginsStore.setState({
    rootPath: root.path,
    disabledByEnv: root.disabledByEnv,
    decisions,
    decisionsLoaded: true,
    paused: safe?.paused ?? false,
    pausedReason: safe?.reason ?? null,
  });
  setDecisionPersistence(pluginsSetEnabled);

  // Kill switch: list rows so Settings can say why, load nothing.
  if (root.disabledByEnv) {
    await adoptBundledPlugins();
    return;
  }

  // Safe mode: a crash during this launch leaves the marker for the next one.
  await pluginsStartupCheckpoint("loading").catch(() => undefined);
  window.setTimeout(() => void pluginsStartupCheckpoint("ok").catch(() => undefined), STARTUP_OK_MS);

  await adoptBundledPlugins();
  await watchDisk();
  await scanDiskPlugins();
}

/** Settings → Plugins → Reload: rescan and re-read every enabled plugin. */
export function reloadPlugins(): Promise<void> {
  return scanDiskPlugins(true);
}

/** Safe-mode banner → Resume plugins: lift the pause and load disk plugins. */
export async function resumePlugins(): Promise<void> {
  await pluginsResume();
  usePluginsStore.setState({ paused: false, pausedReason: null });
  await scanDiskPlugins(true);
}
