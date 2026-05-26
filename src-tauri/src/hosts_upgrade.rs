//! Background host-upgrade poller.
//!
//! Runs once per app start (~5 seconds after setup, so the UI is
//! responsive first), iterates every registered SSH host, probes
//! its `codemux-remote` version, and if it differs from the version
//! bundled with this Codemux build, silently re-bootstraps —
//! uploads the new binary and restarts the `serve` daemon via the
//! same code path the Install button uses.
//!
//! Why a background task and not a UI prompt:
//!
//! * Users don't think about "upgrading a helper binary on a remote
//!   host." They think "I updated Codemux." This task makes those
//!   two operations equivalent from the user's perspective.
//! * The upgrade is the same path the Install button already runs.
//!   Consent was implicitly granted the first time the user
//!   bootstrapped the host; re-uploading a newer version of the
//!   same binary to the same location is not a meaningful trust
//!   escalation.
//! * `provision_serve` is idempotent, so even if the host is
//!   already up to date the task is cheap (one SSH version probe).
//!
//! Best-effort by design — a host being offline, a flaky tunnel, a
//! missing bundled-binary target — any of these log and move on.
//! The task never fails the app.

#![cfg(unix)]

use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::database::DatabaseStore;
use crate::ssh::bootstrap::{bootstrap_remote, provision_serve, BootstrapOptions, BootstrapResult};
use crate::ssh::probe::{probe_host, ProbeOptions, ProbeOutcome};

/// Spawn the background upgrade poller. Must be called once during
/// app setup. The task starts after a short delay so it doesn't
/// race the UI for resources during the first second of app
/// startup, then walks every host sequentially.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        run_once(app).await;
    });
}

/// Walk every registered host and upgrade those whose
/// `codemux-remote` version differs from our bundled one. Public so
/// tests can drive it directly without the spawn delay.
pub async fn run_once(app: AppHandle) {
    let hosts = match app.try_state::<DatabaseStore>() {
        Some(state) => state.list_hosts(),
        None => {
            eprintln!(
                "[hosts_upgrade] database state unavailable; skipping background poll"
            );
            return;
        }
    };
    if hosts.is_empty() {
        return;
    }
    eprintln!(
        "[hosts_upgrade] polling {} host(s) for upgrade",
        hosts.len()
    );

    let our_version = env!("CARGO_PKG_VERSION");
    for host in hosts {
        // Each host gets a 30s budget — probe + (optional) upload + restart.
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            check_and_upgrade(&app, &host, our_version),
        )
        .await;
        match result {
            Ok(Ok(UpgradeOutcome::AlreadyCurrent)) => {
                // Quiet success — no log needed for "already current."
            }
            Ok(Ok(UpgradeOutcome::Upgraded { from, to })) => {
                eprintln!(
                    "[hosts_upgrade] {} upgraded codemux-remote {from} → {to}",
                    host.name
                );
            }
            Ok(Ok(UpgradeOutcome::Skipped { reason })) => {
                eprintln!(
                    "[hosts_upgrade] {} skipped: {reason}",
                    host.name
                );
            }
            Ok(Err(e)) => {
                eprintln!("[hosts_upgrade] {} failed: {e}", host.name);
            }
            Err(_) => {
                eprintln!(
                    "[hosts_upgrade] {} timed out (30s) — host offline or slow ssh",
                    host.name
                );
            }
        }
    }
}

enum UpgradeOutcome {
    AlreadyCurrent,
    Upgraded { from: String, to: String },
    Skipped { reason: String },
}

async fn check_and_upgrade(
    app: &AppHandle,
    host: &crate::database::HostRecord,
    our_version: &str,
) -> Result<UpgradeOutcome, String> {
    // Step 1: probe.
    let outcome = probe_host(ProbeOptions::new(&host.ssh_target)).await;
    let (current_version, uname) = match outcome {
        ProbeOutcome::Reachable {
            codemux_remote_version: Some(v),
            uname,
        } => (v, uname),
        ProbeOutcome::Reachable {
            codemux_remote_version: None,
            ..
        } => {
            // Host reachable but codemux-remote not installed. Don't
            // install it as part of the background poll — that
            // requires the user's consent (handled by the Install
            // button on Settings → Hosts). The background poll only
            // *upgrades* existing installations.
            return Ok(UpgradeOutcome::Skipped {
                reason: "codemux-remote not installed (background poll won't auto-install — use Settings → Hosts)".into(),
            });
        }
        ProbeOutcome::Unreachable { reason } => {
            return Ok(UpgradeOutcome::Skipped {
                reason: format!("unreachable: {reason}"),
            });
        }
    };

    if current_version == our_version {
        return Ok(UpgradeOutcome::AlreadyCurrent);
    }

    // Step 2: figure out target triple for the binary we'll upload.
    let uname_str = match uname {
        Some(s) => s,
        None => return Err("probe returned no uname".into()),
    };

    // Step 3: re-bootstrap (uploads new binary, verifies version).
    let outcome = bootstrap_remote(
        BootstrapOptions::new(&host.ssh_target, &uname_str).with_app(app),
    )
    .await;
    let new_version = match outcome {
        BootstrapResult::Installed { reported_version } => reported_version,
        BootstrapResult::BinaryNotBundled { wanted_target } => {
            return Err(format!(
                "this Codemux build doesn't include a codemux-remote for {wanted_target}"
            ));
        }
        BootstrapResult::UploadFailed { reason } => {
            return Err(format!("upload: {reason}"));
        }
        BootstrapResult::PostInstallProbeFailed { reason } => {
            return Err(format!("verify: {reason}"));
        }
    };

    // Step 4: re-provision serve so the systemd unit content stays
    // in sync AND the running daemon picks up the new binary.
    // Idempotent + restart-based, so safe to run unconditionally.
    if let Err(e) = provision_serve(
        &host.ssh_target,
        "~/.local/bin/codemux-remote",
        Duration::from_secs(30),
    )
    .await
    {
        // Don't fail the whole upgrade for this — the binary IS
        // newer on disk; the daemon will pick it up on the next
        // host reboot. Log loudly.
        eprintln!(
            "[hosts_upgrade] {} provision_serve failed after upgrade (binary is current, but daemon needs restart): {e}",
            host.name
        );
    }

    Ok(UpgradeOutcome::Upgraded {
        from: current_version,
        to: new_version,
    })
}
