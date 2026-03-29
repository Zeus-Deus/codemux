// ── Synced Settings ──

export interface AppearanceSettings {
  theme: string;
  shell_font: string | null;
  terminal_font_size: number;
}

export interface EditorSettings {
  default_ide: string | null;
}

export interface TerminalSyncSettings {
  scrollback_limit: number;
  cursor_style: string;
}

export interface GitSyncSettings {
  default_base_branch: string;
}

export interface KeyboardSettings {
  shortcuts: Record<string, string>;
}

export interface NotificationSyncSettings {
  sound_enabled: boolean;
  desktop_enabled: boolean;
}

export interface UserSettings {
  appearance: AppearanceSettings;
  editor: EditorSettings;
  terminal: TerminalSyncSettings;
  git: GitSyncSettings;
  keyboard: KeyboardSettings;
  notifications: NotificationSyncSettings;
}

// ── Auth ──

export interface AuthUser {
  id: string;
  email: string;
  name: string | null;
  image: string | null;
}

export interface AuthResponse {
  user: AuthUser;
  token: string;
  expires_at: string;
}

export interface AuthStatePayload {
  authenticated: boolean;
  user: AuthUser | null;
}

// ── Terminal ──

export interface TerminalSessionSnapshot {
  session_id: string;
  title: string;
  shell: string | null;
  cwd: string;
  cols: number;
  rows: number;
  state: "starting" | "ready" | "exited" | "failed";
  last_message: string | null;
  exit_code: number | null;
}

export interface TerminalStatusPayload {
  session_id: string;
  state: "starting" | "ready" | "exited" | "failed";
  message: string | null;
  exit_code: number | null;
}

// ── Browser ──

export interface BrowserSessionSnapshot {
  browser_id: string;
  title: string;
  current_url: string | null;
  history: string[];
  history_index: number;
  is_loading: boolean;
  last_error: string | null;
}

// ── Pane Status ──

export type PaneStatus = "idle" | "working" | "permission" | "review";
export type ActivePaneStatus = Exclude<PaneStatus, "idle">;

// ── Notifications ──

export interface NotificationSnapshot {
  notification_id: string;
  workspace_id: string;
  pane_id: string | null;
  session_id: string | null;
  level: "info" | "attention";
  message: string;
  read: boolean;
  created_at_ms: number;
}

// ── Memory ──

export type MemorySource = "human" | "system";
export type MemoryEntryKind =
  | "pinned_context"
  | "decision"
  | "next_step"
  | "session_summary";

export interface MemoryEntry {
  entry_id: string;
  kind: MemoryEntryKind;
  source: MemorySource;
  content: string;
  tags: string[];
  tool_name: string | null;
  session_label: string | null;
  created_at_ms: number;
}

export interface ProjectMemorySnapshot {
  schema_version: number;
  project_root: string;
  project_name: string;
  project_brief: string | null;
  current_goal: string | null;
  current_focus: string | null;
  constraints: string[];
  pinned_context: MemoryEntry[];
  recent_decisions: MemoryEntry[];
  next_steps: MemoryEntry[];
  session_summaries: MemoryEntry[];
  updated_at_ms: number;
}

export interface ProjectMemoryUpdate {
  project_brief?: string | null;
  current_goal?: string | null;
  current_focus?: string | null;
  constraints?: string[] | null;
}

export interface HandoffPacket {
  project_name: string;
  project_root: string;
  summary: string;
  suggested_prompt: string;
  current_goal: string | null;
  current_focus: string | null;
  constraints: string[];
  pinned_context: string[];
  recent_decisions: string[];
  next_steps: string[];
}

// ── OpenFlow ──

export type OpenFlowRole =
  | "orchestrator"
  | "planner"
  | "builder"
  | "reviewer"
  | "tester"
  | "debugger"
  | "researcher";

export interface OpenFlowTaskNode {
  task_id: string;
  title: string;
  description: string;
  role: OpenFlowRole;
  status:
    | "pending"
    | "ready"
    | "in_progress"
    | "blocked"
    | "passed"
    | "failed"
    | "cancelled";
  depends_on: string[];
  success_criteria: string[];
  produced_artifacts: string[];
}

export interface OpenFlowArtifact {
  artifact_id: string;
  kind:
    | "plan"
    | "log"
    | "screenshot"
    | "diff"
    | "review_note"
    | "test_result"
    | "browser_evidence";
  title: string;
  location: string | null;
  summary: string;
}

export interface OpenFlowTimelineEntry {
  entry_id: string;
  level: "info" | "warning" | "error";
  message: string;
}

export interface OpenFlowWorkerState {
  role: OpenFlowRole;
  assigned_task_ids: string[];
  status: string;
  last_output: string | null;
}

export interface OpenFlowRetryPolicy {
  max_attempts: number;
  current_attempt: number;
  backoff_seconds: number;
}

export interface OpenFlowRunRecord {
  run_id: string;
  title: string;
  goal: string;
  status:
    | "draft"
    | "planning"
    | "executing"
    | "verifying"
    | "reviewing"
    | "awaiting_approval"
    | "completed"
    | "failed"
    | "cancelled";
  current_phase: string;
  replan_count: number;
  assigned_roles: OpenFlowRole[];
  task_graph: OpenFlowTaskNode[];
  artifacts: OpenFlowArtifact[];
  approvals: Array<{
    checkpoint_id: string;
    kind: string;
    title: string;
    required: boolean;
    reason: string;
  }>;
  timeline: OpenFlowTimelineEntry[];
  workers: OpenFlowWorkerState[];
  retry_policy: OpenFlowRetryPolicy;
  resumable: boolean;
  verification_required: boolean;
  browser_validation_required: boolean;
  command_validation_required: boolean;
  reviewer_score: number | null;
  stop_reason: string | null;
  orchestration_state:
    | "initializing"
    | "active"
    | "waiting_for_response"
    | "correcting_delegation"
    | "stalled"
    | "idle"
    | "error";
  orchestration_detail: string | null;
}

export interface OpenFlowRuntimeSnapshot {
  active_runs: OpenFlowRunRecord[];
}

export interface OpenFlowCreateRunRequest {
  title: string;
  goal: string;
  agent_roles: string[];
  cwd?: string;
}

// ── Workspace & Layout ──

export type WorkspaceTemplateKind = "codemux" | "folder" | "openflow";
export type LayoutPreset = "single" | "pair" | "quad" | "six" | "shell_browser";
export type FileStatus =
  | "added"
  | "modified"
  | "deleted"
  | "renamed"
  | "untracked"
  | "copied"
  | "conflicted";

export interface GitFileStatus {
  path: string;
  status: FileStatus;
  is_staged: boolean;
  is_unstaged: boolean;
  additions: number;
  deletions: number;
  conflict_type: string | null;
}

export interface ConflictFile {
  path: string;
  conflict_type: string;
}

export interface MergeState {
  is_merging: boolean;
  is_rebasing: boolean;
  merge_head: string | null;
  conflicted_files: ConflictFile[];
}

export interface ConflictCheckResult {
  has_conflicts: boolean;
  conflicting_files: ConflictFile[];
  target_branch: string;
}

export interface MergeIntoBaseResult {
  status: string;  // "merged", "conflicts", "already_up_to_date"
  temp_branch: string | null;
  source_branch: string;
  conflicted_files: ConflictFile[];
}

export interface BaseBranchDiff {
  files: GitFileStatus[];
  merge_base_commit: string;
}

export interface GitDiffStat {
  staged_additions: number;
  staged_deletions: number;
  unstaged_additions: number;
  unstaged_deletions: number;
}

export interface GitBranchInfo {
  branch: string | null;
  ahead: number;
  behind: number;
  has_upstream: boolean;
}

export interface GitLogEntry {
  hash: string;
  short_hash: string;
  message: string;
  is_pushed: boolean;
  author: string;
  time_ago: string;
}

export interface CommitFileEntry {
  path: string;
  status: string;
}

export interface EditorInfo {
  id: string;
  name: string;
  command: string;
}

export interface WorktreeInfo {
  path: string;
  branch: string | null;
  is_bare: boolean;
}

export interface PendingWorkspace {
  id: string;
  name: string;
  projectPath: string;
  status: "creating" | "failed";
  errorMessage?: string;
}

export interface PullRequestInfo {
  number: number;
  url: string;
  state: string;
  title: string;
  head_branch: string | null;
  base_branch: string | null;
  is_draft: boolean;
  mergeable: string | null;
  additions: number | null;
  deletions: number | null;
  review_decision: string | null;
  checks_passing: boolean | null;
  updated_at: string | null;
}

export interface CheckInfo {
  name: string;
  status: string;
  conclusion: string | null;
  elapsed_time: string | null;
  detail_url: string | null;
  started_at: string | null;
  completed_at: string | null;
}

export interface ReviewComment {
  id: number;
  author: string;
  body: string;
  state: string;
  created_at: string;
}

export interface InlineReviewComment {
  id: number;
  author: string;
  body: string;
  path: string;
  line: number | null;
  created_at: string;
  in_reply_to_id: number | null;
  pull_request_review_id: number | null;
}

export interface DeploymentInfo {
  id: number;
  environment: string;
  state: string;
  url: string | null;
  created_at: string;
}

export type GhStatus =
  | { status: "NotInstalled" }
  | { status: "NotAuthenticated" }
  | { status: "Authenticated"; username: string };

export type TabKind = "terminal" | "browser" | "diff";

export interface TabSnapshot {
  tab_id: string;
  kind: TabKind;
  title: string;
  surface_id: string | null;
  browser_id: string | null;
}

export type PaneNodeSnapshot =
  | { kind: "terminal"; pane_id: string; session_id: string; title: string }
  | { kind: "browser"; pane_id: string; browser_id: string; title: string }
  | {
      kind: "split";
      pane_id: string;
      direction: "horizontal" | "vertical";
      child_sizes: number[];
      children: PaneNodeSnapshot[];
    };

export interface SurfaceSnapshot {
  surface_id: string;
  title: string;
  root: PaneNodeSnapshot;
  active_pane_id: string;
}

export type WorkspaceType = "standard" | "open_flow";

export interface WorkspaceSnapshot {
  workspace_id: string;
  title: string;
  workspace_type: WorkspaceType;
  cwd: string;
  git_branch: string | null;
  git_ahead: number;
  git_behind: number;
  git_additions: number;
  git_deletions: number;
  git_changed_files: number;
  notification_count: number;
  latest_agent_state: string | null;
  worktree_path: string | null;
  project_root: string | null;
  pr_number: number | null;
  pr_state: string | null;
  pr_url: string | null;
  tabs: TabSnapshot[];
  active_tab_id: string;
  active_surface_id: string;
  surfaces: SurfaceSnapshot[];
}

export interface PersistenceSchema {
  schema_version: number;
  stores_layout_metadata: boolean;
  stores_terminal_metadata: boolean;
  stores_live_process_state: boolean;
}

export interface CodemuxConfigSnapshot {
  config_version: number;
  default_shell: string | null;
  theme_source: string;
  linux_first: boolean;
  notification_sound_enabled: boolean;
  ai_commit_message_enabled: boolean;
  ai_commit_message_model: string | null;
  ai_resolver_enabled: boolean;
  ai_resolver_cli: string | null;
  ai_resolver_model: string | null;
  ai_resolver_strategy: string;
}

export interface ResolverBranchInfo {
  temp_branch: string;
  original_branch: string;
  target_branch: string;
  conflicting_files: ConflictFile[];
}

export interface PortInfoSnapshot {
  port: number;
  pid: number;
  process_name: string;
  workspace_id: string | null;
  label: string | null;
}

export interface AppStateSnapshot {
  schema_version: number;
  active_workspace_id: string;
  workspaces: WorkspaceSnapshot[];
  terminal_sessions: TerminalSessionSnapshot[];
  browser_sessions: BrowserSessionSnapshot[];
  notifications: NotificationSnapshot[];
  detected_ports: PortInfoSnapshot[];
  pane_statuses: Record<string, PaneStatus>;
  persistence: PersistenceSchema;
  config: CodemuxConfigSnapshot;
}

// ── CLI / Agent Config ──

export interface CliToolInfo {
  id: string;
  name: string;
  available: boolean;
  path: string | null;
}

export interface ModelInfo {
  id: string;
  name: string;
  provider: string | null;
}

export interface ThinkingModeInfo {
  id: string;
  name: string;
  description: string;
}

export interface AgentConfig {
  agent_index: number;
  cli_tool: string;
  model: string;
  provider: string;
  thinking_mode: string;
  role: string;
}

export interface AgentSessionState {
  session_id: string;
  run_id: string;
  config: AgentConfig;
  status: "spawning" | "running" | "done" | "failed";
}

export interface CommLogEntry {
  timestamp: string;
  role: string;
  message: string;
}

export interface OrchestratorTriggerResult {
  current_phase: string;
  next_phase: string | null;
  analysis: {
    completed_roles: string[];
    blocked_roles: string[];
    assignments_count: number;
    user_injections_count: number;
  };
  actions_taken: string[];
  comm_log_offset: number;
  orchestration_state: OpenFlowRunRecord["orchestration_state"];
  orchestration_detail: string | null;
}

// ── Presets ──

export type LaunchMode = "split_pane" | "new_tab";

export interface TerminalPreset {
  id: string;
  name: string;
  description: string | null;
  commands: string[];
  working_directory: string | null;
  launch_mode: LaunchMode;
  icon: string | null;
  pinned: boolean;
  is_builtin: boolean;
  auto_run_on_workspace: boolean;
  auto_run_on_new_tab: boolean;
}

export interface PresetStoreSnapshot {
  presets: TerminalPreset[];
  bar_visible: boolean;
  default_preset_id: string | null;
}

export interface WorkspaceConfig {
  setup: string[];
  teardown: string[];
  run: string | null;
}

export interface ProjectScripts {
  setup: string[];
  teardown: string[];
  run: string | null;
}

export interface DetectedSetup {
  id: string;
  label: string;
  command: string;
  enabled: boolean;
}

// ── Theme ──

export interface ThemeColors {
  accent: string;
  cursor: string;
  foreground: string;
  background: string;
  selection_foreground: string;
  selection_background: string;
  color0: string;
  color1: string;
  color2: string;
  color3: string;
  color4: string;
  color5: string;
  color6: string;
  color7: string;
  color8: string;
  color9: string;
  color10: string;
  color11: string;
  color12: string;
  color13: string;
  color14: string;
  color15: string;
}

export interface ShellAppearance {
  font_family: string;
}

// ── Search / Files ──

export interface SearchResult {
  file_path: string;
  line_number: number;
  line_content: string;
  match_start: number;
  match_end: number;
}

export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number | null;
}
