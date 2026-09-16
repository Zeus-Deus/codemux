use super::*;
use std::sync::mpsc;
use std::time::Duration;

const DARK: &str = "background = '#1a1b26'\nforeground = '#c0caf5'\naccent = '#7aa2f7'\nred = '#f7768e'\nmode = 'dark'\n";

#[test]
#[cfg(unix)]
fn rejects_special_and_oversized_palette_files() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().unwrap();
    let fifo = tmp.path().join("palette");
    let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(small_text(&fifo, 128).is_none());
    let oversized = tmp.path().join("large");
    fs::write(&oversized, "a".repeat(129)).unwrap();
    assert!(small_text(&oversized, 128).is_none());
}

#[test]
fn semantic_and_legacy_palettes_are_normalized() {
    let theme = parse_theme(DARK, "tokyo-night").unwrap();
    assert_eq!(theme.scheme, "dark");
    assert_eq!(theme.colors.color1, "#f7768e");
    assert_eq!(theme.colors.cursor, "#c0caf5");
    assert_eq!(theme.surfaces, OmarchySurfaces::default());
    let shaded = parse_theme(
        &format!("{DARK}dark_background = '#13141C'\nselection = '#292e42'\n"),
        "tokyo-night",
    )
    .unwrap();
    assert_eq!(shaded.surfaces.dark_background.as_deref(), Some("#13141c"));
    assert_eq!(shaded.surfaces.selection.as_deref(), Some("#292e42"));
    assert_eq!(shaded.surfaces.muted, None);
    let light = parse_theme(
        "background = '#eff1f5'\nforeground = '#4c4f69'\naccent = '#1e66f5'\nmode = 'light'",
        "latte",
    )
    .unwrap();
    assert_eq!(light.scheme, "light");
    let legacy = parse_theme(&toml::to_string(&ThemeColors::default()).unwrap(), "legacy").unwrap();
    assert_eq!(legacy.colors, ThemeColors::default());
    assert!(parse_theme("background = 'red'", "bad").is_err());
    assert!(parse_theme(&DARK.replace("#7aa2f7", "url(https://example.com)"), "bad").is_err());
}

#[test]
fn current_palette_wins_over_legacy_and_absence_is_not_a_default() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = vec![
        tmp.path().join("state/current/theme/colors.toml"),
        tmp.path().join("config/current/theme/colors.toml"),
    ];
    assert!(read_theme(&paths).is_none());
    for path in &paths {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, DARK).unwrap();
    }
    fs::write(
        paths[0]
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("theme.name"),
        "current",
    )
    .unwrap();
    assert_eq!(read_theme(&paths).unwrap().name, "Current");
    fs::write(&paths[0], "broken").unwrap();
    assert!(
        read_theme(&paths).is_none(),
        "never silently switch to stale legacy data"
    );
}

#[test]
fn watching_survives_repeated_directory_replacement_and_invalid_intermediate_data() {
    let tmp = tempfile::tempdir().unwrap();
    let current = tmp.path().join("current");
    let path = current.join("theme/colors.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, DARK).unwrap();
    let (tx, rx) = mpsc::channel();
    let watcher = watch(vec![path.clone()], move |theme| {
        let _ = tx.send(theme);
    })
    .unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    for (name, accent) in [("first", "#ff8800"), ("second", "#9988ff")] {
        let next = current.join("next-theme");
        fs::create_dir_all(&next).unwrap();
        fs::write(next.join("colors.toml"), DARK.replace("#7aa2f7", accent)).unwrap();
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
        fs::rename(next, path.parent().unwrap()).unwrap();
        fs::write(current.join("theme.name"), name).unwrap();
        let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(seen.colors.accent, accent);
    }
    fs::write(&path, "incomplete = true").unwrap();
    // A valid later write is the receipt; no fallback may appear before it.
    fs::write(&path, DARK.replace("#7aa2f7", "#11aabb")).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap()
            .colors
            .accent,
        "#11aabb"
    );
    drop(watcher);
}
