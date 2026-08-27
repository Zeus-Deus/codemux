//! Host facts that ride along in the `codemux-remote workspace list`
//! envelope for the desktop's Devices page.
//!
//! The desktop's inventory poller already runs `workspace list` over
//! SSH once per tick; adding two optional fields to that envelope costs
//! nothing extra on the wire and means a daemon that predates them
//! simply omits them (the desktop treats absence as "unknown").
//!
//! - `disk_bytes` — the sum of every workspace directory in the
//!   daemon's registry, walked in Rust with a time budget. `null` when
//!   the caller asked to skip the walk ([`SKIP_DISK_ENV`], which the
//!   desktop sets on most ticks because this is the expensive part) or
//!   when the budget ran out.
//! - `remote_control_serving` — a Codemux Remote Control server is up on
//!   this host, meaning the user can open it from a browser. Either
//!   signal is enough: the server answers on its default port, or the
//!   user unit `codemux connect` installs is active.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Serialize;

/// Environment variable the desktop sets on the remote command to skip
/// the disk walk (`CODEMUX_SKIP_DISK=1`). An env prefix rather than a
/// flag so daemons that predate it ignore it instead of rejecting the
/// command line.
pub const SKIP_DISK_ENV: &str = "CODEMUX_SKIP_DISK";

/// Ceiling on the workspace-size walk. The desktop adds exactly this
/// much to its inventory SSH budget on the ticks that ask for a walk,
/// so the two must move together.
pub const DISK_WALK_BUDGET: Duration = Duration::from_secs(6);

/// How long to wait for the Remote Control health endpoint. It is a
/// loopback request that either answers instantly or isn't there.
const HEALTH_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HostStatusReport {
    pub disk_bytes: Option<u64>,
    pub remote_control_serving: bool,
}

/// True when the caller asked to skip the disk walk via [`SKIP_DISK_ENV`].
pub fn skip_disk_requested() -> bool {
    std::env::var(SKIP_DISK_ENV).map(|v| v == "1").unwrap_or(false)
}

/// Gather the report for the given workspace directories. Never fails:
/// an over-budget walk reports `disk_bytes: null` and the serving flag
/// is still answered.
pub fn collect(workspace_paths: Vec<PathBuf>, skip_disk: bool) -> HostStatusReport {
    let disk_bytes = if skip_disk {
        None
    } else {
        crate::fs_size::total_size_bounded(
            workspace_paths,
            Some(Instant::now() + DISK_WALK_BUDGET),
        )
    };
    HostStatusReport {
        disk_bytes,
        remote_control_serving: remote_control_serving(),
    }
}

/// Either signal is enough: the server answers on its default port, or
/// the user unit is active (it may be bound to a non-default port, or
/// still starting).
fn remote_control_serving() -> bool {
    health_answers(&format!(
        "http://127.0.0.1:{}/api/health",
        crate::web_remote::DEFAULT_PORT
    )) || unit_active(crate::web_remote::connect::UNIT_NAME)
}

fn health_answers(url: &str) -> bool {
    let client = match reqwest::blocking::Client::builder()
        .timeout(HEALTH_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .get(url)
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn unit_active(unit: &str) -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", unit])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "active")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_serializes_to_the_envelope_field_names() {
        let report = HostStatusReport {
            disk_bytes: None,
            remote_control_serving: false,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(json, r#"{"disk_bytes":null,"remote_control_serving":false}"#);
    }

    #[test]
    fn skip_disk_yields_null_without_walking() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), b"12345").unwrap();
        let report = collect(vec![dir.path().to_path_buf()], true);
        assert_eq!(report.disk_bytes, None);
    }

    #[test]
    fn walk_sums_registered_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), b"12345").unwrap();
        let report = collect(vec![dir.path().to_path_buf()], false);
        assert_eq!(report.disk_bytes, Some(5));
    }

    #[test]
    fn health_probe_against_closed_port_is_false_quickly() {
        // Bind then drop so the port is known-free.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let started = Instant::now();
        assert!(!health_answers(&format!("http://127.0.0.1:{port}/api/health")));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
