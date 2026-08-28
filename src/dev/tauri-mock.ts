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
import { hasToolResultImages } from "@/lib/agent-chat/tool-result-images";
import { clearPrOverviewSnapshot } from "@/lib/pr-overview-snapshot";

import {
  MOCK_CHAT_THREAD_ID,
  MOCK_DIRTY_ARCHIVE_ID,
  MOCK_HOME_DIR,
  MOCK_USER,
  MOCK_USER_IMAGE_DATA_URL,
  MOCK_WEB_REMOTE_PORT,
  MOCK_MONITORING_THREAD_ID,
  MOCK_WORKFLOW_APPROVAL_THREAD_ID,
  MOCK_WORKFLOW_COMPLETE_THREAD_ID,
  MOCK_WORKFLOW_RUNNING_THREAD_ID,
  MOCK_CHECK_LOG_EXCERPT,
  MOCK_COMMITS_AHEAD,
  MOCK_CREATE_PR_DIRTY,
  MOCK_CREATE_PR_PATH,
  MOCK_LOCAL_ONLY_PATH,
  MOCK_PR_CHECKS,
  MOCK_PR_TIMELINES,
  MOCK_PR_DIFFS,
  MOCK_PR_HISTORY,
  MOCK_PR_OVERVIEW,
  MOCK_PR_TEMPLATE,
  MOCK_PR_TEMPLATE_ROOT,
  MOCK_PR_VIEWER,
  MOCK_PR_REVIEW_REQUESTS,
  MOCK_PR_INLINE_COMMENTS,
  MOCK_PR_REVIEW_THREADS,
  MOCK_PR_PATH_TO_NUMBER,
  MOCK_PR_REVIEWS,
  MOCK_TEMPLATE_PATH_PATTERN,
  MOCK_UNREACHABLE_ROOT,
  MOCK_PULL_REQUESTS,
  MOCK_SUBMIT_FAILURE_PR,
  createSeedAppState,
  mockListDirectory,
  mockReadFile,
  mockWebRemoteEndpoints,
  mockUsageSummary,
  mockUsageExportCsv,
  mockWebRemotePairing,
  mockWebRemoteSessions,
  richChatTurnEnvelopes,
  monitoringEnvelopes,
  subagentTurnEnvelopes,
  workflowApprovalEnvelopes,
  workflowCompleteEnvelopes,
  workflowRunningEnvelopes,
} from "./mock-fixtures";
import {
  STRESS_THREAD_PREFIX,
  getStressFixture,
  stressChatTranscript,
} from "./stress-fixture";
import type {
  ScrollbackMeta,
  ScrollbackPayload,
  ScrollbackRestore,
} from "@/tauri/commands";
import type {
  AgentBrowserSession,
  AppStateSnapshot,
  ArchivedWorkspaceSnapshot,
  CheckInfo,
  ChatModelInfo,
  FeatureFlags,
  PaneNodeSnapshot,
  PresetStoreSnapshot,
  PrReviewThread,
  ProviderOperations,
  PullRequestInfo,
  ReviewComment,
  InlineReviewComment,
  SurfaceSnapshot,
  TabSnapshot,
  TerminalPreset,
  ProviderChatCapabilities,
  ProviderHealthReport,
  ResourceMetricsSnapshot,
  ShellAppearance,
  ThemeColors,
  UserSettings,
  WebRemoteBindScope,
  WebRemoteSessionView,
  WebRemoteStatus,
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

// ── Pull request mock state ──
//
// The fixture map is the seed; this is the copy the mutators write to,
// so merging or editing a PR in the browser sticks for the session
// without the fixture module ever being touched.
const mutablePrs: Record<number, PullRequestInfo> = structuredClone(MOCK_PULL_REQUESTS);
const mutablePrReviews: Record<number, ReviewComment[]> = {};
/** Inline comments posted during the session, on top of the fixture. */
const mutablePrInline: Record<number, InlineReviewComment[]> = {};
/** PRs whose branch has been rewritten by `__codemuxMockForcePush`. */
const forcePushedPrs = new Set<number>();

function mutablePr(number: number): PullRequestInfo | undefined {
  return mutablePrs[number];
}

/** Threads mutate here (reply, resolve) the way they would host-side. */
const mutablePrThreads: Record<number, PrReviewThread[]> = {};

function threadsFor(number: number): PrReviewThread[] {
  return (mutablePrThreads[number] ??= structuredClone(
    MOCK_PR_REVIEW_THREADS[number] ?? [],
  ));
}

/** Whether the seeded reply always fails — see `__codemuxMockThreadReplyFail`. */
let mockThreadReplyFails = false;

/** How long a reply takes. Long enough to see the in-flight state by
 *  default; `__codemuxMockThreadReplyDelay` stretches it further. */
let mockThreadReplyDelayMs = 400;

function withThreadOps(operations: ProviderOperations): ProviderOperations {
  if (mockThreadOpsDeclared) return operations;
  return { ...operations, thread_reply: false, thread_resolve: false };
}

/** The seeded host for a checkout or project root, resolved the way
 *  `check_provider_auth` resolves it. */
function providerKindAt(path: string): string {
  const workspace = appState.workspaces.find(
    (w) => w.cwd === path || w.project_root === path,
  );
  return workspace?.provider_kind ?? "github";
}

/**
 * The refusal the real command layer raises for an operation the host's
 * adapter never declared — `check_operation` in `commands/github.rs`,
 * word for word.
 *
 * The mock has to raise it too. A host whose rows appear here but are
 * refused in production is not a fixture of the app; it is a demo of a
 * different product, and the divergence only ever surfaces as a bug
 * report about a page that "worked in dev". Returned rather than thrown
 * so a handler can gate on it in one line.
 */
function undeclaredRefusal(
  path: string,
  op: keyof ProviderOperations,
  describe: string,
): string | null {
  const kind = providerKindAt(path);
  const operations = withThreadOps(MOCK_PROVIDER_OPERATIONS[kind] ?? NO_MOCK_OPERATIONS);
  if (operations[op]) return null;
  const name = PROVIDER_DISPLAY_NAMES[kind] ?? "an unrecognised host";
  return (
    `${name} does not declare this operation — Codemux cannot ${describe} here. ` +
    "Open it in the browser instead."
  );
}

/** `ProviderKind::display_name` in `git_provider/detect.rs`. */
const PROVIDER_DISPLAY_NAMES: Record<string, string> = {
  github: "GitHub",
  gitlab: "GitLab",
  bitbucket: "Bitbucket",
  azure_devops: "Azure DevOps",
};

function inlineCommentsFor(number: number): InlineReviewComment[] {
  return (mutablePrInline[number] ??= [...(MOCK_PR_INLINE_COMMENTS[number] ?? [])]);
}

/** The diff the Code tab gets: the rewritten one once a force-push has
 *  been simulated for this PR. */
function prDiffFor(number: number, full: boolean): string {
  const entry = MOCK_PR_DIFFS[number];
  if (!entry) return "";
  if (!full) return entry.nameOnly;
  return forcePushedPrs.has(number) ? (entry.afterForcePush ?? entry.full) : entry.full;
}

/** Pull requests opened through the form this session, by checkout path. */
const createdPrPaths: Record<string, number> = {};

/** Numbers for pull requests created during the session. Deliberately
 *  above every seeded number so nothing can collide. */
let nextMockPrNumber = 900;

/** The PR for a checkout path, via the seeded path→number map. */
function prForPath(path: unknown): PullRequestInfo | undefined {
  const key = String(path ?? "");
  const number = createdPrPaths[key] ?? MOCK_PR_PATH_TO_NUMBER[key];
  return number == null ? undefined : mutablePrs[number];
}

/** The CI rollup the backend reduces host-side, reproduced here from
 *  the same seeded checks the detail column renders. */
function rollupOf(number: number): string {
  const checks = MOCK_PR_CHECKS[number] ?? [];
  if (checks.length === 0) return "none";
  if (checks.some((c) => c.conclusion === "fail")) return "failing";
  if (checks.some((c) => c.conclusion === "pending")) return "pending";
  return "passing";
}

/**
 * Session overrides for the two things a pull-request toast watches.
 *
 * Written by `__codemuxMockReviewRequest` / `__codemuxMockCiFail` so the
 * two toasts can be fired on demand rather than waited for: both are
 * *transitions*, so nothing seeded as already-failing could ever raise
 * one.
 */
const reviewRequestOverrides: Record<number, string[]> = {};
const checksRollupOverrides: Record<number, string> = {};
/** The per-check list behind a forced rollup, so the toast can name the
 *  check that failed and [Fix] can hand an agent something specific. */
const checksDetailOverrides: Record<number, CheckInfo[]> = {};

/**
 * One overview row, derived from the PR the detail column will open.
 *
 * `carriesChecks` mirrors the real split: the gh path answers `null` and
 * pays for a second call, while the glab path reads the pipeline off the
 * merge request list it already fetched and answers in one. Both are
 * exercised in dev because the mock's roots include both products.
 */
function overviewItem(number: number) {
  const pr = mutablePrs[number];
  if (!pr) return null;
  return {
    number: pr.number,
    title: pr.title,
    author: pr.author ?? "",
    head_branch: pr.head_branch,
    is_draft: pr.is_draft,
    // Neither product serves line counts on a list call: gh is asked for
    // them by the stats half, and GitLab would charge a diff-stat
    // request per row, so its rows never get them at all.
    additions: null,
    deletions: null,
    review_decision: pr.review_decision,
    // Never measured on the list call, by either product.
    checks: null,
    review_requested_from:
      reviewRequestOverrides[number] ?? MOCK_PR_REVIEW_REQUESTS[number] ?? [],
    updated_at: pr.updated_at,
    url: pr.url,
  };
}

/** The rollup the stats call would report, session overrides included. */
function overviewRollup(number: number): string {
  return checksRollupOverrides[number] ?? rollupOf(number);
}

/**
 * How long each half of the overview takes.
 *
 * Both are shell-outs to a CLI in the real app, and a mock that answers
 * in zero milliseconds hides the two things this page was rebuilt for:
 * that the listing arrives before the checks do, and that the carried
 * paint is what stands in for the listing until it lands. 600 + 800 is
 * a plausible healthy repository and slow enough to watch.
 *
 * `window.__codemuxMockPrListDelay(ms)` / `__codemuxMockPrStatsDelay(ms)`
 * move them.
 */
/** Both knobs survive a reload, because the thing they exist to make
 *  visible — what the very first paint looks like — only happens on
 *  one. */
function storedDelay(key: string, fallback: number): number {
  const raw = Number(localStorage.getItem(`codemux:dev:${key}`));
  return Number.isFinite(raw) && raw >= 0 && raw > 0 ? raw : fallback;
}

let PR_LIST_DELAY_MS = storedDelay("pr-list-delay", 600);
let PR_STATS_DELAY_MS = storedDelay("pr-stats-delay", 800);

/** Session switches for the strip: the whole world unreachable, or just
 *  the slow half of it. Flipped from the console, not seeded. */
let prOverviewOutage = false;
let prStatsOutage = false;
/**
 * The host is reachable and refusing: the account's hourly API budget is
 * spent.
 *
 * Its own switch rather than a flavour of the outage above, because the
 * page treats it as its own thing — it stops polling instead of
 * retrying, and says when it will resume rather than offering a button
 * as the answer. Being able to see that state is the point of having it
 * here.
 */
let prBudgetSpent = false;

/** Verbatim what `gh pr list` prints when the GraphQL budget is gone. */
const RATE_LIMIT_REFUSAL =
  "GraphQL: API rate limit already exceeded for user ID 100132710.";

/**
 * The expensive half of one row.
 *
 * A row nothing can answer for is *omitted* rather than reported as
 * "none": the two mean different things, and only a host that actually
 * looked may say a pull request has no checks. The listing half never
 * fills `checks` at all — neither product does, which is the whole
 * reason this second call exists.
 */
function overviewStats(number: number) {
  const pr = mutablePrs[number];
  if (!pr) return null;
  if (checksRollupOverrides[number] == null && MOCK_PR_CHECKS[number] == null) {
    return null;
  }
  return {
    number,
    checks: overviewRollup(number),
    additions: pr.additions ?? null,
    deletions: pr.deletions ?? null,
  };
}

/** Mirror a PR state change onto every workspace carrying that PR, so
 *  the sidebar icon and the panel agree — the real backend does this
 *  through its 60s poll. */
function syncWorkspacePrState(number: number, state: string): void {
  let mutated = false;
  for (const ws of appState.workspaces) {
    if (ws.pr_number === number) {
      ws.pr_state = state;
      mutated = true;
    }
  }
  if (mutated) emitAppState();
}

/** Re-emit the current snapshot so subscribers (`useAppStateInit`) pick
 *  up an in-memory mutation, mirroring the backend's `emit_app_state`. */
/** Walk every workspace surface for the `agent_chat` pane bound to
 *  `threadId` — the mock's stand-in for the backend's thread→pane walk
 *  (`find_agent_chat_pane_id`). `null` when nothing carries the thread. */
function findChatPaneIdForThread(threadId: string): string | null {
  const walk = (node: PaneNodeSnapshot): string | null => {
    if (node.kind === "split") {
      for (const child of node.children) {
        const found = walk(child);
        if (found) return found;
      }
      return null;
    }
    return node.kind === "agent_chat" && node.thread_id === threadId
      ? node.pane_id
      : null;
  };
  for (const ws of appState.workspaces) {
    for (const surface of ws.surfaces) {
      const found = walk(surface.root);
      if (found) return found;
    }
  }
  return null;
}

function findChatPaneLocation(threadId: string): {
  workspace: WorkspaceSnapshot;
  surface: SurfaceSnapshot;
  paneId: string;
} | null {
  const walk = (node: PaneNodeSnapshot): string | null => {
    if (node.kind === "agent_chat" && node.thread_id === threadId)
      return node.pane_id;
    if (node.kind === "split") {
      for (const child of node.children) {
        const found = walk(child);
        if (found) return found;
      }
    }
    return null;
  };
  for (const workspace of appState.workspaces) {
    for (const surface of workspace.surfaces) {
      const paneId = walk(surface.root);
      if (paneId) return { workspace, surface, paneId };
    }
  }
  return null;
}

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

// ── MCP fixtures (Settings → MCP Servers) ──

/** Discovered server configs across provider scopes. Mirrors
 *  `McpServerConfig` on the Rust side. */
const MOCK_MCP_SERVERS = [
  {
    id: "codemux-self",
    name: "codemux",
    sources: ["codemux"],
    command: "/usr/bin/codemux",
    args: ["mcp"],
    env: {},
    disabled: false,
    transport: "stdio",
    raw: null,
  },
  {
    id: "docs-kb",
    name: "docs-kb",
    sources: ["claudeUser", "openCodeUser"],
    command: "docker",
    args: ["exec", "-i", "docs-kb", "python", "/app/server.py"],
    env: {},
    disabled: false,
    transport: "stdio",
    raw: null,
  },
  {
    id: "code-search",
    name: "code-search",
    sources: ["codexUser"],
    command: "npx",
    args: ["-y", "@acme/code-search-mcp"],
    env: {},
    disabled: false,
    transport: "stdio",
    raw: null,
  },
  {
    id: "issue-tracker",
    name: "issue-tracker",
    sources: ["codexUser"],
    command: "https://mcp.example.com/mcp",
    args: [],
    env: {},
    disabled: false,
    transport: "http",
    raw: null,
  },
  {
    id: "notes-sync",
    name: "notes-sync",
    sources: ["openCodeUser"],
    command: "uvx",
    args: ["notes-sync-mcp", "-y"],
    env: {},
    disabled: false,
    transport: "stdio",
    raw: null,
  },
] as const;

/** Live runtime rows keyed to the configs above. Tool counts sum to
 *  86 (> the advisory threshold) so the "many tools" note renders. */
const MOCK_MCP_RUNTIMES = [
  {
    id: "codemux-self",
    name: "codemux",
    status: { kind: "running", toolCount: 57 },
    toolsCount: 57,
    errorMessage: null,
    stderrTail: null,
    startedAtMs: 0,
  },
  {
    id: "docs-kb",
    name: "docs-kb",
    status: { kind: "running", toolCount: 12 },
    toolsCount: 12,
    errorMessage: null,
    stderrTail: null,
    startedAtMs: 0,
  },
  {
    id: "code-search",
    name: "code-search",
    status: { kind: "running", toolCount: 9 },
    toolsCount: 9,
    errorMessage: null,
    stderrTail: null,
    startedAtMs: 0,
  },
  {
    id: "issue-tracker",
    name: "issue-tracker",
    status: { kind: "running", toolCount: 8 },
    toolsCount: 8,
    errorMessage: null,
    stderrTail: null,
    startedAtMs: 0,
  },
  {
    id: "notes-sync",
    name: "notes-sync",
    status: { kind: "errored", message: "spawn uvx ENOENT" },
    toolsCount: 0,
    errorMessage: "spawn uvx ENOENT",
    stderrTail: "uvx: command not found",
    startedAtMs: 0,
  },
];

/** Deterministic per-server tool list for the tool modal. */
function mockMcpToolsForServer(id: string) {
  const runtime = MOCK_MCP_RUNTIMES.find((r) => r.id === id);
  const count = runtime?.toolsCount ?? 0;
  return Array.from({ length: count }, (_, i) => ({
    name: `tool_${i + 1}`,
    prefixedName: `mcp__${id === "codemux-self" ? "codemux" : id}__tool_${i + 1}`,
    description: `Sample tool ${i + 1} exposed by ${id}.`,
    inputSchema: { type: "object", properties: {} },
    serverId: id,
  }));
}

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

/** Monotonic PTY-output attach generation, minted by `attach_pty_output`
 *  so callers get a real token to thread into detach/pause/resume — the
 *  same contract the desktop backend and web-remote dispatch honour. */
let ptyOutputGeneration = 0;

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
 *  mock twin of `forward_event`'s channel arm. No channel → drop.
 *
 *  `persisted_id` is always null here: the mock's transcripts are
 *  generated, and simulated live events are never appended to them, so
 *  there is no row for them to point at. The frontend treats that as
 *  "ephemeral event" and leaves its cursor where the tail read put it. */
function emitChatEvent(threadId: string, event: unknown): void {
  const entry = chatChannels.get(threadId);
  if (!entry) return;
  chatChannelPush(entry, { thread_id: threadId, event, persisted_id: null });
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

function buildScrollbackRestore(
  entry: MockScrollbackDiskEntry,
): ScrollbackRestore {
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
 * Emit an OSC 7 "current working directory" sequence into one attached PTY
 * channel — or every attached channel when `sessionId` is omitted.
 *
 * This is how a real shell with integration reports `cd`, so pushing it
 * through the mock's normal output path drives the genuine production
 * handler in `TerminalPane` (parse → store → header hint) rather than
 * poking app state from the side. It's the only way to see the pane
 * header's cwd hint in plain-browser dev, where there is no shell.
 *
 *   window.__codemuxTerminalMock.emitOsc7("/home/zeus/proj/src-tauri")
 */
function emitOsc7(cwd: string, sessionId?: string): void {
  // `ESC ] 7 ; file://<host>/<path> BEL` — the form vte, fish, and
  // kitty's shell integration all emit.
  const seq = `\x1b]7;file://mock${encodeURI(cwd)}\x07`;
  const targets = sessionId ? [sessionId] : [...ptyChannels.keys()];
  for (const id of targets) {
    const entry = ptyChannels.get(id);
    if (entry) ptyChannelPush(entry, seq);
  }
  console.info(`[mock::terminal] osc7 cwd=${cwd} sessions=${targets.length}`);
}

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
    typography_mode: "simple",
    interface_font_family: null,
    interface_font_size: 16,
    conversation_font_family: null,
    conversation_font_size: 14,
    code_font_family: null,
    code_font_size: 13,
    terminal_font_family: null,
    terminal_font_size: 13,
    show_resource_monitor: true,
  },
  editor: { default_ide: null },
  terminal: { scrollback_limit: 10_000, cursor_style: "bar" },
  git: { default_base_branch: "main" },
  // One seeded self-hosted mapping so the custom-hosts editor renders a
  // populated row (and its remove affordance) rather than only the empty
  // state under `npm run dev`.
  source_control: {
    // `gitlab.example.com` is where the seeded merge requests live, and
    // declaring it is what lets one of their URLs route to the page —
    // the same path a user takes for a self-hosted instance.
    custom_hosts: { "git.acme.internal": "gitlab", "gitlab.example.com": "gitlab" },
    open_pr_links_in_browser: false,
  },
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
  agent_chat: {
    checkpoints_enabled: true,
    background_browser_desktop_viewport: true,
  },
  browser: { default_viewport: null },
};

const EMPTY_CAPABILITIES: ProviderChatCapabilities = {
  models: [],
  effort_granularity: "per_session",
  effort_label_map: {},
  permission_modes: [],
  default_permission_mode: null,
  permission_granularity: "per_session",
};

const MOCK_SKILL_BODY =
  "Use the repository context to review the requested change and report actionable findings.";

function mockSkill(
  id: string,
  name: string,
  provider: "claude" | "codex" | "opencode",
  scope: "project" | "user" | "system",
) {
  const filePath = `/mock/${provider}/${scope}/${name}/SKILL.md`;
  return {
    id,
    name,
    description: `${provider} ${scope} skill for ${name}`,
    provider,
    scope,
    skillDir: filePath.replace(/\/SKILL\.md$/, ""),
    filePath,
    body: MOCK_SKILL_BODY,
    rawFrontmatter: { name },
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
    provenance: "provider_catalog",
    readable: true,
    sourceEnabled: true,
    projections: (["claude", "codex", "opencode"] as const).map(
      (targetProvider) => ({
        targetProvider,
        availability:
          targetProvider === provider ? "native" : "explicit-portable",
        compatibility: "compatible",
        reasons: [],
        invocation:
          targetProvider === "codex"
            ? "codex-skill-item"
            : targetProvider === provider && provider === "claude"
              ? "native-command"
              : "prompt-prefix",
      }),
    ),
  };
}

const MOCK_SKILL_INVENTORY = {
  skills: [
    mockSkill("mock-claude-review", "review", "claude", "project"),
    mockSkill("mock-codex-review", "review", "codex", "user"),
    mockSkill("mock-opencode-deploy", "deploy", "opencode", "system"),
  ],
  errors: [],
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

// ── Pending AskUserQuestion seed (`?askq=1`) ────────────────────────
//
// The seeded transcript carries a RESOLVED `user-input` request so the
// reply bubble renders. The composer-docked questionnaire panel needs a
// PENDING one, which a persisted transcript can't provide: cold replay
// expires orphan requests (`finalizeReplay`), so a pending
// `request_opened` has to arrive as a LIVE channel event exactly like
// the real provider sends it. Opt in with `?askq=1` so normal dev flows
// (and every screenshot of the seeded thread) are unaffected.

const MOCK_ASKQ_REQUEST_ID = "seed-user-input-pending";

/** `?askq=1` — seed a pending AskUserQuestion on the main chat thread. */
function askQuestionSeedEnabled(): boolean {
  try {
    return new URLSearchParams(location.search).get("askq") === "1";
  } catch {
    return false;
  }
}

/** One pending seed per page load; re-attaching (pane remount, workspace
 *  switch) must not stack duplicate requests on the same thread. */
let askQuestionSeeded = false;

const MOCK_ASKQ_PAYLOAD = {
  questions: [
    {
      header: "Migration",
      question: "How should the settings store be migrated?",
      multiSelect: false,
      options: [
        {
          label: "Write a versioned migration",
          description: "Bump the schema version and transform on read.",
          preview:
            "// migrations/003_settings.ts\n" +
            "export function up(prev: SettingsV2): SettingsV3 {\n" +
            "  return { ...prev, version: 3, theme: prev.theme ?? 'system' };\n" +
            "}",
        },
        {
          label: "Reset to defaults",
          description: "Drop the old file; users re-pick their preferences.",
        },
        {
          label: "Read both shapes",
          description: "Keep a permissive parser and never migrate on disk.",
        },
      ],
    },
    {
      header: "Rollout",
      question: "Which safeguards should ship with it?",
      multiSelect: true,
      options: [
        {
          label: "Backup the old file",
          description: "Copy to settings.bak before writing.",
        },
        {
          label: "Log every transform",
          description: "One line per migrated key, at info level.",
        },
        {
          label: "Feature-flag the rollout",
          description: "Gate behind CODEMUX_SETTINGS_V3 for one release.",
        },
      ],
    },
  ],
};

/** Push the pending request through the freshly-attached channel. The
 *  small delay lets the pane's cold hydrate settle first, so the live
 *  event merges onto the replayed transcript instead of racing it. */
function seedPendingAskQuestion(threadId: string): void {
  if (askQuestionSeeded || !askQuestionSeedEnabled()) return;
  if (threadId !== MOCK_CHAT_THREAD_ID) return;
  askQuestionSeeded = true;
  setTimeout(() => {
    emitChatEvent(threadId, {
      type: "request_opened",
      thread_id: threadId,
      turn_id: "seed-askq-turn",
      request_id: MOCK_ASKQ_REQUEST_ID,
      request_kind: "user-input",
      payload: MOCK_ASKQ_PAYLOAD,
      tool_use_id: null,
    });
  }, 600);
}

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
  // Mirrors the real Claude backend: fast mode is clamped off (the SDK
  // capability flag is not an entitlement check — silent standard
  // fallback without Extra Usage), so dev never shows the speed picker
  // for Claude rows either.
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

const CODEX_UTILITY_CAPABILITIES: ProviderChatCapabilities = {
  models: [
    {
      ...MOCK_CHAT_MODEL,
      id: "gpt-5.6-luna",
      label: "GPT-5.6 Luna",
      description: "Fast, inexpensive model for high-volume utility work",
      effort_levels: ["low", "medium", "high"],
      default_effort: "low",
      supports_images: true,
    },
  ],
  effort_granularity: "per_turn",
  effort_label_map: { low: "Low", medium: "Medium", high: "High" },
  permission_modes: [],
  default_permission_mode: "danger-full-access",
  permission_granularity: "per_session",
};

/** Mirrors Cursor's ACP `cursor/list_available_models` extension: options
 * are per model, not provider-wide. This keeps browser-dev useful for
 * verifying that controls appear and disappear as the selected row changes. */
const CURSOR_CAPABILITIES: ProviderChatCapabilities = {
  models: [
    {
      ...MOCK_CHAT_MODEL,
      id: "cursor-auto",
      label: "Cursor Auto",
      description: "Dynamically selected by Cursor",
      effort_levels: ["low", "medium", "high", "max"],
      default_effort: "medium",
      context_window_options: [
        {
          value: "200k",
          label: "200K",
          is_default: true,
          context_window_tokens: 200_000,
        },
        {
          value: "1m",
          label: "1M",
          is_default: false,
          context_window_tokens: 1_000_000,
        },
      ],
      supports_fast_mode: true,
      supports_images: true,
    },
    {
      ...MOCK_CHAT_MODEL,
      id: "cursor-small",
      label: "Cursor Small",
      description: "No model-specific options",
      // The real harvest marks every Cursor row image-capable; inheriting
      // the shared mock's `false` made attachments look unsupported.
      supports_images: true,
    },
  ],
  effort_granularity: "per_turn",
  effort_label_map: {
    low: "Low",
    medium: "Medium",
    high: "High",
    max: "Max",
  },
  permission_modes: [
    {
      value: "ask",
      label: "Ask first",
      description: "Ask before commands or edits that need approval.",
      is_default: false,
    },
    {
      value: "agent",
      label: "Full access",
      description: "Allow Cursor Agent to work without approval prompts.",
      is_default: true,
    },
  ],
  default_permission_mode: "agent",
  permission_granularity: "per_turn",
};

/** Number of simulated turns in the seeded transcript. Every 5th turn
 * includes an 8-call tool burst (exercises run folding). At 520 turns the
 * derived transcript still has more than 1,100 independently virtualized
 * slots, so fast-scroll and bounded-DOM behavior are visible in devtools. */
const MOCK_CHAT_TURNS = 520;

const ASSISTANT_BODIES = [
  "Short answer: yes — the transcript only mounts the rows that intersect the viewport.",
  "Here's the longer explanation.\n\nThe message list used to render every item with a plain `.map()`, which meant a 5,000-message session mounted 5,000 DOM subtrees. Virtualization replaces that with a window: rows are measured as they appear and recycled as they leave.\n\n- Stable keys keep React row identity\n- `React.memo` still skips untouched rows\n- The tail snap only fires while you're pinned to the bottom",
  "Let me check a few files to confirm the behavior before answering.",
  "```ts\nconst distance = el.scrollHeight - el.scrollTop - el.clientHeight;\npinned = distance <= PIN_THRESHOLD_PX;\n```\n\nThat predicate decides whether the next token snaps the view to the tail or leaves you reading history in peace.",
  "Done. The change keeps the DOM bounded regardless of conversation length, so session-open time and scroll cost stop scaling with history size.\n\nA second paragraph pads this message out so row heights vary — fixed-height assumptions are exactly what dynamic measurement has to absorb.\n\nAnd a third paragraph for good measure, because real assistant turns are rarely uniform.",
];

let mockChatTranscriptCache: string[] | null = null;

// ── Cursor reads + lazy tool results (mirrors commands/agent_chat.rs) ──

/** Same threshold the Rust read path applies. */
const MOCK_LAZY_TOOL_RESULT_THRESHOLD_BYTES = 32 * 1024;
const MOCK_LAZY_TOOL_RESULT_PREVIEW_BYTES = 2 * 1024;

interface MockMessageRow {
  id: number;
  payload: string;
  created_at_ms: number;
}

/** Row id → the UNSHAPED payload, so `agent_chat_get_tool_result` can
 *  serve the full body the list read stubbed out. */
const mockFullPayloadById = new Map<number, string>();
const mockThreadRowCache = new Map<string, MockMessageRow[]>();

/** The raw persisted payloads a thread would hold. */
function mockThreadPayloads(threadId: string): string[] {
  // Under a stress fixture the generated threads AND the seeded thread serve
  // the synthetic transcript — hydration cost is one of the things being
  // measured, and the curated 520-turn showcase is not that profile.
  const fixture = getStressFixture();
  if (
    fixture &&
    (threadId.startsWith(STRESS_THREAD_PREFIX) ||
      threadId === MOCK_CHAT_THREAD_ID)
  ) {
    return stressChatTranscript(threadId, fixture);
  }
  if (threadId === MOCK_CHAT_THREAD_ID) return mockChatTranscript();
  return mockWorkflowTranscript(threadId) ?? [];
}

/** Ids are per-thread here (the real table is global), which is all the
 *  frontend's cursor arithmetic depends on. */
function mockThreadRows(threadId: string): MockMessageRow[] {
  const cached = mockThreadRowCache.get(threadId);
  if (cached) return cached;
  const payloads = mockThreadPayloads(threadId);
  const firstCreatedAt = Date.now() - payloads.length * 725;
  const rows = payloads.map((payload, index) => {
    const id = index + 1;
    mockFullPayloadById.set(id, payload);
    return {
      id,
      payload: mockShapePayload(id, payload),
      created_at_ms: firstCreatedAt + index * 725,
    };
  });
  mockThreadRowCache.set(threadId, rows);
  return rows;
}

/** Mirror of `shape_persisted_payload`: replace an oversized, non-image
 *  tool-result body with a stub. Exported so a test can pin the parity
 *  with the Rust twin on shapes the stress fixture never produces.
 *
 *  The named presets spread their payload budget over enough turns that
 *  each individual result lands UNDER the threshold; to exercise the
 *  lazy path in `npm run dev`, ask for few turns and a big budget:
 *  `?fixture={"chatEvents":200,"payloadMb":5}` → ~130 KB per result. */
export function mockShapePayload(rowId: number, payload: string): string {
  if (payload.length <= MOCK_LAZY_TOOL_RESULT_THRESHOLD_BYTES) return payload;
  let value: Record<string, unknown>;
  try {
    value = JSON.parse(payload) as Record<string, unknown>;
  } catch {
    return payload;
  }
  if (value.type !== "item_completed") return payload;
  const item = value.item as Record<string, unknown> | undefined;
  if (!item || item.kind !== "tool_result" || item.content == null)
    return payload;
  const content = item.content;
  // The same accept-set the Rust side mirrors — this IS the frontend's.
  if (hasToolResultImages(content)) return payload;
  const serialized = JSON.stringify(content);
  if (serialized.length <= MOCK_LAZY_TOOL_RESULT_THRESHOLD_BYTES)
    return payload;
  const text = mockStringifyToolContent(content);
  item.content = {
    __codemux_lazy_tool_result: {
      row_id: rowId,
      bytes: serialized.length,
      preview: mockTruncateOnCharBoundary(
        text,
        MOCK_LAZY_TOOL_RESULT_PREVIEW_BYTES,
      ),
      line_count: text.split("\n").length,
      has_images: false,
    },
  };
  return JSON.stringify(value);
}

/** Mirror of `stringify_tool_content`: flatten content to the text the
 *  collapsed card renders, joining block entries' `text` fields. Getting
 *  this wrong would hand the mock a preview shaped nothing like the real
 *  one — the point of the mock is to exercise the real shape. */
function mockStringifyToolContent(content: unknown): string {
  if (content == null) return "";
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((entry) => {
        if (entry == null) return "";
        if (typeof entry === "string") return entry;
        const text = (entry as { text?: unknown }).text;
        return typeof text === "string" ? text : JSON.stringify(entry, null, 2);
      })
      .join("\n");
  }
  const text = (content as { text?: unknown }).text;
  return typeof text === "string" ? text : JSON.stringify(content, null, 2);
}

/** Mirror of `truncate_on_char_boundary`. The Rust cap is UTF-8 BYTES, so
 *  a `String.slice` (UTF-16 code units) would disagree on any non-ASCII
 *  body — and could split a character where Rust never would. */
function mockTruncateOnCharBoundary(text: string, maxBytes: number): string {
  const encoded = new TextEncoder().encode(text);
  if (encoded.length <= maxBytes) return text;
  let end = maxBytes;
  // Walk back off any UTF-8 continuation byte (0b10xxxxxx).
  while (end > 0 && (encoded[end] & 0b1100_0000) === 0b1000_0000) end -= 1;
  return new TextDecoder().decode(encoded.subarray(0, end));
}

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
      client_nonce: `seed-nonce-${i + 1}`,
      text: isLast
        ? "Implement the fix for #57298."
        : `Turn ${i + 1}: how does the virtualized transcript hold up at scale?`,
    });
    if (isLast) {
      // Final turn is a curated showcase: a reasoning block, a tool group
      // card, a diff card and a task summary card, all landing at the tail
      // so they're visible the moment the seeded thread opens (design QA).
      for (const envelope of richChatTurnEnvelopes(T, turnId)) push(envelope);
      // Context-usage snapshot so the composer's context meter renders
      // with the seeded thread (donut ring + popover: % · used/max,
      // total-processed row, compaction note). Same persisted envelope
      // the real backend stores; hydrate replays it through the reducer.
      push({
        type: "context_usage_updated",
        thread_id: T,
        usage: {
          used_tokens: 224_302,
          total_processed_tokens: 3_104_886,
          max_tokens: 1_000_000,
          compacts_automatically: true,
        },
      });
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
  // A pull-request URL in the transcript, so in-app link routing (5c) is
  // exercisable in browser dev: this one names a seeded pull request in
  // an open project, so clicking it opens the Pull Requests page on that
  // pull request, and shift-clicking still goes to the browser.
  const linkTurnId = "seed-pr-link";
  push({
    type: "user_message",
    thread_id: T,
    client_nonce: "seed-nonce-pr-link",
    text: "Where did the monitoring work land?",
  });
  push({
    type: "item_completed",
    thread_id: T,
    turn_id: linkTurnId,
    item: {
      kind: "assistant_text",
      text: "It shipped as https://github.com/example/codemux/pull/482 — the checks are green there.",
    },
  });
  push({
    type: "turn_completed",
    thread_id: T,
    turn_id: linkTurnId,
    status: { kind: "success" },
    usage: null,
  });

  // Final showcase turn: the subagent work-log fixture — one completed
  // subagent, one still running — so the compact transcript row, panel
  // watch surface, and drill-in all render on open. Landing last keeps it
  // visible the moment the thread hydrates.
  const subTurnId = "seed-subagents";
  push({
    type: "user_message",
    thread_id: T,
    client_nonce: "seed-nonce-subagents",
    text: "Implement clipboard-paste fallback",
    // Seed an attached image on this tail turn so the
    // thumbnail + lightbox render the moment the default seeded thread
    // opens. The persisted envelope shape is `{ path, media_type }`;
    // the mock uses a data URL as the `path` so it renders in a plain
    // browser (no `convertFileSrc`).
    images: [{ path: MOCK_USER_IMAGE_DATA_URL, media_type: "image/png" }],
  });
  for (const envelope of subagentTurnEnvelopes(T, subTurnId)) push(envelope);
  // A resolved AskUserQuestion (`user-input`) so the transcript shows the
  // user's answer echoed back as a reply bubble (design QA for the
  // UserInputAnswer row).
  push({
    type: "request_opened",
    thread_id: T,
    turn_id: subTurnId,
    request_id: "seed-user-input-1",
    request_kind: "user-input",
    payload: {
      questions: [
        {
          header: "Approach",
          question: "How should we handle the clipboard fallback?",
          multiSelect: false,
          options: [
            { label: "Feature-detect and degrade", description: "" },
            { label: "Always use the legacy path", description: "" },
          ],
        },
      ],
    },
    tool_use_id: null,
  });
  push({
    type: "request_resolved",
    thread_id: T,
    request_id: "seed-user-input-1",
    decision: {
      decision: "allow",
      updated_input: {
        questions: [],
        answers: {
          "How should we handle the clipboard fallback?":
            "Feature-detect and degrade",
        },
      },
    },
  });
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

let mockChatRevertCutoff: number | null = null;

function mockTailTurnCheckpoint() {
  const clientNonce = "seed-nonce-subagents";
  const userIndex = mockChatTranscript().findIndex((payload) => {
    const parsed = JSON.parse(payload) as {
      type?: string;
      client_nonce?: string;
    };
    return parsed.type === "user_message" && parsed.client_nonce === clientNonce;
  });
  if (userIndex < 0 || mockChatRevertCutoff !== null) return null;
  return {
    thread_id: MOCK_CHAT_THREAD_ID,
    workspace_id: "ws-codemux-chat",
    repo_path: `${MOCK_HOME_DIR}/projects/codemux`,
    turn_index: MOCK_CHAT_TURNS + 1,
    client_nonce: clientNonce,
    // Rows are one-based, while userIndex is zero-based: the previous row's
    // id is therefore exactly userIndex.
    transcript_cutoff_id: userIndex,
    ref_name: `refs/codemux/checkpoints/${MOCK_CHAT_THREAD_ID}/turn/${MOCK_CHAT_TURNS + 1}`,
    snapshot_commit: "b1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
    head_commit: "1234567890abcdef1234567890abcdef12345678",
    branch: "main",
    created_at: "2026-08-10 10:00:00",
  };
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
  // Not a workflow, but it rides the same seeded-thread machinery: a
  // finished turn plus a live watch loop, so the pane settles to the calm
  // `monitoring` status and the docked MonitoringBar is reachable.
  [MOCK_MONITORING_THREAD_ID]: () =>
    monitoringEnvelopes(MOCK_MONITORING_THREAD_ID),
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
const WORKFLOW_THREAD_WORKSPACE: Record<
  string,
  { workspaceId: string; cwd: string }
> = {
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
  [MOCK_MONITORING_THREAD_ID]: {
    workspaceId: "ws-codemux-monitoring",
    cwd: `${MOCK_HOME_DIR}/.codemux/worktrees/codemux/demo-monitoring`,
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
// The active streaming turn's interval id per thread, so a soft interrupt
// (stop button / "Send now" steer) can end the turn early — mirroring the
// real backend's `interrupt`, which preserves the session and transcript.
const chatTurnTimers = new Map<string, number>();

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

/** Soft-interrupt the active streaming turn, mirroring the real Claude
 *  `interrupt`: stop the stream and flip the session back to `ready`
 *  WITHOUT discarding anything already emitted. Returns `true` when a turn
 *  was actually running. Does NOT drain — the caller decides what to
 *  dispatch next. */
function interruptMockChatTurn(threadId: string): boolean {
  if (!chatActiveTurns.has(threadId)) return false;
  const timer = chatTurnTimers.get(threadId);
  if (timer !== undefined) {
    window.clearInterval(timer);
    chatTurnTimers.delete(threadId);
  }
  chatActiveTurns.delete(threadId);
  emitChatEvent(threadId, {
    type: "session_state_changed",
    thread_id: threadId,
    status: { status: "ready" },
  });
  return true;
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
  opts: { tokens?: number; intervalMs?: number; emptyGapTicks?: number } = {},
): string {
  const turnId = `live-turn-${++mockChatTurnSeq}`;
  const tokens = Math.max(1, opts.tokens ?? 120);
  const intervalMs = Math.max(5, opts.intervalMs ?? 40);
  // `emptyGapTicks` reproduces the real partial-message stream shape from
  // issue #155: after the tool run, the provider opens the text content
  // block with an EMPTY first delta, then goes silent for a while before
  // prose streams. Used to visually verify the transcript never reads
  // finished-and-empty during that gap. 0 (default) keeps the old shape.
  let gapTicksLeft = Math.max(0, opts.emptyGapTicks ?? 0);
  let emptyDeltaSent = false;
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
      input: {
        file_path: "src-tauri/src/terminal/mod.rs",
        offset: 1450,
        limit: 62,
      },
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
      content:
        "    Checking codemux v0.10.0\n    Finished dev [unoptimized] target(s) in 4.12s",
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
    if (!emptyDeltaSent && gapTicksLeft > 0) {
      emptyDeltaSent = true;
      send({
        type: "content_delta",
        thread_id: threadId,
        turn_id: turnId,
        delta: { kind: "text", text: "" },
      });
      return;
    }
    if (gapTicksLeft > 0) {
      gapTicksLeft -= 1;
      return;
    }
    emitted += 1;
    const chunk =
      emitted % 12 === 0 ? `token ${emitted}.\n\n` : `token ${emitted} `;
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
      chatTurnTimers.delete(threadId);
      drainChatQueue(threadId);
    }
  }, intervalMs);
  chatTurnTimers.set(threadId, timer);
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
  ) =>
    item(subagentId, {
      kind: "tool_use",
      tool_name: toolName,
      tool_use_id: id,
      input,
    });
  const toolResult = (subagentId: string, id: string, content: string) =>
    item(subagentId, {
      kind: "tool_result",
      tool_use_id: id,
      content,
      is_error: false,
    });

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
    () =>
      toolUse("explore", "ex-1", "Grep", {
        pattern: "handlePaste",
        path: "src",
      }),
    () => toolResult("explore", "ex-1", "7 matches"),
    () =>
      snap({
        subagent_id: "explore",
        activity: "mapping the paste handler call graph…",
      }),
    () =>
      toolUse("build", "b-1", "Edit", {
      file_path: "src-tauri/src/clipboard.rs",
      old_string: "",
      new_string: "pub fn read_image() {}\n",
    }),
    () => toolResult("build", "b-1", "Applied edit"),
    () =>
      toolUse("explore", "ex-2", "Read", {
        file_path: "src/components/chat/Composer.tsx",
      }),
    () => toolResult("explore", "ex-2", "// Composer.tsx\n"),
    () =>
      snap({
        subagent_id: "explore",
        status: "completed",
        result_text: "Mapped the paste path · 2 files inspected",
        tool_use_count: 2,
        duration_ms: 12000,
      }),
    () =>
      toolUse("build", "b-2", "Bash", {
        command: "cargo test clipboard_fallback",
      }),
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

/** Background-wait lifecycle, replayed from a real transcript: the agent
 *  launches a delegated subagent plus a background shell job, yields with
 *  "waiting on…" prose, and the provider emits a SUCCESSFUL `turn_completed`
 *  even though the session is still alive. Moments later the task
 *  notifications land, the model resumes in the SAME session (no new user
 *  prompt) and runs a long tool call before the real end. Drives the
 *  interim-turn-end handling in the reducer / transcript slots.
 *  `window.__codemuxChatMock.streamBackgroundWait()` triggers it on demand.
 *
 *  `holdMs` is how long the resumed Bash stays running before the turn
 *  truly finishes — long enough to screenshot the "resumed" state. */
function streamMockBackgroundWait(
  threadId: string = MOCK_CHAT_THREAD_ID,
  opts: { intervalMs?: number; holdMs?: number } = {},
): string {
  const turnId = `live-bgwait-${++mockChatTurnSeq}`;
  const intervalMs = Math.max(60, opts.intervalMs ?? 450);
  const holdMs = Math.max(1_000, opts.holdMs ?? 45_000);
  const send = (event: unknown) => emitChatEvent(threadId, event);
  const snap = (subagent: Record<string, unknown>) =>
    send({ type: "subagent_updated", thread_id: threadId, subagent });
  const item = (it: Record<string, unknown>) =>
    send({ type: "item_completed", thread_id: threadId, turn_id: turnId, item: it });
  const toolUse = (id: string, toolName: string, input: Record<string, unknown>) =>
    item({ kind: "tool_use", tool_name: toolName, tool_use_id: id, input });
  const toolResult = (id: string, content: string) =>
    item({ kind: "tool_result", tool_use_id: id, content, is_error: false });

  chatActiveTurns.add(threadId);
  const frames: Array<() => void> = [
    () =>
      send({
        type: "session_state_changed",
        thread_id: threadId,
        status: { status: "running", active_turn: turnId },
      }),
    () =>
      item({
        kind: "assistant_thinking",
        text: "Build the isolated container first, then fan out the catalog report while npm ci runs.",
      }),
    () => toolUse(`${turnId}-t0`, "Bash", { command: "docker build -t hermes-e2e -f Dockerfile.e2e ." }),
    () => toolResult(`${turnId}-t0`, "Successfully tagged hermes-e2e:latest"),
    () =>
      toolUse(`${turnId}-t1`, "Agent", {
        description: "Model-catalog report",
        prompt: "Read the provider catalog and report which models are missing.",
      }),
    () =>
      snap({
        subagent_id: "catalog",
        name: "Model-catalog report",
        agent_type: "general-purpose",
        model: "sonnet · high",
        status: "running",
        parent_item_id: `${turnId}-t1`,
      }),
    () =>
      toolUse(`${turnId}-t2`, "Bash", {
        command: "docker run --rm hermes-e2e npm ci",
        run_in_background: true,
      }),
    () => toolResult(`${turnId}-t2`, "Command running in background with ID: bg-npm-ci"),
    () =>
      snap({
        subagent_id: "bg-npm-ci",
        name: "npm ci (Docker)",
        status: "running",
        background_task: true,
        parent_item_id: `${turnId}-t2`,
      }),
    () =>
      item({
        kind: "assistant_text",
        text: "Waiting on the model-catalog report and the Docker `npm ci` — both still in flight.",
      }),
    // The provider's main loop yields here: a SUCCESSFUL result lands even
    // though the session keeps going.
    () =>
      send({
        type: "turn_completed",
        thread_id: threadId,
        turn_id: turnId,
        status: { kind: "success" },
        usage: null,
      }),
    // Nothing happens for a while: the model is parked until a task
    // notification arrives. This is the window that used to read as done.
    () => undefined,
    () => undefined,
    () => undefined,
    () => undefined,
    () => undefined,
    () => undefined,
    () =>
      snap({
        subagent_id: "catalog",
        status: "completed",
        result_text: "3 models missing from the catalog · report attached",
        tool_use_count: 6,
        duration_ms: 9_000,
      }),
    // The adapter surfaces the spawn's tool_result only once the task's
    // terminal notification lands.
    () => toolResult(`${turnId}-t1`, "3 models missing from the catalog · report attached"),
    () =>
      snap({
        subagent_id: "bg-npm-ci",
        status: "completed",
        background_task: true,
        result_text: "added 812 packages in 41s",
      }),
    // Task notifications woke the model: it resumes in the same session.
    () =>
      item({
        kind: "assistant_thinking",
        text: "Both reports are in. Run the end-to-end suite inside the container now.",
      }),
    () =>
      toolUse(`${turnId}-t3`, "Bash", {
        command: "docker run --rm hermes-e2e npm test -- --runInBand",
      }),
  ];

  let i = 0;
  const timer = window.setInterval(() => {
    const frame = frames[i++];
    if (frame) frame();
    if (i < frames.length) return;
    window.clearInterval(timer);
    chatTurnTimers.delete(threadId);
    // The resumed tool call runs for a while, then the turn really ends.
    const hold = window.setTimeout(() => {
      chatTurnTimers.delete(threadId);
      toolResult(`${turnId}-t3`, "Tests: 48 passed, 48 total\nTime: 38.2s");
      item({
        kind: "assistant_text",
        text: "End-to-end suite passes inside the isolated container — the fix is verified.",
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
      chatActiveTurns.delete(threadId);
      drainChatQueue(threadId);
    }, holdMs);
    chatTurnTimers.set(threadId, hold);
  }, intervalMs);
  chatTurnTimers.set(threadId, timer);
  return turnId;
}

/** Dead-run detection QA (issue #154): fire a `run_stalled` on a thread,
 *  first flipping it to `streaming` so the amber "no activity" tail notice
 *  renders (the notice only shows mid-turn). */
function streamMockRunStalled(
  threadId: string = MOCK_CHAT_THREAD_ID,
  silentForSecs = 700,
): void {
  emitChatEvent(threadId, {
    type: "session_state_changed",
    thread_id: threadId,
    status: { status: "running", active_turn: `stall-turn-${Date.now()}` },
  });
  emitChatEvent(threadId, {
    type: "run_stalled",
    thread_id: threadId,
    silent_for_secs: silentForSecs,
  });
}

/** Provider-health QA: per-provider override served by the
 *  `agent_chat_provider_health` mock handler. Absent → healthy. */
type MockProviderKind = "claude" | "codex" | "cursor" | "opencode";
const mockProviderHealth: Partial<
  Record<MockProviderKind, ProviderHealthReport>
> = {};

/** Inject (or clear, with `null`) a provider health failure, then nudge
 *  the frontend store to re-probe immediately so the chat surfaces'
 *  status banner reflects it without waiting out the TTL. */
function setMockProviderHealth(
  provider: MockProviderKind,
  health: Partial<ProviderHealthReport> | null,
): void {
  if (health === null) {
    delete mockProviderHealth[provider];
  } else {
    mockProviderHealth[provider] = {
      provider,
      status: "error",
      installed: true,
      message: null,
      version: null,
      ...health,
    };
  }
  void import("@/stores/provider-health-store").then((m) =>
    m.useProviderHealth.getState().refresh(provider, { force: true }),
  );
}

/** Session-death QA: emit the `session_state_changed { error }` the
 *  provider watchdogs fire when a sidecar/server dies WITHOUT a
 *  recoverable turn id. Drives the inline red "Session error" notice. */
function sessionErrorMockRun(
  threadId: string = MOCK_CHAT_THREAD_ID,
  message = "claude-agent sidecar exited unexpectedly",
): void {
  emitChatEvent(threadId, {
    type: "session_state_changed",
    thread_id: threadId,
    status: { status: "running", active_turn: `err-turn-${Date.now()}` },
  });
  emitChatEvent(threadId, {
    type: "session_state_changed",
    thread_id: threadId,
    status: { status: "error", message },
  });
}

/** Dead-run detection QA (issue #154): settle a live turn with a synthetic
 *  `child_exited` error — exactly what the provider watchdogs emit when a
 *  sidecar/server dies mid-turn. Drives the "Run interrupted" divider and
 *  the composer's Continue chip. */
function interruptMockRun(threadId: string = MOCK_CHAT_THREAD_ID): void {
  const turnId = `interrupted-turn-${Date.now()}`;
  emitChatEvent(threadId, {
    type: "session_state_changed",
    thread_id: threadId,
    status: { status: "running", active_turn: turnId },
  });
  emitChatEvent(threadId, {
    type: "turn_completed",
    thread_id: threadId,
    turn_id: turnId,
    status: {
      kind: "error",
      subtype: "child_exited",
      message: "claude-agent sidecar exited unexpectedly",
    },
    usage: null,
  });
}

// Expose the stream triggers for browser-console / automation use.
(
  window as unknown as {
    __codemuxChatMock: {
      threadId: string;
      streamReply: typeof streamMockChatReply;
      streamSubagents: typeof streamMockSubagents;
      streamBackgroundWait: typeof streamMockBackgroundWait;
      streamRunStalled: typeof streamMockRunStalled;
      interruptRun: typeof interruptMockRun;
      sessionError: typeof sessionErrorMockRun;
      setProviderHealth: typeof setMockProviderHealth;
    };
  }
).__codemuxChatMock = {
  threadId: MOCK_CHAT_THREAD_ID,
  streamReply: streamMockChatReply,
  streamSubagents: streamMockSubagents,
  streamBackgroundWait: streamMockBackgroundWait,
  streamRunStalled: streamMockRunStalled,
  interruptRun: interruptMockRun,
  sessionError: sessionErrorMockRun,
  setProviderHealth: setMockProviderHealth,
};

// Expose the terminal flood + serialize triggers for browser-console /
// automation use (issue #128 scrollback serialize/restore e2e).
(
  window as unknown as {
    __codemuxTerminalMock: {
      flood: typeof floodTerminals;
      emitSerializeBuffers: typeof emitSerializeBuffers;
      emitOsc7: typeof emitOsc7;
    };
  }
).__codemuxTerminalMock = {
  flood: floodTerminals,
  emitSerializeBuffers,
  emitOsc7,
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
function mkPreset(
  p: Partial<TerminalPreset> & { id: string; name: string },
): TerminalPreset {
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
      commands: ["codex --dangerously-bypass-approvals-and-sandbox"],
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

// ── Web Remote Access (mocked server state machine) ─────────────────
//
// A small mutable twin of the real `web_remote` server so the Settings →
// Remote Access panel is fully exercisable in plain-browser dev.
// enable/disable/set_config/approve/revoke all mutate this state and
// re-emit `web-remote-state-changed`, exactly like the backend's
// `emit_state_changed`. Starts DISABLED (matching the real default-off
// posture) with three seeded devices and approval mode on so the pending
// row shows the moment the server is turned on.

let webRemotePort = MOCK_WEB_REMOTE_PORT;
let webRemoteEnabled = false;
let webRemoteRequireApproval = true;
let webRemoteBindScope: WebRemoteBindScope = "all";
// Account mode (Stage A): on by default in the mock so the Settings → Remote
// Access "Account access" subsection is exercisable under `npm run dev`.
// `accountSignedIn` mirrors the desktop being signed into a Codemux account
// (the mock user is), which account mode requires.
let webRemoteAccountMode = true;
let webRemoteTrustAccount = false;
const webRemoteAccountSignedIn = true;
let webRemoteSessions: WebRemoteSessionView[] = mockWebRemoteSessions();
let webRemoteLiveSeq = 0;

function buildWebRemoteStatus(): WebRemoteStatus {
  return {
    enabled: webRemoteEnabled,
    running: webRemoteEnabled,
    port: webRemotePort,
    require_approval: webRemoteRequireApproval,
    bind_scope: webRemoteBindScope,
    active_connections: webRemoteEnabled
      ? webRemoteSessions.filter((s) => s.approved && s.connected).length
      : 0,
    // Mirrors the backend: distinct devices with a live socket (connected
    // implies approved). Drives the "N connected" badge + updater defer.
    connected_sessions: webRemoteEnabled
      ? webRemoteSessions.filter((s) => s.connected).length
      : 0,
    sessions: webRemoteSessions,
    account_mode_enabled: webRemoteAccountMode,
    trust_account_browsers: webRemoteTrustAccount,
    account_signed_in: webRemoteAccountSignedIn,
  };
}

function emitWebRemoteState(): void {
  emitEvent("web-remote-state-changed", buildWebRemoteStatus());
}

// Console/automation hooks: inject devices so the live "device wants to
// connect" toast, the pending badge, and the account-vs-paired tagging can all
// be observed in dev.
//   window.__codemuxRemoteMock.addPendingDevice("iPad")
//   window.__codemuxRemoteMock.addAccountDevice("Work laptop")
//
// The invoke mock is a twin of the Tauri command surface, not the HTTP server,
// so it can't host the real `POST /api/pair-account` route (that's exercised by
// unit tests + a real server via `?remote=1`). `addAccountDevice` stands in for
// it: it simulates the outcome of a successful account sign-in — an
// `account`-sourced session appearing in the devices list, pending approval
// unless account browsers are trusted — so the Settings tagging + approval flow
// are fully demoable under `npm run dev`.
(
  window as unknown as {
    __codemuxRemoteMock: {
      addPendingDevice(name?: string): string;
      addAccountDevice(name?: string): string;
      setConnected(id: string, connected: boolean): void;
    };
  }
).__codemuxRemoteMock = {
  addPendingDevice(name?: string): string {
    const id = `sess-live-${++webRemoteLiveSeq}`;
    webRemoteSessions = [
      ...webRemoteSessions,
      {
        id,
        name: name ?? `New device ${webRemoteLiveSeq}`,
        user_agent:
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        created_at: new Date().toISOString(),
        last_seen_at: null,
        approved: false,
        connected: false,
        source: "pair",
      },
    ];
    emitWebRemoteState();
    return id;
  },
  addAccountDevice(name?: string): string {
    const id = `sess-acct-${++webRemoteLiveSeq}`;
    webRemoteSessions = [
      ...webRemoteSessions,
      {
        id,
        name: name ?? `Account device ${webRemoteLiveSeq}`,
        user_agent:
          "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        created_at: new Date().toISOString(),
        last_seen_at: null,
        // Account sessions start pending unless account browsers are trusted.
        approved: webRemoteTrustAccount,
        connected: false,
        source: "account",
      },
    ];
    emitWebRemoteState();
    return id;
  },
  setConnected(id: string, connected: boolean): void {
    webRemoteSessions = webRemoteSessions.map((s) =>
      s.id === id ? { ...s, connected } : s,
    );
    emitWebRemoteState();
  },
};

// ── Command router ──────────────────────────────────────────────────
//
// Keyed by exact Tauri command name. Boot-critical reads return seed
// data; mutators patch `appState` and re-emit. Args arrive with the
// SAME camelCase keys the `src/tauri/commands.ts` wrappers pass.

type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;

/** In-memory staging store for `agent_chat_stage_image` (keyed by the
 *  fake absolute path returned to the frontend). Serves `agent_chat_read_image`
 *  so the IPC-read image fallback works end-to-end in `npm run dev`. */
const stagedImages = new Map<
  string,
  { bytes: Uint8Array; mediaType: string }
>();
let stagedImageSeq = 0;

/**
 * What each host declares, mirroring the Rust adapters.
 *
 * GitHub: everything. GitLab: everything but the verdict it does not
 * have. Anything else: nothing — an unverified declaration is worse than
 * none, because the backend refuses undeclared operations, so the honest
 * blank renders read-only rather than drawing controls that would fail.
 */
const NO_MOCK_OPERATIONS: ProviderOperations = {
  list_read: false,
  comment: false,
  approve: false,
  request_changes: false,
  line_comments: false,
  merge_with_strategies: false,
  draft_ready_close_reopen: false,
  checks_status: false,
  timeline: false,
  thread_reply: false,
  thread_resolve: false,
};

const ALL_MOCK_OPERATIONS: ProviderOperations = {
  list_read: true,
  comment: true,
  approve: true,
  request_changes: true,
  line_comments: true,
  merge_with_strategies: true,
  draft_ready_close_reopen: true,
  checks_status: true,
  timeline: true,
  thread_reply: true,
  thread_resolve: true,
};

/** Flipped by `__codemuxMockThreadOpsOff` so the read-only thread list
 *  (a host that serves threads but declares neither write) can be looked
 *  at without inventing a fifth seeded host.
 *
 *  Kept in localStorage because the declaration is read once per minute
 *  and cached: a reload is the quickest way to make the new answer take
 *  effect, and a switch that a reload undid would be unusable. */
const THREAD_OPS_OFF_KEY = "codemux-mock-thread-ops-off";
let mockThreadOpsDeclared = localStorage.getItem(THREAD_OPS_OFF_KEY) !== "1";

const MOCK_PROVIDER_OPERATIONS: Record<string, ProviderOperations> = {
  github: ALL_MOCK_OPERATIONS,
  // No request-changes verdict on the host, and no line-comment route
  // in this build — see `GitLabProvider::operations`.
  gitlab: { ...ALL_MOCK_OPERATIONS, request_changes: false, line_comments: false },
};

const handlers: Record<string, Handler> = {
  // ── Auth / sync ──
  check_auth: () => MOCK_USER,
  get_auth_token: () => "mock-token",
  get_sync_status: () => ({ syncAvailable: true, authMethod: "github" }),
  sign_out: () => undefined,
  skills_sync_status: () => ({ state: "idle", lastSyncAtMillis: null }),
  // ── Usage ──
  // `tzOffsetMinutes` is accepted and ignored: the fixture derives its
  // buckets from the local `Date` already, so it is local by construction.
  usage_summary: (args: Args) => mockUsageSummary(String(args.period ?? "7d")),
  usage_export_csv: (args: Args) =>
    mockUsageExportCsv(String(args.period ?? "7d")),
  // The dev mock has no provider history to read; report a plausible scan. The
  // footer's session count comes from the summary, not from here.
  usage_scan_provider_history: () => ({
    files_scanned: 34,
    sessions_found: 12,
    rows_updated: 0,
    reimported: false,
  }),

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

  // The right-panel deck's browser pane hosts the workspace's ONE agent
  // browser session. The mock mirrors the real
  // command's contract — flip `right_panel_docked`, hand the session back so
  // the pane knows which `cli_session_name` to stream — but there is no
  // pane tree to adopt from here, so the adopt branch has nothing to do.
  dock_browser_in_right_panel: (a) => {
    const workspaceId = a.workspaceId as string;
    const session = appState.agent_browser_sessions.find(
      (abs) => abs.workspace_id === workspaceId,
    );
    // Mirrors `resolve_agent_browser_session`: the session is minted on
    // first use, keyed by workspace, with a name derived from its cwd.
    if (!session) {
      const ws = appState.workspaces.find(
        (w) => w.workspace_id === workspaceId,
      );
      const fresh: AgentBrowserSession = {
        session_id: `agent-browser-${workspaceId}`,
        workspace_id: workspaceId,
        cli_session_name: `devmock-${(ws?.title ?? workspaceId).replace(/\W+/g, "-")}`,
        stream_url: "ws://127.0.0.1:9777",
        current_url: "http://localhost:1420",
        is_active: true,
        pane_id: null,
        browser_id: null,
        user_dismissed: false,
        right_panel_docked: true,
      };
      appState.agent_browser_sessions.push(fresh);
      emitAppState();
      return structuredClone(fresh);
    }
    session.pane_id = null;
    session.browser_id = null;
    session.right_panel_docked = true;
    session.is_active = true;
    session.user_dismissed = false;
    emitAppState();
    return structuredClone(session);
  },
  undock_browser_from_right_panel: (a) => {
    const session = appState.agent_browser_sessions.find(
      (abs) => abs.workspace_id === (a.workspaceId as string),
    );
    if (!session?.right_panel_docked) return null;
    session.right_panel_docked = false;
    if (a.dismissed) session.user_dismissed = true;
    emitAppState();
    return null;
  },

  // ── Core state ──
  get_app_state: () => appState,
  get_home_dir: () => MOCK_HOME_DIR,
  get_feature_flags: () => FEATURE_FLAGS,
  get_package_format: () => "AppImage",

  // ── Settings ──
  get_synced_settings: () => structuredClone(SYNCED_SETTINGS),
  update_synced_settings: (a) => {
    if (a.settings) Object.assign(SYNCED_SETTINGS, structuredClone(a.settings as UserSettings));
    return structuredClone(SYNCED_SETTINGS);
  },
  update_setting: (a) => {
    const section = a.section as keyof UserSettings;
    const key = String(a.key ?? "");
    const target = SYNCED_SETTINGS[section] as unknown as Record<string, unknown> | undefined;
    if (target && key) target[key] = structuredClone(a.value);
    return structuredClone(SYNCED_SETTINGS);
  },
  reset_synced_settings: () => structuredClone(SYNCED_SETTINGS),
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

  // ── Filesystem listing ──
  // Backs the web-remote path browser (Stage 3b). Serves a small static
  // tree (see `mockListDirectory`) and rejects unknown paths like the real
  // `list_directory` so manual-path validation is exercisable in dev.
  list_directory: (a) => {
    const path = String(a.path ?? "/");
    // No seeded repository keeps GitLab-style templates in a directory,
    // and the generic listing would hand the form a stack of unrelated
    // `.md` files to mistake for one.
    if (path.includes("merge_request_templates")) return [];
    return mockListDirectory(path, Boolean(a.showHidden));
  },

  // Without a handler the mock returned its `undefined` default and the
  // file-search dialog crashed on `results.map`. Serve a small repo-shaped
  // list so "Open file…" (and the right panel's doc panes) are exercisable
  // in browser dev.
  search_file_names: (a) => {
    const query = String(a.query ?? "").toLowerCase();
    const paths = [
      "AGENTS.md",
      "README.md",
      "CLAUDE.md",
      "package.json",
      "CONTRIBUTING.md",
      "src/components/layout/right-panel.tsx",
      "src/stores/ui-store.ts",
      "src-tauri/Cargo.toml",
      // No real filesystem backs browser dev, so an opened image always
      // 404s — which is exactly what makes the ImageViewer failure card
      // reachable here.
      "docs/screenshots/dashboard.png",
    ];
    return query ? paths.filter((p) => p.toLowerCase().includes(query)) : paths;
  },

  // Enough content for the editor surfaces (main-area tab and the right
  // panel's doc panes) to exercise rendered-vs-raw markdown, soft wrap and
  // copy without a real filesystem.
  // Browser dev has no filesystem; every path "exists" so chat file links
  // stay clickable against the synthetic contents `read_file` fabricates.
  file_exists: () => true,
  read_file: (a) => {
    const path = String(a.path ?? "");
    // Template lookup is a series of speculative reads, and the answer
    // that matters most is "no". `mockReadFile` returns prose for any
    // `.md`, which would make every repository look like it had a
    // template — so the template paths are answered explicitly, and only
    // the seeded root has one.
    if (MOCK_TEMPLATE_PATH_PATTERN.test(path)) {
      return path.startsWith(MOCK_PR_TEMPLATE_ROOT)
        ? MOCK_PR_TEMPLATE
        : Promise.reject(`No such file or directory: ${path}`);
    }
    return mockReadFile(path);
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
    a.provider === "claude"
      ? CLAUDE_CAPABILITIES
      : a.provider === "codex"
        ? CODEX_UTILITY_CAPABILITIES
        : a.provider === "cursor"
          ? CURSOR_CAPABILITIES
        : EMPTY_CAPABILITIES,
  // Provider slash commands — in production these are harvested live
  // from the deployed Claude Code CLI (SDK `supportedCommands()`),
  // including custom `.claude/commands` entries. The mock serves a
  // representative subset so the composer's COMMANDS group renders.
  list_chat_slash_commands: (a) =>
    a.provider === "claude"
      ? [
          {
            name: "compact",
            description:
              "Clear conversation history but keep a summary in context",
            argumentHint: "<optional summary instructions>",
          },
          {
            name: "clear",
            description: "Clear conversation history and free up context",
            argumentHint: "",
          },
          {
            name: "init",
            description:
              "Initialize a new CLAUDE.md file with codebase documentation",
            argumentHint: "",
          },
          {
            name: "review",
            description: "Review a pull request",
            argumentHint: "",
          },
          {
            name: "security-review",
            description: "Complete a security review of the pending changes",
            argumentHint: "",
          },
          {
            name: "pr-comments",
            description: "Get comments from a GitHub pull request",
            argumentHint: "",
          },
          {
            name: "release-notes",
            description: "View release notes",
            argumentHint: "<version>",
          },
          {
            name: "todos",
            description: "List current todo items",
            argumentHint: "",
          },
        ]
      : [],
  list_skills: () => MOCK_SKILL_INVENTORY,
  agent_chat_list_messages: (a) => mockThreadPayloads(a.threadId as string),
  // Cursor read (Phase 3). The mock's transcripts are generated and
  // stable per thread, so a row id is just "index + 1" within the
  // thread — enough to exercise the frontend's cursor arithmetic and
  // the lazy tool-result path (the stress fixture's tool bodies are
  // deliberately megabyte-scale).
  agent_chat_list_messages_after: (a) => {
    const threadId = a.threadId as string;
    const afterId = (a.afterId as number | null | undefined) ?? 0;
    return mockThreadRows(threadId).filter(
      (row) =>
        row.id > afterId &&
        (threadId !== MOCK_CHAT_THREAD_ID ||
          mockChatRevertCutoff === null ||
          row.id <= mockChatRevertCutoff),
    );
  },
  agent_chat_thread_head_id: (a) => {
    const threadId = a.threadId as string;
    const rows = mockThreadRows(threadId).filter(
      (row) =>
        threadId !== MOCK_CHAT_THREAD_ID ||
        mockChatRevertCutoff === null ||
        row.id <= mockChatRevertCutoff,
    );
    return rows.length > 0 ? rows[rows.length - 1].id : null;
  },
  // The `@` mention popup and `+ → File…/Folder…` browse the project
  // tree. Without these the dev mock's pickers render empty, which
  // makes the mention menu impossible to exercise (and impossible to
  // screenshot) outside a real Tauri build.
  list_project_files: (a) => {
    const query = String(a.query ?? "").toLowerCase();
    const limit = Math.max(1, Math.min(50, Number(a.limit ?? 20)));
    const paths = [
      ".github/workflows/ci.yml",
      "AGENTS.md",
      "src/components/chat/AgentChatPane.tsx",
      "src/components/chat/Composer.tsx",
      "src/components/chat/SlashCommandPopup.tsx",
      "src/lib/agent-chat/session-mentions.ts",
      "src/lib/agent-chat/slash-commands.ts",
      "src/stores/agent-chat-store.ts",
      "src-tauri/src/commands/agent_chat.rs",
      "src-tauri/src/lib.rs",
    ];
    return paths
      .filter((path) => !query || path.toLowerCase().includes(query))
      .slice(0, limit)
      .map((path, index) => ({
        path,
        absolute_path: `${MOCK_HOME_DIR}/projects/codemux/${path}`,
        score: query ? 200 - index : 0,
      }));
  },
  list_project_folders: (a) => {
    const query = String(a.query ?? "").toLowerCase();
    const limit = Math.max(1, Math.min(50, Number(a.limit ?? 20)));
    const folders: Array<[string, number]> = [
      ["src/components/chat", 42],
      ["src/hooks", 18],
      ["src/lib/agent-chat", 57],
      ["src/stores", 12],
      ["src-tauri/src/commands", 24],
    ];
    return folders
      .filter(([path]) => !query || path.toLowerCase().includes(query))
      .slice(0, limit)
      .map(([path, itemCount], index) => ({
        path,
        absolute_path: `${MOCK_HOME_DIR}/projects/codemux/${path}`,
        score: query ? 200 - index : 0,
        item_count: itemCount,
      }));
  },
  agent_chat_get_tool_result: (a) => {
    const payload = mockFullPayloadById.get(a.rowId as number);
    if (payload == null) throw new Error(`not_found: row ${a.rowId}`);
    return payload;
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
        provider: "codex",
        title: "agent-chat-demo",
        created_at: new Date().toISOString(),
        last_active_at: new Date().toISOString(),
        model: "gpt-5.4",
        effort: null,
        context_window: null,
        permission_mode: "danger-full-access",
      },
    ];
  },
  // Provider-neutral `@session:` picker fixture. Two source chats share the
  // main demo workspace: one in the current checkout and one in a worktree,
  // making grouping, provider labels, filtering, and the handoff chip easy to
  // verify under `npm run dev`.
  agent_chat_list_session_mentions: (a) => {
    if (a.workspaceId !== "ws-codemux-chat") return [];
    const excluded = String(a.excludeThreadId ?? "");
    return [
      {
        thread_id: "mock-session-codex-auth",
        workspace_id: "ws-codemux-chat",
        cwd: `${MOCK_HOME_DIR}/projects/codemux`,
        provider: "codex",
        title: "Harden workspace authentication",
        last_active_at: new Date(Date.now() - 8 * 60_000).toISOString(),
        preview:
          "Implemented token rotation and isolated the remaining failing refresh test.",
        message_count: 18,
      },
      {
        thread_id: "mock-session-cursor-rehydrate",
        workspace_id: "ws-codemux-chat",
        cwd: `${MOCK_HOME_DIR}/projects/codemux`,
        provider: "cursor",
        title:
          "Can you analyze the codebase and figure out when taking a screenshot the continue prompt flickers",
        last_active_at: new Date(Date.now() - 14 * 60_000).toISOString(),
        preview:
          "Root-caused it to frontend state rehydration, not the agent run actually stopping.",
        message_count: 9,
      },
      {
        thread_id: "mock-session-claude-index",
        workspace_id: "ws-codemux-chat",
        cwd: `${MOCK_HOME_DIR}/.codemux/worktrees/codemux/search-index`,
        provider: "claude",
        title: "Conversation search indexing",
        last_active_at: new Date(Date.now() - 52 * 60_000).toISOString(),
        preview:
          "The FTS migration is complete; next step is validating ranked snippets in the palette.",
        message_count: 27,
      },
    ].filter((session) => session.thread_id !== excluded);
  },
  agent_chat_get_session_context: (a) => {
    const threadId = String(a.threadId ?? "");
    const sessions: Record<string, Record<string, unknown>> = {
      "mock-session-codex-auth": {
        thread_id: threadId,
        workspace_id: "ws-codemux-chat",
        cwd: `${MOCK_HOME_DIR}/projects/codemux`,
        provider: "codex",
        title: "Harden workspace authentication",
        last_active_at: new Date(Date.now() - 8 * 60_000).toISOString(),
        content:
          "## Goal\nHarden workspace authentication and add token rotation.\n\n## Current state\nToken rotation is implemented; one refresh-path test remains isolated.\n\n## Changes and evidence\n- Added bounded refresh-token replacement.\n- Invalidated the previous token after a successful exchange.\n\n## Tests and results\nThe authentication suite passes except for the isolated refresh race.\n\n## Next steps\nFix the delayed refresh assertion and rerun the focused suite.",
        message_count: 18,
        included_message_count: 18,
        truncated: false,
        handoff_kind: "summary",
        summary_cached: false,
        summary_error: null,
        summarizer_provider: "codex",
        summarizer_model: "gpt-5.6-luna",
        summarizer_effort: "low",
        revision_message_id: 118,
        full_history_available: true,
      },
      "mock-session-cursor-rehydrate": {
        thread_id: threadId,
        workspace_id: "ws-codemux-chat",
        cwd: `${MOCK_HOME_DIR}/projects/codemux`,
        provider: "cursor",
        title:
          "Can you analyze the codebase and figure out when taking a screenshot the continue prompt flickers",
        last_active_at: new Date(Date.now() - 14 * 60_000).toISOString(),
        content:
          "## Goal\nExplain why the continue prompt flickers while a screenshot is taken.\n\n## Current state\nRoot-caused to frontend state rehydration, not the agent run stopping.\n\n## Next steps\nBound the interim turn hold to the current prompt.",
        message_count: 9,
        included_message_count: 9,
        truncated: false,
        handoff_kind: "summary",
        summary_cached: true,
        summary_error: null,
        summarizer_provider: "codex",
        summarizer_model: "gpt-5.6-luna",
        summarizer_effort: "low",
        revision_message_id: 91,
        full_history_available: true,
      },
      "mock-session-claude-index": {
        thread_id: threadId,
        workspace_id: "ws-codemux-chat",
        cwd: `${MOCK_HOME_DIR}/.codemux/worktrees/codemux/search-index`,
        provider: "claude",
        title: "Conversation search indexing",
        last_active_at: new Date(Date.now() - 52 * 60_000).toISOString(),
        content:
          "User:\nBuild durable conversation search.\n\nAssistant:\nThe FTS migration is complete; validate ranked snippets next.",
        message_count: 27,
        included_message_count: 27,
        truncated: false,
        handoff_kind: "summary",
        summary_cached: true,
        summary_error: null,
        summarizer_provider: "codex",
        summarizer_model: "gpt-5.6-luna",
        summarizer_effort: "low",
        revision_message_id: 227,
        full_history_available: true,
      },
    };
    const context = sessions[threadId];
    if (!context || a.workspaceId !== "ws-codemux-chat") {
      throw new Error(`conversation_not_found: ${threadId}`);
    }
    return context;
  },
  agent_chat_search: (a) => {
    const query = String(a.query ?? "")
      .trim()
      .toLocaleLowerCase();
    const workspaceIds = new Set(
      (a.workspaceIds as string[] | undefined) ?? [],
    );
    const limit = Math.max(1, Math.min(50, Number(a.limit ?? 12)));
    if (!query || workspaceIds.size === 0) return [];
    const mainRecord = {
      thread_id: MOCK_CHAT_THREAD_ID,
      workspace_id: "ws-codemux-chat",
      cwd: `${MOCK_HOME_DIR}/projects/codemux`,
      provider: "claude",
      title: "agent-chat-demo",
    };
    const records = [
      mainRecord,
      ...Object.keys(WORKFLOW_THREAD_WORKSPACE)
        .map((threadId) => mockWorkflowSessionRecord(threadId))
        .filter((record): record is Record<string, unknown> => !!record)
        .map((record) => ({
          thread_id: String(record.thread_id),
          workspace_id: String(record.workspace_id),
          cwd: String(record.cwd),
          provider: String(record.provider),
          title: String(record.title),
        })),
    ];
    const results: Array<Record<string, unknown>> = [];
    for (const record of records) {
      if (!workspaceIds.has(record.workspace_id)) continue;
      const titleMatches = record.title.toLocaleLowerCase().includes(query);
      if (titleMatches) {
        results.push({
          message_id: null,
          thread_id: record.thread_id,
          workspace_id: record.workspace_id,
          cwd: record.cwd,
          provider: record.provider,
          session_title: record.title,
          role: "title",
          turn_id: null,
          snippet: record.title,
          created_at: new Date().toISOString(),
        });
      }
      // Production prefers one title row over repeating content hits from
      // that same conversation in the limited palette group.
      if (titleMatches) continue;
      // Search raw persisted payloads and stop as soon as the palette cap is
      // satisfied. Going through `mockThreadRows` would eagerly shape every
      // row in the 520-turn virtualization fixture (including every tool
      // result) just to find the first few prose hits, freezing the dev UI on
      // its first search even though the real SQLite FTS lookup is bounded.
      const payloads = mockThreadPayloads(record.thread_id);
      const firstCreatedAt = Date.now() - payloads.length * 725;
      for (let sourceIndex = 0; sourceIndex < payloads.length; sourceIndex++) {
        const payload = payloads[sourceIndex];
        let envelope: Record<string, unknown>;
        try {
          envelope = JSON.parse(payload) as Record<string, unknown>;
        } catch {
          continue;
        }
        let role: "user" | "assistant" | null = null;
        let text = "";
        if (envelope.type === "user_message") {
          role = "user";
          text = String(envelope.text ?? "");
        } else if (
          envelope.type === "item_completed" &&
          !envelope.subagent_id
        ) {
          const item = envelope.item as Record<string, unknown> | undefined;
          if (item?.kind === "assistant_text") {
            role = "assistant";
            text = String(item.text ?? "");
          }
        }
        const matchAt = text.toLocaleLowerCase().indexOf(query);
        if (!role || matchAt < 0) continue;
        const from = Math.max(0, matchAt - 48);
        const to = Math.min(text.length, matchAt + query.length + 90);
        results.push({
          message_id: sourceIndex + 1,
          thread_id: record.thread_id,
          workspace_id: record.workspace_id,
          cwd: record.cwd,
          provider: record.provider,
          session_title: record.title,
          role,
          turn_id:
            role === "assistant"
              ? String(envelope.turn_id ?? "") || null
              : null,
          snippet: `${from > 0 ? "… " : ""}${text.slice(from, to)}${to < text.length ? " …" : ""}`,
          created_at: new Date(
            firstCreatedAt + sourceIndex * 725,
          ).toISOString(),
        });
        if (results.length >= limit) return results.slice(0, limit);
      }
      if (results.length >= limit) break;
    }
    return results.slice(0, limit);
  },
  agent_chat_open_search_result: (a) => {
    const threadId = String(a.threadId ?? "");
    const location = findChatPaneLocation(threadId);
    if (!location) throw new Error(`not_found: mock chat session ${threadId}`);
    location.workspace.active_surface_id = location.surface.surface_id;
    location.surface.active_pane_id = location.paneId;
    const tab = location.workspace.tabs.find(
      (candidate) => candidate.surface_id === location.surface.surface_id,
    );
    if (tab) location.workspace.active_tab_id = tab.tab_id;
    appState = {
      ...appState,
      active_workspace_id: location.workspace.workspace_id,
    };
    emitAppState();
    return {
      pane_id: location.paneId,
      workspace_id: location.workspace.workspace_id,
    };
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
        provider: "codex",
        title: "agent-chat-demo",
        created_at: new Date().toISOString(),
        last_active_at: new Date().toISOString(),
        model: "gpt-5.4",
        effort: null,
        context_window: null,
        permission_mode: "danger-full-access",
      };
    }
    return mockWorkflowSessionRecord(threadId);
  },
  // DB-only config persist (design G). The mock has no SQLite, so this
  // is a no-op — the demo pane keeps its in-memory slice values.
  agent_chat_update_session_config: () => undefined,
  // ── Image staging (attach-time upload; Bug 1 fix) ──
  //
  // `agent_chat_stage_image` is a RAW-BODY invoke (bytes as the payload,
  // MIME on an `x-media-type` header) and is dispatched specially in
  // `invoke` below — it never reaches this handler map. The read/discard
  // counterparts take normal args. `agent_chat_read_image` serves bytes
  // straight back so the hydrated-style fs-path render path (which routes
  // through `readChatImage` in the browser, where `convertFileSrc` is the
  // identity → the `<img>` 404s) still resolves an image end-to-end.
  agent_chat_read_image: (a) => {
    const entry = stagedImages.get(String(a.path));
    if (!entry) throw new Error(`[dev mock] no staged image at ${a.path}`);
    return {
      bytes: Array.from(entry.bytes),
      media_type: entry.mediaType,
    };
  },
  // Local screenshot links use the same browser fallback as persisted chat
  // attachments. The mock returns its seeded preview PNG for any accepted
  // path so the real card + lightbox can be visually exercised without host
  // filesystem access from the browser origin.
  agent_chat_read_local_image: () => {
    const encoded = MOCK_USER_IMAGE_DATA_URL.split(",")[1] ?? "";
    return {
      bytes: Array.from(
        Uint8Array.from(atob(encoded), (char) => char.charCodeAt(0)),
      ),
      media_type: "image/png",
    };
  },
  agent_chat_discard_staged_image: (a) => {
    stagedImages.delete(String(a.path));
    return undefined;
  },
  // MCP warmup is a real background prime in the app; the mock has no MCP
  // host, so this is a no-op that returns immediately.
  agent_chat_prime_mcp: () => undefined,
  // Provider runtime health probe. Healthy by default; QA can inject a
  // failure via `window.__codemuxChatMock.setProviderHealth(...)` to
  // exercise the chat surfaces' provider status banner.
  agent_chat_provider_health: (a) => {
    const provider = String(a.provider) as MockProviderKind;
    return (
      mockProviderHealth[provider] ?? {
        provider,
        status: "ready",
        installed: true,
        message: null,
        version: "0.0.0-mock",
      }
    );
  },
  agent_chat_start_session: (a) => (a.input as { thread_id: string }).thread_id,
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
    // A prompt mentioning background work replays the yield-and-resume
    // lifecycle (interim turn ends) instead of the plain streaming reply.
    const turnId = /\bbackground\b/i.test(input.text)
      ? streamMockBackgroundWait(threadId)
      : streamMockChatReply(threadId);
    return { turn_id: turnId, queued_id: null };
  },
  agent_chat_cancel_queued_turn: (a) => {
    const { threadId, queuedId } = a as {
      threadId: string;
      queuedId: string;
    };
    const q = chatQueues.get(threadId);
    let removed = false;
    if (q) {
      const idx = q.findIndex((e) => e.queuedId === queuedId);
      if (idx >= 0) {
        q.splice(idx, 1);
        removed = true;
        emitChatEvent(threadId, {
          type: "queued_turn_cancelled",
          thread_id: threadId,
          queued_id: queuedId,
        });
      }
    }
    return removed;
  },
  agent_chat_send_queued_turn_now: (a) => {
    const { threadId, queuedId } = a as {
      threadId: string;
      queuedId: string;
    };
    const q = chatQueues.get(threadId);
    const idx = q ? q.findIndex((e) => e.queuedId === queuedId) : -1;
    // Unknown / already-dispatched id — idempotent no-op, matching the
    // real backend.
    if (!q || idx < 0) return undefined;
    // Promote the item to the front of the queue so the drain dispatches
    // it next (mirrors `promote_queued_to_front`).
    if (idx > 0) {
      const [item] = q.splice(idx, 1);
      q.unshift(item);
    }
    // Soft-interrupt the active turn (if any); either way, drain the
    // now-front item out immediately.
    interruptMockChatTurn(threadId);
    drainChatQueue(threadId);
    return undefined;
  },
  agent_chat_interrupt_turn: () => undefined,
  // The docked MonitoringBar's Stop. Mirrors the real backend closely
  // enough to be demoable: clear the pane's `monitoring` status + its
  // manual-monitor reason, then re-emit so the bar unmounts and the
  // sidebar badge clears in one frame.
  agent_chat_stop_monitoring: (a) => {
    // `threadId` is nullable: a pane can be flagged monitoring with no chat
    // thread bound to it, and Stop has to work there too. The caller's pane
    // id wins, exactly like the real command.
    const { threadId, paneId: explicitPaneId } = a as {
      threadId: string | null;
      paneId?: string;
    };
    const paneId =
      explicitPaneId || (threadId ? findChatPaneIdForThread(threadId) : null);
    if (!paneId) return undefined;
    const paneStatuses = { ...appState.pane_statuses };
    const manualMonitors = { ...(appState.manual_monitors ?? {}) };
    if (paneStatuses[paneId] === "monitoring") delete paneStatuses[paneId];
    delete manualMonitors[paneId];
    appState = {
      ...appState,
      pane_statuses: paneStatuses,
      manual_monitors: manualMonitors,
    };
    emitAppState();
    return undefined;
  },
  // Liveness probe used by the remount-hydrate path to tell a live run
  // (whose terminal event simply hasn't persisted yet) apart from a
  // genuinely-interrupted one. The mock tracks in-flight turns in
  // `chatActiveTurns`, so mirror that directly.
  agent_chat_turn_active: (a) => {
    const { threadId } = a as { threadId: string };
    return chatActiveTurns.has(threadId);
  },
  // Real backend hands the decision to the provider, which resolves the
  // request and echoes `request_resolved` back over the thread channel.
  // Mirror that so the `?askq=1` seed actually settles (panel dismounts,
  // the answer lands as a reply bubble) instead of hanging pending.
  agent_chat_respond_to_request: (a) => {
    const { threadId, requestId, decision } = a as {
      threadId: string;
      requestId: string;
      decision: unknown;
    };
    emitChatEvent(threadId, {
      type: "request_resolved",
      thread_id: threadId,
      request_id: requestId,
      decision,
    });
    return undefined;
  },
  agent_chat_set_model: () => undefined,
  agent_chat_set_permission_mode: () => undefined,
  agent_chat_stop_session: () => undefined,
  agent_chat_rename_session: () => undefined,
  agent_chat_delete_session: () => undefined,
  agent_chat_list_turn_checkpoints: (a) => {
    if (a.threadId !== MOCK_CHAT_THREAD_ID) return [];
    const checkpoint = mockTailTurnCheckpoint();
    return checkpoint ? [checkpoint] : [];
  },
  agent_chat_revert_turn_checkpoint: (a) => {
    const checkpoint = mockTailTurnCheckpoint();
    if (
      a.threadId !== MOCK_CHAT_THREAD_ID ||
      checkpoint === null ||
      a.turnIndex !== checkpoint.turn_index
    ) {
      throw new Error("This turn checkpoint no longer exists.");
    }
    mockChatRevertCutoff = checkpoint.transcript_cutoff_id;
    emitEvent("agent_chat_turn_checkpoint_reverted", {
      thread_id: MOCK_CHAT_THREAD_ID,
      turn_index: checkpoint.turn_index,
      transcript_cutoff_id: checkpoint.transcript_cutoff_id,
      remaining_checkpoints: [],
    });
    return [];
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
    seedPendingAskQuestion(threadId);
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
        launch_mode:
          (a.launchMode as TerminalPreset["launch_mode"]) ?? "new_tab",
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
      if (a.description !== undefined)
        p.description = (a.description as string | null) ?? null;
      if (a.commands != null) p.commands = a.commands as string[];
      if (a.launchMode != null)
        p.launch_mode = a.launchMode as TerminalPreset["launch_mode"];
      if (a.icon !== undefined) p.icon = (a.icon as string | null) ?? null;
      if (a.autoRunOnWorkspace != null)
        p.auto_run_on_workspace = a.autoRunOnWorkspace as boolean;
      if (a.autoRunOnNewTab != null)
        p.auto_run_on_new_tab = a.autoRunOnNewTab as boolean;
      if (a.launchConfig != null)
        p.launch_config = a.launchConfig as TerminalPreset["launch_config"];
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
  /**
   * Records the launch instead of spawning anything.
   *
   * The prompt is the whole point of the agent-handoff surfaces, and in
   * dev there is no PTY to type it into — so it is echoed to the console
   * and kept on `window.__codemuxAgentHandoffs`, which makes "did the
   * prompt survive the route?" answerable from the browser console.
   */
  apply_preset: (a) => {
    const record = {
      workspaceId: String(a.workspaceId ?? ""),
      presetId: String(a.presetId ?? ""),
      overrideMode: (a.overrideMode as string) ?? null,
      prompt: (a.initialPrompt as string) ?? null,
    };
    agentHandoffs.push(record);
    console.info(
      `[mock] apply_preset → ${record.presetId} in ${record.workspaceId} (${record.overrideMode})`,
      record.prompt ?? "(no prompt)",
    );
    return undefined;
  },

  // ── Editors / tooling ──
  detect_editors: () => [],

  // ── GitHub / PRs (pre-seeded; never hit a real API) ──
  check_gh_available: () => true,
  check_gh_status: () => ({ status: "Authenticated", username: "mock-dev" }),
  // Host-scoped readiness for one checkout. Answers from the seeded
  // workspace's `provider_kind` so a GitLab workspace exercises the
  // GitLab copy path rather than falling through to GitHub's.
  check_provider_auth: (a) => {
    const path = a.path as string;
    const workspace = appState.workspaces.find(
      (w) => w.cwd === path || w.project_root === path,
    );
    const kind = workspace?.provider_kind ?? "github";
    return {
      kind,
      supported: kind === "github" || kind === "gitlab",
      installed: true,
      authenticated: true,
      username: kind === "gitlab" ? "mock-glab" : "mock-dev",
      // Standing in for the adapters' own declarations — see
      // `git_provider/provider.rs`. GitLab is missing exactly one
      // operation, and Bitbucket declares nothing at all, so both the
      // partial and the empty case are reachable in the running app.
      operations: withThreadOps(MOCK_PROVIDER_OPERATIONS[kind] ?? NO_MOCK_OPERATIONS),
    };
  },
  // Settings → Source Control. Deliberately mixed so both diagnostic
  // states are visible at once: GitHub ready, GitLab installed-but-
  // missing (the case whose fix-it line points at an install URL), and
  // the two products with no adapter rendering as dimmed rows.
  discover_source_control: () => [
    {
      kind: "github",
      supported: true,
      cli_installed: true,
      cli_version: "gh version 2.63.2 (2026-01-14)",
      authenticated: true,
      account: "mock-dev",
      detail: null,
      capabilities: {
        has_pull_requests: true,
        has_checks: true,
        has_issues: true,
        has_inline_comments: true,
        has_review_threads: true,
        has_deployments: true,
        has_reviews: true,
        has_fork_pr_fetch: true,
      },
    },
    {
      kind: "gitlab",
      supported: true,
      cli_installed: false,
      cli_version: null,
      authenticated: false,
      account: null,
      detail: "`glab` was not found on PATH, so Codemux cannot talk to GitLab.",
      capabilities: {
        has_pull_requests: true,
        has_checks: true,
        has_issues: true,
        has_inline_comments: true,
        has_review_threads: true,
        has_deployments: false,
        has_reviews: true,
        has_fork_pr_fetch: true,
      },
    },
    {
      kind: "bitbucket",
      supported: false,
      cli_installed: false,
      cli_version: null,
      authenticated: false,
      account: null,
      detail: "Codemux has no Bitbucket integration yet.",
      capabilities: {
        has_pull_requests: false,
        has_checks: false,
        has_issues: false,
        has_inline_comments: false,
        has_review_threads: false,
        has_deployments: false,
        has_reviews: false,
        has_fork_pr_fetch: false,
      },
    },
    {
      kind: "azure_devops",
      supported: false,
      cli_installed: false,
      cli_version: null,
      authenticated: false,
      account: null,
      detail: "Codemux has no Azure DevOps integration yet.",
      capabilities: {
        has_pull_requests: false,
        has_checks: false,
        has_issues: false,
        has_inline_comments: false,
        has_review_threads: false,
        has_deployments: false,
        has_reviews: false,
        has_fork_pr_fetch: false,
      },
    },
  ],
  refresh_workspace_pr: (a) => findWorkspace(a.workspaceId)?.pr_number ?? null,
  refresh_workspace_issue: () => null,
  // Issue detail popover (sidebar row + Agent Chat Context Row): expand
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

  // ── Pull Requests page ──
  //
  // Two roots always reject, for the two different reasons the footer
  // has to be able to say: one is unreachable, and one is on a host with
  // no adapter — so the footer line (and the promise that the other
  // repositories keep their rows) is reachable without unplugging
  // anything.
  list_prs_overview: async (a) => {
    const path = String(a.path ?? "");
    if (path === MOCK_UNREACHABLE_ROOT) {
      return Promise.reject(
        "could not resolve host: git.scratchpad.example.com",
      );
    }
    const refused = undeclaredRefusal(path, "list_read", "list or read pull requests");
    if (refused) return Promise.reject(refused);
    // Before the delay: a refusal for spending comes straight back, and
    // it is the fact that it comes back *fast* that makes a retrying
    // client so expensive.
    if (prBudgetSpent) return Promise.reject(RATE_LIMIT_REFUSAL);
    await new Promise((resolve) => setTimeout(resolve, PR_LIST_DELAY_MS));
    if (prOverviewOutage) {
      return Promise.reject("could not resolve host: api.github.com");
    }
    const seed = MOCK_PR_OVERVIEW[path];
    if (!seed) return { viewer: null, items: [] };
    return {
      viewer: seed.viewer,
      // No host answers the rollup on the list call: `null` here means
      // "not measured", and the stats half is what measures it.
      items: seed.numbers.map((n) => overviewItem(n)).filter(Boolean),
    };
  },

  /**
   * What is left of the budget, and when it refills.
   *
   * Asked only after a refusal, and never metered by the host — so the
   * mock answers instantly and without a delay knob. The reset it
   * reports is what the strip counts down to.
   */
  github_rate_limit: async () => ({
    graphql_remaining: prBudgetSpent ? 0 : 4_140,
    graphql_reset: Math.floor(Date.now() / 1000) + 15 * 60,
    core_remaining: 5_000,
    core_reset: Math.floor(Date.now() / 1000) + 42 * 60,
  }),

  /**
   * The slow half, deliberately slow.
   *
   * 800ms is not a guess at gh's latency — it is long enough that the
   * two-phase paint is something you can watch happen in dev rather than
   * something you have to trust the code about. If the dots ever stop
   * arriving after the rows, this handler is where you'll see it.
   */
  list_prs_overview_stats: async (a) => {
    const path = String(a.path ?? "");
    if (prBudgetSpent) return Promise.reject(RATE_LIMIT_REFUSAL);
    if (path === MOCK_UNREACHABLE_ROOT || prOverviewOutage) {
      return Promise.reject("could not resolve host: api.github.com");
    }
    // Gated on `checks_status` in the backend, not `list_read`.
    const refused = undeclaredRefusal(path, "checks_status", "read check or pipeline status");
    if (refused) return Promise.reject(refused);
    await new Promise((resolve) => setTimeout(resolve, PR_STATS_DELAY_MS));
    if (prStatsOutage) {
      return Promise.reject("gh: could not compute status checks");
    }
    const seed = MOCK_PR_OVERVIEW[path];
    if (!seed) return [];
    return seed.numbers.map(overviewStats).filter(Boolean);
  },

  // Closed and merged rows, for the list's state dropdown.
  list_pull_requests: (a) => {
    const path = String(a.path ?? "");
    const refused = undeclaredRefusal(path, "list_read", "list or read pull requests");
    if (refused) return Promise.reject(refused);
    const state = String(a.state ?? "open");
    const open = MOCK_PR_OVERVIEW[path]?.numbers ?? [];
    const history = MOCK_PR_HISTORY[path] ?? [];
    const numbers =
      state === "closed" ? history : state === "all" ? [...open, ...history] : open;
    return numbers.map((n) => mutablePrs[n]).filter(Boolean);
  },

  // The page opens a PR by number, not by whatever branch a checkout
  // happens to be standing on.
  get_github_pr_by_path: (a) => mutablePrs[Number(a.prNumber)] ?? null,
  // Host events only. An agent run reaches the rail by being recorded
  // locally during a real handoff, never from here.
  get_pr_timeline: (a) => MOCK_PR_TIMELINES[Number(a.prNumber)] ?? [],

  // ── Review panel ──
  //
  // Without `check_github_repo` the panel's gate resolves `repoSupported`
  // to null, every PR query stays disabled and the pane sits on a
  // skeleton forever — so this is load-bearing, not decoration.
  check_github_repo: () => true,

  get_branch_pull_request: (a) => prForPath(a.path) ?? null,

  // ── Opening a pull request (5a) ──

  /** The commits the form drafts a title and a description from. */
  git_commits_ahead: (a) => MOCK_COMMITS_AHEAD[String(a.path ?? "")] ?? [],

  /**
   * Create, for real as far as the rest of the mock is concerned.
   *
   * The new pull request is registered under the checkout's path and
   * written onto the workspace snapshot, so everything downstream — the
   * panel flipping to the review view, the sidebar's PR pill, the
   * overview row — behaves the way it would after a real create rather
   * than needing its own special case.
   */
  create_pull_request: (a) => {
    const path = String(a.path ?? "");
    const workspace = appState.workspaces.find(
      (w) => (w.worktree_path ?? w.cwd) === path,
    );
    const number = nextMockPrNumber++;
    const pr: PullRequestInfo = {
      ...structuredClone(MOCK_PULL_REQUESTS[142]),
      number,
      title: String(a.title ?? ""),
      body: String(a.body ?? "") || null,
      base_branch: String(a.base ?? "main"),
      head_branch: workspace?.git_branch ?? null,
      is_draft: Boolean(a.draft),
      state: "OPEN",
      url: `https://github.com/example/codemux/pull/${number}`,
      // A pull request you just opened is yours, has no verdict on it
      // and nobody has been asked to look at it yet — none of which is
      // true of the fixture whose shape is being borrowed.
      author: MOCK_PR_VIEWER,
      review_decision: null,
      review_requests: [],
      latest_reviews: [],
      mergeable: "MERGEABLE",
      merge_state_status: "CLEAN",
      merged_by: null,
      merged_at: null,
      checks_passing: null,
      comments: [],
      totalComments: 0,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    };
    mutablePrs[number] = pr;
    createdPrPaths[path] = number;
    if (workspace) {
      workspace.pr_number = number;
      workspace.pr_state = a.draft ? "draft" : "open";
      workspace.pr_url = pr.url;
      workspace.pr_head_branch = workspace.git_branch;
      emitAppState();
    }
    return pr;
  },

  // Which of the two no-PR empty states applies turns on whether the
  // branch is on the remote at all, so one seeded checkout is
  // deliberately local-only and everything else is pushed.
  get_git_branch_info: (a) => {
    const path = String(a.path ?? "");
    const ws = appState.workspaces.find((w) => (w.worktree_path ?? w.cwd) === path);
    return {
      branch: ws?.git_branch ?? null,
      ahead: ws?.git_ahead ?? 0,
      behind: ws?.git_behind ?? 0,
      has_upstream: path !== MOCK_LOCAL_ONLY_PATH,
    };
  },

  get_pull_request_checks: (a) => {
    const number = a.prNumber != null ? Number(a.prNumber) : prForPath(a.path)?.number;
    if (number == null) return [];
    return checksDetailOverrides[number] ?? MOCK_PR_CHECKS[number] ?? [];
  },

  get_pr_review_comments: (a) => {
    const number = a.prNumber != null ? Number(a.prNumber) : prForPath(a.path)?.number;
    if (number == null) return [];
    return mutablePrReviews[number] ?? MOCK_PR_REVIEWS[number] ?? [];
  },

  get_pr_inline_comments: (a) => {
    const num = Number(a.prNumber);
    return inlineCommentsFor(num);
  },

  get_pr_review_threads: (a) => threadsFor(Number(a.prNumber)),

  /**
   * A reply, with a delay you can see.
   *
   * ~400ms on purpose: the promise is what clears the composer, so the
   * gap between pressing ⌘↵ and the text disappearing is the whole
   * behaviour being demonstrated — the words are still there while the
   * request is in flight, and a failure leaves them there for good.
   */
  reply_to_pr_thread: (a) => {
    const num = Number(a.prNumber);
    const threadId = String(a.threadId ?? "");
    const body = String(a.body ?? "");
    return new Promise((resolve, reject) => {
      setTimeout(() => {
        if (mockThreadReplyFails) {
          reject("host unreachable: dial tcp: lookup api.github.com: no such host");
          return;
        }
        const thread = threadsFor(num).find((t) => t.id === threadId);
        if (!thread) {
          reject(`No thread ${threadId} on #${num}`);
          return;
        }
        const id = Date.now();
        thread.comments.push({
          id: `PRRC_${id}`,
          database_id: id,
          author: "mock-dev",
          body,
          created_at: new Date().toISOString(),
        });
        resolve(undefined);
      }, mockThreadReplyDelayMs);
    });
  },

  set_pr_thread_resolved: (a) => {
    const num = Number(a.prNumber);
    const threadId = String(a.threadId ?? "");
    const thread = threadsFor(num).find((t) => t.id === threadId);
    if (!thread) return Promise.reject(`No thread ${threadId} on #${num}`);
    if (!thread.is_resolvable) {
      return Promise.reject("This discussion cannot be resolved.");
    }
    thread.is_resolved = Boolean(a.resolved);
    return undefined;
  },

  get_github_pr_diff_by_path: (a) => prDiffFor(Number(a.prNumber), Boolean(a.full)),

  get_pr_review_diff: (a) => prDiffFor(Number(a.prNumber), true),

  add_pr_inline_comment: (a) => {
    const num = Number(a.prNumber);
    if (num === MOCK_SUBMIT_FAILURE_PR) {
      return Promise.reject(
        "host unreachable: dial tcp: lookup api.github.com: no such host",
      );
    }
    const comment = (a.comment ?? {}) as {
      file?: string;
      body?: string;
      line?: number;
    };
    inlineCommentsFor(num).push({
      id: Date.now(),
      author: "mock-dev",
      body: String(comment.body ?? ""),
      path: String(comment.file ?? ""),
      line: comment.line ?? null,
      created_at: new Date().toISOString(),
      in_reply_to_id: null,
      pull_request_review_id: null,
    });
    return undefined;
  },

  /**
   * The whole pending review in one call.
   *
   * All-or-nothing on purpose, matching the host: the rejection happens
   * before anything is written, so a failed submit leaves neither a
   * review nor a stray comment behind — which is what makes "your notes
   * are still here" true rather than hopeful.
   */
  submit_pr_review_with_comments: (a) => {
    const num = Number(a.prNumber);
    if (num === MOCK_SUBMIT_FAILURE_PR) {
      return Promise.reject(
        "host unreachable: dial tcp: lookup api.github.com: no such host",
      );
    }
    const event = String(a.event ?? "comment");
    const reviewId = Date.now();
    const reviews = (mutablePrReviews[num] ??= [...(MOCK_PR_REVIEWS[num] ?? [])]);
    reviews.push({
      id: reviewId,
      author: "mock-dev",
      body: String(a.body ?? ""),
      state:
        event === "approve"
          ? "APPROVED"
          : event === "request-changes"
            ? "CHANGES_REQUESTED"
            : "COMMENTED",
      created_at: new Date().toISOString(),
    });

    const comments = Array.isArray(a.comments) ? a.comments : [];
    const inline = inlineCommentsFor(num);
    comments.forEach((raw, i) => {
      const c = raw as { file?: string; body?: string; line?: number };
      inline.push({
        id: reviewId + i + 1,
        author: "mock-dev",
        body: String(c.body ?? ""),
        path: String(c.file ?? ""),
        line: c.line ?? null,
        created_at: new Date().toISOString(),
        in_reply_to_id: null,
        // Grouped under the review that carried them, so they surface
        // as one thread on Summary rather than N loose remarks.
        pull_request_review_id: reviewId,
      });
    });

    const pr = mutablePr(num);
    if (pr && event === "approve") pr.review_decision = "APPROVED";
    if (pr && event === "request-changes") pr.review_decision = "CHANGES_REQUESTED";
    emitAppState();
    return undefined;
  },

  get_check_log_excerpt: (a) => {
    const checks = MOCK_PR_CHECKS[Number(a.prNumber)] ?? [];
    const check = checks.find((c) => c.name === a.checkName);
    // Only failing checks carry an excerpt, mirroring
    // `gh run view --log-failed`.
    return check?.conclusion === "fail" ? MOCK_CHECK_LOG_EXCERPT : "";
  },

  submit_pr_review: (a) => {
    const num = Number(a.prNumber);
    if (num === MOCK_SUBMIT_FAILURE_PR) {
      // The one PR that always refuses, so the submit-failed drift
      // notice (and the "your text survived" promise behind it) can be
      // re-triggered on demand instead of once per session.
      return Promise.reject(
        "host unreachable: dial tcp: lookup api.github.com: no such host",
      );
    }
    const reviews = (mutablePrReviews[num] ??= [
      ...(MOCK_PR_REVIEWS[num] ?? []),
    ]);
    const event = String(a.event ?? "comment");
    reviews.push({
      id: Date.now(),
      author: "mock-dev",
      body: String(a.body ?? ""),
      state:
        event === "approve"
          ? "APPROVED"
          : event === "request-changes"
            ? "CHANGES_REQUESTED"
            : "COMMENTED",
      created_at: new Date().toISOString(),
    });
    const pr = mutablePr(num);
    if (pr && event === "approve") pr.review_decision = "APPROVED";
    if (pr && event === "request-changes") pr.review_decision = "CHANGES_REQUESTED";
    emitAppState();
    return undefined;
  },

  merge_pull_request: (a) => {
    const num = Number(a.prNumber);
    const pr = mutablePr(num);
    if (!pr) return Promise.reject(`No merge target for #${num}`);
    pr.state = "MERGED";
    pr.is_draft = false;
    pr.merged_by = "mock-dev";
    pr.merged_at = new Date().toISOString();
    syncWorkspacePrState(num, "merged");
    return undefined;
  },

  close_pull_request: (a) => {
    const pr = mutablePr(Number(a.prNumber));
    if (!pr) return Promise.reject(`No pull request #${a.prNumber}`);
    pr.state = "CLOSED";
    syncWorkspacePrState(pr.number, "closed");
    return undefined;
  },

  reopen_pull_request: (a) => {
    const pr = mutablePr(Number(a.prNumber));
    if (!pr) return Promise.reject(`No pull request #${a.prNumber}`);
    pr.state = "OPEN";
    syncWorkspacePrState(pr.number, "open");
    return undefined;
  },

  set_pr_ready: (a) => {
    const pr = mutablePr(Number(a.prNumber));
    if (!pr) return Promise.reject(`No pull request #${a.prNumber}`);
    pr.is_draft = !a.ready;
    syncWorkspacePrState(pr.number, a.ready ? "open" : "draft");
    return undefined;
  },

  update_pull_request: (a) => {
    const pr = mutablePr(Number(a.prNumber));
    if (!pr) return Promise.reject(`No pull request #${a.prNumber}`);
    if (typeof a.title === "string") pr.title = a.title;
    if (typeof a.body === "string") pr.body = a.body;
    pr.updated_at = new Date().toISOString();
    return undefined;
  },

  request_pr_review: (a) => {
    const pr = mutablePr(Number(a.prNumber));
    if (!pr) return Promise.reject(`No pull request #${a.prNumber}`);
    const reviewer = String(a.reviewer ?? "").trim();
    if (!reviewer) return Promise.reject("A reviewer name is required.");
    if (!pr.review_requests.includes(reviewer)) pr.review_requests.push(reviewer);
    return undefined;
  },

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

  // ── Web Remote Access ──
  //
  // Server state machine defined above. Reads reflect current state;
  // mutators patch it and re-emit `web-remote-state-changed`, so the panel
  // (and any second desktop UI) live-updates just like the real backend.
  web_remote_status: () => buildWebRemoteStatus(),
  web_remote_enable: () => {
    webRemoteEnabled = true;
    emitWebRemoteState();
    return buildWebRemoteStatus();
  },
  web_remote_disable: () => {
    webRemoteEnabled = false;
    emitWebRemoteState();
    return buildWebRemoteStatus();
  },
  web_remote_set_config: (a) => {
    if (typeof a.port === "number") webRemotePort = a.port;
    if (typeof a.requireApproval === "boolean") {
      webRemoteRequireApproval = a.requireApproval;
    }
    if (
      a.bindScope === "all" ||
      a.bindScope === "tailscale" ||
      a.bindScope === "loopback"
    ) {
      webRemoteBindScope = a.bindScope;
    }
    if (typeof a.accountModeEnabled === "boolean") {
      webRemoteAccountMode = a.accountModeEnabled;
    }
    if (typeof a.trustAccountBrowsers === "boolean") {
      webRemoteTrustAccount = a.trustAccountBrowsers;
    }
    emitWebRemoteState();
    return buildWebRemoteStatus();
  },
  web_remote_create_pairing: () => mockWebRemotePairing(),
  web_remote_list_endpoints: () => mockWebRemoteEndpoints(webRemotePort),
  web_remote_list_sessions: () => webRemoteSessions,
  web_remote_revoke_session: (a) => {
    webRemoteSessions = webRemoteSessions.filter((s) => s.id !== a.sessionId);
    emitWebRemoteState();
    return buildWebRemoteStatus();
  },
  web_remote_approve_session: (a) => {
    webRemoteSessions = webRemoteSessions.map((s) =>
      s.id === a.sessionId ? { ...s, approved: true, connected: true } : s,
    );
    emitWebRemoteState();
    return buildWebRemoteStatus();
  },
  web_remote_reject_session: (a) => {
    webRemoteSessions = webRemoteSessions.filter((s) => s.id !== a.sessionId);
    emitWebRemoteState();
    return buildWebRemoteStatus();
  },

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
  //
  // Sample fleet sized so the Settings section exercises its states:
  // the always-on Codemux row plus user servers from several provider
  // scopes, one errored. Tool counts intentionally sum past the
  // "many tools" advisory threshold so the informational note renders.
  get_mcp_runtime_status: () => MOCK_MCP_RUNTIMES,
  list_mcp_servers: () => MOCK_MCP_SERVERS,
  prime_mcp_runtime: () => MOCK_MCP_RUNTIMES,
  set_mcp_disabled_ids: () => undefined,
  start_mcp_server_cmd: (args: Args) =>
    MOCK_MCP_RUNTIMES.find((r) => r.id === args.id) ?? MOCK_MCP_RUNTIMES[0],
  stop_mcp_server_cmd: (args: Args) => ({
    ...(MOCK_MCP_RUNTIMES.find((r) => r.id === args.id) ?? MOCK_MCP_RUNTIMES[0]),
    status: { kind: "stopped" },
  }),
  restart_mcp_server_cmd: (args: Args) =>
    MOCK_MCP_RUNTIMES.find((r) => r.id === args.id) ?? MOCK_MCP_RUNTIMES[0],
  list_mcp_tools: () =>
    MOCK_MCP_RUNTIMES.flatMap((r) => mockMcpToolsForServer(r.id)),
  list_mcp_tools_for_server: (args: Args) =>
    mockMcpToolsForServer(String(args.id)),

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
      {
        name: "main",
        last_commit_unix: now - 2 * DAY,
        is_local: true,
        is_remote: true,
        is_head: false,
      },
      // The mock project is "on" a feature branch, not main — that's what
      // the composer's base-branch pill should show for a current-checkout
      // thread.
      {
        name: "feature/auth-refactor",
        last_commit_unix: now - 3 * DAY,
        is_local: true,
        is_remote: true,
        is_head: true,
      },
      {
        name: "fix/sidebar-flicker",
        last_commit_unix: now - 5 * DAY,
        is_local: true,
        is_remote: false,
        is_head: false,
      },
      {
        name: "feature/40-dev-mock-tauri-runtime",
        last_commit_unix: now - 8 * DAY,
        is_local: true,
        is_remote: true,
        is_head: false,
      },
      {
        name: "chore/port-detection",
        last_commit_unix: now - 12 * DAY,
        is_local: true,
        is_remote: false,
        is_head: false,
      },
      {
        name: "feature/75-chat-channel",
        last_commit_unix: now - 15 * DAY,
        is_local: true,
        is_remote: true,
        is_head: false,
      },
      {
        name: "agent/094b1560",
        last_commit_unix: now - 21 * DAY,
        is_local: true,
        is_remote: false,
        is_head: false,
      },
      {
        name: "feature-208-address-and-sector-support",
        last_commit_unix: now - 35 * DAY,
        is_local: false,
        is_remote: true,
        is_head: false,
      },
      {
        name: "feature-189-search-history-delete",
        last_commit_unix: now - 38 * DAY,
        is_local: false,
        is_remote: true,
        is_head: false,
      },
      {
        name: "fix-prod-dockerfile",
        last_commit_unix: now - 44 * DAY,
        is_local: false,
        is_remote: true,
        is_head: false,
      },
      {
        name: "feature-186-roleGuard",
        last_commit_unix: now - 60 * DAY,
        is_local: false,
        is_remote: true,
        is_head: false,
      },
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
    // Mint a real generation like the desktop backend + web-remote
    // dispatch do, so `detach/pause/resumePtyOutput` are exercised with a
    // concrete token instead of `undefined`.
    return ++ptyOutputGeneration;
  },
  resize_pty: () => undefined,
  write_to_pty: () => undefined,
  detach_pty_output: (a) => {
    ptyChannels.delete(a.sessionId as string);
    return undefined;
  },
  pause_pty_output: () => undefined,
  resume_pty_output: () => undefined,
  // Twin of `terminal_session_cwds`: the real backend readlinks
  // `/proc/<pid>/cwd` for each session's shell. There is no shell here, so
  // the faithful answer is the session's own recorded directory — sessions
  // the mock doesn't know are omitted, exactly as the real command omits
  // remote/exited ones. To see a non-root cwd hint in the dev UI, push an
  // OSC 7 sequence through the pty stream (see `mockEmitOsc7` below); that
  // exercises the real production handler rather than a mock shortcut.
  terminal_session_cwds: (a) => {
    const ids = new Set((a.sessionIds as string[]) ?? []);
    const out: Record<string, string> = {};
    for (const session of appState.terminal_sessions) {
      if (ids.has(session.session_id) && session.cwd) {
        out[session.session_id] = session.cwd;
      }
    }
    return out;
  },
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
      logScrollback(
        `get ws=${shortId(workspaceId)} pane=${shortId(paneId)} -> miss`,
      );
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

  // ── Changes panel (git status) ──
  // Shape-safe stubs: the panel expects an ARRAY from get_git_status —
  // the `list_*`-only default of `null` crashes `files.filter(...)`.
  // Empty status = "Working tree clean" for git workspaces, and lets
  // the non-git `scratchpad` seed exercise the "Not a git repository"
  // empty state (`is_git: false` → showNoGitState).
  // …with one exception: the checkout the create-pull-request form opens
  // from has uncommitted work, because the form's warning row is only
  // reachable when something is actually uncommitted.
  get_git_status: (a) =>
    String(a.path ?? "") === MOCK_CREATE_PR_PATH ? MOCK_CREATE_PR_DIRTY : [],
  get_merge_state: () => null,
  check_claude_available: () => false,

  // ── Initialize Git (non-git project affordance) ──
  // These three let the "Initialize Git" click be exercised end-to-end
  // in browser dev against the non-git `scratchpad` seed: the probe
  // reports it as non-git, the bare-init mutator flips its `is_git` flag
  // and re-emits, and the follow-up refresh is a no-op (the flag already
  // flipped).
  //
  // `check_is_git_repo` returns false only for the `scratchpad` seed
  // (`src/dev/mock-fixtures.ts`), true for every real git project.
  check_is_git_repo: (a) => !String(a.path ?? "").endsWith("/scratchpad"),

  // Bare `git init` (no stage/commit) — flip `is_git` on every workspace
  // rooted at the given path so the UI swaps the affordance for the
  // normal git surfaces without waiting for a poll.
  init_git_repo_no_commit: (a) => {
    const path = String(a.path ?? "");
    let mutated = false;
    for (const ws of appState.workspaces) {
      if ((ws.project_root ?? ws.cwd) === path) {
        ws.is_git = true;
        mutated = true;
      }
    }
    if (mutated) emitAppState();
    return "Repository initialized";
  },
  // No-op: the init handler above already flipped the flag + re-emitted.
  refresh_workspace_git_info: () => undefined,

  // Fake `git clone` that streams realistic `git-clone-progress` events
  // over ~5s, then resolves with the target dir — so the New Project /
  // Clone progress UI is demoable in `npm run dev` without a real remote.
  // Mirrors src-tauri/src/commands/git.rs:GitCloneProgress (camelCase).
  git_clone_repo: async (a) => {
    const targetDir = String(a.targetDir ?? `${MOCK_HOME_DIR}/projects/cloned`);
    const emit = (phase: string, percent: number | null, detail: string) =>
      emitEvent("git-clone-progress", {
        targetDir,
        phase,
        percent,
        detail,
      });
    const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

    emit("Cloning", null, `Cloning into '${targetDir.split("/").pop()}'...`);
    await sleep(500);
    for (const pct of [20, 55, 88, 100]) {
      emit(
        "Counting objects",
        pct,
        `remote: Counting objects: ${pct}% (2934/2934)`,
      );
      await sleep(300);
    }
    // Receiving objects climbs 5%→96% with a throughput detail line.
    for (const pct of [5, 23, 47, 68, 84, 96]) {
      const mib = ((pct / 100) * 42).toFixed(2);
      emit(
        "Receiving objects",
        pct,
        `Receiving objects: ${pct}% (${Math.round((pct / 100) * 2934)}/2934), ${mib} MiB | 1.20 MiB/s`,
      );
      await sleep(500);
    }
    emit("Resolving deltas", 100, "Resolving deltas: 100% (1420/1420), done.");
    await sleep(300);
    return targetDir;
  },

  // ── Deferred worktree first-send (Thread Scope) ──
  //
  // Faithfully reproduces the async race the fix targets: the new
  // worktree workspace is RETURNED synchronously from the invoke, but
  // only reaches the app-store via an ASYNCHRONOUS `app-state-changed`
  // emit (a macrotask later) — so a synchronous store read-back right
  // after the invoke resolves misses, exactly like the real Tauri
  // runtime. The workspace carries a DISTINCT cwd (the sibling worktree
  // path, not the repo root) so root-vs-worktree is observable in dev.
  generate_branch_name: (a) => {
    const slug = String(a.prompt ?? "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .split("-")
      .slice(0, 4)
      .join("-");
    return slug || "mock-branch";
  },
  generate_random_branch_name: () =>
    `mock-worktree-${Math.random().toString(36).slice(2, 8)}`,
  create_worktree_workspace: (a) => {
    const ws = buildWorktreeWorkspace(
      String(a.repoPath ?? `${MOCK_HOME_DIR}/projects/codemux`),
      String(a.branch ?? "mock-branch"),
    );
    // ASYNC emit — the workspace lands in the store only AFTER this
    // invoke's promise has already resolved (macrotask), mirroring the
    // real runtime's event ordering that the bug depended on.
    setTimeout(() => {
      appState = { ...appState, workspaces: [...appState.workspaces, ws] };
      emitAppState();
    }, 0);
    return ws.workspace_id;
  },
  /**
   * A real extra terminal tab, so an agent handoff is visible in dev.
   *
   * The handoff opens a fresh tab and then applies the preset to it
   * (the only `apply_preset` path that carries a prompt), so a mock that
   * returned a bare id would make the most important half of the
   * interaction invisible in the browser.
   */
  create_tab: (a) => {
    const ws = findWorkspace(a.workspaceId);
    if (!ws || a.kind !== "terminal") return `tab-mock-${Date.now()}`;
    const n = ++mockTabSeq;
    const paneId = `pane-mock-tab-${n}`;
    const surfaceId = `surface-mock-tab-${n}`;
    const tabId = `tab-mock-tab-${n}`;
    const sessionId = `sess-mock-tab-${n}`;
    const label = `Terminal ${ws.tabs.filter((t) => t.kind === "terminal").length + 1}`;

    ws.surfaces.push({
      surface_id: surfaceId,
      title: label,
      root: { kind: "terminal", pane_id: paneId, session_id: sessionId, title: label },
      active_pane_id: paneId,
    });
    ws.tabs.push({
      tab_id: tabId,
      kind: "terminal",
      title: label,
      surface_id: surfaceId,
      browser_id: null,
      icon: null,
    });
    ws.active_tab_id = tabId;
    ws.active_surface_id = surfaceId;
    appState.terminal_sessions.push({
      session_id: sessionId,
      title: label,
      shell: "/bin/bash",
      cwd: ws.cwd,
      cols: 120,
      rows: 32,
      state: "ready",
      last_message: null,
      exit_code: null,
      original_command: null,
      adapter_captures: {},
    });
    emitAppState();
    return tabId;
  },
  agent_chat_create_pane: (a) => {
    // The deferred worktree workspace already carries a fresh
    // agent_chat pane (empty thread); bind the session to it. Falls
    // back to a synthetic id for any other workspace.
    const ws = findWorkspace(a.workspaceId);
    const root = ws?.surfaces?.[0]?.root;
    if (root && root.kind === "agent_chat") return root.pane_id;
    return `pane-mock-${Date.now()}`;
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
  set_workspace_pinned: (a) => {
    const ws = findWorkspace(a.workspaceId);
    if (ws) {
      const pinned = Boolean(a.pinned);
      if (pinned && ws.pinned_at == null) ws.pinned_at = Date.now();
      if (!pinned) ws.pinned_at = null;
      emitAppState();
    }
    return undefined;
  },
  close_workspace: (a) => removeWorkspace(a.workspaceId),
  close_workspace_with_worktree: (a) => {
    const ws = findWorkspace(a.workspaceId);
    if (ws && a.removeWorktree === true) {
      // Mirror the real backend's guards: the protected repo root is
      // never destructively deletable, and a dirty worktree requires an
      // explicit force (the /use force/i message drives the delete
      // dialog's escalation state).
      if (ws.protected === true) {
        return Promise.reject(
          "This workspace is the protected repo root — its files can't be deleted. Archive it instead.",
        );
      }
      if (a.forceDelete !== true && ws.git_changed_files > 0) {
        return Promise.reject(
          `Worktree has ${ws.git_changed_files} uncommitted change(s). Use force to override.`,
        );
      }
    }
    return removeWorkspace(a.workspaceId);
  },

  // ── Workspace archive ──
  //
  // Twin of the Rust archive commands: archive moves a live workspace
  // into `archived_workspaces` (files untouched), unarchive rebuilds a
  // workspace row and activates it, delete drops the entry (respecting
  // the protected refusal and the dirty-worktree force escalation).
  archive_workspace: (a) => {
    const ws = findWorkspace(a.workspaceId);
    if (!ws) return Promise.reject("Workspace not found.");
    if (ws.attach_only === true) {
      return Promise.reject(
        "Attach-in-place workspaces can't be archived — close them from the workspaces overview.",
      );
    }
    // Mirror the backend's remote refusal: a workspace pushed to a host
    // (host_id set) can't be archived locally — the plain close is its
    // removal path (it stays available on its host in the overview).
    if (ws.host_id !== null && ws.host_id !== undefined) {
      return Promise.reject(
        "Remote workspaces can't be archived — they're managed from the Workspaces Overview. Pull the workspace back to this device first.",
      );
    }
    const archiveId = `arch-${ws.workspace_id}-${++archiveSeq}`;
    const entry: ArchivedWorkspaceSnapshot = {
      archive_id: archiveId,
      workspace_id: ws.workspace_id,
      title: ws.title,
      cwd: ws.cwd,
      worktree_path: ws.worktree_path,
      project_root: ws.project_root,
      project_uid: ws.project_uid ?? null,
      workspace_kind:
        ws.workspace_kind === "main" || ws.workspace_kind === "worktree"
          ? ws.workspace_kind
          : ws.worktree_path
            ? "worktree"
            : "main",
      git_branch: ws.git_branch,
      protected: ws.protected === true,
      is_git: ws.is_git ?? true,
      archived_at: Math.floor(Date.now() / 1000),
    };
    // A live-dirty workspace stays dirty in the archive, so deleting it
    // later exercises the force escalation just like the seeded entry.
    if (ws.git_changed_files > 0) dirtyArchivedIds.add(archiveId);
    const next = appState.workspaces.filter(
      (w) => w.workspace_id !== ws.workspace_id,
    );
    let active = appState.active_workspace_id;
    if (active === ws.workspace_id) active = next[0]?.workspace_id ?? "";
    appState = {
      ...appState,
      workspaces: next,
      active_workspace_id: active,
      archived_workspaces: [...(appState.archived_workspaces ?? []), entry],
    };
    emitAppState();
    return archiveId;
  },
  unarchive_workspace: (a) => {
    const entries = appState.archived_workspaces ?? [];
    const entry = entries.find((e) => e.archive_id === a.archiveId);
    if (!entry) {
      return Promise.reject(
        "Archive entry not found — nothing left to restore.",
      );
    }
    const ws = buildRestoredWorkspace(entry);
    appState = {
      ...appState,
      workspaces: [...appState.workspaces, ws],
      // The real backend activates the restored workspace.
      active_workspace_id: ws.workspace_id,
      archived_workspaces: entries.filter((e) => e.archive_id !== a.archiveId),
    };
    emitAppState();
    return ws.workspace_id;
  },
  delete_archived_workspace: (a) => {
    const entries = appState.archived_workspaces ?? [];
    const entry = entries.find((e) => e.archive_id === a.archiveId);
    if (!entry) return Promise.reject("Archive entry not found.");
    if (entry.protected && a.deleteWorktree === true) {
      return Promise.reject(
        "This entry is a protected repo root — its files are never deleted from the archive.",
      );
    }
    if (
      a.deleteWorktree === true &&
      a.forceDelete !== true &&
      dirtyArchivedIds.has(entry.archive_id)
    ) {
      return Promise.reject(
        "Worktree has 3 uncommitted change(s). Use force to override.",
      );
    }
    dirtyArchivedIds.delete(entry.archive_id);
    appState = {
      ...appState,
      archived_workspaces: entries.filter((e) => e.archive_id !== a.archiveId),
    };
    emitAppState();
    return undefined;
  },
};

// Archive entries whose (simulated) worktree is dirty: deleting them
// with forceDelete=false rejects with the /use force/i message. Seeded
// with the fixture entry so the escalation is testable on first run.
const dirtyArchivedIds = new Set<string>([MOCK_DIRTY_ARCHIVE_ID]);
let archiveSeq = 0;

/** Rebuild a live workspace row from an archive entry — the mock twin
 *  of the backend's unarchive restore. Mirrors `buildWorktreeWorkspace`'s
 *  structure (fresh thread-less agent_chat pane) with the entry's
 *  identity fields carried back over. */
function buildRestoredWorkspace(
  entry: ArchivedWorkspaceSnapshot,
): WorkspaceSnapshot {
  const n = ++archiveSeq;
  const paneId = `pane-restored-${n}`;
  const surfaceId = `surface-restored-${n}`;
  const tabId = `tab-restored-${n}`;

  const pane: PaneNodeSnapshot = {
    kind: "agent_chat",
    pane_id: paneId,
    title: entry.title,
    thread_id: null,
    provider: "claude",
    cwd: entry.cwd,
  };
  const surface: SurfaceSnapshot = {
    surface_id: surfaceId,
    title: entry.title,
    root: pane,
    active_pane_id: paneId,
  };
  const tab: TabSnapshot = {
    tab_id: tabId,
    kind: "terminal",
    title: entry.title,
    surface_id: surfaceId,
    browser_id: null,
    icon: null,
  };

  return {
    workspace_id: entry.workspace_id,
    title: entry.title,
    workspace_type: "standard",
    cwd: entry.cwd,
    is_git: entry.is_git,
    git_branch: entry.git_branch,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: entry.worktree_path,
    project_root: entry.project_root,
    project_uid: entry.project_uid,
    workspace_kind: entry.workspace_kind,
    protected: entry.protected,
    divergent_copy: false,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [tab],
    active_tab_id: tabId,
    active_surface_id: surfaceId,
    surfaces: [surface],
    host_id: null,
    remote_cwd: null,
    attach_only: false,
  };
}

/** Build a `worktree`-kind workspace with a fresh (thread-less)
 *  agent_chat pane and a DISTINCT sibling-worktree cwd, mirroring the
 *  real `create_worktree_workspace` with the `"empty"` layout. Kept
 *  self-contained (mock-fixtures' builders aren't exported) so the
 *  deferred-worktree first-send path is exercisable end-to-end in
 *  `npm run dev`. */
let deferredWorktreeSeq = 0;
let mockTabSeq = 0;

/**
 * Every preset launch the dev session has performed, newest last.
 *
 * Exposed on `window` so the agent-handoff surfaces can be checked in a
 * plain browser: the question those buttons have to answer is "did the
 * composed prompt reach the thread", and this is where the answer is.
 */
const agentHandoffs: {
  workspaceId: string;
  presetId: string;
  overrideMode: string | null;
  prompt: string | null;
}[] = [];
function buildWorktreeWorkspace(
  repoPath: string,
  branch: string,
): WorkspaceSnapshot {
  const n = ++deferredWorktreeSeq;
  const repoName = repoPath.split("/").filter(Boolean).pop() ?? "repo";
  // Sibling worktree path — NOT the repo root — so a caller can tell the
  // worktree cwd apart from the parent checkout.
  const cwd = `${MOCK_HOME_DIR}/.codemux/worktrees/${repoName}/${branch}`;
  const paneId = `pane-deferred-wt-${n}`;
  const surfaceId = `surface-deferred-wt-${n}`;
  const tabId = `tab-deferred-wt-${n}`;

  const pane: PaneNodeSnapshot = {
    kind: "agent_chat",
    pane_id: paneId,
    title: branch,
    thread_id: null,
    provider: "claude",
    cwd,
  };
  const surface: SurfaceSnapshot = {
    surface_id: surfaceId,
    title: branch,
    root: pane,
    active_pane_id: paneId,
  };
  const tab: TabSnapshot = {
    tab_id: tabId,
    kind: "terminal",
    title: branch,
    surface_id: surfaceId,
    browser_id: null,
    icon: null,
  };

  return {
    workspace_id: `ws-deferred-worktree-${n}`,
    title: branch,
    workspace_type: "standard",
    cwd,
    git_branch: branch,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: cwd,
    project_root: repoPath,
    project_uid: null,
    workspace_kind: "worktree",
    protected: false,
    divergent_copy: false,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [tab],
    active_tab_id: tabId,
    active_surface_id: surfaceId,
    surfaces: [surface],
    host_id: null,
    remote_cwd: null,
    attach_only: false,
  };
}

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

async function invoke(
  cmd: string,
  args: Args | Uint8Array | ArrayBuffer = {},
  options?: unknown,
): Promise<unknown> {
  // Raw-body invoke: `agent_chat_stage_image` sends the image bytes as
  // the payload (not an args object), with the MIME on an `x-media-type`
  // header. Stash the bytes under a fake absolute staging path and return
  // it — the turn later references the path via `send_turn.images`.
  if (args instanceof Uint8Array || args instanceof ArrayBuffer) {
    if (cmd === "agent_chat_stage_image") {
      const bytes = args instanceof Uint8Array ? args : new Uint8Array(args);
      const headers = (
        options as { headers?: Record<string, string> } | undefined
      )?.headers;
      const mediaType = headers?.["x-media-type"] ?? "image/png";
      const ext = mediaType.split("/")[1] ?? "png";
      const path = `${MOCK_HOME_DIR}/.codemux/staging/chat-image-${++stagedImageSeq}.${ext}`;
      stagedImages.set(path, { bytes: new Uint8Array(bytes), mediaType });
      return { path, media_type: mediaType };
    }
    return defaultResult(cmd);
  }

  const handler = handlers[cmd];
  if (handler) return handler(args);

  const viaPlugin = routePlugin(cmd, args);
  if (viaPlugin !== MISS) return viaPlugin;

  return defaultResult(cmd);
}

interface TauriInternals {
  invoke: (
    cmd: string,
    args?: Args | Uint8Array | ArrayBuffer,
    options?: unknown,
  ) => Promise<unknown>;
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

(
  window as unknown as { __TAURI_INTERNALS__: TauriInternals }
).__TAURI_INTERNALS__ = internals;

(
  window as unknown as {
    __TAURI_EVENT_PLUGIN_INTERNALS__: {
      unregisterListener: (event: string, eventId: number) => void;
    };
  }
).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener };

// Dev affordance: fire a backend-style global `notification` event from the
// browser console to exercise the web-remote notification bridge
// (`useWebNotifications`). Enable the remote flag first so the hook is live:
//   window.__CODEMUX_REMOTE__ = true
//   __codemuxMockNotify("Agent finished — Demo", "Ready for review", "Demo")
(
  window as unknown as {
    __codemuxMockNotify: (
      title: string,
      body?: string,
      workspaceTitle?: string,
    ) => void;
  }
).__codemuxMockNotify = (
  title,
  body = "Codemux is waiting for your review.",
  workspaceTitle = "Demo",
) => {
  emitEvent("notification", { title, body, workspace_title: workspaceTitle });
};

// Dev affordance: rewrite a PR's branch under you.
//
// Flips the head sha and swaps in that PR's `afterForcePush` diff, which
// is the whole force-push story the Code tab has to survive: the 2.5s
// detail poll notices the new sha, refetches the diff, and re-anchors
// every pending note against it. Deterministic — the same call always
// produces the same rewritten diff — and reversible, so the flow can be
// walked more than once without a reload.
//
//   __codemuxMockForcePush()      // PR 172, the seeded review PR
//   __codemuxMockForcePush(172, false)   // put it back
(
  window as unknown as {
    __codemuxMockForcePush: (prNumber?: number, rewritten?: boolean) => string;
  }
).__codemuxMockForcePush = (prNumber = 172, rewritten = true) => {
  const pr = mutablePr(prNumber);
  if (!pr) return `no mock PR #${prNumber}`;
  if (rewritten) forcePushedPrs.add(prNumber);
  else forcePushedPrs.delete(prNumber);
  pr.head_ref_oid = rewritten
    ? `forcepush${prNumber}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa`.slice(0, 40)
    : `mock${prNumber}headsha0000000000000000000`.slice(0, 40);
  pr.updated_at = new Date().toISOString();
  emitAppState();
  return `#${prNumber} head is now ${pr.head_ref_oid}`;
};

// Dev affordances: fire each of the two pull-request toasts on demand.
//
// Both toasts are raised by a *transition* between consecutive polls, so
// neither can be reached by seeding a state — a pull request that is
// already red at load has not just turned red, and correctly says
// nothing. These flip the overview fixture and kick the shared query, so
// the very next poll sees a change that did not exist a moment ago.
//
//   __codemuxMockReviewRequest()   // #482 starts waiting on you
//   __codemuxMockCiFail()          // #285 goes red on rust (ubuntu-latest)
//   __codemuxMockCiFail(285, false)  // back to green, so it can fire again
function kickPrOverview(): void {
  (
    window as unknown as {
      __codemuxQueryClient?: {
        invalidateQueries: (filters: { predicate: (q: { queryKey: unknown[] }) => boolean }) => void;
      };
    }
  ).__codemuxQueryClient?.invalidateQueries({
    predicate: (q) => q.queryKey[0] === "prs",
  });
}

(
  window as unknown as {
    __codemuxMockReviewRequest: (prNumber?: number, requested?: boolean) => string;
  }
).__codemuxMockReviewRequest = (prNumber = 482, requested = true) => {
  if (!mutablePrs[prNumber]) return `no mock PR #${prNumber}`;
  reviewRequestOverrides[prNumber] = requested ? [MOCK_PR_VIEWER] : [];
  kickPrOverview();
  return requested
    ? `#${prNumber} is now waiting on ${MOCK_PR_VIEWER}`
    : `#${prNumber} no longer needs your review`;
};

(
  window as unknown as {
    __codemuxMockCiFail: (prNumber?: number, failing?: boolean) => string;
  }
).__codemuxMockCiFail = (prNumber = 285, failing = true) => {
  const pr = mutablePrs[prNumber];
  if (!pr) return `no mock PR #${prNumber}`;
  if (failing) {
    checksRollupOverrides[prNumber] = "failing";
    // The toast's second line and the [Fix] handoff both name a check,
    // so the per-check list has to agree with the rollup.
    checksDetailOverrides[prNumber] = [
      ...(MOCK_PR_CHECKS[prNumber] ?? []),
      {
        name: "rust (ubuntu-latest)",
        status: "COMPLETED",
        conclusion: "fail",
        elapsed_time: "2m 14s",
        detail_url: `https://github.com/example/codemux/actions/runs/9${prNumber}`,
        started_at: new Date(Date.now() - 134_000).toISOString(),
        completed_at: new Date().toISOString(),
      },
    ];
  } else {
    delete checksRollupOverrides[prNumber];
    delete checksDetailOverrides[prNumber];
  }
  kickPrOverview();
  return failing
    ? `#${prNumber} is now failing on rust (ubuntu-latest)`
    : `#${prNumber} is green again`;
};

// Dev affordance: the two thread switches.
//
//   __codemuxMockThreadReplyFail(true)   // every reply is refused
//   __codemuxMockThreadReplyFail(false)  // back to normal
//
// The first one is how you see binding rule 4 hold: type a reply, send
// it, and the words are still in the box with the reason beside them and
// Retry sending exactly what failed.
(
  window as unknown as {
    __codemuxMockThreadReplyFail: (failing?: boolean) => string;
  }
).__codemuxMockThreadReplyFail = (failing = true) => {
  mockThreadReplyFails = failing;
  return failing
    ? "thread replies will now be refused by the host"
    : "thread replies succeed again";
};

// How long a reply takes, for looking at the in-flight state.
//
//   __codemuxMockThreadReplyDelay(5000)
(
  window as unknown as {
    __codemuxMockThreadReplyDelay: (ms?: number) => string;
  }
).__codemuxMockThreadReplyDelay = (ms = 400) => {
  mockThreadReplyDelayMs = ms;
  return `thread replies now take ${ms}ms`;
};

// Withdraw the two thread operations from every host's declaration, so
// the read-only thread list can be looked at. Switch workspaces (or wait
// out the 60s provider-auth cache) for the declaration to be re-read.
//
//   __codemuxMockThreadOpsOff()       // no composer, no Resolve button
//   __codemuxMockThreadOpsOff(false)  // declared again
(
  window as unknown as {
    __codemuxMockThreadOpsOff: (off?: boolean) => string;
  }
).__codemuxMockThreadOpsOff = (off = true) => {
  mockThreadOpsDeclared = !off;
  if (off) localStorage.setItem(THREAD_OPS_OFF_KEY, "1");
  else localStorage.removeItem(THREAD_OPS_OFF_KEY);
  return off
    ? "thread reply/resolve undeclared — reload to see the read-only list"
    : "thread reply/resolve declared again — reload to pick it up";
};

// Dev affordance: read back what the agent-handoff buttons actually sent.
//   window.__codemuxAgentHandoffs.at(-1).prompt
(
  window as unknown as { __codemuxAgentHandoffs: typeof agentHandoffs }
).__codemuxAgentHandoffs = agentHandoffs;

// ── The performance pack's three switches ─────────────────────────────

// Dev affordance: throw away the carried paint.
//
// The snapshot's whole point is that it makes the *second* launch fast,
// which makes the first one hard to see. Clear it and reload to watch
// the cold path; reload again to watch the carried one.
//
//   __codemuxClearPrSnapshot()
(
  window as unknown as { __codemuxClearPrSnapshot: () => string }
).__codemuxClearPrSnapshot = () => {
  clearPrOverviewSnapshot();
  return "pull-request snapshot cleared — reload for a cold paint";
};

// Dev affordance: take the host away, and watch the rows stay.
//
//   __codemuxMockPrOutage()              // everything fails
//   __codemuxMockPrOutage(true, "stats")  // only the slow half fails
//   __codemuxMockPrOutage(true, "budget") // the host refuses: budget spent
//   __codemuxMockPrOutage(false)         // back to normal
(
  window as unknown as {
    __codemuxMockPrOutage: (down?: boolean, which?: "all" | "stats" | "budget") => string;
  }
).__codemuxMockPrOutage = (down = true, which = "all") => {
  prBudgetSpent = down && which === "budget";
  prStatsOutage = down && which !== "budget";
  prOverviewOutage = down && which === "all";
  kickPrOverview();
  if (!down) return "pull-request host reachable again";
  if (which === "budget") {
    return "the host now refuses for spending — the page stops polling and says until when";
  }
  return which === "stats"
    ? "the stats half now fails — rows keep the checks they had"
    : "every root now fails — the list keeps its rows and says how old they are";
};

// Dev affordances: change how slow each half is.
//   __codemuxMockPrStatsDelay(3000)
//   __codemuxMockPrListDelay(3000)
(
  window as unknown as { __codemuxMockPrStatsDelay: (ms: number) => string }
).__codemuxMockPrStatsDelay = (ms) => {
  PR_STATS_DELAY_MS = Math.max(0, ms);
  localStorage.setItem("codemux:dev:pr-stats-delay", String(PR_STATS_DELAY_MS));
  return `stats now take ${PR_STATS_DELAY_MS}ms (across reloads)`;
};
(
  window as unknown as { __codemuxMockPrListDelay: (ms: number) => string }
).__codemuxMockPrListDelay = (ms) => {
  PR_LIST_DELAY_MS = Math.max(0, ms);
  localStorage.setItem("codemux:dev:pr-list-delay", String(PR_LIST_DELAY_MS));
  return `the listing now takes ${PR_LIST_DELAY_MS}ms (across reloads)`;
};

export {};
