use notify::{Config, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use tauri::Emitter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeColors {
    pub accent: String,
    pub cursor: String,
    pub foreground: String,
    pub background: String,
    pub selection_foreground: String,
    pub selection_background: String,
    pub color0: String,
    pub color1: String,
    pub color2: String,
    pub color3: String,
    pub color4: String,
    pub color5: String,
    pub color6: String,
    pub color7: String,
    pub color8: String,
    pub color9: String,
    pub color10: String,
    pub color11: String,
    pub color12: String,
    pub color13: String,
    pub color14: String,
    pub color15: String,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            accent: "#7aa2f7".into(),
            cursor: "#c0caf5".into(),
            foreground: "#c0caf5".into(),
            background: "#1a1b26".into(),
            selection_foreground: "#c0caf5".into(),
            selection_background: "#283457".into(),
            color0: "#15161e".into(),
            color1: "#f7768e".into(),
            color2: "#9ece6a".into(),
            color3: "#e0af68".into(),
            color4: "#7aa2f7".into(),
            color5: "#bb9af7".into(),
            color6: "#7dcfff".into(),
            color7: "#a9b1d6".into(),
            color8: "#414868".into(),
            color9: "#f7768e".into(),
            color10: "#9ece6a".into(),
            color11: "#e0af68".into(),
            color12: "#7aa2f7".into(),
            color13: "#bb9af7".into(),
            color14: "#7dcfff".into(),
            color15: "#c0caf5".into(),
        }
    }
}

pub fn get_omarchy_theme_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config/omarchy/current/theme/colors.toml"))
}

pub fn read_theme_colors() -> Result<ThemeColors, String> {
    let path = get_omarchy_theme_path()
        .ok_or_else(|| "Could not determine home directory for Omarchy theme lookup".to_string())?;
    let contents =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read theme file: {}", e))?;

    let theme: ThemeColors =
        toml::from_str(&contents).map_err(|e| format!("Failed to parse theme file: {}", e))?;

    Ok(theme)
}

pub fn read_theme_colors_or_default() -> ThemeColors {
    match read_theme_colors() {
        Ok(theme) => theme,
        Err(error) => {
            eprintln!("[codemux::theme] {error}. Falling back to default theme.");
            ThemeColors::default()
        }
    }
}

pub fn watch_theme_file(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        let (tx, rx) = channel();
        let mut watcher = match notify::RecommendedWatcher::new(tx, Config::default()) {
            Ok(watcher) => watcher,
            Err(error) => {
                eprintln!("[codemux::theme] Failed to create watcher: {error}");
                return;
            }
        };

        let Some(theme_path) = get_omarchy_theme_path() else {
            eprintln!("[codemux::theme] Home directory unavailable, skipping theme watching");
            return;
        };

        let watch_target = if theme_path.exists() {
            theme_path.clone()
        } else {
            theme_path
                .parent()
                .map(PathBuf::from)
                .unwrap_or(theme_path.clone())
        };

        if let Err(error) = watcher.watch(&watch_target, RecursiveMode::NonRecursive) {
            eprintln!(
                "[codemux::theme] Failed to watch theme path {}: {error}",
                watch_target.display()
            );
            return;
        }

        for res in rx {
            match res {
                Ok(_event) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    let theme = read_theme_colors_or_default();
                    if let Err(error) = app_handle.emit("theme-changed", theme) {
                        eprintln!("[codemux::theme] Failed to emit theme change event: {error}");
                    }
                }
                Err(error) => eprintln!("[codemux::theme] Watch error: {error:?}"),
            }
        }
    });
}
