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
 * hidden parking node but NEVER call term.dispose(). Unlike the first
 * v0.1.30 persistence implementation, parking DOES detach the PTY output
 * channel so hidden workspaces do not keep pushing bytes through Tauri IPC
 * and xterm parsing on the renderer main thread. On activate we reattach the
 * channel and Rust replays pending_output into the SAME xterm instance, so
 * mode flags / cursor / alt-screen state still advance from the correct prior
 * state instead of replaying into a fresh terminal.
 *
 * Phantom query replies (the `?62;...` glitch class that 0.1.30 was working
 * around) are prevented at the xterm parser layer via suppressQueryResponses()
 * — see ./query-suppression.ts.
 *
 * Design references:
 * - v0.1.29 TerminalPane: detach PTY output on unmount for low input latency
 * - Superset's apps/desktop/.../v1-terminal-cache.ts: same wrapper-div pattern
 */
import { Terminal } from "@xterm/xterm";
import type { ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import {
  writeToPty,
  attachPtyOutput,
  detachPtyOutput,
  pausePtyOutput,
  resumePtyOutput,
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
import { useAppStore, getSessionWorkspaceId } from "@/stores/app-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { registerTerminalForSerialize } from "@/hooks/use-scrollback-serializer";
import { suppressQueryResponses } from "./query-suppression";

const PARKING_NODE_ID = "codemux-terminal-parking";

/**
 * Terminal output flow control watermarks.
 *
 * `writeQueue` holds PTY bytes received over IPC but not yet handed to xterm
 * (the throttled `pumpWrites` drains ~one chunk per macrotask to keep the
 * main thread responsive). Under a sustained flood — `yes`, `cat huge-file`,
 * a verbose build, a runaway agent — a fast producer outruns that drain and
 * the queue would grow without bound, ballooning renderer memory.
 *
 * When the queued byte count crosses HIGH we ask the backend to pause the
 * PTY read loop; the child then blocks on `write()` once the kernel PTY
 * buffer fills (real backpressure to the producer). We resume once the queue
 * drains below LOW. The watermarks are deliberately generous so ordinary
 * bursts and the multi-MB reattach-replay (already spread out by the pump)
 * never trip backpressure — only genuine floods do.
 *
 * Backpressure is enforced only on the daemon spawn path; in-process sessions
 * treat pause/resume as a no-op and keep their prior behavior. The backend
 * carries its own fail-safes (resume on attach/close + a max-park backstop)
 * so a dropped resume can never wedge a PTY.
 */
const FLOW_HIGH_WATERMARK_BYTES = 16 * 1024 * 1024; // 16 MiB
const FLOW_LOW_WATERMARK_BYTES = 4 * 1024 * 1024; // 4 MiB

/**
 * One shared TextDecoder for every channel callback for every session.
 * Allocating a fresh `new TextDecoder()` per chunk was measurable main-thread
 * overhead under load. With `stream: false` (the default for `decode()`),
 * each call is independent — no cross-call state to worry about.
 */
const SHARED_DECODER = new TextDecoder("utf-8", { fatal: false });

function maybeKittyOrDA(data: Uint8Array): boolean {
  const last = data.length - 2;
  for (let i = 0; i < last; i++) {
    if (data[i] === 0x1b && data[i + 1] === 0x5b) {
      const c = data[i + 2];
      if (c === 0x3e || c === 0x3c || c === 0x3f || c === 0x63) return true;
    }
  }
  return false;
}

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
  /** Last cols/rows pushed to the backend; used for resize no-op detection. */
  lastDims: { cols: number; rows: number };
  /** Latest pane ID this session is rendered under. Updated on each attach
   *  because the custom key handler (interrupt-clears) consults pane status. */
  paneId: string | null;
  /** Cleared by disposeTerminal so a late channel callback short-circuits
   *  instead of writing into a disposed Terminal. */
  disposed: boolean;
  /** True when the wrapper has been moved to the parking node. Bytes still
   *  flow into xterm regardless — this is purely a DOM-location signal. */
  parked: boolean;
  /** True after Rust has a live output_channel for this session. */
  outputAttached: boolean;
  /** In-flight attach promise to avoid duplicate attach_pty_output calls. */
  outputAttachPromise: Promise<void> | null;
  /** Incremented on detach/dispose so stale async attach completions are ignored. */
  outputAttachEpoch: number;
  /** Kitty keyboard protocol stack (push/pop/reset).
   *  Stays per-session because the agent-side stack is per-PTY. */
  kittyStack: number[];
  kittyLevel: number;
  /** Pending PTY-output chunks queued from the channel callback. Drained
   *  by `pumpWrites` which yields to the macrotask queue between writes
   *  so a multi-MB pending_output replay on workspace reattach can't
   *  monopolise the main thread. Without throttling, Rust replays the
   *  entire backlog (capped at 256 MiB by `OUTPUT_BUFFER_BYTE_LIMIT`)
   *  through thousands of back-to-back channel sends; the renderer then
   *  parses thousands of IPC payloads + xterm escape sequences in one
   *  go and freezes hard with input dead until parsing finishes. */
  writeQueue: Uint8Array[];
  /** Running total of `writeQueue` chunk byte lengths. Kept in lockstep with
   *  the queue (push adds, pump-drain subtracts, dispose resets) so the
   *  flow-control watermark check is O(1) instead of summing the queue. */
  writeQueueBytes: number;
  /** True when we've asked the backend to pause this session's PTY read loop
   *  because `writeQueueBytes` crossed the HIGH watermark. Cleared when the
   *  queue drains below LOW, or on detach/dispose (so a parked pane never
   *  leaves a background agent blocked on write). */
  flowPaused: boolean;
  /** True while `pumpWrites` is iterating. Re-entrant pump kicks become
   *  no-ops; the running pump picks up newly queued chunks on its next
   *  iteration. */
  writePumpRunning: boolean;
  /** Disposers for resources the cache owns (input handler, serialize
   *  registration, query-suppression handlers). Called from disposeTerminal. */
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

function scanKittyProtocol(entry: CachedTerminal, data: Uint8Array) {
  if (!maybeKittyOrDA(data)) return;
  const decoded = SHARED_DECODER.decode(data);
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

  // O(1) reverse-index lookup; see `buildSessionWorkspaceIndex` in
  // `app-store.ts`.
  const workspaceId = getSessionWorkspaceId(sid);

  const scrollbackLines =
    useSyncedSettingsStore.getState().settings.session_restore.scrollback_lines;
  const data = sa.serialize({ scrollback: scrollbackLines });

  return {
    pane_id: entry.paneId ?? sid,
    session_id: sid,
    workspace_id: workspaceId ?? "",
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
    lastDims: { cols: 0, rows: 0 },
    paneId: options.paneId,
    disposed: false,
    parked: true,
    outputAttached: false,
    outputAttachPromise: null,
    outputAttachEpoch: 0,
    kittyStack: [],
    kittyLevel: 0,
    writeQueue: [],
    writeQueueBytes: 0,
    flowPaused: false,
    writePumpRunning: false,
    cleanups: [],
  };

  const cleanupQuerySuppression = suppressQueryResponses(terminal);
  entry.cleanups.push(cleanupQuerySuppression);

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
 * Drain `entry.writeQueue` into `entry.terminal.write`, yielding to the
 * macrotask queue between chunks so the main thread can interleave input
 * handling, paint, and xterm's internal parser tick.
 *
 * Why this exists: on workspace reattach, Rust replays the session's
 * `pending_output` ring (capped at `OUTPUT_BUFFER_BYTE_LIMIT` = 256 MiB) by
 * firing a channel send for each ~32 KiB chunk in the deque. Without
 * throttling, the channel callback ran `terminal.write` synchronously for
 * thousands of chunks in succession; the renderer pegged on Tauri IPC
 * dispatch + xterm parser work for several seconds, input died, and the
 * webview hit ~1 GB RSS before recovering. setTimeout(0) is the cheapest
 * yield that crosses a macrotask boundary, which is what the browser uses
 * to schedule input/paint between long-running JS tasks.
 *
 * Re-entry: the pump is single-flight per entry (`writePumpRunning`).
 * Any callback that queues bytes during an active drain just appends to
 * `writeQueue`; the running pump picks them up on its next iteration.
 */
async function pumpWrites(entry: CachedTerminal): Promise<void> {
  if (entry.writePumpRunning) return;
  entry.writePumpRunning = true;
  try {
    while (entry.writeQueue.length > 0 && !entry.disposed) {
      const next = entry.writeQueue.shift();
      if (!next) continue;
      // The chunk leaves our queue here; account for it before writing so
      // the flow-control watermark reflects only bytes still waiting on us.
      entry.writeQueueBytes -= next.length;
      if (entry.writeQueueBytes < 0) entry.writeQueueBytes = 0;
      entry.terminal.write(next);
      maybeReleaseBackpressure(entry);
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
  } finally {
    entry.writePumpRunning = false;
  }
}

/**
 * Ask the backend to pause this session's PTY read loop when the write queue
 * has grown past the HIGH watermark. Called from the channel callback right
 * after a chunk is queued. Single-shot per pause cycle (guarded by
 * `flowPaused`).
 */
function maybeApplyBackpressure(entry: CachedTerminal): void {
  if (entry.flowPaused) return;
  if (entry.writeQueueBytes <= FLOW_HIGH_WATERMARK_BYTES) return;
  entry.flowPaused = true;
  pausePtyOutput(entry.sessionId).catch((err) => {
    // Pause didn't land (e.g. the session just exited). Drop the guard so a
    // later over-watermark push retries rather than us believing we paused.
    entry.flowPaused = false;
    console.error(
      `[Codemux] flow-control pause failed for ${entry.sessionId}:`,
      err,
    );
  });
}

/**
 * Release backpressure once the queue has drained below the LOW watermark.
 * Called from the pump after each chunk. On failure we still clear
 * `flowPaused` (fail open): the backend's attach/close/max-park fail-safes
 * guarantee the PTY resumes regardless, and a fresh flood will re-pause.
 */
function maybeReleaseBackpressure(entry: CachedTerminal): void {
  if (!entry.flowPaused) return;
  if (entry.writeQueueBytes >= FLOW_LOW_WATERMARK_BYTES) return;
  entry.flowPaused = false;
  resumePtyOutput(entry.sessionId).catch((err) => {
    console.error(
      `[Codemux] flow-control resume failed for ${entry.sessionId}:`,
      err,
    );
  });
}

/**
 * Wire up the PTY → xterm channel while the terminal is visible. Hidden panes
 * detach the backend channel to preserve v0.1.29 input latency; reattach uses
 * Rust's pending_output replay and writes into the same xterm instance.
 */
async function attachPtyChannel(entry: CachedTerminal): Promise<void> {
  if (entry.disposed || entry.outputAttached) return;
  if (entry.outputAttachPromise) return entry.outputAttachPromise;

  const epoch = entry.outputAttachEpoch;
  const channel = new Channel<unknown>((payload) => {
    if (entry.disposed) return;
    const bytes = extractBytes(payload);
    if (!bytes) return;
    // Kitty stack has to advance at receive time regardless of park state —
    // the input handler reads kittyLevel synchronously on every keystroke and
    // a stale stack would send the wrong CSI-u sequences. Runs before
    // queueing so query responses are dispatched in IPC-arrival order, not
    // delayed behind the throttled xterm-write pump.
    scanKittyProtocol(entry, bytes);
    // Queue + pump rather than calling terminal.write synchronously. See
    // pumpWrites for the rationale (reattach-replay freeze fix).
    entry.writeQueue.push(bytes);
    entry.writeQueueBytes += bytes.length;
    // Flow control: if a fast producer has outrun the pump, ask the backend
    // to pause the PTY so the queue can't grow without bound.
    maybeApplyBackpressure(entry);
    void pumpWrites(entry);
  });

  const promise = attachPtyOutput(entry.sessionId, channel)
    .then(() => {
      if (entry.disposed || entry.outputAttachEpoch !== epoch) {
        detachPtyOutput(entry.sessionId).catch(console.error);
        return;
      }
      entry.outputAttached = true;
    })
    .finally(() => {
      if (entry.outputAttachPromise === promise) {
        entry.outputAttachPromise = null;
      }
    });

  entry.outputAttachPromise = promise;
  return promise;
}

function detachPtyChannel(entry: CachedTerminal): void {
  entry.outputAttachEpoch += 1;
  const shouldDetach = entry.outputAttached || entry.outputAttachPromise !== null;
  entry.outputAttached = false;
  // If we had this session's PTY paused for flow control, resume it on the
  // way out. A parked/disposed pane keeps its daemon-backed agent running in
  // the background — leaving it paused would block that agent on write. (The
  // daemon also resumes on its own fail-safes; this just makes it immediate.)
  if (entry.flowPaused) {
    entry.flowPaused = false;
    resumePtyOutput(entry.sessionId).catch(console.error);
  }
  if (shouldDetach) {
    detachPtyOutput(entry.sessionId).catch(console.error);
  }
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

  // O(1) reverse-index lookup; see `buildSessionWorkspaceIndex` in
  // `app-store.ts`.
  const workspaceId = getSessionWorkspaceId(entry.sessionId);
  if (!workspaceId || !entry.paneId) return;

  try {
    const scrollback = await getTerminalScrollback(
      workspaceId,
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

  entry.parked = false;
  attachPtyChannel(entry).catch((err) => {
    console.error(`Failed to attach terminal output for ${sessionId}:`, err);
  });

  const { clientWidth, clientHeight } = containerEl;
  if (clientWidth > 0 && clientHeight > 0) {
    entry.fitAddon.fit();
  }
  return entry;
}

/**
 * Reparent the wrapperEl back to the parking node and detach the PTY output
 * channel. xterm stays alive; Rust buffers output until the next attach.
 *
 * Idempotent — calling on an already-parked entry is a no-op.
 */
export function detachFromContainer(sessionId: string): void {
  const entry = cache.get(sessionId);
  if (!entry || entry.disposed) return;
  if (entry.parked) {
    detachPtyChannel(entry);
    return;
  }

  entry.parked = true;
  detachPtyChannel(entry);
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
  detachPtyChannel(entry);
  // Drop any chunks still queued for the throttled write pump — the
  // pump's loop guard re-checks `disposed` between iterations and exits.
  entry.writeQueue.length = 0;
  entry.writeQueueBytes = 0;

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
