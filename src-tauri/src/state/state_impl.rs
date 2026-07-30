use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

const MAX_NOTIFICATIONS: usize = 500;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceType {
    Standard,
    OpenFlow,
    /// The Home singleton is being retired across the Stage A→E
    /// rework. Stage B keeps the variant intact (code paths like
    /// `get_or_create_home_workspace` still construct it). Stage E
    /// deletes it and, in the SAME commit, adds `#[serde(alias =
    /// "home")]` on `Standard` so legacy SQLite rows continue to
    /// deserialise. Adding the alias now would collide with this
    /// variant and trigger a `serde_derive`-emitted
    /// `unreachable_patterns` warning that neither enum- nor
    /// variant-level `#[allow]` silences — so the alias work is
    /// deferred to Stage E.
    Home,
}

impl Default for WorkspaceType {
    fn default() -> Self {
        WorkspaceType::Standard
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TabKind {
    Terminal,
    Browser,
    Diff,
    Editor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabSnapshot {
    pub tab_id: String,
    pub kind: TabKind,
    pub title: String,
    pub surface_id: Option<SurfaceId>,
    pub browser_id: Option<BrowserId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Debounces disk persistence so that rapid state changes (e.g. drag-swap +
/// multiple resize events) only result in a single write after a quiet period.
struct PersistDebouncer {
    pending: Arc<AtomicBool>,
    last_snapshot: Arc<Mutex<Option<AppStateSnapshot>>>,
}

impl PersistDebouncer {
    fn new() -> Self {
        Self {
            pending: Arc::new(AtomicBool::new(false)),
            last_snapshot: Arc::new(Mutex::new(None)),
        }
    }

    /// Queue a persist. If a write is already scheduled, just update the
    /// buffered snapshot — the background thread will pick up the latest value.
    fn schedule(&self, snapshot: AppStateSnapshot) {
        {
            let mut guard = self.last_snapshot.lock().unwrap();
            *guard = Some(snapshot);
        }

        // If a background task is already running, nothing more to do.
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }

        let pending = Arc::clone(&self.pending);
        let last_snapshot = Arc::clone(&self.last_snapshot);

        std::thread::spawn(move || {
            // Wait for the quiet period before writing.
            std::thread::sleep(Duration::from_millis(500));

            // Take the snapshot and clear the flag while still holding the
            // mutex. This ensures no second worker can slip through the
            // pending.swap guard between the flag clear and the file write.
            let snapshot = {
                let mut guard = last_snapshot.lock().unwrap();
                pending.store(false, Ordering::Release);
                guard.take()
            };

            if let Some(snapshot) = snapshot {
                if let Err(error) = save_persisted_state(&snapshot) {
                    eprintln!("[codemux::state] Failed to persist layout state: {error}");
                }
            }
        });
    }
}

static PERSIST_DEBOUNCER: std::sync::OnceLock<PersistDebouncer> = std::sync::OnceLock::new();

fn persist_debouncer() -> &'static PersistDebouncer {
    PERSIST_DEBOUNCER.get_or_init(PersistDebouncer::new)
}

/// Synchronously persist the current app state. Called on app close to
/// ensure the debounced write completes before the process exits.
pub fn flush_persisted_state(store: &AppStateStore) {
    let snapshot = store.snapshot();
    if let Err(e) = save_persisted_state(&snapshot) {
        eprintln!("[codemux::state] Failed to persist layout state on close: {e}");
    }
}

use crate::project::current_project_root;

const APP_STATE_SCHEMA_VERSION: u32 = 1;
const CODEMUX_CONFIG_VERSION: u32 = 1;
const PERSISTENCE_SCHEMA_VERSION: u32 = 1;
pub const MAX_TERMINAL_SESSIONS: usize = 8;
const DEFAULT_BROWSER_URL: &str = "about:blank";

#[derive(Debug, Clone)]
enum WorkspaceInsertBehavior {
    Horizontal,
    Smart,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePresetLayout {
    Single,
    Pair,
    Quad,
    Six,
    Eight,
    ShellBrowser,
    /// No surfaces, no tabs, no terminal sessions — an "empty shell"
    /// workspace whose panes are populated later by the caller
    /// (e.g. `agent_chat_create_pane` after an inline worktree
    /// creation). Matches the shape of
    /// [`create_empty_workspace_at_path`]; differs from it only in
    /// that the workspace also carries worktree metadata set by the
    /// bundled `create_worktree_workspace` command.
    Empty,
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct WorkspaceId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct SurfaceId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct PaneId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct BrowserId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSessionState {
    Starting,
    Ready,
    Exited,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSessionSnapshot {
    pub session_id: SessionId,
    pub title: String,
    pub shell: Option<String>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub state: TerminalSessionState,
    pub last_message: Option<String>,
    pub exit_code: Option<u32>,
    /// The command that was written to this terminal (e.g. from a preset).
    /// Used by the session adapter system for resume detection.
    #[serde(default)]
    pub original_command: Option<String>,
    /// Adapter-captured metadata (e.g. Claude session UUID from hooks).
    /// Written immediately when hooks fire, so each pane has its own data.
    #[serde(default)]
    pub adapter_captures: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSessionSnapshot {
    pub browser_id: BrowserId,
    pub title: String,
    pub current_url: Option<String>,
    pub history: Vec<String>,
    pub history_index: usize,
    pub is_loading: bool,
    pub last_error: Option<String>,
    /// When set, this pane is backed by an agent browser session. The pane
    /// must start its screencast daemon with this session name so it connects
    /// to the same Chromium instance the agent's MCP commands use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBrowserSession {
    pub session_id: String,
    pub workspace_id: WorkspaceId,
    /// Stable CLI --session name derived from workspace_id.
    pub cli_session_name: String,
    /// WebSocket stream URL (ws://localhost:9223 in this phase).
    pub stream_url: String,
    pub current_url: Option<String>,
    pub is_active: bool,
    /// Attached pane (None when detached after pane close).
    pub pane_id: Option<PaneId>,
    /// BrowserSessionSnapshot link (None when detached).
    pub browser_id: Option<BrowserId>,
    /// Set when user explicitly closes the agent's browser pane.
    /// Prevents auto-recreation until user manually reopens.
    #[serde(default)]
    pub user_dismissed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneStatus {
    Idle,
    Working,
    Permission,
    Review,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    Info,
    Attention,
}

impl NotificationLevel {
    /// Parse a level string as sent by the `notify` control command / MCP tool
    /// (schema: "info" | "attention" | "error"). "info" → Info; "attention",
    /// "error", and anything unrecognized → Attention (the most severe level
    /// the enum exposes — there is no dedicated Error variant).
    pub fn from_str_or_attention(s: &str) -> Self {
        match s {
            "info" => NotificationLevel::Info,
            _ => NotificationLevel::Attention,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSnapshot {
    pub notification_id: String,
    pub workspace_id: WorkspaceId,
    pub pane_id: Option<PaneId>,
    pub session_id: Option<SessionId>,
    pub level: NotificationLevel,
    pub message: String,
    pub read: bool,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaneNodeSnapshot {
    Terminal {
        pane_id: PaneId,
        session_id: SessionId,
        title: String,
    },
    Browser {
        pane_id: PaneId,
        browser_id: BrowserId,
        title: String,
    },
    /// Agent-chat pane. Feature-flagged behind
    /// [`FeatureFlags::enable_agent_chat`](crate::observability::FeatureFlags::enable_agent_chat);
    /// the stub renderer only shows a placeholder while the chat UI lands
    /// in a follow-up task.
    AgentChat {
        pane_id: PaneId,
        title: String,
        /// Thread identifier when a chat session has been started.
        /// `None` on a freshly-created pane that has not yet been bound
        /// to a session.
        #[serde(default)]
        thread_id: Option<String>,
        /// Provider family selected for this thread. `None` until the
        /// user picks one (or a default is applied in Step 3).
        #[serde(default)]
        provider: Option<crate::agent_provider::ProviderKind>,
        /// Working directory the thread should act on. `None` falls back
        /// to the workspace cwd at render time.
        #[serde(default)]
        cwd: Option<String>,
    },
    Split {
        pane_id: PaneId,
        direction: SplitDirection,
        child_sizes: Vec<f32>,
        children: Vec<PaneNodeSnapshot>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceSnapshot {
    pub surface_id: SurfaceId,
    pub title: String,
    pub root: PaneNodeSnapshot,
    pub active_pane_id: PaneId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub workspace_type: WorkspaceType,
    pub cwd: String,
    /// Whether this workspace's directory is inside a git repository
    /// (`find_git_root` resolves). Non-git folders are first-class:
    /// they run in plain-directory mode (no worktrees, no diffs, no
    /// checkpoints) and the UI offers an explicit "Initialize Git"
    /// action instead of hiding everything silently. Stamped by
    /// `populate_git_info` alongside the other git fields. Additive;
    /// old persisted snapshots deserialize as `true` (optimistic, like
    /// the pre-field behavior) and are corrected on the first refresh.
    #[serde(default = "default_true")]
    pub is_git: bool,
    pub git_branch: Option<String>,
    #[serde(default)]
    pub git_ahead: u32,
    #[serde(default)]
    pub git_behind: u32,
    #[serde(default)]
    pub git_additions: u32,
    #[serde(default)]
    pub git_deletions: u32,
    #[serde(default)]
    pub git_changed_files: u32,
    pub notification_count: u32,
    pub latest_agent_state: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub project_root: Option<String>,
    /// Stable, deterministic project identity — `UUIDv5(canonical git
    /// remote ?? project_root)`, see `crate::project_identity`. Stamped
    /// at create time (when `project_root` is set). Shared by a repo's
    /// main checkout and its worktrees, and identical on every device
    /// that has the same repo. Additive; old persisted state
    /// deserializes as `None`.
    #[serde(default)]
    pub project_uid: Option<String>,
    /// `"main"` (repo root checkout) | `"worktree"` (per-branch git
    /// worktree). Derived from `worktree_path` at create time.
    #[serde(default)]
    pub workspace_kind: Option<String>,
    /// True when this workspace is the repo's protected default-branch
    /// root checkout — the genuine main checkout (not a linked worktree,
    /// not a bare repo, and not a divergent full copy under
    /// `~/.codemux/worktrees/`) on its default branch. The overview must
    /// not let a protected workspace be deleted like a disposable
    /// worktree. Stamped at create time (and backfilled at boot) by
    /// `crate::git::is_protected_repo_root`. Additive; old persisted
    /// state deserializes as `false`.
    #[serde(default)]
    pub protected: bool,
    /// True when this workspace's checkout is a *divergent full copy* of
    /// a repo (its own `.git` object store) living in the worktrees tree
    /// — the legacy artifact of the old default-branch push/pull. It is
    /// NOT linked to the real repo's history and will drift from it. The
    /// overview surfaces a warning so the user can re-pull cleanly. New
    /// adoptions land repo roots under `~/.codemux/projects/` and never
    /// produce this. Additive; old snapshots read as `false`.
    #[serde(default)]
    pub divergent_copy: bool,
    #[serde(default)]
    pub pr_number: Option<u32>,
    #[serde(default)]
    pub pr_state: Option<String>,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub linked_issue: Option<crate::github::LinkedIssue>,
    /// When true, agent-completion desktop notifications for panes in this
    /// workspace are suppressed. Only the OS popup is gated — the status
    /// pills (working spinner, review/permission dots) are unaffected.
    #[serde(default)]
    pub notifications_muted: bool,
    #[serde(default)]
    pub tabs: Vec<TabSnapshot>,
    #[serde(default)]
    pub active_tab_id: String,
    pub active_surface_id: SurfaceId,
    pub surfaces: Vec<SurfaceSnapshot>,
    /// Which host this workspace runs on. `None` means local (this
    /// device). When set, refers to a row id in the local `hosts`
    /// table (the SQLite primary key, not the cloud server_id, so
    /// reassignment after sync just bumps the row's `server_id`
    /// without breaking workspace references).
    ///
    /// Added in step 2b of the cloud-push series. Strictly additive —
    /// existing persisted workspaces deserialize as `None` thanks to
    /// `#[serde(default)]`, and all today-shipping code paths treat
    /// `None` as "local" exactly as before.
    #[serde(default)]
    pub host_id: Option<i64>,
    /// The actual working directory of this workspace **on its host**.
    /// `None` for ordinary local workspaces and for pushed workspaces
    /// (which land at a deterministic `conventional_remote_path`). It is
    /// set for "open on host" / attach-in-place workspaces, where the
    /// host directory was discovered by the inventory poller and lives at
    /// an arbitrary path the desktop can't reconstruct. When set, the
    /// daemon-backed terminal spawns into this exact path on the remote
    /// instead of the reconstructed conventional path. Additive; old
    /// snapshots deserialize as `None`.
    #[serde(default)]
    pub remote_cwd: Option<String>,
    /// True when this workspace is operated **in place on its host with no
    /// local copy of the files**. Created by `workspace_open_on_host`. The
    /// `cwd`/`worktree_path` are host paths that do not exist on this
    /// device, so local-filesystem work (git info, worktree deletion,
    /// re-push) is skipped and the only teardown is a detach
    /// (`close_workspace` — never `close_workspace_with_worktree`), which
    /// leaves the host process running. Additive; old snapshots
    /// deserialize as `false`.
    #[serde(default)]
    pub attach_only: bool,
    /// Ms epoch of the last time an agent in this workspace genuinely did
    /// something — stamped when any of its panes transitions to a
    /// non-idle status (working / waiting on permission / finished
    /// review), and at create time.
    ///
    /// It is deliberately backend-owned and persisted: the sidebar's
    /// idle sweep decides whether to hide a workspace after N days of
    /// silence, and a stamp invented by the frontend the first time it
    /// laid eyes on a workspace resets that clock on every reinstall and
    /// throws away the real history it is supposed to be measuring.
    ///
    /// Workspaces persisted before this field existed get a one-time
    /// boot backfill (`backfill_workspace_activity`); `None` survives
    /// only when nothing on disk can date the checkout, and the frontend
    /// reads `None` as "unknown" and refuses to sweep on it. Additive;
    /// old persisted state deserializes as `None`.
    #[serde(default)]
    pub last_active_at: Option<i64>,
    /// Ms epoch of the last time the user had this workspace focused.
    /// Stamped on both edges of a switch — when the workspace becomes
    /// active, and again when the user switches away from it, since a
    /// visit that ends now is a visit that lasted until now. Stamping
    /// only the entry edge would mark work the user watched finish as
    /// unread the moment they leave it.
    ///
    /// Kept apart from `last_active_at` on purpose — looking at a
    /// workspace is not the agent working in it, so collapsing the two
    /// would let a glance keep dead work permanently unswept. Additive;
    /// old persisted state deserializes as `None`.
    #[serde(default)]
    pub last_visited_at: Option<i64>,
}

/// A workspace the user archived: the workspace itself is closed (no
/// sessions, no sidebar row) but its files stay on disk and enough
/// metadata is kept here to restore it later — or to delete its
/// worktree explicitly from the archive UI. Lives in
/// [`AppStateSnapshot::archived_workspaces`], so it persists to
/// layout.json and reaches the frontend through the ordinary snapshot
/// emit with zero extra plumbing.
///
/// Every optional field is `#[serde(default)]` so entries written by a
/// future version with more fields still load on this one, matching the
/// additive-field convention used across the snapshot types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedWorkspaceSnapshot {
    /// Identity of the archive entry itself (fresh UUID at archive
    /// time). The original workspace id is NOT reused because a
    /// restore creates a brand-new workspace — keeping the ids apart
    /// prevents stale frontend references from resolving to the entry.
    pub archive_id: String,
    /// The id the workspace had when it was archived. Informational
    /// only (e.g. for correlating logs); never looked up against live
    /// state.
    pub workspace_id: String,
    pub title: String,
    pub cwd: String,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub project_uid: Option<String>,
    #[serde(default)]
    pub workspace_kind: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub protected: bool,
    /// Optimistic default mirrors `WorkspaceSnapshot::is_git` — see the
    /// rationale on that field.
    #[serde(default = "default_true")]
    pub is_git: bool,
    /// Unix seconds at archive time. Lets the frontend sort the archive
    /// list by recency without a separate timestamp store.
    pub archived_at: u64,
}

/// Copy the archive-relevant fields out of a live workspace. Pure —
/// callers decide when to add the result to state (specifically AFTER
/// the close path succeeded, so a failed teardown never leaves a
/// phantom archive entry).
pub fn build_archive_entry(ws: &WorkspaceSnapshot) -> ArchivedWorkspaceSnapshot {
    ArchivedWorkspaceSnapshot {
        archive_id: uuid::Uuid::new_v4().to_string(),
        workspace_id: ws.workspace_id.0.clone(),
        title: ws.title.clone(),
        cwd: ws.cwd.clone(),
        worktree_path: ws.worktree_path.clone(),
        project_root: ws.project_root.clone(),
        project_uid: ws.project_uid.clone(),
        workspace_kind: ws.workspace_kind.clone(),
        git_branch: ws.git_branch.clone(),
        protected: ws.protected,
        is_git: ws.is_git,
        archived_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistenceSchema {
    pub schema_version: u32,
    pub stores_layout_metadata: bool,
    pub stores_terminal_metadata: bool,
    pub stores_live_process_state: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemuxConfigSnapshot {
    pub config_version: u32,
    pub default_shell: Option<String>,
    pub theme_source: String,
    pub linux_first: bool,
    pub notification_sound_enabled: bool,
    #[serde(default = "default_true")]
    pub ai_commit_message_enabled: bool,
    #[serde(default)]
    pub ai_commit_message_cli: Option<String>,
    #[serde(default)]
    pub ai_commit_message_model: Option<String>,
    #[serde(default)]
    pub ai_resolver_enabled: bool,
    #[serde(default)]
    pub ai_resolver_cli: Option<String>,
    #[serde(default)]
    pub ai_resolver_model: Option<String>,
    #[serde(default = "default_resolver_strategy")]
    pub ai_resolver_strategy: String,
}

fn default_true() -> bool {
    true
}

fn default_resolver_strategy() -> String {
    "smart_merge".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortInfoSnapshot {
    pub port: u16,
    pub pid: u32,
    pub process_name: String,
    pub workspace_id: Option<String>,
    pub label: Option<String>,
    /// Discovery source: `None` = OS-level scan, `Some("docker")` = a
    /// published container port. Drives the dedicated "Docker" group in the
    /// ports UI. `#[serde(default)]` keeps older persisted snapshots loadable.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStateSnapshot {
    pub schema_version: u32,
    pub active_workspace_id: WorkspaceId,
    pub workspaces: Vec<WorkspaceSnapshot>,
    pub terminal_sessions: Vec<TerminalSessionSnapshot>,
    pub browser_sessions: Vec<BrowserSessionSnapshot>,
    #[serde(default)]
    pub agent_browser_sessions: Vec<AgentBrowserSession>,
    pub notifications: Vec<NotificationSnapshot>,
    #[serde(default)]
    pub detected_ports: Vec<PortInfoSnapshot>,
    /// Per-pane agent status (pane_id → status). Only non-idle entries are stored.
    #[serde(default)]
    pub pane_statuses: HashMap<String, PaneStatus>,
    /// Workspaces the user archived (closed but restorable — see
    /// [`ArchivedWorkspaceSnapshot`]). Additive: old layout.json files
    /// deserialize as an empty list, and persisting/emitting comes for
    /// free because the whole snapshot is written/emitted wholesale.
    #[serde(default)]
    pub archived_workspaces: Vec<ArchivedWorkspaceSnapshot>,
    pub persistence: PersistenceSchema,
    pub config: CodemuxConfigSnapshot,
}

pub struct AppStateStore {
    inner: Mutex<AppStateSnapshot>,
}

/// Outcome of [`AppStateStore::adopt_or_create_worktree_workspace`]:
/// either an existing live workspace already claimed the worktree path
/// (adopt it — do NOT run the create-side effects again), or a fresh
/// workspace was inserted with its `worktree_path` claim already stamped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeWorkspaceClaim {
    /// Id of the live workspace that already claims the path.
    Adopted(String),
    /// Id of the freshly created workspace.
    Created(WorkspaceId),
}

fn terminal_count_for_workspace(workspace: &WorkspaceSnapshot) -> usize {
    collect_terminal_sessions(&workspace.surfaces).len()
}

impl Default for AppStateStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(default_app_state()),
        }
    }
}

impl AppStateStore {
    pub fn snapshot(&self) -> AppStateSnapshot {
        self.inner.lock().unwrap().clone()
    }

    pub fn replace_snapshot(&self, snapshot: AppStateSnapshot) {
        *self.inner.lock().unwrap() = snapshot;
    }

    /// Remove all workspaces and sessions, keeping config intact.
    /// Used on first launch so the splash screen shows instead of
    /// the auto-created CWD workspace from default_app_state().
    pub fn clear_workspaces(&self) {
        let mut snapshot = self.inner.lock().unwrap();
        snapshot.workspaces.clear();
        snapshot.terminal_sessions.clear();
        snapshot.active_workspace_id = WorkspaceId(String::new());
    }

    pub fn set_notification_sound_enabled(&self, enabled: bool) {
        self.inner.lock().unwrap().config.notification_sound_enabled = enabled;
    }

    pub fn set_ai_commit_message_enabled(&self, enabled: bool) {
        self.inner.lock().unwrap().config.ai_commit_message_enabled = enabled;
    }

    pub fn set_ai_commit_message_cli(&self, cli: Option<String>) {
        self.inner.lock().unwrap().config.ai_commit_message_cli = cli;
    }

    pub fn set_ai_commit_message_model(&self, model: Option<String>) {
        self.inner.lock().unwrap().config.ai_commit_message_model = model;
    }

    pub fn set_ai_resolver_enabled(&self, enabled: bool) {
        self.inner.lock().unwrap().config.ai_resolver_enabled = enabled;
    }

    pub fn set_ai_resolver_cli(&self, cli: Option<String>) {
        self.inner.lock().unwrap().config.ai_resolver_cli = cli;
    }

    pub fn set_ai_resolver_model(&self, model: Option<String>) {
        self.inner.lock().unwrap().config.ai_resolver_model = model;
    }

    pub fn set_ai_resolver_strategy(&self, strategy: String) {
        self.inner.lock().unwrap().config.ai_resolver_strategy = strategy;
    }

    pub fn active_terminal_session_id(&self) -> Option<SessionId> {
        let snapshot = self.inner.lock().unwrap();
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == snapshot.active_workspace_id)?;
        let surface = workspace
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == workspace.active_surface_id)?;
        session_id_for_pane(&surface.root, &surface.active_pane_id)
    }

    /// The bookkeeping every workspace switch owes, whatever surface the
    /// switch came in through — sidebar click, palette, keyboard jump,
    /// control socket, or a pane/session activation that happens to live
    /// in another workspace. Centralised so no activation path can move
    /// `active_workspace_id` while forgetting the ledger around it:
    ///
    /// - Leaving a workspace counts as having visited it up to this
    ///   moment. Without this, work the user sat and watched finish turns
    ///   bold-unread the instant they switch away: the pane writers stamp
    ///   `last_active_at` even while the workspace is focused, and only
    ///   the incoming side used to move `last_visited_at`, so
    ///   `last_active_at > last_visited_at` came out true for the one
    ///   workspace the user had definitely seen.
    /// - The incoming workspace's notifications are marked read, its
    ///   badge count cleared, its active surface's Review statuses
    ///   dropped, and its own visit stamped.
    ///
    /// Callers must have verified that `workspace_id` names an existing
    /// workspace.
    fn record_workspace_switch(snapshot: &mut AppStateSnapshot, workspace_id: &str) {
        let previous_id = snapshot.active_workspace_id.0.clone();
        if previous_id != workspace_id {
            if let Some(previous) = snapshot
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.workspace_id.0 == previous_id)
            {
                previous.last_visited_at = Some(current_time_ms_signed());
            }
        }
        snapshot.active_workspace_id = WorkspaceId(workspace_id.to_string());
        for notification in snapshot.notifications.iter_mut() {
            if notification.workspace_id.0 == workspace_id {
                notification.read = true;
            }
        }
        // Clear review statuses only for the active tab's panes (not all tabs)
        if let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            let active_surface_id = workspace.active_surface_id.clone();
            if let Some(surface) = workspace
                .surfaces
                .iter()
                .find(|s| s.surface_id == active_surface_id)
            {
                let pane_ids = collect_pane_ids_from_node(&surface.root);
                for pid in pane_ids {
                    if snapshot.pane_statuses.get(&pid) == Some(&PaneStatus::Review) {
                        snapshot.pane_statuses.remove(&pid);
                    }
                }
            }
        }
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            workspace.notification_count = 0;
            workspace.last_visited_at = Some(current_time_ms_signed());
        }
    }

    pub fn activate_terminal_session(&self, session_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();

        // Locate first, mutate after: the switch bookkeeping needs the
        // whole snapshot, so the target has to be named by index rather
        // than held as a borrow into the workspace list.
        let mut target: Option<(usize, usize, PaneId)> = None;
        'search: for (workspace_index, workspace) in snapshot.workspaces.iter().enumerate() {
            for (surface_index, surface) in workspace.surfaces.iter().enumerate() {
                if let Some(pane_id) = find_terminal_pane_id(&surface.root, session_id) {
                    target = Some((workspace_index, surface_index, pane_id));
                    break 'search;
                }
            }
        }
        let Some((workspace_index, surface_index, pane_id)) = target else {
            return false;
        };

        let workspace = &mut snapshot.workspaces[workspace_index];
        let workspace_id = workspace.workspace_id.clone();
        workspace.active_surface_id = workspace.surfaces[surface_index].surface_id.clone();
        workspace.surfaces[surface_index].active_pane_id = pane_id;
        // A session can be focused from another workspace (jump-to-session
        // navigation): that is a workspace switch and owes the same visit
        // bookkeeping as an explicit activation. Within the already-active
        // workspace it is only a focus move — re-stamping would clear
        // Review badges the user hasn't looked at.
        if snapshot.active_workspace_id != workspace_id {
            Self::record_workspace_switch(&mut snapshot, &workspace_id.0);
        }
        true
    }

    pub fn activate_workspace(&self, workspace_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if snapshot
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            Self::record_workspace_switch(&mut snapshot, workspace_id);
            return true;
        }
        false
    }

    pub fn activate_pane(&self, pane_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();

        // Same locate-then-mutate shape as `activate_terminal_session`,
        // for the same borrow reason.
        let mut target: Option<(usize, usize)> = None;
        'search: for (workspace_index, workspace) in snapshot.workspaces.iter().enumerate() {
            for (surface_index, surface) in workspace.surfaces.iter().enumerate() {
                if pane_tree_contains_pane(&surface.root, pane_id) {
                    target = Some((workspace_index, surface_index));
                    break 'search;
                }
            }
        }
        let Some((workspace_index, surface_index)) = target else {
            return false;
        };

        let workspace = &mut snapshot.workspaces[workspace_index];
        let workspace_id = workspace.workspace_id.clone();
        workspace.active_surface_id = workspace.surfaces[surface_index].surface_id.clone();
        workspace.surfaces[surface_index].active_pane_id = PaneId(pane_id.to_string());
        // Cross-workspace pane focus is a workspace switch; same-workspace
        // focus is not (see `activate_terminal_session`).
        if snapshot.active_workspace_id != workspace_id {
            Self::record_workspace_switch(&mut snapshot, &workspace_id.0);
        }
        true
    }

    pub fn create_workspace(&self) -> WorkspaceId {
        self.create_workspace_at_path(current_project_root())
    }

    pub fn create_openflow_workspace(&self, title: String, _goal: String) -> WorkspaceId {
        self.create_openflow_workspace_at_path(title, _goal, current_project_root())
    }

    pub fn create_openflow_workspace_at_path(
        &self,
        title: String,
        _goal: String,
        cwd_path: PathBuf,
    ) -> WorkspaceId {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace_id = WorkspaceId(next_id("workspace"));
        let surface_id = SurfaceId(next_id("surface"));
        let cwd = cwd_path.display().to_string();
        let _workspace_index = snapshot.workspaces.len() + 1;
        let pane_id = PaneId(next_id("pane"));

        eprintln!(
            "DEBUG: Creating OpenFlow workspace with title: {} at {}",
            title, cwd
        );

        snapshot.workspaces.push(WorkspaceSnapshot {
            workspace_id: workspace_id.clone(),
            is_git: true,
            title,
            workspace_type: WorkspaceType::OpenFlow,
            cwd,
            git_branch: None,
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            worktree_path: None,
            project_root: None,
            project_uid: None,
            workspace_kind: None,
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            notifications_muted: false,
            notification_count: 0,
            latest_agent_state: Some("configuring".into()),
            tabs: vec![],
            active_tab_id: String::new(),
            active_surface_id: surface_id.clone(),
            surfaces: vec![SurfaceSnapshot {
                surface_id,
                title: "OpenFlow".into(),
                active_pane_id: pane_id.clone(),
                root: PaneNodeSnapshot::Split {
                    pane_id,
                    direction: SplitDirection::Vertical,
                    child_sizes: vec![100.0],
                    children: vec![],
                },
            }],
            host_id: None,
            remote_cwd: None,
            attach_only: false,
            last_active_at: Some(current_time_ms_signed()),
            last_visited_at: Some(current_time_ms_signed()),
        });

        snapshot.active_workspace_id = workspace_id.clone();
        workspace_id
    }

    /// Add a new terminal session to a specific OpenFlow workspace.
    ///
    /// Returns the new `SessionId`.  Unlike `create_terminal_session` (which
    /// uses the *active* workspace and enforces the normal per-workspace
    /// terminal limit), this method targets an explicit workspace and bypasses
    /// the limit because OpenFlow workspaces need one pane per agent.
    pub fn add_agent_terminal_to_workspace(
        &self,
        workspace_id: &str,
        title: String,
        working_directory: String,
    ) -> Result<SessionId, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let cwd = working_directory;

        let session_id = SessionId(next_id("session"));
        let pane_id = PaneId(next_id("pane"));

        snapshot.terminal_sessions.push(TerminalSessionSnapshot {
            session_id: session_id.clone(),
            title: title.clone(),
            shell: None, // will be set when the PTY spawns
            cwd: cwd.clone(),
            cols: 80,
            rows: 24,
            state: TerminalSessionState::Starting,
            last_message: Some("Preparing agent session".into()),
            exit_code: None,
            original_command: None,
            adapter_captures: Default::default(),
        });

        let workspace = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

        let surface = workspace
            .surfaces
            .iter_mut()
            .find(|s| s.surface_id == workspace.active_surface_id)
            .ok_or_else(|| "OpenFlow workspace has no active surface".to_string())?;

        // The OpenFlow workspace root is a Split with no children.  Append
        // the new terminal pane as a child.
        match &mut surface.root {
            PaneNodeSnapshot::Split {
                children,
                child_sizes,
                ..
            } => {
                children.push(PaneNodeSnapshot::Terminal {
                    pane_id: pane_id.clone(),
                    session_id: session_id.clone(),
                    title,
                });
                // Keep child_sizes length in sync: equal weights.
                let n = children.len() as f32;
                let each = 100.0 / n;
                *child_sizes = vec![each; children.len()];
                surface.active_pane_id = pane_id;
            }
            _ => {
                return Err(
                    "OpenFlow workspace root is not a split node; cannot add agent pane".into(),
                )
            }
        }

        Ok(session_id)
    }

    pub fn create_workspace_at_path(&self, cwd_path: PathBuf) -> WorkspaceId {
        self.create_workspace_with_layout(cwd_path, WorkspacePresetLayout::Single)
    }

    /// Title a freshly-created workspace that has nothing better to go on.
    ///
    /// Prefers the directory's own name (`~/projects/codemux` → `codemux`),
    /// falling back to `Workspace {n}` only when the path has no usable
    /// final component (filesystem root, empty path).
    ///
    /// Why not always `Workspace {n}`: several creation paths never get a
    /// second chance at a name. Worktree creation overwrites the title with
    /// its branch (`set_workspace_worktree`) and the chat surfaces rename
    /// from the first prompt, but "Open project", "Clone", and "New project"
    /// have no branch and no prompt — so their workspaces kept a bare
    /// ordinal forever. The directory name is what the user picked and what
    /// every other tool shows for the same thing.
    ///
    /// Callers that DO have a better name still overwrite this afterwards;
    /// the frontend's `isDefaultWorkspaceTitle` recognises both this shape
    /// and the legacy `Workspace {n}` so auto-naming can still upgrade it.
    fn default_workspace_title(cwd_path: &Path, workspace_index: usize) -> String {
        cwd_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.trim())
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("Workspace {workspace_index}"))
    }

    /// Create a workspace with no tabs, surfaces, or terminal sessions.
    /// Used by "Add repository" so the empty workspace state shows.
    pub fn create_empty_workspace_at_path(&self, cwd_path: PathBuf) -> WorkspaceId {
        let mut snapshot = self.inner.lock().unwrap();
        Self::create_empty_workspace_at_path_locked(&mut snapshot, cwd_path)
    }

    /// Body of [`Self::create_empty_workspace_at_path`], factored out so
    /// callers that already hold the state lock (the atomic
    /// adopt-or-create path) can insert without re-locking.
    fn create_empty_workspace_at_path_locked(
        snapshot: &mut AppStateSnapshot,
        cwd_path: PathBuf,
    ) -> WorkspaceId {
        let workspace_id = WorkspaceId(next_id("workspace"));
        let workspace_index = snapshot.workspaces.len() + 1;
        let cwd = cwd_path.display().to_string();

        snapshot.workspaces.push(WorkspaceSnapshot {
            workspace_id: workspace_id.clone(),
            is_git: true,
            title: Self::default_workspace_title(&cwd_path, workspace_index),
            workspace_type: WorkspaceType::Standard,
            cwd,
            git_branch: None,
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            worktree_path: None,
            project_root: None,
            project_uid: None,
            workspace_kind: None,
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            notifications_muted: false,
            notification_count: 0,
            latest_agent_state: Some("idle".into()),
            tabs: vec![],
            active_tab_id: String::new(),
            active_surface_id: SurfaceId(String::new()),
            surfaces: vec![],
            host_id: None,
            remote_cwd: None,
            attach_only: false,
            last_active_at: Some(current_time_ms_signed()),
            last_visited_at: Some(current_time_ms_signed()),
        });

        snapshot.active_workspace_id = workspace_id.clone();
        snapshot
            .notifications
            .retain(|notification| notification.workspace_id != workspace_id);
        workspace_id
    }

    /// Create a workspace shell that will receive its files from an
    /// upcoming `workspace_pull_back` call. Differs from
    /// `create_workspace_at_path` and `create_empty_workspace_at_path`
    /// in three ways:
    ///
    /// 1. No git operations run — the rsync from the remote will
    ///    populate the worktree directory; running `git worktree add`
    ///    here would just create files rsync would have to clobber.
    /// 2. `host_id` is set up front, because the caller already knows
    ///    which device the workspace is being pulled from and the
    ///    pull-back path keys off `host_id` to find the remote.
    /// 3. `worktree_path` is set to a path that doesn't exist yet —
    ///    rsync creates it. This is the entire point of the helper.
    ///
    /// Used by `workspaces_adopt_synced` (cross-device adoption) where
    /// we're materialising a workspace another device of the same
    /// account already created and pushed to a host. After this
    /// returns, the caller is expected to immediately call the
    /// existing `workspace_pull_back` machinery to fetch the files.
    pub fn create_synced_workspace_shell(
        &self,
        title: String,
        host_id: i64,
        project_root: Option<String>,
        worktree_path: String,
        git_branch: Option<String>,
    ) -> WorkspaceId {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace_id = WorkspaceId(next_id("workspace"));

        snapshot.workspaces.push(WorkspaceSnapshot {
            workspace_id: workspace_id.clone(),
            is_git: true,
            title,
            workspace_type: WorkspaceType::Standard,
            // `cwd` mirrors `worktree_path` for adopted workspaces —
            // when the user opens the workspace, terminals get
            // spawned in the worktree directory, same as a
            // freshly-created worktree workspace.
            cwd: worktree_path.clone(),
            git_branch,
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            worktree_path: Some(worktree_path),
            project_root,
            // Adoption lands at a worktree-style path; project_uid is
            // stamped when the project root is (re)resolved on this
            // device. The synced row already carries the cross-device
            // identity from the poller.
            project_uid: None,
            workspace_kind: Some("worktree".into()),
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            notifications_muted: false,
            notification_count: 0,
            latest_agent_state: Some("idle".into()),
            tabs: vec![],
            active_tab_id: String::new(),
            active_surface_id: SurfaceId(String::new()),
            surfaces: vec![],
            // Critical: host_id is set so the pull-back call that
            // follows knows which device to rsync from. It will be
            // cleared to None on successful pull.
            host_id: Some(host_id),
            remote_cwd: None,
            attach_only: false,
            // Adoption is not activity. This workspace has a history — it
            // just happened on another device — so stamping `now` would tell
            // the idle sweep a month-old checkout is brand new, and the boot
            // backfill could never correct it because it only fills `None`.
            // Left unknown, the sweep declines to act until the backfill
            // dates the pulled checkout from its own git history.
            last_active_at: None,
            last_visited_at: Some(current_time_ms_signed()),
        });

        // We deliberately do NOT set this as the active workspace.
        // The user clicked "Pull to this device" from the overview;
        // they should land back in the overview when the pull
        // completes, not in a half-populated pane tree. The frontend
        // can navigate explicitly on success if it wants.
        workspace_id
    }

    /// Like [`create_synced_workspace_shell`], but for adopting a repo's
    /// ROOT (default-branch) checkout — `workspace_kind = "main"`, no
    /// `worktree_path`. The shell lands at `project_root`
    /// (`~/.codemux/projects/<repo>`), NOT in the worktrees tree, so the
    /// adopted root is classified as a genuine repo root rather than a
    /// divergent copy. `host_id` is pre-set so the upcoming pull routes
    /// to the right host; it's cleared on success. `protected` is left
    /// false here and stamped once the pull lands and the root is
    /// verified on its default branch.
    pub fn create_synced_root_shell(
        &self,
        title: String,
        host_id: i64,
        project_root: String,
        git_branch: Option<String>,
    ) -> WorkspaceId {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace_id = WorkspaceId(next_id("workspace"));

        snapshot.workspaces.push(WorkspaceSnapshot {
            workspace_id: workspace_id.clone(),
            is_git: true,
            title,
            workspace_type: WorkspaceType::Standard,
            cwd: project_root.clone(),
            git_branch,
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            worktree_path: None,
            project_root: Some(project_root),
            project_uid: None,
            workspace_kind: Some("main".into()),
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            notifications_muted: false,
            notification_count: 0,
            latest_agent_state: Some("idle".into()),
            tabs: vec![],
            active_tab_id: String::new(),
            active_surface_id: SurfaceId(String::new()),
            surfaces: vec![],
            host_id: Some(host_id),
            remote_cwd: None,
            attach_only: false,
            // Same as [`create_synced_workspace_shell`]: an adopted root
            // carries history from the device that made it, so it is left
            // unknown for the backfill to date rather than faked as `now`.
            last_active_at: None,
            last_visited_at: Some(current_time_ms_signed()),
        });

        workspace_id
    }

    /// Create an **attach-in-place** workspace: a local workspace that is
    /// operated directly on its host (`host_id`), with its files staying
    /// on the host (`remote_cwd`) and **nothing copied to this device**.
    /// This backs the "Open on host" overview action.
    ///
    /// Unlike [`create_synced_workspace_shell`] / [`create_synced_root_shell`]
    /// (which are empty placeholders awaiting an rsync pull), this builds a
    /// ready-to-use single-terminal layout: opening the workspace
    /// immediately spawns a shell on the host (the daemon-backed terminal
    /// path routes `host_id`-bearing workspaces through the SSH-tunneled
    /// remote daemon, and prefers `remote_cwd` as the spawn directory).
    ///
    /// `attach_only` is set so every file-system-bound code path (git info,
    /// worktree deletion, re-push) is skipped and the only teardown is a
    /// detach that leaves the host process running.
    #[allow(clippy::too_many_arguments)]
    pub fn create_remote_attach_workspace(
        &self,
        title: String,
        host_id: i64,
        remote_cwd: String,
        git_branch: Option<String>,
        project_root: Option<String>,
        project_uid: Option<String>,
        workspace_kind: Option<String>,
    ) -> WorkspaceId {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace_id = WorkspaceId(next_id("workspace"));
        let surface_id = SurfaceId(next_id("surface"));
        let session_id = SessionId(next_id("session"));
        let base_terminal_index = snapshot.terminal_sessions.len() + 1;

        // A single shell session whose cwd is the host directory. The
        // daemon-backed spawn path re-derives the effective cwd from
        // `remote_cwd`, so this value is informational, but we keep it
        // consistent so the UI shows the right path.
        snapshot.terminal_sessions.push(TerminalSessionSnapshot {
            session_id: session_id.clone(),
            title: format!("Terminal {base_terminal_index}"),
            shell: None,
            cwd: remote_cwd.clone(),
            cols: 80,
            rows: 24,
            state: TerminalSessionState::Starting,
            last_message: Some("Preparing shell session".into()),
            exit_code: None,
            original_command: None,
            adapter_captures: Default::default(),
        });

        let pane_id = PaneId(next_id("pane"));
        let root = PaneNodeSnapshot::Terminal {
            pane_id: pane_id.clone(),
            session_id: session_id.clone(),
            title: "Terminal".into(),
        };
        let default_tab_id = next_id("tab");

        snapshot.workspaces.push(WorkspaceSnapshot {
            workspace_id: workspace_id.clone(),
            is_git: true,
            title,
            workspace_type: WorkspaceType::Standard,
            // `cwd` is a host path that does not exist on this device. It
            // is never used for local FS work (gated behind `attach_only`)
            // — it's here so the workspace header shows the host directory.
            cwd: remote_cwd.clone(),
            git_branch,
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            worktree_path: None,
            project_root,
            project_uid,
            workspace_kind,
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            notifications_muted: false,
            notification_count: 0,
            latest_agent_state: Some("idle".into()),
            tabs: vec![TabSnapshot {
                tab_id: default_tab_id.clone(),
                kind: TabKind::Terminal,
                title: "Terminal".into(),
                surface_id: Some(surface_id.clone()),
                browser_id: None,
                icon: None,
            }],
            active_tab_id: default_tab_id,
            active_surface_id: surface_id.clone(),
            surfaces: vec![SurfaceSnapshot {
                surface_id,
                title: "Main Surface".into(),
                active_pane_id: pane_id,
                root,
            }],
            host_id: Some(host_id),
            remote_cwd: Some(remote_cwd),
            attach_only: true,
            last_active_at: Some(current_time_ms_signed()),
            last_visited_at: Some(current_time_ms_signed()),
        });

        snapshot.active_workspace_id = workspace_id.clone();
        snapshot
            .notifications
            .retain(|notification| notification.workspace_id != workspace_id);
        workspace_id
    }

    /// Set the repo-root `protected` flag on a workspace (used by the
    /// adopt path to mark a freshly-pulled repo root protected without
    /// recomputing its `project_uid`, which would diverge for
    /// local-only repos — see `set_workspace_project_identity`).
    pub fn set_workspace_protected(&self, workspace_id: &str, protected: bool) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
        {
            workspace.protected = protected;
        }
    }

    pub fn create_workspace_with_layout(
        &self,
        cwd_path: PathBuf,
        layout: WorkspacePresetLayout,
    ) -> WorkspaceId {
        let mut snapshot = self.inner.lock().unwrap();
        Self::create_workspace_with_layout_locked(&mut snapshot, cwd_path, layout)
    }

    /// Body of [`Self::create_workspace_with_layout`], factored out so
    /// callers that already hold the state lock (the atomic
    /// adopt-or-create path) can insert without re-locking.
    fn create_workspace_with_layout_locked(
        snapshot: &mut AppStateSnapshot,
        cwd_path: PathBuf,
        layout: WorkspacePresetLayout,
    ) -> WorkspaceId {
        // Empty layout short-circuits to the same shape
        // `create_empty_workspace_at_path` produces: no tabs, no
        // surfaces, no terminal sessions. Callers (e.g. the inline
        // chat-worktree flow) fill the workspace with their own pane
        // afterward via `agent_chat_create_pane`.
        if matches!(layout, WorkspacePresetLayout::Empty) {
            return Self::create_empty_workspace_at_path_locked(snapshot, cwd_path);
        }

        let workspace_id = WorkspaceId(next_id("workspace"));
        let surface_id = SurfaceId(next_id("surface"));
        let cwd = cwd_path.display().to_string();
        let shell = env::var("SHELL").ok();
        let workspace_index = snapshot.workspaces.len() + 1;
        let base_terminal_index = snapshot.terminal_sessions.len() + 1;
        let shell_count = layout_shell_count(&layout);
        let mut session_ids = Vec::with_capacity(shell_count);

        for offset in 0..shell_count {
            let session_id = SessionId(next_id("session"));
            snapshot.terminal_sessions.push(TerminalSessionSnapshot {
                session_id: session_id.clone(),
                title: format!("Terminal {}", base_terminal_index + offset),
                shell: shell.clone(),
                cwd: cwd.clone(),
                cols: 80,
                rows: 24,
                state: TerminalSessionState::Starting,
                last_message: Some("Preparing shell session".into()),
                exit_code: None,
                original_command: None,
                adapter_captures: Default::default(),
            });
            session_ids.push(session_id);
        }

        let mut browser = None;
        if matches!(layout, WorkspacePresetLayout::ShellBrowser) {
            let browser_id = BrowserId(next_id("browser"));
            let browser_title = format!("Browser {}", snapshot.browser_sessions.len() + 1);
            snapshot.browser_sessions.push(BrowserSessionSnapshot {
                browser_id: browser_id.clone(),
                title: browser_title,
                current_url: Some(DEFAULT_BROWSER_URL.into()),
                history: vec![DEFAULT_BROWSER_URL.into()],
                history_index: 0,
                is_loading: false,
                last_error: None,
                agent_session_name: None,
            });
            browser = Some(browser_id);
        }

        let root = build_workspace_layout(&layout, &session_ids, browser.as_ref());
        let active_pane_id =
            rightmost_leaf_pane_id(&root).unwrap_or_else(|| PaneId(next_id("pane")));

        let default_tab_id = next_id("tab");
        snapshot.workspaces.push(WorkspaceSnapshot {
            workspace_id: workspace_id.clone(),
            is_git: true,
            title: Self::default_workspace_title(&cwd_path, workspace_index),
            workspace_type: WorkspaceType::Standard,
            cwd,
            git_branch: None,
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            worktree_path: None,
            project_root: None,
            project_uid: None,
            workspace_kind: None,
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            notifications_muted: false,
            notification_count: 0,
            latest_agent_state: Some("idle".into()),
            tabs: vec![TabSnapshot {
                tab_id: default_tab_id.clone(),
                kind: TabKind::Terminal,
                title: "Terminal".into(),
                surface_id: Some(surface_id.clone()),
                browser_id: None,
                icon: None,
            }],
            active_tab_id: default_tab_id,
            active_surface_id: surface_id.clone(),
            surfaces: vec![SurfaceSnapshot {
                surface_id,
                title: "Main Surface".into(),
                active_pane_id,
                root,
            }],
            host_id: None,
            remote_cwd: None,
            attach_only: false,
            last_active_at: Some(current_time_ms_signed()),
            last_visited_at: Some(current_time_ms_signed()),
        });

        snapshot.active_workspace_id = workspace_id.clone();
        snapshot
            .notifications
            .retain(|notification| notification.workspace_id != workspace_id);
        workspace_id
    }

    /// Atomically either adopt the live workspace `probe` finds, or create a
    /// new one at `cwd_path` with its worktree claim (`worktree_path` +
    /// `title`) already stamped — all under ONE acquisition of the state
    /// lock.
    ///
    /// This is the linearization point for the duplicate-worktree guard:
    /// with lookup and insert as separate lock acquisitions, two concurrent
    /// `create_worktree_workspace` calls for the same path could both probe
    /// empty and both insert — exactly the two-workspaces/one-checkout state
    /// the guard exists to prevent. Stamping `worktree_path` inside the same
    /// critical section matters for the same reason: it is the claim a
    /// concurrent creator's probe matches on.
    ///
    /// `probe` runs under the state lock; it must only read the snapshot and
    /// must not touch this store (or any other lock). The slow work around a
    /// worktree creation (`git worktree add`, git-info population, PTY
    /// spawning) stays outside — only the check+insert is serialized here.
    pub fn adopt_or_create_worktree_workspace<F>(
        &self,
        cwd_path: PathBuf,
        layout: WorkspacePresetLayout,
        worktree_path: String,
        title: String,
        probe: F,
    ) -> WorktreeWorkspaceClaim
    where
        F: FnOnce(&AppStateSnapshot) -> Option<String>,
    {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(existing_id) = probe(&snapshot) {
            return WorktreeWorkspaceClaim::Adopted(existing_id);
        }
        let workspace_id =
            Self::create_workspace_with_layout_locked(&mut snapshot, cwd_path, layout);
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id == workspace_id)
        {
            workspace.worktree_path = Some(worktree_path);
            workspace.title = title;
        }
        WorktreeWorkspaceClaim::Created(workspace_id)
    }

    pub fn create_browser_pane(&self, pane_id: &str, url: Option<&str>) -> Result<(PaneId, BrowserId), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let (workspace_index, surface_index) = find_pane_location(&snapshot.workspaces, pane_id)
            .ok_or_else(|| format!("No pane found for {pane_id}"))?;

        let browser_id = BrowserId(next_id("browser"));
        let new_pane_id = PaneId(next_id("pane"));
        let split_pane_id = PaneId(next_id("pane"));
        let title = format!("Browser {}", snapshot.browser_sessions.len() + 1);
        let initial_url = url.unwrap_or(DEFAULT_BROWSER_URL).to_string();

        snapshot.browser_sessions.push(BrowserSessionSnapshot {
            browser_id: browser_id.clone(),
            title: title.clone(),
            current_url: Some(initial_url.clone()),
            history: vec![initial_url],
            history_index: 0,
            is_loading: false,
            last_error: None,
            agent_session_name: None,
        });

        let workspace = snapshot
            .workspaces
            .get_mut(workspace_index)
            .ok_or_else(|| "Workspace disappeared while creating browser pane".to_string())?;
        let surface = workspace
            .surfaces
            .get_mut(surface_index)
            .ok_or_else(|| "Surface disappeared while creating browser pane".to_string())?;

        let inserted = insert_split_at_pane(
            &mut surface.root,
            pane_id,
            split_pane_id,
            SplitDirection::Horizontal,
            PaneNodeSnapshot::Browser {
                pane_id: new_pane_id.clone(),
                browser_id: browser_id.clone(),
                title,
            },
        );

        if !inserted {
            return Err(format!("Failed to create browser pane next to {pane_id}"));
        }

        workspace.active_surface_id = surface.surface_id.clone();
        surface.active_pane_id = new_pane_id.clone();
        snapshot.active_workspace_id = workspace.workspace_id.clone();

        Ok((new_pane_id, browser_id))
    }

    /// Spawn a new agent-chat pane inside the given workspace.
    ///
    /// Pane tree behavior mirrors [`create_browser_pane`](Self::create_browser_pane):
    /// the new pane is inserted by splitting the workspace's currently
    /// active pane horizontally. Returns the new [`PaneId`] on success.
    ///
    /// This method does not consult the `enable_agent_chat` feature
    /// flag — callers (the Tauri command layer) are responsible for
    /// gating. Keeping the flag check at the command boundary keeps
    /// state-level operations reusable from tests that bypass the flag.
    pub fn create_agent_chat_pane(
        &self,
        workspace_id: &str,
        provider: Option<crate::agent_provider::ProviderKind>,
        cwd: Option<String>,
        launch_mode: Option<crate::presets::LaunchMode>,
    ) -> Result<PaneId, String> {
        let mut snapshot = self.inner.lock().unwrap();

        let workspace_index = snapshot
            .workspaces
            .iter()
            .position(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

        let new_pane_id = PaneId(next_id("pane"));
        let split_pane_id = PaneId(next_id("pane"));
        let title = "Agent Chat".to_string();
        let new_node = PaneNodeSnapshot::AgentChat {
            pane_id: new_pane_id.clone(),
            title,
            thread_id: None,
            provider,
            cwd,
        };

        let workspace = snapshot
            .workspaces
            .get_mut(workspace_index)
            .ok_or_else(|| "Workspace disappeared while creating chat pane".to_string())?;

        // Force a fresh tab+surface when the caller explicitly asks
        // for `NewTab`. Without this the existing surface check below
        // would split the active pane on a populated workspace —
        // which is the right default for materialise / sidebar paths
        // (the chat IS the workspace) but wrong when a Chat Agent
        // preset button is clicked on a workspace the user is already
        // working in: clicking the button should mirror CLI presets
        // (new tab on plain click, split on shift+click), not always
        // split.
        let force_new_tab = matches!(
            launch_mode,
            Some(crate::presets::LaunchMode::NewTab)
        );

        // Find the active surface. If the workspace has no surfaces
        // (empty workspace state) OR the caller wants a new tab,
        // create a fresh surface carrying the new chat pane as its
        // only child. Otherwise split at the active pane, mirroring
        // browser/terminal pane insertion.
        if workspace.surfaces.is_empty() || force_new_tab {
            let surface_id = SurfaceId(next_id("surface"));
            let tab_id = next_id("tab");
            workspace.tabs.push(TabSnapshot {
                tab_id: tab_id.clone(),
                kind: TabKind::Terminal,
                title: "Agent Chat".into(),
                surface_id: Some(surface_id.clone()),
                browser_id: None,
                icon: None,
            });
            workspace.active_tab_id = tab_id;
            workspace.active_surface_id = surface_id.clone();
            workspace.surfaces.push(SurfaceSnapshot {
                surface_id,
                title: "Agent Chat".into(),
                active_pane_id: new_pane_id.clone(),
                root: new_node,
            });
            let active_workspace_id = workspace.workspace_id.clone();
            snapshot.active_workspace_id = active_workspace_id;
            return Ok(new_pane_id);
        }

        let active_surface_id = workspace.active_surface_id.clone();
        let surface_index = workspace
            .surfaces
            .iter()
            .position(|s| s.surface_id == active_surface_id)
            .unwrap_or(0);
        let surface = workspace
            .surfaces
            .get_mut(surface_index)
            .ok_or_else(|| "Workspace has no usable surface".to_string())?;

        let target_pane_id = surface.active_pane_id.clone();
        let inserted = insert_split_at_pane(
            &mut surface.root,
            &target_pane_id.0,
            split_pane_id,
            SplitDirection::Horizontal,
            new_node,
        );
        if !inserted {
            return Err(format!(
                "Failed to create agent_chat pane next to {}",
                target_pane_id.0
            ));
        }

        surface.active_pane_id = new_pane_id.clone();
        let surface_id = surface.surface_id.clone();
        workspace.active_surface_id = surface_id;
        let active_workspace_id = workspace.workspace_id.clone();
        snapshot.active_workspace_id = active_workspace_id;

        Ok(new_pane_id)
    }

    /// Return the [`ThreadId`] bound to the given chat pane, if any.
    pub fn agent_chat_thread_id(&self, pane_id: &str) -> Option<String> {
        let snapshot = self.inner.lock().unwrap();
        for workspace in &snapshot.workspaces {
            for surface in &workspace.surfaces {
                if let Some(tid) = agent_chat_thread_for_pane(&surface.root, pane_id) {
                    return tid;
                }
            }
        }
        None
    }

    /// Store a [`ThreadId`] on an existing chat pane. Returns true on
    /// success. A no-op on non-chat panes or unknown pane ids.
    pub fn set_agent_chat_thread_id(
        &self,
        pane_id: &str,
        new_thread_id: Option<String>,
    ) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        for workspace in &mut snapshot.workspaces {
            for surface in &mut workspace.surfaces {
                if assign_agent_chat_thread(&mut surface.root, pane_id, &new_thread_id) {
                    return true;
                }
            }
        }
        false
    }

    /// Atomically bind an agent-chat pane to the provider + thread that
    /// actually owns its live session. Provider switches must update both
    /// fields together: persisting only the new thread id leaves the pane
    /// snapshot on its previous adapter, so an app restart routes the thread
    /// through the wrong provider even though session startup succeeded.
    pub fn set_agent_chat_binding(
        &self,
        pane_id: &str,
        provider: crate::agent_provider::ProviderKind,
        thread_id: String,
    ) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        for workspace in &mut snapshot.workspaces {
            for surface in &mut workspace.surfaces {
                if assign_agent_chat_binding(
                    &mut surface.root,
                    pane_id,
                    provider,
                    &thread_id,
                ) {
                    return true;
                }
            }
        }
        false
    }

    pub fn rename_workspace(&self, workspace_id: &str, title: String) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            workspace.title = title;
            return true;
        }
        false
    }

    /// Assign (or clear) the host this workspace runs on. `None`
    /// means local. Used by the DevicePicker pill at workspace
    /// create time and by the future "Push to host" / "Pull back"
    /// actions. Returns Err with a clear message if the workspace
    /// id isn't found so the frontend can surface it.
    pub fn set_workspace_host_id(
        &self,
        workspace_id: &str,
        host_id: Option<i64>,
    ) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("Workspace not found: {workspace_id}"))?;
        workspace.host_id = host_id;
        Ok(())
    }

    /// Toggle agent-completion desktop notifications for a workspace.
    /// Returns true if the workspace was found. Only gates the OS popup;
    /// status pills are driven separately and stay live.
    pub fn set_workspace_muted(&self, workspace_id: &str, muted: bool) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            workspace.notifications_muted = muted;
            return true;
        }
        false
    }

    /// Whether the workspace containing `session_id` has notifications muted.
    /// Used by the hook handler to suppress the completion popup.
    pub fn is_session_workspace_muted(&self, session_id: &str) -> bool {
        let snapshot = self.inner.lock().unwrap();
        snapshot.workspaces.iter().any(|ws| {
            ws.notifications_muted
                && ws
                    .surfaces
                    .iter()
                    .any(|s| find_terminal_pane_id(&s.root, session_id).is_some())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_workspace_git_info(
        &self,
        workspace_id: &str,
        is_git: bool,
        branch: Option<String>,
        ahead: u32,
        behind: u32,
        additions: u32,
        deletions: u32,
        changed_files: u32,
    ) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            workspace.is_git = is_git;
            workspace.git_branch = branch;
            workspace.git_ahead = ahead;
            workspace.git_behind = behind;
            workspace.git_additions = additions;
            workspace.git_deletions = deletions;
            workspace.git_changed_files = changed_files;
        }
    }

    pub fn update_workspace_pr_info(
        &self,
        workspace_id: &str,
        pr_number: Option<u32>,
        pr_state: Option<String>,
        pr_url: Option<String>,
    ) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            workspace.pr_number = pr_number;
            workspace.pr_state = pr_state;
            workspace.pr_url = pr_url;
        }
    }

    pub fn set_workspace_worktree(
        &self,
        workspace_id: &str,
        worktree_path: String,
        title: String,
    ) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
        {
            workspace.worktree_path = Some(worktree_path);
            workspace.title = title;
        }
    }

    pub fn link_workspace_issue(
        &self,
        workspace_id: &str,
        linked_issue: crate::github::LinkedIssue,
    ) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
        {
            workspace.linked_issue = Some(linked_issue);
        }
    }

    pub fn unlink_workspace_issue(&self, workspace_id: &str) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
        {
            workspace.linked_issue = None;
        }
    }

    pub fn set_workspace_project_root(&self, workspace_id: &str, project_root: String) {
        // Read the on-disk checkout path + branch under a brief lock so
        // the git subprocesses below (uid + protection) run WITHOUT
        // holding the state mutex. The checkout to classify is the
        // worktree dir when this is a worktree, else the project root.
        let (checkout_path, git_branch) = {
            let snapshot = self.inner.lock().unwrap();
            match snapshot
                .workspaces
                .iter()
                .find(|w| w.workspace_id.0 == workspace_id)
            {
                Some(w) => (
                    w.worktree_path
                        .clone()
                        .unwrap_or_else(|| project_root.clone()),
                    w.git_branch.clone(),
                ),
                None => return,
            }
        };

        // Compute the deterministic project_uid OUTSIDE the lock — the
        // git remote lookup spawns a subprocess and we don't want to
        // hold the state mutex across it. Uses the canonical remote when
        // the repo has one (so it converges with the same repo on other
        // devices/hosts), else the project-root path. Matches the daemon
        // (`remote::workspace::create`) so a host copy and a local copy
        // of the same repo share a project_uid.
        let remote =
            crate::project_identity::git_canonical_remote(std::path::Path::new(&project_root));
        let project_uid =
            crate::project_identity::project_uid_for(remote.as_deref(), &project_root);

        // Repo-root protection (also outside the lock — more git
        // subprocesses). Divergence-safe: a full copy under the
        // worktrees tree is NOT protected even though its `.git` is a
        // directory (see `crate::git::is_protected_repo_root`).
        let protected = crate::git::is_protected_repo_root(
            std::path::Path::new(&checkout_path),
            git_branch.as_deref(),
        );
        // Cheap pure-filesystem check (no subprocess): is this checkout a
        // divergent full copy living in the worktrees tree?
        let divergent_copy =
            crate::git::is_divergent_copy(std::path::Path::new(&checkout_path));

        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
        {
            // kind: a worktree has worktree_path set (the per-branch
            // checkout); otherwise this is the repo root (main).
            let kind = if workspace.worktree_path.is_some() {
                "worktree"
            } else {
                "main"
            };
            workspace.project_root = Some(project_root);
            workspace.project_uid = Some(project_uid);
            workspace.workspace_kind = Some(kind.to_string());
            workspace.protected = protected;
            workspace.divergent_copy = divergent_copy;
        }
    }

    /// Boot backfill for `protected` on workspaces that predate the
    /// repo-unit-sync change (new workspaces get it stamped at create
    /// time in `set_workspace_project_root`). Spawns git subprocesses
    /// per workspace, so callers MUST run this off the app-startup path
    /// (a background thread). Skips remote (host-backed) workspaces —
    /// their files may not exist locally and they're never protected on
    /// this device.
    ///
    /// Also chains [`Self::backfill_workspace_activity`] — see the note
    /// at the call site for why the two share one background pass.
    pub fn backfill_workspace_protection(&self) {
        // Snapshot the inputs (id, checkout path, branch) under a brief
        // lock so the git work below runs unlocked.
        let inputs: Vec<(String, String, Option<String>)> = {
            let snapshot = self.inner.lock().unwrap();
            snapshot
                .workspaces
                .iter()
                .filter(|w| w.host_id.is_none())
                .map(|w| {
                    let checkout = w
                        .worktree_path
                        .clone()
                        .or_else(|| w.project_root.clone())
                        .unwrap_or_else(|| w.cwd.clone());
                    (w.workspace_id.0.clone(), checkout, w.git_branch.clone())
                })
                .collect()
        };

        let computed: Vec<(String, bool, bool)> = inputs
            .into_iter()
            .map(|(id, checkout, branch)| {
                let path = std::path::Path::new(&checkout);
                let protected =
                    crate::git::is_protected_repo_root(path, branch.as_deref());
                let divergent_copy = crate::git::is_divergent_copy(path);
                (id, protected, divergent_copy)
            })
            .collect();

        let mut snapshot = self.inner.lock().unwrap();
        for (id, protected, divergent_copy) in computed {
            if let Some(w) = snapshot
                .workspaces
                .iter_mut()
                .find(|w| w.workspace_id.0 == id)
            {
                w.protected = protected;
                w.divergent_copy = divergent_copy;
            }
        }
        drop(snapshot);

        // Ride along on this same off-boot pass rather than claiming a
        // second startup thread: both backfills walk every workspace's
        // checkout with git subprocesses and then take the state lock, so
        // running them back to back costs one thread and one lock
        // hand-off instead of two threads contending during startup.
        self.backfill_workspace_activity();
    }

    /// Boot backfill for `last_active_at` on workspaces persisted before
    /// that field existed. Without it every pre-existing workspace would
    /// read as "unknown" forever and the idle sweep would never act on
    /// the history that is actually sitting on disk.
    ///
    /// Shells out to git per workspace, so it inherits
    /// `backfill_workspace_protection`'s rule: never on the app-startup
    /// path. Cost is bounded — only workspaces still missing the stamp
    /// are probed, so this is a one-time pass that goes empty on the
    /// next launch.
    ///
    /// Skips host-backed and attach-only workspaces: their paths name
    /// directories on another machine, so probing them locally would
    /// either miss or, worse, date an unrelated same-named directory on
    /// this device.
    ///
    /// That skip is why adopted workspaces
    /// ([`Self::create_synced_workspace_shell`] /
    /// [`Self::create_synced_root_shell`]) are dated a launch late rather
    /// than never: the shell carries `host_id` only until its pull lands,
    /// and adoption clears it as soon as the files are local, so the next
    /// boot pass sees an ordinary local checkout with no stamp and reads
    /// its real date out of the git history that came across with it.
    /// Until then it stays `None`, which the frontend treats as unknown
    /// and refuses to sweep on — the honest state for a workspace whose
    /// files have not arrived yet.
    pub fn backfill_workspace_activity(&self) {
        let inputs: Vec<(String, String)> = {
            let snapshot = self.inner.lock().unwrap();
            snapshot
                .workspaces
                .iter()
                .filter(|w| w.last_active_at.is_none() && w.host_id.is_none() && !w.attach_only)
                .map(|w| {
                    let checkout = w
                        .worktree_path
                        .clone()
                        .or_else(|| w.project_root.clone())
                        .unwrap_or_else(|| w.cwd.clone());
                    (w.workspace_id.0.clone(), checkout)
                })
                .collect()
        };
        if inputs.is_empty() {
            return;
        }

        let computed: Vec<(String, i64)> = inputs
            .into_iter()
            .filter_map(|(id, checkout)| {
                derive_last_activity_ms(Path::new(&checkout)).map(|ms| (id, ms))
            })
            .collect();

        let mut snapshot = self.inner.lock().unwrap();
        for (id, derived) in computed {
            if let Some(w) = snapshot
                .workspaces
                .iter_mut()
                .find(|w| w.workspace_id.0 == id)
            {
                // The lock was released for the git work above, so an
                // agent may have stamped a real, newer timestamp in the
                // meantime — a derived guess must never overwrite it.
                if w.last_active_at.is_none() {
                    w.last_active_at = Some(derived);
                }
            }
        }
    }

    /// Stamp a workspace's cross-device project identity directly from
    /// known values (e.g. the synced row the workspace was adopted
    /// from), WITHOUT recomputing the uid or touching `project_root`.
    ///
    /// Adoption uses this instead of `set_workspace_project_root`: the
    /// synced row already carries the daemon-computed `project_uid`, so
    /// copying it verbatim guarantees the adopted workspace converges
    /// with its siblings on every device — including local-only repos,
    /// whose path-derived uid would otherwise DIVERGE if recomputed
    /// against the (different) local landing path. Without this stamp
    /// the shell's `project_uid: None` also gets pushed back to the
    /// sync row by `reconcile_from_snapshot` (a plain SET, not COALESCE),
    /// actively wiping the row's identity.
    pub fn set_workspace_project_identity(
        &self,
        workspace_id: &str,
        project_uid: Option<String>,
        workspace_kind: Option<String>,
    ) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
        {
            if project_uid.is_some() {
                workspace.project_uid = project_uid;
            }
            if workspace_kind.is_some() {
                workspace.workspace_kind = workspace_kind;
            }
        }
    }

    pub fn set_workspace_type(&self, workspace_id: &str, workspace_type: WorkspaceType) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
        {
            workspace.workspace_type = workspace_type;
            true
        } else {
            false
        }
    }

    /// First workspace with `workspace_type == Home`, if any.
    pub fn find_home_workspace_id(&self) -> Option<String> {
        let snapshot = self.inner.lock().unwrap();
        snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_type == WorkspaceType::Home)
            .map(|w| w.workspace_id.0.clone())
    }

    /// Update detected ports. Returns true if the port list actually changed.
    pub fn update_detected_ports(&self, ports: Vec<PortInfoSnapshot>) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if snapshot.detected_ports == ports {
            return false;
        }
        snapshot.detected_ports = ports;
        true
    }

    /// Returns workspace_id -> cwd for all workspaces.
    pub fn all_workspace_cwds(&self) -> std::collections::HashMap<String, String> {
        let snapshot = self.inner.lock().unwrap();
        snapshot
            .workspaces
            .iter()
            // Attach-in-place ("open on host") workspaces have no local
            // checkout — their `cwd` is a host path that doesn't exist on
            // this device, so the periodic git enrichment sweep must skip
            // them (it would spawn 5-8 doomed git subprocesses every tick).
            .filter(|w| !w.attach_only)
            .map(|w| (w.workspace_id.0.clone(), w.cwd.clone()))
            .collect()
    }

    /// Returns absolute path -> workspace_id for all workspaces, covering
    /// both the working directory and the git worktree path. The inverse
    /// direction of `all_workspace_cwds` (path is the key) because Docker
    /// container matching looks up by the compose `working_dir` path. Paths
    /// are stored with any trailing slash trimmed so lookups normalize.
    pub fn all_workspace_paths(&self) -> std::collections::HashMap<String, String> {
        let snapshot = self.inner.lock().unwrap();
        let mut map = std::collections::HashMap::new();
        for w in &snapshot.workspaces {
            let id = w.workspace_id.0.clone();
            let cwd = w.cwd.trim_end_matches('/');
            if !cwd.is_empty() {
                map.insert(cwd.to_string(), id.clone());
            }
            if let Some(worktree_path) = &w.worktree_path {
                let wt = worktree_path.trim_end_matches('/');
                if !wt.is_empty() {
                    map.insert(wt.to_string(), id.clone());
                }
            }
        }
        map
    }

    /// Returns session_id -> workspace_id for all terminal sessions across all workspaces.
    pub fn all_session_workspaces(&self) -> std::collections::HashMap<String, String> {
        let snapshot = self.inner.lock().unwrap();
        let mut map = std::collections::HashMap::new();
        for workspace in &snapshot.workspaces {
            for session_id in collect_terminal_sessions(&workspace.surfaces) {
                map.insert(session_id, workspace.workspace_id.0.clone());
            }
        }
        map
    }

    pub fn active_workspace_cwd(&self) -> Option<(String, String)> {
        let snapshot = self.inner.lock().unwrap();
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == snapshot.active_workspace_id)?;
        // Attach-in-place workspaces have no local checkout, so the active-
        // workspace PR/issue refresh (which shells out to `gh` in `cwd`)
        // would run against a path that doesn't exist on this device.
        if workspace.attach_only {
            return None;
        }
        Some((workspace.workspace_id.0.clone(), workspace.cwd.clone()))
    }

    pub fn update_workspace_cwd(&self, workspace_id: &str, cwd: String) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            workspace.cwd = cwd;
            return true;
        }
        false
    }

    pub fn close_workspace(&self, workspace_id: &str) -> Result<CloseWorkspaceResult, String> {
        let mut snapshot = self.inner.lock().unwrap();

        if snapshot.workspaces.len() <= 1 {
            let workspace_index = snapshot
                .workspaces
                .iter()
                .position(|workspace| workspace.workspace_id.0 == workspace_id)
                .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

            let removed = snapshot.workspaces.remove(workspace_index);
            let removed_session_id_strings = collect_terminal_sessions(&removed.surfaces);
            let removed_agent_chat_threads = collect_agent_chat_threads(&removed.surfaces);
            snapshot
                .notifications
                .retain(|notification| notification.workspace_id != removed.workspace_id);
            snapshot.terminal_sessions.retain(|session| {
                !removed_session_id_strings
                    .iter()
                    .any(|id| id == &session.session_id.0)
            });
            let removed_agent_browser_sessions: Vec<String> = snapshot
                .agent_browser_sessions
                .iter()
                .filter(|s| s.workspace_id.0 == workspace_id)
                .map(|s| s.cli_session_name.clone())
                .collect();
            snapshot
                .agent_browser_sessions
                .retain(|s| s.workspace_id.0 != workspace_id);
            snapshot.active_workspace_id = WorkspaceId("".into());
            return Ok(CloseWorkspaceResult {
                fallback: WorkspaceId("".into()),
                removed_sessions: removed_session_id_strings
                    .into_iter()
                    .map(SessionId)
                    .collect(),
                removed_agent_chat_threads,
                removed_agent_browser_sessions,
            });
        }

        let workspace_index = snapshot
            .workspaces
            .iter()
            .position(|workspace| workspace.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

        let removed = snapshot.workspaces.remove(workspace_index);
        let removed_session_id_strings = collect_terminal_sessions(&removed.surfaces);
        let removed_agent_chat_threads = collect_agent_chat_threads(&removed.surfaces);
        snapshot
            .notifications
            .retain(|notification| notification.workspace_id != removed.workspace_id);
        snapshot.terminal_sessions.retain(|session| {
            !removed_session_id_strings
                .iter()
                .any(|id| id == &session.session_id.0)
        });
        let removed_agent_browser_sessions: Vec<String> = snapshot
            .agent_browser_sessions
            .iter()
            .filter(|s| s.workspace_id.0 == workspace_id)
            .map(|s| s.cli_session_name.clone())
            .collect();
        snapshot
            .agent_browser_sessions
            .retain(|s| s.workspace_id.0 != workspace_id);
        let fallback_workspace = snapshot
            .workspaces
            .first()
            .map(|workspace| workspace.workspace_id.clone())
            .ok_or_else(|| "No fallback workspace available".to_string())?;
        snapshot.active_workspace_id = fallback_workspace.clone();

        Ok(CloseWorkspaceResult {
            fallback: fallback_workspace,
            removed_sessions: removed_session_id_strings
                .into_iter()
                .map(SessionId)
                .collect(),
            removed_agent_chat_threads,
            removed_agent_browser_sessions,
        })
    }

    /// Add an archive entry, first evicting any stale entry for the same
    /// on-disk location (same `(cwd, worktree_path)` pair). Dedupe keyed
    /// on location rather than workspace id because re-archiving the
    /// same folder produces a NEW workspace id each cycle — without the
    /// location key the list would grow one ghost entry per
    /// archive/restore round-trip.
    pub fn add_archived_workspace(&self, entry: ArchivedWorkspaceSnapshot) {
        let mut snapshot = self.inner.lock().unwrap();
        snapshot
            .archived_workspaces
            .retain(|e| !(e.cwd == entry.cwd && e.worktree_path == entry.worktree_path));
        snapshot.archived_workspaces.push(entry);
    }

    /// Remove and return the archive entry with `archive_id`. Returning
    /// the removed entry lets callers act on its metadata (e.g. the
    /// worktree path) without a second lookup racing a concurrent
    /// mutation.
    pub fn remove_archived_workspace(
        &self,
        archive_id: &str,
    ) -> Result<ArchivedWorkspaceSnapshot, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let index = snapshot
            .archived_workspaces
            .iter()
            .position(|e| e.archive_id == archive_id)
            .ok_or_else(|| format!("No archived workspace found for {archive_id}"))?;
        Ok(snapshot.archived_workspaces.remove(index))
    }

    /// Clone of just the archive list — for readers (e.g. the
    /// `list_archived_workspaces` control command) that would otherwise
    /// clone the entire snapshot to look at one vec.
    pub fn archived_workspaces_list(&self) -> Vec<ArchivedWorkspaceSnapshot> {
        self.inner.lock().unwrap().archived_workspaces.clone()
    }

    /// Clone of the live workspace with `workspace_id`, if any — the
    /// narrow sibling of `snapshot()` for single-workspace lookups.
    pub fn find_workspace(&self, workspace_id: &str) -> Option<WorkspaceSnapshot> {
        let snapshot = self.inner.lock().unwrap();
        snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .cloned()
    }

    /// Clone of the archive entry with `archive_id`, if any. Read-only
    /// sibling of `remove_archived_workspace` for flows that must keep
    /// the entry until the restore/delete actually succeeded.
    pub fn find_archived_workspace(&self, archive_id: &str) -> Option<ArchivedWorkspaceSnapshot> {
        let snapshot = self.inner.lock().unwrap();
        snapshot
            .archived_workspaces
            .iter()
            .find(|e| e.archive_id == archive_id)
            .cloned()
    }

    pub fn reorder_workspaces(&self, workspace_ids: Vec<String>) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if workspace_ids.len() != snapshot.workspaces.len() {
            return false;
        }
        let mut reordered = Vec::with_capacity(workspace_ids.len());
        for wid in &workspace_ids {
            if let Some(ws) = snapshot.workspaces.iter().find(|w| w.workspace_id.0 == *wid).cloned() {
                reordered.push(ws);
            } else {
                return false;
            }
        }
        snapshot.workspaces = reordered;
        true
    }

    pub fn reorder_tabs(&self, workspace_id: &str, tab_ids: Vec<String>) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace = match snapshot.workspaces.iter_mut().find(|w| w.workspace_id.0 == workspace_id) {
            Some(ws) => ws,
            None => return false,
        };
        if tab_ids.len() != workspace.tabs.len() {
            return false;
        }
        let mut reordered = Vec::with_capacity(tab_ids.len());
        for tid in &tab_ids {
            if let Some(tab) = workspace.tabs.iter().find(|t| t.tab_id == *tid).cloned() {
                reordered.push(tab);
            } else {
                return false;
            }
        }
        workspace.tabs = reordered;
        true
    }

    pub fn workspace_navigation_target(&self, step: isize) -> Option<WorkspaceId> {
        let snapshot = self.inner.lock().unwrap();
        let current_index = snapshot
            .workspaces
            .iter()
            .position(|workspace| workspace.workspace_id == snapshot.active_workspace_id)?;
        let total = snapshot.workspaces.len() as isize;
        if total == 0 {
            return None;
        }

        let next_index = (current_index as isize + step).rem_euclid(total) as usize;
        snapshot
            .workspaces
            .get(next_index)
            .map(|workspace| workspace.workspace_id.clone())
    }

    pub fn pane_navigation_target(&self, step: isize) -> Option<PaneId> {
        let snapshot = self.inner.lock().unwrap();
        let (workspace_index, surface_index) = active_workspace_surface_indices(&snapshot)?;
        let surface = snapshot
            .workspaces
            .get(workspace_index)?
            .surfaces
            .get(surface_index)?;
        let pane_ids = collect_leaf_pane_ids(&surface.root);
        let current_index = pane_ids
            .iter()
            .position(|pane_id| *pane_id == surface.active_pane_id)?
            as isize;
        let total = pane_ids.len() as isize;
        if total == 0 {
            return None;
        }

        let next_index = (current_index + step).rem_euclid(total) as usize;
        pane_ids.get(next_index).cloned()
    }

    pub fn create_terminal_session(&self) -> Result<SessionId, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let active_workspace_id = snapshot.active_workspace_id.clone();
        let workspace_terminal_count = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == active_workspace_id)
            .map(terminal_count_for_workspace)
            .ok_or_else(|| "No active workspace available".to_string())?;

        if workspace_terminal_count >= MAX_TERMINAL_SESSIONS {
            return Err(format!(
                "Reached the current workspace terminal limit of {MAX_TERMINAL_SESSIONS}"
            ));
        }

        let session_id = SessionId(next_id("session"));
        let pane_id = PaneId(next_id("pane"));
        let cwd = current_project_root().display().to_string();
        let shell = env::var("SHELL").ok();
        let title = format!("Terminal {}", snapshot.terminal_sessions.len() + 1);

        snapshot.terminal_sessions.push(TerminalSessionSnapshot {
            session_id: session_id.clone(),
            title: title.clone(),
            shell: shell.clone(),
            cwd: cwd.clone(),
            cols: 80,
            rows: 24,
            state: TerminalSessionState::Starting,
            last_message: Some("Preparing shell session".into()),
            exit_code: None,
            original_command: None,
            adapter_captures: Default::default(),
        });

        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id == active_workspace_id)
        {
            if let Some(surface) = workspace
                .surfaces
                .iter_mut()
                .find(|surface| surface.surface_id == workspace.active_surface_id)
            {
                let target_pane_id = surface.active_pane_id.0.clone();
                let split_pane_id = PaneId(next_id("pane"));
                let inserted = insert_split_at_pane_with_behavior(
                    &mut surface.root,
                    &target_pane_id,
                    split_pane_id,
                    SplitDirection::Horizontal,
                    PaneNodeSnapshot::Terminal {
                        pane_id: pane_id.clone(),
                        session_id: session_id.clone(),
                        title: title.clone(),
                    },
                    WorkspaceInsertBehavior::Smart,
                );

                if inserted {
                    surface.active_pane_id = pane_id;
                }
            }
        }

        Ok(session_id)
    }

    pub fn split_pane(
        &self,
        pane_id: &str,
        direction: SplitDirection,
    ) -> Result<SessionId, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let (workspace_index, surface_index) = find_pane_location(&snapshot.workspaces, pane_id)
            .ok_or_else(|| format!("No pane found for {pane_id}"))?;
        let workspace_terminal_count = snapshot
            .workspaces
            .get(workspace_index)
            .map(terminal_count_for_workspace)
            .ok_or_else(|| "Workspace disappeared while splitting pane".to_string())?;

        if workspace_terminal_count >= MAX_TERMINAL_SESSIONS {
            return Err(format!(
                "Reached the current workspace terminal limit of {MAX_TERMINAL_SESSIONS}"
            ));
        }

        let session_id = SessionId(next_id("session"));
        let new_pane_id = PaneId(next_id("pane"));
        let split_pane_id = PaneId(next_id("pane"));
        let cwd = snapshot
            .workspaces
            .get(workspace_index)
            .map(|w| w.cwd.clone())
            .unwrap_or_else(|| current_project_root().display().to_string());
        let shell = env::var("SHELL").ok();
        let title = format!("Terminal {}", snapshot.terminal_sessions.len() + 1);

        snapshot.terminal_sessions.push(TerminalSessionSnapshot {
            session_id: session_id.clone(),
            title: title.clone(),
            shell,
            cwd,
            cols: 80,
            rows: 24,
            state: TerminalSessionState::Starting,
            last_message: Some("Preparing shell session".into()),
            exit_code: None,
            original_command: None,
            adapter_captures: Default::default(),
        });

        let workspace = snapshot
            .workspaces
            .get_mut(workspace_index)
            .ok_or_else(|| "Workspace disappeared while splitting pane".to_string())?;
        let surface = workspace
            .surfaces
            .get_mut(surface_index)
            .ok_or_else(|| "Surface disappeared while splitting pane".to_string())?;

        let inserted = insert_split_at_pane(
            &mut surface.root,
            pane_id,
            split_pane_id,
            direction,
            PaneNodeSnapshot::Terminal {
                pane_id: new_pane_id.clone(),
                session_id: session_id.clone(),
                title: title.clone(),
            },
        );

        if !inserted {
            return Err(format!("Failed to split pane {pane_id}"));
        }

        workspace.active_surface_id = surface.surface_id.clone();
        surface.active_pane_id = new_pane_id;
        snapshot.active_workspace_id = workspace.workspace_id.clone();

        Ok(session_id)
    }

    pub fn resize_split(&self, pane_id: &str, child_sizes: Vec<f32>) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let (workspace_index, surface_index) = find_pane_location(&snapshot.workspaces, pane_id)
            .ok_or_else(|| format!("No pane found for {pane_id}"))?;

        let workspace = snapshot
            .workspaces
            .get_mut(workspace_index)
            .ok_or_else(|| "Workspace disappeared while resizing split".to_string())?;
        let surface = workspace
            .surfaces
            .get_mut(surface_index)
            .ok_or_else(|| "Surface disappeared while resizing split".to_string())?;

        if update_split_sizes(&mut surface.root, pane_id, child_sizes) {
            Ok(())
        } else {
            Err(format!("Pane {pane_id} is not a split pane"))
        }
    }

    pub fn resize_active_pane(&self, delta: f32) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let (workspace_index, surface_index) = active_workspace_surface_indices(&snapshot)
            .ok_or_else(|| "No active surface available".to_string())?;

        let workspace = snapshot
            .workspaces
            .get_mut(workspace_index)
            .ok_or_else(|| "Workspace disappeared while resizing active pane".to_string())?;
        let surface = workspace
            .surfaces
            .get_mut(surface_index)
            .ok_or_else(|| "Surface disappeared while resizing active pane".to_string())?;

        if nudge_active_pane_size(&mut surface.root, &surface.active_pane_id, delta) {
            Ok(())
        } else {
            Err("No resizable split found for active pane".into())
        }
    }

    pub fn close_terminal_session(&self, session_id: &str) -> Result<SessionId, String> {
        let mut snapshot = self.inner.lock().unwrap();

        if snapshot.terminal_sessions.len() <= 1 {
            return Err("Cannot close the last terminal session".into());
        }

        let session_index = snapshot
            .terminal_sessions
            .iter()
            .position(|session| session.session_id.0 == session_id)
            .ok_or_else(|| format!("No terminal session found for {session_id}"))?;
        snapshot.terminal_sessions.remove(session_index);

        let mut fallback_session_id: Option<SessionId> = None;
        let mut next_active_workspace_id: Option<WorkspaceId> = None;

        for workspace in &mut snapshot.workspaces {
            for surface in &mut workspace.surfaces {
                if !pane_tree_contains_session(&surface.root, session_id) {
                    continue;
                }

                let updated_root = remove_terminal_from_tree(&surface.root, session_id)
                    .ok_or_else(|| {
                        format!("Unable to remove session {session_id} from pane tree")
                    })?;
                let (next_pane_id, next_session_id) = first_terminal_pane(&updated_root)
                    .ok_or_else(|| {
                        "Pane tree lost its last terminal pane unexpectedly".to_string()
                    })?;

                surface.root = updated_root;
                surface.active_pane_id = next_pane_id;
                workspace.active_surface_id = surface.surface_id.clone();
                next_active_workspace_id = Some(workspace.workspace_id.clone());
                fallback_session_id = Some(next_session_id);
                break;
            }
        }

        if let Some(workspace_id) = next_active_workspace_id {
            snapshot.active_workspace_id = workspace_id;
        }

        fallback_session_id
            .ok_or_else(|| format!("No fallback session available after closing {session_id}"))
    }

    /// Bulk-remove terminal sessions by ID without the "last session" guard.
    /// Also removes their panes from all workspace trees. Use when stopping an OpenFlow run.
    pub fn remove_terminal_sessions(&self, session_ids: &[String]) {
        let ids: std::collections::HashSet<&str> =
            session_ids.iter().map(String::as_str).collect();
        if ids.is_empty() {
            return;
        }
        let mut snapshot = self.inner.lock().unwrap();
        snapshot
            .terminal_sessions
            .retain(|s| !ids.contains(s.session_id.0.as_str()));
        for workspace in &mut snapshot.workspaces {
            for surface in &mut workspace.surfaces {
                if let Some(new_root) = remove_terminals_from_tree(&surface.root, &ids) {
                    surface.root = new_root;
                } else {
                    surface.root = PaneNodeSnapshot::Split {
                        pane_id: PaneId(next_id("pane")),
                        direction: SplitDirection::Vertical,
                        child_sizes: vec![100.0],
                        children: vec![],
                    };
                    surface.active_pane_id = pane_id_from_node(&surface.root);
                }
            }
        }
    }

    /// Returns the workspace that contains the given terminal session, if any.
    pub fn workspace_id_for_session(&self, session_id: &str) -> Option<WorkspaceId> {
        let snapshot = self.inner.lock().unwrap();
        find_workspace_id_for_session(&snapshot.workspaces, session_id)
    }

    /// Look up the `(provider, thread_id)` pair for an `AgentChat` pane.
    /// Returns `None` when the pane id is unknown, the pane is not an
    /// agent-chat leaf, or no session has been bound yet. Callers use this
    /// to capture the session-cleanup target before mutating the tree.
    pub fn agent_chat_pane_thread(
        &self,
        pane_id: &str,
    ) -> Option<(crate::agent_provider::ProviderKind, String)> {
        let snapshot = self.inner.lock().unwrap();
        for workspace in &snapshot.workspaces {
            for surface in &workspace.surfaces {
                if let Some(pair) = agent_chat_thread_pair_for_pane(&surface.root, pane_id) {
                    return Some(pair);
                }
            }
        }
        None
    }

    pub fn close_pane(&self, pane_id: &str) -> Result<Option<SessionId>, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let (workspace_index, surface_index) = find_pane_location(&snapshot.workspaces, pane_id)
            .ok_or_else(|| format!("No pane found for {pane_id}"))?;

        let target_pane = PaneId(pane_id.to_string());
        let removed_session_id = {
            let surface = snapshot
                .workspaces
                .get(workspace_index)
                .and_then(|workspace| workspace.surfaces.get(surface_index))
                .ok_or_else(|| "Surface disappeared while closing pane".to_string())?;
            session_id_for_pane(&surface.root, &target_pane)
        };
        let removed_browser_id = {
            let surface = snapshot
                .workspaces
                .get(workspace_index)
                .and_then(|workspace| workspace.surfaces.get(surface_index))
                .ok_or_else(|| "Surface disappeared while closing pane".to_string())?;
            browser_id_for_pane(&surface.root, &target_pane)
        };

        let active_workspace_id: WorkspaceId;

        {
            let workspace = snapshot
                .workspaces
                .get_mut(workspace_index)
                .ok_or_else(|| "Workspace disappeared while closing pane".to_string())?;
            let surface = workspace
                .surfaces
                .get_mut(surface_index)
                .ok_or_else(|| "Surface disappeared while closing pane".to_string())?;

            let ordered_before = collect_leaf_pane_ids(&surface.root);
            let active_before = surface.active_pane_id.clone();
            let updated_root = remove_pane_from_tree(&surface.root, pane_id);

            if let Some(new_root) = updated_root {
                // Pane removed but surface still has content. Keep focus on the
                // current pane when a different pane was closed; when the active
                // pane itself was closed, move to the adjacent pane (next, then
                // previous) rather than the leftmost leaf.
                let next_active_pane =
                    active_id_after_removal(&ordered_before, &active_before, pane_id)
                        .filter(|pid| pane_tree_contains_pane(&new_root, &pid.0))
                        .or_else(|| first_leaf_pane_id(&new_root))
                        .ok_or_else(|| "No fallback pane available after close".to_string())?;
                surface.root = new_root;
                surface.active_pane_id = next_active_pane;
                workspace.active_surface_id = surface.surface_id.clone();
            } else {
                // Last pane closed — remove the surface and its tab
                let surface_id = workspace.surfaces[surface_index].surface_id.clone();
                workspace.surfaces.remove(surface_index);
                // Index of the tab about to be removed, so we can focus its
                // neighbor instead of always jumping to the first tab.
                let removed_tab_index = workspace
                    .tabs
                    .iter()
                    .position(|t| t.surface_id.as_ref() == Some(&surface_id));
                let was_active = workspace.active_surface_id == surface_id;
                workspace.tabs.retain(|t| t.surface_id.as_ref() != Some(&surface_id));
                if workspace.tabs.is_empty() {
                    workspace.active_tab_id = String::new();
                    workspace.active_surface_id = SurfaceId(String::new());
                } else if was_active {
                    // Focus the adjacent tab (next, then previous) — matches
                    // close_tab and active_id_after_removal.
                    let new_index = match removed_tab_index {
                        Some(idx) if idx < workspace.tabs.len() => idx,
                        Some(idx) => idx.saturating_sub(1),
                        None => 0,
                    };
                    let new_tab = &workspace.tabs[new_index];
                    workspace.active_tab_id = new_tab.tab_id.clone();
                    if let Some(ref sid) = new_tab.surface_id {
                        workspace.active_surface_id = sid.clone();
                    }
                }
            }
            active_workspace_id = workspace.workspace_id.clone();
        }

        snapshot.active_workspace_id = active_workspace_id;

        if let Some(session_id) = &removed_session_id {
            snapshot
                .terminal_sessions
                .retain(|session| session.session_id != *session_id);
        }

        if let Some(browser_id) = removed_browser_id {
            snapshot
                .browser_sessions
                .retain(|browser| browser.browser_id != browser_id);
        }

        Ok(removed_session_id)
    }

    pub fn pane_browser_id(&self, pane_id: &str) -> Option<String> {
        let snapshot = self.inner.lock().unwrap();
        let (workspace_index, surface_index) = find_pane_location(&snapshot.workspaces, pane_id)?;
        let surface = snapshot
            .workspaces
            .get(workspace_index)
            .and_then(|workspace| workspace.surfaces.get(surface_index))?;

        browser_id_for_pane(&surface.root, &PaneId(pane_id.to_string()))
            .map(|browser_id| browser_id.0)
    }

    // ── Agent Browser Session Methods ──

    /// Find or create an agent browser session for a workspace.
    pub fn resolve_agent_browser_session(
        &self,
        workspace_id: &str,
        stream_port: u16,
    ) -> AgentBrowserSession {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(session) = snapshot
            .agent_browser_sessions
            .iter()
            .find(|s| s.workspace_id.0 == workspace_id)
        {
            return session.clone();
        }
        // Derive a stable session name from the workspace cwd so the
        // agent-browser Chromium storage state (cookies, localStorage)
        // persists across app restarts. Workspace IDs are regenerated
        // on each startup, but the cwd stays the same.
        let workspace_cwd = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .map(|w| w.cwd.clone());
        let workspace_exists = workspace_cwd.is_some();
        let cli_session_name = workspace_cwd
            .map(|cwd| stable_browser_session_name(&cwd))
            .unwrap_or_else(|| format!("ws-{workspace_id}"));
        let session = AgentBrowserSession {
            session_id: next_id("agent-browser"),
            workspace_id: WorkspaceId(workspace_id.to_string()),
            cli_session_name,
            stream_url: format!("ws://localhost:{stream_port}"),
            current_url: None,
            is_active: false,
            pane_id: None,
            browser_id: None,
            user_dismissed: false,
        };
        // Only persist the record when the workspace actually exists
        // (issue #126 review): an in-flight `browser_automation` racing a
        // workspace close can land here with a dead workspace_id, and a
        // persisted record for a dead workspace can never be closed — it
        // would permanently poison the orphan sweep's live set, which
        // treats every tracked `cli_session_name` as live unconditionally.
        // Returning the transient session keeps the in-flight caller
        // working; if a daemon does get spawned under this name, the
        // periodic sweep reaps it later precisely BECAUSE the name never
        // enters the live set.
        if workspace_exists {
            snapshot.agent_browser_sessions.push(session.clone());
        }
        session
    }

    /// Link an agent browser session to a pane.
    pub fn attach_agent_browser_to_pane(
        &self,
        workspace_id: &str,
        pane_id: &PaneId,
        browser_id: &BrowserId,
    ) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let session = snapshot
            .agent_browser_sessions
            .iter_mut()
            .find(|s| s.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No agent browser session for workspace {workspace_id}"))?;
        session.pane_id = Some(pane_id.clone());
        session.browser_id = Some(browser_id.clone());
        session.is_active = true;
        session.user_dismissed = false;
        Ok(())
    }

    /// Unlink a pane from its agent browser session but keep the session alive.
    /// When `dismissed` is true, prevents auto-recreation on next browser_navigate
    /// (use for explicit user close). When false, the agent can reopen the pane.
    pub fn detach_agent_browser_from_pane(
        &self,
        browser_id: &str,
        dismissed: bool,
    ) -> Option<AgentBrowserSession> {
        let mut snapshot = self.inner.lock().unwrap();
        let session = snapshot
            .agent_browser_sessions
            .iter_mut()
            .find(|s| s.browser_id.as_ref().map(|b| b.0.as_str()) == Some(browser_id))?;
        session.pane_id = None;
        session.browser_id = None;
        if dismissed {
            session.user_dismissed = true;
        }
        Some(session.clone())
    }

    /// Check if a browser_id belongs to an agent browser session.
    pub fn is_agent_browser_session(&self, browser_id: &str) -> bool {
        let snapshot = self.inner.lock().unwrap();
        snapshot
            .agent_browser_sessions
            .iter()
            .any(|s| s.browser_id.as_ref().map(|b| b.0.as_str()) == Some(browser_id))
    }

    /// Find an active but pane-less agent browser session for a workspace.
    pub fn find_detached_agent_browser(
        &self,
        workspace_id: &str,
    ) -> Option<AgentBrowserSession> {
        let snapshot = self.inner.lock().unwrap();
        snapshot
            .agent_browser_sessions
            .iter()
            .find(|s| s.workspace_id.0 == workspace_id && s.is_active && s.pane_id.is_none())
            .cloned()
    }

    /// Return the stream port currently recorded for a workspace's agent
    /// browser session, parsed from `stream_url`. Used by `control.rs`
    /// to resolve the canonical `cli_session_name` before allocating a
    /// new port — see P2 in `docs/plans/browser-stream-fix.md`. Returns
    /// `None` when the workspace has no session yet (first call).
    pub fn agent_browser_stream_port_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Option<u16> {
        let snapshot = self.inner.lock().unwrap();
        let session = snapshot
            .agent_browser_sessions
            .iter()
            .find(|s| s.workspace_id.0 == workspace_id)?;
        session
            .stream_url
            .rsplit(':')
            .next()
            .and_then(|p| p.parse::<u16>().ok())
    }

    /// Update the stream port recorded on an agent browser session so
    /// the frontend's `stream_url` becomes reactive on re-allocation
    /// (P6 in `docs/plans/browser-stream-fix.md`). Returns Err when no
    /// session exists for the workspace yet — callers should
    /// `resolve_agent_browser_session` first if that matters.
    pub fn update_agent_browser_stream_port(
        &self,
        workspace_id: &str,
        stream_port: u16,
    ) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let session = snapshot
            .agent_browser_sessions
            .iter_mut()
            .find(|s| s.workspace_id.0 == workspace_id)
            .ok_or_else(|| {
                format!("No agent browser session for workspace {workspace_id}")
            })?;
        session.stream_url = format!("ws://localhost:{stream_port}");
        Ok(())
    }

    /// Mark an agent browser session as live/active without attaching it to
    /// a pane. Used by the GUI-mode background-browsing gate in
    /// `control.rs`'s `browser_automation` handler: when pane creation is
    /// suppressed (Agent Chat GUI beta on, non-OpenFlow workspace), the
    /// session would otherwise never flip `is_active` (only
    /// `attach_agent_browser_to_pane` does that), so the frontend's
    /// "background session is live" chip/indicator would never see it.
    /// Returns `false` when no session exists yet for the workspace.
    pub fn mark_agent_browser_active(&self, workspace_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        let Some(session) = snapshot
            .agent_browser_sessions
            .iter_mut()
            .find(|s| s.workspace_id.0 == workspace_id)
        else {
            return false;
        };
        session.is_active = true;
        true
    }

    /// Mark an agent browser session as no longer live. Mirror of
    /// [`mark_agent_browser_active`](Self::mark_agent_browser_active): the
    /// `browser_automation` handler calls this when a `close` action
    /// completes successfully, so the GUI-mode background chip /
    /// context-bar indicator / peek overlay (which key off `is_active`)
    /// stop presenting the session as live. The session itself is kept
    /// (URL, cli_session_name) for a later reopen — this only flips the
    /// liveness flag. Returns `false` when no session exists for the
    /// workspace.
    pub fn mark_agent_browser_inactive(&self, workspace_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        let Some(session) = snapshot
            .agent_browser_sessions
            .iter_mut()
            .find(|s| s.workspace_id.0 == workspace_id)
        else {
            return false;
        };
        session.is_active = false;
        true
    }

    /// Release a workspace's *background/detached* agent browser session
    /// when the agent-chat run that was driving it finishes. Backs the
    /// run-lifecycle wiring in `agent_chat.rs`'s `publish_pane_status`:
    /// agents rarely call `browser close` themselves, so without this the
    /// GUI-mode background chip / context-bar indicator (which key off
    /// `is_active`) would show "LIVE" forever after the turn is long done.
    /// Only flips the flag — and only returns `true` — when the session
    /// exists, is currently active, AND has no `pane_id` (i.e. it is in
    /// background/detached mode, not the legacy split-pane flow). A
    /// pane-attached session has its own lifecycle (explicit user close /
    /// pane close) and must be left untouched here. The session itself
    /// (URL, cli_session_name) is kept so a later browser action from the
    /// agent simply re-marks it active and the chip returns.
    pub fn release_detached_agent_browser(&self, workspace_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        let Some(session) = snapshot
            .agent_browser_sessions
            .iter_mut()
            .find(|s| s.workspace_id.0 == workspace_id)
        else {
            return false;
        };
        if !session.is_active || session.pane_id.is_some() {
            return false;
        }
        session.is_active = false;
        true
    }

    /// Update the current URL on the agent browser session for a workspace.
    pub fn update_agent_browser_url(
        &self,
        workspace_id: &str,
        url: String,
    ) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let session = snapshot
            .agent_browser_sessions
            .iter_mut()
            .find(|s| s.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No agent browser session for workspace {workspace_id}"))?;
        session.current_url = Some(url);
        Ok(())
    }

    /// Find the workspace that contains a given pane.
    pub fn workspace_id_for_pane(&self, pane_id: &str) -> Option<String> {
        let snapshot = self.inner.lock().unwrap();
        find_workspace_id_for_pane(&snapshot.workspaces, pane_id).map(|wid| wid.0)
    }

    pub fn swap_panes(&self, source_pane_id: &str, target_pane_id: &str) -> Result<(), String> {
        if source_pane_id == target_pane_id {
            return Ok(());
        }

        let mut snapshot = self.inner.lock().unwrap();

        let source_location = find_pane_location(&snapshot.workspaces, source_pane_id)
            .ok_or_else(|| format!("No pane found for {source_pane_id}"))?;
        let target_location = find_pane_location(&snapshot.workspaces, target_pane_id)
            .ok_or_else(|| format!("No pane found for {target_pane_id}"))?;

        if source_location != target_location {
            return Err("Pane swapping is currently limited to the same workspace surface".into());
        }

        let (workspace_index, surface_index) = source_location;
        let workspace = snapshot
            .workspaces
            .get_mut(workspace_index)
            .ok_or_else(|| "Workspace disappeared while swapping panes".to_string())?;
        let surface = workspace
            .surfaces
            .get_mut(surface_index)
            .ok_or_else(|| "Surface disappeared while swapping panes".to_string())?;

        let source_node = clone_pane_node(&surface.root, source_pane_id)
            .ok_or_else(|| format!("Failed to clone source pane {source_pane_id}"))?;
        let target_node = clone_pane_node(&surface.root, target_pane_id)
            .ok_or_else(|| format!("Failed to clone target pane {target_pane_id}"))?;
        let temp_pane_id = PaneId(next_id("pane-swap-temp"));
        let temp_source_node = with_pane_id(source_node.clone(), temp_pane_id.clone());

        if !replace_pane_node(&mut surface.root, source_pane_id, temp_source_node) {
            return Err(format!("Failed to replace source pane {source_pane_id}"));
        }

        if !replace_pane_node(&mut surface.root, target_pane_id, source_node.clone()) {
            return Err(format!("Failed to replace target pane {target_pane_id}"));
        }

        if !replace_pane_node(&mut surface.root, &temp_pane_id.0, target_node.clone()) {
            return Err(format!(
                "Failed to replace temporary pane {}",
                temp_pane_id.0
            ));
        }

        if surface.active_pane_id.0 == source_pane_id {
            surface.active_pane_id = pane_id_from_node(&target_node);
        } else if surface.active_pane_id.0 == target_pane_id {
            surface.active_pane_id = pane_id_from_node(&source_node);
        }

        snapshot.active_workspace_id = workspace.workspace_id.clone();
        Ok(())
    }

    pub fn update_browser_url(&self, browser_id: &str, url: String) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let browser = snapshot
            .browser_sessions
            .iter_mut()
            .find(|browser| browser.browser_id.0 == browser_id)
            .ok_or_else(|| format!("No browser session found for {browser_id}"))?;

        let normalized = normalize_url(&url);
        if browser.history_index + 1 < browser.history.len() {
            browser.history.truncate(browser.history_index + 1);
        }
        browser.history.push(normalized.clone());
        browser.history_index = browser.history.len() - 1;
        browser.current_url = Some(normalized);
        browser.is_loading = true;
        browser.last_error = None;
        Ok(())
    }

    /// Mark a browser session as agent-backed so the pane connects to the
    /// agent's existing stream instead of starting its own daemon.
    pub fn set_browser_agent_session_name(&self, browser_id: &BrowserId, session_name: String) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(browser) = snapshot
            .browser_sessions
            .iter_mut()
            .find(|b| b.browser_id == *browser_id)
        {
            browser.agent_session_name = Some(session_name);
        }
    }

    pub fn browser_history_step(&self, browser_id: &str, step: isize) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let browser = snapshot
            .browser_sessions
            .iter_mut()
            .find(|browser| browser.browser_id.0 == browser_id)
            .ok_or_else(|| format!("No browser session found for {browser_id}"))?;

        if browser.history.is_empty() {
            return Ok(());
        }

        let current = browser.history_index as isize;
        let next = (current + step).clamp(0, browser.history.len() as isize - 1) as usize;
        browser.history_index = next;
        browser.current_url = browser.history.get(next).cloned();
        browser.is_loading = true;
        browser.last_error = None;
        Ok(())
    }

    pub fn reload_browser(&self, browser_id: &str) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let browser = snapshot
            .browser_sessions
            .iter_mut()
            .find(|browser| browser.browser_id.0 == browser_id)
            .ok_or_else(|| format!("No browser session found for {browser_id}"))?;
        browser.is_loading = true;
        browser.last_error = None;
        Ok(())
    }

    pub fn set_browser_loading_state(
        &self,
        browser_id: &str,
        is_loading: bool,
        error: Option<String>,
    ) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let browser = snapshot
            .browser_sessions
            .iter_mut()
            .find(|browser| browser.browser_id.0 == browser_id)
            .ok_or_else(|| format!("No browser session found for {browser_id}"))?;
        browser.is_loading = is_loading;
        browser.last_error = error;
        Ok(())
    }

    pub fn update_terminal_session_shell(&self, session: &str, shell: String) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(terminal) = snapshot
            .terminal_sessions
            .iter_mut()
            .find(|terminal| terminal.session_id.0 == session)
        {
            terminal.shell = Some(shell);
            return true;
        }
        false
    }

    pub fn update_terminal_session_command(&self, session: &str, command: String) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(terminal) = snapshot
            .terminal_sessions
            .iter_mut()
            .find(|terminal| terminal.session_id.0 == session)
        {
            terminal.original_command = Some(command);
            return true;
        }
        false
    }

    /// Store an adapter capture key-value pair on a terminal session.
    /// Called immediately when hooks fire so each pane owns its own data.
    pub fn set_terminal_adapter_capture(&self, session: &str, key: &str, value: &str) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(terminal) = snapshot
            .terminal_sessions
            .iter_mut()
            .find(|terminal| terminal.session_id.0 == session)
        {
            terminal.adapter_captures.insert(key.to_string(), value.to_string());
        }
    }

    /// Get all adapter captures for a terminal session.
    pub fn get_terminal_adapter_captures(&self, session: &str) -> std::collections::HashMap<String, String> {
        let snapshot = self.inner.lock().unwrap();
        snapshot
            .terminal_sessions
            .iter()
            .find(|terminal| terminal.session_id.0 == session)
            .map(|t| t.adapter_captures.clone())
            .unwrap_or_default()
    }

    /// Clear all adapter captures for a terminal session (e.g. when user clicks "Start fresh").
    pub fn clear_terminal_adapter_captures(&self, session: &str) {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(terminal) = snapshot
            .terminal_sessions
            .iter_mut()
            .find(|terminal| terminal.session_id.0 == session)
        {
            terminal.adapter_captures.clear();
        }
    }

    pub fn update_terminal_session_size(&self, session: &str, cols: u16, rows: u16) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(terminal) = snapshot
            .terminal_sessions
            .iter_mut()
            .find(|terminal| terminal.session_id.0 == session)
        {
            terminal.cols = cols;
            terminal.rows = rows;
            return true;
        }
        false
    }

    pub fn update_terminal_session_status(
        &self,
        session: &str,
        state: TerminalSessionState,
        message: Option<String>,
        exit_code: Option<u32>,
    ) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if let Some(terminal) = snapshot
            .terminal_sessions
            .iter_mut()
            .find(|terminal| terminal.session_id.0 == session)
        {
            terminal.state = state;
            terminal.last_message = message;
            terminal.exit_code = exit_code;
            return true;
        }
        false
    }

    /// Set agent status for a pane. Idle removes the entry.
    ///
    /// A non-idle status is the one trustworthy "an agent is doing
    /// something here" signal the backend sees, so it doubles as the
    /// activity stamp the idle sweep reads. Idle is deliberately not
    /// stamped — going quiet is the absence of work, not work.
    pub fn set_pane_status(&self, pane_id: &str, status: PaneStatus) {
        let mut snapshot = self.inner.lock().unwrap();
        if status == PaneStatus::Idle {
            snapshot.pane_statuses.remove(pane_id);
        } else {
            snapshot.pane_statuses.insert(pane_id.to_string(), status);
            if let Some(workspace_id) = find_workspace_id_for_pane(&snapshot.workspaces, pane_id) {
                stamp_workspace_activity(&mut snapshot, &workspace_id);
            }
        }
    }

    /// Resolve session_id to pane_id and set status. Returns true if found.
    pub fn set_pane_status_by_session(&self, session_id: &str, status: PaneStatus) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        // The owning workspace is captured alongside the pane during the
        // same walk — the tree has already been searched, and re-finding
        // the workspace by pane id afterwards would just walk it twice.
        let resolved = snapshot.workspaces.iter().find_map(|ws| {
            ws.surfaces
                .iter()
                .find_map(|s| find_terminal_pane_id(&s.root, session_id))
                .map(|pane_id| (ws.workspace_id.clone(), pane_id))
        });
        if let Some((workspace_id, pane_id)) = resolved {
            if status == PaneStatus::Idle {
                snapshot.pane_statuses.remove(&pane_id.0);
            } else {
                snapshot.pane_statuses.insert(pane_id.0, status);
                stamp_workspace_activity(&mut snapshot, &workspace_id);
            }
            true
        } else {
            false
        }
    }

    /// Resolve a chat `thread_id` to its `AgentChat` pane and update the
    /// pane status, the chat-pane analogue of
    /// [`set_pane_status_by_session`]. The terminal resolver deliberately
    /// skips `AgentChat` nodes (chat panes carry a `thread_id`, not a PTY
    /// `session_id`), so chat sessions need their own thread→pane walk to
    /// publish into the shared `pane_statuses` store the sidebar reads.
    ///
    /// Returns `true` only when the stored status actually changed, so the
    /// high-frequency `content_delta` token stream can skip a redundant
    /// frontend emit once the pane is already `Working`. `Idle` removes the
    /// entry, mirroring the session-keyed variant.
    pub fn set_pane_status_by_thread(&self, thread_id: &str, status: PaneStatus) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        let resolved = snapshot.workspaces.iter().find_map(|ws| {
            ws.surfaces
                .iter()
                .find_map(|s| find_agent_chat_pane_id(&s.root, thread_id))
                .map(|pane_id| (ws.workspace_id.clone(), pane_id))
        });
        let Some((workspace_id, pane_id)) = resolved else {
            return false;
        };
        if status == PaneStatus::Idle {
            return snapshot.pane_statuses.remove(&pane_id.0).is_some();
        }
        if snapshot.pane_statuses.get(&pane_id.0) == Some(&status) {
            return false;
        }
        snapshot.pane_statuses.insert(pane_id.0, status);
        // Stamped below the change-guard on purpose: the `content_delta`
        // stream re-asserts `Working` for every token, and re-stamping on
        // each one would be a write per token for no added fidelity.
        stamp_workspace_activity(&mut snapshot, &workspace_id);
        true
    }

    /// Resolve a chat `thread_id` to the id of the `AgentChat` pane it is
    /// bound to, walking every workspace surface. Returns `None` when no
    /// pane currently carries the thread (e.g. the pane was closed). Used
    /// by the backend auto-resume path to rebuild the workspace env
    /// overlay for a session it is silently restarting.
    pub fn agent_chat_pane_id_for_thread(&self, thread_id: &str) -> Option<String> {
        let snapshot = self.inner.lock().unwrap();
        snapshot.workspaces.iter().find_map(|ws| {
            ws.surfaces
                .iter()
                .find_map(|s| find_agent_chat_pane_id(&s.root, thread_id))
                .map(|pane_id| pane_id.0)
        })
    }

    /// Whether the `AgentChat` pane bound to `thread_id` currently lives in
    /// the active workspace. Mirrors `hooks.rs`'s `is_pane_active_for_session`
    /// so a chat turn that finishes in the workspace the user is already
    /// looking at clears to `Idle` instead of raising a review dot.
    pub fn is_thread_pane_in_active_workspace(&self, thread_id: &str) -> bool {
        let snapshot = self.inner.lock().unwrap();
        snapshot.workspaces.iter().any(|ws| {
            ws.workspace_id == snapshot.active_workspace_id
                && ws
                    .surfaces
                    .iter()
                    .any(|s| find_agent_chat_pane_id(&s.root, thread_id).is_some())
        })
    }

    /// Clear working/permission status for a session (on terminal exit).
    pub fn clear_transient_pane_status_by_session(&self, session_id: &str) {
        let mut snapshot = self.inner.lock().unwrap();
        let pane_id = snapshot.workspaces.iter().find_map(|ws| {
            ws.surfaces
                .iter()
                .find_map(|s| find_terminal_pane_id(&s.root, session_id))
        });
        if let Some(pane_id) = pane_id {
            if let Some(status) = snapshot.pane_statuses.get(&pane_id.0) {
                if matches!(status, PaneStatus::Working | PaneStatus::Permission) {
                    snapshot.pane_statuses.remove(&pane_id.0);
                }
            }
        }
    }

    pub fn add_notification(
        &self,
        session_id: Option<String>,
        pane_id: Option<String>,
        message: String,
        level: NotificationLevel,
    ) -> Result<String, String> {
        let mut snapshot = self.inner.lock().unwrap();

        let workspace_id = if let Some(session_id) = &session_id {
            find_workspace_id_for_session(&snapshot.workspaces, session_id)
                .ok_or_else(|| format!("No workspace found for session {session_id}"))?
        } else if let Some(pane_id) = &pane_id {
            find_workspace_id_for_pane(&snapshot.workspaces, pane_id)
                .ok_or_else(|| format!("No workspace found for pane {pane_id}"))?
        } else {
            snapshot.active_workspace_id.clone()
        };
        let is_active_workspace = workspace_id == snapshot.active_workspace_id;

        let notification_id = next_id("notification");
        snapshot.notifications.push(NotificationSnapshot {
            notification_id: notification_id.clone(),
            workspace_id: workspace_id.clone(),
            pane_id: pane_id.map(PaneId),
            session_id: session_id.map(SessionId),
            level,
            message: message.clone(),
            read: is_active_workspace,
            created_at_ms: current_time_ms(),
        });
        let count = snapshot.notifications.len();
        if count > MAX_NOTIFICATIONS {
            snapshot.notifications.drain(..count - MAX_NOTIFICATIONS);
        }

        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id == workspace_id)
        {
            workspace.notification_count += 1;
            workspace.latest_agent_state = Some(message);
        }

        Ok(notification_id)
    }

    // ---- Tab management ----

    pub fn create_tab(
        &self,
        workspace_id: &str,
        kind: TabKind,
    ) -> Result<(String, Option<SessionId>), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

        if workspace.workspace_type == WorkspaceType::OpenFlow {
            return Err("Cannot create tabs in OpenFlow workspaces".into());
        }

        let tab_id = next_id("tab");
        let mut new_session_id: Option<SessionId> = None;

        match kind {
            TabKind::Terminal => {
                let count = workspace
                    .tabs
                    .iter()
                    .filter(|t| t.kind == TabKind::Terminal)
                    .count();
                let surface_id = SurfaceId(next_id("surface"));
                let pane_id = PaneId(next_id("pane"));
                let session_id = SessionId(next_id("session"));
                let cwd = workspace.cwd.clone();
                let shell = env::var("SHELL").ok();

                workspace.surfaces.push(SurfaceSnapshot {
                    surface_id: surface_id.clone(),
                    title: "Surface".into(),
                    active_pane_id: pane_id.clone(),
                    root: PaneNodeSnapshot::Terminal {
                        pane_id,
                        session_id: session_id.clone(),
                        title: "Terminal".into(),
                    },
                });

                snapshot.terminal_sessions.push(TerminalSessionSnapshot {
                    session_id: session_id.clone(),
                    title: "Terminal".into(),
                    shell,
                    cwd,
                    cols: 80,
                    rows: 24,
                    state: TerminalSessionState::Starting,
                    last_message: None,
                    exit_code: None,
                    original_command: None,
                    adapter_captures: Default::default(),
                });

                new_session_id = Some(session_id);

                // Re-borrow workspace after snapshot mutation
                let workspace = snapshot
                    .workspaces
                    .iter_mut()
                    .find(|w| w.workspace_id.0 == workspace_id)
                    .unwrap();

                workspace.tabs.push(TabSnapshot {
                    tab_id: tab_id.clone(),
                    kind: TabKind::Terminal,
                    title: format!("Terminal {}", count + 1),
                    surface_id: Some(surface_id.clone()),
                    browser_id: None,
                    icon: None,
                });
                workspace.active_tab_id = tab_id.clone();
                workspace.active_surface_id = surface_id;

                return Ok((tab_id, new_session_id));
            }
            TabKind::Browser => {
                let browser_id = BrowserId(next_id("browser"));
                snapshot.browser_sessions.push(BrowserSessionSnapshot {
                    browser_id: browser_id.clone(),
                    title: "Browser".into(),
                    current_url: Some(DEFAULT_BROWSER_URL.into()),
                    history: vec![DEFAULT_BROWSER_URL.into()],
                    history_index: 0,
                    is_loading: false,
                    last_error: None,
                    agent_session_name: None,
                });

                let workspace = snapshot
                    .workspaces
                    .iter_mut()
                    .find(|w| w.workspace_id.0 == workspace_id)
                    .unwrap();

                workspace.tabs.push(TabSnapshot {
                    tab_id: tab_id.clone(),
                    kind: TabKind::Browser,
                    title: "Browser".into(),
                    surface_id: None,
                    browser_id: Some(browser_id),
                    icon: None,
                });
                workspace.active_tab_id = tab_id.clone();
            }
            TabKind::Diff => {
                workspace.tabs.push(TabSnapshot {
                    tab_id: tab_id.clone(),
                    kind: TabKind::Diff,
                    title: "Changes".into(),
                    surface_id: None,
                    browser_id: None,
                    icon: None,
                });
                workspace.active_tab_id = tab_id.clone();
            }
            TabKind::Editor => {
                workspace.tabs.push(TabSnapshot {
                    tab_id: tab_id.clone(),
                    kind: TabKind::Editor,
                    title: "Editor".into(),
                    surface_id: None,
                    browser_id: None,
                    icon: None,
                });
                workspace.active_tab_id = tab_id.clone();
            }
        }

        Ok((tab_id, new_session_id))
    }

    pub fn close_tab(&self, workspace_id: &str, tab_id: &str) -> Result<CloseTabResult, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

        let tab_index = workspace
            .tabs
            .iter()
            .position(|t| t.tab_id == tab_id)
            .ok_or_else(|| format!("No tab found for {tab_id}"))?;

        let tab = workspace.tabs.remove(tab_index);

        // Collect resources to clean up
        let mut removed_sessions: Vec<SessionId> = vec![];
        let mut removed_browser_ids: Vec<BrowserId> = vec![];
        let mut removed_agent_chat_threads: Vec<(crate::agent_provider::ProviderKind, String)> =
            vec![];

        // Tab-level browser ID (for dedicated browser tabs)
        if let Some(ref bid) = tab.browser_id {
            removed_browser_ids.push(bid.clone());
        }

        // If terminal tab, remove its surface and collect session + browser IDs
        // from the pane tree (browser panes are embedded in splits, not on the tab).
        if let Some(ref surface_id) = tab.surface_id {
            if let Some(surface_index) = workspace
                .surfaces
                .iter()
                .position(|s| s.surface_id == *surface_id)
            {
                let surface = workspace.surfaces.remove(surface_index);
                collect_session_ids_from_tree(&surface.root, &mut removed_sessions);
                collect_browser_ids_from_tree(&surface.root, &mut removed_browser_ids);
                collect_agent_chat_threads_from_tree(
                    &surface.root,
                    &mut removed_agent_chat_threads,
                );
            }
        }

        // If closed tab was active, activate adjacent tab (or clear if no tabs left)
        if workspace.active_tab_id == tab_id {
            if workspace.tabs.is_empty() {
                workspace.active_tab_id = String::new();
                workspace.active_surface_id = SurfaceId(String::new());
            } else {
                // Select the next tab — after the removal it slid into
                // tab_index — falling back to the previous tab when the closed
                // tab was last. Mirrors active_id_after_removal and standard
                // editor/browser tab behavior (was: always the previous tab).
                let new_index = if tab_index < workspace.tabs.len() {
                    tab_index
                } else {
                    tab_index - 1
                };
                let new_tab = &workspace.tabs[new_index];
                workspace.active_tab_id = new_tab.tab_id.clone();
                if let Some(ref sid) = new_tab.surface_id {
                    workspace.active_surface_id = sid.clone();
                }
            }
        }

        // Remove terminal sessions from snapshot
        for session_id in &removed_sessions {
            snapshot
                .terminal_sessions
                .retain(|s| s.session_id != *session_id);
        }

        // Remove browser sessions from snapshot
        for browser_id in &removed_browser_ids {
            snapshot
                .browser_sessions
                .retain(|b| b.browser_id != *browser_id);
        }

        Ok(CloseTabResult {
            removed_sessions,
            removed_browser_ids,
            removed_agent_chat_threads,
        })
    }

    pub fn activate_tab(&self, workspace_id: &str, tab_id: &str) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

        let tab = workspace
            .tabs
            .iter()
            .find(|t| t.tab_id == tab_id)
            .ok_or_else(|| format!("No tab found for {tab_id}"))?;

        workspace.active_tab_id = tab_id.to_string();
        if let Some(ref surface_id) = tab.surface_id {
            workspace.active_surface_id = surface_id.clone();

            // Clear "review" pane statuses for panes in this tab only
            if let Some(surface) = workspace
                .surfaces
                .iter()
                .find(|s| s.surface_id == *surface_id)
            {
                let pane_ids = collect_pane_ids_from_node(&surface.root);
                for pid in pane_ids {
                    if snapshot.pane_statuses.get(&pid) == Some(&PaneStatus::Review) {
                        snapshot.pane_statuses.remove(&pid);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn rename_tab(
        &self,
        workspace_id: &str,
        tab_id: &str,
        title: String,
    ) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

        let tab = workspace
            .tabs
            .iter_mut()
            .find(|t| t.tab_id == tab_id)
            .ok_or_else(|| format!("No tab found for {tab_id}"))?;

        tab.title = title;
        Ok(())
    }

    pub fn set_tab_icon(
        &self,
        workspace_id: &str,
        tab_id: &str,
        icon: Option<String>,
    ) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let workspace = snapshot
            .workspaces
            .iter_mut()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

        let tab = workspace
            .tabs
            .iter_mut()
            .find(|t| t.tab_id == tab_id)
            .ok_or_else(|| format!("No tab found for {tab_id}"))?;

        tab.icon = icon;
        Ok(())
    }

    /// Migrate workspaces loaded from disk that predate the tab system.
    /// Only applies to workspaces that have surfaces but no tabs (old format).
    /// Workspaces with 0 surfaces AND 0 tabs are intentionally empty.
    pub fn migrate_tabs_if_needed(&self) {
        let mut snapshot = self.inner.lock().unwrap();
        for workspace in &mut snapshot.workspaces {
            if workspace.workspace_type == WorkspaceType::Standard
                && workspace.tabs.is_empty()
                && !workspace.surfaces.is_empty()
            {
                let tab_id = next_id("tab");
                let surface_id = workspace
                    .surfaces
                    .first()
                    .map(|s| s.surface_id.clone());
                workspace.tabs.push(TabSnapshot {
                    tab_id: tab_id.clone(),
                    kind: TabKind::Terminal,
                    title: "Terminal".into(),
                    surface_id,
                    browser_id: None,
                    icon: None,
                });
                workspace.active_tab_id = tab_id;
            }
        }
    }

    /// Re-resolve project_root for all workspaces using find_git_root().
    /// Fixes old workspaces created before worktree-aware project detection.
    pub fn migrate_project_roots(&self) {
        let mut snapshot = self.inner.lock().unwrap();
        for workspace in &mut snapshot.workspaces {
            let path_str = workspace
                .project_root
                .as_deref()
                .unwrap_or(&workspace.cwd);
            let path = std::path::Path::new(path_str);
            if let Some(resolved) = crate::config::workspace_config::find_git_root(path) {
                let resolved_str = resolved.display().to_string();
                let current = workspace.project_root.as_deref().unwrap_or("");
                if resolved_str != current {
                    eprintln!(
                        "[migrate] project_root updated for {}: {} -> {}",
                        workspace.workspace_id.0, current, resolved_str
                    );
                    workspace.project_root = Some(resolved_str);
                }
            }
        }
    }

    /// Every agent-browser CLI session name that could belong to a live
    /// workspace: the stable cwd-derived name for every workspace, plus the
    /// `cli_session_name` of every tracked `agent_browser_sessions` entry.
    /// Consumed by the periodic orphan sweep (issue #126) — a tracked
    /// session survives the sweep as long as its name shows up here.
    pub fn live_agent_browser_session_names(&self) -> std::collections::HashSet<String> {
        let snapshot = self.inner.lock().unwrap();
        let mut names = std::collections::HashSet::new();
        for w in &snapshot.workspaces {
            names.insert(stable_browser_session_name(&w.cwd));
        }
        for s in &snapshot.agent_browser_sessions {
            names.insert(s.cli_session_name.clone());
        }
        names
    }
}

pub struct CloseTabResult {
    pub removed_sessions: Vec<SessionId>,
    pub removed_browser_ids: Vec<BrowserId>,
    pub removed_agent_chat_threads: Vec<(crate::agent_provider::ProviderKind, String)>,
}

/// Return value of `AppStateStore::close_workspace`. Carries both the fallback
/// workspace id the UI should activate next and the list of terminal sessions
/// whose PTY children the command layer still needs to kill.
///
/// Returning the session ids from the same lock acquisition that removes the
/// workspace closes the TOCTOU race a command-level snapshot would leave:
/// if a new pane were created between a pre-close snapshot and the state
/// mutation, the snapshotted id list would miss it and the PTY would leak.
pub struct CloseWorkspaceResult {
    pub fallback: WorkspaceId,
    pub removed_sessions: Vec<SessionId>,
    pub removed_agent_chat_threads: Vec<(crate::agent_provider::ProviderKind, String)>,
    /// `cli_session_name`s of the agent-browser sessions removed alongside
    /// this workspace (issue #126). Collected under the same lock
    /// acquisition that removes the workspace, so there is no TOCTOU race
    /// with a concurrent agent-browser session creation for this
    /// workspace: a session created after this snapshot simply won't
    /// belong to the (now-gone) workspace, and one created before it is
    /// guaranteed to be caught here rather than leaking as an orphaned
    /// daemon.
    pub removed_agent_browser_sessions: Vec<String>,
}

fn collect_session_ids_from_tree(node: &PaneNodeSnapshot, out: &mut Vec<SessionId>) {
    match node {
        PaneNodeSnapshot::Terminal { session_id, .. } => out.push(session_id.clone()),
        PaneNodeSnapshot::Browser { .. } | PaneNodeSnapshot::AgentChat { .. } => {}
        PaneNodeSnapshot::Split { children, .. } => {
            for child in children {
                collect_session_ids_from_tree(child, out);
            }
        }
    }
}

fn collect_agent_chat_threads_from_tree(
    node: &PaneNodeSnapshot,
    out: &mut Vec<(crate::agent_provider::ProviderKind, String)>,
) {
    match node {
        PaneNodeSnapshot::AgentChat {
            thread_id: Some(thread),
            provider: Some(kind),
            ..
        } => out.push((*kind, thread.clone())),
        PaneNodeSnapshot::AgentChat { .. }
        | PaneNodeSnapshot::Terminal { .. }
        | PaneNodeSnapshot::Browser { .. } => {}
        PaneNodeSnapshot::Split { children, .. } => {
            for child in children {
                collect_agent_chat_threads_from_tree(child, out);
            }
        }
    }
}

fn collect_browser_ids_from_tree(node: &PaneNodeSnapshot, out: &mut Vec<BrowserId>) {
    match node {
        PaneNodeSnapshot::Browser { browser_id, .. } => out.push(browser_id.clone()),
        PaneNodeSnapshot::Terminal { .. } | PaneNodeSnapshot::AgentChat { .. } => {}
        PaneNodeSnapshot::Split { children, .. } => {
            for child in children {
                collect_browser_ids_from_tree(child, out);
            }
        }
    }
}

pub fn emit_app_state<R: tauri::Runtime>(app: &AppHandle<R>) {
    let state: State<'_, AppStateStore> = app.state();
    let snapshot = state.snapshot();
    if let Err(error) = app.emit("app-state-changed", &snapshot) {
        eprintln!("[codemux::state] Failed to emit app state: {error}");
    }
    // Persist asynchronously with debounce — rapid consecutive calls (e.g.
    // swap + multiple resize events) collapse into a single disk write.
    persist_debouncer().schedule(snapshot);
}

/// Coalesce many emits into one. The first call schedules a worker
/// thread that sleeps `EMIT_COALESCE_MS` then invokes
/// `emit_app_state` with the *current* AppStateStore — so any state
/// mutations that happen during the window are picked up by the
/// single eventual emit. Subsequent calls during the window are
/// no-ops at the emit boundary; the snapshot is read fresh from the
/// store at flush time, not from any cached value.
///
/// Use this for high-frequency background sources (5 s git poll, PR /
/// port refresh, agent runtime hooks, control protocol bookkeeping)
/// where multiple emits can pile up within a single frame and the
/// frontend would otherwise pay the JSON-serialise cost N times for
/// what coalesces into one re-render anyway (the renderer already
/// debounces 16 ms in `use-app-state.ts`).
///
/// Do NOT use for user-action paths (split, swap, activate, tab
/// switch, command palette) — those should fire `emit_app_state`
/// directly so the UI updates without any added latency.
///
/// Mirrors the `PersistDebouncer` shape but with a 16 ms quiet
/// window (one frame) and an emit instead of a disk write. Stores
/// the latest AppHandle clone so the worker can call back into the
/// Tauri runtime once the timer elapses.
pub fn schedule_emit_app_state<R: tauri::Runtime>(app: &AppHandle<R>) {
    emit_debouncer().schedule(app);
}

const EMIT_COALESCE_MS: u64 = 16;

/// The process-wide [`EmitDebouncer`] lives in a `static`, which cannot be
/// generic over the Tauri runtime `R`. To stay runtime-agnostic we type-erase
/// the stashed handle into a boxed `emit` closure that captures the concrete
/// `AppHandle<R>` — a single runtime is live per process, so the closure is
/// monomorphised at the (generic) `schedule` call site and the static stays
/// non-generic.
type PendingEmit = Box<dyn Fn() + Send + 'static>;

struct EmitDebouncer {
    pending: Arc<AtomicBool>,
    app: Arc<Mutex<Option<PendingEmit>>>,
}

impl EmitDebouncer {
    fn new() -> Self {
        Self {
            pending: Arc::new(AtomicBool::new(false)),
            app: Arc::new(Mutex::new(None)),
        }
    }

    /// Stash the AppHandle (always overwriting — the latest clone is
    /// what the worker will use) and, if no worker is in flight,
    /// spawn one that sleeps then emits.
    fn schedule<R: tauri::Runtime>(&self, app: &AppHandle<R>) {
        {
            let app = app.clone();
            let mut guard = self.app.lock().unwrap();
            *guard = Some(Box::new(move || emit_app_state(&app)));
        }

        // If a worker is already counting down, nothing more to do.
        // It will pick up the latest `app` reference at flush time.
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }

        let pending = Arc::clone(&self.pending);
        let app_slot = Arc::clone(&self.app);

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(EMIT_COALESCE_MS));

            // Take the AppHandle and clear the pending flag while
            // still holding the mutex. This ensures no second worker
            // can slip through the pending.swap guard between the
            // flag clear and the emit. Any schedule() arriving AFTER
            // this point spawns a fresh worker for the next window.
            let emit = {
                let mut guard = app_slot.lock().unwrap();
                pending.store(false, Ordering::Release);
                guard.take()
            };

            if let Some(emit) = emit {
                emit();
            }
        });
    }
}

static EMIT_DEBOUNCER: std::sync::OnceLock<EmitDebouncer> = std::sync::OnceLock::new();

fn emit_debouncer() -> &'static EmitDebouncer {
    EMIT_DEBOUNCER.get_or_init(EmitDebouncer::new)
}

pub fn load_persisted_state() -> Option<AppStateSnapshot> {
    let path = persisted_layout_path()?;
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Remove all browser panes and browser sessions from a snapshot.
/// Browser panes are live streaming connections — there is nothing to restore
/// after restart. Stale browser panes cause white-screen bugs because they
/// start a daemon with the wrong session name.
pub fn strip_browser_panes_from_snapshot(mut snapshot: AppStateSnapshot) -> AppStateSnapshot {
    snapshot.browser_sessions.clear();

    for workspace in &mut snapshot.workspaces {
        // Remove browser pane nodes from all surface trees.
        for surface in &mut workspace.surfaces {
            if let Some(cleaned) = remove_browser_nodes(&surface.root) {
                surface.root = cleaned;
            }
            // else: entire tree was browser-only — leave it as-is;
            // workspace startup will recreate the surface layout.

            // If active pane was a browser, reset to first remaining pane.
            if !pane_exists_in_node(&surface.root, &surface.active_pane_id.0) {
                surface.active_pane_id = pane_id_from_node(&surface.root);
            }
        }

        // Remove browser tabs and their orphaned surfaces.
        let browser_surface_ids: std::collections::HashSet<String> = workspace
            .tabs
            .iter()
            .filter(|t| t.kind == TabKind::Browser)
            .filter_map(|t| t.surface_id.as_ref().map(|s| s.0.clone()))
            .collect();
        workspace.tabs.retain(|t| t.kind != TabKind::Browser);
        workspace
            .surfaces
            .retain(|s| !browser_surface_ids.contains(&s.surface_id.0));

        // If active tab was removed, fall back to the first remaining tab.
        if !workspace.tabs.iter().any(|t| t.tab_id == workspace.active_tab_id) {
            if let Some(first) = workspace.tabs.first() {
                workspace.active_tab_id = first.tab_id.clone();
                if let Some(sid) = &first.surface_id {
                    workspace.active_surface_id = sid.clone();
                }
            }
        }
    }

    snapshot
}

/// Recursively remove Browser leaf nodes from a pane tree, collapsing splits.
/// Returns None when the entire subtree consisted only of browser panes.
fn remove_browser_nodes(node: &PaneNodeSnapshot) -> Option<PaneNodeSnapshot> {
    match node {
        PaneNodeSnapshot::Browser { .. } => None,
        PaneNodeSnapshot::Terminal { .. } | PaneNodeSnapshot::AgentChat { .. } => {
            Some(node.clone())
        }
        PaneNodeSnapshot::Split {
            pane_id,
            direction,
            child_sizes,
            children,
        } => {
            let mut remaining: Vec<PaneNodeSnapshot> = Vec::new();
            let mut kept: Vec<usize> = Vec::new();
            for (i, child) in children.iter().enumerate() {
                if let Some(new_child) = remove_browser_nodes(child) {
                    remaining.push(new_child);
                    kept.push(i);
                }
            }
            match remaining.len() {
                0 => None,
                1 => remaining.into_iter().next(),
                _ => Some(PaneNodeSnapshot::Split {
                    pane_id: pane_id.clone(),
                    direction: direction.clone(),
                    child_sizes: retained_sizes(child_sizes, &kept),
                    children: remaining,
                }),
            }
        }
    }
}

/// Locate an `AgentChat` leaf by pane id and return its bound thread
/// id (inner `Option<String>` is the field itself; the outer `Option`
/// distinguishes "not found" from "found but unbound").
fn agent_chat_thread_for_pane(
    root: &PaneNodeSnapshot,
    target_pane_id: &str,
) -> Option<Option<String>> {
    match root {
        PaneNodeSnapshot::AgentChat {
            pane_id,
            thread_id,
            ..
        } if pane_id.0 == target_pane_id => Some(thread_id.clone()),
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .find_map(|child| agent_chat_thread_for_pane(child, target_pane_id)),
        _ => None,
    }
}

/// Locate an `AgentChat` leaf with an active session and return its
/// `(provider, thread_id)` pair. `None` when the pane id does not match,
/// is not an agent-chat leaf, or has not yet been bound to a thread —
/// callers (workspace/tab/pane close) treat that as nothing to tear down.
fn agent_chat_thread_pair_for_pane(
    root: &PaneNodeSnapshot,
    target_pane_id: &str,
) -> Option<(crate::agent_provider::ProviderKind, String)> {
    match root {
        PaneNodeSnapshot::AgentChat {
            pane_id,
            thread_id: Some(thread),
            provider: Some(kind),
            ..
        } if pane_id.0 == target_pane_id => Some((*kind, thread.clone())),
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .find_map(|child| agent_chat_thread_pair_for_pane(child, target_pane_id)),
        _ => None,
    }
}

/// Assign (or clear) the `thread_id` on an `AgentChat` leaf.
fn assign_agent_chat_thread(
    root: &mut PaneNodeSnapshot,
    target_pane_id: &str,
    new_thread_id: &Option<String>,
) -> bool {
    match root {
        PaneNodeSnapshot::AgentChat {
            pane_id,
            thread_id,
            ..
        } if pane_id.0 == target_pane_id => {
            *thread_id = new_thread_id.clone();
            true
        }
        PaneNodeSnapshot::Split { children, .. } => children
            .iter_mut()
            .any(|child| assign_agent_chat_thread(child, target_pane_id, new_thread_id)),
        _ => false,
    }
}

/// Assign the live provider and thread id as one pane-tree mutation.
fn assign_agent_chat_binding(
    root: &mut PaneNodeSnapshot,
    target_pane_id: &str,
    new_provider: crate::agent_provider::ProviderKind,
    new_thread_id: &str,
) -> bool {
    match root {
        PaneNodeSnapshot::AgentChat {
            pane_id,
            provider,
            thread_id,
            ..
        } if pane_id.0 == target_pane_id => {
            *provider = Some(new_provider);
            *thread_id = Some(new_thread_id.to_string());
            true
        }
        PaneNodeSnapshot::Split { children, .. } => children.iter_mut().any(|child| {
            assign_agent_chat_binding(child, target_pane_id, new_provider, new_thread_id)
        }),
        _ => false,
    }
}

fn pane_exists_in_node(node: &PaneNodeSnapshot, pane_id: &str) -> bool {
    match node {
        PaneNodeSnapshot::Terminal { pane_id: pid, .. }
        | PaneNodeSnapshot::Browser { pane_id: pid, .. }
        | PaneNodeSnapshot::AgentChat { pane_id: pid, .. } => pid.0 == pane_id,
        PaneNodeSnapshot::Split { children, .. } => {
            children.iter().any(|c| pane_exists_in_node(c, pane_id))
        }
    }
}

/// Remove OpenFlow workspaces and their terminal sessions from a snapshot.
/// Used on startup so persisted agent sessions from crashed runs are not respawned.
pub fn strip_openflow_from_snapshot(mut snapshot: AppStateSnapshot) -> AppStateSnapshot {
    let openflow_session_ids: std::collections::HashSet<String> = snapshot
        .workspaces
        .iter()
        .filter(|w| w.workspace_type == WorkspaceType::OpenFlow)
        .flat_map(|w| collect_terminal_sessions(&w.surfaces))
        .collect();

    let removed_workspace_ids: std::collections::HashSet<String> = snapshot
        .workspaces
        .iter()
        .filter(|w| w.workspace_type == WorkspaceType::OpenFlow)
        .map(|w| w.workspace_id.0.clone())
        .collect();

    snapshot
        .terminal_sessions
        .retain(|s| !openflow_session_ids.contains(s.session_id.0.as_str()));
    snapshot
        .workspaces
        .retain(|w| w.workspace_type != WorkspaceType::OpenFlow);

    if removed_workspace_ids.contains(&snapshot.active_workspace_id.0) {
        snapshot.active_workspace_id = snapshot
            .workspaces
            .first()
            .map(|w| w.workspace_id.clone())
            .unwrap_or_else(|| WorkspaceId(String::new()));
    }

    snapshot
}

pub fn restore_session_ids(snapshot: &AppStateSnapshot) {
    let max_id = snapshot
        .workspaces
        .iter()
        .flat_map(|workspace| {
            let mut ids = vec![extract_numeric_suffix(&workspace.workspace_id.0)];
            ids.extend(workspace.surfaces.iter().flat_map(|surface| {
                let mut surface_ids = vec![
                    extract_numeric_suffix(&surface.surface_id.0),
                    extract_numeric_suffix(&surface.active_pane_id.0),
                ];
                surface_ids.extend(collect_numeric_ids_from_node(&surface.root));
                surface_ids
            }));
            ids.extend(workspace.tabs.iter().flat_map(|tab| {
                let mut tab_ids = vec![extract_numeric_suffix(&tab.tab_id)];
                if let Some(ref sid) = tab.surface_id {
                    tab_ids.push(extract_numeric_suffix(&sid.0));
                }
                if let Some(ref bid) = tab.browser_id {
                    tab_ids.push(extract_numeric_suffix(&bid.0));
                }
                tab_ids
            }));
            ids
        })
        .chain(
            snapshot
                .terminal_sessions
                .iter()
                .map(|session| extract_numeric_suffix(&session.session_id.0)),
        )
        .chain(
            snapshot
                .browser_sessions
                .iter()
                .map(|session| extract_numeric_suffix(&session.browser_id.0)),
        )
        .chain(
            snapshot
                .agent_browser_sessions
                .iter()
                .map(|session| extract_numeric_suffix(&session.session_id.as_str())),
        )
        .flatten()
        .max()
        .unwrap_or(0);

    ID_COUNTER.store(max_id + 1, Ordering::Relaxed);
}

fn default_app_state() -> AppStateSnapshot {
    let workspace_id = WorkspaceId(next_id("workspace"));
    let surface_id = SurfaceId(next_id("surface"));
    let pane_id = PaneId(next_id("pane"));
    let session_id = SessionId(next_id("session"));
    let default_tab_id = next_id("tab");
    let cwd = current_project_root().display().to_string();
    let shell = env::var("SHELL").ok();

    AppStateSnapshot {
        schema_version: APP_STATE_SCHEMA_VERSION,
        archived_workspaces: Vec::new(),
        active_workspace_id: workspace_id.clone(),
        workspaces: vec![WorkspaceSnapshot {
            workspace_id,
            is_git: true,
            title: "Workspace 1".into(),
            workspace_type: WorkspaceType::Standard,
            cwd: cwd.clone(),
            git_branch: None,
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            worktree_path: None,
            project_root: None,
            project_uid: None,
            workspace_kind: None,
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            notifications_muted: false,
            notification_count: 0,
            latest_agent_state: Some("idle".into()),
            tabs: vec![TabSnapshot {
                tab_id: default_tab_id.clone(),
                kind: TabKind::Terminal,
                title: "Terminal".into(),
                surface_id: Some(surface_id.clone()),
                browser_id: None,
                icon: None,
            }],
            active_tab_id: default_tab_id,
            active_surface_id: surface_id.clone(),
            surfaces: vec![SurfaceSnapshot {
                surface_id,
                title: "Main Surface".into(),
                active_pane_id: pane_id.clone(),
                root: PaneNodeSnapshot::Terminal {
                    pane_id,
                    session_id: session_id.clone(),
                    title: "Terminal".into(),
                },
            }],
            host_id: None,
            remote_cwd: None,
            attach_only: false,
            last_active_at: Some(current_time_ms_signed()),
            last_visited_at: Some(current_time_ms_signed()),
        }],
        terminal_sessions: vec![TerminalSessionSnapshot {
            session_id,
            title: "Terminal 1".into(),
            shell,
            cwd,
            cols: 80,
            rows: 24,
            state: TerminalSessionState::Starting,
            last_message: Some("Preparing shell session".into()),
            exit_code: None,
            original_command: None,
            adapter_captures: Default::default(),
        }],
        browser_sessions: vec![],
        agent_browser_sessions: vec![],
        notifications: vec![],
        detected_ports: vec![],
        pane_statuses: HashMap::new(),
        persistence: PersistenceSchema {
            schema_version: PERSISTENCE_SCHEMA_VERSION,
            stores_layout_metadata: true,
            stores_terminal_metadata: true,
            stores_live_process_state: false,
        },
        config: CodemuxConfigSnapshot {
            config_version: CODEMUX_CONFIG_VERSION,
            default_shell: env::var("SHELL").ok(),
            theme_source: "omarchy_or_default".into(),
            linux_first: true,
            notification_sound_enabled: true,
            ai_commit_message_enabled: true,
            ai_commit_message_cli: None,
            ai_commit_message_model: None,
            ai_resolver_enabled: false,
            ai_resolver_cli: None,
            ai_resolver_model: None,
            ai_resolver_strategy: default_resolver_strategy(),
        },
    }
}

pub fn session_id_for_pane(root: &PaneNodeSnapshot, target_pane_id: &PaneId) -> Option<SessionId> {
    match root {
        PaneNodeSnapshot::Terminal {
            pane_id,
            session_id,
            ..
        } if pane_id == target_pane_id => Some(session_id.clone()),
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .find_map(|child| session_id_for_pane(child, target_pane_id)),
        _ => None,
    }
}

fn browser_id_for_pane(root: &PaneNodeSnapshot, target_pane_id: &PaneId) -> Option<BrowserId> {
    match root {
        PaneNodeSnapshot::Browser {
            pane_id,
            browser_id,
            ..
        } if pane_id == target_pane_id => Some(browser_id.clone()),
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .find_map(|child| browser_id_for_pane(child, target_pane_id)),
        _ => None,
    }
}

fn pane_tree_contains_session(root: &PaneNodeSnapshot, target_session_id: &str) -> bool {
    match root {
        PaneNodeSnapshot::Terminal { session_id, .. } => session_id.0 == target_session_id,
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .any(|child| pane_tree_contains_session(child, target_session_id)),
        PaneNodeSnapshot::Browser { .. } | PaneNodeSnapshot::AgentChat { .. } => false,
    }
}

fn remove_terminal_from_tree(
    root: &PaneNodeSnapshot,
    target_session_id: &str,
) -> Option<PaneNodeSnapshot> {
    match root {
        PaneNodeSnapshot::Terminal { session_id, .. } if session_id.0 == target_session_id => None,
        PaneNodeSnapshot::Terminal { .. }
        | PaneNodeSnapshot::Browser { .. }
        | PaneNodeSnapshot::AgentChat { .. } => Some(root.clone()),
        PaneNodeSnapshot::Split {
            pane_id,
            direction,
            child_sizes,
            children,
        } => {
            let mut remaining_children: Vec<PaneNodeSnapshot> = Vec::new();
            let mut kept: Vec<usize> = Vec::new();
            for (i, child) in children.iter().enumerate() {
                if let Some(new_child) = remove_terminal_from_tree(child, target_session_id) {
                    remaining_children.push(new_child);
                    kept.push(i);
                }
            }

            match remaining_children.len() {
                0 => None,
                1 => remaining_children.into_iter().next(),
                _ => Some(PaneNodeSnapshot::Split {
                    pane_id: pane_id.clone(),
                    direction: direction.clone(),
                    child_sizes: retained_sizes(child_sizes, &kept),
                    children: remaining_children,
                }),
            }
        }
    }
}

/// Remove multiple terminal sessions from the pane tree in one pass.
fn remove_terminals_from_tree(
    root: &PaneNodeSnapshot,
    session_ids: &std::collections::HashSet<&str>,
) -> Option<PaneNodeSnapshot> {
    match root {
        PaneNodeSnapshot::Terminal { session_id, .. } if session_ids.contains(session_id.0.as_str()) => {
            None
        }
        PaneNodeSnapshot::Terminal { .. }
        | PaneNodeSnapshot::Browser { .. }
        | PaneNodeSnapshot::AgentChat { .. } => Some(root.clone()),
        PaneNodeSnapshot::Split {
            pane_id,
            direction,
            child_sizes,
            children,
        } => {
            let mut remaining_children: Vec<PaneNodeSnapshot> = Vec::new();
            let mut kept: Vec<usize> = Vec::new();
            for (i, child) in children.iter().enumerate() {
                if let Some(new_child) = remove_terminals_from_tree(child, session_ids) {
                    remaining_children.push(new_child);
                    kept.push(i);
                }
            }

            match remaining_children.len() {
                0 => None,
                1 => remaining_children.into_iter().next(),
                _ => Some(PaneNodeSnapshot::Split {
                    pane_id: pane_id.clone(),
                    direction: direction.clone(),
                    child_sizes: retained_sizes(child_sizes, &kept),
                    children: remaining_children,
                }),
            }
        }
    }
}

fn first_terminal_pane(root: &PaneNodeSnapshot) -> Option<(PaneId, SessionId)> {
    match root {
        PaneNodeSnapshot::Terminal {
            pane_id,
            session_id,
            ..
        } => Some((pane_id.clone(), session_id.clone())),
        PaneNodeSnapshot::Split { children, .. } => children.iter().find_map(first_terminal_pane),
        PaneNodeSnapshot::Browser { .. } | PaneNodeSnapshot::AgentChat { .. } => None,
    }
}

fn collect_pane_ids_from_node(node: &PaneNodeSnapshot) -> Vec<String> {
    match node {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. } => vec![pane_id.0.clone()],
        PaneNodeSnapshot::Split { children, .. } => {
            children.iter().flat_map(collect_pane_ids_from_node).collect()
        }
    }
}

pub fn find_terminal_pane_id(root: &PaneNodeSnapshot, target_session_id: &str) -> Option<PaneId> {
    match root {
        PaneNodeSnapshot::Terminal {
            pane_id,
            session_id,
            ..
        } if session_id.0 == target_session_id => Some(pane_id.clone()),
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .find_map(|child| find_terminal_pane_id(child, target_session_id)),
        _ => None,
    }
}

/// Resolve a chat `thread_id` to the `AgentChat` pane bound to it, if any.
/// The chat-pane counterpart to [`find_terminal_pane_id`]: the terminal
/// walker matches on the PTY `session_id` and skips `AgentChat` nodes, so
/// chat status publishing needs this thread-keyed resolver instead. Only
/// panes that have already been bound to a session (`thread_id.is_some()`)
/// can match.
pub fn find_agent_chat_pane_id(root: &PaneNodeSnapshot, target_thread_id: &str) -> Option<PaneId> {
    match root {
        PaneNodeSnapshot::AgentChat {
            pane_id,
            thread_id: Some(thread_id),
            ..
        } if thread_id == target_thread_id => Some(pane_id.clone()),
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .find_map(|child| find_agent_chat_pane_id(child, target_thread_id)),
        _ => None,
    }
}

pub fn collect_terminal_sessions(surfaces: &[SurfaceSnapshot]) -> Vec<String> {
    surfaces
        .iter()
        .flat_map(|surface| collect_terminal_sessions_from_node(&surface.root))
        .collect()
}

/// Walk every surface in a workspace and pull out `(provider, thread_id)`
/// pairs for `AgentChat` panes that have an active session bound. Used by
/// the workspace/tab close paths to feed `provider.stop_session` so the
/// JSON-RPC sidecar process and its background tokio tasks tear down
/// instead of leaking — the tasks keep `Arc<Session>` alive in a refcycle
/// with the session's `JoinHandle` vec, so `Drop` never fires unless
/// `stop_session` aborts them via the shutdown channel.
pub fn collect_agent_chat_threads(
    surfaces: &[SurfaceSnapshot],
) -> Vec<(crate::agent_provider::ProviderKind, String)> {
    let mut out = Vec::new();
    for surface in surfaces {
        collect_agent_chat_threads_from_tree(&surface.root, &mut out);
    }
    out
}

fn collect_terminal_sessions_from_node(root: &PaneNodeSnapshot) -> Vec<String> {
    match root {
        PaneNodeSnapshot::Terminal { session_id, .. } => vec![session_id.0.clone()],
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .flat_map(collect_terminal_sessions_from_node)
            .collect(),
        PaneNodeSnapshot::Browser { .. } | PaneNodeSnapshot::AgentChat { .. } => vec![],
    }
}

fn rebalance_sizes(existing: &[f32], target_len: usize) -> Vec<f32> {
    if target_len == 0 {
        return vec![];
    }

    if existing.len() == target_len {
        return normalize_sizes(existing.to_vec());
    }

    normalize_sizes(vec![1.0 / target_len as f32; target_len])
}

/// Sizes for the children that survived a removal, renormalized so their
/// relative proportions are preserved. `kept` holds the original indices of
/// the surviving children. Unlike resetting to equal weights, closing one
/// pane in a custom-sized split leaves the remaining panes' ratios intact
/// (e.g. closing the third child of a `[0.5, 0.3, 0.2]` split yields
/// `[0.625, 0.375]`, not `[0.5, 0.5]`).
fn retained_sizes(child_sizes: &[f32], kept: &[usize]) -> Vec<f32> {
    let fallback = if child_sizes.is_empty() {
        0.0
    } else {
        1.0 / child_sizes.len() as f32
    };
    let sizes: Vec<f32> = kept
        .iter()
        .map(|&i| child_sizes.get(i).copied().unwrap_or(fallback))
        .collect();
    normalize_sizes(sizes)
}

fn normalize_sizes(mut sizes: Vec<f32>) -> Vec<f32> {
    if sizes.is_empty() {
        return sizes;
    }

    let min_size = 0.1f32;
    for size in &mut sizes {
        *size = size.max(min_size);
    }

    let total: f32 = sizes.iter().sum();
    if total <= f32::EPSILON {
        return vec![1.0 / sizes.len() as f32; sizes.len()];
    }

    sizes.into_iter().map(|size| size / total).collect()
}

fn persisted_layout_path() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    Some(base.join(crate::APP_DIR_NAME).join("layout.json"))
}

/// Drop the pane statuses that mean nothing after a restart, keeping the ones
/// that do.
///
/// `Working` and `Permission` describe a live process. The agent is dead once
/// the app exits, so restoring a spinner or a permission badge would show the
/// user a state that no process backs — the reason this whole map used to be
/// cleared wholesale.
///
/// `Review` is a different kind of fact: it means "this agent finished and you
/// have not looked at the result yet", which is just as true after a restart as
/// it was before. Persisting it is what keeps the sidebar's done-checkmark
/// alive across a quit; clearing it made every finished workspace come back
/// looking like the user had already reviewed it.
///
/// Entries whose pane no longer exists are dropped too. Nothing removes a
/// status when its pane goes away (`close_pane` leaves the key behind, and the
/// openflow/browser strips above delete panes wholesale), so without this the
/// map would grow forever now that it survives restarts.
fn retain_persistable_pane_statuses(snapshot: &mut AppStateSnapshot) {
    let live_panes: std::collections::HashSet<String> = snapshot
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.surfaces.iter())
        .flat_map(|surface| collect_pane_ids_from_node(&surface.root))
        .collect();

    snapshot
        .pane_statuses
        .retain(|pane_id, status| *status == PaneStatus::Review && live_panes.contains(pane_id));
}

fn save_persisted_state(snapshot: &AppStateSnapshot) -> Result<(), String> {
    let Some(path) = persisted_layout_path() else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create config dir: {error}"))?;
    }

    // Never persist OpenFlow workspaces or their terminal sessions so they cannot accumulate.
    let mut snapshot = strip_openflow_from_snapshot(snapshot.clone());
    // Browser panes are live streaming connections — stale panes start a
    // daemon with the wrong session name, causing white-screen bugs.
    snapshot = strip_browser_panes_from_snapshot(snapshot);
    // Detected ports are runtime-only state — never persist them.
    snapshot.detected_ports.clear();
    // Keep only the review checkmarks; working/permission are runtime-only.
    retain_persistable_pane_statuses(&mut snapshot);
    // Agent browser sessions are runtime-only — pane_id/browser_id/user_dismissed
    // become stale after restart and block auto-creation.
    snapshot.agent_browser_sessions.clear();

    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|error| format!("Failed to serialize layout state: {error}"))?;
    fs::write(path, json).map_err(|error| format!("Failed to write layout state: {error}"))
}

fn find_workspace_id_for_session(
    workspaces: &[WorkspaceSnapshot],
    target_session_id: &str,
) -> Option<WorkspaceId> {
    workspaces.iter().find_map(|workspace| {
        let found = workspace
            .surfaces
            .iter()
            .any(|surface| find_terminal_pane_id(&surface.root, target_session_id).is_some());

        if found {
            Some(workspace.workspace_id.clone())
        } else {
            None
        }
    })
}

fn find_workspace_id_for_pane(
    workspaces: &[WorkspaceSnapshot],
    target_pane_id: &str,
) -> Option<WorkspaceId> {
    workspaces.iter().find_map(|workspace| {
        let found = workspace
            .surfaces
            .iter()
            .any(|surface| pane_tree_contains_pane(&surface.root, target_pane_id));

        if found {
            Some(workspace.workspace_id.clone())
        } else {
            None
        }
    })
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Signed ms epoch for the workspace activity stamps. They are `i64`
/// rather than `u64` because the boot backfill derives them from git
/// commit times, which git itself reports as signed seconds.
fn current_time_ms_signed() -> i64 {
    current_time_ms() as i64
}

/// Record that an agent just did real work in `workspace_id`.
///
/// Kept as a free function taking `&mut AppStateSnapshot` so the pane
/// status writers can call it while already holding the state lock —
/// re-entering the mutex from a `&self` method would deadlock.
fn stamp_workspace_activity(snapshot: &mut AppStateSnapshot, workspace_id: &WorkspaceId) {
    if let Some(workspace) = snapshot
        .workspaces
        .iter_mut()
        .find(|workspace| workspace.workspace_id == *workspace_id)
    {
        workspace.last_active_at = Some(current_time_ms_signed());
    }
}

/// Best-effort "when did anything last happen in this checkout", used
/// only to seed `last_active_at` for workspaces persisted before that
/// field existed.
///
/// Prefers the checkout's last commit (git's own record of real work,
/// and stable across clones/reinstalls) and falls back to the
/// directory's mtime. Returns `None` — never `now` — when the path is
/// gone, is not a repo, or git fails: a fabricated "just active" stamp
/// would quietly exempt genuinely stale work from the idle sweep
/// forever, whereas `None` reads as "unknown" and simply declines to
/// sweep.
fn derive_last_activity_ms(path: &Path) -> Option<i64> {
    if !path.is_dir() {
        return None;
    }
    last_commit_ms(path).or_else(|| directory_mtime_ms(path))
}

/// Commit time of `HEAD` in ms. A non-repo, an unborn `HEAD`, or a
/// missing git binary all surface as `None` rather than an error,
/// because the only caller is a best-effort backfill that must never
/// take the app down with it.
fn last_commit_ms(path: &Path) -> Option<i64> {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%ct"])
        .current_dir(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let seconds: i64 = String::from_utf8_lossy(&output.stdout).trim().parse().ok()?;
    seconds.checked_mul(1000)
}

fn directory_mtime_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let millis = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(millis).ok()
}

fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    // Don't prepend https:// if the URL already has any scheme
    if trimmed.contains("://") || trimmed.starts_with("data:") || trimmed.starts_with("about:") {
        return trimmed.to_string();
    }
    format!("https://{trimmed}")
}

fn layout_shell_count(layout: &WorkspacePresetLayout) -> usize {
    match layout {
        WorkspacePresetLayout::Single => 1,
        WorkspacePresetLayout::Pair => 2,
        WorkspacePresetLayout::Quad => 4,
        WorkspacePresetLayout::Six => 6,
        WorkspacePresetLayout::Eight => 8,
        WorkspacePresetLayout::ShellBrowser => 1,
        WorkspacePresetLayout::Empty => 0,
    }
}

fn build_workspace_layout(
    layout: &WorkspacePresetLayout,
    session_ids: &[SessionId],
    browser_id: Option<&BrowserId>,
) -> PaneNodeSnapshot {
    match layout {
        WorkspacePresetLayout::Single => terminal_node(&session_ids[0], 0),
        WorkspacePresetLayout::Pair => split_node(
            SplitDirection::Horizontal,
            vec![
                terminal_node(&session_ids[0], 0),
                terminal_node(&session_ids[1], 1),
            ],
        ),
        WorkspacePresetLayout::Quad => split_node(
            SplitDirection::Vertical,
            vec![
                split_node(
                    SplitDirection::Horizontal,
                    vec![
                        terminal_node(&session_ids[0], 0),
                        terminal_node(&session_ids[1], 1),
                    ],
                ),
                split_node(
                    SplitDirection::Horizontal,
                    vec![
                        terminal_node(&session_ids[2], 2),
                        terminal_node(&session_ids[3], 3),
                    ],
                ),
            ],
        ),
        WorkspacePresetLayout::Six => split_node(
            SplitDirection::Vertical,
            vec![
                split_node(
                    SplitDirection::Horizontal,
                    vec![
                        terminal_node(&session_ids[0], 0),
                        terminal_node(&session_ids[1], 1),
                        terminal_node(&session_ids[2], 2),
                    ],
                ),
                split_node(
                    SplitDirection::Horizontal,
                    vec![
                        terminal_node(&session_ids[3], 3),
                        terminal_node(&session_ids[4], 4),
                        terminal_node(&session_ids[5], 5),
                    ],
                ),
            ],
        ),
        WorkspacePresetLayout::Eight => split_node(
            SplitDirection::Vertical,
            vec![
                split_node(
                    SplitDirection::Horizontal,
                    vec![
                        terminal_node(&session_ids[0], 0),
                        terminal_node(&session_ids[1], 1),
                        terminal_node(&session_ids[2], 2),
                        terminal_node(&session_ids[3], 3),
                    ],
                ),
                split_node(
                    SplitDirection::Horizontal,
                    vec![
                        terminal_node(&session_ids[4], 4),
                        terminal_node(&session_ids[5], 5),
                        terminal_node(&session_ids[6], 6),
                        terminal_node(&session_ids[7], 7),
                    ],
                ),
            ],
        ),
        WorkspacePresetLayout::ShellBrowser => split_node(
            SplitDirection::Horizontal,
            vec![
                terminal_node(&session_ids[0], 0),
                browser_node(
                    browser_id.expect("browser layout requires browser session"),
                    0,
                ),
            ],
        ),
        WorkspacePresetLayout::Empty => {
            // `create_workspace_with_layout` short-circuits the Empty
            // variant before reaching this helper; if we ever land
            // here it means the short-circuit has been bypassed.
            unreachable!(
                "build_workspace_layout should not be called with Empty — \
                 create_workspace_with_layout short-circuits that variant \
                 to `create_empty_workspace_at_path`"
            )
        }
    }
}

fn terminal_node(session_id: &SessionId, index: usize) -> PaneNodeSnapshot {
    PaneNodeSnapshot::Terminal {
        pane_id: PaneId(next_id("pane")),
        session_id: session_id.clone(),
        title: format!("Terminal {}", index + 1),
    }
}

fn browser_node(browser_id: &BrowserId, index: usize) -> PaneNodeSnapshot {
    PaneNodeSnapshot::Browser {
        pane_id: PaneId(next_id("pane")),
        browser_id: browser_id.clone(),
        title: format!("Browser {}", index + 1),
    }
}

fn split_node(direction: SplitDirection, children: Vec<PaneNodeSnapshot>) -> PaneNodeSnapshot {
    PaneNodeSnapshot::Split {
        pane_id: PaneId(next_id("pane")),
        direction,
        child_sizes: normalize_sizes(vec![1.0 / children.len() as f32; children.len()]),
        children,
    }
}

fn rightmost_leaf_pane_id(root: &PaneNodeSnapshot) -> Option<PaneId> {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. } => Some(pane_id.clone()),
        PaneNodeSnapshot::Split { children, .. } => {
            children.last().and_then(rightmost_leaf_pane_id)
        }
    }
}

fn active_workspace_surface_indices(snapshot: &AppStateSnapshot) -> Option<(usize, usize)> {
    let workspace_index = snapshot
        .workspaces
        .iter()
        .position(|workspace| workspace.workspace_id == snapshot.active_workspace_id)?;
    let surface_index = snapshot
        .workspaces
        .get(workspace_index)?
        .surfaces
        .iter()
        .position(|surface| {
            surface.surface_id == snapshot.workspaces[workspace_index].active_surface_id
        })?;
    Some((workspace_index, surface_index))
}

fn update_split_sizes(
    root: &mut PaneNodeSnapshot,
    target_pane_id: &str,
    child_sizes: Vec<f32>,
) -> bool {
    match root {
        PaneNodeSnapshot::Split {
            pane_id,
            child_sizes: existing_sizes,
            children,
            ..
        } if pane_id.0 == target_pane_id => {
            if child_sizes.len() != children.len() {
                return false;
            }

            *existing_sizes = normalize_sizes(child_sizes);
            true
        }
        PaneNodeSnapshot::Split { children, .. } => children
            .iter_mut()
            .any(|child| update_split_sizes(child, target_pane_id, child_sizes.clone())),
        _ => false,
    }
}

fn nudge_active_pane_size(
    root: &mut PaneNodeSnapshot,
    active_pane_id: &PaneId,
    delta: f32,
) -> bool {
    match root {
        PaneNodeSnapshot::Split {
            child_sizes,
            children,
            ..
        } => {
            let active_child_index = children
                .iter()
                .position(|child| pane_contains_leaf(child, active_pane_id));

            if let Some(index) = active_child_index {
                if children.len() >= 2 {
                    let neighbor_index = if index == 0 { 1 } else { index - 1 };
                    let min_size = 0.1f32;
                    let new_current = (child_sizes[index] + delta).max(min_size);
                    let delta_applied = new_current - child_sizes[index];
                    let new_neighbor = (child_sizes[neighbor_index] - delta_applied).max(min_size);
                    let neighbor_adjustment = child_sizes[neighbor_index] - new_neighbor;

                    child_sizes[index] += neighbor_adjustment;
                    child_sizes[neighbor_index] = new_neighbor;
                    *child_sizes = normalize_sizes(child_sizes.clone());
                    return true;
                }

                for child in children.iter_mut() {
                    if nudge_active_pane_size(child, active_pane_id, delta) {
                        return true;
                    }
                }
            }

            children
                .iter_mut()
                .any(|child| nudge_active_pane_size(child, active_pane_id, delta))
        }
        _ => false,
    }
}

fn pane_contains_leaf(root: &PaneNodeSnapshot, target_pane_id: &PaneId) -> bool {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. } => pane_id == target_pane_id,
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .any(|child| pane_contains_leaf(child, target_pane_id)),
    }
}

fn pane_tree_contains_pane(root: &PaneNodeSnapshot, target_pane_id: &str) -> bool {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. } => pane_id.0 == target_pane_id,
        PaneNodeSnapshot::Split {
            pane_id, children, ..
        } => {
            pane_id.0 == target_pane_id
                || children
                    .iter()
                    .any(|child| pane_tree_contains_pane(child, target_pane_id))
        }
    }
}

fn collect_leaf_pane_ids(root: &PaneNodeSnapshot) -> Vec<PaneId> {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. } => vec![pane_id.clone()],
        PaneNodeSnapshot::Split { children, .. } => {
            children.iter().flat_map(collect_leaf_pane_ids).collect()
        }
    }
}

fn first_leaf_pane_id(root: &PaneNodeSnapshot) -> Option<PaneId> {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. } => Some(pane_id.clone()),
        PaneNodeSnapshot::Split { children, .. } => children.iter().find_map(first_leaf_pane_id),
    }
}

/// Pick the pane to focus after `removed` is closed. If a *different* pane was
/// closed, keep the current `active` pane; if the active pane itself was
/// closed, move to the next pane in leaf order, falling back to the previous
/// one — instead of jumping focus to the leftmost leaf. `ordered` is the leaf
/// order *before* removal (so `removed` is still present in it).
fn active_id_after_removal(ordered: &[PaneId], active: &PaneId, removed: &str) -> Option<PaneId> {
    if active.0 != removed {
        return Some(active.clone());
    }
    let idx = ordered.iter().position(|p| p.0 == removed)?;
    ordered
        .get(idx + 1)
        .cloned()
        .or_else(|| idx.checked_sub(1).and_then(|prev| ordered.get(prev).cloned()))
}

fn clone_pane_node(root: &PaneNodeSnapshot, target_pane_id: &str) -> Option<PaneNodeSnapshot> {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. }
            if pane_id.0 == target_pane_id =>
        {
            Some(root.clone())
        }
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .find_map(|child| clone_pane_node(child, target_pane_id)),
        _ => None,
    }
}

fn with_pane_id(node: PaneNodeSnapshot, pane_id: PaneId) -> PaneNodeSnapshot {
    match node {
        PaneNodeSnapshot::Terminal {
            session_id, title, ..
        } => PaneNodeSnapshot::Terminal {
            pane_id,
            session_id,
            title,
        },
        PaneNodeSnapshot::Browser {
            browser_id, title, ..
        } => PaneNodeSnapshot::Browser {
            pane_id,
            browser_id,
            title,
        },
        PaneNodeSnapshot::AgentChat {
            title,
            thread_id,
            provider,
            cwd,
            ..
        } => PaneNodeSnapshot::AgentChat {
            pane_id,
            title,
            thread_id,
            provider,
            cwd,
        },
        PaneNodeSnapshot::Split {
            direction,
            child_sizes,
            children,
            ..
        } => PaneNodeSnapshot::Split {
            pane_id,
            direction,
            child_sizes,
            children,
        },
    }
}

fn replace_pane_node(
    root: &mut PaneNodeSnapshot,
    target_pane_id: &str,
    replacement: PaneNodeSnapshot,
) -> bool {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. }
            if pane_id.0 == target_pane_id =>
        {
            *root = replacement;
            true
        }
        PaneNodeSnapshot::Split { children, .. } => children
            .iter_mut()
            .any(|child| replace_pane_node(child, target_pane_id, replacement.clone())),
        _ => false,
    }
}

fn pane_id_from_node(node: &PaneNodeSnapshot) -> PaneId {
    match node {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. } => pane_id.clone(),
        PaneNodeSnapshot::Split { pane_id, .. } => pane_id.clone(),
    }
}

fn find_pane_location(workspaces: &[WorkspaceSnapshot], pane_id: &str) -> Option<(usize, usize)> {
    for (workspace_index, workspace) in workspaces.iter().enumerate() {
        for (surface_index, surface) in workspace.surfaces.iter().enumerate() {
            if pane_tree_contains_pane(&surface.root, pane_id) {
                return Some((workspace_index, surface_index));
            }
        }
    }

    None
}

fn insert_split_at_pane(
    root: &mut PaneNodeSnapshot,
    target_pane_id: &str,
    split_pane_id: PaneId,
    direction: SplitDirection,
    new_node: PaneNodeSnapshot,
) -> bool {
    insert_split_at_pane_with_behavior(
        root,
        target_pane_id,
        split_pane_id,
        direction,
        new_node,
        WorkspaceInsertBehavior::Horizontal,
    )
}

fn insert_split_at_pane_with_behavior(
    root: &mut PaneNodeSnapshot,
    target_pane_id: &str,
    split_pane_id: PaneId,
    direction: SplitDirection,
    new_node: PaneNodeSnapshot,
    behavior: WorkspaceInsertBehavior,
) -> bool {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. }
            if pane_id.0 == target_pane_id =>
        {
            let previous = root.clone();
            *root = PaneNodeSnapshot::Split {
                pane_id: split_pane_id,
                direction,
                child_sizes: vec![0.5, 0.5],
                children: vec![previous, new_node],
            };
            true
        }
        PaneNodeSnapshot::Split {
            child_sizes,
            children,
            ..
        } => {
            if matches!(behavior, WorkspaceInsertBehavior::Smart)
                && direction == SplitDirection::Horizontal
            {
                if let Some((_, target_child)) = children
                    .iter_mut()
                    .enumerate()
                    .find(|(_, child)| pane_tree_contains_pane(child, target_pane_id))
                {
                    match target_child {
                        PaneNodeSnapshot::Terminal { .. }
                        | PaneNodeSnapshot::Browser { .. }
                        | PaneNodeSnapshot::AgentChat { .. } => {
                            let existing_child = target_child.clone();
                            let nested_split_id = PaneId(next_id("pane"));
                            *target_child = PaneNodeSnapshot::Split {
                                pane_id: nested_split_id,
                                direction: SplitDirection::Vertical,
                                child_sizes: vec![0.5, 0.5],
                                children: vec![existing_child, new_node],
                            };
                            return true;
                        }
                        PaneNodeSnapshot::Split {
                            direction: child_direction,
                            child_sizes: nested_sizes,
                            children: nested_children,
                            ..
                        } if *child_direction == SplitDirection::Vertical
                            && nested_children.len() < 2 =>
                        {
                            nested_children.push(new_node);
                            *nested_sizes = rebalance_sizes(nested_sizes, nested_children.len());
                            return true;
                        }
                        PaneNodeSnapshot::Split { .. } => {
                            if insert_split_at_pane_with_behavior(
                                target_child,
                                target_pane_id,
                                split_pane_id.clone(),
                                direction.clone(),
                                new_node.clone(),
                                behavior.clone(),
                            ) {
                                *child_sizes = rebalance_sizes(child_sizes, children.len());
                                return true;
                            }
                        }
                    }
                }
            }

            let inserted = children.iter_mut().any(|child| {
                insert_split_at_pane_with_behavior(
                    child,
                    target_pane_id,
                    split_pane_id.clone(),
                    direction.clone(),
                    new_node.clone(),
                    behavior.clone(),
                )
            });

            if inserted {
                *child_sizes = rebalance_sizes(child_sizes, children.len());
            }
            inserted
        }
        _ => false,
    }
}

fn remove_pane_from_tree(
    root: &PaneNodeSnapshot,
    target_pane_id: &str,
) -> Option<PaneNodeSnapshot> {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. }
        | PaneNodeSnapshot::Browser { pane_id, .. }
        | PaneNodeSnapshot::AgentChat { pane_id, .. }
            if pane_id.0 == target_pane_id =>
        {
            None
        }
        PaneNodeSnapshot::Terminal { .. }
        | PaneNodeSnapshot::Browser { .. }
        | PaneNodeSnapshot::AgentChat { .. } => Some(root.clone()),
        PaneNodeSnapshot::Split {
            pane_id,
            direction,
            child_sizes,
            children,
        } => {
            let mut remaining_children: Vec<PaneNodeSnapshot> = Vec::new();
            let mut kept: Vec<usize> = Vec::new();
            for (i, child) in children.iter().enumerate() {
                if let Some(new_child) = remove_pane_from_tree(child, target_pane_id) {
                    remaining_children.push(new_child);
                    kept.push(i);
                }
            }

            match remaining_children.len() {
                0 => None,
                1 => remaining_children.into_iter().next(),
                _ => Some(PaneNodeSnapshot::Split {
                    pane_id: pane_id.clone(),
                    direction: direction.clone(),
                    child_sizes: retained_sizes(child_sizes, &kept),
                    children: remaining_children,
                }),
            }
        }
    }
}

/// Build a stable browser session name from a workspace cwd so that
/// agent-browser's storage state (cookies, localStorage) persists across
/// app restarts. Uses the directory basename plus a short hash for uniqueness.
///
/// agent-browser turns the session name into a Unix domain socket path
/// (`<socket_dir>/<session>.sock`) and rejects it once that path exceeds
/// the platform `sun_path` limit (~103 bytes). For codemux-managed
/// worktrees the directory basename *is* the git branch name, which is
/// unbounded — a long branch like
/// `feature/add-integration-tests-for-the-browser-pane` overflows the
/// socket path and breaks `codemux browser` entirely for that workspace.
/// Cap the human-readable portion so the full session name stays well
/// under both that limit and agent-browser's own 1–64 char cap, while
/// the hash (computed over the *full* cwd) keeps it stable and unique.
fn stable_browser_session_name(cwd: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    /// Max bytes of branch/dir name kept in the session name. With the
    /// `ws-` prefix (3) and `-<6 hex>` hash suffix (7) this caps the
    /// whole session name at 42 bytes, leaving ~56 bytes of headroom for
    /// the socket directory before the ~103-byte `sun_path` limit.
    const MAX_DIR_NAME: usize = 32;

    let dir_name = std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default");
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    let hash = hasher.finish();
    // Truncate on a UTF-8 char boundary so multi-byte branch names stay
    // valid; the hash below still disambiguates shared prefixes.
    let short_name: String = dir_name.chars().take(MAX_DIR_NAME).collect();
    format!("ws-{}-{:06x}", short_name, hash & 0xFFFFFF)
}

fn next_id(prefix: &str) -> String {
    let value = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{value}")
}

fn extract_numeric_suffix(value: &str) -> Option<u64> {
    value.rsplit('-').next()?.parse::<u64>().ok()
}

fn collect_numeric_ids_from_node(root: &PaneNodeSnapshot) -> Vec<Option<u64>> {
    match root {
        PaneNodeSnapshot::Terminal {
            pane_id,
            session_id,
            ..
        } => vec![
            extract_numeric_suffix(&pane_id.0),
            extract_numeric_suffix(&session_id.0),
        ],
        PaneNodeSnapshot::Browser {
            pane_id,
            browser_id,
            ..
        } => vec![
            extract_numeric_suffix(&pane_id.0),
            extract_numeric_suffix(&browser_id.0),
        ],
        PaneNodeSnapshot::AgentChat { pane_id, .. } => {
            vec![extract_numeric_suffix(&pane_id.0)]
        }
        PaneNodeSnapshot::Split {
            pane_id, children, ..
        } => {
            let mut ids = vec![extract_numeric_suffix(&pane_id.0)];
            ids.extend(children.iter().flat_map(collect_numeric_ids_from_node));
            ids
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Default workspace titles ────────────────────────────────────
    //
    // "Open project" / "Clone" / "New project" have no branch and no
    // first prompt, so whatever these produce is what the user lives
    // with permanently.

    #[test]
    fn default_title_uses_the_directory_name() {
        assert_eq!(
            AppStateStore::default_workspace_title(Path::new("/home/u/projects/codemux"), 58),
            "codemux"
        );
    }

    #[test]
    fn default_title_ignores_a_trailing_slash() {
        assert_eq!(
            AppStateStore::default_workspace_title(Path::new("/home/u/projects/codemux/"), 58),
            "codemux"
        );
    }

    #[test]
    fn default_title_falls_back_to_the_ordinal_without_a_usable_name() {
        // Filesystem root and empty paths have no final component.
        assert_eq!(
            AppStateStore::default_workspace_title(Path::new("/"), 58),
            "Workspace 58"
        );
        assert_eq!(
            AppStateStore::default_workspace_title(Path::new(""), 7),
            "Workspace 7"
        );
    }

    #[test]
    fn created_workspace_is_titled_after_its_directory() {
        let store = AppStateStore::default();
        let id = store.create_empty_workspace_at_path(PathBuf::from("/home/u/projects/codemux"));
        let snapshot = store.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == id)
            .expect("workspace exists");
        assert_eq!(ws.title, "codemux");
    }

    fn sample_split_tree() -> PaneNodeSnapshot {
        PaneNodeSnapshot::Split {
            pane_id: PaneId("pane-root".into()),
            direction: SplitDirection::Vertical,
            child_sizes: vec![0.5, 0.5],
            children: vec![
                PaneNodeSnapshot::Terminal {
                    pane_id: PaneId("pane-a".into()),
                    session_id: SessionId("session-a".into()),
                    title: "A".into(),
                },
                PaneNodeSnapshot::Terminal {
                    pane_id: PaneId("pane-b".into()),
                    session_id: SessionId("session-b".into()),
                    title: "B".into(),
                },
            ],
        }
    }

    fn collect_leaf_payload_ids(root: &PaneNodeSnapshot) -> Vec<String> {
        match root {
            PaneNodeSnapshot::Terminal { session_id, .. } => {
                vec![format!("terminal:{}", session_id.0)]
            }
            PaneNodeSnapshot::Browser { browser_id, .. } => {
                vec![format!("browser:{}", browser_id.0)]
            }
            PaneNodeSnapshot::AgentChat { pane_id, .. } => {
                vec![format!("agent_chat:{}", pane_id.0)]
            }
            PaneNodeSnapshot::Split { children, .. } => {
                children.iter().flat_map(collect_leaf_payload_ids).collect()
            }
        }
    }

    fn swap_positions<T: Clone>(items: &[T], first: usize, second: usize) -> Vec<T> {
        let mut swapped = items.to_vec();
        swapped.swap(first, second);
        swapped
    }

    fn workspace_by_id<'a>(
        snapshot: &'a AppStateSnapshot,
        workspace_id: &WorkspaceId,
    ) -> &'a WorkspaceSnapshot {
        snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == *workspace_id)
            .unwrap()
    }

    fn assert_swap_invariants_for_workspace(store: &AppStateStore, workspace_id: &WorkspaceId) {
        let base_snapshot = store.snapshot();
        let base_workspace = workspace_by_id(&base_snapshot, workspace_id);
        let base_surface = &base_workspace.surfaces[0];
        let pane_ids = collect_leaf_pane_ids(&base_surface.root);
        let payload_ids = collect_leaf_payload_ids(&base_surface.root);

        assert_eq!(pane_ids.len(), payload_ids.len());

        for source_index in 0..pane_ids.len() {
            for target_index in 0..pane_ids.len() {
                if source_index == target_index {
                    continue;
                }

                store.replace_snapshot(base_snapshot.clone());
                store
                    .swap_panes(&pane_ids[source_index].0, &pane_ids[target_index].0)
                    .unwrap();

                let swapped_snapshot = store.snapshot();
                let swapped_surface = &workspace_by_id(&swapped_snapshot, workspace_id).surfaces[0];

                assert_eq!(
                    collect_leaf_payload_ids(&swapped_surface.root),
                    swap_positions(&payload_ids, source_index, target_index),
                    "unexpected leaf order for source index {source_index} and target index {target_index}"
                );
                assert!(pane_tree_contains_pane(
                    &swapped_surface.root,
                    &swapped_surface.active_pane_id.0,
                ));

                store
                    .swap_panes(&pane_ids[source_index].0, &pane_ids[target_index].0)
                    .unwrap();

                let restored_snapshot = store.snapshot();
                let restored_surface =
                    &workspace_by_id(&restored_snapshot, workspace_id).surfaces[0];
                assert_eq!(
                    collect_leaf_payload_ids(&restored_surface.root),
                    payload_ids
                );
            }
        }

        store.replace_snapshot(base_snapshot);
    }

    #[test]
    fn removing_terminal_collapses_split_tree() {
        let updated = remove_terminal_from_tree(&sample_split_tree(), "session-a").unwrap();

        match updated {
            PaneNodeSnapshot::Terminal { session_id, .. } => {
                assert_eq!(session_id.0, "session-b");
            }
            _ => panic!("expected collapsed terminal node"),
        }
    }

    fn terminal_leaf(pane: &str, session: &str) -> PaneNodeSnapshot {
        PaneNodeSnapshot::Terminal {
            pane_id: PaneId(pane.into()),
            session_id: SessionId(session.into()),
            title: pane.to_string(),
        }
    }

    fn custom_sized_triple_split() -> PaneNodeSnapshot {
        PaneNodeSnapshot::Split {
            pane_id: PaneId("pane-root".into()),
            direction: SplitDirection::Horizontal,
            child_sizes: vec![0.5, 0.3, 0.2],
            children: vec![
                terminal_leaf("pane-a", "session-a"),
                terminal_leaf("pane-b", "session-b"),
                terminal_leaf("pane-c", "session-c"),
            ],
        }
    }

    #[test]
    fn remove_pane_preserves_sibling_proportions() {
        let tree = custom_sized_triple_split();

        // Closing the last child keeps the first two at their 0.5:0.3 ratio
        // (renormalized to 0.625:0.375), not reset to equal 0.5:0.5.
        match remove_pane_from_tree(&tree, "pane-c").unwrap() {
            PaneNodeSnapshot::Split { child_sizes, children, .. } => {
                assert_eq!(children.len(), 2);
                assert!((child_sizes[0] - 0.625).abs() < 1e-4, "got {child_sizes:?}");
                assert!((child_sizes[1] - 0.375).abs() < 1e-4, "got {child_sizes:?}");
            }
            _ => panic!("expected split"),
        }

        // Closing the middle child keeps the outer two at their 0.5:0.2 ratio.
        match remove_pane_from_tree(&tree, "pane-b").unwrap() {
            PaneNodeSnapshot::Split { child_sizes, .. } => {
                assert!((child_sizes[0] - 0.5 / 0.7).abs() < 1e-4, "got {child_sizes:?}");
                assert!((child_sizes[1] - 0.2 / 0.7).abs() < 1e-4, "got {child_sizes:?}");
            }
            _ => panic!("expected split"),
        }
    }

    fn close_pane_with_root(active: &str, close: &str) -> String {
        let store = AppStateStore::default();
        let mut snap = store.snapshot();
        {
            let surface = &mut snap.workspaces[0].surfaces[0];
            surface.root = custom_sized_triple_split();
            surface.active_pane_id = PaneId(active.into());
        }
        store.replace_snapshot(snap);
        store.close_pane(close).unwrap();
        store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id
            .0
            .clone()
    }

    #[test]
    fn close_active_pane_focuses_next_then_previous() {
        // Closing the active middle pane focuses the next (right) pane…
        assert_eq!(close_pane_with_root("pane-b", "pane-b"), "pane-c");
        // …and closing the active last pane focuses the previous one.
        assert_eq!(close_pane_with_root("pane-c", "pane-c"), "pane-b");
    }

    #[test]
    fn close_inactive_pane_keeps_focus() {
        // Closing a different pane must not move focus off the active pane.
        assert_eq!(close_pane_with_root("pane-b", "pane-a"), "pane-b");
        assert_eq!(close_pane_with_root("pane-b", "pane-c"), "pane-b");
    }

    #[test]
    fn close_pane_preserves_sibling_proportions() {
        // Exercise the real public close path on a live store/surface.
        let store = AppStateStore::default();
        let mut snap = store.snapshot();
        {
            let surface = &mut snap.workspaces[0].surfaces[0];
            surface.root = custom_sized_triple_split();
            surface.active_pane_id = PaneId("pane-a".into());
        }
        store.replace_snapshot(snap);

        store.close_pane("pane-c").unwrap();

        let snap = store.snapshot();
        match &snap.workspaces[0].surfaces[0].root {
            PaneNodeSnapshot::Split { child_sizes, children, .. } => {
                assert_eq!(children.len(), 2);
                assert!((child_sizes[0] - 0.625).abs() < 1e-4, "got {child_sizes:?}");
                assert!((child_sizes[1] - 0.375).abs() < 1e-4, "got {child_sizes:?}");
            }
            _ => panic!("expected split after closing one pane"),
        }
    }

    #[test]
    fn app_state_creates_and_closes_terminal_sessions() {
        let store = AppStateStore::default();
        let first_active = store.active_terminal_session_id().unwrap();
        let created = store.create_terminal_session().unwrap();

        assert_ne!(first_active.0, created.0);
        assert!(store.activate_terminal_session(&created.0));
        assert_eq!(store.active_terminal_session_id().unwrap().0, created.0);

        let fallback = store.close_terminal_session(&created.0).unwrap();
        assert_eq!(fallback.0, first_active.0);
        assert_eq!(store.snapshot().terminal_sessions.len(), 1);
    }

    #[test]
    fn set_workspace_project_root_stamps_project_identity() {
        let store = AppStateStore::default();
        let wid = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux-identity-test"),
            WorkspacePresetLayout::Single,
        );

        store.set_workspace_project_root(&wid.0, "/tmp/codemux-identity-test".into());

        let snap = store.snapshot();
        let w = workspace_by_id(&snap, &wid);
        // No worktree_path on a layout workspace → it's the root (main).
        assert_eq!(w.workspace_kind.as_deref(), Some("main"));
        // Deterministic uid: this path isn't a git repo, so it falls
        // back to the path-based identity the helper computes.
        let expected = crate::project_identity::project_uid_for(
            None,
            "/tmp/codemux-identity-test",
        );
        assert_eq!(
            w.project_uid.as_deref(),
            Some(expected.as_str()),
            "project_uid must be stamped deterministically at create"
        );
    }

    #[test]
    fn create_remote_attach_workspace_sets_in_place_fields_and_a_terminal() {
        let store = AppStateStore::default();
        let wid = store.create_remote_attach_workspace(
            "remote-svc".into(),
            7,
            "/srv/agent/work/svc".into(),
            Some("feature/y".into()),
            Some("/home/agent/svc".into()),
            Some("uid-svc".into()),
            Some("worktree".into()),
        );

        let snap = store.snapshot();
        let w = workspace_by_id(&snap, &wid);
        assert!(w.attach_only, "open-on-host workspace must be attach_only");
        assert_eq!(w.host_id, Some(7), "routes terminals to the host daemon");
        assert_eq!(w.remote_cwd.as_deref(), Some("/srv/agent/work/svc"));
        // cwd mirrors the host path (display only); no local worktree.
        assert_eq!(w.cwd, "/srv/agent/work/svc");
        assert!(w.worktree_path.is_none(), "no local checkout");
        assert_eq!(w.git_branch.as_deref(), Some("feature/y"));
        assert_eq!(w.project_uid.as_deref(), Some("uid-svc"));
        assert_eq!(w.workspace_kind.as_deref(), Some("worktree"));
        // It opens ready-to-use: one terminal surface, and it's active.
        assert_eq!(w.surfaces.len(), 1, "a single terminal pane to attach");
        assert!(matches!(
            w.surfaces[0].root,
            PaneNodeSnapshot::Terminal { .. }
        ));
        assert_eq!(
            snap.active_workspace_id, wid,
            "opening on host activates the workspace"
        );
        // It must be excluded from the periodic local git-enrichment sweep
        // (its cwd is a host path that doesn't exist on this device).
        assert!(
            !store.all_workspace_cwds().contains_key(&wid.0),
            "attach_only workspaces are skipped by the git enrichment sweep"
        );
    }

    #[test]
    fn workspace_mute_toggles_and_scopes_to_the_owning_session() {
        let store = AppStateStore::default();
        let workspace_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux"),
            WorkspacePresetLayout::Single,
        );

        // Pull the terminal session id that belongs to this workspace.
        let snapshot = store.snapshot();
        let workspace = workspace_by_id(&snapshot, &workspace_id);
        let session_id = match &workspace.surfaces[0].root {
            PaneNodeSnapshot::Terminal { session_id, .. } => session_id.0.clone(),
            other => panic!("Single layout should have a terminal root, got {other:?}"),
        };

        // Default: not muted — the hook handler would NOT suppress.
        assert!(!workspace.notifications_muted);
        assert!(!store.is_session_workspace_muted(&session_id));

        // Mute: both the snapshot flag and the session lookup flip.
        assert!(store.set_workspace_muted(&workspace_id.0, true));
        assert!(store.is_session_workspace_muted(&session_id));
        assert!(
            workspace_by_id(&store.snapshot(), &workspace_id).notifications_muted
        );

        // Muting is scoped: an unknown session is never reported muted.
        assert!(!store.is_session_workspace_muted("session-does-not-exist"));

        // Unmute: back to the un-suppressed state.
        assert!(store.set_workspace_muted(&workspace_id.0, false));
        assert!(!store.is_session_workspace_muted(&session_id));

        // Unknown workspace id: the mutator reports "not found".
        assert!(!store.set_workspace_muted("workspace-does-not-exist", true));
    }

    #[test]
    fn split_size_normalization_preserves_total() {
        let normalized = normalize_sizes(vec![0.8, 0.3]);
        let total: f32 = normalized.iter().sum();
        assert!((total - 1.0).abs() < 0.0001);
        assert!(normalized.iter().all(|value| *value >= 0.1));
    }

    #[test]
    fn workspace_preset_six_creates_three_by_three_grid() {
        let store = AppStateStore::default();
        let workspace_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux"),
            WorkspacePresetLayout::Six,
        );
        let snapshot = store.snapshot();
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
            .unwrap();
        let root = &workspace.surfaces[0].root;

        match root {
            PaneNodeSnapshot::Split {
                direction: SplitDirection::Vertical,
                children,
                ..
            } => {
                assert_eq!(children.len(), 2);
                for child in children {
                    match child {
                        PaneNodeSnapshot::Split {
                            direction: SplitDirection::Horizontal,
                            children,
                            ..
                        } => assert_eq!(children.len(), 3),
                        _ => panic!("expected each row to be a horizontal split"),
                    }
                }
            }
            _ => panic!("expected six-slot preset root to be vertical split"),
        }
    }

    #[test]
    fn workspace_preset_shell_browser_creates_one_shell_and_one_browser() {
        let store = AppStateStore::default();
        let workspace_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux"),
            WorkspacePresetLayout::ShellBrowser,
        );
        let snapshot = store.snapshot();
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
            .unwrap();

        assert_eq!(snapshot.browser_sessions.len(), 1);

        match &workspace.surfaces[0].root {
            PaneNodeSnapshot::Split {
                direction: SplitDirection::Horizontal,
                children,
                ..
            } => {
                assert_eq!(children.len(), 2);
                assert!(matches!(children[0], PaneNodeSnapshot::Terminal { .. }));
                assert!(matches!(children[1], PaneNodeSnapshot::Browser { .. }));
            }
            _ => panic!("expected shell+browser preset to be a horizontal split"),
        }
    }

    #[test]
    fn workspace_preset_empty_produces_no_surfaces_terminals_or_tabs() {
        // Empty layout is what the inline "+ New worktree…" chat
        // flow uses: the backend creates the git worktree + workspace
        // record, but leaves no surfaces / terminal sessions / tabs
        // behind — the frontend attaches a chat pane afterward via
        // `agent_chat_create_pane`. Regressing this to any other
        // layout reintroduces the split-with-leftover-terminal bug.
        let store = AppStateStore::default();
        let terminals_before = store.snapshot().terminal_sessions.len();
        let workspace_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux-empty"),
            WorkspacePresetLayout::Empty,
        );
        let snapshot = store.snapshot();
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
            .expect("workspace must be registered");

        assert!(
            workspace.surfaces.is_empty(),
            "Empty layout must produce zero surfaces (got {})",
            workspace.surfaces.len(),
        );
        assert!(
            workspace.tabs.is_empty(),
            "Empty layout must produce zero tabs (got {})",
            workspace.tabs.len(),
        );
        assert_eq!(
            workspace.active_tab_id, "",
            "Empty layout must leave active_tab_id blank, not a dangling Terminal tab",
        );
        assert_eq!(
            snapshot.terminal_sessions.len(),
            terminals_before,
            "Empty layout must NOT allocate any terminal sessions",
        );
        assert_eq!(
            snapshot.active_workspace_id, workspace_id,
            "Empty layout still activates the new workspace (parity with other variants)",
        );
    }

    #[test]
    fn create_synced_workspace_shell_sets_host_id_and_worktree_path() {
        // The shell is what cross-device adoption creates BEFORE the
        // rsync runs — host_id must be pre-set (the upcoming
        // workspace_pull_back routes off it), worktree_path must be
        // the empty target the rsync will populate, and no tabs /
        // surfaces / sessions get allocated (the user hasn't opened
        // panes yet — they will after the pull completes).
        let store = AppStateStore::default();
        let workspace_id = store.create_synced_workspace_shell(
            "codemux/feature-x".into(),
            42, // local hosts.id
            Some("/home/zeus/projects/codemux".into()),
            "/home/zeus/.codemux/worktrees/codemux/feature-x".into(),
            Some("feature-x".into()),
        );
        let snapshot = store.snapshot();
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == workspace_id)
            .expect("shell must be registered");
        assert_eq!(workspace.title, "codemux/feature-x");
        assert_eq!(workspace.host_id, Some(42));
        assert_eq!(
            workspace.worktree_path.as_deref(),
            Some("/home/zeus/.codemux/worktrees/codemux/feature-x"),
        );
        // cwd mirrors worktree_path so terminals spawn in the right
        // place once the pull completes.
        assert_eq!(
            workspace.cwd,
            "/home/zeus/.codemux/worktrees/codemux/feature-x",
        );
        assert_eq!(
            workspace.project_root.as_deref(),
            Some("/home/zeus/projects/codemux"),
        );
        assert_eq!(workspace.git_branch.as_deref(), Some("feature-x"));
        assert!(
            workspace.surfaces.is_empty(),
            "Shell must have no surfaces — rsync populates the worktree, panes come later",
        );
        assert!(
            workspace.tabs.is_empty(),
            "Shell must have no tabs",
        );
    }

    #[test]
    fn create_synced_root_shell_registers_a_repo_root_not_a_worktree() {
        // Adopting a `main` row creates a ROOT shell: it lands at the
        // project root (~/.codemux/projects/<repo>), has NO worktree_path,
        // and is kind "main" — so it classifies as a genuine repo root
        // rather than a divergent copy in the worktrees tree.
        let store = AppStateStore::default();
        let wid = store.create_synced_root_shell(
            "passpage".into(),
            42,
            "/home/zeus/.codemux/projects/passpage".into(),
            Some("main".into()),
        );
        let snapshot = store.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == wid)
            .expect("root shell registered");
        assert_eq!(ws.host_id, Some(42));
        assert_eq!(ws.worktree_path, None, "a repo root has no worktree_path");
        assert_eq!(ws.cwd, "/home/zeus/.codemux/projects/passpage");
        assert_eq!(
            ws.project_root.as_deref(),
            Some("/home/zeus/.codemux/projects/passpage"),
        );
        assert_eq!(ws.workspace_kind.as_deref(), Some("main"));
        assert!(!ws.protected, "protected is stamped after the pull lands");

        // The protected setter flips it without touching identity.
        store.set_workspace_protected(&wid.0, true);
        assert!(
            store
                .snapshot()
                .workspaces
                .iter()
                .find(|w| w.workspace_id == wid)
                .unwrap()
                .protected
        );
    }

    #[test]
    fn set_workspace_project_identity_stamps_uid_from_synced_row() {
        // Adoption stamps the daemon-derived identity from the synced
        // row (NOT a local recompute) so the adopted workspace converges
        // with its siblings and the post-adopt reconcile doesn't push a
        // None uid back over the row, wiping it.
        let store = AppStateStore::default();
        let wid = store.create_synced_workspace_shell(
            "proj".into(),
            7,
            Some("/srv/proj".into()),
            "/home/zeus/.codemux/worktrees/proj/main".into(),
            Some("main".into()),
        );
        // Shell starts with no project_uid.
        assert!(workspace_by_id(&store.snapshot(), &wid).project_uid.is_none());

        store.set_workspace_project_identity(
            &wid.0,
            Some("daemon-uid-xyz".into()),
            Some("main".into()),
        );

        let snap = store.snapshot();
        let w = workspace_by_id(&snap, &wid);
        assert_eq!(
            w.project_uid.as_deref(),
            Some("daemon-uid-xyz"),
            "uid must be copied verbatim from the synced row",
        );
        assert_eq!(w.workspace_kind.as_deref(), Some("main"));

        // A None passed in must NOT clobber an already-set value
        // (older daemons send no uid — leave whatever's there).
        store.set_workspace_project_identity(&wid.0, None, None);
        assert_eq!(
            workspace_by_id(&store.snapshot(), &wid)
                .project_uid
                .as_deref(),
            Some("daemon-uid-xyz"),
            "None must be a no-op, not a wipe",
        );
    }

    #[test]
    fn create_synced_workspace_shell_does_not_activate_workspace() {
        // The user clicked Pull from the Workspaces overview; they
        // should land back in the overview when the pull completes,
        // NOT in a half-populated workspace. Activation is the
        // frontend's call via the success toast's "Open" action.
        let store = AppStateStore::default();
        let prior_active = store.snapshot().active_workspace_id.clone();
        let _ = store.create_synced_workspace_shell(
            "shell".into(),
            1,
            None,
            "/tmp/shell".into(),
            None,
        );
        let snapshot = store.snapshot();
        assert_eq!(
            snapshot.active_workspace_id, prior_active,
            "Shell creation must not steal the active workspace slot",
        );
    }

    #[test]
    fn split_pane_inherits_workspace_cwd() {
        // Shift+clicking a preset (or a plain split) opens a new pane
        // in the same surface. The new terminal session must spawn in
        // the workspace directory — not the app process's current dir,
        // which is typically the home directory. Regression guard for
        // the split-pane preset landing in $HOME instead of the
        // workspace.
        let store = AppStateStore::default();
        let workspace_id =
            store.create_workspace_at_path(PathBuf::from("/tmp/codemux-split-cwd-test"));

        let active_pane_id = {
            let snapshot = store.snapshot();
            let workspace = workspace_by_id(&snapshot, &workspace_id);
            let surface = workspace
                .surfaces
                .iter()
                .find(|surface| surface.surface_id == workspace.active_surface_id)
                .expect("workspace must have an active surface");
            surface.active_pane_id.clone()
        };

        let new_session_id = store
            .split_pane(&active_pane_id.0, SplitDirection::Horizontal)
            .expect("split must succeed");

        let snapshot = store.snapshot();
        let session = snapshot
            .terminal_sessions
            .iter()
            .find(|session| session.session_id == new_session_id)
            .expect("split must create a terminal session");
        assert_eq!(
            session.cwd, "/tmp/codemux-split-cwd-test",
            "split pane must inherit the workspace cwd, not the app's current dir",
        );
    }

    #[test]
    fn swap_invariants_hold_for_all_builtin_workspace_layouts() {
        let layouts = [
            WorkspacePresetLayout::Single,
            WorkspacePresetLayout::Pair,
            WorkspacePresetLayout::Quad,
            WorkspacePresetLayout::Six,
            WorkspacePresetLayout::Eight,
            WorkspacePresetLayout::ShellBrowser,
        ];

        for layout in layouts {
            let store = AppStateStore::default();
            let workspace_id =
                store.create_workspace_with_layout(PathBuf::from("/tmp/codemux"), layout);
            assert_swap_invariants_for_workspace(&store, &workspace_id);
        }
    }

    #[test]
    fn swap_invariants_hold_for_incrementally_built_terminal_workspace() {
        let store = AppStateStore::default();
        let workspace_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux"),
            WorkspacePresetLayout::Single,
        );

        for _ in 0..5 {
            store.create_terminal_session().unwrap();
        }

        assert_swap_invariants_for_workspace(&store, &workspace_id);
    }

    #[test]
    fn swap_invariants_hold_for_mixed_terminal_browser_workspace() {
        let store = AppStateStore::default();
        let workspace_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux"),
            WorkspacePresetLayout::ShellBrowser,
        );

        let initial_snapshot = store.snapshot();
        let workspace = workspace_by_id(&initial_snapshot, &workspace_id);
        let active_pane_id = workspace.surfaces[0].active_pane_id.0.clone();

        store.create_browser_pane(&active_pane_id, None).unwrap();
        store.create_terminal_session().unwrap();

        assert_swap_invariants_for_workspace(&store, &workspace_id);
    }

    #[test]
    fn terminal_creation_and_split_respect_workspace_session_limit() {
        let store = AppStateStore::default();

        for _ in 0..(MAX_TERMINAL_SESSIONS - 1) {
            store.create_terminal_session().unwrap();
        }

        let snapshot = store.snapshot();
        assert_eq!(snapshot.terminal_sessions.len(), MAX_TERMINAL_SESSIONS);
        let active_workspace = workspace_by_id(&snapshot, &snapshot.active_workspace_id);
        let active_pane_id = active_workspace.surfaces[0].active_pane_id.0.clone();
        assert_eq!(
            terminal_count_for_workspace(active_workspace),
            MAX_TERMINAL_SESSIONS
        );

        let create_error = store.create_terminal_session().unwrap_err();
        assert!(create_error.contains("limit"));

        let split_error = store
            .split_pane(&active_pane_id, SplitDirection::Horizontal)
            .unwrap_err();
        assert!(split_error.contains("limit"));
    }

    #[test]
    fn workspace_terminal_limit_does_not_block_other_workspaces() {
        let store = AppStateStore::default();

        for _ in 0..(MAX_TERMINAL_SESSIONS - 1) {
            store.create_terminal_session().unwrap();
        }

        let second_workspace_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux"),
            WorkspacePresetLayout::Single,
        );

        let second_snapshot = store.snapshot();
        let second_workspace = workspace_by_id(&second_snapshot, &second_workspace_id);
        assert_eq!(terminal_count_for_workspace(second_workspace), 1);

        store.create_terminal_session().unwrap();

        let after_create = store.snapshot();
        let second_workspace = workspace_by_id(&after_create, &second_workspace_id);
        assert_eq!(terminal_count_for_workspace(second_workspace), 2);

        let active_pane_id = second_workspace.surfaces[0].active_pane_id.0.clone();
        store
            .split_pane(&active_pane_id, SplitDirection::Vertical)
            .unwrap();

        let after_split = store.snapshot();
        let second_workspace = workspace_by_id(&after_split, &second_workspace_id);
        assert_eq!(terminal_count_for_workspace(second_workspace), 3);
    }

    #[test]
    fn close_last_workspace_empties_state() {
        let store = AppStateStore::default();
        let snapshot = store.snapshot();
        assert_eq!(snapshot.workspaces.len(), 1);

        let ws_id = snapshot.workspaces[0].workspace_id.0.clone();
        let result = store.close_workspace(&ws_id);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().fallback.0, "");

        let after = store.snapshot();
        assert!(after.workspaces.is_empty());
        assert_eq!(after.active_workspace_id.0, "");
    }

    #[test]
    fn close_one_of_many_workspaces_keeps_others() {
        let store = AppStateStore::default();

        let ws2_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/project-b"),
            WorkspacePresetLayout::Single,
        );
        let ws3_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/project-c"),
            WorkspacePresetLayout::Single,
        );

        let snapshot = store.snapshot();
        assert_eq!(snapshot.workspaces.len(), 3);

        // Close the second workspace
        let result = store.close_workspace(&ws2_id.0);
        assert!(result.is_ok());

        let after = store.snapshot();
        assert_eq!(after.workspaces.len(), 2);
        assert!(after.workspaces.iter().all(|w| w.workspace_id.0 != ws2_id.0));
        assert!(after.workspaces.iter().any(|w| w.workspace_id.0 == ws3_id.0));
        // Active workspace should be set to the first remaining
        assert!(!after.active_workspace_id.0.is_empty());
    }

    /// Regression guard for the PTY child-process leak fix: `close_workspace`
    /// must return every terminal session that was attached to the workspace
    /// so the command layer can kill each PTY child process tree. Without
    /// this atomic return path, the command layer had to snapshot session
    /// IDs before the state mutation, opening a TOCTOU race where a pane
    /// created between snapshot and close would leak its PTY.
    #[test]
    fn close_workspace_result_contains_session_ids() {
        let store = AppStateStore::default();

        // Fresh workspace is created with one default session. Split the
        // active pane twice to reach three terminal sessions without
        // creating new tabs.
        let ws_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/project-close-test"),
            WorkspacePresetLayout::Single,
        );
        store.activate_workspace(&ws_id.0);

        let initial_snapshot = store.snapshot();
        let workspace = workspace_by_id(&initial_snapshot, &ws_id);
        let active_pane_id = workspace.surfaces[0].active_pane_id.0.clone();

        let split1 = store
            .split_pane(&active_pane_id, SplitDirection::Horizontal)
            .expect("first split should succeed");
        let split2 = store
            .split_pane(&active_pane_id, SplitDirection::Vertical)
            .expect("second split should succeed");

        // Collect the expected session IDs from the live snapshot before close.
        let expected: Vec<String> = {
            let snap = store.snapshot();
            let ws = workspace_by_id(&snap, &ws_id);
            crate::state::collect_terminal_sessions(&ws.surfaces)
        };
        assert_eq!(
            expected.len(),
            3,
            "test setup should have produced exactly 3 terminal sessions \
             (one default + two splits), got {expected:?}"
        );
        assert!(expected.contains(&split1.0), "split1 session should be tracked");
        assert!(expected.contains(&split2.0), "split2 session should be tracked");

        // Close the workspace and assert the returned removed_sessions
        // contains exactly the same set (order not guaranteed).
        let result = store
            .close_workspace(&ws_id.0)
            .expect("close should succeed");
        let returned: std::collections::HashSet<String> = result
            .removed_sessions
            .iter()
            .map(|s| s.0.clone())
            .collect();
        let expected_set: std::collections::HashSet<String> =
            expected.into_iter().collect();
        assert_eq!(
            returned, expected_set,
            "CloseWorkspaceResult.removed_sessions must contain every terminal \
             session that was attached to the workspace at close time"
        );
    }

    /// Regression guard mirroring `close_workspace_result_contains_session_ids`
    /// for `close_tab`. `CloseTabResult.removed_sessions` is what the
    #[test]
    fn notification_level_parses_from_string() {
        // "info" must map to Info — the notify control handler used to drop the
        // level and hardcode Attention, wrongly elevating info notifications.
        assert!(matches!(
            NotificationLevel::from_str_or_attention("info"),
            NotificationLevel::Info
        ));
        assert!(matches!(
            NotificationLevel::from_str_or_attention("attention"),
            NotificationLevel::Attention
        ));
        // "error" has no dedicated variant → surfaces at Attention.
        assert!(matches!(
            NotificationLevel::from_str_or_attention("error"),
            NotificationLevel::Attention
        ));
        // Unknown → Attention (safe default).
        assert!(matches!(
            NotificationLevel::from_str_or_attention("bogus"),
            NotificationLevel::Attention
        ));
    }

    #[test]
    fn close_active_middle_tab_focuses_next_tab() {
        let store = AppStateStore::default();
        let ws_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/project-close-tab-next"),
            WorkspacePresetLayout::Single,
        );
        store.activate_workspace(&ws_id.0);

        // Default tab + two more → order [default, t2, t3].
        let (t2, _) = store
            .create_tab(&ws_id.0, TabKind::Terminal)
            .expect("create t2");
        let (t3, _) = store
            .create_tab(&ws_id.0, TabKind::Terminal)
            .expect("create t3");

        // Make the middle tab active, then close it.
        store.activate_tab(&ws_id.0, &t2).expect("activate t2");
        store.close_tab(&ws_id.0, &t2).expect("close t2");

        let snap = store.snapshot();
        let ws = workspace_by_id(&snap, &ws_id);
        assert_eq!(
            ws.active_tab_id, t3,
            "closing the active middle tab should select the NEXT tab, not the previous"
        );
    }

    #[test]
    fn closing_last_pane_of_active_middle_tab_focuses_next_tab() {
        let store = AppStateStore::default();
        let ws_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/project-close-lastpane-next"),
            WorkspacePresetLayout::Single,
        );
        store.activate_workspace(&ws_id.0);

        // Default tab + two more → [default, t2, t3], each a single-pane tab.
        let (t2, _) = store
            .create_tab(&ws_id.0, TabKind::Terminal)
            .expect("create t2");
        let (t3, _) = store
            .create_tab(&ws_id.0, TabKind::Terminal)
            .expect("create t3");
        store.activate_tab(&ws_id.0, &t2).expect("activate t2");

        // The only pane in t2's surface.
        let pane_id = {
            let snap = store.snapshot();
            let ws = workspace_by_id(&snap, &ws_id);
            let sid = ws
                .tabs
                .iter()
                .find(|t| t.tab_id == t2)
                .and_then(|t| t.surface_id.clone())
                .expect("t2 has a surface");
            let surface = ws
                .surfaces
                .iter()
                .find(|s| s.surface_id == sid)
                .expect("surface exists");
            first_leaf_pane_id(&surface.root).expect("a pane").0
        };

        // Closing the last pane removes the tab; focus should move to the next.
        store.close_pane(&pane_id).expect("close pane");

        let snap = store.snapshot();
        let ws = workspace_by_id(&snap, &ws_id);
        assert!(
            !ws.tabs.iter().any(|t| t.tab_id == t2),
            "t2 should be gone after closing its last pane"
        );
        assert_eq!(
            ws.active_tab_id, t3,
            "closing the last pane of the active middle tab should focus the NEXT tab"
        );
    }

    /// `close_tab` command hands to `terminate_pty_session` for PTY cleanup;
    /// if this list ever drops a session the corresponding PTY leaks.
    #[test]
    fn close_tab_result_contains_session_ids() {
        let store = AppStateStore::default();

        let ws_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/project-close-tab-test"),
            WorkspacePresetLayout::Single,
        );
        store.activate_workspace(&ws_id.0);

        // The workspace has a default tab with one session. Create a
        // second terminal tab, then split its pane so the tab has two
        // sessions. Close THAT tab — not the default — so we know exactly
        // which sessions must come back.
        let (tab_id, first_session_id) = store
            .create_tab(&ws_id.0, TabKind::Terminal)
            .expect("create_tab should succeed");
        let first_session_id = first_session_id.expect("terminal tab should have a session");

        // Activate the newly created tab so split_pane targets its pane.
        store
            .activate_tab(&ws_id.0, &tab_id)
            .expect("activate_tab should succeed");

        let snap = store.snapshot();
        let ws = workspace_by_id(&snap, &ws_id);
        let active_pane_id = ws
            .surfaces
            .iter()
            .find(|s| Some(s.surface_id.clone()) == ws.tabs.iter().find(|t| t.tab_id == tab_id).and_then(|t| t.surface_id.clone()))
            .expect("new tab should have a surface")
            .active_pane_id
            .0
            .clone();

        let second_session_id = store
            .split_pane(&active_pane_id, SplitDirection::Horizontal)
            .expect("split_pane should succeed");

        let expected: std::collections::HashSet<String> = [
            first_session_id.0.clone(),
            second_session_id.0.clone(),
        ]
        .into_iter()
        .collect();

        let result = store
            .close_tab(&ws_id.0, &tab_id)
            .expect("close_tab should succeed");
        let returned: std::collections::HashSet<String> = result
            .removed_sessions
            .iter()
            .map(|s| s.0.clone())
            .collect();

        assert_eq!(
            returned, expected,
            "CloseTabResult.removed_sessions must contain every terminal \
             session that was attached to the tab at close time"
        );
    }

    #[test]
    fn clear_workspaces_empties_state_but_keeps_config() {
        let store = AppStateStore::default();
        let before = store.snapshot();
        assert_eq!(before.workspaces.len(), 1);
        assert!(!before.terminal_sessions.is_empty());

        store.clear_workspaces();

        let after = store.snapshot();
        assert!(after.workspaces.is_empty());
        assert!(after.terminal_sessions.is_empty());
        assert_eq!(after.active_workspace_id.0, "");
        // Config should be preserved
        assert!(after.config.linux_first);
        assert_eq!(after.config.theme_source, "omarchy_or_default");
    }

    // ── Agent Browser Session Tests ──

    #[test]
    fn agent_browser_session_created_for_workspace() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();

        let session = store.resolve_agent_browser_session(&ws_id.0, 9223);
        assert_eq!(session.workspace_id, ws_id);
        // cli_session_name is now derived from the workspace cwd for cookie persistence
        assert!(session.cli_session_name.starts_with("ws-"), "expected stable cwd-based name, got: {}", session.cli_session_name);
        assert_eq!(session.stream_url, "ws://localhost:9223");
        assert!(!session.is_active);
        assert!(session.pane_id.is_none());
        assert!(session.browser_id.is_none());

        let snap = store.snapshot();
        assert_eq!(snap.agent_browser_sessions.len(), 1);
    }

    #[test]
    fn agent_browser_session_scoped_per_workspace() {
        let store = AppStateStore::default();
        let ws1_id = store.snapshot().workspaces[0].workspace_id.clone();
        let ws2_id = store.create_workspace_at_path(std::path::PathBuf::from("/tmp"));

        let s1 = store.resolve_agent_browser_session(&ws1_id.0, 9223);
        let s2 = store.resolve_agent_browser_session(&ws2_id.0, 9223);

        assert_ne!(s1.session_id, s2.session_id);
        assert_ne!(s1.cli_session_name, s2.cli_session_name);
        assert_eq!(store.snapshot().agent_browser_sessions.len(), 2);
    }

    #[test]
    fn agent_browser_resolve_idempotent() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();

        let s1 = store.resolve_agent_browser_session(&ws_id.0, 9223);
        let s2 = store.resolve_agent_browser_session(&ws_id.0, 9223);

        assert_eq!(s1.session_id, s2.session_id);
        assert_eq!(store.snapshot().agent_browser_sessions.len(), 1);
    }

    /// Regression guard for issue #126: closing a workspace must return the
    /// `cli_session_name`s of the agent-browser sessions that were removed
    /// alongside it, so the command layer can reap the daemon instead of
    /// leaking it for the rest of the app's lifetime. Exercises the
    /// "close the last workspace" branch of `close_workspace`.
    #[test]
    fn close_last_workspace_returns_removed_agent_browser_sessions() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();

        let session = store.resolve_agent_browser_session(&ws_id.0, 9223);

        let result = store
            .close_workspace(&ws_id.0)
            .expect("close should succeed");
        assert_eq!(
            result.removed_agent_browser_sessions,
            vec![session.cli_session_name]
        );
        assert!(store.snapshot().agent_browser_sessions.is_empty());
    }

    /// Same as above but for the "other workspaces remain" branch, and
    /// checks that an unrelated workspace's agent-browser session survives
    /// the close untouched (the reap must be scoped to the closed
    /// workspace only, never anyone else's live daemon).
    #[test]
    fn close_one_of_many_workspaces_returns_only_its_agent_browser_sessions() {
        let store = AppStateStore::default();
        let ws1_id = store.snapshot().workspaces[0].workspace_id.clone();
        let ws2_id = store.create_workspace_at_path(std::path::PathBuf::from("/tmp/ab-b"));

        let s1 = store.resolve_agent_browser_session(&ws1_id.0, 9223);
        let s2 = store.resolve_agent_browser_session(&ws2_id.0, 9224);

        let result = store
            .close_workspace(&ws2_id.0)
            .expect("close should succeed");
        assert_eq!(result.removed_agent_browser_sessions, vec![s2.cli_session_name]);

        let after = store.snapshot();
        assert_eq!(after.agent_browser_sessions.len(), 1);
        assert_eq!(after.agent_browser_sessions[0].cli_session_name, s1.cli_session_name);
    }

    /// `live_agent_browser_session_names` must contain both the cwd-derived
    /// stable name (for workspaces that never opened a browser pane yet)
    /// and the actual tracked `cli_session_name` (which is what the sweep
    /// compares tracked daemon keys against). Issue #126.
    #[test]
    fn live_agent_browser_session_names_contains_cwd_derived_and_tracked_names() {
        let store = AppStateStore::default();
        let ws1_id = store.snapshot().workspaces[0].workspace_id.clone();
        let ws1_cwd = store.snapshot().workspaces[0].cwd.clone();
        let ws2_id = store.create_workspace_at_path(std::path::PathBuf::from("/tmp/ab-live"));

        // ws1 has an active agent-browser session; ws2 has never opened one.
        let s1 = store.resolve_agent_browser_session(&ws1_id.0, 9223);

        let live = store.live_agent_browser_session_names();
        assert!(live.contains(&s1.cli_session_name));
        assert!(live.contains(&stable_browser_session_name(&ws1_cwd)));
        let ws2_cwd = store
            .snapshot()
            .workspaces
            .iter()
            .find(|w| w.workspace_id == ws2_id)
            .unwrap()
            .cwd
            .clone();
        assert!(live.contains(&stable_browser_session_name(&ws2_cwd)));
    }

    /// Issue #126 review, Fix 3: resolving an agent-browser session for a
    /// workspace_id that no longer exists (an in-flight `browser_automation`
    /// racing a close) must return a usable transient session WITHOUT
    /// persisting it — a persisted record for a dead workspace can never be
    /// closed and would permanently poison the orphan sweep's live set.
    #[test]
    fn resolve_agent_browser_session_for_dead_workspace_is_not_persisted() {
        let store = AppStateStore::default();

        let session = store.resolve_agent_browser_session("no-such-workspace", 9223);
        // The transient session keeps the in-flight caller working...
        assert_eq!(session.workspace_id.0, "no-such-workspace");
        assert_eq!(session.cli_session_name, "ws-no-such-workspace");
        // ...but nothing is persisted, so the sweep's live set stays clean.
        assert!(store.snapshot().agent_browser_sessions.is_empty());
        assert!(!store
            .live_agent_browser_session_names()
            .contains("ws-no-such-workspace"));
    }

    /// Issue #126 review, Fix 1: two workspaces at the SAME cwd share one
    /// cli_session_name (it's a pure hash of the cwd) and therefore one
    /// daemon. Closing one workspace returns the shared name in
    /// `removed_agent_browser_sessions`, but the post-close live set must
    /// STILL contain that name — that is the exact property the reap
    /// helper's skip check relies on to avoid killing the daemon the
    /// surviving workspace's pane is using.
    #[test]
    fn shared_cwd_close_keeps_session_name_in_live_set() {
        let store = AppStateStore::default();
        let shared = std::path::PathBuf::from("/tmp/ab-shared-cwd");
        let ws1_id = store.create_workspace_at_path(shared.clone());
        let ws2_id = store.create_workspace_at_path(shared);

        let s1 = store.resolve_agent_browser_session(&ws1_id.0, 9223);
        let s2 = store.resolve_agent_browser_session(&ws2_id.0, 9224);
        assert_eq!(
            s1.cli_session_name, s2.cli_session_name,
            "same cwd must derive the same stable session name"
        );

        let result = store
            .close_workspace(&ws1_id.0)
            .expect("close should succeed");
        // The closed workspace's session IS reported...
        assert_eq!(
            result.removed_agent_browser_sessions,
            vec![s1.cli_session_name.clone()]
        );
        // ...but the surviving workspace still maps to the same name, so
        // the post-close live set keeps it and the reap skips the close.
        assert!(store
            .live_agent_browser_session_names()
            .contains(&s1.cli_session_name));
    }

    #[test]
    fn stable_browser_session_name_bounds_socket_path_for_long_branches() {
        // Reproduction: a codemux worktree whose git branch name is long.
        // The worktree dir basename *is* the branch name, and before the
        // fix it landed in the session name verbatim, overflowing
        // agent-browser's Unix-domain-socket `sun_path` budget.
        let long_branch = "feature/add-comprehensive-integration-tests-for-the-agent-browser-pane";
        let cwd = format!("/home/zeus/.codemux/worktrees/codemux/{long_branch}");
        let name = stable_browser_session_name(&cwd);

        // agent-browser rejects session names outside 1..=64 chars.
        assert!(
            (1..=64).contains(&name.len()),
            "session name must fit agent-browser's 1-64 char cap, got {} chars: {name}",
            name.len()
        );

        // agent-browser builds `<socket_dir>/<session>.sock` and refuses
        // paths over ~103 bytes. Check the worst realistic socket dir
        // (`$XDG_RUNTIME_DIR/agent-browser` with a 7-digit uid) still fits.
        let socket_path = format!("/run/user/1234567/agent-browser/{name}.sock");
        assert!(
            socket_path.len() <= 103,
            "socket path must stay under the sun_path limit, got {} bytes: {socket_path}",
            socket_path.len()
        );

        assert!(name.starts_with("ws-"), "expected ws- prefix, got: {name}");
    }

    #[test]
    fn stable_browser_session_name_is_stable_and_unique() {
        // Same cwd -> same name (cookie/storage persistence across restarts).
        let cwd = "/home/zeus/.codemux/worktrees/codemux/main";
        assert_eq!(
            stable_browser_session_name(cwd),
            stable_browser_session_name(cwd)
        );

        // Two long branches sharing the first 32 chars still get distinct
        // names: the hash is taken over the *full* cwd, not the truncated
        // basename, so truncation never collapses two workspaces together.
        let a = stable_browser_session_name(
            "/home/zeus/.codemux/worktrees/codemux/feature-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-one",
        );
        let b = stable_browser_session_name(
            "/home/zeus/.codemux/worktrees/codemux/feature-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-two",
        );
        assert_ne!(a, b, "shared 32-char prefix must not collide: {a} == {b}");
    }

    #[test]
    fn agent_browser_attach_detach_cycle() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        let _session = store.resolve_agent_browser_session(&ws_id.0, 9223);

        // Create a browser pane to attach to
        let active_pane = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id.clone();
        let (pane_id, browser_id) = store
            .create_browser_pane(&active_pane.0, None)
            .unwrap();

        // Attach
        store
            .attach_agent_browser_to_pane(&ws_id.0, &pane_id, &browser_id)
            .unwrap();

        let snap = store.snapshot();
        let attached = &snap.agent_browser_sessions[0];
        assert_eq!(attached.pane_id.as_ref().unwrap(), &pane_id);
        assert_eq!(attached.browser_id.as_ref().unwrap(), &browser_id);
        assert!(attached.is_active);

        // Detach
        let detached = store
            .detach_agent_browser_from_pane(&browser_id.0, true)
            .unwrap();
        assert!(detached.pane_id.is_none());
        assert!(detached.browser_id.is_none());

        // Session still exists and is_active stays true (process still running)
        let snap = store.snapshot();
        assert_eq!(snap.agent_browser_sessions.len(), 1);
    }

    #[test]
    fn mark_agent_browser_active_flips_is_active_without_attaching_pane() {
        // Backs the GUI-mode background-browsing gate: control.rs suppresses
        // pane creation but must still be able to mark the detached session
        // live so the frontend's background chip/indicator can render it.
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        let session = store.resolve_agent_browser_session(&ws_id.0, 9223);
        assert!(!session.is_active);

        let found = store.mark_agent_browser_active(&ws_id.0);
        assert!(found);

        let snap = store.snapshot();
        let updated = &snap.agent_browser_sessions[0];
        assert!(updated.is_active);
        assert!(updated.pane_id.is_none());
        assert!(updated.browser_id.is_none());
    }

    #[test]
    fn mark_agent_browser_active_returns_false_when_no_session() {
        let store = AppStateStore::default();
        assert!(!store.mark_agent_browser_active("no-such-workspace"));
    }

    #[test]
    fn mark_agent_browser_inactive_clears_is_active_but_keeps_session() {
        // Backs the `close`-action wiring in control.rs: a successful
        // browser close must drop `is_active` so the GUI-mode background
        // chip/indicator/peek stop showing LIVE — while the session itself
        // (URL, cli_session_name) survives for a later reopen.
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        store.resolve_agent_browser_session(&ws_id.0, 9223);
        store.mark_agent_browser_active(&ws_id.0);
        store
            .update_agent_browser_url(&ws_id.0, "https://example.com".into())
            .unwrap();
        assert!(store.snapshot().agent_browser_sessions[0].is_active);

        let found = store.mark_agent_browser_inactive(&ws_id.0);
        assert!(found);

        let snap = store.snapshot();
        assert_eq!(snap.agent_browser_sessions.len(), 1);
        let updated = &snap.agent_browser_sessions[0];
        assert!(!updated.is_active);
        assert_eq!(updated.current_url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn mark_agent_browser_inactive_returns_false_when_no_session() {
        let store = AppStateStore::default();
        assert!(!store.mark_agent_browser_inactive("no-such-workspace"));
    }

    #[test]
    fn release_detached_agent_browser_flips_paneless_active_session() {
        // Backs the run-finished release in `agent_chat.rs`'s
        // `publish_pane_status`: a background (pane-less) session that is
        // active should flip inactive so the GUI chip stops showing LIVE.
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        store.resolve_agent_browser_session(&ws_id.0, 9223);
        store.mark_agent_browser_active(&ws_id.0);
        assert!(store.snapshot().agent_browser_sessions[0].is_active);

        let released = store.release_detached_agent_browser(&ws_id.0);
        assert!(released);

        let snap = store.snapshot();
        assert_eq!(snap.agent_browser_sessions.len(), 1);
        assert!(!snap.agent_browser_sessions[0].is_active);
    }

    #[test]
    fn release_detached_agent_browser_leaves_pane_attached_session_untouched() {
        // A pane-attached session (legacy split-pane mode) has its own
        // close lifecycle; the run-finished release must not touch it.
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        store.resolve_agent_browser_session(&ws_id.0, 9223);

        let active_pane = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id.clone();
        let (pane_id, browser_id) = store
            .create_browser_pane(&active_pane.0, None)
            .unwrap();
        store
            .attach_agent_browser_to_pane(&ws_id.0, &pane_id, &browser_id)
            .unwrap();
        assert!(store.snapshot().agent_browser_sessions[0].is_active);

        let released = store.release_detached_agent_browser(&ws_id.0);
        assert!(!released);

        let snap = store.snapshot();
        assert!(snap.agent_browser_sessions[0].is_active);
        assert!(snap.agent_browser_sessions[0].pane_id.is_some());
    }

    #[test]
    fn release_detached_agent_browser_returns_false_when_no_session_or_inactive() {
        let store = AppStateStore::default();
        assert!(!store.release_detached_agent_browser("no-such-workspace"));

        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        store.resolve_agent_browser_session(&ws_id.0, 9223);
        // Freshly resolved session starts inactive.
        assert!(!store.release_detached_agent_browser(&ws_id.0));
    }

    #[test]
    fn user_browser_close_destroys_session() {
        let store = AppStateStore::default();
        let active_pane = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id.clone();
        let (pane_id, _browser_id) = store
            .create_browser_pane(&active_pane.0, None)
            .unwrap();

        assert_eq!(store.snapshot().browser_sessions.len(), 1);

        // close_pane removes BrowserSessionSnapshot
        store.close_pane(&pane_id.0).unwrap();
        assert_eq!(store.snapshot().browser_sessions.len(), 0);
    }

    #[test]
    fn agent_browser_pane_close_preserves_session() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        let _session = store.resolve_agent_browser_session(&ws_id.0, 9223);

        let active_pane = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id.clone();
        let (pane_id, browser_id) = store
            .create_browser_pane(&active_pane.0, None)
            .unwrap();

        store
            .attach_agent_browser_to_pane(&ws_id.0, &pane_id, &browser_id)
            .unwrap();

        // Detach before close (simulates what close_pane command does)
        store.detach_agent_browser_from_pane(&browser_id.0, true);

        // close_pane removes BrowserSessionSnapshot but agent session survives
        store.close_pane(&pane_id.0).unwrap();

        let snap = store.snapshot();
        assert_eq!(snap.browser_sessions.len(), 0);
        assert_eq!(snap.agent_browser_sessions.len(), 1);
        assert!(snap.agent_browser_sessions[0].pane_id.is_none());
    }

    #[test]
    fn workspace_close_removes_agent_session() {
        let store = AppStateStore::default();
        let ws1_id = store.snapshot().workspaces[0].workspace_id.clone();
        let ws2_id = store.create_workspace_at_path(std::path::PathBuf::from("/tmp"));

        store.resolve_agent_browser_session(&ws1_id.0, 9223);
        store.resolve_agent_browser_session(&ws2_id.0, 9223);
        assert_eq!(store.snapshot().agent_browser_sessions.len(), 2);

        store.close_workspace(&ws2_id.0).unwrap();

        let snap = store.snapshot();
        assert_eq!(snap.agent_browser_sessions.len(), 1);
        assert_eq!(snap.agent_browser_sessions[0].workspace_id, ws1_id);
    }

    #[test]
    fn find_detached_finds_correct_workspace() {
        let store = AppStateStore::default();
        let ws1_id = store.snapshot().workspaces[0].workspace_id.clone();
        let ws2_id = store.create_workspace_at_path(std::path::PathBuf::from("/tmp"));

        let _s1 = store.resolve_agent_browser_session(&ws1_id.0, 9223);
        let _s2 = store.resolve_agent_browser_session(&ws2_id.0, 9223);

        // Create and attach browser panes in both workspaces
        let pane1 = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id.clone();
        let (pane_id1, browser_id1) = store
            .create_browser_pane(&pane1.0, None)
            .unwrap();
        store
            .attach_agent_browser_to_pane(&ws1_id.0, &pane_id1, &browser_id1)
            .unwrap();

        let pane2 = store.snapshot().workspaces[1].surfaces[0]
            .active_pane_id.clone();
        let (pane_id2, browser_id2) = store
            .create_browser_pane(&pane2.0, None)
            .unwrap();
        store
            .attach_agent_browser_to_pane(&ws2_id.0, &pane_id2, &browser_id2)
            .unwrap();

        // Detach ws1 only
        store.detach_agent_browser_from_pane(&browser_id1.0, true);

        assert!(store.find_detached_agent_browser(&ws1_id.0).is_some());
        assert!(store.find_detached_agent_browser(&ws2_id.0).is_none());
    }

    // ── Dismissed vs non-dismissed detach tests ──

    #[test]
    fn detach_dismissed_true_sets_user_dismissed() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        store.resolve_agent_browser_session(&ws_id.0, 9223);

        let active_pane = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id.clone();
        let (pane_id, browser_id) = store
            .create_browser_pane(&active_pane.0, None)
            .unwrap();
        store
            .attach_agent_browser_to_pane(&ws_id.0, &pane_id, &browser_id)
            .unwrap();

        store.detach_agent_browser_from_pane(&browser_id.0, true);

        let snap = store.snapshot();
        assert!(snap.agent_browser_sessions[0].user_dismissed);
        assert!(snap.agent_browser_sessions[0].pane_id.is_none());
    }

    #[test]
    fn detach_dismissed_false_does_not_set_user_dismissed() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        store.resolve_agent_browser_session(&ws_id.0, 9223);

        let active_pane = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id.clone();
        let (pane_id, browser_id) = store
            .create_browser_pane(&active_pane.0, None)
            .unwrap();
        store
            .attach_agent_browser_to_pane(&ws_id.0, &pane_id, &browser_id)
            .unwrap();

        store.detach_agent_browser_from_pane(&browser_id.0, false);

        let snap = store.snapshot();
        assert!(!snap.agent_browser_sessions[0].user_dismissed);
        assert!(snap.agent_browser_sessions[0].pane_id.is_none());
    }

    #[test]
    fn after_non_dismissed_detach_pane_can_be_auto_created() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        store.resolve_agent_browser_session(&ws_id.0, 9223);

        let active_pane = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id.clone();
        let (pane_id, browser_id) = store
            .create_browser_pane(&active_pane.0, None)
            .unwrap();
        store
            .attach_agent_browser_to_pane(&ws_id.0, &pane_id, &browser_id)
            .unwrap();

        // Tab close: dismissed=false
        store.detach_agent_browser_from_pane(&browser_id.0, false);

        let session = store.resolve_agent_browser_session(&ws_id.0, 9223);
        let should_create = session.pane_id.is_none() && !session.user_dismissed;
        assert!(should_create, "Pane should be auto-creatable after non-dismissed detach");
    }

    #[test]
    fn after_dismissed_detach_pane_cannot_be_auto_created() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().workspaces[0].workspace_id.clone();
        store.resolve_agent_browser_session(&ws_id.0, 9223);

        let active_pane = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id.clone();
        let (pane_id, browser_id) = store
            .create_browser_pane(&active_pane.0, None)
            .unwrap();
        store
            .attach_agent_browser_to_pane(&ws_id.0, &pane_id, &browser_id)
            .unwrap();

        // User close: dismissed=true
        store.detach_agent_browser_from_pane(&browser_id.0, true);

        let session = store.resolve_agent_browser_session(&ws_id.0, 9223);
        let should_create = session.pane_id.is_none() && !session.user_dismissed;
        assert!(!should_create, "Pane should NOT be auto-creatable after dismissed detach");
    }

    #[test]
    fn preset_workspace_reuses_default_tab() {
        let store = AppStateStore::default();
        let ws_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux"),
            WorkspacePresetLayout::Single,
        );

        // Workspace starts with exactly 1 default "Terminal" tab.
        let snap = store.snapshot();
        let ws = workspace_by_id(&snap, &ws_id);
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.tabs[0].title, "Terminal");
        assert!(ws.tabs[0].icon.is_none());

        // --- Single-command preset: reuse the default tab ---
        let default_tab_id = ws.tabs[0].tab_id.clone();
        let default_session = collect_terminal_sessions(&ws.surfaces)
            .into_iter()
            .next()
            .expect("default tab should have a terminal session");
        drop(snap);

        // Rename + set icon on the existing tab (mirrors workspace.rs reuse path).
        store
            .rename_tab(&ws_id.0, &default_tab_id, "Claude Code".into())
            .unwrap();
        store
            .set_tab_icon(&ws_id.0, &default_tab_id, Some("claude".into()))
            .unwrap();

        let snap = store.snapshot();
        let ws = workspace_by_id(&snap, &ws_id);
        assert_eq!(ws.tabs.len(), 1, "single-command preset should reuse the default tab");
        assert_eq!(ws.tabs[0].title, "Claude Code");
        assert_eq!(ws.tabs[0].icon.as_deref(), Some("claude"));

        // Session ID should be unchanged.
        let session_after = collect_terminal_sessions(&ws.surfaces)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(default_session, session_after, "session ID must be stable after rename");
        drop(snap);

        // --- Multi-command preset: second command creates a new tab ---
        let (new_tab_id, new_session) = store
            .create_tab(&ws_id.0, TabKind::Terminal)
            .expect("create_tab for second command should succeed");
        store
            .rename_tab(&ws_id.0, &new_tab_id, "Claude Code".into())
            .unwrap();
        store
            .set_tab_icon(&ws_id.0, &new_tab_id, Some("claude".into()))
            .unwrap();

        let snap = store.snapshot();
        let ws = workspace_by_id(&snap, &ws_id);
        assert_eq!(ws.tabs.len(), 2, "two-command preset should produce exactly 2 tabs");
        assert_ne!(
            ws.tabs[0].tab_id, ws.tabs[1].tab_id,
            "tabs should have distinct IDs"
        );
        assert!(new_session.is_some(), "new tab should have its own session");
        assert_ne!(
            default_session,
            new_session.unwrap().0,
            "new tab session must differ from the reused default"
        );
    }

    /// `PaneNodeSnapshot::AgentChat` must round-trip through serde
    /// cleanly so persisted layouts restore chat panes after a
    /// restart.
    #[test]
    fn agent_chat_pane_serde_round_trips() {
        use crate::agent_provider::ProviderKind;

        let node = PaneNodeSnapshot::AgentChat {
            pane_id: PaneId("pane-42".into()),
            title: "Agent Chat".into(),
            thread_id: Some("thread-abc".into()),
            provider: Some(ProviderKind::Claude),
            cwd: Some("/tmp/project".into()),
        };
        let json = serde_json::to_string(&node).expect("serialize agent_chat pane");
        let round: PaneNodeSnapshot =
            serde_json::from_str(&json).expect("deserialize agent_chat pane");
        match round {
            PaneNodeSnapshot::AgentChat {
                pane_id,
                title,
                thread_id,
                provider,
                cwd,
            } => {
                assert_eq!(pane_id.0, "pane-42");
                assert_eq!(title, "Agent Chat");
                assert_eq!(thread_id, Some("thread-abc".to_string()));
                assert_eq!(provider, Some(ProviderKind::Claude));
                assert_eq!(cwd, Some("/tmp/project".to_string()));
            }
            other => panic!("expected AgentChat variant, got {other:?}"),
        }
    }

    /// Step 12 Stage 4 — chat panes must round-trip through serde for
    /// all three provider variants, not just Claude. The picker UI now
    /// emits `Some(Codex)` and `Some(OpenCode)` from the rail click
    /// handler, so persisting and rehydrating those values must keep
    /// the variant intact.
    #[test]
    fn agent_chat_pane_serde_round_trips_for_every_provider_variant() {
        use crate::agent_provider::ProviderKind;
        for kind in [
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::OpenCode,
        ] {
            let node = PaneNodeSnapshot::AgentChat {
                pane_id: PaneId("pane-1".into()),
                title: "Agent Chat".into(),
                thread_id: None,
                provider: Some(kind),
                cwd: None,
            };
            let json = serde_json::to_string(&node).expect("serialize");
            let round: PaneNodeSnapshot =
                serde_json::from_str(&json).expect("deserialize");
            match round {
                PaneNodeSnapshot::AgentChat { provider, .. } => {
                    assert_eq!(
                        provider,
                        Some(kind),
                        "provider variant {kind:?} must survive serde round-trip"
                    );
                }
                other => panic!("expected AgentChat variant, got {other:?}"),
            }
        }
    }

    /// Unset optional fields must survive the round-trip without being
    /// re-populated. `thread_id`, `provider`, and `cwd` are all
    /// `#[serde(default)]` so `None` serializes to `null`.
    #[test]
    fn agent_chat_pane_defaults_round_trip() {
        let node = PaneNodeSnapshot::AgentChat {
            pane_id: PaneId("pane-99".into()),
            title: "Agent Chat".into(),
            thread_id: None,
            provider: None,
            cwd: None,
        };
        let json = serde_json::to_string(&node).unwrap();
        let round: PaneNodeSnapshot = serde_json::from_str(&json).unwrap();
        match round {
            PaneNodeSnapshot::AgentChat {
                thread_id,
                provider,
                cwd,
                ..
            } => {
                assert!(thread_id.is_none());
                assert!(provider.is_none());
                assert!(cwd.is_none());
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    /// Legacy `AgentChat` payloads without the optional fields (e.g.
    /// produced by an older binary) should deserialize, with the
    /// missing fields defaulting to `None`.
    #[test]
    fn agent_chat_pane_deserializes_without_optional_fields() {
        let json = r#"{"kind":"agent_chat","pane_id":"pane-7","title":"Agent Chat"}"#;
        let round: PaneNodeSnapshot = serde_json::from_str(json).unwrap();
        match round {
            PaneNodeSnapshot::AgentChat {
                pane_id,
                title,
                thread_id,
                provider,
                cwd,
            } => {
                assert_eq!(pane_id.0, "pane-7");
                assert_eq!(title, "Agent Chat");
                assert!(thread_id.is_none());
                assert!(provider.is_none());
                assert!(cwd.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn create_agent_chat_pane_inserts_into_active_surface() {
        let store = AppStateStore::default();
        let snapshot = store.snapshot();
        let workspace_id = snapshot.active_workspace_id.clone();

        let pane_id = store
            .create_agent_chat_pane(&workspace_id.0, None, None, None)
            .expect("create_agent_chat_pane should succeed");

        let after = store.snapshot();
        let workspace = workspace_by_id(&after, &workspace_id);
        let surface = &workspace.surfaces[0];
        assert_eq!(surface.active_pane_id, pane_id);
        assert!(pane_tree_contains_pane(&surface.root, &pane_id.0));
    }

    #[test]
    fn create_agent_chat_pane_on_missing_workspace_errors() {
        let store = AppStateStore::default();
        let err = store
            .create_agent_chat_pane("does-not-exist", None, None, None)
            .unwrap_err();
        assert!(err.contains("does-not-exist"));
    }

    #[test]
    fn set_agent_chat_thread_id_binds_chat_pane() {
        let store = AppStateStore::default();
        let snapshot = store.snapshot();
        let workspace_id = snapshot.active_workspace_id.clone();
        let pane_id = store
            .create_agent_chat_pane(&workspace_id.0, None, None, None)
            .unwrap();

        assert!(store.agent_chat_thread_id(&pane_id.0).is_none());
        let ok = store.set_agent_chat_thread_id(&pane_id.0, Some("thread-xyz".into()));
        assert!(ok);
        assert_eq!(
            store.agent_chat_thread_id(&pane_id.0),
            Some("thread-xyz".into())
        );
    }

    #[test]
    fn set_agent_chat_binding_updates_provider_and_thread_together() {
        use crate::agent_provider::ProviderKind;

        let store = AppStateStore::default();
        let workspace_id = store.snapshot().active_workspace_id.clone();
        let pane_id = store
            .create_agent_chat_pane(
                &workspace_id.0,
                Some(ProviderKind::Claude),
                None,
                None,
            )
            .unwrap();
        store.set_agent_chat_thread_id(&pane_id.0, Some("thread-claude".into()));

        assert!(store.set_agent_chat_binding(
            &pane_id.0,
            ProviderKind::Codex,
            "thread-codex".into(),
        ));
        assert_eq!(
            store.agent_chat_pane_thread(&pane_id.0),
            Some((ProviderKind::Codex, "thread-codex".to_string()))
        );
    }

    /// Regression guard for the agent-chat sidecar leak fix.
    ///
    /// Closing a workspace must surface every bound `(provider, thread_id)`
    /// pair so the command layer can call `provider.stop_session` for
    /// each. Without this list the JSON-RPC sidecar children stayed
    /// alive and their background tokio tasks held `Arc<Session>` in a
    /// refcycle with the session's own `JoinHandle` vec, so `Drop`
    /// never fired and every closed worktree leaked one sidecar plus
    /// its task graph until the whole app was killed.
    #[test]
    fn close_workspace_result_contains_agent_chat_threads() {
        use crate::agent_provider::ProviderKind;

        let store = AppStateStore::default();
        let ws_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/project-agent-chat-close"),
            WorkspacePresetLayout::Single,
        );
        store.activate_workspace(&ws_id.0);

        // Bind two chat panes: one with a live session, one still unbound.
        // Only the live one should be returned for cleanup.
        let pane_a = store
            .create_agent_chat_pane(&ws_id.0, Some(ProviderKind::Claude), None, None)
            .unwrap();
        store.set_agent_chat_thread_id(&pane_a.0, Some("thread-live".into()));

        let _pane_unbound = store
            .create_agent_chat_pane(&ws_id.0, Some(ProviderKind::Codex), None, None)
            .unwrap();

        let result = store
            .close_workspace(&ws_id.0)
            .expect("close_workspace should succeed");

        assert_eq!(
            result.removed_agent_chat_threads,
            vec![(ProviderKind::Claude, "thread-live".to_string())],
            "close_workspace must surface only chat panes with bound threads",
        );
    }

    /// Regression guard mirroring the workspace-close cleanup for `close_pane`.
    /// Closing a single agent-chat pane must let the command layer reach
    /// the bound session before the pane disappears from the tree.
    #[test]
    fn agent_chat_pane_thread_returns_bound_pair() {
        use crate::agent_provider::ProviderKind;

        let store = AppStateStore::default();
        let ws_id = store.snapshot().active_workspace_id.clone();
        let pane_id = store
            .create_agent_chat_pane(&ws_id.0, Some(ProviderKind::Claude), None, None)
            .unwrap();

        // Unbound pane returns None — nothing to tear down.
        assert_eq!(store.agent_chat_pane_thread(&pane_id.0), None);

        store.set_agent_chat_thread_id(&pane_id.0, Some("thread-zzz".into()));
        assert_eq!(
            store.agent_chat_pane_thread(&pane_id.0),
            Some((ProviderKind::Claude, "thread-zzz".to_string()))
        );

        // Unknown pane id is a clean None, not a panic.
        assert_eq!(store.agent_chat_pane_thread("does-not-exist"), None);
    }

    #[test]
    fn set_pane_status_by_thread_resolves_and_reports_changes() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().active_workspace_id.clone();
        let pane_id = store
            .create_agent_chat_pane(&ws_id.0, None, None, None)
            .unwrap();

        // Unbound pane cannot be resolved by thread id yet.
        assert!(!store.set_pane_status_by_thread("thread-1", PaneStatus::Working));
        assert!(store.snapshot().pane_statuses.is_empty());

        store.set_agent_chat_thread_id(&pane_id.0, Some("thread-1".into()));

        // First transition writes and reports "changed".
        assert!(store.set_pane_status_by_thread("thread-1", PaneStatus::Working));
        assert_eq!(
            store.snapshot().pane_statuses.get(&pane_id.0),
            Some(&PaneStatus::Working)
        );

        // Re-writing the same status is a no-op (drives the change-guard
        // that keeps the content_delta stream from spamming emits).
        assert!(!store.set_pane_status_by_thread("thread-1", PaneStatus::Working));

        // A real change writes again.
        assert!(store.set_pane_status_by_thread("thread-1", PaneStatus::Review));
        assert_eq!(
            store.snapshot().pane_statuses.get(&pane_id.0),
            Some(&PaneStatus::Review)
        );

        // Idle removes the entry (and reports the removal as a change).
        assert!(store.set_pane_status_by_thread("thread-1", PaneStatus::Idle));
        assert!(store.snapshot().pane_statuses.is_empty());
        // Removing a missing entry is not a change.
        assert!(!store.set_pane_status_by_thread("thread-1", PaneStatus::Idle));

        // Unknown thread never resolves.
        assert!(!store.set_pane_status_by_thread("does-not-exist", PaneStatus::Working));
    }

    /// The sidebar's "Done · review" checkmark is `PaneStatus::Review`, and it
    /// has to still be there tomorrow morning. Persisting it is the whole point
    /// of the filter — the map used to be cleared wholesale, so every finished
    /// workspace came back from a restart looking already-reviewed.
    #[test]
    fn review_status_survives_persistence_but_live_process_statuses_do_not() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().active_workspace_id.clone();
        let reviewed = store
            .create_agent_chat_pane(&ws_id.0, None, None, None)
            .unwrap();
        let working = store
            .create_agent_chat_pane(&ws_id.0, None, None, None)
            .unwrap();
        let prompting = store
            .create_agent_chat_pane(&ws_id.0, None, None, None)
            .unwrap();

        store.set_pane_status(&reviewed.0, PaneStatus::Review);
        store.set_pane_status(&working.0, PaneStatus::Working);
        store.set_pane_status(&prompting.0, PaneStatus::Permission);

        let mut snapshot = store.snapshot();
        retain_persistable_pane_statuses(&mut snapshot);

        assert_eq!(
            snapshot.pane_statuses.get(&reviewed.0),
            Some(&PaneStatus::Review),
            "a finished agent's review checkmark must survive a restart",
        );
        // Working/Permission describe a process that is dead after the quit —
        // restoring them would render a spinner nothing is driving.
        assert_eq!(snapshot.pane_statuses.get(&working.0), None);
        assert_eq!(snapshot.pane_statuses.get(&prompting.0), None);
    }

    /// Nothing removes a status when its pane goes away, so the filter has to
    /// drop orphans itself — otherwise the map grows without bound now that it
    /// is written to disk instead of cleared.
    #[test]
    fn persisted_pane_statuses_drop_entries_whose_pane_is_gone() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().active_workspace_id.clone();
        let pane = store
            .create_agent_chat_pane(&ws_id.0, None, None, None)
            .unwrap();
        store.set_pane_status(&pane.0, PaneStatus::Review);
        store.set_pane_status("pane-that-never-existed", PaneStatus::Review);

        let mut snapshot = store.snapshot();
        retain_persistable_pane_statuses(&mut snapshot);

        assert_eq!(
            snapshot.pane_statuses.get(&pane.0),
            Some(&PaneStatus::Review)
        );
        assert_eq!(snapshot.pane_statuses.get("pane-that-never-existed"), None);
    }

    #[test]
    fn is_thread_pane_in_active_workspace_tracks_focus() {
        let store = AppStateStore::default();
        let active_ws = store.snapshot().active_workspace_id.clone();
        let active_pane = store
            .create_agent_chat_pane(&active_ws.0, None, None, None)
            .unwrap();
        store.set_agent_chat_thread_id(&active_pane.0, Some("thread-active".into()));

        // A second workspace that is NOT the active one.
        let other_ws = store.create_workspace_with_layout(
            PathBuf::from("/tmp/codemux-thread-focus"),
            WorkspacePresetLayout::Single,
        );
        let other_pane = store
            .create_agent_chat_pane(&other_ws.0, None, None, None)
            .unwrap();
        store.set_agent_chat_thread_id(&other_pane.0, Some("thread-other".into()));

        // create_agent_chat_pane activates the workspace it inserts into, so
        // pin focus back to the first workspace for a deterministic check.
        store.activate_workspace(&active_ws.0);

        assert!(store.is_thread_pane_in_active_workspace("thread-active"));
        assert!(!store.is_thread_pane_in_active_workspace("thread-other"));
        assert!(!store.is_thread_pane_in_active_workspace("nope"));

        store.activate_workspace(&other_ws.0);
        assert!(!store.is_thread_pane_in_active_workspace("thread-active"));
        assert!(store.is_thread_pane_in_active_workspace("thread-other"));
    }

    #[test]
    fn find_agent_chat_pane_id_walks_splits_and_skips_unbound() {
        let store = AppStateStore::default();
        let ws_id = store.snapshot().active_workspace_id.clone();
        // Two chat panes in the same surface (the second splits the first).
        let pane_a = store
            .create_agent_chat_pane(&ws_id.0, None, None, None)
            .unwrap();
        let pane_b = store
            .create_agent_chat_pane(&ws_id.0, None, None, None)
            .unwrap();
        store.set_agent_chat_thread_id(&pane_a.0, Some("thread-a".into()));
        store.set_agent_chat_thread_id(&pane_b.0, Some("thread-b".into()));

        let snapshot = store.snapshot();
        let workspace = workspace_by_id(&snapshot, &ws_id);
        let root = &workspace.surfaces[0].root;

        assert_eq!(find_agent_chat_pane_id(root, "thread-a"), Some(pane_a));
        assert_eq!(find_agent_chat_pane_id(root, "thread-b"), Some(pane_b.clone()));
        // Unbound / unknown thread ids never match.
        assert_eq!(find_agent_chat_pane_id(root, "thread-c"), None);

        // Clearing a binding drops it from the walk.
        store.set_agent_chat_thread_id(&pane_b.0, None);
        let cleared = store.snapshot();
        let workspace = workspace_by_id(&cleared, &ws_id);
        assert_eq!(
            find_agent_chat_pane_id(&workspace.surfaces[0].root, "thread-b"),
            None
        );
    }

    #[test]
    fn workspace_type_serde_roundtrip_all_variants() {
        for variant in [
            WorkspaceType::Standard,
            WorkspaceType::OpenFlow,
            WorkspaceType::Home,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: WorkspaceType = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back, "roundtrip failed for {variant:?} ({json})");
        }
        assert_eq!(
            serde_json::to_string(&WorkspaceType::Home).unwrap(),
            "\"home\""
        );
    }

    #[test]
    fn find_home_workspace_id_returns_none_when_absent() {
        let store = AppStateStore::default();
        assert!(store.find_home_workspace_id().is_none());
    }

    #[test]
    fn set_workspace_type_tags_home_and_find_returns_it() {
        let store = AppStateStore::default();
        let ws_id = store.create_empty_workspace_at_path(PathBuf::from("/tmp/home"));
        assert!(store.set_workspace_type(&ws_id.0, WorkspaceType::Home));
        assert_eq!(store.find_home_workspace_id(), Some(ws_id.0.clone()));
    }

    #[test]
    fn get_or_create_home_workspace_is_idempotent() {
        // Models the command body's state-level logic: if a Home workspace
        // exists, return it; otherwise create + tag.
        let store = AppStateStore::default();

        // First call: none exists → create and tag.
        let first = store.find_home_workspace_id().unwrap_or_else(|| {
            let id = store.create_empty_workspace_at_path(PathBuf::from("/tmp/home"));
            store.set_workspace_type(&id.0, WorkspaceType::Home);
            id.0
        });

        // Second call: the tagged workspace is returned unchanged.
        let second = store.find_home_workspace_id().unwrap_or_else(|| {
            let id = store.create_empty_workspace_at_path(PathBuf::from("/tmp/home"));
            store.set_workspace_type(&id.0, WorkspaceType::Home);
            id.0
        });
        assert_eq!(first, second, "repeat call must return the same Home id");

        // Simulate a hard-delete of the Home workspace.
        {
            let mut snap = store.inner.lock().unwrap();
            snap.workspaces.retain(|w| w.workspace_id.0 != first);
        }
        assert!(store.find_home_workspace_id().is_none());

        // Post-delete: next call creates a fresh Home with a new id.
        let third = store.find_home_workspace_id().unwrap_or_else(|| {
            let id = store.create_empty_workspace_at_path(PathBuf::from("/tmp/home"));
            store.set_workspace_type(&id.0, WorkspaceType::Home);
            id.0
        });
        assert_ne!(third, first, "post-deletion call must create a fresh id");
        assert_eq!(store.find_home_workspace_id(), Some(third));
    }
}

#[cfg(test)]
mod archived_workspace_tests {
    //! Coverage for the workspace-archive state layer: entry
    //! add/dedupe/remove semantics and — critically — snapshot serde
    //! backward compatibility, since `archived_workspaces` piggybacks on
    //! the persisted layout.json and MUST NOT break loading of files
    //! written before the field existed.

    use super::*;

    fn sample_entry(cwd: &str, worktree_path: Option<&str>) -> ArchivedWorkspaceSnapshot {
        ArchivedWorkspaceSnapshot {
            archive_id: uuid::Uuid::new_v4().to_string(),
            workspace_id: "workspace-1".into(),
            title: "My branch".into(),
            cwd: cwd.to_string(),
            worktree_path: worktree_path.map(str::to_string),
            project_root: Some("/tmp/repo".into()),
            project_uid: None,
            workspace_kind: Some(if worktree_path.is_some() {
                "worktree".into()
            } else {
                "main".into()
            }),
            git_branch: Some("feature/x".into()),
            protected: false,
            is_git: true,
            archived_at: 1_700_000_000,
        }
    }

    #[test]
    fn add_archived_workspace_appends_entry() {
        let store = AppStateStore::default();
        let entry = sample_entry("/tmp/repo/wt", Some("/tmp/repo/wt"));
        let archive_id = entry.archive_id.clone();
        store.add_archived_workspace(entry);

        let snap = store.snapshot();
        assert_eq!(snap.archived_workspaces.len(), 1);
        assert_eq!(snap.archived_workspaces[0].archive_id, archive_id);
    }

    #[test]
    fn add_archived_workspace_dedupes_on_location() {
        // Re-archiving the same on-disk location must replace the stale
        // entry, not accumulate ghosts — the dedupe key is the
        // (cwd, worktree_path) pair, NOT the workspace id (which changes
        // on every archive/restore cycle).
        let store = AppStateStore::default();
        let first = sample_entry("/tmp/repo/wt", Some("/tmp/repo/wt"));
        let second = sample_entry("/tmp/repo/wt", Some("/tmp/repo/wt"));
        let second_id = second.archive_id.clone();
        store.add_archived_workspace(first);
        store.add_archived_workspace(second);

        let snap = store.snapshot();
        assert_eq!(
            snap.archived_workspaces.len(),
            1,
            "same-location entries must dedupe to the newest"
        );
        assert_eq!(snap.archived_workspaces[0].archive_id, second_id);
    }

    #[test]
    fn add_archived_workspace_keeps_distinct_locations() {
        // A root-checkout entry (worktree_path=None) and a worktree
        // entry that happens to share the cwd string are DIFFERENT
        // locations — the pair key must keep both.
        let store = AppStateStore::default();
        store.add_archived_workspace(sample_entry("/tmp/repo", None));
        store.add_archived_workspace(sample_entry("/tmp/repo", Some("/tmp/repo/wt")));
        store.add_archived_workspace(sample_entry("/tmp/other", None));

        assert_eq!(store.snapshot().archived_workspaces.len(), 3);
    }

    #[test]
    fn remove_archived_workspace_returns_entry_and_errors_on_unknown() {
        let store = AppStateStore::default();
        let entry = sample_entry("/tmp/repo/wt", Some("/tmp/repo/wt"));
        let archive_id = entry.archive_id.clone();
        store.add_archived_workspace(entry);

        let removed = store
            .remove_archived_workspace(&archive_id)
            .expect("existing entry must remove");
        assert_eq!(removed.archive_id, archive_id);
        assert!(store.snapshot().archived_workspaces.is_empty());

        let err = store
            .remove_archived_workspace(&archive_id)
            .expect_err("second removal must error");
        assert!(
            err.contains("No archived workspace"),
            "unexpected error string: {err}"
        );
    }

    #[test]
    fn build_archive_entry_copies_workspace_metadata() {
        let store = AppStateStore::default();
        let ws_id = store.create_workspace_at_path(PathBuf::from("/tmp/archive-build"));
        let snapshot = store.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == ws_id)
            .expect("workspace exists");

        let entry = build_archive_entry(ws);
        assert!(!entry.archive_id.is_empty(), "archive_id must be generated");
        assert_ne!(
            entry.archive_id, entry.workspace_id,
            "archive identity must be distinct from the workspace id"
        );
        assert_eq!(entry.workspace_id, ws.workspace_id.0);
        assert_eq!(entry.title, ws.title);
        assert_eq!(entry.cwd, ws.cwd);
        assert_eq!(entry.worktree_path, ws.worktree_path);
        assert_eq!(entry.protected, ws.protected);
        assert!(entry.archived_at > 0, "archived_at must be a real timestamp");
    }

    #[test]
    fn snapshot_without_archived_workspaces_field_still_deserializes() {
        // Backward compat: layout.json files written before the archive
        // feature have no `archived_workspaces` key. Simulate one by
        // serializing a current snapshot and deleting the key.
        let snapshot = default_app_state();
        let mut value = serde_json::to_value(&snapshot).expect("serialize snapshot");
        let map = value.as_object_mut().expect("snapshot serializes to object");
        assert!(
            map.remove("archived_workspaces").is_some(),
            "current snapshots must serialize the field (so it persists)"
        );

        let restored: AppStateSnapshot =
            serde_json::from_value(value).expect("old-shape snapshot must deserialize");
        assert!(
            restored.archived_workspaces.is_empty(),
            "missing field must default to an empty archive list"
        );
    }

    #[test]
    fn archived_entries_round_trip_through_snapshot_serde() {
        // The persistence path serializes the WHOLE snapshot; entries
        // must survive the round trip byte-for-byte in the fields that
        // matter for restore.
        let store = AppStateStore::default();
        store.add_archived_workspace(sample_entry("/tmp/repo/wt", Some("/tmp/repo/wt")));
        let snapshot = store.snapshot();

        let json = serde_json::to_string(&snapshot).expect("serialize");
        let restored: AppStateSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.archived_workspaces.len(), 1);
        let entry = &restored.archived_workspaces[0];
        assert_eq!(entry.cwd, "/tmp/repo/wt");
        assert_eq!(entry.worktree_path.as_deref(), Some("/tmp/repo/wt"));
        assert_eq!(entry.git_branch.as_deref(), Some("feature/x"));
        assert_eq!(entry.archived_at, 1_700_000_000);
    }
}

/// Coverage for the backend-owned workspace activity stamps
/// (`last_active_at` / `last_visited_at`) that the sidebar's idle sweep
/// reads: who stamps them, that they persist, and that the boot backfill
/// dates old workspaces without ever inventing a timestamp.
#[cfg(test)]
mod workspace_activity_tests {
    use super::*;

    /// Initialize a repo whose single commit is dated `unix_seconds`, so a
    /// test can tell a commit-derived stamp apart from the directory mtime
    /// (which is unavoidably "now").
    fn init_repo_committed_at(dir: &Path, unix_seconds: i64) {
        let date = format!("{unix_seconds} +0000");
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date)
                .output()
                .expect("git is available in tests");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init"]);
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@test.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "seed",
        ]);
    }

    /// The active workspace of a freshly built default store, cloned out
    /// so assertions don't hold the state lock.
    fn active_workspace_of(store: &AppStateStore) -> WorkspaceSnapshot {
        let snapshot = store.snapshot();
        snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == snapshot.active_workspace_id)
            .cloned()
            .expect("the default store always has an active workspace")
    }

    /// A store holding two workspaces with the first one active, so a
    /// switch has both an incoming and an outgoing side. Every creation
    /// path activates what it creates, hence the explicit switch back.
    fn store_with_two_workspaces() -> (AppStateStore, WorkspaceId, WorkspaceId) {
        let store = AppStateStore::default();
        let first = store.snapshot().active_workspace_id.clone();
        let second = store.create_empty_workspace_at_path(PathBuf::from("/tmp"));
        assert!(store.activate_workspace(&first.0));
        (store, first, second)
    }

    /// Wipe every visit stamp so an assertion can only pass on a stamp the
    /// call under test wrote.
    fn clear_visit_stamps(store: &AppStateStore) {
        let mut snapshot = store.snapshot();
        for workspace in snapshot.workspaces.iter_mut() {
            workspace.last_visited_at = None;
        }
        store.replace_snapshot(snapshot);
    }

    /// Point the default store's single workspace at `cwd` with no
    /// activity stamp — the shape a workspace persisted before the field
    /// existed loads in.
    fn store_with_unstamped_workspace(cwd: &str) -> AppStateStore {
        let store = AppStateStore::default();
        let mut snapshot = store.snapshot();
        snapshot.workspaces[0].cwd = cwd.to_string();
        snapshot.workspaces[0].worktree_path = None;
        snapshot.workspaces[0].project_root = None;
        snapshot.workspaces[0].last_active_at = None;
        store.replace_snapshot(snapshot);
        store
    }

    #[test]
    fn non_idle_pane_status_stamps_workspace_activity() {
        let store = AppStateStore::default();
        let pane_id = store.snapshot().workspaces[0].surfaces[0]
            .active_pane_id
            .clone();

        // Start from "never active" so the assertion cannot pass on the
        // create-time stamp.
        let mut snapshot = store.snapshot();
        snapshot.workspaces[0].last_active_at = None;
        store.replace_snapshot(snapshot);

        // Idle is the absence of work — it must not stamp.
        store.set_pane_status(&pane_id.0, PaneStatus::Idle);
        assert_eq!(active_workspace_of(&store).last_active_at, None);

        let before = current_time_ms_signed();
        store.set_pane_status(&pane_id.0, PaneStatus::Working);
        let stamped = active_workspace_of(&store)
            .last_active_at
            .expect("working must stamp the owning workspace");
        assert!(stamped >= before);

        // Permission and Review are equally "the agent did something".
        for status in [PaneStatus::Permission, PaneStatus::Review] {
            let mut snapshot = store.snapshot();
            snapshot.workspaces[0].last_active_at = None;
            store.replace_snapshot(snapshot);
            store.set_pane_status(&pane_id.0, status.clone());
            assert!(
                active_workspace_of(&store).last_active_at.is_some(),
                "{status:?} must stamp activity"
            );
        }
    }

    #[test]
    fn thread_status_transition_stamps_workspace_activity() {
        let store = AppStateStore::default();
        let workspace_id = store.snapshot().active_workspace_id.clone();
        let pane_id = store
            .create_agent_chat_pane(&workspace_id.0, None, None, None)
            .unwrap();
        store.set_agent_chat_thread_id(&pane_id.0, Some("thread-activity".into()));

        let mut snapshot = store.snapshot();
        snapshot.workspaces[0].last_active_at = None;
        store.replace_snapshot(snapshot);

        assert!(store.set_pane_status_by_thread("thread-activity", PaneStatus::Working));
        assert!(
            active_workspace_of(&store).last_active_at.is_some(),
            "a chat pane going to work must stamp its workspace"
        );
    }

    #[test]
    fn activating_a_workspace_stamps_last_visited() {
        let store = AppStateStore::default();
        let workspace_id = store.snapshot().active_workspace_id.clone();

        let mut snapshot = store.snapshot();
        snapshot.workspaces[0].last_visited_at = None;
        snapshot.workspaces[0].last_active_at = None;
        store.replace_snapshot(snapshot);

        let before = current_time_ms_signed();
        assert!(store.activate_workspace(&workspace_id.0));

        let workspace = active_workspace_of(&store);
        assert!(
            workspace.last_visited_at.expect("visit must stamp") >= before,
            "activating must record the visit"
        );
        assert_eq!(
            workspace.last_active_at, None,
            "looking at a workspace is not the agent working in it"
        );
    }

    #[test]
    fn switching_away_stamps_the_outgoing_workspace_as_visited() {
        // The workspace you just watched must not turn unread the instant
        // you leave it: the pane writers stamp `last_active_at` while it is
        // focused, so the visit has to be extended to the moment of leaving.
        let (store, first, second) = store_with_two_workspaces();
        clear_visit_stamps(&store);

        let before = current_time_ms_signed();
        assert!(store.activate_workspace(&second.0));

        let snapshot = store.snapshot();
        let outgoing = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == first)
            .expect("the workspace we switched away from");
        assert!(
            outgoing.last_visited_at.expect("leaving must stamp") >= before,
            "switching away has to close out the visit on the workspace being left"
        );
        let incoming = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == second)
            .expect("the workspace we switched to");
        assert!(incoming.last_visited_at.is_some());
    }

    #[test]
    fn reactivating_the_open_workspace_stamps_it_once_and_leaves_others_alone() {
        let (store, first, second) = store_with_two_workspaces();
        clear_visit_stamps(&store);

        // Activating the already-active workspace has no outgoing side —
        // it must not stamp the workspace the user is *not* in.
        assert!(store.activate_workspace(&first.0));
        let snapshot = store.snapshot();
        assert!(
            snapshot
                .workspaces
                .iter()
                .find(|w| w.workspace_id == first)
                .unwrap()
                .last_visited_at
                .is_some(),
            "the workspace being activated is always stamped"
        );
        assert_eq!(
            snapshot
                .workspaces
                .iter()
                .find(|w| w.workspace_id == second)
                .unwrap()
                .last_visited_at,
            None,
            "a re-activation must not backdate a visit to some other workspace"
        );
    }

    #[test]
    fn activating_with_no_previous_workspace_stamps_only_the_target() {
        // First activation of a session (or after the active id was cleared)
        // names no outgoing workspace — the lookup must simply find nothing.
        let store = AppStateStore::default();
        let workspace_id = store.snapshot().active_workspace_id.clone();
        let mut snapshot = store.snapshot();
        snapshot.active_workspace_id = WorkspaceId(String::new());
        snapshot.workspaces[0].last_visited_at = None;
        store.replace_snapshot(snapshot);

        assert!(store.activate_workspace(&workspace_id.0));
        assert!(active_workspace_of(&store).last_visited_at.is_some());
    }

    /// Two workspaces that each own one terminal pane, first active — the
    /// pane- and session-addressed activation paths need a real pane in a
    /// *non-active* workspace to aim at.
    fn store_with_two_terminal_workspaces() -> (AppStateStore, WorkspaceId, WorkspaceId) {
        let store = AppStateStore::default();
        let mut snapshot = store.snapshot();
        let first = snapshot.active_workspace_id.clone();
        let second = WorkspaceId("workspace-second".into());

        let mut second_workspace = snapshot.workspaces[0].clone();
        second_workspace.workspace_id = second.clone();
        second_workspace.title = "Workspace 2".into();
        let surface_id = SurfaceId("surface-second".into());
        let pane_id = PaneId("pane-second".into());
        second_workspace.active_surface_id = surface_id.clone();
        second_workspace.surfaces = vec![SurfaceSnapshot {
            surface_id,
            title: "Main Surface".into(),
            active_pane_id: pane_id.clone(),
            root: PaneNodeSnapshot::Terminal {
                pane_id,
                session_id: SessionId("session-second".into()),
                title: "Terminal".into(),
            },
        }];
        snapshot.workspaces.push(second_workspace);
        store.replace_snapshot(snapshot);
        (store, first, second)
    }

    #[test]
    fn activating_a_terminal_session_across_workspaces_stamps_the_switch() {
        // Jump-to-session navigation activates by session id, not
        // workspace id — it must still close the outgoing visit and stamp
        // the incoming one, or the sidebar's unread state lies after every
        // such jump.
        let (store, first, second) = store_with_two_terminal_workspaces();
        clear_visit_stamps(&store);

        let before = current_time_ms_signed();
        assert!(store.activate_terminal_session("session-second"));

        let snapshot = store.snapshot();
        assert_eq!(snapshot.active_workspace_id, second);
        let outgoing = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == first)
            .expect("the workspace we switched away from");
        assert!(
            outgoing.last_visited_at.expect("leaving must stamp") >= before,
            "a session-addressed switch has to close out the outgoing visit"
        );
        let incoming = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == second)
            .expect("the workspace we switched to");
        assert!(incoming.last_visited_at.is_some());
    }

    #[test]
    fn activating_a_pane_across_workspaces_stamps_the_switch() {
        let (store, first, second) = store_with_two_terminal_workspaces();
        clear_visit_stamps(&store);

        let before = current_time_ms_signed();
        assert!(store.activate_pane("pane-second"));

        let snapshot = store.snapshot();
        assert_eq!(snapshot.active_workspace_id, second);
        let outgoing = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == first)
            .expect("the workspace we switched away from");
        assert!(
            outgoing.last_visited_at.expect("leaving must stamp") >= before,
            "a pane-addressed switch has to close out the outgoing visit"
        );
        let incoming = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == second)
            .expect("the workspace we switched to");
        assert!(incoming.last_visited_at.is_some());
    }

    #[test]
    fn activating_a_pane_inside_the_active_workspace_stamps_nothing() {
        // Focus moves within the workspace the user is already looking at
        // are not visits — stamping them would also clear Review badges
        // for panes the user never glanced at.
        let (store, first, _second) = store_with_two_terminal_workspaces();
        let pane_id = {
            let snapshot = store.snapshot();
            snapshot
                .workspaces
                .iter()
                .find(|w| w.workspace_id == first)
                .unwrap()
                .surfaces[0]
                .active_pane_id
                .clone()
        };
        clear_visit_stamps(&store);

        assert!(store.activate_pane(&pane_id.0));

        let snapshot = store.snapshot();
        assert_eq!(snapshot.active_workspace_id, first);
        assert!(
            snapshot
                .workspaces
                .iter()
                .all(|w| w.last_visited_at.is_none()),
            "a same-workspace focus move must not write any visit stamp"
        );
    }

    #[test]
    fn adoption_shells_leave_activity_unknown_for_the_backfill() {
        // A workspace adopted from another device has a real history that
        // simply happened elsewhere. Stamping `now` would present a
        // month-old checkout as brand new AND lock the backfill out of it,
        // since the backfill only fills `None`.
        let store = AppStateStore::default();
        let worktree = store.create_synced_workspace_shell(
            "codemux/feature-x".into(),
            42,
            Some("/home/zeus/projects/codemux".into()),
            "/home/zeus/.codemux/worktrees/codemux/feature-x".into(),
            Some("feature-x".into()),
        );
        let root = store.create_synced_root_shell(
            "codemux".into(),
            42,
            "/home/zeus/.codemux/projects/codemux".into(),
            Some("main".into()),
        );

        let snapshot = store.snapshot();
        for id in [&worktree, &root] {
            let workspace = snapshot
                .workspaces
                .iter()
                .find(|w| &w.workspace_id == id)
                .expect("shell must be registered");
            assert_eq!(
                workspace.last_active_at, None,
                "adoption is not activity — {id:?} must stay datable",
            );
        }
    }

    #[test]
    fn activity_backfill_dates_an_adopted_workspace_once_its_pull_clears_host_id() {
        // The backfill skips host-backed workspaces, and an adoption shell
        // carries `host_id` until its files land. Adoption clears it as soon
        // as the pull succeeds, so the next boot pass reads the real date out
        // of the git history that came across with the checkout.
        let dir = tempfile::tempdir().expect("tempdir");
        let commit_seconds = 1_609_459_200_i64;
        init_repo_committed_at(dir.path(), commit_seconds);
        let checkout = dir.path().display().to_string();

        let store = AppStateStore::default();
        let workspace_id = store.create_synced_workspace_shell(
            "codemux/feature-x".into(),
            42,
            None,
            checkout,
            Some("feature-x".into()),
        );

        let dated = |store: &AppStateStore| {
            store
                .snapshot()
                .workspaces
                .iter()
                .find(|w| w.workspace_id == workspace_id)
                .expect("shell must be registered")
                .last_active_at
        };

        // Still host-backed: the path names a directory on the other device
        // as far as this pass knows, so it is left alone.
        store.backfill_workspace_activity();
        assert_eq!(dated(&store), None, "a pending pull must not be dated");

        store
            .set_workspace_host_id(&workspace_id.0, None)
            .expect("pull-back clears host_id");
        store.backfill_workspace_activity();
        assert_eq!(
            dated(&store),
            Some(commit_seconds * 1000),
            "once local, the adopted checkout is dated from its own history",
        );
    }

    #[test]
    fn activity_backfill_prefers_commit_time_over_directory_mtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        // 2021-01-01T00:00:00Z — far enough in the past that it cannot be
        // confused with the directory's just-created mtime.
        let commit_seconds = 1_609_459_200_i64;
        init_repo_committed_at(dir.path(), commit_seconds);

        let store = store_with_unstamped_workspace(&dir.path().display().to_string());
        store.backfill_workspace_activity();

        assert_eq!(
            store.snapshot().workspaces[0].last_active_at,
            Some(commit_seconds * 1000),
            "the checkout's last commit is the truest available date"
        );
    }

    #[test]
    fn activity_backfill_falls_back_to_directory_mtime_for_non_git_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_with_unstamped_workspace(&dir.path().display().to_string());
        store.backfill_workspace_activity();

        let stamped = store.snapshot().workspaces[0]
            .last_active_at
            .expect("a plain directory still has an mtime");
        assert!(stamped > 0);
        assert!(stamped <= current_time_ms_signed());
    }

    #[test]
    fn activity_backfill_tolerates_a_missing_checkout_path() {
        let missing = std::env::temp_dir().join("codemux-activity-backfill-does-not-exist");
        let store = store_with_unstamped_workspace(&missing.display().to_string());

        // Must not panic, and must not invent "now" — an unknown date has
        // to stay unknown so the idle sweep declines to act on it.
        store.backfill_workspace_activity();
        assert_eq!(store.snapshot().workspaces[0].last_active_at, None);
        assert_eq!(derive_last_activity_ms(&missing), None);
    }

    #[test]
    fn activity_backfill_never_overwrites_a_real_stamp() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repo_committed_at(dir.path(), 1_609_459_200);

        let store = store_with_unstamped_workspace(&dir.path().display().to_string());
        let mut snapshot = store.snapshot();
        snapshot.workspaces[0].last_active_at = Some(1_900_000_000_000);
        store.replace_snapshot(snapshot);

        store.backfill_workspace_activity();
        assert_eq!(
            store.snapshot().workspaces[0].last_active_at,
            Some(1_900_000_000_000),
            "a derived guess must never clobber observed activity"
        );
    }

    #[test]
    fn old_snapshot_without_activity_stamps_deserializes_and_persists_new_ones() {
        // Old layout.json files have neither key. Simulate one by
        // serializing a current snapshot and deleting them.
        let snapshot = default_app_state();
        let mut value = serde_json::to_value(&snapshot).expect("serialize snapshot");
        let workspace = value["workspaces"][0]
            .as_object_mut()
            .expect("workspace serializes to object");
        assert!(
            workspace.remove("last_active_at").is_some()
                && workspace.remove("last_visited_at").is_some(),
            "current snapshots must serialize both stamps (so they persist)"
        );

        let restored: AppStateSnapshot =
            serde_json::from_value(value).expect("old-shape snapshot must deserialize");
        assert_eq!(restored.workspaces[0].last_active_at, None);
        assert_eq!(restored.workspaces[0].last_visited_at, None);

        // And a stamped workspace survives the persistence round trip —
        // these are durable history, not runtime state like pane_statuses.
        let mut stamped = restored;
        stamped.workspaces[0].last_active_at = Some(1_700_000_000_000);
        stamped.workspaces[0].last_visited_at = Some(1_700_000_001_000);
        let json = serde_json::to_string(&stamped).expect("serialize");
        let round_tripped: AppStateSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            round_tripped.workspaces[0].last_active_at,
            Some(1_700_000_000_000)
        );
        assert_eq!(
            round_tripped.workspaces[0].last_visited_at,
            Some(1_700_000_001_000)
        );
    }
}
