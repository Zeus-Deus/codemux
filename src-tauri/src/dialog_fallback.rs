//! Direct zenity fallback for Linux file dialogs (issue #95).
//!
//! The dialog plugin is compiled portal-only (`tauri-plugin-dialog`
//! with the `xdg-portal` feature → rfd → ashpd). rfd *does* fall back
//! to spawning `zenity`, but only when the portal call returns an
//! error. On minimal window-manager sessions the portal frequently
//! *hangs* instead (the preflight catches that via timeout), so rfd's
//! own fallback never runs — and when it does run, rfd spawns zenity
//! with the inherited session environment, the very environment that
//! broke the portal in the first place (a stale/dead D-Bus session
//! bus, or a GTK module that blocks `gtk_init`), so zenity hangs too.
//!
//! This module is the escape hatch: when the preflight decides the
//! portal is unusable but a `zenity` binary exists, the dialog command
//! calls in here instead of rfd. We spawn zenity ourselves with
//!
//! 1. a **sanitized environment** — the vars that hang GTK clients on
//!    broken sessions are cleared, and the accessibility bridge (a
//!    classic source of multi-second `gtk_init` stalls) is disabled;
//! 2. a **hard timeout** — a wedged zenity can never leave the picker
//!    button dead forever; it resolves as a cancel-with-error instead.
//!
//! Non-Linux platforms never use this module: their native dialogs
//! need no external process.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// Environment variables cleared for the zenity subprocess. On a
/// healthy desktop these are harmless to keep; we only ever reach this
/// module when the portal is already broken, which strongly correlates
/// with the session environment below being the cause.
///
/// - `DBUS_SESSION_BUS_ADDRESS`: when it points at a dead or wedged bus
///   (e.g. a greeter's address that outlived its bus), GTK blocks
///   talking to it during init. Clearing it lets GTK autolaunch a
///   private session bus, or run without one — either way it no longer
///   hangs.
/// - `GTK_MODULES` / `GTK3_MODULES`: a module that blocks (a stale
///   accessibility bridge is the usual culprit) stalls `gtk_init`.
const SANITIZE_VARS: &[&str] = &["DBUS_SESSION_BUS_ADDRESS", "GTK_MODULES", "GTK3_MODULES"];

/// Default safety timeout. Generous enough that a user genuinely
/// browsing the filesystem is never cut off, but bounded so a zenity
/// wedged at init eventually releases the dialog. Overridable via
/// `CODEMUX_ZENITY_TIMEOUT_MS` (used by tests, and as an operational
/// escape hatch).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

fn timeout() -> Duration {
    std::env::var("CODEMUX_ZENITY_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// A zenity command pre-seeded with the sanitized environment.
fn base_command(zenity: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(zenity);
    cmd.arg("--no-markup");
    for var in SANITIZE_VARS {
        cmd.env_remove(var);
    }
    // Disable the at-spi accessibility bridge for this child. Without a
    // running a11y bus, GTK clients can block for many seconds during
    // init waiting for it; the file picker needs none of it.
    cmd.env("NO_AT_BRIDGE", "1");
    cmd.env("GTK_A11Y", "none");
    cmd.kill_on_drop(true);
    cmd
}

/// Apply rfd-compatible `--file-filter NAME | *.ext *.ext` arguments.
fn add_filters(cmd: &mut tokio::process::Command, name: Option<&str>, extensions: Option<&[String]>) {
    if let (Some(name), Some(exts)) = (name, extensions) {
        if !exts.is_empty() {
            let globs: Vec<String> = exts.iter().map(|ext| format!("*.{ext}")).collect();
            cmd.arg("--file-filter");
            cmd.arg(format!("{name} | {}", globs.join(" ")));
        }
    }
}

/// Run a prepared zenity command to completion under the timeout.
///
/// Returns `Ok(Some(stdout))` when the user confirmed a selection
/// (zenity exits 0 with output), `Ok(None)` on cancel (exit 1 / empty),
/// and `Err(msg)` when zenity could not be run or timed out.
async fn run(mut cmd: tokio::process::Command) -> Result<Option<String>, String> {
    let child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("could not launch zenity: {err}"))?;

    match tokio::time::timeout(timeout(), child.wait_with_output()).await {
        // Timed out: the future is dropped here, and `kill_on_drop`
        // reaps the wedged zenity so it can't linger.
        Err(_) => Err("zenity timed out (no dialog appeared)".to_string()),
        Ok(Err(err)) => Err(format!("zenity failed: {err}")),
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let trimmed = stdout.trim();
            if output.status.success() && !trimmed.is_empty() {
                Ok(Some(trimmed.to_string()))
            } else {
                // Non-zero exit or empty output == user cancelled.
                Ok(None)
            }
        }
    }
}

/// Folder picker. `Ok(None)` on cancel.
pub async fn pick_folder(zenity: &Path, title: &str) -> Result<Option<PathBuf>, String> {
    let mut cmd = base_command(zenity);
    cmd.args(["--file-selection", "--directory", "--title", title]);
    Ok(run(cmd).await?.map(PathBuf::from))
}

/// Multi-file picker. Returns an empty vec on cancel.
pub async fn pick_files(zenity: &Path, title: &str) -> Result<Vec<PathBuf>, String> {
    let mut cmd = base_command(zenity);
    // Newline separator so paths containing the default `|` don't split
    // incorrectly.
    cmd.args([
        "--file-selection",
        "--multiple",
        "--separator",
        "\n",
        "--title",
        title,
    ]);
    Ok(run(cmd)
        .await?
        .map(|out| out.lines().map(PathBuf::from).collect())
        .unwrap_or_default())
}

/// Single-file open dialog with an optional filter. `Ok(None)` on cancel.
pub async fn pick_open_file(
    zenity: &Path,
    title: &str,
    filter_name: Option<&str>,
    filter_extensions: Option<&[String]>,
) -> Result<Option<PathBuf>, String> {
    let mut cmd = base_command(zenity);
    cmd.args(["--file-selection", "--title", title]);
    add_filters(&mut cmd, filter_name, filter_extensions);
    Ok(run(cmd).await?.map(PathBuf::from))
}

/// Save-as dialog. `Ok(None)` on cancel.
pub async fn save_file(
    zenity: &Path,
    title: &str,
    default_filename: Option<&str>,
    filter_name: Option<&str>,
    filter_extensions: Option<&[String]>,
) -> Result<Option<PathBuf>, String> {
    let mut cmd = base_command(zenity);
    cmd.args([
        "--file-selection",
        "--save",
        "--confirm-overwrite",
        "--title",
        title,
    ]);
    if let Some(name) = default_filename {
        cmd.args(["--filename", name]);
    }
    add_filters(&mut cmd, filter_name, filter_extensions);
    Ok(run(cmd).await?.map(PathBuf::from))
}
