//! Local CLI maintenance for agent chat. Never accepts commands from the renderer.
//! Detection precedes every update; an unknown owner is deliberately manual-only.
use crate::agent_provider::{claude, ProviderKind};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::LazyLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize)]
pub struct ProviderUpdate {
    pub provider: ProviderKind,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub available: bool,
    pub manager: String,
    pub can_update: bool,
    pub message: Option<String>,
}

#[derive(Clone, Debug)]
struct Installation {
    binary: PathBuf,
    manager: String,
    // All arguments are built here, never shell-interpolated or supplied by IPC.
    command: Option<(PathBuf, Vec<String>)>,
    mise_tool: Option<String>,
    mise_request: Option<String>,
}

// Serialize package-manager mutations across providers and coalesce checks across panes.
static MAINTENANCE: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
type Cache = HashMap<(ProviderKind, Option<String>), (Instant, ProviderUpdate)>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const CHECK_TTL: Duration = Duration::from_secs(3600);

fn binary_name(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "claude",
        ProviderKind::Codex => "codex",
        ProviderKind::Cursor => "cursor-agent",
        ProviderKind::Grok => "grok",
        ProviderKind::Hermes => "hermes",
        ProviderKind::Acp => "acp",
        ProviderKind::OpenCode => "opencode",
    }
}
fn package(provider: ProviderKind) -> Option<&'static str> {
    match provider {
        ProviderKind::Claude => Some("@anthropic-ai/claude-code"),
        ProviderKind::Codex => Some("@openai/codex"),
        ProviderKind::Grok => Some("@xai-official/grok"),
        ProviderKind::OpenCode => Some("opencode-ai"),
        _ => None,
    }
}
fn omarchy() -> bool {
    cfg!(target_os = "linux")
        && (Path::new("/usr/share/omarchy").is_dir()
            || crate::config::omarchy::theme_paths()
                .iter()
                .any(|p| p.exists()))
}
fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or("Home directory is unavailable".into())
}
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

async fn run(program: &Path, args: &[String], seconds: u64, mise: bool) -> Result<String, String> {
    let mut command = crate::execution::host_command_tokio(program);
    command
        .args(args)
        .current_dir(home()?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("NO_COLOR", "1");
    if mise {
        command
            .env("MISE_UPGRADE_AUTO_PRUNE", "false")
            .env("MISE_YES", "1")
            .env("MISE_FETCH_REMOTE_VERSIONS_CACHE", "0")
            .env("MISE_AQUA_REGISTRY_CACHE_TTL", "0s");
        if omarchy() {
            command.env("MISE_MINIMUM_RELEASE_AGE", "0");
        }
    }
    #[cfg(unix)]
    command.process_group(0);
    let child = command
        .spawn()
        .map_err(|e| format!("Could not run {}: {e}", program.display()))?;
    #[cfg(unix)]
    let child_id = child.id();
    let output =
        match tokio::time::timeout(Duration::from_secs(seconds), child.wait_with_output()).await {
            Ok(result) => result.map_err(|e| format!("Could not read updater output: {e}"))?,
            Err(_) => {
                // Only our isolated process group: npm/mise children must not keep
                // installing after the maintenance lock has been released.
                #[cfg(unix)]
                if let Some(pid) = child_id {
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
                return Err(
                    "Provider command timed out. Check the installation before retrying.".into(),
                );
            }
        };
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = if detail.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout)
        } else {
            detail
        };
        return Err(format!(
            "{} exited with {}: {}",
            program.file_name().unwrap_or_default().to_string_lossy(),
            output.status,
            detail.chars().take(1400).collect::<String>().trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}

// Match ownership using the actual resolved install, never just the OS or presence of mise.
fn mise_owner<'a>(data: &'a Value, binary: &Path) -> Option<(&'a str, &'a Value)> {
    data.as_object()?.iter().find_map(|(tool, versions)| {
        versions
            .as_array()?
            .iter()
            .find(|v| {
                v["active"].as_bool() == Some(true)
                    && v["install_path"]
                        .as_str()
                        .is_some_and(|root| binary.starts_with(root))
            })
            .map(|v| (tool.as_str(), v))
    })
}

fn is_provider_tool(provider: ProviderKind, tool: &str) -> bool {
    tool == binary_name(provider)
        || package(provider).is_some_and(|p| tool == format!("npm:{p}"))
        || match provider {
            ProviderKind::Codex => matches!(tool, "github:openai/codex" | "aqua:openai/codex"),
            ProviderKind::Claude => tool == "aqua:anthropics/claude-code",
            ProviderKind::OpenCode => matches!(
                tool,
                "github:anomalyco/opencode" | "aqua:anomalyco/opencode"
            ),
            ProviderKind::Hermes => tool == "github:NousResearch/hermes-agent",
            _ => false,
        }
}

// Omarchy installs shell wrappers, not symlinks. Recognize both its original
// and locked wrappers without executing `mise use` during a passive version check.
fn wrapper_tool(provider: ProviderKind, script: &str) -> Option<String> {
    if !script.starts_with("#!/") || !script.contains("mise use -g") {
        return None;
    }
    script.lines().find_map(|line| {
        // Official wrappers quote these fixed identifiers; no shell evaluation.
        let words: Vec<_> = line
            .split_whitespace()
            .map(|word| word.trim_matches('"'))
            .collect();
        if words.len() < 5 || words[..3] != ["exec", "mise", "x"] || words[4] != "--" {
            return None;
        }
        let tool = &words[3];
        is_provider_tool(provider, tool).then(|| (*tool).to_string())
    })
}
fn read_wrapper(provider: ProviderKind, binary: &Path) -> Option<String> {
    if std::fs::metadata(binary).ok()?.len() > 8192 {
        return None;
    }
    wrapper_tool(provider, &std::fs::read_to_string(binary).ok()?)
}

async fn detect(
    provider: ProviderKind,
    installation: Option<&str>,
) -> Result<Installation, String> {
    if provider == ProviderKind::Acp {
        return Err("Custom ACP executables are managed externally; Codemux does not update them.".into());
    }
    let binary = if provider == ProviderKind::Hermes {
        crate::agent_provider::hermes::profile::resolve_installation(Path::new(
            installation.unwrap_or("hermes"),
        ))?
    } else {
        which::which(binary_name(provider))
            .map_err(|_| "Provider CLI is not installed".to_string())?
    };
    let mut resolved = canonical(&binary);
    let wrapper = read_wrapper(provider, &binary);
    if let Ok(mise) = which::which("mise") {
        if let Some(tool) = &wrapper {
            resolved = PathBuf::from(
                run(
                    &mise,
                    &args(&["which", binary_name(provider), "--tool", tool]),
                    15,
                    true,
                )
                .await?,
            );
        }
        // A shim resolves to mise itself; ask mise for its actual selected executable.
        if binary.parent().is_some_and(|p| p.ends_with("shims")) {
            resolved = PathBuf::from(
                run(&mise, &args(&["which", binary_name(provider)]), 15, true).await?,
            );
        }
        if let Ok(raw) = run(&mise, &args(&["ls", "--json"]), 15, true).await {
            if let Ok(data) = serde_json::from_str::<Value>(&raw) {
                if let Some((tool, entry)) = mise_owner(&data, &resolved)
                    .filter(|(tool, _)| is_provider_tool(provider, tool))
                {
                    let request = entry["requested_version"].as_str().unwrap_or("latest");
                    return Ok(Installation {
                        binary: resolved,
                        manager: if omarchy() { "Omarchy · mise" } else { "mise" }.into(),
                        command: Some((mise, args(&["upgrade", tool]))),
                        mise_tool: Some(tool.into()),
                        mise_request: Some(request.into()),
                    });
                }
            }
        }
    }
    if wrapper.is_some() {
        return Err("Could not resolve this mise wrapper to its active installation".into());
    }
    let (mut manager, recipe) = path_recipe(provider, &resolved);
    let mut command = recipe.and_then(|(program, arguments)| {
        if program == "native" {
            Some((binary.clone(), arguments))
        } else {
            which::which(program).ok().map(|p| (p, arguments))
        }
    });
    // npm on PATH can belong to a different Node installation. Only mutate the
    // prefix that actually owns this CLI, never install a second shadowing copy.
    if manager == "npm" {
        if let Some((npm, _)) = &command {
            let prefix = run(npm, &args(&["prefix", "-g"]), 15, false).await?;
            if !resolved.starts_with(canonical(Path::new(&prefix))) {
                command = None;
            }
        }
    }
    // System package ownership takes precedence over a familiar install path.
    if cfg!(target_os = "linux") {
        if let Ok(pacman) = which::which("pacman") {
            if run(
                &pacman,
                &args(&["-Qqo", &resolved.to_string_lossy()]),
                5,
                false,
            )
            .await
            .is_ok()
            {
                command = None;
                manager = if omarchy() {
                    "Omarchy system package"
                } else {
                    "Arch system package"
                };
            }
        }
    }
    Ok(Installation {
        binary,
        manager: manager.into(),
        command,
        mise_tool: None,
        mise_request: None,
    })
}

fn path_recipe(
    provider: ProviderKind,
    binary: &Path,
) -> (&'static str, Option<(&'static str, Vec<String>)>) {
    let p = binary.to_string_lossy().replace('\\', "/").to_lowercase();
    // Never bypass an unrecognized mise installation with npm/native update.
    if p.contains("/mise/") && !p.contains("/installs/node/") {
        return ("mise (unresolved)", None);
    }
    if (p.contains("/cellar/") || p.contains("/caskroom/")) && !p.contains("/node_modules/") {
        let formula = match provider {
            ProviderKind::Claude => "claude-code",
            ProviderKind::Codex => "codex",
            ProviderKind::OpenCode => "opencode",
            _ => return ("Homebrew", None),
        };
        if !p.contains(&format!("/cellar/{formula}/"))
            && !p.contains(&format!("/caskroom/{formula}/"))
        {
            return ("Homebrew", None);
        }
        return ("Homebrew", Some(("brew", args(&["upgrade", formula]))));
    }
    if let Some(pkg) = package(provider) {
        let target = format!("{pkg}@latest");
        if p.contains("/pnpm/") {
            return ("pnpm", Some(("pnpm", args(&["add", "-g", &target]))));
        }
        if p.contains("/.bun/") {
            return ("Bun", Some(("bun", args(&["add", "-g", &target]))));
        }
        if p.contains(&format!("/lib/node_modules/{pkg}/"))
            || p.contains(&format!("/npm/node_modules/{pkg}/"))
        {
            return (
                "npm",
                Some((
                    "npm",
                    args(&["install", "-g", &format!("--allow-scripts={pkg}"), &target]),
                )),
            );
        }
    }
    match provider {
        ProviderKind::Claude if p.contains("/claude/versions/") => {
            ("Claude updater", Some(("native", args(&["update"]))))
        }
        ProviderKind::Cursor if p.contains("/cursor-agent/versions/") => {
            ("Cursor updater", Some(("native", args(&["update"]))))
        }
        ProviderKind::OpenCode if p.contains("/.opencode/bin/") => {
            ("OpenCode updater", Some(("native", args(&["upgrade"]))))
        }
        // Hermes owns its admission checks (source, package, Docker and Nix) and channel.
        ProviderKind::Hermes => (
            "Hermes updater",
            Some(("native", args(&["update", "--yes"]))),
        ),
        _ => ("External installation", None),
    }
}

fn version(text: &str) -> Option<semver::Version> {
    text.split_whitespace().find_map(|s| {
        semver::Version::parse(
            s.trim_matches(|c: char| c == '(' || c == ')' || c == ',')
                .trim_start_matches('v'),
        )
        .ok()
    })
}
fn newer(current: &str, latest: &str) -> bool {
    matches!((version(current), version(latest)), (Some(a), Some(b)) if b > a && b.pre.is_empty())
}
fn cursor_version(text: &str) -> Option<&str> {
    text.split_whitespace().find(|s| {
        s.is_ascii()
            && s.len() >= 18
            && s.as_bytes().get(4) == Some(&b'.')
            && s.as_bytes().get(7) == Some(&b'.')
            && s.as_bytes().get(10) == Some(&b'-')
            && s[..10].chars().all(|c| c.is_ascii_digit() || c == '.')
    })
}
async fn installed_version(provider: ProviderKind, binary: &Path) -> Result<String, String> {
    if provider == ProviderKind::Claude {
        let sidecar = claude::sidecar_path::resolve_sidecar_path().map_err(|e| e.to_string())?;
        return claude::auth::probe_installed(&sidecar, Some(binary))
            .await
            .map_err(|e| e.to_string())?
            .version
            .ok_or("Claude did not report its version".into());
    }
    run(binary, &args(&["--version"]), 15, false).await
}
async fn fetch(url: &str) -> Result<String, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .header("User-Agent", "Codemux-provider-updates")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())
}

async fn check(provider: ProviderKind, install: &Installation) -> Result<ProviderUpdate, String> {
    let current = installed_version(provider, &install.binary).await?;
    let (latest, available) = if let Some(tool) = &install.mise_tool {
        let selector = format!(
            "{tool}@{}",
            install.mise_request.as_deref().unwrap_or("latest")
        );
        let latest = run(
            &install.command.as_ref().unwrap().0,
            &args(&["latest", &selector]),
            30,
            true,
        )
        .await?;
        let available = if provider == ProviderKind::Cursor {
            cursor_newer(&current, &latest)
        } else {
            newer(&current, &latest)
        };
        (latest, available)
    } else if provider == ProviderKind::Hermes {
        let verdict = run(&install.binary, &args(&["update", "--check"]), 45, false).await?;
        let available = verdict.contains("Update available");
        if !available && !verdict.contains("Already up to date") {
            return Err("Hermes did not report a conclusive update status".into());
        }
        ("Configured channel".into(), available)
    } else if provider == ProviderKind::Cursor {
        // Read the official installer as metadata only; never execute downloaded text.
        let script = fetch("https://cursor.com/install").await?;
        let latest = script
            .split("https://downloads.cursor.com/lab/")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .filter(|s| cursor_version(s).is_some())
            .ok_or("Cursor release metadata was not recognized")?
            .to_string();
        let available = cursor_newer(&current, &latest);
        (latest, available)
    } else {
        let pkg = package(provider).ok_or("No release source for this provider")?;
        let raw = fetch(&format!("https://registry.npmjs.org/{pkg}/latest")).await?;
        let json: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let latest = json["version"]
            .as_str()
            .ok_or("Release did not contain a version")?
            .to_string();
        let available = newer(&current, &latest);
        (latest, available)
    };
    Ok(ProviderUpdate {
        provider,
        installed_version: Some(version(&current).map(|v| v.to_string()).unwrap_or_else(|| {
            current
                .lines()
                .next()
                .unwrap_or(&current)
                .chars()
                .take(100)
                .collect()
        })),
        latest_version: Some(latest),
        available,
        manager: install.manager.clone(),
        can_update: install.command.is_some(),
        message: install.command.is_none().then(|| {
            if install.manager == "Omarchy system package" {
                "This provider is owned by the system. Run `omarchy update` in a terminal, then restart Codemux.".into()
            } else {
                "Update this CLI with the package manager that installed it, then restart Codemux.".into()
            }
        }),
    })
}
fn cursor_newer(current: &str, latest: &str) -> bool {
    matches!((cursor_version(current), cursor_version(latest)), (Some(a), Some(b)) if b[..10] >= a[..10] && b != a)
}

#[tauri::command]
pub async fn agent_chat_provider_update_check(
    provider: ProviderKind,
    installation: Option<String>,
) -> Result<ProviderUpdate, String> {
    let key = (provider, installation.clone());
    let _guard = MAINTENANCE.lock().await;
    if let Some((at, result)) = CACHE.lock().await.get(&key) {
        if at.elapsed() < CHECK_TTL {
            return Ok(result.clone());
        }
    }
    let result = match detect(provider, installation.as_deref()).await {
        Ok(install) => check(provider, &install).await,
        Err(error) => Err(error),
    };
    // Cache failures as unknown, never as "up to date"; avoid repeated offline probes.
    let result = result.unwrap_or_else(|message| ProviderUpdate {
        provider,
        installed_version: None,
        latest_version: None,
        available: false,
        manager: String::new(),
        can_update: false,
        message: Some(message),
    });
    CACHE
        .lock()
        .await
        .insert(key, (Instant::now(), result.clone()));
    Ok(result)
}

#[tauri::command]
pub async fn agent_chat_provider_update(
    provider: ProviderKind,
    installation: Option<String>,
) -> Result<ProviderUpdate, String> {
    let _guard = MAINTENANCE.lock().await;
    CACHE.lock().await.remove(&(provider, installation.clone()));
    let install = detect(provider, installation.as_deref()).await?;
    let after = apply_update(provider, &install).await?;
    CACHE
        .lock()
        .await
        .insert((provider, installation), (Instant::now(), after.clone()));
    Ok(after)
}

async fn apply_update(
    provider: ProviderKind,
    install: &Installation,
) -> Result<ProviderUpdate, String> {
    let (program, arguments) = install
        .command
        .as_ref()
        .ok_or("This installation must be updated with its own package manager")?;
    let before = check(provider, install).await?;
    if !before.available {
        return Err("No eligible update is available for this installation.".into());
    }
    if let Some(tool) = &install.mise_tool {
        run(program, &args(&["cache", "clear", tool]), 30, true).await?;
    }
    run(program, arguments, 900, install.mise_tool.is_some()).await?;
    let mut updated = install.clone();
    if install.mise_tool.is_some() {
        run(program, &args(&["reshim"]), 30, true).await?;
        // The GUI's inherited PATH may still name the previous mise install.
        updated.binary = PathBuf::from(
            run(
                program,
                &args(&[
                    "which",
                    binary_name(provider),
                    "--tool",
                    install.mise_tool.as_deref().unwrap(),
                ]),
                15,
                true,
            )
            .await?,
        );
    }
    let after = check(provider, &updated).await?;
    if after.available
        || (provider != ProviderKind::Hermes && after.installed_version == before.installed_version)
    {
        return Err("The updater finished, but the new version could not be verified. Check your package manager and PATH, then try again.".into());
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_comparison_uses_versions_not_strings() {
        assert!(newer("codex-cli 0.9.0", "0.10.0"));
        assert!(newer("2.1.2 (Claude Code)", "2.1.3"));
        assert!(!newer("2.0.0", "1.9.0"));
        assert!(!newer("unknown", "2.0.0"));
        assert!(!newer("1.0.0", "2.0.0-beta.1"));
        assert!(cursor_newer("2026.09.01-abcdef0", "2026.09.28-1234567"));
        assert!(!cursor_newer("unknown", "2026.09.28-1234567"));
    }
    #[test]
    fn owners_are_matched_to_the_active_install() {
        let data = serde_json::json!({"npm:@xai-official/grok": [{"active":true,"install_path":"/tools/grok/1.0.0","requested_version":"latest"}]});
        assert_eq!(
            mise_owner(&data, Path::new("/tools/grok/1.0.0/bin/grok"))
                .unwrap()
                .0,
            "npm:@xai-official/grok"
        );
        assert!(mise_owner(&data, Path::new("/usr/bin/grok")).is_none());
        assert!(mise_owner(&data, Path::new("/tools/grok/1.0.00/bin/grok")).is_none());
    }
    #[test]
    fn never_guess_npm_or_native_for_an_unknown_install() {
        for provider in [
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::Cursor,
            ProviderKind::Grok,
            ProviderKind::OpenCode,
        ] {
            assert!(path_recipe(provider, Path::new("/usr/bin/tool"))
                .1
                .is_none());
            assert!(path_recipe(
                provider,
                Path::new("/home/me/.local/share/mise/installs/tool/bin/tool")
            )
            .1
            .is_none());
        }
        assert_eq!(
            path_recipe(
                ProviderKind::Codex,
                Path::new("/opt/homebrew/Caskroom/codex/1/bin/codex")
            )
            .0,
            "Homebrew"
        );
        assert_eq!(
            path_recipe(
                ProviderKind::Claude,
                Path::new("/home/me/.local/share/claude/versions/2.0.0")
            )
            .0,
            "Claude updater"
        );
        assert_eq!(
            path_recipe(
                ProviderKind::Codex,
                Path::new("/usr/lib/node_modules/@openai/codex/bin/codex.js")
            )
            .0,
            "npm"
        );
        assert!(path_recipe(
            ProviderKind::Codex,
            Path::new("/project/node_modules/@openai/codex/bin/codex.js")
        )
        .1
        .is_none());
        assert!(path_recipe(
            ProviderKind::Codex,
            Path::new("/opt/homebrew/Cellar/custom-codex/1.0/bin/codex")
        )
        .1
        .is_none());
    }
    #[test]
    fn non_omarchy_installations_use_their_own_updaters() {
        let cases = [
            (
                ProviderKind::Codex,
                "/usr/local/lib/node_modules/@openai/codex/bin/codex.js",
                "npm",
                "npm",
                args(&[
                    "install",
                    "-g",
                    "--allow-scripts=@openai/codex",
                    "@openai/codex@latest",
                ]),
            ),
            (
                ProviderKind::Grok,
                "/home/user/.local/share/pnpm/grok",
                "pnpm",
                "pnpm",
                args(&["add", "-g", "@xai-official/grok@latest"]),
            ),
            (
                ProviderKind::Claude,
                "/home/user/.bun/install/global/node_modules/@anthropic-ai/claude-code/cli.js",
                "Bun",
                "bun",
                args(&["add", "-g", "@anthropic-ai/claude-code@latest"]),
            ),
            (
                ProviderKind::Codex,
                "/opt/homebrew/Caskroom/codex/1.0/codex",
                "Homebrew",
                "brew",
                args(&["upgrade", "codex"]),
            ),
            (
                ProviderKind::Claude,
                "/Users/user/.local/share/claude/versions/2.1.0",
                "Claude updater",
                "native",
                args(&["update"]),
            ),
            (
                ProviderKind::Cursor,
                "/Users/user/.local/share/cursor-agent/versions/2026.09.28-abcdef0/cursor-agent",
                "Cursor updater",
                "native",
                args(&["update"]),
            ),
            (
                ProviderKind::OpenCode,
                "/home/user/.opencode/bin/opencode",
                "OpenCode updater",
                "native",
                args(&["upgrade"]),
            ),
            (
                ProviderKind::Hermes,
                "/home/user/.hermes/hermes-agent/.venv/bin/hermes",
                "Hermes updater",
                "native",
                args(&["update", "--yes"]),
            ),
        ];
        for (provider, path, manager, program, arguments) in cases {
            let (actual_manager, recipe) = path_recipe(provider, Path::new(path));
            assert_eq!(actual_manager, manager, "{path}");
            assert_eq!(recipe, Some((program, arguments)), "{path}");
        }
    }

    #[test]
    fn omarchy_wrappers_do_not_execute_install_on_version_check() {
        let original = "#!/bin/bash\nmise use -g --quiet \"codex\" || exit 1\nexec mise x \"codex\" -- \"codex\" \"$@\"";
        assert_eq!(
            wrapper_tool(ProviderKind::Codex, original).as_deref(),
            Some("codex")
        );
        let locked = "#!/bin/bash\nflock \"$lock\" mise use -g --quiet \"npm:@xai-official/grok\" || exit 1\nexec mise x \"npm:@xai-official/grok\" -- \"$bin_path\" \"$@\"";
        assert_eq!(
            wrapper_tool(ProviderKind::Grok, locked).as_deref(),
            Some("npm:@xai-official/grok")
        );
        assert!(wrapper_tool(ProviderKind::Codex, locked).is_none());
        assert!(!is_provider_tool(ProviderKind::Codex, "node"));
        assert_eq!(path_recipe(ProviderKind::Codex, Path::new("/home/me/.local/share/mise/installs/node/24/lib/node_modules/@openai/codex/bin/codex.js")).0, "npm");
    }

    #[cfg(unix)]
    async fn fake_update(outcome: &str) -> (Result<ProviderUpdate, String>, String) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("fake-mise");
        // All fixture paths are derived from $0, never injected into shell source.
        let script = r#"#!/bin/sh
root=$(dirname "$0")
printf '%s\n' "$*" >> "$root/log"
case "$1" in
  --version) cat "$root/version" ;;
  latest) echo 2.0.0 ;;
  cache) test "$2 $3" = 'clear codex' ;;
  upgrade)
    test "$MISE_UPGRADE_AUTO_PRUNE" = false || exit 10
    test "$MISE_FETCH_REMOTE_VERSIONS_CACHE" = 0 || exit 11
    case "OUTCOME" in
      success) echo 2.0.0 > "$root/version" ;;
      fail) echo 'Registry unavailable' >&2; exit 1 ;;
      noop) : ;;
    esac ;;
  reshim) : ;;
  which) echo "$0" ;;
  *) exit 99 ;;
esac
"#
        .replace("OUTCOME", outcome);
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.path().join("version"), "1.0.0").unwrap();
        let install = Installation {
            binary: program.clone(),
            manager: "mise".into(),
            command: Some((program, args(&["upgrade", "codex"]))),
            mise_tool: Some("codex".into()),
            mise_request: Some("latest".into()),
        };
        let result = apply_update(ProviderKind::Codex, &install).await;
        (
            result,
            std::fs::read_to_string(dir.path().join("log")).unwrap(),
        )
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn provider_update_clears_only_its_cache_and_verifies_the_new_binary() {
        let (result, log) = fake_update("success").await;
        let report = result.unwrap();
        assert_eq!(report.installed_version.as_deref(), Some("2.0.0"));
        assert!(!report.available);
        assert!(log.find("cache clear codex").unwrap() < log.find("upgrade codex").unwrap());
        assert!(log.contains("reshim\nwhich codex --tool codex"));
        assert!(!log.contains("use -g"));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn provider_update_rejects_failed_and_noop_installers() {
        let (failure, _) = fake_update("fail").await;
        assert!(failure.unwrap_err().contains("Registry unavailable"));
        let (noop, _) = fake_update("noop").await;
        assert!(noop.unwrap_err().contains("could not be verified"));
    }
}
