//! Create a workspace whose thread runs on a device.
//!
//! The device gets its own checkout of the project from the project's git
//! remote (`codemux-remote project ensure`, cloned on first use) and, for a
//! new branch, a worktree in that checkout (`codemux-remote worktree
//! create`). Nothing is copied from this computer. Locally Codemux keeps an
//! attach-in-place workspace pointing at the device path: terminals reach it
//! through the SSH-tunnelled pty-daemon and the agent-chat provider is
//! spawned there over `ssh -T`.

use tauri::Manager;

use crate::commands::workspace::WorkspaceCreated;
use crate::state::InitialChatPane;

/// `project_path` is the local project root that identifies the repo;
/// `None` starts in the device's home directory. A worktree is created only
/// when `new_branch` is set and `branch` is given; otherwise the thread works
/// in the device's checkout.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_workspace_on_host<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    host_id: i64,
    project_path: Option<String>,
    branch: Option<String>,
    new_branch: bool,
    base_branch: Option<String>,
    initial_chat: Option<InitialChatPane>,
    select: Option<bool>,
) -> Result<WorkspaceCreated, String> {
    if let Some(chat) = &initial_chat {
        crate::commands::agent_chat::feature_flag_on(
            &app.state::<crate::observability::ObservabilityStore>(),
        )?;
        if chat.thread_id.trim().is_empty() {
            return Err("thread id is required".into());
        }
    }

    let project_path = non_empty(project_path);
    let worktree_branch = if new_branch { non_empty(branch) } else { None };
    let base_branch = non_empty(base_branch);
    let select = select.unwrap_or(true);

    #[cfg(unix)]
    {
        on_device::create(
            &app,
            host_id,
            project_path,
            worktree_branch,
            base_branch,
            initial_chat,
            select,
        )
        .await
    }
    #[cfg(not(unix))]
    {
        let _ = (
            host_id,
            project_path,
            worktree_branch,
            base_branch,
            initial_chat,
            select,
        );
        Err("Running threads on a device isn't supported on Windows yet.".into())
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(unix)]
mod on_device {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use serde::Deserialize;
    use tauri::{AppHandle, Manager, Runtime};

    use crate::commands::workspace::WorkspaceCreated;
    use crate::database::{DatabaseStore, HostRecord};
    use crate::hosts_upgrade::{is_older, UpgradeOutcome};
    use crate::ssh::exec::{
        codemux_remote_command, remote_home, run_remote_json, sh_quote, validate_ssh_target,
    };
    use crate::ssh::probe::{probe_host, ProbeOptions, ProbeOutcome};
    use crate::state::{AppStateStore, InitialChatPane, RemoteAttachWorkspace};

    /// A first clone of a large repo over a slow link takes minutes. Longer
    /// than the device's own caps (the clone, shared with its https retry,
    /// then the origin/HEAD fix-up) plus SSH and startup slack, so its error
    /// reaches the user instead of a bare timeout.
    const ENSURE_PROJECT_BUDGET: Duration = Duration::from_secs(
        crate::remote::git::CLONE_TIMEOUT.as_secs()
            + crate::remote::git::ORIGIN_HEAD_TIMEOUT.as_secs()
            + 40,
    );
    /// Fetches the base branch, then `git worktree add`. Longer than the
    /// device's own 3-minute cap on that work, so its error reaches the user
    /// instead of a bare timeout.
    const CREATE_WORKTREE_BUDGET: Duration = Duration::from_secs(4 * 60);

    /// Last stdout line of `codemux-remote project ensure`.
    #[derive(Debug, Deserialize)]
    struct EnsuredProject {
        path: String,
        #[serde(default)]
        cloned: bool,
        /// `None` on a detached HEAD.
        #[serde(default)]
        branch: Option<String>,
        #[serde(default)]
        project_uid: Option<String>,
    }

    /// Last stdout line of `codemux-remote worktree create`.
    #[derive(Debug, Deserialize)]
    struct CreatedWorktree {
        path: String,
        branch: String,
    }

    pub(super) async fn create<R: Runtime>(
        app: &AppHandle<R>,
        host_id: i64,
        project_path: Option<String>,
        worktree_branch: Option<String>,
        base_branch: Option<String>,
        initial_chat: Option<InitialChatPane>,
        select: bool,
    ) -> Result<WorkspaceCreated, String> {
        let host = app
            .state::<DatabaseStore>()
            .list_hosts()
            .into_iter()
            .find(|h| h.id == host_id)
            .ok_or_else(|| {
                "That device isn't set up on this computer. Add it in Settings → Devices."
                    .to_string()
            })?;
        let device = host.name.as_str();
        validate_ssh_target(&host.ssh_target).map_err(|e| {
            format!("{device}'s SSH address isn't valid ({e}). Fix it in Settings → Devices.")
        })?;

        ensure_codemux_remote(app, &host).await?;

        // A Claude thread needs the ~100 MB Claude runtime on the device.
        // Install it while the device gets the project ready, so the upload
        // happens under "Setting up on <device>…" instead of stalling the
        // first message. Session start re-checks it (cached).
        let wants_claude = initial_chat
            .as_ref()
            .is_some_and(|chat| chat.provider == crate::agent_provider::ProviderKind::Claude);
        let claude_runtime = async {
            if wants_claude {
                crate::ssh::sidecar::ensure_claude_sidecar(&host.ssh_target)
                    .await
                    .map_err(|e| format!("Couldn't set up Claude on {device}: {e}"))
            } else {
                Ok(())
            }
        };

        let place = async {
            match project_path {
                Some(project_path) => {
                    place_in_project(&host, &project_path, worktree_branch, base_branch).await
                }
                None => place_in_home(&host).await,
            }
        };
        let spec = place_and_install(place, claude_runtime).await?;

        let cwd = spec.remote_cwd.clone();
        let workspace_id = app
            .state::<AppStateStore>()
            .create_remote_workspace_with_selection(spec, initial_chat, select);
        crate::state::emit_app_state(app);
        Ok(WorkspaceCreated {
            workspace_id: workspace_id.0,
            cwd,
            adopted: false,
        })
    }

    /// Run the placement and the runtime install side by side. The first
    /// failure is reported at once instead of after a long clone or upload
    /// it makes pointless; the other step is dropped, which kills its ssh,
    /// and a clone cut short leaves only a temp folder the next ensure
    /// removes.
    async fn place_and_install<T>(
        place: impl std::future::Future<Output = Result<T, String>>,
        install: impl std::future::Future<Output = Result<(), String>>,
    ) -> Result<T, String> {
        let (spec, ()) = tokio::try_join!(place, install)?;
        Ok(spec)
    }

    /// A thread with no project starts in the device's home directory.
    async fn place_in_home(host: &HostRecord) -> Result<RemoteAttachWorkspace, String> {
        let device = host.name.as_str();
        let home = remote_home(&host.ssh_target)
            .await
            .map_err(|e| format!("Couldn't reach {device}: {e}"))?;
        Ok(RemoteAttachWorkspace {
            title: "Home".into(),
            host_id: host.id,
            remote_cwd: home.clone(),
            is_git: false,
            git_branch: None,
            // The local home groups the thread with this computer's Home
            // threads in the sidebar.
            project_root: dirs::home_dir().map(|p| p.display().to_string()),
            // Not a checkout: on the device the root is the home directory.
            remote_root: Some(home),
            project_uid: None,
            workspace_kind: None,
        })
    }

    /// Make sure the device's `codemux-remote` is this build's or newer;
    /// the project and worktree steps need its CLI. An older one gets the
    /// same gentle upgrade as the background one (the device's terminals
    /// keep running, and an upgrade already under way is waited for); a
    /// missing or broken one is installed.
    async fn ensure_codemux_remote<R: Runtime>(
        app: &AppHandle<R>,
        host: &HostRecord,
    ) -> Result<(), String> {
        let device = host.name.as_str();
        let set_up_failed = |e: String| {
            format!(
                "Couldn't set up codemux on {device}: {e}. \
                 Finish setting up {device} in Settings → Devices."
            )
        };
        match probe_host(ProbeOptions::new(&host.ssh_target)).await {
            ProbeOutcome::Unreachable { reason } => {
                Err(format!("Couldn't reach {device}: {reason}"))
            }
            ProbeOutcome::Reachable {
                codemux_remote_version: Some(version),
                ..
            } => {
                if !is_older(&version, env!("CARGO_PKG_VERSION")) {
                    return Ok(());
                }
                match crate::hosts_upgrade::upgrade_host(app, host).await {
                    Ok(UpgradeOutcome::AlreadyCurrent | UpgradeOutcome::Upgraded { .. }) => Ok(()),
                    Ok(UpgradeOutcome::NotAttempted { reason }) => Err(set_up_failed(reason)),
                    Err(e) => Err(set_up_failed(e)),
                }
            }
            ProbeOutcome::Reachable { .. } => {
                crate::commands::hosts::ensure_remote_binary_current(app, host)
                    .await
                    .map_err(set_up_failed)
            }
        }
    }

    /// Make sure the device has the project, plus a worktree when one was
    /// asked for, and describe the workspace that runs there.
    async fn place_in_project(
        host: &HostRecord,
        project_path: &str,
        worktree_branch: Option<String>,
        base_branch: Option<String>,
    ) -> Result<RemoteAttachWorkspace, String> {
        let device = host.name.as_str();
        let name = project_name(project_path);

        let repo = PathBuf::from(project_path);
        let remotes = tokio::task::spawn_blocking(move || {
            crate::git_provider::detect::run_git_allow_empty(&repo, &["remote", "-v"])
        })
        .await
        .map_err(|e| format!("Couldn't read {name}'s git remote: {e}"))?
        .map_err(|_| format!("{name} isn't a git repository, so {device} can't get a copy."))?;
        let remote_url = remote_url_for_device(&remotes, &name, device)?;

        let project = run_remote_json::<EnsuredProject>(
            &host.ssh_target,
            &project_ensure_script(&remote_url, &name),
            ENSURE_PROJECT_BUDGET,
        )
        .await
        .and_then(|p| absolute(&p.path).map(|()| p))
        .map_err(|e| format!("Couldn't get a copy of {name} on {device}: {e}"))?;
        if project.cloned {
            eprintln!(
                "[remote_workspace] cloned {name} onto {device} at {}",
                project.path
            );
        }

        let remote_root = project.path.clone();
        let (remote_cwd, git_branch, workspace_kind, title) = match worktree_branch {
            Some(branch) => {
                let worktree = run_remote_json::<CreatedWorktree>(
                    &host.ssh_target,
                    &worktree_create_script(&project.path, &branch, base_branch.as_deref()),
                    CREATE_WORKTREE_BUDGET,
                )
                .await
                .and_then(|w| absolute(&w.path).map(|()| w))
                .map_err(|e| format!("Couldn't create the {branch} worktree on {device}: {e}"))?;
                let title = worktree.branch.clone();
                (worktree.path, Some(worktree.branch), "worktree", title)
            }
            None => (project.path, project.branch, "main", name),
        };

        Ok(RemoteAttachWorkspace {
            title,
            host_id: host.id,
            remote_cwd,
            is_git: true,
            git_branch,
            // The local root, not the device path: the sidebar groups by it,
            // so the thread sits under the same project as local ones.
            project_root: Some(project_path.to_string()),
            // The device's checkout, which env and agent context name as
            // the project root there.
            remote_root: Some(remote_root),
            project_uid: project.project_uid,
            workspace_kind: Some(workspace_kind.into()),
        })
    }

    fn project_name(project_path: &str) -> String {
        Path::new(project_path.trim_end_matches('/'))
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .unwrap_or("project")
            .to_string()
    }

    fn absolute(path: &str) -> Result<(), String> {
        if path.starts_with('/') {
            Ok(())
        } else {
            Err(format!("the device returned an unexpected path: {path:?}"))
        }
    }

    /// Pick the URL the device clones from `git remote -v` output: `origin`,
    /// else the first remote.
    fn remote_url_for_device(remotes: &str, project: &str, device: &str) -> Result<String, String> {
        let remotes = crate::git_provider::detect::parse_remote_lines(remotes);
        let url = remotes
            .iter()
            .find(|r| r.name == "origin")
            .or_else(|| remotes.first())
            .map(|r| r.url.clone())
            .ok_or_else(|| {
                format!(
                    "{project} has no git remote, so {device} can't get a copy. \
                     Push it to a git remote first."
                )
            })?;
        // A remote that is a folder on this computer can't be cloned from
        // anywhere else.
        if url.starts_with('/')
            || url.starts_with('.')
            || url.starts_with('~')
            || url.starts_with("file:")
        {
            return Err(format!(
                "{project}'s git remote is a folder on this computer, so {device} can't \
                 get a copy. Push it to a hosted git remote first."
            ));
        }
        Ok(url)
    }

    // Values go in `--flag=value` form so one starting with `-` can't be
    // read as a flag.
    fn project_ensure_script(remote_url: &str, name: &str) -> String {
        codemux_remote_command(&format!(
            "project ensure {} {}",
            sh_quote(&format!("--remote-url={remote_url}")),
            sh_quote(&format!("--name={name}")),
        ))
    }

    fn worktree_create_script(repo: &str, branch: &str, base: Option<&str>) -> String {
        let mut args = format!(
            "worktree create {} {}",
            sh_quote(&format!("--repo={repo}")),
            sh_quote(&format!("--branch={branch}")),
        );
        if let Some(base) = base {
            args.push(' ');
            args.push_str(&sh_quote(&format!("--base={base}")));
        }
        codemux_remote_command(&args)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::ssh::exec::parse_last_json_line;

        #[test]
        fn ensure_script_quotes_every_value() {
            let script = project_ensure_script("git@github.com:acme/app.git", "it's; rm -rf ~");
            assert!(script.ends_with(
                "\"$CMR\" project ensure '--remote-url=git@github.com:acme/app.git' \
                 '--name=it'\\''s; rm -rf ~'"
            ));
        }

        #[test]
        fn worktree_script_adds_base_only_when_given() {
            let with_base =
                worktree_create_script("/home/deus/.codemux/projects/app", "feat/x", Some("main"));
            assert!(with_base.ends_with(
                "\"$CMR\" worktree create '--repo=/home/deus/.codemux/projects/app' \
                 '--branch=feat/x' '--base=main'"
            ));
            let without = worktree_create_script("/srv/app", "feat/x", None);
            assert!(without.ends_with("'--branch=feat/x'"));
            assert!(!without.contains("--base"));
        }

        #[test]
        fn prefers_origin_then_first_remote() {
            let both = "upstream\thttps://github.com/up/app (fetch)\n\
                        upstream\thttps://github.com/up/app (push)\n\
                        origin\tgit@github.com:me/app.git (fetch)\n\
                        origin\tgit@github.com:me/app.git (push)\n";
            assert_eq!(
                remote_url_for_device(both, "app", "zeus").unwrap(),
                "git@github.com:me/app.git"
            );
            let only = "fork\thttps://gitlab.com/me/app.git (fetch)\n";
            assert_eq!(
                remote_url_for_device(only, "app", "zeus").unwrap(),
                "https://gitlab.com/me/app.git"
            );
        }

        #[test]
        fn explains_a_missing_or_local_remote() {
            let none = remote_url_for_device("", "app", "zeus").unwrap_err();
            assert_eq!(
                none,
                "app has no git remote, so zeus can't get a copy. Push it to a git remote first."
            );
            let local = remote_url_for_device("origin\t/srv/git/app.git (fetch)\n", "app", "zeus")
                .unwrap_err();
            assert!(local.contains("folder on this computer"), "{local}");
        }

        #[test]
        fn parses_device_responses_after_a_login_banner() {
            let project: EnsuredProject = parse_last_json_line(
                "Welcome to deus\n{\"path\":\"/home/deus/.codemux/projects/app-1a2b3c4d\",\
                 \"cloned\":true,\"default_branch\":\"main\",\"branch\":null,\
                 \"project_uid\":\"uid\",\"canonical_remote\":\"github.com/me/app\"}\n",
            )
            .unwrap();
            assert!(project.cloned);
            assert_eq!(project.branch, None);
            assert_eq!(project.project_uid.as_deref(), Some("uid"));

            let worktree: CreatedWorktree = parse_last_json_line(
                "{\"path\":\"/home/deus/.codemux/worktrees/app/feat-x\",\"branch\":\"feat/x\",\
                 \"repo_root\":\"/home/deus/.codemux/projects/app\",\"created\":true}",
            )
            .unwrap();
            assert_eq!(worktree.branch, "feat/x");
            assert!(absolute(&worktree.path).is_ok());
            assert!(absolute("~/x").is_err());
        }

        #[test]
        fn names_the_project_after_its_folder() {
            assert_eq!(project_name("/home/zeus/projects/app/"), "app");
            assert_eq!(project_name("/"), "project");
        }

        #[tokio::test]
        async fn the_first_failure_returns_without_waiting_for_the_other_step() {
            let never = std::future::pending::<Result<(), String>>();
            let placed = tokio::time::timeout(
                Duration::from_secs(5),
                place_and_install(async { Err::<(), _>("no remote".to_string()) }, never),
            )
            .await
            .expect("a fast placement failure must not wait for the install");
            assert_eq!(placed.unwrap_err(), "no remote");

            let never = std::future::pending::<Result<(), String>>();
            let installed = tokio::time::timeout(
                Duration::from_secs(5),
                place_and_install(never, async { Err("no runtime".to_string()) }),
            )
            .await
            .expect("a fast install failure must not wait for the placement");
            assert_eq!(installed.unwrap_err(), "no runtime");
        }

        #[test]
        fn ensure_budget_outlasts_the_device_clone_cap() {
            let device_caps =
                crate::remote::git::CLONE_TIMEOUT + crate::remote::git::ORIGIN_HEAD_TIMEOUT;
            assert!(ENSURE_PROJECT_BUDGET >= device_caps + Duration::from_secs(30));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::non_empty;

    #[test]
    fn blank_values_count_as_missing() {
        assert_eq!(non_empty(Some("  ".into())), None);
        assert_eq!(non_empty(Some(" main ".into())).as_deref(), Some("main"));
        assert_eq!(non_empty(None), None);
    }
}
