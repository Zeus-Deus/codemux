//! Opening, creating and closing workspaces and worktrees for every caller:
//! the desktop, the control socket and outside tools.
//!
//! The requests are typed and the service never sees a caller. Desktop
//! commands pass every choice explicitly; callers that act for an agent
//! decide authorization themselves and only then come here, with a
//! [`Checkpoint`] the service asks again wherever it waited.
use crate::commands::workspace::WorkspaceCreated;
pub(crate) use crate::git::CommitLoss;
use crate::git::Kept;
use crate::state::{
    AppStateStore, InitialChatPane, WorkspaceId, WorkspacePresetLayout, WorkspaceSnapshot,
    WorkspaceType,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};

/// Asks again whether whoever requested a lifecycle operation still may, at
/// the points it cannot take back after a wait: once the shared lifecycle
/// lock is held (a checkout removal holds it for as long as its delete
/// takes), before the checkout is made, and before a workspace is created,
/// terminals or each teardown command start, a closing workspace leaves or
/// its chats stop, or files are deleted. A new workspace's setup outlives the
/// call and asks [`Self::background`] instead. Outside callers and plugins
/// pass their grant check. The desktop and the control socket act for the
/// person at this computer and pass the default, which always passes.
#[derive(Clone, Default)]
pub(crate) struct Checkpoint {
    check: Option<Check>,
    background: Option<Check>,
}

type Check = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

impl Checkpoint {
    pub(crate) fn new(check: impl Fn() -> Result<(), String> + Send + Sync + 'static) -> Self {
        Self { check: Some(Arc::new(check)), background: None }
    }

    /// Adds the check for work the operation leaves running once it
    /// returned: a new workspace's setup copies its worktree includes and
    /// runs its setup commands in the background. The call has ended by then,
    /// and so has authority scoped to it (an agent's turn, a plugin
    /// invocation, a launch receipt), so this asks only what a person
    /// withdraws, such as a connector grant or a plugin's grant. Without it,
    /// that work is not asked again.
    pub(crate) fn with_background(
        self,
        check: impl Fn() -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self { background: Some(Arc::new(check)), ..self }
    }

    /// The refusal reads `code: message`, as the caller's own check words it.
    pub(crate) fn check(&self) -> Result<(), String> {
        self.check.as_ref().map_or(Ok(()), |check| check())
    }

    /// The checkpoint work left running asks (see [`Self::with_background`]).
    pub(crate) fn background(&self) -> Checkpoint {
        Checkpoint { check: self.background.clone(), background: None }
    }
}

/// Where a new branch forks from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WorktreeBase {
    /// The repository checkout's HEAD (`git worktree add -b` with no base).
    /// The desktop's "no base selected".
    RepoHead,
    /// The repository's default branch (origin/HEAD, then main/master, then
    /// the checked-out branch), freshened from origin like `Ref`; `RepoHead`
    /// when none resolves. What agent-facing callers get by default.
    DefaultBranch,
    /// A caller-named ref: best-effort `origin/<ref>` fetch and preference,
    /// exactly as `git::git_create_worktree` does.
    Ref(String),
    /// An exact commit, fetched by id only when the repository lacks it.
    /// `label` is recorded as the base branch; without one the commit itself
    /// is.
    Commit { sha: String, label: Option<String> },
}

/// A terminal CLI preset launched in the new workspace's first terminal.
#[derive(Debug, Clone)]
pub(crate) struct PresetLaunch {
    pub preset_id: String,
    pub initial_prompt: Option<String>,
    pub model_selection: Option<crate::agent_capability::ModelSelection>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorktreeRequest {
    pub repo_path: String,
    pub branch: String,
    pub new_branch: bool,
    pub base: WorktreeBase,
    pub layout: WorkspacePresetLayout,
    pub preset: Option<PresetLaunch>,
    pub pr_number: Option<u32>,
    pub select: bool,
    pub initial_chat: Option<InitialChatPane>,
    /// Inserted under this id when it is created (never when adopted).
    pub reserved_id: Option<WorkspaceId>,
}

/// Arguments of agent-facing worktree creation, which own its defaults: a
/// new branch from the repository's default branch, one terminal, and no
/// focus change unless `select` asks for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorktreeArgs {
    pub repo_path: String,
    pub branch: String,
    /// Default true.
    pub new_branch: Option<bool>,
    /// Default: the repository's default branch.
    pub base: Option<String>,
    /// For a new branch, `true` requires a freshly fetched origin base and
    /// `false` uses the local ref exactly. Omitted: best-effort freshening.
    pub start_from_origin: Option<bool>,
    /// Default "single".
    pub layout: Option<String>,
    pub agent_preset_id: Option<String>,
    pub initial_prompt: Option<String>,
    pub pr_number: Option<u32>,
    #[serde(default)]
    pub select: bool,
    /// Accepted so strict parsing admits it; the receipt layer reads the
    /// raw arguments, so nothing here does.
    #[allow(dead_code)]
    pub client_request_id: Option<String>,
}

impl WorktreeArgs {
    /// Resolves the defaults. With `start_from_origin` set, a new branch's
    /// base is resolved to an exact commit first, so worktree creation's own
    /// best-effort fetch can never apply a different origin policy.
    pub(crate) async fn into_request(
        self,
        model_selection: Option<crate::agent_capability::ModelSelection>,
        reserved_id: Option<WorkspaceId>,
    ) -> Result<WorktreeRequest, String> {
        let new_branch = self.new_branch.unwrap_or(true);
        let layout = parse_layout(self.layout.as_deref().unwrap_or("single"))?;
        let base = match self.start_from_origin.filter(|_| new_branch) {
            Some(from_origin) => {
                let repo = PathBuf::from(&self.repo_path);
                let name = match self.base {
                    Some(base) => base,
                    None => {
                        let repo = repo.clone();
                        tokio::task::spawn_blocking(move || default_base_name(&repo))
                            .await
                            .map_err(|error| format!("default branch lookup failed: {error}"))?
                    }
                };
                let origin = if from_origin { OriginPolicy::Required } else { OriginPolicy::Local };
                let resolved = resolve_base(&repo, &name, origin).await?;
                WorktreeBase::Commit {
                    // A detached default records the commit, not "HEAD".
                    label: (resolved.label != "HEAD").then_some(resolved.label),
                    sha: resolved.commit,
                }
            }
            None => self.base.map(WorktreeBase::Ref).unwrap_or(WorktreeBase::DefaultBranch),
        };
        Ok(WorktreeRequest {
            repo_path: self.repo_path,
            branch: self.branch,
            new_branch,
            base,
            layout,
            preset: self.agent_preset_id.map(|preset_id| PresetLaunch {
                preset_id,
                initial_prompt: self.initial_prompt,
                model_selection,
            }),
            pr_number: self.pr_number,
            select: self.select,
            initial_chat: None,
            reserved_id,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OpenWorkspace {
    /// `None` = `project::current_project_root()` (local legacy callers only).
    pub path: Option<String>,
    pub select: bool,
    /// Return a live top-level local workspace already at this folder.
    pub reuse_open: bool,
    pub reserved_id: Option<WorkspaceId>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct OpenedWorkspace {
    pub workspace_id: String,
    pub cwd: String,
    pub created: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CloseWorkspace {
    pub workspace_id: String,
    pub remove_worktree: bool,
    pub delete_branch: bool,
    pub force: bool,
    /// Whether removing the worktree may lose commits nothing else keeps.
    pub commits: CommitLoss,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ClosedWorkspace {
    pub workspace_id: String,
    pub closed: bool,
    pub worktree_removed: bool,
    /// Why a close that may not lose commits kept the worktree or branch it
    /// was asked to delete: something committed in it while it closed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_because: Option<String>,
}

/// The slim row outside catalogs list: the connector's existing
/// `workspace_list` row plus the worktree and parent links.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct WorkspaceSummary {
    pub workspace_id: String,
    pub title: String,
    pub cwd: String,
    pub project_root: Option<String>,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub parent_workspace_id: Option<String>,
    pub native_control_available: bool,
}

impl WorkspaceSummary {
    fn of(workspace: &WorkspaceSnapshot) -> Self {
        Self {
            workspace_id: workspace.workspace_id.0.clone(),
            title: workspace.title.clone(),
            cwd: workspace.cwd.clone(),
            project_root: workspace.project_root.clone(),
            branch: workspace.git_branch.clone(),
            worktree_path: workspace.worktree_path.clone(),
            parent_workspace_id: workspace.parent_workspace_id.clone(),
            native_control_available: native_control(workspace).is_ok(),
        }
    }
}

/// Why outside thread tools cannot act in a workspace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NotControllable {
    /// An imported, read-only snapshot.
    ImportedSnapshot,
    /// On an SSH device or in a remote folder.
    Remote,
}

/// Whether outside thread tools may act in `workspace`: only a live local one.
/// The one rule behind both `native_control_available` and the thread tools'
/// refusal, so a listing never offers a target the tools then refuse.
pub(crate) fn native_control(workspace: &WorkspaceSnapshot) -> Result<(), NotControllable> {
    if workspace.is_local_import_snapshot_only() {
        Err(NotControllable::ImportedSnapshot)
    } else if workspace.host_id.is_some() || workspace.remote_cwd.is_some() {
        Err(NotControllable::Remote)
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum WorkspaceTarget<'a> {
    Id(&'a str),
    /// The workspace focused in Codemux.
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum InfoError {
    NotFound,
    NoActive,
}

/// How a named base relates to `origin`, for callers that resolve a commit
/// up front instead of letting worktree creation fetch on its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OriginPolicy {
    /// The local ref exactly; nothing is fetched.
    Local,
    /// Best-effort fetch of `origin/<base>`, preferred when it exists.
    PreferFresh,
    /// Fetch `origin/<base>`; fail rather than fall back to a stale or local ref.
    Required,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedBase {
    pub commit: String,
    /// The branch or ref the commit was read from, for display.
    pub label: String,
}

pub(crate) struct WorkspaceService<R: Runtime> {
    app: AppHandle<R>,
    checkpoint: Checkpoint,
}

impl<R: Runtime> WorkspaceService<R> {
    pub(crate) fn new(app: &AppHandle<R>) -> Self {
        Self { app: app.clone(), checkpoint: Checkpoint::default() }
    }

    /// Open, create and close ask `checkpoint` again at their points of no
    /// return.
    pub(crate) fn checked(self, checkpoint: Checkpoint) -> Self {
        Self { checkpoint, ..self }
    }

    /// Create or open a workspace at a folder. Runs setup scripts only when
    /// it creates one.
    pub(crate) async fn open(&self, request: OpenWorkspace) -> Result<OpenedWorkspace, String> {
        crate::commands::workspace::open_workspace_impl(self.app.clone(), request, &self.checkpoint)
            .await
    }

    pub(crate) async fn create_worktree(
        &self,
        request: WorktreeRequest,
    ) -> Result<WorkspaceCreated, String> {
        crate::commands::workspace::create_worktree_checked(
            self.app.clone(),
            request,
            &self.checkpoint,
        )
        .await
    }

    /// Close a workspace through the same teardown and deletion guards as the
    /// desktop's close dialog. Error text passes through verbatim.
    pub(crate) async fn close(&self, request: CloseWorkspace) -> Result<ClosedWorkspace, String> {
        let kept = crate::commands::workspace::close_workspace_checked(
            self.app.clone(),
            request.clone(),
            &self.checkpoint,
        )
        .await?;
        // A close that was asked to remove the worktree and succeeded did (a
        // workspace without one is refused before any teardown), unless its
        // HEAD gained a commit nothing else keeps while it closed.
        Ok(ClosedWorkspace {
            workspace_id: request.workspace_id,
            closed: true,
            worktree_removed: request.remove_worktree && !matches!(kept, Some(Kept::Checkout(_))),
            kept_because: kept.map(Kept::into_reason),
        })
    }

    /// Archive a workspace: the close path without deleting anything, plus a
    /// restorable entry. Returns its archive id.
    pub(crate) async fn archive(&self, workspace_id: String) -> Result<String, String> {
        let state = self.app.state::<AppStateStore>();
        let db = self.app.state::<crate::database::DatabaseStore>();
        crate::commands::workspace::archive_workspace_checked(self.app.clone(), &state, &db, workspace_id, &self.checkpoint)
            .await
    }

    /// Restore an archive entry. Returns the restored workspace's id.
    pub(crate) async fn unarchive(&self, archive_id: String, select: bool) -> Result<String, String> {
        let state = self.app.state::<AppStateStore>();
        let db = self.app.state::<crate::database::DatabaseStore>();
        crate::commands::workspace::unarchive_workspace_checked(self.app.clone(), &state, &db, archive_id, select, &self.checkpoint)
            .await
    }

    pub(crate) fn summaries(&self) -> Vec<WorkspaceSummary> {
        let snapshot = self.app.state::<AppStateStore>().snapshot();
        snapshot.workspaces.iter().map(WorkspaceSummary::of).collect()
    }

    pub(crate) fn snapshots(&self) -> Vec<WorkspaceSnapshot> {
        self.app.state::<AppStateStore>().snapshot().workspaces
    }

    /// One workspace's full snapshot. Callers choose the wording and shape of
    /// the two failures.
    pub(crate) fn info(&self, target: WorkspaceTarget<'_>) -> Result<WorkspaceSnapshot, InfoError> {
        let snapshot = self.app.state::<AppStateStore>().snapshot();
        let (id, missing) = match target {
            WorkspaceTarget::Id(id) => (id, InfoError::NotFound),
            WorkspaceTarget::Active => (snapshot.active_workspace_id.0.as_str(), InfoError::NoActive),
        };
        snapshot
            .workspaces
            .iter()
            .find(|workspace| !id.is_empty() && workspace.workspace_id.0 == id)
            .cloned()
            .ok_or(missing)
    }
}

/// The layouts a new workspace can start with.
pub(crate) fn parse_layout(layout: &str) -> Result<WorkspacePresetLayout, String> {
    Ok(match layout {
        "single" => WorkspacePresetLayout::Single,
        "pair" => WorkspacePresetLayout::Pair,
        "quad" => WorkspacePresetLayout::Quad,
        "six" => WorkspacePresetLayout::Six,
        "eight" => WorkspacePresetLayout::Eight,
        "shell_browser" => WorkspacePresetLayout::ShellBrowser,
        // No terminal, tab or surface: the caller attaches its own pane (the
        // inline "+ New worktree" chat flow, agent launches).
        "empty" => WorkspacePresetLayout::Empty,
        _ => return Err(format!("Unsupported layout: {layout}")),
    })
}

/// The base agent-facing callers fork from when they name none: the
/// repository's default branch, else `HEAD`. Runs git synchronously.
pub(crate) fn default_base_name(repo: &Path) -> String {
    crate::git::resolve_default_branch(repo).unwrap_or_else(|| "HEAD".into())
}

/// How many of the most recent Recent projects count as known.
pub(crate) const RECENT_PROJECT_LIMIT: u32 = 200;

/// Project roots this computer already works in: live and archived local
/// workspaces' project roots and folders, plus Recent projects. Canonical and
/// git-rooted. Home workspaces, `$HOME` and every folder that contains it are
/// never known, so a dotfiles repository at `$HOME` (or a checkout at `/`)
/// cannot make every folder known. Reads the filesystem; call it off the
/// async runtime.
pub(crate) fn known_project_roots<R: Runtime>(app: &AppHandle<R>) -> Vec<PathBuf> {
    known_roots(known_folders(app), dirs::home_dir().as_deref())
}

/// The folders behind [`known_project_roots`], before git-rooting. A
/// workspace contributes its folder as well as its project root: under a
/// dotfiles `$HOME` the project root is `$HOME`, which is never known.
fn known_folders<R: Runtime>(app: &AppHandle<R>) -> Vec<PathBuf> {
    let snapshot = app.state::<AppStateStore>().snapshot();
    let live = snapshot
        .workspaces
        .iter()
        .filter(|w| {
            w.host_id.is_none()
                && !w.attach_only
                && !w.is_local_import_snapshot_only()
                && w.workspace_type != WorkspaceType::Home
        })
        .flat_map(|w| w.project_root.iter().chain([&w.cwd]));
    // Remote workspaces cannot be archived, so every entry is a local path.
    let archived = snapshot
        .archived_workspaces
        .iter()
        .flat_map(|entry| entry.project_root.iter().chain([&entry.cwd]));
    let recent = app
        .try_state::<crate::database::DatabaseStore>()
        .map(|db| db.get_recent_projects(RECENT_PROJECT_LIMIT))
        .unwrap_or_default()
        .into_iter()
        .map(|project| PathBuf::from(project.path));
    live.chain(archived).map(PathBuf::from).chain(recent).collect()
}

/// The pure part of [`known_project_roots`]: canonical git roots of the
/// folders that exist, deduplicated, leaving out `home` and every folder that
/// contains it. A folder whose git root contains `home` stands for itself.
fn known_roots(folders: impl IntoIterator<Item = PathBuf>, home: Option<&Path>) -> Vec<PathBuf> {
    let home = home.and_then(|home| home.canonicalize().ok());
    let holds_home = |root: &PathBuf| home.as_ref().is_some_and(|home| home.starts_with(root));
    let mut roots = Vec::new();
    for folder in folders {
        let Ok(folder) = folder.canonicalize() else {
            continue;
        };
        let root = git_root(&folder).filter(|root| !holds_home(root)).unwrap_or(folder);
        if !holds_home(&root) && !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// The main checkout the canonical `folder` belongs to: the repository root
/// for a folder in a repository, the main checkout for a linked worktree and
/// the superproject for a submodule. `None` outside git.
///
/// This reads `.git` files itself: `find_git_root` takes a `gitdir:` as
/// written, so a relative one (submodules, worktrees added with
/// `--relative-paths`) would resolve against the app's own working folder.
fn git_root(folder: &Path) -> Option<PathBuf> {
    let checkout = folder.ancestors().find(|dir| dir.join(".git").exists())?;
    let dot_git = checkout.join(".git");
    if dot_git.is_dir() {
        return Some(checkout.to_path_buf());
    }
    Some(repository_of_git_file(checkout, &dot_git).unwrap_or_else(|| checkout.to_path_buf()))
}

/// For a checkout whose `.git` is a file, the repository that holds its git
/// dir, canonical: linked worktrees and submodules keep theirs inside the
/// main checkout's `.git` folder (`worktrees/<name>`, `modules/<name>`), and
/// a bare repository's worktree names the repository as its `commondir`.
/// `None` for a repository whose git dir lives elsewhere on its own.
fn repository_of_git_file(checkout: &Path, dot_git: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dot_git).ok()?;
    let gitdir = checkout.join(text.strip_prefix("gitdir:")?.trim()).canonicalize().ok()?;
    if let Some(repo_git) = gitdir
        .ancestors()
        .skip(1)
        .find(|dir| dir.file_name() == Some(std::ffi::OsStr::new(".git")))
    {
        return repo_git.parent().map(Path::to_path_buf);
    }
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    gitdir.join(common.trim()).canonicalize().ok()
}

/// Checks that `path` is an existing folder whose git root (or, outside git,
/// the folder itself) is one of `known`. A subfolder of a known repository
/// and that repository's linked worktrees and submodules pass; a symlink is
/// judged by where it points. `~` is the user's home, as in the legacy open;
/// any other relative path is refused, since it would resolve against the
/// app's own working folder. Returns the canonical folder.
pub(crate) fn resolve_under_known_root(known: &[PathBuf], path: &Path) -> Result<PathBuf, String> {
    let path = match path.to_str() {
        Some(text) => PathBuf::from(crate::project::expand_tilde(text)),
        None => path.to_path_buf(),
    };
    if !path.is_absolute() {
        return Err(format!("invalid_arguments: path must be absolute: {}", path.display()));
    }
    let folder = path
        .canonicalize()
        .map_err(|error| format!("invalid_arguments: {} does not exist: {error}", path.display()))?;
    if !folder.is_dir() {
        return Err(format!("invalid_arguments: {} is not a folder", path.display()));
    }
    let known_root = |candidate: &PathBuf| known.iter().any(|root| root == candidate);
    if git_root(&folder).is_some_and(|root| known_root(&root)) || known_root(&folder) {
        Ok(folder)
    } else {
        Err(format!(
            "unknown_project_root: {} is not in a project already opened in Codemux",
            path.display()
        ))
    }
}

/// The plain-text rules a branch or base name must pass before git sees it:
/// non-empty, at most 256 bytes, no leading `-` (never an option) and no
/// control characters.
pub(crate) fn check_ref_text(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || value.starts_with('-')
        || value.chars().any(char::is_control)
    {
        return Err(format!("invalid_input: Invalid {field}"));
    }
    Ok(())
}

/// Checks that `branch` can become a new worktree branch: a literal branch
/// name (`check-ref-format --branch` returns it unchanged, so no `@{-1}`
/// shorthand), not an existing `refs/heads/<branch>`, and no checkout at its
/// conventional worktree path. Local git only; nothing is fetched.
pub(crate) async fn check_new_worktree(repo: &Path, branch: &str) -> Result<(), String> {
    check_ref_text("branch", branch)?;
    if !is_literal_branch(repo, branch).await {
        return Err("invalid_input: Worktree branch must be literal".into());
    }
    let existing = git_text(
        repo,
        &[
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            &format!("refs/heads/{branch}"),
        ],
    )
    .await?;
    if !existing.is_empty()
        || crate::git::conventional_worktree_path(repo, branch).is_some_and(|path| path.exists())
    {
        return Err(
            "conflict: Worktree branch/path already exists; no existing checkout is adopted".into(),
        );
    }
    Ok(())
}

/// Resolves `base` to the commit a new worktree branch starts from, under
/// one origin policy. `origin/<b>` and `<b>` name the same origin branch for
/// the two fetching policies. Errors keep the `invalid_input:` and
/// `operation_failed:` prefixes callers map to codes.
pub(crate) async fn resolve_base(
    repo: &Path,
    base: &str,
    origin: OriginPolicy,
) -> Result<ResolvedBase, String> {
    check_ref_text("base", base)?;
    let branch = base.strip_prefix("origin/").unwrap_or(base);
    let (revision, label) = match origin {
        OriginPolicy::Local => (base.to_string(), base.to_string()),
        OriginPolicy::Required => {
            if !is_literal_branch(repo, branch).await {
                return Err("invalid_input: A remote base must name a literal origin branch".into());
            }
            if !fetch_origin(repo, branch).await? {
                return Err("operation_failed: The requested origin base could not be fetched; no stale/local fallback was used".into());
            }
            (format!("refs/remotes/origin/{branch}"), branch.to_string())
        }
        OriginPolicy::PreferFresh => {
            let remote = format!("refs/remotes/origin/{branch}");
            // Only a literal branch has an origin counterpart; HEAD, commits
            // and full refs resolve locally.
            let fetchable = branch != "HEAD"
                && !crate::git::is_object_id(branch)
                && is_literal_branch(repo, branch).await;
            if fetchable {
                // Best effort: a failed fetch still leaves the last fetched
                // origin ref, which is preferred over the local branch.
                let _ = fetch_origin(repo, branch).await;
            }
            if fetchable
                && git_text(repo, &["rev-parse", "--verify", "--quiet", &remote]).await.is_ok()
            {
                (remote, branch.to_string())
            } else {
                (base.to_string(), base.to_string())
            }
        }
    };
    let commit = git_text(
        repo,
        &["rev-parse", "--verify", "--end-of-options", &format!("{revision}^{{commit}}")],
    )
    .await?;
    Ok(ResolvedBase { commit, label })
}

async fn is_literal_branch(repo: &Path, name: &str) -> bool {
    git_text(repo, &["check-ref-format", "--branch", name])
        .await
        .is_ok_and(|literal| literal == name)
}

/// `git fetch origin <branch>` on the blocking pool; `Ok(false)` when there
/// is no origin or the fetch failed.
async fn fetch_origin(repo: &Path, branch: &str) -> Result<bool, String> {
    let (repo, branch) = (repo.to_path_buf(), branch.to_string());
    tokio::task::spawn_blocking(move || crate::git::fetch_origin_branch(&repo, &branch))
        .await
        .map_err(|_| "operation_failed: Origin fetch worker failed".to_string())
}

/// Bounded, hook-free local git with trimmed stdout.
async fn git_text(repo: &Path, args: &[&str]) -> Result<String, String> {
    crate::plugins::platform_package::git_run_bounded(repo, args, 4096, false)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_isolation::isolated;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(repo)
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Codemux test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// A repository with one commit on `branch`.
    fn repo(parent: &Path, name: &str, branch: &str) -> PathBuf {
        let repo = parent.join(name);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", branch]);
        git(&repo, &["commit", "--allow-empty", "-m", "first"]);
        repo
    }

    fn mock_app(state: AppStateStore) -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        app.manage(state);
        app.manage(crate::database::DatabaseStore::new_in_memory());
        app.manage(crate::terminal::PtyState::default());
        app
    }

    #[test]
    fn default_base_name_prefers_default_branch_then_current_then_head() {
        let dir = tempfile::tempdir().unwrap();
        let trunk = repo(dir.path(), "trunk-only", "trunk");
        assert_eq!(default_base_name(&trunk), "trunk");

        let master = repo(dir.path(), "with-master", "master");
        git(&master, &["checkout", "-b", "feature"]);
        assert_eq!(default_base_name(&master), "master", "master beats the checked-out branch");

        let develop = repo(dir.path(), "origin-head", "main");
        git(&develop, &["update-ref", "refs/remotes/origin/develop", "HEAD"]);
        git(&develop, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/develop"]);
        assert_eq!(default_base_name(&develop), "develop", "origin/HEAD beats main");

        let detached = repo(dir.path(), "detached", "trunk");
        git(&detached, &["checkout", "--detach"]);
        assert_eq!(default_base_name(&detached), "HEAD");
    }

    #[tokio::test]
    async fn worktree_args_default_to_a_new_branch_from_the_default_branch() {
        let parse = |args: serde_json::Value| serde_json::from_value::<WorktreeArgs>(args);
        let request = parse(serde_json::json!({"repo_path": "/repo", "branch": "feat"}))
            .unwrap()
            .into_request(None, None)
            .await
            .unwrap();
        assert!(request.new_branch);
        assert_eq!(request.base, WorktreeBase::DefaultBranch);
        assert!(matches!(request.layout, WorkspacePresetLayout::Single));
        assert!(!request.select, "agent callers never take focus by default");
        assert!(request.preset.is_none() && request.pr_number.is_none());
        assert!(request.initial_chat.is_none() && request.reserved_id.is_none());

        let reserved = AppStateStore::reserve_workspace_id();
        let model = crate::agent_capability::ModelSelection::default();
        let request = parse(serde_json::json!({
            "repo_path": "/repo", "branch": "feat", "new_branch": false, "base": "release",
            "start_from_origin": true, "layout": "empty", "agent_preset_id": "claude",
            "initial_prompt": "Fix it", "pr_number": 7, "select": true,
            "client_request_id": "key-1"
        }))
        .unwrap()
        .into_request(Some(model.clone()), Some(reserved.clone()))
        .await
        .unwrap();
        assert!(!request.new_branch);
        // An existing branch has no base to resolve, so origin policy is moot.
        assert_eq!(request.base, WorktreeBase::Ref("release".into()));
        assert!(matches!(request.layout, WorkspacePresetLayout::Empty));
        let preset = request.preset.unwrap();
        assert_eq!(
            (preset.preset_id.as_str(), preset.initial_prompt.as_deref()),
            ("claude", Some("Fix it"))
        );
        assert_eq!(preset.model_selection, Some(model));
        assert_eq!((request.pr_number, request.select), (Some(7), true));
        assert_eq!(request.reserved_id, Some(reserved));

        let error = parse(serde_json::json!({"repo_path": "/repo", "branch": "b", "bogus": 1}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown field `bogus`"), "{error}");
        let error = parse(serde_json::json!({"repo_path": "/repo", "branch": "b", "layout": "grid"}))
            .unwrap()
            .into_request(None, None)
            .await
            .unwrap_err();
        assert_eq!(error, "Unsupported layout: grid");

        // start_from_origin pins a new branch's base to an exact commit.
        let dir = tempfile::tempdir().unwrap();
        let trunk = repo(dir.path(), "project", "trunk");
        let tip = git(&trunk, &["rev-parse", "HEAD"]);
        async fn pinned(args: serde_json::Value) -> Result<WorktreeRequest, String> {
            let args: WorktreeArgs = serde_json::from_value(args).unwrap();
            args.into_request(None, None).await
        }
        let local = pinned(serde_json::json!({
            "repo_path": trunk, "branch": "feat", "start_from_origin": false
        }))
        .await
        .unwrap();
        assert_eq!(
            local.base,
            WorktreeBase::Commit { sha: tip.clone(), label: Some("trunk".into()) }
        );
        let error = pinned(serde_json::json!({
            "repo_path": trunk, "branch": "feat", "start_from_origin": true
        }))
        .await
        .unwrap_err();
        assert!(error.starts_with("operation_failed: "), "no origin to fetch: {error}");
        git(&trunk, &["checkout", "--detach"]);
        let detached = pinned(serde_json::json!({
            "repo_path": trunk, "branch": "feat", "start_from_origin": false
        }))
        .await
        .unwrap();
        assert_eq!(detached.base, WorktreeBase::Commit { sha: tip, label: None });
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_worktree_takes_the_reserved_id_and_forks_the_default_branch() {
        if isolated(
            "workspace_service::tests::create_worktree_takes_the_reserved_id_and_forks_the_default_branch",
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let project = repo(dir.path(), "project", "main");
        let main = git(&project, &["rev-parse", "HEAD"]);
        // The root checkout sits on another branch with its own commit, so
        // forking HEAD instead of the default branch would show.
        git(&project, &["checkout", "-b", "side"]);
        git(&project, &["commit", "--allow-empty", "-m", "side only"]);
        let state = AppStateStore::default();
        state.create_empty_workspace_at_path(project.clone());
        let focused = state.snapshot().active_workspace_id;
        let app = mock_app(state);
        let service = WorkspaceService::new(app.handle());
        let args = |new_branch: bool| {
            serde_json::from_value::<WorktreeArgs>(serde_json::json!({
                "repo_path": project, "branch": "feat", "new_branch": new_branch,
                "layout": "empty"
            }))
            .unwrap()
        };

        let reserved = AppStateStore::reserve_workspace_id();
        let request = args(true).into_request(None, Some(reserved.clone())).await.unwrap();
        let created = service.create_worktree(request).await.unwrap();
        let path = crate::git::conventional_worktree_path(&project, "feat").unwrap();
        assert_eq!(created.workspace_id, reserved.0);
        assert_eq!(created.cwd, path.display().to_string());
        assert!(!created.adopted);
        assert_eq!(created.base_branch.as_deref(), Some("main"));
        assert_eq!(git(&path, &["rev-parse", "HEAD"]), main);
        let state = app.state::<AppStateStore>();
        let workspace = state.find_workspace(&reserved.0).unwrap();
        assert_eq!(workspace.worktree_path.as_deref(), Some(created.cwd.as_str()));
        assert_eq!(workspace.base_branch.as_deref(), Some("main"));
        assert_eq!(state.snapshot().active_workspace_id, focused, "created in the background");

        let count = state.snapshot().workspaces.len();
        let unused = AppStateStore::reserve_workspace_id();
        let request = args(false).into_request(None, Some(unused.clone())).await.unwrap();
        let adopted = service.create_worktree(request).await.unwrap();
        assert!(adopted.adopted);
        assert_eq!(adopted.workspace_id, reserved.0, "the live checkout is adopted");
        assert_eq!((adopted.cwd.as_str(), adopted.base_branch), (created.cwd.as_str(), None));
        assert_eq!(state.snapshot().workspaces.len(), count);
        assert!(state.find_workspace(&unused.0).is_none());
        assert_eq!(state.snapshot().active_workspace_id, focused);
    }

    /// Passes `passes` times, then refuses as a revoked grant does.
    fn refuses_after(passes: usize) -> impl Fn() -> Result<(), String> + Send + Sync + 'static {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        move || {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < passes {
                Ok(())
            } else {
                Err("grant_revoked: withdrawn".into())
            }
        }
    }

    fn withdrawn_after(passes: usize) -> Checkpoint {
        Checkpoint::new(refuses_after(passes))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_worktree_stops_where_its_checkpoint_is_withdrawn() {
        if isolated("workspace_service::tests::create_worktree_stops_where_its_checkpoint_is_withdrawn") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let project = repo(dir.path(), "project", "main");
        let state = AppStateStore::default();
        state.create_empty_workspace_at_path(project.clone());
        let app = mock_app(state);
        let state = app.state::<AppStateStore>();
        let count = state.snapshot().workspaces.len();
        let request = |branch: &str, reserved: &WorkspaceId| WorktreeRequest {
            repo_path: project.display().to_string(),
            branch: branch.into(),
            new_branch: true,
            base: WorktreeBase::RepoHead,
            layout: WorkspacePresetLayout::Empty,
            preset: None,
            pr_number: None,
            select: false,
            initial_chat: None,
            reserved_id: Some(reserved.clone()),
        };
        let checked = |checkpoint| WorkspaceService::new(app.handle()).checked(checkpoint);
        let db = app.state::<crate::database::DatabaseStore>();
        let closed = || crate::worktree_cleanup::closed_paths(&db);

        // Withdrawn while it waited for the lifecycle lock: no checkout.
        let reserved = AppStateStore::reserve_workspace_id();
        let error = checked(withdrawn_after(0)).create_worktree(request("early", &reserved)).await.unwrap_err();
        assert_eq!(error, "grant_revoked: withdrawn");
        assert!(!crate::git::conventional_worktree_path(&project, "early").unwrap().exists());
        assert_eq!(git(&project, &["branch", "--list", "early"]), "");
        assert!(closed().is_empty());

        // Withdrawn while git checked out: the checkout stays, and no
        // workspace claims it or starts anything in it. Cleanup remembers it
        // as a closed workspace's checkout, so it is offered once stale.
        let error = checked(withdrawn_after(1)).create_worktree(request("late", &reserved)).await.unwrap_err();
        let path = crate::git::conventional_worktree_path(&project, "late").unwrap();
        assert_eq!(
            error,
            format!("grant_revoked: withdrawn; the checkout at {} was kept and nothing was started in it", path.display())
        );
        assert!(path.join(".git").exists());
        assert!(state.find_workspace(&reserved.0).is_none());
        assert_eq!(state.snapshot().workspaces.len(), count);
        assert_eq!(closed(), [path.display().to_string()]);

        // A later worktree create of that branch takes the kept checkout.
        let created = WorkspaceService::new(app.handle()).create_worktree(request("late", &reserved)).await.unwrap();
        assert_eq!((created.workspace_id.as_str(), created.cwd.as_str()), (reserved.0.as_str(), path.to_str().unwrap()));
        assert!(!created.adopted);

        // A checkout that was already there before the call is not its to
        // hand to cleanup.
        let holder = AppStateStore::reserve_workspace_id();
        WorkspaceService::new(app.handle()).create_worktree(request("held", &holder)).await.unwrap();
        let refused = AppStateStore::reserve_workspace_id();
        let error = checked(withdrawn_after(1)).create_worktree(request("held", &refused)).await.unwrap_err();
        assert!(error.starts_with("grant_revoked: withdrawn; the checkout at "), "{error}");
        assert!(state.find_workspace(&refused.0).is_none());
        assert!(state.find_workspace(&holder.0).is_some());
        assert_eq!(closed(), [path.display().to_string()]);
    }

    // Creation through `open` starts a terminal; a checkpoint withdrawn
    // after the git reads stops it before the workspace exists, so this test
    // starts none.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn open_stops_where_its_checkpoint_is_withdrawn() {
        if isolated("workspace_service::tests::open_stops_where_its_checkpoint_is_withdrawn") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh");
        std::fs::create_dir(&fresh).unwrap();
        let state = AppStateStore::default();
        state.create_empty_workspace_at_path(dir.path().to_path_buf());
        let other = state.create_empty_workspace_at_path(dir.path().join("elsewhere"));
        state.activate_workspace(&other.0);
        let app = mock_app(state);
        let state = app.state::<AppStateStore>();
        let count = state.snapshot().workspaces.len();
        let open = |path: &Path, select| OpenWorkspace {
            path: Some(path.display().to_string()),
            select,
            reuse_open: true,
            reserved_id: None,
        };
        let checked = |checkpoint| WorkspaceService::new(app.handle()).checked(checkpoint);

        // Withdrawn while it waited for the lifecycle lock: an open folder
        // is not even focused.
        let error = checked(withdrawn_after(0)).open(open(dir.path(), true)).await.unwrap_err();
        assert_eq!(error, "grant_revoked: withdrawn");
        assert_eq!(state.snapshot().active_workspace_id, other);

        // Withdrawn while it read git: no workspace was made, so focus stays
        // where it was.
        let error = checked(withdrawn_after(1)).open(open(&fresh, true)).await.unwrap_err();
        assert_eq!(error, "grant_revoked: withdrawn");
        let snapshot = state.snapshot();
        assert_eq!(snapshot.workspaces.len(), count);
        assert!(snapshot.workspaces.iter().all(|w| Path::new(&w.cwd) != fresh), "{:?}", snapshot.workspaces);
        assert_eq!(snapshot.active_workspace_id, other);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_stops_where_its_checkpoint_is_withdrawn() {
        if isolated("workspace_service::tests::close_stops_where_its_checkpoint_is_withdrawn") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let project = repo(dir.path(), "project", "main");
        let checkout = dir.path().join("checkout");
        git(&project, &["worktree", "add", "-b", "feat", checkout.to_str().unwrap()]);
        let state = AppStateStore::default();
        let closing = state.create_empty_workspace_at_path(checkout.clone()).0;
        state.set_workspace_worktree(&closing, checkout.display().to_string(), "feat".into());
        let app = mock_app(state);
        let state = app.state::<AppStateStore>();
        let close = || CloseWorkspace {
            workspace_id: closing.clone(),
            remove_worktree: true,
            delete_branch: true,
            force: false,
            commits: CommitLoss::Allow,
        };
        let checked = |checkpoint| WorkspaceService::new(app.handle()).checked(checkpoint);

        // Withdrawn while the pre-flight read git, while Hermes chats
        // stopped, or while teardown ran: the workspace stays as it was.
        for passes in [0, 1, 2] {
            let error = checked(withdrawn_after(passes)).close(close()).await.unwrap_err();
            assert_eq!(error, "grant_revoked: withdrawn");
            assert!(state.find_workspace(&closing).is_some());
        }

        // Withdrawn while its agents stopped and the lifecycle lock was taken:
        // the close stands, and the files and the branch stay.
        let error = checked(withdrawn_after(3)).close(close()).await.unwrap_err();
        assert_eq!(error, "grant_revoked: withdrawn; the workspace was closed and its files were kept");
        assert!(state.find_workspace(&closing).is_none());
        assert!(checkout.join(".git").exists());
        git(&project, &["rev-parse", "--verify", "refs/heads/feat"]);
    }

    /// A new worktree's setup runs once the create returned, so it asks the
    /// background check, never the call's own: that one ends with the call
    /// (an agent's turn, a plugin invocation). Each case is (checkpoint, files
    /// in the checkout afterwards, how setup ended, the command it stopped
    /// before).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn worktree_setup_asks_only_its_background_checkpoint() {
        if isolated("workspace_service::tests::worktree_setup_asks_only_its_background_checkpoint") {
            return;
        }
        use tauri::Listener;
        let dir = tempfile::tempdir().unwrap();
        let project = repo(dir.path(), "project", "main");
        std::fs::create_dir(project.join(".codemux")).unwrap();
        std::fs::write(project.join(".codemux/config.json"), r#"{"setup": ["touch first", "touch second"]}"#).unwrap();
        std::fs::write(project.join(".env"), "SECRET=1").unwrap();
        let state = AppStateStore::default();
        state.create_empty_workspace_at_path(project.clone());
        let app = mock_app(state);
        let (sender, mut ended) = tokio::sync::mpsc::unbounded_channel();
        for name in ["workspace-setup-failed", "workspace-setup-complete"] {
            let sender = sender.clone();
            app.listen_any(name, move |event| {
                let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
                let _ = sender.send((name, payload));
            });
        }

        type Case = (Checkpoint, &'static [&'static str], &'static str, Option<&'static str>);
        let cases: [(&str, Case); 4] = [
            // The call's check would refuse now; setup does not ask it.
            ("ended-call", (withdrawn_after(2), &[".env", "first", "second"], "workspace-setup-complete", None)),
            // Withdrawn before setup began: nothing copied, nothing run.
            ("before-copy", (Checkpoint::default().with_background(refuses_after(0)), &[], "workspace-setup-failed", Some("touch first"))),
            // Withdrawn while the includes were copied.
            ("after-copy", (Checkpoint::default().with_background(refuses_after(1)), &[".env"], "workspace-setup-failed", Some("touch first"))),
            // Withdrawn while a command ran.
            ("mid-setup", (Checkpoint::default().with_background(refuses_after(2)), &[".env", "first"], "workspace-setup-failed", Some("touch second"))),
        ];
        for (branch, (checkpoint, made, event, stopped_before)) in cases {
            let request = WorktreeRequest {
                repo_path: project.display().to_string(),
                branch: branch.into(),
                new_branch: true,
                base: WorktreeBase::RepoHead,
                layout: WorkspacePresetLayout::Empty,
                preset: None,
                pr_number: None,
                select: false,
                initial_chat: None,
                reserved_id: None,
            };
            let created = WorkspaceService::new(app.handle()).checked(checkpoint).create_worktree(request).await.unwrap();
            let (name, payload) = tokio::time::timeout(std::time::Duration::from_secs(15), ended.recv()).await.unwrap().unwrap();
            assert_eq!((name, payload["workspace_id"].as_str()), (event, Some(created.workspace_id.as_str())), "{branch}: {payload}");
            if let Some(command) = stopped_before {
                assert_eq!(payload["command"], command, "{branch}");
                assert_eq!(payload["stderr"], "grant_revoked: withdrawn\nSetup stopped before this command started.", "{branch}");
            }
            let present: Vec<_> = [".env", "first", "second"].into_iter().filter(|file| Path::new(&created.cwd).join(file).exists()).collect();
            assert_eq!(present, made, "{branch}");
        }
    }

    #[test]
    fn parse_layout_accepts_known_layouts_only() {
        for layout in ["single", "pair", "quad", "six", "eight", "shell_browser", "empty"] {
            let parsed = parse_layout(layout).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), layout);
        }
        assert_eq!(parse_layout("grid").unwrap_err(), "Unsupported layout: grid");
    }

    #[test]
    fn known_root_accepts_repo_subdir_and_linked_worktree_only() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path(), "project", "main");
        std::fs::create_dir(repo.join("sub")).unwrap();
        let linked = dir.path().join("linked");
        git(&repo, &["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, repo.join("escape")).unwrap();
        let folder = dir.path().join("plain-folder");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        let known = known_roots([repo.clone(), folder.clone()], None);
        assert_eq!(known.len(), 2);

        for inside in [repo.clone(), repo.join("sub"), linked.clone(), folder.clone()] {
            assert_eq!(
                resolve_under_known_root(&known, &inside).unwrap(),
                inside.canonicalize().unwrap(),
                "{}",
                inside.display()
            );
        }
        let mut refused = vec![outside, folder.join("sub")];
        #[cfg(unix)]
        refused.push(repo.join("escape"));
        for path in refused {
            let error = resolve_under_known_root(&known, &path).unwrap_err();
            assert!(error.starts_with("unknown_project_root: "), "{}: {error}", path.display());
        }
        let missing = resolve_under_known_root(&known, &dir.path().join("missing")).unwrap_err();
        assert!(missing.starts_with("invalid_arguments: "), "{missing}");
        std::fs::write(repo.join("file.txt"), "x").unwrap();
        let file = resolve_under_known_root(&known, &repo.join("file.txt")).unwrap_err();
        assert!(file.starts_with("invalid_arguments: "), "{file}");

        // A relative path would resolve against the app's own folder.
        for relative in [".", "project", "../outside"] {
            let error = resolve_under_known_root(&known, Path::new(relative)).unwrap_err();
            assert!(
                error.starts_with("invalid_arguments: path must be absolute"),
                "{relative}: {error}"
            );
        }
        // `~` is the user's home, as in the legacy open.
        if let Some(home) = std::env::var_os("HOME").and_then(|home| PathBuf::from(home).canonicalize().ok()) {
            for tilde in ["~", "~/."] {
                assert_eq!(resolve_under_known_root(&[home.clone()], Path::new(tilde)).unwrap(), home);
            }
        }
    }

    #[test]
    fn known_root_reads_relative_gitdirs_against_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let project = repo(dir.path(), "project", "main");
        let canonical = project.canonicalize().unwrap();
        let source = repo(dir.path(), "lib-source", "main");
        git(
            &project,
            &["-c", "protocol.file.allow=always", "submodule", "add", source.to_str().unwrap(), "vendor/lib"],
        );
        let submodule = project.join("vendor").join("lib");
        let gitdir = |checkout: &Path| std::fs::read_to_string(checkout.join(".git")).unwrap();
        assert!(gitdir(&submodule).starts_with("gitdir: ../"), "{}", gitdir(&submodule));
        // What `git worktree add --relative-paths` writes, without needing a
        // git new enough to have the flag.
        let linked = dir.path().join("linked-relative");
        git(&project, &["worktree", "add", "-b", "linked-relative", linked.to_str().unwrap()]);
        let admin = PathBuf::from(gitdir(&linked).trim().strip_prefix("gitdir: ").unwrap());
        let name = admin.file_name().unwrap().to_str().unwrap();
        std::fs::write(linked.join(".git"), format!("gitdir: ../project/.git/worktrees/{name}\n"))
            .unwrap();
        assert_eq!(git(&linked, &["rev-parse", "--abbrev-ref", "HEAD"]), "linked-relative");

        assert_eq!(known_roots([submodule.clone(), linked.clone()], None), vec![canonical.clone()]);
        let known = known_roots([project.clone()], None);
        for inside in [&submodule, &submodule.join("."), &linked] {
            assert_eq!(
                resolve_under_known_root(&known, inside).unwrap(),
                inside.canonicalize().unwrap(),
                "{}",
                inside.display()
            );
        }
    }

    #[test]
    fn known_roots_exclude_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        // A dotfiles repository at $HOME.
        git(&home, &["init", "-b", "main"]);
        let project = home.join("project");
        std::fs::create_dir(&project).unwrap();
        let nested = repo(&home, "nested-repo", "main");

        let known = known_roots(
            [home.clone(), project.clone(), nested.join("."), home.join("missing")],
            Some(&home),
        );
        assert_eq!(
            known,
            vec![project.canonicalize().unwrap(), nested.canonicalize().unwrap()],
            "$HOME is never known; a folder under a dotfiles $HOME stands for itself"
        );
        assert!(resolve_under_known_root(&known, &home)
            .unwrap_err()
            .starts_with("unknown_project_root: "));
        assert!(resolve_under_known_root(&known, &project).is_ok());

        // A checkout above $HOME (say at `/`) is excluded like $HOME itself,
        // so it makes nothing below it known.
        let top = dir.path().join("top");
        let users = top.join("users");
        let below_home = users.join("me");
        let below_project = below_home.join("project");
        let elsewhere = top.join("opt");
        std::fs::create_dir_all(&below_project).unwrap();
        std::fs::create_dir(&elsewhere).unwrap();
        git(&top, &["init", "-b", "main"]);
        let known = known_roots(
            [top.clone(), users, below_home.clone(), below_project.clone()],
            Some(&below_home),
        );
        assert_eq!(known, vec![below_project.canonicalize().unwrap()]);
        for refused in [&top, &below_home, &elsewhere] {
            let error = resolve_under_known_root(&known, refused).unwrap_err();
            assert!(error.starts_with("unknown_project_root: "), "{}: {error}", refused.display());
        }
    }

    #[test]
    fn known_project_roots_skip_home_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let project = repo(dir.path(), "project", "main");
        let other = dir.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let state = AppStateStore::default();
        let open = state.create_workspace_at_path(project.join("."));
        state.set_workspace_project_root(&open.0, project.display().to_string());
        let home_like = state.create_workspace_at_path(other.clone());
        state.set_workspace_type(&home_like.0, WorkspaceType::Home);
        // Under a dotfiles $HOME, opening a folder records $HOME as its
        // project root; the folder itself must still count.
        let home = dir.path().join("home");
        let under_home = home.join("project");
        std::fs::create_dir_all(&under_home).unwrap();
        git(&home, &["init", "-b", "main"]);
        let dotfiles = state.create_workspace_at_path(under_home.clone());
        let recorded = crate::config::workspace_config::find_git_root(&under_home).unwrap();
        assert_eq!(recorded, home, "the project root open_workspace_impl records");
        state.set_workspace_project_root(&dotfiles.0, recorded.display().to_string());
        let app = mock_app(state);

        let known = known_project_roots(app.handle());
        assert!(known.contains(&project.canonicalize().unwrap()));
        assert!(!known.contains(&other.canonicalize().unwrap()), "{known:?}");
        let known = known_roots(known_folders(app.handle()), Some(&home));
        assert!(known.contains(&under_home.canonicalize().unwrap()), "{known:?}");
        assert!(!known.contains(&home.canonicalize().unwrap()), "{known:?}");
        assert!(!known.contains(&other.canonicalize().unwrap()), "{known:?}");
    }

    #[tokio::test]
    async fn check_new_worktree_refuses_existing_branch_path_and_shorthand() {
        if isolated(
            "workspace_service::tests::check_new_worktree_refuses_existing_branch_path_and_shorthand",
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(dir.path(), "project", "main");
        git(&repo, &["branch", "taken"]);
        git(&repo, &["checkout", "-b", "previous"]);
        git(&repo, &["checkout", "main"]);
        std::fs::create_dir_all(crate::git::conventional_worktree_path(&repo, "on-disk").unwrap())
            .unwrap();

        check_new_worktree(&repo, "fresh/branch").await.unwrap();
        for taken in ["taken", "on-disk"] {
            let error = check_new_worktree(&repo, taken).await.unwrap_err();
            assert!(error.starts_with("conflict: "), "{taken}: {error}");
        }
        let long = "x".repeat(257);
        for invalid in ["-x", "a..b", "@{-1}", "", "bad\nname", long.as_str()] {
            let error = check_new_worktree(&repo, invalid).await.unwrap_err();
            assert!(error.starts_with("invalid_input: "), "{invalid:?}: {error}");
        }
    }

    #[tokio::test]
    async fn resolve_base_local_never_fetches_and_required_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let seed = repo(dir.path(), "seed", "main");
        let origin = dir.path().join("origin.git");
        git(dir.path(), &["clone", "--bare", seed.to_str().unwrap(), origin.to_str().unwrap()]);
        let local = dir.path().join("local");
        git(dir.path(), &["clone", origin.to_str().unwrap(), local.to_str().unwrap()]);
        let local_tip = git(&local, &["rev-parse", "HEAD"]);
        // Origin moves on without the local clone fetching.
        git(&seed, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&seed, &["commit", "--allow-empty", "-m", "origin only"]);
        git(&seed, &["push", "origin", "main"]);
        let origin_tip = git(&seed, &["rev-parse", "HEAD"]);
        assert_ne!(origin_tip, local_tip);

        let local_base = resolve_base(&local, "main", OriginPolicy::Local).await.unwrap();
        assert_eq!(local_base, ResolvedBase { commit: local_tip.clone(), label: "main".into() });
        assert_eq!(
            git(&local, &["rev-parse", "refs/remotes/origin/main"]),
            local_tip,
            "a local base never fetches"
        );

        // Each fetching policy starts from the stale origin ref, so reaching
        // the origin tip proves that it fetched.
        let make_stale = || git(&local, &["update-ref", "refs/remotes/origin/main", &local_tip]);
        let origin_base = ResolvedBase { commit: origin_tip.clone(), label: "main".into() };
        for base in ["main", "origin/main"] {
            make_stale();
            assert_eq!(resolve_base(&local, base, OriginPolicy::PreferFresh).await.unwrap(), origin_base);
            make_stale();
            assert_eq!(resolve_base(&local, base, OriginPolicy::Required).await.unwrap(), origin_base);
        }

        git(&local, &["remote", "set-url", "origin", dir.path().join("gone.git").to_str().unwrap()]);
        let error = resolve_base(&local, "main", OriginPolicy::Required).await.unwrap_err();
        assert!(error.starts_with("operation_failed: "), "{error}");
        // A failed best-effort fetch keeps the last fetched origin ref.
        assert_eq!(resolve_base(&local, "main", OriginPolicy::PreferFresh).await.unwrap(), origin_base);
        let error = resolve_base(&local, "@{-1}", OriginPolicy::Required).await.unwrap_err();
        assert!(error.starts_with("invalid_input: "), "{error}");
        assert!(resolve_base(&local, "-main", OriginPolicy::Local)
            .await
            .unwrap_err()
            .starts_with("invalid_input: "));

        let no_origin = repo(dir.path(), "no-origin", "main");
        let head = git(&no_origin, &["rev-parse", "HEAD"]);
        for base in ["main", "HEAD", head.as_str()] {
            let fresh = resolve_base(&no_origin, base, OriginPolicy::PreferFresh).await.unwrap();
            assert_eq!(fresh, ResolvedBase { commit: head.clone(), label: base.into() });
        }
    }

    #[test]
    fn native_control_is_one_rule_for_listing_and_thread_tools() {
        let local = AppStateStore::default().snapshot().workspaces[0].clone();
        let imported = WorkspaceSnapshot { imported_snapshot_only: Some(true), ..local.clone() };
        let device = WorkspaceSnapshot { host_id: Some(7), ..local.clone() };
        let remote_folder = WorkspaceSnapshot { remote_cwd: Some("/srv/app".into()), ..local.clone() };
        for (workspace, expected) in [
            (&local, Ok(())),
            (&imported, Err(NotControllable::ImportedSnapshot)),
            (&device, Err(NotControllable::Remote)),
            (&remote_folder, Err(NotControllable::Remote)),
        ] {
            assert_eq!(native_control(workspace), expected, "{:?}", workspace.workspace_id);
            assert_eq!(WorkspaceSummary::of(workspace).native_control_available, expected.is_ok());
        }
    }

    #[test]
    fn info_targets_one_workspace_and_summaries_list_every_one() {
        let state = AppStateStore::default();
        let child = state.create_workspace_at_path(PathBuf::from("/tmp/codemux-info-child"));
        state.set_workspace_parent(&child.0, Some("parent-workspace".into()));
        let app = mock_app(state);
        let service = WorkspaceService::new(app.handle());

        let active = app.state::<AppStateStore>().snapshot().active_workspace_id;
        assert_eq!(service.info(WorkspaceTarget::Active).unwrap().workspace_id, active);
        assert_eq!(service.info(WorkspaceTarget::Id(&child.0)).unwrap().workspace_id, child);
        assert_eq!(service.info(WorkspaceTarget::Id("missing")).unwrap_err(), InfoError::NotFound);
        assert_eq!(service.info(WorkspaceTarget::Id("")).unwrap_err(), InfoError::NotFound);

        let summaries = service.summaries();
        let snapshots = service.snapshots();
        assert_eq!(summaries.len(), snapshots.len());
        let row = summaries.iter().find(|row| row.workspace_id == child.0).unwrap();
        assert_eq!(row.parent_workspace_id.as_deref(), Some("parent-workspace"));
        assert!(row.native_control_available);
        let mut keys: Vec<String> = serde_json::to_value(row)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "branch",
                "cwd",
                "native_control_available",
                "parent_workspace_id",
                "project_root",
                "title",
                "workspace_id",
                "worktree_path"
            ]
        );

        let empty = mock_app(AppStateStore::default());
        let state = empty.state::<AppStateStore>();
        for workspace in state.snapshot().workspaces {
            state.close_workspace(&workspace.workspace_id.0).unwrap();
        }
        assert_eq!(
            WorkspaceService::new(empty.handle()).info(WorkspaceTarget::Active).unwrap_err(),
            InfoError::NoActive
        );
    }

    // Only the paths that create nothing. A created workspace starts a
    // terminal (`open` always uses the single layout), which would spawn a
    // shell or the terminal daemon from the test binary, so creation through
    // `open` is not exercised here.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn open_reuses_an_open_folder_or_reservation_without_taking_focus() {
        if isolated(
            "workspace_service::tests::open_reuses_an_open_folder_or_reservation_without_taking_focus",
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let state = AppStateStore::default();
        let root = state.create_empty_workspace_at_path(dir.path().to_path_buf());
        let other = state.create_empty_workspace_at_path(dir.path().join("elsewhere"));
        state.activate_workspace(&other.0);
        let app = mock_app(state);
        let service = WorkspaceService::new(app.handle());
        let state = app.state::<AppStateStore>();
        let count = state.snapshot().workspaces.len();
        let reuse = |path: &Path, select| OpenWorkspace {
            path: Some(path.display().to_string()),
            select,
            reuse_open: true,
            reserved_id: None,
        };

        let opened = service.open(reuse(dir.path(), false)).await.unwrap();
        assert_eq!(
            opened,
            OpenedWorkspace {
                workspace_id: root.0.clone(),
                cwd: dir.path().display().to_string(),
                created: false
            }
        );
        assert_eq!(state.snapshot().workspaces.len(), count);
        assert_eq!(state.snapshot().active_workspace_id, other, "no focus change");

        let selected = service.open(reuse(&dir.path().join("."), true)).await.unwrap();
        assert_eq!((selected.workspace_id.as_str(), selected.created), (root.0.as_str(), false));
        assert_eq!(state.snapshot().active_workspace_id, root, "select focuses it");

        // An open folder is reused only on request, and a reserved id that
        // already names a workspace is returned rather than recreated.
        let reserved = OpenWorkspace {
            path: Some(dir.path().display().to_string()),
            select: false,
            reuse_open: false,
            reserved_id: Some(other.clone()),
        };
        let existing = service.open(reserved).await.unwrap();
        assert_eq!((existing.workspace_id, existing.created), (other.0.clone(), false));
        assert_eq!(state.snapshot().workspaces.len(), count);
    }
}
