import { invoke, Channel } from "@tauri-apps/api/core";

export { Channel };
import type {
  UserSettings,
  AgentConfig,
  AgentSessionState,
  AppStateSnapshot,
  AuthResponse,
  AuthUser,
  BaseBranchDiff,
  CheckInfo,
  CliToolInfo,
  CommLogEntry,
  EditorInfo,
  FileEntry,
  GhStatus,
  GitBranchInfo,
  GitDiffStat,
  GitFileStatus,
  GitLogEntry,
  HandoffPacket,
  LaunchMode,
  ModelInfo,
  OpenFlowCreateRunRequest,
  OpenFlowRunRecord,
  OpenFlowRuntimeSnapshot,
  OrchestratorTriggerResult,
  PresetStoreSnapshot,
  ProjectMemorySnapshot,
  ProjectMemoryUpdate,
  PullRequestInfo,
  ReviewComment,
  InlineReviewComment,
  DeploymentInfo,
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

// ── Settings Sync ──

export const getSyncedSettings = () =>
  invoke<UserSettings>("get_synced_settings");

export const updateSyncedSettings = (settings: UserSettings) =>
  invoke<UserSettings>("update_synced_settings", { settings });

export const updateSetting = (section: string, key: string, value: unknown) =>
  invoke<UserSettings>("update_setting", { section, key, value });

export const resetSyncedSettings = () =>
  invoke<UserSettings>("reset_synced_settings");

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

export const createEmptyWorkspace = (cwd: string) =>
  invoke<string>("create_empty_workspace", { cwd });

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

export const markWorkspaceNotificationsRead = (workspaceId: string) =>
  invoke("mark_workspace_notifications_read", { workspaceId });

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
) =>
  invoke<string>("create_worktree_workspace", {
    repoPath,
    branch,
    newBranch,
    base: base ?? null,
    layout,
    initialPrompt: initialPrompt ?? null,
    agentPresetId: agentPresetId ?? null,
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

export const getPrDeployments = (path: string, prNumber: number) =>
  invoke<DeploymentInfo[]>("get_pr_deployments", { path, prNumber });

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

export const gitStashPush = (path: string, includeUntracked: boolean) =>
  invoke("git_stash_push", { path, includeUntracked });

export const gitStashPop = (path: string) =>
  invoke("git_stash_pop", { path });

export const gitDiscardFile = (path: string, file: string) =>
  invoke("git_discard_file", { path, file });

export const gitLogEntries = (path: string, count: number) =>
  invoke<GitLogEntry[]>("git_log_entries", { path, count });

export const listBranches = (path: string, remote: boolean) =>
  invoke<string[]>("list_branches", { path, remote });

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

export const generateAiCommitMessage = (path: string, model: string | null = null) =>
  invoke<string>("generate_ai_commit_message", { path, model });

export const setAiCommitMessageEnabled = (enabled: boolean) =>
  invoke("set_ai_commit_message_enabled", { enabled });

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

export const browserSpawn = (browserId: string) =>
  invoke<string>("browser_spawn", { browserId });

export const browserNavigate = (browserId: string, url: string) =>
  invoke<string>("browser_navigate", { browserId, url });

export const browserScreenshot = (browserId: string) =>
  invoke<string>("browser_screenshot", { browserId });

export const browserClick = (browserId: string, x: number, y: number) =>
  invoke<string>("browser_click", { browserId, x, y });

export const browserType = (browserId: string, text: string) =>
  invoke<string>("browser_type", { browserId, text });

export const browserClose = (browserId: string) =>
  invoke("browser_close", { browserId });

export const browserResizeViewport = (browserId: string, width: number, height: number) =>
  invoke("browser_resize_viewport", { browserId, width, height });

export const agentBrowserSpawn = (browserId: string) =>
  invoke("agent_browser_spawn", { browserId });

export const agentBrowserRun = (browserId: string, action: string, params: unknown) =>
  invoke("agent_browser_run", { browserId, action, params });

export const agentBrowserClose = (browserId: string) =>
  invoke("agent_browser_close", { browserId });

export const agentBrowserGetStreamUrl = () =>
  invoke<string>("agent_browser_get_stream_url");

export const agentBrowserScreenshot = (browserId: string) =>
  invoke<string>("agent_browser_screenshot", { browserId });

export const startBrowserStream = (browserId: string) =>
  invoke<string>("start_browser_stream", { browserId });

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
  launchMode: LaunchMode,
) =>
  invoke("apply_preset", { workspaceId, presetId, launchMode });

export const setPresetBarVisible = (visible: boolean) =>
  invoke("set_preset_bar_visible", { visible });

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

export const listDirectory = (path: string) =>
  invoke<FileEntry[]>("list_directory", { path });

// ── Dialogs ──

export const pickFolderDialog = (title: string) =>
  invoke<string | null>("pick_folder_dialog", { title });

export const pickFilesDialog = (title?: string) =>
  invoke<string[]>("pick_files_dialog", { title: title ?? null });
