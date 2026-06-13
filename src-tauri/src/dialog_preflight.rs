//! Preflight check for native file dialogs on Linux (issue #95).
//!
//! The dialog plugin is compiled portal-only on unix (see Cargo.toml:
//! `tauri-plugin-dialog` with `features = ["xdg-portal"]`), and the
//! underlying rfd crate falls back to spawning `zenity` when the
//! portal call fails. On minimal window-manager setups (i3, dwm, ...)
//! neither the XDG desktop portal nor zenity is guaranteed to exist.
//! When both are missing, rfd resolves the dialog to `None` — the
//! exact same value as a user cancel — and the UI silently does
//! nothing.
//!
//! This module detects that situation *before* the dialog is opened
//! so the command can return a real error that the frontend turns
//! into an actionable toast, and `codemux doctor` can print a
//! diagnosis. Non-Linux platforms always pass: their native dialogs
//! need no external services.

/// Stable, greppable marker the frontend matches on to distinguish
/// the "no backend installed" failure from any other dialog error.
/// Kept in sync with `NO_FILE_PICKER_BACKEND` in
/// `src/lib/file-dialog.ts`.
pub const NO_BACKEND_MARKER: &str = "NO_FILE_PICKER_BACKEND";

/// Human install hint appended to the error and surfaced in the
/// frontend toast and `codemux doctor`.
pub const INSTALL_HINT: &str = "Install xdg-desktop-portal plus a backend such as \
     xdg-desktop-portal-gtk and restart your session, or install zenity.";

/// Result of probing every dialog path the compiled backend can take.
#[cfg(target_os = "linux")]
pub struct FilePickerDiagnosis {
    /// `Ok(version)` when the portal's FileChooser interface answered
    /// a `version` property read; `Err(reason)` otherwise. A running
    /// portal without any FileChooser backend fails this probe too,
    /// which is exactly what we want — the interface is only exported
    /// when a backend implements it.
    pub portal: Result<u32, String>,
    /// Path of the `zenity` binary when present on `PATH` (rfd's
    /// automatic fallback when the portal call fails).
    pub zenity: Option<std::path::PathBuf>,
}

#[cfg(target_os = "linux")]
impl FilePickerDiagnosis {
    /// At least one path can produce a visible dialog.
    pub fn usable(&self) -> bool {
        self.portal.is_ok() || self.zenity.is_some()
    }
}

#[cfg(target_os = "linux")]
pub async fn diagnose() -> FilePickerDiagnosis {
    FilePickerDiagnosis {
        portal: portal_file_chooser_version().await,
        zenity: which::which("zenity").ok(),
    }
}

/// Read the `version` property of `org.freedesktop.portal.FileChooser`
/// on the session bus. This is the canonical availability probe: the
/// portal frontend only exports an interface when some backend
/// implements it, so this fails fast for "portal not installed",
/// "no session bus", and "portal running but no FileChooser backend"
/// alike.
#[cfg(target_os = "linux")]
async fn portal_file_chooser_version() -> Result<u32, String> {
    use std::time::Duration;

    let probe = async {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| format!("session bus unavailable: {error}"))?;
        let proxy = zbus::Proxy::new(
            &connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.FileChooser",
        )
        .await
        .map_err(|error| format!("portal proxy setup failed: {error}"))?;
        proxy
            .get_property::<u32>("version")
            .await
            .map_err(|error| format!("FileChooser interface not provided: {error}"))
    };

    // A healthy portal answers in single-digit milliseconds; the
    // timeout only guards against a wedged bus so the UI button never
    // feels dead for longer than this.
    tokio::time::timeout(Duration::from_secs(4), probe)
        .await
        .map_err(|_| "timed out talking to the desktop portal".to_string())?
}

/// `Err(actionable message)` when no file-dialog backend can possibly
/// work, `Ok(())` otherwise. Called by every dialog command before
/// the dialog is built.
pub async fn ensure_file_picker_backend() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let diagnosis = diagnose().await;
        if !diagnosis.usable() {
            let portal_error = match &diagnosis.portal {
                Err(reason) => reason.clone(),
                Ok(_) => unreachable!("usable() is false, portal must be Err"),
            };
            log::error!(
                "no file picker backend: portal probe failed ({portal_error}); zenity not on PATH"
            );
            return Err(format!(
                "{NO_BACKEND_MARKER}: cannot open a file dialog. \
                 The XDG desktop portal is unavailable ({portal_error}) \
                 and zenity is not installed. {INSTALL_HINT}"
            ));
        }
    }
    Ok(())
}
