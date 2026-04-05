use crate::config::workspace_config::{
    find_git_root, read_effective_config, read_workspace_config, WorkspaceConfig,
};
use crate::database::DatabaseStore;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Emitter};

/// Default worktree include patterns when no `.codemuxinclude` file or project
/// setting is configured.
pub const DEFAULT_WORKTREE_INCLUDES: &[&str] = &[".env", ".env.*", ".env.local"];

#[derive(Debug, Clone, Serialize)]
struct SetupProgress {
    workspace_id: String,
    command: String,
    index: usize,
    total: usize,
}

#[derive(Debug, Clone, Serialize)]
struct SetupFailed {
    workspace_id: String,
    command: String,
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
struct SetupComplete {
    workspace_id: String,
}

/// Build the common environment variables for setup/teardown/run scripts.
fn script_env(
    workspace_path: &Path,
    root_path: &Path,
    branch: Option<&str>,
    port: Option<u16>,
) -> Vec<(&'static str, String)> {
    let mut vars = vec![
        ("CODEMUX_ROOT_PATH", root_path.to_string_lossy().to_string()),
        (
            "CODEMUX_WORKSPACE_PATH",
            workspace_path.to_string_lossy().to_string(),
        ),
    ];
    if let Some(branch) = branch {
        vars.push(("CODEMUX_BRANCH", branch.to_string()));
    }
    if let Some(port) = port {
        vars.push(("CODEMUX_PORT", port.to_string()));
    }
    vars
}

/// Allocate a stable port for a workspace based on a hash of its ID.
/// Uses a range of 3100–6499 (340 slots of 10 ports each).
/// The port is deterministic for a given workspace ID and will not change
/// if other workspaces are created or deleted.
pub fn allocate_workspace_port(workspace_id: &str) -> u16 {
    const BASE_PORT: u16 = 3100;
    const PORT_STRIDE: u16 = 10;
    const SLOT_COUNT: u16 = 340; // (65535 - 3100) / 10, capped to stay in user range

    // Simple FNV-1a-inspired hash for deterministic port assignment
    let mut hash: u32 = 2166136261;
    for byte in workspace_id.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16777619);
    }

    let slot = (hash % SLOT_COUNT as u32) as u16;
    BASE_PORT + slot * PORT_STRIDE
}

/// Which source provided the worktree include patterns.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IncludeSource {
    File,
    Setting,
    Defaults,
}

#[derive(Debug, Clone, Serialize)]
pub struct IncludeResult {
    pub copied: Vec<String>,
    pub source: IncludeSource,
}

/// Process worktree includes: copy gitignored files matching patterns from the
/// main worktree into the new worktree.
///
/// Pattern source priority:
///   1. `.codemuxinclude` file in project root
///   2. `worktree_includes` from project settings
///   3. Hardcoded defaults (`DEFAULT_WORKTREE_INCLUDES`)
pub fn process_worktree_includes(
    root_path: &Path,
    worktree_path: &Path,
    setting_patterns: &[String],
) -> Result<IncludeResult, String> {
    let include_file = root_path.join(".codemuxinclude");

    // Determine source and write a temp exclude file for non-file sources
    let (exclude_path, source, _tmp) = if include_file.exists() {
        (include_file.clone(), IncludeSource::File, None)
    } else if !setting_patterns.is_empty() {
        let tmp = write_temp_patterns(setting_patterns)?;
        let path = tmp.path().to_path_buf();
        (path, IncludeSource::Setting, Some(tmp))
    } else {
        let defaults: Vec<String> = DEFAULT_WORKTREE_INCLUDES.iter().map(|s| s.to_string()).collect();
        let tmp = write_temp_patterns(&defaults)?;
        let path = tmp.path().to_path_buf();
        (path, IncludeSource::Defaults, Some(tmp))
    };

    eprintln!(
        "[codemux::scripts] worktree includes source={:?} for {}",
        source,
        root_path.display()
    );

    let copied = copy_matching_files(root_path, worktree_path, &exclude_path)?;

    if !copied.is_empty() {
        eprintln!(
            "[codemux::scripts] worktree includes: copied {} file(s) from {} to {}",
            copied.len(),
            root_path.display(),
            worktree_path.display()
        );
    }

    Ok(IncludeResult { copied, source })
}

/// Write patterns to a temporary file for use with `git ls-files --exclude-from`.
fn write_temp_patterns(patterns: &[String]) -> Result<tempfile::NamedTempFile, String> {
    let mut tmp = tempfile::NamedTempFile::new()
        .map_err(|e| format!("Failed to create temp file for include patterns: {e}"))?;
    for pattern in patterns {
        writeln!(tmp, "{}", pattern)
            .map_err(|e| format!("Failed to write include pattern: {e}"))?;
    }
    tmp.flush().map_err(|e| format!("Failed to flush include patterns: {e}"))?;
    Ok(tmp)
}

/// Use `git ls-files` to find gitignored files matching the exclude patterns,
/// then copy them from `root_path` to `worktree_path`.
fn copy_matching_files(
    root_path: &Path,
    worktree_path: &Path,
    exclude_file: &Path,
) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--others",
            "--ignored",
            &format!("--exclude-from={}", exclude_file.display()),
        ])
        .current_dir(root_path)
        .output()
        .map_err(|e| format!("Failed to run git ls-files for worktree includes: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git ls-files failed: {stderr}"));
    }

    let file_list = String::from_utf8_lossy(&output.stdout);
    let mut copied = Vec::new();

    for relative_path in file_list.lines() {
        if relative_path.is_empty() {
            continue;
        }

        let src = root_path.join(relative_path);
        let dst = worktree_path.join(relative_path);

        if !src.exists() || !src.is_file() {
            continue;
        }

        if let Some(parent) = dst.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
            }
        }

        std::fs::copy(&src, &dst)
            .map_err(|e| format!("Failed to copy {} → {}: {e}", src.display(), dst.display()))?;
        copied.push(relative_path.to_string());
    }

    Ok(copied)
}

/// Resolve the git root for a workspace path, with fallback.
pub fn resolve_root_path(workspace_path: &Path) -> PathBuf {
    find_git_root(workspace_path).unwrap_or_else(|| workspace_path.to_path_buf())
}

#[derive(Debug, Clone, Serialize)]
struct WorktreeIncludesApplied {
    workspace_id: String,
    source: IncludeSource,
    copied: Vec<String>,
}

/// Run the full setup pipeline: worktree includes copy + setup scripts.
/// Resolves config from file/DB.
pub fn run_setup_scripts(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    app_handle: &AppHandle,
    db: Option<&DatabaseStore>,
    branch: Option<&str>,
    port: Option<u16>,
) -> Result<(), String> {
    eprintln!(
        "[codemux::scripts] Looking for setup config at workspace_path={}",
        workspace_path.display()
    );

    let root_path = resolve_root_path(workspace_path);

    // Read config to get worktree_includes setting for fallback
    let config = if let Some(db) = db {
        read_effective_config(workspace_path, db)
    } else {
        read_workspace_config(workspace_path)
    };

    let setting_patterns = config
        .as_ref()
        .map(|c| c.worktree_includes.clone())
        .unwrap_or_default();

    // Step 1: Process worktree includes (copy gitignored files from main worktree)
    match process_worktree_includes(&root_path, workspace_path, &setting_patterns) {
        Ok(result) => {
            let _ = app_handle.emit(
                "worktree-includes-applied",
                WorktreeIncludesApplied {
                    workspace_id: workspace_id.to_string(),
                    source: result.source,
                    copied: result.copied,
                },
            );
        }
        Err(e) => {
            eprintln!("[codemux::scripts] worktree includes error: {e}");
        }
    }

    // Step 2: Run setup commands

    let config = match config {
        Some(c) if !c.setup.is_empty() => c,
        _ => {
            eprintln!("[codemux::scripts] No setup config found for workspace {workspace_id}");
            return Ok(());
        }
    };

    run_setup_scripts_with_config(
        workspace_path,
        workspace_name,
        workspace_id,
        app_handle,
        &config,
        &root_path,
        branch,
        port,
    )
}

/// Run setup scripts with a pre-resolved config and root path.
/// The `root_path` should be resolved on the calling thread (not inside a spawned thread)
/// to avoid race conditions with worktree `.git` file resolution.
pub fn run_setup_scripts_with_config(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    app_handle: &AppHandle,
    config: &WorkspaceConfig,
    root_path: &Path,
    branch: Option<&str>,
    port: Option<u16>,
) -> Result<(), String> {
    if config.setup.is_empty() {
        return Ok(());
    }

    eprintln!(
        "[codemux::scripts] Running {} setup command(s) for workspace {workspace_id} (root={})",
        config.setup.len(),
        root_path.display()
    );

    let env_vars = script_env(workspace_path, root_path, branch, port);
    let total = config.setup.len();

    for (index, command) in config.setup.iter().enumerate() {
        let _ = app_handle.emit(
            "workspace-setup-progress",
            SetupProgress {
                workspace_id: workspace_id.to_string(),
                command: command.clone(),
                index,
                total,
            },
        );

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(workspace_path)
            .env("CODEMUX_WORKSPACE_NAME", workspace_name)
            .env("CODEMUX_WORKSPACE_ID", workspace_id);
        for (k, v) in &env_vars {
            cmd.env(k, v);
        }

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run setup command `{command}`: {e}"))?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code();

            let _ = app_handle.emit(
                "workspace-setup-failed",
                SetupFailed {
                    workspace_id: workspace_id.to_string(),
                    command: command.clone(),
                    stdout: stdout.clone(),
                    stderr: stderr.clone(),
                    exit_code,
                },
            );

            return Err(format!(
                "Setup command `{command}` failed (exit {}): {}",
                exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                if stderr.is_empty() { &stdout } else { &stderr }
            ));
        }
    }

    let _ = app_handle.emit(
        "workspace-setup-complete",
        SetupComplete {
            workspace_id: workspace_id.to_string(),
        },
    );

    Ok(())
}

pub fn run_teardown_scripts(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    db: Option<&DatabaseStore>,
) -> Result<(), String> {
    let config = if let Some(db) = db {
        read_effective_config(workspace_path, db)
    } else {
        read_workspace_config(workspace_path)
    };

    let config = match config {
        Some(c) if !c.teardown.is_empty() => c,
        _ => return Ok(()),
    };

    let root_path = resolve_root_path(workspace_path);
    let env_vars = script_env(workspace_path, &root_path, None, None);

    for command in &config.teardown {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(workspace_path)
            .env("CODEMUX_WORKSPACE_NAME", workspace_name)
            .env("CODEMUX_WORKSPACE_ID", workspace_id);
        for (k, v) in &env_vars {
            cmd.env(k, v);
        }

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run teardown command `{command}`: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let exit_code = output.status.code();

            return Err(format!(
                "Teardown command `{command}` failed (exit {}): {}",
                exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                if stderr.is_empty() { &stdout } else { &stderr }
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_env_vars_include_workspace_path() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let output_file = workspace_dir.path().join("ws_path.txt");

        let cmd_str = format!(
            "echo -n \"$CODEMUX_WORKSPACE_PATH\" > {}",
            output_file.display()
        );

        let root_path = workspace_dir.path().to_path_buf();
        let env_vars = script_env(workspace_dir.path(), &root_path, None, None);

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&cmd_str)
            .current_dir(workspace_dir.path());
        for (k, v) in &env_vars {
            cmd.env(k, v);
        }

        let output = cmd.output().expect("Failed to run test command");
        assert!(output.status.success(), "Script failed: {:?}", output);

        let written = fs::read_to_string(&output_file).expect("Output file should exist");
        assert_eq!(
            written,
            workspace_dir.path().to_string_lossy(),
            "CODEMUX_WORKSPACE_PATH should match workspace directory"
        );
    }

    #[test]
    fn test_env_vars_do_not_include_compose_project_name() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let project_root = workspace_dir.path().join("my-cool-project");
        fs::create_dir_all(&project_root).unwrap();

        let env_vars = script_env(workspace_dir.path(), &project_root, None, None);
        assert!(
            env_vars.iter().all(|(k, _)| *k != "COMPOSE_PROJECT_NAME"),
            "COMPOSE_PROJECT_NAME should not be set automatically"
        );
    }

    #[test]
    fn test_env_vars_include_branch_and_port() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let root_path = workspace_dir.path().to_path_buf();

        let env_vars = script_env(
            workspace_dir.path(),
            &root_path,
            Some("feature/login"),
            Some(3110),
        );

        let branch_var = env_vars.iter().find(|(k, _)| *k == "CODEMUX_BRANCH");
        let port_var = env_vars.iter().find(|(k, _)| *k == "CODEMUX_PORT");

        assert_eq!(branch_var.unwrap().1, "feature/login");
        assert_eq!(port_var.unwrap().1, "3110");
    }

    #[test]
    fn test_env_vars_omit_branch_and_port_when_none() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let root_path = workspace_dir.path().to_path_buf();

        let env_vars = script_env(workspace_dir.path(), &root_path, None, None);

        assert!(env_vars.iter().all(|(k, _)| *k != "CODEMUX_BRANCH"));
        assert!(env_vars.iter().all(|(k, _)| *k != "CODEMUX_PORT"));
    }

    #[test]
    fn test_allocate_workspace_port_deterministic() {
        // Same ID always produces the same port
        let port1 = allocate_workspace_port("ws-abc-123");
        let port2 = allocate_workspace_port("ws-abc-123");
        assert_eq!(port1, port2);

        // Port is in valid range
        assert!(port1 >= 3100);
        assert!(port1 < 6500);
        // Port is 10-aligned
        assert_eq!(port1 % 10, 0);

        // Different IDs produce different ports (probabilistically)
        let port_a = allocate_workspace_port("workspace-alpha");
        let port_b = allocate_workspace_port("workspace-beta");
        // Not guaranteed unique, but extremely unlikely to collide for different inputs
        assert_ne!(port_a, port_b);
    }

    #[test]
    fn test_worktree_includes_no_file_uses_defaults() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        // Set up a git repo so git ls-files works
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        // Create .gitignore so default patterns can match
        fs::write(root.path().join(".gitignore"), ".env\n").unwrap();

        // No .codemuxinclude, no setting patterns — should use defaults
        let result = process_worktree_includes(root.path(), worktree.path(), &[]);
        let result = result.unwrap();
        assert_eq!(result.source, IncludeSource::Defaults);
    }

    #[test]
    fn test_worktree_includes_setting_fallback() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n").unwrap();
        fs::write(root.path().join(".env"), "SECRET=abc").unwrap();

        // No .codemuxinclude file, but setting patterns provided
        let patterns = vec![".env".to_string()];
        let result = process_worktree_includes(root.path(), worktree.path(), &patterns).unwrap();

        assert_eq!(result.source, IncludeSource::Setting);
        assert!(result.copied.contains(&".env".to_string()));
        assert_eq!(
            fs::read_to_string(worktree.path().join(".env")).unwrap(),
            "SECRET=abc"
        );
    }

    #[test]
    fn test_worktree_includes_file_takes_priority() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\nconfig/\n").unwrap();

        // Create .codemuxinclude file
        fs::write(root.path().join(".codemuxinclude"), ".env\nconfig/master.key\n").unwrap();

        // Create the gitignored files
        fs::write(root.path().join(".env"), "SECRET=abc").unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(root.path().join("config/master.key"), "key123").unwrap();

        // Even though setting patterns are provided, file should win
        let patterns = vec!["something-else".to_string()];
        let result = process_worktree_includes(root.path(), worktree.path(), &patterns).unwrap();

        assert_eq!(result.source, IncludeSource::File);
        assert!(result.copied.contains(&".env".to_string()));
        assert!(result.copied.contains(&"config/master.key".to_string()));

        assert_eq!(
            fs::read_to_string(worktree.path().join(".env")).unwrap(),
            "SECRET=abc"
        );
        assert_eq!(
            fs::read_to_string(worktree.path().join("config/master.key")).unwrap(),
            "key123"
        );
    }

    #[test]
    fn test_worktree_includes_patterns_match_nothing() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n").unwrap();
        // .codemuxinclude asks for files that don't exist
        fs::write(root.path().join(".codemuxinclude"), "nonexistent.txt\n").unwrap();

        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert_eq!(result.source, IncludeSource::File);
        assert!(result.copied.is_empty());
    }

    #[test]
    fn test_worktree_includes_source_file_deleted() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\nsecrets/\n").unwrap();
        fs::write(root.path().join(".codemuxinclude"), ".env\nsecrets/key.pem\n").unwrap();

        // Create .env but NOT secrets/key.pem — simulates a file that was
        // listed by git ls-files but deleted before the copy step
        fs::write(root.path().join(".env"), "DB_URL=postgres://...").unwrap();

        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert_eq!(result.source, IncludeSource::File);
        // Only .env should be copied; the missing file is silently skipped
        assert_eq!(result.copied, vec![".env"]);
        assert!(worktree.path().join(".env").exists());
        assert!(!worktree.path().join("secrets/key.pem").exists());
    }

    #[test]
    fn test_worktree_includes_empty_patterns_uses_defaults() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n").unwrap();

        // Empty setting patterns should fall through to defaults
        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert_eq!(result.source, IncludeSource::Defaults);
    }

    #[test]
    fn test_worktree_includes_default_patterns_copy_env() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n.env.*\n").unwrap();
        fs::write(root.path().join(".env"), "SECRET=val").unwrap();
        fs::write(root.path().join(".env.local"), "LOCAL=val").unwrap();

        // No file, no settings — defaults should pick up .env and .env.local
        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert_eq!(result.source, IncludeSource::Defaults);
        assert!(result.copied.contains(&".env".to_string()));
        assert!(result.copied.contains(&".env.local".to_string()));
        assert_eq!(
            fs::read_to_string(worktree.path().join(".env")).unwrap(),
            "SECRET=val"
        );
        assert_eq!(
            fs::read_to_string(worktree.path().join(".env.local")).unwrap(),
            "LOCAL=val"
        );
    }
}
