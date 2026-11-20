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
//! Each role keeps its own path override env var, so tests and e2e harnesses
//! can still point one role at a different executable.

use std::path::{Path, PathBuf};

use crate::agent_provider::claude::sidecar_path::target_triple;

/// File name stem of the bundled executable.
pub const BINARY_STEM: &str = "codemux-sidecar";

/// Which program a bundled sidecar process runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidecarRole {
    Claude,
    ManagedWorkflow,
    PluginHost,
}

impl SidecarRole {
    /// Env var that overrides this role's executable path. Honored verbatim
    /// and ahead of every bundled location.
    pub const fn path_env(self) -> &'static str {
        match self {
            Self::Claude => "CODEMUX_CLAUDE_SIDECAR_PATH",
            Self::ManagedWorkflow => "CODEMUX_MANAGED_WORKFLOW_SIDECAR_PATH",
            Self::PluginHost => "CODEMUX_PLUGIN_HOST_PATH",
        }
    }

    /// Arguments that must come first on the command line to select this
    /// role. Claude needs none: it is the binary's default role, which is
    /// what every Claude spawn site and SSH devices (which install the binary
    /// as `codemux-claude-sidecar` and run it without arguments) rely on.
    /// The other roles' programs ignore unknown arguments, so the role
    /// argument is also harmless when an override points at a standalone
    /// build of that program.
    pub fn args(self) -> Vec<String> {
        match self {
            Self::Claude => Vec::new(),
            Self::ManagedWorkflow => vec!["--codemux-role=managed-workflow".into()],
            Self::PluginHost => vec!["--codemux-role=plugin-host".into()],
        }
    }
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

/// The role's non-empty path override, if set.
pub fn path_override(role: SidecarRole) -> Option<PathBuf> {
    std::env::var_os(role.path_env())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Resolve the executable to launch for `role`.
///
/// Priority:
///   1. The role's [`SidecarRole::path_env`] override, returned verbatim
///      without an existence check (harnesses pin paths they own).
///   2. `<resource_dir>/binaries/codemux-sidecar-<triple>` — the packaged
///      location Tauri copies from the `resources` glob.
///   3. Debug builds only: the `src-tauri/binaries/` copy, because
///      `npm run tauri:dev` does not stage it into the resource dir.
///
/// `None` when the binary exists nowhere (e.g. a checkout that never ran
/// the build script); callers report the feature as unavailable.
pub fn resolve(role: SidecarRole, resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(explicit) = path_override(role) {
        return Some(explicit);
    }
    if let Some(dir) = resource_dir {
        let candidate = dir.join("binaries").join(binary_name());
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    #[cfg(debug_assertions)]
    {
        let candidate = dev_binaries_dir().join(binary_name());
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(SidecarRole::PluginHost.args(), ["--codemux-role=plugin-host"]);
    }

    #[test]
    fn roles_keep_their_existing_override_variables() {
        assert_eq!(SidecarRole::Claude.path_env(), "CODEMUX_CLAUDE_SIDECAR_PATH");
        assert_eq!(
            SidecarRole::ManagedWorkflow.path_env(),
            "CODEMUX_MANAGED_WORKFLOW_SIDECAR_PATH"
        );
        assert_eq!(SidecarRole::PluginHost.path_env(), "CODEMUX_PLUGIN_HOST_PATH");
    }

    #[test]
    fn every_role_resolves_the_same_bundled_binary() {
        // The managed and plugin overrides are only set by tests and
        // harnesses; this test does not touch any override.
        if path_override(SidecarRole::ManagedWorkflow).is_some()
            || path_override(SidecarRole::PluginHost).is_some()
        {
            return;
        }
        let resources = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(resources.path().join("binaries")).unwrap();
        let staged = resources.path().join("binaries").join(binary_name());
        std::fs::write(&staged, b"bundled").unwrap();
        for role in [SidecarRole::ManagedWorkflow, SidecarRole::PluginHost] {
            assert_eq!(resolve(role, Some(resources.path())), Some(staged.clone()));
        }
    }
}
