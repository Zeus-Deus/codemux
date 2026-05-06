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
//! Tauri resource-dir resolution is intentionally NOT attempted here.
//! That requires a live `tauri::AppHandle`, which adapter code does
//! not have at construction time. When the main runtime loads the
//! provider from a Tauri command path, it should set
//! `CODEMUX_CLAUDE_SIDECAR_PATH` before calling us.

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

/// Candidate locations to probe, in priority order.
fn candidate_paths() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(explicit) = std::env::var(SIDECAR_PATH_ENV) {
        if !explicit.is_empty() {
            candidates.push(PathBuf::from(explicit));
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let binaries = manifest_dir.join("binaries");
    let triple = target_triple();
    let ext = exe_suffix();

    candidates.push(binaries.join(format!("codemux-claude-sidecar-{triple}{ext}")));
    candidates.push(binaries.join(format!("codemux-claude-sidecar{ext}")));

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
}
