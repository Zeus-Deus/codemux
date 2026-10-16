/**
 * Disk plugins: `<data>/codemux[-dev]/plugins/<id>/plugin.js`, plain ESM,
 * hot-reloaded (port of Hermes Desktop's runtime loader).
 *
 * SECURITY: this is not a sandbox. A loaded plugin runs in the renderer with
 * full app authority. Isolation here is error isolation only (boundaries,
 * try/catch around listeners). The import allowlist is the one runtime
 * tripwire: a plugin cannot pull a second stage from a URL.
 *
 * Off means off: a disabled disk plugin's file is never read or evaluated.
 */
import { getVersion } from "@tauri-apps/api/app";
import {
  activatePlugin,
  markLoading,
  reportPluginError,
  satisfiesMinVersion,
  unloadPlugin,
} from "./activation";
import {
  pluginsGetDecisions,
  pluginsList,
  pluginsReadSource,
  type DiskPluginEntry,
} from "./ipc";
import {
  dropPlugin,
  pluginActive,
  publishPlugin,
  replaceDecisions,
  usePluginsStore,
  type PluginRecord,
  type PluginTier,
} from "./plugins-store";

const SHADOW_SUFFIX = ":disk-shadowed";

/** Known disk folders → their last listing entry. */
const known = new Map<string, DiskPluginEntry>();
/** Per-folder serialization so a burst of saves loads in order. */
const queues = new Map<string, Promise<void>>();

let appVersion: string | null = null;
async function currentVersion(): Promise<string> {
  if (appVersion) return appVersion;
  try {
    appVersion = await getVersion();
  } catch {
    appVersion = "0.0.0";
  }
  return appVersion;
}

function tierOf(entry: DiskPluginEntry): PluginTier {
  if (!entry.marker) return "local";
  return entry.marker.tier === "official" ? "official" : "unverified";
}

/** Installed plugins (with a marker) start off; a Local folder starts on. */
function defaultEnabledOf(entry: DiskPluginEntry): boolean {
  return entry.marker == null;
}

function baseRecord(entry: DiskPluginEntry): Omit<PluginRecord, "status"> {
  return {
    id: entry.id,
    name: entry.marker?.name ?? entry.id,
    description: entry.marker?.description,
    kind: "disk",
    tier: tierOf(entry),
    defaultEnabled: defaultEnabledOf(entry),
    file: entry.file,
    origin: entry.marker
      ? {
          source: entry.marker.source,
          repo: entry.marker.repo,
          sha: entry.marker.sha,
          catalogId: entry.marker.catalogId,
        }
      : undefined,
  };
}

function isBundled(id: string): boolean {
  return usePluginsStore.getState().records[id]?.kind === "bundled";
}

/** Read, evaluate and register one disk plugin. Leaves an error row and
 *  unloads any previous incarnation on failure: a broken save never keeps
 *  stale code live under a "loaded" row. */
async function activateDisk(entry: DiskPluginEntry): Promise<void> {
  const record = baseRecord(entry);
  markLoading(entry.id);
  try {
    const source = await pluginsReadSource(entry.id);
    // The evaluator (and the SDK runtime shims it installs) is its own chunk:
    // nothing of it loads until an enabled disk plugin needs it.
    const { evaluatePluginSource } = await import("./evaluate");
    const plugin = await evaluatePluginSource(source, entry.id);
    if (plugin.id !== entry.id) {
      throw new Error(`plugin id "${plugin.id}" must equal its folder name "${entry.id}"`);
    }
    const version = await currentVersion();
    if (!satisfiesMinVersion(version, plugin.minCodemux)) {
      throw new Error(`needs Codemux ${plugin.minCodemux} or newer (this is ${version})`);
    }
    // The user may have switched it off while the source was loading.
    if (!pluginActive(entry.id, defaultEnabledOf(entry)) || usePluginsStore.getState().paused) {
      publishPlugin({ ...record, name: plugin.name ?? record.name, status: "disabled" });
      return;
    }
    activatePlugin(plugin, {
      ...record,
      name: plugin.name ?? record.name,
      description: plugin.description ?? record.description,
      version: plugin.version,
    });
  } catch (error) {
    unloadPlugin(entry.id);
    reportPluginError(record.name, "failed to load", error);
    publishPlugin({
      ...record,
      status: "error",
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

function enqueue(id: string, job: () => Promise<void>): Promise<void> {
  const next = (queues.get(id) ?? Promise.resolve()).then(job, job);
  queues.set(id, next);
  void next.finally(() => {
    if (queues.get(id) === next) queues.delete(id);
  });
  return next;
}

/** Publish (or refresh) one folder's row and load it when enabled. */
function adoptDisk(entry: DiskPluginEntry, reload: boolean): Promise<void> {
  return enqueue(entry.id, async () => {
    known.set(entry.id, entry);
    if (isBundled(entry.id)) {
      publishPlugin({
        ...baseRecord(entry),
        id: `${entry.id}${SHADOW_SUFFIX}`,
        name: `${entry.marker?.name ?? entry.id} (disk copy)`,
        description: `Shadowed by the bundled "${entry.id}" plugin. This folder is not used and can be deleted.`,
        status: "disabled",
        inert: true,
      });
      return;
    }
    if (entry.error) {
      unloadPlugin(entry.id);
      publishPlugin({ ...baseRecord(entry), status: "error", error: entry.error, inert: true });
      return;
    }
    const record = baseRecord(entry);
    const existing = usePluginsStore.getState().records[entry.id];
    publishPlugin(
      // Keep a live row's name/status while it reloads; the activation
      // republishes it.
      existing && existing.status !== "error"
        ? { ...existing, ...record, name: existing.name, status: existing.status }
        : { ...record, status: "disabled" },
      {
        activate: () => enqueue(entry.id, () => activateDisk(known.get(entry.id) ?? entry)),
        deactivate: () => unloadPlugin(entry.id),
      },
    );
    const paused = usePluginsStore.getState().paused;
    if (paused || !pluginActive(entry.id, defaultEnabledOf(entry))) {
      unloadPlugin(entry.id);
      publishPlugin({ ...record, name: existing?.name ?? record.name, status: "disabled" });
      return;
    }
    if (reload || existing?.status !== "loaded") await activateDisk(entry);
  });
}

/** Forget a folder that is gone: unload, drop rows. */
function retire(id: string): void {
  void enqueue(id, async () => {
    known.delete(id);
    unloadPlugin(id);
    if (usePluginsStore.getState().records[id]?.kind === "disk") dropPlugin(id);
    dropPlugin(`${id}${SHADOW_SUFFIX}`);
  });
}

let scanning: Promise<void> | null = null;
/** The one follow-up pass queued behind the scan in flight. */
let rescan: Promise<void> | null = null;
let rescanReload = false;

/** Native decisions are the truth after an install or remove; read them
 *  before acting on a folder change. A failed read keeps what we have. */
async function syncDecisions(): Promise<void> {
  if (!usePluginsStore.getState().decisionsLoaded) return;
  try {
    replaceDecisions(await pluginsGetDecisions());
  } catch (error) {
    console.warn("[plugins] could not re-read plugin decisions", error);
  }
}

/**
 * Reconcile the root with the inventory. `reloadKnown` (Reload plugins)
 * re-reads every enabled plugin too: an installer's atomic folder swap can
 * leave a watcher on the old inode.
 */
export function scanDiskPlugins(
  reloadKnown = false,
  { decisionsFresh = false }: { decisionsFresh?: boolean } = {},
): Promise<void> {
  // A request during a scan is a NEW invalidation (a file changed, a block
  // landed): the scan in flight may have read the old inventory, so run a
  // fresh pass after it. Requests arriving meanwhile share that one pass.
  if (scanning) {
    rescanReload ||= reloadKnown;
    rescan ??= scanning.then(() => {
      const reload = rescanReload;
      rescan = null;
      rescanReload = false;
      return scanDiskPlugins(reload);
    });
    return rescan;
  }
  scanning = (async () => {
    if (!decisionsFresh) await syncDecisions();
    let entries: DiskPluginEntry[];
    try {
      entries = await pluginsList();
    } catch (error) {
      console.error("[plugins] cannot list the plugins folder", error);
      return;
    }
    const seen = new Set(entries.map((e) => e.id));
    await Promise.all(
      entries.map((entry) => {
        const previous = known.get(entry.id);
        const changed = !previous || JSON.stringify(previous) !== JSON.stringify(entry);
        return changed || reloadKnown ? adoptDisk(entry, reloadKnown) : Promise.resolve();
      }),
    );
    for (const id of [...known.keys()]) if (!seen.has(id)) retire(id);
  })().finally(() => {
    scanning = null;
  });
  return scanning;
}

/** One folder changed on disk (from the native watcher). */
export async function onDiskChange(id: string | null): Promise<void> {
  if (id === null) {
    await scanDiskPlugins();
    return;
  }
  await syncDecisions();
  let entries: DiskPluginEntry[];
  try {
    entries = await pluginsList();
  } catch {
    return;
  }
  const entry = entries.find((e) => e.id === id);
  if (!entry) {
    if (known.has(id)) retire(id);
    return;
  }
  // A save is a reload for an enabled plugin; a disabled one only refreshes
  // its row (its file is still never evaluated).
  await adoptDisk(entry, true);
}

export function knownDiskPlugins(): readonly DiskPluginEntry[] {
  return [...known.values()];
}

/** Test-only. */
export function resetRuntimeLoaderForTests(): void {
  known.clear();
  queues.clear();
  scanning = null;
  rescan = null;
  rescanReload = false;
  appVersion = null;
}
