/**
 * Web-remote Tauri runtime shim.
 *
 * The production twin of `src/dev/tauri-mock.ts`. Where the mock answers
 * every call from in-process fixtures, this shim installs the same
 * `window.__TAURI_INTERNALS__` surface backed by a real
 * {@link RemoteTransport} WebSocket to the desktop app. The unmodified
 * `@tauri-apps/api` code — `invoke()`, `listen`/`emit`/`once`, the
 * window/app plugin wrappers, and `Channel` — routes every call through
 * here with zero app-component changes.
 *
 * Mirrors the mock's five-member internals contract and its plugin-route
 * inventory (see `tauri-mock.ts` ~1137-1263):
 *   - `plugin:event|listen`/`unlisten` → transport `{t:listen}`/`{t:unlisten}`
 *     with per-event refcounting; `emit`/`emit_to` → local fan-out.
 *   - `plugin:opener|open_url` → `window.open`.
 *   - `plugin:window|*` / `plugin:webview*` → no-ops.
 *   - `plugin:app|version` → the value served by `/api/health`.
 *   - `plugin:updater|*` / `plugin:process|*` / `plugin:dialog|*` → null
 *     (Stage 1; web fallbacks land in Stage 3).
 * Every other command is a real backend command and is forwarded over the
 * socket as `{t:"invoke"}`.
 */
import {
  RemoteTransport,
  type ConnectionStatus,
  type TransportDeps,
} from "./transport";

type RawCallback = (payload: unknown) => void;
type Args = Record<string, unknown>;

export interface InstallShimOptions {
  /** Server origin the transport talks to (also the asset origin). */
  baseUrl: string;
  /** Current session bearer token (null → cookie fallback). */
  getToken(): string | null;
  /** App version served by `/api/health`, for `plugin:app|version`. */
  appVersion?: string | null;
  /** Connection-state changes (drives the reconnecting banner). */
  onStatusChange?(status: ConnectionStatus): void;
  /** Session revoked / invalid → clear + return to pairing. */
  onUnauthorized?(): void;
  /** Injectable transport seams (tests). */
  deps?: TransportDeps;
}

/** Sentinel: a command that isn't handled locally and must hit the wire. */
const MISS = Symbol("web-remote-miss");

export interface ShimHandle {
  transport: RemoteTransport;
}

/**
 * Install the shim onto `window` and return the transport (not yet
 * connected — the caller calls `transport.connect()`). Sets
 * `window.__CODEMUX_REMOTE__ = true` so other subsystems can detect the
 * web client before React mounts.
 */
export function installShim(options: InstallShimOptions): ShimHandle {
  // ── Callback registry (shared by Channels and event listeners) ──
  const callbacks = new Map<number, { fn: RawCallback; once: boolean }>();
  let nextCallbackId = 1;

  // event name -> (eventId -> callbackId)
  const eventListeners = new Map<string, Map<number, number>>();
  let nextEventId = 1;
  // event name -> active listener count, gating WS listen/unlisten frames.
  const eventRefcount = new Map<string, number>();

  const appVersion = options.appVersion ?? "0.0.0";

  const transport = new RemoteTransport({
    baseUrl: options.baseUrl,
    getToken: options.getToken,
    deps: options.deps,
    hooks: {
      onChannelMessage: dispatchChannel,
      onEvent: dispatchEvent,
      onStatusChange: (status) => options.onStatusChange?.(status),
      onUnauthorized: () => options.onUnauthorized?.(),
    },
  });

  function transformCallback(fn: RawCallback, once = false): number {
    const id = nextCallbackId++;
    callbacks.set(id, { fn, once });
    return id;
  }

  /** Deliver a channel body to its stored callback as `{index, message}`
   *  — exactly the shape `@tauri-apps/api`'s `Channel` orders and unwraps
   *  (mirrors `tauri-mock.ts` `chatChannelPush`). */
  function dispatchChannel(
    callbackId: number,
    index: number,
    message: unknown,
  ): void {
    const cb = callbacks.get(callbackId);
    if (!cb) return;
    try {
      cb.fn({ index, message } as unknown as never);
    } catch (err) {
      console.error("[web-remote] channel dispatch threw:", err);
    }
  }

  /** Fan a backend event out to its subscribed listeners with the real
   *  Tauri event shape `{event, id, payload}`. */
  function dispatchEvent(event: string, payload: unknown): void {
    const map = eventListeners.get(event);
    if (!map) return;
    // Snapshot: a `once` listener mutates the map mid-iteration.
    for (const [eventId, callbackId] of [...map.entries()]) {
      const cb = callbacks.get(callbackId);
      if (!cb) continue;
      try {
        cb.fn({ event, id: eventId, payload } as unknown as never);
      } catch (err) {
        console.error(`[web-remote] listener for "${event}" threw:`, err);
      }
      if (cb.once) {
        callbacks.delete(callbackId);
        removeListener(event, eventId);
      }
    }
  }

  function registerListener(event: string, callbackId: number): number {
    const eventId = nextEventId++;
    let map = eventListeners.get(event);
    if (!map) {
      map = new Map();
      eventListeners.set(event, map);
    }
    map.set(eventId, callbackId);
    const next = (eventRefcount.get(event) ?? 0) + 1;
    eventRefcount.set(event, next);
    if (next === 1) transport.subscribe(event);
    return eventId;
  }

  /** Remove one listener. Idempotent: the real unlisten path signals
   *  twice (via `__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener` and
   *  the `plugin:event|unlisten` invoke), and this must decrement the
   *  refcount only once. */
  function removeListener(event: string, eventId: number): void {
    const map = eventListeners.get(event);
    if (!map || !map.has(eventId)) return;
    map.delete(eventId);
    if (map.size === 0) eventListeners.delete(event);
    const next = (eventRefcount.get(event) ?? 1) - 1;
    if (next <= 0) {
      eventRefcount.delete(event);
      transport.unsubscribe(event);
    } else {
      eventRefcount.set(event, next);
    }
  }

  function routePlugin(cmd: string, args: Args): unknown {
    // Event system — the backbone of listen()/emit()/once().
    if (cmd === "plugin:event|listen") {
      return registerListener(args.event as string, args.handler as number);
    }
    if (cmd === "plugin:event|unlisten") {
      removeListener(args.event as string, args.eventId as number);
      return undefined;
    }
    if (cmd === "plugin:event|emit" || cmd === "plugin:event|emit_to") {
      dispatchEvent(args.event as string, args.payload);
      return undefined;
    }

    // Opener — open in a new tab (real behavior, unlike the mock's toast).
    if (cmd === "plugin:opener|open_url" || cmd === "plugin:opener|open_path") {
      const url = args.url ?? args.path;
      if (typeof url === "string") {
        window.open(url, "_blank", "noopener,noreferrer");
      }
      return undefined;
    }
    if (cmd.startsWith("plugin:opener|")) return undefined;

    // Window / webview controls — no-ops so WindowControls & drag regions
    // don't crash on the web.
    if (cmd.startsWith("plugin:window|") || cmd.startsWith("plugin:webview")) {
      if (cmd.endsWith("|scale_factor")) return 1;
      if (cmd.endsWith("|theme")) return "dark";
      if (/\|is_/.test(cmd)) return false;
      return undefined;
    }

    // App metadata.
    if (cmd === "plugin:app|version") return appVersion;
    if (cmd === "plugin:app|name") return "Codemux";
    if (cmd === "plugin:app|tauri_version") return "2.0.0";
    if (cmd.startsWith("plugin:app|")) return undefined;

    // Updater / process / dialog — inert for Stage 1 (Stage 3 adds web
    // fallbacks). `null` reads as "no update / cancelled".
    if (cmd.startsWith("plugin:updater|")) return null;
    if (cmd.startsWith("plugin:process|")) return null;
    if (cmd.startsWith("plugin:dialog|")) return null;

    // Any other plugin: don't forward a plugin command to the backend
    // dispatcher (it only knows app commands); no-op like the mock.
    if (cmd.startsWith("plugin:")) {
      console.warn(`[web-remote] unhandled plugin command: ${cmd}`);
      return undefined;
    }

    return MISS;
  }

  async function invoke(cmd: string, args: Args = {}): Promise<unknown> {
    const local = routePlugin(cmd, args);
    if (local !== MISS) return local;
    // Real backend command → over the wire.
    return transport.invoke(cmd, args);
  }

  const internals = {
    invoke,
    transformCallback,
    unregisterCallback: (id: number) => callbacks.delete(id),
    // `convertFileSrc` maps a local file path to the authed asset route on
    // the server origin. The route lands in Stage 3; the shape is fixed
    // now so image/asset call sites resolve correctly.
    convertFileSrc: (filePath: string) =>
      `${options.baseUrl.replace(/\/+$/, "")}/api/assets?path=${encodeURIComponent(
        filePath,
      )}`,
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main", windowLabel: "main" },
    },
  };

  const win = window as unknown as {
    __TAURI_INTERNALS__: typeof internals;
    __TAURI_EVENT_PLUGIN_INTERNALS__: {
      unregisterListener: (event: string, eventId: number) => void;
    };
    __CODEMUX_REMOTE__: boolean;
  };
  win.__TAURI_INTERNALS__ = internals;
  win.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: removeListener };
  // Set before the app mounts — other subsystems (e.g. the scrollback
  // serializer) key off this to opt out of desktop-only behavior.
  win.__CODEMUX_REMOTE__ = true;

  return { transport };
}
