use crate::config::workspace_config::{
    find_git_root, read_effective_config, read_workspace_config, WorkspaceConfig,
};
use crate::database::DatabaseStore;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

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
fn script_env(workspace_path: &Path, root_path: &Path) -> Vec<(&'static str, String)> {
    let compose_name = root_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    vec![
        ("CODEMUX_ROOT_PATH", root_path.to_string_lossy().to_string()),
        (
            "CODEMUX_WORKSPACE_PATH",
            workspace_path.to_string_lossy().to_string(),
        ),
        ("COMPOSE_PROJECT_NAME", compose_name),
    ]
}

/// Resolve the git root for a workspace path, with fallback.
pub fn resolve_root_path(workspace_path: &Path) -> PathBuf {
    find_git_root(workspace_path).unwrap_or_else(|| workspace_path.to_path_buf())
}

/// Run setup scripts, resolving config from file/DB.
pub fn run_setup_scripts(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    app_handle: &AppHandle,
    db: Option<&DatabaseStore>,
) -> Result<(), String> {
    eprintln!(
        "[codemux::scripts] Looking for setup config at workspace_path={}",
        workspace_path.display()
    );

    let config = if let Some(db) = db {
        read_effective_config(workspace_path, db)
    } else {
        read_workspace_config(workspace_path)
    };

    let config = match config {
        Some(c) if !c.setup.is_empty() => c,
        _ => {
            eprintln!("[codemux::scripts] No setup config found for workspace {workspace_id}");
            return Ok(());
        }
    };

    let root_path = resolve_root_path(workspace_path);
    run_setup_scripts_with_config(
        workspace_path,
        workspace_name,
        workspace_id,
        app_handle,
        &config,
        &root_path,
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
) -> Result<(), String> {
    if config.setup.is_empty() {
        return Ok(());
    }

    eprintln!(
        "[codemux::scripts] Running {} setup command(s) for workspace {workspace_id} (root={})",
        config.setup.len(),
        root_path.display()
    );

    let env_vars = script_env(workspace_path, root_path);
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
    let env_vars = script_env(workspace_path, &root_path);

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
        let env_vars = script_env(workspace_dir.path(), &root_path);

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
    fn test_env_vars_include_compose_project_name() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let output_file = workspace_dir.path().join("compose_name.txt");

        let project_root = workspace_dir.path().join("my-cool-project");
        fs::create_dir_all(&project_root).unwrap();

        let cmd_str = format!(
            "echo -n \"$COMPOSE_PROJECT_NAME\" > {}",
            output_file.display()
        );

        let env_vars = script_env(workspace_dir.path(), &project_root);

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
            written, "my-cool-project",
            "COMPOSE_PROJECT_NAME should equal the project folder name"
        );
    }
}
