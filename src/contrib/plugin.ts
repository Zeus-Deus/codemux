/**
 * The plugin contract and the scoped `ctx` handed to `register(ctx)`. `ctx`
 * namespaces every contribution id as `<pluginId>:<localId>`, stamps its
 * source as `plugin:<pluginId>`, and tracks every disposer so turning the
 * plugin off (or hot-reloading it) runs every cleanup.
 */
import { registerMcpServer, type PluginMcpServer } from "./mcp";
import type { ReactNode } from "react";
import { registry } from "./registry";
import type { Contribution } from "./types";
import { pluginStorage } from "./storage";
import type { PluginTier } from "./plugins-store";
import { bindOwner, ownedDisposer, type Owner } from "./lifetime";
import { createHost, type Host } from "@/sdk/host";
import { apiVersion, rendererPluginCompatibility, type RendererPluginCapability, type RendererPluginRequirements } from "@/sdk/compatibility";

export const SETTINGS_PAGES_AREA = "settings.pages";

export interface CodemuxPlugin extends RendererPluginRequirements {
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

/** Setup state a plugin shows on its Extensions row. It is separate from
 *  the on/off switch: an enabled plugin can still need setup. */
export interface PluginSetupStatus {
  label: string;
  tone: "ready" | "attention" | "neutral";
}

export interface PluginSettingsPage {
  /** Local id; defaults to `settings`. */
  id?: string;
  title: string;
  render: () => ReactNode;
  /** Optional setup status for the plugin's Extensions row. `get` must return
   *  the same object until it changes; `subscribe` reports changes. */
  status?: {
    get: () => PluginSetupStatus | null;
    subscribe: (fn: () => void) => () => void;
  };
}

export interface PluginContext {
  readonly apiVersion: typeof apiVersion;
  readonly capabilities: readonly RendererPluginCapability[];
  /** Aborted before disposal on disable, reload, registration failure or removal. */
  readonly signal: AbortSignal;
  readonly id: string;
  /** `plugin:<id>` */
  readonly source: string;
  readonly tier: PluginTier;
  register: (c: PluginContribution) => () => void;
  registerMcpServer: (server: PluginMcpServer) => Promise<() => Promise<void>>;
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
  /**
   * The host API bound to this plugin: every subscription made through it is
   * removed when the plugin turns off, including ones made after an `await`
   * (where a bare `host.*` call can't tell which plugin is calling).
   */
  host: Host;
}

/** Timers and DOM listeners, owned by the incarnation: cancelled when the
 *  plugin turns off, cancelled at once if started after that, and released
 *  from the owner when they finish or are cancelled early (so a plugin that
 *  keeps scheduling timeouts never accumulates them). Callbacks run as the
 *  owner, so host subscriptions made inside them are owned too. */
function createLifetime(owner: Owner) {
  return {
    setTimeout: (fn: () => void, ms: number) => {
      if (owner.ended) return () => {};
      const run = bindOwner(owner, fn);
      const id = globalThis.setTimeout(() => {
        dispose();
        run();
      }, ms);
      const dispose = ownedDisposer(owner, () => globalThis.clearTimeout(id));
      return dispose;
    },
    setInterval: (fn: () => void, ms: number) => {
      if (owner.ended) return () => {};
      const id = globalThis.setInterval(bindOwner(owner, fn), ms);
      return ownedDisposer(owner, () => globalThis.clearInterval(id));
    },
    addEventListener: (
      target: EventTarget,
      type: string,
      listener: EventListenerOrEventListenerObject,
      options?: AddEventListenerOptions | boolean,
    ) => {
      if (owner.ended) return () => {};
      const bound: EventListenerOrEventListenerObject =
        typeof listener === "function"
          ? bindOwner(owner, listener)
          : { handleEvent: bindOwner(owner, (e: Event) => listener.handleEvent(e)) };
      target.addEventListener(type, bound, options);
      return ownedDisposer(owner, () => target.removeEventListener(type, bound, options));
    },
  };
}

export function createPluginContext(pluginId: string, tier: PluginTier, owner: Owner, requirements: RendererPluginRequirements = {}): PluginContext {
  const source = `plugin:${pluginId}`;
  const scope = (c: PluginContribution): Contribution => ({
    ...c,
    id: `${pluginId}:${c.id}`,
    source,
  });
  // Unregistering early releases the disposer from the owner too.
  const track = (dispose: () => void) => ownedDisposer(owner, dispose);
  return {
    apiVersion,
    capabilities: rendererPluginCompatibility(requirements).capabilities,
    signal: owner.signal,
    id: pluginId,
    source,
    tier,
    registerMcpServer: (server) => registerMcpServer(owner, server),
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
            data: { pluginId, status: page.status },
          }),
        ),
      ),
    onDispose: (fn) => void owner.track(fn),
    ...createLifetime(owner),
    storage: pluginStorage(pluginId),
    host: createHost(owner),
  };
}
