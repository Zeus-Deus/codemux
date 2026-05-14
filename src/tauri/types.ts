// ── Feature flags ──

export interface FeatureFlags {
  unstable_openflow: boolean;
  unstable_browser_automation: boolean;
  unstable_indexing: boolean;
  enable_agent_chat: boolean;
  enable_lazy_workspace_creation: boolean;
}

// ── Synced Settings ──

export interface AppearanceSettings {
  theme: string;
  shell_font: string | null;
  terminal_font_size: number;
  show_resource_monitor: boolean;
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

export interface FileTreeSyncSettings {
  show_hidden_files: boolean;
}

export interface SessionRestoreSettings {
  enabled: boolean;
  scrollback_lines: number;
  max_total_mb: number;
}

export interface UserSettings {
  appearance: AppearanceSettings;
  editor: EditorSettings;
  terminal: TerminalSyncSettings;
  git: GitSyncSettings;
  keyboard: KeyboardSettings;
  notifications: NotificationSyncSettings;
  file_tree: FileTreeSyncSettings;
  session_restore: SessionRestoreSettings;
}

// ── Resource Monitor ──
// Mirrors src-tauri/src/resource_metrics.rs. `cpu` is a percentage where
// 100 == one core fully busy (it can exceed 100 across cores). `memory`
// values are resident bytes.

export interface ResourceUsageValues {
  cpu: number;
  memory: number;
}

export interface ResourceSessionMetrics {
  session_id: string;
  pane_id: string;
  pid: number;
  title: string | null;
  cpu: number;
  memory: number;
}

export interface ResourceWorkspaceMetrics {
  workspace_id: string;
  project_id: string;
  project_name: string;
  workspace_name: string;
  cpu: number;
  memory: number;
  sessions: ResourceSessionMetrics[];
}

export interface ResourceAppMetrics {
  cpu: number;
  memory: number;
  main: ResourceUsageValues;
  web_view: ResourceUsageValues;
  other: ResourceUsageValues;
}

export interface ResourceHostMetrics {
  total_memory: number;
  free_memory: number;
  used_memory: number;
  memory_usage_percent: number;
  cpu_core_count: number;
  load_average_1m: number;
}

export interface ResourceMetricsSnapshot {
  app: ResourceAppMetrics;
  workspaces: ResourceWorkspaceMetrics[];
  host: ResourceHostMetrics;
  total_cpu: number;
  total_memory: number;
  collected_at: number;
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
  original_command: string | null;
  adapter_captures: Record<string, string>;
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
  agent_session_name?: string | null;
}

export interface AgentBrowserSession {
  session_id: string;
  workspace_id: string;
  cli_session_name: string;
  stream_url: string;
  current_url: string | null;
  is_active: boolean;
  pane_id: string | null;
  browser_id: string | null;
  user_dismissed: boolean;
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

export interface BranchDetail {
  name: string;
  last_commit_unix: number;
  is_local: boolean;
  is_remote: boolean;
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
  /** Stage 5 — populated by `get_github_pr_by_path` only. List paths
   *  leave it null so list queries stay cheap. Body is truncated at
   *  50 KB on a char boundary, mirroring the issue path. */
  body: string | null;
  /** First 20 conversation comments. Empty for list rows. */
  comments: IssueComment[];
  /** Total conversation count — equals `comments.length` when under
   *  the cap, exceeds it when truncated. */
  totalComments: number;
  /** PR author login. Null when gh JSON didn't carry the field
   *  (e.g. ghost users / list rows that didn't request author). */
  author: string | null;
}

export interface IncomingPrItem {
  number: number;
  title: string;
  author: string;
  head_branch: string | null;
  is_draft: boolean;
  additions: number | null;
  deletions: number | null;
  review_decision: string | null;
  checks_status: string | null;
  updated_at: string | null;
  url: string;
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

export type IssueState = "Open" | "Closed";

export interface IssueComment {
  author: string;
  body: string;
  createdAt: string;
}

export interface GitHubIssue {
  number: number;
  title: string;
  state: IssueState;
  labels: string[];
  assignees: string[];
  url: string;
  body: string | null;
  /** Stage 4 — populated by the detail fetch only. List queries return
   *  an empty array. Capped at 20 by the backend. */
  comments: IssueComment[];
  /** Total comment count (may exceed `comments.length` when the
   *  backend truncated the visible slice). */
  totalComments: number;
  /** ISO8601 last-update timestamp. Used to sort the issue popup by
   *  recency. Null when `gh` didn't include the field. */
  updatedAt: string | null;
}

export interface LinkedIssue {
  number: number;
  title: string;
  state: IssueState;
  labels: string[];
}

export type GhStatus =
  | { status: "NotInstalled" }
  | { status: "NotAuthenticated" }
  | { status: "Authenticated"; username: string };

export type TabKind = "terminal" | "browser" | "diff" | "editor";

export interface TabSnapshot {
  tab_id: string;
  kind: TabKind;
  title: string;
  surface_id: string | null;
  browser_id: string | null;
  icon: string | null;
}

export type AgentChatProviderKind = "claude" | "codex" | "opencode";

/** Step 12 Stage 1 — driver-equals-instance shim (`"claude"` /
 *  `"codex"` / `"opencode"`). v2 lifts this to a richer
 *  `(driver, instance)` map; today it round-trips with
 *  `AgentChatProviderKind` so any payload carrying either field
 *  decodes interchangeably. */
export type ProviderInstanceId = AgentChatProviderKind;

/** Step 12 Stage 1 — diagnostic snapshot returned by
 *  `opencode_check_availability`. Mirrors the Rust struct in
 *  `src-tauri/src/agent_provider/opencode/discovery.rs` field for
 *  field. The picker surface in later stages reads this verbatim;
 *  Stage 1 only wires the wrapper so consumers can land without
 *  schema churn when the live integration ships. */
export interface OpenCodeAvailability {
  installed: boolean;
  binary_path: string | null;
  version: string | null;
  server_running: boolean;
  server_url: string | null;
}

/** Step 12 Stage 2 — one model entry as returned by
 *  `opencode_list_models`. Mirrors the Rust `OpenCodeModel`
 *  struct in `src-tauri/src/agent_provider/opencode/client.rs`
 *  and is itself a flattened view over OpenCode's `/provider`
 *  envelope (the wire-format `Model` carries far more — cost,
 *  capabilities, release date — that Codemux does not need
 *  yet). Stage 3 maps each entry into `ChatModelInfo`. */
export interface OpenCodeModel {
  id: string;
  name: string;
  description: string | null;
  variants: string[];
  context_window: number | null;
}

/** Step 12 Stage 2 — one provider entry from
 *  `opencode_list_models`. Each entry is a flattened view over
 *  one `Provider` block from the `GET /provider` response, with
 *  `connected` derived from the top-level `connected: string[]`
 *  array. The picker rebuild (Stage 4) treats this list as the
 *  source of truth for the OpenCode model menu. */
export interface OpenCodeProviderEntry {
  id: string;
  name: string;
  connected: boolean;
  models: Record<string, OpenCodeModel>;
}

// ── Chat-side provider capabilities ───────────────────────────────────

export type EffortGranularity = "per_session" | "per_turn";

export interface ContextWindowOption {
  value: string;
  label: string;
  is_default: boolean;
}

export interface ChatModelInfo {
  id: string;
  label: string;
  description: string | null;
  effort_levels: string[];
  default_effort: string | null;
  prompt_injected_effort_levels: string[];
  context_window_options: ContextWindowOption[];
  supports_adaptive_thinking: boolean;
  supports_thinking_toggle: boolean;
  supports_fast_mode: boolean;
  /** Step 8 Stage 6 — true when the model accepts image attachments.
   *  Drives the `+ → Image…` enable state and whether the composer's
   *  paste/drop handlers stage attachments at all. */
  supports_images: boolean;
  /** Step 12 Stage 3 — for federated providers (OpenCode), the
   *  upstream provider id this model belongs to (e.g. `"openai"`,
   *  `"anthropic"`, `"openrouter"`). `null` for direct providers
   *  (Claude, Codex) where the driver IS the provider. Drives the
   *  picker's grouping rail and the secondary label rendered below
   *  the model name. */
  sub_provider: string | null;
  /** True when the model is free-tier on the upstream provider's
   *  configured plan (both input and output token costs are 0).
   *  Drives a "FREE" pill in the picker and a soft sort boost so
   *  free models bubble to the top of their provider's list, after
   *  favorites. Today only OpenCode federated entries can ever set
   *  this — Claude / Codex always emit `false`. */
  is_free: boolean;
}

export interface PermissionModeOption {
  value: string;
  label: string;
  description: string;
  is_default: boolean;
}

export interface ProviderChatCapabilities {
  models: ChatModelInfo[];
  effort_granularity: EffortGranularity;
  effort_label_map: Record<string, string>;
  permission_modes: PermissionModeOption[];
  default_permission_mode: string | null;
  permission_granularity: EffortGranularity;
}

export type PaneNodeSnapshot =
  | { kind: "terminal"; pane_id: string; session_id: string; title: string }
  | { kind: "browser"; pane_id: string; browser_id: string; title: string }
  | {
      kind: "agent_chat";
      pane_id: string;
      title: string;
      thread_id: string | null;
      provider: AgentChatProviderKind | null;
      cwd: string | null;
    }
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

export type WorkspaceType = "standard" | "open_flow" | "home";

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
  linked_issue: LinkedIssue | null;
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
  ai_commit_message_cli: string | null;
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
  agent_browser_sessions: AgentBrowserSession[];
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

/** Mirror of the Rust `PresetKind` enum. `cli` is the classic
 *  terminal-launch preset; `chat_agent` is a native agent-chat preset
 *  that spawns an `agent_chat` pane instead of a CLI subprocess. The
 *  frontend `materializeWithPreset` dispatches on this field. */
export type PresetKind = "cli" | "chat_agent";

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
  /** Defaults to `"cli"` on the Rust side for presets persisted
   *  before this field existed. */
  kind: PresetKind;
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
  worktree_includes: string[];
}

export interface ProjectScripts {
  setup: string[];
  teardown: string[];
  run: string | null;
  worktree_includes: string[];
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
  is_gitignored: boolean;
}

/** Result row from `list_project_files` (Step 8 Stage 1). Used for the
 *  `@` mention popup and `+ → File…` picker in the chat composer. */
export interface FileMatch {
  /** Path relative to cwd. */
  path: string;
  /** Absolute, canonicalized path. */
  absolute_path: string;
  /** Fuzzy-match score; higher is better. `0` for empty-query / alphabetical. */
  score: number;
}

/** Top-level outline entry surfaced when a large file is truncated.
 *  Step 8 Stage 2: regex-extracted declarations per language; Stage 7
 *  promotes to tree-sitter for richer extraction. */
export interface OutlineEntry {
  /** Coarse declaration kind: "function", "class", "type", "trait",
   *  "impl", "mod", "enum", "struct", "interface", "heading", etc.
   *  Free-form by design — UI just displays as-is. */
  kind: string;
  /** Symbol name (the captured identifier or, for headings, the
   *  heading text). */
  name: string;
  /** 1-indexed line number where the declaration was found. */
  line: number;
}

/** Result row from `list_project_folders` (Step 8 Stage 3). Used for
 *  the `+ → Folder…` picker. Derives directory prefixes from the
 *  same file-walk cache as `FileMatch`. */
export interface FolderMatch {
  /** Path relative to cwd, no trailing slash. */
  path: string;
  /** Absolute, canonicalized path. */
  absolute_path: string;
  /** Fuzzy-match score; `0` for empty-query / alphabetical. */
  score: number;
  /** Immediate-children count (files + dirs at depth 1). */
  item_count: number;
}

/** Backend response for `read_folder_for_attachment` (Step 8 Stage 3).
 *  Carries a pre-rendered unicode tree (depth-bounded) plus the true
 *  pre-truncation item count. */
export interface FolderAttachmentInfo {
  absolutePath: string;
  relativePath: string | null;
  /** Pre-rendered unicode tree, depth-bounded by the caller's
   *  `max_depth`. Truncated at 100 entries with a trailing
   *  "… N more entries" marker. */
  tree: string;
  itemCount: number;
}

/** Backend response for `read_file_for_attachment` (Step 8 Stage 2).
 *  Reports text/binary, full-vs-truncated content, and an outline
 *  when truncated. The frontend's chip preview + injection block both
 *  derive from this struct. */
export interface FileAttachmentInfo {
  /** Echo of the input absolute path — agent uses this for Read/Grep. */
  absolutePath: string;
  /** Path relative to `cwd` if the file is under it; else null. */
  relativePath: string | null;
  /** Total line count of the file (not of the truncated preview). */
  lineCount: number;
  /** File size in bytes. */
  bytes: number;
  /** File extension as a hint for code fences. `null` for extension-less files. */
  language: string | null;
  /** False for binary files (null-byte sniff in first 8KB). */
  isText: boolean;
  /** Full text for ≤200KB / ≤1500-line files; first-50-lines preview for
   *  larger files; empty for binaries. */
  content: string;
  /** True when `content` is a preview, not the full file. */
  truncated: boolean;
  /** Top-level declarations extracted when truncated. Null otherwise. */
  outline: OutlineEntry[] | null;
}
