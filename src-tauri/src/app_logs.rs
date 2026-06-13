//! Locating and tailing the desktop app's persistent log file from a
//! CLI context (`codemux logs`, `codemux doctor`).
//!
//! The Tauri side writes the file via tauri-plugin-log into the app
//! log dir (see the logger registration in lib.rs). CLI invocations
//! have no `AppHandle`, so this mirrors Tauri's `app_log_dir`
//! platform resolution for our fixed bundle identifier instead.

use std::path::{Path, PathBuf};

/// Base name passed to tauri-plugin-log's `LogDir` target; the plugin
/// appends `.log`.
pub const LOG_FILE_STEM: &str = "codemux";

/// Must match `identifier` in `tauri.conf.json`.
const BUNDLE_IDENTIFIER: &str = "com.codemux.app";

/// Platform path of the app's log file. Mirrors Tauri's
/// `app_log_dir`:
/// - Linux:   `$XDG_DATA_HOME/{identifier}/logs`
/// - macOS:   `$HOME/Library/Logs/{identifier}`
/// - Windows: `{FOLDERID_LocalAppData}/{identifier}/logs`
pub fn app_log_file() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    let dir = dirs::data_dir()?.join(BUNDLE_IDENTIFIER).join("logs");
    #[cfg(target_os = "macos")]
    let dir = dirs::home_dir()?.join("Library/Logs").join(BUNDLE_IDENTIFIER);
    #[cfg(target_os = "windows")]
    let dir = dirs::data_local_dir()?.join(BUNDLE_IDENTIFIER).join("logs");

    Some(dir.join(format!("{LOG_FILE_STEM}.log")))
}

/// Last `count` lines of `path`. The log rotates at a small fixed
/// size (see lib.rs), so reading the whole file is fine.
pub fn tail_lines(path: &Path, count: usize) -> std::io::Result<Vec<String>> {
    let content = std::fs::read_to_string(path)?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(count);
    Ok(lines[start..].iter().map(|line| line.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tail_returns_last_n_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codemux.log");
        let mut file = std::fs::File::create(&path).expect("create");
        for i in 1..=5 {
            writeln!(file, "line {i}").expect("write");
        }

        let tail = tail_lines(&path, 2).expect("tail");
        assert_eq!(tail, vec!["line 4".to_string(), "line 5".to_string()]);

        // Asking for more lines than exist returns everything.
        let all = tail_lines(&path, 50).expect("tail");
        assert_eq!(all.len(), 5);
    }
}
