import "@remote-dom/core/polyfill";
import { RemoteRootElement } from "@remote-dom/core/elements";
import { RemoteReceiver } from "@remote-dom/core/receivers";
import { h, render, options } from "preact";
import {
  PluginError,
  type Plugin,
  type PluginContext,
  type ContextHandle,
  type Json,
  type Handler,
  type ViewRenderer,
} from "./types.js";
options.debounceRendering = (fn) => Promise.resolve().then(fn);
customElements.define("cmx-root", RemoteRootElement);
type Kind = "commands" | "panels" | "composerActions" | "composerViews";
interface Manifest {
  id: string;
  contributes: Record<Kind, { id: string }[]>;
}
interface Transport {
  manifest: Manifest;
  now(): number;
  send(method: string, params: unknown, id?: number): void;
}
interface Message {
  id?: number;
  method?: string;
  result?: unknown;
  error?: { message: string; data: { code: PluginError["code"] } };
  params: any;
}
declare global {
  var __codemuxRegister: (
    plugin: Plugin,
    adapter: (
      transport: Transport,
    ) => (message: Message, plugin: Plugin) => void,
  ) => void;
}
export function register(plugin: Plugin) {
  globalThis.__codemuxRegister(plugin, adapter);
}
function adapter({ manifest, send, now }: Transport) {
  let seq = 0,
    callbackSeq = 0,
    stopped = false;
  const handlers = new Map<string, Handler | ViewRenderer>();
  const workspaceListeners = new Set<(c: ContextHandle | null) => void>();
  const settingsListeners = new Set<(s: Record<string, Json>) => void>();
  const pending = new Map<
    number,
    {
      resolve: (value: any) => void;
      reject: (error: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    }
  >();
  const views = new Map<
    string,
    {
      root: RemoteRootElement;
      receiver: RemoteReceiver;
      callbacks: Map<string, Function>;
      ids: Map<Function, string>;
    }
  >();
  function request<T>(operation: string, params: unknown): Promise<T> {
    if (stopped)
      return Promise.reject(
        new PluginError("PLUGIN_STOPPED", "Plugin stopped"),
      );
    if (pending.size >= 16)
      return Promise.reject(
        new PluginError("RESOURCE_LIMIT", "Too many outstanding requests"),
      );
    const id = ++seq;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => {
          pending.delete(id);
          reject(new PluginError("TIMEOUT", "Host request timed out"));
        },
        operation === "http.fetch" ? 30000 : 15000,
      );
      pending.set(id, { resolve, reject, timer });
      send("host.request", { operation, params }, id);
    });
  }
  function registration(kind: Kind) {
    return (id: string, handler: Handler | ViewRenderer) => {
      const key = kind + "/" + id;
      if (
        !manifest.contributes[kind].some((d) => d.id === id) ||
        handlers.has(key)
      )
        throw new PluginError(
          "INVALID_MESSAGE",
          "Unknown or duplicate contribution " + id,
        );
      handlers.set(key, handler);
      return () => {
        handlers.delete(key);
      };
    };
  }
  const ctx: PluginContext = {
    commands: { register: registration("commands") },
    panels: {
      register: registration("panels"),
      open: (id, context) => request("panels.open", { id, context }),
    },
    composerActions: { register: registration("composerActions") },
    composerViews: {
      register: registration("composerViews"),
      open: (id, context) => request("composerViews.open", { id, context }),
    },
    workspace: {
      current: (context) => request("workspace.current", { context }),
      subscribe: (callback) => {
        workspaceListeners.add(callback);
        return () => {
          workspaceListeners.delete(callback);
        };
      },
    },
    git: { summary: (context) => request("git.summary", { context }) },
    composer: {
      appendText: (context, text) =>
        request("composer.appendText", { context, text }),
    },
    settings: {
      get: () => request("settings.get", {}),
      subscribe: (callback) => {
        settingsListeners.add(callback);
        return () => {
          settingsListeners.delete(callback);
        };
      },
    },
    storage: {
      get: (scope, key) => request("storage.get", { ...scope, key }),
      set: (scope, key, value) =>
        request("storage.set", { ...scope, key, value }),
      delete: (scope, key) => request("storage.delete", { ...scope, key }),
    },
    http: {
      fetch: (context, requestData) =>
        request("http.fetch", { context, ...requestData }),
    },
    links: { open: (url, context) => request("links.open", { url, context }) },
    ui: { notify: (message) => request("ui.notify", { message }) },
  };
  const unmount = (viewId: string) => {
    const view = views.get(viewId);
    if (!view) return;
    render(null, view.root);
    view.callbacks.clear();
    view.ids.clear();
    views.delete(viewId);
  };
  const fail = () => {
    throw new PluginError("INVALID_MESSAGE", "Invalid runtime operation");
  };
  return (message: Message, definition: Plugin) => {
    if (!message.method) {
      const call = pending.get(message.id!);
      if (!call) return;
      pending.delete(message.id!);
      clearTimeout(call.timer);
      if (message.error)
        call.reject(
          new PluginError(message.error.data.code, message.error.message),
        );
      else call.resolve(message.result);
      return;
    }
    const p = message.params;
    switch (message.method) {
      case "activate": {
        Promise.resolve(definition.activate(ctx)).then(() => {
          for (const kind of Object.keys(manifest.contributes) as Kind[])
            for (const { id } of manifest.contributes[kind])
              if (!handlers.has(kind + "/" + id)) fail();
          send("ready", {
            phase: "activated",
            registrations: [...handlers.keys()],
          });
        });
        break;
      }
      case "command.execute": {
        const kind =
          p.kind === "composerActions" ? "composerActions" : "commands";
        const handler = handlers.get(kind + "/" + p.id) as Handler | undefined;
        if (!handler) fail();
        Promise.resolve(handler!(p.context)).catch(fail);
        break;
      }
      case "view.mount": {
        if (views.size >= 4 || views.has(p.viewId))
          throw new PluginError("RESOURCE_LIMIT", "View limit");
        const kind = p.kind === "composerViews" ? "composerViews" : "panels";
        const renderer = handlers.get(kind + "/" + p.id) as
          | ViewRenderer
          | undefined;
        if (!renderer) fail();
        const root = document.createElement("cmx-root") as RemoteRootElement;
        const receiver = new RemoteReceiver();
        const callbacks = new Map<string, Function>();
        const ids = new Map<Function, string>();
        views.set(p.viewId, { root, receiver, callbacks, ids });
        // Preact can emit many individual mutations during one commit. Batch one
        // microtask, with a bound before enqueueing, so native validation sees the
        // commit atomically rather than consuming the two-batch queue per node.
        let queued: any[] = [];
        let scheduled = false;
        const flush = () => {
          scheduled = false;
          const records = queued;
          queued = [];
          if (records.length === 0) return;
          receiver.connection.mutate(records);
          const live = new Set<Function>();
          const visit = (v: any): void => {
            if (typeof v === "function") {
              live.add(v);
              return;
            }
            if (v && typeof v === "object")
              for (const x of Object.values(v)) visit(x);
          };
          visit(receiver.root);
          for (const [fn, id] of ids)
            if (!live.has(fn)) {
              ids.delete(fn);
              callbacks.delete(id);
            }
          const resets = new Set([
            "spacing",
            "direction",
            "columns",
            "align",
            "width",
            "height",
            "size",
            "color",
            "name",
            "disabled",
            "checked",
            "level",
            "max",
            "options",
            "rows",
            "headers",
            "items",
          ]);
          // Preact uses an empty string to clear custom-element properties. Map
          // that deletion to the wire's null sentinel, without weakening the
          // native enum/type validator or permitting arbitrary properties.
          const normalize = (value: any): any => {
            if (Array.isArray(value)) return value.map(normalize);
            if (value && typeof value === "object") {
              const copy: any = { ...value };
              if (copy.properties)
                copy.properties = Object.fromEntries(
                  Object.entries(copy.properties).filter(
                    ([key, v]) => !(v === "" && resets.has(key)),
                  ),
                );
              if (copy.children) copy.children = copy.children.map(normalize);
              return copy;
            }
            return value;
          };
          const normalized = records.map((record) => {
            const copy = normalize(record);
            if (
              copy[0] === 3 &&
              (copy[4] ?? 1) === 1 &&
              copy[3] === "" &&
              resets.has(copy[2])
            )
              copy[3] = null;
            return copy;
          });
          const serialized = JSON.parse(
            JSON.stringify(normalized, (_key, value) => {
              if (typeof value !== "function") return value;
              let id = ids.get(value);
              if (!id) {
                id = String(++callbackSeq);
                ids.set(value, id);
                callbacks.set(id, value);
              }
              return { callbackId: id };
            }),
          );
          if (
            [...views.values()].reduce((n, v) => n + v.callbacks.size, 0) > 4096
          )
            throw new PluginError("RESOURCE_LIMIT", "Callback limit");
          send("ui.patch", { viewId: p.viewId, records: serialized });
        };
        root.connect({
          mutate(records) {
            if (queued.length + records.length > 1000)
              throw new PluginError("RESOURCE_LIMIT", "Mutation limit");
            queued.push(...records);
            if (!scheduled) {
              scheduled = true;
              Promise.resolve().then(flush);
            }
          },
          call() {
            throw new PluginError(
              "PERMISSION_DENIED",
              "Remote methods are not exposed",
            );
          },
        });
        render(h(renderer!, { viewId: p.viewId, context: p.context }), root);
        break;
      }
      case "view.unmount":
        unmount(p.viewId);
        break;
      case "ui.event": {
        const fn = views.get(p.viewId)?.callbacks.get(p.callbackId);
        if (!fn) fail();
        fn!({ context: p.context, value: p.value });
        break;
      }
      case "workspace.changed":
        for (const id of views.keys()) unmount(id);
        for (const callback of workspaceListeners) callback(p.context);
        break;
      case "settings.changed":
        for (const callback of settingsListeners) callback(p.settings);
        break;
      case "deactivate": {
        stopped = true;
        for (const id of views.keys()) unmount(id);
        handlers.clear();
        workspaceListeners.clear();
        settingsListeners.clear();
        for (const call of pending.values()) {
          clearTimeout(call.timer);
          call.reject(new PluginError("PLUGIN_STOPPED", "Plugin stopped"));
        }
        pending.clear();
        definition.deactivate?.();
        break;
      }
      default:
        fail();
    }
  };
}
