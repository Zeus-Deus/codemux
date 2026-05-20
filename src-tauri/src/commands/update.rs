/// Returns the install-package format of the running app.
///
/// The frontend uses this to decide whether in-app auto-update
/// (`downloadAndInstall`) is available for the current install:
/// - `appimage` — Linux AppImage; the Tauri updater swaps it in place
/// - `nsis` — Windows NSIS installer; the Tauri updater downloads and
///   applies the new installer in place
/// - `other` — `.deb` / `.rpm` and anything else, where updates are
///   owned by the system package manager and the app can only point the
///   user at the download page
///
/// Windows ships exclusively as an NSIS installer (see
/// `release.yml` — the Windows leg builds with `--bundles nsis`), so
/// every Windows install is auto-updatable.
#[tauri::command]
pub fn get_package_format() -> String {
    if cfg!(target_os = "windows") {
        "nsis".to_string()
    } else if std::env::var("APPIMAGE").is_ok() {
        "appimage".to_string()
    } else {
        "other".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_format_is_a_known_value() {
        // Whatever platform CI runs this on, the value must be one the
        // frontend's `canAutoUpdateFormat` knows how to interpret.
        let fmt = get_package_format();
        assert!(
            matches!(fmt.as_str(), "appimage" | "nsis" | "other"),
            "unexpected package format: {fmt}"
        );
    }

    /// Regression guard: Windows installs MUST report `nsis` so the
    /// in-app updater is offered. Previously this returned `other` on
    /// Windows, which silently downgraded every Windows user to a
    /// manual download-and-reinstall flow.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_reports_nsis() {
        assert_eq!(get_package_format(), "nsis");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_without_appimage_env_reports_other() {
        // The CI runner is not an AppImage, so the env var is unset.
        if std::env::var("APPIMAGE").is_err() {
            assert_eq!(get_package_format(), "other");
        }
    }
}
