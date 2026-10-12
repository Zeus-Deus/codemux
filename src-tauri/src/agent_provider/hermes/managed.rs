//! Managed Hermes uses the official library in a prepared, isolated Python process.
//! Ordinary ACP conversations and personal profile history are never imported.

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use serde_json::json;
use tokio::sync::broadcast;

use crate::agent_provider::{
    managed::{ManagedCapabilities, ManagedSession},
    managed_bridge::ManagedBridgeSession,
    ProviderError, ProviderKind, ProviderRuntimeEvent, StartSessionInput,
};
use crate::json_rpc_child::SpawnConfig;

const SOURCE_ENV: &str = "CODEMUX_HERMES_SOURCE";
const PYTHON_ENV: &str = "CODEMUX_HERMES_PYTHON";
const PROFILE_ENV: &str = "CODEMUX_HERMES_PROFILE_HOME";
const ADAPTER: &str = "hermes-library-v1";
const SIDECAR: &str = include_str!("../../../../sidecar/managed-workflow/hermes.py");

fn required_path(name: &str, directory: bool) -> Result<PathBuf, ProviderError> {
    let path = prepared_path(std::env::var_os(name), directory);
    path.ok_or_else(|| ProviderError::ValidationError {
        message: format!("managed-start-rejected: setup_required: managed Hermes requires a prepared path in {name}; CodeMux will not install or repair Hermes"),
    })
}

fn prepared_path(value: Option<std::ffi::OsString>, directory: bool) -> Option<PathBuf> {
    value
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|value| value.is_absolute())
        .filter(|value| {
            if directory {
                value.is_dir()
            } else {
                value.is_file()
            }
        })
        .and_then(|value| {
            if directory {
                value.canonicalize().ok()
            } else {
                Some(value)
            }
        })
}

pub fn capabilities() -> ManagedCapabilities {
    let ready = cfg!(target_os = "linux")
        && required_path(SOURCE_ENV, true).is_ok_and(|path| path.join("run_agent.py").is_file())
        && required_path(PYTHON_ENV, false).is_ok();
    ManagedCapabilities {
        scoped_tools: ready,
        native_fanout_disabled: ready,
        enforced_read_only: ready,
        isolated_writes: ready,
        verified_stop: ready,
    }
}

pub async fn start(
    input: StartSessionInput,
    context: Arc<ManagedSession>,
    events: broadcast::Sender<ProviderRuntimeEvent>,
) -> Result<Arc<ManagedBridgeSession>, ProviderError> {
    let source = required_path(SOURCE_ENV, true)?;
    let python = required_path(PYTHON_ENV, false)?;
    let profile_home = std::env::var_os(PROFILE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(super::profile::default_root)
        .canonicalize()
        .map_err(|_| ProviderError::ValidationError {
            message:
                "managed-start-rejected: setup_required: managed Hermes profile home is unavailable"
                    .into(),
        })?;
    if !source.join("run_agent.py").is_file() || !profile_home.join("config.yaml").is_file() {
        return Err(ProviderError::ValidationError {
            message: "managed-start-rejected: setup_required: select a prepared Hermes source and profile with routing configuration".into(),
        });
    }
    // The script is embedded in the app; no installer, launcher parsing or
    // staged executable can substitute an unrestricted Hermes runtime.
    let config = SpawnConfig {
        program: python,
        args: vec!["-I".into(), "-c".into(), SIDECAR.into()],
        env: HashMap::from([
            ("PYTHONIOENCODING".into(), "utf-8".into()),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
        ]),
        cwd: Some(input.cwd.clone()),
        default_timeout: Duration::from_secs(60),
    };
    ManagedBridgeSession::spawn(
        input,
        ProviderKind::Hermes,
        ADAPTER,
        context,
        config,
        json!({"source":source,"profileHome":profile_home}),
        events,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermes_managed_requires_explicit_prepared_runtime() {
        let error = required_path("CODEMUX_HERMES_NONEXISTENT_TEST_PATH", true).unwrap_err();
        assert!(
            matches!(error, ProviderError::ValidationError { message } if message.starts_with("managed-start-rejected: setup_required:"))
        );
        assert!(prepared_path(Some("relative-python".into()), false).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn hermes_managed_keeps_venv_interpreter_symlink_for_python_prefix() {
        let root = tempfile::tempdir().unwrap();
        let interpreter = root.path().join("python");
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &interpreter).unwrap();
        assert_eq!(
            prepared_path(Some(interpreter.clone().into_os_string()), false),
            Some(interpreter)
        );
    }
}
