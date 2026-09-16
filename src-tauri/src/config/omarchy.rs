use super::ThemeColors;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OmarchyTheme {
    pub name: String,
    pub scheme: String,
    pub colors: ThemeColors,
    pub surfaces: OmarchySurfaces,
}

/// Omarchy's shell shades. Absent keys let the frontend derive a fallback.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct OmarchySurfaces {
    pub dark_background: Option<String>,
    pub selection: Option<String>,
    pub muted: Option<String>,
}

pub fn theme_paths() -> Vec<PathBuf> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let xdg = |key: &str, fallback: &str| {
        std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(fallback))
    };
    vec![
        xdg("XDG_STATE_HOME", ".local/state").join("omarchy/current/theme/colors.toml"),
        xdg("XDG_CONFIG_HOME", ".config").join("omarchy/current/theme/colors.toml"),
    ]
}

fn small_text(path: &Path, limit: u64) -> Option<String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Reject special files without blocking the app on a FIFO open.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(limit + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= limit).then_some(text)
}

pub fn read_theme(paths: &[PathBuf]) -> Option<OmarchyTheme> {
    // A present but temporarily incomplete current palette must not select an
    // unrelated legacy palette during Omarchy's directory replacement.
    let path = paths.iter().find(|path| {
        path.parent()
            .and_then(Path::parent)
            .is_some_and(Path::exists)
    })?;
    let contents = small_text(path, 32 * 1024)?;
    let name = small_text(&path.parent()?.parent()?.join("theme.name"), 256)
        .unwrap_or_else(|| "Desktop".into());
    parse_theme(&contents, &name).ok()
}

fn hex(value: &str) -> Option<String> {
    let raw = value.strip_prefix('#')?;
    if !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match raw.len() {
        6 => Some(value.to_ascii_lowercase()),
        3 => Some(
            format!("#{}", raw.chars().flat_map(|c| [c, c]).collect::<String>())
                .to_ascii_lowercase(),
        ),
        _ => None,
    }
}

pub fn parse_theme(contents: &str, name: &str) -> Result<OmarchyTheme, String> {
    let values: toml::Table =
        toml::from_str(contents).map_err(|e| format!("Invalid Omarchy palette: {e}"))?;
    let color = |keys: &[&str], fallback: Option<&str>| -> Result<String, String> {
        for key in keys {
            if let Some(value) = values.get(*key) {
                return value
                    .as_str()
                    .and_then(hex)
                    .ok_or_else(|| format!("Invalid Omarchy color: {key}"));
            }
        }
        fallback
            .map(str::to_owned)
            .ok_or_else(|| format!("Missing Omarchy color: {}", keys[0]))
    };
    // Cosmetic shades: an unusable value falls back instead of rejecting the palette.
    let optional = |key: &str| values.get(key).and_then(toml::Value::as_str).and_then(hex);
    let surfaces = OmarchySurfaces {
        dark_background: optional("dark_background"),
        selection: optional("selection"),
        muted: optional("muted"),
    };
    let background = color(&["background", "bg"], None)?;
    let foreground = color(&["foreground", "fg"], None)?;
    let accent = color(&["accent", "blue", "color4"], None)?;
    let scheme = match values
        .get("mode")
        .or_else(|| values.get("theme_type"))
        .and_then(toml::Value::as_str)
    {
        Some("light") => "light",
        Some("dark") => "dark",
        Some(_) => return Err("Invalid Omarchy theme mode".into()),
        None => {
            let luminance: f64 = [1, 3, 5]
                .into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(i, weight)| {
                    let c = u8::from_str_radix(&background[i..i + 2], 16).unwrap() as f64 / 255.0;
                    weight
                        * if c <= 0.04045 {
                            c / 12.92
                        } else {
                            ((c + 0.055) / 1.055).powf(2.4)
                        }
                })
                .sum();
            if luminance > 0.45 {
                "light"
            } else {
                "dark"
            }
        }
    };
    let red = color(&["color1", "red"], Some("#d65d5d"))?;
    let green = color(&["color2", "green"], Some("#70a858"))?;
    let yellow = color(&["color3", "yellow"], Some("#c49b38"))?;
    let blue = color(&["color4", "blue"], Some(&accent))?;
    let magenta = color(&["color5", "magenta"], Some("#ad71bf"))?;
    let cyan = color(&["color6", "cyan"], Some("#4c9a9d"))?;
    let colors = ThemeColors {
        cursor: color(
            &["cursor", "bright_foreground", "bright_fg"],
            Some(&foreground),
        )?,
        selection_foreground: color(&["selection_foreground"], Some(&foreground))?,
        selection_background: color(&["selection_background", "selection"], Some(&accent))?,
        color0: color(&["color0", "dark_background", "dark_bg"], Some(&background))?,
        color7: color(&["color7"], Some(&foreground))?,
        color8: color(&["color8", "muted"], Some(&foreground))?,
        color9: color(&["color9", "bright_red"], Some(&red))?,
        color10: color(&["color10", "bright_green"], Some(&green))?,
        color11: color(&["color11", "bright_yellow"], Some(&yellow))?,
        color12: color(&["color12", "bright_blue"], Some(&blue))?,
        color13: color(&["color13", "bright_magenta"], Some(&magenta))?,
        color14: color(&["color14", "bright_cyan"], Some(&cyan))?,
        color15: color(
            &["color15", "bright_foreground", "bright_fg"],
            Some(&foreground),
        )?,
        color1: red,
        color2: green,
        color3: yellow,
        color4: blue,
        color5: magenta,
        color6: cyan,
        background,
        foreground,
        accent,
    };
    let name = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>();
    let name = name
        .split(['-', '_'])
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(OmarchyTheme {
        name: if name.is_empty() {
            "Desktop".into()
        } else {
            name
        },
        scheme: scheme.into(),
        colors,
        surfaces,
    })
}

/** Watches stable current directories, never the replaceable colors inode.
 * Returning the handle binds lifetime to the app (and lets tests stop cleanly).
 */
pub fn watch(
    paths: Vec<PathBuf>,
    mut publish: impl FnMut(OmarchyTheme) + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |event: notify::Result<Event>| {
            if let Ok(event) = event {
                if !matches!(event.kind, EventKind::Access(_)) {
                    let _ = tx.send(());
                }
            }
        },
        Config::default(),
    )?;
    let mut watched = 0;
    for path in &paths {
        if let Some(current) = path.parent().and_then(Path::parent).filter(|p| p.is_dir()) {
            watcher.watch(current, RecursiveMode::Recursive)?;
            watched += 1;
        }
    }
    // Non-Omarchy installations don't keep an idle worker or watch the home.
    if watched == 0 {
        return Ok(watcher);
    }
    std::thread::spawn(move || {
        let mut previous = read_theme(&paths);
        if let Some(theme) = previous.clone() {
            publish(theme);
        }
        while rx.recv().is_ok() {
            while rx.recv_timeout(Duration::from_millis(80)).is_ok() {}
            if let Some(theme) = read_theme(&paths) {
                if previous.as_ref() != Some(&theme) {
                    previous = Some(theme.clone());
                    publish(theme);
                }
            }
        }
    });
    Ok(watcher)
}

#[cfg(test)]
#[path = "omarchy_tests.rs"]
mod tests;
