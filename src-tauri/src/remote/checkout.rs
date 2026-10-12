//! Host-side checkouts for threads the desktop runs on this device.
//!
//! Backs two one-shot CLI calls the desktop makes over SSH when a thread
//! is sent to a device:
//!
//! - `codemux-remote project ensure`: find this host's checkout of a
//!   project by its git remote, cloning it on first use, so the desktop
//!   never has to copy its own checkout over.
//! - `codemux-remote worktree create`: plan the `worktree_create` tool
//!   call (attach an existing branch or start a new one, default base) and
//!   shape its result.
//!
//! The JSON contracts live on the subcommands in `bin/codemux_remote.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::workspace::{WorkspaceError, WorkspaceStore};
use crate::project_identity::{canonical_remote, git_canonical_remote, remote_host_path};

/// `project ensure` result. Field names are the CLI's stdout contract.
#[derive(Debug, Clone, Serialize)]
pub struct EnsuredProject {
    /// Absolute path of the checkout on this host.
    pub path: String,
    /// True when this call cloned it.
    pub cloned: bool,
    pub default_branch: Option<String>,
    /// The branch currently checked out, `None` when HEAD is detached.
    pub branch: Option<String>,
    pub project_uid: String,
    pub canonical_remote: String,
}

const URL_SCHEMES: &[&str] = &["ssh", "git", "http", "https", "file", "git+ssh", "ssh+git"];

/// Accept only URLs git clones without side effects: `scheme://…` for the
/// usual transports, or scp-style `[user@]host:path`. Rejects anything
/// that could be read as an option (leading `-`), remote-helper syntax
/// (`ext::…`), and plain words or paths.
pub fn validate_remote_url(url: &str) -> Result<(), String> {
    let not_git = || format!("\"{url}\" doesn't look like a git URL");
    if url.is_empty() {
        return Err("a git remote URL is required".into());
    }
    if url.starts_with('-') {
        return Err(not_git());
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) || url.contains("::") {
        return Err(not_git());
    }
    let looks_ok = match url.split_once("://") {
        Some((scheme, rest)) => {
            URL_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) && !rest.is_empty()
        }
        None => match url.split_once(':') {
            Some((host, path)) => !host.is_empty() && !host.contains('/') && !path.is_empty(),
            None => false,
        },
    };
    if looks_ok && canonical_remote(url).is_some() {
        Ok(())
    } else {
        Err(not_git())
    }
}

/// The repo name from a remote (`git@github.com:acme/App.git` → `App`).
pub fn project_name_from_remote(url: &str) -> Option<String> {
    remote_host_path(url)?
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// The https form of a github.com / gitlab.com remote, for one retry when
/// the given URL fails to clone — typically an ssh URL on a device with no
/// key for that forge. Works for public repos and for devices set up with
/// `gh` or a credential helper. `None` for other forges and https input.
pub fn https_fallback_url(url: &str) -> Option<String> {
    if url.to_ascii_lowercase().starts_with("https://") {
        return None;
    }
    let host_path = remote_host_path(url)?;
    let (host, path) = host_path.split_once('/')?;
    let host = host.to_ascii_lowercase();
    if (host == "github.com" || host == "gitlab.com") && !path.is_empty() {
        Some(format!("https://{host}/{path}.git"))
    } else {
        None
    }
}

/// `project ensure`: this host's checkout of the project at `remote_url`.
///
/// Looks for an existing checkout whose origin is the same repo — first in
/// the workspace registry (`store`), then under `~/.codemux/projects/` —
/// and otherwise clones into `~/.codemux/projects/<name>-<uid8>`. A new or
/// unregistered checkout is recorded in `store` (best-effort) so the
/// desktop's inventory poller lists it. `home` is injected for tests.
pub fn ensure_project(
    home: &Path,
    store: Option<&WorkspaceStore>,
    remote_url: &str,
    name: Option<&str>,
) -> Result<EnsuredProject, String> {
    let url = remote_url.trim();
    validate_remote_url(url)?;
    let canonical =
        canonical_remote(url).ok_or_else(|| format!("\"{url}\" doesn't look like a git URL"))?;
    // The desktop keys a checkout with this remote the same way
    // (`project_uid_for(git_canonical_remote(root), root)`), so the host's
    // copy groups with the desktop's project.
    let project_uid = crate::project_identity::project_uid(&canonical);
    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .or_else(|| project_name_from_remote(url))
        .unwrap_or_else(|| "project".to_string());

    let projects_dir = home.join(".codemux").join("projects");
    let target = projects_dir.join(crate::workspace_paths::project_dir_component(
        Some(&project_uid),
        &name,
    ));

    let registered = store.and_then(|s| find_registered_checkout(s, &project_uid, &canonical));
    let found = registered
        .clone()
        .or_else(|| find_checkout_in(&projects_dir, &canonical, &target));
    let (path, cloned) = match found {
        Some(path) => (path, false),
        None => {
            let cloned = clone_project(url, &canonical, &target)?;
            (target, cloned)
        }
    };

    super::git::ensure_origin_head(&path);
    let default_branch = crate::git::resolve_default_branch(&path);
    let branch = crate::git::current_branch(&path);
    let path = path.to_string_lossy().to_string();

    if registered.is_none() {
        if let Some(store) = store {
            if let Err(e) = register_root(store, &project_uid, &name, &path, branch.clone()) {
                eprintln!("couldn't record {path} in the workspace registry: {e}");
            }
        }
    }

    Ok(EnsuredProject {
        path,
        cloned,
        default_branch,
        branch,
        project_uid,
        canonical_remote: canonical,
    })
}

/// A checkout of `canonical` on disk: a repo root (`.git` directory, not a
/// worktree's `.git` file) whose origin is that repo.
fn is_checkout_of(path: &Path, canonical: &str) -> bool {
    path.join(".git").is_dir() && git_canonical_remote(path).as_deref() == Some(canonical)
}

/// The registry's checkout of the project, preferring its `main` (root)
/// row over a worktree row's `project_root`. Rows are only hints: the path
/// must still be a checkout of the same remote.
fn find_registered_checkout(
    store: &WorkspaceStore,
    project_uid: &str,
    canonical: &str,
) -> Option<PathBuf> {
    let mut candidates: Vec<(bool, PathBuf)> = store
        .list()
        .ok()?
        .into_iter()
        .filter(|w| w.project_uid.as_deref() == Some(project_uid))
        .filter_map(|w| {
            let is_main = w.kind.as_deref() == Some("main");
            let root = if is_main {
                Some(w.path)
            } else {
                w.project_root
            };
            root.map(|p| (is_main, crate::workspace_paths::expand_tilde(Path::new(&p))))
        })
        .collect();
    candidates.sort_by_key(|(is_main, _)| !*is_main);
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .map(|(_, path)| path)
        .filter(|path| seen.insert(path.clone()))
        .find(|path| is_checkout_of(path, canonical))
}

/// A checkout of `canonical` among the folders in `projects_dir`, trying
/// the conventional `preferred` folder first. Skips dot-folders, which are
/// in-progress clones.
fn find_checkout_in(projects_dir: &Path, canonical: &str, preferred: &Path) -> Option<PathBuf> {
    if is_checkout_of(preferred, canonical) {
        return Some(preferred.to_path_buf());
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(projects_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .filter(|path| path != preferred && path.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .find(|path| is_checkout_of(path, canonical))
}

/// Clone `url` to `target` through a sibling temp folder, so a half-done
/// clone never sits at `target` where a later scan would take it for a
/// checkout. Returns false when a concurrent ensure won the race and
/// `target` is already a checkout of the same repo.
fn clone_project(url: &str, canonical: &str, target: &Path) -> Result<bool, String> {
    let occupied = std::fs::read_dir(target)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(target.exists());
    if occupied {
        return Err(format!(
            "{} already exists and isn't a checkout of {canonical}",
            target.display()
        ));
    }
    let (Some(parent), Some(dir_name)) = (target.parent(), target.file_name()) else {
        return Err(format!("can't clone into {}", target.display()));
    };
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
    let dir_name = dir_name.to_string_lossy();
    let tmp_prefix = format!(".{dir_name}.clone-");
    remove_abandoned_clones(parent, &tmp_prefix);
    let tmp = parent.join(format!("{tmp_prefix}{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    let deadline = Instant::now() + super::git::CLONE_TIMEOUT;
    let result = match super::git::clone_repo(url, &tmp, super::git::CLONE_TIMEOUT) {
        Ok(()) => Ok(()),
        Err(first) => match https_retry(url, deadline, Instant::now()) {
            Some((https, left)) => {
                let _ = std::fs::remove_dir_all(&tmp);
                super::git::clone_repo(&https, &tmp, left)
                    .map_err(|retry| format!("{first} (https retry: {retry})"))
            }
            None => Err(first),
        },
    };
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("couldn't clone {canonical}: {e}"));
    }

    match std::fs::rename(&tmp, target) {
        Ok(()) => Ok(true),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            if is_checkout_of(target, canonical) {
                Ok(false)
            } else {
                Err(format!(
                    "couldn't move the clone into {}: {e}",
                    target.display()
                ))
            }
        }
    }
}

/// The https URL and time left for retrying a failed clone. Both attempts
/// share one `deadline`, so the desktop waiting on `project ensure` can
/// outlast them. `None` when the URL has no https form, or when the first
/// attempt used up the deadline: that was a timeout, and a slow link
/// isn't what the retry is for.
fn https_retry(url: &str, deadline: Instant, now: Instant) -> Option<(String, Duration)> {
    let left = deadline
        .checked_duration_since(now)
        .filter(|left| !left.is_zero())?;
    Some((https_fallback_url(url)?, left))
}

/// Remove temp clone folders (`<prefix><pid>`) left by an ensure that was
/// killed mid-clone. A folder whose pid is still running belongs to a
/// concurrent ensure and is left alone.
fn remove_abandoned_clones(parent: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        // Parsed as a positive pid_t: 0 or a negative value would make the
        // liveness probe ask about a whole process group instead.
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix(prefix))
            .and_then(|pid| pid.parse::<i32>().ok())
            .filter(|pid| *pid > 0)
        else {
            continue;
        };
        if !super::manifest::pid_alive(pid as u32) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Record the project root so the desktop's inventory poller lists it and
/// the next ensure finds it without a scan. A `main` row whose folder is
/// gone would win `create`'s one-root-per-project collapse (earliest
/// wins), so those rows are dropped first. Registry rows only; no files
/// are touched.
fn register_root(
    store: &WorkspaceStore,
    project_uid: &str,
    name: &str,
    path: &str,
    branch: Option<String>,
) -> Result<(), WorkspaceError> {
    for row in store.list()? {
        let stale = row.project_uid.as_deref() == Some(project_uid)
            && row.kind.as_deref() == Some("main")
            && !crate::workspace_paths::expand_tilde(Path::new(&row.path)).exists();
        if stale {
            store.close(&row.id)?;
        }
    }
    store.create(Some(name.to_string()), path.to_string(), branch, None)?;
    Ok(())
}

/// `worktree_create` tool arguments for `worktree create`. The branch is
/// attached when it already exists locally or on origin (so a thread can
/// pick up a branch pushed from another device) and created otherwise,
/// off `base` or the repo's default branch.
pub fn worktree_create_args(
    repo: &Path,
    branch: &str,
    base: Option<&str>,
) -> Result<Value, String> {
    if !repo.is_absolute() {
        return Err(format!(
            "the repo path must be absolute: {}",
            repo.display()
        ));
    }
    let repo_root = super::git::git_root(repo)
        .ok_or_else(|| format!("{} isn't a git repository", repo.display()))?;
    let branch = branch.trim();
    if branch.is_empty() || !super::git::is_valid_branch_name(&repo_root, branch) {
        return Err(format!("\"{branch}\" isn't a valid branch name"));
    }
    let base = base.map(str::trim).filter(|b| !b.is_empty());
    if base.is_some_and(|b| b.starts_with('-')) {
        return Err(format!(
            "\"{}\" isn't a valid base branch",
            base.unwrap_or_default()
        ));
    }

    let new_branch = !super::git::branch_exists(&repo_root, branch);
    let base = base
        .map(str::to_string)
        .or_else(|| crate::git::resolve_default_branch(&repo_root));
    let mut args = json!({
        "repo_path": repo_root.to_string_lossy(),
        "branch": branch,
        "new_branch": new_branch,
    });
    if let Some(base) = base {
        args["base"] = json!(base);
    }
    Ok(args)
}

/// Shape a `worktree_create` tool result (from the daemon or in-process)
/// into the `worktree create` contract:
/// `{"path","branch","repo_root","created"}`.
pub fn worktree_create_output(data: &Value) -> Result<Value, String> {
    let missing = |field: &str| format!("worktree_create returned no {field}");
    let workspace = data.get("workspace").ok_or_else(|| missing("workspace"))?;
    let field = |key: &str| workspace.get(key).and_then(Value::as_str);
    let path = field("path").ok_or_else(|| missing("path"))?;
    let branch = field("branch").ok_or_else(|| missing("branch"))?;
    let repo_root = data
        .get("repo_root")
        .and_then(Value::as_str)
        .or_else(|| field("project_root"))
        .ok_or_else(|| missing("repo_root"))?;
    // Daemons older than the `created` flag always reported a new worktree.
    let created = data.get("created").and_then(Value::as_bool).unwrap_or(true);
    Ok(json!({
        "path": path,
        "branch": branch,
        "repo_root": repo_root,
        "created": created,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A bare "origin" with one commit on `main`, addressed by file:// URL.
    fn bare_origin(root: &Path) -> (PathBuf, String) {
        let seed = root.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "--initial-branch=main"]);
        git(&seed, &["config", "user.email", "t@e.com"]);
        git(&seed, &["config", "user.name", "T"]);
        std::fs::write(seed.join("README.md"), "hi").unwrap();
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-m", "init"]);
        let bare = root.join("acme").join("app.git");
        std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
        git(
            root,
            &[
                "clone",
                "--bare",
                seed.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let url = format!("file://{}", bare.display());
        (bare, url)
    }

    fn open_store(dir: &Path) -> WorkspaceStore {
        WorkspaceStore::open(
            &dir.join("codemux.db"),
            "test-host".into(),
            dir.join("workspaces"),
        )
        .unwrap()
    }

    #[test]
    fn validate_accepts_common_git_urls() {
        for url in [
            "git@github.com:acme/app.git",
            "github.com:acme/app",
            "ssh://git@github.com/acme/app.git",
            "https://github.com/acme/app",
            "HTTPS://gitlab.com/group/sub/proj.git",
            "file:///srv/git/app.git",
        ] {
            assert!(validate_remote_url(url).is_ok(), "{url} should be accepted");
        }
    }

    #[test]
    fn validate_rejects_options_helpers_and_non_urls() {
        for url in [
            "",
            "-oProxyCommand=evil",
            "--upload-pack=touch /tmp/x",
            "ext::sh -c touch% /tmp/pwned",
            "fd::17",
            "app",
            "/home/me/app",
            "./dir:with-colon",
            "ftp://example.com/app.git",
            "https://",
            "git@github.com:",
            "https://github.com/acme/app\n--bare",
        ] {
            assert!(
                validate_remote_url(url).is_err(),
                "{url:?} should be rejected"
            );
        }
    }

    #[test]
    fn name_comes_from_the_last_path_segment() {
        assert_eq!(
            project_name_from_remote("git@github.com:acme/App.git").as_deref(),
            Some("App")
        );
        assert_eq!(
            project_name_from_remote("https://gitlab.com/g/sub/proj/").as_deref(),
            Some("proj")
        );
        assert_eq!(
            project_name_from_remote("file:///srv/git/app.git").as_deref(),
            Some("app")
        );
        assert_eq!(project_name_from_remote(""), None);
    }

    #[test]
    fn https_fallback_maps_github_and_gitlab_only() {
        assert_eq!(
            https_fallback_url("git@github.com:Acme/App.git").as_deref(),
            Some("https://github.com/Acme/App.git")
        );
        assert_eq!(
            https_fallback_url("ssh://git@GitLab.com/group/sub/proj.git").as_deref(),
            Some("https://gitlab.com/group/sub/proj.git")
        );
        assert_eq!(
            https_fallback_url("https://github.com/acme/app"),
            None,
            "already https"
        );
        assert_eq!(https_fallback_url("git@bitbucket.org:acme/app.git"), None);
        assert_eq!(https_fallback_url("git@github.com:"), None);
    }

    #[test]
    fn https_retry_gets_only_the_time_left_and_none_after_a_timeout() {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(600);
        let url = "git@github.com:acme/app.git";

        let (https, left) = https_retry(url, deadline, start + Duration::from_secs(30)).unwrap();
        assert_eq!(https, "https://github.com/acme/app.git");
        assert_eq!(left, Duration::from_secs(570));

        assert_eq!(https_retry(url, deadline, deadline), None, "timed out");
        assert_eq!(
            https_retry(url, deadline, deadline + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            https_retry("git@bitbucket.org:acme/app.git", deadline, start),
            None
        );
    }

    #[test]
    fn ensure_clones_once_then_reuses_the_registered_checkout() {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let store = open_store(state.path());
        let (_bare, url) = bare_origin(root.path());

        let first = ensure_project(home.path(), Some(&store), &url, None).expect("ensure");
        let canonical = canonical_remote(&url).unwrap();
        let uid = crate::project_identity::project_uid(&canonical);
        assert!(first.cloned);
        assert_eq!(first.canonical_remote, canonical);
        assert_eq!(first.project_uid, uid);
        assert_eq!(first.default_branch.as_deref(), Some("main"));
        assert_eq!(first.branch.as_deref(), Some("main"));
        let expected = home
            .path()
            .join(".codemux/projects")
            .join(format!("app-{}", &uid.replace('-', "")[..8]));
        assert_eq!(Path::new(&first.path), expected);
        assert!(
            expected.join("README.md").exists(),
            "a full checkout, not --no-checkout"
        );

        // Registered as the project's root, with the uid the desktop uses.
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, first.path);
        assert_eq!(rows[0].kind.as_deref(), Some("main"));
        assert_eq!(rows[0].project_uid.as_deref(), Some(uid.as_str()));

        let second = ensure_project(home.path(), Some(&store), &url, None).expect("ensure again");
        assert!(!second.cloned);
        assert_eq!(second.path, first.path);
        assert_eq!(store.list().unwrap().len(), 1, "no duplicate registration");
    }

    #[test]
    fn ensure_finds_an_existing_checkout_under_projects_and_registers_it() {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let store = open_store(state.path());
        let (_bare, url) = bare_origin(root.path());

        // A checkout pushed earlier under a different folder name.
        let projects = home.path().join(".codemux/projects");
        std::fs::create_dir_all(&projects).unwrap();
        let existing = projects.join("my-app");
        git(
            root.path(),
            &["clone", "--quiet", &url, existing.to_str().unwrap()],
        );

        let ensured =
            ensure_project(home.path(), Some(&store), &url, Some("ignored")).expect("ensure");
        assert!(!ensured.cloned);
        assert_eq!(Path::new(&ensured.path), existing);
        assert_eq!(store.list().unwrap()[0].path, ensured.path);
    }

    #[test]
    fn ensure_prefers_a_registered_checkout_outside_projects() {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let store = open_store(state.path());
        let (_bare, url) = bare_origin(root.path());

        let elsewhere = root.path().join("code").join("app");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        git(
            root.path(),
            &["clone", "--quiet", &url, elsewhere.to_str().unwrap()],
        );
        store
            .create(None, elsewhere.to_string_lossy().to_string(), None, None)
            .unwrap();

        let ensured = ensure_project(home.path(), Some(&store), &url, None).expect("ensure");
        assert!(!ensured.cloned);
        assert_eq!(Path::new(&ensured.path), elsewhere);
        assert!(
            !home.path().join(".codemux/projects").exists(),
            "nothing cloned"
        );
    }

    #[test]
    fn ensure_replaces_a_stale_registry_root() {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let store = open_store(state.path());
        let (_bare, url) = bare_origin(root.path());

        // A root registered earlier whose folder has since been deleted.
        let gone = root.path().join("gone");
        git(
            root.path(),
            &["clone", "--quiet", &url, gone.to_str().unwrap()],
        );
        store
            .create(None, gone.to_string_lossy().to_string(), None, None)
            .unwrap();
        std::fs::remove_dir_all(&gone).unwrap();

        let ensured = ensure_project(home.path(), Some(&store), &url, None).expect("ensure");
        assert!(ensured.cloned);
        let rows = store.list().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "stale root dropped, not kept by the collapse"
        );
        assert_eq!(rows[0].path, ensured.path);
    }

    #[test]
    fn ensure_works_without_a_registry() {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let (_bare, url) = bare_origin(root.path());
        let ensured = ensure_project(home.path(), None, &url, Some("Renamed")).expect("ensure");
        assert!(ensured.cloned);
        assert!(
            ensured.path.contains("/.codemux/projects/Renamed-"),
            "path: {}",
            ensured.path
        );
    }

    #[test]
    fn ensure_refuses_to_clone_over_an_unrelated_folder() {
        let root = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let (_bare, url) = bare_origin(root.path());
        let uid = crate::project_identity::project_uid(&canonical_remote(&url).unwrap());
        let target = home.path().join(".codemux/projects").join(
            crate::workspace_paths::project_dir_component(Some(&uid), "app"),
        );
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("notes.txt"), "mine").unwrap();

        let err = ensure_project(home.path(), None, &url, None).unwrap_err();
        assert!(err.contains("already exists"), "got: {err}");
        assert_eq!(
            std::fs::read_to_string(target.join("notes.txt")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn clone_failure_carries_git_stderr_on_one_line() {
        let home = TempDir::new().unwrap();
        let url = format!("file://{}/missing/app.git", home.path().display());
        let err = ensure_project(home.path(), None, &url, None).unwrap_err();
        assert!(err.starts_with("couldn't clone "), "got: {err}");
        assert!(err.contains("repository"), "git's own words survive: {err}");
        assert!(!err.contains('\n'), "single line: {err:?}");
        let projects = home.path().join(".codemux/projects");
        assert_eq!(
            std::fs::read_dir(&projects).unwrap().count(),
            0,
            "temp clone cleaned up"
        );
    }

    #[test]
    fn abandoned_temp_clones_are_removed_but_live_ones_kept() {
        let dir = TempDir::new().unwrap();
        let dead = dir.path().join(".app-1234.clone-2000000000");
        let live = dir
            .path()
            .join(format!(".app-1234.clone-{}", std::process::id()));
        let other = dir.path().join(".other.clone-2000000000");
        for d in [&dead, &live, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        remove_abandoned_clones(dir.path(), ".app-1234.clone-");
        assert!(!dead.exists());
        assert!(live.exists());
        assert!(
            other.exists(),
            "another project's temp clone is not ours to remove"
        );
    }

    #[test]
    fn worktree_args_start_new_branches_from_the_default_branch() {
        let root = TempDir::new().unwrap();
        let (_bare, url) = bare_origin(root.path());
        let repo = root.path().join("app");
        git(
            root.path(),
            &["clone", "--quiet", &url, repo.to_str().unwrap()],
        );

        let args = worktree_create_args(&repo, "feature/x", None).unwrap();
        assert_eq!(args["new_branch"], true);
        assert_eq!(args["base"], "main");
        assert_eq!(args["branch"], "feature/x");

        let args = worktree_create_args(&repo, "feature/x", Some("develop")).unwrap();
        assert_eq!(args["base"], "develop", "explicit base wins");
    }

    #[test]
    fn worktree_args_attach_branches_that_exist_locally_or_on_origin() {
        let root = TempDir::new().unwrap();
        let (bare, url) = bare_origin(root.path());
        let repo = root.path().join("app");
        git(
            root.path(),
            &["clone", "--quiet", &url, repo.to_str().unwrap()],
        );

        git(&repo, &["branch", "local-only"]);
        assert_eq!(
            worktree_create_args(&repo, "local-only", None).unwrap()["new_branch"],
            false
        );

        // Pushed from another device after this clone was made.
        git(&bare, &["branch", "from-elsewhere", "main"]);
        assert_eq!(
            worktree_create_args(&repo, "from-elsewhere", None).unwrap()["new_branch"],
            false
        );
    }

    #[test]
    fn worktree_args_reject_bad_input() {
        let root = TempDir::new().unwrap();
        let (_bare, url) = bare_origin(root.path());
        let repo = root.path().join("app");
        git(
            root.path(),
            &["clone", "--quiet", &url, repo.to_str().unwrap()],
        );

        assert!(worktree_create_args(Path::new("relative/app"), "x", None).is_err());
        assert!(worktree_create_args(&root.path().join("nope"), "x", None).is_err());
        assert!(worktree_create_args(&repo, "-x", None).is_err());
        assert!(worktree_create_args(&repo, "bad..name", None).is_err());
        assert!(worktree_create_args(&repo, "  ", None).is_err());
        assert!(worktree_create_args(&repo, "ok", Some("--orphan")).is_err());
    }

    #[test]
    fn worktree_output_maps_the_tool_result() {
        let data = json!({
            "workspace": { "path": "/h/.codemux/worktrees/app/x", "branch": "x", "project_root": "/h/app" },
            "repo_root": "/h/app",
            "created": false,
        });
        assert_eq!(
            worktree_create_output(&data).unwrap(),
            json!({ "path": "/h/.codemux/worktrees/app/x", "branch": "x", "repo_root": "/h/app", "created": false })
        );

        // An older daemon: no repo_root / created fields.
        let old = json!({ "workspace": { "path": "/p", "branch": "b", "project_root": "/r" } });
        let out = worktree_create_output(&old).unwrap();
        assert_eq!(out["repo_root"], "/r");
        assert_eq!(out["created"], true);

        assert!(worktree_create_output(&json!({})).is_err());
    }
}
