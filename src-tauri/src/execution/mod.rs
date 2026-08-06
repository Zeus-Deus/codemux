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

// ── AppImage environment hygiene ────────────────────────────────────────
//
// When Codemux runs from an AppImage, `AppRun` rewrites a batch of loader and
// toolkit variables to point into the mounted AppDir (`/tmp/.mount_codemuXXXX`)
// so the bundled binary finds its bundled libraries. Those values are correct
// for *us* and wrong for everyone else: a child process that inherits
// `LD_LIBRARY_PATH` resolves system binaries against our bundled `libssl`,
// `libpcre2`, etc. On a host whose libraries are newer than the bundle, that is
// a hard failure before `main` — e.g. `cargo` dying with
// "version `OPENSSL_3.5.0' not found" without ever reading `Cargo.toml`.
//
// The AppDir libraries must stay visible to the Codemux binary itself (and to
// the pty-daemon, which is the same binary re-executed), so this hygiene is
// applied at the *leaf* child spawns only — never at the daemon supervisor.

/// Variables whose value is a `:`-separated search path.
///
/// AppRun *prepends* AppDir entries to these, so the user's original entries
/// are still present and must be preserved. Only AppDir entries are dropped.
const APPIMAGE_PATH_LIST_KEYS: &[&str] = &[
    "PATH",
    "LD_LIBRARY_PATH",
    "PYTHONPATH",
    "PERLLIB",
    "PERL5LIB",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "GTK_PATH",
    "GSETTINGS_SCHEMA_DIR",
    "GIO_EXTRA_MODULES",
    "QT_PLUGIN_PATH",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GST_PLUGIN_PATH",
    "LD_PRELOAD",
];

/// Variables whose value is a single AppDir-rooted path or prefix.
///
/// AppRun *overwrites* these outright, so a value pointing into the AppDir has
/// no user-supplied part worth keeping. Removed only when the value actually
/// points into the AppDir, so a user's own `PYTHONHOME` survives.
const APPIMAGE_SCALAR_KEYS: &[&str] = &[
    "PYTHONHOME",
    "GTK_EXE_PREFIX",
    "GTK_DATA_PREFIX",
    "GTK_IM_MODULE_FILE",
    "GDK_PIXBUF_MODULE_FILE",
    "GDK_PIXBUF_MODULEDIR",
    "GCONV_PATH",
    "APPDIR",
];

/// Bookkeeping variables AppRun exports that describe the *launch*, not a path.
///
/// `APPIMAGE` points at the `.AppImage` file (outside the AppDir), `OWD` at the
/// launch directory, `ARGV0` at the invoked name. None are AppDir-rooted, so
/// they need an unconditional rule: if we are running from an AppImage at all,
/// a child has no business seeing them and tools that probe `APPIMAGE` to
/// detect a bundle should conclude "not bundled".
const APPIMAGE_MARKER_KEYS: &[&str] = &["APPIMAGE", "APPIMAGE_UUID", "ARGV0", "OWD"];

/// The AppDir mount root, when running from an AppImage.
fn appdir_root() -> Option<String> {
    std::env::var("APPDIR")
        .ok()
        .map(|v| v.trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}

/// True when `value` names a path inside `appdir`.
fn is_under_appdir(value: &str, appdir: &str) -> bool {
    // AppRun emits both `/mount/usr/lib` and doubled-slash forms like
    // `/mount//usr/lib`, so compare on the prefix rather than exact segments.
    value == appdir || value.starts_with(&format!("{appdir}/"))
}

/// Compute the child-environment fixups needed to undo AppRun's rewrites.
///
/// Returns `(key, Some(value))` to overwrite and `(key, None)` to remove.
/// Empty when not running from an AppImage, so every call site is a no-op on
/// normal installs.
pub fn appimage_env_fixups() -> Vec<(String, Option<String>)> {
    let Some(appdir) = appdir_root() else {
        return Vec::new();
    };
    compute_appimage_fixups(&appdir, |key| std::env::var(key).ok())
}

/// Pure core of [`appimage_env_fixups`], parameterized on the environment.
///
/// Split out so the rules are unit-testable without mutating the process
/// environment, which is both racy across parallel tests and `unsafe`.
fn compute_appimage_fixups(
    appdir: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<(String, Option<String>)> {
    let mut fixups = Vec::new();

    for key in APPIMAGE_PATH_LIST_KEYS {
        let Some(value) = lookup(key) else { continue };
        let original: Vec<&str> = value.split(':').filter(|e| !e.is_empty()).collect();
        let kept: Vec<&str> = original
            .iter()
            .copied()
            .filter(|entry| !is_under_appdir(entry, appdir))
            .collect();
        if kept.len() == original.len() {
            continue; // nothing of ours in here; leave it untouched
        }
        if kept.is_empty() {
            fixups.push(((*key).to_string(), None));
        } else {
            fixups.push(((*key).to_string(), Some(kept.join(":"))));
        }
    }

    for key in APPIMAGE_SCALAR_KEYS {
        let Some(value) = lookup(key) else { continue };
        if is_under_appdir(&value, appdir) {
            fixups.push(((*key).to_string(), None));
        }
    }

    for key in APPIMAGE_MARKER_KEYS {
        if lookup(key).is_some() {
            fixups.push(((*key).to_string(), None));
        }
    }

    fixups
}

/// `PATH` as a child should see it: AppDir `bin` entries removed.
///
/// Call sites that build a child `PATH` (shim injection) must start from this
/// rather than the raw process `PATH`, otherwise they re-introduce the AppDir
/// entries the sanitizers just dropped.
pub fn sanitized_child_path() -> String {
    let raw = std::env::var("PATH").unwrap_or_default();
    match appdir_root() {
        Some(appdir) => strip_appdir_from_path_list(&raw, &appdir),
        None => raw,
    }
}

/// Pure core of [`sanitized_child_path`].
fn strip_appdir_from_path_list(raw: &str, appdir: &str) -> String {
    raw.split(':')
        .filter(|entry| !entry.is_empty() && !is_under_appdir(entry, appdir))
        .collect::<Vec<_>>()
        .join(":")
}

/// Apply AppImage hygiene to a PTY [`portable_pty::CommandBuilder`].
pub fn sanitize_appimage_env_pty(cmd: &mut portable_pty::CommandBuilder) {
    for (key, value) in appimage_env_fixups() {
        match value {
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        }
    }
}

/// Apply AppImage hygiene to a `std::process::Command`.
pub fn sanitize_appimage_env_std(cmd: &mut std::process::Command) {
    for (key, value) in appimage_env_fixups() {
        match value {
            Some(value) => {
                cmd.env(key, value);
            }
            None => {
                cmd.env_remove(key);
            }
        }
    }
}

/// Tokio equivalent of [`sanitize_appimage_env_std`].
pub fn sanitize_appimage_env_tokio(cmd: &mut tokio::process::Command) {
    for (key, value) in appimage_env_fixups() {
        match value {
            Some(value) => {
                cmd.env(key, value);
            }
            None => {
                cmd.env_remove(key);
            }
        }
    }
}

/// Construct a `std::process::Command` for a **host** binary, pre-sanitized.
///
/// Prefer this over `std::process::Command::new` anywhere Codemux shells out to
/// a program that belongs to the user's system (`git`, `gh`, `sh`, their
/// editor) rather than to our own bundle. Under an AppImage, a raw
/// `Command::new` inherits AppRun's `LD_LIBRARY_PATH` and the child links
/// against our bundled libraries — `git` is a live example, failing with
/// "libpcre2-8.so.0: no version information available".
///
/// Constructor form (rather than a `&mut` sanitizer) so it drops into builder
/// chains like `host_command("git").args(..).output()`. Explicit `.env()` calls
/// made afterwards still win, which is intended.
///
/// This is orthogonal to the GUI sanitizers: it touches only loader/toolkit
/// path variables, never `DISPLAY`/`WAYLAND_DISPLAY`, so it is safe even at
/// sites that must keep the desktop environment (e.g. launching an editor).
pub fn host_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    sanitize_appimage_env_std(&mut cmd);
    cmd
}

/// Tokio equivalent of [`host_command`].
pub fn host_command_tokio(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    sanitize_appimage_env_tokio(&mut cmd);
    cmd
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

    // ── AppImage hygiene ────────────────────────────────────────────────

    const APPDIR: &str = "/tmp/.mount_codemuAfDPIN";

    /// Build a lookup over a fixed key/value table.
    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    fn fixup<'a>(
        fixups: &'a [(String, Option<String>)],
        key: &str,
    ) -> Option<&'a Option<String>> {
        fixups.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    #[test]
    fn appimage_fixups_drop_apprun_only_library_path() {
        // Verbatim from an AppImage-launched shell: every entry is AppDir, and
        // AppRun's trailing `:` leaves an empty final field.
        let ld = "/tmp/.mount_codemuAfDPIN/usr/lib/:\
                  /tmp/.mount_codemuAfDPIN/usr/lib/x86_64-linux-gnu/:\
                  /tmp/.mount_codemuAfDPIN/lib/:";
        let fixups = compute_appimage_fixups(APPDIR, env_of(&[("LD_LIBRARY_PATH", ld)]));
        assert_eq!(
            fixup(&fixups, "LD_LIBRARY_PATH"),
            Some(&None),
            "an all-AppDir LD_LIBRARY_PATH must be removed outright, not emptied"
        );
    }

    #[test]
    fn appimage_fixups_preserve_user_entries_in_mixed_path() {
        // AppRun *prepends* to PATH; the user's own entries must survive.
        let path = "/tmp/.mount_codemuAfDPIN/usr/bin/:\
                    /tmp/.mount_codemuAfDPIN/bin/:\
                    /home/zeus/.local/bin:/usr/bin";
        let fixups = compute_appimage_fixups(APPDIR, env_of(&[("PATH", path)]));
        assert_eq!(
            fixup(&fixups, "PATH"),
            Some(&Some("/home/zeus/.local/bin:/usr/bin".to_string()))
        );
    }

    #[test]
    fn appimage_fixups_ignore_env_with_no_appdir_entries() {
        // A user-set LD_LIBRARY_PATH with nothing of ours in it must be left
        // completely alone — no fixup emitted at all.
        let fixups = compute_appimage_fixups(
            APPDIR,
            env_of(&[
                ("LD_LIBRARY_PATH", "/opt/mylibs:/usr/local/lib"),
                ("PYTHONHOME", "/home/zeus/venv"),
            ]),
        );
        assert!(
            fixups.is_empty(),
            "user-owned values must not be touched, got {fixups:?}"
        );
    }

    #[test]
    fn appimage_fixups_remove_appdir_rooted_scalars() {
        let fixups = compute_appimage_fixups(
            APPDIR,
            env_of(&[
                ("PYTHONHOME", "/tmp/.mount_codemuAfDPIN/usr/"),
                // AppRun emits doubled slashes for some keys.
                (
                    "GDK_PIXBUF_MODULE_FILE",
                    "/tmp/.mount_codemuAfDPIN//usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders.cache",
                ),
            ]),
        );
        assert_eq!(fixup(&fixups, "PYTHONHOME"), Some(&None));
        assert_eq!(
            fixup(&fixups, "GDK_PIXBUF_MODULE_FILE"),
            Some(&None),
            "doubled-slash AppRun paths must still be recognized as AppDir-rooted"
        );
    }

    #[test]
    fn appimage_fixups_remove_launch_markers_outside_appdir() {
        // APPIMAGE points at the .AppImage file, which is NOT under APPDIR, so
        // it needs the unconditional rule rather than the prefix test.
        let fixups = compute_appimage_fixups(
            APPDIR,
            env_of(&[("APPIMAGE", "/home/zeus/Downloads/codemux_0.17.0_amd64.AppImage")]),
        );
        assert_eq!(fixup(&fixups, "APPIMAGE"), Some(&None));
    }

    #[test]
    fn appimage_fixups_are_empty_without_appdir() {
        // The whole feature is gated on APPDIR: on a distro package (AUR,
        // .deb, .rpm) there is no AppDir and every call site must be a no-op.
        // Only assert when this test process genuinely has no APPDIR — the
        // suite may itself be run from an AppImage-launched terminal.
        if appdir_root().is_none() {
            assert!(
                appimage_env_fixups().is_empty(),
                "with no APPDIR set, fixups must be empty"
            );
            assert_eq!(
                sanitized_child_path(),
                std::env::var("PATH").unwrap_or_default(),
                "with no APPDIR set, PATH must pass through untouched"
            );
        }
    }

    #[test]
    fn sanitized_path_is_identity_when_no_appdir_entries() {
        let raw = "/home/zeus/.local/bin:/usr/bin:/bin";
        assert_eq!(strip_appdir_from_path_list(raw, APPDIR), raw);
    }

    #[test]
    fn sanitized_path_drops_appdir_bins() {
        let raw = "/tmp/.mount_codemuAfDPIN/usr/bin/:/usr/bin:/tmp/.mount_codemuAfDPIN/bin/:/bin";
        assert_eq!(strip_appdir_from_path_list(raw, APPDIR), "/usr/bin:/bin");
    }

    #[test]
    fn appdir_prefix_match_is_not_a_bare_substring_match() {
        // A sibling mount that merely shares a textual prefix must not be
        // swallowed, or we would strip an unrelated directory.
        assert!(!is_under_appdir("/tmp/.mount_codemuAfDPIN-other/lib", APPDIR));
        assert!(is_under_appdir("/tmp/.mount_codemuAfDPIN/usr/lib", APPDIR));
        assert!(is_under_appdir(APPDIR, APPDIR));
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
