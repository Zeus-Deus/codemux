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
 * the session ends (close button, workspace delete, app shutdown). Workspace
 * switches and tab switches reparent the wrapper into a hidden parking node
 * but never call term.dispose() — the xterm keeps consuming PTY bytes the
 * whole time, so its mode flags / cursor / alt-screen state stay in sync
 * with the agent.
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
  /** Frame ID for the pending PTY-write batch. */
  ptyWriteFrame: number | null;
  pendingPtyWrites: Uint8Array[];
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
  return null;
}

function flushPtyWrites(entry: CachedTerminal) {
  entry.ptyWriteFrame = null;
  if (entry.disposed) return;
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

  let webglAddon: WebglAddon | null = null;
  try {
    webglAddon = new WebglAddon();
    webglAddon.onContextLoss(() => {
      webglAddon?.dispose();
      webglAddon = null;
    });
    terminal.loadAddon(webglAddon);
  } catch (err) {
    console.warn(
      "[Codemux] WebGL renderer unavailable, falling back to DOM",
      err,
    );
    webglAddon = null;
  }

  const entry: CachedTerminal = {
    sessionId,
    terminal,
    wrapperEl,
    fitAddon,
    serializeAddon,
    unicode11Addon,
    webglAddon,
    lastDims: { cols: 0, rows: 0 },
    paneId: options.paneId,
    disposed: false,
    ptyWriteFrame: null,
    pendingPtyWrites: [],
    kittyStack: [],
    kittyLevel: 0,
    cleanups: [],
  };

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
    scanKittyProtocol(entry, bytes);
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

  const { clientWidth, clientHeight } = containerEl;
  if (clientWidth > 0 && clientHeight > 0) {
    entry.fitAddon.fit();
  }
  return entry;
}

/**
 * Reparent the wrapperEl back to the parking node. The xterm and PTY
 * channel stay alive — bytes keep flowing into the wrapper while it's
 * parked, so workspace return shows the live frame.
 *
 * Idempotent.
 */
export function detachFromContainer(sessionId: string): void {
  const entry = cache.get(sessionId);
  if (!entry || entry.disposed) return;
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

  for (const cleanup of entry.cleanups) {
    try {
      cleanup();
    } catch (err) {
      console.error("[Codemux] terminal cache cleanup failed", err);
    }
  }
  entry.cleanups = [];

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
