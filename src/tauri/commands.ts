import { invoke } from "@tauri-apps/api/core";
import type {
  AgentConfig,
  AgentSessionState,
  AppStateSnapshot,
  CheckInfo,
  CliToolInfo,
  CommLogEntry,
  EditorInfo,
  FileEntry,
  GhStatus,
  GitBranchInfo,
  GitDiffStat,
  GitFileStatus,
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
  SearchResult,
  ShellAppearance,
  TabKind,
  TerminalStatusPayload,
  ThemeColors,
  ThinkingModeInfo,
  WorkspaceConfig,
  WorktreeInfo,
} from "./types";

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

export const createBrowserPane = (paneId: string) =>
  invoke<string>("create_browser_pane", { paneId });

export const createTab = (workspaceId: string, kind: TabKind) =>
  invoke<string>("create_tab", { workspaceId, kind });

export const closeTab = (workspaceId: string, tabId: string) =>
  invoke("close_tab", { workspaceId, tabId });

export const activateTab = (workspaceId: string, tabId: string) =>
  invoke("activate_tab", { workspaceId, tabId });

export const renameTab = (workspaceId: string, tabId: string, title: string) =>
  invoke("rename_tab", { workspaceId, tabId, title });

export const killPort = (port: number) =>
  invoke("kill_port", { port });

export const detectEditors = () =>
  invoke<EditorInfo[]>("detect_editors");

export const openInEditor = (editorId: string, path: string) =>
  invoke<void>("open_in_editor", { editorId, path });

export const createWorktreeWorkspace = (
  branch: string,
  path: string,
  cwd: string,
  createBranch: boolean,
) =>
  invoke<string>("create_worktree_workspace", { branch, path, cwd, createBranch });

export const importWorktreeWorkspace = (path: string, cwd: string) =>
  invoke<string>("import_worktree_workspace", { path, cwd });

export const closeWorkspaceWithWorktree = (
  workspaceId: string,
  removeWorktree: boolean,
  forceDelete: boolean,
) =>
  invoke<void>("close_workspace_with_worktree", { workspaceId, removeWorktree, forceDelete });

export const getWorkspaceConfig = (path: string) =>
  invoke<WorkspaceConfig | null>("get_workspace_config", { path });

export const runWorkspaceSetup = (workspaceId: string) =>
  invoke<void>("run_workspace_setup", { workspaceId });

// ── Sections ──

export const createSection = (name: string, color: string) =>
  invoke<string>("create_section", { name, color });

export const renameSection = (sectionId: string, name: string) =>
  invoke("rename_section", { sectionId, name });

export const deleteSection = (sectionId: string) =>
  invoke("delete_section", { sectionId });

export const setSectionColor = (sectionId: string, color: string) =>
  invoke("set_section_color", { sectionId, color });

export const toggleSectionCollapsed = (sectionId: string) =>
  invoke("toggle_section_collapsed", { sectionId });

export const moveWorkspaceToSection = (
  workspaceId: string,
  sectionId: string,
  position?: number | null,
) =>
  invoke("move_workspace_to_section", { workspaceId, sectionId, position: position ?? null });

export const reorderWorkspaces = (workspaceIds: string[]) =>
  invoke("reorder_workspaces", { workspaceIds });

export const reorderSections = (sectionIds: string[]) =>
  invoke("reorder_sections", { sectionIds });

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

// ── Git ──

export const getGitStatus = (path: string) =>
  invoke<GitFileStatus[]>("get_git_status", { path });

export const getGitDiff = (path: string, file: string, staged: boolean) =>
  invoke<string>("get_git_diff", { path, file, staged });

export const getGitDiffStat = (path: string) =>
  invoke<GitDiffStat>("get_git_diff_stat", { path });

export const gitStageFiles = (path: string, files: string[]) =>
  invoke("git_stage_files", { path, files });

export const gitUnstageFiles = (path: string, files: string[]) =>
  invoke("git_unstage_files", { path, files });

export const gitCommitChanges = (path: string, message: string) =>
  invoke("git_commit_changes", { path, message });

export const gitPushChanges = (path: string) =>
  invoke("git_push_changes", { path });

export const getGitBranchInfo = (path: string) =>
  invoke<GitBranchInfo>("get_git_branch_info", { path });

export const listBranches = (path: string, remote: boolean) =>
  invoke<string[]>("list_branches", { path, remote });

export const listWorktrees = (path: string) =>
  invoke<WorktreeInfo[]>("list_worktrees", { path });

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

export const stopOpenflowRun = (runId: string, reason: string) =>
  invoke<OpenFlowRunRecord>("stop_openflow_run", { runId, reason });

export const listAvailableCliTools = () =>
  invoke<CliToolInfo[]>("list_available_cli_tools");

export const listModelsForTool = (toolId: string) =>
  invoke<ModelInfo[]>("list_models_for_tool", { toolId });

export const listThinkingModesForTool = (toolId: string) =>
  invoke<ThinkingModeInfo[]>("list_thinking_modes_for_tool", { toolId });

export const spawnOpenflowAgents = (
  runId: string,
  configs: AgentConfig[],
  cwd: string,
) =>
  invoke<string[]>("spawn_openflow_agents", { runId, configs, cwd });

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

export const detachPtyOutput = (sessionId: string) =>
  invoke("detach_pty_output", { sessionId });

export const attachPtyOutput = (
  sessionId: string,
  channel: string,
  scrollbackLines?: number,
) =>
  invoke("attach_pty_output", { channel, sessionId, scrollbackLines });

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

export const listDirectory = (path: string) =>
  invoke<FileEntry[]>("list_directory", { path });

// ── Dialogs ──

export const pickFolderDialog = (title: string) =>
  invoke<string | null>("pick_folder_dialog", { title });
