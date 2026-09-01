/**
 * Hand-curated seed data for the dev-only Tauri mock (`tauri-mock.ts`).
 *
 * This module is loaded ONLY by `tauri-mock.ts`, which is itself
 * dynamically imported by `main.tsx` exclusively under `npm run dev`
 * (Vite dev) when no real Tauri runtime is present. The dual guard in
 * `main.tsx` (`import.meta.env.DEV` + `!("__TAURI_INTERNALS__" in window)`)
 * guarantees Rollup tree-shakes this whole file out of the production
 * bundle and that it never loads under `npm run tauri:dev`.
 *
 * The shapes here mirror `src/tauri/types.ts` exactly so TypeScript
 * catches drift the moment a real snapshot field changes.
 *
 * Coverage baked into the seed (per issue #40 acceptance criteria):
 *   - 3 mock projects (codemux / vexis / personal-site)
 *   - each project has a primary workspace + worktree workspaces
 *   - PR states: open, merged, closed, draft (≥1 each)
 *   - agent states (via `pane_statuses`): working (amber pulse),
 *     review (green), permission (red pulse)
 *   - a workspace with `linked_issue` populated
 *   - a workspace with `notifications_muted: true`
 *   - a workspace with non-zero git ahead/behind/additions/deletions
 *   - a workspace with a non-zero `notification_count`
 */
import type {
  AgentBrowserSession,
  AppStateSnapshot,
  ArchivedWorkspaceSnapshot,
  AuthUser,
  CheckInfo,
  CodemuxConfigSnapshot,
  CommitSummary,
  FileEntry,
  GitFileStatus,
  InlineReviewComment,
  PrReviewThread,
  LinkedIssue,
  PrTimelineEvent,
  PullRequestInfo,
  ReviewComment,
  PaneNodeSnapshot,
  PaneStatus,
  PersistenceSchema,
  SurfaceSnapshot,
  TabSnapshot,
  TerminalSessionSnapshot,
  WebRemoteEndpoint,
  WebRemotePairingInfo,
  WebRemoteSessionView,
  WorkspaceSnapshot,
} from "@/tauri/types";
import { STRESS_THREAD_PREFIX, getStressFixture } from "./stress-fixture";

/** Authenticated user reported by the mock `check_auth`. Bypasses the
 *  login screen so the real app shell renders immediately. */
export const MOCK_USER: AuthUser = {
  id: "dev-mock-user",
  email: "dev@localhost",
  name: "Mock Dev",
  image: null,
};

const HOME = "/home/dev";
const PROJECTS = `${HOME}/projects`;

// ── Small builders ──────────────────────────────────────────────────
//
// Keep the workspace literals below readable: a workspace's main
// surface is always a single terminal pane in the mock (the terminal
// body itself is out of scope — see issue #40 "Out of scope"). These
// helpers stamp the repetitive pane/surface/tab plumbing.

let paneSeq = 0;
let sessionSeq = 0;

interface PaneRefs {
  pane: PaneNodeSnapshot;
  surface: SurfaceSnapshot;
  tab: TabSnapshot;
  session: TerminalSessionSnapshot;
}

/** Build a one-terminal surface (+ its tab + backing session) for a
 *  workspace. The returned `pane.pane_id` is what you key into
 *  `pane_statuses` to drive the sidebar status dot. */
function terminalSurface(label: string, cwd: string): PaneRefs {
  const n = ++paneSeq;
  const s = ++sessionSeq;
  const paneId = `pane-${n}`;
  const sessionId = `sess-${s}`;
  const surfaceId = `surface-${n}`;
  const tabId = `tab-${n}`;

  const pane: PaneNodeSnapshot = {
    kind: "terminal",
    pane_id: paneId,
    session_id: sessionId,
    title: label,
  };
  const surface: SurfaceSnapshot = {
    surface_id: surfaceId,
    title: label,
    root: pane,
    active_pane_id: paneId,
  };
  const tab: TabSnapshot = {
    tab_id: tabId,
    kind: "terminal",
    title: label,
    surface_id: surfaceId,
    browser_id: null,
    icon: null,
  };
  const session: TerminalSessionSnapshot = {
    session_id: sessionId,
    title: label,
    shell: "/bin/bash",
    cwd,
    cols: 120,
    rows: 32,
    state: "ready",
    last_message: null,
    exit_code: null,
    original_command: null,
    adapter_captures: {},
  };
  return { pane, surface, tab, session };
}

/** Thread id of the pre-seeded agent-chat workspace. The mock's
 *  `agent_chat_list_messages` returns a long generated transcript for
 *  this id so the virtualized MessageList can be exercised in a plain
 *  browser (issue #77). */
export const MOCK_CHAT_THREAD_ID = "thread-mock-chat";

/** Inline data-URL PNG (200×120, teal field + magenta band + border)
 *  used to seed a user turn's attached image in the dev mock. A
 *  self-contained data URL is what an optimistic send produces at
 *  runtime, and — crucially — it passes straight through
 *  `resolveAssetSrc` (bypassing `convertFileSrc`), so the thumbnail +
 *  lightbox render in a plain browser where Tauri's asset protocol is
 *  unavailable. Persisted turns use `{ path, media_type }`; the mock
 *  smuggles the data URL in via `path` so the same hydrate mapping
 *  (`path` → `src`) exercises the render path. */
export const MOCK_USER_IMAGE_DATA_URL =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAMgAAAB4CAIAAAA48Cq8AAADNUlEQVR4nO3cu7HVUBAF0RcENjYOaRAiQTybSIgIm6K4+tw5mj57umoHoOlapqSPL1+/OVe+j/YncJETllsyYbkl+wvW7x8/y/f91yd83lt176OwhrdmbtG9T8Manhu78nt7YM1sDV/tvW2wZuZuP+exe5thjWo96t5+WHNaj7oXAWtO7jn3gmDFtx51LwtWfO459xJhpbYedS8UVmTrUfdyYUXmnnMvHVZS61H3bgArKfece7eBFdB61L07wdq99ah7N4O1e+459/4XVnkdW4+69xWsFYGG555z7zEsbWnrxr2nYMF5tafU1r/3XoBFtrVL7jn3XoOlLW2tgiUveS2EpS1trYIF59Wedritd2GRbQFzz+FVAEtb2loFS17yWghLW9paBQvOq53OHFtLYJFtyWtvWNpC1YuCJS9avShY2kLVi4IF59VOJ9LWc7DItuS1NyxtoepFwZIXrV4ULG2h6kXB0haqXhQsedHqRcHSFqpeFCx50epFwdIWql4ULDivdjob2SLCItuS196wtIWqFwVLXrR6UbC0haoXBQvOq70M09Y2sMi25LU3LG2h6kXBkhetXhQsbaHqXYM1MJC2Cu99BWtgIHlV3XsAa2AdbZXcewxrZiB5vXnvWVgD62jrnV2ANTOQth6CNS2QvJ6DNaeOtp6GNSeQvBpgxdfRVhus+EDa6oQVHEhe9+qVwYqso63b9SphRQaS17169bCS6mjrdr0lsJICaeteuoWwMgLJ6169tbB2r9O+9ji36y2HtXsgwtrj3Kj36p1363DWHudqveOPKYYH4qy9zKV0x7C0hVp7nJP1TsHSFmrtcc7UOwtLXrS1x3m9a7C0hVp7nEpY2kKtPU4lLHnR1h6nEpa2UGuPUwlLXrS1x6mEpS3U2uNUwtIWau1x6v+PlRdIXghY2kItCpa8aIuCpS3UomDJC7U0WNpCLQqWtlCLgiUv2qJgaQu1KFjyQi0NlrZQi4KlLdSiYMmLtihY2kItCpa8UEuDpS3UomBpC7UoWPKiLQqWtlCLgiUv1NJgaQu1KFjaQi0Klrxoi4KlLdSiYMkLtTuwnKuasNySCcstmbDckv0BOQvYwYr8AtoAAAAASUVORK5CYII=";

/** Build an agent-chat surface (+ tab) for a workspace. Mirrors the
 *  backend's `create_agent_chat_pane`: the surface root is an
 *  `agent_chat` pane and the tab keeps `kind: "terminal"` (TabKind has
 *  no chat variant — the pane node drives rendering).
 *
 *  `threadId` defaults to the pre-seeded transcript thread; pass
 *  `null` for a fresh pane with no thread bound yet — exactly the
 *  state `create_agent_chat_pane` leaves a new pane in, which drives
 *  the start_session → attach channel → streamed `content_delta`
 *  flow (issue #75). */
function chatSurface(
  label: string,
  cwd: string,
  threadId: string | null = MOCK_CHAT_THREAD_ID,
): { pane: PaneNodeSnapshot; surface: SurfaceSnapshot; tab: TabSnapshot } {
  const n = ++paneSeq;
  const paneId = `pane-${n}`;
  const surfaceId = `surface-${n}`;
  const tabId = `tab-${n}`;

  const pane: PaneNodeSnapshot = {
    kind: "agent_chat",
    pane_id: paneId,
    title: label,
    thread_id: threadId,
    provider: "codex",
    cwd,
  };
  const surface: SurfaceSnapshot = {
    surface_id: surfaceId,
    title: label,
    root: pane,
    active_pane_id: paneId,
  };
  const tab: TabSnapshot = {
    tab_id: tabId,
    kind: "terminal",
    title: label,
    surface_id: surfaceId,
    browser_id: null,
    icon: null,
  };
  return { pane, surface, tab };
}

/** Browser-pane id used by the pre-seeded browser workspace. The mock's
 *  `start_browser_stream` handler points at the dev stream port, so with
 *  a real `agent-browser` daemon running there the pane streams real
 *  frames in plain-browser dev (see `tauri-mock.ts`). Without a daemon
 *  it shows its connecting state and retries — same as a dead daemon. */
export const MOCK_BROWSER_ID = "browser-mock-1";

/** Build a browser surface (+ tab) for a workspace. Mirrors the
 *  backend's `create_browser_pane`: the surface root is a `browser`
 *  pane node carrying the browser id. */
function browserSurface(label: string): {
  pane: PaneNodeSnapshot;
  surface: SurfaceSnapshot;
  tab: TabSnapshot;
} {
  const n = ++paneSeq;
  const paneId = `pane-${n}`;
  const surfaceId = `surface-${n}`;
  const tabId = `tab-${n}`;

  const pane: PaneNodeSnapshot = {
    kind: "browser",
    pane_id: paneId,
    browser_id: MOCK_BROWSER_ID,
    title: label,
  };
  const surface: SurfaceSnapshot = {
    surface_id: surfaceId,
    title: label,
    root: pane,
    active_pane_id: paneId,
  };
  const tab: TabSnapshot = {
    tab_id: tabId,
    kind: "terminal",
    title: label,
    surface_id: surfaceId,
    browser_id: MOCK_BROWSER_ID,
    icon: null,
  };
  return { pane, surface, tab };
}

interface WorkspaceSeed extends Partial<WorkspaceSnapshot> {
  workspace_id: string;
  title: string;
  cwd: string;
  /** Terminal-pane label (defaults to the title). */
  paneLabel?: string;
  /** Agent status to surface on the sidebar dot, keyed onto this
   *  workspace's pane in `pane_statuses`. `idle` => no dot. */
  status?: PaneStatus;
}

const terminalSessions: TerminalSessionSnapshot[] = [];
const paneStatuses: Record<string, PaneStatus> = {};
/** Runtime-only `codemux monitor start` flags (pane_id → reason). Mirrors
 *  `AppStateSnapshot.manual_monitors`; `tauri-mock.ts` mutates it when the
 *  docked bar's Stop button fires. */
const manualMonitors: Record<string, string | null> = {};

/** Materialize a `WorkspaceSnapshot` from a terse seed, filling in the
 *  many optional/boilerplate fields with sane defaults and wiring the
 *  terminal surface + status-dot plumbing. */
function makeWorkspace(seed: WorkspaceSeed): WorkspaceSnapshot {
  const { pane, surface, tab, session } = terminalSurface(
    seed.paneLabel ?? seed.title,
    seed.cwd,
  );
  terminalSessions.push(session);
  if (seed.status && seed.status !== "idle") {
    paneStatuses[pane.pane_id] = seed.status;
  }

  return {
    workspace_id: seed.workspace_id,
    title: seed.title,
    workspace_type: seed.workspace_type ?? "standard",
    cwd: seed.cwd,
    is_git: seed.is_git ?? true,
    git_branch: seed.git_branch ?? null,
    git_ahead: seed.git_ahead ?? 0,
    git_behind: seed.git_behind ?? 0,
    git_additions: seed.git_additions ?? 0,
    git_deletions: seed.git_deletions ?? 0,
    git_changed_files: seed.git_changed_files ?? 0,
    notification_count: seed.notification_count ?? 0,
    latest_agent_state: seed.latest_agent_state ?? seed.status ?? null,
    worktree_path: seed.worktree_path ?? null,
    project_root: seed.project_root ?? null,
    project_uid: seed.project_uid ?? null,
    workspace_kind: seed.workspace_kind ?? null,
    protected: seed.protected ?? false,
    divergent_copy: seed.divergent_copy ?? false,
    pr_number: seed.pr_number ?? null,
    pr_state: seed.pr_state ?? null,
    pr_url: seed.pr_url ?? null,
    // Absent means GitHub, matching the frontend's back-compat rule, so
    // only the deliberately-GitLab seeds set this.
    provider_kind: seed.provider_kind ?? null,
    linked_issue: seed.linked_issue ?? null,
    notifications_muted: seed.notifications_muted ?? false,
    tabs: [tab],
    active_tab_id: tab.tab_id,
    active_surface_id: surface.surface_id,
    surfaces: [surface],
    host_id: seed.host_id ?? null,
    remote_cwd: seed.remote_cwd ?? null,
    attach_only: seed.attach_only ?? false,
  };
}

// ── Linked issues ───────────────────────────────────────────────────

const ISSUE_AUTH: LinkedIssue = {
  number: 128,
  title: "Refactor auth token refresh to avoid mid-session expiry",
  state: "Closed",
  labels: ["enhancement", "auth"],
};

const ISSUE_MOCK: LinkedIssue = {
  number: 40,
  title: "dev: mock Tauri runtime so the Codemux UI boots in a plain browser",
  state: "Open",
  labels: ["enhancement"],
};

/** Linked to the agent-chat-demo workspace so the Context Row's
 *  linked-issue chip (retained after the old bottom bar's removal) is visible and
 *  clickable under `npm run dev` — the mock's `get_github_issue`
 *  expands this into a full issue for the popover. */
const ISSUE_CHAT: LinkedIssue = {
  number: 146,
  title: "Context Row loses the linked-issue chip on agent-chat surfaces",
  state: "Open",
  labels: ["bug", "regression"],
};

// ── Workspaces ──────────────────────────────────────────────────────
//
// Project 1 — codemux: 1 primary (repo root) + 4 worktrees, covering
// the full PR matrix and all three agent states.

const codemuxRoot = `${PROJECTS}/codemux`;
const codemuxUid = "uid-codemux";

const wsCodemuxMain = makeWorkspace({
  workspace_id: "ws-codemux-main",
  title: "codemux",
  paneLabel: "Claude Code",
  cwd: codemuxRoot,
  project_root: codemuxRoot,
  project_uid: codemuxUid,
  workspace_kind: "main",
  protected: true,
  git_branch: "main",
  status: "working", // amber pulsing dot on the primary row
});

/** Agent-chat workspace: a single `agent_chat` pane bound to
 *  `MOCK_CHAT_THREAD_ID`. The mock hydrates a long transcript for it
 *  so list virtualization is observable in the browser. */
const wsCodemuxChat: WorkspaceSnapshot = (() => {
  const ws = makeWorkspace({
    workspace_id: "ws-codemux-chat",
    title: "agent-chat-demo",
    cwd: codemuxRoot,
    project_root: codemuxRoot,
    project_uid: codemuxUid,
    workspace_kind: "main",
    git_branch: "main",
    // Git counters + PR so the below-composer Context Row (behind chip,
    // PR chip, workspace-details popover) is fully populated in the mock.
    pr_number: 172,
    pr_state: "open",
    pr_url: "https://github.com/example/codemux/pull/172",
    // Linked issue so the Context Row's relocated linked-issue chip
    // (PR #144 dropped it from agent-chat surfaces) is demoable here.
    linked_issue: ISSUE_CHAT,
    git_ahead: 2,
    git_behind: 5,
    git_additions: 9,
    git_deletions: 1,
    git_changed_files: 1,
  });
  const { pane, surface, tab } = chatSurface("Agent Chat", codemuxRoot);
  // Chat sessions publish into pane_statuses just like terminal agents
  // (backend `set_pane_status_by_thread`); seed a green "review" dot so
  // the dev mock demonstrates a finished chat agent in the sidebar.
  paneStatuses[pane.pane_id] = "review";
  return {
    ...ws,
    tabs: [tab],
    active_tab_id: tab.tab_id,
    active_surface_id: surface.surface_id,
    surfaces: [surface],
  };
})();

/** Detached (pane-less) agent browser session for `wsCodemuxChat` — drives the
 *  background-browser chip, terminal-header indicator, and peek overlay:
 *  the agent opened a browser mid-chat but it stayed off to the side
 *  instead of splitting the chat into a pane. `pane_id: null` + `is_active:
 *  true` is exactly the predicate `MessageList`, the terminal-header
 *  browser control, and `BrowserPeekOverlay` all key off of. */
const MOCK_AGENT_BROWSER_SESSION: AgentBrowserSession = {
  session_id: "agent-browser-mock-chat",
  workspace_id: wsCodemuxChat.workspace_id,
  cli_session_name: "devmock",
  stream_url: "ws://127.0.0.1:9777",
  current_url: "http://localhost:1420",
  is_active: true,
  pane_id: null,
  browser_id: null,
  user_dismissed: false,
  right_panel_docked: false,
};

/** CLI-agent counterpart to the chat fixture above. This keeps the visual
 *  regression case for the retired bottom bar directly reachable under
 *  `npm run dev`: the primary Codemux workspace's Claude Code terminal owns
 *  a detached browser surfaced by the compact pane-header control. */
const MOCK_CLI_AGENT_BROWSER_SESSION: AgentBrowserSession = {
  session_id: "agent-browser-mock-cli",
  workspace_id: wsCodemuxMain.workspace_id,
  cli_session_name: "devmock-cli",
  stream_url: "ws://127.0.0.1:9777",
  current_url: "http://localhost:1420",
  is_active: true,
  pane_id: null,
  browser_id: null,
  user_dismissed: false,
  right_panel_docked: false,
};

/** Browser-pane workspace: a single `browser` pane streaming from the
 *  dev stream port — lets browser-pane UI work be exercised end-to-end
 *  in plain-browser dev when a daemon is running (issue-free without). */
const wsCodemuxBrowser: WorkspaceSnapshot = (() => {
  const ws = makeWorkspace({
    workspace_id: "ws-codemux-browser",
    title: "browser-demo",
    cwd: codemuxRoot,
    project_root: codemuxRoot,
    project_uid: codemuxUid,
    workspace_kind: "main",
    git_branch: "main",
  });
  const { surface, tab } = browserSurface("Browser");
  return {
    ...ws,
    tabs: [tab],
    active_tab_id: tab.tab_id,
    active_surface_id: surface.surface_id,
    surfaces: [surface],
  };
})();

const wsCodemuxAuth = makeWorkspace({
  workspace_id: "ws-codemux-auth",
  title: "auth-refactor",
  cwd: `${HOME}/.codemux/worktrees/codemux/feature-auth-refactor`,
  worktree_path: `${HOME}/.codemux/worktrees/codemux/feature-auth-refactor`,
  project_root: codemuxRoot,
  project_uid: codemuxUid,
  workspace_kind: "worktree",
  git_branch: "feature/auth-refactor",
  pr_number: 128,
  pr_state: "merged",
  pr_url: "https://github.com/example/codemux/pull/128",
  linked_issue: ISSUE_AUTH,
  status: "review", // green dot
});

const wsCodemuxSidebar = makeWorkspace({
  workspace_id: "ws-codemux-sidebar",
  title: "sidebar-flicker",
  cwd: `${HOME}/.codemux/worktrees/codemux/fix-sidebar-flicker`,
  worktree_path: `${HOME}/.codemux/worktrees/codemux/fix-sidebar-flicker`,
  project_root: codemuxRoot,
  project_uid: codemuxUid,
  workspace_kind: "worktree",
  git_branch: "fix/sidebar-flicker",
  pr_number: 131,
  pr_state: "closed",
  pr_url: "https://github.com/example/codemux/pull/131",
  notifications_muted: true,
});

const wsCodemuxMock = makeWorkspace({
  workspace_id: "ws-codemux-mock",
  title: "dev-mock-runtime",
  cwd: `${HOME}/.codemux/worktrees/codemux/feature-40-dev-mock`,
  worktree_path: `${HOME}/.codemux/worktrees/codemux/feature-40-dev-mock`,
  project_root: codemuxRoot,
  project_uid: codemuxUid,
  workspace_kind: "worktree",
  git_branch: "feature/40-dev-mock-tauri-runtime",
  pr_number: 140,
  pr_state: "draft",
  pr_url: "https://github.com/example/codemux/pull/140",
  linked_issue: ISSUE_MOCK,
  notification_count: 3,
  git_ahead: 2,
  git_behind: 1,
  git_additions: 214,
  git_deletions: 37,
  git_changed_files: 6,
});

/** Live-streaming chat workspace: its `agent_chat` pane has NO thread
 *  bound, so mounting it walks the fresh-session path — start_session,
 *  per-thread channel attach, then token-by-token `content_delta`
 *  streaming on send (issue #75). Complements `wsCodemuxChat` above,
 *  which exercises the hydrated long-transcript path (issue #77). */
const wsCodemuxChatLive = (() => {
  const cwd = `${HOME}/.codemux/worktrees/codemux/feature-75-chat-channel`;
  const ws = makeWorkspace({
    workspace_id: "ws-codemux-chat-live",
    title: "chat-streaming",
    cwd,
    worktree_path: cwd,
    project_root: codemuxRoot,
    project_uid: codemuxUid,
    workspace_kind: "worktree",
    git_branch: "feature/75-chat-channel",
  });
  const { pane, surface, tab } = chatSurface("Agent Chat", cwd, null);
  // Amber "working" dot: a chat agent mid-turn, mirroring how the backend
  // maps streaming events → PaneStatus::Working into pane_statuses.
  paneStatuses[pane.pane_id] = "working";
  return {
    ...ws,
    tabs: [tab],
    active_tab_id: tab.tab_id,
    active_surface_id: surface.surface_id,
    surfaces: [surface],
  };
})();

/** Thread id of the seeded "Monitoring" demo. */
export const MOCK_MONITORING_THREAD_ID = "thread-mock-monitoring";

/** Reason string the seeded monitoring pane carries, as if an agent had run
 *  `codemux monitor start --reason "…"`. Surfaces in the docked
 *  `MonitoringBar` title and proves the reason plumbing end to end. */
export const MOCK_MONITORING_REASON = "CI checks on PR #482";

/** Monitoring demo: a chat agent that
 *  finished its deliverable and is now babysitting a CI run. Seeds the calm
 *  cyan sidebar badge + the docked "Monitoring in the background" bar with a
 *  working Stop button, so both are screenshot-reachable under `npm run dev`
 *  without needing a live provider that emits watch-loop tasks. */
const wsCodemuxMonitoring = (() => {
  const cwd = `${HOME}/.codemux/worktrees/codemux/demo-monitoring`;
  const ws = makeWorkspace({
    workspace_id: "ws-codemux-monitoring",
    title: "monitoring-demo",
    cwd,
    worktree_path: cwd,
    project_root: codemuxRoot,
    project_uid: codemuxUid,
    workspace_kind: "worktree",
    git_branch: "demo/monitoring",
    pr_number: 482,
    pr_state: "open",
    pr_url: "https://github.com/example/codemux/pull/482",
  });
  const { pane, surface, tab } = chatSurface(
    "Agent Chat",
    cwd,
    MOCK_MONITORING_THREAD_ID,
  );
  // Steady cyan dot. Seeded through BOTH halves of the feature — the
  // effective `pane_statuses` entry the sidebar reads, and the runtime-only
  // `manual_monitors` reason the chat bar reads — because the real backend
  // publishes them together (`apply_manual_monitors`).
  paneStatuses[pane.pane_id] = "monitoring";
  manualMonitors[pane.pane_id] = MOCK_MONITORING_REASON;
  return {
    ...ws,
    tabs: [tab],
    active_tab_id: tab.tab_id,
    active_surface_id: surface.surface_id,
    surfaces: [surface],
  };
})();

const wsCodemuxPorts = makeWorkspace({
  workspace_id: "ws-codemux-ports",
  title: "port-detection",
  cwd: `${HOME}/.codemux/worktrees/codemux/chore-port-detection`,
  worktree_path: `${HOME}/.codemux/worktrees/codemux/chore-port-detection`,
  project_root: codemuxRoot,
  project_uid: codemuxUid,
  workspace_kind: "worktree",
  git_branch: "chore/port-detection",
  pr_number: 142,
  pr_state: "open",
  pr_url: "https://github.com/example/codemux/pull/142",
  git_additions: 58,
  git_deletions: 4,
  git_changed_files: 2,
});

/** Thread ids for the three `/workflow` orchestration demo panes below
 *  (design fixture `.design/workflow-orchestration.dc.html`, "Audit
 *  route auth"). Each gets its OWN workspace + thread so every stage of
 *  the workflow lifecycle — gated on approval, mid-run, and finished —
 *  is directly reachable in the sidebar rather than requiring a live
 *  interaction to walk between states. */
export const MOCK_WORKFLOW_APPROVAL_THREAD_ID = "thread-mock-workflow-approval";
export const MOCK_WORKFLOW_RUNNING_THREAD_ID = "thread-mock-workflow-running";
export const MOCK_WORKFLOW_COMPLETE_THREAD_ID = "thread-mock-workflow-complete";

/** Approval-gated workflow: `/workflow` turn lands, Claude proposes the
 *  script, and the run sits in `pending_approval` waiting on the linked
 *  permission request. */
const wsCodemuxWorkflowApproval = (() => {
  const cwd = `${HOME}/.codemux/worktrees/codemux/demo-workflow-approval`;
  const ws = makeWorkspace({
    workspace_id: "ws-codemux-workflow-approval",
    title: "workflow-approval",
    cwd,
    worktree_path: cwd,
    project_root: codemuxRoot,
    project_uid: codemuxUid,
    workspace_kind: "worktree",
    git_branch: "demo/workflow-approval",
  });
  const { pane, surface, tab } = chatSurface(
    "Agent Chat",
    cwd,
    MOCK_WORKFLOW_APPROVAL_THREAD_ID,
  );
  // Red "permission" dot: the run is gated on the user's decision.
  paneStatuses[pane.pane_id] = "permission";
  return {
    ...ws,
    tabs: [tab],
    active_tab_id: tab.tab_id,
    active_surface_id: surface.surface_id,
    surfaces: [surface],
  };
})();

/** Running workflow: approved and mid-phase-2, with a mix of done /
 *  running / queued subagents auditing route files. */
const wsCodemuxWorkflowRunning = (() => {
  const cwd = `${HOME}/.codemux/worktrees/codemux/demo-workflow-running`;
  const ws = makeWorkspace({
    workspace_id: "ws-codemux-workflow-running",
    title: "workflow-running",
    cwd,
    worktree_path: cwd,
    project_root: codemuxRoot,
    project_uid: codemuxUid,
    workspace_kind: "worktree",
    git_branch: "demo/workflow-running",
  });
  const { pane, surface, tab } = chatSurface(
    "Agent Chat",
    cwd,
    MOCK_WORKFLOW_RUNNING_THREAD_ID,
  );
  // Amber "working" dot: the workflow is actively spawning/running agents.
  paneStatuses[pane.pane_id] = "working";
  return {
    ...ws,
    tabs: [tab],
    active_tab_id: tab.tab_id,
    active_surface_id: surface.surface_id,
    surfaces: [surface],
  };
})();

/** Completed workflow: the full run finished with a findings summary and
 *  a trailing assistant report message. */
const wsCodemuxWorkflowComplete = (() => {
  const cwd = `${HOME}/.codemux/worktrees/codemux/demo-workflow-complete`;
  const ws = makeWorkspace({
    workspace_id: "ws-codemux-workflow-complete",
    title: "workflow-complete",
    cwd,
    worktree_path: cwd,
    project_root: codemuxRoot,
    project_uid: codemuxUid,
    workspace_kind: "worktree",
    git_branch: "demo/workflow-complete",
  });
  const { pane, surface, tab } = chatSurface(
    "Agent Chat",
    cwd,
    MOCK_WORKFLOW_COMPLETE_THREAD_ID,
  );
  // Green "review" dot: the run finished and is ready to look at.
  paneStatuses[pane.pane_id] = "review";
  return {
    ...ws,
    tabs: [tab],
    active_tab_id: tab.tab_id,
    active_surface_id: surface.surface_id,
    surfaces: [surface],
  };
})();

// Project 2 — vexis: 1 primary + 3 worktrees (one in `permission`).
//
// Seeded as a GitLab-hosted project so the provider-aware surfaces are
// exercisable side by side with the GitHub projects above: the sidebar
// chips read `!N` instead of `#N`, tooltips say "merge request", and the
// review panel's copy names `glab`. Everything else in the fixture set
// stays GitHub, which is what keeps the visual diff honest.

const vexisRoot = `${PROJECTS}/vexis`;
const vexisUid = "uid-vexis";
const vexisWebRoot = "https://gitlab.example.com/acme/vexis";

const wsVexisMain = makeWorkspace({
  workspace_id: "ws-vexis-main",
  title: "vexis",
  cwd: vexisRoot,
  project_root: vexisRoot,
  project_uid: vexisUid,
  workspace_kind: "main",
  protected: true,
  git_branch: "main",
  provider_kind: "gitlab",
});

const wsVexisCuda = makeWorkspace({
  workspace_id: "ws-vexis-cuda",
  title: "cuda-variant",
  cwd: `${HOME}/.codemux/worktrees/vexis/feat-cuda-variant`,
  worktree_path: `${HOME}/.codemux/worktrees/vexis/feat-cuda-variant`,
  project_root: vexisRoot,
  project_uid: vexisUid,
  workspace_kind: "worktree",
  git_branch: "feat/cuda-variant",
  pr_number: 88,
  pr_state: "open",
  pr_url: `${vexisWebRoot}/-/merge_requests/88`,
  provider_kind: "gitlab",
  notification_count: 1,
  status: "permission", // red pulsing dot
});

const wsVexisBench = makeWorkspace({
  workspace_id: "ws-vexis-bench",
  title: "bench-suite",
  cwd: `${HOME}/.codemux/worktrees/vexis/perf-bench-suite`,
  worktree_path: `${HOME}/.codemux/worktrees/vexis/perf-bench-suite`,
  project_root: vexisRoot,
  project_uid: vexisUid,
  workspace_kind: "worktree",
  git_branch: "perf/bench-suite",
  pr_number: 90,
  pr_state: "draft",
  pr_url: `${vexisWebRoot}/-/merge_requests/90`,
  provider_kind: "gitlab",
  git_additions: 132,
  git_deletions: 9,
  git_changed_files: 4,
});

const wsVexisInstaller = makeWorkspace({
  workspace_id: "ws-vexis-installer",
  title: "installer-detect",
  cwd: `${HOME}/.codemux/worktrees/vexis/fix-installer-detect`,
  worktree_path: `${HOME}/.codemux/worktrees/vexis/fix-installer-detect`,
  project_root: vexisRoot,
  project_uid: vexisUid,
  workspace_kind: "worktree",
  git_branch: "fix/installer-detect",
  provider_kind: "gitlab",
});

// Project 3 — personal-site: 1 primary + 1 worktree (open PR + diff).

// ── Bitbucket: a host that declares nothing ─────────────────────────
//
// Seeded so the undeclared-host state can be walked end to end rather
// than only reasoned about. The workspace has a real pull request with
// real data behind it — that is the point: Codemux can *see* it, and
// still draws no write control anywhere, because the adapter declares
// no operations and the backend would refuse them.
const ledgerRoot = `${PROJECTS}/ledger-api`;
const ledgerUid = "uid-ledger-api";

const wsLedgerMain = makeWorkspace({
  workspace_id: "ws-ledger-main",
  title: "ledger-api",
  cwd: ledgerRoot,
  project_root: ledgerRoot,
  project_uid: ledgerUid,
  workspace_kind: "main",
  protected: true,
  git_branch: "main",
  provider_kind: "bitbucket",
});

const wsLedgerRates = makeWorkspace({
  workspace_id: "ws-ledger-rates",
  title: "fx-rates-cache",
  cwd: `${HOME}/.codemux/worktrees/ledger-api/feat-fx-rates-cache`,
  worktree_path: `${HOME}/.codemux/worktrees/ledger-api/feat-fx-rates-cache`,
  project_root: ledgerRoot,
  project_uid: ledgerUid,
  workspace_kind: "worktree",
  git_branch: "feat/fx-rates-cache",
  pr_number: 64,
  pr_state: "open",
  pr_url: "https://bitbucket.org/acme/ledger-api/pull-requests/64",
  provider_kind: "bitbucket",
  git_ahead: 3,
  git_additions: 128,
  git_deletions: 19,
  git_changed_files: 4,
});

const siteRoot = `${PROJECTS}/personal-site`;
const siteUid = "uid-personal-site";

const wsSiteMain = makeWorkspace({
  workspace_id: "ws-site-main",
  title: "personal-site",
  cwd: siteRoot,
  project_root: siteRoot,
  project_uid: siteUid,
  workspace_kind: "main",
  protected: true,
  git_branch: "main",
});

const wsSiteRedesign = makeWorkspace({
  workspace_id: "ws-site-redesign",
  title: "landing-refresh",
  cwd: `${HOME}/.codemux/worktrees/personal-site/design-landing-refresh`,
  worktree_path: `${HOME}/.codemux/worktrees/personal-site/design-landing-refresh`,
  project_root: siteRoot,
  project_uid: siteUid,
  workspace_kind: "worktree",
  git_branch: "design/landing-refresh",
  pr_number: 12,
  pr_state: "open",
  pr_url: "https://github.com/example/personal-site/pull/12",
  git_ahead: 5,
  git_additions: 401,
  git_deletions: 220,
  git_changed_files: 11,
});

// Project 4 — scratchpad: a plain non-git folder opened as a project.
// Exercises the non-git degradation surfaces: no branch/PR chips, the
// "no git" state + "Initialize Git" affordance in the Context Row and
// Changes panel, and the hidden worktree option in Thread Scope.

const scratchRoot = `${PROJECTS}/scratchpad`;

const wsScratchpad = makeWorkspace({
  workspace_id: "ws-scratchpad-main",
  title: "scratchpad",
  cwd: scratchRoot,
  project_root: scratchRoot,
  project_uid: "uid-scratchpad",
  workspace_kind: "main",
  is_git: false,
});

const ALL_WORKSPACES: WorkspaceSnapshot[] = [
  wsCodemuxMain,
  wsCodemuxChat,
  wsCodemuxBrowser,
  wsCodemuxAuth,
  wsCodemuxSidebar,
  wsCodemuxMock,
  wsCodemuxChatLive,
  wsCodemuxMonitoring,
  wsCodemuxPorts,
  wsCodemuxWorkflowApproval,
  wsCodemuxWorkflowRunning,
  wsCodemuxWorkflowComplete,
  wsVexisMain,
  wsVexisCuda,
  wsVexisBench,
  wsVexisInstaller,
  wsSiteMain,
  wsSiteRedesign,
  wsLedgerMain,
  wsLedgerRates,
  wsScratchpad,
];

// ── Archived workspaces ─────────────────────────────────────────────
//
// Three seeded entries across two projects so Settings → Archive renders
// every state end to end:
//   - a codemux worktree archived 2 days ago whose worktree is DIRTY —
//     the first delete attempt with forceDelete=false rejects with a
//     /use force/i message so the escalation flow is visually testable
//     (see `delete_archived_workspace` in tauri-mock.ts);
//   - an old-blog worktree archived 45 days ago (exercises the muted
//     "stale" hint chip for entries older than 30 days);
//   - the old-blog repo root (protected) — offers "Remove from archive"
//     only, never worktree deletion.

const nowSeconds = Math.floor(Date.now() / 1000);
const blogRoot = `${PROJECTS}/old-blog`;

/** Archive id of the seeded dirty-worktree entry — the mock's
 *  `delete_archived_workspace` rejects the first non-forced delete for
 *  this entry so the "Force delete" escalation is exercisable. */
export const MOCK_DIRTY_ARCHIVE_ID = "arch-codemux-payments";

const ARCHIVED_WORKSPACES: ArchivedWorkspaceSnapshot[] = [
  {
    archive_id: MOCK_DIRTY_ARCHIVE_ID,
    workspace_id: "ws-codemux-payments",
    title: "payments-flow",
    cwd: `${HOME}/.codemux/worktrees/codemux/feature-payments-flow`,
    worktree_path: `${HOME}/.codemux/worktrees/codemux/feature-payments-flow`,
    project_root: codemuxRoot,
    project_uid: codemuxUid,
    workspace_kind: "worktree",
    git_branch: "feature/payments-flow",
    protected: false,
    is_git: true,
    archived_at: nowSeconds - 2 * 86_400,
  },
  {
    archive_id: "arch-blog-drafts",
    workspace_id: "ws-blog-drafts",
    title: "draft-cleanup",
    cwd: `${HOME}/.codemux/worktrees/old-blog/chore-draft-cleanup`,
    worktree_path: `${HOME}/.codemux/worktrees/old-blog/chore-draft-cleanup`,
    project_root: blogRoot,
    project_uid: "uid-old-blog",
    workspace_kind: "worktree",
    git_branch: "chore/draft-cleanup",
    protected: false,
    is_git: true,
    archived_at: nowSeconds - 45 * 86_400,
  },
  {
    archive_id: "arch-blog-root",
    workspace_id: "ws-blog-main",
    title: "old-blog",
    cwd: blogRoot,
    worktree_path: null,
    project_root: blogRoot,
    project_uid: "uid-old-blog",
    workspace_kind: "main",
    git_branch: "main",
    protected: true,
    is_git: true,
    archived_at: nowSeconds - 45 * 86_400,
  },
];

// ── Config + persistence ────────────────────────────────────────────

const MOCK_CONFIG: CodemuxConfigSnapshot = {
  config_version: 1,
  default_shell: "/bin/bash",
  theme_source: "system",
  linux_first: true,
  notification_sound_enabled: true,
  ai_commit_message_enabled: true,
  ai_commit_message_cli: null,
  ai_commit_message_model: null,
  ai_resolver_enabled: false,
  ai_resolver_cli: null,
  ai_resolver_model: null,
  ai_resolver_strategy: "smart_merge",
};

const MOCK_PERSISTENCE: PersistenceSchema = {
  schema_version: 1,
  stores_layout_metadata: true,
  stores_terminal_metadata: true,
  stores_live_process_state: false,
};

/** The home directory string the mock `get_home_dir` returns. Used by
 *  the sidebar's "Home" project grouping rule. */
export const MOCK_HOME_DIR = HOME;

// ── Fake filesystem for the web-remote path browser ─────────────────
//
// Stage 3b's remote path picker walks the host over `list_directory`.
// In plain-browser dev there is no host FS, so we serve a small, static
// tree here — just enough to exercise navigation (breadcrumbs, enter
// dir, up, hidden toggle, file selection) end-to-end. Set
// `window.__CODEMUX_REMOTE__ = true` in the console, then trigger any
// pick-folder / pick-files flow to see it.

function mockDir(parent: string, name: string): FileEntry {
  return {
    name,
    path: `${parent}/${name}`,
    is_dir: true,
    size: null,
    is_gitignored: false,
  };
}

function mockFile(
  parent: string,
  name: string,
  size = 2048,
  isGitignored = false,
): FileEntry {
  return {
    name,
    path: `${parent}/${name}`,
    is_dir: false,
    size,
    is_gitignored: isGitignored,
  };
}

/** Absolute path → its directory entries, matching the real
 *  `list_directory` command's `FileEntry[]` shape. */
const MOCK_DIR_TREE: Record<string, FileEntry[]> = {
  "/": [mockDir("", "home")],
  "/home": [mockDir("/home", "dev")],
  [HOME]: [
    mockDir(HOME, "projects"),
    mockDir(HOME, "Documents"),
    mockDir(HOME, "Downloads"),
    mockDir(HOME, ".config"), // hidden — only shown with show_hidden
    mockFile(HOME, "README.md", 1200),
    mockFile(HOME, "notes.txt", 480),
  ],
  [`${HOME}/projects`]: [
    mockDir(`${HOME}/projects`, "codemux"),
    mockDir(`${HOME}/projects`, "web-app"),
    mockDir(`${HOME}/projects`, "scripts"),
  ],
  [`${HOME}/projects/codemux`]: [
    mockDir(`${HOME}/projects/codemux`, "src"),
    mockDir(`${HOME}/projects/codemux`, "docs"),
    mockFile(`${HOME}/projects/codemux`, "Cargo.toml", 890),
    mockFile(`${HOME}/projects/codemux`, "package.json", 1340),
    mockFile(`${HOME}/projects/codemux`, ".env", 64, true), // hidden + gitignored
  ],
  [`${HOME}/projects/codemux/src`]: [
    mockDir(`${HOME}/projects/codemux/src`, "components"),
    mockFile(`${HOME}/projects/codemux/src`, "main.tsx", 512),
    mockFile(`${HOME}/projects/codemux/src`, "App.tsx", 1024),
  ],
  [`${HOME}/projects/codemux/src/components`]: [
    mockFile(`${HOME}/projects/codemux/src/components`, "button.tsx", 640),
    mockFile(`${HOME}/projects/codemux/src/components`, "dialog.tsx", 720),
  ],
  [`${HOME}/projects/codemux/docs`]: [
    mockFile(`${HOME}/projects/codemux/docs`, "INDEX.md", 300),
    // No real filesystem backs browser dev, so opening this always 404s —
    // which is exactly what makes the ImageViewer failure card reachable.
    mockFile(`${HOME}/projects/codemux/docs`, "dashboard.png", 48211),
  ],
  [`${HOME}/projects/web-app`]: [
    mockDir(`${HOME}/projects/web-app`, "src"),
    mockFile(`${HOME}/projects/web-app`, "index.html", 420),
    mockFile(`${HOME}/projects/web-app`, "package.json", 980),
  ],
  [`${HOME}/projects/web-app/src`]: [
    mockFile(`${HOME}/projects/web-app/src`, "index.ts", 256),
  ],
  [`${HOME}/projects/scripts`]: [
    mockFile(`${HOME}/projects/scripts`, "deploy.sh", 210),
    mockFile(`${HOME}/projects/scripts`, "backup.sh", 180),
  ],
  [`${HOME}/Documents`]: [
    mockFile(`${HOME}/Documents`, "resume.pdf", 90000),
    mockFile(`${HOME}/Documents`, "todo.md", 320),
  ],
  [`${HOME}/Downloads`]: [],
  [`${HOME}/.config`]: [mockDir(`${HOME}/.config`, "codemux")],
  [`${HOME}/.config/codemux`]: [
    mockFile(`${HOME}/.config/codemux`, "config.toml", 410),
  ],
};

const WORKTREES = `${HOME}/.codemux/worktrees`;

/**
 * A worktree checkout is a copy of its project, so the fake FS serves the
 * project's own shape under every `~/.codemux/worktrees/<project>/<slug>`
 * root (and below it).
 *
 * Without this, `list_directory` threw for all but the three project-root
 * workspaces, and the right panel's Files pane rendered "No files" for
 * most of the seed — which made the file tree the one deck pane you could
 * not actually look at in browser dev.
 */
function worktreeListing(path: string): FileEntry[] | null {
  if (!path.startsWith(`${WORKTREES}/`)) return null;
  const [project, slug, ...deeper] = path.slice(WORKTREES.length + 1).split("/");
  if (!project || !slug) return null;
  const entries = MOCK_DIR_TREE[[`${PROJECTS}/${project}`, ...deeper].join("/")];
  if (!entries) return null;
  // Re-root each child so expanding a directory asks for a path that
  // resolves back through this same rewrite.
  return entries.map((entry) => ({ ...entry, path: `${path}/${entry.name}` }));
}

/** Mock twin of the `list_directory` command. Mirrors the backend's
 *  contract: rejects on an unknown path (so the picker's manual-path
 *  validation is exercisable) and hides dot-entries unless
 *  `show_hidden`. */
export function mockListDirectory(
  path: string,
  showHidden: boolean,
): FileEntry[] {
  const entries = MOCK_DIR_TREE[path] ?? worktreeListing(path);
  if (!entries) {
    throw new Error(`Not a directory: ${path}`);
  }
  return showHidden
    ? entries
    : entries.filter((e) => !e.name.startsWith("."));
}

const MOCK_MARKDOWN = `# Pane deck

The right panel is a deck of openable panes rather than a fixed set of
tabs. Panes are declared in \`pane-registry.ts\`, opened from the \`+\`
menu, and persisted per workspace.

## Rows

Every pane sits under the same two rows:

\`\`\`text
tab strip   38px   closable icon tabs + the "+" menu
pane bar    32px   breadcrumb on the left, pane actions on the right
status foot 26px   active pane's status + running token total
\`\`\`

## Notes

- Pane controls live in the shared bar, never in a per-pane header.
- Badges are plain counts; the working affordance is the orb.
- Colour comes from tokens only — nothing hardcodes a palette value.
`;

/** Mock twin of the `read_file` command. Markdown paths get real prose so
 *  the rendered/raw toggle has something to switch between; anything else
 *  gets a short plain-text stand-in. */
export function mockReadFile(path: string): string {
  if (/\.(md|mdx|markdown)$/i.test(path)) return MOCK_MARKDOWN;
  if (path.endsWith(".json")) {
    return `{\n  "name": "codemux",\n  "private": true,\n  "mock": true\n}\n`;
  }
  return `// ${path}\n// Mock contents — no filesystem in browser dev.\n`;
}

/**
 * Runtime-event envelopes for the final seeded chat turn so the redesigned
 * transcript surfaces every new presentation in browser dev: a reasoning
 * block (streamed thinking → sealed), a tool group card (a run of reads),
 * a diff card (an Edit with old/new text) and a task summary card (a
 * TodoWrite with a todos array). Consumed by tauri-mock's
 * `mockChatTranscript`; shapes mirror `ProviderRuntimeEvent`.
 */
/**
 * A tiny banded PNG (base64) used as a demo "screenshot" so the mock
 * transcript exercises the agent → user image render path (a `Read` on a
 * PNG returns an Anthropic image block).
 */
const DEMO_SCREENSHOT_PNG_B64 =
  "iVBORw0KGgoAAAANSUhEUgAAAMgAAAB4CAYAAAC3kr3rAAABJUlEQVR42u3ToRGAQAwAwS8LQRFoinidctBUEk1DoYHoqBXbwM3cOiIL6C0RwCBgEDAIGAQMAgYBg4BBwCCAQcAgYBAwCBgEDAIGAYOAQcAgQoBBwCBgEBgb5HqjgJ5BwCBgEDAIGAQMAgYBg4BBwCCAQcAgYBAwCBgEDAIGAYOAQcAgIoBBwCBgEJgb5HvOAnoGAYOAQcAgYBAwCBgEDAIGAYMABgGDgEHAIGAQMAgYBAwCBgGDiAAGAYOAQWBukLx3AT2DgEHAIGAQMAgYBAwCBgGDgEEAg4BBwCBgEDAIGAQMAgYBg4BBRACDgEHAIDA3SOwsoGcQMAgYBAwCBgGDgEHAIGAQMAhgEDAIGAQMAgYBg4BBwCBgEDCICGAQMAgYBMb8mqVc0Zmz1dEAAAAASUVORK5CYII=";

export function richChatTurnEnvelopes(
  threadId: string,
  turnId: string,
): unknown[] {
  const evt = (rest: Record<string, unknown>) => ({
    thread_id: threadId,
    turn_id: turnId,
    ...rest,
  });
  const read = (n: number, file: string): unknown[] => {
    const id = `${turnId}-read-${n}`;
    return [
      evt({
        type: "item_completed",
        item: { kind: "tool_use", tool_name: "Read", tool_use_id: id, input: { file_path: file } },
      }),
      evt({
        type: "item_completed",
        item: { kind: "tool_result", tool_use_id: id, content: `// ${file}\n`, is_error: false },
      }),
    ];
  };
  const readScreenshot = (n: number, file: string): unknown[] => {
    const id = `${turnId}-read-${n}`;
    return [
      evt({
        type: "item_completed",
        item: { kind: "tool_use", tool_name: "Read", tool_use_id: id, input: { file_path: file } },
      }),
      evt({
        type: "item_completed",
        item: {
          kind: "tool_result",
          tool_use_id: id,
          content: [
            {
              type: "image",
              source: {
                type: "base64",
                media_type: "image/png",
                data: DEMO_SCREENSHOT_PNG_B64,
              },
            },
          ],
          is_error: false,
        },
      }),
    ];
  };
  const editId = `${turnId}-edit`;
  const todoId = `${turnId}-todo`;
  const thinking =
    "The issue cites gateway/run.py:225, but line numbers rarely survive across versions. " +
    "I'll grep for the actual symbol and compare against origin/main before editing anything.";

  return [
    // Reasoning: two streamed thinking deltas then a sealed completion.
    evt({ type: "content_delta", delta: { kind: "thinking", text: thinking.slice(0, 90) } }),
    evt({ type: "content_delta", delta: { kind: "thinking", text: thinking.slice(90) } }),
    evt({ type: "item_completed", item: { kind: "assistant_thinking", text: thinking } }),
    evt({
      type: "item_completed",
      item: {
        kind: "assistant_text",
        text: "I'll locate the symbol upstream, then apply the fix where the bug actually lives.",
      },
    }),
    // Tool group card: a run of reads.
    ...read(0, "gateway/run.py"),
    ...read(1, "gateway/errors.py"),
    ...read(2, "tests/test_traceback.py"),
    // Diff card: an Edit carrying old_string / new_string.
    evt({
      type: "item_completed",
      item: {
        kind: "tool_use",
        tool_name: "Edit",
        tool_use_id: editId,
        input: {
          file_path: "gateway/run.py",
          old_string: "        cur = cur.__cause__ or cur.__context__",
          new_string:
            '        cur = getattr(cur, "__cause__", None) or getattr(cur, "__context__", None)',
        },
      },
    }),
    evt({
      type: "item_completed",
      item: { kind: "tool_result", tool_use_id: editId, content: "Applied edit to gateway/run.py", is_error: false },
    }),
    // Durable provider-normalized task snapshot: drives the conditional
    // composer toggle and the right-side Tasks panel.
    {
      type: "tasks_updated",
      thread_id: threadId,
      tasks: {
        explanation: "Fix the traceback regression and prepare it for review.",
        tasks: [
          { task_id: "branch", title: "Branch fix/57298 off origin/main", status: "completed", blocked_by: [] },
          { task_id: "patch", title: "Patch the traceback chain walker", status: "completed", blocked_by: [] },
          { task_id: "test", title: "Add a regression test", status: "completed", blocked_by: [] },
          {
            task_id: "pr",
            title: "Open a PR against origin/main",
            status: "in_progress",
            detail: "Preparing the change summary and visual evidence",
            blocked_by: [],
          },
          { task_id: "review", title: "Address review feedback", status: "pending", blocked_by: ["pr"] },
        ],
      },
    },
    // Task summary card: a TodoWrite with a todos array.
    evt({
      type: "item_completed",
      item: {
        kind: "tool_use",
        tool_name: "TodoWrite",
        tool_use_id: todoId,
        input: {
          todos: [
            { content: "Branch fix/57298 off origin/main", status: "completed" },
            { content: "Patch the traceback chain walker", status: "completed" },
            { content: "Add a regression test", status: "completed" },
            { content: "Open a PR against origin/main", status: "in_progress" },
          ],
        },
      },
    }),
    evt({
      type: "item_completed",
      item: { kind: "tool_result", tool_use_id: todoId, content: "Todos updated", is_error: false },
    }),
    // Image tool result: `Read` on a PNG returns an Anthropic image block
    // (the shape a screenshot arrives as). Stands alone so it renders as
    // its own auto-expanded tool card — an inline thumbnail + click-to-open
    // lightbox, not a base64 text dump.
    evt({
      type: "item_completed",
      item: {
        kind: "assistant_text",
        text: "Here's the dashboard before the fix — captured for the PR description:",
      },
    }),
    ...readScreenshot(3, "/tmp/codemux-dashboard-before.png"),
    evt({
      type: "item_completed",
      item: {
        kind: "assistant_text",
        text:
          "Done — [issue #57298](https://github.com/example/gateway/issues/57298) " +
          "implemented in [`gateway/run.py:225`](gateway/run.py:225), with " +
          "coverage in [`tests/test_traceback.py:88`](tests/test_traceback.py:88). " +
          "The regression test passes, and the " +
          "[exception-chain notes](https://docs.python.org/3/library/exceptions.html) are ready.\n\n" +
          "[Dashboard screenshot](/tmp/codemux-dashboard-proof.png)",
      },
    }),
  ];
}

/**
 * Runtime-event envelopes for a seeded subagents turn: the orchestrator
 * delegates to two subagents — "Implement" (completed) and "Verify" (still
 * running). Replayed through the real reducer by `mockChatTranscript`, so
 * the orchestration card, the running pill, and the drill-in all render
 * from real `SubagentRunItem` state on open. Shapes mirror the Stage 1
 * `subagent_updated` event + `subagent_id`-tagged `item_completed`.
 */
export function subagentTurnEnvelopes(
  threadId: string,
  turnId: string,
): unknown[] {
  const snap = (subagent: Record<string, unknown>) => ({
    type: "subagent_updated",
    thread_id: threadId,
    subagent,
  });
  const sub = (subagentId: string, rest: Record<string, unknown>) => ({
    thread_id: threadId,
    turn_id: turnId,
    subagent_id: subagentId,
    ...rest,
  });
  const tool = (
    subagentId: string,
    id: string,
    toolName: string,
    input: Record<string, unknown>,
    result: string | null,
  ): unknown[] => {
    const out: unknown[] = [
      sub(subagentId, {
        type: "item_completed",
        item: { kind: "tool_use", tool_name: toolName, tool_use_id: id, input },
      }),
    ];
    // A null result leaves the tool "running" (design shows Verify's
    // `npm run verify` still in flight).
    if (result !== null) {
      out.push(
        sub(subagentId, {
          type: "item_completed",
          item: { kind: "tool_result", tool_use_id: id, content: result, is_error: false },
        }),
      );
    }
    return out;
  };

  return [
    // Orchestrator lead-in (parent flow).
    {
      thread_id: threadId,
      turn_id: turnId,
      type: "item_completed",
      item: {
        kind: "assistant_text",
        text: "Root cause confirmed. I'll run this as a two-stage workflow and delegate to subagents so the work and the review happen independently.",
      },
    },
    // Spawn "Implement" and stream its sub-transcript.
    snap({
      subagent_id: "impl",
      name: "Implement",
      agent_type: "implement",
      description: "Implement the paste fallback",
      model: "opus · xhigh",
      status: "running",
    }),
    sub("impl", {
      type: "item_completed",
      item: {
        kind: "assistant_text",
        text: "Adding the Rust clipboard fallback to the composer paste handler. Keeping the fast clipboardData path and falling back through the existing handleAttachImage so mime validation still applies.",
      },
    }),
    ...tool("impl", "impl-edit-1", "Edit", {
      file_path: "src/components/chat/Composer.tsx",
      old_string: "const x = 1;",
      new_string: "const x = 1;\nconst y = 2;",
    }, "Applied edit to src/components/chat/Composer.tsx"),
    ...tool("impl", "impl-edit-2", "Edit", {
      file_path: "src-tauri/src/clipboard.rs",
      old_string: "",
      new_string: "pub fn read_image() {}\n",
    }, "Applied edit to src-tauri/src/clipboard.rs"),
    ...tool("impl", "impl-run-1", "Bash", {
      command: "cargo test clipboard_fallback",
    }, "test result: ok. 1 passed"),
    sub("impl", {
      type: "item_completed",
      item: {
        kind: "assistant_text",
        text: "Done — reported 6 changed files back to the orchestrator. npm run verify and cargo test both pass.",
      },
    }),
    // Spawn "Verify" and stream its (still-running) sub-transcript.
    snap({
      subagent_id: "verify",
      name: "Verify",
      agent_type: "verify",
      description: "Verify the paste fallback",
      model: "opus · xhigh",
      status: "running",
      activity: "reading diff for async preventDefault timing regressions…",
    }),
    sub("verify", {
      type: "item_completed",
      item: {
        kind: "assistant_text",
        text: "Reviewing the diff produced by the Implement subagent. Hunting specifically for paste regressions, double-attach, and async timing bugs.",
      },
    }),
    ...tool("verify", "verify-read-1", "Read", {
      file_path: "src/components/chat/Composer.tsx:1420-1470",
    }, "// Composer.tsx\n"),
    ...tool("verify", "verify-grep-1", "Grep", {
      pattern: "preventDefault",
      path: "src/components/chat",
    }, "4 matches"),
    // Left running (no tool_result) — matches the design's "running".
    ...tool("verify", "verify-run-1", "Bash", {
      command: "npm run verify",
    }, null),
    // Implement finishes with final usage; Verify keeps working.
    snap({
      subagent_id: "impl",
      status: "completed",
      result_text: "Done · 6 files changed, npm run verify + cargo test passed",
      tool_use_count: 28,
      duration_ms: 161000,
    }),
    snap({
      subagent_id: "verify",
      status: "running",
      activity: "checking async preventDefault timing on the paste handler…",
      tool_use_count: 11,
      duration_ms: 52000,
    }),
  ];
}

// ── /workflow orchestration demo (Claude dynamic-workflow feature) ────
//
// Mirrors the design fixture `.design/workflow-orchestration.dc.html`
// ("Audit route auth"): a `/workflow` turn that audits every route
// handler under `src/routes/` for missing auth checks. The three
// lifecycle stages — gated on approval, mid-run, and finished — are
// seeded as three SEPARATE threads (see `wsCodemuxWorkflowApproval` /
// `Running` / `Complete` above) so each is directly reachable rather
// than requiring a live interaction to advance between states.

const WORKFLOW_NAME = "Audit route auth";
const WORKFLOW_DESCRIPTION =
  "Audit every route handler under src/routes/ for missing auth checks, and adversarially verify each finding.";
/** The `/workflow` slash-command turn, verbatim from the design's
 *  conversation bubble. */
const WORKFLOW_USER_TEXT = `/workflow\n${WORKFLOW_DESCRIPTION}`;

/** Planned phases parsed from the script's `meta.phases` — titles are
 *  the routing key subagents attribute to via `SubagentSnapshot.phase`,
 *  so they must match exactly what the phase-tagged snapshots below use.
 *  `detail` carries the design's "~N agents" estimate. */
const WORKFLOW_PLANNED_PHASES = [
  { title: "Discover route files", detail: "~1 agent" },
  { title: "Audit each file for missing auth", detail: "~42 agents" },
  { title: "Adversarially verify findings", detail: "~1 per finding" },
];

const WORKFLOW_PHASE_DISCOVER = WORKFLOW_PLANNED_PHASES[0].title;
const WORKFLOW_PHASE_AUDIT = WORKFLOW_PLANNED_PHASES[1].title;
const WORKFLOW_PHASE_VERIFY = WORKFLOW_PLANNED_PHASES[2].title;

/** Small, realistic workflow script — the "View script" affordance on
 *  the approval card reads this back. */
const WORKFLOW_SCRIPT = `export const meta = {
  name: "Audit route auth",
  description:
    "Audit every route handler under src/routes/ for missing auth checks, and adversarially verify each finding.",
  phases: [
    { title: "Discover route files", detail: "~1 agent" },
    { title: "Audit each file for missing auth", detail: "~42 agents" },
    { title: "Adversarially verify findings", detail: "~1 per finding" },
  ],
};

export async function run({ glob, agent, phase }) {
  const files = await phase("Discover route files", () =>
    glob("src/routes/**/*.ts"),
  );

  const findings = await phase("Audit each file for missing auth", () =>
    Promise.all(
      files.map((file) =>
        agent({
          prompt:
            "Audit " + file + " for endpoints missing the requireAuth " +
            "middleware. Report each unprotected route with its method and path.",
        }),
      ),
    ),
  );

  await phase("Adversarially verify findings", () =>
    Promise.all(
      findings
        .filter((f) => f.issues.length > 0)
        .map((f) =>
          agent({ prompt: "Independently confirm: " + JSON.stringify(f) }),
        ),
    ),
  );
}
`;

/** JSON \`resultText\` shapes: an empty \`issues\` array parses to the
 *  green "clean" badge, a non-empty one to the red "N issues" badge —
 *  see \`subagentFindingBadge\` in \`lib/agent-chat/workflows.ts\`. */
function auditClean(file: string): string {
  return JSON.stringify({ file, issues: [] });
}
function auditIssues(
  file: string,
  issues: Array<{ method: string; path: string; note?: string }>,
): string {
  return JSON.stringify({ file, issues });
}

/**
 * Envelopes for the approval-gated thread: the `/workflow` turn lands,
 * Claude proposes the script (`workflow_updated` in `pending_approval`
 * with the planned phases + script), and a linked `request_opened`
 * (`tool_use_id` == `workflow_id`) gates the run — exactly the "Run as a
 * workflow?" approval card in the design.
 */
export function workflowApprovalEnvelopes(threadId: string): unknown[] {
  const workflowId = "wf-audit-route-auth-approval";
  return [
    { type: "user_message", thread_id: threadId, text: WORKFLOW_USER_TEXT },
    {
      type: "workflow_updated",
      thread_id: threadId,
      workflow: {
        workflow_id: workflowId,
        status: "pending_approval",
        name: WORKFLOW_NAME,
        description: WORKFLOW_DESCRIPTION,
        script: WORKFLOW_SCRIPT,
        phases: WORKFLOW_PLANNED_PHASES,
      },
    },
    {
      type: "request_opened",
      thread_id: threadId,
      turn_id: "turn-workflow-approval",
      request_id: "req-workflow-approval",
      // Mirrors the reducer's own workflow-gate tests (reducer.test.ts):
      // the workflow card renders its own approval UI keyed off
      // `tool_use_id`, so the standalone request doesn't need a
      // specialized `request_kind`.
      request_kind: "other",
      payload: { workflow_id: workflowId, name: WORKFLOW_NAME },
      tool_use_id: workflowId,
    },
  ];
}

/**
 * Envelopes for the running thread: approved and mid-phase-2. Phase 1
 * (Discover) is already done; phase 2 (Audit) has 8 subagents in mixed
 * states mirroring the design's agent list exactly (routes/auth.ts done
 * clean, routes/billing.ts done with 2 issues + a short read/grep
 * sub-transcript, routes/users.ts done clean, routes/webhooks.ts done 1
 * issue, routes/orders.ts + routes/reports.ts running, routes/admin.ts +
 * routes/search.ts still queued); phase 3 (Verify) stays planned/pending
 * — no subagents attributed to it yet.
 */
export function workflowRunningEnvelopes(threadId: string): unknown[] {
  const workflowId = "wf-audit-route-auth-running";
  const turnId = "turn-workflow-running";
  const requestId = "req-workflow-running";

  const snap = (subagent: Record<string, unknown>) => ({
    type: "subagent_updated",
    thread_id: threadId,
    subagent,
  });
  const subItem = (subagentId: string, item: Record<string, unknown>) => ({
    type: "item_completed",
    thread_id: threadId,
    turn_id: turnId,
    subagent_id: subagentId,
    item,
  });
  const tool = (
    subagentId: string,
    id: string,
    toolName: string,
    input: Record<string, unknown>,
    result: string,
  ): unknown[] => [
    subItem(subagentId, {
      kind: "tool_use",
      tool_name: toolName,
      tool_use_id: id,
      input,
    }),
    subItem(subagentId, {
      kind: "tool_result",
      tool_use_id: id,
      content: result,
      is_error: false,
    }),
  ];

  return [
    { type: "user_message", thread_id: threadId, text: WORKFLOW_USER_TEXT },
    {
      type: "workflow_updated",
      thread_id: threadId,
      workflow: {
        workflow_id: workflowId,
        status: "pending_approval",
        name: WORKFLOW_NAME,
        description: WORKFLOW_DESCRIPTION,
        script: WORKFLOW_SCRIPT,
        phases: WORKFLOW_PLANNED_PHASES,
      },
    },
    {
      type: "request_opened",
      thread_id: threadId,
      turn_id: turnId,
      request_id: requestId,
      request_kind: "other",
      payload: { workflow_id: workflowId, name: WORKFLOW_NAME },
      tool_use_id: workflowId,
    },
    {
      type: "request_resolved",
      thread_id: threadId,
      request_id: requestId,
      decision: { decision: "allow" },
    },

    // Phase 1 — Discover: spawned then finished small.
    snap({
      subagent_id: "discover",
      name: "Discover",
      agent_type: "explore",
      model: "sonnet",
      status: "running",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_DISCOVER,
    }),
    snap({
      subagent_id: "discover",
      status: "completed",
      workflow_id: workflowId,
      result_text: "Found 42 route handler files under src/routes/.",
      tool_use_count: 3,
      total_tokens: 12_000,
      duration_ms: 9_000,
    }),

    // Phase 2 — Audit: 8 of the ~42 files, mirroring the design's list.
    snap({
      subagent_id: "audit-auth",
      name: "routes/auth.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "running",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
    }),
    snap({
      subagent_id: "audit-auth",
      status: "completed",
      workflow_id: workflowId,
      result_text: auditClean("src/routes/auth.ts"),
      tool_use_count: 4,
      total_tokens: 21_000,
      duration_ms: 26_000,
    }),

    snap({
      subagent_id: "audit-billing",
      name: "routes/billing.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "running",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
    }),
    ...tool(
      "audit-billing",
      "billing-read-1",
      "Read",
      { file_path: "src/routes/billing.ts" },
      "// src/routes/billing.ts — 214 ln\n",
    ),
    ...tool(
      "audit-billing",
      "billing-grep-1",
      "Grep",
      { pattern: "requireAuth", path: "src/routes/billing.ts" },
      "3 hits",
    ),
    ...tool(
      "audit-billing",
      "billing-grep-2",
      "Grep",
      { pattern: "router.post", path: "src/routes/billing.ts" },
      "5 hits",
    ),
    snap({
      subagent_id: "audit-billing",
      status: "completed",
      workflow_id: workflowId,
      result_text: auditIssues("src/routes/billing.ts", [
        { method: "POST", path: "/refund" },
        { method: "POST", path: "/credit" },
      ]),
      tool_use_count: 6,
      total_tokens: 24_000,
      duration_ms: 31_000,
    }),

    snap({
      subagent_id: "audit-users",
      name: "routes/users.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "running",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
    }),
    snap({
      subagent_id: "audit-users",
      status: "completed",
      workflow_id: workflowId,
      result_text: auditClean("src/routes/users.ts"),
      tool_use_count: 4,
      total_tokens: 19_000,
      duration_ms: 24_000,
    }),

    snap({
      subagent_id: "audit-webhooks",
      name: "routes/webhooks.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "running",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
    }),
    snap({
      subagent_id: "audit-webhooks",
      status: "completed",
      workflow_id: workflowId,
      result_text: auditIssues("src/routes/webhooks.ts", [
        { method: "POST", path: "/stripe", note: "no signature check" },
      ]),
      tool_use_count: 5,
      total_tokens: 22_000,
      duration_ms: 28_000,
    }),

    snap({
      subagent_id: "audit-orders",
      name: "routes/orders.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "running",
      activity: "auditing src/routes/orders.ts for requireAuth coverage…",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
    }),
    snap({
      subagent_id: "audit-reports",
      name: "routes/reports.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "running",
      activity: "auditing src/routes/reports.ts for requireAuth coverage…",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
    }),
    snap({
      subagent_id: "audit-admin",
      name: "routes/admin.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "pending",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
    }),
    snap({
      subagent_id: "audit-search",
      name: "routes/search.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "pending",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
    }),
    // Phase 3 (Verify) intentionally has no subagents yet — it renders
    // "pending" purely from the workflow still being `running` with an
    // empty agents list (see `workflowPhaseStatus`).
  ];
}

/**
 * Envelopes for the finished thread: all three phases complete, a
 * roll-up (`44 agents · 3 phases · ~2.9M tokens · 4m 51s`) on the final
 * `workflow_updated`, and a trailing assistant report matching the
 * design's completion copy verbatim.
 */
export function workflowCompleteEnvelopes(threadId: string): unknown[] {
  const workflowId = "wf-audit-route-auth-complete";
  const turnId = "turn-workflow-complete";
  const requestId = "req-workflow-complete";

  const snap = (subagent: Record<string, unknown>) => ({
    type: "subagent_updated",
    thread_id: threadId,
    subagent,
  });

  const REPORT =
    "Audit complete. **3 endpoints** are missing auth checks — each was " +
    "independently confirmed by a second agent:\n\n" +
    "- routes/billing.ts — POST /refund, POST /credit\n" +
    "- routes/webhooks.ts — POST /stripe (no signature check)";

  return [
    { type: "user_message", thread_id: threadId, text: WORKFLOW_USER_TEXT },
    {
      type: "workflow_updated",
      thread_id: threadId,
      workflow: {
        workflow_id: workflowId,
        status: "pending_approval",
        name: WORKFLOW_NAME,
        description: WORKFLOW_DESCRIPTION,
        script: WORKFLOW_SCRIPT,
        phases: WORKFLOW_PLANNED_PHASES,
      },
    },
    {
      type: "request_opened",
      thread_id: threadId,
      turn_id: turnId,
      request_id: requestId,
      request_kind: "other",
      payload: { workflow_id: workflowId, name: WORKFLOW_NAME },
      tool_use_id: workflowId,
    },
    {
      type: "request_resolved",
      thread_id: threadId,
      request_id: requestId,
      decision: { decision: "allow" },
    },

    snap({
      subagent_id: "discover",
      name: "Discover",
      agent_type: "explore",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_DISCOVER,
      result_text: "Found 42 route handler files under src/routes/.",
      tool_use_count: 3,
      total_tokens: 12_000,
      duration_ms: 9_000,
    }),

    snap({
      subagent_id: "audit-auth",
      name: "routes/auth.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
      result_text: auditClean("src/routes/auth.ts"),
      tool_use_count: 4,
      total_tokens: 21_000,
      duration_ms: 26_000,
    }),
    snap({
      subagent_id: "audit-billing",
      name: "routes/billing.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
      result_text: auditIssues("src/routes/billing.ts", [
        { method: "POST", path: "/refund" },
        { method: "POST", path: "/credit" },
      ]),
      tool_use_count: 6,
      total_tokens: 24_000,
      duration_ms: 31_000,
    }),
    snap({
      subagent_id: "audit-users",
      name: "routes/users.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
      result_text: auditClean("src/routes/users.ts"),
      tool_use_count: 4,
      total_tokens: 19_000,
      duration_ms: 24_000,
    }),
    snap({
      subagent_id: "audit-webhooks",
      name: "routes/webhooks.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
      result_text: auditIssues("src/routes/webhooks.ts", [
        { method: "POST", path: "/stripe", note: "no signature check" },
      ]),
      tool_use_count: 5,
      total_tokens: 22_000,
      duration_ms: 28_000,
    }),
    snap({
      subagent_id: "audit-orders",
      name: "routes/orders.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
      result_text: auditClean("src/routes/orders.ts"),
      tool_use_count: 4,
      total_tokens: 20_000,
      duration_ms: 25_000,
    }),
    snap({
      subagent_id: "audit-reports",
      name: "routes/reports.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
      result_text: auditClean("src/routes/reports.ts"),
      tool_use_count: 4,
      total_tokens: 20_000,
      duration_ms: 25_000,
    }),
    snap({
      subagent_id: "audit-admin",
      name: "routes/admin.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
      result_text: auditClean("src/routes/admin.ts"),
      tool_use_count: 4,
      total_tokens: 20_000,
      duration_ms: 25_000,
    }),
    snap({
      subagent_id: "audit-search",
      name: "routes/search.ts",
      agent_type: "audit",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_AUDIT,
      result_text: auditClean("src/routes/search.ts"),
      tool_use_count: 4,
      total_tokens: 20_000,
      duration_ms: 25_000,
    }),

    // Phase 3 — Verify: one verifier per flagged file, confirming the
    // finding independently (design copy: "each was independently
    // confirmed by a second agent").
    snap({
      subagent_id: "verify-billing",
      name: "Verify routes/billing.ts",
      agent_type: "verify",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_VERIFY,
      result_text:
        "Confirmed: POST /refund and POST /credit call the handler " +
        "directly without requireAuth.",
      tool_use_count: 3,
      total_tokens: 15_000,
      duration_ms: 18_000,
    }),
    snap({
      subagent_id: "verify-webhooks",
      name: "Verify routes/webhooks.ts",
      agent_type: "verify",
      model: "sonnet",
      status: "completed",
      workflow_id: workflowId,
      phase: WORKFLOW_PHASE_VERIFY,
      result_text:
        "Confirmed: POST /stripe has no signature verification before " +
        "handling the webhook payload.",
      tool_use_count: 3,
      total_tokens: 14_000,
      duration_ms: 17_000,
    }),

    // Final roll-up snapshot: seals the run and carries the aggregate
    // figures the design's completion pill shows.
    {
      type: "workflow_updated",
      thread_id: threadId,
      workflow: {
        workflow_id: workflowId,
        status: "completed",
        result_text:
          "3 endpoints missing auth checks across 2 files (billing.ts, webhooks.ts).",
        total_tokens: 2_900_000,
        agent_count: 44,
        duration_ms: 291_000, // 4m 51s
      },
    },
    {
      type: "item_completed",
      thread_id: threadId,
      turn_id: turnId,
      item: { kind: "assistant_text", text: REPORT },
    },
    {
      type: "turn_completed",
      thread_id: threadId,
      turn_id: turnId,
      status: { kind: "success" },
      usage: null,
    },
  ];
}

// ── Monitoring demo transcript ───────────────────────────────────────
//
// A finished deliverable plus a still-running watch loop, which is exactly
// the shape that settles a thread to `PaneStatus::Monitoring`: the turn
// completed, no agent tasks are live, and one `task_kind: "monitor"` subagent
// is still going. That monitor row deliberately keeps its transcript card
// (the user should be able to see what is being watched) while being excluded
// from the docked "N subagents running" bar.

/** Envelopes for {@link MOCK_MONITORING_THREAD_ID}. */
export function monitoringEnvelopes(threadId: string): unknown[] {
  const turnId = "turn-monitoring";
  return [
    {
      type: "user_message",
      thread_id: threadId,
      text: "Push the auth fix and keep an eye on CI — ping me if it goes red.",
    },
    {
      type: "item_completed",
      thread_id: threadId,
      turn_id: turnId,
      item: {
        kind: "assistant_message",
        text:
          "Pushed `fix/token-refresh` and opened **PR #482**. I'll watch the " +
          "checks in the background and report back if anything fails.",
      },
    },
    {
      type: "subagent_updated",
      thread_id: threadId,
      subagent: {
        subagent_id: "monitor-ci-482",
        name: "CI watch",
        agent_type: "monitor",
        task_kind: "monitor",
        model: "haiku",
        status: "running",
        activity: "Polling checks on PR #482 (3 of 7 green)",
        tool_use_count: 12,
        total_tokens: 4_200,
      },
    },
    {
      type: "turn_completed",
      thread_id: threadId,
      turn_id: turnId,
      status: { kind: "success" },
      usage: null,
    },
  ];
}

// ── Stress-fixture scaling ──────────────────────────────────────────
//
// The curated 21 workspaces above are a design fixture. Selecting a stress
// fixture (`?fixture=large`, see `stress-fixture.ts`) scales that list to the
// audited real profile so switch latency is measured against something honest.
// Generated workspaces reuse the same builders, so they carry the same pane /
// session / status bookkeeping as the curated ones.

const STRESS_STATUS_CYCLE: PaneStatus[] = ["idle", "working", "review", "permission"];

let stressWorkspacesCache: WorkspaceSnapshot[] | null = null;

function stressWorkspaces(target: number): WorkspaceSnapshot[] {
  if (stressWorkspacesCache) return stressWorkspacesCache;

  if (target <= ALL_WORKSPACES.length) {
    const kept = ALL_WORKSPACES.slice(0, target);
    // The seed's active workspace must survive any cut, or the snapshot
    // points `active_workspace_id` at a workspace that isn't in the list.
    if (!kept.some((w) => w.workspace_id === wsCodemuxChat.workspace_id)) {
      kept[kept.length - 1] = wsCodemuxChat;
    }
    stressWorkspacesCache = kept;
    return kept;
  }

  const generated: WorkspaceSnapshot[] = [];
  for (let i = 1; i <= target - ALL_WORKSPACES.length; i += 1) {
    const title = `stress-${String(i).padStart(2, "0")}`;
    const cwd = `${HOME}/projects/stress/${title}`;
    const workspace = makeWorkspace({
      workspace_id: `ws-stress-${i}`,
      title,
      cwd,
      project_root: `${HOME}/projects/stress`,
      project_uid: "uid-stress",
      git_branch: `feat/${title}`,
      git_changed_files: i % 7,
      status: STRESS_STATUS_CYCLE[i % STRESS_STATUS_CYCLE.length],
    });
    // Every third generated workspace opens on a chat pane, so a switch
    // sweep crosses both the terminal and the chat mount paths.
    if (i % 3 === 0) {
      const { surface, tab } = chatSurface(title, cwd, `${STRESS_THREAD_PREFIX}${i}`);
      generated.push({
        ...workspace,
        tabs: [tab],
        active_tab_id: tab.tab_id,
        active_surface_id: surface.surface_id,
        surfaces: [surface],
      });
    } else {
      generated.push(workspace);
    }
  }

  stressWorkspacesCache = [...ALL_WORKSPACES, ...generated];
  return stressWorkspacesCache;
}

/**
 * Build a fresh, deep-ish-cloned `AppStateSnapshot`. The mock holds a
 * single mutable instance and re-emits it after mutating commands, so
 * we hand out a structuredClone to keep the canonical seed pristine
 * across hot reloads / re-installs.
 */
export function createSeedAppState(): AppStateSnapshot {
  const fixture = getStressFixture();
  const workspaces = fixture ? stressWorkspaces(fixture.workspaces) : ALL_WORKSPACES;
  return structuredClone({
    schema_version: 1,
    // Agent Chat is the default interface, so the plain-browser mock should
    // open on its hydrated transcript instead of a legacy terminal fixture.
    active_workspace_id: wsCodemuxChat.workspace_id,
    workspaces,
    terminal_sessions: terminalSessions,
    browser_sessions: [],
    agent_browser_sessions: [
      MOCK_AGENT_BROWSER_SESSION,
      MOCK_CLI_AGENT_BROWSER_SESSION,
    ],
    notifications: [],
    detected_ports: [
      {
        port: 1420,
        pid: 4242,
        process_name: "vite",
        workspace_id: wsCodemuxMock.workspace_id,
        label: "dev server",
        source: null,
      },
      {
        port: 5432,
        pid: 909,
        process_name: "postgres",
        workspace_id: null,
        label: null,
        source: "docker",
      },
    ],
    pane_statuses: paneStatuses,
    manual_monitors: manualMonitors,
    archived_workspaces: ARCHIVED_WORKSPACES,
    persistence: MOCK_PERSISTENCE,
    config: MOCK_CONFIG,
  });
}

// ── Web Remote Access seed ───────────────────────────────────────────
//
// Exercises the Settings → Remote Access panel in plain-browser dev. The
// mock state machine in `tauri-mock.ts` owns the mutable enabled/config
// state; these are the immutable seed shapes it starts from. The endpoint
// set spans every display group (this device / local network / Tailscale /
// other) so the grouped "Reachable at" list and pairing chips render in
// full, alongside three sessions (one live, one idle, one pending approval)
// and a pairing response builder.

export const MOCK_WEB_REMOTE_PORT = 4377;

/**
 * Reachable endpoints, one per display group so the grouped UI renders end
 * to end: loopback (this device, secure), a LAN IP (local network, plain
 * HTTP), a tailnet IP + a MagicDNS hostname served over HTTPS (Tailscale;
 * the MagicDNS name is the recommended "from anywhere" option), and a lone
 * IPv6 ULA address to exercise the collapsed "Other addresses" disclosure.
 */
export function mockWebRemoteEndpoints(port: number): WebRemoteEndpoint[] {
  const ep = (
    kind: string,
    group: string,
    h: string,
    secure: boolean,
    scheme: string,
    recommended: boolean,
    label: string,
  ): WebRemoteEndpoint => ({
    kind,
    group,
    host: h,
    port,
    // Bracket IPv6 literals in the URL host, matching the backend.
    url: `${scheme}://${h.includes(":") ? `[${h}]` : h}:${port}`,
    secure,
    recommended,
    label,
  });
  return [
    ep("loopback", "this_device", "127.0.0.1", true, "http", false, "This device only (secure context)"),
    ep("lan", "local_network", "192.168.68.58", false, "http", false, "Local network (plain HTTP)"),
    ep("tailnet", "tailscale", "100.119.27.64", false, "http", false, "Over your tailnet"),
    ep(
      "magicdns",
      "tailscale",
      "mac-studio.tail9c2f.ts.net",
      true,
      "https",
      true,
      "MagicDNS name over Tailscale's HTTPS serve (trusted certificate)",
    ),
    ep("lan", "other", "fd7a:115c:a1e0::42", false, "http", false, "Other network address (plain HTTP)"),
  ];
}

/** Three seeded devices: a live-connected laptop, an idle phone, and a
 *  Linux desktop still waiting for approval. */
export function mockWebRemoteSessions(): WebRemoteSessionView[] {
  const now = Date.now();
  const iso = (msAgo: number) => new Date(now - msAgo).toISOString();
  return [
    {
      id: "sess-macbook",
      name: "Sam's MacBook Air",
      user_agent:
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15",
      created_at: iso(3 * 24 * 60 * 60 * 1000),
      last_seen_at: iso(4000),
      approved: true,
      connected: true,
      source: "pair",
    },
    {
      id: "sess-iphone",
      name: "iPhone 15",
      user_agent:
        "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1",
      created_at: iso(2 * 60 * 60 * 1000),
      last_seen_at: iso(18 * 60 * 1000),
      approved: true,
      connected: false,
      // Admitted by signing into the desktop's Codemux account (account mode).
      source: "account",
    },
    {
      id: "sess-linux-pending",
      name: "workstation",
      user_agent:
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
      created_at: iso(30 * 1000),
      last_seen_at: null,
      approved: false,
      connected: false,
      source: "pair",
    },
  ];
}

/** A fresh one-time pairing response (10-minute TTL) with a random token. */
export function mockWebRemotePairing(): WebRemotePairingInfo {
  const token = Array.from({ length: 32 }, () =>
    Math.floor(Math.random() * 16).toString(16),
  ).join("");
  return {
    url_path: `/#pair=${token}`,
    token,
    expires_at: new Date(Date.now() + 10 * 60 * 1000).toISOString(),
  };
}

// ── Settings → Usage ──

/** Deterministic value hash, the design script's `rnd()`. Seeded rather
 *  than `Math.random()` so the dev mock renders identically on every
 *  reload — a chart that reshuffles between hot-reloads makes visual
 *  regressions impossible to spot. */
function usageRnd(seed: number): number {
  const x = Math.sin(seed * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
}

const USAGE_PROVIDERS = ["claude", "codex", "grok", "opencode"] as const;

/** Rough $/Mtok blend per provider, used to derive a plausible cost
 *  from a token count. */
const USAGE_RATE: Record<string, number> = {
  claude: 9.4,
  codex: 7.1,
  grok: 5.8,
  opencode: 4.6,
};

const USAGE_MODELS: Record<string, { model: string; weight: number; subagent?: boolean }[]> = {
  claude: [
    { model: "claude-opus-4-5", weight: 0.62 },
    { model: "claude-sonnet-4-5", weight: 0.29 },
    { model: "claude-haiku-4-5", weight: 0.09, subagent: true },
  ],
  codex: [
    { model: "gpt-5-codex", weight: 0.64 },
    { model: "gpt-5-codex-mini", weight: 0.36 },
  ],
  // Keep the runtime-owned sentinel here too: browser-dev must not pin the
  // frontend to a concrete Grok release name.
  grok: [{ model: "default", weight: 1 }],
  opencode: [
    { model: "anthropic/claude-sonnet-4-5", weight: 0.48 },
    { model: "openrouter/kimi-k2", weight: 0.31 },
    { model: "openai/gpt-5-mini", weight: 0.21 },
  ],
};

const USAGE_MONTHS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/** Realistic fixture for `usage_summary`.
 *
 *  Mirrors the shapes the real aggregation produces: Claude dominates,
 *  Codex is second, Grok/OpenCode follow, weekends dip,
 *  and one Claude model carries all the subagent tokens. */
export function mockUsageSummary(period: string): unknown {
  const hourly = period === "today";
  const bucketMs = hourly ? 3_600_000 : 86_400_000;
  const count = hourly
    ? 24
    : period === "90d"
      ? 90
      : period === "30d"
        ? 30
        : 7;
  const now = Date.now();
  const newestStart = Math.floor(now / bucketMs) * bucketMs;
  const startMs = newestStart - (count - 1) * bucketMs;

  const buckets = [];
  // Per-provider running totals, accumulated from the same numbers the
  // buckets render so the lanes and the chart always agree.
  const totals: Record<string, number> = {
    claude: 0,
    codex: 0,
    grok: 0,
    opencode: 0,
  };

  for (let i = 0; i < count; i++) {
    const start = startMs + i * bucketMs;
    const date = new Date(start);
    let shape: number;
    let label: string;
    let subLabel: string;
    if (hourly) {
      const hour = date.getHours();
      // Work happens during the day; nights are near-idle.
      shape = hour >= 8 && hour <= 22 ? 1 : 0.12;
      label = `${hour}:00`;
      // "Today" is a trailing 24-hour window, so its older buckets belong
      // to yesterday — the same naming `bucket_labels` applies in Rust.
      const isToday = date.toDateString() === new Date(now).toDateString();
      subLabel = `${isToday ? "Today" : "Yesterday"} ${hour}:00`;
    } else {
      const day = date.getDay();
      shape = day === 0 || day === 6 ? 0.28 : 1;
      label = String(date.getDate());
      subLabel = `${USAGE_MONTHS[date.getMonth()]} ${date.getDate()}`;
    }

    const providers: Record<string, { tokens: number; cost_usd: number }> = {};
    USAGE_PROVIDERS.forEach((provider, index) => {
      const base = hourly
        ? provider === "claude"
          ? 90e3
          : provider === "codex"
            ? 34e3
            : provider === "grok"
              ? 26e3
              : 22e3
        : provider === "claude"
          ? 620e3
          : provider === "codex"
            ? 300e3
            : provider === "grok"
              ? 230e3
              : 190e3;
      const span = hourly ? base * 1.5 : base * 1.4;
      const seed = Math.floor(start / bucketMs) * 3 + index * 97;
      const tokens = Math.round((base + usageRnd(seed) * span) * shape);
      totals[provider] += tokens;
      providers[provider] = {
        tokens,
        cost_usd: (tokens / 1e6) * USAGE_RATE[provider],
      };
    });

    buckets.push({ start_ms: start, label, sub_label: subLabel, providers });
  }

  const providers = USAGE_PROVIDERS.map((provider) => {
    const tokens = totals[provider];
    const cost = (tokens / 1e6) * USAGE_RATE[provider];
    return {
      provider,
      tokens,
      // A plausible non-overlapping split: cache reads dominate a
      // long-running agent session, which is the point of the
      // "% served from cache" hero note.
      input_tokens: Math.round(tokens * 0.18),
      output_tokens: Math.round(tokens * 0.12),
      cache_read_tokens: Math.round(tokens * 0.64),
      cache_write_tokens: Math.round(tokens * 0.06),
      cost_usd: cost,
      session_count: Math.max(1, Math.round(tokens / 1e6 * 3.1)),
      models: USAGE_MODELS[provider].map((entry) => ({
        model: entry.model,
        tokens: Math.round(tokens * entry.weight),
        cost_usd: cost * entry.weight,
        subagent_tokens: entry.subagent ? Math.round(tokens * entry.weight) : 0,
      })),
    };
  }).sort((a, b) => b.tokens - a.tokens);

  const totalTokens = providers.reduce((sum, p) => sum + p.tokens, 0);
  const cacheRead = providers.reduce((sum, p) => sum + p.cache_read_tokens, 0);

  // Plan quota for the two providers that actually report it. OpenCode
  // has no quota API at all, so it stays absent and its lane renders
  // meter-less — the same shape the real backend produces.
  const resetIn = (mins: number) => now + mins * 60_000;
  const quota = {
    claude: {
      windows: [
        { kind: "five_hour", used_pct: 41, resets_at_ms: resetIn(134) },
        { kind: "seven_day", used_pct: 88, resets_at_ms: resetIn(3_400) },
        // A per-model weekly window, to exercise the "shown in the note,
        // not as a bar" path.
        { kind: "seven_day_opus", used_pct: 72, resets_at_ms: resetIn(3_400) },
      ],
      plan_label: "Max 20×",
      auth_mode: "subscription",
      received_at_ms: now,
    },
    codex: {
      windows: [
        { kind: "five_hour", used_pct: 18, resets_at_ms: resetIn(220) },
        { kind: "seven_day", used_pct: 63, resets_at_ms: resetIn(5_000) },
      ],
      plan_label: "ChatGPT Pro",
      auth_mode: "subscription",
      received_at_ms: now,
    },
  };

  // Composition + confidence + flat models, derived from the same
  // totals the lanes use so the mock stays internally consistent.
  const inputTokens = providers.reduce((n, p) => n + p.input_tokens, 0);
  const outputTokens = providers.reduce((n, p) => n + p.output_tokens, 0);
  const cacheWrite = providers.reduce((n, p) => n + p.cache_write_tokens, 0);
  const observedInput = inputTokens + cacheRead + cacheWrite;
  // Codex + OpenCode report a reasoning split; Claude does not.
  const reasoningTokens = Math.round(
    providers
      .filter((p) => p.provider !== "claude")
      .reduce((n, p) => n + p.output_tokens, 0) * 0.38,
  );
  const providerPricedCost = providers
    .filter((p) => p.provider === "opencode")
    .reduce((n, p) => n + p.cost_usd, 0);
  const allCost = providers.reduce((n, p) => n + p.cost_usd, 0);
  const savings = allCost * 2.2;

  const models = providers
    .flatMap((p) =>
      p.models.map((m) => ({
        provider: p.provider,
        model: m.model,
        tokens: m.tokens,
        cost_usd: m.cost_usd,
        // One deliberately unpriced model so the em-dash / token-share
        // path renders in the dev mock.
        priced: m.model !== "openrouter/kimi-k2",
        provider_reported: p.provider === "opencode",
      })),
    )
    .sort((a, b) =>
      a.priced === b.priced ? b.cost_usd - a.cost_usd : Number(b.priced) - Number(a.priced),
    );

  return {
    period,
    start_ms: startMs,
    end_ms: newestStart + bucketMs,
    buckets,
    providers,
    composition: {
      processed_tokens: totalTokens,
      cache_read_tokens: cacheRead,
      cache_read_share_of_input: observedInput > 0 ? cacheRead / observedInput : 0,
      input_tokens: inputTokens,
      cache_write_tokens: cacheWrite,
      output_tokens: outputTokens,
      reasoning_tokens: reasoningTokens,
      cache_savings_usd: savings,
      cache_savings_multiplier: 3.2,
    },
    confidence: {
      provider_reported_share: allCost > 0 ? providerPricedCost / allCost : 0,
      table_priced_share: allCost > 0 ? 1 - providerPricedCost / allCost : 0,
      unpriced_token_share: 0.041,
      cache_savings_usd: savings,
    },
    models,
    quota,
    totals: {
      estimated_cost_usd: allCost,
      total_tokens: totalTokens,
      cache_read_share: totalTokens > 0 ? cacheRead / totalTokens : 0,
      session_count: providers.reduce((sum, p) => sum + p.session_count, 0),
    },
    synced_at_ms: now,
  };
}

/** CSV matching `mockUsageSummary`, one row per bucket × provider × model. */
export function mockUsageExportCsv(period: string): string {
  const summary = mockUsageSummary(period) as {
    buckets: { sub_label: string; providers: Record<string, { tokens: number }> }[];
    providers: { provider: string; models: { model: string }[] }[];
  };
  let out =
    "bucket_start,provider,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,total_tokens,cost_usd\n";
  for (const bucket of summary.buckets) {
    for (const provider of summary.providers) {
      const tokens = bucket.providers[provider.provider]?.tokens ?? 0;
      for (const model of provider.models) {
        out += `${bucket.sub_label},${provider.provider},${model.model},0,0,0,0,${tokens},0.0000\n`;
      }
    }
  }
  return out;
}

// ── Pull request fixtures ───────────────────────────────────────────
//
// The Review panel is the densest read-only surface in the app, so the
// mock has to be able to reach every state it can render: a healthy PR,
// one with a failing and a running check, a draft, a merged record, a
// closed one, and a GitLab merge request. Each seeded workspace's
// checkout path maps to exactly one of these.
//
// Why a path map instead of `find(w => w.cwd === path)`: three codemux
// workspaces share `/home/dev/projects/codemux`, and only one of them
// carries a PR. A naive lookup returns the wrong workspace and the
// panel sits on a skeleton forever.

const MOCK_PR_AUTHOR = "mock-dev";
const OTHER_AUTHOR = "juliusm";

function minutesAgo(minutes: number): string {
  return new Date(Date.now() - minutes * 60_000).toISOString();
}

/** Fill in the fields every `PullRequestInfo` carries so each fixture
 *  below only has to state what makes it interesting. */
function makePr(seed: Partial<PullRequestInfo> & { number: number; title: string }): PullRequestInfo {
  return {
    url: `https://github.com/example/codemux/pull/${seed.number}`,
    state: "OPEN",
    head_branch: null,
    base_branch: "main",
    is_draft: false,
    mergeable: "MERGEABLE",
    additions: null,
    deletions: null,
    review_decision: null,
    checks_passing: null,
    updated_at: minutesAgo(38),
    created_at: minutesAgo(190),
    body: null,
    comments: [],
    totalComments: 0,
    author: MOCK_PR_AUTHOR,
    // Distinct per PR so the panel's force-push watch has something
    // real to compare across polls.
    head_ref_oid: `mock${seed.number}headsha0000000000000000000`.slice(0, 40),
    head_repository_owner: "example",
    merge_state_status: "CLEAN",
    changed_files: null,
    merged_by: null,
    merged_at: null,
    review_requests: [],
    latest_reviews: [],
    ...seed,
  };
}

const PR_172_BODY = `Unsent prompts currently disappear from the user's working context when they navigate away, even though the client already models pre-materialized chat drafts. This makes it easy to lose an invested prompt or accidentally reuse it when starting another agent.

This change keeps invested drafts as independent sessions and surfaces them one click away above the workspace inbox.

## Verification

- 237 affected tests across the draft store, sidebar rail and empty-state lifecycle
- browser E2E: create, navigate away, restore, reload, promote on \`Send\`, discard

Closes #168.

## Before / after

![before: an invested draft is gone after navigating away](data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI4ODAiIGhlaWdodD0iNDIwIj48cmVjdCB3aWR0aD0iODgwIiBoZWlnaHQ9IjQyMCIgZmlsbD0iIzE0MTIxMCIvPjxyZWN0IHg9IjI0IiB5PSIyNCIgd2lkdGg9IjgzMiIgaGVpZ2h0PSIzNzIiIHJ4PSIxMCIgZmlsbD0iIzFjMTkxNyIgc3Ryb2tlPSIjMmUyYTI2Ii8+PHRleHQgeD0iNDAiIHk9IjY2IiBmb250LWZhbWlseT0ibW9ub3NwYWNlIiBmb250LXNpemU9IjIyIiBmaWxsPSIjZTdlMmRjIj5iZWZvcmUgJiMxODM7IGRyYWZ0cyBsb3N0IG9uIG5hdmlnYXRlPC90ZXh0PjxyZWN0IHg9IjQwIiB5PSI5MiIgd2lkdGg9IjcwMCIgaGVpZ2h0PSIyNiIgcng9IjUiIGZpbGw9IiM4YTgwNzgiIG9wYWNpdHk9IjAuMjUiLz48cmVjdCB4PSI0MCIgeT0iMTM4IiB3aWR0aD0iNTQwIiBoZWlnaHQ9IjI2IiByeD0iNSIgZmlsbD0iIzhhODA3OCIgb3BhY2l0eT0iMC4zOSIvPjxyZWN0IHg9IjQwIiB5PSIxODQiIHdpZHRoPSI2MTAiIGhlaWdodD0iMjYiIHJ4PSI1IiBmaWxsPSIjOGE4MDc4IiBvcGFjaXR5PSIwLjUzIi8+PHJlY3QgeD0iNDAiIHk9IjIzMCIgd2lkdGg9IjQzMCIgaGVpZ2h0PSIyNiIgcng9IjUiIGZpbGw9IiM4YTgwNzgiIG9wYWNpdHk9IjAuNjciLz48cmVjdCB4PSI0MCIgeT0iMjc2IiB3aWR0aD0iNjYwIiBoZWlnaHQ9IjI2IiByeD0iNSIgZmlsbD0iIzhhODA3OCIgb3BhY2l0eT0iMC44MSIvPjxyZWN0IHg9IjQwIiB5PSIzMjIiIHdpZHRoPSI1MDAiIGhlaWdodD0iMjYiIHJ4PSI1IiBmaWxsPSIjOGE4MDc4IiBvcGFjaXR5PSIwLjk1Ii8+PC9zdmc+)

![after: the same draft is still there, one click above the inbox](data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI4ODAiIGhlaWdodD0iNDIwIj48cmVjdCB3aWR0aD0iODgwIiBoZWlnaHQ9IjQyMCIgZmlsbD0iIzE0MTIxMCIvPjxyZWN0IHg9IjI0IiB5PSIyNCIgd2lkdGg9IjgzMiIgaGVpZ2h0PSIzNzIiIHJ4PSIxMCIgZmlsbD0iIzFjMTkxNyIgc3Ryb2tlPSIjMmUyYTI2Ii8+PHRleHQgeD0iNDAiIHk9IjY2IiBmb250LWZhbWlseT0ibW9ub3NwYWNlIiBmb250LXNpemU9IjIyIiBmaWxsPSIjZTdlMmRjIj5hZnRlciAmIzE4MzsgZHJhZnRzIGtlcHQgYXMgc2Vzc2lvbnM8L3RleHQ+PHJlY3QgeD0iNDAiIHk9IjkyIiB3aWR0aD0iNzAwIiBoZWlnaHQ9IjI2IiByeD0iNSIgZmlsbD0iI2Q5N2EzZCIgb3BhY2l0eT0iMC4yNSIvPjxyZWN0IHg9IjQwIiB5PSIxMzgiIHdpZHRoPSI1NDAiIGhlaWdodD0iMjYiIHJ4PSI1IiBmaWxsPSIjZDk3YTNkIiBvcGFjaXR5PSIwLjM5Ii8+PHJlY3QgeD0iNDAiIHk9IjE4NCIgd2lkdGg9IjYxMCIgaGVpZ2h0PSIyNiIgcng9IjUiIGZpbGw9IiNkOTdhM2QiIG9wYWNpdHk9IjAuNTMiLz48cmVjdCB4PSI0MCIgeT0iMjMwIiB3aWR0aD0iNDMwIiBoZWlnaHQ9IjI2IiByeD0iNSIgZmlsbD0iI2Q5N2EzZCIgb3BhY2l0eT0iMC42NyIvPjxyZWN0IHg9IjQwIiB5PSIyNzYiIHdpZHRoPSI2NjAiIGhlaWdodD0iMjYiIHJ4PSI1IiBmaWxsPSIjZDk3YTNkIiBvcGFjaXR5PSIwLjgxIi8+PHJlY3QgeD0iNDAiIHk9IjMyMiIgd2lkdGg9IjUwMCIgaGVpZ2h0PSIyNiIgcng9IjUiIGZpbGw9IiNkOTdhM2QiIG9wYWNpdHk9IjAuOTUiLz48L3N2Zz4=)
`;

/** Long enough to exercise the description fold (>40 rendered lines). */
const PR_482_BODY = [
  "Adds a monitoring pane that watches long-running agent sessions.",
  "",
  ...Array.from({ length: 46 }, (_, i) => `- step ${i + 1}: verified against the staging cluster`),
].join("\n");

/** Keyed by PR number. The panel's queries resolve a path to a number
 *  through `MOCK_PR_PATH_TO_NUMBER` first. */
export const MOCK_PULL_REQUESTS: Record<number, PullRequestInfo> = {
  // The canonical open PR: one failing check, one running, reviewers
  // not yet requested. This is the panel screenshot state.
  172: makePr({
    number: 172,
    title: "feat: keep unsent drafts in the sidebar",
    head_branch: "agent/investigate-draft-mode",
    additions: 1180,
    deletions: 33,
    // Agrees with MOCK_PR_DIFFS[172] — the Code tab's count comes from
    // here and its file list from there, so a mismatch reads as a bug.
    changed_files: 5,
    body: PR_172_BODY,
    updated_at: minutesAgo(38),
    merge_state_status: "UNSTABLE",
    author: MOCK_PR_AUTHOR,
  }),
  // Someone else's PR, so the reviewer bar renders — and the one whose
  // submissions always fail, so the submit-failed notice (and the
  // promise that your text survives it) is reachable on demand.
  142: makePr({
    number: 142,
    title: "chore: detect dev-server ports without polling",
    head_branch: "chore/port-detection",
    author: OTHER_AUTHOR,
    additions: 58,
    deletions: 4,
    changed_files: 1,
    mergeable: "CONFLICTING",
    merge_state_status: "DIRTY",
    review_decision: "CHANGES_REQUESTED",
    updated_at: minutesAgo(12),
    body: "Replaces the 500ms port poll with an inotify watch on the project root.",
    latest_reviews: [{ author: OTHER_AUTHOR, state: "CHANGES_REQUESTED" }],
  }),
  // Author + everything green: the merge-sheet path.
  482: makePr({
    number: 482,
    title: "feat(monitoring): watch long-running agent sessions",
    head_branch: "demo/monitoring",
    additions: 902,
    deletions: 114,
    changed_files: 17,
    review_decision: "APPROVED",
    merge_state_status: "CLEAN",
    updated_at: minutesAgo(4),
    body: PR_482_BODY,
    latest_reviews: [{ author: OTHER_AUTHOR, state: "APPROVED" }],
  }),
  // Yours, with changes requested on it, on a branch a workspace here
  // is already standing in — the row that has to say three things at
  // once on the Pull Requests page.
  285: makePr({
    number: 285,
    title: "docs: drop the ports section from AGENTS.md",
    head_branch: "demo/workflow-approval",
    additions: 3,
    deletions: 41,
    changed_files: 1,
    review_decision: "CHANGES_REQUESTED",
    merge_state_status: "BLOCKED",
    updated_at: minutesAgo(184),
    body: "The ports guidance moved into the workspace docs; this drops the duplicate.",
    latest_reviews: [{ author: OTHER_AUTHOR, state: "CHANGES_REQUESTED" }],
  }),
  // Author + draft: Close / Ready for review.
  140: makePr({
    number: 140,
    title: "dev: mock Tauri runtime so the UI boots in a plain browser",
    head_branch: "feature/40-dev-mock-tauri-runtime",
    is_draft: true,
    additions: 214,
    deletions: 37,
    changed_files: 6,
    merge_state_status: "BLOCKED",
    updated_at: minutesAgo(95),
    body: "Seeds a full in-memory Tauri runtime so every surface is reachable in a plain browser.",
  }),
  // A merged record — the panel is read-only here.
  128: makePr({
    number: 128,
    title: "Refactor auth token refresh to avoid mid-session expiry",
    head_branch: "feature/auth-refactor",
    state: "MERGED",
    additions: 340,
    deletions: 288,
    changed_files: 11,
    review_decision: "APPROVED",
    merged_by: OTHER_AUTHOR,
    merged_at: minutesAgo(40 / 60),
    updated_at: minutesAgo(1),
    body: "Moves the refresh to a background timer keyed on token expiry.",
    latest_reviews: [{ author: OTHER_AUTHOR, state: "APPROVED" }],
  }),
  // Closed without merging.
  131: makePr({
    number: 131,
    title: "fix: stop the sidebar flickering on workspace switch",
    head_branch: "fix/sidebar-flicker",
    state: "CLOSED",
    additions: 22,
    deletions: 19,
    changed_files: 3,
    updated_at: minutesAgo(2880),
    body: "Superseded by the pane-deck rewrite.",
  }),
  // Reviewer state: someone else's PR, awaiting your verdict.
  12: makePr({
    number: 12,
    title: "design: refresh the landing hero",
    url: "https://github.com/example/personal-site/pull/12",
    head_branch: "design/landing-refresh",
    author: OTHER_AUTHOR,
    additions: 401,
    deletions: 220,
    changed_files: 11,
    review_requests: [MOCK_PR_AUTHOR],
    updated_at: minutesAgo(22),
    body: "New hero type scale, plus a reduced-motion path for the gradient.",
  }),
  // GitLab merge request (sigil `!`).
  88: makePr({
    number: 88,
    title: "feat: CUDA variant of the inference runtime",
    url: "https://gitlab.example.com/acme/vexis/-/merge_requests/88",
    head_branch: "feat/cuda-variant",
    additions: 512,
    deletions: 40,
    changed_files: 9,
    updated_at: minutesAgo(58),
    body: "Builds a second AppImage against the CUDA toolkit.",
  }),
  90: makePr({
    number: 90,
    title: "perf: add a benchmark suite",
    url: "https://gitlab.example.com/acme/vexis/-/merge_requests/90",
    head_branch: "perf/bench-suite",
    is_draft: true,
    additions: 132,
    deletions: 9,
    changed_files: 4,
    merge_state_status: "BLOCKED",
    updated_at: minutesAgo(310),
    body: "Criterion-based, runs nightly.",
  }),
  // Bitbucket: readable, and nothing else. Every write control is
  // absent here because the adapter declares no operations.
  64: makePr({
    number: 64,
    title: "feat: cache FX rates for the settlement job",
    url: "https://bitbucket.org/acme/ledger-api/pull-requests/64",
    head_branch: "feat/fx-rates-cache",
    additions: 128,
    deletions: 19,
    changed_files: 4,
    updated_at: minutesAgo(74),
    body: "Adds a 60s in-process cache in front of the rates provider.",
  }),
};

// ── Timelines ───────────────────────────────────────────────────────
//
// The host half of the Timeline tab. Agent runs are deliberately NOT
// here: they are local records written by a real handoff (see
// `pr-agent-runs-store`), so the only way one appears in the mock is by
// actually handing something off — which is exactly the property worth
// being able to check by hand.
//
// #172 carries the force-push that ties this tab to the Code tab's
// re-anchoring simulator, plus one event type this build has never seen,
// so the never-drop-an-unknown-event rule is visible in the running app.
export const MOCK_PR_TIMELINES: Record<number, PrTimelineEvent[]> = {
  172: [
    {
      id: "tl-172-1",
      actor: OTHER_AUTHOR,
      created_at: minutesAgo(160),
      kind: "review_requested",
      reviewer: MOCK_PR_AUTHOR,
    },
    {
      id: "tl-172-2",
      actor: "juliusm",
      created_at: minutesAgo(64),
      kind: "reviewed",
      verdict: "CHANGES_REQUESTED",
      body: "The discoverability leg is a separate follow-up — worth a line here saying so.",
      anchor: "AGENTS.md:12",
    },
    {
      id: "tl-172-3",
      actor: MOCK_PR_AUTHOR,
      created_at: minutesAgo(50),
      kind: "committed",
      sha: "a1f9c2e5d41b0c7a",
      message: "docs: note the follow-up and link the tracking issue",
    },
    {
      id: "tl-172-4",
      actor: MOCK_PR_AUTHOR,
      created_at: minutesAgo(41),
      kind: "head_ref_force_pushed",
      sha: "bb31d70e9c2f4a18",
    },
    // Renders as a plain one-liner rather than vanishing.
    {
      id: "tl-172-5",
      actor: MOCK_PR_AUTHOR,
      created_at: minutesAgo(39),
      kind: "other",
      label: "automatic base change succeeded",
    },
    // Deliberately very recent, so an agent run started during a dev
    // session lands *between* host events rather than always last —
    // which is the arrangement the rail exists to show.
    {
      id: "tl-172-6",
      actor: "juliusm",
      created_at: minutesAgo(1),
      kind: "commented",
      body: "Thanks — that reads much better.",
    },
  ],
  142: [
    {
      id: "tl-142-1",
      actor: OTHER_AUTHOR,
      created_at: minutesAgo(220),
      kind: "commented",
      body: "Opening early to get eyes on the polling removal.",
    },
    {
      id: "tl-142-2",
      actor: OTHER_AUTHOR,
      created_at: minutesAgo(96),
      kind: "committed",
      sha: "77c40be1aa9d3e02",
      message: "chore: drop the port poller",
    },
  ],
  88: [
    {
      id: "tl-88-1",
      actor: "mock-glab",
      created_at: minutesAgo(120),
      kind: "committed",
      sha: "d41f77b2c9e01a55",
      message: "",
    },
    {
      id: "tl-88-2",
      actor: "juliusm",
      created_at: minutesAgo(74),
      kind: "reviewed",
      verdict: "APPROVED",
      body: "",
      anchor: null,
    },
    // GitLab system notes this build has no specific row for.
    {
      id: "tl-88-3",
      actor: "mock-glab",
      created_at: minutesAgo(70),
      kind: "other",
      label: "changed target branch from **develop** to **main**",
    },
  ],
  64: [
    {
      id: "tl-64-1",
      actor: MOCK_PR_AUTHOR,
      created_at: minutesAgo(70),
      kind: "committed",
      sha: "5c0ffee1234abcd0",
      message: "feat: add the rates cache",
    },
  ],
};

/** Checkout path → PR number. Mirrors `worktree_path ?? cwd`, which is
 *  what the Review panel passes to every PR query. */
export const MOCK_PR_PATH_TO_NUMBER: Record<string, number> = {
  [`${HOME}/projects/codemux`]: 172,
  [`${HOME}/.codemux/worktrees/codemux/chore-port-detection`]: 142,
  [`${HOME}/.codemux/worktrees/codemux/demo-monitoring`]: 482,
  [`${HOME}/.codemux/worktrees/codemux/feature-40-dev-mock`]: 140,
  [`${HOME}/.codemux/worktrees/codemux/feature-auth-refactor`]: 128,
  [`${HOME}/.codemux/worktrees/codemux/fix-sidebar-flicker`]: 131,
  [`${HOME}/.codemux/worktrees/personal-site/design-landing-refresh`]: 12,
  [`${HOME}/.codemux/worktrees/vexis/feat-cuda-variant`]: 88,
  [`${HOME}/.codemux/worktrees/vexis/perf-bench-suite`]: 90,
  [`${HOME}/.codemux/worktrees/ledger-api/feat-fx-rates-cache`]: 64,
};

/**
 * What the Pull Requests page lists, per repository root.
 *
 * Numbers rather than rows: every field the page shows is derived from
 * the same `MOCK_PULL_REQUESTS` entry the detail column opens, so the
 * list and the detail can never disagree in the mock the way they could
 * if the rows were typed out twice.
 */
export const MOCK_PR_OVERVIEW: Record<string, { viewer: string; numbers: number[] }> = {
  [codemuxRoot]: { viewer: MOCK_PR_AUTHOR, numbers: [142, 172, 285, 482, 140] },
  [siteRoot]: { viewer: MOCK_PR_AUTHOR, numbers: [12] },
  // A different account on a different product — which is why the page
  // resolves the viewer per root rather than once.
  [vexisRoot]: { viewer: "mock-glab", numbers: [88, 90] },
  // `ledgerRoot` is deliberately absent. Bitbucket declares no
  // operations, so the real command layer refuses `list_prs_overview`
  // for it outright and the page draws a failing-root footer line. The
  // mock refuses it the same way (see `undeclaredRefusal` in
  // `tauri-mock.ts`) rather than serving rows production can never show
  // — a fixture that disagrees with the backend is a demo of a product
  // that doesn't exist. The two ledger workspaces stay: the read-only
  // workspace-panel path is still worth walking, and it never asks for
  // this list.
};

/** Who each pull request is waiting on. */
export const MOCK_PR_REVIEW_REQUESTS: Record<number, string[]> = {
  142: [MOCK_PR_AUTHOR],
  12: [MOCK_PR_AUTHOR],
};

/**
 * The root whose overview always fails.
 *
 * One unreachable repository is the case the footer line exists for:
 * the other roots keep their rows, and the page says how many it
 * couldn't read rather than blanking (binding rule 2).
 */
export const MOCK_UNREACHABLE_ROOT = scratchRoot;

/** Closed and merged pull requests per root, for the state dropdown. */
export const MOCK_PR_HISTORY: Record<string, number[]> = {
  [codemuxRoot]: [128, 131],
};

/**
 * The PR whose review submissions always fail.
 *
 * Deliberately a fixed PR rather than a call counter: the submit-failed
 * drift notice is the one state that has to be re-triggerable on demand
 * (its whole point is that the typed text survives a retry), and a
 * counter makes the second attempt succeed just when you want to look
 * at the failure again.
 */
export const MOCK_SUBMIT_FAILURE_PR = 142;

/** Checks per PR. #172 is the interesting one: four green, one failing,
 *  one still running. */
export const MOCK_PR_CHECKS: Record<number, CheckInfo[]> = {
  172: [
    checkOf("build (ubuntu-latest)", "pass", "1m 02s"),
    checkOf("lint", "pass", "24s"),
    checkOf("typecheck", "pass", "41s"),
    checkOf("test (ubuntu-latest)", "pass", "3m 18s"),
    checkOf("rust (windows-latest)", "fail", "2m 14s"),
    checkOf("e2e (chromium)", "pending", "2m 14s"),
  ],
  142: [
    checkOf("build (ubuntu-latest)", "pass", "58s"),
    checkOf("lint", "fail", "19s"),
  ],
  482: [
    checkOf("build (ubuntu-latest)", "pass", "1m 11s"),
    checkOf("lint", "pass", "22s"),
    checkOf("typecheck", "pass", "38s"),
    checkOf("test (ubuntu-latest)", "pass", "2m 55s"),
  ],
  140: [checkOf("lint", "pass", "20s"), checkOf("typecheck", "pending", "11s")],
  128: [checkOf("build (ubuntu-latest)", "pass", "1m 04s")],
  131: [],
  285: [checkOf("lint", "pass", "18s"), checkOf("typecheck", "pass", "34s")],
  // Running, so the page's "needs your review" group has one of each.
  12: [checkOf("build", "pass", "33s"), checkOf("visual-diff", "pending", "1m 12s")],
  88: [checkOf("cargo build", "pass", "4m 02s"), checkOf("cargo test", "pending", "1m 30s")],
  90: [checkOf("cargo build", "pass", "3m 48s")],
};

function checkOf(name: string, bucket: string, elapsed: string): CheckInfo {
  const done = bucket !== "pending";
  return {
    name,
    status: done ? "COMPLETED" : "IN_PROGRESS",
    conclusion: bucket,
    elapsed_time: elapsed,
    detail_url: `https://github.com/example/codemux/actions/runs/9${name.length}${bucket.length}`,
    started_at: minutesAgo(6),
    completed_at: done ? minutesAgo(3) : null,
  };
}

/** Canned 2-line failure tail for the failing-check card. */
export const MOCK_CHECK_LOG_EXCERPT =
  "error[E0432]: unresolved import `crate::pty::winpty`\n" +
  "  --> src/pty/mod.rs:14:5   |   14 | use crate::pty::winpty::Session;";

export const MOCK_PR_REVIEWS: Record<number, ReviewComment[]> = {
  142: [
    {
      id: 9001,
      author: OTHER_AUTHOR,
      body: "The inotify watch leaks a descriptor when the project root is deleted while watching.",
      state: "CHANGES_REQUESTED",
      created_at: minutesAgo(30),
    },
  ],
  482: [
    {
      id: 9002,
      author: OTHER_AUTHOR,
      body: "Reads well. The buffering on the status stream is the right call.",
      state: "APPROVED",
      created_at: minutesAgo(9),
    },
  ],
  128: [
    {
      id: 9003,
      author: OTHER_AUTHOR,
      body: "Confirmed against a 12-hour session — no mid-session expiry.",
      state: "APPROVED",
      created_at: minutesAgo(120),
    },
  ],
  172: [
    {
      id: 9004,
      author: OTHER_AUTHOR,
      body: "Left two notes on the draft store; nothing blocking.",
      state: "COMMENTED",
      created_at: minutesAgo(20),
    },
  ],
};

export const MOCK_PR_INLINE_COMMENTS: Record<number, InlineReviewComment[]> = {
  172: [
    {
      id: 8001,
      author: OTHER_AUTHOR,
      body: "Worth a comment on why this survives a reload but not a quit.",
      path: "src/stores/draft-store.ts",
      line: 84,
      created_at: minutesAgo(20),
      in_reply_to_id: null,
      pull_request_review_id: 9004,
    },
    {
      id: 8002,
      author: OTHER_AUTHOR,
      body: "Nit: this map never shrinks.",
      path: "src/components/sidebar/draft-rail.tsx",
      line: 132,
      created_at: minutesAgo(19),
      in_reply_to_id: null,
      pull_request_review_id: 9004,
    },
  ],
  142: [
    {
      id: 8003,
      author: OTHER_AUTHOR,
      body: "Descriptor leak: this needs a close on the error path.",
      path: "src-tauri/src/ports/watch.rs",
      line: 61,
      created_at: minutesAgo(30),
      in_reply_to_id: null,
      pull_request_review_id: 9001,
    },
  ],
};

/**
 * Review threads — the shape the GraphQL/discussions path returns.
 *
 * Deliberately overlapping with `MOCK_PR_INLINE_COMMENTS` by
 * `database_id`: 8001 and 8002 appear in both, so the dedupe (a comment
 * that lives in a thread is drawn by the thread, once) is exercised by
 * simply opening the panel rather than by a test alone. The review
 * *summary* for #172 (id 9004) is a different resource and survives,
 * which is the other half of the same rule.
 */
export const MOCK_PR_REVIEW_THREADS: Record<number, PrReviewThread[]> = {
  172: [
    {
      id: "PRRT_draft-store",
      is_resolved: false,
      is_outdated: false,
      is_resolvable: true,
      path: "src/stores/draft-store.ts",
      line: 84,
      comments: [
        {
          id: "PRRC_8001",
          database_id: 8001,
          author: OTHER_AUTHOR,
          body: "Worth a comment on why this survives a reload but not a quit.",
          created_at: minutesAgo(20),
        },
      ],
    },
    {
      id: "PRRT_draft-rail",
      is_resolved: false,
      is_outdated: false,
      is_resolvable: true,
      path: "src/components/sidebar/draft-rail.tsx",
      line: 132,
      comments: [
        {
          id: "PRRC_8002",
          database_id: 8002,
          author: OTHER_AUTHOR,
          body: "Nit: this map never shrinks.",
          created_at: minutesAgo(19),
        },
        {
          id: "PRRC_8102",
          database_id: 8102,
          author: "mock-dev",
          body: "It is bounded by the number of open workspaces — but I will add the eviction anyway.",
          created_at: minutesAgo(12),
        },
      ],
    },
    {
      id: "PRRT_outdated",
      is_resolved: false,
      is_outdated: true,
      is_resolvable: true,
      // Outdated threads keep their path and lose their line: the lines
      // they were written against are no longer in the diff.
      path: "src/lib/sessions.ts",
      line: null,
      comments: [
        {
          id: "PRRC_8201",
          database_id: 8201,
          author: OTHER_AUTHOR,
          body: "This early return skips the flush on the quit path.",
          created_at: minutesAgo(95),
        },
      ],
    },
    {
      id: "PRRT_resolved",
      is_resolved: true,
      is_outdated: false,
      is_resolvable: true,
      path: "src-tauri/src/pty/mod.rs",
      line: 214,
      comments: [
        {
          id: "PRRC_8301",
          database_id: 8301,
          author: OTHER_AUTHOR,
          body: "Is the handle closed if the spawn fails here?",
          created_at: minutesAgo(140),
        },
        {
          id: "PRRC_8302",
          database_id: 8302,
          author: "mock-dev",
          body: "It is — the guard drops on the error path. Added a test.",
          created_at: minutesAgo(130),
        },
        {
          id: "PRRC_8303",
          database_id: 8303,
          author: OTHER_AUTHOR,
          body: "Good, thanks.",
          created_at: minutesAgo(128),
        },
      ],
    },
  ],
  142: [
    {
      id: "PRRT_watch",
      is_resolved: false,
      is_outdated: false,
      is_resolvable: true,
      path: "src-tauri/src/ports/watch.rs",
      line: 61,
      comments: [
        {
          id: "PRRC_8003",
          database_id: 8003,
          author: OTHER_AUTHOR,
          body: "Descriptor leak: this needs a close on the error path.",
          created_at: minutesAgo(30),
        },
      ],
    },
  ],
  // GitLab: a resolvable diff discussion and a plain comment that has no
  // resolution state at all — so one draws a Resolve button and the
  // other does not.
  88: [
    {
      id: "3f1a9c0d5b",
      is_resolved: false,
      is_outdated: false,
      is_resolvable: true,
      path: "src/runner/pipeline.rs",
      line: 47,
      comments: [
        {
          id: "5501",
          database_id: 5501,
          author: "mira",
          body: "Can this retry loop hit the API more than once per second?",
          created_at: minutesAgo(48),
        },
      ],
    },
    {
      id: "9b2e7d4c1a",
      is_resolved: false,
      is_outdated: false,
      is_resolvable: false,
      path: null,
      line: null,
      comments: [
        {
          id: "5502",
          database_id: 5502,
          author: "mira",
          body: "Overall this reads well. One question on the runner.",
          created_at: minutesAgo(50),
        },
      ],
    },
  ],
};

// ── PR diffs ────────────────────────────────────────────────────────
//
// The Code tab parses these, so they have to be real unified diffs with
// line numbers that add up — a hunk header that lies about its counts
// puts every note below it on the wrong line, which is exactly the bug
// the surface exists to avoid. The anchored inline comments above
// (draft-store.ts:84, draft-rail.tsx:132, watch.rs:61) are addressable
// in these.

/** A file big enough to trip the 2,000-changed-line threshold. */
function hugeFileDiff(): string {
  const path = "src/generated/api-types.ts";
  const dels: string[] = [];
  const adds: string[] = [];
  for (let i = 1; i <= 1_100; i++) {
    dels.push(`-export type Op${i} = { kind: "op${i}"; payload: unknown };`);
    adds.push(`+export type Op${i} = { kind: "op${i}"; payload: Payload${i} };`);
  }
  return [
    `diff --git a/${path} b/${path}`,
    "index 3f1a0c2..9b7e441 100644",
    `--- a/${path}`,
    `+++ b/${path}`,
    "@@ -1,1100 +1,1100 @@",
    ...dels,
    ...adds,
  ].join("\n");
}

/**
 * Diffs are written as line arrays, not template literals.
 *
 * A blank context line in a unified diff is a single space, and a
 * formatter that strips trailing whitespace would silently turn it into
 * an empty line — which ends the hunk as far as a parser is concerned,
 * and quietly renumbers everything below it. `BLANK` makes that
 * un-strippable.
 */
const BLANK = " ";

const DRAFT_STORE_DIFF = [
  "diff --git a/src/stores/draft-store.ts b/src/stores/draft-store.ts",
  "index 1a2b3c4..5d6e7f8 100644",
  "--- a/src/stores/draft-store.ts",
  "+++ b/src/stores/draft-store.ts",
  "@@ -81,8 +81,12 @@ export const useDraftStore = create<DraftStore>((set, get) => ({",
  "   /** Drafts are keyed by workspace, not by tab. */",
  "   drafts: {},",
  BLANK,
  "+  /** Restores what you typed after a reload — but not after a quit. */",
  "+  hydrate: (saved) => set({ drafts: saved }),",
  "+",
  "-  setDraft: (workspaceId, text) => {",
  "-    set((state) => ({ drafts: { ...state.drafts, [workspaceId]: text } }));",
  "-  },",
  "+  setDraft: (workspaceId, text) => {",
  "+    const trimmed = text.trimEnd();",
  "+    set((state) => ({ drafts: { ...state.drafts, [workspaceId]: trimmed } }));",
  "+  },",
  BLANK,
  "   clearDraft: (workspaceId) => {",
].join("\n");

/**
 * The same file after a force-push.
 *
 * Two lines carry the demonstration: `hydrate` survives untouched but
 * slides three rows down (a note on it must move, quietly), and `const
 * trimmed` is rewritten (a note on it must be reported lost, never
 * guessed onto the line that replaced it). That is the shape of the live
 * experiment the matcher was validated against.
 */
const DRAFT_STORE_DIFF_AFTER = [
  "diff --git a/src/stores/draft-store.ts b/src/stores/draft-store.ts",
  "index 1a2b3c4..77c0e19 100644",
  "--- a/src/stores/draft-store.ts",
  "+++ b/src/stores/draft-store.ts",
  "@@ -81,8 +81,15 @@ export const useDraftStore = create<DraftStore>((set, get) => ({",
  "   /** Drafts are keyed by workspace, not by tab. */",
  "   drafts: {},",
  BLANK,
  "+  /** Bumped whenever the shape below changes. */",
  "+  version: 2,",
  "+",
  "+  /** Restores what you typed after a reload — but not after a quit. */",
  "+  hydrate: (saved) => set({ drafts: saved }),",
  "+",
  "-  setDraft: (workspaceId, text) => {",
  "-    set((state) => ({ drafts: { ...state.drafts, [workspaceId]: text } }));",
  "-  },",
  "+  setDraft: (workspaceId, text) => {",
  '+    const cleaned = text.replace(/\\s+$/, "");',
  "+    set((state) => ({ drafts: { ...state.drafts, [workspaceId]: cleaned } }));",
  "+  },",
  BLANK,
  "   clearDraft: (workspaceId) => {",
].join("\n");

const DRAFT_RAIL_DIFF = [
  "diff --git a/src/components/sidebar/draft-rail.tsx b/src/components/sidebar/draft-rail.tsx",
  "index 7c1da03..2e4f9a1 100644",
  "--- a/src/components/sidebar/draft-rail.tsx",
  "+++ b/src/components/sidebar/draft-rail.tsx",
  "@@ -128,6 +128,8 @@ export function DraftRail({ workspaceId }: Props) {",
  "   const drafts = useDraftStore((s) => s.drafts);",
  "   const entries = Object.entries(drafts);",
  BLANK,
  "-  const recent = entries.slice(0, 8);",
  "+  // Cap the rail before it becomes a second sidebar.",
  "+  const recent = entries.slice(0, MAX_RAIL_ENTRIES);",
  "+  const overflow = Math.max(0, entries.length - MAX_RAIL_ENTRIES);",
  BLANK,
  "   if (!entries.length) return null;",
].join("\n");

const ICON_BINARY_DIFF = [
  "diff --git a/src-tauri/icons/128x128.png b/src-tauri/icons/128x128.png",
  "index 4a1f9c3..b02e7d5 100644",
  "Binary files a/src-tauri/icons/128x128.png and b/src-tauri/icons/128x128.png differ",
].join("\n");

const LOCKFILE_DIFF = [
  "diff --git a/package-lock.json b/package-lock.json",
  "index aa11bb2..cc33dd4 100644",
  "--- a/package-lock.json",
  "+++ b/package-lock.json",
  "@@ -1204,6 +1204,7 @@",
  '       "dev": true',
  "     },",
  '     "node_modules/zustand": {',
  '+      "version": "5.0.3",',
  '       "resolved": "https://registry.npmjs.org/zustand/-/zustand-5.0.2.tgz",',
  '       "integrity": "sha512-fake"',
  "     },",
].join("\n");

const WATCH_RS_DIFF = [
  "diff --git a/src-tauri/src/ports/watch.rs b/src-tauri/src/ports/watch.rs",
  "index 9f0e1a2..3c5b8d7 100644",
  "--- a/src-tauri/src/ports/watch.rs",
  "+++ b/src-tauri/src/ports/watch.rs",
  "@@ -57,6 +57,8 @@ impl PortWatcher {",
  "     pub fn poll_once(&mut self) -> Result<Vec<PortEvent>, String> {",
  "         let listeners = self.scan()?;",
  BLANK,
  '-        let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;',
  '+        let socket = UdpSocket::bind("127.0.0.1:0")',
  '+            .map_err(|e| format!("port probe: {e}"))?;',
  "+        socket.set_nonblocking(true).map_err(|e| e.to_string())?;",
  BLANK,
  "         Ok(self.diff(listeners))",
].join("\n");

/**
 * Per-PR diffs, in the two forms the backend serves: `nameOnly` mirrors
 * `gh pr diff --name-only`, `full` is the patch the Code tab renders.
 * `afterForcePush` is the same PR rewritten — see the mock's
 * `__codemuxMockForcePush` trigger.
 */
/** A small GitLab diff, so the Code tab and its verdict sheet are
 *  reachable on a host that declares no request-changes verdict. */
const CUDA_RUNTIME_DIFF = [
  "diff --git a/src/runtime/cuda.rs b/src/runtime/cuda.rs",
  "index 3f1a2b4..9c0d5e6 100644",
  "--- a/src/runtime/cuda.rs",
  "+++ b/src/runtime/cuda.rs",
  "@@ -18,6 +18,9 @@ impl Runtime {",
  "     pub fn device_count() -> usize {",
  "         unsafe { cuda_device_count() as usize }",
  "     }",
  "+",
  "+    /// Returns None when the toolkit is present but no device is.",
  "+    pub fn preferred_device() -> Option<Device> {",
  "+        Self::devices().into_iter().max_by_key(|d| d.total_memory)",
  "+    }",
  " ",
  "     fn devices() -> Vec<Device> {",
  "         (0..Self::device_count()).map(Device::new).collect()",
].join("\n");

export const MOCK_PR_DIFFS: Record<
  number,
  { nameOnly: string; full: string; afterForcePush?: string }
> = {
  88: {
    nameOnly: "src/runtime/cuda.rs",
    full: CUDA_RUNTIME_DIFF,
  },
  172: {
    nameOnly: [
      "src/stores/draft-store.ts",
      "src/components/sidebar/draft-rail.tsx",
      "src/generated/api-types.ts",
      "package-lock.json",
      "src-tauri/icons/128x128.png",
    ].join("\n"),
    full: [
      DRAFT_STORE_DIFF,
      DRAFT_RAIL_DIFF,
      hugeFileDiff(),
      LOCKFILE_DIFF,
      ICON_BINARY_DIFF,
    ].join("\n"),
    afterForcePush: [
      DRAFT_STORE_DIFF_AFTER,
      DRAFT_RAIL_DIFF,
      hugeFileDiff(),
      LOCKFILE_DIFF,
      ICON_BINARY_DIFF,
    ].join("\n"),
  },
  142: {
    nameOnly: "src-tauri/src/ports/watch.rs",
    full: WATCH_RS_DIFF,
  },
};

/**
 * The one seeded checkout that has never been pushed.
 *
 * The "no pull request yet" and "nothing pushed yet" empty states differ
 * only by whether the branch exists on the remote, so the mock needs at
 * least one of each to be reachable.
 */
export const MOCK_LOCAL_ONLY_PATH = `${HOME}/.codemux/worktrees/vexis/fix-installer-detect`;

// ── Opening a pull request (5a) ──────────────────────────────────────
//
// The create form's whole premise is that the branch already contains a
// title and a description, so the mock has to contain commits worth
// drafting from. This checkout is the seeded "no pull request yet"
// branch: pushed, four commits ahead, and two files still uncommitted,
// which is every row of the form at once.

/** The account every seeded GitHub root is signed in as — who "yours"
 *  and "needs your review" are measured against. */
export const MOCK_PR_VIEWER = MOCK_PR_AUTHOR;

/** The seeded checkout the create form is reachable from. */
export const MOCK_CREATE_PR_PATH = `${HOME}/.codemux/worktrees/codemux/feature-75-chat-channel`;

/**
 * Commits ahead of the base, newest first — the same order `git log`
 * returns and `git_commits_ahead` preserves.
 *
 * Written so the drafting heuristics are exercised rather than merely
 * satisfied: all four share a `feat(chat)` prefix and the first two
 * words of their subject, which is what produces a title, and one body
 * carries a verification line, which is the only thing that puts a
 * `Verification ·` line in the description.
 */
export const MOCK_COMMITS_AHEAD: Record<string, CommitSummary[]> = {
  [MOCK_CREATE_PR_PATH]: [
    {
      short_hash: "9f2c1ab",
      subject: "feat(chat): stream replies without buffering the whole turn",
      body: "Verification · npm run check and the chat suite pass locally.",
    },
    {
      short_hash: "41d7e05",
      subject: "feat(chat): stream replies through a single channel",
      body: "",
    },
    {
      short_hash: "c08b3d9",
      subject: "feat(chat): stream replies as runtime events",
      body: "Co-Authored-By: someone <someone@example.com>",
    },
    {
      short_hash: "77a41e2",
      subject: "feat(chat): stream replies over a dedicated socket",
      body: "",
    },
  ],
};

/** Uncommitted work in the create form's checkout, for the warning row. */
export const MOCK_CREATE_PR_DIRTY: GitFileStatus[] = [
  {
    path: "src/components/chat/ChatChannel.tsx",
    status: "modified",
    is_staged: false,
    is_unstaged: true,
    additions: 14,
    deletions: 2,
    conflict_type: null,
  },
  {
    path: "src/lib/agent-chat/channel.ts",
    status: "modified",
    is_staged: false,
    is_unstaged: true,
    additions: 6,
    deletions: 0,
    conflict_type: null,
  },
];

/**
 * The repository's pull-request template.
 *
 * Seeded for the codemux root only, so the chip's other half — a
 * repository with no template, where the chip must not be drawn at all —
 * is equally reachable in browser dev.
 */
export const MOCK_PR_TEMPLATE_ROOT = codemuxRoot;

export const MOCK_PR_TEMPLATE = `## What changed

<!-- One or two sentences. -->

## Why

## Verification

- [ ] \`npm run check\`
- [ ] Affected tests
`;

/** Every path a repository template is looked for at. */
export const MOCK_TEMPLATE_PATH_PATTERN =
  /(PULL_REQUEST_TEMPLATE|pull_request_template|merge_request_templates)/;
