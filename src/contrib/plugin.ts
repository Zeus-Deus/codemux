/**
 * The plugin contract and the scoped `ctx` handed to `register(ctx)`. `ctx`
 * namespaces every contribution id as `<pluginId>:<localId>`, stamps its
 * source as `plugin:<pluginId>`, and tracks every disposer so turning the
 * plugin off (or hot-reloading it) runs every cleanup.
 */
import type { ReactNode } from "react";
import { registry } from "./registry";
import type { Contribution } from "./types";
import { pluginStorage } from "./storage";
import type { PluginTier } from "./plugins-store";

export const SETTINGS_PAGES_AREA = "settings.pages";

export interface CodemuxPlugin {
  /** `[a-z0-9-]{1,64}`; equals the folder name. */
  id: string;
  name?: string;
  description?: string;
  version?: string;
  /** Refuse to load on an older Codemux. */
  minCodemux?: string;
  /** Bundled plugins only. Installed plugins always start off. */
  defaultEnabled?: boolean;
  /** May be async. A throw or rejection rolls back everything registered. */
  register: (ctx: PluginContext) => void | Promise<void>;
}

/** A contribution as a plugin writes it: id is local, source is stamped. */
export type PluginContribution = Omit<Contribution, "source">;

export interface PluginStorage {
  get<T>(key: string, fallback: T): T;
  set(key: string, value: unknown): void;
  remove(key: string): void;
}

export interface PluginSettingsPage {
  /** Local id; defaults to `settings`. */
  id?: string;
  title: string;
  render: () => ReactNode;
}

export interface PluginContext {
  readonly id: string;
  /** `plugin:<id>` */
  readonly source: string;
  readonly tier: PluginTier;
  register: (c: PluginContribution) => () => void;
  registerMany: (cs: PluginContribution[]) => () => void;
  /** A page under Settings → Plugins → this plugin. */
  registerSettingsPage: (page: PluginSettingsPage) => () => void;
  onDispose: (fn: () => void) => void;
  setTimeout: (fn: () => void, ms: number) => () => void;
  setInterval: (fn: () => void, ms: number) => () => void;
  addEventListener: (
    target: EventTarget,
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: AddEventListenerOptions | boolean,
  ) => () => void;
  /** JSON in the device-local settings DB under `plugin.<id>.<key>`. Never synced. */
  storage: PluginStorage;
}

/** Timers and DOM listeners, retired as ONE tracked disposer. A fired
 *  timeout drops out of the set so long-lived plugins don't accumulate. */
function createLifetime(track: (dispose: () => void) => void) {
  const cleanups = new Set<() => void>();
  let tracked = false;
  const scoped = (cleanup: () => void) => {
    if (!tracked) {
      tracked = true;
      track(() => {
        cleanups.forEach((pending) => pending());
        cleanups.clear();
      });
    }
    cleanups.add(cleanup);
    return () => {
      if (!cleanups.delete(cleanup)) return;
      cleanup();
    };
  };
  return {
    setTimeout: (fn: () => void, ms: number) => {
      const clear = () => globalThis.clearTimeout(id);
      const id = globalThis.setTimeout(() => {
        cleanups.delete(clear);
        fn();
      }, ms);
      return scoped(clear);
    },
    setInterval: (fn: () => void, ms: number) => {
      const id = globalThis.setInterval(fn, ms);
      return scoped(() => globalThis.clearInterval(id));
    },
    addEventListener: (
      target: EventTarget,
      type: string,
      listener: EventListenerOrEventListenerObject,
      options?: AddEventListenerOptions | boolean,
    ) => {
      target.addEventListener(type, listener, options);
      return scoped(() => target.removeEventListener(type, listener, options));
    },
  };
}

export function createPluginContext(
  pluginId: string,
  tier: PluginTier,
  onDispose: (dispose: () => void) => void,
): PluginContext {
  const source = `plugin:${pluginId}`;
  const scope = (c: PluginContribution): Contribution => ({
    ...c,
    id: `${pluginId}:${c.id}`,
    source,
  });
  const track = (dispose: () => void) => {
    onDispose(dispose);
    return dispose;
  };
  return {
    id: pluginId,
    source,
    tier,
    register: (c) => track(registry.register(scope(c))),
    registerMany: (cs) => track(registry.registerMany(cs.map(scope))),
    registerSettingsPage: (page) =>
      track(
        registry.register(
          scope({
            id: page.id ?? "settings",
            area: SETTINGS_PAGES_AREA,
            title: page.title,
            render: page.render,
            data: { pluginId },
          }),
        ),
      ),
    onDispose: (fn) => void track(fn),
    ...createLifetime(track),
    storage: pluginStorage(pluginId),
  };
}
