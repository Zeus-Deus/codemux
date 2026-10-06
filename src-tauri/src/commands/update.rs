/// Returns the install-package format of the running app.
///
/// The frontend uses this to decide whether in-app auto-update
/// (`downloadAndInstall`) is available for the current install:
/// - `appimage` — Linux AppImage; the Tauri updater swaps it in place
/// - `nsis` — Windows NSIS installer; the Tauri updater downloads and
///   applies the new installer in place
/// - `pacman` — Arch / AUR install (`codemux-bin`); pacman owns the binary,
///   so the app tells the user which command updates it
/// - `other` — `.deb` / `.rpm` and anything else, where updates are
///   owned by the system package manager and the app can only point the
///   user at the download page
///
/// Windows ships exclusively as an NSIS installer (see
/// `release.yml` — the Windows leg builds with `--bundles nsis`), so
/// every Windows install is auto-updatable.
///
/// Async so the `pacman -Qo` probe never runs on the main thread.
#[tauri::command]
pub async fn get_package_format() -> String {
    tauri::async_runtime::spawn_blocking(detect_package_format)
        .await
        .unwrap_or_else(|_| "other".to_string())
}

fn detect_package_format() -> String {
    classify_package_format(
        cfg!(target_os = "windows"),
        std::env::var_os("APPIMAGE").is_some(),
        installed_by_pacman,
    )
    .to_string()
}

/// Pure decision behind [`get_package_format`]. The pacman probe spawns a
/// process, so it only runs when nothing cheaper has already answered.
fn classify_package_format(
    is_windows: bool,
    is_appimage: bool,
    installed_by_pacman: impl FnOnce() -> bool,
) -> &'static str {
    if is_windows {
        "nsis"
    } else if is_appimage {
        "appimage"
    } else if installed_by_pacman() {
        "pacman"
    } else {
        "other"
    }
}

/// True when pacman owns the running executable. Fails closed: no pacman on
/// PATH, or a binary pacman does not know (a dev build, a .deb install),
/// reports false.
fn installed_by_pacman() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    std::process::Command::new("pacman")
        .arg("-Qqo")
        .arg(exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_format_is_a_known_value() {
        // Whatever platform CI runs this on, the value must be one the
        // frontend's `canAutoUpdateFormat` knows how to interpret.
        let fmt = detect_package_format();
        assert!(
            matches!(fmt.as_str(), "appimage" | "nsis" | "pacman" | "other"),
            "unexpected package format: {fmt}"
        );
    }

    /// Regression guard: Windows installs MUST report `nsis` so the
    /// in-app updater is offered. Previously this returned `other` on
    /// Windows, which silently downgraded every Windows user to a
    /// manual download-and-reinstall flow.
    #[test]
    fn windows_reports_nsis_without_probing_pacman() {
        let fmt = classify_package_format(true, false, || panic!("probed pacman"));
        assert_eq!(fmt, "nsis");
    }

    #[test]
    fn appimage_wins_over_pacman() {
        let fmt = classify_package_format(false, true, || panic!("probed pacman"));
        assert_eq!(fmt, "appimage");
    }

    #[test]
    fn pacman_owned_install_reports_pacman() {
        assert_eq!(classify_package_format(false, false, || true), "pacman");
    }

    #[test]
    fn unowned_install_reports_other() {
        assert_eq!(classify_package_format(false, false, || false), "other");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_binary_is_not_pacman_owned() {
        // The test executable lives in a cargo target dir, which no package
        // owns, so the probe must fail closed.
        assert!(!installed_by_pacman());
    }
}
