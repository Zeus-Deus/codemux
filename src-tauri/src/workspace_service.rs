//! Opening, creating and closing workspaces and worktrees for every caller:
//! the desktop, the control socket and outside tools.
//!
//! The requests are typed and the service never sees a caller. Desktop
//! commands pass every choice explicitly; callers that act for an agent
//! decide authorization themselves and only then come here.
use crate::commands::workspace::WorkspaceCreated;
use crate::state::{
    AppStateStore, InitialChatPane, WorkspaceId, WorkspacePresetLayout, WorkspaceSnapshot,
    WorkspaceType,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

/// Where a new branch forks from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WorktreeBase {
    /// The repository checkout's HEAD (`git worktree add -b` with no base).
    /// The desktop's "no base selected".
    RepoHead,
    /// A caller-named ref: best-effort `origin/<ref>` fetch and preference,
    /// exactly as `git::git_create_worktree` does.
    Ref(String),
    /// An exact commit, never fetched. `label` is recorded as the base branch;
    /// without one the commit itself is.
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
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ClosedWorkspace {
    pub workspace_id: String,
    pub closed: bool,
    pub worktree_removed: bool,
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
            native_control_available: !workspace.is_local_import_snapshot_only()
                && workspace.host_id.is_none(),
        }
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
}

impl<R: Runtime> WorkspaceService<R> {
    pub(crate) fn new(app: &AppHandle<R>) -> Self {
        Self { app: app.clone() }
    }

    /// Create or open a workspace at a folder. Runs setup scripts and writes
    /// `.mcp.json` only when it creates one.
    pub(crate) async fn open(&self, request: OpenWorkspace) -> Result<OpenedWorkspace, String> {
        crate::commands::workspace::open_workspace_impl(self.app.clone(), request).await
    }

    pub(crate) async fn create_worktree(
        &self,
        request: WorktreeRequest,
    ) -> Result<WorkspaceCreated, String> {
        crate::commands::workspace::create_worktree_from_request(self.app.clone(), request).await
    }

    /// Close a workspace through the same teardown and deletion guards as the
    /// desktop's close dialog. Error text passes through verbatim.
    pub(crate) async fn close(&self, request: CloseWorkspace) -> Result<ClosedWorkspace, String> {
        let state = self.app.state::<AppStateStore>();
        let db = self.app.state::<crate::database::DatabaseStore>();
        crate::commands::workspace::close_workspace_with_worktree_impl(
            self.app.clone(),
            &state,
            &db,
            request.workspace_id.clone(),
            request.remove_worktree,
            Some(request.delete_branch),
            Some(request.force),
        )
        .await?;
        // A close that was asked to remove the worktree and succeeded did:
        // a workspace without one is refused before any teardown.
        Ok(ClosedWorkspace {
            workspace_id: request.workspace_id,
            closed: true,
            worktree_removed: request.remove_worktree,
        })
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

/// How many of the most recent Recent projects count as known.
const RECENT_PROJECT_LIMIT: u32 = 200;

/// Project roots this computer already works in: live and archived local
/// workspaces' project roots, plus Recent projects. Canonical and git-rooted.
/// Home workspaces and `$HOME` itself are never known, so a dotfiles repo at
/// `$HOME` cannot make every folder known.
pub(crate) fn known_project_roots<R: Runtime>(app: &AppHandle<R>) -> Vec<PathBuf> {
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
        .map(|w| PathBuf::from(w.project_root.as_deref().unwrap_or(&w.cwd)));
    // Remote workspaces cannot be archived, so every entry is a local path.
    let archived = snapshot
        .archived_workspaces
        .iter()
        .map(|entry| PathBuf::from(entry.project_root.as_deref().unwrap_or(&entry.cwd)));
    let recent = app
        .try_state::<crate::database::DatabaseStore>()
        .map(|db| db.get_recent_projects(RECENT_PROJECT_LIMIT))
        .unwrap_or_default()
        .into_iter()
        .map(|project| PathBuf::from(project.path));
    known_roots(live.chain(archived).chain(recent), dirs::home_dir().as_deref())
}

/// The pure part of [`known_project_roots`]: canonical git roots of the
/// folders that exist, without `home`, deduplicated. A folder whose git root
/// is `home` stands for itself.
fn known_roots(folders: impl IntoIterator<Item = PathBuf>, home: Option<&Path>) -> Vec<PathBuf> {
    let home = home.and_then(|home| home.canonicalize().ok());
    let mut roots = Vec::new();
    for folder in folders {
        let Ok(folder) = folder.canonicalize() else {
            continue;
        };
        let root = git_root(&folder)
            .filter(|root| Some(root) != home.as_ref())
            .unwrap_or(folder);
        if Some(&root) != home.as_ref() && !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

fn git_root(folder: &Path) -> Option<PathBuf> {
    crate::config::workspace_config::find_git_root(folder).and_then(|root| root.canonicalize().ok())
}

/// Checks that `path` is an existing folder whose git root (or, outside git,
/// the folder itself) is one of `known`. A subfolder of a known repository
/// and that repository's linked worktrees pass; a symlink is judged by where
/// it points. Returns the canonical folder.
pub(crate) fn resolve_under_known_root(known: &[PathBuf], path: &Path) -> Result<PathBuf, String> {
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
            if fetchable
                && fetch_origin(repo, branch).await.is_ok()
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
        let app = mock_app(state);

        let known = known_project_roots(app.handle());
        assert!(known.contains(&project.canonicalize().unwrap()));
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

        for base in ["main", "origin/main"] {
            let required = resolve_base(&local, base, OriginPolicy::Required).await.unwrap();
            assert_eq!(required, ResolvedBase { commit: origin_tip.clone(), label: "main".into() });
        }
        let fresh = resolve_base(&local, "main", OriginPolicy::PreferFresh).await.unwrap();
        assert_eq!(fresh.commit, origin_tip);

        git(&local, &["remote", "set-url", "origin", dir.path().join("gone.git").to_str().unwrap()]);
        let error = resolve_base(&local, "main", OriginPolicy::Required).await.unwrap_err();
        assert!(error.starts_with("operation_failed: "), "{error}");
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn open_reuses_an_open_folder_and_creates_new_ones_in_the_background() {
        if isolated(
            "workspace_service::tests::open_reuses_an_open_folder_and_creates_new_ones_in_the_background",
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
