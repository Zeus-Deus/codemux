import { invoke, Channel } from "@tauri-apps/api/core";

export { Channel };
import type {
  AgentChatProviderKind,
  OpenCodeAvailability,
  OpenCodeProviderEntry,
  ProviderChatCapabilities,
} from "./types";
import type { ApprovalDecision } from "./events";
import type {
  UserSettings,
  AgentConfig,
  AgentSessionState,
  AppStateSnapshot,
  AuthResponse,
  AuthUser,
  BaseBranchDiff,
  BranchDetail,
  CheckInfo,
  CliToolInfo,
  CommLogEntry,
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
  HandoffPacket,
  LaunchMode,
  ModelInfo,
  OpenFlowCreateRunRequest,
  OpenFlowRunRecord,
  ResourceMetricsSnapshot,
  OpenFlowRuntimeSnapshot,
  OrchestratorTriggerResult,
  PresetStoreSnapshot,
  ProjectMemorySnapshot,
  ProjectMemoryUpdate,
  PullRequestInfo,
  IncomingPrItem,
  ReviewComment,
  InlineReviewComment,
  MergeState,
  MergeIntoBaseResult,
  ConflictCheckResult,
  ResolverBranchInfo,
  SearchResult,
  ShellAppearance,
  TabKind,
  TerminalStatusPayload,
  ThemeColors,
  ThinkingModeInfo,
  WorkspaceConfig,
  WorktreeInfo,
  ProjectScripts,
  DetectedSetup,
} from "./types";

// ── Platform ──

// Returns the Rust target_os string: "linux", "macos", "windows", etc.
// Used to gate Windows-incompatible features at the UI layer (e.g. OpenFlow's
// bash wrappers). Safe to call before the rest of the app has initialized.
export const getPlatform = () =>
  invoke<string>("get_platform");

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

export const signOut = () =>
  invoke<void>("sign_out");

export const getAuthToken = () =>
  invoke<string | null>("get_auth_token");

// ── Skills Sync (Step 10 — Stage 2) ──
//
// `syncAvailable` reports whether the in-memory encryption key is
// loaded — sync features can only fire when this is true. False
// means either:
//   - GitHub OAuth user who hasn't run `setupSyncPassword` yet
//   - Local `sync-key.enc` was lost (call `providePasswordForSync`)
//   - User just signed out
//
// `authMethod` distinguishes "needs setup" from "needs repair":
//   - `"github"` + sync_available=false  → SetupSyncPasswordForm
//   - `"email"` or null + sync_available=false → ProvidePasswordForm

export interface SyncStatus {
  syncAvailable: boolean;
  authMethod: "email" | "github" | null;
}

/// Read the current sync state. Cheap, in-memory; no API roundtrip.
/// Use this on Settings → Sync mount and after any setup/repair.
export const getSyncStatus = () => invoke<SyncStatus>("get_sync_status");

/// One-time GitHub-OAuth-user setup. Derives credentials from
/// `(password, user.email)`, posts the AuthSecret to Better Auth's
/// `/api/auth/set-password`, then persists the encryption key
/// machine-bound at `~/.local/share/codemux/sync-key.enc` and loads
/// it into in-process memory. Returns the updated SyncStatus.
export const setupSyncPassword = (password: string) =>
  invoke<SyncStatus>("setup_sync_password", { password });

/// Repair flow: re-derive the encryption key when the local
/// `sync-key.enc` file is missing or undecryptable. No server call;
/// wrong password is detected lazily by Stage 3's first sync attempt.
export const providePasswordForSync = (password: string) =>
  invoke<SyncStatus>("provide_password_for_sync", { password });

// ── Skills sync engine (Stage 3) ──
//
// `skills_sync_now` pulls every encrypted skill from /api/skills,
// decrypts + writes to ~/.codemux/skills/, then walks every
// syncable user-scope skill path and pushes anything that's
// changed. Idempotent; safe to call back-to-back.
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

// ── Skills sync — local export / import / reset (Stage 4) ──
//
// `exportSkillsToFile` pulls every encrypted skill, decrypts with
// the in-memory key, and writes a plaintext JSON file at the
// path the user picked via the OS save-dialog.
//
// `importSkillsFromFile` is the inverse: read a JSON backup,
// re-encrypt every skill with the CURRENT key (post-reset), and
// push to the server. Use `mismatchedEmail` to surface a soft
// warning when the backup belongs to a different account.
//
// `wipeRemoteSkillsForReset` is the destructive helper used by
// the reset-sync-password dialog: it wipes the server's
// encrypted skills, clears the local key, and triggers Better
// Auth's email-reset flow. The user finishes the reset by
// clicking the link in their email and then signs back in here.

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

export const wipeRemoteSkillsForReset = () =>
  invoke<void>("wipe_remote_skills_for_reset");

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

export const getResourceMetrics = () =>
  invoke<ResourceMetricsSnapshot>("get_resource_metrics");

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

export const createEmptyWorkspace = (
  cwd: string,
  opts?: { skipSetup?: boolean },
) =>
  invoke<string>("create_empty_workspace", {
    cwd,
    skipSetup: opts?.skipSetup ?? null,
  });

export const getOrCreateHomeWorkspace = () =>
  invoke<string>("get_or_create_home_workspace");

export const listChatProviderCapabilities = (
  provider: AgentChatProviderKind,
) =>
  invoke<ProviderChatCapabilities>("list_chat_provider_capabilities", {
    provider,
  });

export const regenerateMcpConfig = (workspaceId: string) =>
  invoke<void>("regenerate_mcp_config", { workspaceId });

export const updateWorkspaceCwd = (workspaceId: string, cwd: string) =>
  invoke("update_workspace_cwd", { workspaceId, cwd });

export const createOpenflowWorkspace = (title: string, goal: string, cwd: string | null) =>
  invoke<string>("create_openflow_workspace", { title, goal, cwd });

export const createWorkspaceWithPreset = (cwd: string, presetId: string) =>
  invoke<string>("create_workspace_with_preset", { cwd, presetId });

export const activateWorkspace = (workspaceId: string) =>
  invoke("activate_workspace", { workspaceId });

export const renameWorkspace = (workspaceId: string, title: string) =>
  invoke("rename_workspace", { workspaceId, title });

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

export const createWorktreeWorkspace = (
  repoPath: string,
  branch: string,
  newBranch: boolean,
  layout: string,
  base?: string | null,
  initialPrompt?: string | null,
  agentPresetId?: string | null,
  prNumber?: number | null,
) =>
  invoke<string>("create_worktree_workspace", {
    repoPath,
    branch,
    newBranch,
    base: base ?? null,
    layout,
    initialPrompt: initialPrompt ?? null,
    agentPresetId: agentPresetId ?? null,
    prNumber: prNumber ?? null,
  });

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

export const mergePullRequest = (path: string, prNumber: number, method: string) =>
  invoke("merge_pull_request", { path, prNumber, method });

export const getPullRequestChecks = (path: string) =>
  invoke<CheckInfo[]>("get_pull_request_checks", { path });

export const getPrReviewComments = (path: string) =>
  invoke<ReviewComment[]>("get_pr_review_comments", { path });

export const getPrInlineComments = (path: string, prNumber: number) =>
  invoke<InlineReviewComment[]>("get_pr_inline_comments", { path, prNumber });

export const submitPrReview = (path: string, prNumber: number, event: string, body: string) =>
  invoke("submit_pr_review", { path, prNumber, event, body });

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

export const dbSaveOpenflowRun = (params: {
  runId: string;
  title?: string;
  goal?: string;
  status?: string;
  agentCount?: number;
  startedAt?: string;
  completedAt?: string;
}) =>
  invoke("db_save_openflow_run", params);

export const dbGetOpenflowHistory = (limit?: number) =>
  invoke<Array<{
    run_id: string;
    title: string | null;
    goal: string | null;
    status: string | null;
    agent_count: number | null;
    started_at: string | null;
    completed_at: string | null;
  }>>("db_get_openflow_history", { limit });

// ── Git ──

export const checkIsGitRepo = (path: string) =>
  invoke<boolean>("check_is_git_repo", { path });

export const initGitRepo = (path: string) =>
  invoke<string>("init_git_repo", { path });

export const gitCloneRepo = (url: string, targetDir: string) =>
  invoke<string>("git_clone_repo", { url, targetDir });

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

export const gitDiscardFile = (path: string, file: string) =>
  invoke("git_discard_file", { path, file });

export const gitLogEntries = (path: string, count: number) =>
  invoke<GitLogEntry[]>("git_log_entries", { path, count });

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

// ── OpenFlow ──

export const getOpenflowRuntimeSnapshot = () =>
  invoke<OpenFlowRuntimeSnapshot>("get_openflow_runtime_snapshot");

export const createOpenflowRun = (request: OpenFlowCreateRunRequest) =>
  invoke<OpenFlowRunRecord>("create_openflow_run", { request });

export const retryOpenflowRun = (runId: string) =>
  invoke<OpenFlowRunRecord>("retry_openflow_run", { runId });

export const applyOpenflowReviewResult = (
  runId: string,
  approved: boolean,
  feedback: string,
) =>
  invoke<OpenFlowRunRecord>("apply_openflow_review_result", { runId, approved, feedback });

export const stopOpenflowRun = (runId: string, reason: string, status: string = "cancelled") =>
  invoke<OpenFlowRunRecord>("stop_openflow_run", { runId, status, reason });

export const listAvailableCliTools = () =>
  invoke<CliToolInfo[]>("list_available_cli_tools");

export const listModelsForTool = (toolId: string) =>
  invoke<ModelInfo[]>("list_models_for_tool", { toolId });

export const listThinkingModesForTool = (toolId: string) =>
  invoke<ThinkingModeInfo[]>("list_thinking_modes_for_tool", { toolId });

export const spawnOpenflowAgents = (
  workspaceId: string,
  runId: string,
  goal: string,
  workingDirectory: string,
  agentConfigs: AgentConfig[],
) =>
  invoke<string[]>("spawn_openflow_agents", { workspaceId, runId, goal, workingDirectory, agentConfigs });

export const getAgentSessionsForRun = (runId: string) =>
  invoke<AgentSessionState[]>("get_agent_sessions_for_run", { runId });

export const getCommunicationLog = (runId: string, offset: number) =>
  invoke<[CommLogEntry[], number]>("get_communication_log", { runId, offset });

export const injectOrchestratorMessage = (runId: string, message: string) =>
  invoke<number>("inject_orchestrator_message", { runId, message });

export const triggerOrchestratorCycle = (runId: string) =>
  invoke<OrchestratorTriggerResult>("trigger_orchestrator_cycle", { runId });

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
  icon: string | null;
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
) =>
  invoke("apply_preset", { workspaceId, presetId, overrideMode, initialPrompt });

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

export const detachPtyOutput = (sessionId: string) =>
  invoke("detach_pty_output", { sessionId });

export const attachPtyOutput = (
  sessionId: string,
  channel: Channel<unknown>,
  skipPending?: boolean,
) =>
  invoke("attach_pty_output", { channel, sessionId, skipPending });

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
  permission_mode: string | null;
  /** Claude only — session-scoped reasoning effort. */
  effort?: string | null;
  /** Claude only — context-window selector (e.g. "1m" adds [1m] bracket). */
  context_window?: string | null;
  additional_directories: string[];
  env: Record<string, string> | null;
  extra?: unknown;
}

export interface AgentChatSendTurnInput {
  thread_id: string;
  text: string;
  images?: Array<{ data: number[]; media_type: string }>;
  model_override: string | null;
  /** Codex only — per-turn effort. Claude ignores this. */
  effort_override?: string | null;
  /** Codex only (future) — per-turn permission-mode override via
   *  `sandboxPolicy` on `turn/start`. Wired in the backend struct;
   *  MVP Codex UI still round-trips mode via session restart. */
  permission_mode_override?: string | null;
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
) => invoke<string>("agent_chat_send_turn", { provider, input });

export const agentChatInterruptTurn = (
  provider: AgentChatProviderKind,
  threadId: string,
  turnId: string | null = null,
) =>
  invoke<void>("agent_chat_interrupt_turn", {
    provider,
    threadId,
    turnId,
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
  provider: string;
  title: string | null;
  created_at: string;
  last_active_at: string;
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

export const agentChatRenameSession = (threadId: string, title: string) =>
  invoke<void>("agent_chat_rename_session", { threadId, title });

export const agentChatDeleteSession = (threadId: string) =>
  invoke<void>("agent_chat_delete_session", { threadId });

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

// ── Skills ──

export type SkillProvider = "claude" | "codex" | "opencode" | "codemux";
export type SkillScope = "user" | "project" | "plugin";
export type SkillCompatibility = "compatible" | "soft-warn" | "hard-warn";

export interface Skill {
  id: string;
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
}

export const listSkills = (
  projectRoot: string | null,
  includePlugins: boolean,
) => invoke<Skill[]>("list_skills", { projectRoot, includePlugins });

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
  | "cursorProject";

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

export interface CappedTools {
  tools: McpTool[];
  totalBeforeCap: number;
  droppedCount: number;
  droppedServers: string[];
}

/** Same as `listMcpTools` but returns the cap-info envelope so the
 *  Settings UI can show a "N tools dropped to fit cap" banner. */
export const listMcpToolsWithCapInfo = () =>
  invoke<CappedTools>("list_mcp_tools_with_cap_info");

/** Tools registered by a single server, uncapped. Drives the
 *  Settings tool-list modal — the user sees the full surface even
 *  when some tools were dropped from the agent's view to fit the
 *  cap. */
export const listMcpToolsForServer = (id: string) =>
  invoke<McpTool[]>("list_mcp_tools_for_server", { id });
