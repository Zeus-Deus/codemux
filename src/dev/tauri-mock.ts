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
  createSeedAppState,
} from "./mock-fixtures";
import type {
  AppStateSnapshot,
  ChatModelInfo,
  CliToolInfo,
  FeatureFlags,
  PresetStoreSnapshot,
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

/**
 * Feed a one-shot banner line into a terminal pane's PTY output
 * `Channel`. A `@tauri-apps/api` `Channel` registers its internal
 * dispatcher through `transformCallback`, exposing the resulting id as
 * `channel.id`. We look that callback up and hand it a single ordered
 * message (`{ index: 0, message }`); the dispatcher forwards
 * `message` to the pane's `onmessage`, which decodes the string to
 * bytes and writes them to xterm. Best-effort: any shape mismatch is
 * swallowed so a terminal that renders differently never breaks boot.
 */
function pushMockTerminalBanner(channel: unknown): void {
  const id = (channel as { id?: number } | undefined)?.id;
  if (typeof id !== "number") return;
  const cb = callbacks.get(id);
  if (!cb) return;
  const banner =
    "\r\n  \x1b[2m(mock terminal — no PTY in plain-browser dev; " +
    "run `npm run tauri:dev` for a real shell)\x1b[0m\r\n";
  try {
    cb.fn({ index: 0, message: banner } as unknown as never);
  } catch {
    /* channel shape differs — banner is cosmetic, ignore */
  }
}

// ── Static command returns ──────────────────────────────────────────

const FEATURE_FLAGS: FeatureFlags = {
  unstable_openflow: false,
  unstable_browser_automation: false,
  unstable_indexing: false,
  // On in the mock so the seeded agent-chat workspace renders its chat
  // pane (the virtualized transcript is a primary dev-iteration
  // surface — issue #77). All chat IPC is mocked below.
  enable_agent_chat: true,
  enable_lazy_workspace_creation: false,
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
  agent_chat: { checkpoints_enabled: true },
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
// simulates a streaming reply through the real `agent_chat_event`
// channel; `window.__codemuxChatMock.streamReply()` triggers one on
// demand for scroll/perf testing.

const MOCK_CHAT_MODEL: ChatModelInfo = {
  id: "mock-sonnet",
  label: "Mock Sonnet",
  description: "Simulated model served by the dev mock",
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
    push({
      type: "user_message",
      thread_id: T,
      text: `Turn ${i + 1}: how does the virtualized transcript hold up at scale?`,
    });
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
        text: `(${i + 1}/${MOCK_CHAT_TURNS}) ${ASSISTANT_BODIES[i % ASSISTANT_BODIES.length]}`,
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
  mockChatTranscriptCache = out;
  return out;
}

let mockChatTurnSeq = 0;

/** Simulate a streaming assistant reply on the real event channel.
 *  Returns the turn id immediately; deltas tick on an interval so
 *  stick-to-bottom / scroll-up-freedom can be observed live. */
function streamMockChatReply(
  threadId: string,
  opts: { tokens?: number; intervalMs?: number } = {},
): string {
  const turnId = `live-turn-${++mockChatTurnSeq}`;
  const tokens = Math.max(1, opts.tokens ?? 120);
  const intervalMs = Math.max(5, opts.intervalMs ?? 40);
  const send = (event: unknown) =>
    emitEvent("agent_chat_event", { thread_id: threadId, event });

  send({
    type: "session_state_changed",
    thread_id: threadId,
    status: { status: "running", active_turn: turnId },
  });

  let emitted = 0;
  let fullText = "";
  const timer = window.setInterval(() => {
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
    }
  }, intervalMs);
  return turnId;
}

// Expose the stream trigger for browser-console / automation use.
(
  window as unknown as {
    __codemuxChatMock: {
      threadId: string;
      streamReply: typeof streamMockChatReply;
    };
  }
).__codemuxChatMock = {
  threadId: MOCK_CHAT_THREAD_ID,
  streamReply: streamMockChatReply,
};

const EMPTY_PRESETS: PresetStoreSnapshot = {
  presets: [],
  bar_visible: false,
  default_preset_id: null,
};

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

  // ── Theme / appearance ──
  get_current_theme: () => THEME,
  get_shell_appearance: () => SHELL_APPEARANCE,

  // ── Resource monitor ──
  get_resource_metrics: () => resourceMetrics(),

  // ── Agent chat (mocked end-to-end for the seeded chat workspace) ──
  list_chat_provider_capabilities: (a) =>
    a.provider === "claude" ? CLAUDE_CAPABILITIES : EMPTY_CAPABILITIES,
  agent_chat_list_messages: (a) =>
    a.threadId === MOCK_CHAT_THREAD_ID ? mockChatTranscript() : [],
  agent_chat_list_sessions: () => [],
  agent_chat_start_session: (a) =>
    (a.input as { thread_id: string }).thread_id,
  agent_chat_send_turn: (a) =>
    streamMockChatReply((a.input as { thread_id: string }).thread_id),
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

  // ── Presets ──
  get_presets: () => EMPTY_PRESETS,

  // ── Editors / tooling ──
  detect_editors: () => [],
  list_available_cli_tools: () => CLI_TOOLS,

  // ── GitHub / PRs (pre-seeded; never hit a real API) ──
  check_gh_available: () => true,
  check_gh_status: () => ({ status: "Authenticated", username: "mock-dev" }),
  refresh_workspace_pr: (a) => findWorkspace(a.workspaceId)?.pr_number ?? null,
  refresh_workspace_issue: () => null,
  list_incoming_prs: () => [],

  // ── Hosts (remote devices) ──
  hosts_list: () => [],

  // ── MCP runtime ──
  get_mcp_runtime_status: () => ({ servers: [], running: false }),
  list_mcp_servers: () => [],

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
  get_terminal_scrollback: () => null,
  attach_pty_output: (a) => {
    pushMockTerminalBanner(a.channel);
    return undefined;
  },
  resize_pty: () => undefined,
  write_to_pty: () => undefined,
  detach_pty_output: () => undefined,
  pause_pty_output: () => undefined,
  resume_pty_output: () => undefined,
  clear_agent_status: () => undefined,
  cache_terminal_scrollback: () => undefined,
  uncache_terminal_scrollback: () => undefined,
  save_terminal_scrollback: () => undefined,
  flush_scrollback_cache: () => 0,

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
