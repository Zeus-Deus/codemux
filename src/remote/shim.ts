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
import type { SnapshotSeed } from "./snapshot-seed";

type RawCallback = (payload: unknown) => void;
type Args = Record<string, unknown>;

/** The backend command whose first response the HTTP snapshot seeds. */
const APP_STATE_COMMAND = "get_app_state";
/** The event carrying a full `AppStateSnapshot` over the WS. A snapshot
 *  delivered on this event supersedes the HTTP seed (the no-stale guard). */
const APP_STATE_EVENT = "app-state-changed";

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
  /**
   * Fire an `/api/snapshot` prefetch, resolving to the seed (`app_state`) or
   * `null` on any failure. Called once at install (the initial prefetch, run
   * concurrently with the WS handshake) and again on every reconnect. When
   * omitted, seeding is disabled and every `get_app_state` takes the WS path.
   */
  fetchSnapshot?(): Promise<SnapshotSeed | null>;
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

  // ── HTTP snapshot seed (parallel bootstrap) ─────────────────────────
  // The initial `/api/snapshot` prefetch is kicked off here — synchronously at
  // install, so it runs concurrently with the caller's subsequent
  // `transport.connect()` (the ticket POST + WS upgrade). We answer the app's
  // first `get_app_state` from it (below) so the UI paints real state without a
  // post-mount socket round-trip.
  const seedPromise: Promise<SnapshotSeed | null> = options.fetchSnapshot
    ? options.fetchSnapshot()
    : Promise.resolve(null);
  let seedValue: SnapshotSeed | null = null;
  let seedSettled = false;
  let seedConsumed = false;
  // Monotonic count of app-state snapshots delivered over the WS. This is the
  // no-stale-overwrite guard: the seed is applied only while this is still 0
  // (initial connect) or unchanged since a reseed was kicked off — i.e. nothing
  // newer has arrived over the socket.
  let wsAppStateSeq = 0;
  let connectedCount = 0;

  // Track the prefetch's settled value WITHOUT ever awaiting it on the hot
  // path: `get_app_state` consults `seedSettled` synchronously, so a slow or
  // hung prefetch can never delay the first render — the WS invoke covers that.
  void seedPromise.then(
    (v) => {
      seedValue = v ?? null;
      seedSettled = true;
    },
    () => {
      seedSettled = true;
    },
  );

  const transport = new RemoteTransport({
    baseUrl: options.baseUrl,
    getToken: options.getToken,
    deps: options.deps,
    hooks: {
      onChannelMessage: dispatchChannel,
      onEvent: (event, payload) => {
        // A WS-delivered app-state snapshot supersedes the HTTP seed.
        if (event === APP_STATE_EVENT) wsAppStateSeq += 1;
        dispatchEvent(event, payload);
      },
      onStatusChange: handleStatusChange,
      onUnauthorized: () => options.onUnauthorized?.(),
    },
  });

  /** Forward connection-state changes to the caller's banner unchanged, and
   *  drive reconnect re-seeding. */
  function handleStatusChange(status: ConnectionStatus): void {
    if (status === "connected") {
      connectedCount += 1;
      // The first connect's snapshot is consumed by the app's own initial
      // `get_app_state` (seeded below). Every *re*connect refetches and pushes
      // the snapshot straight into the store to catch up on state that changed
      // while the socket was down — those events had no subscriber and were lost.
      if (connectedCount > 1) reseedOnReconnect();
    }
    options.onStatusChange?.(status);
  }

  /** On reconnect, refetch the snapshot and apply it via the same
   *  `app-state-changed` path the app already consumes — but only if no fresher
   *  WS snapshot lands while the refetch is in flight (no-stale guard). */
  function reseedOnReconnect(): void {
    if (!options.fetchSnapshot) return;
    const seqAtKickoff = wsAppStateSeq;
    void options.fetchSnapshot().then(
      (seed) => {
        if (seed != null && wsAppStateSeq === seqAtKickoff) {
          dispatchEvent(APP_STATE_EVENT, seed);
        }
      },
      () => {
        /* refetch failed — the resumed WS events self-heal the view */
      },
    );
  }

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
    // First `get_app_state`: answer from the HTTP-prefetched snapshot when it
    // is already in hand and no WS-delivered app-state has superseded it, so the
    // UI renders without a post-mount round-trip. Never awaits the prefetch — if
    // it isn't ready, fall through to the authoritative WS invoke (today's path).
    if (cmd === APP_STATE_COMMAND && !seedConsumed) {
      seedConsumed = true;
      if (seedSettled && seedValue != null && wsAppStateSeq === 0) {
        return seedValue;
      }
    }
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
