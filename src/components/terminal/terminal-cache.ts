/**
 * Terminal cache: keeps xterm.js Terminal instances alive across React unmounts.
 *
 * The xterm Terminal, its addons, the persistent wrapper <div>, the PTY-output
 * channel attachment, and the input handlers all live in this module-level
 * cache keyed by sessionId. React's TerminalPane becomes a thin DOM-attach
 * wrapper that reparents the cached <div> in and out of its layout container.
 *
 * Lifetime invariant: the cache entry exists for the lifetime of the PTY
 * session. It is created on first mount (cold start) and disposed only when
 * the session ends (close button, workspace delete, app shutdown).
 *
 * Park / activate semantics: workspace switches reparent the wrapper into a
 * hidden parking node but NEVER call term.dispose(). While parked, the PTY
 * channel keeps streaming bytes from Rust — but instead of writing each chunk
 * straight into xterm (which would parse ANSI for an invisible buffer and
 * also drag the WebGL context along), we accumulate bytes in
 * `pendingParkedBytes`. The kitty keyboard state machine still ticks at
 * receive time so the per-PTY kitty stack stays in sync with the agent.
 *
 * On reactivate (attachToContainer when transitioning out of `parked`), we
 * concatenate the buffered bytes into a single terminal.write — xterm
 * processes them in order, so the final visible state is identical to what
 * you would see if writes had happened live. We also reload the WebGL addon
 * which we disposed on park (browsers cap concurrent WebGL contexts at ~16;
 * leaving them alive on every parked terminal is the typing-lag cliff this
 * caching strategy fell into in 0.1.30).
 *
 * Buffer cap: PARKED_BUFFER_CAP guards against pathological agents that
 * dump megabytes while the workspace sits parked. On overflow we drop the
 * oldest chunk — xterm's scrollback would discard those lines anyway and a
 * TUI repaint on next user interaction reconciles any lost frames.
 *
 * Design references:
 * - /tmp/terminal-rendering-analysis.md §5.1 (xterm-instance-lifetime split)
 * - Superset's apps/desktop/.../v1-terminal-cache.ts: same wrapper-div pattern
 *   for the same reason. We do not transplant; we reimplement for our React
 *   + Tauri stack.
 */
import { Terminal } from "@xterm/xterm";
import type { ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import {
  writeToPty,
  attachPtyOutput,
  getTerminalScrollback,
  cacheTerminalScrollback,
  uncacheTerminalScrollback,
  Channel,
  type ScrollbackPayload,
} from "@/tauri/commands";
import {
  scanKittySequences,
  applyKittyStack,
  kittyFlags,
} from "@/lib/kitty-keyboard";
import { useAppStore } from "@/stores/app-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { registerTerminalForSerialize } from "@/hooks/use-scrollback-serializer";

const PARKING_NODE_ID = "codemux-terminal-parking";

/**
 * Cap (in bytes) on how much PTY output we'll buffer for a single parked
 * terminal before we start dropping the oldest chunk. 16 MB is enormously
 * larger than any practical TUI redraw cycle yet small enough that a
 * runaway agent can't OOM the renderer process.
 *
 * Tunable via _setParkedBufferCapForTest from tests; runtime callers should
 * never touch it. Module-level `let` (not const) only for that reason.
 */
let PARKED_BUFFER_CAP = 16 * 1024 * 1024;

export interface TerminalCreateOptions {
  paneId: string | null;
  fontFamily: string;
  fontSize: number;
  cursorStyle: "bar" | "block" | "underline";
  theme: ITheme;
}

export interface CachedTerminal {
  sessionId: string;
  terminal: Terminal;
  wrapperEl: HTMLDivElement;
  fitAddon: FitAddon;
  serializeAddon: SerializeAddon;
  unicode11Addon: Unicode11Addon;
  webglAddon: WebglAddon | null;
  /** Last cols/rows pushed to the backend; used for resize no-op detection. */
  lastDims: { cols: number; rows: number };
  /** Latest pane ID this session is rendered under. Updated on each attach
   *  because the custom key handler (interrupt-clears) consults pane status. */
  paneId: string | null;
  /** Cleared by disposeTerminal so RAFs queued from the channel callback
   *  short-circuit instead of writing into a disposed Terminal. */
  disposed: boolean;
  /** True when the wrapper has been moved to the parking node and the entry
   *  should buffer PTY bytes instead of writing them. Flipped by
   *  detachFromContainer / attachToContainer. */
  parked: boolean;
  /** Frame ID for the pending PTY-write batch (only used while not parked). */
  ptyWriteFrame: number | null;
  pendingPtyWrites: Uint8Array[];
  /** Bytes received from the PTY while parked, awaiting drain on next
   *  attachToContainer. Capped at PARKED_BUFFER_CAP bytes total — see the
   *  module header. */
  pendingParkedBytes: Uint8Array[];
  pendingParkedSize: number;
  /** Set whenever enqueueParkedBytes had to drop a chunk because the cap was
   *  exceeded. Logged on drain so a regression in the cap surfaces in
   *  devtools instead of silently shaving lines off scrollback. */
  parkedOverflow: boolean;
  /** Kitty keyboard protocol stack (push/pop/reset).
   *  Stays per-session because the agent-side stack is per-PTY. */
  kittyStack: number[];
  kittyLevel: number;
  /** Disposers for resources the cache owns (input handler, serialize
   *  registration). Called from disposeTerminal. */
  cleanups: Array<() => void>;
}

const cache = new Map<string, CachedTerminal>();

function ensureParkingNode(): HTMLElement {
  let node = document.getElementById(PARKING_NODE_ID);
  if (!node) {
    node = document.createElement("div");
    node.id = PARKING_NODE_ID;
    node.style.position = "absolute";
    node.style.width = "0";
    node.style.height = "0";
    node.style.overflow = "hidden";
    node.style.visibility = "hidden";
    node.style.pointerEvents = "none";
    document.body.appendChild(node);
  }
  return node;
}

function extractBytes(payload: unknown): Uint8Array | null {
  if (payload instanceof Uint8Array) return payload;
  if (payload instanceof ArrayBuffer) return new Uint8Array(payload);
  if (Array.isArray(payload)) return new Uint8Array(payload as number[]);
  if (typeof payload === "string") return new TextEncoder().encode(payload);
  // Cross-realm typed-array fallback. The `instanceof Uint8Array` check above
  // returns false when payload was constructed in a different module realm
  // (Vitest's mock isolation hits this; a future Tauri Channel that crossed
  // a worker boundary could too). ArrayBuffer.isView is realm-agnostic.
  if (
    payload &&
    typeof payload === "object" &&
    ArrayBuffer.isView(payload as ArrayBufferView)
  ) {
    const view = payload as ArrayBufferView;
    return new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
  }
  return null;
}

/**
 * Load the WebGL renderer onto an open xterm. Safe no-op if it's already
 * loaded. Failure to load (jsdom, no GPU, blocked context) is swallowed —
 * xterm falls back to its DOM renderer transparently.
 */
function loadWebglAddon(entry: CachedTerminal): void {
  if (entry.webglAddon || entry.disposed) return;
  try {
    const addon = new WebglAddon();
    addon.onContextLoss(() => {
      try {
        addon.dispose();
      } catch {
        // ignore — context-loss already torpedoed it
      }
      if (entry.webglAddon === addon) entry.webglAddon = null;
    });
    entry.terminal.loadAddon(addon);
    entry.webglAddon = addon;
  } catch (err) {
    console.warn(
      "[Codemux] WebGL renderer unavailable, falling back to DOM",
      err,
    );
    entry.webglAddon = null;
  }
}

/**
 * Tear down the WebGL renderer. Called on park (so the GPU context is freed
 * for the active workspace's terminals) and on dispose. xterm reverts to its
 * DOM renderer automatically.
 */
function unloadWebglAddon(entry: CachedTerminal): void {
  if (!entry.webglAddon) return;
  try {
    entry.webglAddon.dispose();
  } catch (err) {
    console.error("[Codemux] webgl dispose failed", err);
  }
  entry.webglAddon = null;
}

/**
 * Append a PTY chunk to the parked buffer, dropping the oldest chunks if the
 * total would exceed PARKED_BUFFER_CAP. Sets parkedOverflow on first drop so
 * drainParkedBytes can warn.
 */
function enqueueParkedBytes(entry: CachedTerminal, bytes: Uint8Array): void {
  entry.pendingParkedBytes.push(bytes);
  entry.pendingParkedSize += bytes.length;
  while (
    entry.pendingParkedSize > PARKED_BUFFER_CAP &&
    entry.pendingParkedBytes.length > 1
  ) {
    const dropped = entry.pendingParkedBytes.shift();
    if (!dropped) break;
    entry.pendingParkedSize -= dropped.length;
    entry.parkedOverflow = true;
  }
}

/**
 * Concatenate every buffered chunk into one Uint8Array and write it to xterm
 * in a single call. xterm's parser handles large batched writes far better
 * than many small ones — this is the whole point of the buffer.
 *
 * No-op when the queue is empty.
 */
function drainParkedBytes(entry: CachedTerminal): void {
  // Defensive re-check: disposeTerminal could have been invoked between
  // attachToContainer's top-of-function disposed guard and now (e.g. the
  // terminal-cache GC reacting to a session vanishing from app-state). xterm
  // throws on write-after-dispose, so bail before we do that.
  if (entry.disposed) return;
  if (entry.pendingParkedBytes.length === 0) {
    entry.parkedOverflow = false;
    return;
  }
  const totalLen = entry.pendingParkedSize;
  const combined = new Uint8Array(totalLen);
  let offset = 0;
  for (const chunk of entry.pendingParkedBytes) {
    combined.set(chunk, offset);
    offset += chunk.length;
  }
  const overflowed = entry.parkedOverflow;
  entry.pendingParkedBytes = [];
  entry.pendingParkedSize = 0;
  entry.parkedOverflow = false;
  entry.terminal.write(combined);
  if (overflowed) {
    console.warn(
      `[Codemux] parked terminal ${entry.sessionId} exceeded buffer cap; oldest scrollback dropped`,
    );
  }
}

function flushPtyWrites(entry: CachedTerminal) {
  entry.ptyWriteFrame = null;
  if (entry.disposed) return;
  // Defensive: detachFromContainer cancels the queued RAF, but if the cancel
  // raced with an already-running frame (rare — single-threaded JS makes
  // this hard) we'd otherwise write into a parked terminal whose WebGL
  // context has been torn down. Move the bytes into the parked queue so the
  // next reactivate still drains them.
  if (entry.parked) {
    if (entry.pendingPtyWrites.length > 0) {
      for (const chunk of entry.pendingPtyWrites) {
        enqueueParkedBytes(entry, chunk);
      }
      entry.pendingPtyWrites = [];
    }
    return;
  }
  const pending = entry.pendingPtyWrites;
  if (pending.length === 0) return;

  if (pending.length === 1) {
    entry.terminal.write(pending[0]);
  } else {
    let totalLen = 0;
    for (const chunk of pending) totalLen += chunk.length;
    const combined = new Uint8Array(totalLen);
    let offset = 0;
    for (const chunk of pending) {
      combined.set(chunk, offset);
      offset += chunk.length;
    }
    entry.terminal.write(combined);
  }
  entry.pendingPtyWrites = [];
}

function scanKittyProtocol(entry: CachedTerminal, data: Uint8Array) {
  const decoded = new TextDecoder("utf-8", { fatal: false }).decode(data);
  const scan = scanKittySequences(decoded);

  if (scan.hasQuery) {
    writeToPty(
      entry.sessionId,
      `\x1b[?${kittyFlags(entry.kittyStack)}u`,
    ).catch(console.error);
  }
  entry.kittyStack = applyKittyStack(
    entry.kittyStack,
    scan.pushValues,
    scan.popCount,
    scan.hasDAQuery,
  );
  entry.kittyLevel = kittyFlags(entry.kittyStack);
}

function buildScrollbackPayload(
  entry: CachedTerminal,
): ScrollbackPayload | null {
  const t = entry.terminal;
  const sa = entry.serializeAddon;
  const sid = entry.sessionId;

  const appState = useAppStore.getState().appState;
  if (!appState) return null;

  const session = appState.terminal_sessions.find(
    (s) => s.session_id === sid,
  );
  if (!session) return null;

  const workspace = appState.workspaces.find((ws) =>
    ws.surfaces.some((surf) => {
      const json = JSON.stringify(surf.root);
      return json.includes(sid);
    }),
  );

  const scrollbackLines =
    useSyncedSettingsStore.getState().settings.session_restore.scrollback_lines;
  const data = sa.serialize({ scrollback: scrollbackLines });

  return {
    pane_id: entry.paneId ?? sid,
    session_id: sid,
    workspace_id: workspace?.workspace_id ?? "",
    working_directory: session.cwd,
    original_command: session.original_command,
    cols: session.cols,
    rows: session.rows,
    data,
    adapter_captures: session.adapter_captures ?? {},
    adapter_id: null,
    alternate_buffer: t.buffer.active.type === "alternate",
  };
}

/**
 * Construct a new xterm Terminal, load addons, and wire up the input handler
 * + PTY-output channel. Returns a CachedTerminal whose wrapperEl lives in the
 * parking node until attachToContainer reparents it.
 */
function createCachedTerminal(
  sessionId: string,
  options: TerminalCreateOptions,
): CachedTerminal {
  const wrapperEl = document.createElement("div");
  wrapperEl.className = "codemux-terminal-wrapper";
  wrapperEl.style.width = "100%";
  wrapperEl.style.height = "100%";
  ensureParkingNode().appendChild(wrapperEl);

  const terminal = new Terminal({
    fontFamily: options.fontFamily,
    theme: options.theme,
    convertEol: false,
    cursorBlink: true,
    cursorWidth: 2,
    lineHeight: 1.15,
    letterSpacing: 0,
    fontSize: options.fontSize,
    cursorStyle: options.cursorStyle,
    altClickMovesCursor: true,
    // Required so Unicode11Addon.activate() can call terminal.unicode.register
    // — `unicode.activeVersion = "11"` is a proposed API in xterm 6.x.
    allowProposedApi: true,
  });

  const fitAddon = new FitAddon();
  const serializeAddon = new SerializeAddon();
  const unicode11Addon = new Unicode11Addon();

  terminal.loadAddon(unicode11Addon);
  terminal.unicode.activeVersion = "11";
  terminal.loadAddon(fitAddon);
  terminal.loadAddon(serializeAddon);

  // xterm needs to be open()'d into a real DOM element so its renderer can
  // measure cells. Even while parked the wrapper is in the body, so this
  // works. WebGL is loaded lazily after open() returns.
  terminal.open(wrapperEl);

  const entry: CachedTerminal = {
    sessionId,
    terminal,
    wrapperEl,
    fitAddon,
    serializeAddon,
    unicode11Addon,
    webglAddon: null,
    lastDims: { cols: 0, rows: 0 },
    paneId: options.paneId,
    disposed: false,
    parked: false,
    ptyWriteFrame: null,
    pendingPtyWrites: [],
    pendingParkedBytes: [],
    pendingParkedSize: 0,
    parkedOverflow: false,
    kittyStack: [],
    kittyLevel: 0,
    cleanups: [],
  };

  // Cold-start WebGL load. Done after the entry exists so the load helper
  // can store the addon onto entry.webglAddon directly.
  loadWebglAddon(entry);

  // ── User input handler ──
  // Stable: sessionId never changes for a given cache entry.
  let pendingInput = "";
  let inputQueued = false;
  const dataDisposable = terminal.onData((data) => {
    if (entry.disposed) return;
    pendingInput += data;
    if (!inputQueued) {
      inputQueued = true;
      queueMicrotask(() => {
        const batch = pendingInput;
        pendingInput = "";
        inputQueued = false;
        writeToPty(sessionId, batch).catch((err) => {
          console.error(`Failed to write to PTY for ${sessionId}:`, err);
        });
      });
    }
  });
  entry.cleanups.push(() => dataDisposable.dispose());

  // ── Serialization registration ──
  // Lives for the cache entry's lifetime (not the React mount), so unmounted-
  // but-cached panes still serialize on app close.
  const unregisterSerialize = registerTerminalForSerialize(sessionId, () =>
    buildScrollbackPayload(entry),
  );
  entry.cleanups.push(unregisterSerialize);

  return entry;
}

/**
 * Wire up the PTY → xterm channel for a freshly-created cache entry. This
 * runs once per session (cold start). Stays attached for the entire session
 * lifetime — workspace switches do NOT detach.
 */
async function attachPtyChannel(entry: CachedTerminal): Promise<void> {
  const channel = new Channel<unknown>((payload) => {
    if (entry.disposed) return;
    const bytes = extractBytes(payload);
    if (!bytes) return;
    // Kitty stack has to advance at receive time regardless of park state —
    // the input handler reads kittyLevel synchronously on every keystroke and
    // a stale stack would send the wrong CSI-u sequences.
    scanKittyProtocol(entry, bytes);
    if (entry.parked) {
      enqueueParkedBytes(entry, bytes);
      return;
    }
    entry.pendingPtyWrites.push(bytes);
    if (entry.ptyWriteFrame === null) {
      entry.ptyWriteFrame = requestAnimationFrame(() => flushPtyWrites(entry));
    }
  });
  await attachPtyOutput(entry.sessionId, channel);
}

/**
 * Restore on-disk scrollback into a freshly-created xterm. Skips replay if
 * the saved buffer was alt-screen (TUI) — fresh shell is more useful than a
 * frozen frame of vim.
 */
async function restoreScrollback(entry: CachedTerminal): Promise<void> {
  const restoreEnabled =
    useSyncedSettingsStore.getState().settings.session_restore.enabled;
  if (!restoreEnabled) return;

  const appState = useAppStore.getState().appState;
  const workspace = appState?.workspaces.find((ws) =>
    ws.surfaces.some((surf) => {
      const json = JSON.stringify(surf.root);
      return json.includes(entry.sessionId);
    }),
  );
  if (!workspace || !entry.paneId) return;

  try {
    const scrollback = await getTerminalScrollback(
      workspace.workspace_id,
      entry.paneId,
    );
    if (!scrollback || entry.disposed) return;
    if (!scrollback.meta.alternate_buffer && scrollback.data) {
      entry.terminal.write(scrollback.data);
      entry.terminal.write(
        "\r\n\x1b[2m── session restored ──\x1b[0m\r\n\r\n",
      );
    }
  } catch {
    // Restore failed — fresh shell is still usable
  }
}

/**
 * Get an existing cache entry or create + initialize a new one.
 *
 * On cold start (cache miss) this also restores disk scrollback and attaches
 * the PTY-output channel. On warm reuse, it returns the existing entry —
 * the caller just needs to attachToContainer.
 *
 * `isNew` lets the caller distinguish the two paths so it can show a status
 * overlay only on cold start.
 */
export async function getOrCreateTerminal(
  sessionId: string,
  options: TerminalCreateOptions,
): Promise<{ entry: CachedTerminal; isNew: boolean }> {
  const existing = cache.get(sessionId);
  if (existing) {
    existing.paneId = options.paneId;
    return { entry: existing, isNew: false };
  }

  const entry = createCachedTerminal(sessionId, options);
  cache.set(sessionId, entry);

  // Clear stale on-disk-cached bytes (we are about to attach live)
  uncacheTerminalScrollback(sessionId).catch(() => {});

  try {
    await restoreScrollback(entry);
    if (entry.disposed) return { entry, isNew: true };
    await attachPtyChannel(entry);
  } catch (err) {
    // Init failed partway — tear the half-formed entry down so the next
    // mount retries cold-start instead of finding a stuck cache hit.
    if (cache.get(sessionId) === entry) {
      disposeTerminal(sessionId);
    }
    throw err;
  }
  return { entry, isNew: true };
}

/**
 * Reparent the cached wrapperEl into the live container and fit the terminal
 * to its new dimensions. No-ops if the container has zero dims (it hasn't
 * laid out yet) — the caller's ResizeObserver will fit once it does.
 */
export function attachToContainer(
  sessionId: string,
  containerEl: HTMLElement,
): CachedTerminal | null {
  const entry = cache.get(sessionId);
  if (!entry || entry.disposed) return null;

  if (entry.wrapperEl.parentElement !== containerEl) {
    containerEl.appendChild(entry.wrapperEl);
  }

  // Transitioning out of parked: reload WebGL (we dropped the GPU context on
  // detach) and drain the buffered PTY bytes into a single xterm.write so the
  // visible frame catches up to the live stream.
  const wasParked = entry.parked;
  entry.parked = false;
  if (wasParked) {
    loadWebglAddon(entry);
    drainParkedBytes(entry);
  }

  const { clientWidth, clientHeight } = containerEl;
  if (clientWidth > 0 && clientHeight > 0) {
    entry.fitAddon.fit();
  }
  return entry;
}

/**
 * Reparent the wrapperEl back to the parking node and put the entry into
 * parked mode: PTY bytes will be buffered (not written to xterm) and the
 * WebGL renderer is torn down to free the GPU context.
 *
 * Any pending RAF batch is canceled and its bytes migrated into the parked
 * queue so we don't lose output mid-flush.
 *
 * Idempotent — calling on an already-parked entry is a no-op.
 */
export function detachFromContainer(sessionId: string): void {
  const entry = cache.get(sessionId);
  if (!entry || entry.disposed) return;
  if (entry.parked) return;

  entry.parked = true;

  // Cancel the in-flight RAF; its bytes have to go into the parked queue
  // because once we unload WebGL we don't want them rendered until reactivate.
  if (entry.ptyWriteFrame !== null) {
    cancelAnimationFrame(entry.ptyWriteFrame);
    entry.ptyWriteFrame = null;
  }
  if (entry.pendingPtyWrites.length > 0) {
    for (const chunk of entry.pendingPtyWrites) {
      enqueueParkedBytes(entry, chunk);
    }
    entry.pendingPtyWrites = [];
  }

  unloadWebglAddon(entry);

  const parking = ensureParkingNode();
  if (entry.wrapperEl.parentElement !== parking) {
    parking.appendChild(entry.wrapperEl);
  }
}

/**
 * Dispose the cache entry. Call this when the session itself ends (close
 * button, workspace deletion, app shutdown) — NOT on component unmount.
 *
 * Caches the final scrollback to the Rust backstop so app-close-while-the-
 * pane-is-mid-dispose still gets persisted on disk.
 */
export function disposeTerminal(sessionId: string): void {
  const entry = cache.get(sessionId);
  if (!entry) return;

  // Best-effort persistence of final state. We're not awaiting this — the
  // session is going away regardless and Rust accepts a fire-and-forget cache.
  const restoreEnabled =
    useSyncedSettingsStore.getState().settings.session_restore.enabled;
  if (restoreEnabled) {
    const payload = buildScrollbackPayload(entry);
    if (payload && payload.data) {
      cacheTerminalScrollback(payload).catch(() => {});
    }
  }

  entry.disposed = true;
  cache.delete(sessionId);

  if (entry.ptyWriteFrame !== null) {
    cancelAnimationFrame(entry.ptyWriteFrame);
    entry.ptyWriteFrame = null;
  }
  entry.pendingPtyWrites = [];
  entry.pendingParkedBytes = [];
  entry.pendingParkedSize = 0;
  entry.parkedOverflow = false;

  for (const cleanup of entry.cleanups) {
    try {
      cleanup();
    } catch (err) {
      console.error("[Codemux] terminal cache cleanup failed", err);
    }
  }
  entry.cleanups = [];

  // Drop the WebGL context before tearing down the terminal so the GPU
  // resource is released even if terminal.dispose's internal addon cleanup
  // misses it.
  unloadWebglAddon(entry);

  try {
    entry.terminal.dispose();
  } catch (err) {
    console.error("[Codemux] terminal.dispose failed", err);
  }

  if (entry.wrapperEl.parentElement) {
    entry.wrapperEl.parentElement.removeChild(entry.wrapperEl);
  }
}

/**
 * Apply a fresh theme to every cached terminal. Called by the app-level
 * theme observer so unmounted-but-cached panes still pick up theme changes
 * — without this, switching theme while a pane is parked would leave it
 * stuck on the old palette until the user changed theme a second time.
 */
export function applyThemeToAllTerminals(theme: ITheme): void {
  for (const entry of cache.values()) {
    if (entry.disposed) continue;
    try {
      entry.terminal.options.theme = theme;
    } catch (err) {
      console.error("[Codemux] failed to apply theme to cached terminal", err);
    }
  }
}

/** Test/debug helper: peek at the cache without leaking the Map. */
export function hasCachedTerminal(sessionId: string): boolean {
  return cache.has(sessionId);
}

/** Test/debug helper: read cache entry for identity assertions. */
export function peekCachedTerminal(sessionId: string): CachedTerminal | null {
  return cache.get(sessionId) ?? null;
}

/** Test-only helper: count of live cache entries. */
export function _cacheSize(): number {
  return cache.size;
}

/**
 * Test-only helper: override the parked-buffer cap so overflow behavior can
 * be exercised without pumping 16 MB through a mock channel. Pass null to
 * restore the production default.
 */
export function _setParkedBufferCapForTest(cap: number | null): void {
  PARKED_BUFFER_CAP = cap ?? 16 * 1024 * 1024;
}
