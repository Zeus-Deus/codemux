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
  AppStateSnapshot,
  AuthUser,
  CodemuxConfigSnapshot,
  LinkedIssue,
  PaneNodeSnapshot,
  PaneStatus,
  PersistenceSchema,
  SurfaceSnapshot,
  TabSnapshot,
  TerminalSessionSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

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
    provider: "claude",
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

// ── Workspaces ──────────────────────────────────────────────────────
//
// Project 1 — codemux: 1 primary (repo root) + 4 worktrees, covering
// the full PR matrix and all three agent states.

const codemuxRoot = `${PROJECTS}/codemux`;
const codemuxUid = "uid-codemux";

const wsCodemuxMain = makeWorkspace({
  workspace_id: "ws-codemux-main",
  title: "codemux",
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
  });
  const { surface, tab } = chatSurface("Agent Chat", codemuxRoot);
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
  const { surface, tab } = chatSurface("Agent Chat", cwd, null);
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

// Project 2 — vexis: 1 primary + 3 worktrees (one in `permission`).

const vexisRoot = `${PROJECTS}/vexis`;
const vexisUid = "uid-vexis";

const wsVexisMain = makeWorkspace({
  workspace_id: "ws-vexis-main",
  title: "vexis",
  cwd: vexisRoot,
  project_root: vexisRoot,
  project_uid: vexisUid,
  workspace_kind: "main",
  protected: true,
  git_branch: "main",
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
  pr_url: "https://github.com/example/vexis/pull/88",
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
  pr_url: "https://github.com/example/vexis/pull/90",
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
});

// Project 3 — personal-site: 1 primary + 1 worktree (open PR + diff).

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

const ALL_WORKSPACES: WorkspaceSnapshot[] = [
  wsCodemuxMain,
  wsCodemuxChat,
  wsCodemuxAuth,
  wsCodemuxSidebar,
  wsCodemuxMock,
  wsCodemuxChatLive,
  wsCodemuxPorts,
  wsVexisMain,
  wsVexisCuda,
  wsVexisBench,
  wsVexisInstaller,
  wsSiteMain,
  wsSiteRedesign,
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

/**
 * Build a fresh, deep-ish-cloned `AppStateSnapshot`. The mock holds a
 * single mutable instance and re-emits it after mutating commands, so
 * we hand out a structuredClone to keep the canonical seed pristine
 * across hot reloads / re-installs.
 */
export function createSeedAppState(): AppStateSnapshot {
  return structuredClone({
    schema_version: 1,
    active_workspace_id: wsCodemuxMock.workspace_id,
    workspaces: ALL_WORKSPACES,
    terminal_sessions: terminalSessions,
    browser_sessions: [],
    agent_browser_sessions: [],
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
    persistence: MOCK_PERSISTENCE,
    config: MOCK_CONFIG,
  });
}
