//! Keeps every device's `codemux-remote` at this build's version.
//!
//! Users think "I updated Codemux", not "I should upgrade a helper on each
//! of my devices", so the desktop does it for them, the way t3code re-runs
//! a version-pinned launch on every connection. Three triggers share one
//! path ([`run_upgrade`]):
//!
//! * **App start** ([`spawn`]): one pass over every device ~5 seconds after
//!   setup.
//! * **The inventory poller** ([`auto_upgrade_for`]): every tick that finds
//!   a device reachable with an older helper starts an upgrade in the
//!   background, so a device that was offline at app start catches up as
//!   soon as it is back.
//! * **A thread starting on the device** ([`upgrade_host`]): waits for the
//!   upgrade, since the thread needs the new helper's commands.
//!
//! The upgrade is gentle. The new binary is uploaded beside the old one and
//! swapped in atomically (running processes keep the old file), verified,
//! and the `serve` daemon is restarted only when it has no live sessions.
//! The pty-daemon behind the user's terminals is never killed; it picks up
//! the new binary the next time it starts. A newer helper, installed by a
//! newer desktop sharing the device, is never downgraded. A missing or
//! broken one is never installed: that still takes the user's Connect /
//! Set up again in Settings → Devices.
//!
//! One upload per device at a time ([`claim_device`], shared with the
//! manual install buttons). After a failed attempt the poller waits
//! [`RETRY_AFTER`] before trying that device again; a success forgets the
//! failure.
//!
//! A device that is up to date also gets its Claude runtime refreshed in
//! the background when an older build's runtime is installed there
//! ([`refresh_claude_runtime_in_background`]), so the first Claude thread
//! after a desktop update doesn't wait on a ~100 MB upload.
//!
//! Best-effort throughout: a failure logs and a later trigger tries again.
//! Nothing here fails the app.

#![cfg(unix)]

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Runtime};

use crate::database::{DatabaseStore, HostRecord};
use crate::ssh::bootstrap::{
    bootstrap_remote, provision_serve, BootstrapOptions, BootstrapResult, UPLOAD_DEADLINE,
};
use crate::ssh::probe::{probe_host, ProbeOptions, ProbeOutcome};
use crate::ssh::sidecar::RuntimeRefresh;

/// Budget for one whole upgrade: the upload's own deadline (a ~20 MB binary
/// over a slow uplink takes minutes) plus the probe, verify, session check
/// and daemon restart around it.
const UPGRADE_BUDGET: Duration = Duration::from_secs(UPLOAD_DEADLINE.as_secs() + 3 * 60);

/// How long the poller leaves a device alone after an automatic attempt on
/// it failed, so a link too slow for the upload isn't saturated every tick.
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

/// Spawn the app-start pass. Must be called once during app setup. It
/// waits a few seconds so it doesn't compete with the first paint.
pub fn spawn<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        run_once(app).await;
    });
}

/// Upgrade every registered device whose helper is older than this build.
/// One device at a time, so the uplink carries one upload; a device another
/// trigger is already upgrading is left to it. Public so tests can drive it
/// without the spawn delay.
pub async fn run_once<R: Runtime>(app: AppHandle<R>) {
    let hosts = match app.try_state::<DatabaseStore>() {
        Some(state) => state.list_hosts(),
        None => {
            eprintln!("[hosts_upgrade] database state unavailable; skipping background poll");
            return;
        }
    };
    for host in hosts {
        if let Some(device) = try_claim_device(&host.ssh_target) {
            let _ = run_upgrade(&app, &host, device).await;
        }
    }
}

/// Bring one device's helper up to this build now, waiting for an upgrade
/// already running there instead of starting a second. For flows that need
/// the new helper, such as a thread starting on the device.
pub(crate) async fn upgrade_host<R: Runtime>(
    app: &AppHandle<R>,
    host: &HostRecord,
) -> Result<UpgradeOutcome, String> {
    let device = claim_device(&host.ssh_target).await;
    run_upgrade(app, host, device).await
}

/// What the poller does for a device after probing it.
#[derive(Debug)]
pub(crate) enum AutoUpgrade {
    /// Unreachable, or its helper is missing or broken: nothing automatic.
    Nothing,
    /// The helper is current or newer: check the device's Claude runtime.
    Current,
    /// The helper is older and the device is now claimed for its upgrade.
    Start(DeviceClaim),
    /// The helper is older and an upgrade is already running.
    Running,
    /// The helper is older, but the last attempt failed less than
    /// [`RETRY_AFTER`] ago.
    Waiting,
}

impl AutoUpgrade {
    /// An upgrade is under way, so the device's card can say so.
    pub(crate) fn updating(&self) -> bool {
        matches!(self, Self::Start(_) | Self::Running)
    }

    /// Start the follow-up in the background. Call it after the probe's
    /// observation is recorded, so the fresh status an upgrade ends with
    /// lands after it.
    pub(crate) fn start<R: Runtime>(self, app: &AppHandle<R>, host: &HostRecord) {
        match self {
            Self::Start(device) => {
                let (app, host) = (app.clone(), host.clone());
                tauri::async_runtime::spawn(async move {
                    let _ = run_upgrade(&app, &host, device).await;
                });
            }
            Self::Current => refresh_claude_runtime_in_background(&host.ssh_target),
            Self::Nothing | Self::Running | Self::Waiting => {}
        }
    }
}

/// Decide what to do after a probe of `ssh_target`, claiming the device
/// when its helper should be upgraded now.
pub(crate) fn auto_upgrade_for(outcome: &ProbeOutcome, ssh_target: &str) -> AutoUpgrade {
    match helper_state(outcome, env!("CARGO_PKG_VERSION")) {
        HelperState::Unusable => AutoUpgrade::Nothing,
        HelperState::Current => AutoUpgrade::Current,
        HelperState::Older => {
            let Some(device) = try_claim_device(ssh_target) else {
                return AutoUpgrade::Running;
            };
            if lock(&HELPER_RETRY).due(ssh_target, Instant::now()) {
                AutoUpgrade::Start(device)
            } else {
                AutoUpgrade::Waiting
            }
        }
    }
}

/// Whether a device's helper is older than `ours`. A newer one already has
/// everything this build uses, and replacing it would downgrade it under
/// the newer desktop that installed it. A version that doesn't parse counts
/// as older.
pub(crate) fn is_older(device_version: &str, ours: &str) -> bool {
    match (
        semver::Version::parse(device_version.trim()),
        semver::Version::parse(ours),
    ) {
        (Ok(device), Ok(ours)) => device < ours,
        _ => device_version.trim() != ours,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum HelperState {
    Current,
    Older,
    /// Unreachable, missing or broken.
    Unusable,
}

fn helper_state(outcome: &ProbeOutcome, ours: &str) -> HelperState {
    match outcome {
        ProbeOutcome::Reachable {
            codemux_remote_version: Some(version),
            ..
        } if is_older(version, ours) => HelperState::Older,
        ProbeOutcome::Reachable {
            codemux_remote_version: Some(_),
            ..
        } => HelperState::Current,
        _ => HelperState::Unusable,
    }
}

// ── one upload per device ──────────────────────────────────────────

/// Held while a trigger uploads to a device, so two never upload at once.
/// Keyed by SSH target: two device entries for one machine share it.
pub(crate) type DeviceClaim = tokio::sync::OwnedMutexGuard<()>;

static DEVICE_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

fn device_lock(ssh_target: &str) -> Arc<tokio::sync::Mutex<()>> {
    Arc::clone(
        lock(&DEVICE_LOCKS)
            .entry(ssh_target.to_string())
            .or_default(),
    )
}

/// Claim a device for an upload, or `None` while another trigger has it.
pub(crate) fn try_claim_device(ssh_target: &str) -> Option<DeviceClaim> {
    device_lock(ssh_target).try_lock_owned().ok()
}

/// Wait for the upload running on a device, if any, then claim it.
pub(crate) async fn claim_device(ssh_target: &str) -> DeviceClaim {
    device_lock(ssh_target).lock_owned().await
}

// ── retry after a failure ──────────────────────────────────────────

/// When automatic attempts on each device last failed.
#[derive(Debug, Default)]
struct RetryBackoff {
    failed_at: HashMap<String, Instant>,
}

impl RetryBackoff {
    /// No failure on record, or the last one is [`RETRY_AFTER`] old.
    fn due(&self, ssh_target: &str, now: Instant) -> bool {
        self.failed_at
            .get(ssh_target)
            .is_none_or(|at| now.saturating_duration_since(*at) >= RETRY_AFTER)
    }

    /// A success forgets the failure; a failure restarts the wait.
    fn record(&mut self, ssh_target: &str, succeeded: bool, now: Instant) {
        if succeeded {
            self.failed_at.remove(ssh_target);
        } else {
            self.failed_at.insert(ssh_target.to_string(), now);
        }
    }
}

static HELPER_RETRY: LazyLock<Mutex<RetryBackoff>> = LazyLock::new(Default::default);
static RUNTIME_RETRY: LazyLock<Mutex<RetryBackoff>> = LazyLock::new(Default::default);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// ── the upgrade ────────────────────────────────────────────────────

/// How an upgrade attempt ended, short of an error.
#[derive(Debug)]
pub(crate) enum UpgradeOutcome {
    /// Already this build's version, or newer.
    AlreadyCurrent,
    /// The new helper is on the device. `deferred_restart` counts the live
    /// sessions that kept the `serve` daemon on the old binary; it switches
    /// over when the daemon next restarts.
    Upgraded {
        from: String,
        to: String,
        deferred_restart: Option<u64>,
    },
    /// Nothing was tried: the device was offline, or its helper is missing
    /// or broken, and installing one needs the user's go-ahead.
    NotAttempted { reason: String },
}

/// Run one upgrade with the device claimed, then record how it went: the
/// poller's retry wait, the log, and a fresh status for the device's card.
async fn run_upgrade<R: Runtime>(
    app: &AppHandle<R>,
    host: &HostRecord,
    device: DeviceClaim,
) -> Result<UpgradeOutcome, String> {
    let result = match tokio::time::timeout(
        UPGRADE_BUDGET,
        check_and_upgrade(app, host, env!("CARGO_PKG_VERSION")),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(format!(
            "timed out after {} minutes",
            UPGRADE_BUDGET.as_secs() / 60
        )),
    };
    // A device that was offline or was never set up hasn't failed
    // anything, so it keeps no retry wait. Recorded before the device is
    // released, so a poller tick can't slip in a retry straight after a
    // failure.
    if !matches!(result, Ok(UpgradeOutcome::NotAttempted { .. })) {
        lock(&HELPER_RETRY).record(&host.ssh_target, result.is_ok(), Instant::now());
    }
    drop(device);
    match &result {
        Ok(UpgradeOutcome::AlreadyCurrent) => {}
        Ok(UpgradeOutcome::NotAttempted { reason }) => {
            eprintln!("[hosts_upgrade] {} skipped: {reason}", host.name);
        }
        Ok(UpgradeOutcome::Upgraded {
            from,
            to,
            deferred_restart,
        }) => {
            let deferred = deferred_restart
                .map(|n| format!("; serve restart deferred, {n} live session(s)"))
                .unwrap_or_default();
            eprintln!(
                "[hosts_upgrade] {} upgraded codemux-remote {from} → {to}{deferred}",
                host.name
            );
        }
        Err(e) => eprintln!("[hosts_upgrade] {} failed: {e}", host.name),
    }
    // Probe again so the card drops "Updating" whatever happened: Connected
    // after an upgrade or when another trigger got there first (that probe
    // also refreshes the device's Claude runtime), out of date after a
    // failure, offline if the device went away.
    tauri::async_runtime::spawn(crate::hosts_inventory::refresh_host(app.clone(), host.id));
    result
}

async fn check_and_upgrade<R: Runtime>(
    app: &AppHandle<R>,
    host: &HostRecord,
    ours: &str,
) -> Result<UpgradeOutcome, String> {
    // Probe even when the caller just did: another trigger may have
    // finished the upgrade while this one waited for the device.
    let (from, uname) = match probe_host(ProbeOptions::new(&host.ssh_target)).await {
        ProbeOutcome::Reachable {
            codemux_remote_version: Some(version),
            uname,
            ..
        } => (version, uname),
        ProbeOutcome::Reachable {
            binary_present_but_broken,
            ..
        } => {
            let reason = if binary_present_but_broken {
                "Codemux on the device is damaged"
            } else {
                "Codemux isn't set up on the device"
            };
            return Ok(UpgradeOutcome::NotAttempted {
                reason: reason.into(),
            });
        }
        ProbeOutcome::Unreachable { reason } => {
            return Ok(UpgradeOutcome::NotAttempted {
                reason: format!("couldn't reach it: {reason}"),
            });
        }
    };
    if !is_older(&from, ours) {
        return Ok(UpgradeOutcome::AlreadyCurrent);
    }
    let uname = uname.ok_or("the device didn't report its OS and CPU type")?;

    // Upload beside the old binary, swap it in, verify the version.
    let to = match bootstrap_remote(BootstrapOptions::new(&host.ssh_target, &uname).with_app(app))
        .await
    {
        BootstrapResult::Installed { reported_version } => reported_version,
        BootstrapResult::BinaryNotBundled { wanted_target } => {
            return Err(format!(
                "this Codemux build doesn't include a codemux-remote for {wanted_target}"
            ));
        }
        BootstrapResult::UploadFailed { reason } => return Err(format!("upload: {reason}")),
        BootstrapResult::PostInstallProbeFailed { reason } => {
            return Err(format!("verify: {reason}"));
        }
    };
    // A bundled binary older than the app (a stale dev build) would pass
    // as a success and be uploaded again on every poll.
    if is_older(&to, ours) {
        return Err(format!(
            "this build's codemux-remote reports v{to}, older than v{ours}"
        ));
    }

    // Re-provisioning `serve` keeps its systemd unit in sync AND restarts
    // the daemon onto the new binary, which kills any PTY agents the user
    // runs ON the device (the "work on a device without pulling" flow). So
    // ask the running daemon whether it has live sessions first, and if so
    // leave it be: the new binary is already on disk and takes over when the
    // daemon next restarts. An automatic upgrade must never end the user's
    // work. Terminals of workspaces opened from a desktop live in the
    // separate pty-daemon, which no upgrade touches.
    if let Some(live) = probe_live_host_sessions(&host.ssh_target)
        .await
        .filter(|n| *n > 0)
    {
        return Ok(UpgradeOutcome::Upgraded {
            from,
            to,
            deferred_restart: Some(live),
        });
    }

    // No confirmed live sessions (idle, daemon down, or unknown): restart
    // onto the new binary now.
    if let Err(e) = provision_serve(
        &host.ssh_target,
        "~/.local/bin/codemux-remote",
        Duration::from_secs(30),
    )
    .await
    {
        // The binary IS current on disk; the daemon picks it up when it
        // next restarts.
        eprintln!(
            "[hosts_upgrade] {} provision_serve failed after upgrade (binary is current, \
             but the daemon needs a restart): {e}",
            host.name
        );
    }
    Ok(UpgradeOutcome::Upgraded {
        from,
        to,
        deferred_restart: None,
    })
}

/// Probe the host daemon for its live PTY-session count by running
/// `codemux-remote serve status` over SSH and reading `live_terminals` from
/// its JSON output. Returns:
/// - `Some(n)` — the daemon answered (`n` may be 0).
/// - `None` — couldn't determine (daemon down, ssh error, or an OLD daemon
///   binary whose `serve status` predates the field). Callers treat `None`
///   as "no confirmed live sessions → safe to restart", because a daemon we
///   can't reach has nothing to lose by restarting.
async fn probe_live_host_sessions(ssh_target: &str) -> Option<u64> {
    use std::process::Stdio;
    // Mirror the PATH fallback every other SSH call site uses: non-interactive
    // SSH shells often don't have ~/.local/bin on PATH.
    let cmd_str = "if command -v codemux-remote >/dev/null 2>&1 ; then \
                     codemux-remote serve status ; \
                   elif [ -x \"$HOME/.local/bin/codemux-remote\" ] ; then \
                     \"$HOME/.local/bin/codemux-remote\" serve status ; \
                   fi";
    let output = tokio::time::timeout(Duration::from_secs(12), async {
        tokio::process::Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "StrictHostKeyChecking=accept-new",
                ssh_target,
                cmd_str,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
    })
    .await
    .ok()?
    .ok()?;
    // `serve status` prints one JSON line to stdout (even when the daemon is
    // down, with `live_terminals: null`). Find it and read the field.
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) {
            if let Some(n) = v.get("live_terminals").and_then(|x| x.as_u64()) {
                return Some(n);
            }
        }
    }
    None
}

// ── Claude runtime ─────────────────────────────────────────────────

/// Refresh a device's Claude runtime in the background when an older
/// build's runtime is installed there (see
/// [`crate::ssh::sidecar::refresh_claude_sidecar`]). Cheap enough for every
/// poll: a device already checked this run answers without SSH, and one
/// whose refresh failed waits [`RETRY_AFTER`].
pub(crate) fn refresh_claude_runtime_in_background(ssh_target: &str) {
    if !lock(&RUNTIME_RETRY).due(ssh_target, Instant::now()) {
        return;
    }
    let target = ssh_target.to_string();
    tauri::async_runtime::spawn(async move {
        let result = crate::ssh::sidecar::refresh_claude_sidecar(&target).await;
        lock(&RUNTIME_RETRY).record(&target, result.is_ok(), Instant::now());
        match result {
            Ok(RuntimeRefresh::Refreshed) => {
                eprintln!("[hosts_upgrade] refreshed the Claude runtime on {target}");
            }
            Ok(_) => {}
            Err(e) => eprintln!("[hosts_upgrade] Claude runtime refresh on {target} failed: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reachable(version: Option<&str>) -> ProbeOutcome {
        ProbeOutcome::Reachable {
            codemux_remote_version: version.map(Into::into),
            uname: Some("Linux x86_64".into()),
            binary_present_but_broken: false,
        }
    }

    #[test]
    fn only_an_older_helper_is_upgraded() {
        assert!(!is_older("0.23.1", "0.23.1"));
        assert!(!is_older("0.24.0\n", "0.23.1"), "never downgrade");
        assert!(is_older("0.23.0", "0.23.1"));
        assert!(
            is_older("0.9.9", "0.23.1"),
            "compared as versions, not text"
        );
        assert!(is_older("0.23.1-dev", "0.23.1"), "a pre-release is older");
        assert!(is_older("garbage", "0.23.1"));
    }

    #[test]
    fn helper_state_follows_the_probe() {
        let ours = "0.23.1";
        assert_eq!(
            helper_state(&reachable(Some("0.23.0")), ours),
            HelperState::Older
        );
        assert_eq!(
            helper_state(&reachable(Some("0.23.1")), ours),
            HelperState::Current
        );
        assert_eq!(
            helper_state(&reachable(Some("0.30.0")), ours),
            HelperState::Current
        );
        assert_eq!(
            helper_state(&reachable(None), ours),
            HelperState::Unusable,
            "a missing helper is never installed automatically"
        );
        let broken = ProbeOutcome::Reachable {
            codemux_remote_version: None,
            uname: None,
            binary_present_but_broken: true,
        };
        assert_eq!(helper_state(&broken, ours), HelperState::Unusable);
        let down = ProbeOutcome::Unreachable {
            reason: "timed out".into(),
        };
        assert_eq!(helper_state(&down, ours), HelperState::Unusable);
    }

    #[test]
    fn a_failure_waits_and_a_success_clears_it() {
        let mut backoff = RetryBackoff::default();
        let t0 = Instant::now();
        assert!(backoff.due("u@a", t0), "never tried");

        backoff.record("u@a", false, t0);
        assert!(!backoff.due("u@a", t0));
        assert!(!backoff.due("u@a", t0 + RETRY_AFTER - Duration::from_secs(1)));
        assert!(backoff.due("u@a", t0 + RETRY_AFTER));
        assert!(backoff.due("u@b", t0), "devices wait independently");

        backoff.record("u@a", false, t0 + RETRY_AFTER);
        assert!(
            !backoff.due("u@a", t0 + RETRY_AFTER + Duration::from_secs(60)),
            "another failure restarts the wait"
        );
        backoff.record("u@a", true, t0 + RETRY_AFTER + Duration::from_secs(60));
        assert!(backoff.due("u@a", t0 + RETRY_AFTER + Duration::from_secs(60)));
    }

    #[test]
    fn retry_wait_is_minutes() {
        assert!(RETRY_AFTER >= Duration::from_secs(5 * 60));
    }

    #[test]
    fn upgrade_budget_outlasts_the_upload() {
        assert!(UPGRADE_BUDGET >= UPLOAD_DEADLINE + Duration::from_secs(60));
    }

    #[test]
    fn one_claim_per_device_at_a_time() {
        let first = try_claim_device("claim-test@a").expect("free device");
        assert!(
            try_claim_device("claim-test@a").is_none(),
            "already claimed"
        );
        assert!(
            try_claim_device("claim-test@b").is_some(),
            "other devices are independent"
        );
        drop(first);
        assert!(try_claim_device("claim-test@a").is_some(), "released");
    }

    #[tokio::test]
    async fn claim_device_waits_for_the_running_upload() {
        let held = try_claim_device("wait-test@a").unwrap();
        let waiter = tokio::spawn(async { claim_device("wait-test@a").await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !waiter.is_finished(),
            "must wait while the device is claimed"
        );
        drop(held);
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("claimed once released")
            .unwrap();
    }

    #[test]
    fn the_poller_starts_one_upgrade_and_backs_off_after_a_failure() {
        let target = "auto-test@a";
        let older = reachable(Some("0.0.1"));

        let started = auto_upgrade_for(&older, target);
        assert!(matches!(started, AutoUpgrade::Start(_)));
        assert!(started.updating());
        let again = auto_upgrade_for(&older, target);
        assert!(matches!(again, AutoUpgrade::Running), "no second upload");
        assert!(again.updating());
        drop(started);

        lock(&HELPER_RETRY).record(target, false, Instant::now());
        let waiting = auto_upgrade_for(&older, target);
        assert!(matches!(waiting, AutoUpgrade::Waiting));
        assert!(!waiting.updating());
        assert!(
            try_claim_device(target).is_some(),
            "waiting doesn't keep the device claimed"
        );

        lock(&HELPER_RETRY).record(target, true, Instant::now());
        assert!(matches!(
            auto_upgrade_for(&older, target),
            AutoUpgrade::Start(_)
        ));

        let current = reachable(Some(env!("CARGO_PKG_VERSION")));
        assert!(matches!(
            auto_upgrade_for(&current, target),
            AutoUpgrade::Current
        ));
        assert!(matches!(
            auto_upgrade_for(&reachable(None), target),
            AutoUpgrade::Nothing
        ));
    }
}
