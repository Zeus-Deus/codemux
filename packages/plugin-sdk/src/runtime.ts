import "@remote-dom/core/polyfill";
import { RemoteRootElement } from "@remote-dom/core/elements";
import { RemoteReceiver } from "@remote-dom/core/receivers";
import { h, render, options } from "preact";
import {
  PluginError,
  type ErrorCode,
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
// Stable host errors are ordinary outcomes of a handler or UI callback.
const handled: Record<ErrorCode, true> = {
  PERMISSION_DENIED: true,
  CONTEXT_STALE: true,
  NO_WORKSPACE: true,
  REMOTE_UNSUPPORTED: true,
  NO_COMPOSER: true,
  INTERACTION_REQUIRED: true,
  NOT_A_GIT_REPO: true,
  INCOMPATIBLE_API: true,
  RESOURCE_LIMIT: true,
  TIMEOUT: true,
  PLUGIN_STOPPED: true,
  INVALID_MESSAGE: true,
  CREDENTIAL_REQUIRED: true,
  NETWORK_DENIED: true,
  STORAGE_UNAVAILABLE: true,
};
// Sliding windows kept stricter than the native quotas. The parent counts on
// arrival, so longer windows and lower counts absorb pipe jitter, millisecond
// rounding and the 100 ms timer floor: a paced plugin never reaches a quota.
function limiter(windows: [number, number][]) {
  const span = Math.max(...windows.map(([ms]) => ms));
  let times: number[] = [];
  return {
    // Earliest time one more event fits behind `queued` earlier FIFO events.
    next(t: number, queued = 0) {
      const list = times.filter((x) => x > t - span);
      let at = t;
      for (let n = 0; n <= queued; n++) {
        for (const [ms, count] of windows)
          if (list.length >= count)
            at = Math.max(at, list[list.length - count] + ms);
        list.push(at);
      }
      return at;
    },
    take(t: number) {
      times = times.filter((x) => x > t - span);
      times.push(t);
    },
  };
}
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
    stopped = false,
    activated = false;
  // A null entry is a registration disposed after activation.
  const handlers = new Map<string, Handler | ViewRenderer | null>();
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
      acknowledge(revision: number): void;
      flush(): void;
    }
  >();
  // Host quotas: 20 requests/s and 100/min, 30 UI batches/s, 5 logs/s.
  const requestQuota = limiter([
    [1200, 18],
    [62000, 95],
  ]);
  const uiQuota = limiter([
    [250, 4],
    [1200, 20],
  ]);
  const logQuota = limiter([
    [1200, 4],
    [61000, 240],
  ]);
  const waiting: (() => void)[] = [];
  let requestTimer: ReturnType<typeof setTimeout> | undefined;
  let uiTimer: ReturnType<typeof setTimeout> | undefined;
  const pace = (quota: ReturnType<typeof limiter>) => {
    const t = now();
    if (quota.next(t) > t) return false;
    quota.take(t);
    return true;
  };
  // Excess diagnostics are dropped locally rather than sent over quota.
  const diagnose = (message: string) => {
    if (pace(logQuota)) send("log", { message: message.slice(0, 1024) });
  };
  try {
    const native = globalThis.console as unknown as Record<
      string,
      (...args: unknown[]) => void
    >;
    globalThis.console = Object.freeze(
      Object.fromEntries(
        ["log", "info", "warn", "error", "debug"].map((level) => [
          level,
          (...args: unknown[]) => {
            if (pace(logQuota)) native[level](...args);
          },
        ]),
      ),
    ) as unknown as Console;
  } catch {
    // A host that freezes console keeps its own bounded logging.
  }
  // Report a stable host rejection from an author callback and keep running.
  // Synchronous throws and any other rejection remain runtime faults.
  const settle = (result: unknown, source: string) => {
    if (!result || typeof (result as PromiseLike<unknown>).then !== "function")
      return;
    Promise.resolve(result).then(undefined, (error: unknown) => {
      if (!(error instanceof PluginError) || handled[error.code] !== true)
        throw error;
      diagnose(`${source} rejected with ${error.code}: ${error.message}`);
    });
  };
  const pump = () => {
    requestTimer = undefined;
    while (waiting.length && !stopped) {
      const t = now(),
        at = requestQuota.next(t);
      if (at > t) {
        requestTimer = setTimeout(pump, at - t);
        return;
      }
      requestQuota.take(t);
      waiting.shift()!();
    }
  };
  function request<T>(operation: string, params: unknown): Promise<T> {
    if (stopped)
      return Promise.reject(
        new PluginError("PLUGIN_STOPPED", "Plugin stopped"),
      );
    if (pending.size >= 16)
      return Promise.reject(
        new PluginError("RESOURCE_LIMIT", "Too many outstanding requests"),
      );
    // Short bursts wait for the per-second window. A request that could only
    // go once the per-minute window drains is rejected before it reaches the
    // host.
    const t = now();
    if (requestQuota.next(t, waiting.length) - t > 1500)
      return Promise.reject(
        new PluginError(
          "RESOURCE_LIMIT",
          "Too many host requests in the last minute; try again later",
        ),
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
      waiting.push(() => {
        if (pending.has(id)) send("host.request", { operation, params }, id);
      });
      if (!requestTimer) pump();
    });
  }
  function registration(kind: Kind) {
    return (id: string, handler: Handler | ViewRenderer) => {
      const key = kind + "/" + id;
      if (
        !manifest.contributes[kind].some((d) => d.id === id) ||
        handlers.get(key)
      )
        throw new PluginError(
          "INVALID_MESSAGE",
          "Unknown or duplicate contribution " + id,
        );
      handlers.set(key, handler);
      return () => {
        if (handlers.get(key) !== handler) return;
        // Contributions are fixed once the host has the registration list;
        // disposing afterwards only stops callbacks for this generation.
        if (activated) handlers.set(key, null);
        else handlers.delete(key);
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
    views.delete(viewId);
    render(null, view.root);
    view.callbacks.clear();
    view.ids.clear();
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
          activated = true;
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
        const key = kind + "/" + p.id;
        if (!handlers.has(key)) fail();
        const handler = handlers.get(key) as Handler | null;
        if (!handler) diagnose(`${key} was disposed; the call was ignored`);
        else settle(handler(p.context), key);
        break;
      }
      case "view.mount": {
        // The desktop enforces the view limit and issues each view ID once. It
        // can send a mount before the unmount that freed its slot, so views
        // beyond the limit here are not a fault.
        if (views.has(p.viewId)) fail();
        const kind = p.kind === "composerViews" ? "composerViews" : "panels";
        const key = kind + "/" + p.id;
        if (!handlers.has(key)) fail();
        const renderer = handlers.get(key) as ViewRenderer | null;
        const root = document.createElement("cmx-root") as RemoteRootElement;
        const receiver = new RemoteReceiver();
        const callbacks = new Map<string, Function>();
        const ids = new Map<Function, string>();
        let sentRevision = 0;
        let acknowledgedRevision = 0;
        views.set(p.viewId, {
          root,
          receiver,
          callbacks,
          ids,
          acknowledge(revision) {
            if (!Number.isSafeInteger(revision) || revision > sentRevision)
              fail();
            acknowledgedRevision = Math.max(acknowledgedRevision, revision);
            flush();
          },
          flush: () => flush(),
        });
        // Preact can emit many individual mutations during one commit. Batch one
        // microtask, with a bound before enqueueing, so native validation sees the
        // commit atomically rather than consuming the two-batch queue per node.
        let queued: any[] = [];
        let scheduled = false;
        const flush = () => {
          scheduled = false;
          if (!views.has(p.viewId)) {
            queued = [];
            return;
          }
          // One batch in flight; keep ordered, bounded mutations until the
          // trusted renderer acknowledges it. Slow rendering is not a fault.
          if (acknowledgedRevision < sentRevision || queued.length === 0)
            return;
          // All views share the plugin's batch rate. While paced, records keep
          // coalescing and one shared timer retries every view.
          const t = now(),
            at = uiQuota.next(t);
          if (at > t) {
            uiTimer ??= setTimeout(() => {
              uiTimer = undefined;
              for (const view of views.values()) view.flush();
            }, at - t);
            return;
          }
          uiQuota.take(t);
          const records = queued;
          queued = [];
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
          // The desktop bounds live callbacks over the views it holds. A view
          // it has dropped can still be here, so no local total is enforced.
          sentRevision++;
          send("ui.patch", { viewId: p.viewId, records: serialized });
        };
        root.connect({
          mutate(records) {
            if (!views.has(p.viewId)) return;
            for (const record of records) {
              // Only the latest text or property value of a node matters while
              // a batch waits, so held input cannot exhaust the mutation bound.
              if (record[0] === 2 || record[0] === 3) {
                const index = queued.findIndex(
                  (r) =>
                    r[0] === record[0] &&
                    r[1] === record[1] &&
                    (r[0] === 2 ||
                      (r[2] === record[2] && (r[4] ?? 1) === (record[4] ?? 1))),
                );
                if (index >= 0) queued.splice(index, 1);
              }
              queued.push(record);
            }
            if (queued.length > 1000)
              throw new PluginError("RESOURCE_LIMIT", "Mutation limit");
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
        if (renderer)
          render(h(renderer, { viewId: p.viewId, context: p.context }), root);
        else diagnose(`${key} was disposed; the view stays empty`);
        break;
      }
      case "view.unmount":
        unmount(p.viewId);
        break;
      case "ui.ack":
        views.get(p.viewId)?.acknowledge(p.revision);
        break;
      case "ui.event": {
        if (
          typeof p?.viewId !== "string" ||
          typeof p.callbackId !== "string" ||
          typeof p.context !== "string" ||
          (p.value != null &&
            typeof p.value !== "string" &&
            typeof p.value !== "boolean")
        )
          fail();
        // The desktop checks each event against the tree it has applied, so an
        // event can still cross the patch that released its callback, or an
        // unmount. That is a stale click, not a fault: ignore it.
        const view = views.get(p.viewId);
        const fn = view?.callbacks.get(p.callbackId);
        if (!fn) {
          diagnose(
            `A UI event for a ${view ? "released callback" : "closed view"} was ignored`,
          );
          break;
        }
        // Remote DOM returns the author's promise through the event response.
        settle(fn({ context: p.context, value: p.value }), "UI callback");
        break;
      }
      case "workspace.changed":
        for (const id of views.keys()) unmount(id);
        for (const callback of workspaceListeners)
          settle(callback(p.context), "workspace.subscribe");
        break;
      case "settings.changed":
        for (const callback of settingsListeners)
          settle(callback(p.settings), "settings.subscribe");
        break;
      case "deactivate": {
        stopped = true;
        for (const id of views.keys()) unmount(id);
        handlers.clear();
        workspaceListeners.clear();
        settingsListeners.clear();
        waiting.length = 0;
        clearTimeout(requestTimer);
        clearTimeout(uiTimer);
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
