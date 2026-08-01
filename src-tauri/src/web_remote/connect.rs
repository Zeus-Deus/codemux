//! `codemux connect` — the one-command remote-access bootstrap.
//!
//! The flagship path for a fresh headless box: one command takes a machine
//! from "nothing configured" to "reachable from anywhere, and still reachable
//! after the SSH session closes". It is deliberately a *composition* of paths
//! that already exist rather than a new mechanism:
//!
//! | step | what it does | reuses |
//! |------|--------------|--------|
//! | 1 | sign in (or report the existing session) | [`crate::auth::cli_login::ensure_signed_in`] |
//! | 2 | persist `enabled` + `relay_mode_enabled` | [`super::update_config_headless`] / the `web_remote_enable` + `web_remote_set_relay` control commands |
//! | 3 | keep it running across logout/reboot | a systemd **user** unit running `codemux serve` |
//!
//! ## Why step 3 branches on a running instance
//!
//! `codemux serve` refuses to start when a GUI or another `serve` already
//! holds this machine's control endpoint (they would fight over the DB, the
//! PTY daemon, and the web-remote port). Installing a unit next to a running
//! desktop app would therefore produce a service that fails on every start.
//! So when something IS running we drive it over the control socket instead —
//! and because the running instance persists `enabled` / `relay_mode_enabled`,
//! `restore_on_boot` brings the same state back on its next launch.
//!
//! ## Why the config write is split the same way
//!
//! A running instance owns the config in memory and re-persists it on every
//! change; a direct settings-row write would be silently clobbered by the next
//! toggle. With nothing running there is no such owner, so the headless
//! load-modify-save in [`super::update_config_headless`] is the only way to
//! configure a box *before* anything is up. Each branch uses exactly one of
//! the two, never both.
//!
//! ## Relay is config-driven, not flag-driven
//!
//! The installed unit runs a bare `codemux serve` — no `--relay`. That is
//! correct rather than incidental: `serve` → `control_enable` → `enable_core`
//! starts the iroh endpoint and device registration whenever the *persisted*
//! `relay_mode_enabled` is set, and `restore_on_boot` does the same for the
//! GUI. Step 2 persists that flag, so the unit stays generic and a later
//! `codemux connect off` (or the Settings switch) changes behaviour without
//! rewriting the unit file.
//!
//! Every external command goes through the [`ServiceHost`] seam so the
//! install/rollback sequences are unit-tested against scripted failures
//! instead of a real service manager.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use super::{WebRemoteConfig, DEFAULT_PORT};
use crate::auth::cli_login;
use crate::control::{send_control_request, ControlRequest};

/// Where a browser signed into the same account manages this device. Printed
/// as the closing line of a successful `codemux connect`.
pub const MANAGE_URL: &str = "https://app.codemux.org";

/// The systemd **user** unit `codemux connect` installs.
///
/// Deliberately distinct from the two units in `automations/service.rs`
/// (`codemux-remote.service`, `codemux-scheduler.service`): those run the slim
/// `codemux-remote` binary provisioned on a *managed host* by the SSH
/// bootstrap. This one runs the full Codemux binary on the machine the user is
/// sitting on (or SSH'd into), so it takes the product's own name.
pub const UNIT_NAME: &str = "codemux.service";

/// How many times `systemctl --user is-active` is polled before the install is
/// treated as failed. `serve` boots the whole backend (DB, PTY warmup, bind),
/// so "not active yet" on the first poll is normal, not a failure.
const ACTIVE_POLL_ATTEMPTS: usize = 10;
/// Delay between `is-active` polls.
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(500);

// ── Unit file ────────────────────────────────────────────────────────

/// Render the systemd user unit.
///
/// Follows the shape of the `serve` unit in `automations/service.rs`, with
/// the same rationale:
///
/// * `Wants`/`After=network-online.target` — relay mode registers with the
///   account control plane and dials an iroh relay; both need the network up.
/// * `Restart=on-failure` (not `always`) — `serve` exits 0 on SIGTERM, and a
///   clean stop must not be restarted into a loop.
/// * `RestartSec=5s` — a remote-access daemon that is down is invisible to the
///   user, so retry quickly.
/// * `StandardOutput=null` — `serve`'s stdout carries a one-time pairing token
///   and QR. Sending that to a durable journal would leave a credential in the
///   logs; `StandardError=journal` keeps the diagnostics that matter, tailable
///   with `journalctl --user -u codemux -f`.
///
/// `api_url` mirrors the caller's `CODEMUX_API_URL` when it is set. A user
/// pointed at a self-hosted API would otherwise get a service that silently
/// talks to the default host, because a systemd unit inherits nothing from the
/// shell that installed it.
pub fn systemd_unit(exec_path: &str, api_url: Option<&str>) -> String {
    let environment = match api_url {
        Some(url) if !url.trim().is_empty() => format!("Environment=CODEMUX_API_URL={url}\n"),
        _ => String::new(),
    };
    format!(
        "[Unit]\n\
         Description=Codemux remote access (headless web-remote server)\n\
         Documentation=https://codemux.org\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec_path} serve\n\
         {environment}\
         Restart=on-failure\n\
         RestartSec=5s\n\
         StandardOutput=null\n\
         StandardError=journal\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

// ── What the unit should exec ────────────────────────────────────────

/// Decide the command a service unit should run, given how this process was
/// started.
///
/// The subtlety is the AppImage-extract install shape: `codemux` on `PATH` is
/// a small wrapper that execs the payload's `AppRun`, and `AppRun` is what
/// exports the GTK/GIO/pixbuf environment the bundled libraries need before
/// exec'ing the real binary. `current_exe()` inside that process points at the
/// *inner* binary, so putting it straight into `ExecStart` would produce a
/// service that starts without any of that environment. Native packages
/// (`.deb`/`.rpm`/AUR) install a plain binary and set no `APPDIR`, so they
/// take the first branch and nothing changes for them.
///
/// Pure — `appdir` and `on_path` are the two facts the caller looks up — so
/// the branch that only exists on one install shape is still testable.
fn service_exec_path(
    current_exe: &std::path::Path,
    appdir: Option<&str>,
    on_path: Option<PathBuf>,
) -> PathBuf {
    let appdir = match appdir.map(str::trim).filter(|d| !d.is_empty()) {
        // Not launched through AppRun: the running binary is the whole story.
        None => return current_exe.to_path_buf(),
        Some(dir) => dir,
    };
    // Prefer the wrapper the user's own shell resolves — it is the supported
    // entry point and survives an in-place upgrade of the payload.
    match on_path {
        Some(path) if path != current_exe => path,
        // No wrapper on PATH (someone ran the inner binary directly, or PATH
        // is unusual): exec AppRun itself, which is the next best thing.
        _ => PathBuf::from(appdir).join("AppRun"),
    }
}

/// First executable named `name` on `PATH`. Hand-rolled rather than shelling
/// out to `which`/`command -v` so it works identically in the minimal
/// environments this command targets.
///
/// The name is matched literally — no Windows `PATHEXT` expansion — because the
/// only caller is [`resolve_exec_path`], which runs on the systemd install path
/// and is therefore Linux-only (see [`default_host`]). A Windows caller would
/// have to spell the extension out.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return meta.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    true
}

/// The absolute command line the installed unit should run.
fn resolve_exec_path() -> Result<PathBuf, String> {
    let current = std::env::current_exe()
        .map_err(|e| format!("could not resolve the codemux binary path: {e}"))?;
    Ok(service_exec_path(
        &current,
        std::env::var("APPDIR").ok().as_deref(),
        find_on_path("codemux"),
    ))
}

// ── Service-manager seam ─────────────────────────────────────────────

/// The outcome of one external command.
#[derive(Debug, Clone)]
pub struct CmdOutcome {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutcome {
    /// The most useful line to show a user: stderr if the tool wrote one,
    /// otherwise stdout, otherwise a generic note.
    fn message(&self) -> String {
        let stderr = self.stderr.trim();
        if !stderr.is_empty() {
            return stderr.to_string();
        }
        let stdout = self.stdout.trim();
        if !stdout.is_empty() {
            return stdout.to_string();
        }
        "no output".to_string()
    }
}

/// Everything the install/uninstall sequences do to the outside world. Behind
/// a trait so the sequences — including every rollback branch — are testable
/// against scripted failures without a service manager, a real unit file, or
/// a half-second-per-poll wall clock.
pub trait ServiceHost {
    /// Absolute path of the unit file this host installs to.
    fn unit_path(&self) -> PathBuf;
    fn unit_exists(&self) -> bool;
    fn write_unit(&self, contents: &str) -> Result<(), String>;
    /// Remove the unit file. Missing file is success (idempotent teardown).
    fn remove_unit(&self) -> Result<(), String>;
    /// Run a command to completion. `Err` means the binary could not be
    /// executed at all; a non-zero exit is `Ok` with `success: false`.
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutcome, String>;
    fn sleep(&self, duration: Duration);
}

/// The real host: `~/.config/systemd/user/<unit>` plus `std::process::Command`.
pub struct SystemdUserHost {
    unit_path: PathBuf,
}

impl SystemdUserHost {
    /// `None` when the platform has no home/config directory to install into.
    pub fn new() -> Option<Self> {
        let dir = dirs::config_dir()?.join("systemd").join("user");
        Some(Self {
            unit_path: dir.join(UNIT_NAME),
        })
    }
}

impl ServiceHost for SystemdUserHost {
    fn unit_path(&self) -> PathBuf {
        self.unit_path.clone()
    }

    fn unit_exists(&self) -> bool {
        self.unit_path.exists()
    }

    fn write_unit(&self, contents: &str) -> Result<(), String> {
        if let Some(parent) = self.unit_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        std::fs::write(&self.unit_path, contents)
            .map_err(|e| format!("could not write {}: {e}", self.unit_path.display()))
    }

    fn remove_unit(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.unit_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!(
                "could not remove {}: {e}",
                self.unit_path.display()
            )),
        }
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutcome, String> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|e| format!("could not run `{program}`: {e}"))?;
        Ok(CmdOutcome {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Is a per-user systemd instance actually usable here? `systemctl --user`
/// exists on any systemd install but fails without a user bus — the normal
/// state inside containers and on hosts where the user has no session. Probing
/// `show-environment` (a read-only call that needs the bus) separates the two,
/// so the caller can print the manual fallback instead of a systemd error.
pub fn systemd_user_available(host: &dyn ServiceHost) -> bool {
    matches!(
        host.run("systemctl", &["--user", "show-environment"]),
        Ok(outcome) if outcome.success
    )
}

/// Whether `loginctl enable-linger` succeeded. Never fatal: linger is what
/// keeps the service alive after logout, but plenty of hosts (containers,
/// some managed environments) refuse it, and the service still runs for the
/// duration of a session there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LingerOutcome {
    Enabled,
    Refused(String),
}

/// What [`install_service`] achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub unit_path: String,
    pub linger: LingerOutcome,
}

/// Install, enable, and start the unit — rolling every partial state back on
/// failure so a failed `codemux connect` never leaves a broken service behind.
///
/// Order matters: the unit file is written first (nothing else can succeed
/// without it), then `daemon-reload` so systemd sees it, then `enable --now`
/// (persist across reboot + start now), then the activity check, and only then
/// the best-effort linger call. Any failure from `daemon-reload` onward tears
/// the whole thing down via [`rollback_service`].
pub fn install_service(host: &dyn ServiceHost, unit_contents: &str) -> Result<InstallReport, String> {
    host.write_unit(unit_contents)?;

    let reload = host.run("systemctl", &["--user", "daemon-reload"])?;
    if !reload.success {
        // Nothing has been enabled yet, but drop the file we just wrote so the
        // next attempt starts from a clean slate.
        let _ = host.remove_unit();
        return Err(format!(
            "`systemctl --user daemon-reload` failed: {}",
            reload.message()
        ));
    }

    let enable = host.run("systemctl", &["--user", "enable", "--now", UNIT_NAME])?;
    if !enable.success {
        rollback_service(host);
        return Err(format!(
            "`systemctl --user enable --now {UNIT_NAME}` failed: {}",
            enable.message()
        ));
    }

    if !wait_until_active(host) {
        // Enabled and started, but it never reached `active` — usually `serve`
        // exiting immediately because another instance holds the control
        // endpoint, or a port clash. Tear it down rather than leave a unit that
        // restart-loops, and point at the journal for the real reason.
        rollback_service(host);
        return Err(format!(
            "{UNIT_NAME} was installed but never became active — it has been removed again. \
             Run `journalctl --user -u {stem} -n 50` for the reason (a Codemux instance \
             already running on this machine is the usual cause).",
            stem = unit_stem()
        ));
    }

    Ok(InstallReport {
        unit_path: host.unit_path().display().to_string(),
        linger: enable_linger(host),
    })
}

/// Poll `is-active` until the service is up or the attempts run out. `serve`
/// boots the full backend, so the first poll is expected to miss.
fn wait_until_active(host: &dyn ServiceHost) -> bool {
    for attempt in 0..ACTIVE_POLL_ATTEMPTS {
        if let Ok(outcome) = host.run("systemctl", &["--user", "is-active", UNIT_NAME]) {
            if outcome.stdout.trim() == "active" {
                return true;
            }
            // `failed` is terminal — restarts are systemd's job from here, and
            // waiting out the remaining polls only delays the error.
            if outcome.stdout.trim() == "failed" {
                return false;
            }
        }
        if attempt + 1 < ACTIVE_POLL_ATTEMPTS {
            host.sleep(ACTIVE_POLL_INTERVAL);
        }
    }
    false
}

/// Ask logind to keep this user's services running with no login session —
/// the difference between "reachable while I'm SSH'd in" and "reachable after
/// I log out", which is the whole point of the command.
fn enable_linger(host: &dyn ServiceHost) -> LingerOutcome {
    let user = std::env::var("USER").unwrap_or_default();
    let args: Vec<&str> = if user.is_empty() {
        vec!["enable-linger"]
    } else {
        vec!["enable-linger", user.as_str()]
    };
    match host.run("loginctl", &args) {
        Ok(outcome) if outcome.success => LingerOutcome::Enabled,
        Ok(outcome) => LingerOutcome::Refused(outcome.message()),
        Err(e) => LingerOutcome::Refused(e),
    }
}

/// Undo a partial install. Best-effort by construction: each step is
/// independently skippable, and the caller already has the error to report.
fn rollback_service(host: &dyn ServiceHost) {
    let _ = host.run("systemctl", &["--user", "disable", "--now", UNIT_NAME]);
    let _ = host.remove_unit();
    let _ = host.run("systemctl", &["--user", "daemon-reload"]);
}

/// Stop, disable, and remove the unit. `Ok(false)` when there was nothing
/// installed, so the caller can say so instead of claiming it removed
/// something.
pub fn uninstall_service(host: &dyn ServiceHost) -> Result<bool, String> {
    if !host.unit_exists() {
        return Ok(false);
    }
    // `disable --now` both stops and un-links; run it before deleting the file
    // because systemd needs the unit to still exist to act on it.
    let disable = host.run("systemctl", &["--user", "disable", "--now", UNIT_NAME])?;
    host.remove_unit()?;
    let _ = host.run("systemctl", &["--user", "daemon-reload"]);
    if !disable.success {
        // The file is gone either way — report the detail without failing, so
        // `codemux connect off` is idempotent on a half-broken install.
        eprintln!(
            "[codemux connect] `systemctl --user disable --now {UNIT_NAME}` reported: {}",
            disable.message()
        );
    }
    Ok(true)
}

/// The unit name without its `.service` suffix — what `systemctl`/`journalctl`
/// print in hints.
fn unit_stem() -> &'static str {
    UNIT_NAME.strip_suffix(".service").unwrap_or(UNIT_NAME)
}

/// Live service-manager state, for `codemux connect status`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceState {
    pub installed: bool,
    pub active: bool,
    pub enabled: bool,
    /// `None` when logind could not be asked (no linger support here).
    pub lingering: Option<bool>,
}

pub fn service_state(host: &dyn ServiceHost) -> ServiceState {
    let installed = host.unit_exists();
    if !installed {
        return ServiceState::default();
    }
    let active = matches!(
        host.run("systemctl", &["--user", "is-active", UNIT_NAME]),
        Ok(o) if o.stdout.trim() == "active"
    );
    let enabled = matches!(
        host.run("systemctl", &["--user", "is-enabled", UNIT_NAME]),
        Ok(o) if o.stdout.trim() == "enabled"
    );
    let user = std::env::var("USER").unwrap_or_default();
    let lingering = if user.is_empty() {
        None
    } else {
        match host.run(
            "loginctl",
            &["show-user", user.as_str(), "--property=Linger", "--value"],
        ) {
            Ok(o) if o.success => Some(o.stdout.trim().eq_ignore_ascii_case("yes")),
            _ => None,
        }
    };
    ServiceState {
        installed,
        active,
        enabled,
        lingering,
    }
}

// ── Config mutation ──────────────────────────────────────────────────

/// Fold a `codemux connect` request into the persisted config: remote access
/// on, relay mode on, plus any explicit scope/port.
///
/// Rejects an unknown scope before touching anything — the caller persists
/// only on `Ok`, so a typo can never leave a config the bind logic refuses to
/// honour. Scope/port are left alone when not passed, which is what makes a
/// re-run of `codemux connect` non-destructive to a tuned setup.
pub fn apply_connect_request(
    cfg: &mut WebRemoteConfig,
    scope: Option<String>,
    port: Option<u16>,
) -> Result<(), String> {
    super::apply_enable_request(cfg, scope, port)?;
    cfg.relay_mode_enabled = true;
    Ok(())
}

/// Turn the from-anywhere transport off while leaving the local server's
/// configuration (scope, port, and whether it binds at all) untouched. Sign-in
/// state is likewise untouched — `codemux connect off` is not a sign-out.
pub fn apply_disconnect_request(cfg: &mut WebRemoteConfig) {
    cfg.relay_mode_enabled = false;
}

// ── Reports (pure formatting) ────────────────────────────────────────

/// How step 3 was satisfied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceReport {
    /// A background unit was installed and is running.
    Installed(InstallReport),
    /// A GUI / `serve` instance was already running and was driven over the
    /// control socket instead.
    RunningInstance,
    /// No supported service manager here; steps 1–2 still succeeded.
    Unsupported { reason: String, fallback: String },
}

/// Everything a successful `codemux connect` should tell the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectReport {
    pub identity: String,
    pub scope: String,
    pub port: u16,
    pub service: ServiceReport,
}

/// Render the report. Pure so the exact copy is pinned by tests instead of
/// drifting every time a branch is touched.
pub fn format_connect_report(report: &ConnectReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("✓ Signed in as {}\n", report.identity));
    out.push_str(&format!(
        "✓ Remote access configured (relay mode on, scope {}, port {})\n",
        report.scope, report.port
    ));
    match &report.service {
        ServiceReport::Installed(install) => {
            out.push_str(
                "✓ Background service ready — Codemux stays reachable after you log out\n",
            );
            if let LingerOutcome::Refused(detail) = &install.linger {
                out.push_str(&format!(
                    "⚠ Could not keep the service alive past logout ({detail}).\n  \
                     It runs while you are logged in; ask an administrator to enable \
                     lingering for this user to make it permanent.\n"
                ));
            }
            out.push_str(&format!("  Service: {}\n", install.unit_path));
        }
        ServiceReport::RunningInstance => {
            out.push_str(
                "✓ Handled by the Codemux instance already running on this machine\n  \
                 The setting is persisted, so it comes back the next time that instance starts.\n",
            );
        }
        ServiceReport::Unsupported { reason, fallback } => {
            out.push_str(&format!("⚠ No background service was installed: {reason}\n"));
            out.push_str(&format!("  {fallback}\n"));
        }
    }
    out.push_str(&format!("  Manage this device from {MANAGE_URL}\n"));
    out
}

/// Everything `codemux connect status` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectStatus {
    /// `None` when signed out.
    pub identity: Option<String>,
    pub enabled: bool,
    pub relay_mode_enabled: bool,
    pub scope: String,
    pub port: u16,
    pub service: ServiceState,
    pub service_supported: bool,
    pub instance_running: bool,
    pub device_name: String,
    pub device_id: Option<String>,
}

/// Human-readable gloss for a bind scope, matching the CLI's existing copy.
fn scope_note(scope: &str) -> &'static str {
    match scope {
        super::BIND_SCOPE_TAILSCALE => "tailnet + loopback only",
        super::BIND_SCOPE_LOOPBACK => "this machine only",
        _ => "every interface",
    }
}

pub fn format_connect_status(status: &ConnectStatus) -> String {
    let mut out = String::new();

    out.push_str("Account\n");
    match &status.identity {
        Some(identity) => out.push_str(&format!("  Signed in as {identity}\n")),
        None => out.push_str("  Not signed in — run `codemux connect` to sign in.\n"),
    }

    out.push_str("\nRemote access\n");
    out.push_str(&format!(
        "  Enabled:      {}\n",
        if status.enabled { "yes" } else { "no" }
    ));
    out.push_str(&format!(
        "  Relay mode:   {}\n",
        if status.relay_mode_enabled { "on" } else { "off" }
    ));
    out.push_str(&format!(
        "  Access scope: {} ({})\n",
        status.scope,
        scope_note(&status.scope)
    ));
    out.push_str(&format!("  Port:         {}\n", status.port));

    out.push_str("\nBackground service\n");
    if !status.service_supported {
        out.push_str(&format!(
            "  {UNIT_NAME}: unsupported here (no per-user systemd)\n"
        ));
    } else if !status.service.installed {
        out.push_str(&format!("  {UNIT_NAME}: not installed\n"));
    } else {
        out.push_str(&format!(
            "  {UNIT_NAME}: installed, {}, {} at boot\n",
            if status.service.active {
                "active"
            } else {
                "not running"
            },
            if status.service.enabled {
                "starts"
            } else {
                "does not start"
            }
        ));
        match status.service.lingering {
            Some(true) => out.push_str("  Survives logout: yes\n"),
            Some(false) => out.push_str(
                "  Survives logout: no — lingering is off for this user.\n",
            ),
            None => {}
        }
    }

    out.push_str("\nThis machine\n");
    out.push_str(&format!(
        "  Codemux running: {}\n",
        if status.instance_running {
            "yes (a GUI or serve instance holds the control endpoint)"
        } else {
            "no"
        }
    ));
    out.push_str(&format!("  Device name:     {}\n", status.device_name));
    if let Some(id) = &status.device_id {
        out.push_str(&format!("  Device id:       {id}\n"));
    }
    out
}

/// What `codemux connect off` actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisconnectReport {
    pub service_removed: bool,
    pub service_supported: bool,
    pub relay_turned_off: bool,
    pub instance_running: bool,
    /// `None` when signed out.
    pub identity: Option<String>,
}

pub fn format_disconnect_report(report: &DisconnectReport) -> String {
    let mut out = String::new();
    if report.service_removed {
        out.push_str(&format!(
            "✓ Background service stopped and removed ({UNIT_NAME})\n"
        ));
    } else if report.service_supported {
        out.push_str("• No background service was installed — nothing to stop\n");
    }
    if report.relay_turned_off {
        out.push_str(
            "✓ From-anywhere access is off — this device is no longer reachable from other networks\n",
        );
    } else {
        out.push_str("• From-anywhere access was already off\n");
    }
    if report.instance_running {
        out.push_str(
            "  The Codemux instance running on this machine applied the change immediately.\n",
        );
    }
    match &report.identity {
        Some(identity) => out.push_str(&format!(
            "  Still signed in as {identity} — run `codemux logout` to clear the session.\n"
        )),
        None => {}
    }
    out
}

// ── Command entry points ─────────────────────────────────────────────

/// Options parsed from `codemux connect [--email …] [--scope …] [--port N]`.
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    pub email: Option<String>,
    pub scope: Option<String>,
    pub port: Option<u16>,
}

/// The manual fallback text for a machine with no per-user systemd. Steps 1–2
/// have already succeeded at this point, so the message is about keeping the
/// process alive, not about remote access being unavailable.
fn manual_fallback() -> String {
    if cfg!(target_os = "linux") {
        "Start it yourself with `codemux serve` (inside tmux/screen, or from your \
         own init system) — the relay configuration above is already persisted."
            .to_string()
    } else {
        "Start it yourself with `codemux serve` (inside tmux/screen) — the relay \
         configuration above is already persisted. Managed background service \
         support on this platform is still to come."
            .to_string()
    }
}

/// Why no unit could be installed on this machine, when that is the case.
fn unsupported_reason(host: Option<&dyn ServiceHost>) -> Option<String> {
    match host {
        None => Some("this platform has no systemd user services".to_string()),
        Some(host) if !systemd_user_available(host) => Some(
            "`systemctl --user` is not usable here (no per-user systemd instance)".to_string(),
        ),
        Some(_) => None,
    }
}

/// The real service host on Linux; `None` everywhere else. Split out so the
/// entry points read the same on every platform.
fn default_host() -> Option<SystemdUserHost> {
    if cfg!(target_os = "linux") {
        SystemdUserHost::new()
    } else {
        None
    }
}

/// `codemux connect [--email …] [--scope …] [--port N]`.
pub async fn run_connect(opts: ConnectOptions) -> Result<(), String> {
    let db = crate::database::init_database()?;

    // Step 1 — account. Everything downstream (relay registration, discovery
    // from a browser) is account-scoped, so this is the gate.
    let identity = cli_login::ensure_signed_in(&db, opts.email)
        .await?
        .identity()
        .to_string();

    let instance_running = crate::control::control_server_is_running();

    // Step 2 — configuration. Exactly one of the two write paths, per the
    // module note: the running instance owns its in-memory config, so with one
    // up we go through the control socket and never touch the settings row.
    let cfg = if instance_running {
        configure_via_control_socket(opts.scope.clone(), opts.port).await?
    } else {
        super::update_config_headless(&db, |cfg| {
            apply_connect_request(cfg, opts.scope.clone(), opts.port)
        })?
    };

    // Step 3 — keep it running.
    let host = default_host();
    let service = if instance_running {
        // `serve` refuses to start alongside another instance, so a unit here
        // would fail on every start. The running instance is already serving.
        ServiceReport::RunningInstance
    } else {
        let host_ref = host.as_ref().map(|h| h as &dyn ServiceHost);
        match unsupported_reason(host_ref) {
            Some(reason) => ServiceReport::Unsupported {
                reason,
                fallback: manual_fallback(),
            },
            None => {
                let host_ref = host_ref.expect("a supported host exists");
                let exec = resolve_exec_path()?;
                let unit = systemd_unit(
                    &exec.display().to_string(),
                    std::env::var("CODEMUX_API_URL").ok().as_deref(),
                );
                ServiceReport::Installed(install_service(host_ref, &unit)?)
            }
        }
    };

    // A blank line separates the report from whatever the sign-in prompt (or
    // the DB's own startup chatter) left on the terminal.
    println!();
    print!(
        "{}",
        format_connect_report(&ConnectReport {
            identity,
            scope: cfg.bind_scope.clone(),
            port: cfg.port,
            service,
        })
    );
    Ok(())
}

/// Configure a RUNNING instance: enable web remote (with any scope/port), then
/// flip relay mode on. Two calls because they are two different concerns in
/// the running app — binding the listener and starting the parallel iroh
/// endpoint — and each has its own rollback semantics there.
async fn configure_via_control_socket(
    scope: Option<String>,
    port: Option<u16>,
) -> Result<WebRemoteConfig, String> {
    let mut params = json!({});
    if let Some(s) = scope {
        params["scope"] = json!(s);
    }
    if let Some(p) = port {
        params["port"] = json!(p);
    }
    let enable = send_control_request(ControlRequest {
        command: "web_remote_enable".into(),
        params,
    })
    .await?;
    if !enable.ok {
        return Err(enable
            .error
            .unwrap_or_else(|| "the running Codemux instance refused to enable remote access".into()));
    }

    let relay = send_control_request(ControlRequest {
        command: "web_remote_set_relay".into(),
        params: json!({ "enabled": true }),
    })
    .await?;
    if !relay.ok {
        return Err(relay
            .error
            .unwrap_or_else(|| "the running Codemux instance refused to enable relay mode".into()));
    }

    // The relay response carries the authoritative post-change status.
    let status = relay.data.unwrap_or(json!({}));
    Ok(WebRemoteConfig {
        enabled: status["enabled"].as_bool().unwrap_or(true),
        port: status["port"]
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or(DEFAULT_PORT),
        bind_scope: status["bind_scope"]
            .as_str()
            .unwrap_or(super::BIND_SCOPE_ALL)
            .to_string(),
        relay_mode_enabled: status["relay_mode_enabled"].as_bool().unwrap_or(true),
        ..WebRemoteConfig::default()
    })
}

/// `codemux connect status`.
pub fn run_connect_status() -> Result<(), String> {
    let db = crate::database::init_database()?;
    let identity = match cli_login::auth_status(&db) {
        cli_login::AuthStatusReport::SignedIn { user, .. } => {
            Some(user.map(|u| u.email).unwrap_or_else(|| "(no cached profile)".to_string()))
        }
        _ => None,
    };
    let cfg = super::load_config_from_db(&db);
    let host = default_host();
    let host_ref = host.as_ref().map(|h| h as &dyn ServiceHost);
    let service_supported = unsupported_reason(host_ref).is_none();
    let service = match (service_supported, host_ref) {
        (true, Some(host)) => service_state(host),
        _ => ServiceState::default(),
    };

    print!(
        "{}",
        format_connect_status(&ConnectStatus {
            identity,
            enabled: cfg.enabled,
            relay_mode_enabled: cfg.relay_mode_enabled,
            scope: cfg.bind_scope.clone(),
            port: cfg.port,
            service,
            service_supported,
            instance_running: crate::control::control_server_is_running(),
            // The registry keys on these two; both are local facts, readable
            // whether or not anything is running. Live registration health is
            // only knowable inside a running instance, so it is deliberately
            // not claimed here.
            device_name: super::registration::device_name(),
            device_id: super::registration::persisted_device_id(&db),
        })
    );
    Ok(())
}

/// `codemux connect off`.
pub async fn run_connect_off() -> Result<(), String> {
    let db = crate::database::init_database()?;
    let instance_running = crate::control::control_server_is_running();

    // Config BEFORE the service, mirroring `connect`'s order — and load-bearing
    // rather than cosmetic. The instance we ask is usually the one
    // `codemux.service` is running, and its control endpoint dies with the unit.
    // Uninstalling first would leave us asking a socket that no longer answers
    // (`Failed to connect to Codemux control endpoint: Connection refused`), so
    // `off` would abort without turning relay off, without printing its report,
    // and with the unit already gone.
    //
    // Same split as `connect`: a running instance owns the config, so ask it.
    let was_on = super::load_config_from_db(&db).relay_mode_enabled;
    if instance_running {
        let resp = send_control_request(ControlRequest {
            command: "web_remote_set_relay".into(),
            params: json!({ "enabled": false }),
        })
        .await?;
        if !resp.ok {
            return Err(resp.error.unwrap_or_else(|| {
                "the running Codemux instance refused to disable relay mode".into()
            }));
        }
    } else {
        super::update_config_headless(&db, |cfg| {
            apply_disconnect_request(cfg);
            Ok(())
        })?;
    }

    let host = default_host();
    let host_ref = host.as_ref().map(|h| h as &dyn ServiceHost);
    let service_supported = unsupported_reason(host_ref).is_none();
    let service_removed = match (service_supported, host_ref) {
        (true, Some(host)) => uninstall_service(host)?,
        _ => false,
    };

    let identity = match cli_login::auth_status(&db) {
        cli_login::AuthStatusReport::SignedIn { user, .. } => user.map(|u| u.email),
        _ => None,
    };

    print!(
        "{}",
        format_disconnect_report(&DisconnectReport {
            service_removed,
            service_supported,
            relay_turned_off: was_on,
            instance_running,
            identity,
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ── Unit file ────────────────────────────────────────────────

    #[test]
    fn unit_runs_the_serve_subcommand_from_the_current_binary() {
        let unit = systemd_unit("/usr/bin/codemux", None);
        assert!(unit.contains("ExecStart=/usr/bin/codemux serve"));
        // No `--relay`: relay mode rides the persisted config, so the unit
        // never has to be rewritten when the toggle changes.
        assert!(!unit.contains("--relay"), "unit must stay config-driven:\n{unit}");
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("RestartSec=5s"));
        assert!(unit.contains("Wants=network-online.target"));
        assert!(unit.contains("After=network-online.target"));
        assert!(unit.contains("WantedBy=default.target"));
        // Diagnostics are tailable; the pairing token on stdout is not logged.
        assert!(unit.contains("StandardError=journal"));
        assert!(unit.contains("StandardOutput=null"));
    }

    #[test]
    fn unit_name_does_not_collide_with_the_managed_host_units() {
        // `codemux-remote.service` / `codemux-scheduler.service` are installed
        // on managed hosts by the SSH bootstrap and run a different binary.
        assert_eq!(UNIT_NAME, "codemux.service");
        assert_ne!(UNIT_NAME, crate::automations::service::SERVE_SYSTEMD_UNIT_NAME);
        assert_ne!(UNIT_NAME, crate::automations::service::SYSTEMD_UNIT_NAME);
        assert_eq!(unit_stem(), "codemux");
    }

    #[test]
    fn unit_carries_a_custom_api_url_but_omits_the_line_by_default() {
        let default = systemd_unit("/usr/bin/codemux", None);
        assert!(!default.contains("Environment="), "no stray env line:\n{default}");

        let custom = systemd_unit("/usr/bin/codemux", Some("https://api.example.test"));
        assert!(custom.contains("Environment=CODEMUX_API_URL=https://api.example.test\n"));
        // Still a well-formed unit: the env line sits inside [Service].
        let service_idx = custom.find("[Service]").unwrap();
        let install_idx = custom.find("[Install]").unwrap();
        let env_idx = custom.find("Environment=").unwrap();
        assert!(service_idx < env_idx && env_idx < install_idx);

        // An empty value is treated as unset rather than written as a blank.
        assert!(!systemd_unit("/usr/bin/codemux", Some("  ")).contains("Environment="));
    }

    // ── Which binary the unit execs ──────────────────────────────

    #[test]
    fn a_packaged_install_execs_the_binary_it_is_running_as() {
        // .deb / .rpm / AUR: a plain binary, no AppRun in sight.
        let exe = PathBuf::from("/usr/bin/codemux");
        assert_eq!(
            service_exec_path(&exe, None, Some(PathBuf::from("/usr/bin/codemux"))),
            exe
        );
        // An empty APPDIR is not an AppImage either.
        assert_eq!(service_exec_path(&exe, Some("  "), None), exe);
    }

    #[test]
    fn an_appimage_install_execs_the_wrapper_not_the_inner_binary() {
        // The installer's fallback shape: `codemux` on PATH is a wrapper that
        // execs AppRun, and AppRun is what exports the bundled-library
        // environment. `current_exe()` is the inner binary, which would start
        // without that environment.
        let inner = PathBuf::from("/usr/local/lib/codemux/usr/bin/codemux");
        let wrapper = PathBuf::from("/usr/local/bin/codemux");
        assert_eq!(
            service_exec_path(&inner, Some("/usr/local/lib/codemux"), Some(wrapper.clone())),
            wrapper
        );
    }

    #[test]
    fn an_appimage_with_no_wrapper_on_path_falls_back_to_apprun() {
        let inner = PathBuf::from("/opt/codemux.AppDir/usr/bin/codemux");
        assert_eq!(
            service_exec_path(&inner, Some("/opt/codemux.AppDir"), None),
            PathBuf::from("/opt/codemux.AppDir/AppRun")
        );
        // Same when PATH resolves to the very binary we are already running:
        // exec'ing it again would skip AppRun a second time.
        assert_eq!(
            service_exec_path(&inner, Some("/opt/codemux.AppDir"), Some(inner.clone())),
            PathBuf::from("/opt/codemux.AppDir/AppRun")
        );
    }

    #[test]
    fn find_on_path_locates_a_real_executable() {
        // Sanity-check the PATH walk against something every host has, so a
        // broken implementation can't silently always return `None` (which
        // would quietly downgrade every AppImage install to the AppRun path).
        //
        // The fixture is per-platform: the lookup matches names literally, so
        // on Windows the shell is `cmd.exe` (there is no extensionless `cmd`
        // file in System32), while `sh` is the safe bet everywhere else.
        let shell = if cfg!(windows) { "cmd.exe" } else { "sh" };
        assert!(
            find_on_path(shell).is_some(),
            "PATH lookup should find {shell}"
        );
        assert!(find_on_path("codemux-definitely-not-installed").is_none());
    }

    // ── Fake service host ────────────────────────────────────────

    /// Records every call and answers from a scripted table, so the install
    /// sequence — including each rollback branch — is exercised without a
    /// service manager.
    struct FakeHost {
        calls: RefCell<Vec<String>>,
        /// `(command-prefix, outcome)`; first prefix match wins.
        scripted: Vec<(String, CmdOutcome)>,
        /// Successive `is-active` answers, consumed in order; the last one
        /// repeats.
        active_sequence: RefCell<Vec<String>>,
        unit: RefCell<Option<String>>,
        write_fails: bool,
    }

    fn ok(stdout: &str) -> CmdOutcome {
        CmdOutcome {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn fail(stderr: &str) -> CmdOutcome {
        CmdOutcome {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                scripted: Vec::new(),
                active_sequence: RefCell::new(vec!["active".to_string()]),
                unit: RefCell::new(None),
                write_fails: false,
            }
        }

        fn script(mut self, prefix: &str, outcome: CmdOutcome) -> Self {
            self.scripted.push((prefix.to_string(), outcome));
            self
        }

        fn active_after(self, states: &[&str]) -> Self {
            *self.active_sequence.borrow_mut() =
                states.iter().map(|s| s.to_string()).collect();
            self
        }

        fn with_unit_installed(self) -> Self {
            *self.unit.borrow_mut() = Some("existing".to_string());
            self
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }

        fn ran(&self, needle: &str) -> bool {
            self.calls().iter().any(|c| c.contains(needle))
        }
    }

    impl ServiceHost for FakeHost {
        fn unit_path(&self) -> PathBuf {
            PathBuf::from("/home/test/.config/systemd/user").join(UNIT_NAME)
        }

        fn unit_exists(&self) -> bool {
            self.unit.borrow().is_some()
        }

        fn write_unit(&self, contents: &str) -> Result<(), String> {
            self.calls.borrow_mut().push("write_unit".to_string());
            if self.write_fails {
                return Err("permission denied".to_string());
            }
            *self.unit.borrow_mut() = Some(contents.to_string());
            Ok(())
        }

        fn remove_unit(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("remove_unit".to_string());
            *self.unit.borrow_mut() = None;
            Ok(())
        }

        fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutcome, String> {
            let line = format!("{program} {}", args.join(" "));
            self.calls.borrow_mut().push(line.clone());
            if line.contains("is-active") {
                let mut seq = self.active_sequence.borrow_mut();
                let state = if seq.len() > 1 {
                    seq.remove(0)
                } else {
                    seq.first().cloned().unwrap_or_default()
                };
                return Ok(ok(&state));
            }
            for (prefix, outcome) in &self.scripted {
                if line.contains(prefix.as_str()) {
                    return Ok(outcome.clone());
                }
            }
            Ok(ok(""))
        }

        fn sleep(&self, _duration: Duration) {
            self.calls.borrow_mut().push("sleep".to_string());
        }
    }

    // ── Install: happy path ──────────────────────────────────────

    #[test]
    fn install_writes_reloads_enables_and_verifies_in_order() {
        let host = FakeHost::new();
        let report = install_service(&host, "unit-body").expect("install succeeds");

        let calls = host.calls();
        let positions = |needle: &str| calls.iter().position(|c| c.contains(needle)).unwrap();
        assert!(positions("write_unit") < positions("daemon-reload"));
        assert!(positions("daemon-reload") < positions("enable --now"));
        assert!(positions("enable --now") < positions("is-active"));
        assert!(positions("is-active") < positions("enable-linger"));
        assert_eq!(report.linger, LingerOutcome::Enabled);
        assert!(report.unit_path.ends_with(UNIT_NAME));
        assert_eq!(host.unit.borrow().as_deref(), Some("unit-body"));
    }

    #[test]
    fn install_waits_out_a_slow_start_instead_of_failing() {
        // `serve` boots the whole backend, so `activating` on the first polls
        // is normal — failing there would make the command flaky on slow boxes.
        let host = FakeHost::new().active_after(&["activating", "activating", "active"]);
        install_service(&host, "unit-body").expect("a slow start still succeeds");
        assert!(host.ran("sleep"), "polls are spaced out");
        assert!(!host.ran("disable --now"), "no rollback on a slow start");
    }

    // ── Install: rollback ────────────────────────────────────────

    #[test]
    fn a_failed_daemon_reload_removes_the_unit_it_wrote() {
        let host = FakeHost::new().script("daemon-reload", fail("Failed to connect to bus"));
        let err = install_service(&host, "unit-body").expect_err("reload failure fails install");
        assert!(err.contains("daemon-reload"), "{err}");
        assert!(err.contains("Failed to connect to bus"), "surfaces the reason: {err}");
        assert!(!host.unit_exists(), "the half-written unit is gone");
    }

    #[test]
    fn a_failed_enable_rolls_the_whole_install_back() {
        let host = FakeHost::new().script("enable --now", fail("Unit is masked"));
        let err = install_service(&host, "unit-body").expect_err("enable failure fails install");
        assert!(err.contains("Unit is masked"), "{err}");
        assert!(host.ran("disable --now"), "rollback disables the unit");
        assert!(!host.unit_exists(), "rollback removes the unit file");
        assert!(
            host.calls().iter().filter(|c| c.contains("daemon-reload")).count() >= 2,
            "rollback reloads systemd after removing the file: {:?}",
            host.calls()
        );
    }

    #[test]
    fn a_unit_that_never_activates_is_removed_again() {
        let host = FakeHost::new().active_after(&["activating"]);
        let err = install_service(&host, "unit-body").expect_err("never-active fails install");
        assert!(err.contains("never became active"), "{err}");
        assert!(err.contains("journalctl --user -u codemux"), "points at the log: {err}");
        assert!(host.ran("disable --now"));
        assert!(!host.unit_exists());
    }

    #[test]
    fn a_failed_unit_gives_up_immediately_rather_than_polling_it_out() {
        let host = FakeHost::new().active_after(&["failed"]);
        install_service(&host, "unit-body").expect_err("a failed unit fails install");
        assert!(
            !host.ran("sleep"),
            "a terminal `failed` state must not wait out every poll: {:?}",
            host.calls()
        );
    }

    #[test]
    fn an_unwritable_unit_path_fails_before_touching_systemd() {
        let mut host = FakeHost::new();
        host.write_fails = true;
        let err = install_service(&host, "unit-body").expect_err("write failure fails install");
        assert!(err.contains("permission denied"), "{err}");
        assert!(
            !host.ran("systemctl"),
            "nothing is asked of systemd when the unit was never written"
        );
    }

    // ── Linger ───────────────────────────────────────────────────

    #[test]
    fn a_refused_linger_still_reports_a_successful_install() {
        // Containers and some managed hosts refuse lingering; the service
        // still runs, so this is a warning rather than a failed connect.
        let host = FakeHost::new().script("enable-linger", fail("Operation not permitted"));
        let report = install_service(&host, "unit-body").expect("linger is best-effort");
        assert_eq!(
            report.linger,
            LingerOutcome::Refused("Operation not permitted".to_string())
        );
        assert!(host.unit_exists(), "the service is left installed and running");
    }

    // ── Uninstall ────────────────────────────────────────────────

    #[test]
    fn uninstall_stops_disables_and_removes() {
        let host = FakeHost::new().with_unit_installed();
        assert!(uninstall_service(&host).expect("uninstall succeeds"));
        assert!(host.ran("disable --now"));
        assert!(host.ran("remove_unit"));
        assert!(host.ran("daemon-reload"));
        assert!(!host.unit_exists());
    }

    #[test]
    fn uninstall_is_a_no_op_when_nothing_is_installed() {
        let host = FakeHost::new();
        assert!(!uninstall_service(&host).expect("no unit is not an error"));
        assert!(
            !host.ran("systemctl"),
            "systemd is never asked about a unit that was never installed"
        );
    }

    #[test]
    fn uninstall_removes_the_file_even_when_disable_fails() {
        // A half-broken install (unit file present, systemd unhappy) must
        // still be cleanable — otherwise `connect off` can never recover.
        let host = FakeHost::new()
            .with_unit_installed()
            .script("disable --now", fail("Unit codemux.service not loaded"));
        assert!(uninstall_service(&host).expect("still reports the removal"));
        assert!(!host.unit_exists());
    }

    // ── Availability probe ───────────────────────────────────────

    #[test]
    fn systemd_availability_follows_the_user_bus_probe() {
        assert!(systemd_user_available(&FakeHost::new()));
        let no_bus = FakeHost::new()
            .script("show-environment", fail("Failed to connect to bus: No such file"));
        assert!(!systemd_user_available(&no_bus));
        assert!(unsupported_reason(Some(&no_bus)).is_some());
        assert!(unsupported_reason(None).is_some());
        assert!(unsupported_reason(Some(&FakeHost::new())).is_none());
    }

    // ── Service state ────────────────────────────────────────────

    #[test]
    fn service_state_reports_nothing_when_no_unit_is_installed() {
        let host = FakeHost::new();
        assert_eq!(service_state(&host), ServiceState::default());
        assert!(!host.ran("is-active"), "no unit, nothing to ask about");
    }

    #[test]
    fn service_state_reads_active_and_enabled() {
        let host = FakeHost::new()
            .with_unit_installed()
            .script("is-enabled", ok("enabled"));
        let state = service_state(&host);
        assert!(state.installed);
        assert!(state.active);
        assert!(state.enabled);
    }

    // ── Config mutation ──────────────────────────────────────────

    #[test]
    fn connect_turns_remote_access_and_relay_mode_on() {
        let mut cfg = WebRemoteConfig::default();
        assert!(!cfg.enabled && !cfg.relay_mode_enabled);
        apply_connect_request(&mut cfg, None, None).unwrap();
        assert!(cfg.enabled, "the server binds");
        assert!(cfg.relay_mode_enabled, "and is reachable from anywhere");
        assert_eq!(cfg.bind_scope, super::super::BIND_SCOPE_ALL);
        assert_eq!(cfg.port, DEFAULT_PORT);
    }

    #[test]
    fn connect_folds_in_scope_and_port_and_leaves_them_alone_otherwise() {
        let mut cfg = WebRemoteConfig::default();
        apply_connect_request(
            &mut cfg,
            Some(super::super::BIND_SCOPE_TAILSCALE.to_string()),
            Some(5100),
        )
        .unwrap();
        assert_eq!(cfg.bind_scope, super::super::BIND_SCOPE_TAILSCALE);
        assert_eq!(cfg.port, 5100);

        // A re-run with no flags must not reset a tuned setup.
        apply_connect_request(&mut cfg, None, None).unwrap();
        assert_eq!(cfg.bind_scope, super::super::BIND_SCOPE_TAILSCALE);
        assert_eq!(cfg.port, 5100);
    }

    #[test]
    fn connect_rejects_an_unknown_scope_without_mutating_anything() {
        let mut cfg = WebRemoteConfig::default();
        let err = apply_connect_request(&mut cfg, Some("wan".to_string()), None).unwrap_err();
        assert!(err.contains("Unknown access scope"), "{err}");
        assert!(!cfg.enabled, "a rejected request persists nothing");
        assert!(!cfg.relay_mode_enabled);
    }

    #[test]
    fn disconnect_only_turns_relay_off() {
        let mut cfg = WebRemoteConfig {
            enabled: true,
            relay_mode_enabled: true,
            port: 5100,
            ..WebRemoteConfig::default()
        };
        apply_disconnect_request(&mut cfg);
        assert!(!cfg.relay_mode_enabled, "from-anywhere access is off");
        assert!(cfg.enabled, "the LAN/tailnet server is left exactly as it was");
        assert_eq!(cfg.port, 5100);
    }

    #[test]
    fn headless_config_round_trips_through_the_settings_store() {
        // The write path a fresh box uses: no app, no running instance — the
        // config must still land where `restore_on_boot` reads it.
        let db = crate::database::init_test_database();
        let cfg = super::super::update_config_headless(&db, |cfg| {
            apply_connect_request(cfg, Some(super::super::BIND_SCOPE_LOOPBACK.to_string()), Some(4400))
        })
        .expect("headless write succeeds");
        assert!(cfg.enabled && cfg.relay_mode_enabled);

        let read_back = super::super::load_config_from_db(&db);
        assert!(read_back.enabled);
        assert!(read_back.relay_mode_enabled);
        assert_eq!(read_back.bind_scope, super::super::BIND_SCOPE_LOOPBACK);
        assert_eq!(read_back.port, 4400);
    }

    #[test]
    fn a_rejected_headless_mutation_writes_nothing() {
        let db = crate::database::init_test_database();
        super::super::update_config_headless(&db, |cfg| {
            apply_connect_request(cfg, Some("wan".to_string()), None)
        })
        .expect_err("an invalid scope is rejected");
        let read_back = super::super::load_config_from_db(&db);
        assert!(!read_back.enabled, "nothing was persisted");
        assert!(!read_back.relay_mode_enabled);
    }

    // ── Output copy ──────────────────────────────────────────────

    fn installed_report() -> ConnectReport {
        ConnectReport {
            identity: "user@example.com".to_string(),
            scope: super::super::BIND_SCOPE_ALL.to_string(),
            port: DEFAULT_PORT,
            service: ServiceReport::Installed(InstallReport {
                unit_path: "/home/u/.config/systemd/user/codemux.service".to_string(),
                linger: LingerOutcome::Enabled,
            }),
        }
    }

    #[test]
    fn the_success_output_reads_as_three_ticks_and_a_link() {
        let text = format_connect_report(&installed_report());
        assert!(text.contains("✓ Signed in as user@example.com"));
        assert!(text.contains("✓ Remote access configured (relay mode on"));
        assert!(text.contains("✓ Background service ready — Codemux stays reachable after you log out"));
        assert!(text.contains(MANAGE_URL));
        assert_eq!(text.matches('✓').count(), 3, "three steps, three ticks:\n{text}");
        assert!(!text.contains('⚠'), "nothing to warn about:\n{text}");
    }

    #[test]
    fn a_refused_linger_shows_a_warning_alongside_the_success() {
        let mut report = installed_report();
        report.service = ServiceReport::Installed(InstallReport {
            unit_path: "/home/u/.config/systemd/user/codemux.service".to_string(),
            linger: LingerOutcome::Refused("Operation not permitted".to_string()),
        });
        let text = format_connect_report(&report);
        assert!(text.contains("✓ Background service ready"), "still a success:\n{text}");
        assert!(text.contains("⚠ Could not keep the service alive past logout"));
        assert!(text.contains("Operation not permitted"));
    }

    #[test]
    fn a_running_instance_is_reported_instead_of_a_service() {
        let mut report = installed_report();
        report.service = ServiceReport::RunningInstance;
        let text = format_connect_report(&report);
        assert!(text.contains("already running on this machine"));
        assert!(text.contains("comes back the next time that instance starts"));
        assert!(!text.contains("Background service ready"));
    }

    #[test]
    fn an_unsupported_platform_still_reports_the_configured_steps() {
        let mut report = installed_report();
        report.service = ServiceReport::Unsupported {
            reason: "`systemctl --user` is not usable here".to_string(),
            fallback: manual_fallback(),
        };
        let text = format_connect_report(&report);
        assert!(text.contains("✓ Signed in as"), "step 1 still happened:\n{text}");
        assert!(text.contains("✓ Remote access configured"), "step 2 still happened");
        assert!(text.contains("⚠ No background service was installed"));
        assert!(text.contains("codemux serve"), "names the manual fallback");
    }

    #[test]
    fn status_names_every_layer() {
        let text = format_connect_status(&ConnectStatus {
            identity: Some("user@example.com".to_string()),
            enabled: true,
            relay_mode_enabled: true,
            scope: super::super::BIND_SCOPE_TAILSCALE.to_string(),
            port: 4377,
            service: ServiceState {
                installed: true,
                active: true,
                enabled: true,
                lingering: Some(true),
            },
            service_supported: true,
            instance_running: false,
            device_name: "vps-fra-1".to_string(),
            device_id: Some("dev-123".to_string()),
        });
        assert!(text.contains("Signed in as user@example.com"));
        assert!(text.contains("Relay mode:   on"));
        assert!(text.contains("tailscale (tailnet + loopback only)"));
        assert!(text.contains("codemux.service: installed, active, starts at boot"));
        assert!(text.contains("Survives logout: yes"));
        assert!(text.contains("vps-fra-1"));
        assert!(text.contains("dev-123"));
    }

    #[test]
    fn status_on_a_fresh_machine_says_what_is_missing() {
        let text = format_connect_status(&ConnectStatus {
            identity: None,
            enabled: false,
            relay_mode_enabled: false,
            scope: super::super::BIND_SCOPE_ALL.to_string(),
            port: DEFAULT_PORT,
            service: ServiceState::default(),
            service_supported: true,
            instance_running: false,
            device_name: "fresh-box".to_string(),
            device_id: None,
        });
        assert!(text.contains("Not signed in"));
        assert!(text.contains("Enabled:      no"));
        assert!(text.contains("Relay mode:   off"));
        assert!(text.contains("codemux.service: not installed"));
        assert!(!text.contains("Device id"), "no id before first registration:\n{text}");
    }

    #[test]
    fn off_reports_what_it_removed_and_keeps_the_session() {
        let text = format_disconnect_report(&DisconnectReport {
            service_removed: true,
            service_supported: true,
            relay_turned_off: true,
            instance_running: false,
            identity: Some("user@example.com".to_string()),
        });
        assert!(text.contains("✓ Background service stopped and removed (codemux.service)"));
        assert!(text.contains("✓ From-anywhere access is off"));
        assert!(text.contains("Still signed in as user@example.com"));
        assert!(text.contains("codemux logout"), "says how to go further");
    }

    #[test]
    fn off_on_an_already_disconnected_machine_claims_nothing() {
        let text = format_disconnect_report(&DisconnectReport {
            service_removed: false,
            service_supported: true,
            relay_turned_off: false,
            instance_running: false,
            identity: None,
        });
        assert!(text.contains("No background service was installed"));
        assert!(text.contains("already off"));
        assert!(!text.contains('✓'), "nothing was done, so nothing is claimed:\n{text}");
    }
}
