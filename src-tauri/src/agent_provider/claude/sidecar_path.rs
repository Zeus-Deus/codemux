//! Resolve the path to the bundled claude-agent sidecar.
//!
//! The Claude sidecar is the default role of the shared Bun executable
//! `codemux-sidecar-<triple>[.exe]` (see [`crate::bundled_sidecar`]), staged
//! under `src-tauri/binaries/` in dev builds and bundled into the installer
//! via Tauri's `resources` mechanism in release builds. Claude is the
//! binary's default role, so Claude spawn sites pass no role argument.
//!
//! Search order:
//!   1. `CODEMUX_CLAUDE_SIDECAR_PATH` env var (tests and manual
//!      overrides).
//!   2. `CODEMUX_SIDECAR_PATH`, the multi-role binary override shared with
//!      the other roles (see [`crate::bundled_sidecar`]).
//!   3. `<manifest-dir>/binaries/codemux-sidecar-<triple>` —
//!      the dev-tree location.
//!   4. `<manifest-dir>/binaries/codemux-sidecar` — an optional
//!      triple-less fallback for environments that flatten names.
//!
//! Tauri resource-dir resolution is intentionally NOT attempted by
//! [`resolve_sidecar_path`]. That requires a live `tauri::AppHandle`,
//! which adapter code does not have at construction time. Instead the
//! main runtime calls [`resolve_sidecar`] at startup — where the
//! resource dir IS available — and pins the winner into
//! `CODEMUX_CLAUDE_SIDECAR_PATH` before the adapter is constructed.
//! [`resolve_sidecar`] also carries a debug-only dev-tree fallback so
//! `npm run tauri:dev` (whose resource dir does not contain the
//! sidecar) resolves the `src-tauri/binaries/` copy.

use std::path::{Path, PathBuf};

use crate::agent_provider::{ProviderError, ProviderKind};
use crate::bundled_sidecar::{self, SidecarRole};

/// Env var honored as the highest-priority override.
pub const SIDECAR_PATH_ENV: &str = SidecarRole::Claude.path_env();

/// Best-effort Rust target triple for the running binary.
///
/// Uses `cfg!` at compile time so the returned string always matches
/// what this binary was actually compiled for. Tauri stages external
/// bins by triple, so this is the identifier we need to match.
pub fn target_triple() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else {
        "unknown"
    }
}

/// Triple-qualified file name of the staged sidecar binary, e.g.
/// `codemux-sidecar-x86_64-unknown-linux-gnu[.exe]`, shared with the other
/// bundled roles. Tests that stage a fake sidecar use it.
fn sidecar_binary_name() -> String {
    bundled_sidecar::binary_name()
}

/// Candidate locations to probe, in priority order.
fn candidate_paths() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(explicit) = bundled_sidecar::path_override(SidecarRole::Claude) {
        candidates.push(explicit);
    }
    if let Some(multi_role) = bundled_sidecar::unified_override() {
        candidates.push(multi_role);
    }

    let binaries = bundled_sidecar::dev_binaries_dir();
    candidates.push(binaries.join(sidecar_binary_name()));
    candidates.push(binaries.join(format!(
        "{}{}",
        bundled_sidecar::BINARY_STEM,
        bundled_sidecar::exe_suffix()
    )));

    candidates
}

/// Return the first candidate path that exists on disk. Does not
/// check executability — that is enforced by the OS when we try to
/// spawn the subprocess.
pub fn resolve_sidecar_path() -> Result<PathBuf, ProviderError> {
    let candidates = candidate_paths();
    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }
    let listed = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(ProviderError::NotInstalled {
        provider: ProviderKind::Claude,
        hint: format!(
            "claude-agent sidecar binary not found. Set {SIDECAR_PATH_ENV} or run \
             `bash scripts/build-sidecars.sh` to stage it. Searched: {listed}"
        ),
    })
}

/// Resolve the sidecar path for the main runtime at startup, where a
/// Tauri `resource_dir` IS available (unlike the adapter-side
/// [`resolve_sidecar_path`]). Returns the path that should be pinned
/// into [`SIDECAR_PATH_ENV`] so downstream adapter code — which has no
/// `AppHandle` — finds the same binary. See
/// [`bundled_sidecar::resolve_command`] for the priority order: an
/// already-set override (returned verbatim), then the multi-role
/// `CODEMUX_SIDECAR_PATH` override, then the resource dir, then (debug
/// builds only) the dev-tree copy that `npm run tauri:dev` does not stage
/// into the resource dir.
///
/// Returns `None` when the binary exists nowhere (e.g. a fresh
/// checkout that never ran the build script); callers should leave the
/// provider slot empty rather than panic — resolution then falls to
/// `provider_not_configured` semantics.
pub fn resolve_sidecar(resource_dir: Option<&Path>) -> Option<PathBuf> {
    // Claude is the default role, so its command never carries arguments
    // and the path alone is enough to launch it.
    bundled_sidecar::resolve_command(SidecarRole::Claude, resource_dir)
        .map(|command| command.program)
}

/// Assert the sidecar binary exists — used by the provider's health
/// check before a session is started.
pub fn probe_sidecar_binary_exists() -> Result<PathBuf, ProviderError> {
    resolve_sidecar_path()
}

/// Check executability of a specific path. Returned as a bool so
/// callers can decide what to do — e.g. use it anyway on non-Unix
/// platforms where the mode bits are meaningless.
#[cfg(unix)]
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes every test that reads or writes `SIDECAR_PATH_ENV`.
    /// `env::set_var`/`remove_var` mutate process-global state that
    /// leaks across the parallel test threads, so these tests must not
    /// run concurrently or they clobber each other's view of the
    /// override.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A fresh, empty temp directory unique to this call. Removed first
    /// in case a prior run left it behind.
    fn unique_tmp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("codemux-sidecar-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn target_triple_matches_cfg() {
        let t = target_triple();
        assert!(!t.is_empty());
        // Not a hard assertion on specific value — just a sanity
        // check that the mapping returned something reasonable for
        // this build.
        assert!(
            t.contains("linux")
                || t.contains("darwin")
                || t.contains("windows")
                || t == "unknown"
        );
    }

    #[test]
    fn candidate_paths_contain_manifest_dev_location() {
        let cps = candidate_paths();
        assert!(cps
            .iter()
            .any(|p| p.to_string_lossy().contains("binaries")
                && p.to_string_lossy().contains("codemux-sidecar")));
    }

    #[test]
    fn env_var_override_is_first_candidate() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `env::set_var` leaks into sibling threads running in
        // parallel; gate on a marker to avoid clobbering the real
        // env in tests that actually spawn the sidecar.
        let marker = "/tmp/nonexistent-sidecar-override-marker-abcdef";
        std::env::set_var(SIDECAR_PATH_ENV, marker);
        let cps = candidate_paths();
        assert_eq!(cps.first().map(|p| p.to_string_lossy().to_string()),
                   Some(marker.to_string()));
        std::env::remove_var(SIDECAR_PATH_ENV);
    }

    #[test]
    fn resolve_sidecar_honors_env_override_verbatim() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The override wins even when a valid resource-dir copy is
        // present, and is returned verbatim without an existence check
        // — the e2e suite pins a path the harness owns.
        let marker = "/tmp/nonexistent-sidecar-override-marker-resolve";
        std::env::set_var(SIDECAR_PATH_ENV, marker);

        let tmp = unique_tmp_dir();
        let binaries = tmp.join("binaries");
        std::fs::create_dir_all(&binaries).unwrap();
        std::fs::write(binaries.join(sidecar_binary_name()), b"real").unwrap();

        let resolved = resolve_sidecar(Some(&tmp));

        std::env::remove_var(SIDECAR_PATH_ENV);
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(resolved, Some(PathBuf::from(marker)));
    }

    #[test]
    fn resolve_sidecar_prefers_resource_dir_when_present() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(SIDECAR_PATH_ENV);

        let tmp = unique_tmp_dir();
        let binaries = tmp.join("binaries");
        std::fs::create_dir_all(&binaries).unwrap();
        let staged = binaries.join(sidecar_binary_name());
        std::fs::write(&staged, b"real").unwrap();

        let resolved = resolve_sidecar(Some(&tmp));

        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(resolved, Some(staged));
    }

    #[test]
    fn resolve_sidecar_falls_back_when_resource_dir_missing_binary() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(SIDECAR_PATH_ENV);

        // An empty resource dir forces step 2 to miss. In debug builds
        // the dev-tree fallback then resolves the staged
        // `src-tauri/binaries/` copy when it exists on this machine; in
        // release builds (fallback compiled out) or a checkout that
        // never ran the build script the result is `None`. Assert the
        // exact branch this build+machine actually takes so the test is
        // deterministic everywhere.
        let empty_resource = unique_tmp_dir();
        std::fs::create_dir_all(&empty_resource).unwrap();
        let manifest_copy = bundled_sidecar::dev_binaries_dir().join(sidecar_binary_name());

        let resolved = resolve_sidecar(Some(&empty_resource));

        let _ = std::fs::remove_dir_all(&empty_resource);
        if cfg!(debug_assertions) && manifest_copy.exists() {
            assert_eq!(resolved, Some(manifest_copy));
        } else {
            assert_eq!(resolved, None);
        }
    }
}
