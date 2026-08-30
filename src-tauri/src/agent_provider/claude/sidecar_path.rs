//! Resolve the path to the bundled claude-agent sidecar binary.
//!
//! The sidecar is a Bun-compiled TypeScript binary staged under
//! `src-tauri/binaries/codemux-claude-sidecar-<triple>[.exe]` in dev
//! builds and bundled into the installer via Tauri's `resources`
//! mechanism in release builds. (Originally shipped via `externalBin`,
//! moved to `resources` because linuxdeploy's patchelf step corrupts
//! the ~100 MB bun-compiled binary — see commit 025fa19.) This module
//! finds whichever copy is reachable from the current process.
//!
//! Search order:
//!   1. `CODEMUX_CLAUDE_SIDECAR_PATH` env var (tests and manual
//!      overrides).
//!   2. `<manifest-dir>/binaries/codemux-claude-sidecar-<triple>` —
//!      the dev-tree location.
//!   3. `<manifest-dir>/binaries/codemux-claude-sidecar` — an optional
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

/// Env var honored as the highest-priority override.
pub const SIDECAR_PATH_ENV: &str = "CODEMUX_CLAUDE_SIDECAR_PATH";

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

/// `.exe` suffix on Windows, empty elsewhere. Kept private since
/// callers only need the full resolved path.
fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

/// Triple-qualified file name of the staged sidecar binary, e.g.
/// `codemux-claude-sidecar-x86_64-unknown-linux-gnu[.exe]`. This is
/// the name `scripts/build-claude-sidecar.sh` writes into
/// `src-tauri/binaries/` and the name Tauri copies into the resource
/// dir for release builds.
fn sidecar_binary_name() -> String {
    format!("codemux-claude-sidecar-{}{}", target_triple(), exe_suffix())
}

/// The `CODEMUX_CLAUDE_SIDECAR_PATH` override, if set to a non-empty
/// value. Highest-priority source everywhere the sidecar is resolved.
fn env_override() -> Option<PathBuf> {
    match std::env::var(SIDECAR_PATH_ENV) {
        Ok(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => None,
    }
}

/// Candidate locations to probe, in priority order.
fn candidate_paths() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(explicit) = env_override() {
        candidates.push(explicit);
    }

    let binaries = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries");
    candidates.push(binaries.join(sidecar_binary_name()));
    candidates.push(binaries.join(format!("codemux-claude-sidecar{}", exe_suffix())));

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
             `bash scripts/build-claude-sidecar.sh` to stage it. Searched: {listed}"
        ),
    })
}

/// Resolve the sidecar path for the main runtime at startup, where a
/// Tauri `resource_dir` IS available (unlike the adapter-side
/// [`resolve_sidecar_path`]). Returns the path that should be pinned
/// into [`SIDECAR_PATH_ENV`] so downstream adapter code — which has no
/// `AppHandle` — finds the same binary.
///
/// Priority:
///   1. An already-set [`SIDECAR_PATH_ENV`] override — returned
///      verbatim and never overridden, because the e2e suite and unit
///      tests pin it explicitly.
///   2. `<resource_dir>/binaries/codemux-claude-sidecar-<triple>` —
///      the packaged/release location Tauri copies from the
///      `resources` glob in `tauri.conf.json`.
///   3. Debug builds only: `<CARGO_MANIFEST_DIR>/binaries/…` — the
///      dev-tree copy staged by `scripts/build-claude-sidecar.sh`.
///      `resource_dir` does NOT contain it under `npm run tauri:dev`,
///      so without this fallback the chat GUI fails to spawn the
///      sidecar in run-from-source builds. Compiled out of release
///      builds via `debug_assertions`, so release resolution stays
///      resource-dir only.
///
/// Returns `None` when the binary exists nowhere (e.g. a fresh
/// checkout that never ran the build script); callers should leave the
/// provider slot empty rather than panic — resolution then falls to
/// `provider_not_configured` semantics.
pub fn resolve_sidecar(resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(explicit) = env_override() {
        #[cfg(debug_assertions)]
        warn_if_override_shadows_dev_copy(&explicit);
        return Some(explicit);
    }

    if let Some(dir) = resource_dir {
        let candidate = dir.join("binaries").join(sidecar_binary_name());
        if candidate.exists() {
            return Some(candidate);
        }
    }

    #[cfg(debug_assertions)]
    {
        let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(sidecar_binary_name());
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

/// Debug-build tripwire for an inherited override. Terminals spawned by
/// Codemux strip [`SIDECAR_PATH_ENV`] (see
/// `terminal::app_process_only_env_vars`), but a value can still arrive
/// through other routes (an old daemon, a shell rc file, a stale export).
/// If it points outside this checkout while a staged dev-tree copy exists,
/// the dev build is almost certainly about to run someone else's sidecar —
/// say so loudly, naming both paths. The override still wins; this only
/// makes the mismatch visible instead of surfacing later as a baffling
/// "unknown method" from a binary built from different sources.
#[cfg(debug_assertions)]
fn warn_if_override_shadows_dev_copy(explicit: &Path) {
    let dev_copy = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(sidecar_binary_name());
    if !dev_copy.exists() {
        return;
    }
    let inside_dev_tree = explicit.starts_with(env!("CARGO_MANIFEST_DIR"));
    if inside_dev_tree {
        return;
    }
    eprintln!(
        "[codemux::sidecar] WARNING: {SIDECAR_PATH_ENV} overrides the sidecar with \
         {} but this dev tree has its own staged copy at {}. The override wins, so \
         this build will run a sidecar compiled from different sources. Unset the \
         variable unless that is intentional.",
        explicit.display(),
        dev_copy.display()
    );
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
                && p.to_string_lossy().contains("codemux-claude-sidecar")));
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
        let manifest_copy = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(sidecar_binary_name());

        let resolved = resolve_sidecar(Some(&empty_resource));

        let _ = std::fs::remove_dir_all(&empty_resource);
        if cfg!(debug_assertions) && manifest_copy.exists() {
            assert_eq!(resolved, Some(manifest_copy));
        } else {
            assert_eq!(resolved, None);
        }
    }
}
