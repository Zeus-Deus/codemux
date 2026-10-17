/**
 * Bundled plugins: `src/plugins/<id>/`, built with the app, always Official.
 *
 * `meta.ts` is read eagerly (id, name, description, defaultEnabled) so the
 * plugin can show in Settings. `plugin.ts(x)` is a separate lazy chunk that
 * is imported only when the plugin is on: a disabled bundled plugin's code is
 * never fetched or evaluated.
 */
import { activatePlugin, markLoading, reportPluginError, unloadPlugin } from "./activation";
import type { CodemuxPlugin } from "./plugin";
import {
  patchPlugin,
  pluginActive,
  publishPlugin,
  usePluginsStore,
  type PluginRecord,
} from "./plugins-store";

export interface BundledPluginMeta {
  id: string;
  name: string;
  description?: string;
  version?: string;
  /** Default false: a bundled plugin ships off unless it says otherwise. */
  defaultEnabled?: boolean;
}

type Loader = () => Promise<{ default: CodemuxPlugin }>;

const metas = import.meta.glob<{ default: BundledPluginMeta }>("../plugins/*/meta.ts", {
  eager: true,
});
const loaders = import.meta.glob<{ default: CodemuxPlugin }>("../plugins/*/plugin.{ts,tsx}");

function folderOf(path: string): string {
  return path.split("/").slice(-2, -1)[0] ?? "";
}

export interface BundledEntry {
  meta: BundledPluginMeta;
  load: Loader;
}

/** Discovery is pure data; nothing is imported here. */
export function bundledEntries(
  metaModules: Record<string, { default: BundledPluginMeta }> = metas,
  loaderModules: Record<string, Loader> = loaders,
): BundledEntry[] {
  const byFolder = new Map<string, Loader>();
  for (const [path, load] of Object.entries(loaderModules)) byFolder.set(folderOf(path), load);
  const out: BundledEntry[] = [];
  for (const [path, mod] of Object.entries(metaModules)) {
    const folder = folderOf(path);
    const meta = mod.default;
    const load = byFolder.get(folder);
    if (!meta?.id || meta.id !== folder || !load) {
      console.warn(`[plugins] ${path}: meta id must equal the folder and plugin.ts(x) must exist; skipped`);
      continue;
    }
    out.push({ meta, load });
  }
  return out.sort((a, b) => a.meta.id.localeCompare(b.meta.id));
}

function recordOf(meta: BundledPluginMeta): Omit<PluginRecord, "status"> {
  return {
    id: meta.id,
    name: meta.name,
    description: meta.description,
    version: meta.version,
    kind: "bundled",
    tier: "official",
    defaultEnabled: meta.defaultEnabled ?? false,
  };
}

async function activateBundled(entry: BundledEntry): Promise<void> {
  const record = recordOf(entry.meta);
  markLoading(entry.meta.id);
  try {
    const mod = await entry.load();
    const plugin = mod.default;
    if (!plugin || plugin.id !== entry.meta.id || typeof plugin.register !== "function") {
      throw new Error(`src/plugins/${entry.meta.id}/plugin has no valid default export`);
    }
    // Switched off while the chunk was loading.
    if (!pluginActive(entry.meta.id, entry.meta.defaultEnabled ?? false)) {
      patchPlugin(entry.meta.id, { status: "disabled" });
      return;
    }
    activatePlugin(plugin, record);
  } catch (error) {
    unloadPlugin(entry.meta.id);
    reportPluginError(record.name, "failed to load", error);
    publishPlugin({ ...record, status: "error", error: error instanceof Error ? error.message : String(error) });
  }
}

/** Publish every bundled plugin's row and import only the enabled ones. */
export async function adoptBundledPlugins(entries = bundledEntries()): Promise<void> {
  const jobs: Promise<void>[] = [];
  for (const entry of entries) {
    publishPlugin(
      { ...recordOf(entry.meta), status: "disabled" },
      {
        activate: () => activateBundled(entry),
        deactivate: () => unloadPlugin(entry.meta.id),
      },
    );
    if (usePluginsStore.getState().disabledByEnv) continue;
    if (pluginActive(entry.meta.id, entry.meta.defaultEnabled ?? false)) {
      jobs.push(activateBundled(entry));
    }
  }
  await Promise.all(jobs);
}
