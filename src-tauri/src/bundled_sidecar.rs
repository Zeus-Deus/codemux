//! The one bundled Bun executable behind Codemux's three sidecar programs.
//!
//! The Claude sidecar, the managed workflow sidecar and the plugin host are
//! compiled into a single `codemux-sidecar-<triple>[.exe]` by
//! `scripts/build-sidecars.sh`, so the installer carries one Bun runtime
//! instead of three. Every role still runs as its own process: the binary
//! picks its program from a leading `--codemux-role=<role>` argument (see
//! `sidecar/codemux-sidecar.mjs`). The plugin host runs third-party code and
//! never shares a process with a Claude session.
//!
//! The binary is a Tauri `resource`, not an `externalBin`, because
//! linuxdeploy's patchelf corrupts Bun executables (commit 025fa19).
//!
//! # Path overrides
//!
//! | Variable                                | Points at                          | Role argument |
//! |-----------------------------------------|------------------------------------|---------------|
//! | `CODEMUX_CLAUDE_SIDECAR_PATH`           | any Claude sidecar executable      | none          |
//! | `CODEMUX_MANAGED_WORKFLOW_SIDECAR_PATH` | a managed workflow-only executable | none          |
//! | `CODEMUX_PLUGIN_HOST_PATH`              | a plugin host-only executable      | none          |
//! | `CODEMUX_SIDECAR_PATH`                  | a multi-role `codemux-sidecar`     | yes           |
//!
//! A role's own variable wins and is launched exactly as before the merge,
//! with no role argument or role environment. `CODEMUX_SIDECAR_PATH`
//! replaces the bundled binary for every role and, like it, receives both
//! (Claude needs neither: it is the default role).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::agent_provider::claude::sidecar_path::target_triple;

/// File name stem of the bundled executable.
pub const BINARY_STEM: &str = "codemux-sidecar";

/// Overrides the multi-role binary for every role; see the module docs.
pub const PATH_ENV: &str = "CODEMUX_SIDECAR_PATH";

/// Role a `codemux-sidecar` process falls back to when its first argument
/// is not a role selector. Codemux sets it only in the plugin host's
/// environment ([`SidecarRole::env`]), so plugin code that re-executes
/// `process.execPath` (this binary) with ordinary arguments gets the plugin
/// host back, as it did when the plugin host was its own binary. It must be
/// in the real process environment: Bun child processes do not see
/// assignments to `process.env`. Claude spawn sites rely on the default
/// role, so Codemux's executables drop an inherited one first thing
/// ([`scrub_inherited_role`]) and user terminals never get one
/// (`terminal::strip_agent_identity_env`).
pub const ROLE_ENV: &str = "CODEMUX_SIDECAR_ROLE";

/// Drops a [`ROLE_ENV`] this process inherited, e.g. when Codemux is started
/// from a shell under the plugin host. Call it first in `main`, before any
/// thread or child starts: everything started later inherits the process
/// environment, and removing a variable races with concurrent reads of it.
pub fn scrub_inherited_role() {
    if std::env::var_os(ROLE_ENV).is_some() {
        // SAFETY: called at the top of `main`, while the process is still
        // single-threaded (the same pattern as the identity scrub).
        unsafe { std::env::remove_var(ROLE_ENV) };
    }
}

/// Which program a bundled sidecar process runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidecarRole {
    Claude,
    ManagedWorkflow,
    PluginHost,
}

impl SidecarRole {
    /// Env var that overrides this role's executable path. Honored verbatim,
    /// ahead of every other location, and launched without a role argument.
    pub const fn path_env(self) -> &'static str {
        match self {
            Self::Claude => "CODEMUX_CLAUDE_SIDECAR_PATH",
            Self::ManagedWorkflow => "CODEMUX_MANAGED_WORKFLOW_SIDECAR_PATH",
            Self::PluginHost => "CODEMUX_PLUGIN_HOST_PATH",
        }
    }

    /// Arguments that must come first on the multi-role binary's command
    /// line to select this role. Claude needs none: it is the binary's
    /// default role, which is what every Claude spawn site and SSH devices
    /// (which install the binary as `codemux-claude-sidecar` and run it
    /// without arguments) rely on.
    pub fn args(self) -> Vec<String> {
        match self {
            Self::Claude => Vec::new(),
            Self::ManagedWorkflow => vec!["--codemux-role=managed-workflow".into()],
            Self::PluginHost => vec!["--codemux-role=plugin-host".into()],
        }
    }

    /// Environment the multi-role binary is started with for this role. Only
    /// the plugin host records its role for its descendants: the other roles
    /// run agents whose shells would otherwise turn a bare `codemux-sidecar`
    /// (e.g. a Codemux dev build started from an agent's terminal) into the
    /// wrong program.
    pub fn env(self) -> Vec<(String, String)> {
        match self {
            Self::PluginHost => vec![(ROLE_ENV.into(), "plugin-host".into())],
            Self::Claude | Self::ManagedWorkflow => Vec::new(),
        }
    }
}

/// An executable, the leading arguments and the environment it must be
/// started with. Spawn sites put no arguments before `args` (a role selector
/// must come first) and apply `env` after any environment filtering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidecarCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// `.exe` on Windows, empty elsewhere.
pub fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

/// Triple-qualified name the build script stages into `src-tauri/binaries/`
/// and Tauri copies into the resource dir, e.g.
/// `codemux-sidecar-x86_64-unknown-linux-gnu[.exe]`.
pub fn binary_name() -> String {
    format!("{BINARY_STEM}-{}{}", target_triple(), exe_suffix())
}

/// `src-tauri/binaries/` in the source tree this build was compiled from.
pub fn dev_binaries_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries")
}

static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Record Tauri's resource dir at startup so spawn sites without an
/// `AppHandle` (the managed workflow adapters) find the bundled binary.
/// Later calls are ignored.
pub fn set_resource_dir(dir: PathBuf) {
    let _ = RESOURCE_DIR.set(dir);
}

fn non_empty_env(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// The role's non-empty path override, if set.
pub fn path_override(role: SidecarRole) -> Option<PathBuf> {
    non_empty_env(role.path_env()).map(PathBuf::from)
}

/// The non-empty [`PATH_ENV`] override, if set.
pub fn unified_override() -> Option<PathBuf> {
    non_empty_env(PATH_ENV).map(PathBuf::from)
}

/// Resolve how to launch `role`.
///
/// Priority:
///   1. The role's [`SidecarRole::path_env`] override, returned verbatim
///      without an existence check (harnesses pin paths they own) and with
///      no role argument or environment.
///   2. The [`PATH_ENV`] override, verbatim, with the role's arguments and
///      environment, as for the bundled binary.
///   3. `<resource_dir>/binaries/codemux-sidecar-<triple>` — the packaged
///      location Tauri copies from the `resources` glob. Without an explicit
///      `resource_dir`, the one recorded by [`set_resource_dir`].
///   4. Debug builds only: the `src-tauri/binaries/` copy, because
///      `npm run tauri:dev` does not stage it into the resource dir.
///
/// `None` when the binary exists nowhere (e.g. a checkout that never ran
/// the build script); callers report the feature as unavailable.
pub fn resolve_command(role: SidecarRole, resource_dir: Option<&Path>) -> Option<SidecarCommand> {
    let resource_dir = resource_dir.or_else(|| RESOURCE_DIR.get().map(PathBuf::as_path));
    let mut dirs = Vec::new();
    if let Some(dir) = resource_dir {
        dirs.push(dir.join("binaries"));
    }
    if cfg!(debug_assertions) {
        dirs.push(dev_binaries_dir());
    }
    resolve_command_from(role, |name| non_empty_env(name), &dirs)
}

/// [`resolve_command`] with the environment and the directories searched
/// for the bundled binary supplied by the caller.
fn resolve_command_from(
    role: SidecarRole,
    env: impl Fn(&str) -> Option<OsString>,
    dirs: &[PathBuf],
) -> Option<SidecarCommand> {
    if let Some(program) = env(role.path_env()) {
        return Some(SidecarCommand {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        });
    }
    let program = env(PATH_ENV).map(PathBuf::from).or_else(|| {
        dirs.iter()
            .map(|dir| dir.join(binary_name()))
            .find(|candidate| candidate.is_file())
    })?;
    Some(SidecarCommand {
        program,
        args: role.args(),
        env: role.env(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executables_drop_an_inherited_role_before_anything_else() {
        for (file, source, signature) in [
            ("main.rs", include_str!("main.rs"), "\nfn main() {"),
            (
                "bin/codemux_remote.rs",
                include_str!("bin/codemux_remote.rs"),
                "\n#[cfg(unix)]\nfn main() -> ExitCode {",
            ),
        ] {
            // A Windows checkout can carry CRLF line endings.
            let source = source.replace("\r\n", "\n");
            let start = source.find(signature).unwrap_or_else(|| panic!("{file}: `{}` not found", signature.trim()));
            let first = source[start + signature.len()..]
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with("//"))
                .unwrap();
            assert_eq!(first, "codemux_lib::bundled_sidecar::scrub_inherited_role();", "{file}");
        }
    }

    const ROLES: [SidecarRole; 3] = [
        SidecarRole::Claude,
        SidecarRole::ManagedWorkflow,
        SidecarRole::PluginHost,
    ];

    fn env_of(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let vars: Vec<(String, OsString)> = vars
            .iter()
            .map(|(name, value)| (name.to_string(), OsString::from(value)))
            .collect();
        move |name| {
            vars.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    fn staged_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join(binary_name());
        std::fs::write(&binary, b"bundled").unwrap();
        (dir, binary)
    }

    #[test]
    fn binary_name_is_triple_qualified() {
        let name = binary_name();
        assert!(name.starts_with("codemux-sidecar-"));
        assert!(name.contains(target_triple()));
        assert_eq!(name.ends_with(".exe"), cfg!(windows));
    }

    #[test]
    fn only_non_default_roles_pass_a_role_argument() {
        assert!(SidecarRole::Claude.args().is_empty());
        assert_eq!(
            SidecarRole::ManagedWorkflow.args(),
            ["--codemux-role=managed-workflow"]
        );
        assert_eq!(
            SidecarRole::PluginHost.args(),
            ["--codemux-role=plugin-host"]
        );
    }

    #[test]
    fn only_the_plugin_host_records_its_role_for_descendants() {
        assert!(SidecarRole::Claude.env().is_empty());
        assert!(SidecarRole::ManagedWorkflow.env().is_empty());
        assert_eq!(
            SidecarRole::PluginHost.env(),
            [(
                "CODEMUX_SIDECAR_ROLE".to_string(),
                "plugin-host".to_string()
            )]
        );
    }

    #[test]
    fn roles_keep_their_existing_override_variables() {
        assert_eq!(
            SidecarRole::Claude.path_env(),
            "CODEMUX_CLAUDE_SIDECAR_PATH"
        );
        assert_eq!(
            SidecarRole::ManagedWorkflow.path_env(),
            "CODEMUX_MANAGED_WORKFLOW_SIDECAR_PATH"
        );
        assert_eq!(
            SidecarRole::PluginHost.path_env(),
            "CODEMUX_PLUGIN_HOST_PATH"
        );
    }

    #[test]
    fn a_role_override_launches_verbatim_without_a_role_argument() {
        let (dir, _) = staged_dir();
        for role in ROLES {
            // It also beats the multi-role override and the bundled binary.
            let env = env_of(&[
                (role.path_env(), "/opt/standalone"),
                (PATH_ENV, "/opt/multi"),
            ]);
            assert_eq!(
                resolve_command_from(role, env, &[dir.path().into()]),
                Some(SidecarCommand {
                    program: "/opt/standalone".into(),
                    args: Vec::new(),
                    env: Vec::new(),
                }),
                "{role:?}"
            );
        }
    }

    #[test]
    fn the_multi_role_override_receives_the_role_argument() {
        let (dir, _) = staged_dir();
        for role in ROLES {
            let env = env_of(&[(PATH_ENV, "/opt/multi")]);
            assert_eq!(
                resolve_command_from(role, env, &[dir.path().into()]),
                Some(SidecarCommand {
                    program: "/opt/multi".into(),
                    args: role.args(),
                    env: role.env(),
                }),
                "{role:?}"
            );
        }
    }

    #[test]
    fn another_roles_override_does_not_apply() {
        let (dir, staged) = staged_dir();
        let env = env_of(&[(SidecarRole::PluginHost.path_env(), "/opt/plugin-host")]);
        assert_eq!(
            resolve_command_from(SidecarRole::ManagedWorkflow, env, &[dir.path().into()]),
            Some(SidecarCommand {
                program: staged,
                args: SidecarRole::ManagedWorkflow.args(),
                env: Vec::new(),
            })
        );
    }

    #[test]
    fn every_role_launches_the_bundled_binary_with_its_role_argument() {
        let empty = tempfile::tempdir().unwrap();
        let (dir, staged) = staged_dir();
        for role in ROLES {
            assert_eq!(
                resolve_command_from(role, env_of(&[]), &[empty.path().into(), dir.path().into()]),
                Some(SidecarCommand {
                    program: staged.clone(),
                    args: role.args(),
                    env: role.env(),
                }),
                "{role:?}"
            );
        }
    }

    #[test]
    fn nothing_resolves_without_an_override_or_a_staged_binary() {
        let empty = tempfile::tempdir().unwrap();
        for role in ROLES {
            assert_eq!(
                resolve_command_from(role, env_of(&[]), &[empty.path().into()]),
                None
            );
        }
    }
}
