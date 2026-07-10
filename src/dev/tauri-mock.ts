/**
 * Dev-only Tauri runtime shim.
 *
 * Loaded EXCLUSIVELY by `main.tsx` under `npm run dev` (Vite serving the
 * frontend at localhost:1420 with no desktop window) when no real Tauri
 * runtime is present. The dual guard in `main.tsx`:
 *
 *     if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
 *       await import("./dev/tauri-mock");
 *     }
 *
 * guarantees two things:
 *   1. `import.meta.env.DEV` is statically `false` in production, so
 *      Rollup drops this dynamic-import branch entirely — `grep -r
 *      "tauri-mock" dist/` is empty after `npm run build`.
 *   2. The runtime `__TAURI_INTERNALS__` check keeps the shim dormant
 *      under `npm run tauri:dev`, where the real desktop WebView already
 *      injected `window.__TAURI_INTERNALS__`. So real-IPC behavior is
 *      byte-identical to today.
 *
 * How it works: every `@tauri-apps/api` surface — `invoke()`, the event
 * system (`listen`/`emit`/`once`), the window/app plugins, and the
 * `plugin-opener` / `plugin-updater` / `plugin-process` / `plugin-dialog`
 * wrappers — ultimately funnels through `window.__TAURI_INTERNALS__`.
 * We install a faithful enough `__TAURI_INTERNALS__` (plus the event
 * plugin internals) so the unmodified `@tauri-apps/api` code routes
 * every call into the command router below. No module aliasing needed.
 *
 * The command surface was enumerated once at implementation time (see
 * issue #40 §2). Boot-critical reads return hand-curated fixtures;
 * mutators update the in-memory `AppStateSnapshot` and re-emit
 * `app-state-changed` so the UI reflects the change; everything else
 * falls through to a logged, shape-safe default.
 */
import {
  MOCK_CHAT_THREAD_ID,
  MOCK_HOME_DIR,
  MOCK_USER,
  MOCK_WORKFLOW_APPROVAL_THREAD_ID,
  MOCK_WORKFLOW_COMPLETE_THREAD_ID,
  MOCK_WORKFLOW_RUNNING_THREAD_ID,
  createSeedAppState,
  richChatTurnEnvelopes,
  subagentTurnEnvelopes,
  workflowApprovalEnvelopes,
  workflowCompleteEnvelopes,
  workflowRunningEnvelopes,
} from "./mock-fixtures";
import type {
  ScrollbackMeta,
  ScrollbackPayload,
  ScrollbackRestore,
} from "@/tauri/commands";
import type {
  AppStateSnapshot,
  ChatModelInfo,
  CliToolInfo,
  FeatureFlags,
  PresetStoreSnapshot,
  TerminalPreset,
  ProviderChatCapabilities,
  ResourceMetricsSnapshot,
  ShellAppearance,
  ThemeColors,
  UserSettings,
  WorkspaceSnapshot,
} from "@/tauri/types";

console.log(
  "%c[dev mock] installed",
  "color:#f59e0b;font-weight:bold",
  "— Tauri IPC is mocked. Run `npm run tauri:dev` for real IPC.",
);

// ── Mutable seed state ──────────────────────────────────────────────

let appState: AppStateSnapshot = createSeedAppState();

function findWorkspace(id: unknown): WorkspaceSnapshot | undefined {
  return appState.workspaces.find((w) => w.workspace_id === id);
}

/** Re-emit the current snapshot so subscribers (`useAppStateInit`) pick
 *  up an in-memory mutation, mirroring the backend's `emit_app_state`. */
function emitAppState(): void {
  emitEvent("app-state-changed", appState);
}

// ── Event subsystem ─────────────────────────────────────────────────
//
// `@tauri-apps/api/event`'s `listen()` calls
// `invoke("plugin:event|listen", { event, target, handler })` where
// `handler` is an id returned by `transformCallback`. We keep the raw
// callbacks in `callbacks`, the per-event subscription map in
// `eventListeners`, and fan out on emit.

type RawCallback = (payload: unknown) => void;

const callbacks = new Map<number, { fn: RawCallback; once: boolean }>();
let nextCallbackId = 1;

// event name -> (eventId -> callbackId)
const eventListeners = new Map<string, Map<number, number>>();
let nextEventId = 1;

function transformCallback(fn: RawCallback, once = false): number {
  const id = nextCallbackId++;
  callbacks.set(id, { fn, once });
  return id;
}

function registerListener(event: string, callbackId: number): number {
  const eventId = nextEventId++;
  let map = eventListeners.get(event);
  if (!map) {
    map = new Map();
    eventListeners.set(event, map);
  }
  map.set(eventId, callbackId);
  return eventId;
}

function unregisterListener(event: string, eventId: number): void {
  eventListeners.get(event)?.delete(eventId);
}

/** Fan a payload out to every listener subscribed to `name`. Exposed to
 *  the rest of the mock so mutators can push `app-state-changed`. */
function emitEvent(name: string, payload: unknown): void {
  const map = eventListeners.get(name);
  if (!map) return;
  // Snapshot entries first — a once-listener mutates the map mid-loop.
  for (const [eventId, callbackId] of [...map.entries()]) {
    const cb = callbacks.get(callbackId);
    if (!cb) continue;
    try {
      cb.fn({ event: name, id: eventId, payload } as unknown as never);
    } catch (err) {
      console.error(`[dev mock] listener for "${name}" threw:`, err);
    }
    if (cb.once) {
      callbacks.delete(callbackId);
      map.delete(eventId);
    }
  }
}

// ── Terminal PTY output channels ────────────────────────────────────
//
// Each mounted TerminalPane registers a `@tauri-apps/api` `Channel` via
// `attach_pty_output`; live PTY bytes arrive as ordered
// `{ index, message }` frames pushed through that channel's
// `transformCallback` dispatcher (index MUST be sequential from 0 — the
// Channel's internal ordering buffer stalls on a gap). We keep a
// session→channel map so (a) the mount banner and (b) the on-demand
// flood (issue #128) can push through EVERY currently-attached pane,
// tracking `nextIndex` per channel so ordering stays intact across both.

interface MockPtyChannelEntry {
  /** The `@tauri-apps/api` Channel object passed by the frontend. */
  channel: unknown;
  /** Next ordered frame index for this channel (starts at 0 per attach:
   *  a fresh Channel instance has an empty ordering buffer). */
  nextIndex: number;
}

const ptyChannels = new Map<string, MockPtyChannelEntry>();

const MOCK_TERMINAL_BANNER =
  "\r\n  \x1b[2m(mock terminal — no PTY in plain-browser dev; " +
  "run `npm run tauri:dev` for a real shell)\x1b[0m\r\n";

/** Push one ordered frame through a tracked PTY channel. Best-effort:
 *  any shape mismatch / stale callback is swallowed so a terminal that
 *  renders differently never breaks boot. Returns whether it landed. */
function ptyChannelPush(entry: MockPtyChannelEntry, message: string): boolean {
  const id = (entry.channel as { id?: number } | undefined)?.id;
  if (typeof id !== "number") return false;
  const cb = callbacks.get(id);
  if (!cb) return false;
  // Consume the frame index only AFTER a successful dispatch: the
  // @tauri-apps/api Channel buffers frames by index and stalls forever on a
  // gap, so burning an index on a frame whose callback threw would wedge
  // every subsequent frame on this channel.
  const frame = { index: entry.nextIndex, message };
  try {
    cb.fn(frame as unknown as never);
    entry.nextIndex++;
    return true;
  } catch {
    return false;
  }
}

// ── Agent chat: per-thread Channel streaming simulator ─────────────
//
// Mirrors the issue #75 backend: a chat pane registers a
// `tauri::ipc::Channel` per thread via `attach_agent_chat_output`,
// and live runtime events — crucially the `content_delta` token
// stream — arrive over that channel only (never the global event
// bus). The mock keeps a thread→channel map and pushes ordered
// `{ index, message }` frames through the channel's
// `transformCallback` dispatcher, exactly like the real IPC layer, so
// the unmodified `@tauri-apps/api` Channel machinery (ordering buffer
// included) runs for real in browser dev.

interface MockChatChannelEntry {
  /** The `@tauri-apps/api` Channel object passed by the frontend. */
  channel: unknown;
  /** Attach generation; detach only removes on match (issue #75). */
  generation: number;
  /** Next ordered frame index for this channel. */
  nextIndex: number;
}

const chatChannels = new Map<string, MockChatChannelEntry>();
let chatChannelGeneration = 0;

function chatChannelPush(entry: MockChatChannelEntry, payload: unknown): void {
  const id = (entry.channel as { id?: number } | undefined)?.id;
  if (typeof id !== "number") return;
  const cb = callbacks.get(id);
  if (!cb) return;
  try {
    cb.fn({ index: entry.nextIndex++, message: payload } as unknown as never);
  } catch (err) {
    console.error("[dev mock] chat channel push threw:", err);
  }
}

/** Route one runtime event to the thread's attached channel — the
 *  mock twin of `forward_event`'s channel arm. No channel → drop. */
function emitChatEvent(threadId: string, event: unknown): void {
  const entry = chatChannels.get(threadId);
  if (!entry) return;
  chatChannelPush(entry, { thread_id: threadId, event });
}

// The turn simulator itself (`streamMockChatReply`) lives in the
// "Agent chat (mocked end-to-end)" section below — it routes every
// frame through `emitChatEvent`, so the channel is the only live
// transport, exactly like the real backend.

// ── Terminal scrollback stores (issue #128) ─────────────────────────
//
// Faithful twin of `src-tauri/src/scrollback.rs`: TWO distinct stores.
//
//   (a) An in-memory CACHE keyed by `session_id`. A TerminalPane puts
//       its serialized buffer here on unmount (`cache_terminal_scrollback`)
//       and removes it on the next mount (`uncache_terminal_scrollback`).
//   (b) A "disk" store keyed by `(workspace_id, pane_id)`. Written
//       directly by `save_terminal_scrollback` (live panes on close) and
//       by `flush_scrollback_cache` DRAINING the cache. Read ONLY by
//       `get_terminal_scrollback` — the cache is never read directly,
//       matching the backend.
//
// Every handler emits a `[mock::scrollback]` console line so the close /
// restore dance is observable end-to-end in a plain browser (this is what
// the issue #128 e2e run asserts on).

/** A "disk" entry: the stored payload plus the epoch-ms save time used
 *  to synthesize `ScrollbackMeta.saved_at` on read (the backend stamps
 *  this at write time in `save_scrollback`). */
interface MockScrollbackDiskEntry {
  payload: ScrollbackPayload;
  savedAt: number;
}

const scrollbackCache = new Map<string, ScrollbackPayload>();
const scrollbackDisk = new Map<string, MockScrollbackDiskEntry>();

const diskKey = (workspaceId: string, paneId: string): string =>
  `${workspaceId}::${paneId}`;

/** First 8 chars of an id — matches the backend log convention and keeps
 *  the observability lines compact but greppable. */
const shortId = (id: string): string => id.slice(0, 8);

/** Approximate byte size for the `bytes=` log fields: the JS (UTF-16)
 *  string length, not an encoded byte count. Terminal payloads are
 *  ASCII-dominant, so length ≈ UTF-8 bytes — and exactness isn't worth a
 *  full O(n) TextEncoder copy of a multi-MB payload on the timed
 *  unmount/switch path these log lines sit on. */
const approxByteLen = (s: string): number => s.length;

/** Last ~40 chars of `data`, JSON-escaped (quotes included) so control
 *  bytes like `\r\n` render as escapes in one greppable token. */
const tailOf = (data: string): string => JSON.stringify(data.slice(-40));

function logScrollback(line: string): void {
  console.info(`[mock::scrollback] ${line}`);
}

/** Build the `ScrollbackRestore` a `get_terminal_scrollback` hit returns:
 *  the stored `data` plus a well-formed `ScrollbackMeta` derived from the
 *  stored payload (mirrors `save_scrollback`'s meta construction). */
/** Write one payload to the "disk" store, stamping the save time — shared
 *  by `save_terminal_scrollback` (live panes on close) and
 *  `flush_scrollback_cache` (cache drain), mirroring the backend's
 *  `save_scrollback`. */
function writeScrollbackDisk(payload: ScrollbackPayload): void {
  scrollbackDisk.set(diskKey(payload.workspace_id, payload.pane_id), {
    payload,
    savedAt: Date.now(),
  });
}

function buildScrollbackRestore(entry: MockScrollbackDiskEntry): ScrollbackRestore {
  const p = entry.payload;
  const meta: ScrollbackMeta = {
    pane_id: p.pane_id,
    session_id: p.session_id,
    workspace_id: p.workspace_id,
    working_directory: p.working_directory,
    original_command: p.original_command ?? null,
    cols: p.cols,
    rows: p.rows,
    adapter_captures: p.adapter_captures ?? {},
    adapter_id: p.adapter_id ?? null,
    alternate_buffer: p.alternate_buffer ?? false,
    saved_at: entry.savedAt,
  };
  return { data: p.data, meta };
}

// ── Terminal flood + serialize triggers (issue #128) ────────────────
//
// Exposed on `window.__codemuxTerminalMock` for browser-console /
// automation use, and bound to two rare keydown combos below so the
// CLI-driven browser (`codemux browser key-press`, no JS eval) can drive
// them too.

let floodCounter = 0;

/**
 * Push `lines` numbered lines through EVERY currently-attached PTY output
 * channel, ending with a distinctive `MOCK-FLOOD-END <n>` marker so the
 * e2e run can wait for the tail. Delivered in ~64 KiB chunks, a few
 * chunks per `setTimeout(…, 0)` tick, so the whole flood lands within a
 * couple of seconds without one giant synchronous burst — the real PTY
 * delivers chunked too, and this keeps the serialize path realistic.
 */
function floodTerminals(lines = 150_000): void {
  const sessionIds = [...ptyChannels.keys()];
  const floodId = ++floodCounter;
  const CHUNK_BYTES = 64 * 1024;
  const CHUNKS_PER_TICK = 8;
  let lineNo = 0;
  let totalBytes = 0;

  console.info(
    `[mock::terminal] flood start sessions=${sessionIds.length} lines=${lines} id=${floodId} totalBytes=0`,
  );

  const pushToAll = (chunk: string): void => {
    // Resolve each session's channel entry FRESH on every push: a pane that
    // re-attaches mid-flood registers a NEW entry (nextIndex reset to 0) and
    // must receive the remaining frames plus the MOCK-FLOOD-END marker — a
    // one-shot snapshot of the entries would keep pushing into the detached
    // channel instead.
    for (const sessionId of sessionIds) {
      const entry = ptyChannels.get(sessionId);
      if (entry) ptyChannelPush(entry, chunk);
    }
    // Flood chunks are pure ASCII, so string length IS the byte count.
    totalBytes += chunk.length;
  };

  const tick = (): void => {
    for (let c = 0; c < CHUNKS_PER_TICK; c++) {
      if (lineNo >= lines) {
        // Final distinctive marker closes the flood — one greppable tail.
        pushToAll(`MOCK-FLOOD-END ${floodId}\r\n`);
        console.info(
          `[mock::terminal] flood done sessions=${sessionIds.length} totalBytes=${totalBytes}`,
        );
        return;
      }
      let buf = "";
      while (lineNo < lines && buf.length < CHUNK_BYTES) {
        lineNo++;
        buf += `flood line ${lineNo}\r\n`;
      }
      pushToAll(buf);
    }
    setTimeout(tick, 0);
  };

  // Kick off async so the caller (keydown handler / console) returns
  // immediately and the flood streams across subsequent ticks.
  setTimeout(tick, 0);
}

/**
 * Dispatch the backend's app-close `serialize-terminal-buffers` event
 * (payload `null`, see `src/tauri/events.ts`) to the app's listeners so
 * the close-path serialization dance (`use-scrollback-serializer.ts`)
 * runs live/flushes against the mock stores above.
 */
function emitSerializeBuffers(): void {
  console.info(
    "[mock::terminal] emitSerializeBuffers -> serialize-terminal-buffers",
  );
  emitEvent("serialize-terminal-buffers", null);
}

// ── Static command returns ──────────────────────────────────────────

const FEATURE_FLAGS: FeatureFlags = {
  unstable_openflow: false,
  unstable_browser_automation: false,
  unstable_indexing: false,
  // On in the mock so the seeded agent-chat workspaces render their
  // chat panes: the virtualized long transcript (issue #77) and the
  // per-thread Channel streaming path (issue #75) are both primary
  // dev-iteration surfaces. All chat IPC is mocked below.
  enable_agent_chat: true,
  // On in the mock so the draft surface (chat-home landing + Thread
  // Scope row below the composer) is a primary dev-iteration surface —
  // "New thread"/"New agent" render the draft composer with the
  // location · checkout · from-branch controls instead of eagerly
  // creating a workspace.
  enable_lazy_workspace_creation: true,
};

const SYNCED_SETTINGS: UserSettings = {
  appearance: {
    theme: "system",
    shell_font: null,
    terminal_font_size: 13,
    show_resource_monitor: true,
  },
  editor: { default_ide: null },
  terminal: { scrollback_limit: 10_000, cursor_style: "bar" },
  git: { default_base_branch: "main" },
  keyboard: { shortcuts: {} },
  notifications: { sound_enabled: true, desktop_enabled: true },
  file_tree: { show_hidden_files: false },
  session_restore: {
    enabled: true,
    scrollback_lines: 10_000,
    max_total_mb: 100,
  },
  // Checkpoints ON in the mock so the seeded chat pane exercises the
  // restore affordance (issue #80) without a real backend.
  agent_chat: { checkpoints_enabled: true, background_browser_desktop_viewport: false },
};

const EMPTY_CAPABILITIES: ProviderChatCapabilities = {
  models: [],
  effort_granularity: "per_session",
  effort_label_map: {},
  permission_modes: [],
  default_permission_mode: null,
  permission_granularity: "per_session",
};

// ── Agent chat (mocked end-to-end) ──────────────────────────────────
//
// The seeded `ws-codemux-chat` workspace carries an `agent_chat` pane
// bound to MOCK_CHAT_THREAD_ID. On mount the pane hydrates via
// `agent_chat_list_messages`, which returns a long generated
// transcript replayed through the real reducer — so the virtualized
// MessageList renders real ChatViewItems. `agent_chat_send_turn`
// simulates a streaming reply through the pane's attached per-thread
// Channel (issue #75, `emitChatEvent` above);
// `window.__codemuxChatMock.streamReply()` triggers one on demand for
// scroll/perf testing.

const MOCK_CHAT_MODEL: ChatModelInfo = {
  id: "mock-sonnet",
  label: "Mock Sonnet",
  // Mirrors the resolved-version + blurb the real backend now serves
  // for Claude rows so `npm run dev` visually exercises the picker's
  // description subtitle.
  description: "Sonnet 5 with 1M context · Efficient for routine tasks",
  effort_levels: [],
  default_effort: null,
  prompt_injected_effort_levels: [],
  context_window_options: [],
  supports_adaptive_thinking: false,
  supports_thinking_toggle: false,
  supports_fast_mode: false,
  supports_images: false,
  sub_provider: null,
  is_free: false,
};

const CLAUDE_CAPABILITIES: ProviderChatCapabilities = {
  models: [MOCK_CHAT_MODEL],
  effort_granularity: "per_session",
  effort_label_map: {},
  permission_modes: [
    {
      value: "bypassPermissions",
      label: "Full access",
      description: "Run every tool without asking",
      is_default: true,
    },
    {
      value: "default",
      label: "Ask first",
      description: "Prompt before tool use",
      is_default: false,
    },
  ],
  default_permission_mode: "bypassPermissions",
  permission_granularity: "per_session",
};

/** Number of simulated turns in the seeded transcript. Every 5th turn
 *  includes an 8-call tool burst (exercises the run-collapse row), so
 *  the item total lands well past 1,200 — enough to make unbounded
 *  row mounting obvious in devtools if virtualization ever regresses. */
const MOCK_CHAT_TURNS = 220;

const ASSISTANT_BODIES = [
  "Short answer: yes — the transcript only mounts the rows that intersect the viewport.",
  "Here's the longer explanation.\n\nThe message list used to render every item with a plain `.map()`, which meant a 5,000-message session mounted 5,000 DOM subtrees. Virtualization replaces that with a window: rows are measured as they appear and recycled as they leave.\n\n- Stable keys keep React row identity\n- `React.memo` still skips untouched rows\n- The tail snap only fires while you're pinned to the bottom",
  "Let me check a few files to confirm the behavior before answering.",
  "```ts\nconst distance = el.scrollHeight - el.scrollTop - el.clientHeight;\npinned = distance <= PIN_THRESHOLD_PX;\n```\n\nThat predicate decides whether the next token snaps the view to the tail or leaves you reading history in peace.",
  "Done. The change keeps the DOM bounded regardless of conversation length, so session-open time and scroll cost stop scaling with history size.\n\nA second paragraph pads this message out so row heights vary — fixed-height assumptions are exactly what dynamic measurement has to absorb.\n\nAnd a third paragraph for good measure, because real assistant turns are rarely uniform.",
];

let mockChatTranscriptCache: string[] | null = null;

/** Generate the persisted-payload list `agent_chat_list_messages`
 *  returns for the seeded thread: the same JSON envelopes the real
 *  backend stores, replayed by the frontend through the pure reducer. */
function mockChatTranscript(): string[] {
  if (mockChatTranscriptCache) return mockChatTranscriptCache;
  const T = MOCK_CHAT_THREAD_ID;
  const out: string[] = [];
  const push = (envelope: unknown) => out.push(JSON.stringify(envelope));
  for (let i = 0; i < MOCK_CHAT_TURNS; i++) {
    const turnId = `seed-turn-${i + 1}`;
    const isLast = i === MOCK_CHAT_TURNS - 1;
    push({
      type: "user_message",
      thread_id: T,
      text: isLast
        ? "Implement the fix for #57298."
        : `Turn ${i + 1}: how does the virtualized transcript hold up at scale?`,
    });
    if (isLast) {
      // Final turn is a curated showcase: a reasoning block, a tool group
      // card, a diff card and a task summary card, all landing at the tail
      // so they're visible the moment the seeded thread opens (design QA).
      for (const envelope of richChatTurnEnvelopes(T, turnId)) push(envelope);
      push({
        type: "turn_completed",
        thread_id: T,
        turn_id: turnId,
        status: { kind: "success" },
        usage: null,
      });
      continue;
    }
    if (i % 5 === 4) {
      // Tool burst — 8 consecutive calls so the run-collapse toggle
      // ("Show 4 earlier tool calls") appears inside one virtual row.
      for (let k = 0; k < 8; k++) {
        const toolUseId = `seed-tu-${i}-${k}`;
        push({
          type: "item_completed",
          thread_id: T,
          turn_id: turnId,
          item: {
            kind: "tool_use",
            tool_name: "Read",
            input: { file_path: `/src/components/chat/file-${i}-${k}.ts` },
            tool_use_id: toolUseId,
          },
        });
        push({
          type: "item_completed",
          thread_id: T,
          turn_id: turnId,
          item: {
            kind: "tool_result",
            tool_use_id: toolUseId,
            content: `// mock contents ${i}-${k}\nexport const value = ${k};`,
            is_error: false,
          },
        });
      }
    }
    push({
      type: "item_completed",
      thread_id: T,
      turn_id: turnId,
      item: {
        kind: "assistant_text",
        // Counter prefix on its own paragraph — inlining it before a
        // body that starts with a ``` fence would break the fence
        // (CommonMark fences must start at the beginning of a line).
        text: `(${i + 1}/${MOCK_CHAT_TURNS})\n\n${ASSISTANT_BODIES[i % ASSISTANT_BODIES.length]}`,
      },
    });
    push({
      type: "turn_completed",
      thread_id: T,
      turn_id: turnId,
      status: { kind: "success" },
      usage: null,
    });
  }
  // Final showcase turn: the subagents orchestration card (design
  // fixture) — one completed subagent, one still running — so the card,
  // the "1 subagent running" pane pill, and the drill-in all render on
  // open. Landing last keeps it visible the moment the thread hydrates.
  const subTurnId = "seed-subagents";
  push({
    type: "user_message",
    thread_id: T,
    text: "Implement clipboard-paste fallback",
  });
  for (const envelope of subagentTurnEnvelopes(T, subTurnId)) push(envelope);
  push({
    type: "turn_completed",
    thread_id: T,
    turn_id: subTurnId,
    status: { kind: "success" },
    usage: null,
  });

  mockChatTranscriptCache = out;
  return out;
}

// ── /workflow orchestration demo threads ────────────────────────────
//
// Three separate seeded threads — one per lifecycle stage of the
// "Audit route auth" `/workflow` run (design fixture
// `.design/workflow-orchestration.dc.html`) — so pending-approval,
// mid-run, and finished are each directly reachable from their own
// workspace instead of requiring a live interaction to advance between
// states. `agent_chat_list_messages` replays the same persisted-payload
// path as the main seeded thread (`mockChatTranscript` above): JSON
// envelopes through the real reducer via `replayPayloads`.
const WORKFLOW_THREAD_BUILDERS: Record<string, () => unknown[]> = {
  [MOCK_WORKFLOW_APPROVAL_THREAD_ID]: () =>
    workflowApprovalEnvelopes(MOCK_WORKFLOW_APPROVAL_THREAD_ID),
  [MOCK_WORKFLOW_RUNNING_THREAD_ID]: () =>
    workflowRunningEnvelopes(MOCK_WORKFLOW_RUNNING_THREAD_ID),
  [MOCK_WORKFLOW_COMPLETE_THREAD_ID]: () =>
    workflowCompleteEnvelopes(MOCK_WORKFLOW_COMPLETE_THREAD_ID),
};

const workflowTranscriptCache = new Map<string, string[]>();

/** `agent_chat_list_messages` payload for a workflow-demo thread, or
 *  `null` when `threadId` isn't one of the three seeded above. */
function mockWorkflowTranscript(threadId: string): string[] | null {
  const builder = WORKFLOW_THREAD_BUILDERS[threadId];
  if (!builder) return null;
  let cached = workflowTranscriptCache.get(threadId);
  if (!cached) {
    cached = builder().map((e) => JSON.stringify(e));
    workflowTranscriptCache.set(threadId, cached);
  }
  return cached;
}

/** `workspace_id` + `cwd` for each workflow-demo thread — keeps
 *  `agent_chat_list_sessions` / `agent_chat_get_session` in sync with
 *  the panes/worktree paths seeded in `mock-fixtures.ts`. */
const WORKFLOW_THREAD_WORKSPACE: Record<string, { workspaceId: string; cwd: string }> = {
  [MOCK_WORKFLOW_APPROVAL_THREAD_ID]: {
    workspaceId: "ws-codemux-workflow-approval",
    cwd: `${MOCK_HOME_DIR}/.codemux/worktrees/codemux/demo-workflow-approval`,
  },
  [MOCK_WORKFLOW_RUNNING_THREAD_ID]: {
    workspaceId: "ws-codemux-workflow-running",
    cwd: `${MOCK_HOME_DIR}/.codemux/worktrees/codemux/demo-workflow-running`,
  },
  [MOCK_WORKFLOW_COMPLETE_THREAD_ID]: {
    workspaceId: "ws-codemux-workflow-complete",
    cwd: `${MOCK_HOME_DIR}/.codemux/worktrees/codemux/demo-workflow-complete`,
  },
};

function mockWorkflowSessionRecord(threadId: string): unknown | null {
  const loc = WORKFLOW_THREAD_WORKSPACE[threadId];
  if (!loc) return null;
  return {
    thread_id: threadId,
    sdk_session_id: `sdk-${threadId}`,
    workspace_id: loc.workspaceId,
    cwd: loc.cwd,
    provider: "claude",
    title: "Audit route auth",
    created_at: new Date().toISOString(),
    last_active_at: new Date().toISOString(),
    model: "claude-opus-4-8",
    effort: null,
    context_window: null,
    permission_mode: "bypassPermissions",
  };
}

let mockChatTurnSeq = 0;
let mockQueuedSeq = 0;

// Follow-up queueing: track which threads have a turn in flight and the
// FIFO follow-up queue behind each, so `agent_chat_send_turn` can mirror
// the real backend (queue-while-busy) and drain on completion.
const chatActiveTurns = new Set<string>();
interface MockQueuedTurn {
  queuedId: string;
  text: string;
  clientNonce: string | null;
}
const chatQueues = new Map<string, MockQueuedTurn[]>();

/** Pop the next queued follow-up (if any) and dispatch it, mirroring the
 *  provider's drain-on-completion. */
function drainChatQueue(threadId: string): void {
  const q = chatQueues.get(threadId);
  if (!q || q.length === 0) return;
  const next = q.shift() as MockQueuedTurn;
  const turnId = streamMockChatReply(threadId);
  emitChatEvent(threadId, {
    type: "queued_turn_dispatched",
    thread_id: threadId,
    queued_id: next.queuedId,
    turn_id: turnId,
    text: next.text,
  });
}

/** Simulate a streaming assistant reply on the real event channel.
 *  Returns the turn id immediately; deltas tick on an interval so
 *  stick-to-bottom / scroll-up-freedom can be observed live.
 *
 *  Delivery is the per-thread Channel registered via
 *  `attach_agent_chat_output` (issue #75) — the global event bus
 *  carries no thread-scoped chat traffic anymore, matching the real
 *  backend's `forward_event` routing. */
function streamMockChatReply(
  threadId: string,
  opts: { tokens?: number; intervalMs?: number } = {},
): string {
  const turnId = `live-turn-${++mockChatTurnSeq}`;
  const tokens = Math.max(1, opts.tokens ?? 120);
  const intervalMs = Math.max(5, opts.intervalMs ?? 40);
  const send = (event: unknown) => emitChatEvent(threadId, event);

  chatActiveTurns.add(threadId);
  send({
    type: "session_state_changed",
    thread_id: threadId,
    status: { status: "running", active_turn: turnId },
  });

  // A short live thinking segment precedes the text tokens: thinking
  // deltas stream first (Reasoning block header shimmers "Thinking…"),
  // then an assistant_thinking completion seals the block before prose
  // begins.
  const THINKING_CHUNKS = [
    "Let me look at what the user is asking ",
    "and figure out the cleanest approach ",
    "before I start writing any code.",
  ];
  // A run of reasoning + tool calls between thinking and prose: this is
  // what folds into the live "Working" Activity block. Each step is
  // emitted as a `tool_use` (goes `running`) then, on the next tick, a
  // `tool_result` (settles) — so a single tool is briefly running and the
  // block stays "Working" for the whole run. Kept out of the persisted
  // seed so it only appears on an on-demand stream.
  const TOOL_STEPS: Array<{
    tool_name: string;
    input: unknown;
    content: string;
  }> = [
    {
      tool_name: "Read",
      input: { file_path: "src-tauri/src/terminal/mod.rs", offset: 1450, limit: 62 },
      content:
        "fn spawn_pty(&self) -> Result<Pty> {\n    let mut cmd = CommandBuilder::new(&shell);\n    cmd.envs(std::env::vars());\n    // ... env assembled from the process only\n}",
    },
    {
      tool_name: "Grep",
      input: { pattern: "fn workspace_pty_env", path: "src-tauri/src" },
      content:
        "src-tauri/src/workspace.rs:88:    pub fn workspace_pty_env(&self) -> Vec<(String, String)> {\n src-tauri/src/workspace.rs:140:    // exposed but never called at spawn time",
    },
    {
      tool_name: "Read",
      input: { file_path: "src-tauri/src/workspace.rs", offset: 88, limit: 52 },
      content:
        "pub fn workspace_pty_env(&self) -> Vec<(String, String)> {\n    self.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect()\n}",
    },
    {
      tool_name: "Edit",
      input: {
        file_path: "src-tauri/src/terminal/mod.rs",
        old_string: "    cmd.envs(std::env::vars());",
        new_string:
          "    cmd.envs(std::env::vars());\n    cmd.envs(workspace.pty_env());",
      },
      content: "Applied edit to src-tauri/src/terminal/mod.rs",
    },
    {
      tool_name: "Bash",
      input: { command: "cargo check -p codemux" },
      content: "    Checking codemux v0.10.0\n    Finished dev [unoptimized] target(s) in 4.12s",
    },
  ];
  let phase: "thinking" | "tools" | "text" = "thinking";
  let thinkIdx = 0;
  let thinkingText = "";
  let toolIdx = 0;
  let toolSubphase: "use" | "result" = "use";
  let emitted = 0;
  let fullText = "";
  const timer = window.setInterval(() => {
    if (phase === "thinking") {
      const chunk = THINKING_CHUNKS[thinkIdx];
      thinkingText += chunk;
      send({
        type: "content_delta",
        thread_id: threadId,
        turn_id: turnId,
        delta: { kind: "thinking", text: chunk },
      });
      thinkIdx += 1;
      if (thinkIdx >= THINKING_CHUNKS.length) {
        send({
          type: "item_completed",
          thread_id: threadId,
          turn_id: turnId,
          item: { kind: "assistant_thinking", text: thinkingText },
        });
        phase = "tools";
      }
      return;
    }
    if (phase === "tools") {
      const step = TOOL_STEPS[toolIdx];
      const toolUseId = `${turnId}-tool-${toolIdx}`;
      if (toolSubphase === "use") {
        send({
          type: "item_completed",
          thread_id: threadId,
          turn_id: turnId,
          item: {
            kind: "tool_use",
            tool_name: step.tool_name,
            tool_use_id: toolUseId,
            input: step.input,
          },
        });
        toolSubphase = "result";
        return;
      }
      send({
        type: "item_completed",
        thread_id: threadId,
        turn_id: turnId,
        item: {
          kind: "tool_result",
          tool_use_id: toolUseId,
          content: step.content,
          is_error: false,
        },
      });
      toolIdx += 1;
      toolSubphase = "use";
      if (toolIdx >= TOOL_STEPS.length) phase = "text";
      return;
    }
    emitted += 1;
    const chunk =
      emitted % 12 === 0
        ? `token ${emitted}.\n\n`
        : `token ${emitted} `;
    fullText += chunk;
    send({
      type: "content_delta",
      thread_id: threadId,
      turn_id: turnId,
      delta: { kind: "text", text: chunk },
    });
    if (emitted >= tokens) {
      window.clearInterval(timer);
      send({
        type: "item_completed",
        thread_id: threadId,
        turn_id: turnId,
        item: { kind: "assistant_text", text: fullText },
      });
      send({
        type: "turn_completed",
        thread_id: threadId,
        turn_id: turnId,
        status: { kind: "success" },
        usage: null,
      });
      send({
        type: "session_state_changed",
        thread_id: threadId,
        status: { status: "ready" },
      });
      // Turn done — drain the next queued follow-up (FIFO).
      chatActiveTurns.delete(threadId);
      drainChatQueue(threadId);
    }
  }, intervalMs);
  return turnId;
}

/** Live-stream a full two-subagent lifecycle over the real per-thread
 *  Channel for visual dev: the orchestrator spawns "Explore" and
 *  "Implement", both work through a few tool calls, then finish. Drives
 *  the orchestration card, the running pill, the shimmer activity lines,
 *  and (if entered) the drill-in — all from live `subagent_updated` +
 *  `subagent_id`-tagged events, exactly like the real adapter.
 *
 *  `window.__codemuxChatMock.streamSubagents()` triggers it on demand. */
function streamMockSubagents(
  threadId: string,
  opts: { intervalMs?: number } = {},
): string {
  const turnId = `live-subagents-${++mockChatTurnSeq}`;
  const intervalMs = Math.max(60, opts.intervalMs ?? 550);
  const send = (event: unknown) => emitChatEvent(threadId, event);
  const snap = (subagent: Record<string, unknown>) =>
    send({ type: "subagent_updated", thread_id: threadId, subagent });
  const item = (subagentId: string, it: Record<string, unknown>) =>
    send({
      type: "item_completed",
      thread_id: threadId,
      turn_id: turnId,
      subagent_id: subagentId,
      item: it,
    });
  const toolUse = (
    subagentId: string,
    id: string,
    toolName: string,
    input: Record<string, unknown>,
  ) => item(subagentId, { kind: "tool_use", tool_name: toolName, tool_use_id: id, input });
  const toolResult = (subagentId: string, id: string, content: string) =>
    item(subagentId, { kind: "tool_result", tool_use_id: id, content, is_error: false });

  // Scripted timeline — one frame per tick so the lifecycle is watchable.
  const frames: Array<() => void> = [
    () =>
      send({
        type: "session_state_changed",
        thread_id: threadId,
        status: { status: "running", active_turn: turnId },
      }),
    () =>
      send({
        type: "item_completed",
        thread_id: threadId,
        turn_id: turnId,
        item: {
          kind: "assistant_text",
          text: "Delegating to two subagents so exploration and implementation run in parallel.",
        },
      }),
    () =>
      snap({
        subagent_id: "explore",
        name: "Explore",
        agent_type: "explore",
        model: "sonnet · high",
        status: "running",
      }),
    () =>
      snap({
        subagent_id: "build",
        name: "Implement",
        agent_type: "implement",
        model: "opus · xhigh",
        status: "running",
      }),
    () => toolUse("explore", "ex-1", "Grep", { pattern: "handlePaste", path: "src" }),
    () => toolResult("explore", "ex-1", "7 matches"),
    () => snap({ subagent_id: "explore", activity: "mapping the paste handler call graph…" }),
    () => toolUse("build", "b-1", "Edit", {
      file_path: "src-tauri/src/clipboard.rs",
      old_string: "",
      new_string: "pub fn read_image() {}\n",
    }),
    () => toolResult("build", "b-1", "Applied edit"),
    () => toolUse("explore", "ex-2", "Read", { file_path: "src/components/chat/Composer.tsx" }),
    () => toolResult("explore", "ex-2", "// Composer.tsx\n"),
    () =>
      snap({
        subagent_id: "explore",
        status: "completed",
        result_text: "Mapped the paste path · 2 files inspected",
        tool_use_count: 2,
        duration_ms: 12000,
      }),
    () => toolUse("build", "b-2", "Bash", { command: "cargo test clipboard_fallback" }),
    () => toolResult("build", "b-2", "test result: ok. 1 passed"),
    () =>
      snap({
        subagent_id: "build",
        status: "completed",
        result_text: "Done · clipboard fallback landed, tests pass",
        tool_use_count: 4,
        duration_ms: 21000,
      }),
    () =>
      send({
        type: "item_completed",
        thread_id: threadId,
        turn_id: turnId,
        item: {
          kind: "assistant_text",
          text: "Both subagents reported back — clipboard paste fallback is implemented and verified.",
        },
      }),
    () =>
      send({
        type: "turn_completed",
        thread_id: threadId,
        turn_id: turnId,
        status: { kind: "success" },
        usage: null,
      }),
    () =>
      send({
        type: "session_state_changed",
        thread_id: threadId,
        status: { status: "ready" },
      }),
  ];

  let i = 0;
  const timer = window.setInterval(() => {
    const frame = frames[i++];
    if (frame) frame();
    if (i >= frames.length) window.clearInterval(timer);
  }, intervalMs);
  return turnId;
}

// Expose the stream triggers for browser-console / automation use.
(
  window as unknown as {
    __codemuxChatMock: {
      threadId: string;
      streamReply: typeof streamMockChatReply;
      streamSubagents: typeof streamMockSubagents;
    };
  }
).__codemuxChatMock = {
  threadId: MOCK_CHAT_THREAD_ID,
  streamReply: streamMockChatReply,
  streamSubagents: streamMockSubagents,
};

// Expose the terminal flood + serialize triggers for browser-console /
// automation use (issue #128 scrollback serialize/restore e2e).
(
  window as unknown as {
    __codemuxTerminalMock: {
      flood: typeof floodTerminals;
      emitSerializeBuffers: typeof emitSerializeBuffers;
    };
  }
).__codemuxTerminalMock = {
  flood: floodTerminals,
  emitSerializeBuffers,
};

// CLI-drivable triggers: the browser is driven by `codemux browser
// key-press` (no JS eval), so bind two RARE combos to the same helpers.
// Capture phase + preventDefault/stopPropagation so the chord never
// reaches xterm. `e.code` (physical key) is used so the binding is
// layout-independent even while Alt mangles `e.key`.
window.addEventListener(
  "keydown",
  (e) => {
    if (!(e.ctrlKey && e.altKey && e.shiftKey)) return;
    if (e.code === "KeyF") {
      e.preventDefault();
      e.stopPropagation();
      console.info("[mock::terminal] key-combo Ctrl+Alt+Shift+F -> flood()");
      floodTerminals();
    } else if (e.code === "KeyS") {
      e.preventDefault();
      e.stopPropagation();
      console.info(
        "[mock::terminal] key-combo Ctrl+Alt+Shift+S -> emitSerializeBuffers()",
      );
      emitSerializeBuffers();
    }
  },
  true,
);

// A small, mutable preset store so the preset bar + structured editor are
// exercisable in the browser dev environment. Mirrors the real backend:
// mutations update this object and re-emit `presets-changed`.
function mkPreset(p: Partial<TerminalPreset> & { id: string; name: string }): TerminalPreset {
  return {
    description: null,
    commands: [],
    working_directory: null,
    launch_mode: "new_tab",
    icon: null,
    pinned: true,
    is_builtin: false,
    auto_run_on_workspace: false,
    auto_run_on_new_tab: false,
    kind: "cli",
    launch_config: null,
    ...p,
  };
}

const presetState: PresetStoreSnapshot = {
  presets: [
    mkPreset({
      id: "builtin-claude",
      name: "Claude Code",
      commands: ["claude --dangerously-skip-permissions"],
      icon: "claude",
      is_builtin: true,
    }),
    mkPreset({
      id: "builtin-codex",
      name: "Codex",
      commands: ["codex --full-auto"],
      icon: "codex",
      is_builtin: true,
    }),
    mkPreset({
      id: "builtin-gemini",
      name: "Gemini",
      commands: ["gemini --yolo"],
      icon: "gemini",
      is_builtin: true,
    }),
    mkPreset({
      id: "builtin-copilot",
      name: "Copilot",
      commands: ["copilot --allow-all"],
      icon: "copilot",
      is_builtin: true,
    }),
    // Seeded LAST on purpose: the preset bar pins the native chat preset
    // to the far-left slot regardless of its position in the store, so
    // its trailing seed order proves that render-time pinning works.
    mkPreset({
      id: "builtin-chat-agent",
      name: "Chat Agent",
      icon: "chat-agent",
      kind: "chat_agent",
      is_builtin: true,
    }),
  ],
  bar_visible: true,
  default_preset_id: null,
};

function emitPresets(): void {
  emitEvent("presets-changed", presetState);
}

// A plausible dark palette so `useTerminalThemeSync` has real values to
// apply to xterm instead of choking on a partial object.
const THEME: ThemeColors = {
  accent: "#f59e0b",
  cursor: "#e5e7eb",
  foreground: "#e5e7eb",
  background: "#0a0a0a",
  selection_foreground: "#0a0a0a",
  selection_background: "#f59e0b",
  color0: "#1e1e1e",
  color1: "#f87171",
  color2: "#4ade80",
  color3: "#fbbf24",
  color4: "#60a5fa",
  color5: "#c084fc",
  color6: "#22d3ee",
  color7: "#e5e7eb",
  color8: "#6b7280",
  color9: "#ef4444",
  color10: "#22c55e",
  color11: "#f59e0b",
  color12: "#3b82f6",
  color13: "#a855f7",
  color14: "#06b6d4",
  color15: "#f9fafb",
};

const SHELL_APPEARANCE: ShellAppearance = {
  font_family: "'JetBrains Mono Variable', monospace",
};

const CLI_TOOLS: CliToolInfo[] = [
  { id: "claude", name: "Claude Code", available: true, path: "/usr/bin/claude" },
  { id: "codex", name: "Codex", available: false, path: null },
];

function resourceMetrics(): ResourceMetricsSnapshot {
  return {
    app: {
      cpu: 3.2,
      memory: 412_000_000,
      main: { cpu: 1.1, memory: 180_000_000 },
      web_view: { cpu: 2.0, memory: 210_000_000 },
      other: { cpu: 0.1, memory: 22_000_000 },
    },
    workspaces: [],
    host: {
      total_memory: 32_000_000_000,
      free_memory: 18_000_000_000,
      used_memory: 14_000_000_000,
      memory_usage_percent: 43.75,
      cpu_core_count: 16,
      load_average_1m: 1.42,
    },
    total_cpu: 3.2,
    total_memory: 412_000_000,
    collected_at: Date.now(),
  };
}

// ── Command router ──────────────────────────────────────────────────
//
// Keyed by exact Tauri command name. Boot-critical reads return seed
// data; mutators patch `appState` and re-emit. Args arrive with the
// SAME camelCase keys the `src/tauri/commands.ts` wrappers pass.

type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;

const handlers: Record<string, Handler> = {
  // ── Auth / sync ──
  check_auth: () => MOCK_USER,
  get_auth_token: () => "mock-token",
  get_sync_status: () => ({ syncAvailable: true, authMethod: "github" }),
  sign_out: () => undefined,
  skills_sync_status: () => ({ state: "idle", lastSyncAtMillis: null }),
  skills_sync_now: () => ({
    pushedCount: 0,
    pulledCount: 0,
    conflictCount: 0,
    errorCount: 0,
  }),

  // ── Browser pane ──
  //
  // The pane connects its stream WebSocket to whatever URL this
  // returns. To exercise the browser pane end-to-end in plain-browser
  // dev, run a real daemon on the dev stream port first:
  //
  //   AGENT_BROWSER_STREAM_PORT=9777 agent-browser open https://example.com --headless --session devmock
  //
  // Without a daemon the pane sits in its connecting/retry state —
  // the same UI as a dead daemon in the real app.
  start_browser_stream: () => "ws://127.0.0.1:9777",
  // Toolbar/viewport/inspector commands shell out to the CLI in the
  // real app; the mock accepts and ignores them.
  agent_browser_run: () => null,

  // ── Core state ──
  get_app_state: () => appState,
  get_home_dir: () => MOCK_HOME_DIR,
  get_feature_flags: () => FEATURE_FLAGS,
  get_platform: () => "linux",
  get_package_format: () => "AppImage",

  // ── Settings ──
  get_synced_settings: () => SYNCED_SETTINGS,
  update_synced_settings: (a) => (a.settings as UserSettings) ?? SYNCED_SETTINGS,
  update_setting: () => SYNCED_SETTINGS,
  reset_synced_settings: () => SYNCED_SETTINGS,
  db_get_all_settings: () => ({}),
  db_get_ui_state: () => null,
  db_set_ui_state: () => undefined,
  db_set_setting: () => undefined,
  db_get_setting: () => null,
  db_delete_setting: () => undefined,
  db_get_recent_projects: () => [],
  db_add_recent_project: () => undefined,

  // ── File dialogs ──
  // Default: resolve as a cancel (null / empty) like the previous
  // fall-through did. Set the sessionStorage flag below to simulate
  // the Linux "no file picker backend" preflight rejection (issue
  // #95) and visually verify the error toast in the browser:
  //   sessionStorage.setItem("codemux-mock-no-file-picker", "1")
  pick_folder_dialog: () => {
    if (sessionStorage.getItem("codemux-mock-no-file-picker")) {
      return Promise.reject(
        "NO_FILE_PICKER_BACKEND: cannot open a file dialog (dev mock simulation). " +
          "The XDG desktop portal is unavailable and zenity is not installed.",
      );
    }
    return null;
  },
  pick_files_dialog: () => {
    if (sessionStorage.getItem("codemux-mock-no-file-picker")) {
      return Promise.reject(
        "NO_FILE_PICKER_BACKEND: cannot open a file dialog (dev mock simulation). " +
          "The XDG desktop portal is unavailable and zenity is not installed.",
      );
    }
    return [];
  },

  // ── Theme / appearance ──
  get_current_theme: () => THEME,
  get_shell_appearance: () => SHELL_APPEARANCE,

  // ── Resource monitor ──
  get_resource_metrics: () => resourceMetrics(),

  // ── Agent chat (mocked end-to-end for the seeded chat workspaces) ──
  // start_session echoes back the frontend-minted thread id;
  // send_turn answers with the channel-streamed mock reply.
  list_chat_provider_capabilities: (a) =>
    a.provider === "claude" ? CLAUDE_CAPABILITIES : EMPTY_CAPABILITIES,
  agent_chat_list_messages: (a) => {
    const threadId = a.threadId as string;
    if (threadId === MOCK_CHAT_THREAD_ID) return mockChatTranscript();
    return mockWorkflowTranscript(threadId) ?? [];
  },
  // Return one record for the seeded thread so the pane can resolve the
  // D2 session-start marker's `created_at` (and the SessionSelector
  // dropdown has an entry). `created_at` is "now" so the marker reads
  // "Today · HH:MM" like the design. Also covers the three `/workflow`
  // demo threads, keyed by the requesting workspace id.
  agent_chat_list_sessions: (a) => {
    const workspaceId = a.workspaceId as string | undefined;
    const workflowEntry = Object.entries(WORKFLOW_THREAD_WORKSPACE).find(
      ([, loc]) => loc.workspaceId === workspaceId,
    );
    if (workflowEntry) {
      const record = mockWorkflowSessionRecord(workflowEntry[0]);
      return record ? [record] : [];
    }
    return [
      {
        thread_id: MOCK_CHAT_THREAD_ID,
        sdk_session_id: "sdk-mock-chat",
        workspace_id: "ws-codemux-chat",
        cwd: `${MOCK_HOME_DIR}/projects/codemux`,
        provider: "claude",
        title: "agent-chat-demo",
        created_at: new Date().toISOString(),
        last_active_at: new Date().toISOString(),
        model: "claude-opus-4-8",
        effort: null,
        context_window: null,
        permission_mode: "bypassPermissions",
      },
    ];
  },
  // Restart-resume seed (design F): the pane's mount-seed effect fetches
  // this to restore picker config + resume cursor. Return a plausible
  // record for the seeded thread so `npm run dev` rehydrates the pickers;
  // any other thread has no persisted row yet.
  agent_chat_get_session: (a) => {
    const threadId = a.threadId as string;
    if (threadId === MOCK_CHAT_THREAD_ID) {
      return {
        thread_id: MOCK_CHAT_THREAD_ID,
        sdk_session_id: "sdk-mock-chat",
        workspace_id: "ws-codemux-chat",
        cwd: `${MOCK_HOME_DIR}/projects/codemux`,
        provider: "claude",
        title: "agent-chat-demo",
        created_at: new Date().toISOString(),
        last_active_at: new Date().toISOString(),
        model: "claude-opus-4-8",
        effort: null,
        context_window: null,
        permission_mode: "bypassPermissions",
      };
    }
    return mockWorkflowSessionRecord(threadId);
  },
  // DB-only config persist (design G). The mock has no SQLite, so this
  // is a no-op — the demo pane keeps its in-memory slice values.
  agent_chat_update_session_config: () => undefined,
  agent_chat_start_session: (a) =>
    (a.input as { thread_id: string }).thread_id,
  agent_chat_send_turn: (a) => {
    const input = a.input as {
      thread_id: string;
      text: string;
      client_nonce?: string | null;
    };
    const threadId = input.thread_id;
    // Queue-while-busy, mirroring the real provider: a send during an
    // active turn parks in the FIFO queue and renders greyed-out.
    if (chatActiveTurns.has(threadId)) {
      const queuedId = `mock-queued-${++mockQueuedSeq}`;
      const q = chatQueues.get(threadId) ?? [];
      q.push({
        queuedId,
        text: input.text,
        clientNonce: input.client_nonce ?? null,
      });
      chatQueues.set(threadId, q);
      emitChatEvent(threadId, {
        type: "turn_queued",
        thread_id: threadId,
        queued_id: queuedId,
        client_nonce: input.client_nonce ?? null,
        text: input.text,
      });
      return { turn_id: "", queued_id: queuedId };
    }
    const turnId = streamMockChatReply(threadId);
    return { turn_id: turnId, queued_id: null };
  },
  agent_chat_cancel_queued_turn: (a) => {
    const { threadId, queuedId } = a as {
      threadId: string;
      queuedId: string;
    };
    const q = chatQueues.get(threadId);
    if (q) {
      const idx = q.findIndex((e) => e.queuedId === queuedId);
      if (idx >= 0) {
        q.splice(idx, 1);
        emitChatEvent(threadId, {
          type: "queued_turn_cancelled",
          thread_id: threadId,
          queued_id: queuedId,
        });
      }
    }
    return undefined;
  },
  agent_chat_interrupt_turn: () => undefined,
  agent_chat_respond_to_request: () => undefined,
  agent_chat_set_model: () => undefined,
  agent_chat_set_permission_mode: () => undefined,
  agent_chat_stop_session: () => undefined,
  agent_chat_rename_session: () => undefined,
  agent_chat_delete_session: () => undefined,
  // Run-start rollback checkpoint (issue #80). The seeded thread has a
  // checkpoint so the pane header shows the restore affordance;
  // restore just logs (the mock has no real working tree to rewrite).
  agent_chat_get_checkpoint: (a) =>
    a.threadId === MOCK_CHAT_THREAD_ID
      ? {
          thread_id: MOCK_CHAT_THREAD_ID,
          workspace_id: "ws-codemux-chat",
          repo_path: `${MOCK_HOME_DIR}/projects/codemux`,
          ref_name: `refs/codemux/checkpoints/${MOCK_CHAT_THREAD_ID}`,
          snapshot_commit: "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
          head_commit: "1234567890abcdef1234567890abcdef12345678",
          branch: "main",
          created_at: "2026-06-09 10:00:00",
        }
      : null,
  agent_chat_restore_checkpoint: (a) => {
    console.info("[tauri-mock] agent_chat_restore_checkpoint", a.threadId);
    return undefined;
  },
  grep_count_pattern: () => 0,

  // Clipboard-image paste fallback (agent-chat composer). The real
  // command reads the OS clipboard server-side; in the browser there's
  // no such surface, so reject like the "no image on clipboard" case.
  // The composer's clipboardData fast path handles real image pastes in
  // the browser, and this rejection makes a text-only paste fall
  // through to the default (plain-text) behaviour.
  paste_clipboard_image: () => {
    throw new Error("clipboard read_image failed: no image (dev mock)");
  },

  // ── Agent chat per-thread event channel (issue #75) ──
  // Mirrors AgentChatChannelRegistry: newest attach wins, detach is
  // generation-guarded so a stale unmount can't tear down a newer
  // pane's channel.
  attach_agent_chat_output: (a) => {
    const threadId = a.threadId as string;
    const generation = ++chatChannelGeneration;
    chatChannels.set(threadId, {
      channel: a.channel,
      generation,
      nextIndex: 0,
    });
    return generation;
  },
  detach_agent_chat_output: (a) => {
    const threadId = a.threadId as string;
    const entry = chatChannels.get(threadId);
    if (entry && entry.generation === a.generation) {
      chatChannels.delete(threadId);
    }
    return undefined;
  },

  // ── Presets ──
  get_presets: () => presetState,
  create_preset: (a) => {
    const id = `custom-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
    presetState.presets.push(
      mkPreset({
        id,
        name: (a.name as string) ?? "New preset",
        description: (a.description as string | null) ?? null,
        commands: (a.commands as string[]) ?? [],
        working_directory: (a.workingDirectory as string | null) ?? null,
        launch_mode: (a.launchMode as TerminalPreset["launch_mode"]) ?? "new_tab",
        icon: (a.icon as string | null) ?? null,
        pinned: (a.pinned as boolean) ?? true,
        launch_config:
          (a.launchConfig as TerminalPreset["launch_config"]) ?? null,
      }),
    );
    emitPresets();
    return id;
  },
  update_preset: (a) => {
    const p = presetState.presets.find((x) => x.id === a.id);
    if (p) {
      if (a.name != null) p.name = a.name as string;
      if (a.description !== undefined) p.description = (a.description as string | null) ?? null;
      if (a.commands != null) p.commands = a.commands as string[];
      if (a.launchMode != null) p.launch_mode = a.launchMode as TerminalPreset["launch_mode"];
      if (a.icon !== undefined) p.icon = (a.icon as string | null) ?? null;
      if (a.autoRunOnWorkspace != null) p.auto_run_on_workspace = a.autoRunOnWorkspace as boolean;
      if (a.autoRunOnNewTab != null) p.auto_run_on_new_tab = a.autoRunOnNewTab as boolean;
      if (a.launchConfig != null) p.launch_config = a.launchConfig as TerminalPreset["launch_config"];
      else if (a.clearLaunchConfig === true) p.launch_config = null;
      emitPresets();
    }
    return undefined;
  },
  delete_preset: (a) => {
    presetState.presets = presetState.presets.filter((x) => x.id !== a.id);
    emitPresets();
    return undefined;
  },
  set_preset_pinned: (a) => {
    const p = presetState.presets.find((x) => x.id === a.id);
    if (p) {
      p.pinned = a.pinned as boolean;
      emitPresets();
    }
    return undefined;
  },
  set_preset_bar_visible: (a) => {
    presetState.bar_visible = a.visible as boolean;
    emitPresets();
    return undefined;
  },
  reorder_presets: (a) => {
    const from = presetState.presets.findIndex((x) => x.id === a.presetId);
    const to = a.targetIndex as number;
    if (from >= 0 && to >= 0 && to < presetState.presets.length) {
      const [moved] = presetState.presets.splice(from, 1);
      presetState.presets.splice(to, 0, moved);
      emitPresets();
    }
    return undefined;
  },
  apply_preset: () => undefined,

  // ── Editors / tooling ──
  detect_editors: () => [],
  list_available_cli_tools: () => CLI_TOOLS,

  // ── GitHub / PRs (pre-seeded; never hit a real API) ──
  check_gh_available: () => true,
  check_gh_status: () => ({ status: "Authenticated", username: "mock-dev" }),
  refresh_workspace_pr: (a) => findWorkspace(a.workspaceId)?.pr_number ?? null,
  refresh_workspace_issue: () => null,
  // Issue detail popover (sidebar row + workspace context bar): expand
  // the workspace's seeded `linked_issue` into a full GitHubIssue so
  // the popover renders real content instead of an empty shell.
  get_github_issue: (a) => {
    const linked = findWorkspace(a.workspaceId)?.linked_issue;
    if (!linked || linked.number !== (a.issueNumber as number)) return null;
    return {
      number: linked.number,
      title: linked.title,
      state: linked.state,
      labels: linked.labels,
      assignees: ["mock-dev"],
      url: `https://github.com/mock/repo/issues/${linked.number}`,
      body:
        "Seeded by the dev mock runtime so the issue-detail popover is " +
        "fully exercisable in a plain browser.\n\nIn the real app this " +
        "body comes from `gh issue view`.",
      comments: [],
      totalComments: 0,
      updatedAt: null,
    };
  },
  list_incoming_prs: () => [],

  // ── Hosts (remote devices) ──
  //
  // Seed one host so the Settings → Hosts detail pane (Test connection,
  // Reinstall agent, Edit/Remove) renders in the browser dev runtime.
  // The mutating commands echo back plausible payloads — nothing here
  // touches a real SSH target.
  hosts_list: () => [
    {
      id: 1,
      server_id: "srv-pandora",
      name: "pandora",
      ssh_target: "deus@pandora",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      dirty: false,
    },
  ],
  hosts_add: (a) => ({
    id: Date.now(),
    server_id: null,
    name: String(a.name ?? "new-device"),
    ssh_target: String(a.sshTarget ?? "user@host"),
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    dirty: true,
  }),
  hosts_update: (a) => ({
    id: Number(a.id ?? 1),
    server_id: null,
    name: String(a.name ?? "homelab"),
    ssh_target: String(a.sshTarget ?? "deus@homelab.local"),
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    dirty: true,
  }),
  hosts_delete: () => undefined,
  hosts_test_connection: () => ({
    ok: true,
    message: "Connected. codemux-remote v0.9.5 is installed (Linux x86_64)",
    needs_install: false,
    uname: null,
  }),
  hosts_reinstall_remote: () => ({
    ok: true,
    message:
      "codemux-remote v0.9.5 reinstalled on pandora — daemon restarted; " +
      "the next push uses the fresh binary.",
  }),

  // ── Cross-device workspace sync registry ──
  //
  // Seed a few sibling-device rows (workspace_id: null) on the "pandora"
  // host so the Workspaces overview renders its second device bucket with
  // "lives on another device" cards. Mirrors the redesign mock data.
  workspaces_sync_list: () => [
    {
      id: 9001,
      server_id: "ws-partpilot",
      workspace_id: null,
      title: "partpilot",
      host_server_id: "srv-pandora",
      project_path: "/home/deus/projects/partpilot",
      project_remote: "github.com/deus/partpilot",
      git_branch: "main",
      git_head_sha: null,
      project_uid: "uid-partpilot",
      workspace_kind: "main",
      default_branch: "main",
      origin_path: "/home/deus/projects/partpilot",
      created_at: "2026-06-20T00:00:00Z",
      updated_at: "2026-06-26T00:00:00Z",
      dirty: false,
    },
    {
      id: 9002,
      server_id: "ws-activity-labels",
      workspace_id: null,
      title: "activity-labels",
      host_server_id: "srv-pandora",
      project_path: "/home/deus/projects/partpilot",
      project_remote: "github.com/deus/partpilot",
      git_branch: "chat-ux-activity-labels",
      git_head_sha: null,
      project_uid: "uid-partpilot",
      workspace_kind: "worktree",
      default_branch: "main",
      origin_path: "/home/deus/projects/partpilot-activity-labels",
      created_at: "2026-06-21T00:00:00Z",
      updated_at: "2026-06-26T00:00:00Z",
      dirty: false,
    },
    {
      id: 9003,
      server_id: "ws-passpage",
      workspace_id: null,
      title: "passpage",
      host_server_id: "srv-pandora",
      project_path: "/home/deus/projects/passpage",
      project_remote: "github.com/deus/passpage",
      git_branch: "main",
      git_head_sha: null,
      project_uid: "uid-passpage",
      workspace_kind: "main",
      default_branch: "main",
      origin_path: "/home/deus/projects/passpage",
      created_at: "2026-06-22T00:00:00Z",
      updated_at: "2026-06-26T00:00:00Z",
      dirty: false,
    },
  ],
  workspaces_sync_now: () => undefined,

  // ── MCP runtime ──
  get_mcp_runtime_status: () => ({ servers: [], running: false }),
  list_mcp_servers: () => [],

  // ── Branch listing (Thread Scope row's "from ⑂ branch" picker) ──
  //
  // Mirrors `git_list_branches_detailed`: deduped local/remote
  // branches sorted by recency. Names overlap the seeded codemux
  // worktree workspaces (mock-fixtures.ts) so the picker's
  // All/Worktrees tabs and WORKTREE badges light up, plus a few
  // local-only and remote-only rows for the kind icons.
  list_branches_detailed: () => {
    const now = Math.floor(Date.now() / 1000);
    const DAY = 86_400;
    return [
      { name: "main", last_commit_unix: now - 2 * DAY, is_local: true, is_remote: true },
      { name: "feature/auth-refactor", last_commit_unix: now - 3 * DAY, is_local: true, is_remote: true },
      { name: "fix/sidebar-flicker", last_commit_unix: now - 5 * DAY, is_local: true, is_remote: false },
      { name: "feature/40-dev-mock-tauri-runtime", last_commit_unix: now - 8 * DAY, is_local: true, is_remote: true },
      { name: "chore/port-detection", last_commit_unix: now - 12 * DAY, is_local: true, is_remote: false },
      { name: "feature/75-chat-channel", last_commit_unix: now - 15 * DAY, is_local: true, is_remote: true },
      { name: "t3code/094b1560", last_commit_unix: now - 21 * DAY, is_local: true, is_remote: false },
      { name: "feature-208-address-and-sector-support", last_commit_unix: now - 35 * DAY, is_local: false, is_remote: true },
      { name: "feature-189-search-history-delete", last_commit_unix: now - 38 * DAY, is_local: false, is_remote: true },
      { name: "fix-prod-dockerfile", last_commit_unix: now - 44 * DAY, is_local: false, is_remote: true },
      { name: "feature-186-roleGuard", last_commit_unix: now - 60 * DAY, is_local: false, is_remote: true },
    ];
  },

  // ── Workspace config / scripts (sidebar setup banner, default-branch
  //    cache) ──
  get_default_branch: () => "main",
  get_workspace_config: () => ({
    setup: [],
    teardown: [],
    run: null,
    worktree_includes: [],
  }),
  get_project_scripts: () => ({
    setup: [],
    teardown: [],
    run: null,
    worktree_includes: [],
  }),

  // ── Terminal (the body is out of scope per issue #40, but the mount
  //    MUST NOT crash). `get_terminal_status` is the load-bearing one:
  //    TerminalPane writes whatever it returns into a ref that the next
  //    render reads, so a `null` here poisons the render and black-screens
  //    the app. Return a valid "ready" payload (hides the loading
  //    overlay), then push a one-shot "(mock terminal)" banner through
  //    the PTY output channel so the pane reads as an intentional mock
  //    rather than a dead black box. ──
  get_terminal_status: (a) => ({
    session_id: a.sessionId as string,
    state: "ready",
    message: null,
    exit_code: null,
  }),
  attach_pty_output: (a) => {
    const sessionId = a.sessionId as string;
    // Newest attach wins: a fresh Channel has an empty ordering buffer,
    // so nextIndex resets to 0. The mount banner is the first frame.
    const entry: MockPtyChannelEntry = { channel: a.channel, nextIndex: 0 };
    ptyChannels.set(sessionId, entry);
    ptyChannelPush(entry, MOCK_TERMINAL_BANNER);
    return undefined;
  },
  resize_pty: () => undefined,
  write_to_pty: () => undefined,
  detach_pty_output: (a) => {
    ptyChannels.delete(a.sessionId as string);
    return undefined;
  },
  pause_pty_output: () => undefined,
  resume_pty_output: () => undefined,
  clear_agent_status: () => undefined,

  // ── Terminal scrollback (issue #128) — faithful two-store twin of
  //    `src-tauri/src/scrollback.rs`; see the store section above. ──
  save_terminal_scrollback: (a) => {
    const payload = a.payload as ScrollbackPayload;
    writeScrollbackDisk(payload);
    logScrollback(
      `save session=${shortId(payload.session_id)} bytes=${approxByteLen(payload.data)} tail=${tailOf(payload.data)}`,
    );
    return undefined;
  },
  get_terminal_scrollback: (a) => {
    const workspaceId = a.workspaceId as string;
    const paneId = a.paneId as string;
    const entry = scrollbackDisk.get(diskKey(workspaceId, paneId));
    if (!entry) {
      logScrollback(`get ws=${shortId(workspaceId)} pane=${shortId(paneId)} -> miss`);
      return null;
    }
    logScrollback(
      `get ws=${shortId(workspaceId)} pane=${shortId(paneId)} -> hit bytes=${approxByteLen(entry.payload.data)}`,
    );
    return buildScrollbackRestore(entry);
  },
  cache_terminal_scrollback: (a) => {
    const payload = a.payload as ScrollbackPayload;
    scrollbackCache.set(payload.session_id, payload);
    logScrollback(
      `cache session=${shortId(payload.session_id)} bytes=${approxByteLen(payload.data)} alt=${payload.alternate_buffer} tail=${tailOf(payload.data)}`,
    );
    return undefined;
  },
  uncache_terminal_scrollback: (a) => {
    const sessionId = a.sessionId as string;
    scrollbackCache.delete(sessionId);
    logScrollback(`uncache session=${shortId(sessionId)}`);
    return undefined;
  },
  flush_scrollback_cache: () => {
    // Drain the cache; write only non-empty payloads to the disk store
    // (empty ones are useless to restore from) — mirrors
    // `flush_cache_to_disk`. Returns the count actually written.
    let saved = 0;
    for (const payload of scrollbackCache.values()) {
      if (payload.data.length > 0) {
        writeScrollbackDisk(payload);
        saved++;
      }
    }
    scrollbackCache.clear();
    logScrollback(`flush -> ${saved}`);
    return saved;
  },

  // ── Mutators: patch in-memory state + re-emit ──
  activate_workspace: (a) => {
    if (findWorkspace(a.workspaceId)) {
      appState = { ...appState, active_workspace_id: a.workspaceId as string };
      emitAppState();
    }
    return undefined;
  },
  rename_workspace: (a) => {
    const ws = findWorkspace(a.workspaceId);
    if (ws && typeof a.title === "string") {
      ws.title = a.title;
      emitAppState();
    }
    return undefined;
  },
  set_workspace_muted: (a) => {
    const ws = findWorkspace(a.workspaceId);
    if (ws) {
      ws.notifications_muted = Boolean(a.muted);
      emitAppState();
    }
    return undefined;
  },
  close_workspace: (a) => removeWorkspace(a.workspaceId),
  close_workspace_with_worktree: (a) => removeWorkspace(a.workspaceId),
};

/** Drop a workspace from the in-memory snapshot, reassigning the active
 *  workspace if it was the one closed, then re-emit. */
function removeWorkspace(id: unknown): string {
  const next = appState.workspaces.filter((w) => w.workspace_id !== id);
  let active = appState.active_workspace_id;
  if (active === id) active = next[0]?.workspace_id ?? "";
  appState = { ...appState, workspaces: next, active_workspace_id: active };
  emitAppState();
  return String(id);
}

// ── Plugin routing ──────────────────────────────────────────────────

/** Sentinel returned by `routePlugin` when a command isn't a plugin
 *  command, so `invoke` can fall through to the default. */
const MISS = Symbol("mock-miss");

async function showOpenerToast(url: unknown): Promise<void> {
  const msg = `Would open: ${url}`;
  console.log(`[dev mock] openUrl →`, url);
  try {
    // Lazy-import so the mock doesn't eagerly pull sonner; the <Toaster />
    // is mounted by App, so the toast renders once the UI is up.
    const { toast } = await import("sonner");
    toast.message(msg);
  } catch {
    /* toast unavailable (pre-mount) — the console.log above suffices */
  }
}

function routePlugin(cmd: string, args: Args): unknown {
  // Event system — the backbone of listen()/emit()/once().
  if (cmd === "plugin:event|listen") {
    return registerListener(args.event as string, args.handler as number);
  }
  if (cmd === "plugin:event|unlisten") {
    unregisterListener(args.event as string, args.eventId as number);
    return undefined;
  }
  if (cmd === "plugin:event|emit" || cmd === "plugin:event|emit_to") {
    emitEvent(args.event as string, args.payload);
    return undefined;
  }

  // Opener — surface a toast instead of throwing/navigating away.
  if (cmd === "plugin:opener|open_url" || cmd === "plugin:opener|open_path") {
    void showOpenerToast(args.url ?? args.path);
    return undefined;
  }
  if (cmd.startsWith("plugin:opener|")) {
    console.log(`[dev mock] ${cmd}`, args);
    return undefined;
  }

  // Window controls — no-op so WindowControls / drag regions don't crash.
  if (cmd.startsWith("plugin:window|") || cmd.startsWith("plugin:webview")) {
    if (cmd.endsWith("|scale_factor")) return 1;
    if (cmd.endsWith("|theme")) return "dark";
    // is_maximized / is_minimized / is_fullscreen / is_focused / … → false
    if (/\|is_/.test(cmd)) return false;
    return undefined;
  }

  // App metadata.
  if (cmd === "plugin:app|version") return "0.0.0-dev-mock";
  if (cmd === "plugin:app|name") return "Codemux";
  if (cmd === "plugin:app|tauri_version") return "2.10.1";
  if (cmd === "plugin:app|identifier") return "dev.codemux.mock";
  if (cmd.startsWith("plugin:app|")) return undefined;

  // Process control — log the intent, do nothing.
  if (cmd.startsWith("plugin:process|")) {
    console.log(`[dev mock] ${cmd} (no-op in browser)`);
    return undefined;
  }

  // Updater — always "no update available".
  if (cmd.startsWith("plugin:updater|")) return null;

  // Dialogs — cancelled (null) so file pickers resolve cleanly.
  if (cmd.startsWith("plugin:dialog|")) return null;

  // Resource cleanup, misc plugins.
  if (cmd.startsWith("plugin:")) {
    console.warn(`[dev mock] unhandled plugin command: ${cmd}`);
    return undefined;
  }

  return MISS;
}

// ── Default fallback ────────────────────────────────────────────────

const warnedCommands = new Set<string>();

/** Shape-safe default for any command without an explicit handler.
 *  `list_*` returns `[]` so `.map()` call sites survive; everything
 *  else returns `null`. Warns once per command so gaps surface during
 *  UI iteration without spamming the console. */
function defaultResult(cmd: string): unknown {
  if (!warnedCommands.has(cmd)) {
    warnedCommands.add(cmd);
    console.warn(`[dev mock] no handler for "${cmd}" — returning default`);
  }
  return cmd.startsWith("list_") ? [] : null;
}

// ── The internals contract ──────────────────────────────────────────

async function invoke(cmd: string, args: Args = {}): Promise<unknown> {
  const handler = handlers[cmd];
  if (handler) return handler(args);

  const viaPlugin = routePlugin(cmd, args);
  if (viaPlugin !== MISS) return viaPlugin;

  return defaultResult(cmd);
}

interface TauriInternals {
  invoke: (cmd: string, args?: Args, options?: unknown) => Promise<unknown>;
  transformCallback: (cb: RawCallback, once?: boolean) => number;
  unregisterCallback: (id: number) => void;
  convertFileSrc: (filePath: string, protocol?: string) => string;
  metadata: {
    currentWindow: { label: string };
    currentWebview: { label: string; windowLabel: string };
  };
}

const internals: TauriInternals = {
  invoke,
  transformCallback,
  unregisterCallback: (id) => callbacks.delete(id),
  // The webview can't load file:// directly; in the browser we just pass
  // the path through. Mock images may 404, but nothing crashes.
  convertFileSrc: (filePath) => filePath,
  metadata: {
    currentWindow: { label: "main" },
    currentWebview: { label: "main", windowLabel: "main" },
  },
};

// `globalThis.isTauri` is read by `@tauri-apps/api`'s `isTauri()`. We
// deliberately DON'T set it — some app code uses it to distinguish real
// runtime from web, and the mock should read as "not the real thing".

(window as unknown as { __TAURI_INTERNALS__: TauriInternals }).__TAURI_INTERNALS__ =
  internals;

(
  window as unknown as {
    __TAURI_EVENT_PLUGIN_INTERNALS__: {
      unregisterListener: (event: string, eventId: number) => void;
    };
  }
).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener };

export {};
