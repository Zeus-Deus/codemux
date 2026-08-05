//! Environment hygiene for child processes spawned by Codemux.
//!
//! Internal helpers strip desktop-session variables and add neutralizers so
//! background CLI work cannot accidentally open windows on the user's host
//! desktop. This is best-effort process hygiene, not a security boundary.

/// Environment variable keys that leak host-display access to a child process.
///
/// Clearing these stops well-behaved GTK/Qt/Xlib/Wayland/DBus clients from
/// finding the user's desktop. A determined child can still probe socket paths
/// directly, so callers must not treat this as sandboxing.
pub fn gui_env_keys() -> &'static [&'static str] {
    &[
        // X11
        "DISPLAY",
        "XAUTHORITY",
        // Wayland
        "WAYLAND_DISPLAY",
        "WAYLAND_SOCKET",
        // Session bus
        "DBUS_SESSION_BUS_ADDRESS",
        // Desktop-environment discovery
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "DESKTOP_SESSION",
        "GNOME_DESKTOP_SESSION_ID",
        // Compositor-specific reach-back
        "HYPRLAND_INSTANCE_SIGNATURE",
        "HYPRCURSOR_THEME",
        "HYPRCURSOR_SIZE",
        "AQ_DRM_DEVICES",
        "SWAYSOCK",
        // Toolkit backend selectors
        "GDK_BACKEND",
        "QT_QPA_PLATFORM",
        "QT_QPA_PLATFORMTHEME",
        "CLUTTER_BACKEND",
        "SDL_VIDEODRIVER",
        "NIXOS_OZONE_WL",
        "MOZ_ENABLE_WAYLAND",
        "MOZ_X11_EGL",
        // Portal / gio bridges
        "GTK_USE_PORTAL",
        // Startup notification
        "DESKTOP_STARTUP_ID",
    ]
}

/// Neutralizers applied after the desktop-session variables are stripped.
///
/// These close common escape hatches that unsetting display variables alone
/// does not cover: browser handoff, D-Bus autolaunch, desktop-specific
/// `xdg-open` routing, portals, gio VFS activation, and accessibility buses.
pub fn gui_env_overrides() -> &'static [(&'static str, &'static str)] {
    &[
        ("BROWSER", "true"),
        ("MOZ_NO_REMOTE", "1"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/dev/null"),
        ("XDG_CURRENT_DESKTOP", "X-Generic"),
        ("DE", "generic"),
        ("GTK_USE_PORTAL", "0"),
        ("GIO_USE_VFS", "local"),
        ("NO_AT_BRIDGE", "1"),
    ]
}

/// Apply GUI-environment hygiene to a `std::process::Command`.
///
/// Call this after setting explicit environment pairs so these overrides win.
pub fn sanitize_gui_env_std(cmd: &mut std::process::Command) {
    for key in gui_env_keys() {
        cmd.env_remove(key);
    }
    for (key, value) in gui_env_overrides() {
        cmd.env(key, value);
    }
}

/// Tokio equivalent of [`sanitize_gui_env_std`].
pub fn sanitize_gui_env_tokio(cmd: &mut tokio::process::Command) {
    for key in gui_env_keys() {
        cmd.env_remove(key);
    }
    for (key, value) in gui_env_overrides() {
        cmd.env(key, value);
    }
}

/// Variant of [`sanitize_gui_env_std`] that preserves the D-Bus session bus.
///
/// Trusted credential-handling CLIs may need the bus to reach the user's
/// keyring. Display, compositor, portal, and browser hygiene remains applied.
pub fn sanitize_gui_env_std_keep_dbus(cmd: &mut std::process::Command) {
    for key in gui_env_keys() {
        if *key != "DBUS_SESSION_BUS_ADDRESS" {
            cmd.env_remove(key);
        }
    }
    for (key, value) in gui_env_overrides() {
        if *key != "DBUS_SESSION_BUS_ADDRESS" {
            cmd.env(key, value);
        }
    }
}

/// Tokio equivalent of [`sanitize_gui_env_std_keep_dbus`].
pub fn sanitize_gui_env_tokio_keep_dbus(cmd: &mut tokio::process::Command) {
    for key in gui_env_keys() {
        if *key != "DBUS_SESSION_BUS_ADDRESS" {
            cmd.env_remove(key);
        }
    }
    for (key, value) in gui_env_overrides() {
        if *key != "DBUS_SESSION_BUS_ADDRESS" {
            cmd.env(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn gui_env_keys_cover_common_escape_hatches() {
        let keys = gui_env_keys();
        for expected in [
            "DISPLAY",
            "XAUTHORITY",
            "WAYLAND_DISPLAY",
            "WAYLAND_SOCKET",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_CURRENT_DESKTOP",
            "HYPRLAND_INSTANCE_SIGNATURE",
            "MOZ_ENABLE_WAYLAND",
            "NIXOS_OZONE_WL",
            "DESKTOP_STARTUP_ID",
        ] {
            assert!(keys.contains(&expected), "missing GUI env key {expected}");
        }
    }

    #[test]
    fn gui_env_overrides_neutralize_browser_and_dbus() {
        let map: std::collections::HashMap<_, _> =
            gui_env_overrides().iter().copied().collect();
        assert_eq!(map.get("BROWSER").copied(), Some("true"));
        assert_eq!(map.get("MOZ_NO_REMOTE").copied(), Some("1"));
        assert_eq!(
            map.get("DBUS_SESSION_BUS_ADDRESS").copied(),
            Some("unix:path=/dev/null")
        );
        assert_eq!(map.get("XDG_CURRENT_DESKTOP").copied(), Some("X-Generic"));
        assert_eq!(map.get("GTK_USE_PORTAL").copied(), Some("0"));
    }

    #[test]
    fn strict_sanitizer_strips_display_and_overrides_bus() {
        let mut cmd = std::process::Command::new("true");
        sanitize_gui_env_std(&mut cmd);
        let envs: Vec<(&OsStr, Option<&OsStr>)> = cmd.get_envs().collect();

        let display = envs
            .iter()
            .find(|(key, _)| *key == OsStr::new("DISPLAY"))
            .expect("DISPLAY mutation");
        assert_eq!(display.1, None);

        let bus = envs
            .iter()
            .find(|(key, _)| *key == OsStr::new("DBUS_SESSION_BUS_ADDRESS"))
            .expect("D-Bus override");
        assert_eq!(bus.1, Some(OsStr::new("unix:path=/dev/null")));
    }

    #[test]
    fn keep_dbus_sanitizer_preserves_bus_and_strips_display() {
        let mut cmd = std::process::Command::new("true");
        sanitize_gui_env_std_keep_dbus(&mut cmd);
        let envs: Vec<(&OsStr, Option<&OsStr>)> = cmd.get_envs().collect();

        assert!(!envs
            .iter()
            .any(|(key, _)| *key == OsStr::new("DBUS_SESSION_BUS_ADDRESS")));
        let display = envs
            .iter()
            .find(|(key, _)| *key == OsStr::new("DISPLAY"))
            .expect("DISPLAY mutation");
        assert_eq!(display.1, None);
        let browser = envs
            .iter()
            .find(|(key, _)| *key == OsStr::new("BROWSER"))
            .expect("BROWSER override");
        assert_eq!(browser.1, Some(OsStr::new("true")));
    }
}
