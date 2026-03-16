use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceType {
    Standard,
    OpenFlow,
}

impl Default for WorkspaceType {
    fn default() -> Self {
        WorkspaceType::Standard
    }
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
    pub reload_nonce: u32,
    pub last_screenshot_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    Info,
    Attention,
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
    pub git_branch: Option<String>,
    pub notification_count: u32,
    pub latest_agent_state: Option<String>,
    pub active_surface_id: SurfaceId,
    pub surfaces: Vec<SurfaceSnapshot>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStateSnapshot {
    pub schema_version: u32,
    pub active_workspace_id: WorkspaceId,
    pub workspaces: Vec<WorkspaceSnapshot>,
    pub terminal_sessions: Vec<TerminalSessionSnapshot>,
    pub browser_sessions: Vec<BrowserSessionSnapshot>,
    pub notifications: Vec<NotificationSnapshot>,
    pub persistence: PersistenceSchema,
    pub config: CodemuxConfigSnapshot,
}

pub struct AppStateStore {
    inner: Mutex<AppStateSnapshot>,
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

    pub fn set_notification_sound_enabled(&self, enabled: bool) {
        self.inner.lock().unwrap().config.notification_sound_enabled = enabled;
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

    pub fn activate_terminal_session(&self, session_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();

        for workspace in &mut snapshot.workspaces {
            for surface in &mut workspace.surfaces {
                if let Some(pane_id) = find_terminal_pane_id(&surface.root, session_id) {
                    workspace.active_surface_id = surface.surface_id.clone();
                    surface.active_pane_id = pane_id;
                    snapshot.active_workspace_id = workspace.workspace_id.clone();
                    return true;
                }
            }
        }

        false
    }

    pub fn activate_workspace(&self, workspace_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        if snapshot
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            snapshot.active_workspace_id = WorkspaceId(workspace_id.to_string());
            for notification in snapshot.notifications.iter_mut() {
                if notification.workspace_id.0 == workspace_id {
                    notification.read = true;
                }
            }
            if let Some(workspace) = snapshot
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.workspace_id.0 == workspace_id)
            {
                workspace.notification_count = 0;
            }
            return true;
        }
        false
    }

    pub fn activate_pane(&self, pane_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();

        for workspace in &mut snapshot.workspaces {
            for surface in &mut workspace.surfaces {
                if pane_tree_contains_pane(&surface.root, pane_id) {
                    workspace.active_surface_id = surface.surface_id.clone();
                    surface.active_pane_id = PaneId(pane_id.to_string());
                    snapshot.active_workspace_id = workspace.workspace_id.clone();
                    return true;
                }
            }
        }

        false
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
            title,
            workspace_type: WorkspaceType::OpenFlow,
            cwd,
            git_branch: None,
            notification_count: 0,
            latest_agent_state: Some("configuring".into()),
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

    pub fn create_workspace_with_layout(
        &self,
        cwd_path: PathBuf,
        layout: WorkspacePresetLayout,
    ) -> WorkspaceId {
        let mut snapshot = self.inner.lock().unwrap();
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
                reload_nonce: 0,
                last_screenshot_path: None,
            });
            browser = Some(browser_id);
        }

        let root = build_workspace_layout(&layout, &session_ids, browser.as_ref());
        let active_pane_id =
            rightmost_leaf_pane_id(&root).unwrap_or_else(|| PaneId(next_id("pane")));

        snapshot.workspaces.push(WorkspaceSnapshot {
            workspace_id: workspace_id.clone(),
            title: format!("Workspace {workspace_index}"),
            workspace_type: WorkspaceType::Standard,
            cwd,
            git_branch: None,
            notification_count: 0,
            latest_agent_state: Some("idle".into()),
            active_surface_id: surface_id.clone(),
            surfaces: vec![SurfaceSnapshot {
                surface_id,
                title: "Main Surface".into(),
                active_pane_id,
                root,
            }],
        });

        snapshot.active_workspace_id = workspace_id.clone();
        snapshot
            .notifications
            .retain(|notification| notification.workspace_id != workspace_id);
        workspace_id
    }

    pub fn create_browser_pane(&self, pane_id: &str) -> Result<(PaneId, BrowserId), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let (workspace_index, surface_index) = find_pane_location(&snapshot.workspaces, pane_id)
            .ok_or_else(|| format!("No pane found for {pane_id}"))?;

        let browser_id = BrowserId(next_id("browser"));
        let new_pane_id = PaneId(next_id("pane"));
        let split_pane_id = PaneId(next_id("pane"));
        let title = format!("Browser {}", snapshot.browser_sessions.len() + 1);
        let initial_url = DEFAULT_BROWSER_URL.to_string();

        snapshot.browser_sessions.push(BrowserSessionSnapshot {
            browser_id: browser_id.clone(),
            title: title.clone(),
            current_url: Some(initial_url.clone()),
            history: vec![initial_url],
            history_index: 0,
            is_loading: false,
            last_error: None,
            reload_nonce: 0,
            last_screenshot_path: None,
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

    pub fn close_workspace(&self, workspace_id: &str) -> Result<WorkspaceId, String> {
        let mut snapshot = self.inner.lock().unwrap();

        if snapshot.workspaces.len() <= 1 {
            let workspace_index = snapshot
                .workspaces
                .iter()
                .position(|workspace| workspace.workspace_id.0 == workspace_id)
                .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

            let removed = snapshot.workspaces.remove(workspace_index);
            let removed_session_ids = collect_terminal_sessions(&removed.surfaces);
            snapshot
                .notifications
                .retain(|notification| notification.workspace_id != removed.workspace_id);
            snapshot.terminal_sessions.retain(|session| {
                !removed_session_ids
                    .iter()
                    .any(|id| id == &session.session_id.0)
            });
            snapshot.active_workspace_id = WorkspaceId("".into());
            return Ok(WorkspaceId("".into()));
        }

        let workspace_index = snapshot
            .workspaces
            .iter()
            .position(|workspace| workspace.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

        let removed = snapshot.workspaces.remove(workspace_index);
        let removed_session_ids = collect_terminal_sessions(&removed.surfaces);
        snapshot
            .notifications
            .retain(|notification| notification.workspace_id != removed.workspace_id);
        snapshot.terminal_sessions.retain(|session| {
            !removed_session_ids
                .iter()
                .any(|id| id == &session.session_id.0)
        });

        let fallback_workspace = snapshot
            .workspaces
            .first()
            .map(|workspace| workspace.workspace_id.clone())
            .ok_or_else(|| "No fallback workspace available".to_string())?;
        snapshot.active_workspace_id = fallback_workspace.clone();

        Ok(fallback_workspace)
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
        let cwd = current_project_root().display().to_string();
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

            let updated_root = remove_pane_from_tree(&surface.root, pane_id)
                .ok_or_else(|| "Cannot close the last pane in a surface".to_string())?;
            let next_active_pane = first_leaf_pane_id(&updated_root)
                .ok_or_else(|| "No fallback pane available after close".to_string())?;

            surface.root = updated_root;
            surface.active_pane_id = next_active_pane;
            workspace.active_surface_id = surface.surface_id.clone();
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
        browser.reload_nonce = browser.reload_nonce.wrapping_add(1);
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

    pub fn set_browser_screenshot_path(
        &self,
        browser_id: &str,
        screenshot_path: String,
    ) -> Result<(), String> {
        let mut snapshot = self.inner.lock().unwrap();
        let browser = snapshot
            .browser_sessions
            .iter_mut()
            .find(|browser| browser.browser_id.0 == browser_id)
            .ok_or_else(|| format!("No browser session found for {browser_id}"))?;
        browser.last_screenshot_path = Some(screenshot_path);
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

    pub fn mark_workspace_notifications_read(&self, workspace_id: &str) -> bool {
        let mut snapshot = self.inner.lock().unwrap();
        let mut changed = false;

        for notification in snapshot.notifications.iter_mut() {
            if notification.workspace_id.0 == workspace_id && !notification.read {
                notification.read = true;
                changed = true;
            }
        }

        let unread_count = snapshot
            .notifications
            .iter()
            .filter(|notification| {
                notification.workspace_id.0 == workspace_id && !notification.read
            })
            .count() as u32;

        if let Some(workspace) = snapshot
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id.0 == workspace_id)
        {
            workspace.notification_count = unread_count;
            return changed;
        }

        false
    }
}

pub fn emit_app_state(app: &AppHandle) {
    let state: State<'_, AppStateStore> = app.state();
    let snapshot = state.snapshot();
    if let Err(error) = app.emit("app-state-changed", &snapshot) {
        eprintln!("[codemux::state] Failed to emit app state: {error}");
    }
    // Persist asynchronously with debounce — rapid consecutive calls (e.g.
    // swap + multiple resize events) collapse into a single disk write.
    persist_debouncer().schedule(snapshot);
}

pub fn load_persisted_state() -> Option<AppStateSnapshot> {
    let path = persisted_layout_path()?;
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
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
            ids
        })
        .chain(
            snapshot
                .terminal_sessions
                .iter()
                .map(|session| extract_numeric_suffix(&session.session_id.0)),
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
    let cwd = current_project_root().display().to_string();
    let shell = env::var("SHELL").ok();

    AppStateSnapshot {
        schema_version: APP_STATE_SCHEMA_VERSION,
        active_workspace_id: workspace_id.clone(),
        workspaces: vec![WorkspaceSnapshot {
            workspace_id,
            title: "Workspace 1".into(),
            workspace_type: WorkspaceType::Standard,
            cwd: cwd.clone(),
            git_branch: None,
            notification_count: 0,
            latest_agent_state: Some("idle".into()),
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
        }],
        browser_sessions: vec![],
        notifications: vec![],
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
        },
    }
}

fn session_id_for_pane(root: &PaneNodeSnapshot, target_pane_id: &PaneId) -> Option<SessionId> {
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
        PaneNodeSnapshot::Browser { .. } => false,
    }
}

fn remove_terminal_from_tree(
    root: &PaneNodeSnapshot,
    target_session_id: &str,
) -> Option<PaneNodeSnapshot> {
    match root {
        PaneNodeSnapshot::Terminal { session_id, .. } if session_id.0 == target_session_id => None,
        PaneNodeSnapshot::Terminal { .. } | PaneNodeSnapshot::Browser { .. } => Some(root.clone()),
        PaneNodeSnapshot::Split {
            pane_id,
            direction,
            child_sizes,
            children,
        } => {
            let remaining_children = children
                .iter()
                .filter_map(|child| remove_terminal_from_tree(child, target_session_id))
                .collect::<Vec<_>>();

            match remaining_children.len() {
                0 => None,
                1 => remaining_children.into_iter().next(),
                _ => Some(PaneNodeSnapshot::Split {
                    pane_id: pane_id.clone(),
                    direction: direction.clone(),
                    child_sizes: rebalance_sizes(child_sizes, remaining_children.len()),
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
        PaneNodeSnapshot::Terminal { .. } | PaneNodeSnapshot::Browser { .. } => Some(root.clone()),
        PaneNodeSnapshot::Split {
            pane_id,
            direction,
            child_sizes,
            children,
        } => {
            let remaining_children = children
                .iter()
                .filter_map(|child| remove_terminals_from_tree(child, session_ids))
                .collect::<Vec<_>>();

            match remaining_children.len() {
                0 => None,
                1 => remaining_children.into_iter().next(),
                _ => Some(PaneNodeSnapshot::Split {
                    pane_id: pane_id.clone(),
                    direction: direction.clone(),
                    child_sizes: rebalance_sizes(child_sizes, remaining_children.len()),
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
        PaneNodeSnapshot::Browser { .. } => None,
    }
}

fn find_terminal_pane_id(root: &PaneNodeSnapshot, target_session_id: &str) -> Option<PaneId> {
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

pub fn collect_terminal_sessions(surfaces: &[SurfaceSnapshot]) -> Vec<String> {
    surfaces
        .iter()
        .flat_map(|surface| collect_terminal_sessions_from_node(&surface.root))
        .collect()
}

fn collect_terminal_sessions_from_node(root: &PaneNodeSnapshot) -> Vec<String> {
    match root {
        PaneNodeSnapshot::Terminal { session_id, .. } => vec![session_id.0.clone()],
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .flat_map(collect_terminal_sessions_from_node)
            .collect(),
        PaneNodeSnapshot::Browser { .. } => vec![],
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
    Some(base.join("codemux").join("layout.json"))
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
    let snapshot = strip_openflow_from_snapshot(snapshot.clone());

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

fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
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
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. } => {
            Some(pane_id.clone())
        }
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
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. } => {
            pane_id == target_pane_id
        }
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .any(|child| pane_contains_leaf(child, target_pane_id)),
    }
}

fn pane_tree_contains_pane(root: &PaneNodeSnapshot, target_pane_id: &str) -> bool {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. } => {
            pane_id.0 == target_pane_id
        }
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
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. } => {
            vec![pane_id.clone()]
        }
        PaneNodeSnapshot::Split { children, .. } => {
            children.iter().flat_map(collect_leaf_pane_ids).collect()
        }
    }
}

fn first_leaf_pane_id(root: &PaneNodeSnapshot) -> Option<PaneId> {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. } => {
            Some(pane_id.clone())
        }
        PaneNodeSnapshot::Split { children, .. } => children.iter().find_map(first_leaf_pane_id),
    }
}

fn clone_pane_node(root: &PaneNodeSnapshot, target_pane_id: &str) -> Option<PaneNodeSnapshot> {
    match root {
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. }
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
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. }
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
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. } => {
            pane_id.clone()
        }
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
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. }
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
                        PaneNodeSnapshot::Terminal { .. } | PaneNodeSnapshot::Browser { .. } => {
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
        PaneNodeSnapshot::Terminal { pane_id, .. } | PaneNodeSnapshot::Browser { pane_id, .. }
            if pane_id.0 == target_pane_id =>
        {
            None
        }
        PaneNodeSnapshot::Terminal { .. } | PaneNodeSnapshot::Browser { .. } => Some(root.clone()),
        PaneNodeSnapshot::Split {
            pane_id,
            direction,
            child_sizes,
            children,
        } => {
            let remaining_children = children
                .iter()
                .filter_map(|child| remove_pane_from_tree(child, target_pane_id))
                .collect::<Vec<_>>();

            match remaining_children.len() {
                0 => None,
                1 => remaining_children.into_iter().next(),
                _ => Some(PaneNodeSnapshot::Split {
                    pane_id: pane_id.clone(),
                    direction: direction.clone(),
                    child_sizes: rebalance_sizes(child_sizes, remaining_children.len()),
                    children: remaining_children,
                }),
            }
        }
    }
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

        store.create_browser_pane(&active_pane_id).unwrap();
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
}
