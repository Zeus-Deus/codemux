use crate::config::workspace_config::{find_git_root, read_workspace_config};
use serde::Serialize;
use std::path::Path;
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

pub fn run_setup_scripts(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    app_handle: &AppHandle,
) -> Result<(), String> {
    eprintln!(
        "[codemux::scripts] Looking for setup config at workspace_path={}",
        workspace_path.display()
    );
    let config = match read_workspace_config(workspace_path) {
        Some(c) if !c.setup.is_empty() => {
            eprintln!(
                "[codemux::scripts] Found {} setup command(s) for workspace {workspace_id}",
                c.setup.len()
            );
            c
        }
        _ => {
            eprintln!("[codemux::scripts] No setup config found for workspace {workspace_id}");
            return Ok(());
        }
    };

    let root_path = find_git_root(workspace_path)
        .unwrap_or_else(|| workspace_path.to_path_buf());
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

        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(workspace_path)
            .env("CODEMUX_ROOT_PATH", root_path.to_string_lossy().as_ref())
            .env("CODEMUX_WORKSPACE_NAME", workspace_name)
            .env("CODEMUX_WORKSPACE_ID", workspace_id)
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
                exit_code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
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
) -> Result<(), String> {
    let config = match read_workspace_config(workspace_path) {
        Some(c) if !c.teardown.is_empty() => c,
        _ => return Ok(()),
    };

    let root_path = find_git_root(workspace_path)
        .unwrap_or_else(|| workspace_path.to_path_buf());

    for command in &config.teardown {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(workspace_path)
            .env("CODEMUX_ROOT_PATH", root_path.to_string_lossy().as_ref())
            .env("CODEMUX_WORKSPACE_NAME", workspace_name)
            .env("CODEMUX_WORKSPACE_ID", workspace_id)
            .output()
            .map_err(|e| format!("Failed to run teardown command `{command}`: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let exit_code = output.status.code();

            return Err(format!(
                "Teardown command `{command}` failed (exit {}): {}",
                exit_code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
                if stderr.is_empty() { &stdout } else { &stderr }
            ));
        }
    }

    Ok(())
}
