//! OpenCode CLI / server discovery.
//!
//! Discovery resolves an explicit prepared-binary override or the first
//! `opencode` on PATH, passively normalizing known npm and mise launchers before
//! probing `--version`. An optional server URL is probed independently. Discovery
//! never spawns `opencode serve`; that lifecycle belongs to the server manager.
//!
//! Availability reports failed resolution as `installed: false`, keeping fresh
//! machines and invalid overrides out of the spawn path without installing or
//! updating any tools.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::execution::sanitize_gui_env_tokio;

/// Minimum OpenCode CLI version Codemux requires.
/// Versions below this are reported as `installed: true` with the
/// detected version still attached so the eventual UI can surface a
/// "please upgrade" hint instead of a hard error.
pub const MINIMUM_OPENCODE_VERSION: &str = "1.14.19";

/// Hard cap on each subprocess / network probe so a flaky CLI or
/// unreachable server never stalls startup. Tuned to match the codex
/// auth-probe budget at `agent_provider/codex/auth.rs`.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Snapshot returned by [`check_opencode_availability`].
///
/// All fields are independently meaningful so the UI can render
/// granular states (installed but server down, server up but version
/// too old, etc.) without re-probing. JSON shape is locked at Stage 1
/// — Stage 2/3 may add fields but must not rename existing ones, since
/// the frontend wrapper in `src/tauri/commands.ts` consumes this
/// verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeAvailability {
    /// True when a prepared OpenCode executable was resolved from the explicit
    /// override or PATH. Independent of whether the server is reachable.
    pub installed: bool,
    /// Canonical prepared executable path. `None` when resolution failed.
    pub binary_path: Option<PathBuf>,
    /// Version string parsed out of `opencode --version`. Best-effort:
    /// `None` when the binary refused to run, exited non-zero, or
    /// emitted an unparseable banner.
    pub version: Option<String>,
    /// True when an HTTP server was reachable at the URL the caller
    /// supplied. Always `false` when `server_url` is `None` — Stage 1
    /// does not spawn its own server, so a true value here only
    /// happens when discovery was given a URL to probe.
    pub server_running: bool,
    /// URL the caller asked us to probe. Echoed back so the frontend
    /// does not need to keep two copies of the value in sync.
    pub server_url: Option<String>,
}

impl OpenCodeAvailability {
    /// Construct the "binary not installed" empty state.
    ///
    /// Hand-rolled because `Default` would imply `installed = false`
    /// is "valid forever"; this constructor exists so failure paths
    /// inside [`check_opencode_availability`] can short-circuit
    /// without manually filling every field, and so tests can assert
    /// against a single canonical not-installed value.
    pub fn not_installed() -> Self {
        Self {
            installed: false,
            binary_path: None,
            version: None,
            server_running: false,
            server_url: None,
        }
    }
}

/// Probe whether OpenCode is usable on this machine.
///
/// Steps:
///
/// 1. Resolve the prepared executable (override, otherwise PATH).
/// 2. Run `opencode --version` (timeout-bound) and pluck the version
///    token out of the first line. A non-zero exit or unparseable
///    banner leaves `version` as `None` but keeps `installed: true`.
/// 3. If `server_url` was supplied, send a `GET` and treat any HTTP
///    response (including 401/403) as "server is up". The caller's
///    auth credentials are not validated here — that's the eventual
///    Stage 2 job.
///
/// `server_url` is **always echoed back** in the response, independent
/// of whether the binary was found — a user could point the integration
/// at a remote OpenCode server they manage themselves, and the eventual
/// settings-panel UI needs to render a "url X is/isn't reachable" hint
/// even when the local binary is absent.
///
/// Spawns are routed through [`sanitize_gui_env_tokio`] so an OpenCode binary
/// that inadvertently launches a GUI helper does not pop windows on the host
/// display.
pub async fn check_opencode_availability(server_url: Option<String>) -> OpenCodeAvailability {
    let (server_running, server_url) = match server_url {
        Some(url) => {
            let reachable = probe_server(&url).await;
            (reachable, Some(url))
        }
        None => (false, None),
    };

    let binary_path = match prepared_binary().await {
        Ok(path) => path,
        Err(_) => {
            return OpenCodeAvailability {
                installed: false,
                binary_path: None,
                version: None,
                server_running,
                server_url,
            };
        }
    };

    let version = probe_version(&binary_path).await;

    OpenCodeAvailability {
        installed: true,
        binary_path: Some(binary_path),
        version,
        server_running,
        server_url,
    }
}

/// Resolve one prepared installation for availability, ordinary chat and managed
/// workflows. Explicit overrides are authoritative, including invalid ones.
/// Never run the OpenCode launcher to discover its target or install/update tools.
pub(crate) async fn prepared_binary() -> Result<PathBuf, String> {
    let configured = std::env::var_os("CODEMUX_OPENCODE_BINARY").filter(|value| !value.is_empty());
    let path = configured
        .map(PathBuf::from)
        .or_else(|| which::which("opencode").ok())
        .ok_or_else(|| {
            "Install a supported OpenCode CLI or set CODEMUX_OPENCODE_BINARY to its prepared executable"
                .to_string()
        })?;
    resolve_prepared_binary(path).await
}

fn mise_wrapper(script: &str) -> bool {
    script.starts_with("#!/")
        && script.contains("mise use -g")
        && script.lines().any(|line| {
            let words: Vec<_> = line
                .split_whitespace()
                .map(|word| word.trim_matches('"'))
                .collect();
            words.len() >= 5 && words[..5] == ["exec", "mise", "x", "opencode", "--"]
        })
}

fn mise_launcher_kind(path: &Path) -> Option<bool> {
    if read_small_text(path, 8192).is_some_and(|script| mise_wrapper(&script)) {
        return Some(true);
    }
    let shim = path
        .parent()
        .is_some_and(|parent| parent.ends_with("shims"))
        && path
            .canonicalize()
            .ok()
            .is_some_and(|resolved| resolved.file_stem().is_some_and(|name| name == "mise"));
    shim.then_some(false)
}

async fn resolve_prepared_binary(mut path: PathBuf) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("OpenCode requires an absolute prepared executable path".into());
    }
    // Omarchy's wrapper runs `mise use -g` even for --version. Inspect it,
    // then ask mise only for an already-installed target, with hooks disabled.
    if let Some(wrapper) = mise_launcher_kind(&path) {
        let mise = which::which("mise")
            .map_err(|_| "Cannot resolve the prepared OpenCode mise installation".to_string())?;
        let mut command = crate::execution::host_command_tokio(mise);
        command.args(["which", "opencode"]);
        if wrapper {
            command.args(["--tool", "opencode"]);
        }
        command
            .env("MISE_AUTO_INSTALL", "false")
            .env("MISE_NO_ENV", "1")
            .env("MISE_NO_HOOKS", "1")
            .env("MISE_AUTO_UPDATE", "false")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        if let Some(home) = dirs::home_dir() {
            command.current_dir(home);
        }
        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .map_err(|_| "Resolving prepared OpenCode timed out".to_string())?
            .map_err(|_| "Cannot resolve prepared OpenCode".to_string())?;
        if !output.status.success() || output.stdout.len() > 8192 {
            return Err("OpenCode is not prepared; CodeMux will not install it".into());
        }
        let resolved = std::str::from_utf8(&output.stdout)
            .ok()
            .map(str::trim)
            .filter(|text| !text.is_empty() && !text.contains(['\n', '\r']))
            .ok_or_else(|| "OpenCode resolver returned an invalid path".to_string())?;
        path = resolved.into();
        if !path.is_absolute() {
            return Err("OpenCode requires an absolute prepared executable path".into());
        }
        if mise_launcher_kind(&path).is_some() {
            return Err(
                "OpenCode mise resolver returned a launcher instead of a prepared executable"
                    .into(),
            );
        }
    }
    normalize_windows_npm_launcher(&path)?
        .canonicalize()
        .ok()
        .filter(|value| value.is_file())
        .ok_or_else(|| "The prepared OpenCode executable is unavailable".into())
}

/// Bounded passive reads only: npm launchers are never evaluated. Recognizing
/// .cmd is not a workaround for Rust Command (which can launch .cmd on Windows);
/// returning the native target removes the shell hop and gives both owners the
/// same executable. Keep the lexical launcher parent before canonicalization:
/// local node_modules/.bin launchers resolve relative to that directory.
fn normalize_windows_npm_launcher(path: &Path) -> Result<PathBuf, String> {
    #[cfg(target_arch = "x86_64")]
    let avx2 = std::is_x86_feature_detected!("avx2");
    #[cfg(not(target_arch = "x86_64"))]
    let avx2 = false;
    normalize_windows_npm_launcher_for_target(path, std::env::consts::ARCH, avx2)
}

fn normalize_windows_npm_launcher_for_target(
    path: &Path,
    arch: &str,
    avx2: bool,
) -> Result<PathBuf, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !extension.eq_ignore_ascii_case("cmd") && !extension.eq_ignore_ascii_case("ps1") {
        return Ok(path.to_path_buf());
    }
    let error = || {
        "Cannot resolve this OpenCode npm launcher to an adjacent prepared native executable"
            .to_string()
    };
    let name = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(error)?
        .to_ascii_lowercase();
    if name != "opencode" && name != "opencode2" {
        return Err(error());
    }
    let script = read_small_text(path, 8192).ok_or_else(error)?;
    let parent = path.parent().ok_or_else(error)?;
    let local = parent
        .file_name()
        .is_some_and(|value| value.eq_ignore_ascii_case(".bin"));
    let modules = if local {
        let modules = parent.parent().ok_or_else(error)?;
        if !modules
            .file_name()
            .is_some_and(|value| value.eq_ignore_ascii_case("node_modules"))
        {
            return Err(error());
        }
        modules.to_path_buf()
    } else {
        parent.join("node_modules")
    };
    // Preserve the selected installation, not whichever adjacent package exists
    // first. The quoted literal target and package/bin metadata must all agree.
    for package in ["opencode-ai", "@opencode/cli"] {
        let package_dir = modules.join(package);
        let Some(text) = read_small_text(&package_dir.join("package.json"), 65536) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if manifest.get("name").and_then(|value| value.as_str()) != Some(package) {
            continue;
        }
        let Some(bin) = manifest
            .get("bin")
            .and_then(|value| value.get(&name))
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        let bin = bin.replace('\\', "/");
        let bin = bin.strip_prefix("./").unwrap_or(&bin);
        let legacy = package == "opencode-ai" && bin.eq_ignore_ascii_case("bin/opencode");
        if !legacy && !bin.eq_ignore_ascii_case("bin/opencode.exe") {
            continue;
        }
        let relative = if local {
            format!("../{package}/{bin}")
        } else {
            format!("node_modules/{package}/{bin}")
        };
        let matches = script.split('"').skip(1).step_by(2).any(|quoted| {
            let quoted = quoted.replace('\\', "/");
            let quoted = quoted.to_ascii_lowercase();
            ["%dp0%/", "$basedir/", "${basedir}/"].iter().any(|prefix| {
                quoted
                    .strip_prefix(prefix)
                    .is_some_and(|value| value.eq_ignore_ascii_case(&relative))
            })
        });
        let native = package_dir.join(bin);
        if matches && native.is_file() {
            if legacy {
                return resolve_legacy_windows_dependency(
                    &package_dir,
                    &modules,
                    &manifest,
                    arch,
                    avx2,
                );
            }
            return Ok(native);
        }
    }
    Err(error())
}

fn resolve_legacy_windows_dependency(
    package_dir: &Path,
    modules: &Path,
    manifest: &serde_json::Value,
    arch: &str,
    avx2: bool,
) -> Result<PathBuf, String> {
    let error = || {
        "Cannot resolve this legacy OpenCode npm launcher to a same-version prepared Windows optional dependency; check its node_modules or set CODEMUX_OPENCODE_BINARY to the prepared native executable".to_string()
    };
    let version = manifest
        .get("version")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(error)?;
    let dependencies = manifest.get("optionalDependencies").ok_or_else(error)?;
    // Match the published launcher's ordering, without running Node/PowerShell.
    let names: &[&str] = match (arch, avx2) {
        ("x86_64", true) => &["opencode-windows-x64", "opencode-windows-x64-baseline"],
        ("x86_64", false) => &["opencode-windows-x64-baseline", "opencode-windows-x64"],
        ("aarch64", _) => &["opencode-windows-arm64"],
        _ => return Err(error()),
    };
    // Only this selected package's nested dependencies and enclosing modules
    // are eligible. Never walk arbitrary ancestors or try another PATH install.
    for root in [package_dir.join("node_modules"), modules.to_path_buf()] {
        for name in names {
            let directory = root.join(name);
            if !directory.exists() {
                continue;
            }
            // An installed but mismatched package is a broken selected install,
            // not permission to use a different release or an enclosing copy.
            if dependencies.get(name).and_then(|value| value.as_str()) != Some(version) {
                return Err(error());
            }
            let text = read_small_text(&directory.join("package.json"), 65536).ok_or_else(error)?;
            let dependency: serde_json::Value = serde_json::from_str(&text).map_err(|_| error())?;
            if dependency.get("name").and_then(|value| value.as_str()) != Some(*name)
                || dependency.get("version").and_then(|value| value.as_str()) != Some(version)
            {
                return Err(error());
            }
            let native = directory.join("bin/opencode.exe");
            if native.is_file() {
                return Ok(native);
            }
        }
    }
    Err(error())
}

fn read_small_text(path: &Path, limit: u64) -> Option<String> {
    use std::io::Read;
    // Reject directories/devices/FIFOs before opening; a FIFO open can block
    // even when a later handle-metadata check would reject it.
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    let mut text = String::new();
    file.take(limit + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= limit).then_some(text)
}

/// Run `opencode --version` and pull the version token out of the
/// first line. Returns `None` on any failure — the caller treats that
/// as "binary present but version unknown".
async fn probe_version(binary_path: &Path) -> Option<String> {
    let mut cmd = Command::new(binary_path);
    cmd.arg("--version").kill_on_drop(true);
    sanitize_gui_env_tokio(&mut cmd);
    let result = tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await;
    let output = match result {
        Ok(Ok(out)) => out,
        Ok(Err(_)) => return None,
        Err(_) => return None,
    };
    if !output.status.success() {
        return None;
    }
    parse_version_output(&String::from_utf8_lossy(&output.stdout))
}

/// Pure helper so version-banner parsing is unit-testable.
///
/// Behaviour:
///
/// * Trim and split on lines; bail on empty input.
/// * Take the first non-empty line.
/// * If that line has more than one whitespace-separated token, the
///   last token is the version (matches typical `opencode 1.14.19`
///   output and is robust against future banners that prepend a
///   binary name).
/// * Otherwise the whole line is the version (some builds print
///   `1.14.19` with no prefix).
pub fn parse_version_output(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first_line = trimmed.lines().find(|l| !l.trim().is_empty())?.trim();
    let tokens: Vec<&str> = first_line.split_whitespace().collect();
    let token = match tokens.as_slice() {
        [] => return None,
        [single] => *single,
        many => *many.last().unwrap(),
    };
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Treat any HTTP response (2xx, 4xx, 5xx) as "server is up" — the
/// goal here is "is something listening", not "is the user
/// authenticated". A network-level failure (connection refused, DNS,
/// timeout) returns false.
async fn probe_server(url: &str) -> bool {
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(c) => c,
        Err(_) => return false,
    };
    client.get(url).send().await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_opencode_wrapper_detection_never_evaluates_shell() {
        assert!(mise_wrapper("#!/bin/bash\nflock lock mise use -g opencode\nexec mise x \"opencode\" -- \"$bin_path\" \"$@\""));
        assert!(!mise_wrapper(
            "#!/bin/bash\nexec mise x \"opencode; arbitrary\" -- opencode"
        ));
        assert!(!mise_wrapper(
            "#!/bin/bash\nmise use -g other\nexec mise x other -- opencode"
        ));
    }

    fn npm_fixture(root: &Path, local: bool, package: &str, launcher: &str) -> (PathBuf, PathBuf) {
        let modules = root.join("node_modules");
        let bin_dir = if local {
            modules.join(".bin")
        } else {
            root.to_path_buf()
        };
        let package_dir = modules.join(package);
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::create_dir_all(package_dir.join("bin")).unwrap();
        std::fs::write(
            package_dir.join("package.json"),
            serde_json::json!({"name": package, "bin": {"opencode": "./bin/opencode.exe"}})
                .to_string(),
        )
        .unwrap();
        let native = package_dir.join("bin/opencode.exe");
        std::fs::write(&native, "synthetic native marker; never executed").unwrap();
        let relative = if local {
            format!("../{package}/bin/opencode.exe")
        } else {
            format!("node_modules/{package}/bin/opencode.exe")
        };
        let path = bin_dir.join(launcher);
        std::fs::write(
            &path,
            format!("@ECHO off\n\"%dp0%\\{}\" %*\n", relative.replace('/', "\\")),
        )
        .unwrap();
        (path, native.canonicalize().unwrap())
    }

    fn legacy_npm_fixture(root: &Path, local: bool, nested: bool) -> (PathBuf, PathBuf) {
        let (launcher, modern) = npm_fixture(root, local, "opencode-ai", "opencode.cmd");
        std::fs::remove_file(modern).unwrap();
        let modules = root.join("node_modules");
        let package = modules.join("opencode-ai");
        std::fs::write(
            package.join("bin/opencode"),
            "#!/usr/bin/env node\nthrow Error('must never execute');\n",
        )
        .unwrap();
        std::fs::write(
            package.join("package.json"),
            serde_json::json!({
                "name":"opencode-ai", "version":"1.14.31", "bin":{"opencode":"./bin/opencode"},
                "optionalDependencies": {
                    "opencode-windows-x64":"1.14.31", "opencode-windows-x64-baseline":"1.14.31",
                    "opencode-windows-arm64":"1.14.31"
                }
            })
            .to_string(),
        )
        .unwrap();
        let relative = if local {
            "../opencode-ai/bin/opencode"
        } else {
            "node_modules/opencode-ai/bin/opencode"
        };
        std::fs::write(&launcher, format!("@echo off\n\"%dp0%/{}\" %*\n", relative)).unwrap();
        let dependencies = if nested {
            package.join("node_modules")
        } else {
            modules
        };
        for name in [
            "opencode-windows-x64",
            "opencode-windows-x64-baseline",
            "opencode-windows-arm64",
        ] {
            let dependency = dependencies.join(name);
            std::fs::create_dir_all(dependency.join("bin")).unwrap();
            std::fs::write(
                dependency.join("package.json"),
                serde_json::json!({"name":name,"version":"1.14.31"}).to_string(),
            )
            .unwrap();
            std::fs::write(
                dependency.join("bin/opencode.exe"),
                "inert native marker; never executed",
            )
            .unwrap();
        }
        (launcher, dependencies)
    }

    #[tokio::test]
    async fn npm_legacy_1_14_31_resolves_selected_same_version_dependency_passively() {
        for local in [false, true] {
            for nested in [false, true] {
                let tmp = tempfile::tempdir().unwrap();
                let (launcher, dependencies) = legacy_npm_fixture(tmp.path(), local, nested);
                #[cfg(target_arch = "x86_64")]
                let name = if std::is_x86_feature_detected!("avx2") {
                    "opencode-windows-x64"
                } else {
                    "opencode-windows-x64-baseline"
                };
                #[cfg(target_arch = "aarch64")]
                let name = "opencode-windows-arm64";
                #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
                let name = "unsupported-architecture";
                assert_eq!(
                    resolve_prepared_binary(launcher).await.unwrap(),
                    dependencies
                        .join(name)
                        .join("bin/opencode.exe")
                        .canonicalize()
                        .unwrap()
                );
            }
        }
    }

    #[test]
    fn npm_legacy_selects_windows_architecture_and_avx2_without_execution() {
        let tmp = tempfile::tempdir().unwrap();
        let (launcher, dependencies) = legacy_npm_fixture(tmp.path(), false, false);
        for (arch, avx2, name) in [
            ("x86_64", true, "opencode-windows-x64"),
            ("x86_64", false, "opencode-windows-x64-baseline"),
            ("aarch64", false, "opencode-windows-arm64"),
        ] {
            assert_eq!(
                normalize_windows_npm_launcher_for_target(&launcher, arch, avx2).unwrap(),
                dependencies.join(name).join("bin/opencode.exe")
            );
        }
        assert!(normalize_windows_npm_launcher_for_target(&launcher, "riscv64", false).is_err());
    }

    #[test]
    fn npm_legacy_rejects_mismatched_or_missing_selected_dependency() {
        for mutation in [
            "wrong-name",
            "wrong-version",
            "wrong-declaration",
            "missing",
            "directory",
            "missing-launcher",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let (launcher, dependencies) = legacy_npm_fixture(tmp.path(), false, true);
            let selected = dependencies.join("opencode-windows-x64");
            match mutation {
                "wrong-name" | "wrong-version" => {
                    std::fs::write(selected.join("package.json"), serde_json::json!({
                        "name": if mutation == "wrong-name" { "other" } else { "opencode-windows-x64" },
                        "version": if mutation == "wrong-version" { "1.18.35" } else { "1.14.31" }
                    }).to_string()).unwrap();
                    // A valid enclosing copy must not mask a broken nested selection.
                    let enclosing = tmp.path().join("node_modules/opencode-windows-x64");
                    std::fs::create_dir_all(enclosing.join("bin")).unwrap();
                    std::fs::write(
                        enclosing.join("package.json"),
                        r#"{"name":"opencode-windows-x64","version":"1.14.31"}"#,
                    )
                    .unwrap();
                    std::fs::write(enclosing.join("bin/opencode.exe"), "inert").unwrap();
                }
                "wrong-declaration" => {
                    let package = tmp.path().join("node_modules/opencode-ai/package.json");
                    let mut manifest: serde_json::Value =
                        serde_json::from_str(&std::fs::read_to_string(&package).unwrap()).unwrap();
                    manifest["optionalDependencies"]["opencode-windows-x64"] = "1.18.35".into();
                    std::fs::write(package, manifest.to_string()).unwrap();
                }
                "missing" | "directory" => {
                    for name in ["opencode-windows-x64", "opencode-windows-x64-baseline"] {
                        let native = dependencies.join(name).join("bin/opencode.exe");
                        std::fs::remove_file(&native).unwrap();
                        if mutation == "directory" {
                            std::fs::create_dir(native).unwrap();
                        }
                    }
                }
                "missing-launcher" => {
                    std::fs::remove_file(tmp.path().join("node_modules/opencode-ai/bin/opencode"))
                        .unwrap()
                }
                _ => unreachable!(),
            }
            assert!(
                normalize_windows_npm_launcher_for_target(&launcher, "x86_64", true).is_err(),
                "accepted {mutation}"
            );
        }
    }

    #[test]
    fn npm_legacy_prefers_nested_dependency_and_falls_back_only_within_selected_install() {
        let tmp = tempfile::tempdir().unwrap();
        let (launcher, dependencies) = legacy_npm_fixture(tmp.path(), true, true);
        let regular = dependencies.join("opencode-windows-x64/bin/opencode.exe");
        let baseline = dependencies.join("opencode-windows-x64-baseline/bin/opencode.exe");
        // Modern adjacent package must not steal this legacy launcher's selection.
        npm_fixture(tmp.path(), true, "@opencode/cli", "opencode2.cmd");
        assert_eq!(
            normalize_windows_npm_launcher_for_target(&launcher, "x86_64", true).unwrap(),
            regular
        );
        std::fs::remove_file(regular).unwrap();
        assert_eq!(
            normalize_windows_npm_launcher_for_target(&launcher, "x86_64", true).unwrap(),
            baseline
        );
        std::fs::write(
            &launcher,
            "& \"${basedir}/../opencode-ai/bin/opencode\" $args\n",
        )
        .unwrap();
        let powershell = launcher.with_extension("ps1");
        std::fs::rename(launcher, &powershell).unwrap();
        assert_eq!(
            normalize_windows_npm_launcher_for_target(&powershell, "x86_64", false).unwrap(),
            baseline
        );
    }

    #[tokio::test]
    async fn npm_global_launcher_resolves_its_adjacent_native_v1() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("npm prefix with spaces");
        let (launcher, native) = npm_fixture(&root, false, "opencode-ai", "OpenCode.CmD");
        assert_eq!(resolve_prepared_binary(launcher).await.unwrap(), native);
    }

    #[tokio::test]
    async fn npm_local_powershell_launcher_resolves_v2_with_spaces_and_case() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("local install with spaces");
        let (launcher, native) = npm_fixture(&root, true, "@opencode/cli", "OPENCODE.PS1");
        std::fs::write(
            &launcher,
            "#!/usr/bin/env pwsh\n& \"$basedir/../@OPENCODE/CLI/BIN/OPENCODE.EXE\" $args\n",
        )
        .unwrap();
        assert_eq!(resolve_prepared_binary(launcher).await.unwrap(), native);
    }

    #[tokio::test]
    async fn npm_local_layout_names_are_case_insensitive_and_v2_alias_matches_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let (launcher, _) = npm_fixture(tmp.path(), true, "@opencode/cli", "opencode2.ps1");
        let modules = tmp.path().join("node_modules");
        std::fs::write(
            modules.join("@opencode/cli/package.json"),
            serde_json::json!({
                "name":"@opencode/cli", "bin":{"opencode2":"./bin/opencode.exe"}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &launcher,
            "& \"${basedir}/../@opencode/cli/bin/opencode.exe\" $args\n",
        )
        .unwrap();
        std::fs::rename(modules.join(".bin"), modules.join(".BIN")).unwrap();
        let mixed_modules = tmp.path().join("NoDe_MoDuLeS");
        std::fs::rename(&modules, &mixed_modules).unwrap();
        let native = mixed_modules
            .join("@opencode/cli/bin/opencode.exe")
            .canonicalize()
            .unwrap();
        assert_eq!(
            resolve_prepared_binary(mixed_modules.join(".BIN/opencode2.ps1"))
                .await
                .unwrap(),
            native
        );
    }

    #[tokio::test]
    async fn npm_launcher_keeps_its_selected_package_not_adjacent_v1() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("global prefix");
        let (_, v1) = npm_fixture(&root, false, "opencode-ai", "opencode.cmd");
        let (launcher, v2) = npm_fixture(&root, false, "@opencode/cli", "opencode.cmd");
        assert_ne!(v1, v2);
        assert_eq!(resolve_prepared_binary(launcher.clone()).await.unwrap(), v2);
        std::fs::remove_file(v2).unwrap();
        assert!(
            resolve_prepared_binary(launcher).await.is_err(),
            "must not fall back to adjacent v1"
        );
    }

    #[tokio::test]
    async fn npm_launcher_requires_matching_package_identity_bin_and_native_file() {
        for manifest in [
            serde_json::json!({"name":"other", "bin":{"opencode":"bin/opencode.exe"}}),
            serde_json::json!({"name":"opencode-ai", "bin":{"other":"bin/opencode.exe"}}),
            serde_json::json!({"name":"opencode-ai", "bin":{"opencode":"../bin/opencode.exe"}}),
            serde_json::json!({"name":"opencode-ai", "bin":{"opencode":"bin/other.exe"}}),
            serde_json::json!({"name":"opencode-ai", "bin":"bin/opencode.exe"}),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let (launcher, _) = npm_fixture(tmp.path(), false, "opencode-ai", "opencode.cmd");
            std::fs::write(
                tmp.path().join("node_modules/opencode-ai/package.json"),
                manifest.to_string(),
            )
            .unwrap();
            assert!(
                resolve_prepared_binary(launcher).await.is_err(),
                "accepted {manifest}"
            );
        }
        let tmp = tempfile::tempdir().unwrap();
        let (launcher, native) = npm_fixture(tmp.path(), true, "@opencode/cli", "opencode.cmd");
        std::fs::remove_file(&native).unwrap();
        std::fs::create_dir(&native).unwrap();
        assert!(
            resolve_prepared_binary(launcher).await.is_err(),
            "a directory is not a native executable"
        );
    }

    #[tokio::test]
    async fn npm_launcher_rejects_unrelated_or_unreadable_shims_without_execution() {
        for text in [
            "@echo off\n\"%dp0%/node_modules/other/bin/opencode.exe\" %*".to_string(),
            "@echo off\n\"%dp0%/node_modules/opencode-ai/bin/opencode.exe.extra\" %*".to_string(),
            "x".repeat(8193),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let (launcher, _) = npm_fixture(tmp.path(), false, "opencode-ai", "opencode.cmd");
            std::fs::write(&launcher, text).unwrap();
            assert!(resolve_prepared_binary(launcher).await.is_err());
        }
        let tmp = tempfile::tempdir().unwrap();
        let (launcher, _) = npm_fixture(tmp.path(), false, "opencode-ai", "opencode.cmd");
        std::fs::write(
            tmp.path().join("node_modules/opencode-ai/package.json"),
            "not JSON",
        )
        .unwrap();
        assert!(resolve_prepared_binary(launcher.clone()).await.is_err());
        std::fs::remove_file(&launcher).unwrap();
        assert!(resolve_prepared_binary(launcher).await.is_err());
    }

    #[tokio::test]
    async fn prepared_native_path_is_preserved_and_relative_paths_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let native = tmp.path().join("OpenCode.EXE");
        std::fs::write(&native, "native marker").unwrap();
        assert_eq!(
            resolve_prepared_binary(native.clone()).await.unwrap(),
            native.canonicalize().unwrap()
        );
        assert!(
            resolve_prepared_binary(PathBuf::from("relative/opencode.exe"))
                .await
                .is_err()
        );
        assert!(resolve_prepared_binary(tmp.path().to_path_buf())
            .await
            .is_err());
    }

    #[test]
    fn not_installed_constructor_zeros_every_field() {
        let av = OpenCodeAvailability::not_installed();
        assert!(!av.installed);
        assert_eq!(av.binary_path, None);
        assert_eq!(av.version, None);
        assert!(!av.server_running);
        assert_eq!(av.server_url, None);
    }

    #[test]
    fn parse_version_output_handles_prefixed_banner() {
        assert_eq!(
            parse_version_output("opencode 1.14.19\n"),
            Some("1.14.19".into())
        );
    }

    #[test]
    fn parse_version_output_handles_bare_version() {
        assert_eq!(parse_version_output("1.14.19"), Some("1.14.19".into()));
    }

    #[test]
    fn parse_version_output_skips_leading_blank_lines() {
        // Some CLIs print a blank line before the banner when stdout
        // is line-buffered.
        assert_eq!(
            parse_version_output("\n\nopencode 1.15.0\nthird line\n"),
            Some("1.15.0".into())
        );
    }

    #[test]
    fn parse_version_output_returns_none_for_empty() {
        assert_eq!(parse_version_output(""), None);
        assert_eq!(parse_version_output("   \n  "), None);
    }

    #[test]
    fn parse_version_output_takes_last_token_when_multiple_words() {
        // Future-proofs against `opencode cli 1.14.19` style banners.
        assert_eq!(
            parse_version_output("opencode cli 1.14.19"),
            Some("1.14.19".into())
        );
    }

    #[test]
    fn minimum_version_constant_matches_reference_pin() {
        // Pinned: changing this value silently changes which OpenCode
        // versions Codemux warns about.
        assert_eq!(MINIMUM_OPENCODE_VERSION, "1.14.19");
    }

    /// Tests in this module that mutate `PATH` or rely on the live
    /// `opencode` binary are serialised under the `opencode_path`
    /// label (shared with `manager.rs`'s sibling tests). Without
    /// this guard a PATH-mutating sibling test would race the
    /// live-binary tests under parallel cargo runs.
    use serial_test::serial;

    struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl EnvGuard {
        fn new(values: &[(&'static str, Option<&std::ffi::OsStr>)]) -> Self {
            let guard = Self(
                values
                    .iter()
                    .map(|(key, _)| (*key, std::env::var_os(key)))
                    .collect(),
            );
            for (key, value) in values {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn windows_legacy_path_selection_never_falls_back_to_later_modern_install() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("legacy first");
        let second = tmp.path().join("modern second");
        let (launcher, dependencies) = legacy_npm_fixture(&first, false, false);
        npm_fixture(&second, false, "@opencode/cli", "opencode.cmd");
        let path = std::env::join_paths([&first, &second]).unwrap();
        let _env = EnvGuard::new(&[
            ("PATH", Some(&path)),
            ("PATHEXT", Some(std::ffi::OsStr::new(".CMD;.EXE"))),
            ("CODEMUX_OPENCODE_BINARY", None),
        ]);
        assert_eq!(
            prepared_binary().await.unwrap(),
            resolve_prepared_binary(launcher).await.unwrap()
        );
        for name in [
            "opencode-windows-x64",
            "opencode-windows-x64-baseline",
            "opencode-windows-arm64",
        ] {
            std::fs::remove_file(dependencies.join(name).join("bin/opencode.exe")).unwrap();
        }
        let error = prepared_binary().await.unwrap_err();
        assert!(
            error.contains("same-version prepared Windows optional dependency"),
            "{error}"
        );
    }

    /// Parent CI owns installation and environment isolation. Run this ignored
    /// gate explicitly in separate processes for legacy/current and global/local
    /// layouts; absent fixtures are errors, never an optional live-host skip.
    #[cfg(windows)]
    #[tokio::test]
    #[serial(opencode_path)]
    #[ignore = "requires an isolated, locally installed real npm package fixture"]
    async fn windows_real_npm_launcher_resolves_and_reports_native_version() {
        let launcher = PathBuf::from(
            std::env::var_os("CODEMUX_TEST_OPENCODE_NPM_LAUNCHER")
                .expect("set CODEMUX_TEST_OPENCODE_NPM_LAUNCHER to the fixture's opencode.cmd"),
        );
        let expected = std::env::var("CODEMUX_TEST_OPENCODE_VERSION")
            .expect("set CODEMUX_TEST_OPENCODE_VERSION to the installed release pin");
        assert!(matches!(
            expected.as_str(),
            "1.14.31" | "1.18.35" | "2.0.27"
        ));
        assert!(launcher.is_absolute());
        assert!(launcher.extension().unwrap().eq_ignore_ascii_case("cmd"));
        let native = resolve_prepared_binary(launcher.clone()).await.unwrap();
        assert!(native.extension().unwrap().eq_ignore_ascii_case("exe"));
        assert_ne!(native, launcher.canonicalize().unwrap());
        assert_eq!(
            probe_version(&native).await.as_deref(),
            Some(expected.as_str())
        );
        // Only the normalized native executable may run. In particular the
        // legacy Node launcher creates a subprocess and cannot own our server.
        if expected == "1.14.31" {
            let dependency = native.parent().unwrap().parent().unwrap();
            let manifest: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dependency.join("package.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(manifest["version"], "1.14.31");
            assert!(manifest["name"]
                .as_str()
                .unwrap()
                .starts_with("opencode-windows-"));
        }
        let powershell = launcher.with_extension("ps1");
        assert_eq!(
            resolve_prepared_binary(powershell.clone()).await.unwrap(),
            native
        );
        let _env = EnvGuard::new(&[
            ("PATH", Some(launcher.parent().unwrap().as_os_str())),
            ("CODEMUX_OPENCODE_BINARY", None),
        ]);
        assert_eq!(prepared_binary().await.unwrap(), native);
        let availability = check_opencode_availability(None).await;
        assert_eq!(availability.binary_path, Some(native.clone()));
        assert_eq!(availability.version.as_deref(), Some(expected.as_str()));
        let _override = EnvGuard::new(&[("CODEMUX_OPENCODE_BINARY", Some(powershell.as_os_str()))]);
        assert_eq!(prepared_binary().await.unwrap(), native);
        let missing = launcher.parent().unwrap().join("missing.exe");
        let _bad = EnvGuard::new(&[("CODEMUX_OPENCODE_BINARY", Some(missing.as_os_str()))]);
        assert!(prepared_binary().await.is_err());
        assert!(!check_opencode_availability(None).await.installed);
    }

    #[cfg(unix)]
    fn executable_script(path: &Path, text: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn prepared_mise_wrapper_is_resolved_without_updating_or_executing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let native = tmp.path().join("prepared native");
        std::fs::write(&native, "prepared marker").unwrap();
        let marker = tmp.path().join("wrapper-executed");
        let args = tmp.path().join("mise-args");
        let wrapper = tmp.path().join("opencode");
        executable_script(&wrapper, &format!("#!/bin/sh\nprintf touched > '{}'\nmise use -g opencode\nexec mise x \"opencode\" -- \"$@\"\n", marker.display()));
        let mise = tmp.path().join("mise");
        executable_script(&mise, &format!("#!/bin/sh\n[ \"$MISE_AUTO_INSTALL\" = false ] && [ \"$MISE_AUTO_UPDATE\" = false ] && [ \"$MISE_NO_ENV\" = 1 ] && [ \"$MISE_NO_HOOKS\" = 1 ] || exit 7\nprintf '%s' \"$*\" > '{}'\nprintf '%s\\n' '{}'\n", args.display(), native.display()));
        let _env = EnvGuard::new(&[
            ("PATH", Some(tmp.path().as_os_str())),
            ("CODEMUX_OPENCODE_BINARY", None),
        ]);
        assert_eq!(
            prepared_binary().await.unwrap(),
            native.canonicalize().unwrap()
        );
        assert!(!marker.exists(), "OpenCode wrapper must never execute");
        assert_eq!(
            std::fs::read_to_string(&args).unwrap(),
            "which opencode --tool opencode"
        );
        // A real mise shim is a symlink to mise; it requires no tool override.
        let shims = tmp.path().join("shims");
        std::fs::create_dir(&shims).unwrap();
        let shim = shims.join("opencode");
        std::os::unix::fs::symlink(&mise, &shim).unwrap();
        assert_eq!(
            resolve_prepared_binary(shim).await.unwrap(),
            native.canonicalize().unwrap()
        );
        assert_eq!(std::fs::read_to_string(&args).unwrap(), "which opencode");
        executable_script(&mise, "#!/bin/sh\nexit 1\n");
        assert!(prepared_binary()
            .await
            .unwrap_err()
            .contains("will not install"));
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn prepared_mise_rejects_an_updating_wrapper_as_its_resolved_target() {
        let tmp = tempfile::tempdir().unwrap();
        let wrapper = tmp.path().join("opencode");
        executable_script(
            &wrapper,
            "#!/bin/sh\nmise use -g opencode\nexec mise x \"opencode\" -- \"$@\"\n",
        );
        let mise = tmp.path().join("mise");
        executable_script(
            &mise,
            &format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", wrapper.display()),
        );
        let _env = EnvGuard::new(&[
            ("PATH", Some(tmp.path().as_os_str())),
            ("CODEMUX_OPENCODE_BINARY", None),
        ]);
        assert!(
            prepared_binary().await.is_err(),
            "mise must resolve a prepared target, not its updating launcher"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn prepared_resolution_preserves_path_order_and_explicit_override() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let first_binary = first.join("opencode");
        let second_binary = second.join("opencode");
        executable_script(&first_binary, "#!/bin/sh\nprintf '1.18.35\\n'\n");
        executable_script(&second_binary, "#!/bin/sh\nprintf '2.0.27\\n'\n");
        let path = std::env::join_paths([&first, &second]).unwrap();
        let _env = EnvGuard::new(&[("PATH", Some(&path)), ("CODEMUX_OPENCODE_BINARY", None)]);
        assert_eq!(
            prepared_binary().await.unwrap(),
            first_binary.canonicalize().unwrap()
        );
        let _override =
            EnvGuard::new(&[("CODEMUX_OPENCODE_BINARY", Some(second_binary.as_os_str()))]);
        let availability = check_opencode_availability(None).await;
        assert_eq!(
            availability.binary_path,
            Some(second_binary.canonicalize().unwrap())
        );
        assert_eq!(availability.version.as_deref(), Some("2.0.27"));
        assert_eq!(
            prepared_binary().await.unwrap(),
            availability.binary_path.unwrap()
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dropping_version_probe_reaps_its_owned_child() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("owned-pid");
        let binary = tmp.path().join("opencode");
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec /bin/sleep 30\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let probe = tokio::spawn(async move { probe_version(&binary).await });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let pid = loop {
            if let Ok(text) = std::fs::read_to_string(&marker) {
                if let Ok(pid) = text.parse::<u32>() {
                    break pid;
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "synthetic version child did not start"
            );
            tokio::task::yield_now().await;
        };
        probe.abort();
        let _ = probe.await;
        let proc = PathBuf::from(format!("/proc/{pid}"));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while proc.exists() && tokio::time::Instant::now() < deadline {
            tokio::task::yield_now().await;
        }
        let reaped = !proc.exists();
        if !reaped {
            // Failure-safe cleanup is confined to the PID emitted by this child.
            let _ = Command::new("/bin/kill")
                .args(["-KILL", &pid.to_string()])
                .status()
                .await;
            while proc.exists() && tokio::time::Instant::now() < deadline + Duration::from_secs(3) {
                tokio::task::yield_now().await;
            }
        }
        assert!(reaped, "dropping the version probe left its child running");
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn availability_rejects_invalid_explicit_override_without_path_fallback() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let binary = tmp.path().join("opencode");
        std::fs::write(&binary, "#!/bin/sh\nprintf '1.18.35\\n'\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let missing = tmp.path().join("missing");
        let _env = EnvGuard::new(&[
            ("PATH", Some(tmp.path().as_os_str())),
            ("CODEMUX_OPENCODE_BINARY", Some(missing.as_os_str())),
        ]);
        let availability = check_opencode_availability(None).await;
        assert!(
            !availability.installed,
            "a bad explicit override must not use PATH"
        );
    }

    #[tokio::test]
    #[serial(opencode_path)]
    async fn check_when_binary_missing_returns_not_installed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Exclude inherited overrides as well as PATH; restore both on panic.
        let _env = EnvGuard::new(&[
            ("PATH", Some(tmp.path().as_os_str())),
            ("CODEMUX_OPENCODE_BINARY", None),
        ]);
        let av = check_opencode_availability(None).await;
        assert_eq!(av, OpenCodeAvailability::not_installed());
    }

    #[tokio::test]
    #[serial(opencode_path)]
    async fn check_with_unreachable_server_url_marks_server_down() {
        // No need for a binary on PATH; we're probing the server-URL
        // branch independently. Pick an obviously-unroutable URL so
        // reqwest fails fast on connect.
        // (Address chosen from RFC 5737 documentation block — never
        // routable.)
        let url = "http://192.0.2.1:1/".to_string();
        let av = check_opencode_availability(Some(url.clone())).await;

        // Whether `installed` is true or false depends on the test
        // host having opencode on PATH; we only care about the server
        // probe result.
        assert_eq!(av.server_url, Some(url));
        assert!(!av.server_running);
    }

    #[tokio::test]
    #[serial(opencode_path)]
    async fn check_against_live_binary_when_present() {
        // Live smoke: when this dev box has `opencode` on PATH, run
        // the full discovery pipeline and assert the live binary
        // reports a parseable version. Skipped (with a printed
        // breadcrumb) on machines without opencode so the test
        // remains green in CI / fresh worktrees.
        if which::which("opencode").is_err() {
            eprintln!(
                "[opencode::discovery] skipping live-binary smoke — \
                 opencode not on PATH"
            );
            return;
        }

        let av = check_opencode_availability(None).await;
        assert!(av.installed, "expected installed=true on live host");
        assert!(av.binary_path.is_some(), "binary_path must be populated");
        // Version should parse as something containing a dot — both
        // the `1.14.19` and `1.14.19-canary.0` shapes the upstream
        // banner can produce satisfy this.
        let version = av
            .version
            .as_ref()
            .expect("live opencode --version must produce a version");
        assert!(
            version.contains('.'),
            "version banner unexpectedly unparseable: {version:?}"
        );
        // server_url remained None because we passed None — the
        // server-probe branch is fully covered by the previous test.
        assert!(!av.server_running);
        assert_eq!(av.server_url, None);
    }
}
