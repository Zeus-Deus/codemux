// ── Feature flags ──

export interface FeatureFlags {
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

/** Mirrors src-tauri/src/settings_sync.rs:SourceControlSettings.
 *  `custom_hosts` maps a bare hostname to a hosting product
 *  (`{"git.acme.internal": "gitlab"}`) for self-hosted instances whose
 *  domain gives nothing away. It is the highest-priority input to
 *  backend provider detection. Edited in Settings → Source Control. */
export interface SourceControlSyncSettings {
  custom_hosts: Record<string, string>;
}

/** One hosting product's readiness, from `discover_source_control`.
 *  Mirrors src-tauri/src/commands/source_control.rs:ProviderDiagnostic.
 *
 *  Probe results only — the product name, CLI name, login command and
 *  install URL come from the presentation map in
 *  `src/lib/source-control.ts` so copy has one source of truth. */
export interface ProviderDiagnostic {
  /** Wire form of the backend's ProviderKind: "github", "gitlab", … */
  kind: string;
  /** False for products Codemux has no adapter for; those rows render
   *  dimmed and are never probed. */
  supported: boolean;
  cli_installed: boolean;
  /** First line of `<cli> --version`, or null when missing/timed out. */
  cli_version: string | null;
  authenticated: boolean;
  /** Account the CLI reports, when it names one. Never a token. */
  account: string | null;
  /** One sanitized sentence explaining a non-ready state. */
  detail: string | null;
  /** What the adapter serves. Declared by the backend adapter so the UI
   *  does not restate it. All-false on an unsupported row. */
  capabilities: ProviderCapabilities;
}

/** Mirrors src-tauri/src/git_provider/provider.rs:Capabilities. */
export interface ProviderCapabilities {
  has_pull_requests: boolean;
  has_checks: boolean;
  has_issues: boolean;
  has_inline_comments: boolean;
  has_review_threads: boolean;
  has_deployments: boolean;
  has_reviews: boolean;
  has_fork_pr_fetch: boolean;
}

/** Host-scoped readiness for one checkout.
 *  Mirrors src-tauri/src/commands/source_control.rs:ProviderAuthStatus.
 *
 *  Unlike `ProviderDiagnostic`, this is scoped to a repository: it
 *  answers "can Codemux act on *this* checkout right now", which for a
 *  self-hosted instance is a different question from whether the CLI is
 *  logged in to some other instance. */
export interface ProviderAuthStatus {
  /** Wire form of the detected ProviderKind; "unknown" when nothing
   *  classified, which is an answer rather than an error. */
  kind: string;
  /** Does Codemux ship an adapter for this checkout's product? */
  supported: boolean;
  installed: boolean;
  authenticated: boolean;
  /** Account the CLI reports, when it names one. Never a token. */
  username: string | null;
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

/** Mirrors src-tauri/src/settings_sync.rs:AgentChatSettings.
 *  `checkpoints_enabled` is the opt-in for the run-start rollback
 *  checkpoint (issue #80) — default OFF.
 *  `background_browser_desktop_viewport` pins the GUI-mode background
 *  browser's peek popover (`BrowserPeekOverlay.tsx`) to a real desktop
 *  viewport (1280×800) instead of shrinking to the popover's pixel
 *  size — default ON. */
export interface AgentChatSyncSettings {
  checkpoints_enabled: boolean;
  background_browser_desktop_viewport: boolean;
}

/** Mirrors src-tauri/src/settings_sync.rs:BrowserSettings.
 *  `default_viewport` is the preferred starting viewport for
 *  agent-browser sessions — a `"WxH"` string like `"2560x1440"` (or a
 *  preset name) applied to freshly launched daemons and used as the
 *  `viewport reset` target, so agent screenshots match the user's own
 *  screen proportions. `null` (default) keeps the built-in 1280×800
 *  baseline. */
export interface BrowserSyncSettings {
  default_viewport: string | null;
}

export interface UserSettings {
  appearance: AppearanceSettings;
  editor: EditorSettings;
  terminal: TerminalSyncSettings;
  git: GitSyncSettings;
  source_control: SourceControlSyncSettings;
  keyboard: KeyboardSettings;
  notifications: NotificationSyncSettings;
  file_tree: FileTreeSyncSettings;
  session_restore: SessionRestoreSettings;
  agent_chat: AgentChatSyncSettings;
  browser: BrowserSyncSettings;
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
  // "migrating" is a transient cloud-push / pull-back state — shown as a
  // "Switching to <host>…" overlay between the old PTY dying and the
  // replacement session's first output. It only ever rides this event;
  // the persisted session snapshot collapses it to "starting".
  state: "starting" | "migrating" | "ready" | "exited" | "failed";
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
  /** The session is hosted by the right-panel deck's `browser` pane rather
   *  than by a pane-tree node. Mutually exclusive with `pane_id`; together
   *  they mean "the user can already see this browser", which is what the
   *  background chip / peek overlay and the backend's auto-pane gate test.
   *  Optional because older persisted snapshots predate the field. */
  right_panel_docked?: boolean;
}

// ── Pane Status ──

/** Mirror of `state_impl.rs::PaneStatus`. Priority ladder (see
 *  `src/lib/pane-status.ts`): permission > working > monitoring > review > idle.
 *
 *  `monitoring` is the calm one — the agent finished its deliverable but is
 *  still watching something in the background (a CI run, a tailed process, a
 *  PR poll). Nothing that renders it animates. */
export type PaneStatus =
  | "idle"
  | "working"
  | "permission"
  | "monitoring"
  | "review";
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

// ── Workspace & Layout ──

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

/** Launch-time model / reasoning / context choice for a CLI agent
 *  preset. Every field is optional — `null` means "use the agent's own
 *  default" and emits no flag. `context` is Claude-only (`"1m"` opts
 *  into the 1M window). Mirrors the Rust `agent_capability::ModelSelection`
 *  consumed by `apply_preset` / `create_worktree_workspace`. */
export interface ModelSelection {
  model: string | null;
  reasoning: string | null;
  context: string | null;
}

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
  /** Numeric window size in tokens for this option (e.g. 200_000 for
   *  `"200k"`, 1_000_000 for `"1m"`). Seed value for the context meter's
   *  first paint only — the provider's own runtime report always wins.
   *  Optional: absent/null on older payloads or when unknown. */
  context_window_tokens?: number | null;
}

export interface ChatModelInfo {
  id: string;
  label: string;
  description: string | null;
  effort_levels: string[];
  default_effort: string | null;
  /** Per-effort human descriptions reported by the provider's own model
   *  catalog, keyed by effort level. Rendered as each effort row's
   *  second line in the reasoning picker, winning over the built-in
   *  fallback text. Absent/empty when the provider ships no blurbs
   *  (Claude, OpenCode) — optional so older payloads still decode. */
  effort_descriptions?: Record<string, string>;
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
  /** Numeric context-window size in tokens for models with no
   *  selectable window options (OpenCode harvests this from the
   *  upstream catalog's `limit.context`). Seed for the context meter's
   *  first paint; the provider's runtime report always wins. Absent/
   *  null = unknown — the meter degrades to a bare token count rather
   *  than guessing. */
  max_context_tokens?: number | null;
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

export type WorkspaceType = "standard" | "home";

export interface WorkspaceSnapshot {
  workspace_id: string;
  title: string;
  workspace_type: WorkspaceType;
  cwd: string;
  /** Whether the workspace directory is inside a git repository. Non-git
   *  folders run in plain-directory mode (no worktrees/diffs/checkpoints)
   *  and the UI offers an explicit "Initialize Git" action. Optional —
   *  older snapshots persisted without it; treat missing as `true`
   *  (optimistic, matching the Rust serde default). */
  is_git?: boolean;
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
  /** Deterministic project identity (UUIDv5 of canonical remote or
   *  project root); stamped at create. Optional — older snapshots
   *  persisted without it. */
  project_uid?: string | null;
  /** "main" (repo root checkout) | "worktree". Optional for the same
   *  reason. */
  workspace_kind?: string | null;
  /** True when this is the repo's protected default-branch root
   *  checkout — the overview must not let it be deleted like a
   *  disposable worktree. Divergence-safe: a full copy living under
   *  ~/.codemux/worktrees/ is NOT protected. Defaults to false on
   *  older snapshots. */
  protected?: boolean;
  /** True when this checkout is a legacy divergent full copy of a repo
   *  (its own object store) sitting in the worktrees tree — not linked
   *  to the real repo's history. The overview warns so the user can
   *  re-pull cleanly. Defaults to false. */
  divergent_copy?: boolean;
  pr_number: number | null;
  pr_state: string | null;
  pr_url: string | null;
  /** Head branch of the associated PR, as GitHub reports it.
   *
   *  Usually equal to `git_branch`. It differs when the badge came from the
   *  backend's side-branch fallback — a PR opened from a branch this worktree
   *  checked out recently and then left. The badge is worth showing either
   *  way; the *inferences* drawn from a PR are not, so auto-settle requires
   *  this to match the checked-out branch (see `isPrOnCurrentBranch`).
   *
   *  Optional because older persisted snapshots have no such field; `null` /
   *  absent is read as the pre-field case and settles as before. */
  pr_head_branch?: string | null;
  /** Which hosting product this checkout's remotes point at — `"github"`,
   *  `"gitlab"`, `"bitbucket"`, `"azure_devops"`. `null` when there is no
   *  remote or the host isn't recognised.
   *
   *  Re-derived by the backend pollers alongside the PR pill, never
   *  synced. Optional for older snapshots; absent means GitHub wording
   *  for back-compat (see src/lib/source-control.ts). */
  provider_kind?: string | null;
  linked_issue: LinkedIssue | null;
  /** When true, agent-completion desktop notifications for this workspace's
   *  panes are suppressed. Status pills (spinner/dots) are unaffected. */
  notifications_muted: boolean;
  /** Ms epoch when this workspace was pinned to the top of the workspace
   *  inbox. Pinning overrides settled/snoozed presentation without deleting
   *  that underlying lifecycle state. Optional for older snapshots. */
  pinned_at?: number | null;
  tabs: TabSnapshot[];
  active_tab_id: string;
  active_surface_id: string;
  surfaces: SurfaceSnapshot[];
  /** Cloud-push (step 2b+): which host this workspace runs on. `null`
   *  means local. Refers to the local `hosts` table id. Optional in
   *  the TS type because older snapshots persisted without the field
   *  and the Rust side falls back to `None` via serde default. */
  host_id?: number | null;
  /** The workspace's real working directory ON ITS HOST. Set for
   *  "open on host" / attach-in-place workspaces (terminals spawn here
   *  on the remote). `null`/undefined for local + pushed workspaces. */
  remote_cwd?: string | null;
  /** True when this workspace is operated in place on its host with no
   *  local copy of the files (`cwd` is a host path). The UI shows a
   *  host badge and offers only "Close (detach)" — never delete/push.
   *  Defaults to false on older snapshots. */
  attach_only?: boolean;
  /** Ms epoch of the last time an agent genuinely did something in this
   *  workspace — stamped by the backend whenever one of its panes goes
   *  non-idle (working / permission / review), and at create time. This
   *  is the timestamp the idle sweep must measure against: it is
   *  persisted, so it survives reinstalls instead of restarting the
   *  clock. `null` means "unknown" (nothing on disk could date the
   *  checkout) and must never be treated as "idle since forever".
   *  Optional in TS because older snapshots persisted without it. */
  last_active_at?: number | null;
  /** Ms epoch of the last time the user had this workspace focused.
   *  Deliberately separate from `last_active_at` — a glance is not agent
   *  work, so it must not keep dead work out of the idle sweep.
   *  Optional for the same backward-compat reason. */
  last_visited_at?: number | null;
}

/** A workspace that was archived out of the sidebar. The entry records
 *  everything needed to restore the workspace later (or delete its
 *  worktree/branch for good); the files, branch, and worktree on disk
 *  are untouched by the archive action itself. */
export interface ArchivedWorkspaceSnapshot {
  archive_id: string;
  workspace_id: string;
  title: string;
  cwd: string;
  worktree_path: string | null;
  project_root: string | null;
  project_uid: string | null;
  workspace_kind: "main" | "worktree" | null;
  git_branch: string | null;
  /** True for the repo's protected default-branch root checkout — its
   *  files can never be deleted through the archive, only the entry
   *  itself can be removed. */
  protected: boolean;
  is_git: boolean;
  /** Unix seconds when the workspace was archived. */
  archived_at: number;
  /** Preserve the live workspace's pin across archive/restore. */
  pinned_at?: number | null;
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
  /** Discovery source: `null` for the OS scan, `"docker"` for a published
   *  container port. Drives the dedicated "Docker" group in the ports UI. */
  source: string | null;
}

export interface AppStateSnapshot {
  schema_version: number;
  /** Monotonic per-emit counter stamped by the backend. Strictly increasing
   *  across emitted snapshots, so a snapshot whose revision is not above the
   *  last applied one is stale and dropped (`app-store.ts`). Optional: state
   *  restored from disk and older backends carry no revision, which reads as
   *  0 — "unrevisioned, always apply". */
  snapshot_revision?: number;
  /** The backend process that stamped `snapshot_revision`. The counter is
   *  process-lifetime and restarts at 0, so a revision is only comparable
   *  against another from the SAME instance — a token change means "the
   *  counter restarted", not "this message is stale". Absent/empty on a
   *  restored layout, a mock, or an older backend (all of which carry
   *  revision 0 and are applied unconditionally). */
  snapshot_instance?: string;
  active_workspace_id: string;
  workspaces: WorkspaceSnapshot[];
  terminal_sessions: TerminalSessionSnapshot[];
  browser_sessions: BrowserSessionSnapshot[];
  agent_browser_sessions: AgentBrowserSession[];
  notifications: NotificationSnapshot[];
  detected_ports: PortInfoSnapshot[];
  pane_statuses: Record<string, PaneStatus>;
  /** Panes an agent flagged via `codemux monitor start`, mapped to the
   *  optional reason it gave. The backend has already folded these into
   *  `pane_statuses` as `monitoring`, so this map exists only so a surface
   *  can show the *reason* (the docked monitoring bar's title). Runtime-only
   *  and never persisted; optional because older snapshots lack it. */
  manual_monitors?: Record<string, string | null>;
  /** Archived (hidden-but-restorable) workspaces. Optional — older
   *  snapshots persisted without the field; treat missing as `[]`. */
  archived_workspaces?: ArchivedWorkspaceSnapshot[];
  persistence: PersistenceSchema;
  config: CodemuxConfigSnapshot;
}

// ── Domain deltas ──
//
// Mirror of `AppStateDelta` / `RevisionedDelta` in
// src-tauri/src/state/state_impl.rs. High-frequency metadata refreshes ship
// one of these on `app-state-delta` instead of a full `AppStateSnapshot`;
// the snapshot stays the boot / resync / activation vehicle.
//
// Deltas and snapshots share ONE revision counter, so the renderer sees a
// single totally ordered stream: a delta at revision N reflects the backend
// state at N, and a snapshot at N supersedes every delta at or below N. No
// variant carries `active_workspace_id` — deliberately, so a background
// refresh can never clobber an optimistic selection.

/** The git-metadata subset of `WorkspaceSnapshot` carried by a
 *  `workspace_git` delta. Field names match the snapshot's own, so applying
 *  the delta is a spread. */
export interface WorkspaceGitDelta {
  is_git: boolean;
  git_branch: string | null;
  git_ahead: number;
  git_behind: number;
  git_additions: number;
  git_deletions: number;
  git_changed_files: number;
}

export type AppStateDelta =
  | { domain: "workspace_git"; workspace_id: string; git: WorkspaceGitDelta }
  | { domain: "detected_ports"; ports: PortInfoSnapshot[] }
  | {
      domain: "pane_status";
      pane_id: string;
      /** `null` clears the pane's entry — the map only stores non-idle panes. */
      status: PaneStatus | null;
    };

/** Payload of the `app-state-delta` event. */
export interface RevisionedDelta {
  revision: number;
  /** The backend process that stamped `revision` — see
   *  `AppStateSnapshot.snapshot_instance`. A delta from a restarted backend
   *  has no valid baseline to patch and opens a resync instead. */
  instance?: string;
  delta: AppStateDelta;
}

/** Payload of the `app-state-revision` heartbeat: the backend's current
 *  revision counter, with no snapshot attached. */
export interface RevisionHeartbeat {
  revision: number;
  /** The backend process that owns `revision` — see
   *  `AppStateSnapshot.snapshot_instance`. A token change is a resync on its
   *  own, whatever the revision numbers say. */
  instance?: string;
}

// ── Presets ──

export type LaunchMode = "split_pane" | "new_tab";

/** Mirror of the Rust `PresetKind` enum. `cli` is the classic
 *  terminal-launch preset; `chat_agent` is a native agent-chat preset
 *  that spawns an `agent_chat` pane instead of a CLI subprocess. The
 *  frontend `materializeWithPreset` dispatches on this field. */
export type PresetKind = "cli" | "chat_agent";

/** Structured "agent launcher" configuration (mirror of the Rust
 *  `presets::PresetLaunchConfig`). Present when a preset was built with the
 *  structured editor; `null` for raw command presets. The model/reasoning/
 *  context are NOT baked into the command — they ride in `model_selection`
 *  and are applied at launch via `apply_model_selection` (the same path the
 *  New Workspace dialog uses). The prompt is baked into
 *  `TerminalPreset.commands[0]` as a trailing positional arg. */
export interface PresetLaunchConfig {
  /** Base agent invocation (binary + autonomy flag), no prompt or model. */
  agent_command: string;
  /** Model / reasoning / context, applied at launch. */
  model_selection: ModelSelection;
  /** Free-form prompt passed to the agent (may be empty). */
  prompt: string;
}

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
  /** Structured agent-launcher config, when built with the structured
   *  editor. `null`/absent for raw command presets. */
  launch_config?: PresetLaunchConfig | null;
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

// ── Web Remote Access ─────────────────────────────────────────────────
//
// Wire types for the embedded web-remote server (default-off HTTP+WS
// second frontend). These mirror the Rust structs in
// `src-tauri/src/web_remote/` exactly — serde serializes them
// snake_case, so every field name below matches the Rust field name.
// The `web-remote-state-changed` event payload is `WebRemoteStatus`.

/** Persisted, user-controlled server config. Mirrors
 *  `web_remote::WebRemoteConfig`. */
export interface WebRemoteConfig {
  enabled: boolean;
  port: number;
  require_approval: boolean;
  /** Which interfaces the server binds: `all` (0.0.0.0) | `tailscale`
   *  (tailnet + loopback) | `loopback` (this device only). Optional so a
   *  config persisted before the field existed loads as `all`; the Rust
   *  payload always sends it. */
  bind_scope?: WebRemoteBindScope;
  /** Account mode (Stage A) master toggle. When on, a browser can sign in with
   *  the desktop's Codemux account via `POST /api/pair-account` instead of a
   *  pairing code. Optional so legacy configs load as off. */
  account_mode_enabled?: boolean;
  /** "Trust browsers on my account without approval" opt-out. When off
   *  (default), account-minted sessions start pending approval regardless of
   *  `require_approval`; when on they connect immediately. */
  trust_account_browsers?: boolean;
  /** Master toggle for the from-anywhere iroh (relay) transport. When on and
   *  the server is enabled, the desktop binds an iroh endpoint reachable by
   *  `node_id`. Optional so legacy configs load as off. */
  relay_mode_enabled?: boolean;
}

/** Bind-scope choices for the remote server. Mirrors the `BIND_SCOPE_*`
 *  constants in `web_remote::mod`. */
export type WebRemoteBindScope = "all" | "tailscale" | "loopback";

/** A paired device row as shown in the device-management UI. Mirrors
 *  `web_remote::SessionView`. */
export interface WebRemoteSessionView {
  id: string;
  /** Device name the browser reported at pairing time. */
  name: string | null;
  /** Raw `User-Agent` captured at pairing time. */
  user_agent: string | null;
  /** RFC3339 timestamp. */
  created_at: string;
  /** RFC3339 timestamp, or null if it never reconnected. */
  last_seen_at: string | null;
  /** Approved to connect. `false` = pending approval. */
  approved: boolean;
  /** Has at least one live WebSocket right now. */
  connected: boolean;
  /** How the device was admitted: `"pair"` (pairing token) or `"account"`
   *  (signed into the desktop's Codemux account). Optional so legacy fixtures
   *  need not enumerate it; the Rust payload always sends it. */
  source?: string;
}

/** Live server + device state. Returned by `web_remote_status` and
 *  emitted as the `web-remote-state-changed` payload. Mirrors
 *  `web_remote::WebRemoteStatus`. */
export interface WebRemoteStatus {
  enabled: boolean;
  running: boolean;
  port: number;
  require_approval: boolean;
  /** Which interfaces the server binds: `all` | `tailscale` | `loopback`.
   *  Optional so existing status fixtures need not enumerate it; the Rust
   *  payload always sends it. */
  bind_scope?: WebRemoteBindScope;
  /** Raw live-WebSocket count (a single device with two tabs counts
   *  twice). */
  active_connections: number;
  /** Distinct paired devices with at least one live WebSocket right now.
   *  This is the "remote sessions active" signal the desktop updater's
   *  defer-while-remote policy keys off (Stage 3b). */
  connected_sessions: number;
  sessions: WebRemoteSessionView[];
  /** Whether the desktop updater found an update ready to install. The
   *  desktop frontend publishes this (the web client has no updater plugin)
   *  so a paired browser can offer a "desktop update available" prompt.
   *  Optional so existing status fixtures need not enumerate it; the Rust
   *  payload always sends it. */
  update_available?: boolean;
  /** Version of the available desktop update, when `update_available`. */
  update_version?: string | null;
  /** Account mode (Stage A) master toggle. Optional so existing status fixtures
   *  need not enumerate it; the Rust payload always sends it. */
  account_mode_enabled?: boolean;
  /** The "trust account browsers without approval" opt-out. */
  trust_account_browsers?: boolean;
  /** Whether the desktop is currently signed into a Codemux account — account
   *  mode can only verify "same account" while it is. */
  account_signed_in?: boolean;
  /** Master toggle for the from-anywhere iroh (relay) transport (Stage C).
   *  Optional so existing status fixtures need not enumerate it; the Rust
   *  payload always sends it. */
  relay_mode_enabled?: boolean;
  /** The device's stable iroh `node_id` (a browser on the hosted origin dials
   *  this), when relay mode has ever been enabled. `null` otherwise. */
  iroh_node_id?: string | null;
  /** Whether this desktop is currently registered with the account device
   *  registry (discoverable by a browser signed into the same account by
   *  `node_id`). Only meaningful while relay mode is on and the desktop is
   *  signed in; `false` when signed out or the registry is unreachable.
   *  Optional so existing status fixtures need not enumerate it; the Rust
   *  payload always sends it. */
  device_registered?: boolean;
  /** The stable device id this desktop registers under, once a registration
   *  attempt has run. `null` before then. Mirrors the same-named field on
   *  {@link WebRemoteRegistrationStatus}. */
  device_id?: string | null;
}

/** Control-plane registration state for the from-anywhere iroh transport.
 *  Returned by `web_remote_registration_status`. Mirrors
 *  `web_remote::registration::RegistrationStatus` (serde default = snake_case).
 *  Registration is best-effort and only runs while relay mode is on and the
 *  desktop is signed into a Codemux account. */
export interface WebRemoteRegistrationStatus {
  /** The device is currently registered with the control plane. */
  registered: boolean;
  /** Stable per-install device id sent at registration, when known. */
  device_id?: string | null;
  /** Display name this device registers under — the machine hostname. Always
   *  present from a current backend (it is a local fact, so it is filled in
   *  even before the first registration attempt); optional here because an
   *  older backend omits the field entirely. */
  name?: string | null;
  /** The iroh `node_id` registered for this device, when known. */
  node_id?: string | null;
  /** RFC3339 timestamp of the last successful registration heartbeat. */
  last_registered_at?: string | null;
  /** Last registration error, for diagnostics (`null` when healthy). */
  last_error?: string | null;
}

/** Result of `web_remote_create_pairing`. QR rendering is the
 *  frontend's job. Mirrors `web_remote::PairingInfo`. */
export interface WebRemotePairingInfo {
  /** Relative link a browser opens to auto-pair: `/#pair=<token>`. */
  url_path: string;
  token: string;
  /** RFC3339 expiry timestamp. */
  expires_at: string;
}

/** One reachable place a browser could load the UI. Mirrors
 *  `web_remote::endpoints::Endpoint`. */
export interface WebRemoteEndpoint {
  /** `loopback` | `lan` | `tailnet` | `magicdns`. */
  kind: string;
  /** Coarse UI grouping: `this_device` | `local_network` | `tailscale` |
   *  `other`. Drives the labelled sections in the settings panel. */
  group: string;
  /** IP literal or DNS hostname (no scheme, no port). */
  host: string;
  port: number;
  /** Ready-to-copy `http(s)://host:port` URL. */
  url: string;
  /** Whether a browser treats this origin as a secure context. Only
   *  loopback qualifies over plain HTTP. */
  secure: boolean;
  /** The single best "reach from anywhere" endpoint, surfaced with a
   *  "Recommended" chip. At most one endpoint carries this. */
  recommended: boolean;
  /** Short human hint for the settings UI. */
  label: string;
}
