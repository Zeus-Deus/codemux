import { invoke, Channel } from "@tauri-apps/api/core";

import { bytesToBase64 } from "@/lib/agent-chat/attachment-block";
import { isRemoteClient } from "@/components/remote/is-remote-client";

export { Channel };
import type {
  AgentBrowserSession,
  AgentChatProviderKind,
  ModelSelection,
  OpenCodeAvailability,
  OpenCodeProviderEntry,
  ProviderChatCapabilities,
  ProviderHealthReport,
} from "./types";
import type { AgentChatEventPayload, ApprovalDecision } from "./events";
import type {
  UserSettings,
  PresetLaunchConfig,
  AppStateSnapshot,
  AuthResponse,
  AuthUser,
  SessionBootstrap,
  SessionRefresh,
  BaseBranchDiff,
  BranchDetail,
  CheckInfo,
  EditorInfo,
  FileAttachmentInfo,
  FileEntry,
  FileMatch,
  FolderAttachmentInfo,
  FolderMatch,
  GhStatus,
  GitHubIssue,
  GitBranchInfo,
  GitDiffStat,
  GitFileStatus,
  GitLogEntry,
  CommitFileEntry,
  CommitSummary,
  HandoffPacket,
  LaunchMode,
  ProviderDiagnostic,
  ProviderAuthStatus,
  ResourceMetricsSnapshot,
  PresetStoreSnapshot,
  ProjectMemorySnapshot,
  ProjectMemoryUpdate,
  PullRequestInfo,
  IncomingPrItem,
  PrOverviewStats,
  PrsOverview,
  GhRateLimit,
  ReviewComment,
  InlineReviewComment,
  PrReviewThread,
  PrTimelineEvent,
  PrDraftComment,
  MergeState,
  MergeIntoBaseResult,
  ConflictCheckResult,
  ResolverBranchInfo,
  SearchResult,
  ShellAppearance,
  TabKind,
  TerminalStatusPayload,
  ThemeColors,
  WorkspaceConfig,
  WorktreeInfo,
  ProjectScripts,
  DetectedSetup,
  WebRemoteStatus,
  WebRemoteEndpoint,
  WebRemoteSessionView,
  WebRemotePairingInfo,
  WebRemoteBindScope,
  WebRemoteRegistrationStatus,
  NativePerformanceDiagnostics,
} from "./types";

/** Bounded, identifier-free native latency summaries for support reports. */
export const getPerformanceDiagnostics = () =>
  invoke<NativePerformanceDiagnostics>("get_performance_diagnostics");

// ── Project files (Step 8 — attachments) ──

/** Fuzzy-ranked list of project files for the chat composer's `@`
 *  mention popup. Walks `cwd` respecting `.gitignore` (60s in-process
 *  cache). Empty / null `query` returns alphabetical paths. */
export const listProjectFiles = (
  cwd: string,
  query: string | null,
  limit: number,
) =>
  invoke<FileMatch[]>("list_project_files", { cwd, query, limit });

/** Read a file for attachment — full content for small files, first
 *  50 lines + outline for large ones. Used when the user picks a file
 *  in the `@` popup; the resulting body becomes
 *  `attachment.resolvedContent` and is injected at send time. */
export const readFileForAttachment = (
  absolutePath: string,
  cwd: string | null,
) =>
  invoke<FileAttachmentInfo>("read_file_for_attachment", {
    absolutePath,
    cwd,
  });

/** Distinct project folders derived from the same walk cache as
 *  `listProjectFiles`. Used by the `+ → Folder…` picker (Step 8
 *  Stage 3). Empty / null `query` returns alphabetical paths. */
export const listProjectFolders = (
  cwd: string,
  query: string | null,
  limit: number,
) =>
  invoke<FolderMatch[]>("list_project_folders", { cwd, query, limit });

/** Read a folder for attachment — depth-bounded walk that produces a
 *  pre-rendered unicode tree. The agent uses the path to explore via
 *  `Read` / `Grep` once attached. */
export const readFolderForAttachment = (
  absolutePath: string,
  cwd: string | null,
  maxDepth: number,
) =>
  invoke<FolderAttachmentInfo>("read_folder_for_attachment", {
    absolutePath,
    cwd,
    maxDepth,
  });

// ── Auth ──

export const startOauthFlow = () =>
  invoke<void>("start_oauth_flow");

export const signinEmail = (email: string, password: string) =>
  invoke<AuthResponse>("signin_email", { email, password });

export const signupEmail = (email: string, password: string, name: string) =>
  invoke<void>("signup_email", { email, password, name });

export const forgotPassword = (email: string) =>
  invoke<void>("forgot_password", { email });

export const checkAuth = () =>
  invoke<AuthUser | null>("check_auth");

/** Local-only auth/settings bootstrap; never waits on the network. */
export const bootstrapSession = () =>
  invoke<SessionBootstrap>("bootstrap_session");

/** Bounded after-paint remote verification and settings reconciliation. */
export const refreshSession = () =>
  invoke<SessionRefresh>("refresh_session");

export const signOut = () =>
  invoke<void>("sign_out");

export const getAuthToken = () =>
  invoke<string | null>("get_auth_token");

// ── Skills Sync (Step 10) ──
//
// Skills sync is stored server-side (no client-held key), so
// `syncAvailable` is simply "the user is signed in." `authMethod`
// is reported so the UI can tailor copy. There's no password to set
// up and no device-local key to repair.

export interface SyncStatus {
  syncAvailable: boolean;
  authMethod: "email" | "github" | null;
}

/// Read the current sync state. Cheap; no API roundtrip. Use this
/// on Settings → Sync mount.
export const getSyncStatus = () => invoke<SyncStatus>("get_sync_status");

// ── Skills sync engine ──
//
// `skills_sync_now` pulls every skill from /api/skills, writes them
// to ~/.codemux/skills/, then walks every syncable user-scope skill
// path and pushes anything that's changed. Idempotent; safe to call
// back-to-back.
//
// `skills_sync_status` is a cheap status read for UI rendering.
// The engine's state is also broadcast over the Tauri
// `sync-state-changed` event after every cycle.

export interface SkillsSyncResult {
  pushedCount: number;
  pulledCount: number;
  conflictCount: number;
  errorCount: number;
}

/// Discriminated union mirroring Rust's `SyncStateSnapshot`.
/// `state="idle"` carries `lastSyncAtMillis` (number | null);
/// `state="syncing"` carries `startedAtMillis`; `state="error"`
/// carries `lastError` and `atMillis`.
export type SkillsSyncStateSnapshot =
  | { state: "idle"; lastSyncAtMillis: number | null }
  | { state: "syncing"; startedAtMillis: number }
  | { state: "error"; lastError: string; atMillis: number };

export const skillsSyncNow = () =>
  invoke<SkillsSyncResult>("skills_sync_now");

export const skillsSyncStatus = () =>
  invoke<SkillsSyncStateSnapshot>("skills_sync_status");

// ── Skills sync — local export / import ──
//
// `exportSkillsToFile` pulls every synced skill and writes a
// plaintext JSON file at the path the user picked via the OS
// save-dialog.
//
// `importSkillsFromFile` is the inverse: read a JSON backup and
// re-push every skill to the server. Use `mismatchedEmail` to
// surface a soft warning when the backup belongs to a different
// account.

export interface ExportSummary {
  path: string;
  skillCount: number;
  bytesWritten: number;
  failedCount: number;
}

export interface ImportSummary {
  queuedCount: number;
  failedCount: number;
  mismatchedEmail: boolean;
}

export const getExportRecommendedFilename = () =>
  invoke<string>("get_export_recommended_filename");

export const exportSkillsToFile = (filePath: string) =>
  invoke<ExportSummary>("export_skills_to_file", { filePath });

export const importSkillsFromFile = (filePath: string) =>
  invoke<ImportSummary>("import_skills_from_file", { filePath });

// ── Settings Sync ──

export const getSyncedSettings = () =>
  invoke<UserSettings>("get_synced_settings");

export const updateSyncedSettings = (settings: UserSettings) =>
  invoke<UserSettings>("update_synced_settings", { settings });

export const updateSetting = (section: string, key: string, value: unknown) =>
  invoke<UserSettings>("update_setting", { section, key, value });

export const resetSyncedSettings = () =>
  invoke<UserSettings>("reset_synced_settings");

// ── Resource Monitor ──

/** `detail: false` serves a cached summary with fresh host totals, skipping
 *  the process-table walk + per-PID `smaps_rollup` reads. Used by the slow
 *  poll that runs while the monitor popover is closed. */
export const getResourceMetrics = (detail = true) =>
  invoke<ResourceMetricsSnapshot>("get_resource_metrics", { detail });

// ── Core ──

export const getAppState = () =>
  invoke<AppStateSnapshot>("get_app_state");

// ── Workspace ──

export const createTerminalSession = () =>
  invoke<string>("create_terminal_session");

export const activateTerminalSession = (sessionId: string) =>
  invoke("activate_terminal_session", { sessionId });

export const closeTerminalSession = (sessionId: string) =>
  invoke<string>("close_terminal_session", { sessionId });

export const restartTerminalSession = (sessionId: string) =>
  invoke("restart_terminal_session", { sessionId });

export const createWorkspace = (cwd: string | null = null) =>
  invoke<string>("create_workspace", { cwd });

/** Normalized result of a workspace-create command. The backend now
 *  returns the freshly-created workspace's absolute `cwd` alongside its
 *  id so first-send paths can skip the `waitForWorkspaceCwd` poll; older
 *  backends (and other code paths) may still return a bare id string, in
 *  which case `cwd` is `null` and the caller falls back to the poll.
 *
 *  `adopted` is true when no workspace was created: a live workspace
 *  already claimed the target worktree path, so the backend focused it and
 *  returned its id instead — dropping any `initialPrompt`/`agentPresetId`
 *  rather than injecting into the in-flight session. Callers use it to
 *  tell the user their prompt wasn't sent. Missing on older backends,
 *  which never adopt — normalized to `false`. */
export interface WorkspaceCreateResult {
  workspaceId: string;
  cwd: string | null;
  adopted: boolean;
}

/** Coerce the create_* response, which is either the legacy bare
 *  workspace-id string or the additive `{ workspace_id, cwd, adopted }`
 *  object, into a stable {@link WorkspaceCreateResult}. Tolerant of either
 *  shape so the frontend never breaks on the sibling backend's rollout
 *  order. */
function normalizeWorkspaceCreate(raw: unknown): WorkspaceCreateResult {
  if (typeof raw === "string") return { workspaceId: raw, cwd: null, adopted: false };
  const obj = (raw ?? {}) as {
    workspace_id?: string;
    workspaceId?: string;
    cwd?: string | null;
    adopted?: boolean;
  };
  return {
    workspaceId: obj.workspace_id ?? obj.workspaceId ?? "",
    cwd: obj.cwd ?? null,
    adopted: obj.adopted === true,
  };
}

/** Create an empty workspace, returning both its id and (when the
 *  backend supplies it) its resolved cwd. */
export const createEmptyWorkspaceResult = (
  cwd: string,
  opts?: { skipSetup?: boolean },
): Promise<WorkspaceCreateResult> =>
  invoke<unknown>("create_empty_workspace", {
    cwd,
    skipSetup: opts?.skipSetup ?? null,
  }).then(normalizeWorkspaceCreate);

export const createEmptyWorkspace = (
  cwd: string,
  opts?: { skipSetup?: boolean },
): Promise<string> =>
  createEmptyWorkspaceResult(cwd, opts).then((r) => r.workspaceId);

export const getOrCreateHomeWorkspace = () =>
  invoke<string>("get_or_create_home_workspace");

export const listChatProviderCapabilities = (
  provider: AgentChatProviderKind,
) =>
  invoke<ProviderChatCapabilities>("list_chat_provider_capabilities", {
    provider,
  });

/** Probe the health of a chat provider's local runtime (installed /
 *  runnable / authenticated). Infallible on the backend — a failed
 *  probe is folded into the report. May take a few seconds for Claude
 *  (spawns the sidecar), so callers cache via the provider-health
 *  store rather than invoking per render. */
export const agentChatProviderHealth = (provider: AgentChatProviderKind) =>
  invoke<ProviderHealthReport>("agent_chat_provider_health", { provider });

/** Gemini launch-time model list. Backend serves a live harvest when
 *  `GEMINI_API_KEY` is set, otherwise the maintained fallback. Either
 *  way it returns quickly; failure inside the live path is caught by
 *  the backend and returns the maintained list. */
export const listLaunchGeminiModels = () =>
  invoke<Array<{ id: string; label: string }>>("list_launch_gemini_models");

export const regenerateMcpConfig = (workspaceId: string) =>
  invoke<void>("regenerate_mcp_config", { workspaceId });

/** Repair deferred MCP config writes for inactive workspaces. This may touch
 * disk and must be called only after the renderer's useful first paint. */
export const repairInactiveMcpConfigs = () =>
  invoke<number>("repair_inactive_mcp_configs");

export const updateWorkspaceCwd = (workspaceId: string, cwd: string) =>
  invoke("update_workspace_cwd", { workspaceId, cwd });

export const createWorkspaceWithPreset = (cwd: string, presetId: string) =>
  invoke<string>("create_workspace_with_preset", { cwd, presetId });

export const activateWorkspace = (workspaceId: string) =>
  invoke("activate_workspace", { workspaceId });

export const renameWorkspace = (workspaceId: string, title: string) =>
  invoke("rename_workspace", { workspaceId, title });

export const setWorkspaceMuted = (workspaceId: string, muted: boolean) =>
  invoke("set_workspace_muted", { workspaceId, muted });

export const setWorkspacePinned = (workspaceId: string, pinned: boolean) =>
  invoke<void>("set_workspace_pinned", { workspaceId, pinned });

export const closeWorkspace = (workspaceId: string, forceDelete: boolean) =>
  invoke<string>("close_workspace", { workspaceId, forceDelete });

export const cycleWorkspace = (step: number) =>
  invoke<string>("cycle_workspace", { step });

export const splitPane = (paneId: string, direction: "horizontal" | "vertical") =>
  invoke<string>("split_pane", { paneId, direction });

export const activatePane = (paneId: string) =>
  invoke("activate_pane", { paneId });

export const cyclePane = (step: number) =>
  invoke<string>("cycle_pane", { step });

export const closePane = (paneId: string) =>
  invoke<string | null>("close_pane", { paneId });

export const swapPanes = (sourcePaneId: string, targetPaneId: string) =>
  invoke("swap_panes", { sourcePaneId, targetPaneId });

export const resizeSplit = (paneId: string, childSizes: number[]) =>
  invoke("resize_split", { paneId, childSizes });

export const resizeActivePane = (delta: number) =>
  invoke("resize_active_pane", { delta });

export const notifyAttention = (message: string, sessionId: string, paneId: string) =>
  invoke<string>("notify_attention", { message, sessionId, paneId });

export const setNotificationSoundEnabled = (enabled: boolean) =>
  invoke("set_notification_sound_enabled", { enabled });

export const createBrowserPane = (paneId: string, url?: string) =>
  invoke<string>("create_browser_pane", { paneId, url: url ?? null });

/** Host this workspace's agent browser session in the right-panel deck.
 *  Returns the session so the caller can mount `BrowserPane` against the
 *  canonical `cli_session_name` — the same daemon key the `codemux browser`
 *  CLI and MCP tools resolve to. Adopts (and closes) an existing main-area
 *  browser pane for the session rather than showing it twice. */
export const dockBrowserInRightPanel = (workspaceId: string) =>
  invoke<AgentBrowserSession>("dock_browser_in_right_panel", { workspaceId });

/** Remove the workspace's agent browser session from the right-panel deck.
 *  `dismissed` marks an explicit user close (the tab's ×), which stops the
 *  agent re-surfacing the browser on its next command. */
export const undockBrowserFromRightPanel = (
  workspaceId: string,
  dismissed: boolean,
) => invoke("undock_browser_from_right_panel", { workspaceId, dismissed });

export const createTab = (workspaceId: string, kind: TabKind) =>
  invoke<string>("create_tab", { workspaceId, kind });

export const closeTab = (workspaceId: string, tabId: string) =>
  invoke("close_tab", { workspaceId, tabId });

export const activateTab = (workspaceId: string, tabId: string) =>
  invoke("activate_tab", { workspaceId, tabId });

export const renameTab = (workspaceId: string, tabId: string, title: string) =>
  invoke("rename_tab", { workspaceId, tabId, title });

export const reorderTabs = (workspaceId: string, tabIds: string[]) =>
  invoke("reorder_tabs", { workspaceId, tabIds });

export const killPort = (port: number) =>
  invoke("kill_port", { port });

export const detectEditors = () =>
  invoke<EditorInfo[]>("detect_editors");

export const openInEditor = (editorId: string, path: string) =>
  invoke<void>("open_in_editor", { editorId, path });

/** Create a worktree workspace, returning both its id and (when the
 *  backend supplies it) its resolved cwd — the deferred-worktree
 *  first-send path uses the cwd to skip the `waitForWorkspaceCwd` poll. */
export const createWorktreeWorkspaceResult = (
  repoPath: string,
  branch: string,
  newBranch: boolean,
  layout: string,
  base?: string | null,
  initialPrompt?: string | null,
  agentPresetId?: string | null,
  prNumber?: number | null,
  modelSelection?: ModelSelection | null,
): Promise<WorkspaceCreateResult> =>
  invoke<unknown>("create_worktree_workspace", {
    repoPath,
    branch,
    newBranch,
    base: base ?? null,
    layout,
    initialPrompt: initialPrompt ?? null,
    agentPresetId: agentPresetId ?? null,
    modelSelection: modelSelection ?? null,
    prNumber: prNumber ?? null,
  }).then(normalizeWorkspaceCreate);

export const createWorktreeWorkspace = (
  repoPath: string,
  branch: string,
  newBranch: boolean,
  layout: string,
  base?: string | null,
  initialPrompt?: string | null,
  agentPresetId?: string | null,
  prNumber?: number | null,
  modelSelection?: ModelSelection | null,
): Promise<string> =>
  createWorktreeWorkspaceResult(
    repoPath,
    branch,
    newBranch,
    layout,
    base,
    initialPrompt,
    agentPresetId,
    prNumber,
    modelSelection,
  ).then((r) => r.workspaceId);

export const generateBranchName = (prompt: string, projectPath: string) =>
  invoke<string>("generate_branch_name", { prompt, projectPath });

export const generateRandomBranchName = (projectPath: string) =>
  invoke<string>("generate_random_branch_name", { projectPath });

export const importWorktreeWorkspace = (
  worktreePath: string,
  branch: string,
  layout: string,
) =>
  invoke<string>("import_worktree_workspace", { worktreePath, branch, layout });

export const closeWorkspaceWithWorktree = (
  workspaceId: string,
  removeWorktree: boolean,
  deleteBranch: boolean,
  forceDelete: boolean,
) =>
  invoke<void>("close_workspace_with_worktree", { workspaceId, removeWorktree, deleteBranch, forceDelete });

// ── Workspace archive ──
//
// Archiving removes a workspace from the sidebar and records an entry in
// `archived_workspaces`; the files, branch, and worktree on disk are left
// untouched. Works for every workspace kind, including the protected repo
// root. Rejects with a message for attach-in-place workspaces.

/** Archive a workspace. Resolves with the new archive entry's id, which
 *  `unarchiveWorkspace` accepts to restore it. */
export const archiveWorkspace = (workspaceId: string) =>
  invoke<string>("archive_workspace", { workspaceId });

/** Restore an archived workspace. Resolves with the restored workspace id;
 *  the backend also activates it. Rejects (entry kept) when nothing is
 *  left on disk to restore. */
export const unarchiveWorkspace = (archiveId: string) =>
  invoke<string>("unarchive_workspace", { archiveId });

/** Permanently drop an archive entry, optionally deleting the worktree
 *  (and branch) from disk. Protected/root entries refuse
 *  `deleteWorktree`; a dirty worktree with `forceDelete=false` rejects
 *  with a message matching /use force/i so the UI can escalate. */
export const deleteArchivedWorkspace = (
  archiveId: string,
  deleteWorktree: boolean,
  deleteBranch: boolean,
  forceDelete: boolean,
) =>
  invoke<void>("delete_archived_workspace", {
    archiveId,
    deleteWorktree,
    deleteBranch,
    forceDelete,
  });

export const getWorkspaceConfig = (path: string) =>
  invoke<WorkspaceConfig | null>("get_workspace_config", { path });

export const hasCodemuxinclude = (path: string) =>
  invoke<boolean>("has_codemuxinclude", { path });

export const runWorkspaceSetup = (workspaceId: string) =>
  invoke<void>("run_workspace_setup", { workspaceId });

export const getProjectScripts = (path: string) =>
  invoke<ProjectScripts | null>("get_project_scripts", { path });

export const setProjectScripts = (path: string, scripts: ProjectScripts) =>
  invoke<void>("set_project_scripts", { path, scripts });

export const runProjectDevCommand = (workspaceId: string, forceNew?: boolean) =>
  invoke<void>("run_project_dev_command", { workspaceId, forceNew: forceNew ?? false });

export const detectPackageManager = (projectPath: string) =>
  invoke<DetectedSetup[]>("detect_package_manager", { projectPath });

export const reorderWorkspaces = (workspaceIds: string[]) =>
  invoke("reorder_workspaces", { workspaceIds });

// Switch a primary workspace's repo to its default branch. Returns the
// branch name on success. Rejects with git's stderr when the checkout is
// refused (dirty conflicts, rebase in progress, etc.) so the caller can
// toast the message verbatim. The backend refreshes git info synchronously
// before returning, so the sidebar label updates without waiting for the
// 5s polling tick.
export const checkoutDefaultBranchInWorkspace = (workspaceId: string) =>
  invoke<string>("checkout_default_branch_in_workspace", { workspaceId });

// ── GitHub ──

export const checkGhStatus = () =>
  invoke<GhStatus>("check_gh_status");

export const checkGhAvailable = () =>
  invoke<boolean>("check_gh_available");

export const checkGithubRepo = (path: string) =>
  invoke<boolean>("check_github_repo", { path });

/** Probe every hosting product's CLI for the Settings → Source Control
 *  pane. Never rejects: a wedged or missing CLI shows up as a
 *  not-installed row rather than an error. */
export const discoverSourceControl = () =>
  invoke<ProviderDiagnostic[]>("discover_source_control");

/** Host-scoped readiness for one checkout: which product serves it, and
 *  whether that product's CLI is installed and signed in *to that
 *  instance*. Never rejects — a wedged probe answers "nothing usable".
 *  Prefer `fetchProviderAuth` in `@/lib/provider-auth`, which adds the
 *  short-TTL cache every gate shares. */
export const checkProviderAuth = (path: string) =>
  invoke<ProviderAuthStatus>("check_provider_auth", { path });

export const getBranchPullRequest = (path: string) =>
  invoke<PullRequestInfo | null>("get_branch_pull_request", { path });

export const createPullRequest = (
  path: string,
  title: string,
  body: string,
  base: string,
  draft: boolean,
) =>
  invoke<PullRequestInfo>("create_pull_request", { path, title, body, base, draft });

export const listPullRequests = (path: string, state: string) =>
  invoke<PullRequestInfo[]>("list_pull_requests", { path, state });

/** Stage 5 — single PR detail (body + first 20 comments) by repo
 *  path. Cached: 5 min TTL. Path-based so the chat composer can
 *  call before a workspace exists. */
export const getGithubPrByPath = (path: string, prNumber: number) =>
  invoke<PullRequestInfo>("get_github_pr_by_path", { path, prNumber });

/** Stage 5 — PR diff body. `full=false` → `--name-only` (cheap,
 *  fits in a chip preview); `full=true` → unified diff capped at
 *  100 KB. Cached separately per (number, full). */
export const getGithubPrDiffByPath = (
  path: string,
  prNumber: number,
  full: boolean,
) =>
  invoke<string>("get_github_pr_diff_by_path", { path, prNumber, full });

export const listIncomingPrs = (path: string, baseBranch: string) =>
  invoke<IncomingPrItem[]>("list_incoming_prs", { path, baseBranch });

/** Every open pull request in one repository, with the viewer relation
 *  and the grouping fields — the Pull Requests page's row data, and the
 *  call whose latency the user watches. One per project root; the page
 *  fans out across the roots the user has open and merges the answers.
 *
 *  Deliberately without the CI rollup and the line counts: see
 *  `listPrsOverviewStats`. */
export const listPrsOverview = (path: string) =>
  invoke<PrsOverview>("list_prs_overview", { path });

/** The expensive half: CI rollup and line counts by pull request number.
 *  Fired behind the listing, and only for roots whose rows came back
 *  with `checks: null`. */
export const listPrsOverviewStats = (path: string) =>
  invoke<PrOverviewStats[]>("list_prs_overview_stats", { path });

/** What is left of the GitHub budget, and when it refills.
 *
 *  Asked only after a call has come back refused for exceeding it —
 *  GitHub does not charge for this endpoint, so it is the one request
 *  still safe to make when there is nothing left to spend. `path` is
 *  only a working directory for `gh`: the reply is the budget of the
 *  host `gh` treats as its default, and the gate it feeds is one gate
 *  for every root `gh` serves. */
export const githubRateLimit = (path: string) =>
  invoke<GhRateLimit>("github_rate_limit", { path });

/** Merge a PR. `deleteBranch` defaults to true on the backend, matching
 *  the behaviour before the merge sheet made it a question. */
export const mergePullRequest = (
  path: string,
  prNumber: number,
  method: string,
  deleteBranch?: boolean,
  commitTitle?: string | null,
  commitBody?: string | null,
) =>
  invoke("merge_pull_request", {
    path,
    prNumber,
    method,
    deleteBranch,
    commitTitle,
    commitBody,
  });

export const closePullRequest = (path: string, prNumber: number) =>
  invoke("close_pull_request", { path, prNumber });

export const reopenPullRequest = (path: string, prNumber: number) =>
  invoke("reopen_pull_request", { path, prNumber });

/** Flip draft ↔ ready-for-review. */
export const setPrReady = (path: string, prNumber: number, ready: boolean) =>
  invoke("set_pr_ready", { path, prNumber, ready });

/** Edit title and/or body; an omitted field is left untouched. */
export const updatePullRequest = (
  path: string,
  prNumber: number,
  title: string | null,
  body: string | null,
) => invoke("update_pull_request", { path, prNumber, title, body });

export const requestPrReview = (path: string, prNumber: number, reviewer: string) =>
  invoke("request_pr_review", { path, prNumber, reviewer });

/** Best-effort log tail for a failing check. Empty string means "no
 *  excerpt available" — the failing-check card renders without one. */
export const getCheckLogExcerpt = (path: string, prNumber: number, checkName: string) =>
  invoke<string>("get_check_log_excerpt", { path, prNumber, checkName });

/** Checks for a pull request. Omit `prNumber` for "whatever PR this
 *  checkout's branch has" — the panel's case; the page passes the
 *  selected PR, which is usually not the checked-out one. */
export const getPullRequestChecks = (path: string, prNumber?: number) =>
  invoke<CheckInfo[]>("get_pull_request_checks", { path, prNumber });

/** Conversation-level reviews. Omit `prNumber` for the current
 *  branch's PR. */
export const getPrReviewComments = (path: string, prNumber?: number) =>
  invoke<ReviewComment[]>("get_pr_review_comments", { path, prNumber });

export const getPrInlineComments = (path: string, prNumber: number) =>
  invoke<InlineReviewComment[]>("get_pr_inline_comments", { path, prNumber });

/** Conversation threads with their resolution state.
 *
 *  Not the same call as `getPrInlineComments`: that one is a flat list of
 *  comments with no thread identity and no resolution state, which is why
 *  both exist — the flat list is what the review *summaries* are grouped
 *  from, and this is what "still open" is read from. */
export const getPrReviewThreads = (path: string, prNumber: number) =>
  invoke<PrReviewThread[]>("get_pr_review_threads", { path, prNumber });

/** Reply into a thread.
 *
 *  Both ids travel because the two hosts address the same act
 *  differently: GitLab replies to the discussion, GitHub to the thread's
 *  first comment (`database_id` on that comment). */
export const replyToPrThread = (
  path: string,
  prNumber: number,
  threadId: string,
  rootCommentId: number | null,
  body: string,
) => invoke("reply_to_pr_thread", { path, prNumber, threadId, rootCommentId, body });

/** Resolve (`true`) or unresolve (`false`) — one command both ways,
 *  because the button is one button whose label flips. */
export const setPrThreadResolved = (
  path: string,
  prNumber: number,
  threadId: string,
  resolved: boolean,
) => invoke("set_pr_thread_resolved", { path, prNumber, threadId, resolved });

export const submitPrReview = (path: string, prNumber: number, event: string, body: string) =>
  invoke("submit_pr_review", { path, prNumber, event, body });

/** The whole unified diff, for the Code tab. Uncapped where
 *  `getGithubPrDiffByPath` is capped at 100 KB, and uncached on the
 *  backend so a force-push shows the new patch immediately. */
export const getPrReviewDiff = (path: string, prNumber: number) =>
  invoke<string>("get_pr_review_diff", { path, prNumber });

/** The host's own history of a pull request, oldest first.
 *
 *  Carries neither the "opened" row nor the checks row: the first is
 *  synthesized from the PR the caller already holds, and checks are not a
 *  timeline event at any host — they are the live checks query. */
export const getPrTimeline = (path: string, prNumber: number) =>
  invoke<PrTimelineEvent[]>("get_pr_timeline", { path, prNumber });

/** Post one line comment now. `commitId` must be the head the rendered
 *  diff came from — a stale one pins the comment to a superseded commit
 *  without erroring. */
export const addPrInlineComment = (
  path: string,
  prNumber: number,
  comment: PrDraftComment,
  commitId: string,
) => invoke("add_pr_inline_comment", { path, prNumber, comment, commitId });

/** Verdict, body and every pending line note in a single request. */
export const submitPrReviewWithComments = (
  path: string,
  prNumber: number,
  event: string,
  body: string,
  comments: PrDraftComment[],
  commitId: string,
) =>
  invoke("submit_pr_review_with_comments", {
    path,
    prNumber,
    event,
    body,
    comments,
    commitId,
  });

// ── GitHub Issues ──

export const listGithubIssues = (workspaceId: string, search?: string) =>
  invoke<GitHubIssue[]>("list_github_issues", { workspaceId, search });

export const listGithubIssuesByPath = (path: string, search?: string) =>
  invoke<GitHubIssue[]>("list_github_issues_by_path", { path, search });

export const getGithubIssueByPath = (path: string, issueNumber: number) =>
  invoke<GitHubIssue>("get_github_issue_by_path", { path, issueNumber });

export const getGithubIssue = (workspaceId: string, issueNumber: number) =>
  invoke<GitHubIssue>("get_github_issue", { workspaceId, issueNumber });

export const linkWorkspaceIssue = (workspaceId: string, issueNumber: number) =>
  invoke("link_workspace_issue", { workspaceId, issueNumber });

export const unlinkWorkspaceIssue = (workspaceId: string) =>
  invoke("unlink_workspace_issue", { workspaceId });

export const refreshWorkspaceIssue = (workspaceId: string) =>
  invoke("refresh_workspace_issue", { workspaceId });

export const refreshWorkspacePr = (workspaceId: string) =>
  invoke("refresh_workspace_pr", { workspaceId });

export const suggestIssueBranchName = (issueNumber: number, issueTitle: string) =>
  invoke<string>("suggest_issue_branch_name", { issueNumber, issueTitle });

// ── Database ──

export const dbGetSetting = (key: string) =>
  invoke<string | null>("db_get_setting", { key });

export const dbSetSetting = (key: string, value: string) =>
  invoke("db_set_setting", { key, value });

export const dbDeleteSetting = (key: string) =>
  invoke("db_delete_setting", { key });

export const dbGetAllSettings = () =>
  invoke<Record<string, string>>("db_get_all_settings");

export const dbGetUiState = (key: string) =>
  invoke<string | null>("db_get_ui_state", { key });

export const dbSetUiState = (key: string, value: string) =>
  invoke("db_set_ui_state", { key, value });

export const dbAddRecentProject = (path: string, name: string) =>
  invoke("db_add_recent_project", { path, name });

export const dbGetRecentProjects = (limit?: number) =>
  invoke<Array<{ path: string; name: string; last_opened_at: string }>>("db_get_recent_projects", { limit });

// ── Git ──

export const checkIsGitRepo = (path: string) =>
  invoke<boolean>("check_is_git_repo", { path });

export const initGitRepo = (path: string) =>
  invoke<string>("init_git_repo", { path });

/** No-stage/no-commit variant behind the "Initialize Git" affordance —
 *  runs a bare `git init` only, never staging or committing the user's
 *  existing files (contrast `initGitRepo`, which also adds + makes an
 *  initial commit for the new-empty-project flow). */
export const initGitRepoNoCommit = (path: string) =>
  invoke<string>("init_git_repo_no_commit", { path });

/** Re-gather branch / ahead-behind / diff counts (and the `is_git` flag)
 *  for one workspace and emit fresh app state — used right after
 *  "Initialize Git" so the UI flips without waiting for the poll loop. */
export const refreshWorkspaceGitInfo = (workspaceId: string) =>
  invoke("refresh_workspace_git_info", { workspaceId });

export const gitCloneRepo = (url: string, targetDir: string) =>
  invoke<string>("git_clone_repo", { url, targetDir });

// Live `git clone --progress` updates stream over the `git-clone-progress`
// Tauri event — the payload type and the typed `onGitCloneProgress`
// subscriber live in the event registry. Re-exported here so callers that
// already import from `@/tauri/commands` can reach both.
export {
  GIT_CLONE_PROGRESS_EVENT,
  onGitCloneProgress,
  type GitCloneProgress,
} from "./events";

export const createEmptyRepo = (parentDir: string, name: string) =>
  invoke<string>("create_empty_repo", { parentDir, name });

export const getGitStatus = (path: string) =>
  invoke<GitFileStatus[]>("get_git_status", { path });

export const getGitDiff = (path: string, file: string, staged: boolean) =>
  invoke<string>("get_git_diff", { path, file, staged });

export const getGitDiffStat = (path: string) =>
  invoke<GitDiffStat>("get_git_diff_stat", { path });

export const getBaseBranchDiff = (path: string, baseBranch: string) =>
  invoke<BaseBranchDiff>("get_base_branch_diff", { path, baseBranch });

export const getBaseBranchFileDiff = (path: string, baseBranch: string, file: string) =>
  invoke<string>("get_base_branch_file_diff", { path, baseBranch, file });

export const getDefaultBranch = (path: string) =>
  invoke<string>("get_default_branch", { path });

export const gitStageFiles = (path: string, files: string[]) =>
  invoke("git_stage_files", { path, files });

export const gitUnstageFiles = (path: string, files: string[]) =>
  invoke("git_unstage_files", { path, files });

export const gitCommitChanges = (path: string, message: string) =>
  invoke("git_commit_changes", { path, message });

export const gitPushChanges = (path: string, setUpstream: boolean = false) =>
  invoke("git_push_changes", { path, setUpstream });

export const getGitBranchInfo = (path: string) =>
  invoke<GitBranchInfo>("get_git_branch_info", { path });

export const gitPullChanges = (path: string) =>
  invoke("git_pull_changes", { path });

export const gitFetchChanges = (path: string) =>
  invoke("git_fetch_changes", { path });

export const gitFetchPrune = (path: string) =>
  invoke("git_fetch_prune", { path });

export const gitStashPush = (path: string, includeUntracked: boolean) =>
  invoke("git_stash_push", { path, includeUntracked });

export const gitStashPop = (path: string) =>
  invoke("git_stash_pop", { path });

export const gitAmendCommit = (path: string, message: string | null = null) =>
  invoke("git_amend_commit", { path, message });

export const gitUndoLastCommit = (path: string) =>
  invoke("git_undo_last_commit", { path });

export const gitDiscardFile = (path: string, file: string) =>
  invoke("git_discard_file", { path, file });

export const gitLogEntries = (path: string, count: number) =>
  invoke<GitLogEntry[]>("git_log_entries", { path, count });

/** Commits this branch has that `base` does not, newest first, with
 *  bodies — what the create-pull-request form drafts its title and
 *  description from. Capped by `limit`. */
export const gitCommitsAhead = (path: string, base: string, limit: number) =>
  invoke<CommitSummary[]>("git_commits_ahead", { path, base, limit });

export const getCommitFiles = (path: string, hash: string) =>
  invoke<CommitFileEntry[]>("get_commit_files", { path, hash });

export const listBranches = (path: string, remote: boolean) =>
  invoke<string[]>("list_branches", { path, remote });

export const listBranchesDetailed = (path: string) =>
  invoke<BranchDetail[]>("list_branches_detailed", { path });

export const listWorktrees = (path: string) =>
  invoke<WorktreeInfo[]>("list_worktrees", { path });

// ── Merge Conflicts ──

export const mergeBranch = (path: string, sourceBranch: string) =>
  invoke<string>("merge_branch", { path, sourceBranch });

export const mergeIntoBase = (path: string, baseBranch: string, deleteSourceBranch: boolean) =>
  invoke<MergeIntoBaseResult>("merge_into_base", { path, baseBranch, deleteSourceBranch });

export const completeMergeIntoBase = (
  path: string, baseBranch: string, tempBranch: string, sourceBranch: string, deleteSourceBranch: boolean,
) => invoke("complete_merge_into_base", { path, baseBranch, tempBranch, sourceBranch, deleteSourceBranch });

export const abortMergeIntoBase = (path: string, sourceBranch: string, tempBranch: string) =>
  invoke("abort_merge_into_base", { path, sourceBranch, tempBranch });

export const getMergeState = (path: string) =>
  invoke<MergeState>("get_merge_state", { path });

export const checkMergeConflicts = (path: string, targetBranch: string) =>
  invoke<ConflictCheckResult>("check_merge_conflicts", { path, targetBranch });

export const resolveConflictOurs = (path: string, file: string) =>
  invoke("resolve_conflict_ours", { path, file });

export const resolveConflictTheirs = (path: string, file: string) =>
  invoke("resolve_conflict_theirs", { path, file });

export const markConflictResolved = (path: string, file: string) =>
  invoke("mark_conflict_resolved", { path, file });

export const abortMerge = (path: string) =>
  invoke("abort_merge", { path });

export const continueMerge = (path: string, message: string) =>
  invoke("continue_merge", { path, message });

// ── Resolver Branches ──

export const createResolverBranch = (path: string, targetBranch: string) =>
  invoke<ResolverBranchInfo>("create_resolver_branch", { path, targetBranch });

export const applyResolution = (path: string, tempBranch: string, originalBranch: string, message: string) =>
  invoke("apply_resolution", { path, tempBranch, originalBranch, message });

export const abortResolution = (path: string, tempBranch: string, originalBranch: string) =>
  invoke("abort_resolution", { path, tempBranch, originalBranch });

export const getResolutionDiff = (path: string) =>
  invoke<string>("get_resolution_diff", { path });

export const resolveConflictsWithAgent = (
  path: string, cli: string, model: string | null, strategy: string, files: string[]
) =>
  invoke<string>("resolve_conflicts_with_agent", { path, cli, model, strategy, files });

// ── AI ──

export const checkClaudeAvailable = () =>
  invoke<boolean>("check_claude_available");

export const generateAiCommitMessage = (
  path: string,
  cli: string | null = null,
  model: string | null = null,
) => invoke<string>("generate_ai_commit_message", { path, cli, model });

export const setAiCommitMessageEnabled = (enabled: boolean) =>
  invoke("set_ai_commit_message_enabled", { enabled });

export const setAiCommitMessageCli = (cli: string | null) =>
  invoke("set_ai_commit_message_cli", { cli });

export const setAiCommitMessageModel = (model: string | null) =>
  invoke("set_ai_commit_message_model", { model });

export const setAiResolverEnabled = (enabled: boolean) =>
  invoke("set_ai_resolver_enabled", { enabled });

export const setAiResolverCli = (cli: string | null) =>
  invoke("set_ai_resolver_cli", { cli });

export const setAiResolverModel = (model: string | null) =>
  invoke("set_ai_resolver_model", { model });

export const setAiResolverStrategy = (strategy: string) =>
  invoke("set_ai_resolver_strategy", { strategy });

// ── Browser ──

export const browserOpenUrl = (browserId: string, url: string) =>
  invoke("browser_open_url", { browserId, url });

export const browserHistoryBack = (browserId: string) =>
  invoke("browser_history_back", { browserId });

export const browserHistoryForward = (browserId: string) =>
  invoke("browser_history_forward", { browserId });

export const browserReload = (browserId: string) =>
  invoke("browser_reload", { browserId });

export const browserSetLoadingState = (
  browserId: string,
  isLoading: boolean,
  error: string | null,
) =>
  invoke("browser_set_loading_state", { browserId, isLoading, error });

export const agentBrowserSpawn = (browserId: string) =>
  invoke("agent_browser_spawn", { browserId });

export const agentBrowserRun = (browserId: string, action: string, params: unknown) =>
  invoke("agent_browser_run", { browserId, action, params });

export const agentBrowserClose = (browserId: string) =>
  invoke("agent_browser_close", { browserId });

export const agentBrowserScreenshot = (browserId: string) =>
  invoke<string>("agent_browser_screenshot", { browserId });

export const startBrowserStream = (browserId: string) =>
  invoke<string>("start_browser_stream", { browserId });

// ── Browser Data ──

export const getBrowserDataSize = () =>
  invoke<number>("get_browser_data_size");

export const clearBrowserCookies = () =>
  invoke<void>("clear_browser_cookies");

export const clearAllBrowserData = () =>
  invoke<void>("clear_all_browser_data");

// ── Memory ──

export const getProjectMemorySnapshot = () =>
  invoke<ProjectMemorySnapshot>("get_project_memory_snapshot");

export const updateProjectMemorySnapshot = (update: ProjectMemoryUpdate) =>
  invoke<ProjectMemorySnapshot>("update_project_memory_snapshot", { update });

export const addProjectMemoryEntry = (
  kind: string,
  content: string,
  tags: string[],
  toolName: string | null,
  sessionLabel: string | null,
) =>
  invoke<ProjectMemorySnapshot>("add_project_memory_entry", {
    kind,
    content,
    tags,
    toolName,
    sessionLabel,
  });

export const generateProjectHandoff = () =>
  invoke<HandoffPacket>("generate_project_handoff");

// ── Theme ──

export const getCurrentTheme = () =>
  invoke<ThemeColors>("get_current_theme");

export const getShellAppearance = () =>
  invoke<ShellAppearance>("get_shell_appearance");

// ── Presets ──

export const getPresets = () =>
  invoke<PresetStoreSnapshot>("get_presets");

export const createPreset = (params: {
  name: string;
  description: string | null;
  commands: string[];
  workingDirectory: string | null;
  launchMode: LaunchMode;
  pinned: boolean;
  icon: string | null;
  /** Structured agent-launcher source, or null for raw presets. */
  launchConfig?: PresetLaunchConfig | null;
}) =>
  invoke<string>("create_preset", params);

export const updatePreset = (params: {
  id: string;
  name: string;
  description: string | null;
  commands: string[];
  workingDirectory: string | null;
  launchMode: LaunchMode;
  icon: string | null;
  autoRunOnWorkspace?: boolean;
  autoRunOnNewTab?: boolean;
  /** Set the structured agent-launcher config (structured save). */
  launchConfig?: PresetLaunchConfig | null;
  /** Remove the structured config (switching to a raw command preset). */
  clearLaunchConfig?: boolean;
}) =>
  invoke("update_preset", params);

export const deletePreset = (id: string) =>
  invoke("delete_preset", { id });

export const setPresetPinned = (id: string, pinned: boolean) =>
  invoke("set_preset_pinned", { id, pinned });

export const applyPreset = (
  workspaceId: string,
  presetId: string,
  overrideMode?: LaunchMode | "current_terminal" | "existing_panes",
  initialPrompt?: string | null,
  modelSelection?: ModelSelection | null,
) =>
  invoke("apply_preset", {
    workspaceId,
    presetId,
    overrideMode,
    initialPrompt,
    modelSelection: modelSelection ?? null,
  });

export const setPresetBarVisible = (visible: boolean) =>
  invoke("set_preset_bar_visible", { visible });

export const reorderPresets = (presetId: string, targetIndex: number) =>
  invoke("reorder_presets", { presetId, targetIndex });

// ── Terminal PTY ──

export const writeToPty = (sessionId: string, data: string) =>
  invoke("write_to_pty", { data, sessionId });

export const resizePty = (sessionId: string, cols: number, rows: number) =>
  invoke("resize_pty", { cols, rows, sessionId });

export const clearAgentStatus = (sessionId: string) =>
  invoke("clear_agent_status", { sessionId });

/** Remove the output subscriber installed by `attachPtyOutput`. `generation`
 *  is the token that attach returned, so a late detach only ever tears down
 *  its own subscriber — never a sibling consumer that attached the same
 *  session concurrently. Omitting it removes every subscriber for the session
 *  (legacy whole-session detach). */
export const detachPtyOutput = (sessionId: string, generation?: number) =>
  invoke("detach_pty_output", { sessionId, generation });

/** Terminal output flow control: signal that this subscriber (`generation`)
 *  is backed up by a fast producer. The backend only stops the PTY read loop
 *  (applying real back-pressure to the child) once EVERY subscriber has
 *  paused, so a caught-up mirror consumer keeps the stream flowing. Omitting
 *  `generation` pauses the whole session (legacy). See `pause_pty_output` in
 *  terminal/mod.rs. */
export const pausePtyOutput = (sessionId: string, generation?: number) =>
  invoke("pause_pty_output", { sessionId, generation });

/** Clear this subscriber's back-pressure request once its write queue has
 *  drained. Omitting `generation` resumes the whole session. See
 *  `pausePtyOutput`. */
export const resumePtyOutput = (sessionId: string, generation?: number) =>
  invoke("resume_pty_output", { sessionId, generation });

/** Resolve the live working directory of each session's shell by reading
 *  `/proc/<pid>/cwd`. The fallback source for the terminal pane header's
 *  cwd hint, used for sessions whose shell doesn't emit OSC 7.
 *
 *  Sessions absent from the response are unknown to this source (remote/SSH
 *  panes, non-Linux hosts, exited shells) — callers should keep whatever
 *  they already had rather than clearing. See `terminal_session_cwds`. */
export const terminalSessionCwds = (sessionIds: string[]) =>
  invoke<Record<string, string>>("terminal_session_cwds", { sessionIds });

/** Attach an output subscriber to a session's PTY stream. Resolves with the
 *  subscriber generation token, which must be handed to `detachPtyOutput` /
 *  `pausePtyOutput` / `resumePtyOutput`. The real desktop backend, the
 *  web-remote dispatch, and the dev mock all mint a real generation now, but
 *  the return type stays `number | undefined` because callers (and a
 *  terminal-cache test outside this lane) still model the undefined case;
 *  they treat a missing generation as "not a real subscriber" and skip
 *  generation-scoped teardown. */
export const attachPtyOutput = (
  sessionId: string,
  channel: Channel<unknown>,
  skipPending?: boolean,
) =>
  invoke<number | undefined>("attach_pty_output", {
    channel,
    sessionId,
    skipPending,
  });

export const getTerminalStatus = (sessionId: string) =>
  invoke<TerminalStatusPayload>("get_terminal_status", { sessionId });

// ── Session Restore ──

export interface ScrollbackPayload {
  pane_id: string;
  session_id: string;
  workspace_id: string;
  working_directory: string;
  original_command: string | null;
  cols: number;
  rows: number;
  data: string;
  adapter_captures: Record<string, string>;
  adapter_id: string | null;
  alternate_buffer: boolean;
}

export interface ScrollbackMeta {
  pane_id: string;
  session_id: string;
  workspace_id: string;
  working_directory: string;
  original_command: string | null;
  cols: number;
  rows: number;
  adapter_captures: Record<string, string>;
  adapter_id: string | null;
  alternate_buffer: boolean;
  saved_at: number;
}

export interface ScrollbackRestore {
  data: string;
  meta: ScrollbackMeta;
}

export const saveTerminalScrollback = (payload: ScrollbackPayload) =>
  invoke("save_terminal_scrollback", { payload });

export const getTerminalScrollback = (workspaceId: string, paneId: string) =>
  invoke<ScrollbackRestore | null>("get_terminal_scrollback", { workspaceId, paneId });

export const cacheTerminalScrollback = (payload: ScrollbackPayload) =>
  invoke("cache_terminal_scrollback", { payload });

export const uncacheTerminalScrollback = (sessionId: string) =>
  invoke("uncache_terminal_scrollback", { sessionId });

export const flushScrollbackCache = () =>
  invoke<number>("flush_scrollback_cache");

// ── Debug ──

export const debugLog = (message: string) =>
  invoke("debug_log", { message }).catch(() => {});

// ── Search / Files ──

export const searchInFiles = (
  path: string,
  query: string,
  regex: boolean,
  caseSensitive: boolean,
  maxResults: number,
) =>
  invoke<SearchResult[]>("search_in_files", { path, query, regex, caseSensitive, maxResults });

export const searchFileNames = (path: string, query: string, maxResults: number) =>
  invoke<string[]>("search_file_names", { path, query, maxResults });

export const revealInFileManager = (path: string) =>
  invoke<void>("reveal_in_file_manager", { path });

export const listDirectory = (path: string, showHidden?: boolean) =>
  invoke<FileEntry[]>("list_directory", { path, showHidden });

/** Stat probe used by chat file links before opening a doc tab, so a
 *  path the agent merely guessed never opens a dead viewer. */
export const fileExists = (path: string) =>
  invoke<boolean>("file_exists", { path });

export const readFile = (path: string) =>
  invoke<string>("read_file", { path });

export const writeFile = (path: string, content: string) =>
  invoke<void>("write_file", { path, content });

export const grepCountPattern = (cwd: string, pattern: string) =>
  invoke<number>("grep_count_pattern", { cwd, pattern });

/// Persist a caller-supplied clipboard image payload to a temp file
/// and return the absolute path on disk.
///
/// Used by call sites that already have encoded image bytes in hand.
/// For the paste flow itself, prefer `pasteClipboardImageToFile`
/// which reads the OS clipboard server-side and avoids shipping the
/// bytes through JS twice.
export const saveClipboardImageBytes = (
  bytes: Uint8Array,
  mime: string,
): Promise<string> =>
  invoke<string>("save_clipboard_image_bytes", {
    bytes: Array.from(bytes),
    mime,
  });

/// Read the OS clipboard as an image, encode it as PNG, write it to
/// the codemux temp directory, and return the absolute path.
///
/// This is the fast path for Ctrl+V on the new-workspace dialog —
/// the image bytes never cross the IPC boundary, only the resulting
/// file path comes back. Throws when the clipboard does not hold an
/// image (e.g. text-only), which the caller should treat as "let
/// the default paste behaviour run".
export const pasteClipboardImageToFile = (): Promise<string> =>
  invoke<string>("paste_clipboard_image_to_file");

/// Read the OS clipboard as an image, encode it as PNG, and return the
/// encoded bytes + MIME directly (no temp file).
///
/// This is the agent-chat sibling of `pasteClipboardImageToFile`. The
/// chat composer stages image bytes in memory rather than a filesystem
/// path, so we ship the encoded PNG across IPC once (Rust→JS) instead
/// of writing a temp file the composer would then have to read back and
/// clean up. Throws when the clipboard holds no image (e.g. text-only),
/// which the caller should treat as "let the default paste run".
///
/// WebKit hands the serialized `Vec<u8>` back as a `number[]`, so we
/// rewrap it in a `Uint8Array` before returning.
export interface ClipboardImagePayload {
  bytes: Uint8Array;
  mime: string;
}
export const pasteClipboardImage = async (): Promise<ClipboardImagePayload> => {
  const raw = await invoke<{ bytes: number[]; mime: string }>(
    "paste_clipboard_image",
  );
  return { bytes: new Uint8Array(raw.bytes), mime: raw.mime };
};

// ── Dialogs ──

export const pickFolderDialog = (title: string) =>
  invoke<string | null>("pick_folder_dialog", { title });

export const pickFilesDialog = (title?: string) =>
  invoke<string[]>("pick_files_dialog", { title: title ?? null });

/// OS save-as dialog. Returns `null` when the user cancels.
export interface SaveDialogOptions {
  title?: string;
  defaultFilename?: string;
  filterName?: string;
  filterExtensions?: string[];
}
export const pickSaveFileDialog = (opts: SaveDialogOptions = {}) =>
  invoke<string | null>("pick_save_file_dialog", {
    title: opts.title ?? null,
    defaultFilename: opts.defaultFilename ?? null,
    filterName: opts.filterName ?? null,
    filterExtensions: opts.filterExtensions ?? null,
  });

/// OS open-file dialog (single selection, optional extension
/// filter). Returns `null` when the user cancels.
export interface OpenDialogOptions {
  title?: string;
  filterName?: string;
  filterExtensions?: string[];
}
export const pickOpenFileDialog = (opts: OpenDialogOptions = {}) =>
  invoke<string | null>("pick_open_file_dialog", {
    title: opts.title ?? null,
    filterName: opts.filterName ?? null,
    filterExtensions: opts.filterExtensions ?? null,
  });

// ── Update ──

export const getPackageFormat = () =>
  invoke<string>("get_package_format");

// ── Agent Chat ──
//
// Frontend wrappers for the agent_chat_* Tauri commands defined in
// src-tauri/src/commands/agent_chat.rs. Every wrapper takes a
// ProviderKind so a single pane can route to either provider without
// the UI needing to know the Rust enum encoding.

export interface AgentChatStartSessionInput {
  thread_id: string;
  cwd: string;
  model: string | null;
  resume_cursor: unknown | null;
  /** Do not recover a persisted provider-native cursor for this start. */
  fresh_session?: boolean;
  permission_mode: string | null;
  /** Claude only — session-scoped reasoning effort. */
  effort?: string | null;
  /** Claude only — context-window selector (e.g. "1m" adds [1m] bracket). */
  context_window?: string | null;
  /** Premium provider speed tier. Capability-gated by the active model. */
  fast_mode?: boolean;
  additional_directories: string[];
  env: Record<string, string> | null;
  extra?: unknown;
}

export interface AgentChatSendTurnInput {
  thread_id: string;
  text: string;
  /** Unexpanded composer text used for the durable transcript. */
  display_text?: string | null;
  /** Exact cwd-scoped skill definitions selected in the composer. */
  skill_ids?: string[];
  /** Skill-token-stripped text before mode/effort/attachment wrappers. */
  skill_text?: string | null;
  /** Whether plugin-bundled skills are included in this selection scope. */
  include_plugins?: boolean;
  /** Staged image references. Each image's bytes are written to a
   *  staging file at attach time (`agent_chat_stage_image`), so the turn
   *  carries only the absolute path + MIME instead of marshalling the
   *  raw bytes across IPC as a JSON `number[]`. */
  images?: Array<{ path: string; media_type: string }>;
  model_override: string | null;
  /** Codex only — per-turn effort. Claude ignores this. */
  effort_override?: string | null;
  /** Codex only (future) — per-turn permission-mode override via
   *  `sandboxPolicy` on `turn/start`. Wired in the backend struct;
   *  MVP Codex UI still round-trips mode via session restart. */
  permission_mode_override?: string | null;
  /** Follow-up queueing — optimistic-send correlation token. When the
   *  turn is queued behind an active turn the backend echoes this on the
   *  `turn_queued` event so the reducer reconciles the greyed bubble
   *  instead of duplicating it. */
  client_nonce?: string | null;
}

/** Result of `agent_chat_send_turn`. When `queued_id` is set the turn
 *  was queued behind an active turn (rendered greyed-out) instead of
 *  starting immediately; `turn_id` is an empty placeholder in that case. */
export interface TurnStartResult {
  turn_id: string;
  queued_id: string | null;
}

export const getHomeDir = () => invoke<string>("get_home_dir");

export const agentChatCreatePane = (
  workspaceId: string,
  provider: AgentChatProviderKind | null = null,
  cwd: string | null = null,
  launchMode: LaunchMode | null = null,
) =>
  invoke<string>("agent_chat_create_pane", {
    workspaceId,
    provider,
    cwd,
    launchMode,
  });

export const agentChatStartSession = (
  paneId: string,
  provider: AgentChatProviderKind,
  input: AgentChatStartSessionInput,
) =>
  invoke<string>("agent_chat_start_session", { paneId, provider, input });

export const agentChatSendTurn = (
  provider: AgentChatProviderKind,
  input: AgentChatSendTurnInput,
) => invoke<TurnStartResult>("agent_chat_send_turn", { provider, input });

/** Stage a pasted/attached image's raw bytes to a backend staging file,
 *  moving the (potentially multi-MB) upload off the send path. The bytes
 *  travel as the invoke's raw request body (NOT a JSON `number[]`); the
 *  MIME rides an `x-media-type` header. Returns the staging file's
 *  absolute path, which the turn later references via `send_turn.images`.
 *
 *  Fallback: the web-remote bridge serializes every invoke as JSON and
 *  drops invoke options (headers included), so the raw-body form can't
 *  reach the command there. When the backend rejects the body shape we
 *  retry once with the base64 JSON form it also accepts — ~1.33x the raw
 *  size, still far cheaper than the per-byte `number[]` JSON this path
 *  replaced. */
export const stageChatImage = async (
  bytes: Uint8Array,
  mime: string,
): Promise<{ path: string; media_type: string }> => {
  // Web-remote client: skip the raw attempt entirely — the WS bridge
  // would JSON-serialize the Uint8Array per-byte (the exact cost this
  // command exists to avoid) only for the backend to reject it.
  if (isRemoteClient()) {
    return await invoke<{ path: string; media_type: string }>(
      "agent_chat_stage_image",
      { bytes_base64: bytesToBase64(bytes), media_type: mime },
    );
  }
  try {
    return await invoke<{ path: string; media_type: string }>(
      "agent_chat_stage_image",
      bytes,
      { headers: { "x-media-type": mime } },
    );
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    // Only the body-shape rejection warrants the JSON retry; genuine
    // validation failures (unsupported MIME, size cap) re-throw as-is.
    if (!message.includes("expected a raw image body")) throw err;
    return await invoke<{ path: string; media_type: string }>(
      "agent_chat_stage_image",
      {
        bytes_base64: bytesToBase64(bytes),
        media_type: mime,
      },
    );
  }
};

/** Best-effort delete of a staged image file (chip removal / draft
 *  discard). Failures are swallowed by callers. */
export const discardStagedChatImage = (path: string) =>
  invoke<void>("agent_chat_discard_staged_image", { path });

/** Read a staged/persisted image back off disk for contexts where the
 *  asset protocol can't load a local path (web-remote, dev mock). Rewraps
 *  the JSON byte array into a `Uint8Array` the caller can Blob-URL. */
export const readChatImage = async (
  path: string,
): Promise<{ bytes: Uint8Array; media_type: string }> => {
  const res = await invoke<{ bytes: number[]; media_type: string }>(
    "agent_chat_read_image",
    { path },
  );
  return { bytes: new Uint8Array(res.bytes), media_type: res.media_type };
};

/** Read an agent-authored absolute image path for chat preview fallback.
 * Desktop normally uses the asset protocol; browser/web-remote clients reach
 * this only after that URL fails to load. */
export const readLocalChatImage = async (
  path: string,
): Promise<{ bytes: Uint8Array; media_type: string }> => {
  const res = await invoke<{ bytes: number[]; media_type: string }>(
    "agent_chat_read_local_image",
    { path },
  );
  return { bytes: new Uint8Array(res.bytes), media_type: res.media_type };
};

/** Warm the MCP servers in the background so the up-front prime cost
 *  overlaps composing instead of blocking the first session start.
 *  Returns immediately; safe to fire-and-forget. */
export const primeChatMcp = () => invoke<void>("agent_chat_prime_mcp");

/** Interrupt the thread's running turn.
 *
 *  Resolves to whether the interrupt actually reached a LIVE session.
 *  `false` — a dead or already-closed session — is a success, not an error
 *  (a stale Stop click must not toast), but it is the caller's cue that no
 *  settlement event is coming and the pane has to settle itself. A
 *  REJECTION is the opposite signal: the turn may well still be running,
 *  so the caller must not settle on it. */
export const agentChatInterruptTurn = (
  provider: AgentChatProviderKind,
  threadId: string,
  turnId: string | null = null,
) =>
  invoke<boolean>("agent_chat_interrupt_turn", {
    provider,
    threadId,
    turnId,
  });

/** Stop the background watch loops a pane is monitoring with.
 *
 *  Clears the thread's monitor tracking (and blocklists those ids for the rest
 *  of the run) plus any `codemux monitor start` flag on the pane, and
 *  best-effort interrupts the provider session so the tasks actually die. The
 *  state clear always happens; the interrupt is only effective while a turn is
 *  in flight (see the Rust command's doc comment for the limitation).
 *
 *  `threadId` is nullable on purpose: a pane can carry a manual monitoring
 *  flag with no chat thread bound to it, and Stop has to work there too —
 *  otherwise the button spins forever on the one surface that can clear it.
 *
 *  Resolves once the backend has published the recomputed pane status — the
 *  caller's "Stopping…" state is released by the status leaving `monitoring`,
 *  not by this promise. */
export const agentChatStopMonitoring = (
  provider: AgentChatProviderKind,
  threadId: string | null,
  paneId: string,
) =>
  invoke<void>("agent_chat_stop_monitoring", {
    provider,
    threadId,
    paneId,
  });

/** True iff the thread has a live (non-dead) session whose turn is
 *  currently in flight. Used by the remount-hydrate path to tell a
 *  genuinely-finished run apart from a still-running one whose terminal
 *  event simply hasn't been persisted yet (a workspace-switch unmount
 *  drops the live event listener while the backend keeps streaming).
 *  Callers must treat any invoke error as `false` so a backend that
 *  lacks the command, or a transient failure, falls back to today's
 *  heuristic behavior. */
export const agentChatTurnActive = (
  provider: AgentChatProviderKind,
  threadId: string,
) =>
  invoke<boolean>("agent_chat_turn_active", {
    provider,
    threadId,
  });

/** Cancel a queued (not-yet-dispatched) follow-up turn. Returns whether the
 *  provider actually removed it; a `false` result means it already
 *  dispatched or otherwise no longer exists. A real cancellation emits a
 *  `queued_turn_cancelled` event so the UI removes the greyed bubble. */
export const agentChatCancelQueuedTurn = (
  provider: AgentChatProviderKind,
  threadId: string,
  queuedId: string,
) =>
  invoke<boolean>("agent_chat_cancel_queued_turn", {
    provider,
    threadId,
    queuedId,
  });

/** Send a queued (not-yet-dispatched) follow-up turn **now**: the backend
 *  promotes it to the front of the queue and soft-interrupts the active
 *  turn (session, transcript, and on-disk work are preserved), then
 *  dispatches it as a normal follow-up. The `queued_turn_dispatched` event
 *  promotes the greyed bubble — no optimistic state change needed here. */
export const agentChatSendQueuedTurnNow = (
  provider: AgentChatProviderKind,
  threadId: string,
  queuedId: string,
) =>
  invoke<void>("agent_chat_send_queued_turn_now", {
    provider,
    threadId,
    queuedId,
  });

export const agentChatRespondToRequest = (
  provider: AgentChatProviderKind,
  threadId: string,
  requestId: string,
  decision: ApprovalDecision,
) =>
  invoke<void>("agent_chat_respond_to_request", {
    provider,
    threadId,
    requestId,
    decision,
  });

export const agentChatSetModel = (
  provider: AgentChatProviderKind,
  threadId: string,
  model: string | null,
) => invoke<void>("agent_chat_set_model", { provider, threadId, model });

export const agentChatSetFastMode = (
  provider: AgentChatProviderKind,
  threadId: string,
  fastMode: boolean,
) => invoke<void>("agent_chat_set_fast_mode", { provider, threadId, fastMode });

export const agentChatSetPermissionMode = (
  provider: AgentChatProviderKind,
  threadId: string,
  mode: string,
) =>
  invoke<void>("agent_chat_set_permission_mode", {
    provider,
    threadId,
    mode,
  });

export const agentChatStopSession = (
  provider: AgentChatProviderKind,
  threadId: string,
) => invoke<void>("agent_chat_stop_session", { provider, threadId });

// ── Session history (pane-header dropdown) ──

export interface AgentChatSessionRecord {
  thread_id: string;
  sdk_session_id: string | null;
  workspace_id: string;
  cwd: string | null;
  provider: AgentChatProviderKind;
  title: string | null;
  created_at: string;
  last_active_at: string;
  /** Persisted per-thread picker config (nullable). Written from
   *  `agent_chat_start_session` + `agent_chat_update_session_config`
   *  and carried across silent restarts so the pane rehydrates the
   *  user's chosen model / reasoning / context / permission mode after
   *  an app restart instead of falling back to the provider default. */
  model: string | null;
  effort: string | null;
  context_window: string | null;
  permission_mode: string | null;
  fast_mode?: boolean;
}

/** Provider-neutral conversation row for the composer's `@session:` picker.
 *  `preview` is the newest persisted user/assistant prose; hidden reasoning
 *  and tool output are excluded by the backend conversation index. */
export interface AgentChatSessionMention {
  thread_id: string;
  workspace_id: string;
  cwd: string | null;
  provider: string;
  title: string | null;
  last_active_at: string;
  preview: string;
  message_count: number;
}

/** Safe, size-bounded handoff payload materialised from one persisted chat. */
export interface AgentChatSessionContext {
  thread_id: string;
  workspace_id: string;
  cwd: string | null;
  provider: string;
  title: string | null;
  last_active_at: string;
  content: string;
  message_count: number;
  included_message_count: number;
  truncated: boolean;
  handoff_kind: "summary" | "direct";
  summary_cached: boolean;
  summary_error: string | null;
  summarizer_provider: string | null;
  summarizer_model: string | null;
  summarizer_effort: string | null;
  revision_message_id: number;
  full_history_available: boolean;
}

export interface UtilityModelSelection {
  provider: AgentChatProviderKind;
  model: string;
  effort: string | null;
}

/** One SQLite FTS5 hit in the persisted conversation store. A content hit
 * carries the durable message row id used for transcript deep-linking; a
 * title-only hit carries `null` and opens at the start of the thread. */
export interface AgentChatSearchResult {
  message_id: number | null;
  thread_id: string;
  workspace_id: string;
  cwd: string | null;
  provider: AgentChatProviderKind;
  session_title: string | null;
  role: "user" | "assistant" | "title";
  turn_id: string | null;
  snippet: string;
  created_at: string;
}

export interface OpenAgentChatSearchResult {
  pane_id: string;
  workspace_id: string;
}

/** Partial per-thread config patch for
 *  `agent_chat_update_session_config`. Only the fields present are
 *  overwritten in the DB row (snake_case to match the Rust struct /
 *  DB columns). This is DB-only — it never requires a live session. */
export interface AgentChatSessionConfigUpdate {
  model?: string | null;
  effort?: string | null;
  context_window?: string | null;
  permission_mode?: string | null;
  fast_mode?: boolean;
}

export const agentChatListSessions = (
  workspaceId: string,
  cwd: string | null = null,
  limit: number | null = null,
) =>
  invoke<AgentChatSessionRecord[]>("agent_chat_list_sessions", {
    workspaceId,
    cwd,
    limit,
  });

export const agentChatListSessionMentions = (
  workspaceId: string,
  currentCwd: string | null,
  excludeThreadId: string | null,
  limit = 30,
) =>
  invoke<AgentChatSessionMention[]>("agent_chat_list_session_mentions", {
    workspaceId,
    currentCwd,
    excludeThreadId,
    limit,
  });

export const agentChatGetSessionContext = (
  workspaceId: string,
  threadId: string,
  utilitySelection: UtilityModelSelection | null = null,
) =>
  invoke<AgentChatSessionContext>("agent_chat_get_session_context", {
    workspaceId,
    threadId,
    utilitySelection,
  });

export const agentChatSearch = (
  query: string,
  workspaceIds: string[],
  limit = 12,
) =>
  invoke<AgentChatSearchResult[]>("agent_chat_search", {
    query,
    workspaceIds,
    limit,
  });

export const agentChatOpenSearchResult = (threadId: string) =>
  invoke<OpenAgentChatSearchResult>("agent_chat_open_search_result", {
    threadId,
  });

export const agentChatRenameSession = (threadId: string, title: string) =>
  invoke<void>("agent_chat_rename_session", { threadId, title });

export const agentChatDeleteSession = (threadId: string) =>
  invoke<void>("agent_chat_delete_session", { threadId });

/** Fetch a single persisted session row by thread id, INCLUDING rows
 *  whose `sdk_session_id` is still null (unlike `agentChatListSessions`,
 *  which filters those out). Returns `null` when no row exists. Used by
 *  the pane's mount-seed effect to restore picker config + resume
 *  cursor after an app restart. */
export const agentChatGetSession = (threadId: string) =>
  invoke<AgentChatSessionRecord | null>("agent_chat_get_session", {
    threadId,
  });

/** Persist per-thread picker config to the session row without touching
 *  the live provider session. Only the provided fields are overwritten.
 *  Fired fire-and-forget from the picker handlers so a value the user
 *  picks survives an app restart even when no live session exists. */
export const agentChatUpdateSessionConfig = (
  threadId: string,
  config: AgentChatSessionConfigUpdate,
) =>
  invoke<void>("agent_chat_update_session_config", { threadId, config });

/**
 * Return the persisted message envelopes for a thread, in original
 * insertion order. Each element is a JSON-encoded envelope: either a
 * canonical `ProviderRuntimeEvent` (`item_completed`, `turn_completed`,
 * `request_opened`, `request_resolved`) or a synthetic
 * `{type: "user_message", text: ...}` record. The frontend hydrate
 * action replays each payload through the agent-chat reducer to
 * rebuild the visible transcript on resume.
 */
export const agentChatListMessages = (threadId: string) =>
  invoke<string[]>("agent_chat_list_messages", { threadId });

/** One persisted row plus its durable `agent_chat_messages.id`. Mirrors
 *  `AgentChatMessageRow` in `commands/agent_chat.rs`. */
export interface AgentChatMessageRow {
  id: number;
  payload: string;
  /** SQLite insertion time, used to rebuild stable turn durations. */
  created_at_ms?: number;
}

/**
 * Cursor read of a thread's transcript: every row after `afterId`
 * (everything when `null`), ascending.
 *
 * This is the resume path — a remounted pane asks only for what it has
 * not applied yet, so a warm revisit transfers and reduces nothing when
 * the thread has not moved. Unlike `agentChatListMessages` the payloads
 * are SHAPED: an oversized tool-result body ships as a lazy stub (see
 * `lib/agent-chat/lazy-tool-result.ts`) and is fetched on expand through
 * `agentChatGetToolResult`.
 */
export const agentChatListMessagesAfter = (
  threadId: string,
  afterId: number | null,
) =>
  invoke<AgentChatMessageRow[]>("agent_chat_list_messages_after", {
    threadId,
    afterId,
  });

/** Highest stored row id for a thread (`null` when it has none). Used to
 *  detect a resume cursor that sits ABOVE the thread's own history — a
 *  cursor inherited from a merged/deleted thread — which falls back to a
 *  cold hydrate. */
export const agentChatThreadHeadId = (threadId: string) =>
  invoke<number | null>("agent_chat_thread_head_id", { threadId });

/** The newest whole turns of a thread. Mirrors `AgentChatMessageTail` in
 *  `commands/agent_chat.rs`. */
export interface AgentChatMessageTail {
  /** Ascending; starts at a `user_message` row unless `complete`. */
  rows: AgentChatMessageRow[];
  /** Rows the thread holds in total. */
  total_rows: number;
  /** `rows` is the whole thread — nothing older to backfill. */
  complete: boolean;
}

/**
 * Tail read for a COLD open: at least the last `limit` rows, widened down
 * to the nearest user-turn boundary so the page replays cleanly (the
 * reducer correlates rows within a turn). The pane renders this at once
 * and backfills the rest through `agentChatListMessagesBefore`.
 * Payloads are shaped like `agentChatListMessagesAfter`.
 */
export const agentChatListMessagesTail = (threadId: string, limit: number) =>
  invoke<AgentChatMessageTail>("agent_chat_list_messages_tail", {
    threadId,
    limit,
  });

/** Backfill page: the newest `limit` rows strictly older than `beforeId`,
 *  ascending. A page shorter than `limit` is the start of the thread. */
export const agentChatListMessagesBefore = (
  threadId: string,
  beforeId: number,
  limit: number,
) =>
  invoke<AgentChatMessageRow[]>("agent_chat_list_messages_before", {
    threadId,
    beforeId,
    limit,
  });

/** Fetch one persisted row verbatim, by id: the full tool-result payload
 *  behind a lazy stub. */
export const agentChatGetToolResult = (rowId: number) =>
  invoke<string>("agent_chat_get_tool_result", { rowId });

/** A filesystem baseline captured immediately before one accepted user turn.
 *  The nonce binds it to the rendered user bubble; the cutoff is the durable
 *  transcript row that remains after a true revert. */
export interface AgentChatTurnCheckpointRecord {
  thread_id: string;
  workspace_id: string;
  repo_path: string;
  turn_index: number;
  client_nonce: string | null;
  transcript_cutoff_id: number;
  ref_name: string;
  snapshot_commit: string;
  head_commit: string;
  branch: string | null;
  created_at: string;
}

export const agentChatListTurnCheckpoints = (threadId: string) =>
  invoke<AgentChatTurnCheckpointRecord[]>(
    "agent_chat_list_turn_checkpoints",
    { threadId },
  );

/** Rewind the workspace, provider conversation, and local transcript to just
 *  before `turnIndex`. Returns the checkpoints that remain addressable. */
export const agentChatRevertTurnCheckpoint = (
  threadId: string,
  turnIndex: number,
) =>
  invoke<AgentChatTurnCheckpointRecord[]>(
    "agent_chat_revert_turn_checkpoint",
    { threadId, turnIndex },
  );

/**
 * Register a per-thread `Channel` that receives every live
 * `AgentChatEventPayload` for `threadId` — including the
 * high-frequency `content_delta` token stream. Mirrors the PTY
 * `attach_pty_output` pattern: Channels are Tauri's high-throughput
 * streaming primitive, unlike the broadcast event bus.
 *
 * Returns the attach generation. Pass it back to
 * `detachAgentChatOutput` on teardown so a stale detach (unmount
 * racing a remount) can never clobber a newer pane's channel.
 *
 * The channel carries live events only: late-attaching / resumed
 * panes rebuild history via `agentChatListMessages` hydrate.
 */
export const attachAgentChatOutput = (
  threadId: string,
  channel: Channel<AgentChatEventPayload>,
) => invoke<number>("attach_agent_chat_output", { threadId, channel });

/** Tear down the channel installed by `attachAgentChatOutput`.
 *  Idempotent; a mismatched generation is a silent no-op. */
export const detachAgentChatOutput = (
  threadId: string,
  generation: number,
) => invoke<void>("detach_agent_chat_output", { threadId, generation });

// ── OpenCode (Step 12 Stage 1) ──
//
// Wrappers for the discovery + ping commands that land alongside the
// `ProviderKind::OpenCode` enum variant. Stage 1 has no UI consumers
// yet — the picker rebuild lives in Stage 4 — but the typed surface
// is locked here so future callers don't depend on stringly-typed
// `invoke()`s spread across the codebase. Stage 2 adds a sibling
// `opencodeListModels` wrapper here without changing this section's
// shape.

/** Probe whether OpenCode is usable on this machine.
 *
 *  Pass `serverUrl` only when there is an actual URL worth probing —
 *  app boot should pass `null`/omit so the discovery layer skips the
 *  HTTP round-trip. The settings panel (later stage) will pass a
 *  candidate URL so the user can validate "is this endpoint
 *  reachable" before saving the change. */
export const opencodeCheckAvailability = (
  serverUrl: string | null = null,
) =>
  invoke<OpenCodeAvailability>("opencode_check_availability", {
    serverUrl,
  });

/** Round-trip a `GET /` against `baseUrl` and report success.
 *
 *  Resolves on any HTTP response (including 401/403 — the goal is
 *  reachability, not auth). Rejects with a stable string code:
 *  `"connect_failed"`, `"request_timed_out"`, `"http_status_<code>"`,
 *  or `"request_error: ..."` for everything else. `serverPassword`
 *  is forwarded as the HTTP Basic password (username is always
 *  `"opencode"` — see `opencodeRuntime.ts:499` in the reference
 *  clone). Pass `null` for the no-password local-loopback case. */
export const opencodePing = (
  baseUrl: string,
  serverPassword: string | null = null,
) =>
  invoke<void>("opencode_ping", { baseUrl, serverPassword });

/** Step 12 Stage 2 — fetch the running OpenCode server's full
 *  provider catalogue. The Rust side spawns `opencode serve`
 *  lazily on the first call; the resulting child stays alive
 *  for the rest of the Codemux session.
 *
 *  Resolves to a flat list of providers; each entry's `models`
 *  map is the per-provider `Record<modelId, OpenCodeModel>`
 *  view. On a fully-populated dev box this returns ~116
 *  providers / ~4,354 models — Stage 4's picker is responsible
 *  for filtering down to the connected subset.
 *
 *  Rejects with the same stable string vocabulary as
 *  `opencodePing`, plus `"opencode_not_installed"` when the
 *  binary can't be located, `"spawn_failed: …"` /
 *  `"ready_banner_missing"` / `"ready_timeout_after_…"` when
 *  the server fails to come up, and `"parse_error: …"` when
 *  the response payload doesn't decode. */
export const opencodeListModels = () =>
  invoke<OpenCodeProviderEntry[]>("opencode_list_models");

// ── Tool permissions ──

/** Mirrors the Rust `PermissionRule` struct. `scope` is the file the
 *  rule lives in: `user` = ~/.claude/settings.json, `project` =
 *  project-shared `.claude/settings.json`, `local` = gitignored
 *  `.claude/settings.local.json`. */
export interface PermissionRule {
  tool_name: string;
  rule_content: string | null;
  behavior: "allow" | "deny" | "ask";
  scope: "user" | "project" | "local";
  source_path: string;
}

export const listToolPermissions = (projectRoot: string | null) =>
  invoke<PermissionRule[]>("list_tool_permissions", { projectRoot });

export const removeToolPermission = (
  rule: PermissionRule,
  projectRoot: string | null,
) =>
  invoke<void>("remove_tool_permission", { rule, projectRoot });

// ── Usage ──

export interface UsageBucketSlice {
  tokens: number;
  cost_usd: number;
}

export interface UsageBucket {
  start_ms: number;
  /** Short axis label. */
  label: string;
  /** Full label for the hover readout. */
  sub_label: string;
  /** Provider id → this bucket's slice. Providers with no activity in
   *  the bucket are absent rather than zero-valued. */
  providers: Record<string, UsageBucketSlice>;
}

export interface UsageModel {
  model: string;
  tokens: number;
  cost_usd: number;
  /** Tokens produced by subagent work; `0` when none. */
  subagent_tokens: number;
}

export interface UsageProvider {
  provider: AgentChatProviderKind | string;
  tokens: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  cost_usd: number;
  session_count: number;
  models: UsageModel[];
}

export interface UsageTotals {
  /** API/list-price equivalent, not necessarily money charged. */
  estimated_cost_usd: number;
  total_tokens: number;
  /** Cache-read share of all tokens, 0–1. */
  cache_read_share: number;
  session_count: number;
}

/** Which quota window a meter describes. */
export type PlanWindowKind =
  | "five_hour"
  | "seven_day"
  | "seven_day_opus"
  | "seven_day_sonnet"
  | "overage"
  | "other";

/** How the user pays a provider, as detected rather than assumed. */
export type PlanAuthMode = "subscription" | "api_key";

export interface PlanUsageWindow {
  kind: PlanWindowKind;
  /** Percent consumed, 0–100 (already normalized backend-side: Claude
   *  reports a 0..1 fraction, Codex an already-scaled percent). */
  used_pct: number;
  /** Unix ms when the window rolls over, when the provider said. */
  resets_at_ms: number | null;
  label?: string | null;
}

/** One provider's live plan-quota reading. Absent for providers that
 *  expose no quota (OpenCode) or that have not run this session. */
export interface ProviderQuota {
  windows: PlanUsageWindow[];
  plan_label?: string | null;
  auth_mode?: PlanAuthMode | null;
  received_at_ms: number;
}

/** Where the period's tokens went. */
export interface UsageComposition {
  processed_tokens: number;
  cache_read_tokens: number;
  /** Cache reads as a share of observed input (input + reads + writes). */
  cache_read_share_of_input: number;
  input_tokens: number;
  cache_write_tokens: number;
  output_tokens: number;
  /** Subset of output_tokens; 0 when no provider reported a split. */
  reasoning_tokens: number;
  cache_savings_usd: number;
  cache_savings_multiplier: number | null;
}

/** How measured-vs-estimated the period's cost figures are. */
export interface CostConfidence {
  provider_reported_share: number;
  table_priced_share: number;
  /** Share of TOKENS from unpriced rows (a cost share would always be 0). */
  unpriced_token_share: number;
  cache_savings_usd: number;
}

/** One row of the flat, cross-provider model breakdown. */
export interface FlatModelUsage {
  provider: string;
  model: string;
  tokens: number;
  cost_usd: number;
  priced: boolean;
  provider_reported: boolean;
}

export interface UsageSummary {
  period: UsagePeriod;
  start_ms: number;
  end_ms: number;
  buckets: UsageBucket[];
  providers: UsageProvider[];
  totals: UsageTotals;
  composition: UsageComposition;
  confidence: CostConfidence;
  /** Flat cross-provider model breakdown, most expensive first. */
  models: FlatModelUsage[];
  /** Live plan quota keyed by provider id; providers with no reading are
   *  simply absent and render no meters. */
  quota: Record<string, ProviderQuota>;
  synced_at_ms: number;
}

export type UsagePeriod = "today" | "7d" | "30d" | "90d";

/** Minutes EAST of UTC for this machine — the sign convention the
 *  backend expects. `Date.prototype.getTimezoneOffset()` returns minutes
 *  WEST (it reports +300 for UTC-5), so it is negated here. Read at call
 *  time rather than cached so a machine that crosses a DST boundary while
 *  the page is open re-buckets correctly on the next poll. */
function localTzOffsetMinutes(): number {
  return -new Date().getTimezoneOffset();
}

/** Usage is read from each provider's durable local history. */
export const usageSummary = (period: UsagePeriod) =>
  invoke<UsageSummary>("usage_summary", {
    period,
    tzOffsetMinutes: localTzOffsetMinutes(),
  });

export const usageExportCsv = (period: UsagePeriod) =>
  invoke<string>("usage_export_csv", {
    period,
    tzOffsetMinutes: localTzOffsetMinutes(),
  });

/** What one provider-history scan did. */
export interface UsageImportReport {
  files_scanned: number;
  sessions_found: number;
  rows_updated: number;
  /** True when the scan rebuilt its materialized cache. */
  reimported: boolean;
}

/** Scan Claude, Codex, and OpenCode history on this machine. Incremental
 *  and safe to call repeatedly. */
export const usageScanProviderHistory = () =>
  invoke<UsageImportReport>("usage_scan_provider_history");

// ── Skills ──

export type SkillProvider = "claude" | "codex" | "opencode" | "codemux";
export type SkillScope =
  | "user"
  | "project"
  | "plugin"
  | "managed"
  | "admin"
  | "system"
  | "configured";
export type SkillCompatibility = "compatible" | "soft-warn" | "hard-warn";
export type SkillAvailability =
  | "native"
  | "explicit-portable"
  | "native-only"
  | "unavailable";
export type SkillInvocationKind =
  | "native-command"
  | "codex-skill-item"
  | "prompt-prefix"
  | "none";
export type SkillProvenance = "filesystem" | "provider_catalog";

export interface SkillProjection {
  targetProvider: SkillProvider;
  availability: SkillAvailability;
  compatibility: SkillCompatibility;
  reasons: string[];
  invocation: SkillInvocationKind;
}

export interface Skill {
  id: string;
  /** Worktree-stable identity used for persisted enable/disable choices. */
  preferenceId?: string;
  name: string;
  description: string | null;
  provider: SkillProvider;
  scope: SkillScope;
  skillDir: string;
  filePath: string;
  body: string;
  rawFrontmatter: unknown;
  bundledFiles: string[];
  compatibility: SkillCompatibility;
  compatibilitySignals: string[];
  symlinked: boolean;
  pluginSlug: string | null;
  provenance?: SkillProvenance;
  readable?: boolean;
  sourceEnabled?: boolean;
  /** Invalid Agent Skills name. The row remains visible in Settings but
   *  cannot be selected or invoked. */
  validationError?: string | null;
  projections?: SkillProjection[];
}

export interface SkillAdapterError {
  provider: SkillProvider;
  message: string;
}

export interface SkillInventory {
  skills: Skill[];
  errors: SkillAdapterError[];
}

export const listSkills = (
  projectRoot: string | null,
  includePlugins: boolean,
  force = false,
) => invoke<SkillInventory>("list_skills", { projectRoot, includePlugins, force });

/** One provider-native slash command (e.g. Claude Code's `/compact`,
 *  `/init`, `/review`, or a custom `.claude/commands` entry).
 *  Discovered live from the provider — never hardcoded. Selecting one
 *  inserts the literal `/name ` text into the draft; the provider
 *  interprets the leading slash itself at send time. */
export interface ProviderSlashCommand {
  /** Command name without the leading slash. */
  name: string;
  /** One-line description. May be empty. */
  description: string;
  /** Argument hint (e.g. `<pr-url>`). May be empty. */
  argumentHint: string;
}

/** List the provider-native slash commands for a thread anchored at
 *  `cwd`. Claude harvests via the Agent SDK's `supportedCommands()`
 *  and Grok uses its ACP initialize/live command snapshot (both cached per
 *  cwd backend-side). Providers without discovery resolve to an empty list. */
export const listChatSlashCommands = (
  provider: AgentChatProviderKind,
  cwd: string,
) =>
  invoke<ProviderSlashCommand[]>("list_chat_slash_commands", {
    provider,
    cwd,
  });

/** Start the file watcher. Returns the count of paths actually being
 *  watched (paths that don't exist on disk are skipped silently).
 *  Idempotent — re-calling with new args swaps the watcher's path set. */
export const startSkillsWatcher = (
  projectRoot: string | null,
  includePlugins: boolean,
) =>
  invoke<number>("start_skills_watcher", { projectRoot, includePlugins });

export const stopSkillsWatcher = () =>
  invoke<void>("stop_skills_watcher");

/** Tauri event name emitted whenever a watched skill file changes. */
export const SKILLS_CHANGED_EVENT = "skills-changed";

// ── MCP Servers ──

export type McpConfigSource =
  | "codemux"
  | "codemuxUser"
  | "codemuxProject"
  | "claudeUser"
  | "claudeLocal"
  | "claudeProject"
  | "cursorUser"
  | "cursorProject"
  | "codexUser"
  | "openCodeUser"
  | "openCodeProject";

export type McpTransport = "stdio" | "http";

export interface McpServerConfig {
  id: string;
  name: string;
  /** All locations this exact config was found in. Length 1 for normal
   *  rows; > 1 when the same `(name, command, args, env)` shows up in
   *  multiple files (e.g. the same MCP added to both Claude and Cursor).
   *  `sources[0]` is the canonical (lowest-rank) source — the one the UI
   *  uses to pick the row's group. */
  sources: McpConfigSource[];
  command: string;
  args: string[];
  env: Record<string, string>;
  disabled: boolean;
  transport: McpTransport;
  raw: unknown;
}

/** Discover all configured MCP servers across providers, plus Codemux's
 *  hardcoded entry. Stateless — no spawn, no caching. */
export const listMcpServers = (
  projectRoot: string | null,
) => invoke<McpServerConfig[]>("list_mcp_servers", { projectRoot });

// ── MCP runtime (Stage 2) ──

/** Server-id reserved for Codemux's hardcoded always-on MCP. The frontend
 *  uses this constant to suppress the toggle row and to guard against
 *  accidentally adding it to the disabled set. */
export const MCP_CODEMUX_SELF_ID = "codemux-self";

/** Tauri event fired whenever an MCP server transitions state. Payload
 *  is a single `McpServerRuntime`. The Settings panel listens to update
 *  the relevant row without polling. */
export const MCP_STATUS_CHANGED_EVENT = "mcp-status-changed";

export type McpServerStatusKind =
  | "discovered"
  | "starting"
  | "running"
  | "errored"
  | "stopped";

/** Discriminated by `kind` so TS exhaustiveness checks catch missing
 *  arms in the UI. Mirrors the Rust `McpServerStatus` enum. */
export type McpServerStatus =
  | { kind: "discovered" }
  | { kind: "starting" }
  | { kind: "running"; toolCount: number }
  | { kind: "errored"; message: string }
  | { kind: "stopped" };

export interface McpServerRuntime {
  id: string;
  name: string;
  status: McpServerStatus;
  toolsCount: number;
  errorMessage: string | null;
  stderrTail: string | null;
  startedAtMs: number | null;
}

export interface McpTool {
  name: string;
  prefixedName: string;
  description: string | null;
  inputSchema: unknown;
  serverId: string;
}

/** Snapshot of every server's runtime state. Settings hydrates from
 *  this on mount, then listens to {@link MCP_STATUS_CHANGED_EVENT}. */
export const getMcpRuntimeStatus = () =>
  invoke<McpServerRuntime[]>("get_mcp_runtime_status");

/** Mirror the frontend zustand `disabledIds` into the Rust registry.
 *  Idempotent — call this on mount and after every toggle. */
export const setMcpDisabledIds = (ids: string[]) =>
  invoke<void>("set_mcp_disabled_ids", { ids });

/** Spawn every enabled MCP server discovered for the active project.
 *  Idempotent — already-running servers are untouched. */
export const primeMcpRuntime = (projectRoot: string | null) =>
  invoke<McpServerRuntime[]>("prime_mcp_runtime", { projectRoot });

export const startMcpServerCmd = (id: string, projectRoot: string | null) =>
  invoke<McpServerRuntime>("start_mcp_server_cmd", { id, projectRoot });

export const stopMcpServerCmd = (id: string) =>
  invoke<McpServerRuntime>("stop_mcp_server_cmd", { id });

export const restartMcpServerCmd = (id: string) =>
  invoke<McpServerRuntime>("restart_mcp_server_cmd", { id });

export const listMcpTools = () => invoke<McpTool[]>("list_mcp_tools");

/** Tools registered by a single server. Drives the Settings
 *  tool-list modal. */
export const listMcpToolsForServer = (id: string) =>
  invoke<McpTool[]>("list_mcp_tools_for_server", { id });

// ── Hosts (Settings → Hosts, Step 2 of cloud-push) ──
//
// SSH credentials never enter these payloads. The frontend only
// sends name + sshTarget; auth is the OS's job (`~/.ssh/config`,
// agent, keys). `dirty` indicates the row has unpushed changes; the
// UI surfaces it as a small "syncing…" hint.
export interface HostView {
  id: number;
  /** The server-assigned id once this host has synced to the cloud,
   *  null for hosts created offline that haven't synced yet. */
  server_id: string | null;
  name: string;
  ssh_target: string;
  created_at: string;
  updated_at: string;
  dirty: boolean;
}

export interface HostTestResult {
  ok: boolean;
  message: string;
  /** True when probe succeeded but `codemux-remote` isn't installed
   *  on the host yet. Triggers the bootstrap-install consent modal. */
  needs_install?: boolean;
  /** Reported `uname -sm` from the probe, forwarded into the
   *  bootstrap call so we don't re-probe. */
  uname?: string | null;
}

export interface HostBootstrapResult {
  ok: boolean;
  message: string;
}

export const hostsList = () => invoke<HostView[]>("hosts_list");

/**
 * Live reachability + inventory facts for one configured device (SSH host),
 * as observed by the background `hosts_inventory` poller. Separate from
 * `HostView` (identity, synced to the account) because none of this is
 * account state: it is what THIS install last saw over SSH.
 */
export interface HostStatusView {
  host_id: number;
  /** False until the poller has probed this host at least once this
   *  session. Unprobed hosts are neither online nor offline yet. */
  probed: boolean;
  /** Last probe succeeded. */
  reachable: boolean;
  /** ISO timestamp of the last successful probe; null if never reached. */
  last_seen_at: string | null;
  /** Last probe problem: the unreachable reason, or — while reachable — a
   *  degraded note such as the host agent being missing or the inventory
   *  failing. Null when clean. */
  last_error: string | null;
  /** Sum of the host's workspace directories in bytes; null if unknown. */
  disk_bytes: number | null;
  /** A Codemux Remote Control server is up on the host. */
  remote_control_serving: boolean;
}

/** One status row per configured host; hosts not probed yet are included
 *  with `probed: false` and null facts. */
export const hostsStatusList = () =>
  invoke<HostStatusView[]>("hosts_status_list");

/** Event name emitted (payload: `HostStatusView[]`) whenever the poller
 *  finishes a round and any host's status changed. */
export const HOSTS_STATUS_CHANGED_EVENT = "hosts-status-changed";

/** Which of the given workspaces the sweep may remove, and how big each
 *  worktree is. The backend is the single authority on eligibility: a key
 *  is present only for a disposable local worktree (never a repo root,
 *  attach-only or host-backed workspace). A null value means the worktree
 *  qualifies but its size could not be measured. */
export const workspacesWorktreeSizes = (workspaceIds: string[]) =>
  invoke<Record<string, number | null>>("workspaces_worktree_sizes", {
    workspaceIds,
  });

export const hostsAdd = (name: string, sshTarget: string) =>
  invoke<HostView>("hosts_add", { name, sshTarget });

export const hostsUpdate = (id: number, name: string, sshTarget: string) =>
  invoke<HostView>("hosts_update", { id, name, sshTarget });

export const hostsDelete = (id: number) =>
  invoke<void>("hosts_delete", { id });

export const hostsTestConnection = (id: number) =>
  invoke<HostTestResult>("hosts_test_connection", { id });

/** Install `codemux-remote` on a host that the probe reported as
 *  reachable-but-missing. Pass the `uname` string returned by the
 *  probe so we don't have to re-probe. */
export const hostsBootstrapInstall = (id: number, uname: string) =>
  invoke<HostBootstrapResult>("hosts_bootstrap_install", { id, uname });

/** Force-reinstall `codemux-remote` on a host regardless of the version
 *  already installed, and restart its pty-daemon so the fresh binary
 *  takes effect immediately. The dev-workflow escape hatch: rebuilding
 *  `codemux-remote` keeps the version string the same, so the push-time
 *  version check would otherwise skip the upgrade and leave the host on
 *  a stale binary. The backend re-probes the uname itself, so no prior
 *  Test connection is required. */
export const hostsReinstallRemote = (id: number) =>
  invoke<HostBootstrapResult>("hosts_reinstall_remote", { id });

// ── Automations (scheduled agent runs) ──
//
// An automation is a named prompt + agent + recurrence. `schedule` is a
// complete RFC 5545 iCalendar block (a DTSTART line plus one RRULE
// line). `dirty` flags unpushed changes for a future account-sync.
export interface AutomationView {
  id: number;
  server_id: string | null;
  name: string;
  prompt: string;
  agent: string;
  schedule: string;
  timezone: string;
  host_id: number | null;
  project_path: string | null;
  /** The project's git remote URL — how a remote host clones the repo.
   *  Resolved server-side from the chosen project. */
  project_remote: string | null;
  enabled: boolean;
  retention_limit: number;
  last_run_at: string | null;
  next_run_at: string | null;
  /** Status of the most recent run — drives the list health dot.
   *  `null` until the automation has fired. */
  last_run_status: string | null;
  created_at: string;
  updated_at: string;
  dirty: boolean;
}

/** Result of probing whether a host can reach an automation's repo. */
export interface RepoAccessResult {
  ok: boolean;
  message: string;
}

/** One fire of an automation (or a skipped fire). `status` is one of
 *  `scheduled | running | succeeded | failed | skipped_offline |
 *  skipped_busy`. */
export interface AutomationRunView {
  id: number;
  automation_id: number;
  status: string;
  scheduled_for: string;
  started_at: string | null;
  finished_at: string | null;
  host_id: number | null;
  workspace_id: string | null;
  /** The branch the run's worktree was created on. */
  branch: string | null;
  /** URL of the pull request the run opened, if any. */
  pr_url: string | null;
  error: string | null;
  created_at: string;
}

/** Editable fields of an automation, shared by create and update.
 *  `project_remote` is resolved server-side from `project_path`; the
 *  UI may omit it. */
export interface AutomationInput {
  name: string;
  prompt: string;
  agent: string;
  schedule: string;
  timezone: string;
  host_id: number | null;
  project_path: string | null;
  project_remote?: string | null;
  retention_limit: number;
}

export const automationsList = () =>
  invoke<AutomationView[]>("automations_list");

export const automationsGet = (id: number) =>
  invoke<AutomationView>("automations_get", { id });

export const automationsCreate = (input: AutomationInput) =>
  invoke<AutomationView>("automations_create", { input });

export const automationsUpdate = (id: number, input: AutomationInput) =>
  invoke<AutomationView>("automations_update", { id, input });

export const automationsSetEnabled = (id: number, enabled: boolean) =>
  invoke<AutomationView>("automations_set_enabled", { id, enabled });

export const automationsDelete = (id: number) =>
  invoke<void>("automations_delete", { id });

export const automationsRuns = (automationId: number, limit?: number) =>
  invoke<AutomationRunView[]>("automations_runs", { automationId, limit });

/** Probe whether a host can reach a project's repo (a read-only
 *  `git ls-remote`). `hostId` null = "This machine" (always reachable).
 *  Pass `projectRemote` when known, else `projectPath` to resolve it. */
export const automationsCheckRepoAccess = (
  hostId: number | null,
  projectPath: string | null,
  projectRemote: string | null,
) =>
  invoke<RepoAccessResult>("automations_check_repo_access", {
    hostId,
    projectPath,
    projectRemote,
  });

export interface WorkspacePushOutcome {
  ok: boolean;
  message: string;
  remote_path: string | null;
  rsync_summary: string | null;
}

export interface WorkspacePullOutcome {
  ok: boolean;
  message: string;
  rsync_summary: string | null;
}

/** Push a workspace to a host. The backend atomically sets the
 *  workspace's host_id only on successful rsync — no need for the
 *  frontend to do an optimistic-set + rollback dance. */
export const workspacePushToHost = (workspaceId: string, hostId: number) =>
  invoke<WorkspacePushOutcome>("workspace_push_to_host", { workspaceId, hostId });

/** Pull a remote workspace back to local. Clears host_id on success. */
export const workspacePullBack = (workspaceId: string) =>
  invoke<WorkspacePullOutcome>("workspace_pull_back", { workspaceId });

/** Assign (or clear) the host a workspace runs on. `null` clears
 *  the assignment (back to local). */
export const setWorkspaceHost = (workspaceId: string, hostId: number | null) =>
  invoke<void>("set_workspace_host", { workspaceId, hostId });

// ── Workspaces sync (cross-device workspace registry) ──
//
// One row per workspace this user owns, across every device they
// have signed in with. Rows whose `workspace_id` is set match a
// local `WorkspaceSnapshot`; rows whose `workspace_id` is null live
// only on a sibling device (the UI offers a "Pull to this device"
// affordance for those).
//
// `host_server_id` matches `HostView.server_id`; the local host
// `id` is unrelated and not portable across devices.
export interface WorkspaceSyncView {
  id: number;
  server_id: string | null;
  workspace_id: string | null;
  title: string;
  host_server_id: string | null;
  project_path: string | null;
  project_remote: string | null;
  git_branch: string | null;
  /** Phase-4 divergence detection: the workspace's git HEAD sha at
   *  the last reconcile. Compared across rows in the overview to
   *  detect when the same (project_remote, git_branch) has different
   *  HEADs on multiple devices. Null when git isn't available or
   *  the worktree has no commits yet. */
  git_head_sha: string | null;
  /** Deterministic project identity (UUIDv5 of the canonical remote or
   *  project root). Workspaces sharing this belong to the same project
   *  — the overview groups by it so a repo's main checkout and its
   *  worktrees cluster together. Null on rows not yet stamped. */
  project_uid: string | null;
  /** "main" (repo root checkout) | "worktree" (per-branch worktree).
   *  Rendered as a small badge in the overview. */
  workspace_kind: string | null;
  /** The repo's default branch as reported by the daemon poller
   *  (origin/HEAD → main/master → current branch). Lets the overview
   *  render a remote project's root distinctly and land a pull on the
   *  right branch even when git_branch is null. Local-only — never sent
   *  to or returned from the cloud API. */
  default_branch: string | null;
  /** The workspace's real absolute path on its host (daemon-reported,
   *  set by the inventory poller). Drives the "Open on host" action and
   *  lets the overview dedupe a host row against an already-open
   *  attach-in-place workspace. Local-only; null on rows that never came
   *  from a host poll. */
  origin_path: string | null;
  created_at: string;
  updated_at: string;
  /** True iff the row has unpushed changes. UI surfaces this as a
   *  "Pending sync" pill the same way Automations does. */
  dirty: boolean;
}

export const workspacesSyncList = () =>
  invoke<WorkspaceSyncView[]>("workspaces_sync_list");

/** Force an immediate pull + push pass. The background loop runs
 *  every 30s; use this only when the user explicitly hits a "Sync
 *  now" affordance. Returns Ok even if the user isn't signed in. */
export const workspacesSyncNow = () =>
  invoke<void>("workspaces_sync_now");

// ── Cross-device adoption (Phase 2) ──
//
// "Adoption" = take a workspace that lives on another device of your
// account and materialize a local copy on THIS device. Two paths:
//   - host-backed (this PR): when the workspace lives on a host you
//     also have configured locally, rsync from the host
//   - clone (future): when there's no shared host, git-clone from
//     `project_remote`
//
// The frontend opens the Pull-to-this-device dialog with
// `workspacesAdoptionPreview` (so it knows which variant to render
// without race conditions), then calls `workspacesAdoptSynced` on
// confirm.

export interface AdoptionPreview {
  can_host_adopt: boolean;
  can_clone_adopt: boolean;
  host_configured: boolean;
  host_label: string | null;
  project_already_cloned_at: string | null;
  suggested_path: string;
  is_path_in_use: boolean;
  /** When the sync row is already linked to a local workspace, this
   *  carries that local id so the UI can offer "Open existing" instead
   *  of re-running the adoption flow. */
  already_adopted_workspace_id: string | null;
  /** Cross-machine "same branch of the same project" conflict guard.
   *  When set, another local workspace on THIS device is already on
   *  the same branch of (heuristically) the same project — matched by
   *  `(basename(project_path), git_branch)`. Pulling would silently
   *  create a parallel copy of work the user's already doing, so the
   *  dialog disables Pull and points at the existing local workspace.
   *  Null when no conflict was detected. */
  same_branch_project_exists_at: string | null;
}

export interface AdoptOutcome {
  workspace_id: string;
  worktree_path: string;
  message: string;
}

export const workspacesAdoptionPreview = (serverId: string) =>
  invoke<AdoptionPreview>("workspaces_adoption_preview", { serverId });

export const workspacesAdoptSynced = (serverId: string) =>
  invoke<AdoptOutcome>("workspaces_adopt_synced", { serverId });

/** Phase-3 clone-fallback adoption: when the sibling-device workspace
 *  has no shared host (`host_server_id` is null), this clones the
 *  `project_remote` git URL and creates a worktree at the branch.
 *
 *  Important: this creates a NEW local workspace with its own fresh
 *  `server_id` — it does NOT link to the original sibling row. Both
 *  devices end up with independent copies that share a git remote.
 *  The user has been warned about uncommitted-work loss via the
 *  dialog before reaching this. */
export const workspacesAdoptViaClone = (serverId: string) =>
  invoke<AdoptOutcome>("workspaces_adopt_via_clone", { serverId });

/** One workspace that failed during a project-granularity pull. */
export interface ProjectAdoptFailure {
  server_id: string;
  title: string;
  error: string;
}

export interface ProjectAdoptOutcome {
  adopted: AdoptOutcome[];
  failures: ProjectAdoptFailure[];
  message: string;
}

/** Project-first pull: materialize the repo ROOT (protected, at
 *  `~/.codemux/projects/<repo>`) first, then recreate every worktree as a
 *  real linked worktree hanging off it — in one action. Pass the shared
 *  `project_uid` of the remote project's rows. Worktree failures are
 *  collected (non-fatal); a root failure rejects. */
export const workspacesAdoptProject = (projectUid: string) =>
  invoke<ProjectAdoptOutcome>("workspaces_adopt_project", { projectUid });

/** Non-destructively reconcile a divergent "standalone copy": detaches its
 *  workspace card (files kept on disk) when it's clean, or rejects with
 *  guidance when it has uncommitted/unpushed work. */
export const workspacesReconcileCopy = (workspaceId: string) =>
  invoke<string>("workspaces_reconcile_copy", { workspaceId });

export interface OpenOnHostOutcome {
  /** The new (or already-open) local attach-in-place workspace id. */
  workspace_id: string;
  /** The directory the workspace operates in on the host. */
  remote_cwd: string;
  message: string;
}

/** Open a host-backed workspace IN PLACE on its host — no rsync, no local
 *  copy of the files. Creates a local "attach-in-place" workspace whose
 *  terminal runs on the host (over the existing SSH-tunneled daemon) in the
 *  workspace's real on-host directory. Because the host daemon is detached +
 *  persistent, closing the app leaves it running and reopening reattaches.
 *
 *  `syncRowId` is the `WorkspaceSyncView.id` of a host-backed row; the host
 *  must be configured on this device. Idempotent — opening twice
 *  re-activates the existing local view. */
export const workspaceOpenOnHost = (syncRowId: number) =>
  invoke<OpenOnHostOutcome>("workspace_open_on_host", { syncRowId });

// ── Web Remote Access ──
//
// Typed wrappers for the `web_remote_*` commands defined in
// `src-tauri/src/web_remote/mod.rs`. The embedded server is default-off;
// enabling it binds `0.0.0.0:<port>` so a browser on another device can
// drive this instance. Every mutator returns the fresh `WebRemoteStatus`
// and also broadcasts it on the `web-remote-state-changed` event bus (see
// `src/remote/web-remote-events.ts`), so callers can either use the
// returned value directly or live-update from the event.

/** Current server + paired-device snapshot. Cheap; safe on mount. */
export const webRemoteStatus = () =>
  invoke<WebRemoteStatus>("web_remote_status");

/** Turn the server on: binds the listener and persists `enabled=true`, so
 *  it is restored on the next app boot. */
export const webRemoteEnable = () =>
  invoke<WebRemoteStatus>("web_remote_enable");

/** Turn the server off: tears the listener down and persists
 *  `enabled=false`. Paired devices are kept (revoke to remove them). */
export const webRemoteDisable = () =>
  invoke<WebRemoteStatus>("web_remote_disable");

/** Update the persisted config. `port` and `bindScope` trigger a
 *  backend-side rebind while running (dropping existing connections, which
 *  can't follow to a new port/interface); `requireApproval` gates whether
 *  new pairings sit pending until approved on the desktop.
 *  `accountModeEnabled` toggles the account sign-in admission path
 *  (`POST /api/pair-account`), and `trustAccountBrowsers` is the "trust
 *  browsers on my account without approval" opt-out — neither rebinds the
 *  listener. `relayModeEnabled` starts/stops the parallel from-anywhere iroh
 *  endpoint (never a rebind of the axum listener). Omitted fields are left
 *  unchanged. */
export const webRemoteSetConfig = (opts: {
  port?: number;
  requireApproval?: boolean;
  bindScope?: WebRemoteBindScope;
  accountModeEnabled?: boolean;
  trustAccountBrowsers?: boolean;
  relayModeEnabled?: boolean;
}) =>
  invoke<WebRemoteStatus>("web_remote_set_config", {
    port: opts.port ?? null,
    requireApproval: opts.requireApproval ?? null,
    bindScope: opts.bindScope ?? null,
    accountModeEnabled: opts.accountModeEnabled ?? null,
    trustAccountBrowsers: opts.trustAccountBrowsers ?? null,
    relayModeEnabled: opts.relayModeEnabled ?? null,
  });

/** The device's stable iroh `node_id` (its `EndpointId`) — the address a
 *  hosted-origin browser dials to reach this desktop over the relay transport.
 *  `null` until relay mode has been enabled at least once (the identity key is
 *  persisted lazily). Also surfaced on {@link WebRemoteStatus.iroh_node_id}. */
export const webRemoteIrohNodeId = () =>
  invoke<string | null>("web_remote_iroh_node_id");

/** The from-anywhere transport's control-plane registration state (registered
 *  flag, device/node ids, last heartbeat, last error). Registration is
 *  best-effort and only runs while relay mode is on and the desktop is signed
 *  into a Codemux account; the Settings pane surfaces this as a "device
 *  registered" indicator distinct from the raw `node_id`. */
export const webRemoteRegistrationStatus = () =>
  invoke<WebRemoteRegistrationStatus>("web_remote_registration_status");

/** Mint a one-time pairing token (10 min TTL, single use). Returns the
 *  relative `/#pair=<token>` path — the frontend composes full per-endpoint
 *  URLs and renders the QR. */
export const webRemoteCreatePairing = () =>
  invoke<WebRemotePairingInfo>("web_remote_create_pairing");

/** Enumerate reachable endpoints (loopback / LAN / tailnet / MagicDNS) for
 *  the currently-configured port. */
export const webRemoteListEndpoints = () =>
  invoke<WebRemoteEndpoint[]>("web_remote_list_endpoints");

/** List paired devices (approved + pending). */
export const webRemoteListSessions = () =>
  invoke<WebRemoteSessionView[]>("web_remote_list_sessions");

/** Revoke a device: deletes its session and immediately closes any live
 *  sockets it holds. */
export const webRemoteRevokeSession = (sessionId: string) =>
  invoke<WebRemoteStatus>("web_remote_revoke_session", { sessionId });

/** Approve a pending device so it can connect. */
export const webRemoteApproveSession = (sessionId: string) =>
  invoke<WebRemoteStatus>("web_remote_approve_session", { sessionId });

/** Reject a pending device: closes its sockets and erases the row. */
export const webRemoteRejectSession = (sessionId: string) =>
  invoke<WebRemoteStatus>("web_remote_reject_session", { sessionId });

/** Publish the desktop updater's availability so paired web clients can offer
 *  a "desktop update available" prompt. Called by the DESKTOP updater hook
 *  only — it rides out on `web-remote-state-changed` and the `web_remote_status`
 *  snapshot. Passing `available: false` clears any stale version. */
export const webRemotePublishUpdateAvailable = (
  available: boolean,
  version: string | null,
) =>
  invoke<void>("web_remote_publish_update_available", { available, version });

/** From a web client, ask the desktop to run its update + restart flow. The
 *  desktop updater hook receives the `web-remote-update-requested` event and
 *  drives its standard download/restart path (desktop confirmation UX still
 *  applies). Agents survive the restart; the web client reconnects. */
export const webRemoteRequestUpdate = () =>
  invoke<void>("web_remote_request_update");

// ── VS Code Marketplace theme import ─────────────────────────────────────

/** A Marketplace extension that ships at least one dark colour theme.
 *  snake_case because that is what the Rust side serializes — the repo's
 *  convention for frontend-facing command payloads (see `tauri/types.ts`). */
export interface MarketplaceTheme {
  extension_id: string;
  display_name: string;
  publisher: string;
  install_count: number;
  version: string;
  vsix_url: string;
}

/** One dark colour theme inside an extension, with its raw JSONC. */
export interface MarketplaceThemeVariant {
  label: string;
  ui_theme: string;
  content: string;
}

export const searchMarketplaceThemes = (query: string) =>
  invoke<MarketplaceTheme[]>("vscode_marketplace_search", { query });

export const fetchMarketplaceThemes = (vsixUrl: string) =>
  invoke<MarketplaceThemeVariant[]>("vscode_marketplace_fetch_themes", {
    vsixUrl,
  });

export type QuestionAction =
  | {
      action: "answer";
      answers: string[];
      submission_id: string;
      retry_unknown?: boolean;
    }
  | { action: "dismiss" | "reopen" | "reconcile" };
export const agentChatAnswerQuestion = (
  threadId: string,
  questionId: string,
  action: QuestionAction,
) =>
  invoke<import("./events").QuestionResolution>("agent_chat_answer_question", {
    threadId,
    questionId,
    action,
  });
