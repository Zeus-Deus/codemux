pub mod omarchy;
pub mod workspace_config;

use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{Emitter, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellAppearance {
    pub font_family: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

pub fn read_theme_colors() -> Result<ThemeColors, String> {
    omarchy::read_theme(&omarchy::theme_paths())
        .map(|theme| theme.colors)
        .ok_or_else(|| "No valid Omarchy palette found".into())
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

pub fn read_shell_appearance() -> Result<ShellAppearance, String> {
    let Some(home) = dirs::home_dir() else {
        return Err("Could not determine home directory for shell appearance lookup".into());
    };

    let candidates = [
        (
            home.join(".config/ghostty/config"),
            parse_ghostty_font_family as fn(&str) -> Option<String>,
        ),
        (
            home.join(".config/kitty/kitty.conf"),
            parse_kitty_font_family as fn(&str) -> Option<String>,
        ),
        (
            home.join(".config/alacritty/alacritty.toml"),
            parse_alacritty_font_family as fn(&str) -> Option<String>,
        ),
    ];

    for (path, parser) in candidates {
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };

        if let Some(font_family) = parser(&contents) {
            return Ok(ShellAppearance { font_family });
        }
    }

    Err("No terminal font configuration found in Ghostty, Kitty, or Alacritty configs".into())
}

pub fn read_shell_appearance_or_default() -> ShellAppearance {
    read_shell_appearance().unwrap_or_else(|error| {
        eprintln!("[codemux::shell_appearance] {error}. Falling back to monospace.");
        ShellAppearance {
            font_family: "monospace".into(),
        }
    })
}

fn parse_ghostty_font_family(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || !trimmed.starts_with("font-family") {
            return None;
        }

        trimmed
            .split_once('=')
            .map(|(_, value)| clean_font_value(value))
    })
}

fn parse_kitty_font_family(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || !trimmed.starts_with("font_family") {
            return None;
        }

        trimmed
            .split_once(' ')
            .map(|(_, value)| clean_font_value(value))
    })
}

fn parse_alacritty_font_family(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || !trimmed.contains("family") {
            return None;
        }

        trimmed
            .split_once('=')
            .map(|(_, value)| clean_font_value(value))
            .filter(|value| !value.is_empty())
    })
}

fn clean_font_value(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('}')
        .trim()
        .trim_matches('"')
        .trim()
        .to_string()
}

pub fn watch_theme_file<R: tauri::Runtime>(app_handle: tauri::AppHandle<R>) {
    let emitter = app_handle.clone();
    match omarchy::watch(omarchy::theme_paths(), move |theme| {
        let _ = emitter.emit("theme-changed", &theme.colors);
        let _ = emitter.emit("omarchy-theme-changed", &theme);
    }) {
        Ok(watcher) => {
            app_handle.manage(std::sync::Mutex::new(watcher));
        }
        Err(error) => log::warn!("Could not watch Omarchy theme: {error}"),
    }
}
