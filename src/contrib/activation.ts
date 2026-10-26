/**
 * The lifecycle shared by bundled and disk plugins: publish a record with
 * activate/deactivate handles, register through a scoped ctx, roll back on a
 * throw or rejection, and unload cleanly (registration released before any
 * disposer runs, each disposer isolated).
 */
import { toast } from "@/lib/toast";
import { Owner, runAs } from "./lifetime";
import { createPluginContext, type CodemuxPlugin } from "./plugin";
import {
  patchPlugin,
  pluginActive,
  publishPlugin,
  usePluginsStore,
  type PluginRecord,
} from "./plugins-store";

/** Live plugins: id → owner of the current incarnation. */
const loaded = new Map<string, Owner>();

/** Live disposers of a plugin's current incarnation (tests, diagnostics). */
export function liveDisposers(id: string): number {
  return loaded.get(id)?.size ?? 0;
}

export function unloadPlugin(id: string): void {
  const owner = loaded.get(id);
  // Released BEFORE disposers run, each in its own try/catch: a throwing
  // disposer must not wedge every later reload (Hermes #126338). Last
  // registered, first disposed: mirrors setup order.
  loaded.delete(id);
  owner?.end();
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function reportPluginError(name: string, title: string, error: unknown): void {
  console.error(`[plugins] ${name}: ${title}`, error);
  toast.error(`Plugin ${name}: ${title}`, { description: message(error) });
}

/**
 * Register `plugin` now. Returns false (and leaves an error row) on failure.
 * A rejected async register rolls back only if this incarnation is current.
 */
export function activatePlugin(plugin: CodemuxPlugin, record: Omit<PluginRecord, "status">): boolean {
  unloadPlugin(plugin.id);
  // Set BEFORE register() so a throw midway is disposable. A disposer added
  // after this incarnation ended (a late async registration, a timer started
  // after unload) runs immediately instead of leaking.
  const owner = new Owner(plugin.id);
  loaded.set(plugin.id, owner);

  const fail = (error: unknown) => {
    unloadPlugin(plugin.id);
    reportPluginError(record.name, "failed to register", error);
    publishPlugin({ ...record, status: "error", error: message(error) });
  };

  let result: unknown;
  try {
    result = runAs(owner, () => plugin.register(createPluginContext(plugin.id, record.tier, owner)));
  } catch (error) {
    fail(error);
    return false;
  }
  publishPlugin({ ...record, status: "loaded", error: undefined });
  if (result && typeof (result as PromiseLike<unknown>).then === "function") {
    void Promise.resolve(result).catch((error: unknown) => {
      if (loaded.get(plugin.id) === owner) fail(error);
    });
  }
  return true;
}

/** Publish the record + handles, then activate when the decision says so. */
export function adoptPlugin(
  plugin: CodemuxPlugin,
  record: Omit<PluginRecord, "status">,
  defaultEnabled: boolean,
  options: { paused?: boolean } = {},
): void {
  publishPlugin(
    { ...record, status: "disabled" },
    {
      activate: () => {
        activatePlugin(plugin, record);
      },
      deactivate: () => unloadPlugin(plugin.id),
    },
  );
  if (options.paused) return;
  if (pluginActive(plugin.id, defaultEnabled)) activatePlugin(plugin, record);
}

/** Version gate for `minCodemux` (numeric dotted compare). */
export function satisfiesMinVersion(app: string, min: string | undefined): boolean {
  if (!min) return true;
  const parse = (v: string) => v.split(/[.+-]/).slice(0, 3).map((p) => Number.parseInt(p, 10) || 0);
  const a = parse(app);
  const b = parse(min);
  for (let i = 0; i < 3; i += 1) {
    if ((a[i] ?? 0) !== (b[i] ?? 0)) return (a[i] ?? 0) > (b[i] ?? 0);
  }
  return true;
}

export function markLoading(id: string): void {
  if (usePluginsStore.getState().records[id]) patchPlugin(id, { status: "loading" });
}

/** Test-only. */
export function resetActivationForTests(): void {
  for (const id of [...loaded.keys()]) unloadPlugin(id);
}
